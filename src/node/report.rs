//! What a running node tells clients about itself: its peers, what it heard and did, how syncing
//! goes, and what waits in its mempool. The node loop builds a report when asked, so it is always
//! current, and keeps nothing for it but a few counters.

use {
    super::{MEMPOOL_CAPACITY, Node},
    crate::net::BacklogLimits,
    crate::structs::tx::SignedTx,
    crate::structs::u256::u256,
    libp2p::{Multiaddr, PeerId},
    r#macro::document,
};

#[document(
    summary = "How a running node is doing, as it answers NetEvent::Report.",
    category = "Node",
    see_also = "NodeHandle::report, Counters",
)]
#[derive(Clone, Debug)]
pub struct NodeReport {
    #[document(summary = "When the node loop first ran, in milliseconds since the Unix epoch.")]
    pub started: u64,
    #[document(summary = "When the report was made.")]
    pub now: u64,
    #[document(summary = "Where peers can reach it, each ending in /p2p/ and its peer id.")]
    pub listening: Vec<Multiaddr>,
    #[document(summary = "Whether it produces blocks: mines, or seals as the authority.")]
    pub producing: bool,
    #[document(summary = "Whether it votes on finality.")]
    pub validating: bool,
    #[document(summary = "How its mining goes, when it mines: its own devices' hashrates and the workers mining for it.")]
    pub mining: Option<crate::consensus::MiningReport>,
    #[document(summary = "The peers connected now.")]
    pub peers: Vec<PeerReport>,
    #[document(summary = "The peer it is fetching blocks from, and from which height, while it catches up.")]
    pub syncing: Option<(PeerId, u64)>,
    #[document(summary = "How many transactions wait in its mempool.")]
    pub waiting: usize,
    #[document(summary = "The most its mempool holds.")]
    pub capacity: usize,
    #[document(summary = "The first transactions waiting, by sender and nonce, as many as were asked for.")]
    pub txs: Vec<SignedTx>,
    #[document(summary = "What it heard and did since it started.")]
    pub counters: Counters,
    #[document(summary = "How much gossip and how many submissions wait for it now, and the backlog's limits.")]
    pub backlog: (usize, BacklogLimits),
}

#[document(
    summary = "A peer connected to a node, and what the node knows of it.",
    category = "Node",
    see_also = "NodeReport",
)]
#[derive(Clone, Debug)]
pub struct PeerReport {
    #[document(summary = "Its libp2p peer id.")]
    pub id: PeerId,
    #[document(summary = "The address its connection came from or went to.")]
    pub address: Multiaddr,
    #[document(summary = "When it connected, in milliseconds since the Unix epoch.")]
    pub since: u64,
    #[document(summary = "The blocks, transactions and votes it gossiped.")]
    pub heard: Heard,
    #[document(summary = "The transactions and votes of its that were dropped, the backlog having no room for them.")]
    pub dropped: u64,
    #[document(summary = "How far its chain got, from its last status: height, total work and the epoch it justified last. None before it answered.")]
    pub chain: Option<(u64, u256, u64)>,
}

#[document(
    summary = "What one peer gossiped to a node.",
    category = "Node",
    see_also = "PeerReport",
)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Heard {
    #[document(summary = "Blocks it gossiped.")]
    pub blocks: u64,
    #[document(summary = "Transactions it gossiped.")]
    pub txs: u64,
    #[document(summary = "Finality votes it gossiped.")]
    pub votes: u64,
}

#[document(
    summary = "What a node heard and did since it started.",
    category = "Node",
    see_also = "NodeReport",
)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counters {
    #[document(summary = "Blocks peers gossiped.")]
    pub blocks_heard: u64,
    #[document(summary = "Blocks that extended the main chain, from anywhere, its own included.")]
    pub blocks_imported: u64,
    #[document(summary = "Blocks stored on a side branch.")]
    pub side_blocks: u64,
    #[document(summary = "Times the main chain moved to another branch.")]
    pub reorgs: u64,
    #[document(summary = "Blocks refused as invalid.")]
    pub blocks_rejected: u64,
    #[document(summary = "Blocks it produced itself.")]
    pub blocks_produced: u64,
    #[document(summary = "Transactions peers gossiped.")]
    pub txs_heard: u64,
    #[document(summary = "Transactions its clients submitted that it took.")]
    pub txs_submitted: u64,
    #[document(summary = "Transactions, gossiped or submitted, its mempool refused.")]
    pub txs_refused: u64,
    #[document(summary = "Finality votes peers gossiped.")]
    pub votes_heard: u64,
    #[document(summary = "Finality votes it signed.")]
    pub votes_cast: u64,
    #[document(summary = "Requests from syncing peers it answered.")]
    pub requests_served: u64,
    #[document(summary = "Requests it sent to peers.")]
    pub requests_sent: u64,
    #[document(summary = "Requests it sent that got no answer.")]
    pub requests_failed: u64,
    #[document(summary = "Connections to peers it made or took.")]
    pub connections: u64,
    #[document(summary = "Transactions, votes and submissions dropped, the backlog having no room for them.")]
    pub gossip_dropped: u64,
    #[document(summary = "Peers disconnected for going over their share of the backlog too often.")]
    pub flooders_disconnected: u64,
    #[document(summary = "Gossip refused as breaking the rules, which counted against its sender.")]
    pub gossip_rejected: u64,
    #[document(summary = "Peers banned for their faults.")]
    pub peers_banned: u64,
    #[document(summary = "Blocks, transactions and votes it published that reached none of its connected peers, as when none had heard what it subscribes to.")]
    pub gossip_unpublished: u64,
}

/// A connected peer, as the node keeps it.
#[derive(Clone, Debug)]
pub(super) struct Peer {
    pub(super) address: Multiaddr,
    pub(super) since: u64,
    pub(super) heard: Heard,
}

impl Node {
    /// How the node is doing now, with up to txs of the transactions waiting.
    pub(super) fn report(&self, now: u64, txs: usize) -> NodeReport {
        let mut peers: Vec<PeerReport> = self
            .peers
            .iter()
            .map(|(id, peer)| PeerReport {
                id: *id,
                address: peer.address.clone(),
                since: peer.since,
                heard: peer.heard,
                dropped: self.backlog.dropped_from(id),
                chain: self.sync.chain_of(id),
            })
            .collect();
        peers.sort_by_key(|peer| peer.since);

        // One lock for both, taken and let go before the rest.
        let (waiting, waiting_txs) = {
            let mempool = self.gate.mempool();

            (mempool.len(), mempool.iter().take(txs).cloned().collect())
        };

        NodeReport {
            started: self.started,
            now,
            listening: self.listening.clone(),
            producing: self.producer.is_some(),
            mining: self.producer.as_ref().and_then(|producer| producer.mining()),
            validating: self.validator.is_some(),
            peers,
            syncing: self.sync.asking(),
            waiting,
            capacity: MEMPOOL_CAPACITY,
            txs: waiting_txs,
            counters: Counters {
                gossip_dropped: self.backlog.dropped(),
                flooders_disconnected: self.backlog.disconnected(),
                ..self.counters
            },
            backlog: (self.backlog.waiting(), self.backlog.limits()),
        }
    }
}
