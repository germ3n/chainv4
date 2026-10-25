//! Syncing from a snapshot: a node that holds only genesis checks every header its peers have,
//! takes the state of a block a few below the best tip from them a part at a time, checking each
//! part against that block's state root, and makes the block its tip. Then it syncs the rest as
//! usual. While it takes state, it imports, produces, votes on and admits nothing, since its state
//! is half the pivot's.

use {
    super::{Node, NodeError},
    crate::db::{Path, SnapshotWalk, WalkError},
    crate::net::{MAX_SYNC_HEADERS, NetCommand, SyncRequest, SyncResponse},
    crate::structs::block::{Block, SealedHeader},
    crate::structs::u256::u256,
    libp2p::PeerId,
    r#macro::document,
    std::collections::HashSet,
};

/// How far below the best tip the pivot sits, so the block whose state it takes is unlikely to
/// come off in a reorg, yet recent enough for peers to serve: they keep undo records for the last
/// max_reorg_depth blocks, so it is at most half that.
pub(super) const PIVOT_DEPTH: u64 = 16;
/// How many times the pivot may move up before the node gives up on the snapshot and syncs by
/// running blocks: peers that cannot serve any pivot would have it move forever.
const MAX_MOVES: u32 = 8;
/// A chain shorter than this is synced by running its blocks: a snapshot would save nothing.
pub(super) const MIN_SNAPSHOT_HEIGHT: u64 = 2 * PIVOT_DEPTH;

/// Where taking a snapshot stands.
pub(super) struct SnapshotSync {
    /// Every header checked so far, genesis first, and each one's total work.
    headers: Vec<SealedHeader>,
    work: Vec<u256>,
    phase: Phase,
    /// The peer asked, and what, while its answer is outstanding.
    asking: Option<(PeerId, Asked)>,
    /// Peers that sent something that did not check: never asked again.
    distrusted: HashSet<PeerId>,
    /// How many times the pivot moved up.
    moves: u32,
}

enum Phase {
    /// Checking headers, up to the heaviest peer's tip.
    Headers,
    /// Taking the state of the block at this height.
    State { pivot: u64, walk: SnapshotWalk },
    /// Fetching the block at this height, to make it the tip.
    Block { pivot: u64 },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Asked {
    Headers,
    State { depth: u16, prefix: Path },
    Block,
}

impl SnapshotSync {
    pub(super) fn new(genesis: SealedHeader) -> SnapshotSync {
        SnapshotSync {
            headers: vec![genesis],
            work: vec![u256::ZERO],
            phase: Phase::Headers,
            asking: None,
            distrusted: HashSet::new(),
            moves: 0,
        }
    }

    /// Stops waiting on a peer that left or failed: what it was asked goes to another.
    pub(super) fn forget(&mut self, peer: PeerId) {
        if self.asking.is_some_and(|(asked, _)| asked == peer) {
            self.asking = None;

            if let Phase::State { walk, .. } = &mut self.phase {
                walk.retry();
            }
        }
    }

    /// What it is doing, for the node's report.
    pub(super) fn stage(&self) -> String {
        match &self.phase {
            Phase::Headers => format!("checking headers, {} so far", self.headers.len() - 1),
            Phase::State { pivot, walk } => format!("taking state at height {pivot}, {} entries so far", walk.taken()),
            Phase::Block { pivot } => format!("fetching block {pivot}"),
        }
    }
}

impl Node {
    #[document(
        parent = "Node",
        summary = "Syncs from a snapshot rather than running every block: checks every header its peers have, takes the state of a block PIVOT_DEPTH below the best tip a part at a time, and makes that block its tip. Only for a node that holds genesis alone; on a chain shorter than MIN_SNAPSHOT_HEIGHT it syncs as usual.",
        returns = "Whether it will: false for a node past genesis.",
    )]
    pub fn sync_from_snapshot(&mut self) -> Result<bool, NodeError> {
        if self.chain.tip().height != 0 || self.chain.snapshot().is_some() {
            return Ok(false);
        }

        let genesis = self.chain.headers_from(0, 1)?.pop().ok_or(NodeError::Db(crate::db::DbError::Corrupt("no genesis header".to_string())))?;
        self.sync.snapshot = Some(SnapshotSync::new(genesis));

        Ok(true)
    }

    /// Whether the node is taking state from a snapshot.
    pub(super) fn snapshotting(&self) -> bool {
        self.sync.snapshot.is_some()
    }

    /// Asks the heaviest peer it still trusts for what the snapshot needs next.
    pub(super) fn snapshot_next(&mut self) -> Result<(), NodeError> {
        let Some(snapshot) = &self.sync.snapshot else {
            return Ok(());
        };

        if snapshot.asking.is_some() {
            return Ok(());
        }

        let Some(peer) = self
            .sync
            .peers
            .iter()
            .filter(|(id, chain)| !snapshot.distrusted.contains(id) && chain.height > 0)
            .max_by_key(|(id, chain)| (chain.work, **id))
            .map(|(id, _)| *id)
        else {
            return Ok(());
        };
        let pivot_id = |pivot: u64| snapshot.headers[pivot as usize].id();
        let (asked, request) = match &snapshot.phase {
            Phase::Headers => (Asked::Headers, SyncRequest::Headers {
                from: snapshot.headers.len() as u64,
                count: MAX_SYNC_HEADERS,
            }),
            Phase::Block { pivot } => (Asked::Block, SyncRequest::Blocks { from: *pivot, count: 1 }),
            Phase::State { pivot, .. } => {
                let block = pivot_id(*pivot);
                let snapshot = self.sync.snapshot.as_mut().expect("still taking one");
                let Phase::State { pivot, walk } = &mut snapshot.phase else { unreachable!("taking state") };

                match self.chain.snapshot_next(walk)? {
                    Some((depth, prefix)) => (Asked::State { depth, prefix }, SyncRequest::State { block, depth, prefix }),
                    None if walk.done() => {
                        log::info!("[node] took {} entries of state at height {pivot}; fetching the block", walk.taken());
                        let pivot = *pivot;
                        snapshot.phase = Phase::Block { pivot };

                        (Asked::Block, SyncRequest::Blocks { from: pivot, count: 1 })
                    }
                    None => return Ok(()),
                }
            }
        };

        self.sync.snapshot.as_mut().expect("still taking one").asking = Some((peer, asked));
        self.send(NetCommand::Request { peer, request });

        Ok(())
    }

    /// Takes a peer's headers, state or block while syncing from a snapshot.
    pub(super) fn on_snapshot_response(&mut self, peer: PeerId, response: SyncResponse, now: u64) -> Result<(), NodeError> {
        let Some(snapshot) = self.sync.snapshot.as_mut() else {
            return Ok(());
        };
        let Some((_, asked)) = snapshot.asking.filter(|(asked, _)| *asked == peer) else {
            log::debug!("[node] ignored an answer {peer} sent unasked");

            return Ok(());
        };

        match (asked, response) {
            (Asked::Headers, SyncResponse::Headers(headers)) => {
                snapshot.asking = None;
                self.take_headers(peer, headers, now);
            }
            (Asked::State { depth, prefix }, SyncResponse::State(part)) => {
                snapshot.asking = None;
                let Phase::State { pivot, walk } = &mut snapshot.phase else {
                    return Ok(());
                };

                match part {
                    // The peer no longer serves the pivot: move it up, and walk again from there,
                    // which skips everything already right.
                    None if snapshot.moves >= MAX_MOVES => {
                        log::warn!("[node] peers serve no pivot for long: syncing by running blocks instead");
                        self.sync.snapshot = None;
                    }
                    None => {
                        log::info!("[node] {peer} no longer serves state at height {pivot}; moving the pivot up");
                        walk.retry();
                        snapshot.moves += 1;
                        snapshot.phase = Phase::Headers;
                    }
                    Some(part) => match self.chain.take_snapshot_part(walk, depth, prefix, part) {
                        Ok(()) => {}
                        Err(WalkError::Db(err)) => return Err(NodeError::Db(err)),
                        Err(WalkError::Unasked) => walk.retry(),
                        Err(WalkError::Invalid) => {
                            log::warn!("[node] {peer} sent state that does not check against the pivot's root; asking others");
                            walk.retry();
                            snapshot.distrusted.insert(peer);
                        }
                    },
                }
            }
            (Asked::Block, SyncResponse::Blocks(blocks)) => {
                snapshot.asking = None;
                let Phase::Block { pivot } = snapshot.phase else {
                    return Ok(());
                };
                let expected = &snapshot.headers[pivot as usize];

                match blocks.into_iter().next().filter(|block| fits(block, expected, self.chain.hash())) {
                    Some(block) => self.install(block, pivot)?,
                    None => {
                        log::warn!("[node] {peer} sent a block that is not the pivot; asking others");
                        snapshot.distrusted.insert(peer);
                    }
                }
            }
            // An answer of the wrong kind is no answer.
            (_, _) => {
                snapshot.asking = None;
                snapshot.distrusted.insert(peer);
            }
        }

        self.sync_next()
    }

    /// Checks headers a peer sent by consensus and keeps them; once it has no more, picks the pivot.
    fn take_headers(&mut self, peer: PeerId, headers: Vec<SealedHeader>, now: u64) {
        let depth = PIVOT_DEPTH.min(self.chain.max_reorg_depth() / 2);
        let consensus = self.chain.consensus();
        let snapshot = self.sync.snapshot.as_mut().expect("taking one");
        let full = headers.len() as u32 == MAX_SYNC_HEADERS;

        if let Some(first) = headers.first()
            && first.header.parent != snapshot.headers.last().expect("genesis").id()
        {
            // Its chain split off below what was checked: step a batch back and ask again.
            let keep = snapshot.headers.len().saturating_sub(MAX_SYNC_HEADERS as usize).max(1);
            snapshot.headers.truncate(keep);
            snapshot.work.truncate(keep);

            return;
        }

        for sealed in headers {
            let parent = &snapshot.headers.last().expect("genesis").header;

            if sealed.header.height != parent.height + 1 || sealed.header.parent != parent.id() || consensus.check(&sealed.header, &sealed.seal, parent, now).is_err() {
                log::warn!("[node] {peer} sent header {} that does not check; asking others", sealed.header.height);
                snapshot.distrusted.insert(peer);

                return;
            }

            let work = *snapshot.work.last().expect("genesis") + consensus.work(&sealed.header);
            snapshot.work.push(work);
            snapshot.headers.push(sealed);
        }

        if full {
            return;
        }

        let height = snapshot.headers.len() as u64 - 1;

        if height < MIN_SNAPSHOT_HEIGHT {
            log::info!("[node] the chain is only {height} blocks long: syncing by running them instead");
            self.sync.snapshot = None;

            return;
        }

        let pivot = height - depth;
        let root = snapshot.headers[pivot as usize].header.state_root;
        log::info!("[node] checked {height} headers; taking state at height {pivot}");
        snapshot.phase = Phase::State {
            pivot,
            walk: SnapshotWalk::new(root),
        };
    }

    /// Makes the pivot the tip, and goes back to syncing as usual from it.
    fn install(&mut self, block: Block, pivot: u64) -> Result<(), NodeError> {
        let snapshot = self.sync.snapshot.take().expect("taking one");
        let headers = &snapshot.headers[1..=pivot as usize];
        self.chain.install_snapshot(headers, &block, snapshot.work[pivot as usize]).map_err(NodeError::Chain)?;
        log::info!("[node] synced from a snapshot to height {pivot} {}", block.id());

        // What peers said was before the pivot moved, perhaps: ask them where they are now.
        let peers: Vec<PeerId> = self.sync.peers.keys().copied().collect();

        for peer in peers {
            self.send(NetCommand::Request {
                peer,
                request: SyncRequest::Status,
            });
        }

        Ok(())
    }
}

/// Whether a block is the one a checked header names, its body included.
fn fits(block: &Block, header: &SealedHeader, hash: crate::structs::hash::HashType) -> bool {
    block.sealed_header() == *header
        && Block::tx_root(hash, &block.txs) == header.header.tx_root
        && Block::proof_root(hash, &block.proofs) == header.header.proof_root
}
