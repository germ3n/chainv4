//! A worker: mines for a node over its mining port, on this machine's CPU threads or GPUs, so a
//! rig with no node of its own adds its hashrate to one that has. It takes whatever work the node
//! hands it, in the range of nonces the node gives it, sends back each nonce it finds and, every
//! few seconds, its devices' hashrates; it reconnects when the connection goes.

use {
    super::pool::{PING, QUIET},
    super::protocol::{self, Receiver, Sender},
    crate::consensus::{DeviceReport, Rig},
    crate::structs::block::Seal,
    crate::structs::hash::{Hash, HashType},
    r#macro::document,
    serde_json::json,
    std::net::{TcpStream, ToSocketAddrs},
    std::sync::atomic::{AtomicBool, Ordering},
    std::sync::mpsc,
    std::sync::{Arc, Mutex, MutexGuard},
    std::thread::JoinHandle,
    std::time::{Duration, Instant},
};

/// How often a worker sends its hashrates.
const RATE_EVERY: Duration = Duration::from_secs(5);
/// How long a worker waits before connecting again.
const RECONNECT: Duration = Duration::from_secs(5);
/// How long connecting may take.
const CONNECT: Duration = Duration::from_secs(10);

#[document(
    summary = "What a worker mines on.",
    category = "Mining",
    see_also = "WorkerConfig",
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Devices {
    #[document(summary = "This many CPU threads.")]
    Cpu(usize),
    #[document(summary = "The GPUs, by their place in chainv4_gpu::list, or every one for None. In a build with the gpu feature.")]
    Gpu(Option<Vec<usize>>),
}

#[document(
    summary = "How a worker reaches its node, and what it mines on.",
    category = "Mining",
    see_also = "Worker, Devices",
)]
#[derive(Clone)]
pub struct WorkerConfig {
    #[document(summary = "The node's mining port, as host:port.")]
    pub node: String,
    #[document(summary = "The secret the node was given, which proves this worker may mine for it.")]
    pub token: Vec<u8>,
    #[document(summary = "What the worker calls itself, in the node's report and on the explorer.")]
    pub name: String,
    #[document(summary = "The node's protocol name, as its --protocol gives it.")]
    pub protocol: String,
    #[document(summary = "What it mines on.")]
    pub devices: Devices,
    #[document(summary = "A SOCKS5 proxy to reach the node through, socks5h://host:port, such as Tor's for a node's onion; None to connect directly.")]
    pub proxy: Option<String>,
}

impl std::fmt::Debug for WorkerConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WorkerConfig")
            .field("node", &self.node)
            .field("name", &self.name)
            .field("protocol", &self.protocol)
            .field("devices", &self.devices)
            .field("proxy", &self.proxy)
            .finish_non_exhaustive()
    }
}

/// The devices, made for the work hash of the first work, and again should another one come.
type Mounted = Mutex<Option<(HashType, Rig)>>;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[document(
    summary = "A worker running on a thread of its own: it connects to the node, mines what it is handed and reconnects whenever the connection goes, until it is dropped.",
    category = "Mining",
    see_also = "WorkerConfig, Pool",
)]
pub struct Worker {
    stop: Arc<AtomicBool>,
    /// The connection now, so stopping can close it.
    stream: Arc<Mutex<Option<TcpStream>>>,
    rig: Arc<Mounted>,
    thread: Option<JoinHandle<()>>,
}

impl Worker {
    #[document(
        parent = "Worker",
        summary = "Starts the worker. Its devices are made once the first work says which hash to mine.",
        returns = "The running worker, or why its thread did not start.",
    )]
    pub fn start(config: WorkerConfig) -> Result<Worker, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let stream = Arc::new(Mutex::new(None));
        let rig: Arc<Mounted> = Arc::new(Mutex::new(None));
        let thread = {
            let (stop, stream, rig) = (stop.clone(), stream.clone(), rig.clone());

            std::thread::Builder::new()
                .name("worker".to_string())
                .spawn(move || run(&config, &stop, &stream, &rig))
                .map_err(|err| format!("the worker's thread did not start: {err}"))?
        };

        Ok(Worker {
            stop,
            stream,
            rig,
            thread: Some(thread),
        })
    }

    #[document(parent = "Worker", summary = "Its devices and their hashrates; none before the first work.")]
    pub fn devices(&self) -> Vec<DeviceReport> {
        lock(&self.rig).as_ref().map(|(_, rig)| rig.devices()).unwrap_or_default()
    }

    #[document(parent = "Worker", summary = "Runs until the process ends: the worker only stops when dropped.")]
    pub fn wait(mut self) {
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);

        if let Some(stream) = lock(&self.stream).as_ref() {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }

        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Connects, mines, and connects again, until stopped. The devices outlive each connection, so a
/// GPU is opened once.
fn run(config: &WorkerConfig, stop: &AtomicBool, current: &Mutex<Option<TcpStream>>, rig: &Mounted) {
    let (found, mut seals) = mpsc::channel();

    while !stop.load(Ordering::SeqCst) {
        let result = connect(config).and_then(|stream| {
            *lock(current) = Some(stream.try_clone().map_err(|err| err.to_string())?);

            // Stopped while connecting: the stream was not there to close.
            if stop.load(Ordering::SeqCst) {
                return Ok(());
            }

            session(config, stream, rig, &found, &mut seals, stop)
        });
        *lock(current) = None;

        if let Some((_, rig)) = lock(rig).as_ref() {
            rig.rest();
        }

        if stop.load(Ordering::SeqCst) {
            break;
        }

        match result {
            Ok(()) => log::info!("[worker] the node closed the connection; connecting again in {}s", RECONNECT.as_secs()),
            Err(why) => log::warn!("[worker] {}: {why}; connecting again in {}s", config.node, RECONNECT.as_secs()),
        }

        let since = Instant::now();

        while since.elapsed() < RECONNECT && !stop.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

/// A connection to the node, past its handshake's TCP part: directly, or through the proxy.
fn connect(config: &WorkerConfig) -> Result<TcpStream, String> {
    if let Some(proxy) = &config.proxy {
        return through(proxy, &config.node);
    }

    let addresses = config.node.to_socket_addrs().map_err(|err| format!("no address for it: {err}"))?;
    let mut last = "no address for it".to_string();

    for address in addresses {
        match TcpStream::connect_timeout(&address, CONNECT) {
            Ok(stream) => return Ok(stream),
            Err(err) => last = format!("{address}: {err}"),
        }
    }

    Err(last)
}

/// How long a proxy may take to reach the node: Tor builds a circuit to an onion first.
const THROUGH_PROXY: Duration = Duration::from_secs(90);

/// A connection to the node through a SOCKS5 proxy, which resolves its name: a Tor onion's only
/// resolves there. No authentication, as Tor's and Orbot's take none.
fn through(proxy: &str, node: &str) -> Result<TcpStream, String> {
    use std::io::{Read, Write};

    let at = proxy.strip_prefix("socks5h://").or_else(|| proxy.strip_prefix("socks5://")).ok_or_else(|| format!("the proxy {proxy} is not socks5h://host:port"))?;
    let (host, port) = node
        .rsplit_once(':')
        .and_then(|(host, port)| Some((host.trim_start_matches('[').trim_end_matches(']'), port.parse::<u16>().ok()?)))
        .ok_or_else(|| format!("the node {node} is not host:port"))?;
    let name = u8::try_from(host.len()).map_err(|_| "the node's name is too long for SOCKS5".to_string())?;
    let address = at.to_socket_addrs().map_err(|err| format!("the proxy {at}: {err}"))?.next().ok_or_else(|| format!("no address for the proxy {at}"))?;
    let mut stream = TcpStream::connect_timeout(&address, CONNECT).map_err(|err| format!("the proxy {at}: {err}"))?;
    let failed = |err: std::io::Error| format!("the proxy {at}: {err}");
    stream.set_read_timeout(Some(THROUGH_PROXY)).map_err(failed)?;

    // No authentication, then CONNECT to the name and port.
    stream.write_all(&[5, 1, 0]).map_err(failed)?;
    let mut chosen = [0; 2];
    stream.read_exact(&mut chosen).map_err(failed)?;

    if chosen != [5, 0] {
        return Err(format!("the proxy {at} wants authentication, or is no SOCKS5 proxy"));
    }

    let mut request = vec![5, 1, 0, 3, name];
    request.extend(host.as_bytes());
    request.extend(port.to_be_bytes());
    stream.write_all(&request).map_err(failed)?;
    let mut reply = [0; 4];
    stream.read_exact(&mut reply).map_err(failed)?;

    if reply[1] != 0 {
        let why = match reply[1] {
            1 => "it failed",
            2 => "its rules refused it",
            3 => "the network is unreachable",
            4 => "the host is unreachable",
            5 => "the node refused it",
            6 => "it took too long",
            _ => "it would not",
        };

        return Err(format!("the proxy {at} could not reach {node}: {why}"));
    }

    // The address it bound, which says nothing worth keeping.
    let skip = match reply[3] {
        1 => 4,
        4 => 16,
        3 => {
            let mut length = [0; 1];
            stream.read_exact(&mut length).map_err(failed)?;
            usize::from(length[0])
        }
        _ => return Err(format!("the proxy {at} answered in no SOCKS5 it speaks")),
    };
    let mut bound = vec![0; skip + 2];
    stream.read_exact(&mut bound).map_err(failed)?;
    stream.set_read_timeout(None).map_err(failed)?;

    Ok(stream)
}

/// One connection: the handshake, then work in on this thread and nonces and rates out on another,
/// until either side goes.
fn session(
    config: &WorkerConfig,
    stream: TcpStream,
    rig: &Mounted,
    found: &mpsc::Sender<(Hash, u64)>,
    seals: &mut mpsc::Receiver<(Hash, u64)>,
    stop: &AtomicBool,
) -> Result<(), String> {
    stream.set_nodelay(true).map_err(|err| err.to_string())?;
    stream.set_read_timeout(Some(QUIET)).map_err(|err| err.to_string())?;
    let (sender, mut receiver) = protocol::connect(stream.try_clone().map_err(|err| err.to_string())?, &config.token, &config.protocol, &config.name)?;
    log::info!("[worker] mining for {} as {}", config.node, config.name);

    // Seals found for a job before this connection belong to no job it knows.
    while seals.try_recv().is_ok() {}

    let job: Mutex<Option<(u64, Hash)>> = Mutex::new(None);
    let closed = AtomicBool::new(false);

    std::thread::scope(|scope| {
        let (job, closed) = (&job, &closed);
        // The receiver moves to the sending thread, borrowed mutably so it is that thread's alone.
        let sending = scope.spawn(move || send(sender, rig, &*seals, job, closed, stop));
        let result = take_work(config, &mut receiver, rig, found, job);

        closed.store(true, Ordering::SeqCst);
        let _ = stream.shutdown(std::net::Shutdown::Both);
        let sent = sending.join().unwrap_or(Ok(()));

        result.and(sent)
    })
}

/// Sends each nonce the devices find for the job now, and the hashrates every few seconds.
fn send(
    mut sender: Sender,
    rig: &Mounted,
    seals: &mpsc::Receiver<(Hash, u64)>,
    job: &Mutex<Option<(u64, Hash)>>,
    closed: &AtomicBool,
    stop: &AtomicBool,
) -> Result<(), String> {
    let mut rated = Instant::now();

    while !closed.load(Ordering::SeqCst) && !stop.load(Ordering::SeqCst) {
        match seals.recv_timeout(Duration::from_millis(200)) {
            Ok((id, nonce)) => {
                let number = lock(job).filter(|(_, current)| *current == id).map(|(number, _)| number);

                if let Some(number) = number {
                    log::info!("[worker] found a nonce for job {number}");
                    sender.send(&json!({ "type": "found", "job": number, "nonce": nonce }))?;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
        }

        if rated.elapsed() >= RATE_EVERY {
            rated = Instant::now();
            let devices = lock(rig).as_ref().map(|(_, rig)| rig.devices()).unwrap_or_default();
            let devices: Vec<_> = devices.iter().map(|device| json!({ "name": device.name, "rate": device.rate })).collect();
            sender.send(&json!({ "type": "rate", "devices": devices }))?;
        }
    }

    Ok(())
}

/// Takes the node's messages: work to mine, verdicts on the nonces sent, and pings.
fn take_work(
    config: &WorkerConfig,
    receiver: &mut Receiver,
    rig: &Mounted,
    found: &mpsc::Sender<(Hash, u64)>,
    job: &Mutex<Option<(u64, Hash)>>,
) -> Result<(), String> {
    loop {
        let message = match receiver.receive() {
            Ok(message) => message,
            Err(why) if why == "the connection closed" => return Ok(()),
            Err(why) => return Err(why),
        };

        match message["type"].as_str() {
            Some("work") => {
                let work = Work::read(&message).ok_or("work that does not read")?;
                let mut mounted = lock(rig);

                if mounted.as_ref().is_none_or(|(hash, _)| *hash != work.hash) {
                    // The old devices stop before the new ones open, so a GPU is never opened twice.
                    *mounted = None;
                    *mounted = Some((work.hash, mount(&config.devices, work.hash, found.clone())?));
                }

                *lock(job) = Some((work.job, work.id));
                mounted.as_ref().expect("mounted above").1.work(work.id, work.difficulty, work.start);
                log::debug!("[worker] job {} at difficulty {}, nonces from {}", work.job, work.difficulty, work.start);
            }
            Some("verdict") if message["took"] == true => log::info!("[worker] the node took the nonce for job {}", message["job"]),
            Some("verdict") => log::info!("[worker] the node refused the nonce for job {}: {}", message["job"], message["why"].as_str().unwrap_or("")),
            // Pings, and a newer node's messages this worker does not know.
            _ => {}
        }
    }
}

/// A work message, read.
struct Work {
    job: u64,
    id: Hash,
    difficulty: u128,
    hash: HashType,
    start: u64,
}

impl Work {
    fn read(message: &serde_json::Value) -> Option<Work> {
        let chain: HashType = message["chain"].as_str()?.parse().ok()?;
        let id = Hash::from_bytes(chain, &crate::structs::hex::from_hex(message["id"].as_str()?)?)?;

        Some(Work {
            job: message["job"].as_u64()?,
            id,
            difficulty: message["difficulty"].as_str()?.parse().ok()?,
            hash: message["hash"].as_str()?.parse().ok()?,
            start: message["start"].as_u64()?,
        })
    }
}

/// Opens the devices for a work hash, each nonce they find going to found.
fn mount(devices: &Devices, hash: HashType, found: mpsc::Sender<(Hash, u64)>) -> Result<Rig, String> {
    let found = move |id: Hash, seal: Seal| {
        if let Seal::Work { nonce } = seal {
            let _ = found.send((id, nonce));
        }
    };

    match devices {
        Devices::Cpu(threads) => {
            log::info!("[worker] mining {} on {} CPU thread{}", hash.name(), threads.max(&1), if *threads > 1 { "s" } else { "" });

            Ok(Rig::cpu(hash, *threads, found))
        }
        #[cfg(feature = "gpu")]
        Devices::Gpu(which) => Rig::gpu(hash, which.as_deref(), found),
        #[cfg(not(feature = "gpu"))]
        Devices::Gpu(_) => {
            let _ = found;

            Err("--gpu needs a build with the gpu feature: cargo build --features gpu".to_string())
        }
    }
}

// The node pings more often than a worker takes it for gone.
const _: () = assert!(PING.as_secs() * 2 < QUIET.as_secs());

#[cfg(test)]
mod tests {
    use {super::*, std::io::{Read, Write}, std::net::TcpListener};

    /// A SOCKS5 proxy for one connection, as Tor's answers: it checks the greeting and that it is
    /// asked for the name, not an address, then answers with reply, and echoes what comes.
    fn proxy(reply: u8) -> (String, std::thread::JoinHandle<Vec<u8>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let at = format!("socks5h://{}", listener.local_addr().unwrap());

        let served = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut greeting = [0; 3];
            stream.read_exact(&mut greeting).unwrap();
            assert_eq!(greeting, [5, 1, 0], "no authentication offered");
            stream.write_all(&[5, 0]).unwrap();
            let mut head = [0; 5];
            stream.read_exact(&mut head).unwrap();
            assert_eq!(head[..4], [5, 1, 0, 3], "CONNECT to a name");
            let mut asked = vec![0; usize::from(head[4]) + 2];
            stream.read_exact(&mut asked).unwrap();
            stream.write_all(&[5, reply, 0, 1, 0, 0, 0, 0, 0, 0]).unwrap();

            if reply == 0 {
                let mut echoed = [0; 4];
                stream.read_exact(&mut echoed).unwrap();
                stream.write_all(&echoed).unwrap();
            }

            asked
        });

        (at, served)
    }

    #[test]
    fn a_worker_reaches_an_onion_through_a_socks5_proxy_which_resolves_its_name() {
        let onion = "abcdefghijklmnopqrstuvwxyz234567abcdefghijklmnopqrstuvwx.onion";
        let (at, served) = proxy(0);
        let mut stream = through(&at, &format!("{onion}:4002")).unwrap();
        stream.write_all(b"ping").unwrap();
        let mut back = [0; 4];
        stream.read_exact(&mut back).unwrap();

        assert_eq!(&back, b"ping");
        assert_eq!(served.join().unwrap(), [onion.as_bytes(), &4002u16.to_be_bytes()].concat(), "the name and port, as given");

        // A proxy that cannot reach it says why.
        let (at, served) = proxy(4);
        let refused = through(&at, &format!("{onion}:4002")).unwrap_err();
        served.join().unwrap();

        assert!(refused.contains("the host is unreachable"), "{refused}");
        assert!(through("http://127.0.0.1:1", "a:1").unwrap_err().contains("socks5h://"));
    }
}
