//! A worker: mines for a node over its mining port, on this machine's CPU threads or GPUs, so a
//! rig with no node of its own adds its hashrate to one that has. It takes whatever work the node
//! hands it, in the range of nonces the node gives it, sends back each nonce it finds and, every
//! few seconds, its devices' hashrates; it reconnects when the connection goes. It reaches an open
//! port, whose token is public, over the node's wss://, and nothing starts over plain TCP to one that
//! can be read on the way unless its operator says so.

use {
    super::protocol::{self, PING, QUIET, Receiver, Sender},
    super::status::Work,
    super::wss::{self, Target},
    crate::consensus::{DeviceReport, Rig},
    crate::structs::address::Address,
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
    #[document(summary = "The node's mining port, as host:port; or its WebSocket, as wss://host/mining/ws, which carries the same protocol over TLS.")]
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
    #[document(summary = "Where a pool pays this worker's shares; a node that runs a pool takes no worker without one, and one that runs none pays its own address whatever this says.")]
    pub payout: Option<Address>,
    #[document(summary = "The file to keep the payout's claims on the pool in, as the pool tells of each new one, so they can be claimed from any node should the pool's be gone; None to keep none.")]
    pub claims: Option<std::path::PathBuf>,
    #[document(summary = "Mine alone: each block found pays the payout in full, on a node started with --mining-solo, which refuses it otherwise. Needs a payout.")]
    pub solo: bool,
    #[document(summary = "Go over plain TCP to an open port all the same, where anyone on the way could rewrite this worker's payout: --insecure-open-port.")]
    pub insecure_open_port: bool,
    #[document(summary = "A PEM file's text holding the certificates to trust for wss://, in place of the roots built in: a node's own. Its host's name is checked all the same.")]
    pub ca_cert: Option<Vec<u8>>,
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
            .field("payout", &self.payout)
            .field("claims", &self.claims)
            .field("solo", &self.solo)
            .field("insecure_open_port", &self.insecure_open_port)
            .finish_non_exhaustive()
    }
}

pub use super::status::{Mode, WorkerStatus};

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
    status: Arc<Mutex<WorkerStatus>>,
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
        let status = Arc::new(Mutex::new(WorkerStatus::default()));

        if config.solo && config.payout.is_none() {
            return Err("mining alone needs a payout address, for the blocks found to pay".to_string());
        }

        route(&config)?;

        if config.insecure_open_port && config.token == protocol::OPEN_TOKEN && !config.node.contains("://") && !protocol::plain_cannot_be_read(&config.node, config.proxy.as_deref()) {
            log::warn!("[worker] --insecure-open-port: anyone on the way to {} can rewrite where this worker's work pays", config.node);
        }

        let thread = {
            let (stop, stream, rig, status) = (stop.clone(), stream.clone(), rig.clone(), status.clone());

            std::thread::Builder::new()
                .name("worker".to_string())
                .spawn(move || run(&config, &stop, &stream, &rig, &status))
                .map_err(|err| format!("the worker's thread did not start: {err}"))?
        };

        Ok(Worker {
            stop,
            status,
            stream,
            rig,
            thread: Some(thread),
        })
    }

    #[document(parent = "Worker", summary = "Its devices and their hashrates; none before the first work.")]
    pub fn devices(&self) -> Vec<DeviceReport> {
        lock(&self.rig).as_ref().map(|(_, rig)| rig.devices()).unwrap_or_default()
    }

    #[document(parent = "Worker", summary = "How it is doing now.")]
    pub fn status(&self) -> WorkerStatus {
        lock(&self.status).clone()
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
fn run(config: &WorkerConfig, stop: &AtomicBool, current: &Mutex<Option<TcpStream>>, rig: &Mounted, status: &Mutex<WorkerStatus>) {
    let (found, mut seals) = mpsc::channel();

    while !stop.load(Ordering::SeqCst) {
        let result = connect(config).and_then(|stream| {
            *lock(current) = Some(stream.try_clone().map_err(|err| err.to_string())?);

            // Stopped while connecting: the stream was not there to close.
            if stop.load(Ordering::SeqCst) {
                return Ok(());
            }

            session(config, stream, rig, &found, &mut seals, stop, status)
        });
        *lock(current) = None;

        {
            let mut status = lock(status);
            status.connected = false;

            if let Err(why) = &result {
                status.trouble = Some(why.clone());
            }
        }

        if let Some((_, rig)) = lock(rig).as_ref() {
            rig.rest();
        }

        if stop.load(Ordering::SeqCst) {
            break;
        }

        // Asked again it would say the same, or mine for someone else: this needs a person.
        if let Err(why) = &result
            && why.starts_with(protocol::PAYOUT_DIFFERS)
        {
            return log::error!("[worker] {}: {why}; stopping", config.node);
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

/// How a worker reaches its node.
enum Route {
    Tcp,
    Wss(Target),
}

/// How a worker goes as told, or why it may not. Everything that mines comes through here, so nothing
/// can start over plain TCP to an open port someone could read on the way: its token is public, so
/// they could make its MACs too, and rewrite where its work pays both ways, unseen.
fn route(config: &WorkerConfig) -> Result<Route, String> {
    if config.node.contains("://") {
        return Target::parse(&config.node).map(Route::Wss);
    }

    if config.token != protocol::OPEN_TOKEN || protocol::plain_cannot_be_read(&config.node, config.proxy.as_deref()) {
        return Ok(Route::Tcp);
    }

    if config.insecure_open_port {
        return Ok(Route::Tcp);
    }

    Err(format!(
        "{} is a port open to anyone, whose token is public, so anyone on the way could rewrite where this worker's work pays, both ways and unseen. Give the node's API, --node https://NODE, or its WebSocket, --node wss://NODE/mining/ws. --insecure-open-port goes over plain TCP all the same",
        config.node
    ))
}

/// A connection to the node, past its handshake's TCP part: directly, or through the proxy.
fn connect(config: &WorkerConfig) -> Result<TcpStream, String> {
    let at = match route(config)? {
        Route::Tcp => config.node.clone(),
        Route::Wss(target) => target.address(),
    };

    connect_to(&at, config.proxy.as_deref())
}

/// A connection to an address, directly or through the proxy.
pub(crate) fn connect_to(at: &str, proxy: Option<&str>) -> Result<TcpStream, String> {
    if let Some(proxy) = proxy {
        return through(proxy, at);
    }

    let addresses = at.to_socket_addrs().map_err(|err| format!("no address for it: {err}"))?;
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
    status: &Mutex<WorkerStatus>,
) -> Result<(), String> {
    stream.set_nodelay(true).map_err(|err| err.to_string())?;
    let me = protocol::Hello { name: config.name.clone(), payout: config.payout, solo: config.solo };
    let held = stream.try_clone().map_err(|err| err.to_string())?;
    let (sender, mut receiver) = match route(config)? {
        Route::Tcp => {
            stream.set_read_timeout(Some(QUIET)).map_err(|err| err.to_string())?;
            protocol::connect_as(held, &config.token, &config.protocol, &me)?
        }
        // The thread keeping the WebSocket ends once this connection is shut, or its lines dropped.
        Route::Wss(target) => {
            let lines = wss::open(held, &target, config.ca_cert.as_deref())?;
            protocol::connect_over(lines.out, lines.came, &config.token, &config.protocol, &me)?
        }
    };
    log::info!("[worker] mining for {} as {}{}", config.node, config.name, if config.solo { ", alone" } else { "" });

    {
        let mut status = lock(status);
        (status.connected, status.trouble, status.mode) = (true, None, None);
    }

    // Seals found for a job before this connection belong to no job it knows.
    while seals.try_recv().is_ok() {}

    let job: Mutex<Option<(u64, Hash)>> = Mutex::new(None);
    let closed = AtomicBool::new(false);

    std::thread::scope(|scope| {
        let (job, closed) = (&job, &closed);
        // The receiver moves to the sending thread, borrowed mutably so it is that thread's alone.
        let sending = scope.spawn(move || send(sender, rig, &*seals, job, closed, stop));
        let result = take_work(config, &mut receiver, rig, found, job, status);

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
                    // A pool's shares come many a second at first: its verdicts say what became of them.
                    log::debug!("[worker] found a nonce for job {number}");
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
    status: &Mutex<WorkerStatus>,
) -> Result<(), String> {
    // Under a pool: the shares it took this connection, and the share difficulty now.
    let mut shares: u64 = 0;
    let mut difficulty: Option<u128> = None;
    // Verdicts come as fast as shares, at an easy share difficulty tens of thousands a second:
    // counted here, and told to the status every so often rather than one by one.
    let (mut taken, mut refused, mut told) = (0, 0, Instant::now());

    loop {
        let message = match receiver.receive() {
            Ok(message) => message,
            Err(why) => {
                tell(status, &mut taken, &mut refused);

                return if why == "the connection closed" { Ok(()) } else { Err(why) };
            }
        };

        if message["type"] != "verdict" || told.elapsed() >= TELL_EVERY {
            tell(status, &mut taken, &mut refused);
            told = Instant::now();
        }

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
                let rig = &mounted.as_ref().expect("mounted above").1;

                {
                    let mut status = lock(status);
                    status.jobs += 1;
                    (status.difficulty, status.share_difficulty) = (Some(work.difficulty), work.share);
                    // A node that said nothing of a pool or of mining alone mines for itself.
                    status.mode.get_or_insert(Mode::Node);
                }

                // A pool's work: every nonce at the share difficulty goes back, the block's among them.
                match work.share {
                    Some(share) => {
                        if difficulty != Some(share) {
                            log::info!("[worker] share difficulty {share}");
                            difficulty = Some(share);
                        }

                        rig.shares(work.id, share, work.start);
                    }
                    None => rig.work(work.id, work.difficulty, work.start),
                }

                log::debug!("[worker] job {} at difficulty {}, nonces from {}", work.job, work.difficulty, work.start);
            }
            Some("verdict") if message["took"] == true && message["kind"] == "share" => {
                shares += 1;
                taken += 1;
                log::debug!("[worker] the pool took share {shares}, for job {}", message["job"]);
            }
            Some("verdict") if message["took"] == true => {
                tell(status, &mut taken, &mut refused);
                lock(status).blocks += 1;
                log::info!("[worker] the node took the nonce for job {}: a block", message["job"]);
            }
            Some("verdict") => {
                refused += 1;
                log::debug!("[worker] the node refused the nonce for job {}: {}", message["job"], message["why"].as_str().unwrap_or(""));
            }
            Some("pool") => {
                let (fee, window) = (message["fee"].as_u64().unwrap_or(0), message["window"].as_u64().unwrap_or(0));
                lock(status).mode = Some(Mode::Pool { fee, window });
                log::info!("[worker] the node runs a pool: a fee of {}%, each block split by the last {window} blocks' worth of shares", fee as f64 / 1e7);
            }
            Some("solo") => {
                lock(status).mode = Some(Mode::Solo);
                log::info!("[worker] mining alone: each block found pays {} in full", config.payout.map(|payout| payout.to_string()).unwrap_or_default());
            }
            Some("refused") => return Err(format!("the node refused this worker: {}", message["why"].as_str().unwrap_or(""))),
            #[cfg(feature = "release")]
            Some("release") => said_release(&message["manifest"]),
            Some("standing") => {
                if let Some(why) = protocol::pays_another(&message, config.payout.as_ref()) {
                    return Err(why);
                }

                log::info!(
                    "[worker] {shares} shares taken since connecting; {} in the window, {:.4}% of the next block; {} earned in all for {}, claimed with chainv4 claim",
                    message["shares"].as_u64().unwrap_or(0),
                    message["parts"].as_u64().unwrap_or(0) as f64 / 1e7,
                    message["earned"].as_str().unwrap_or("0"),
                    message["payout"].as_str().unwrap_or("")
                );
                keep_claim(config, &message);
                lock(status).standing = Some(message.clone());
            }
            // Pings, and a newer node's messages this worker does not know.
            _ => {}
        }
    }
}

/// How often a worker's counts of shares taken and nonces refused reach its status.
const TELL_EVERY: Duration = Duration::from_millis(200);

/// Adds the shares taken and nonces refused counted since to the status.
fn tell(status: &Mutex<WorkerStatus>, taken: &mut u64, refused: &mut u64) {
    if *taken + *refused > 0 {
        let mut status = lock(status);
        status.shares += std::mem::take(taken);
        status.refused += std::mem::take(refused);
    }
}

/// Keeps the claim a pool's standing message carries, in the worker's claims file: only one for
/// its own payout, whatever the pool says.
fn keep_claim(config: &WorkerConfig, message: &serde_json::Value) {
    let (Some(file), Some(payout)) = (&config.claims, config.payout) else {
        return;
    };

    if message["claim"].is_null() {
        return;
    }

    let claim = match super::claims::claim_from_json(&message["claim"], None) {
        Ok(claim) if claim.to == payout => claim,
        Ok(claim) => return log::warn!("[worker] the pool sent a claim for {}, not {payout}; not kept", claim.to),
        Err(why) => return log::warn!("[worker] the pool's claim does not read: {why}"),
    };
    let (height, earned) = (claim.height, claim.earned);

    match super::claims::save_claim(file, claim, message["settled"].as_u64()) {
        Ok(true) => log::info!("[worker] kept the claim on {earned} earned, at block {height}, in {}: chainv4 claim --saved takes it", file.display()),
        Ok(false) => {}
        Err(why) => log::warn!("[worker] the claim was not kept: {why}"),
    }
}

/// Warns when the release the node told of names a newer chainv4 than this worker, once it checks
/// against the release keys built in: the node only relays it.
#[cfg(feature = "release")]
fn said_release(manifest: &serde_json::Value) {
    use crate::release::{ReleaseKeys, Standing, kept_path};
    use crate::structs::version::Version;

    let Ok(keys) = ReleaseKeys::built_in() else {
        return;
    };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |since| since.as_secs());

    match standing_in(manifest, &keys, kept_path().as_deref(), now) {
        Some(Standing::Behind { newest }) => log::warn!("[worker] chainv4 {newest} is out, and this worker runs {}: update it with chainv4 update", Version::ours()),
        Some(Standing::Unsupported { newest, least }) => log::warn!("[worker] chainv4 {newest} is out, and versions older than {least} are no longer supported; this worker runs {}: update it with chainv4 update", Version::ours()),
        _ => {}
    }
}

/// Where this worker stands against the newest release it has seen, the node's among them, in the
/// file it keeps it in so no node takes it back to an older one; None without one that still counts.
#[cfg(feature = "release")]
fn standing_in(manifest: &serde_json::Value, keys: &crate::release::ReleaseKeys, file: Option<&std::path::Path>, now: u64) -> Option<crate::release::Standing> {
    use crate::release::{SignedRelease, current_release, kept_release, take_newest, write_kept};
    use crate::structs::version::Version;

    let mut kept = file.and_then(|path| std::fs::read_to_string(path).ok()).and_then(|text| kept_release(&text, keys));
    let before = kept.as_ref().map(|kept| kept.release.issued);

    match SignedRelease::from_json(manifest) {
        Ok(offered) => {
            if let Err(why) = take_newest(keys, &mut kept, offered, now) {
                log::debug!("[worker] the node told of a release that does not count: {why}");
            }
        }
        Err(why) => log::debug!("[worker] the node told of a release that does not read: {why}"),
    }

    if let (Some(path), Some(newest)) = (file, kept.as_ref().filter(|kept| Some(kept.release.issued) != before))
        && let Err(why) = write_kept(path, newest)
    {
        log::debug!("[worker] the newest release seen is not kept: {why}");
    }

    current_release(&kept, now)?.release.product("chainv4").map(|product| product.standing(Version::ours()))
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
        #[cfg(all(feature = "gpu-mining", not(target_arch = "wasm32")))]
        Devices::Gpu(which) => Rig::gpu(hash, which.as_deref(), found),
        #[cfg(not(all(feature = "gpu-mining", not(target_arch = "wasm32"))))]
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

    #[cfg(feature = "release")]
    #[test]
    fn a_worker_goes_by_the_newest_release_it_saw_whatever_a_node_says_later() {
        use crate::crypto::keypair::Keypair;
        use crate::release::{Product, Release, ReleaseKeys, SignedRelease, Standing};
        use crate::structs::version::Version;

        const NOW: u64 = 1_800_000_000;
        let signer = Keypair::from_seed([4; 32]);
        let keys = ReleaseKeys { threshold: 1, keys: vec![signer.public()], updates: None, floor: 0 };
        let ours = Version::ours();
        let newest = Version::new(ours.major, ours.minor + 1, 0);
        let release = |issued: u64, version: Version| {
            let product = Product { name: "chainv4".to_string(), version, least: Version::new(0, 0, 0), notes: String::new(), files: Vec::new() };
            let mut signed = SignedRelease { release: Release { issued, expires: issued + 1_000, products: vec![product], breaking: Vec::new() }, signatures: Vec::new() };
            signed.sign(&signer);
            signed.to_json()
        };
        let file = std::env::temp_dir().join(format!("chainv4-worker-seen-{}", std::process::id())).join("release.json");
        let _ = std::fs::remove_file(&file);

        assert_eq!(standing_in(&release(NOW, newest), &keys, Some(&file), NOW), Some(Standing::Behind { newest }));
        // A node serving an older one, or nothing that reads, hides nothing: across runs, as kept.
        assert_eq!(standing_in(&release(NOW - 10, ours), &keys, Some(&file), NOW), Some(Standing::Behind { newest }));
        assert_eq!(standing_in(&serde_json::json!({ "release": {} }), &keys, Some(&file), NOW), Some(Standing::Behind { newest }));
        // Past its expires it says nothing, rather than what may be long out of date.
        assert_eq!(standing_in(&release(NOW - 10, ours), &keys, Some(&file), NOW + 1_000), None);
        // With nowhere to keep it, it still goes by what it was told.
        assert_eq!(standing_in(&release(NOW, newest), &keys, None, NOW), Some(Standing::Behind { newest }));
        std::fs::remove_dir_all(file.parent().unwrap()).unwrap();
    }

    /// A node with the token that pays another address than the worker gave: what a port's owner
    /// could do, or anyone on the path of an open port, which is why the worker looks.
    #[test]
    fn a_worker_told_it_pays_another_address_stops_and_says_so() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let theirs = Address::Ed25519([0xe; 32]);
        let served = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            protocol::accept_paying_for_tests(stream, b"token", "chainv4", &theirs);
            // The worker hangs up rather than wait for work.
            listener.set_nonblocking(true).unwrap();
            std::thread::sleep(RECONNECT + Duration::from_millis(500));

            listener.accept().is_ok()
        });
        let ours = Address::Ed25519([0xa; 32]);
        let worker = Worker::start(WorkerConfig {
            node: address.to_string(),
            token: b"token".to_vec(),
            name: "rig".to_string(),
            protocol: "chainv4".to_string(),
            devices: Devices::Cpu(1),
            proxy: None,
            payout: Some(ours),
            claims: None,
            solo: false,
            insecure_open_port: false,
            ca_cert: None,
        })
        .unwrap();

        assert!(!served.join().unwrap(), "it did not connect again");
        let status = worker.status();

        assert!(!status.connected);
        assert_eq!(status.jobs, 0, "it mined nothing for them");
        assert!(status.trouble.as_deref().is_some_and(|why| why.contains(&theirs.to_string()) && why.contains(&ours.to_string())), "{:?}", status.trouble);
        worker.wait();
    }

    /// A SOCKS5 proxy on this machine for one connection, which takes the mining port's handshake
    /// itself, whatever name it was asked for: all a worker going over plain TCP can tell of a port
    /// elsewhere. What the handshake said of the worker.
    fn port_through_proxy(token: &'static [u8]) -> (String, std::thread::JoinHandle<protocol::Hello>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let at = format!("socks5h://{}", listener.local_addr().unwrap());
        let served = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut greeting = [0; 3];
            stream.read_exact(&mut greeting).unwrap();
            stream.write_all(&[5, 0]).unwrap();
            let mut head = [0; 5];
            stream.read_exact(&mut head).unwrap();
            let mut asked = vec![0; usize::from(head[4]) + 2];
            stream.read_exact(&mut asked).unwrap();
            stream.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]).unwrap();

            protocol::accept(stream, token, "chainv4").map_err(|why| format!("the handshake: {why}")).unwrap().2
        });

        (at, served)
    }

    fn config(node: &str, token: &[u8], proxy: Option<String>) -> WorkerConfig {
        WorkerConfig {
            node: node.to_string(),
            token: token.to_vec(),
            name: "rig".to_string(),
            protocol: "chainv4".to_string(),
            devices: Devices::Cpu(1),
            proxy,
            payout: Some(Address::Ed25519([0xa; 32])),
            claims: None,
            solo: false,
            insecure_open_port: false,
            ca_cert: None,
        }
    }

    #[test]
    fn plain_tcp_to_an_open_port_is_refused_without_the_flag_and_goes_with_it() {
        let (at, served) = port_through_proxy(protocol::OPEN_TOKEN);
        let refused = Worker::start(config("pool.example:4002", protocol::OPEN_TOKEN, Some(at.clone()))).err().unwrap();

        assert!(refused.contains("--insecure-open-port") && refused.contains("rewrite"), "{refused}");
        // Nor by writing the token out as a private one's: it is what it is, not what it is called.
        assert!(Worker::start(config("pool.example:4002", b"chainv4 open mining pool", Some(at.clone()))).is_err());

        let worker = Worker::start(WorkerConfig { insecure_open_port: true, ..config("pool.example:4002", protocol::OPEN_TOKEN, Some(at)) }).unwrap();
        assert_eq!(served.join().unwrap().payout, Some(Address::Ed25519([0xa; 32])), "told, it went over plain TCP");
        drop(worker);
    }

    #[test]
    fn a_port_on_this_machine_or_an_onion_through_tor_here_cannot_be_read_on_the_way() {
        let (at, served) = port_through_proxy(protocol::OPEN_TOKEN);
        let onion = "abcdefghijklmnopqrstuvwxyz234567abcdefghijklmnopqrstuvwx.onion:4002";
        let worker = Worker::start(config(onion, protocol::OPEN_TOKEN, Some(at))).unwrap();

        assert_eq!(served.join().unwrap().name, "rig", "at an onion, through Tor on this machine");
        drop(worker);

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let served = std::thread::spawn(move || protocol::accept(listener.accept().unwrap().0, protocol::OPEN_TOKEN, "chainv4").map(|taken| taken.2.name));
        let worker = Worker::start(config(&address, protocol::OPEN_TOKEN, None)).unwrap();

        assert_eq!(served.join().unwrap().unwrap(), "rig", "on this machine");
        drop(worker);

        // Anything else can be: an onion with no Tor goes by the system's resolver, and a proxy
        // elsewhere reads what it is sent.
        let refused = |node: &str, proxy: Option<&str>| Worker::start(config(node, protocol::OPEN_TOKEN, proxy.map(str::to_string))).is_err();

        assert!(refused(onion, None));
        assert!(refused(onion, Some("socks5h://192.0.2.1:9050")));
        assert!(refused("127.0.0.1:4002", Some("socks5h://192.0.2.1:9050")));
        assert!(refused("203.0.113.7:4002", None));
        assert!(refused("localhost.example:4002", None));
    }

    #[test]
    fn a_private_tokens_port_is_unaffected_and_stays_on_plain_tcp() {
        const TOKEN: &[u8] = b"a private port's token, 32 characters";
        let (at, served) = port_through_proxy(TOKEN);
        let worker = Worker::start(config("pool.example:4002", TOKEN, Some(at))).unwrap();

        assert_eq!(served.join().unwrap().name, "rig");
        drop(worker);
    }

    #[test]
    fn a_websocket_without_tls_is_refused_as_it_protects_nothing() {
        assert!(Worker::start(config("ws://pool.example/mining/ws", protocol::OPEN_TOKEN, None)).err().unwrap().contains("without TLS"));
        assert!(Worker::start(config("https://pool.example", protocol::OPEN_TOKEN, None)).err().unwrap().contains("wss://"));
    }

    /// A node's port behind TLS and a WebSocket, as nginx is in front of it: each connection in turn
    /// goes as what it is given says, with the port's own handshake.
    fn behind_tls<F>(sessions: F) -> (String, std::thread::JoinHandle<()>)
    where
        F: FnOnce(&dyn Fn() -> Option<(tungstenite::WebSocket<rustls::StreamOwned<rustls::ServerConnection, TcpStream>>, protocol::Sealer, protocol::Opener)>) + Send + 'static,
    {
        let (listener, tls) = wss::tests::tls_listener();
        let at = format!("wss://localhost:{}/mining/ws", listener.local_addr().unwrap().port());
        let served = std::thread::spawn(move || {
            let next = || {
                let stream = wss::tests::accept_one(&listener, &tls).ok()?;
                let mut socket = tungstenite::accept(stream).ok()?;
                let (greeting, challenge) = protocol::greeting("chainv4").unwrap();
                socket.send(tungstenite::Message::text(greeting.to_string())).unwrap();
                let hello: serde_json::Value = serde_json::from_str(socket.read().unwrap().to_text().unwrap()).unwrap();
                let (_, answer, sealer, opener) = protocol::welcome(protocol::OPEN_TOKEN, "chainv4", &challenge, &hello).unwrap();
                socket.send(tungstenite::Message::text(answer.to_string())).unwrap();

                Some((socket, sealer, opener))
            };

            sessions(&next);
        });

        (at, served)
    }

    fn until(worker: &Worker, what: impl Fn(&WorkerStatus) -> bool) -> WorkerStatus {
        let deadline = Instant::now() + Duration::from_secs(30);

        loop {
            let status = worker.status();

            if what(&status) {
                return status;
            }

            assert!(Instant::now() < deadline, "never came to it: {status:?}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// A proxy closing what it takes for idle, as nginx's proxy_read_timeout would, or going away: the
    /// worker comes back, over TLS, by itself, and takes work again. What it sends says it is alive
    /// well within any such timeout: its rates, every few seconds, whatever it finds.
    #[test]
    fn a_worker_over_wss_mines_keeps_talking_and_comes_back_from_a_proxy_closing_the_socket() {
        let (at, served) = behind_tls(|next| {
            let (mut socket, _, mut opener) = next().unwrap();
            // Quiet, it still says it is there, sealed as the port wants.
            let rate = opener.open(socket.read().unwrap().to_text().unwrap()).unwrap();
            assert_eq!(rate["type"], "rate");
            // Hung up on, with no goodbye.
            socket.get_ref().sock.shutdown(std::net::Shutdown::Both).unwrap();
            drop(socket);

            let (mut socket, mut sealer, mut opener) = next().unwrap();
            let chain = HashType::default_chain();
            let id = Hash::from_bytes(chain, &[7; 32]).unwrap();
            let work = json!({ "type": "work", "job": 4, "chain": chain.name(), "id": crate::structs::hex::to_hex(&id.to_bytes()), "difficulty": "1", "hash": chain.name(), "start": 0 });
            socket.send(tungstenite::Message::text(sealer.seal(&work))).unwrap();

            let found = loop {
                let message = opener.open(socket.read().unwrap().to_text().unwrap()).unwrap();

                if message["type"] == "found" {
                    break message;
                }
            };

            assert_eq!(found["job"], 4);
            socket.send(tungstenite::Message::text(sealer.seal(&json!({ "type": "verdict", "job": 4, "took": true, "kind": "block" })))).unwrap();

            while socket.read().is_ok() {}
        });
        let worker = Worker::start(WorkerConfig { ca_cert: Some(wss::tests::CERT.to_vec()), ..config(&at, protocol::OPEN_TOKEN, None) }).unwrap();
        let status = until(&worker, |status| status.blocks == 1);

        assert!(status.connected && status.jobs == 1, "{status:?}");
        drop(worker);
        served.join().unwrap();
    }

    /// Over wss:// a port that says it pays another is believed no more than over TCP.
    #[test]
    fn a_worker_over_wss_told_it_pays_another_address_stops_and_says_so() {
        let theirs = Address::Ed25519([0xe; 32]);
        let (listener, tls) = wss::tests::tls_listener();
        let at = format!("wss://localhost:{}/mining/ws", listener.local_addr().unwrap().port());
        let served = std::thread::spawn(move || {
            let stream = wss::tests::accept_one(&listener, &tls).unwrap();
            let mut socket = tungstenite::accept(stream).unwrap();
            let (greeting, challenge) = protocol::greeting("chainv4").unwrap();
            socket.send(tungstenite::Message::text(greeting.to_string())).unwrap();
            let hello: serde_json::Value = serde_json::from_str(socket.read().unwrap().to_text().unwrap()).unwrap();
            let answer = protocol::answer_paying_for_tests(protocol::OPEN_TOKEN, "chainv4", &challenge, &hello, &theirs);
            socket.send(tungstenite::Message::text(answer.to_string())).unwrap();

            while socket.read().is_ok() {}
        });
        let worker = Worker::start(WorkerConfig { ca_cert: Some(wss::tests::CERT.to_vec()), ..config(&at, protocol::OPEN_TOKEN, None) }).unwrap();
        let status = until(&worker, |status| status.trouble.is_some());

        assert!(status.trouble.as_deref().is_some_and(|why| why.starts_with(protocol::PAYOUT_DIFFERS)), "{status:?}");
        worker.wait();
        served.join().unwrap();
    }

    /// A certificate that does not check is never mined past, and never falls back: the worker says
    /// why and keeps trying, as one may be put right.
    #[test]
    fn a_worker_refused_a_certificate_says_why_and_never_falls_back_to_plain_tcp() {
        let (listener, tls) = wss::tests::tls_listener();
        let at = format!("wss://localhost:{}/mining/ws", listener.local_addr().unwrap().port());
        let served = std::thread::spawn(move || {
            // Each try is TLS or nothing: a plain handshake would show up here as an error of another kind.
            wss::tests::accept_one(&listener, &tls).err().unwrap()
        });
        let worker = Worker::start(config(&at, protocol::OPEN_TOKEN, None)).unwrap();
        let status = until(&worker, |status| status.trouble.is_some());

        assert!(status.trouble.as_deref().is_some_and(|why| why.contains("does not check") && why.contains("--ca-cert")), "{status:?}");
        assert!(!status.connected);
        assert!(!served.join().unwrap().is_empty());
        drop(worker);
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
