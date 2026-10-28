//! The node's side of the mining port: a producer that hands each template's work to the workers
//! connected, a range of nonces apart, alongside the node's own miner if it has one, and checks
//! every nonce a worker sends before it becomes a seal. With a pool (the pool feature, --pool),
//! workers send shares too, at a difficulty set for each, which the pool's books count; every
//! block sends the miners' share of its reward to the pool's escrow and commits to the root of
//! their claims (see super::shares).

use {
    super::protocol::{self, Receiver, Sender},
    crate::consensus::{DeviceReport, MiningReport, Producer, WorkerReport, meets, work_hash},
    crate::registry::validators::Member,
    crate::structs::address::Address,
    crate::consensus::PoolSettled,
    crate::structs::block::{BlockHeader, Payout, PoolCommitment, Seal},
    crate::structs::hash::{Hash, HashType},
    r#macro::document,
    serde_json::{Value, json},
    std::collections::{BTreeMap, VecDeque},
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
/// How often a pool tells each worker how it stands.
#[cfg(feature = "pool")]
const STANDING_EVERY: Duration = Duration::from_secs(30);
/// How many jobs back shares are taken for: templates change every two seconds, and a share
/// found as one changes, or sent over Tor, comes a moment late.
const RECENT_JOBS: usize = 8;

#[document(
    summary = "The token every worker of an open pool proves it knows, which anyone may: --mining-open's, and a worker's without --token.",
    category = "Mining",
    see_also = "MiningPort",
)]
pub const OPEN_TOKEN: &[u8] = b"chainv4 open mining pool";

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
    #[document(summary = "The pool it runs, with --pool; None for workers mining for the node's reward address alone.")]
    #[cfg(feature = "pool")]
    pub pool: Option<super::shares::PoolConfig>,
    #[document(summary = "The releases the node fetches, which it tells each worker of as it connects, so an older worker says so; None tells them nothing.")]
    #[cfg(feature = "updates")]
    pub releases: Option<Arc<crate::node::Releases>>,
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
    /// The template's height, how it shares out its reward and the claim root it commits to, for
    /// the books once it is sealed.
    height: u64,
    split: Vec<Payout>,
    root: Option<Hash>,
    /// The recent jobs, the current one first, each with its block id and difficulty: what shares
    /// are checked against.
    recent: VecDeque<(u64, Hash, u128)>,
}

/// A pool's books and its view of each worker.
#[cfg(feature = "pool")]
struct Books {
    /// The pool's address, whose escrow its miners' claims are paid from.
    address: Address,
    ledger: Mutex<super::shares::Ledger>,
    workers: Mutex<BTreeMap<u64, super::shares::PoolWorker>>,
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
    #[cfg(feature = "pool")]
    books: Option<Books>,
    /// The releases the node fetches, told to workers as they connect.
    #[cfg(feature = "updates")]
    releases: Option<Arc<crate::node::Releases>>,
}

/// What became of a nonce a worker sent.
#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    Took,
    /// A pool's share: it meets the worker's share difficulty, not the block's.
    #[cfg_attr(not(feature = "pool"), allow(dead_code))]
    Share,
    /// For a template no longer current, or one whose seal was found already; for a pool's
    /// share, a job too old.
    Stale,
    /// It does not meet the difficulty.
    Short,
    /// A share sent already.
    #[cfg_attr(not(feature = "pool"), allow(dead_code))]
    Twice,
}

impl State {
    fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Checks a worker's nonce, and makes it the seal when it is the first good one; under a
    /// pool, counts it as a share when it meets the worker's share difficulty.
    #[cfg_attr(not(feature = "pool"), allow(unused_variables))]
    fn take(&self, number: u64, nonce: u64, worker: u64) -> Verdict {
        let mut job = Self::lock(&self.job);

        let Some(&(_, id, difficulty)) = job.recent.iter().find(|(recent, _, _)| *recent == number) else {
            return Verdict::Stale;
        };
        let work = work_hash(self.hash, &id, nonce);
        let current = job.number == number && job.work.is_some();

        if current && meets(work, difficulty) {
            let (height, split, root) = (job.height, std::mem::take(&mut job.split), job.root);
            job.work = None;
            drop(job);
            #[cfg(feature = "pool")]
            {
                let _ = self.share(worker, number, nonce, work, difficulty);
                self.sealed(height, id, split, root);
            }
            #[cfg(not(feature = "pool"))]
            let _ = (height, split, root);
            (self.found)(id, Seal::Work { nonce });

            return Verdict::Took;
        }

        drop(job);

        #[cfg(feature = "pool")]
        if self.books.is_some() {
            return self.share(worker, number, nonce, work, difficulty);
        }

        match current {
            true => Verdict::Short,
            false => Verdict::Stale,
        }
    }

    /// Counts a share, when it meets the share difficulty the worker had for its job.
    #[cfg(feature = "pool")]
    fn share(&self, worker: u64, number: u64, nonce: u64, work: crate::structs::u256::u256, block: u128) -> Verdict {
        let Some(books) = &self.books else {
            return Verdict::Short;
        };
        let mut workers = Self::lock(&books.workers);
        let Some(standing) = workers.get_mut(&worker) else {
            return Verdict::Stale;
        };
        let Some(difficulty) = standing.difficulty(number) else {
            return Verdict::Stale;
        };

        if !meets(work, difficulty) {
            return Verdict::Short;
        }

        if !standing.took(number, nonce, difficulty) {
            return Verdict::Twice;
        }

        let payout = standing.payout;
        drop(workers);
        Self::lock(&books.ledger).add(payout, difficulty, crate::node::now_ms(), block);

        if let Some(report) = Self::lock(&self.workers).get_mut(&worker) {
            report.shares += 1;
        }

        Verdict::Share
    }

    /// Puts a block the pool sealed in its books, and saves them.
    #[cfg(feature = "pool")]
    fn sealed(&self, height: u64, id: Hash, split: Vec<Payout>, root: Option<Hash>) {
        if let Some(books) = &self.books {
            let mut ledger = Self::lock(&books.ledger);
            ledger.sealed(height, id, crate::node::now_ms(), split, root);
            ledger.save();
        }
    }

    /// The work message for a worker, or None while there is nothing to mine. Under a pool, it
    /// carries the worker's share difficulty for the job.
    fn work(&self, range: u64) -> (u64, Option<Value>) {
        let job = Self::lock(&self.job);
        let message = job.work.map(|(id, difficulty)| {
            #[cfg_attr(not(feature = "pool"), allow(unused_mut))]
            let mut message = json!({
                "type": "work",
                "job": job.number,
                "chain": id.algorithm().name(),
                "id": crate::structs::hex::to_hex(&id.to_bytes()),
                "difficulty": difficulty.to_string(),
                "hash": self.hash.name(),
                "start": FIRST_RANGE + (range % RANGES) * SPAN,
                "span": SPAN,
            });

            #[cfg(feature = "pool")]
            if let Some(books) = &self.books
                && let Some(worker) = Self::lock(&books.workers).get_mut(&range)
            {
                let min_share = Self::lock(&books.ledger).config().min_share;
                let share = worker.job(job.number, min_share, difficulty);
                message["share"] = json!(share.to_string());

                if let Some(report) = Self::lock(&self.workers).get_mut(&range) {
                    report.share_difficulty = share;
                }
            }

            message
        });

        (job.number, message)
    }

    /// How a worker stands in the pool, for it to show: its work and shares in the window, and
    /// its parts of the next block.
    #[cfg(feature = "pool")]
    fn standing(&self, worker: u64) -> Option<Value> {
        let books = self.books.as_ref()?;
        let payout = Self::lock(&books.workers).get(&worker)?.payout;
        let report = Self::lock(&books.ledger).report();
        let mine = report.miners.iter().find(|miner| miner.to == payout);
        // Its claim under the newest root a block of the pool's committed to, one not found
        // dropped, for the worker to keep: with it, it claims even should the pool be gone.
        let dropped = |id: &crate::structs::hash::Hash| report.blocks.iter().any(|block| block.id == *id && block.credited == Some(false));
        let claim = report.claims.iter().find_map(|tree| {
            let (height, id) = tree.committed.iter().rev().find(|(_, id)| !dropped(id))?;

            tree.claim(self.hash, books.address, *height, *id, &payout)
        });

        Some(json!({
            "type": "standing",
            "payout": payout.to_string(),
            "work": mine.map_or(0, |miner| miner.work).to_string(),
            "shares": mine.map_or(0, |miner| miner.shares),
            "parts": mine.map_or(0, |miner| miner.parts),
            "earned": mine.map_or(crate::structs::u256::u256::ZERO, |miner| miner.earned).to_string(),
            "window": report.work.to_string(),
            "miners": report.miners.len(),
            "fee": report.fee,
            "claim": claim.as_ref().map(super::claims::claim_to_json),
            "settled": report.settled(),
        }))
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
    /// The split and claim root pool last gave, which the next header to seal shares out by and
    /// commits to.
    split: Vec<Payout>,
    root: Option<Hash>,
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
        let listen = port.listen;
        let listener = TcpListener::bind(listen).map_err(|err| format!("the mining port {listen} did not open: {err}"))?;
        let listening = listener.local_addr().map_err(|err| err.to_string())?;
        listener.set_nonblocking(true).map_err(|err| err.to_string())?;
        let state = Arc::new(State {
            hash,
            token: port.token,
            protocol: port.protocol,
            found,
            job: Mutex::new(Job { number: 0, work: None, height: 0, split: Vec::new(), root: None, recent: VecDeque::new() }),
            changed: Condvar::new(),
            stopped: AtomicBool::new(false),
            streams: Mutex::new(BTreeMap::new()),
            workers: Mutex::new(BTreeMap::new()),
            #[cfg(feature = "updates")]
            releases: port.releases,
            #[cfg(feature = "pool")]
            books: port.pool.map(|config| Books { address, ledger: Mutex::new(super::shares::Ledger::open(config, hash)), workers: Mutex::new(BTreeMap::new()) }),
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
            split: Vec::new(),
            root: None,
        })
    }

    #[document(
        parent = "Pool",
        summary = "A seal callback for the node's own miner: it ends the pool's work for that block too, so workers' later nonces for it count as stale, then hands the seal on.",
    )]
    pub fn finder(&self) -> impl Fn(Hash, Seal) + Send + Sync + 'static {
        let state = self.state.clone();

        move |id, seal| {
            let mut job = State::lock(&state.job);

            if job.work.take_if(|(current, _)| *current == id).is_some() {
                let (height, split, root) = (job.height, std::mem::take(&mut job.split), job.root);
                drop(job);
                #[cfg(feature = "pool")]
                state.sealed(height, id, split, root);
                #[cfg(not(feature = "pool"))]
                let _ = (height, split, root);
            } else {
                drop(job);
            }

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
            let (number, id) = (job.number, header.id());
            job.work = Some((id, header.difficulty));
            job.height = header.height;
            job.split = std::mem::take(&mut self.split);
            job.root = self.root.take();
            job.recent.push_front((number, id, header.difficulty));
            job.recent.truncate(RECENT_JOBS);
        }

        #[cfg(feature = "pool")]
        if let Some(books) = &self.state.books {
            State::lock(&books.ledger).at_difficulty(header.difficulty);
        }
        self.state.changed.notify_all();

        // The node's own miner only ever finds its seal later; none is ready at once.
        if let Some(local) = self.local.as_mut() {
            let _ = local.seal(header);
        }

        None
    }

    #[cfg_attr(not(feature = "pool"), allow(unused_variables))]
    fn pool(&mut self, settled: &dyn Fn(&Hash, u64) -> PoolSettled) -> Option<PoolCommitment> {
        #[cfg(feature = "pool")]
        if let Some(books) = &self.state.books {
            let mut ledger = State::lock(&books.ledger);
            ledger.credit(settled);
            let (commitment, split) = ledger.commitment();
            (self.split, self.root) = (split, commitment.map(|commitment| commitment.root));

            return commitment;
        }

        None
    }

    fn mining(&self) -> Option<MiningReport> {
        let devices = self.local.as_ref().and_then(|local| local.mining()).map(|report| report.devices).unwrap_or_default();

        Some(MiningReport {
            devices,
            listening: Some(self.listening.to_string()),
            workers: State::lock(&self.state.workers).values().cloned().collect(),
            #[cfg(feature = "pool")]
            pool: self.state.books.as_ref().map(|books| crate::consensus::PoolReport { producer: Some(self.address), ..State::lock(&books.ledger).report() }),
            #[cfg(not(feature = "pool"))]
            pool: None,
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

        #[cfg(feature = "pool")]
        if let Some(books) = &self.state.books {
            State::lock(&books.ledger).save();
        }
    }
}

/// Takes connections until the pool stops, a thread for each, and waits for them all to end.
fn accept(listener: &TcpListener, state: &Arc<State>) {
    let mut connections: Vec<JoinHandle<()>> = Vec::new();
    let mut next: u64 = 0;
    #[cfg(feature = "pool")]
    let mut saved = std::time::Instant::now();

    while !state.stopped.load(Ordering::SeqCst) {
        // The books, every minute, so a crash loses at most that much of the window.
        #[cfg(feature = "pool")]
        if saved.elapsed() >= Duration::from_secs(60) {
            saved = std::time::Instant::now();

            if let Some(books) = &state.books {
                State::lock(&books.ledger).save();
            }
        }

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
                    #[cfg(feature = "pool")]
                    if let Some(books) = &state.books {
                        State::lock(&books.workers).remove(&number);
                    }
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
    #[cfg_attr(not(feature = "pool"), allow(unused_mut))]
    let (mut sender, mut receiver, hello) = protocol::accept(stream.try_clone().map_err(|err| err.to_string())?, &state.token, &state.protocol)?;
    stream.set_read_timeout(Some(QUIET)).map_err(|err| err.to_string())?;

    // A pool pays each worker's shares to the address it gave, and takes none without one.
    #[cfg(feature = "pool")]
    if let Some(books) = &state.books {
        let Some(payout) = hello.payout else {
            let why = "this node runs a pool: give the address to pay, --payout ADDRESS";
            let _ = sender.send(&json!({ "type": "refused", "why": why }));

            return Err(format!("worker {} gave no payout address", hello.name));
        };
        let config = State::lock(&books.ledger).config().clone();
        sender.send(&json!({ "type": "pool", "fee": config.fee, "window": config.window, "open": config.open, "min_share": config.min_share.to_string() }))?;
        State::lock(&books.workers).insert(number, super::shares::PoolWorker::new(payout));
    }

    // The newest release, as signed, so a worker older than it says so.
    #[cfg(feature = "updates")]
    if let Some(signed) = state.releases.as_ref().and_then(|releases| releases.known().signed) {
        sender.send(&json!({ "type": "release", "manifest": signed.to_json() }))?;
    }

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
            #[cfg(feature = "pool")]
            payout: hello.payout.filter(|_| state.books.is_some()),
            #[cfg(not(feature = "pool"))]
            payout: None,
            shares: 0,
            share_difficulty: 0,
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
    #[cfg(feature = "pool")]
    let mut stood = std::time::Instant::now();

    loop {
        #[cfg(feature = "pool")]
        if stood.elapsed() >= STANDING_EVERY {
            stood = std::time::Instant::now();

            if let Some(standing) = state.standing(range)
                && State::lock(sender).send(&standing).is_err()
            {
                return;
            }
        }

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
                let verdict = state.take(job, nonce, number);

                if let Some(worker) = State::lock(&state.workers).get_mut(&number) {
                    match verdict {
                        Verdict::Took => worker.found += 1,
                        Verdict::Share => {}
                        _ => worker.refused += 1,
                    }
                }

                match verdict {
                    Verdict::Took => log::info!("[mining] worker {name} found the seal for job {job}"),
                    Verdict::Share => log::debug!("[mining] worker {name} sent a share for job {job}"),
                    Verdict::Stale => log::debug!("[mining] worker {name} sent a nonce for job {job}, which is over"),
                    Verdict::Short => log::warn!("[mining] worker {name} sent a nonce for job {job} that does not meet its difficulty"),
                    Verdict::Twice => log::warn!("[mining] worker {name} sent a share for job {job} twice"),
                }

                let took = matches!(verdict, Verdict::Took | Verdict::Share);
                let (kind, why) = match verdict {
                    Verdict::Took => ("block", ""),
                    Verdict::Share => ("share", ""),
                    Verdict::Stale => ("", "the block was found already, or the template moved on"),
                    Verdict::Short => ("", "it does not meet the difficulty"),
                    Verdict::Twice => ("", "it was sent already"),
                };
                State::lock(sender).send(&json!({ "type": "verdict", "job": job, "nonce": nonce, "took": took, "kind": kind, "why": why }))?;
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

                #[cfg(feature = "pool")]
                if let Some(books) = &state.books
                    && let Some(worker) = State::lock(&books.workers).get_mut(&number)
                {
                    worker.said(devices.iter().map(|device| device.rate).sum());
                }

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
