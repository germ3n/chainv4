//! The node's main loop, shaped like the engine's server loop: one owner of all node state, the
//! network on its own thread behind channels, and a loop that drains what arrived, produces a
//! block when it may, and sleeps until the next slot or the next event. A producer that seals at
//! once, like an authority, produces in the loop; a miner gets a template, and its seal arrives
//! later as an event. Catching up with peers lives in sync.rs.

use {
    crate::chain::{Chain, ChainError, Genesis, ImportError, Imported},
    crate::consensus::Producer,
    crate::db::{Database, DbError},
    crate::mempool::{Mempool, MempoolError},
    crate::net::{MAX_SYNC_BLOCKS, NetCommand, NetEvent, NetLink, SyncRequest, SyncResponse},
    crate::structs::block::Block,
    crate::structs::hash::Hash,
    crate::structs::tx::SignedTx,
    libp2p::PeerId,
    r#macro::document,
    std::collections::HashSet,
    std::fmt,
    std::sync::Arc,
    std::sync::mpsc::{self, RecvTimeoutError},
    std::time::{Duration, SystemTime, UNIX_EPOCH},
    sync::SyncState,
    tokio::sync::oneshot,
};

mod sync;

/// The most events handled per pass, so a flood cannot starve block production.
const EVENT_BUDGET: usize = 256;
/// The most transactions one block carries.
const MAX_BLOCK_TXS: usize = 1_000;
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
    mempool: Mempool,
    producer: Option<Box<dyn Producer>>,
    /// The block the producer is sealing, until its seal arrives or the tip moves on.
    template: Option<Block>,
    net: NetLink,
    held: Option<NetEvent>,
    peers: HashSet<PeerId>,
    sync: SyncState,
    reported: u64,
}

#[document(
    summary = "Hands transactions to a running node from another thread, such as the HTTP API's, and waits for the mempool's verdict. Cheap to clone.",
    category = "Node",
    see_also = "Node::submitter, NetEvent::Submitted",
)]
#[derive(Clone)]
pub struct Submitter {
    inbox: mpsc::Sender<NetEvent>,
}

impl Submitter {
    #[document(
        parent = "Submitter",
        summary = "A submitter that delivers to whoever reads the other end of inbox, usually a node.",
    )]
    pub fn new(inbox: mpsc::Sender<NetEvent>) -> Submitter {
        Submitter { inbox }
    }

    #[document(
        parent = "Submitter",
        summary = "Submits a transaction: the node checks it, queues it and gossips it.",
        returns = "The transaction's id, or why the node refused it or could not be reached.",
    )]
    pub async fn submit(&self, tx: SignedTx) -> Result<Hash, SubmitError> {
        let (reply, verdict) = oneshot::channel();

        self.inbox
            .send(NetEvent::Submitted { tx, reply })
            .map_err(|_| SubmitError::Stopped)?;

        verdict.await.map_err(|_| SubmitError::Stopped)?.map_err(SubmitError::Refused)
    }
}

#[document(
    summary = "Why a submitted transaction was not taken.",
    category = "Node",
    see_also = "Submitter::submit",
)]
#[derive(Debug, PartialEq, Eq)]
pub enum SubmitError {
    #[document(summary = "The mempool refused it.")]
    Refused(MempoolError),
    #[document(summary = "The node is not running.")]
    Stopped,
}

impl fmt::Display for SubmitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SubmitError::Refused(err) => write!(f, "{err}"),
            SubmitError::Stopped => write!(f, "the node is not running"),
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
}

impl fmt::Display for NodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NodeError::Chain(err) => write!(f, "{err}"),
            NodeError::Db(err) => write!(f, "{err}"),
            NodeError::NetworkStopped => write!(f, "the network stopped"),
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
        let chain = Chain::open(db, genesis).map_err(NodeError::Chain)?;

        Ok(Node {
            chain,
            mempool: Mempool::new(MEMPOOL_CAPACITY),
            producer,
            template: None,
            net,
            held: None,
            peers: HashSet::new(),
            sync: SyncState::default(),
            reported: 0,
        })
    }

    #[document(parent = "Node", summary = "The chain this node follows.")]
    pub fn chain(&self) -> &Chain {
        &self.chain
    }

    #[document(parent = "Node", summary = "The transactions waiting for a block.")]
    pub fn mempool(&self) -> &Mempool {
        &self.mempool
    }

    #[document(
        parent = "Node",
        summary = "A handle other threads use to submit transactions while the node runs.",
    )]
    pub fn submitter(&self) -> Submitter {
        Submitter::new(self.net.inbox.clone())
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
        for _ in 0..EVENT_BUDGET {
            let event = match self.held.take() {
                Some(event) => event,
                None => match self.net.events.try_recv() {
                    Ok(event) => event,
                    Err(_) => break,
                },
            };
            self.handle(event, now)?;
        }

        self.produce_if_due(now)?;
        self.report_if_due(now);

        Ok(())
    }

    #[document(
        parent = "Node",
        summary = "Admits a transaction submitted to this node and gossips it.",
    )]
    pub fn submit(&mut self, tx: SignedTx) -> Result<(), MempoolError> {
        self.mempool.add(tx.clone(), &self.chain.state())?;
        log::info!("[node] accepted transaction {} from {}", tx.id(), tx.sender());
        self.send(NetCommand::PublishTx(tx));

        Ok(())
    }

    fn handle(&mut self, event: NetEvent, now: u64) -> Result<(), NodeError> {
        match event {
            NetEvent::Block { from, block } => {
                let height = block.header.height;

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
                    Err(err) => log::warn!("[node] rejected block {height} from {from}: {err}"),
                }
            }
            NetEvent::Sealed { block, seal } => self.sealed(block, seal, now)?,
            NetEvent::Tx { from, tx } => {
                let id = tx.id();

                match self.mempool.add(tx, &self.chain.state()) {
                    Ok(()) => log::debug!("[node] queued transaction {id} from {from}"),
                    Err(err) => log::debug!("[node] refused transaction {id} from {from}: {err}"),
                }
            }
            NetEvent::Request { peer, id, request } => {
                log::debug!("[node] {peer} asked for {request:?}");
                let response = self.answer(request)?;
                self.send(NetCommand::Respond { id, response });
            }
            NetEvent::Response { peer, response } => self.on_response(peer, response, now)?,
            NetEvent::RequestFailed { peer, error } => {
                log::debug!("[node] request to {peer} failed: {error}");
                self.peer_failed(peer);
            }
            NetEvent::PeerUp(peer) => {
                self.peers.insert(peer);
                log::info!("[node] peer {peer} connected ({} peers)", self.peers.len());
                self.send(NetCommand::Request {
                    peer,
                    request: SyncRequest::Status,
                });
            }
            NetEvent::PeerDown(peer) => {
                self.peers.remove(&peer);
                log::info!("[node] peer {peer} disconnected ({} peers)", self.peers.len());
                self.peer_failed(peer);
            }
            NetEvent::Submitted { tx, reply } => {
                let id = tx.id();
                let verdict = self.submit(tx).map(|()| id);

                if let Err(err) = &verdict {
                    log::info!("[node] refused submitted transaction {id}: {err}");
                }

                // The client may have stopped waiting; the transaction stands either way.
                let _ = reply.send(verdict);
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
        };

        Ok(response)
    }

    /// Logs what an import did and brings the mempool in line: transactions the main chain now
    /// holds leave it, and those a reorg took off come back.
    fn imported(&mut self, block: &Block, imported: Imported, source: &str) -> Result<(), NodeError> {
        let height = block.header.height;

        match imported {
            Imported::Extended(receipts) => {
                log::info!("[node] imported block {height} {} {source} ({} txs)", block.id(), receipts.len());
            }
            Imported::Side => log::info!("[node] stored side block {height} {} {source}", block.id()),
            Imported::Reorganized { undone, applied } => {
                log::warn!(
                    "[node] reorganized to block {height} {} {source}: took off {} blocks, put on {}",
                    block.id(),
                    undone.len(),
                    applied.len()
                );
                let state = self.chain.state();

                for tx in undone.into_iter().flat_map(|block| block.txs) {
                    // Most will be back in the new branch or used up; the rest wait again.
                    let _ = self.mempool.add(tx, &state);
                }
            }
        }

        self.mempool.prune(&self.chain.state())?;

        Ok(())
    }

    /// Builds a block for the producer once it may produce, and starts sealing it. A miner keeps
    /// one template at a time, rebuilt when the tip moves or TEMPLATE_REFRESH passes.
    fn produce_if_due(&mut self, now: u64) -> Result<(), NodeError> {
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

        if now < producer.start_at(&tip) {
            return Ok(());
        }

        let txs = self.mempool.select(&self.chain.state(), MAX_BLOCK_TXS)?;
        let mut block = self.chain.build(txs, producer.address(), now);

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
                log::info!("[node] produced block {height} {} ({} txs)", block.id(), receipts.len());
                self.mempool.prune(&self.chain.state())?;
                self.send(NetCommand::PublishBlock(block));
            }
            Ok(other) => log::warn!("[node] its own block {height} did not extend the tip: {other:?}"),
            Err(ImportError::Db(err)) => return Err(NodeError::Db(err)),
            Err(err) => log::error!("[node] could not import its own block {height}: {err}"),
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
            self.mempool.len()
        );
    }

    /// Sleeps until the next slot, waking early when an event arrives.
    fn wait(&mut self, now: u64) -> Result<(), NodeError> {
        if self.net.stopped() {
            return Err(NodeError::NetworkStopped);
        }

        if self.held.is_some() {
            return Ok(());
        }

        let wake = match (&self.producer, &self.template) {
            (Some(_), Some(template)) => Some(template.header.timestamp + TEMPLATE_REFRESH),
            (Some(producer), None) => Some(producer.start_at(self.chain.tip())),
            (None, _) => None,
        };
        let wait = match wake {
            Some(wake) => Duration::from_millis(wake.saturating_sub(now)).min(IDLE_WAIT),
            None => IDLE_WAIT,
        };

        match self.net.events.recv_timeout(wait) {
            Ok(event) => self.held = Some(event),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Err(NodeError::NetworkStopped),
        }

        Ok(())
    }

    fn send(&self, command: NetCommand) {
        if !self.net.send(command) {
            log::warn!("[node] the network thread is gone");
        }
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
