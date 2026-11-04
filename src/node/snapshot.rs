//! Syncing from a snapshot: a node that holds only genesis checks every header its peers have,
//! takes the state of a block a few below the best tip from them a part at a time, checking each
//! part against that block's state root, and makes the block its tip. Then it syncs the rest as
//! usual. While it takes state, it imports, produces, votes on and admits nothing, since its state
//! is half the pivot's.
//!
//! The headers are checked by a light client of the node's own chain, fed over the peer protocol:
//! consensus checks each as a node would, and under a consensus whose producers finality names, a
//! rotating authority or production by stake, the client learns each epoch's producers from the
//! finality peers serve it, as any light client does, and checks on once it knows them.

use {
    super::{Node, NodeError},
    crate::db::{Path, SnapshotWalk, WalkError},
    crate::light::{Extended, LightClient, LightError, LightRequest, MAX_LIGHT_HEADERS},
    crate::net::{MAX_SYNC_FINALITY, MAX_SYNC_HEADERS, NetCommand, SyncRequest, SyncResponse},
    crate::structs::block::{Block, SealedHeader},
    crate::structs::hash::Hash,
    crate::structs::u256::u256,
    crate::structs::vote::Checkpoint,
    libp2p::PeerId,
    r#macro::document,
    std::collections::HashSet,
};

#[document(
    summary = "How far below the best tip a snapshot sync takes its pivot unless --snapshot-depth says otherwise: 16 blocks, so the block whose state it takes is unlikely to come off in a reorg, yet recent enough for peers to serve. The syncing node picks it alone: a peer serves the state of any main-chain block its --state-history still holds, so peers need not agree on it. It is held to half max_reorg_depth, and a chain shorter than twice it is synced by running its blocks.",
    category = "Node",
    see_also = "NodeTuning::snapshot_depth",
)]
pub const PIVOT_DEPTH: u64 = 16;
/// How many times the pivot may move up before the node gives up on the snapshot and syncs by
/// running blocks: peers that cannot serve any pivot would have it move forever.
const MAX_MOVES: u32 = 8;

/// Where taking a snapshot stands.
pub(super) struct SnapshotSync {
    /// The light client that checks the headers: of the chain from genesis, or from the
    /// checkpoint start once a peer sent it.
    light: Option<LightClient>,
    /// The checkpoint this node trusts, if it starts at one rather than at genesis, and its total
    /// work as the peer that served the start said, less what the light client counts to it: what
    /// the client's figures lack.
    checkpoint: Option<(Hash, u256)>,
    phase: Phase,
    /// The peer asked, and what, while its answer is outstanding.
    asking: Option<(PeerId, Asked)>,
    /// Peers that sent something that did not check: never asked again.
    distrusted: HashSet<PeerId>,
    /// How many times the pivot moved up.
    moves: u32,
}

enum Phase {
    /// Fetching the checkpoint start: the trusted block's header and those below it.
    Checkpoint,
    /// Checking headers, and learning finality, up to the heaviest peer's tip.
    Headers,
    /// Taking the state of the block at this height.
    State { pivot: u64, walk: SnapshotWalk },
    /// Fetching the block at this height, to make it the tip.
    Block { pivot: u64 },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Asked {
    Checkpoint,
    Headers { from: u64 },
    Finality { from: u64 },
    State { depth: u16, prefix: Path },
    Block,
}

impl SnapshotSync {
    pub(super) fn new(mut light: LightClient, checkpoint: Option<Hash>) -> SnapshotSync {
        // What it checks of finality is kept, to store with the snapshot and serve on from.
        light.keep_checked(crate::light::Kept::default());

        SnapshotSync {
            light: match checkpoint {
                Some(_) => None,
                None => Some(light),
            },
            checkpoint: checkpoint.map(|block| (block, u256::ZERO)),
            phase: match checkpoint {
                Some(_) => Phase::Checkpoint,
                None => Phase::Headers,
            },
            asking: None,
            distrusted: HashSet::new(),
            moves: 0,
        }
    }

    /// The light client, once the chain it checks is known.
    fn light(&self) -> &LightClient {
        self.light.as_ref().expect("the checkpoint start is in")
    }

    fn light_mut(&mut self) -> &mut LightClient {
        self.light.as_mut().expect("the checkpoint start is in")
    }

    /// The height of the newest header checked.
    fn checked(&self) -> u64 {
        self.light.as_ref().map_or(0, |light| light.tip().height)
    }

    /// The total work of a checked header: the light client's count, and the checkpoint's own
    /// below it when the node started there.
    fn work_at(&self, height: u64) -> u256 {
        self.checkpoint.map_or(u256::ZERO, |(_, below)| below) + self.light().work_at(height).expect("a checked header")
    }

    /// The height of the trusted checkpoint, when the node starts at one.
    fn checkpoint_height(&self) -> Option<u64> {
        let (block, _) = self.checkpoint?;
        let light = self.light();

        (light.lowest()..=light.tip().height).find(|height| light.sealed_at(*height).is_some_and(|sealed| sealed.id() == block))
    }

    /// Whether this node starts at a checkpoint, so it never falls back to running from genesis.
    fn starts_at_checkpoint(&self) -> bool {
        self.checkpoint.is_some()
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
            Phase::Checkpoint => "fetching the checkpoint".to_string(),
            Phase::Headers => format!("checking headers, {} so far", self.checked()),
            Phase::State { pivot, walk } => format!("taking state at height {pivot}, {} entries so far", walk.taken()),
            Phase::Block { pivot } => format!("fetching block {pivot}"),
        }
    }
}

impl Node {
    #[document(
        parent = "Node",
        summary = "Syncs from a snapshot rather than running every block: checks every header its peers have, as a light client of its own chain fed over the peer protocol, takes the state of a block NodeTuning::snapshot_depth below the best tip, PIVOT_DEPTH by default, a part at a time, and makes that block its tip. Only for a node that holds genesis alone; on a chain shorter than twice that depth it syncs as usual, as a snapshot would save nothing. Under a rotating authority or production by stake it learns each epoch's producers from the finality peers serve, as a light client does, and takes the pivot among the headers it could check. Given a checkpoint it trusts, it fetches no header below it: it takes the checkpoint's headers from a peer, trusted by their hashes alone, holds the checkpoint as finalized, and checks on from there, so a long-range fork below the checkpoint cannot reach it; it then never falls back to running from genesis.",
        params = { checkpoint = "A finalized checkpoint's block id, from someone the operator trusts, or None to check every header from genesis." },
        returns = "Whether it will: false for a node past genesis.",
    )]
    pub fn sync_from_snapshot(&mut self, checkpoint: Option<Hash>) -> Result<bool, NodeError> {
        if self.chain.tip().height != 0 || self.chain.snapshot().is_some() {
            return Ok(false);
        }

        let genesis = self.chain.headers_from(0, 1)?.pop().ok_or(NodeError::Db(crate::db::DbError::Corrupt("no genesis header".to_string())))?;
        let light = LightClient::new(self.chain.light_params(), genesis, self.chain.genesis()).map_err(bad_snapshot)?;
        self.sync.snapshot = Some(SnapshotSync::new(light, checkpoint));

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
        // Every header the peers have is checked, as far as finality lets it: pick the pivot.
        if matches!(snapshot.phase, Phase::Headers) && snapshot.light().next().is_none() {
            return self.pick_pivot();
        }

        let (asked, request) = match &snapshot.phase {
            Phase::Checkpoint => (Asked::Checkpoint, SyncRequest::Checkpoint {
                block: snapshot.checkpoint.expect("fetching one").0,
            }),
            Phase::Headers => match snapshot.light().next().expect("checked above") {
                LightRequest::Headers { from, .. } => (Asked::Headers { from }, SyncRequest::Headers {
                    from,
                    count: MAX_SYNC_HEADERS.min(MAX_LIGHT_HEADERS),
                }),
                LightRequest::Finality { from, .. } => (Asked::Finality { from }, SyncRequest::Finality { from, count: MAX_SYNC_FINALITY }),
            },
            Phase::Block { pivot } => (Asked::Block, SyncRequest::Blocks { from: *pivot, count: 1 }),
            Phase::State { pivot, .. } => {
                let block = snapshot.light().sealed_at(*pivot).expect("a checked header").id();
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

    /// Takes a peer's headers, finality, state or block while syncing from a snapshot.
    pub(super) fn on_snapshot_response(&mut self, peer: PeerId, response: SyncResponse, now: u64) -> Result<(), NodeError> {
        let Some(snapshot) = self.sync.snapshot.as_mut() else {
            return Ok(());
        };
        let Some((_, asked)) = snapshot.asking.filter(|(asked, _)| *asked == peer) else {
            log::debug!("[node] ignored an answer {peer} sent unasked");

            return Ok(());
        };

        match (asked, response) {
            (Asked::Checkpoint, SyncResponse::Checkpoint(answer)) => {
                snapshot.asking = None;
                self.take_checkpoint(peer, answer)?;
            }
            (Asked::Headers { from }, SyncResponse::Headers(headers)) => {
                snapshot.asking = None;
                self.take_headers(peer, from, headers, now);
            }
            (Asked::Finality { from }, SyncResponse::Finality(steps)) => {
                snapshot.asking = None;

                if let Err(err) = snapshot.light_mut().deliver_finality(from, steps) {
                    log::warn!("[node] {peer} sent finality that does not check: {err}; asking others");
                    snapshot.distrusted.insert(peer);
                }
            }
            (Asked::State { depth, prefix }, SyncResponse::State(part)) => {
                snapshot.asking = None;
                let from_checkpoint = snapshot.starts_at_checkpoint();
                let Phase::State { pivot, walk } = &mut snapshot.phase else {
                    return Ok(());
                };

                match part {
                    // The peer no longer serves the pivot: move it up, and walk again from there,
                    // which skips everything already right. From a checkpoint there is no going
                    // back to running every block, so it keeps trying.
                    None if snapshot.moves >= MAX_MOVES && !from_checkpoint => {
                        log::warn!("[node] peers serve no pivot for long: syncing by running blocks instead");
                        self.sync.snapshot = None;
                    }
                    None => {
                        log::info!("[node] {peer} no longer serves state at height {pivot}; moving the pivot up");
                        walk.retry();
                        snapshot.moves += 1;
                        snapshot.phase = Phase::Headers;
                        snapshot.light_mut().refresh();
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
                let expected = snapshot.light().sealed_at(pivot).expect("a checked header").clone();

                match blocks.into_iter().next().filter(|block| block.fits(&expected, self.chain.hash())) {
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

    /// Takes the checkpoint start a peer sent: headers that link up by their hashes to the block
    /// this node trusts, so every one of them is trusted too, and the checkpoint's total work,
    /// taken on the peer's word. The light client starts there, and checking goes on as from
    /// genesis.
    fn take_checkpoint(&mut self, peer: PeerId, answer: Option<crate::net::CheckpointAnswer>) -> Result<(), NodeError> {
        let snapshot = self.sync.snapshot.as_mut().expect("taking one");
        let (trusted, _) = snapshot.checkpoint.expect("fetching one");
        let Some(answer) = answer else {
            log::warn!("[node] {peer} does not serve checkpoint {trusted}; asking others");
            snapshot.distrusted.insert(peer);

            return Ok(());
        };
        let genesis = self.chain.headers_from(0, 1)?.pop().ok_or(NodeError::Db(crate::db::DbError::Corrupt("no genesis header".to_string())))?;

        // The start's updates are checked as the client starts on them: kept with the rest.
        let updates: Vec<_> = answer.start.updates.iter().flatten().cloned().collect();

        match LightClient::from_checkpoint(self.chain.light_params(), genesis, self.chain.genesis(), trusted, answer.start) {
            Ok(mut light) => {
                light.keep_checked(crate::light::Kept { justifications: Vec::new(), updates });
                let height = light.tip().height;
                let counted = light.work_at(height).expect("the checkpoint is followed");
                log::info!("[node] starting at checkpoint {trusted}, height {height}, with headers from {} from {peer}", light.lowest());
                snapshot.checkpoint = Some((trusted, answer.work.saturating_sub(counted)));
                snapshot.light = Some(light);
                snapshot.phase = Phase::Headers;
            }
            Err(err) => {
                log::warn!("[node] {peer} sent a checkpoint start that does not lead to {trusted}: {err}; asking others");
                snapshot.distrusted.insert(peer);
            }
        }

        Ok(())
    }

    /// Hands headers a peer sent to the light client, which checks them by consensus and keeps
    /// them; a peer whose headers do not check, or whose chain is lighter than what it claims, is
    /// not asked again.
    fn take_headers(&mut self, peer: PeerId, from: u64, headers: Vec<SealedHeader>, now: u64) {
        let snapshot = self.sync.snapshot.as_mut().expect("taking one");

        match snapshot.light_mut().deliver_headers(from, headers, now) {
            Ok(Extended::Lighter) => {
                log::warn!("[node] {peer} claims more work than the headers it sends hold; asking others");
                snapshot.distrusted.insert(peer);
            }
            Ok(_) => {}
            Err(err) => {
                log::warn!("[node] {peer} sent headers that do not check: {err}; asking others");
                snapshot.distrusted.insert(peer);
            }
        }
    }

    /// Picks the pivot once every header is checked: the snapshot depth below the newest, never
    /// below the checkpoint, and starts taking its state. A chain shorter than twice the depth is
    /// run instead, as a snapshot would save nothing.
    fn pick_pivot(&mut self) -> Result<(), NodeError> {
        let depth = self.tuning.snapshot_depth.min(self.chain.max_reorg_depth() / 2);
        let snapshot = self.sync.snapshot.as_mut().expect("taking one");
        let height = snapshot.checked();

        if height < 2 * self.tuning.snapshot_depth && !snapshot.starts_at_checkpoint() {
            log::info!("[node] the chain is only {height} blocks long: syncing by running them instead");
            self.sync.snapshot = None;

            return self.sync_next();
        }

        // Never below the checkpoint: its state is the first this node can hold.
        let least = snapshot.checkpoint_height().unwrap_or(0);
        let pivot = height.saturating_sub(depth).max(least);
        let root = snapshot.light().sealed_at(pivot).expect("a checked header").header.state_root;
        log::info!("[node] checked {height} headers; taking state at height {pivot}");
        snapshot.phase = Phase::State {
            pivot,
            walk: SnapshotWalk::new(root),
        };

        self.snapshot_next()
    }

    /// Makes the pivot the tip, and goes back to syncing as usual from it.
    fn install(&mut self, block: Block, pivot: u64) -> Result<(), NodeError> {
        let mut snapshot = self.sync.snapshot.take().expect("taking one");
        let kept = snapshot.light_mut().take_checked();
        let light = snapshot.light();
        // From genesis, the headers after it; from a checkpoint, every one held, the trusted ones
        // below the checkpoint included.
        let first = light.lowest().max(1);
        let headers: Vec<SealedHeader> = (first..=pivot).map(|height| light.sealed_at(height).expect("a checked header").clone()).collect();
        let interval = self.chain.finality_config().map_or(1, |config| config.checkpoint_interval().max(1));
        // The checkpoint the node holds as final: the one it trusts, or a later one the light
        // client saw finalized among the headers installed.
        let trusted = snapshot.checkpoint.and_then(|(block, _)| {
            let height = snapshot.checkpoint_height()?;

            Some(Checkpoint { epoch: height / interval, block })
        });
        let seen = Some(light.finalized()).filter(|finalized| finalized.epoch > 0 && finalized.epoch * interval <= pivot);
        let checkpoint = match (trusted, seen) {
            (Some(trusted), Some(seen)) if seen.epoch > trusted.epoch => Some(seen),
            (Some(trusted), _) => Some(trusted),
            (None, seen) => seen,
        };
        self.chain.install_snapshot(&headers, &block, snapshot.work_at(pivot), checkpoint, &kept).map_err(NodeError::Chain)?;
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

/// A light client that could not start on this node's own chain: the chain's fault, never a peer's.
fn bad_snapshot(err: LightError) -> NodeError {
    NodeError::Chain(crate::chain::ChainError::BadSnapshot(format!("a light client of this chain cannot start: {err}")))
}
