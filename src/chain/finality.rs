//! Finality, in the style of Casper FFG. Bonded validators vote on checkpoints, the block at the
//! start of each epoch, with links from a justified checkpoint to a later one. Two thirds of the
//! bonded weight voting for the same link justifies its target; justifying the very next
//! checkpoint finalizes the source. The chain never rolls back past a finalized checkpoint, and
//! only follows branches that hold the latest justified one.
//!
//! What each node knows about votes and justifications lives in the Finality column, outside
//! block state: it is learned from the network, and every justification carries its own proof.
//! Validator sets are chain state, fixed activation_delay epochs ahead at each checkpoint, so
//! every branch that split off after that checkpoint counts votes with the same weights.

use {
    super::{Chain, ImportError, Imported, id_key, read_value},
    crate::crypto::hashable::Hashable,
    crate::crypto::keypair::PublicKey,
    crate::crypto::verifiable::VerifyError,
    crate::db::{Column, DbError, Read, State},
    crate::registry::validators::{Staking, ValidatorRecord, ValidatorSet, Validators},
    crate::structs::address::Address,
    crate::structs::block::Block,
    crate::structs::hash::Hash,
    crate::structs::u256::{U256Le, u256},
    crate::structs::vote::{Checkpoint, Justification, SignedVote, Vote},
    r#macro::document,
    std::collections::{BTreeMap, HashMap, VecDeque},
    std::fmt,
    std::sync::Arc,
    wincode::{SchemaRead, SchemaWrite},
};

const JUSTIFIED_KEY: &[u8] = b"justified";
const FINALIZED_KEY: &[u8] = b"finalized";
const EVIDENCE_KEY: &[u8] = b"evidence";
/// Votes a node holds while it cannot count them yet: their checkpoints are not on its main
/// chain, or their source is not justified yet.
const MAX_PENDING: usize = 4_096;
/// Pieces of evidence a node keeps for operators to report.
const MAX_EVIDENCE: usize = 64;
/// Votes per validator kept to catch one vote surrounding another.
const MAX_HISTORY: usize = 64;
/// How far back a producer looks for justifications whose voters are still unpaid.
const UNPAID_WINDOW: u64 = 64;

#[document(
    summary = "A validator bonded from genesis.",
    category = "Finality",
    see_also = "FinalityConfig",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct GenesisValidator {
    #[document(summary = "The account that owns the bond.")]
    pub account: Address,
    #[document(summary = "The key that signs its votes.")]
    pub voter: PublicKey,
    #[document(summary = "Its bond, minted at genesis.")]
    #[wincode(with = "U256Le")]
    pub bond: u256,
}

impl Hashable for GenesisValidator {}

#[document(
    summary = "Every finality parameter a chain runs under, set in its genesis. Nothing about finality is fixed in code: change any of these in a genesis file to start a chain that uses them.",
    category = "Finality",
    see_also = "Genesis, Staking, Vote",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct FinalityConfig {
    #[document(summary = "Blocks per epoch. Each epoch's first block is its checkpoint.")]
    pub checkpoint_interval: u64,
    #[document(summary = "How many blocks on top of a checkpoint before validators vote for it, so they rarely vote for a block that is about to lose a fork.")]
    pub vote_depth: u64,
    #[document(summary = "The smallest bond.")]
    #[wincode(with = "U256Le")]
    pub min_bond: u256,
    #[document(summary = "Epochs before a bond, unbond or slash changes who votes. At least 1; larger keeps validator sets agreeing across deeper forks.")]
    pub activation_delay: u64,
    #[document(summary = "Epochs a bond stays locked, and slashable, after its validator stops voting.")]
    pub withdrawal_delay: u64,
    #[document(summary = "A reporter of cheating gets the bond divided by this; the rest is burned.")]
    pub reporter_divisor: u64,
    #[document(summary = "The most validators registered at once.")]
    pub max_validators: u32,
    #[document(summary = "What every voter in a justification gets when a block carries it.")]
    #[wincode(with = "U256Le")]
    pub vote_reward: u256,
    #[document(summary = "The most justifications one block may carry.")]
    pub max_proofs_per_block: u32,
    #[document(summary = "Validators bonded from the start. With none, nothing finalizes until someone bonds.")]
    pub validators: Vec<GenesisValidator>,
}

impl Hashable for FinalityConfig {}

/// A million coins at 18 decimals.
fn coins(count: u64) -> u256 {
    u256::from(count) * u256::from(10u64).pow(18)
}

impl FinalityConfig {
    #[document(
        parent = "FinalityConfig",
        summary = "The development settings: an epoch every 8 blocks, votes 2 blocks deep, changes after 2 epochs, bonds locked 64 epochs after leaving, a 100-coin minimum bond, 1 coin per vote paid, a sixteenth of a slashed bond to its reporter, and one validator from genesis.",
        params = { validator = "The genesis validator's key, which both owns the bond and votes." },
    )]
    pub fn dev(validator: PublicKey) -> FinalityConfig {
        FinalityConfig {
            checkpoint_interval: 8,
            vote_depth: 2,
            min_bond: coins(100),
            activation_delay: 2,
            withdrawal_delay: 64,
            reporter_divisor: 16,
            max_validators: 1_000,
            vote_reward: coins(1),
            max_proofs_per_block: 4,
            validators: vec![GenesisValidator {
                account: validator.address(),
                voter: validator,
                bond: coins(10_000),
            }],
        }
    }

    #[document(parent = "FinalityConfig", summary = "The staking rules these settings make.")]
    pub fn staking(&self) -> Staking {
        Staking {
            min_bond: self.min_bond,
            activation_delay: self.activation_delay,
            withdrawal_delay: self.withdrawal_delay,
            reporter_divisor: self.reporter_divisor,
            max_validators: self.max_validators,
        }
    }

    #[document(parent = "FinalityConfig", summary = "The epoch a height falls in.")]
    pub fn epoch_of(&self, height: u64) -> u64 {
        height / self.checkpoint_interval
    }

    #[document(parent = "FinalityConfig", summary = "The height of an epoch's checkpoint.")]
    pub fn checkpoint_height(&self, epoch: u64) -> u64 {
        epoch.saturating_mul(self.checkpoint_interval)
    }

    /// Settings no chain could finalize with.
    pub(super) fn problem(&self) -> Option<&'static str> {
        if self.checkpoint_interval == 0 {
            return Some("checkpoint_interval must be at least 1");
        }

        if self.activation_delay == 0 {
            return Some("activation_delay must be at least 1");
        }

        if self.reporter_divisor == 0 {
            return Some("reporter_divisor must be at least 1");
        }

        if self.validators.len() > self.max_validators as usize {
            return Some("more genesis validators than max_validators");
        }

        None
    }
}

#[document(
    summary = "Two conflicting votes signed by one voter: what Tx::Slash takes.",
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

impl Hashable for Evidence {}

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
    #[document(summary = "A signature does not match, or a vote's target does not follow its source.")]
    Invalid(VerifyError),
    #[document(summary = "The voter is not in its target epoch's validator set.")]
    NotValidator,
    #[document(summary = "The target is at or before the finalized checkpoint.")]
    Stale,
    #[document(summary = "A justification this node cannot check: its checkpoints are not on its main chain, its source is not justified, or the votes do not add up to two thirds.")]
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
            FinalityError::Invalid(err) => write!(f, "{err}"),
            FinalityError::NotValidator => write!(f, "not a validator in that epoch"),
            FinalityError::Stale => write!(f, "already finalized past it"),
            FinalityError::Unverifiable(why) => write!(f, "cannot check it: {why}"),
            FinalityError::Db(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for FinalityError {}

/// Votes for one link, and the weight they add up to.
#[derive(Default)]
struct Tally {
    votes: BTreeMap<PublicKey, SignedVote>,
    weight: u256,
}

/// A voter's own votes, so it never signs one that conflicts with them.
#[derive(Clone, Copy, Debug, Default, SchemaWrite, SchemaRead)]
struct OwnVotes {
    last_target: u64,
    max_source: u64,
}

impl Hashable for OwnVotes {}

/// What a node knows about finality.
pub(super) struct FinalityState {
    pub(super) config: FinalityConfig,
    pub(super) justified: Checkpoint,
    pub(super) finalized: Checkpoint,
    links: HashMap<(Checkpoint, Checkpoint), Tally>,
    pending: VecDeque<SignedVote>,
    seen: HashMap<(PublicKey, u64), SignedVote>,
    history: HashMap<PublicKey, VecDeque<SignedVote>>,
}

impl FinalityState {
    pub(super) fn load(config: FinalityConfig, db: &dyn Read, genesis: Hash) -> Result<FinalityState, DbError> {
        let start = Checkpoint {
            epoch: 0,
            block: genesis,
        };

        Ok(FinalityState {
            config,
            justified: read_value(db, Column::Finality, JUSTIFIED_KEY)?.unwrap_or(start),
            finalized: read_value(db, Column::Finality, FINALIZED_KEY)?.unwrap_or(start),
            links: HashMap::new(),
            pending: VecDeque::new(),
            seen: HashMap::new(),
            history: HashMap::new(),
        })
    }
}

/// Registers genesis validators and fixes the validator sets genesis already decides.
pub(super) fn write_genesis(state: &State, config: &FinalityConfig) -> Result<(), DbError> {
    let validators = Validators::new(state);
    let mut voters = validators.voters()?;

    for validator in &config.validators {
        validators.register(&validator.voter, ValidatorRecord {
            account: validator.account,
            bond: validator.bond,
            active_from: 0,
            active_until: None,
            withdrawable_from: None,
            slashed: false,
        }, &mut voters);
    }

    for epoch in 0..=config.activation_delay {
        validators.fix_set(epoch)?;
    }

    Ok(())
}

fn justification_key(block: &Hash) -> Vec<u8> {
    let mut key = b"j:".to_vec();
    key.extend_from_slice(&id_key(block));

    key
}

fn own_key(voter: &PublicKey) -> Vec<u8> {
    let mut key = b"own:".to_vec();
    key.extend_from_slice(&voter.hashable_bytes_with_tag());

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
    #[document(parent = "Chain", summary = "The chain's finality settings, if it has finality.")]
    pub fn finality_config(&self) -> Option<&FinalityConfig> {
        self.finality.as_ref().map(|finality| &finality.config)
    }

    #[document(parent = "Chain", summary = "The latest finalized checkpoint, if the chain has finality. Genesis until anything later is.")]
    pub fn finalized(&self) -> Option<Checkpoint> {
        self.finality.as_ref().map(|finality| finality.finalized)
    }

    #[document(parent = "Chain", summary = "The latest justified checkpoint on the main chain, if the chain has finality.")]
    pub fn justified(&self) -> Option<Checkpoint> {
        self.finality.as_ref().map(|finality| finality.justified)
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
        let Some(config) = self.finality_config() else {
            return Ok(Vec::new());
        };
        let last = config.epoch_of(self.tip.height);
        let mut out = Vec::new();

        for epoch in epoch.max(1)..=last {
            if out.len() >= count {
                break;
            }

            if let Some(block) = self.block_at(config.checkpoint_height(epoch))?
                && let Some(justification) = self.justification(&block.id())?
            {
                out.push(justification);
            }
        }

        Ok(out)
    }

    #[document(
        parent = "Chain",
        summary = "Counts a validator's signed vote. Once two thirds of an epoch's weight vote for the same link, its target is justified, and its source finalized when the target is the very next checkpoint. Votes this node cannot place yet are held and counted later, after imports, by retry_votes.",
        returns = "What changed. Conflicting votes come back as evidence instead of being counted.",
    )]
    pub fn add_vote(&mut self, vote: SignedVote) -> Result<FinalityUpdate, FinalityError> {
        if self.finality.is_none() {
            return Err(FinalityError::Disabled);
        }

        vote.verify(&self.genesis).map_err(FinalityError::Invalid)?;

        if let Some(evidence) = self.watch(&vote)? {
            return Ok(FinalityUpdate {
                evidence: Some(evidence),
                ..FinalityUpdate::default()
            });
        }

        self.count(vote)
    }

    #[document(
        parent = "Chain",
        summary = "Counts the votes held back earlier that this node can now place. Call it after the main chain moves.",
    )]
    pub fn retry_votes(&mut self) -> Result<FinalityUpdate, FinalityError> {
        let Some(finality) = self.finality.as_mut() else {
            return Ok(FinalityUpdate::default());
        };
        let held: Vec<SignedVote> = finality.pending.drain(..).collect();
        let mut update = FinalityUpdate::default();

        for vote in held {
            match self.count(vote) {
                Ok(counted) => update.merge(counted),
                Err(FinalityError::Db(err)) => return Err(FinalityError::Db(err)),
                Err(_) => {}
            }
        }

        Ok(update)
    }

    #[document(
        parent = "Chain",
        summary = "Takes a justification from a peer, checking every vote in it against the target epoch's validator set. A target on another branch, later than this node's latest justified checkpoint, moves the main chain to that branch, never past the finalized checkpoint.",
        returns = "What changed, or why it cannot be checked.",
    )]
    pub fn add_justification(&mut self, justification: Justification) -> Result<FinalityUpdate, FinalityError> {
        let Some(finality) = &self.finality else {
            return Err(FinalityError::Disabled);
        };
        let (source, target) = (justification.source, justification.target);

        if target.epoch <= finality.finalized.epoch {
            return Err(FinalityError::Stale);
        }

        if self.justification(&target.block)?.is_some() {
            return Ok(FinalityUpdate::default());
        }

        if !self.is_justified(&source)? {
            return Err(FinalityError::Unverifiable("its source is not justified here"));
        }

        if !self.on_branch(&source, &target)? {
            return Err(FinalityError::Unverifiable("its source is not on its target's branch"));
        }

        let set = self.set_for(&target)?.ok_or(FinalityError::Unverifiable("its validator set is not known here"))?;
        let mut weight = u256::ZERO;
        let mut previous: Option<PublicKey> = None;

        for vote in &justification.votes {
            if previous.is_some_and(|previous| previous >= vote.vote.voter) {
                return Err(FinalityError::Unverifiable("its votes are not sorted by voter"));
            }

            if vote.vote.source != source || vote.vote.target != target {
                return Err(FinalityError::Unverifiable("a vote is for another link"));
            }

            vote.verify(&self.genesis).map_err(FinalityError::Invalid)?;
            weight += set.member(&vote.vote.voter).ok_or(FinalityError::NotValidator)?.weight;
            previous = Some(vote.vote.voter);
        }

        if !set.supermajority(weight) {
            return Err(FinalityError::Unverifiable("its votes are under two thirds"));
        }

        self.justify(justification)
    }

    #[document(
        parent = "Chain",
        summary = "The vote an honest validator with this voting key should sign now, if any: for the newest checkpoint at least vote_depth blocks deep that it has not voted on, from the latest justified checkpoint. Never one that would conflict with a vote it signed before, as recorded by record_own_vote.",
    )]
    pub fn vote_for(&self, voter: &PublicKey) -> Result<Option<Vote>, DbError> {
        let Some(finality) = &self.finality else {
            return Ok(None);
        };
        let config = &finality.config;
        let Some(deep) = self.tip.height.checked_sub(config.vote_depth) else {
            return Ok(None);
        };
        let epoch = config.epoch_of(deep);
        let source = finality.justified;
        let own: OwnVotes = read_value(&*self.db, Column::Finality, &own_key(voter))?.unwrap_or_default();

        if epoch == 0 || epoch <= own.last_target || epoch <= source.epoch || source.epoch < own.max_source {
            return Ok(None);
        }

        let Some(block) = self.block_at(config.checkpoint_height(epoch))? else {
            return Ok(None);
        };
        let target = Checkpoint {
            epoch,
            block: block.id(),
        };

        match self.set_for(&target)? {
            Some(set) if set.member(voter).is_some() => Ok(Some(Vote {
                source,
                target,
                voter: *voter,
            })),
            _ => Ok(None),
        }
    }

    #[document(
        parent = "Chain",
        summary = "Remembers a vote this node's validator signed, before it goes out, so vote_for never offers a conflicting one, even after a restart.",
    )]
    pub fn record_own_vote(&self, vote: &Vote) -> Result<(), DbError> {
        let key = own_key(&vote.voter);
        let mut own: OwnVotes = read_value(&*self.db, Column::Finality, &key)?.unwrap_or_default();
        own.last_target = own.last_target.max(vote.target.epoch);
        own.max_source = own.max_source.max(vote.source.epoch);
        let state = State::new(self.db.clone());
        state.put_value(Column::Finality, key, &own);

        self.db.commit(state.take_changes())
    }

    #[document(
        parent = "Chain",
        summary = "Justifications for main-chain checkpoints whose voters the tip's state has not paid yet, oldest first: what a producer puts in its next block.",
    )]
    pub fn unpaid_proofs(&self) -> Result<Vec<Justification>, DbError> {
        let Some(finality) = &self.finality else {
            return Ok(Vec::new());
        };
        let config = &finality.config;
        let state = self.state();
        let validators = Validators::new(&state);
        let newest = finality.justified.epoch;
        let mut proofs = Vec::new();

        for epoch in newest.saturating_sub(UNPAID_WINDOW).max(1)..=newest {
            if proofs.len() >= config.max_proofs_per_block as usize {
                break;
            }

            let Some(block) = self.block_at(config.checkpoint_height(epoch))? else {
                continue;
            };

            if let Some(proof) = self.justification(&block.id())?
                && !validators.paid(epoch)?
                && self.main_checkpoint(&proof.source)?
            {
                proofs.push(proof);
            }
        }

        Ok(proofs)
    }

    /// Records the vote for conflict checks, and returns evidence when it conflicts with one
    /// the same voter signed before.
    fn watch(&mut self, vote: &SignedVote) -> Result<Option<Evidence>, DbError> {
        let finality = self.finality.as_mut().expect("checked by the caller");
        let voter = vote.vote.voter;
        let mut found = None;

        match finality.seen.get(&(voter, vote.vote.target.epoch)) {
            Some(first) if first != vote => {
                found = Some(Evidence {
                    first: *first,
                    second: *vote,
                })
            }
            Some(_) => {}
            None => {
                finality.seen.insert((voter, vote.vote.target.epoch), *vote);
            }
        }

        let history = finality.history.entry(voter).or_default();

        if found.is_none() {
            found = history
                .iter()
                .find(|earlier| earlier.vote.conflicts(&vote.vote))
                .map(|first| Evidence {
                    first: *first,
                    second: *vote,
                });
        }

        if found.is_none() && !history.contains(vote) {
            history.push_back(*vote);

            if history.len() > MAX_HISTORY {
                history.pop_front();
            }
        }

        if let Some(evidence) = found {
            let mut stored = stored_evidence(&*self.db)?;

            if !stored.contains(&evidence) {
                stored.push(evidence);

                if stored.len() > MAX_EVIDENCE {
                    stored.remove(0);
                }

                let state = State::new(self.db.clone());
                let bytes = wincode::serialize(&stored).map_err(|err| DbError::Corrupt(err.to_string()))?;
                state.put(Column::Finality, EVIDENCE_KEY.to_vec(), bytes);
                self.db.commit(state.take_changes())?;
            }
        }

        Ok(found)
    }

    /// Adds a checked vote to its link's tally, or holds it when it cannot be placed yet.
    fn count(&mut self, vote: SignedVote) -> Result<FinalityUpdate, FinalityError> {
        let finality = self.finality.as_ref().expect("checked by the caller");
        let (source, target) = (vote.vote.source, vote.vote.target);

        if target.epoch <= finality.finalized.epoch {
            return Err(FinalityError::Stale);
        }

        let placeable = self.main_checkpoint(&target)? && self.main_checkpoint(&source)? && self.is_justified(&source)?;
        let set = if placeable { self.set_for(&target)? } else { None };
        let Some(set) = set else {
            let finality = self.finality.as_mut().expect("checked by the caller");

            if finality.pending.len() >= MAX_PENDING {
                finality.pending.pop_front();
            }

            finality.pending.push_back(vote);

            return Ok(FinalityUpdate {
                pending: true,
                ..FinalityUpdate::default()
            });
        };
        let member = *set.member(&vote.vote.voter).ok_or(FinalityError::NotValidator)?;
        let already = self.justification(&target.block)?.is_some();
        let finality = self.finality.as_mut().expect("checked by the caller");
        let tally = finality.links.entry((source, target)).or_default();

        if tally.votes.insert(vote.vote.voter, vote).is_none() {
            tally.weight += member.weight;
        }

        if already || !set.supermajority(tally.weight) {
            return Ok(FinalityUpdate::default());
        }

        let votes = tally.votes.values().copied().collect();

        self.justify(Justification { source, target, votes })
    }

    /// Stores a checked justification, moves the latest justified checkpoint, and finalizes the
    /// source when the target is its direct successor.
    fn justify(&mut self, justification: Justification) -> Result<FinalityUpdate, FinalityError> {
        let (source, target) = (justification.source, justification.target);
        let mut update = FinalityUpdate {
            justified: vec![target],
            ..FinalityUpdate::default()
        };
        let state = State::new(self.db.clone());
        state.put_value(Column::Finality, justification_key(&target.block), &justification);

        if !self.main_checkpoint(&target)? {
            update.reorganized = Some(self.adopt(&target)?);
        }

        let finality = self.finality.as_mut().expect("checked by the caller");

        if target.epoch > finality.justified.epoch {
            finality.justified = target;
            state.put_value(Column::Finality, JUSTIFIED_KEY.to_vec(), &target);
        }

        if target.epoch == source.epoch + 1 && source.epoch > finality.finalized.epoch {
            finality.finalized = source;
            update.finalized = Some(source);
            state.put_value(Column::Finality, FINALIZED_KEY.to_vec(), &source);
            let finalized = source.epoch;
            finality.links.retain(|(_, target), _| target.epoch > finalized);
            finality.seen.retain(|(_, epoch), _| *epoch > finalized);
        }

        self.db.commit(state.take_changes())?;
        update.merge(self.retry_votes()?);

        Ok(update)
    }

    /// Moves the main chain to the best known branch through a justified checkpoint that is not
    /// on it, never past the finalized checkpoint.
    fn adopt(&mut self, target: &Checkpoint) -> Result<(Vec<Block>, Vec<Block>), FinalityError> {
        let block = self.block(&target.block)?.ok_or(FinalityError::Unverifiable("its block is not known here"))?;
        let mut best = (super::work_of(&*self.db, &target.block)?, block.clone());

        for tip in self.tips.clone() {
            let Some(candidate) = self.block(&tip)? else {
                continue;
            };

            if candidate.header.height <= block.header.height || !self.descends(&candidate, &target.block, block.header.height)? {
                continue;
            }

            let work = super::work_of(&*self.db, &tip)?;

            if work > best.0 {
                best = (work, candidate);
            }
        }

        match self.reorganize(&best.1, best.0, true)? {
            Imported::Reorganized { undone, applied } => Ok((undone, applied)),
            _ => Err(FinalityError::Unverifiable("the justified branch splits off past the finalized checkpoint")),
        }
    }

    /// Whether block has ancestor, at height, on its branch.
    fn descends(&self, block: &Block, ancestor: &Hash, height: u64) -> Result<bool, DbError> {
        let mut current = block.clone();

        while current.header.height > height {
            match self.block(&current.header.parent)? {
                Some(parent) => current = parent,
                None => return Ok(false),
            }
        }

        Ok(current.id() == *ancestor)
    }

    /// Whether a checkpoint is the main chain's block at its height.
    pub(super) fn main_checkpoint(&self, checkpoint: &Checkpoint) -> Result<bool, DbError> {
        let Some(config) = self.finality_config() else {
            return Ok(false);
        };
        let height = config.checkpoint_height(checkpoint.epoch);

        if height > self.tip.height {
            return Ok(false);
        }

        Ok(self.block_at(height)?.is_some_and(|block| block.id() == checkpoint.block))
    }

    /// Whether source is target's ancestor checkpoint, on whichever branch target is.
    fn on_branch(&self, source: &Checkpoint, target: &Checkpoint) -> Result<bool, DbError> {
        let Some(config) = self.finality_config() else {
            return Ok(false);
        };

        if source.epoch >= target.epoch {
            return Ok(false);
        }

        let Some(block) = self.block(&target.block)? else {
            return Ok(false);
        };

        if block.header.height != config.checkpoint_height(target.epoch) {
            return Ok(false);
        }

        self.descends(&block, &source.block, config.checkpoint_height(source.epoch))
    }

    fn is_justified(&self, checkpoint: &Checkpoint) -> Result<bool, DbError> {
        if checkpoint.epoch == 0 {
            return Ok(checkpoint.block == self.genesis);
        }

        Ok(self.justification(&checkpoint.block)?.is_some())
    }

    /// The validator set a target's votes count against, when this node can know it: the main
    /// chain's, which also holds for any branch that split off after the checkpoint that fixed it.
    fn set_for(&self, target: &Checkpoint) -> Result<Option<ValidatorSet>, DbError> {
        let Some(config) = self.finality_config() else {
            return Ok(None);
        };
        let fixed_at = config.checkpoint_height(target.epoch.saturating_sub(config.activation_delay));

        if !self.main_checkpoint(target)? {
            let Some(block) = self.block(&target.block)? else {
                return Ok(None);
            };
            let fork = self.fork_height(&block)?;

            if fork.is_none_or(|fork| fork < fixed_at) {
                return Ok(None);
            }
        }

        Validators::new(&self.state()).set(target.epoch)
    }

    /// The height where a stored block's branch meets the main chain, if within reach.
    fn fork_height(&self, block: &Block) -> Result<Option<u64>, DbError> {
        let mut current = block.clone();

        for _ in 0..=self.max_reorg_depth {
            if self.on_main_chain(&current.id())? {
                return Ok(Some(current.header.height));
            }

            match self.block(&current.header.parent)? {
                Some(parent) => current = parent,
                None => return Ok(None),
            }
        }

        Ok(None)
    }
}

/// Checks the justifications a block carries and pays every voter in them, each checkpoint
/// once. A proof that does not check out makes the block invalid.
pub(super) fn pay_proofs(chain: &Chain, below: &Arc<State>, block: &Block) -> Result<(), String> {
    let Some(config) = chain.finality_config() else {
        return match block.proofs.is_empty() {
            true => Ok(()),
            false => Err("this chain has no finality".to_string()),
        };
    };

    if block.proofs.len() > config.max_proofs_per_block as usize {
        return Err(format!("more than {} proofs", config.max_proofs_per_block));
    }

    let validators = Validators::new(below);
    let on_chain = |checkpoint: &Checkpoint| -> Result<bool, String> {
        let height = config.checkpoint_height(checkpoint.epoch);
        let id: Option<Hash> = read_value(&**below, Column::Heights, &height.to_be_bytes()).map_err(|err| err.to_string())?;

        Ok(height < block.header.height && id == Some(checkpoint.block))
    };

    for proof in &block.proofs {
        let (source, target) = (proof.source, proof.target);

        if source.epoch >= target.epoch || !on_chain(&source)? || !on_chain(&target)? {
            return Err(format!("a proof for epoch {} is not for this branch", target.epoch));
        }

        if validators.paid(target.epoch).map_err(|err| err.to_string())? {
            return Err(format!("epoch {}'s voters are paid already", target.epoch));
        }

        let set = validators
            .set(target.epoch)
            .map_err(|err| err.to_string())?
            .ok_or_else(|| format!("no validator set for epoch {}", target.epoch))?;
        let mut weight = u256::ZERO;
        let mut previous: Option<PublicKey> = None;
        let mut paid = Vec::new();

        for vote in &proof.votes {
            if previous.is_some_and(|previous| previous >= vote.vote.voter)
                || vote.vote.source != source
                || vote.vote.target != target
                || vote.verify(&chain.genesis).is_err()
            {
                return Err(format!("a bad vote in the proof for epoch {}", target.epoch));
            }

            let member = set
                .member(&vote.vote.voter)
                .ok_or_else(|| format!("a vote by a non-validator for epoch {}", target.epoch))?;
            weight += member.weight;
            paid.push(member.account);
            previous = Some(vote.vote.voter);
        }

        if !set.supermajority(weight) {
            return Err(format!("the proof for epoch {} is under two thirds", target.epoch));
        }

        if config.vote_reward > u256::ZERO {
            let accounts = crate::registry::Accounts::new(below);

            for account in paid {
                accounts.credit(&account, config.vote_reward).map_err(|err| err.to_string())?;
            }
        }

        validators.mark_paid(target.epoch);
    }

    Ok(())
}
