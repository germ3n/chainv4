//! What a node does, told to clients as it happens. A node only tells when asked to stream; then
//! every event goes to everyone subscribed through a NodeHandle, and a client too slow to keep up
//! misses some and is told so.

use {
    super::Node,
    crate::structs::block::Block,
    crate::structs::hash::Hash,
    crate::structs::tx::SignedTx,
    crate::structs::vote::Checkpoint,
    libp2p::{Multiaddr, PeerId},
    r#macro::document,
    std::sync::Arc,
    tokio::sync::broadcast,
};

#[document(
    summary = "Something a streaming node did, as subscribers of its NodeHandle hear it.",
    category = "Node",
    see_also = "Node::stream, NodeHandle::subscribe",
)]
#[derive(Clone, Debug)]
pub enum NodeEvent {
    #[document(summary = "A block was stored: main when it extended the main chain, otherwise on a side branch.")]
    Block { block: Arc<Block>, main: bool },
    #[document(summary = "The main chain moved to another branch: these blocks came off, newest first, and these went on, oldest first.")]
    Reorganized { undone: Vec<Hash>, applied: Vec<Arc<Block>> },
    #[document(summary = "A checkpoint was justified.")]
    Justified(Checkpoint),
    #[document(summary = "A checkpoint was finalized.")]
    Finalized(Checkpoint),
    #[document(summary = "A transaction joined the mempool.")]
    Queued(Arc<SignedTx>),
    #[document(summary = "A peer connected.")]
    PeerUp { peer: PeerId, address: Multiaddr },
    #[document(summary = "A peer's last connection closed.")]
    PeerDown(PeerId),
}

impl Node {
    #[document(
        parent = "Node",
        summary = "Makes the node tell what it does as it happens, to whoever subscribes through its NodeHandle. Take handles after this, so they can subscribe.",
        params = { backlog = "How many events a subscriber may fall behind by before it misses some." },
    )]
    pub fn stream(mut self, backlog: usize) -> Node {
        let events = broadcast::channel(backlog.max(1)).0;
        self.gate.stream(events.clone());
        self.events = Some(events);
        self
    }

    /// Tells subscribers about something, if the node streams and anyone listens. Events are
    /// only made when someone will hear them.
    pub(super) fn publish(&self, event: impl FnOnce() -> NodeEvent) {
        if let Some(events) = &self.events
            && events.receiver_count() > 0
        {
            let _ = events.send(event());
        }
    }
}
