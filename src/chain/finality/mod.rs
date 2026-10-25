//! Finality: when blocks can no longer come off the chain. Genesis picks one of three kinds, each
//! behind its own cargo feature, on by default:
//!
//! - ffg (ffg.rs): Casper FFG-style. Validators bond on chain and vote on checkpoints, the block
//!   at the start of each epoch, with links from a justified checkpoint to a later one. Two thirds
//!   of the bonded weight voting for one link justifies its target; justifying the very next
//!   checkpoint finalizes the source.
//! - authority (authority.rs): the same votes, from signers genesis names, a threshold of them
//!   justifying, who change by their own approval. No bonds, rewards are optional, and nobody
//!   can be slashed.
//! - depth (depth.rs): no votes. A block is final once enough blocks are on top of it.
//!
//! The chain talks to each through the Finality trait. Whatever the kind, it never rolls back past
//! the finalized checkpoint, and work alone only moves it to branches that hold the justified one.
//! The two vote-based kinds share votes.rs, which counts votes, stores justifications and pays
//! voters; they differ only in their electorate: who votes, with what weight, and how much it
//! takes.
//!
//! What a node learns about votes and justifications lives in the Finality column, outside block
//! state: it comes from the network, and every justification carries its own proof. That holds
//! the votes it counted or held back too, so a restart counts them again.

use {
    super::{Chain, ImportError, Imported, Workers, id_key, read_value, stored_header, stored_header_at, work_of},
    crate::crypto::keypair::PublicKey,
    crate::crypto::verifiable::VerifyError,
    crate::db::{Column, Database, DbError, Read, State},
    crate::structs::block::{Block, BlockHeader},
    crate::structs::hash::Hash,
    crate::structs::vote::{Checkpoint, Justification, SignedVote, Vote},
    r#macro::document,
    std::fmt,
    std::sync::Arc,
    wincode::{SchemaRead, SchemaWrite},
};

#[cfg(feature = "finality-authority")]
mod authority;
mod config;
#[cfg(feature = "finality-depth")]
mod depth;
#[cfg(feature = "finality-ffg")]
mod ffg;
#[cfg(all(test, any(feature = "finality-ffg", feature = "finality-authority", feature = "finality-depth")))]
mod tests;
#[cfg(any(feature = "finality-ffg", feature = "finality-authority"))]
mod votes;

#[cfg(feature = "finality-authority")]
pub use authority::AuthorityConfig;
pub use config::FinalityConfig;
#[cfg(feature = "finality-depth")]
pub use depth::DepthConfig;
#[cfg(feature = "finality-ffg")]
pub use ffg::{FfgConfig, GenesisValidator};

const JUSTIFIED_KEY: &[u8] = b"justified";
const FINALIZED_KEY: &[u8] = b"finalized";
const EVIDENCE_KEY: &[u8] = b"evidence";

#[document(
    summary = "A kind of finality, as the chain uses it. FinalityConfig::build makes one from genesis. apply is consensus: it runs on every block, and every node must agree on it. The rest is what this node has learned, from votes or from its own main chain. Kinds without votes keep the default vote methods, which take none.",
    category = "Finality",
    see_also = "FinalityConfig, View, Chain::add_vote",
)]
pub trait Finality: Send {
    #[document(parent = "Finality", summary = "The kind's name, as genesis files and the API spell it.")]
    fn name(&self) -> &'static str;

    #[document(
        parent = "Finality",
        summary = "Blocks per epoch. Each epoch's first block is its checkpoint; 1 when every block is one.",
    )]
    fn checkpoint_interval(&self) -> u64;

    #[document(
        parent = "Finality",
        summary = "The newest finalized checkpoint: nothing at or below it ever comes off. Genesis until anything later is.",
    )]
    fn finalized(&self) -> Checkpoint;

    #[document(
        parent = "Finality",
        summary = "The newest justified checkpoint: a branch must hold it for work alone to make it the main chain. Never older than the finalized one.",
    )]
    fn justified(&self) -> Checkpoint;

    #[document(parent = "Finality", summary = "The height of a checkpoint's block.")]
    fn height_of(&self, checkpoint: &Checkpoint) -> u64 {
        checkpoint.epoch.saturating_mul(self.checkpoint_interval())
    }

    #[document(
        parent = "Finality",
        summary = "Checks the proofs a block carries and updates state for them, after its transactions run. Consensus: every node runs it the same way.",
        params = {
            state = "The state the block is being run on.",
            block = "The block.",
        },
        returns = "Invalid for a proof that does not check out.",
    )]
    fn apply(&self, state: &Arc<State>, block: &Block) -> Result<(), ImportError>;

    #[document(
        parent = "Finality",
        summary = "Catches up after the main chain moves. The chain calls it whenever its tip changes.",
    )]
    fn tip_moved(&mut self, _chain: &View) -> Result<(), DbError> {
        Ok(())
    }

    #[document(
        parent = "Finality",
        summary = "Counts a signed vote.",
        returns = "What changed. Conflicting votes come back as evidence instead of being counted.",
    )]
    fn add_vote(&mut self, _chain: &View, _vote: SignedVote) -> Result<FinalityUpdate, FinalityError> {
        Err(FinalityError::NoVotes)
    }

    #[document(parent = "Finality", summary = "Counts the votes held back earlier that can be placed now.")]
    fn retry_votes(&mut self, _chain: &View) -> Result<FinalityUpdate, FinalityError> {
        Ok(FinalityUpdate::default())
    }

    #[document(
        parent = "Finality",
        summary = "Checks a justification from a peer: its link, its votes, and that they are enough.",
        returns = "false for one this node holds already.",
    )]
    fn check_justification(&self, _chain: &View, _justification: &Justification) -> Result<bool, FinalityError> {
        Err(FinalityError::NoVotes)
    }

    #[document(
        parent = "Finality",
        summary = "Takes a checked justification whose target is on the main chain: stores it, and moves the justified and finalized checkpoints.",
    )]
    fn justify(&mut self, _chain: &View, _justification: Justification) -> Result<FinalityUpdate, FinalityError> {
        Err(FinalityError::NoVotes)
    }

    #[document(
        parent = "Finality",
        summary = "The vote an honest validator with this voting key should sign now, if any.",
    )]
    fn vote_for(&self, _chain: &View, _voter: &PublicKey) -> Result<Option<Vote>, DbError> {
        Ok(None)
    }

    #[document(
        parent = "Finality",
        summary = "Remembers a vote this node signed, so vote_for never offers one that conflicts with it.",
    )]
    fn record_own_vote(&self, _chain: &View, _vote: &Vote) -> Result<(), DbError> {
        Ok(())
    }

    #[document(
        parent = "Finality",
        summary = "Justifications whose voters the tip's state has not paid yet: what a producer puts in its next block.",
    )]
    fn unpaid_proofs(&self, _chain: &View) -> Result<Vec<Justification>, DbError> {
        Ok(Vec::new())
    }

    #[document(
        parent = "Finality",
        summary = "The justifications this node holds for main-chain checkpoints, from an epoch on, oldest first.",
    )]
    fn justifications_from(&self, _chain: &View, _epoch: u64, _count: usize) -> Result<Vec<Justification>, DbError> {
        Ok(Vec::new())
    }
}

#[document(
    summary = "What finality reads of the chain: its blocks, its main chain, its tip and the state there, and the database it keeps what it learns in.",
    category = "Finality",
    see_also = "Finality",
)]
pub struct View<'a> {
    db: &'a Arc<dyn Database>,
    tip: &'a BlockHeader,
    genesis: Hash,
    max_reorg_depth: u64,
    workers: &'a Arc<Workers>,
}

impl<'a> View<'a> {
    pub(in crate::chain) fn new(
        db: &'a Arc<dyn Database>,
        tip: &'a BlockHeader,
        genesis: Hash,
        max_reorg_depth: u64,
        workers: &'a Arc<Workers>,
    ) -> View<'a> {
        View {
            db,
            tip,
            genesis,
            max_reorg_depth,
            workers,
        }
    }

    #[document(parent = "View", summary = "The chain's worker threads, for checking many signatures at once.")]
    pub fn workers(&self) -> &Arc<Workers> {
        self.workers
    }

    #[document(parent = "View", summary = "The newest main-chain block's header.")]
    pub fn tip(&self) -> &BlockHeader {
        self.tip
    }

    #[document(parent = "View", summary = "The genesis block's id, which votes are signed over.")]
    pub fn genesis(&self) -> Hash {
        self.genesis
    }

    #[document(parent = "View", summary = "The database, for what finality learns: it writes only the Finality column.")]
    pub fn db(&self) -> &Arc<dyn Database> {
        self.db
    }

    #[document(parent = "View", summary = "A read-only view of the state at the tip.")]
    pub fn state(&self) -> State {
        State::new(self.db.clone())
    }

    #[document(parent = "View", summary = "A stored block's header by id, on the main chain or a side branch, the blocks before a snapshot included.")]
    pub fn header(&self, id: &Hash) -> Result<Option<BlockHeader>, DbError> {
        Ok(stored_header(&**self.db, id)?.map(|sealed| sealed.header))
    }

    #[document(parent = "View", summary = "The main-chain block's header at a height.")]
    pub fn header_at(&self, height: u64) -> Result<Option<BlockHeader>, DbError> {
        Ok(stored_header_at(&**self.db, height)?.map(|sealed| sealed.header))
    }

    #[document(parent = "View", summary = "Whether a stored block is on the main chain.")]
    pub fn on_main_chain(&self, id: &Hash) -> Result<bool, DbError> {
        let Some(header) = self.header(id)? else {
            return Ok(false);
        };
        let at_height: Option<Hash> = read_value(&**self.db, Column::Heights, &header.height.to_be_bytes())?;

        Ok(at_height == Some(*id))
    }

    #[document(parent = "View", summary = "Whether the block with this header has ancestor, at height, on its branch.")]
    pub fn descends(&self, header: &BlockHeader, ancestor: &Hash, height: u64) -> Result<bool, DbError> {
        let mut current = header.clone();

        while current.height > height {
            match self.header(&current.parent)? {
                Some(parent) => current = parent,
                None => return Ok(false),
            }
        }

        Ok(current.id() == *ancestor)
    }

    #[document(
        parent = "View",
        summary = "The height where a stored block's branch meets the main chain, if it does within max_reorg_depth.",
    )]
    pub fn fork_height(&self, header: &BlockHeader) -> Result<Option<u64>, DbError> {
        let mut current = header.clone();

        for _ in 0..=self.max_reorg_depth {
            if self.on_main_chain(&current.id())? {
                return Ok(Some(current.height));
            }

            match self.header(&current.parent)? {
                Some(parent) => current = parent,
                None => return Ok(None),
            }
        }

        Ok(None)
    }
}

#[document(
    summary = "Two conflicting votes signed by one voter: what Tx::Slash takes on a chain with staking.",
    category = "Finality",
    see_also = "Tx::Slash, Vote::conflicts",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct Evidence {
    #[document(summary = "The vote seen first.")]
    pub first: SignedVote,
    #[document(summary = "The vote that conflicts with it.")]
    pub second: SignedVote,
}

impl crate::crypto::hashable::Hashable for Evidence {}

#[document(
    summary = "What a vote or justification changed about finality. Several can happen at once: a vote can justify one checkpoint and finalize another.",
    category = "Finality",
    see_also = "Chain::add_vote, Chain::add_justification",
)]
#[derive(Debug, Default)]
pub struct FinalityUpdate {
    #[document(summary = "Checkpoints newly justified, oldest first.")]
    pub justified: Vec<Checkpoint>,
    #[document(summary = "The checkpoint newly finalized, if any.")]
    pub finalized: Option<Checkpoint>,
    #[document(summary = "Proof that a validator cheated, when this vote conflicts with one it signed before.")]
    pub evidence: Option<Evidence>,
    #[document(summary = "When a justification moved the main chain to another branch: the blocks taken off, newest first, and those put on, oldest first.")]
    pub reorganized: Option<(Vec<Block>, Vec<Block>)>,
    #[document(summary = "Whether the vote was held back to count later, because its checkpoints are not on this node's main chain yet.")]
    pub pending: bool,
}

impl FinalityUpdate {
    #[cfg_attr(not(any(feature = "finality-ffg", feature = "finality-authority")), allow(dead_code))]
    fn merge(&mut self, other: FinalityUpdate) {
        self.justified.extend(other.justified);
        self.finalized = other.finalized.or(self.finalized);
        self.evidence = other.evidence.or(self.evidence);
        self.reorganized = other.reorganized.or(self.reorganized.take());
    }
}

#[document(
    summary = "Why a vote or justification was not taken.",
    category = "Finality",
)]
#[derive(Debug)]
pub enum FinalityError {
    #[document(summary = "This chain has no finality.")]
    Disabled,
    #[document(summary = "This chain's finality takes no votes.")]
    NoVotes,
    #[document(summary = "A signature does not match, or a vote's target does not follow its source.")]
    Invalid(VerifyError),
    #[document(summary = "The voter is not in its target epoch's validator set.")]
    NotValidator,
    #[document(summary = "The target is at or before the finalized checkpoint.")]
    Stale,
    #[document(summary = "A justification this node cannot check: its checkpoints are not on its main chain, its source is not justified, or the votes fall short.")]
    Unverifiable(&'static str),
    #[document(summary = "The database failed.")]
    Db(DbError),
}

impl From<DbError> for FinalityError {
    fn from(err: DbError) -> FinalityError {
        FinalityError::Db(err)
    }
}

impl From<ImportError> for FinalityError {
    fn from(err: ImportError) -> FinalityError {
        match err {
            ImportError::Db(err) => FinalityError::Db(err),
            other => FinalityError::Unverifiable(match other {
                ImportError::Invalid(_) => "the justified branch does not run",
                _ => "the justified branch cannot be followed",
            }),
        }
    }
}

impl fmt::Display for FinalityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FinalityError::Disabled => write!(f, "this chain has no finality"),
            FinalityError::NoVotes => write!(f, "this chain's finality takes no votes"),
            FinalityError::Invalid(err) => write!(f, "{err}"),
            FinalityError::NotValidator => write!(f, "not a validator in that epoch"),
            FinalityError::Stale => write!(f, "already finalized past it"),
            FinalityError::Unverifiable(why) => write!(f, "cannot check it: {why}"),
            FinalityError::Db(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for FinalityError {}

fn justification_key(block: &Hash) -> Vec<u8> {
    let mut key = b"j:".to_vec();
    key.extend_from_slice(&id_key(block));

    key
}

#[document(
    summary = "The justification this node holds for a checkpoint block, read straight from a database.",
    category = "Finality",
)]
pub fn stored_justification(db: &dyn Read, block: &Hash) -> Result<Option<Justification>, DbError> {
    read_value(db, Column::Finality, &justification_key(block))
}

#[document(
    summary = "The latest justified and finalized checkpoints this node knows, read straight from a database. None for both on a chain without finality, or before anything beyond genesis.",
    category = "Finality",
)]
pub fn stored_finality(db: &dyn Read) -> Result<(Option<Checkpoint>, Option<Checkpoint>), DbError> {
    Ok((
        read_value(db, Column::Finality, JUSTIFIED_KEY)?,
        read_value(db, Column::Finality, FINALIZED_KEY)?,
    ))
}

#[document(
    summary = "Evidence of cheating this node has seen, newest last, read straight from a database.",
    category = "Finality",
)]
pub fn stored_evidence(db: &dyn Read) -> Result<Vec<Evidence>, DbError> {
    Ok(read_value(db, Column::Finality, EVIDENCE_KEY)?.unwrap_or_default())
}

impl Chain {
    pub(super) fn view(&self) -> View<'_> {
        View::new(&self.db, &self.tip, self.genesis, self.max_reorg_depth, &self.workers)
    }

    /// The finality and a view of the rest of the chain, borrowed apart so finality can learn
    /// while it reads.
    fn parts(&mut self) -> (Option<&mut (dyn Finality + 'static)>, View<'_>) {
        let Chain {
            finality,
            db,
            tip,
            genesis,
            max_reorg_depth,
            workers,
            ..
        } = self;

        (finality.as_deref_mut(), View::new(db, tip, *genesis, *max_reorg_depth, workers))
    }

    /// Lets finality catch up with the tip.
    pub(super) fn tip_moved(&mut self) -> Result<(), DbError> {
        match self.parts() {
            (Some(finality), view) => finality.tip_moved(&view),
            (None, _) => Ok(()),
        }
    }

    /// The height no block may be added at or below, and no reorg may reach: the finalized
    /// checkpoint's.
    pub(super) fn finalized_height(&self) -> Option<u64> {
        let finality = self.finality.as_ref()?;

        Some(finality.height_of(&finality.finalized()))
    }

    #[document(parent = "Chain", summary = "The chain's finality settings, if it has finality.")]
    pub fn finality_config(&self) -> Option<&FinalityConfig> {
        self.finality_config.as_ref()
    }

    #[document(parent = "Chain", summary = "Whether the chain's finality takes votes: false without finality, or for depth finality.")]
    pub fn takes_votes(&self) -> bool {
        self.finality_config.as_ref().is_some_and(FinalityConfig::votes)
    }

    #[document(parent = "Chain", summary = "Whether validators bond on this chain, so staking transactions run and cheaters can be slashed.")]
    pub fn has_staking(&self) -> bool {
        self.rules.staking.is_some()
    }

    #[document(parent = "Chain", summary = "The latest finalized checkpoint, if the chain has finality. Genesis until anything later is.")]
    pub fn finalized(&self) -> Option<Checkpoint> {
        self.finality.as_ref().map(|finality| finality.finalized())
    }

    #[document(parent = "Chain", summary = "The latest justified checkpoint on the main chain, if the chain has finality.")]
    pub fn justified(&self) -> Option<Checkpoint> {
        self.finality.as_ref().map(|finality| finality.justified())
    }

    #[document(parent = "Chain", summary = "The justification this node holds for a checkpoint block.")]
    pub fn justification(&self, block: &Hash) -> Result<Option<Justification>, DbError> {
        stored_justification(&*self.db, block)
    }

    #[document(
        parent = "Chain",
        summary = "The justifications this node holds for main-chain checkpoints, from an epoch on, oldest first. What it serves to syncing peers.",
    )]
    pub fn justifications_from(&self, epoch: u64, count: usize) -> Result<Vec<Justification>, DbError> {
        match &self.finality {
            Some(finality) => finality.justifications_from(&self.view(), epoch, count),
            None => Ok(Vec::new()),
        }
    }

    #[document(
        parent = "Chain",
        summary = "Counts a validator's signed vote. Once enough of an epoch's voters vote for the same link, its target is justified, and its source finalized when the target is the very next checkpoint. Votes this node cannot place yet are held and counted later, after imports, by retry_votes.",
        returns = "What changed. Conflicting votes come back as evidence instead of being counted.",
    )]
    pub fn add_vote(&mut self, vote: SignedVote) -> Result<FinalityUpdate, FinalityError> {
        let (finality, view) = self.parts();

        finality.ok_or(FinalityError::Disabled)?.add_vote(&view, vote)
    }

    #[document(
        parent = "Chain",
        summary = "Counts the votes held back earlier that this node can now place. Call it after the main chain moves.",
    )]
    pub fn retry_votes(&mut self) -> Result<FinalityUpdate, FinalityError> {
        match self.parts() {
            (Some(finality), view) => finality.retry_votes(&view),
            (None, _) => Ok(FinalityUpdate::default()),
        }
    }

    #[document(
        parent = "Chain",
        summary = "Takes a justification from a peer, checking every vote in it against the target epoch's voters. A target on another branch, later than this node's latest justified checkpoint, moves the main chain to that branch, never past the finalized checkpoint.",
        returns = "What changed, or why it cannot be checked.",
    )]
    pub fn add_justification(&mut self, justification: Justification) -> Result<FinalityUpdate, FinalityError> {
        let (finality, view) = self.parts();

        if !finality.ok_or(FinalityError::Disabled)?.check_justification(&view, &justification)? {
            return Ok(FinalityUpdate::default());
        }

        let target = justification.target.block;
        let reorganized = match self.on_main_chain(&target)? {
            true => None,
            false => Some(self.adopt(&target)?),
        };
        let (finality, view) = self.parts();
        let mut update = finality.expect("checked above").justify(&view, justification)?;
        update.reorganized = reorganized;

        Ok(update)
    }

    #[document(
        parent = "Chain",
        summary = "The vote an honest validator with this voting key should sign now, if any: for the newest checkpoint at least vote_depth blocks deep that it has not voted on, from the latest justified checkpoint. Never one that would conflict with a vote it signed before, as recorded by record_own_vote.",
    )]
    pub fn vote_for(&self, voter: &PublicKey) -> Result<Option<Vote>, DbError> {
        match &self.finality {
            Some(finality) => finality.vote_for(&self.view(), voter),
            None => Ok(None),
        }
    }

    #[document(
        parent = "Chain",
        summary = "Remembers a vote this node's validator signed, before it goes out, so vote_for never offers a conflicting one, even after a restart.",
    )]
    pub fn record_own_vote(&self, vote: &Vote) -> Result<(), DbError> {
        match &self.finality {
            Some(finality) => finality.record_own_vote(&self.view(), vote),
            None => Ok(()),
        }
    }

    #[document(
        parent = "Chain",
        summary = "Justifications for main-chain checkpoints whose voters the tip's state has not paid yet, oldest first: what a producer puts in its next block. None when votes earn nothing.",
    )]
    pub fn unpaid_proofs(&self) -> Result<Vec<Justification>, DbError> {
        match &self.finality {
            Some(finality) => finality.unpaid_proofs(&self.view()),
            None => Ok(Vec::new()),
        }
    }

    /// Moves the main chain to the best known branch through a justified checkpoint that is not
    /// on it, never past the finalized checkpoint.
    fn adopt(&mut self, target: &Hash) -> Result<(Vec<Block>, Vec<Block>), FinalityError> {
        let block = self.block(target)?.ok_or(FinalityError::Unverifiable("its block is not known here"))?;
        let mut best = (work_of(&*self.db, target)?, block.clone());

        for tip in self.tips.clone() {
            let Some(candidate) = self.block(&tip)? else {
                continue;
            };

            if candidate.header.height <= block.header.height
                || !self.view().descends(&candidate.header, target, block.header.height)?
            {
                continue;
            }

            let work = work_of(&*self.db, &tip)?;

            if work > best.0 {
                best = (work, candidate);
            }
        }

        match self.reorganize(&best.1, best.0, true)? {
            Imported::Reorganized { undone, applied } => Ok((undone, applied)),
            _ => Err(FinalityError::Unverifiable("the justified branch splits off past the finalized checkpoint")),
        }
    }
}
