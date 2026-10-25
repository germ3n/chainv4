//! Networking. libp2p runs on a thread of its own and talks to the node only through two
//! channels: NetEvent in, NetCommand out. The node never sees libp2p's machinery, and a test can
//! stand in for the whole network by holding the other ends of the channels. Local clients, such
//! as the HTTP API, use the same way in, so one channel wakes the node for everything.

use {
    crate::mempool::MempoolError,
    crate::db::{Path, StatePart},
    crate::structs::block::{Block, Seal, SealedHeader},
    crate::structs::hash::Hash,
    crate::structs::tx::SignedTx,
    crate::structs::vote::{Justification, SignedVote},
    libp2p::{Multiaddr, PeerId},
    r#macro::document,
    std::sync::{Arc, mpsc},
    tokio::sync::mpsc as async_mpsc,
    tokio::sync::oneshot,
    wincode::{SchemaRead, SchemaWrite},
};

mod backlog;
mod codec;
mod swarm;

pub use backlog::{Admission, Backlog, BacklogLimits, MAX_STRIKES, Origin, STRIKE_WINDOW};

pub use swarm::{NetError, spawn};

/// The most blocks one sync response carries.
pub const MAX_SYNC_BLOCKS: u32 = 128;
/// The most justifications one sync response carries.
pub const MAX_SYNC_JUSTIFICATIONS: u32 = 64;
/// The most headers one sync response carries.
pub const MAX_SYNC_HEADERS: u32 = 1_024;
/// The most chain-state entries one sync response carries. Values are at most 64 KiB, so the
/// largest answer still fits a response.
pub const MAX_SYNC_ENTRIES: usize = 512;

#[document(
    summary = "Something that arrived for the node: from a peer, or from a client of this node.",
    category = "Node",
    see_also = "NetCommand, NetLink",
)]
#[derive(Debug)]
pub enum NetEvent {
    #[document(summary = "A block gossiped by a peer.")]
    Block { from: PeerId, block: Block },
    #[document(summary = "A transaction gossiped by a peer.")]
    Tx { from: PeerId, tx: SignedTx },
    #[document(summary = "A finality vote gossiped by a peer.")]
    Vote { from: PeerId, vote: SignedVote },
    #[document(summary = "A peer connected, from or at this address.")]
    PeerUp { peer: PeerId, address: Multiaddr },
    #[document(summary = "A peer's last connection closed.")]
    PeerDown(PeerId),
    #[document(summary = "A peer asked for something. Answer with NetCommand::Respond and the same id.")]
    Request { peer: PeerId, id: u64, request: SyncRequest },
    #[document(summary = "A peer answered a request this node sent.")]
    Response { peer: PeerId, response: SyncResponse },
    #[document(summary = "A request this node sent got no answer.")]
    RequestFailed { peer: PeerId, error: String },
    #[document(summary = "This node's miner found the seal for the block with this id.")]
    Sealed { block: Hash, seal: Seal },
    #[document(summary = "A client of this node submitted a transaction. The node answers on reply with its id, or why the mempool refused it.")]
    Submitted {
        tx: SignedTx,
        reply: oneshot::Sender<Result<Hash, MempoolError>>,
    },
    #[document(summary = "The network listens here now. The address ends in /p2p/ and this node's peer id.")]
    Listening(Multiaddr),
    #[document(summary = "A client of this node asked how it is doing, with up to txs of the transactions waiting. The node answers on reply.")]
    Report {
        txs: usize,
        reply: oneshot::Sender<crate::node::NodeReport>,
    },
}

#[document(
    summary = "Something the node asks the network to do.",
    category = "Node",
    see_also = "NetEvent, NetLink",
)]
#[derive(Debug)]
pub enum NetCommand {
    #[document(summary = "Gossip a block to every peer.")]
    PublishBlock(Block),
    #[document(summary = "Gossip a transaction to every peer.")]
    PublishTx(SignedTx),
    #[document(summary = "Gossip a finality vote to every peer.")]
    PublishVote(SignedVote),
    #[document(summary = "Ask one peer for something.")]
    Request { peer: PeerId, request: SyncRequest },
    #[document(summary = "Answer the request with this id.")]
    Respond { id: u64, response: SyncResponse },
}

#[document(
    summary = "What a node asks a peer for while syncing.",
    category = "Node",
    see_also = "SyncResponse",
)]
#[derive(Clone, Debug, PartialEq, SchemaWrite, SchemaRead)]
#[wincode(tag_encoding = "u8")]
pub enum SyncRequest {
    #[document(summary = "Which chain the peer is on and how far it got.")]
    #[wincode(tag = 0)]
    Status,
    #[document(summary = "Consecutive blocks starting at a height, at most MAX_SYNC_BLOCKS.")]
    #[wincode(tag = 1)]
    Blocks { from: u64, count: u32 },
    #[document(summary = "The justifications the peer holds for its main-chain checkpoints from an epoch on, at most MAX_SYNC_JUSTIFICATIONS.")]
    #[wincode(tag = 2)]
    Justifications { from: u64, count: u32 },
    #[document(summary = "Consecutive main-chain headers with their seals, from a height on, at most MAX_SYNC_HEADERS: what a node syncing from a snapshot checks first.")]
    #[wincode(tag = 3)]
    Headers { from: u64, count: u32 },
    #[document(summary = "One position of the state tree as it was at a recent main-chain block: its entries, at most MAX_SYNC_ENTRIES, or its two halves' hashes.")]
    #[wincode(tag = 4)]
    State { block: Hash, depth: u16, prefix: Path },
}

#[document(
    summary = "A peer's answer to a SyncRequest.",
    category = "Node",
    see_also = "SyncRequest",
)]
#[derive(Clone, Debug, PartialEq, SchemaWrite, SchemaRead)]
#[wincode(tag_encoding = "u8")]
pub enum SyncResponse {
    #[document(summary = "The peer's genesis, tip, height, the total work of its main chain, and the epoch of its latest justified checkpoint, 0 without finality.")]
    #[wincode(tag = 0)]
    Status {
        genesis: Hash,
        tip: Hash,
        height: u64,
        #[wincode(with = "crate::structs::u256::U256Le")]
        work: crate::structs::u256::u256,
        justified: u64,
    },
    #[document(summary = "The blocks asked for that the peer has, in height order.")]
    #[wincode(tag = 1)]
    Blocks(Vec<Block>),
    #[document(summary = "The justifications asked for that the peer holds, oldest first.")]
    #[wincode(tag = 2)]
    Justifications(Vec<Justification>),
    #[document(summary = "The headers asked for that the peer has, in height order.")]
    #[wincode(tag = 3)]
    Headers(Vec<SealedHeader>),
    #[document(summary = "The part of the state tree asked for, or None when the peer cannot serve it: the block is not on its main chain, or older than its undo records reach.")]
    #[wincode(tag = 4)]
    State(Option<StatePart>),
}

#[document(
    summary = "Where and how to network.",
    category = "Node",
    see_also = "spawn",
)]
#[derive(Clone, Debug)]
pub struct NetConfig {
    #[document(summary = "Addresses to listen on, for example /ip4/0.0.0.0/udp/0/quic-v1.")]
    pub listen: Vec<Multiaddr>,
    #[document(summary = "Peers to dial at startup, and again every 15 seconds while no connection to one stands.")]
    pub peers: Vec<Multiaddr>,
    #[document(summary = "Whether to look for peers on the local network with mDNS.")]
    pub mdns: bool,
    #[document(summary = "The chain's hash, which gossip messages are told apart by.")]
    pub hash: crate::structs::hash::HashType,
    #[document(summary = "Names the network in gossip topics, so nodes on different chains ignore each other. Usually derived from the genesis id.")]
    pub network: String,
    #[document(summary = "How much gossip may wait for the node, in all and from any one peer.")]
    pub backlog: BacklogLimits,
}

#[document(
    summary = "The node's end of the network: commands go out, events come in. spawn makes one backed by libp2p; NetLink::pair makes one backed by nothing, for tests.",
    category = "Node",
    see_also = "spawn, Node",
)]
pub struct NetLink {
    pub commands: async_mpsc::UnboundedSender<NetCommand>,
    pub events: mpsc::Receiver<NetEvent>,
    /// Another way into events, for local clients.
    pub inbox: mpsc::Sender<NetEvent>,
    /// Whether the node checks transactions' signatures on its own thread as it handles them,
    /// rather than on a verifier thread in front of it: true for pair, so a test's events are
    /// there the moment it sends them.
    pub check_inline: bool,
    /// What waits for the node, which the network counts in and the node counts out.
    pub backlog: Arc<Backlog>,
}

#[document(
    summary = "The far end of a NetLink made by pair: what the node sends, and a way to hand it events.",
    category = "Node",
)]
pub struct FakeNet {
    pub commands: async_mpsc::UnboundedReceiver<NetCommand>,
    pub events: mpsc::Sender<NetEvent>,
}

impl NetLink {
    #[document(
        parent = "NetLink",
        summary = "A link with no network behind it. The test holds the other end and plays the network.",
    )]
    pub fn pair() -> (NetLink, FakeNet) {
        let (command_tx, command_rx) = async_mpsc::unbounded_channel();
        let (event_tx, event_rx) = mpsc::channel();

        (
            NetLink {
                commands: command_tx,
                events: event_rx,
                inbox: event_tx.clone(),
                check_inline: true,
                backlog: Arc::new(Backlog::new(BacklogLimits::default())),
            },
            FakeNet {
                commands: command_rx,
                events: event_tx,
            },
        )
    }

    #[document(
        parent = "NetLink",
        summary = "Sends a command. Returns false when the network thread has stopped.",
    )]
    pub fn send(&self, command: NetCommand) -> bool {
        self.commands.send(command).is_ok()
    }

    #[document(
        parent = "NetLink",
        summary = "Whether the network thread has stopped. Events cannot tell: the link keeps a sender of its own for local clients.",
    )]
    pub fn stopped(&self) -> bool {
        self.commands.is_closed()
    }
}
