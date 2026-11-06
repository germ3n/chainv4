//! Catching up with peers. A node learns how far each peer got, and how much work its chain
//! holds, from its status: asked for when the peer connects, and again whenever it gossips a
//! block that does not connect. While some peer holds more work, the node asks it for blocks
//! after the heights the two share and imports them in order; side blocks wait until their
//! branch outweighs the main chain, which then reorganizes. When a batch does not connect, the
//! chains split earlier, so the next request starts a batch further back. Once a batch from the
//! heaviest peer connects and it is more than two batches ahead, batches of the heights after go
//! out to up to --sync-peers peers at once, one each, any that claims them; what arrives ahead of
//! the next height needed is held, a few batches at most, and imported in order. Near the tip, one
//! request is in flight at a time. Once nobody holds more work, the node asks a peer that is ahead
//! on justification for the justifications it lacks, which may in turn finalize checkpoints here.

use {
    super::snapshot::SnapshotSync,
    super::{Node, NodeError},
    crate::chain::{FinalityError, ImportError},
    crate::net::{MAX_SYNC_BLOCKS, MAX_SYNC_JUSTIFICATIONS, NetCommand, SyncRequest, SyncResponse, Verdict},
    crate::structs::block::Block,
    crate::structs::u256::u256,
    libp2p::PeerId,
    std::collections::{BTreeMap, HashMap},
};

#[doc = "How many peers a long catch-up fetches blocks from at once unless --sync-peers says otherwise."]
pub(super) const SYNC_PEERS: usize = 4;

/// Where catching up stands.
pub(super) struct SyncState {
    /// What each peer on this chain said about its main chain.
    pub(super) peers: HashMap<PeerId, PeerChain>,
    /// Where taking a snapshot stands, while this node syncs from one.
    pub(super) snapshot: Option<SnapshotSync>,
    /// Block batches asked for and not answered yet, by the height each starts at: who was asked,
    /// and how many. One a peer at most.
    asking: BTreeMap<u64, (PeerId, u32)>,
    /// Batches that came ahead of the height the chain needs next, waiting to go in in order, by
    /// the height each starts at.
    held: BTreeMap<u64, (PeerId, Vec<Block>)>,
    /// How many peers a long catch-up asks at once: --sync-peers, SYNC_PEERS unless set.
    pub(super) most: usize,
    /// The peer asked for justifications, while its answer is outstanding.
    asking_proofs: Option<PeerId>,
}

impl Default for SyncState {
    fn default() -> SyncState {
        SyncState { peers: HashMap::new(), snapshot: None, asking: BTreeMap::new(), held: BTreeMap::new(), most: SYNC_PEERS, asking_proofs: None }
    }
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

    /// A block from this peer at this height, which it has at least: its work stays as its status
    /// said, which is what syncing goes by.
    pub(super) fn heard_height(&mut self, peer: &PeerId, height: u64) {
        if let Some(chain) = self.peers.get_mut(peer) {
            chain.height = chain.height.max(height);
        }
    }

    /// The peer blocks are being fetched from first, and from which height: the lowest batch out.
    pub(super) fn asking(&self) -> Option<(PeerId, u64)> {
        self.asking.iter().next().map(|(from, (peer, _))| (*peer, *from))
    }

    /// How many block batches are out, and held, now.
    #[cfg(test)]
    pub(super) fn in_flight(&self) -> (usize, usize) {
        (self.asking.len(), self.held.len())
    }

    /// Whether a height is in a batch out or held.
    fn covered(&self, height: u64) -> Option<u64> {
        let out = self.asking.range(..=height).next_back().map(|(from, (_, count))| from + u64::from(*count)).filter(|end| *end > height);
        let held = self.held.range(..=height).next_back().map(|(from, (_, blocks))| from + blocks.len() as u64).filter(|end| *end > height);

        out.max(held)
    }
}

impl Node {
    /// A peer left, or failed to answer: stop counting on it.
    pub(super) fn peer_failed(&mut self, peer: PeerId) -> Result<(), NodeError> {
        self.sync.peers.remove(&peer);
        // Its batches go back to be asked of another; what it sent that is held stays.
        self.sync.asking.retain(|_, (asked, _)| *asked != peer);

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
            response @ (SyncResponse::Headers(_) | SyncResponse::Finality(_) | SyncResponse::State(_) | SyncResponse::Checkpoint(_) | SyncResponse::Anchors(_) | SyncResponse::BtcHeaders(_)) => self.on_snapshot_response(peer, response, now)?,
            response @ SyncResponse::Blocks(_) if self.snapshotting() => self.on_snapshot_response(peer, response, now)?,
            SyncResponse::Blocks(blocks) => {
                let Some(from) = self.sync.asking.iter().find(|(_, (asked, _))| *asked == peer).map(|(from, _)| *from) else {
                    log::debug!("[node] ignored blocks {peer} sent unasked");

                    return Ok(());
                };

                self.sync.asking.remove(&from);
                let next = self.chain.tip().height + 1;

                match from > next && !blocks.is_empty() {
                    // Ahead of what the chain needs next: held until the batches before it are in.
                    true => {
                        self.sync.held.insert(from, (peer, blocks));
                    }
                    false => self.import_batch(peer, from, blocks, now)?,
                }

                self.import_held(now)?;
                self.sync_next()?;
            }
        }

        Ok(())
    }

    /// Imports the held batches that now follow the tip, in order; those wholly below it go.
    fn import_held(&mut self, now: u64) -> Result<(), NodeError> {
        while let Some((&from, _)) = self.sync.held.first_key_value() {
            let next = self.chain.tip().height + 1;

            if from > next {
                break;
            }

            let (peer, blocks) = self.sync.held.remove(&from).expect("it was first");

            if from + blocks.len() as u64 > next {
                self.import_batch(peer, from, blocks, now)?;
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
            // Blocks an upgrade stops this version at are left for a newer node.
            if self.stopped_at(block.header.height) {
                break;
            }

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

                    // One that breaks the rules counts against it, as the same block gossiped would.
                    if let Verdict::Reject(fault) = super::block_verdict(&Err(err)) {
                        self.judge(peer, None, Verdict::Reject(fault));
                    }

                    return Ok(());
                }
            }
        }

        if let (Some(height), Some(known)) = (reached, self.sync.peers.get_mut(&peer)) {
            known.next = Some(height + 1);
        }

        // Peers that were asked on from where the tip stood carry on from where it stands now, as
        // another's batch took it on: none of theirs is asked again. One stepping back to find where
        // its chain split stays where it was.
        let tip = self.chain.tip().height;

        if tip > before.0 {
            for known in self.sync.peers.values_mut() {
                if known.next.is_some_and(|next| next > before.0 && next <= tip) {
                    known.next = Some(tip + 1);
                }
            }
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

        let ours = self.chain.tip_work();
        let tip = self.chain.tip().height;
        let heaviest = self
            .sync
            .peers
            .iter()
            .filter(|(_, peer)| peer.work > ours && peer.height > 0)
            // Of those as heavy, one whose batch already connected, so the chains' meeting point
            // is not looked for again with each peer that says it has the same.
            .max_by_key(|(id, peer)| (peer.work, peer.next.is_some(), **id));

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
            self.sync_proofs(false);

            return Ok(());
        };

        // All its blocks are here and the chain still does not follow them: they leave a checkpoint
        // this node justified, on a branch that lost, and work alone never leaves one. Only a newer
        // justification moves the chain, so that is what to ask for, heavier peer or not.
        if chain.next.is_some_and(|next| next > chain.height) {
            self.sync_proofs(true);

            return Ok(());
        }

        let from = chain.next.unwrap_or(tip.min(chain.height) + 1).clamp(1, chain.height);
        let (lead, target, connected) = (*peer, chain.height, chain.next.is_some());
        let batch = u64::from(MAX_SYNC_BLOCKS);

        // Until a batch of the heaviest's connects, and while within two batches of it, one at a
        // time, from it: where the chains split is still being found, or there is little to fetch.
        if !connected || target.saturating_sub(tip) <= 2 * batch {
            if self.sync.asking.is_empty() {
                let count = (target - from + 1).min(batch) as u32;
                self.ask_blocks(lead, from, count);
            }

            return Ok(());
        }

        // A long catch-up: the missing batches from the next height needed on, a few batches ahead
        // at most, each asked of a peer that claims all of it and has nothing out, the heaviest
        // first.
        let most = self.sync.most.max(1);
        let window = from + 2 * batch * most as u64;
        let mut height = from;

        while self.sync.asking.len() < most && height <= target.min(window) {
            if let Some(end) = self.sync.covered(height) {
                height = end;

                continue;
            }

            let next_covered = (height..).take(batch as usize).find(|at| self.sync.covered(*at).is_some()).unwrap_or(height + batch);
            let end = next_covered.min(target + 1);
            let busy: Vec<PeerId> = self.sync.asking.values().map(|(asked, _)| *asked).collect();
            let Some(peer) = std::iter::once(lead)
                .chain(self.sync.peers.iter().filter(|(_, peer)| peer.height >= end - 1).map(|(id, _)| *id))
                .find(|id| !busy.contains(id) && self.sync.peers.get(id).is_some_and(|peer| peer.height >= end - 1))
            else {
                break;
            };

            self.ask_blocks(peer, height, (end - height) as u32);
            height = end;
        }

        Ok(())
    }

    /// Asks a peer for a batch of blocks.
    fn ask_blocks(&mut self, peer: PeerId, from: u64, count: u32) {
        log::debug!("[node] asking {peer} for blocks {from}..{}", from + u64::from(count));
        self.sync.asking.insert(from, (peer, count));
        self.send(NetCommand::Request {
            peer,
            request: SyncRequest::Blocks { from, count },
        });
    }

    /// Asks the peer furthest ahead on justification for the justifications this node lacks: those
    /// past its own, or, on a branch that lost, all those past the finalized checkpoint, as what
    /// comes next on the peer's branch may build on one of the peer's own below this node's.
    fn sync_proofs(&mut self, lost: bool) {
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
            let from = if lost { self.chain.finalized().map_or(0, |finalized| finalized.epoch) } else { ours } + 1;
            log::debug!("[node] asking {peer} for justifications from epoch {from}");
            self.sync.asking_proofs = Some(peer);
            self.send(NetCommand::Request {
                peer,
                request: SyncRequest::Justifications {
                    from,
                    count: MAX_SYNC_JUSTIFICATIONS,
                },
            });
        }
    }
}
