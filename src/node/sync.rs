//! Catching up with peers. A node learns how far each peer got, and how much work its chain
//! holds, from its status: asked for when the peer connects, and again whenever it gossips a
//! block that does not connect. While some peer holds more work, the node asks it for blocks
//! after the heights the two share and imports them in order; side blocks wait until their
//! branch outweighs the main chain, which then reorganizes. When a batch does not connect, the
//! chains split earlier, so the next request starts a batch further back. One request is in
//! flight at a time. Once nobody holds more work, the node asks a peer that is ahead on
//! justification for the justifications it lacks, which may in turn finalize checkpoints here.

use {
    super::snapshot::SnapshotSync,
    super::{Node, NodeError},
    crate::chain::{FinalityError, ImportError},
    crate::net::{MAX_SYNC_BLOCKS, MAX_SYNC_JUSTIFICATIONS, NetCommand, SyncRequest, SyncResponse},
    crate::structs::block::Block,
    crate::structs::u256::u256,
    libp2p::PeerId,
    std::collections::HashMap,
};

/// Where catching up stands.
#[derive(Default)]
pub(super) struct SyncState {
    /// What each peer on this chain said about its main chain.
    pub(super) peers: HashMap<PeerId, PeerChain>,
    /// Where taking a snapshot stands, while this node syncs from one.
    pub(super) snapshot: Option<SnapshotSync>,
    /// The peer asked for blocks, and the height asked from, while its answer is outstanding.
    asking: Option<(PeerId, u64)>,
    /// The peer asked for justifications, while its answer is outstanding.
    asking_proofs: Option<PeerId>,
}

pub(super) struct PeerChain {
    pub(super) height: u64,
    pub(super) work: u256,
    /// The epoch of its latest justified checkpoint.
    justified: u64,
    /// Where to ask from next; None to start just past what both chains have.
    next: Option<u64>,
}

impl SyncState {
    /// How far a peer's chain got, from its last status: its height, total work, and the epoch it
    /// justified last.
    pub(super) fn chain_of(&self, peer: &PeerId) -> Option<(u64, u256, u64)> {
        self.peers.get(peer).map(|chain| (chain.height, chain.work, chain.justified))
    }

    /// The peer blocks are being fetched from, and from which height.
    pub(super) fn asking(&self) -> Option<(PeerId, u64)> {
        self.asking
    }
}

impl Node {
    /// A peer left, or failed to answer: stop counting on it.
    pub(super) fn peer_failed(&mut self, peer: PeerId) -> Result<(), NodeError> {
        self.sync.peers.remove(&peer);

        if self.sync.asking.is_some_and(|(asked, _)| asked == peer) {
            self.sync.asking = None;
        }

        if self.sync.asking_proofs == Some(peer) {
            self.sync.asking_proofs = None;
        }

        if let Some(snapshot) = &mut self.sync.snapshot {
            snapshot.forget(peer);
        }

        self.sync_next()
    }

    pub(super) fn on_response(&mut self, peer: PeerId, response: SyncResponse, now: u64) -> Result<(), NodeError> {
        match response {
            SyncResponse::Status {
                genesis,
                height,
                work,
                justified,
                ..
            } => {
                if genesis != self.chain.genesis() {
                    log::debug!("[node] {peer} is on another chain, {genesis}");
                    self.sync.peers.remove(&peer);

                    return Ok(());
                }

                let next = self.sync.peers.get(&peer).and_then(|known| known.next);
                self.sync.peers.insert(peer, PeerChain {
                    height,
                    work,
                    justified,
                    next,
                });
                self.sync_next()?;
            }
            SyncResponse::Justifications(justifications) => {
                if self.sync.asking_proofs != Some(peer) {
                    return Ok(());
                }

                self.sync.asking_proofs = None;
                let mut learned = false;

                for justification in justifications {
                    let epoch = justification.target.epoch;

                    match self.chain.add_justification(justification) {
                        Ok(update) => {
                            learned |= !update.justified.is_empty();
                            self.finality_changed(update, &format!("synced from {peer}"))?;
                        }
                        Err(FinalityError::Db(err)) => return Err(NodeError::Db(err)),
                        Err(err) => log::debug!("[node] could not take {peer}'s justification for epoch {epoch}: {err}"),
                    }
                }

                // Nothing new from it: stop asking it until it says more.
                if !learned && let Some(known) = self.sync.peers.get_mut(&peer) {
                    known.justified = self.chain.justified().map_or(0, |justified| justified.epoch);
                }

                self.sync_next()?;
            }
            response @ (SyncResponse::Headers(_) | SyncResponse::Finality(_) | SyncResponse::State(_) | SyncResponse::Checkpoint(_)) => self.on_snapshot_response(peer, response, now)?,
            response @ SyncResponse::Blocks(_) if self.snapshotting() => self.on_snapshot_response(peer, response, now)?,
            SyncResponse::Blocks(blocks) => {
                let Some((asked, from)) = self.sync.asking.filter(|(asked, _)| *asked == peer) else {
                    log::debug!("[node] ignored blocks {peer} sent unasked");

                    return Ok(());
                };

                self.sync.asking = None;
                self.import_batch(asked, from, blocks, now)?;
                self.sync_next()?;
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
    /// or nobody holds more work than this node; or, while syncing from a snapshot, for what that
    /// needs next.
    pub(super) fn sync_next(&mut self) -> Result<(), NodeError> {
        if self.snapshotting() {
            return self.snapshot_next();
        }

        if self.sync.asking.is_some() {
            return Ok(());
        }

        let ours = self.chain.tip_work();
        let tip = self.chain.tip().height;
        let heaviest = self
            .sync
            .peers
            .iter()
            .filter(|(_, peer)| peer.work > ours && peer.height > 0)
            .max_by_key(|(id, peer)| (peer.work, **id));

        // Or, with nobody heavier, the peer furthest ahead on justification, until this node has
        // its blocks: a node that mined a heavier branch on its own, cut off from the validators,
        // fetches the branch they justified, which its justification then moves the chain to.
        let justified = self.chain.justified().map_or(0, |justified| justified.epoch);
        let ahead = || {
            self.sync
                .peers
                .iter()
                .filter(|(_, peer)| self.chain.takes_votes() && peer.justified > justified && peer.height > 0)
                .filter(|(_, peer)| peer.next.is_none_or(|next| next <= peer.height))
                .max_by_key(|(id, peer)| (peer.justified, **id))
        };

        let Some((peer, chain)) = heaviest.or_else(ahead) else {
            self.sync_proofs();

            return Ok(());
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

        Ok(())
    }

    /// Asks the peer furthest ahead on justification for the justifications this node lacks.
    fn sync_proofs(&mut self) {
        if self.sync.asking_proofs.is_some() || !self.chain.takes_votes() {
            return;
        }

        let Some(ours) = self.chain.justified().map(|justified| justified.epoch) else {
            return;
        };
        let ahead = self
            .sync
            .peers
            .iter()
            .filter(|(_, peer)| peer.justified > ours)
            .max_by_key(|(id, peer)| (peer.justified, **id))
            .map(|(id, _)| *id);

        if let Some(peer) = ahead {
            log::debug!("[node] asking {peer} for justifications from epoch {}", ours + 1);
            self.sync.asking_proofs = Some(peer);
            self.send(NetCommand::Request {
                peer,
                request: SyncRequest::Justifications {
                    from: ours + 1,
                    count: MAX_SYNC_JUSTIFICATIONS,
                },
            });
        }
    }
}
