//! Catching up with peers. A node learns how far each peer got from its status, asked for when
//! the peer connects, and from gossiped blocks it cannot place yet. Whenever some peer is ahead,
//! it asks the furthest one for the next MAX_SYNC_BLOCKS blocks, imports them in order, and asks
//! again until it has caught up. One request is in flight at a time, so blocks always arrive in
//! the order they must be imported.

use {
    super::{Node, NodeError},
    crate::chain::ImportError,
    crate::net::{MAX_SYNC_BLOCKS, NetCommand, SyncRequest, SyncResponse},
    libp2p::PeerId,
    std::collections::HashMap,
};

/// Where catching up stands.
#[derive(Default)]
pub(super) struct SyncState {
    /// How far each peer on this chain said it got.
    heights: HashMap<PeerId, u64>,
    /// The peer asked for blocks, while its answer is outstanding.
    asking: Option<PeerId>,
}

impl Node {
    /// A peer is known to have reached a height, from its status or a block it gossiped.
    pub(super) fn peer_reached(&mut self, peer: PeerId, height: u64) {
        let known = self.sync.heights.entry(peer).or_insert(0);
        *known = (*known).max(height);
        self.sync_next();
    }

    /// A peer left, or failed to answer: stop counting on it.
    pub(super) fn peer_failed(&mut self, peer: PeerId) {
        self.sync.heights.remove(&peer);

        if self.sync.asking == Some(peer) {
            self.sync.asking = None;
        }

        self.sync_next();
    }

    pub(super) fn on_response(&mut self, peer: PeerId, response: SyncResponse, now: u64) -> Result<(), NodeError> {
        match response {
            SyncResponse::Status { genesis, height, .. } => {
                if genesis != self.chain.genesis() {
                    log::debug!("[node] {peer} is on another chain, {genesis}");
                    self.sync.heights.remove(&peer);

                    return Ok(());
                }

                self.peer_reached(peer, height);
            }
            SyncResponse::Blocks(blocks) => {
                if self.sync.asking != Some(peer) {
                    log::debug!("[node] ignored blocks {peer} sent unasked");

                    return Ok(());
                }

                self.sync.asking = None;
                self.import_batch(peer, blocks, now)?;
                self.sync_next();
            }
        }

        Ok(())
    }

    /// Imports blocks a peer sent, in order, stopping at the first that does not fit.
    fn import_batch(&mut self, peer: PeerId, blocks: Vec<crate::structs::block::Block>, now: u64) -> Result<(), NodeError> {
        let before = self.chain.tip().height;

        if blocks.is_empty() {
            // It claimed more than it has; believe it no further.
            self.sync.heights.insert(peer, before);
        }

        for block in &blocks {
            match self.chain.import(block, &*self.consensus, now) {
                Ok(_) | Err(ImportError::Known) => {}
                Err(ImportError::Db(err)) => return Err(NodeError::Db(err)),
                Err(err) => {
                    log::warn!("[node] {peer} sent block {} that does not fit: {err}", block.header.height);
                    self.sync.heights.remove(&peer);

                    break;
                }
            }
        }

        let after = self.chain.tip().height;

        if after > before {
            self.mempool.prune(&self.chain.state())?;
            log::info!("[node] synced blocks {}..={after} from {peer}", before + 1);
        }

        Ok(())
    }

    /// Asks the furthest peer for the next blocks, unless a request is already out or nobody
    /// is ahead.
    fn sync_next(&mut self) {
        if self.sync.asking.is_some() {
            return;
        }

        let tip = self.chain.tip().height;
        let furthest = self
            .sync
            .heights
            .iter()
            .filter(|(_, height)| **height > tip)
            .max_by_key(|(peer, height)| (**height, **peer));

        let Some((peer, height)) = furthest.map(|(peer, height)| (*peer, *height)) else {
            return;
        };
        let count = (height - tip).min(u64::from(MAX_SYNC_BLOCKS)) as u32;

        log::debug!("[node] asking {peer} for blocks {}..={}", tip + 1, tip + u64::from(count));
        self.sync.asking = Some(peer);
        self.send(NetCommand::Request {
            peer,
            request: SyncRequest::Blocks { from: tip + 1, count },
        });
    }
}
