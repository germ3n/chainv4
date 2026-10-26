//! The node's side of the mining port: a producer that hands each template's work to the workers
//! connected, a range of nonces apart, alongside the node's own miner if it has one, and checks
//! every nonce a worker sends before it becomes a seal.

use {
    super::protocol::{self, Receiver, Sender},
    crate::consensus::{DeviceReport, MiningReport, Producer, WorkerReport, meets, work_hash},
    crate::registry::validators::Member,
    crate::structs::address::Address,
    crate::structs::block::{BlockHeader, Seal},
    crate::structs::hash::{Hash, HashType},
    r#macro::document,
    serde_json::{Value, json},
    std::collections::BTreeMap,
    std::net::{SocketAddr, TcpListener, TcpStream},
    std::sync::atomic::{AtomicBool, Ordering},
    std::sync::{Arc, Condvar, Mutex, MutexGuard},
    std::thread::JoinHandle,
    std::time::Duration,
};

/// The first nonce a worker's range starts at: far above what the node's own devices reach.
const FIRST_RANGE: u64 = 1 << 60;
/// How many nonces each worker's range holds: days of work for the fastest GPU.
pub const SPAN: u64 = 1 << 48;
/// How many ranges there are before they come round again: they fit in a u64 from FIRST_RANGE.
const RANGES: u64 = 1 << 15;
/// The most connections, workers and handshakes together, the port takes at once.
const MOST_CONNECTIONS: usize = 256;
/// How long a connection has to finish its handshake.
const HANDSHAKE: Duration = Duration::from_secs(10);
/// How long a worker may say nothing before it is taken for gone: it sends its rates more often.
pub const QUIET: Duration = Duration::from_secs(60);
/// How often the node writes to a worker with no new work, so the worker knows it is still there.
pub const PING: Duration = Duration::from_secs(20);
/// The most devices one worker's rates may name.
const MOST_DEVICES: usize = 64;

/// A seal's callback, shared by the pool and the node's own miner.
pub type Found = Arc<dyn Fn(Hash, Seal) + Send + Sync>;

#[document(
    summary = "Where a node takes workers, the token they must know, and the protocol name they must speak.",
    category = "Mining",
    see_also = "Pool, Mining",
)]
#[derive(Clone)]
pub struct MiningPort {
    #[document(summary = "The address to listen on, such as 0.0.0.0:4002; port 0 picks one.")]
    pub listen: SocketAddr,
    #[document(summary = "The secret workers share with the node.")]
    pub token: Vec<u8>,
    #[document(summary = "The node's protocol name, as --protocol gives it: a worker must give the same.")]
    pub protocol: String,
}

impl std::fmt::Debug for MiningPort {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("MiningPort").field("listen", &self.listen).field("protocol", &self.protocol).finish_non_exhaustive()
    }
}

/// The work the pool hands out now.
struct Job {
    /// Bumped for every new template, so a worker's nonce for an older one is told apart.
    number: u64,
    /// The block id and difficulty to find a nonce for; None once one was found.
    work: Option<(Hash, u128)>,
}

/// What the pool's threads share.
struct State {
    hash: HashType,
    token: Vec<u8>,
    protocol: String,
    found: Found,
    job: Mutex<Job>,
    changed: Condvar,
    stopped: AtomicBool,
    /// Every connection's stream, handshakes included, so stopping can close them all.
    streams: Mutex<BTreeMap<u64, TcpStream>>,
    /// The workers past their handshake.
    workers: Mutex<BTreeMap<u64, WorkerReport>>,
}

/// What became of a nonce a worker sent.
#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    Took,
    /// For a template no longer current, or one whose seal was found already.
    Stale,
    /// It does not meet the difficulty.
    Short,
}

impl State {
    fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Checks a worker's nonce, and makes it the seal when it is the first good one.
    fn take(&self, number: u64, nonce: u64) -> Verdict {
        let mut job = Self::lock(&self.job);

        let Some((id, difficulty)) = job.work.filter(|_| job.number == number) else {
            return Verdict::Stale;
        };

        if !meets(work_hash(self.hash, &id, nonce), difficulty) {
            return Verdict::Short;
        }

        job.work = None;
        drop(job);
        (self.found)(id, Seal::Work { nonce });

        Verdict::Took
    }

    /// The work message for a worker, or None while there is nothing to mine.
    fn work(&self, range: u64) -> (u64, Option<Value>) {
        let job = Self::lock(&self.job);
        let message = job.work.map(|(id, difficulty)| {
            json!({
                "type": "work",
                "job": job.number,
                "chain": id.algorithm().name(),
                "id": crate::structs::hex::to_hex(&id.to_bytes()),
                "difficulty": difficulty.to_string(),
                "hash": self.hash.name(),
                "start": FIRST_RANGE + (range % RANGES) * SPAN,
                "span": SPAN,
            })
        });

        (job.number, message)
    }
}

#[document(
    summary = "Mines with workers over the mining port, alongside the node's own miner if it has one. Each template's work goes to every worker, each with a range of nonces of its own, far above the ones the node's own devices try; a nonce a worker sends becomes the seal only once it checks. Workers prove they know the token the node was given before they get any work, and every message after is authenticated.",
    category = "Mining",
    see_also = "Producer, Miner, GpuMiner, MiningReport",
    note = "The token authenticates; it does not encrypt. Anyone watching the connection sees the work and the nonces, which only matter until the block is out anyway.",
)]
pub struct Pool {
    address: Address,
    listening: SocketAddr,
    state: Arc<State>,
    accepting: Option<JoinHandle<()>>,
    local: Option<Box<dyn Producer>>,
}

impl Pool {
    #[document(
        parent = "Pool",
        summary = "Opens the mining port and takes workers on it, with nothing to mine until the first header.",
        params = {
            address = "Where rewards for the blocks the pool's workers find go.",
            hash = "The chain's work hash, from genesis.",
            port = "Where workers connect, the token they share with the node, and the protocol name.",
            found = "Called with the block id and its seal when a worker's nonce checks.",
        },
        returns = "The pool, or why the port did not open.",
    )]
    pub fn new(address: Address, hash: HashType, port: MiningPort, found: Found) -> Result<Pool, String> {
        let MiningPort { listen, token, protocol } = port;
        let listener = TcpListener::bind(listen).map_err(|err| format!("the mining port {listen} did not open: {err}"))?;
        let listening = listener.local_addr().map_err(|err| err.to_string())?;
        listener.set_nonblocking(true).map_err(|err| err.to_string())?;
        let state = Arc::new(State {
            hash,
            token,
            protocol,
            found,
            job: Mutex::new(Job { number: 0, work: None }),
            changed: Condvar::new(),
            stopped: AtomicBool::new(false),
            streams: Mutex::new(BTreeMap::new()),
            workers: Mutex::new(BTreeMap::new()),
        });
        let accepting = {
            let state = state.clone();

            std::thread::Builder::new()
                .name("mining-port".to_string())
                .spawn(move || accept(&listener, &state))
                .map_err(|err| format!("the mining port's thread did not start: {err}"))?
        };
        log::info!("[mining] taking workers on {listening}");

        Ok(Pool {
            address,
            listening,
            state,
            accepting: Some(accepting),
            local: None,
        })
    }

    #[document(
        parent = "Pool",
        summary = "A seal callback for the node's own miner: it ends the pool's work for that block too, so workers' later nonces for it count as stale, then hands the seal on.",
    )]
    pub fn finder(&self) -> impl Fn(Hash, Seal) + Send + Sync + 'static {
        let state = self.state.clone();

        move |id, seal| {
            State::lock(&state.job).work.take_if(|(current, _)| *current == id);
            (state.found)(id, seal);
        }
    }

    #[document(
        parent = "Pool",
        summary = "Mines on the node's own devices too: every header goes to this producer as well as to the workers.",
        params = { local = "A Miner or GpuMiner, made with the callback finder gives." },
    )]
    pub fn mining_too(mut self, local: Box<dyn Producer>) -> Pool {
        self.local = Some(local);

        self
    }

    #[document(parent = "Pool", summary = "Where workers connect: the port it was given, or the one picked for port 0.")]
    pub fn listening(&self) -> SocketAddr {
        self.listening
    }
}

impl Producer for Pool {
    fn address(&self, _producers: &[Member]) -> Address {
        self.address
    }

    /// Timestamps must move forward, so the next millisecond after the parent's.
    fn start_at(&self, parent: &BlockHeader, _producers: &[Member]) -> u64 {
        parent.timestamp + 1
    }

    fn seal(&mut self, header: &BlockHeader) -> Option<Seal> {
        {
            let mut job = State::lock(&self.state.job);
            job.number += 1;
            job.work = Some((header.id(), header.difficulty));
        }
        self.state.changed.notify_all();

        // The node's own miner only ever finds its seal later; none is ready at once.
        if let Some(local) = self.local.as_mut() {
            let _ = local.seal(header);
        }

        None
    }

    fn mining(&self) -> Option<MiningReport> {
        let devices = self.local.as_ref().and_then(|local| local.mining()).map(|report| report.devices).unwrap_or_default();

        Some(MiningReport {
            devices,
            listening: Some(self.listening.to_string()),
            workers: State::lock(&self.state.workers).values().cloned().collect(),
        })
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        self.state.stopped.store(true, Ordering::SeqCst);
        self.state.changed.notify_all();

        // Under the lock a connection registers under, so one registering after this sees the stop.
        for stream in State::lock(&self.state.streams).values() {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }

        if let Some(accepting) = self.accepting.take() {
            let _ = accepting.join();
        }
    }
}

/// Takes connections until the pool stops, a thread for each, and waits for them all to end.
fn accept(listener: &TcpListener, state: &Arc<State>) {
    let mut connections: Vec<JoinHandle<()>> = Vec::new();
    let mut next: u64 = 0;

    while !state.stopped.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, peer)) => {
                connections.retain(|connection| !connection.is_finished());

                if connections.len() >= MOST_CONNECTIONS {
                    log::warn!("[mining] {peer}: {MOST_CONNECTIONS} connections already; turned away");

                    continue;
                }

                let (state, number) = (state.clone(), next);
                next += 1;
                let thread = std::thread::Builder::new().name(format!("mining-worker-{number}")).spawn(move || {
                    if let Err(why) = serve(&state, stream, peer, number) {
                        log::info!("[mining] {peer}: {why}");
                    }

                    State::lock(&state.streams).remove(&number);
                    State::lock(&state.workers).remove(&number);
                });

                match thread {
                    Ok(thread) => connections.push(thread),
                    Err(err) => log::error!("[mining] a worker's thread did not start: {err}"),
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(50)),
            Err(err) => {
                log::warn!("[mining] taking a connection failed: {err}");
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }

    for connection in connections {
        let _ = connection.join();
    }
}

/// One connection: the handshake, then work out on a thread of its own and nonces and rates in on
/// this one, until either side goes or the pool stops.
fn serve(state: &Arc<State>, stream: TcpStream, peer: SocketAddr, number: u64) -> Result<(), String> {
    stream.set_nonblocking(false).map_err(|err| err.to_string())?;
    stream.set_nodelay(true).map_err(|err| err.to_string())?;

    {
        let mut streams = State::lock(&state.streams);

        if state.stopped.load(Ordering::SeqCst) {
            return Ok(());
        }

        streams.insert(number, stream.try_clone().map_err(|err| err.to_string())?);
    }

    stream.set_read_timeout(Some(HANDSHAKE)).map_err(|err| err.to_string())?;
    let (sender, mut receiver, hello) = protocol::accept(stream.try_clone().map_err(|err| err.to_string())?, &state.token, &state.protocol)?;
    stream.set_read_timeout(Some(QUIET)).map_err(|err| err.to_string())?;
    log::info!("[mining] worker {} connected from {peer}", hello.name);

    State::lock(&state.workers).insert(
        number,
        WorkerReport {
            name: hello.name.clone(),
            address: peer.to_string(),
            since: crate::node::now_ms(),
            devices: Vec::new(),
            found: 0,
            refused: 0,
        },
    );

    let sender = Arc::new(Mutex::new(sender));
    let closed = Arc::new(AtomicBool::new(false));
    let writing = {
        let (state, sender, closed) = (state.clone(), sender.clone(), closed.clone());

        std::thread::Builder::new()
            .name(format!("mining-work-{number}"))
            .spawn(move || hand_out(&state, &sender, &closed, number))
            .map_err(|err| format!("the work thread did not start: {err}"))?
    };

    let result = listen(state, &mut receiver, &sender, number, &hello.name);

    closed.store(true, Ordering::SeqCst);
    state.changed.notify_all();
    let _ = stream.shutdown(std::net::Shutdown::Both);
    let _ = writing.join();

    result
}

/// Sends the worker each new job as it comes, and a ping when nothing came for a while.
fn hand_out(state: &State, sender: &Mutex<Sender>, closed: &AtomicBool, range: u64) {
    let mut sent = None;

    loop {
        let (number, message) = state.work(range);

        let message = match message {
            Some(message) if sent != Some(number) => {
                sent = Some(number);

                message
            }
            _ => json!({ "type": "ping" }),
        };

        if State::lock(sender).send(&message).is_err() {
            return;
        }

        // Until the job changes, the connection closes, the pool stops, or it is time to ping.
        let job = State::lock(&state.job);
        let (_job, _) = state
            .changed
            .wait_timeout_while(job, PING, |job| {
                job.number == number && !closed.load(Ordering::SeqCst) && !state.stopped.load(Ordering::SeqCst)
            })
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        if closed.load(Ordering::SeqCst) || state.stopped.load(Ordering::SeqCst) {
            return;
        }
    }
}

/// Reads the worker's nonces and rates until it goes.
fn listen(state: &State, receiver: &mut Receiver, sender: &Mutex<Sender>, number: u64, name: &str) -> Result<(), String> {
    loop {
        let message = receiver.receive()?;

        match message["type"].as_str() {
            Some("found") => {
                let (Some(job), Some(nonce)) = (message["job"].as_u64(), message["nonce"].as_u64()) else {
                    return Err("a found message that does not read".to_string());
                };
                let verdict = state.take(job, nonce);

                if let Some(worker) = State::lock(&state.workers).get_mut(&number) {
                    match verdict {
                        Verdict::Took => worker.found += 1,
                        _ => worker.refused += 1,
                    }
                }

                match verdict {
                    Verdict::Took => log::info!("[mining] worker {name} found the seal for job {job}"),
                    Verdict::Stale => log::debug!("[mining] worker {name} sent a nonce for job {job}, which is over"),
                    Verdict::Short => log::warn!("[mining] worker {name} sent a nonce for job {job} that does not meet its difficulty"),
                }

                let took = verdict == Verdict::Took;
                let why = match verdict {
                    Verdict::Took => "",
                    Verdict::Stale => "the block was found already, or the template moved on",
                    Verdict::Short => "it does not meet the difficulty",
                };
                State::lock(sender).send(&json!({ "type": "verdict", "job": job, "nonce": nonce, "took": took, "why": why }))?;
            }
            Some("rate") => {
                let devices: Vec<DeviceReport> = message["devices"]
                    .as_array()
                    .map(|devices| {
                        devices
                            .iter()
                            .take(MOST_DEVICES)
                            .map(|device| DeviceReport {
                                name: device["name"].as_str().unwrap_or("device").chars().filter(|c| !c.is_control()).take(128).collect(),
                                rate: device["rate"].as_f64().filter(|rate| rate.is_finite() && *rate >= 0.0).unwrap_or(0.0),
                            })
                            .collect()
                    })
                    .unwrap_or_default();

                if let Some(worker) = State::lock(&state.workers).get_mut(&number) {
                    worker.devices = devices;
                }
            }
            // A newer worker's messages this node does not know, ignored rather than refused.
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests;
