//! The node's main loop, shaped like the engine's server loop: one owner of all node state, the
//! network on its own thread behind channels, and a loop that drains what arrived, produces a
//! block when it may, and sleeps until the next slot or the next event. A producer that seals at
//! once, like an authority, produces in the loop; a miner gets a template, and its seal arrives
//! later as an event. A validating node also votes on checkpoints as they get deep enough, and
//! counts the votes it hears. Catching up with peers lives in sync.rs.

use {
    crate::chain::{Chain, ChainError, FinalityError, FinalityUpdate, Genesis, ImportError, Imported, IndexKind, KeepFrom},
    crate::consensus::{ConsensusConfig, Producer},
    crate::crypto::keypair::Keypair,
    crate::db::{Database, DbError},
    crate::mempool::{Budget, Mempool, MempoolError},
    crate::net::{Admission, Backlog, MAX_SYNC_BLOCKS, MAX_SYNC_ENTRIES, MAX_SYNC_FINALITY, MAX_SYNC_HEADERS, MAX_SYNC_JUSTIFICATIONS, NetCommand, NetEvent, NetLink, Origin, SyncRequest, SyncResponse},
    crate::structs::address::Address,
    crate::structs::block::Block,
    crate::structs::hash::Hash,
    crate::structs::hex::to_hex,
    crate::structs::tx::SignedTx,
    crate::structs::vote::SignedVote,
    libp2p::{Multiaddr, PeerId},
    r#macro::document,
    report::Peer,
    std::collections::{HashMap, HashSet},
    std::fmt,
    std::sync::Arc,
    std::sync::mpsc::{self, RecvTimeoutError},
    std::time::{Duration, Instant, SystemTime, UNIX_EPOCH},
    sync::SyncState,
    tokio::sync::{broadcast, mpsc as async_mpsc, oneshot},
    gate::{Gate, Tally},
    verifier::Arrival,
};

mod gate;
mod report;
mod snapshot;
mod stream;
mod sync;
mod verifier;

pub use crate::mining::pool::MiningPort;
pub use report::{Counters, Heard, NodeReport, PeerReport};
pub use stream::NodeEvent;

/// The most events handled per pass, so a flood cannot starve block production.
const EVENT_BUDGET: usize = 256;
/// The most transactions a block carries on a chain that bounds blocks by neither bytes nor gas,
/// so its producer never tries to run the whole mempool at once.
const UNBOUNDED_CHAIN_TXS: usize = 1_000;
/// The most transactions waiting at once.
const MEMPOOL_CAPACITY: usize = 50_000;
/// The longest the loop sleeps with nothing scheduled, so it still notices a dead network.
const IDLE_WAIT: Duration = Duration::from_secs(1);
/// How often the node logs where it stands, in milliseconds.
const REPORT_EVERY: u64 = 30_000;
/// How long a miner works on one template before it is rebuilt with newer transactions and a
/// newer timestamp, in milliseconds.
const TEMPLATE_REFRESH: u64 = 2_000;

#[document(
    summary = "A running node: the chain, the mempool, the producer if it produces, and the network, owned in one place. Tests can run several in one process.",
    category = "Node",
    see_also = "Chain, Mempool, Producer, NetLink",
)]
pub struct Node {
    chain: Chain,
    /// The mempool, and what admits transactions to it off the loop.
    gate: Arc<Gate>,
    producer: Option<Box<dyn Producer>>,
    /// What this node can produce with, when it builds its producer for whatever consensus the
    /// chain runs, and the consensus the producer it holds serves.
    producing: Option<Producing>,
    producer_for: Option<ConsensusConfig>,
    /// The block the producer is sealing, until its seal arrives or the tip moves on.
    template: Option<Block>,
    /// What the node asks the network to do.
    commands: async_mpsc::UnboundedSender<NetCommand>,
    /// The way into arrivals for local clients, which handles hand out.
    inbox: mpsc::Sender<NetEvent>,
    arrivals: Arrivals,
    held: Option<Arrival>,
    /// What waits for the node: gossip and submissions are counted out here once handled.
    backlog: Arc<Backlog>,
    peers: HashMap<PeerId, Peer>,
    sync: SyncState,
    reported: u64,
    /// The voting key, on a node that validates.
    validator: Option<Keypair>,
    /// When the loop first ran; 0 before.
    started: u64,
    /// The tip the chain's bootnodes were last read at, and the ones the network was told.
    bootnodes_at: Option<Hash>,
    bootnodes: Vec<String>,
    /// Where the network listens.
    listening: Vec<Multiaddr>,
    counters: Counters,
    /// Where the node tells what it does, when it streams.
    events: Option<broadcast::Sender<NodeEvent>>,
    /// The indexes with catching up left, which keeps the loop from sleeping.
    catching_up: HashSet<IndexKind>,
    /// The most transactions this node puts in a block it produces, if it caps them.
    max_block_txs: Option<usize>,
    /// How long this node runs transactions for a block it builds before it seals what it has, if
    /// it bounds that.
    max_build_ms: Option<u64>,
}

#[document(
    summary = "What a node can produce blocks with: a miner's settings for proof of work, a key for the schemes that sign, or both, so the node serves whatever consensus the chain runs now, and whatever governance moves it to. Node::producing takes one.",
    category = "Node",
    see_also = "Node::producing, Mining, Producer",
)]
pub struct Producing {
    #[document(summary = "How to mine, under proof of work, or None to never mine.")]
    pub mine: Option<Mining>,
    #[document(summary = "The key to sign blocks with under an authority, a rotating authority or production by stake: the authority's key, or a signer's or validator's voting key. None to never sign.")]
    pub sign: Option<Keypair>,
}

#[document(
    summary = "How a node mines, under proof of work: on its own CPU threads or GPUs, with workers over its mining port, or both.",
    category = "Node",
    see_also = "Producing, Miner, GpuMiner, Pool",
)]
#[derive(Clone, Debug)]
pub struct Mining {
    #[document(summary = "Where the rewards go.")]
    pub reward_to: Address,
    #[document(summary = "How many threads mine; 0 for none, when workers mine for the node instead.")]
    pub threads: usize,
    #[document(summary = "Whether to mine on the GPU instead, in a build with the gpu feature.")]
    pub gpu: bool,
    #[document(summary = "The GPUs to mine on, by their place in chainv4_gpu::list; None for every one.")]
    pub gpus: Option<Vec<usize>>,
    #[document(summary = "The mining port to take workers on, or None to take none.")]
    pub port: Option<MiningPort>,
}

impl Mining {
    /// The producer for proof of work: the node's own miner, a pool for workers, or a pool with
    /// the node's own miner in it.
    fn producer(&self, hash: crate::structs::hash::HashType, found: impl Fn(Hash, crate::structs::block::Seal) + Send + Sync + 'static) -> Result<Box<dyn Producer>, NodeError> {
        use crate::mining::pool::{Found, Pool};

        let found: Found = Arc::new(found);
        let pool = match &self.port {
            Some(port) => Some(Pool::new(self.reward_to, hash, port.clone(), found.clone()).map_err(NodeError::Thread)?),
            None => None,
        };
        // The node's own miner, its seals going through the pool when there is one.
        let finder: Found = match &pool {
            Some(pool) => Arc::new(pool.finder()),
            None => found,
        };
        let local = self.local(hash, move |id, seal| finder(id, seal))?;

        match (pool, local) {
            (Some(pool), Some(local)) => Ok(Box::new(pool.mining_too(local))),
            (Some(pool), None) => {
                log::info!("[node] mining with workers only, rewards to {}", self.reward_to);

                Ok(Box::new(pool))
            }
            (None, Some(local)) => Ok(local),
            (None, None) => Err(NodeError::Thread("no threads to mine on and no mining port for workers: give --mine-threads 1 or more, or --mining".to_string())),
        }
    }

    /// The node's own miner, on its GPUs or CPU threads; None for no threads.
    fn local(&self, hash: crate::structs::hash::HashType, found: impl Fn(Hash, crate::structs::block::Seal) + Send + Sync + 'static) -> Result<Option<Box<dyn Producer>>, NodeError> {
        if self.gpu {
            #[cfg(feature = "gpu")]
            {
                log::info!("[node] mining on the GPU, rewards to {}", self.reward_to);

                return Ok(Some(Box::new(crate::consensus::GpuMiner::new(self.reward_to, hash, self.gpus.as_deref(), found).map_err(NodeError::Thread)?)));
            }
            #[cfg(not(feature = "gpu"))]
            {
                let _ = found;

                return Err(NodeError::Thread("--mine-gpu needs a build with the gpu feature: cargo build --features gpu".to_string()));
            }
        }

        if self.threads == 0 {
            return Ok(None);
        }

        log::info!("[node] mining on {} threads, rewards to {}", self.threads, self.reward_to);

        Ok(Some(Box::new(crate::consensus::Miner::new(self.reward_to, hash, self.threads, found))))
    }
}

impl Producing {
    /// The producer for a consensus, when the setup can serve it: None when the chain runs a
    /// scheme the setup has nothing for, or the key is not the authority's.
    fn build(
        &self,
        config: &ConsensusConfig,
        hash: crate::structs::hash::HashType,
        checkpoint_interval: u64,
        finality: Option<&crate::chain::FinalityConfig>,
        found: impl Fn(Hash, crate::structs::block::Seal) + Send + Sync + 'static,
    ) -> Result<Option<Box<dyn Producer>>, NodeError> {
        use crate::consensus::{AuthorityProducer, RotatingProducer, Staked, StakedProducer};

        Ok(match config {
            ConsensusConfig::ProofOfWork(_) => match &self.mine {
                Some(mining) => Some(mining.producer(hash, found)?),
                None => {
                    log::info!("[node] the chain runs proof of work, which this node has no --mine for: following only");

                    None
                }
            },
            ConsensusConfig::Authority { key, slot_ms } => match &self.sign {
                Some(keypair) if keypair.public() == *key => {
                    log::info!("[node] producing blocks as {}", keypair.public().address());

                    Some(Box::new(AuthorityProducer::new(keypair.clone(), *slot_ms)))
                }
                Some(keypair) => {
                    log::warn!("[node] the chain's authority is {key}, not {}: following only", keypair.public());

                    None
                }
                None => {
                    log::info!("[node] the chain runs an authority, which this node has no --produce for: following only");

                    None
                }
            },
            ConsensusConfig::Rotating { slot_ms } => match &self.sign {
                Some(keypair) => {
                    // The signer's voting key signs its blocks; the blocks name its account.
                    let account = finality.and_then(|finality| finality.signer_account(&keypair.public())).unwrap_or_else(|| keypair.public().address());
                    log::info!("[node] producing blocks in turn as {}, for {account}", keypair.public());

                    Some(Box::new(RotatingProducer::new(keypair.clone(), account, *slot_ms)))
                }
                None => {
                    log::info!("[node] the chain runs a rotating authority, which this node has no --produce for: following only");

                    None
                }
            },
            ConsensusConfig::Staked { slot_ms, schedule } => match &self.sign {
                Some(keypair) => {
                    log::info!("[node] producing blocks by stake as {}", keypair.public());

                    Some(Box::new(StakedProducer::new(keypair.clone(), Staked::new(*slot_ms, *schedule, hash, checkpoint_interval))))
                }
                None => {
                    log::info!("[node] the chain runs production by stake, which this node has no --produce for: following only");

                    None
                }
            },
        })
    }
}

#[document(
    summary = "Another thread's way to a running node, such as the HTTP API's: it submits transactions and waits for the mempool's verdict, asks how the node is doing, and follows what it does when it streams. Cheap to clone.",
    category = "Node",
    see_also = "Node::handle, NetEvent::Submitted, NetEvent::Report, NodeEvent",
)]
#[derive(Clone)]
pub struct NodeHandle {
    inbox: mpsc::Sender<NetEvent>,
    events: Option<broadcast::Sender<NodeEvent>>,
    /// Where submissions are counted in, so a client cannot flood the node either.
    backlog: Arc<Backlog>,
}

impl NodeHandle {
    #[document(
        parent = "NodeHandle",
        summary = "A handle that delivers to whoever reads the other end of inbox, usually a node, and has nothing to stream. Submissions are counted into backlog, and whoever reads inbox counts them out.",
    )]
    pub fn new(inbox: mpsc::Sender<NetEvent>, backlog: Arc<Backlog>) -> NodeHandle {
        NodeHandle { inbox, events: None, backlog }
    }

    #[document(
        parent = "NodeHandle",
        summary = "Starts hearing what the node does from now on.",
        returns = "The events, or None when the node does not stream.",
    )]
    pub fn subscribe(&self) -> Option<broadcast::Receiver<NodeEvent>> {
        self.events.as_ref().map(broadcast::Sender::subscribe)
    }

    #[document(
        parent = "NodeHandle",
        summary = "Submits a transaction: the node checks it, queues it and gossips it.",
        returns = "The transaction's id, or why the node refused it, has no room for it now, or could not be reached.",
    )]
    pub async fn submit(&self, tx: SignedTx) -> Result<Hash, SubmitError> {
        if self.backlog.admit(Origin::Local, Instant::now()) != Admission::Admitted {
            return Err(SubmitError::Busy);
        }

        let (reply, verdict) = oneshot::channel();

        if self.inbox.send(NetEvent::Submitted { tx, reply }).is_err() {
            self.backlog.release(Origin::Local);

            return Err(SubmitError::Stopped);
        }

        verdict.await.map_err(|_| SubmitError::Stopped)?.map_err(SubmitError::Refused)
    }

    #[document(
        parent = "NodeHandle",
        summary = "Asks the node how it is doing: its peers, what it heard and did, and its mempool.",
        params = { txs = "How many of the transactions waiting to include, first by sender and nonce." },
        returns = "The report, or Stopped when the node is not running.",
    )]
    pub async fn report(&self, txs: usize) -> Result<NodeReport, SubmitError> {
        let (reply, report) = oneshot::channel();

        self.inbox
            .send(NetEvent::Report { txs, reply })
            .map_err(|_| SubmitError::Stopped)?;

        report.await.map_err(|_| SubmitError::Stopped)
    }
}

#[document(
    summary = "Why a submitted transaction was not taken.",
    category = "Node",
    see_also = "NodeHandle::submit",
)]
#[derive(Debug, PartialEq, Eq)]
pub enum SubmitError {
    #[document(summary = "The mempool refused it.")]
    Refused(MempoolError),
    #[document(summary = "The node is not running.")]
    Stopped,
    #[document(summary = "The node has as much waiting as its backlog allows; try again shortly.")]
    Busy,
}

impl fmt::Display for SubmitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SubmitError::Refused(err) => write!(f, "{err}"),
            SubmitError::Stopped => write!(f, "the node is not running"),
            SubmitError::Busy => write!(f, "the node is busy; try again shortly"),
        }
    }
}

#[document(
    summary = "Why the node stopped.",
    category = "Node",
)]
#[derive(Debug)]
pub enum NodeError {
    #[document(summary = "The chain could not be opened.")]
    Chain(ChainError),
    #[document(summary = "The database failed.")]
    Db(DbError),
    #[document(summary = "The network thread stopped.")]
    NetworkStopped,
    #[document(summary = "A thread the node needs, such as its verifier, could not start.")]
    Thread(String),
}

impl fmt::Display for NodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NodeError::Chain(err) => write!(f, "{err}"),
            NodeError::Db(err) => write!(f, "{err}"),
            NodeError::NetworkStopped => write!(f, "the network stopped"),
            NodeError::Thread(why) => write!(f, "a thread would not start: {why}"),
        }
    }
}

impl std::error::Error for NodeError {}

impl From<DbError> for NodeError {
    fn from(err: DbError) -> NodeError {
        NodeError::Db(err)
    }
}

impl Node {
    #[document(
        parent = "Node",
        summary = "Opens the chain and gets ready to run.",
        params = {
            db = "Where the chain lives.",
            genesis = "The chain to follow. Its consensus decides which blocks are valid.",
            producer = "How this node produces blocks, or None for a node that only follows.",
            net = "The network, from spawn, or NetLink::pair in tests.",
        },
    )]
    pub fn new(
        db: Arc<dyn Database>,
        genesis: &Genesis,
        producer: Option<Box<dyn Producer>>,
        net: NetLink,
    ) -> Result<Node, NodeError> {
        let chain = Chain::open(db.clone(), genesis).map_err(NodeError::Chain)?;
        let NetLink {
            commands,
            events,
            inbox,
            check_inline,
            backlog,
        } = net;
        let gate = Arc::new(Gate::new(Mempool::new(MEMPOOL_CAPACITY), db, chain.shared_fees(), chain.hash(), chain.genesis(), commands.clone(), backlog.clone()));
        // The verifier takes the network's events, has the gate admit transactions, and the node
        // reads the rest and what came of them.
        let arrivals = match check_inline {
            true => Arrivals::Unchecked(events),
            false => Arrivals::Checked(
                verifier::spawn(events, chain.workers(), gate.clone()).map_err(|err| NodeError::Thread(format!("the verifier: {err}")))?,
            ),
        };

        Ok(Node {
            chain,
            gate,
            producer,
            producing: None,
            producer_for: None,
            template: None,
            commands,
            inbox,
            arrivals,
            held: None,
            backlog,
            peers: HashMap::new(),
            sync: SyncState::default(),
            reported: 0,
            validator: None,
            started: 0,
            bootnodes_at: None,
            bootnodes: Vec::new(),
            listening: Vec::new(),
            counters: Counters::default(),
            events: None,
            catching_up: HashSet::new(),
            max_block_txs: None,
            max_build_ms: None,
        })
    }

    #[document(
        parent = "Node",
        summary = "Produces with whatever the setup can, for whatever consensus the chain runs: the node builds its producer from the setup for the consensus of each block it is about to build, and again when governance changes it, so a node started with --mine and --produce follows a chain from proof of work into production by stake. What new was given as the producer is replaced.",
        params = { setup = "The keys and settings to produce with." },
    )]
    pub fn producing(mut self, setup: Producing) -> Node {
        self.producing = Some(setup);
        self.producer = None;
        self.producer_for = None;
        self
    }

    /// Builds the producer for the next block's consensus, when the node has a setup to build
    /// from and holds none for that consensus yet.
    fn ensure_producer(&mut self) -> Result<(), NodeError> {
        let Some(setup) = &self.producing else {
            return Ok(());
        };
        let config = self.chain.consensus_config_at(self.chain.tip().height + 1)?;

        if self.producer_for.as_ref() == Some(&config) {
            return Ok(());
        }

        let inbox = self.inbox.clone();
        let found = move |block, seal| {
            let _ = inbox.send(NetEvent::Sealed { block, seal });
        };
        let interval = self.chain.finality_config().map_or(1, |finality| finality.checkpoint_interval());
        // The old producer goes first: its threads stop, and a pool lets go of its mining port for
        // the new one to take.
        self.producer = None;
        self.producer = setup.build(&config, self.chain.hash(), interval, self.chain.finality_config(), found)?;
        self.producer_for = Some(config);
        self.template = None;

        Ok(())
    }

    #[document(
        parent = "Node",
        summary = "Caps the transactions in each block this node produces. Without a cap, blocks fill to the chain's max_block_bytes and gas limit; a chain that bounds neither takes 1,000 a block.",
        params = { most = "The most, or None for no cap of this node's own." },
    )]
    pub fn max_block_txs(mut self, most: Option<usize>) -> Node {
        self.max_block_txs = most;
        self
    }

    #[document(
        parent = "Node",
        summary = "Bounds how long this node spends running transactions for each block it builds. Once the time is up, the transactions that went in by then are the block, and the rest wait for the next; the bytes and gas limits still apply. A brake for a slow machine, so it does not miss its slot: none by default, so blocks fill to the chain's limits whatever they take to run.",
        params = { most = "Milliseconds per block, or None for no bound." },
    )]
    pub fn max_build_ms(mut self, most: Option<u64>) -> Node {
        self.max_build_ms = most;
        self
    }

    /// How much the producer takes from the mempool for a block: whatever the chain's limits let
    /// in, with the gas limits allowed twice the block's, since few use all of theirs.
    fn budget(&self) -> Budget {
        let bytes = self.chain.max_block_bytes();
        let gas = self.chain.fees().map(|fees| fees.block_gas_limit().saturating_mul(2));
        let unbounded = (bytes.is_none() && gas.is_none()).then_some(UNBOUNDED_CHAIN_TXS);

        Budget { count: self.max_block_txs.or(unbounded), bytes, gas }
    }

    #[document(
        parent = "Node",
        summary = "Makes the node vote on finality with a validator's voting key. It votes only in epochs where that key is in the validator set, and never signs a vote that conflicts with one it signed before.",
        params = { voter = "The voting key named in the validator's bond." },
    )]
    pub fn validate(mut self, voter: Keypair) -> Node {
        self.validator = Some(voter);
        self
    }

    #[document(
        parent = "Node",
        summary = "Makes the node keep one of the indexes. Its loop then catches the index up a step at a time, in between everything else. Once on, the database keeps it on.",
        params = {
            kind = "Which index.",
            keep = "Which blocks to keep; None keeps what the database keeps, or every block for a new index.",
        },
        returns = "The node, or the database failure that kept it from starting.",
        see_also = "Chain::keep_index",
    )]
    pub fn keep_index(mut self, kind: IndexKind, keep: Option<KeepFrom>) -> Result<Node, NodeError> {
        let window = self.chain.keep_index(kind, keep)?;

        if !window.caught_up(self.chain.tip().height) {
            self.catching_up.insert(kind);
        }

        log::info!("[node] keeping the {kind} of {}, held from height {} so far", window.keep, window.from);

        Ok(self)
    }

    #[document(
        parent = "Node",
        summary = "Makes the node keep none of one index, deleting the one its database holds.",
        params = { kind = "Which index." },
        returns = "The node, or the database failure that kept it from starting.",
        see_also = "Chain::drop_index",
    )]
    pub fn drop_index(mut self, kind: IndexKind) -> Result<Node, NodeError> {
        self.catching_up.remove(&kind);

        match self.chain.drop_index(kind)? {
            true => log::info!("[node] dropped the {kind}"),
            false => log::info!("[node] there is no {kind} to drop"),
        }

        Ok(self)
    }

    #[document(parent = "Node", summary = "The chain this node follows.")]
    pub fn chain(&self) -> &Chain {
        &self.chain
    }

    #[document(parent = "Node", summary = "The transactions waiting for a block, locked while the guard lives: the gate admits more off the loop.")]
    pub fn mempool(&self) -> std::sync::MutexGuard<'_, Mempool> {
        self.gate.mempool()
    }

    #[document(
        parent = "Node",
        summary = "A handle other threads use to submit transactions while the node runs.",
    )]
    pub fn handle(&self) -> NodeHandle {
        NodeHandle {
            inbox: self.inbox.clone(),
            backlog: self.backlog.clone(),
            events: self.events.clone(),
        }
    }

    #[document(
        parent = "Node",
        summary = "Runs until something fatal happens: the database fails or the network stops.",
    )]
    pub fn run(mut self) -> Result<(), NodeError> {
        log::info!(
            "[node] following {} at height {}",
            self.chain.genesis(),
            self.chain.tip().height
        );

        loop {
            self.step(now_ms())?;
            self.wait(now_ms())?;
        }
    }

    #[document(
        parent = "Node",
        summary = "One pass of the loop without the sleep: handles what arrived, then produces a block if the slot has come. Tests drive the node with this and their own clock.",
        params = { now = "The time, in milliseconds since the Unix epoch." },
    )]
    pub fn step(&mut self, now: u64) -> Result<(), NodeError> {
        if self.started == 0 {
            self.started = now;
        }

        self.gate.set_syncing(self.snapshotting());

        for _ in 0..EVENT_BUDGET {
            let Some(arrival) = self.held.take().or_else(|| self.arrivals.try_next()) else {
                break;
            };
            self.on_event(arrival, now)?;
        }

        if !self.snapshotting() {
            self.produce_if_due(now)?;
            self.vote_if_due()?;
            self.catch_up_indexes()?;
        }

        self.report_if_due(now);
        self.announce_bootnodes()?;

        Ok(())
    }

    /// Tells the network the chain's bootnodes whenever they change: as the node starts, from
    /// genesis, and once a block governance decides others in is imported.
    fn announce_bootnodes(&mut self) -> Result<(), NodeError> {
        let tip = self.chain.tip().id();

        if self.bootnodes_at == Some(tip) {
            return Ok(());
        }

        self.bootnodes_at = Some(tip);
        let bootnodes = self.chain.bootnodes()?;

        if bootnodes == self.bootnodes {
            return Ok(());
        }

        let addresses = bootnodes
            .iter()
            .filter_map(|text| match text.parse::<Multiaddr>() {
                Ok(address) => Some(address),
                Err(err) => {
                    log::warn!("[node] the chain's bootnode {text} is not an address libp2p reads, so it is skipped: {err}");

                    None
                }
            })
            .collect();
        self.send(NetCommand::Bootnodes(addresses));
        self.bootnodes = bootnodes;

        Ok(())
    }

    #[document(
        parent = "Node",
        summary = "Admits a transaction submitted to this node and gossips it.",
    )]
    pub fn submit(&mut self, tx: SignedTx) -> Result<(), MempoolError> {
        self.gate.submit(tx, None).map(|_| ())
    }

    /// Counts what came of transactions the gate took.
    fn count(&mut self, tally: Tally) {
        self.counters.txs_submitted += tally.submitted;
        self.counters.txs_refused += tally.refused;

        for peer in tally.heard {
            self.counters.txs_heard += 1;
            self.heard(&peer, |heard| heard.txs += 1);
        }
    }

    fn on_event(&mut self, arrival: Arrival, now: u64) -> Result<(), NodeError> {
        // Transactions go to the gate: here only on a link that checks inline, whose transactions
        // the verifier never saw.
        let event = match self.gate.take(arrival) {
            Ok(tally) | Err(Arrival::Admitted(tally)) => {
                self.count(tally);

                return Ok(());
            }
            Err(Arrival::Event { event, .. }) => *event,
        };

        // Handled now, whatever comes of it, so it no longer counts against its peer.
        match &event {
            NetEvent::Vote { from, .. } => self.backlog.release(Origin::Peer(*from)),
            NetEvent::PeerDown(peer) => self.backlog.forget(peer),
            _ => {}
        }

        match event {
            // While it takes a snapshot its state is half the pivot's: nothing can be run on it.
            NetEvent::Block { from, .. } | NetEvent::Vote { from, .. } if self.snapshotting() => {
                log::debug!("[node] ignored gossip from {from} while syncing from a snapshot");
            }
            NetEvent::Block { from, block } => {
                let height = block.header.height;
                self.counters.blocks_heard += 1;
                self.heard(&from, |heard| heard.blocks += 1);

                match self.chain.import(&block, now) {
                    Ok(imported) => self.imported(&block, imported, &format!("from {from}"))?,
                    Err(ImportError::Db(err)) => return Err(NodeError::Db(err)),
                    Err(ImportError::Known) => {}
                    Err(ImportError::UnknownParent(_)) => {
                        // Its status says how far it got and how much work that holds.
                        log::debug!("[node] block {height} from {from} does not connect yet");
                        self.send(NetCommand::Request {
                            peer: from,
                            request: SyncRequest::Status,
                        });
                    }
                    Err(err) => {
                        self.counters.blocks_rejected += 1;
                        log::warn!("[node] rejected block {height} from {from}: {err}");
                    }
                }
            }
            NetEvent::Sealed { block, seal } => self.sealed(block, seal, now)?,
            NetEvent::Vote { from, vote } => {
                self.counters.votes_heard += 1;
                self.heard(&from, |heard| heard.votes += 1);

                match self.chain.add_vote(vote) {
                    Ok(update) => self.finality_changed(update, &format!("from {from}"))?,
                    Err(FinalityError::Db(err)) => return Err(NodeError::Db(err)),
                    Err(err) => log::debug!("[node] ignored a vote from {from}: {err}"),
                }
            }
            NetEvent::Tx { .. } | NetEvent::Submitted { .. } => unreachable!("the gate takes every transaction"),
            NetEvent::Request { peer, id, request } => {
                log::debug!("[node] {peer} asked for {request:?}");
                self.counters.requests_served += 1;
                let response = self.answer(request)?;
                self.send(NetCommand::Respond { id, response });
            }
            NetEvent::Response { peer, response } => self.on_response(peer, response, now)?,
            NetEvent::RequestFailed { peer, error } => {
                log::debug!("[node] request to {peer} failed: {error}");
                self.counters.requests_failed += 1;
                self.peer_failed(peer)?;
            }
            NetEvent::PeerUp { peer, address } => {
                self.counters.connections += 1;
                self.publish(|| NodeEvent::PeerUp {
                    peer,
                    address: address.clone(),
                });
                self.peers.insert(peer, Peer {
                    address,
                    since: now,
                    heard: Heard::default(),
                });
                log::info!("[node] peer {peer} connected ({} peers)", self.peers.len());
                self.send(NetCommand::Request {
                    peer,
                    request: SyncRequest::Status,
                });
            }
            NetEvent::PeerDown(peer) => {
                self.peers.remove(&peer);
                self.publish(|| NodeEvent::PeerDown(peer));
                log::info!("[node] peer {peer} disconnected ({} peers)", self.peers.len());
                self.peer_failed(peer)?;
            }
            NetEvent::Listening(address) => self.listening.push(address),
            NetEvent::Report { txs, reply } => {
                let _ = reply.send(self.report(now, txs));
            }
        }

        Ok(())
    }

    /// What this node tells a syncing peer.
    fn answer(&self, request: SyncRequest) -> Result<SyncResponse, NodeError> {
        let response = match request {
            SyncRequest::Status => SyncResponse::Status {
                genesis: self.chain.genesis(),
                tip: self.chain.tip().id(),
                height: self.chain.tip().height,
                work: self.chain.tip_work(),
                justified: self.chain.justified().map_or(0, |justified| justified.epoch),
            },
            SyncRequest::Blocks { from, count } => {
                let mut blocks = Vec::new();

                for height in from..from.saturating_add(u64::from(count.min(MAX_SYNC_BLOCKS))) {
                    match self.chain.block_at(height)? {
                        Some(block) => blocks.push(block),
                        None => break,
                    }
                }

                SyncResponse::Blocks(blocks)
            }
            SyncRequest::Justifications { from, count } => SyncResponse::Justifications(
                self.chain
                    .justifications_from(from, count.min(MAX_SYNC_JUSTIFICATIONS) as usize)?,
            ),
            SyncRequest::Headers { from, count } => SyncResponse::Headers(self.chain.headers_from(from, count.min(MAX_SYNC_HEADERS))?),
            SyncRequest::State { block, depth, prefix } => SyncResponse::State(match self.snapshotting() {
                // Its own state is half a pivot's while it takes one.
                true => None,
                false => self.chain.state_part(&block, depth, &prefix, MAX_SYNC_ENTRIES)?,
            }),
            SyncRequest::Checkpoint { block } => SyncResponse::Checkpoint(
                self.chain
                    .checkpoint_answer(&block)?
                    .map(|(start, work)| crate::net::CheckpointAnswer { start, work }),
            ),
            SyncRequest::Finality { from, count } => SyncResponse::Finality(self.chain.light_finality_from(from, u64::from(count.min(MAX_SYNC_FINALITY)))?),
        };

        Ok(response)
    }

    /// Logs what a vote or justification changed, and brings the mempool in line after a reorg.
    fn finality_changed(&mut self, update: FinalityUpdate, source: &str) -> Result<(), NodeError> {
        for checkpoint in &update.justified {
            log::info!("[node] justified epoch {} {} {source}", checkpoint.epoch, checkpoint.block);
            self.publish(|| NodeEvent::Justified(*checkpoint));
        }

        if let Some(finalized) = update.finalized {
            log::info!("[node] finalized epoch {} {} {source}", finalized.epoch, finalized.block);
            self.publish(|| NodeEvent::Finalized(finalized));
        }

        if let Some(evidence) = update.evidence {
            let encode = |vote: &SignedVote| to_hex(&wincode::serialize(vote).expect("votes always encode"));

            match self.chain.has_staking() {
                true => log::warn!(
                    "[node] voter {} signed conflicting votes; report it with: chainv4 send slash --first {} --second {}",
                    evidence.first.vote.voter,
                    encode(&evidence.first),
                    encode(&evidence.second)
                ),
                false => log::warn!(
                    "[node] signer {} signed conflicting votes, which this chain cannot slash: {} and {}",
                    evidence.first.vote.voter,
                    encode(&evidence.first),
                    encode(&evidence.second)
                ),
            }
        }

        if let Some((undone, applied)) = update.reorganized {
            log::warn!(
                "[node] followed a justified branch {source}: took off {} blocks, put on {}",
                undone.len(),
                applied.len()
            );
            self.publish(|| reorganized(&undone, &applied));
            let state = self.chain.state();

            for tx in undone.into_iter().flat_map(|block| block.txs) {
                let _ = self.gate.mempool().add(tx, &self.chain.genesis(), &state, self.chain.fees());
            }

            self.prune_mempool()?;
        }

        Ok(())
    }

    /// Brings the mempool in line with the tip: what landed, what can no longer pay and what has
    /// offered too little for too long leave it.
    fn prune_mempool(&self) -> Result<(), NodeError> {
        let dropped = self.gate.mempool().prune(&self.chain.state(), self.chain.fees(), self.chain.next_base_price())?;

        if dropped > 0 {
            log::debug!("[node] dropped {dropped} transactions that cannot pay or offer too little");
        }

        Ok(())
    }

    /// Signs and sends the vote this node's validator owes, if any.
    fn vote_if_due(&mut self) -> Result<(), NodeError> {
        let Some(keypair) = &self.validator else {
            return Ok(());
        };
        let Some(vote) = self.chain.vote_for(&keypair.public())? else {
            return Ok(());
        };
        // Remembered before it leaves, so a crash can never lead to a conflicting vote.
        self.chain.record_own_vote(&vote)?;
        let signed = SignedVote::sign(vote, keypair, &self.chain.genesis());
        log::info!("[node] voting for epoch {} from epoch {}", vote.target.epoch, vote.source.epoch);
        self.counters.votes_cast += 1;
        self.send(NetCommand::PublishVote(signed));

        match self.chain.add_vote(signed) {
            Ok(update) => self.finality_changed(update, "by this node's vote"),
            Err(FinalityError::Db(err)) => Err(NodeError::Db(err)),
            Err(err) => {
                log::warn!("[node] could not count its own vote: {err}");

                Ok(())
            }
        }
    }

    /// Logs what an import did and brings the mempool in line: transactions the main chain now
    /// holds leave it, and those a reorg took off come back.
    fn imported(&mut self, block: &Block, imported: Imported, source: &str) -> Result<(), NodeError> {
        let height = block.header.height;

        match imported {
            Imported::Extended(receipts) => {
                self.counters.blocks_imported += 1;
                self.publish(|| NodeEvent::Block {
                    block: Arc::new(block.clone()),
                    main: true,
                });
                log::info!("[node] imported block {height} {} {source} ({} txs)", block.id(), receipts.len());
            }
            Imported::Side => {
                self.counters.side_blocks += 1;
                self.publish(|| NodeEvent::Block {
                    block: Arc::new(block.clone()),
                    main: false,
                });
                log::info!("[node] stored side block {height} {} {source}", block.id());
            }
            Imported::Reorganized { undone, applied } => {
                self.counters.reorgs += 1;
                self.counters.blocks_imported += applied.len() as u64;
                self.publish(|| reorganized(&undone, &applied));
                log::warn!(
                    "[node] reorganized to block {height} {} {source}: took off {} blocks, put on {}",
                    block.id(),
                    undone.len(),
                    applied.len()
                );
                let state = self.chain.state();

                for tx in undone.into_iter().flat_map(|block| block.txs) {
                    // Most will be back in the new branch or used up; the rest wait again.
                    let _ = self.gate.mempool().add(tx, &self.chain.genesis(), &state, self.chain.fees());
                }
            }
        }

        self.prune_mempool()?;

        // The main chain moved, so votes held back may count now.
        match self.chain.retry_votes() {
            Ok(update) => self.finality_changed(update, "after an import"),
            Err(FinalityError::Db(err)) => Err(NodeError::Db(err)),
            Err(_) => Ok(()),
        }
    }

    /// Builds a block for the producer once it may produce, and starts sealing it. A miner keeps
    /// one template at a time, rebuilt when the tip moves or TEMPLATE_REFRESH passes.
    fn produce_if_due(&mut self, now: u64) -> Result<(), NodeError> {
        self.ensure_producer()?;
        let budget = self.budget();
        let Some(producer) = self.producer.as_mut() else {
            return Ok(());
        };
        let tip = self.chain.tip().clone();

        if let Some(template) = &self.template
            && template.header.parent == tip.id()
            && now < template.header.timestamp + TEMPLATE_REFRESH
        {
            return Ok(());
        }

        // Under a rotating authority or production by stake, who may produce the next block, and
        // so when this node may.
        let producers = self.chain.producers_for(tip.height + 1)?;

        if now < producer.start_at(&tip, &producers) {
            return Ok(());
        }

        let txs = self.gate.mempool().select(&self.chain.state(), budget, &|tx| self.chain.worth(tx))?;
        let proofs = self.chain.unpaid_proofs()?;
        let deadline = self.max_build_ms.map(|most| Instant::now() + Duration::from_millis(most));
        let declared = producer.difficulty(&tip, &producers);
        let mut block = self.chain.build_declaring(txs, proofs, producer.address(&producers), now, deadline, declared)?;

        match producer.seal(&block.header) {
            Some(seal) => {
                self.template = None;
                block.seal = seal;
                self.finish(block, now)
            }
            None => {
                log::debug!("[node] sealing block {} on {} txs", block.header.height, block.txs.len());
                self.template = Some(block);

                Ok(())
            }
        }
    }

    /// A miner found the seal for a template; if it is still the current one, it becomes a block.
    fn sealed(&mut self, id: Hash, seal: crate::structs::block::Seal, now: u64) -> Result<(), NodeError> {
        let Some(mut block) = self.template.take_if(|template| template.id() == id) else {
            log::debug!("[node] a seal arrived for a stale template");

            return Ok(());
        };

        block.seal = seal;
        self.finish(block, now)
    }

    /// Imports a block this node produced and gossips it.
    fn finish(&mut self, block: Block, now: u64) -> Result<(), NodeError> {
        let height = block.header.height;

        match self.chain.import(&block, now) {
            Ok(Imported::Extended(receipts)) => {
                log::info!(
                    "[node] produced block {height} {} ({} txs, {} proofs)",
                    block.id(),
                    receipts.len(),
                    block.proofs.len()
                );
                self.counters.blocks_produced += 1;
                self.counters.blocks_imported += 1;
                self.publish(|| NodeEvent::Block {
                    block: Arc::new(block.clone()),
                    main: true,
                });
                self.prune_mempool()?;
                self.send(NetCommand::PublishBlock(block));

                match self.chain.retry_votes() {
                    Ok(update) => self.finality_changed(update, "after its own block")?,
                    Err(FinalityError::Db(err)) => return Err(NodeError::Db(err)),
                    Err(_) => {}
                }
            }
            Ok(other) => log::warn!("[node] its own block {height} did not extend the tip: {other:?}"),
            Err(ImportError::Db(err)) => return Err(NodeError::Db(err)),
            Err(err) => log::error!("[node] could not import its own block {height}: {err}"),
        }

        Ok(())
    }

    /// Counts something a connected peer gossiped.
    fn heard(&mut self, peer: &PeerId, count: impl FnOnce(&mut Heard)) {
        if let Some(peer) = self.peers.get_mut(peer) {
            count(&mut peer.heard);
        }
    }

    /// Catches each index up a step, and says so when a long catch-up is done.
    fn catch_up_indexes(&mut self) -> Result<(), NodeError> {
        for kind in IndexKind::ALL {
            let was = self.catching_up.contains(&kind);

            match self.chain.catch_up_index(kind)? {
                true => self.catching_up.insert(kind),
                false => self.catching_up.remove(&kind),
            };

            if was
                && !self.catching_up.contains(&kind)
                && let Some(window) = self.chain.index_window(kind)
            {
                log::info!("[node] the {kind} caught up: it holds blocks from height {}", window.from);
            }
        }

        Ok(())
    }

    /// Logs the height, peers and mempool every REPORT_EVERY, so a quiet node still shows it is alive.
    fn report_if_due(&mut self, now: u64) {
        if now < self.reported.saturating_add(REPORT_EVERY) {
            return;
        }

        self.reported = now;
        log::info!(
            "[node] at height {} {}, {} peers, {} transactions waiting",
            self.chain.tip().height,
            self.chain.tip().id(),
            self.peers.len(),
            self.gate.mempool().len()
        );

        if let Some(snapshot) = &self.sync.snapshot {
            log::info!("[node] syncing from a snapshot: {}", snapshot.stage());
        }

        for &kind in &self.catching_up {
            if let Some(window) = self.chain.index_window(kind) {
                log::info!(
                    "[node] filling the {kind}: held from height {}, catching up to {}",
                    window.from,
                    window.keep.lowest(self.chain.tip().height)
                );
            }
        }
    }

    /// Sleeps until the next slot, waking early when an event arrives.
    fn wait(&mut self, now: u64) -> Result<(), NodeError> {
        if self.commands.is_closed() {
            return Err(NodeError::NetworkStopped);
        }

        if self.held.is_some() || !self.catching_up.is_empty() {
            return Ok(());
        }

        self.ensure_producer()?;
        let wake = match (&self.producer, &self.template) {
            (Some(_), Some(template)) => Some(template.header.timestamp + TEMPLATE_REFRESH),
            (Some(producer), None) => Some(producer.start_at(self.chain.tip(), &self.chain.producers_for(self.chain.tip().height + 1)?)),
            (None, _) => None,
        };
        let wait = match wake {
            Some(wake) => Duration::from_millis(wake.saturating_sub(now)).min(IDLE_WAIT),
            None => IDLE_WAIT,
        };

        match self.arrivals.next_within(wait) {
            Ok(arrival) => self.held = Some(arrival),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Err(NodeError::NetworkStopped),
        }

        Ok(())
    }

    fn send(&mut self, command: NetCommand) {
        if matches!(command, NetCommand::Request { .. }) {
            self.counters.requests_sent += 1;
        }

        if self.commands.send(command).is_err() {
            log::warn!("[node] the network thread is gone");
        }
    }
}

/// Where the node's events come from.
enum Arrivals {
    /// Through the verifier, with transactions' signatures checked.
    Checked(mpsc::Receiver<Arrival>),
    /// Straight from the network, for a link that checks inline.
    Unchecked(mpsc::Receiver<NetEvent>),
}

impl Arrivals {
    /// The next event if one is waiting.
    fn try_next(&self) -> Option<Arrival> {
        match self {
            Arrivals::Checked(arrivals) => arrivals.try_recv().ok(),
            Arrivals::Unchecked(events) => events.try_recv().ok().map(Arrival::unchecked),
        }
    }

    /// The next event, waiting at most this long for one.
    fn next_within(&self, wait: Duration) -> Result<Arrival, RecvTimeoutError> {
        match self {
            Arrivals::Checked(arrivals) => arrivals.recv_timeout(wait),
            Arrivals::Unchecked(events) => events.recv_timeout(wait).map(Arrival::unchecked),
        }
    }
}

/// What subscribers hear of a reorg.
fn reorganized(undone: &[Block], applied: &[Block]) -> NodeEvent {
    NodeEvent::Reorganized {
        undone: undone.iter().map(Block::id).collect(),
        applied: applied.iter().cloned().map(Arc::new).collect(),
    }
}

/// The time, in milliseconds since the Unix epoch.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests;
