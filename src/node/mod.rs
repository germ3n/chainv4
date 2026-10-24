//! The node's main loop, shaped like the engine's server loop: one owner of all node state, the
//! network on its own thread behind channels, and a loop that drains what arrived, produces a
//! block when it may, and sleeps until the next slot or the next event. A producer that seals at
//! once, like an authority, produces in the loop; a miner gets a template, and its seal arrives
//! later as an event. A validating node also votes on checkpoints as they get deep enough, and
//! counts the votes it hears. Catching up with peers lives in sync.rs.

use {
    crate::chain::{Chain, ChainError, FinalityError, FinalityUpdate, Genesis, ImportError, Imported},
    crate::consensus::Producer,
    crate::crypto::keypair::Keypair,
    crate::db::{Database, DbError},
    crate::mempool::{Mempool, MempoolError},
    crate::net::{MAX_SYNC_BLOCKS, MAX_SYNC_JUSTIFICATIONS, NetCommand, NetEvent, NetLink, SyncRequest, SyncResponse},
    crate::structs::block::Block,
    crate::structs::hash::Hash,
    crate::structs::hex::to_hex,
    crate::structs::tx::SignedTx,
    crate::structs::vote::SignedVote,
    libp2p::{Multiaddr, PeerId},
    r#macro::document,
    report::Peer,
    std::collections::HashMap,
    std::fmt,
    std::sync::Arc,
    std::sync::mpsc::{self, RecvTimeoutError},
    std::time::{Duration, SystemTime, UNIX_EPOCH},
    sync::SyncState,
    tokio::sync::{broadcast, oneshot},
};

mod report;
mod stream;
mod sync;

pub use report::{Counters, Heard, NodeReport, PeerReport};
pub use stream::NodeEvent;

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
    peers: HashMap<PeerId, Peer>,
    sync: SyncState,
    reported: u64,
    /// The voting key, on a node that validates.
    validator: Option<Keypair>,
    /// When the loop first ran; 0 before.
    started: u64,
    /// Where the network listens.
    listening: Vec<Multiaddr>,
    counters: Counters,
    /// Where the node tells what it does, when it streams.
    events: Option<broadcast::Sender<NodeEvent>>,
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
}

impl NodeHandle {
    #[document(
        parent = "NodeHandle",
        summary = "A handle that delivers to whoever reads the other end of inbox, usually a node, and has nothing to stream.",
    )]
    pub fn new(inbox: mpsc::Sender<NetEvent>) -> NodeHandle {
        NodeHandle { inbox, events: None }
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
        returns = "The transaction's id, or why the node refused it or could not be reached.",
    )]
    pub async fn submit(&self, tx: SignedTx) -> Result<Hash, SubmitError> {
        let (reply, verdict) = oneshot::channel();

        self.inbox
            .send(NetEvent::Submitted { tx, reply })
            .map_err(|_| SubmitError::Stopped)?;

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
            peers: HashMap::new(),
            sync: SyncState::default(),
            reported: 0,
            validator: None,
            started: 0,
            listening: Vec::new(),
            counters: Counters::default(),
            events: None,
        })
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
    pub fn handle(&self) -> NodeHandle {
        NodeHandle {
            inbox: self.net.inbox.clone(),
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

        for _ in 0..EVENT_BUDGET {
            let event = match self.held.take() {
                Some(event) => event,
                None => match self.net.events.try_recv() {
                    Ok(event) => event,
                    Err(_) => break,
                },
            };
            self.on_event(event, now)?;
        }

        self.produce_if_due(now)?;
        self.vote_if_due()?;
        self.report_if_due(now);

        Ok(())
    }

    #[document(
        parent = "Node",
        summary = "Admits a transaction submitted to this node and gossips it.",
    )]
    pub fn submit(&mut self, tx: SignedTx) -> Result<(), MempoolError> {
        self.mempool.add(tx.clone(), &self.chain.state(), self.chain.fees())?;
        log::info!("[node] accepted transaction {} from {}", tx.id(self.chain.hash()), tx.sender());
        self.publish(|| NodeEvent::Queued(Arc::new(tx.clone())));
        self.send(NetCommand::PublishTx(tx));

        Ok(())
    }

    fn on_event(&mut self, event: NetEvent, now: u64) -> Result<(), NodeError> {
        match event {
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
            NetEvent::Tx { from, tx } => {
                let id = tx.id(self.chain.hash());
                self.counters.txs_heard += 1;
                self.heard(&from, |heard| heard.txs += 1);

                match self.mempool.add(tx.clone(), &self.chain.state(), self.chain.fees()) {
                    Ok(()) => {
                        log::debug!("[node] queued transaction {id} from {from}");
                        self.publish(|| NodeEvent::Queued(Arc::new(tx)));
                    }
                    Err(err) => {
                        self.counters.txs_refused += 1;
                        log::debug!("[node] refused transaction {id} from {from}: {err}");
                    }
                }
            }
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
                self.peer_failed(peer);
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
                self.peer_failed(peer);
            }
            NetEvent::Submitted { tx, reply } => {
                let id = tx.id(self.chain.hash());
                let verdict = self.submit(tx).map(|()| id);

                match &verdict {
                    Ok(_) => self.counters.txs_submitted += 1,
                    Err(err) => {
                        self.counters.txs_refused += 1;
                        log::info!("[node] refused submitted transaction {id}: {err}");
                    }
                }

                // The client may have stopped waiting; the transaction stands either way.
                let _ = reply.send(verdict);
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
                    "[node] validator {} signed conflicting votes; report it with: chainv4 send slash --first {} --second {}",
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
                let _ = self.mempool.add(tx, &state, self.chain.fees());
            }

            self.mempool.prune(&self.chain.state())?;
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
                    let _ = self.mempool.add(tx, &state, self.chain.fees());
                }
            }
        }

        self.mempool.prune(&self.chain.state())?;

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

        let txs = self.mempool.select(&self.chain.state(), MAX_BLOCK_TXS, &|tx| self.chain.priority(tx))?;
        let proofs = self.chain.unpaid_proofs()?;
        let mut block = self.chain.build(txs, proofs, producer.address(), now)?;

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
                self.mempool.prune(&self.chain.state())?;
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

    fn send(&mut self, command: NetCommand) {
        if matches!(command, NetCommand::Request { .. }) {
            self.counters.requests_sent += 1;
        }

        if !self.net.send(command) {
            log::warn!("[node] the network thread is gone");
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
