//! Catching up with peers. A node learns how far each peer got, and how much work its chain
//! holds, from its status: asked for when the peer connects, and again whenever it gossips a
//! block that does not connect. While some peer holds more work, the node asks it for blocks
//! after the heights the two share and imports them in order; side blocks wait until their
//! branch outweighs the main chain, which then reorganizes. When a batch does not connect, the
//! chains split earlier, so the next request starts a batch further back. One request is in
//! flight at a time.

use {
    super::{Node, NodeError},
    crate::chain::ImportError,
    crate::net::{MAX_SYNC_BLOCKS, NetCommand, SyncRequest, SyncResponse},
    crate::structs::block::Block,
    crate::structs::u256::u256,
    libp2p::PeerId,
    std::collections::HashMap,
};

/// Where catching up stands.
#[derive(Default)]
pub(super) struct SyncState {
    /// What each peer on this chain said about its main chain.
    peers: HashMap<PeerId, PeerChain>,
    /// The peer asked for blocks, and the height asked from, while its answer is outstanding.
    asking: Option<(PeerId, u64)>,
}

struct PeerChain {
    height: u64,
    work: u256,
    /// Where to ask from next; None to start just past what both chains have.
    next: Option<u64>,
}

impl Node {
    /// A peer left, or failed to answer: stop counting on it.
    pub(super) fn peer_failed(&mut self, peer: PeerId) {
        self.sync.peers.remove(&peer);

        if self.sync.asking.is_some_and(|(asked, _)| asked == peer) {
            self.sync.asking = None;
        }

        self.sync_next();
    }

    pub(super) fn on_response(&mut self, peer: PeerId, response: SyncResponse, now: u64) -> Result<(), NodeError> {
        match response {
            SyncResponse::Status {
                genesis, height, work, ..
            } => {
                if genesis != self.chain.genesis() {
                    log::debug!("[node] {peer} is on another chain, {genesis}");
                    self.sync.peers.remove(&peer);

                    return Ok(());
                }

                let next = self.sync.peers.get(&peer).and_then(|known| known.next);
                self.sync.peers.insert(peer, PeerChain { height, work, next });
                self.sync_next();
            }
            SyncResponse::Blocks(blocks) => {
                let Some((asked, from)) = self.sync.asking.filter(|(asked, _)| *asked == peer) else {
                    log::debug!("[node] ignored blocks {peer} sent unasked");

                    return Ok(());
                };

                self.sync.asking = None;
                self.import_batch(asked, from, blocks, now)?;
                self.sync_next();
            }
        }

        Ok(())
    }

    /// Imports blocks a peer sent, in order, and decides where to ask from next.
    fn import_batch(&mut self, peer: PeerId, from: u64, blocks: Vec<Block>, now: u64) -> Result<(), NodeError> {
        let before = (self.chain.tip().height, self.chain.tip_work());
        let mut reached = None;

        if blocks.is_empty() {
            // It claimed more than it has; believe it no further.
            self.sync.peers.remove(&peer);
        }

        for (position, block) in blocks.iter().enumerate() {
            match self.chain.import(block, now) {
                Ok(imported) => {
                    self.imported(block, imported, &format!("synced from {peer}"))?;
                    reached = Some(block.header.height);
                }
                Err(ImportError::Known) => reached = Some(block.header.height),
                Err(ImportError::Db(err)) => return Err(NodeError::Db(err)),
                Err(ImportError::UnknownParent(_)) if position == 0 && from > 1 => {
                    // The chains split before from: step a batch further back.
                    let back = from.saturating_sub(u64::from(MAX_SYNC_BLOCKS)).max(1);
                    log::debug!("[node] {peer}'s blocks from {from} do not connect; asking from {back}");

                    if let Some(known) = self.sync.peers.get_mut(&peer) {
                        known.next = Some(back);
                    }

                    return Ok(());
                }
                Err(err) => {
                    log::warn!("[node] {peer} sent block {} that does not fit: {err}", block.header.height);
                    self.sync.peers.remove(&peer);

                    return Ok(());
                }
            }
        }

        if let (Some(height), Some(known)) = (reached, self.sync.peers.get_mut(&peer)) {
            known.next = Some(height + 1);
        }

        if (self.chain.tip().height, self.chain.tip_work()) != before {
            log::info!("[node] synced to height {} from {peer}", self.chain.tip().height);
        }

        Ok(())
    }

    /// Asks the peer holding the most work for its next blocks, unless a request is already out
    /// or nobody holds more work than this node.
    fn sync_next(&mut self) {
        if self.sync.asking.is_some() {
            return;
        }

        let ours = self.chain.tip_work();
        let tip = self.chain.tip().height;
        let heaviest = self
            .sync
            .peers
            .iter()
            .filter(|(_, peer)| peer.work > ours && peer.height > 0)
            .max_by_key(|(id, peer)| (peer.work, **id));

        let Some((peer, chain)) = heaviest else {
            return;
        };
        let from = chain.next.unwrap_or(tip.min(chain.height) + 1).clamp(1, chain.height);
        let count = (chain.height - from + 1).min(u64::from(MAX_SYNC_BLOCKS)) as u32;
        let peer = *peer;

        log::debug!("[node] asking {peer} for blocks {from}..{}", from + u64::from(count));
        self.sync.asking = Some((peer, from));
        self.send(NetCommand::Request {
            peer,
            request: SyncRequest::Blocks { from, count },
        });
    }
}
