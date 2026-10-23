//! Vote-based finality, shared by ffg and authority: votes on links between checkpoints, counted
//! until enough of an epoch's voters agree, justifications stored and served, the finalized
//! checkpoint moved, validators who contradict themselves caught, and voters paid through proofs
//! in blocks. What the two kinds differ in is their Electorate: who votes, with what weight, and
//! how much it takes.

use {
    super::{
        EVIDENCE_KEY, Evidence, FINALIZED_KEY, Finality, FinalityError, FinalityUpdate, JUSTIFIED_KEY, View,
        justification_key, stored_evidence, stored_justification,
    },
    crate::chain::{BlockError, ImportError, read_value},
    crate::crypto::hashable::Hashable,
    crate::crypto::keypair::PublicKey,
    crate::db::{Column, DbError, Read, State},
    crate::registry::validators::{ValidatorSet, Validators},
    crate::registry::{Accounts, RegistryError},
    crate::structs::block::Block,
    crate::structs::hash::Hash,
    crate::structs::u256::u256,
    crate::structs::vote::{Checkpoint, Justification, SignedVote, Vote},
    std::collections::{BTreeMap, HashMap, VecDeque},
    std::sync::Arc,
    wincode::{SchemaRead, SchemaWrite},
};

/// Votes a node holds while it cannot count them yet: their checkpoints are not on its main
/// chain, or their source is not justified yet.
const MAX_PENDING: usize = 4_096;
/// Pieces of evidence a node keeps for operators to report.
const MAX_EVIDENCE: usize = 64;
/// Votes per validator kept to catch one vote surrounding another.
const MAX_HISTORY: usize = 64;
/// How far back a producer looks for justifications whose voters are still unpaid.
const UNPAID_WINDOW: u64 = 64;

/// Timing and rewards, which every vote-based kind sets in genesis.
#[derive(Clone, Copy, Debug)]
pub(super) struct VoteParams {
    pub(super) checkpoint_interval: u64,
    pub(super) vote_depth: u64,
    pub(super) vote_reward: u256,
    pub(super) max_proofs_per_block: u32,
}

/// An epoch's voters, and the weight their votes for one link must add up to.
pub(super) struct Voters {
    pub(super) set: ValidatorSet,
    pub(super) needed: u256,
}

/// Who votes, and what it takes: the part of vote-based finality each kind decides.
pub(super) trait Electorate: Send {
    /// The kind's name.
    fn name(&self) -> &'static str;

    /// Its timing and rewards.
    fn params(&self) -> VoteParams;

    /// The voters for an epoch as a state knows them, or None when that state cannot know yet.
    fn voters(&self, state: &State, epoch: u64) -> Result<Option<Voters>, DbError>;

    /// How many epochs ahead each epoch's voters are fixed. A branch only agrees with the main
    /// chain on its voters if it split off after the checkpoint that fixed them.
    fn fixed_ahead(&self) -> u64;

    /// Updates state at a checkpoint, such as fixing a later epoch's voters.
    fn at_checkpoint(&self, _state: &Arc<State>, _epoch: u64) -> Result<(), DbError> {
        Ok(())
    }
}

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

fn own_key(voter: &PublicKey) -> Vec<u8> {
    let mut key = b"own:".to_vec();
    key.extend_from_slice(&voter.hashable_bytes_with_tag());

    key
}

/// Vote-based finality, with whatever electorate the kind has.
pub(super) struct Votes<E> {
    electorate: E,
    params: VoteParams,
    genesis: Hash,
    justified: Checkpoint,
    finalized: Checkpoint,
    links: HashMap<(Checkpoint, Checkpoint), Tally>,
    pending: VecDeque<SignedVote>,
    seen: HashMap<(PublicKey, u64), SignedVote>,
    history: HashMap<PublicKey, VecDeque<SignedVote>>,
}

impl<E: Electorate> Votes<E> {
    /// The finality an electorate makes, with the checkpoints this node justified and finalized
    /// before.
    pub(super) fn load(electorate: E, db: &dyn Read, genesis: Hash) -> Result<Votes<E>, DbError> {
        let start = Checkpoint {
            epoch: 0,
            block: genesis,
        };

        Ok(Votes {
            params: electorate.params(),
            electorate,
            genesis,
            justified: read_value(db, Column::Finality, JUSTIFIED_KEY)?.unwrap_or(start),
            finalized: read_value(db, Column::Finality, FINALIZED_KEY)?.unwrap_or(start),
            links: HashMap::new(),
            pending: VecDeque::new(),
            seen: HashMap::new(),
            history: HashMap::new(),
        })
    }

    fn epoch_of(&self, height: u64) -> u64 {
        height / self.params.checkpoint_interval
    }

    fn checkpoint_height(&self, epoch: u64) -> u64 {
        epoch.saturating_mul(self.params.checkpoint_interval)
    }

    /// Whether a checkpoint is the main chain's block at its height.
    fn main_checkpoint(&self, chain: &View, checkpoint: &Checkpoint) -> Result<bool, DbError> {
        let height = self.checkpoint_height(checkpoint.epoch);

        if height > chain.tip().height {
            return Ok(false);
        }

        Ok(chain.block_at(height)?.is_some_and(|block| block.id() == checkpoint.block))
    }

    /// Whether source is target's ancestor checkpoint, on whichever branch target is.
    fn on_branch(&self, chain: &View, source: &Checkpoint, target: &Checkpoint) -> Result<bool, DbError> {
        if source.epoch >= target.epoch {
            return Ok(false);
        }

        let Some(block) = chain.block(&target.block)? else {
            return Ok(false);
        };

        if block.header.height != self.checkpoint_height(target.epoch) {
            return Ok(false);
        }

        chain.descends(&block, &source.block, self.checkpoint_height(source.epoch))
    }

    fn is_justified(&self, chain: &View, checkpoint: &Checkpoint) -> Result<bool, DbError> {
        if checkpoint.epoch == 0 {
            return Ok(checkpoint.block == self.genesis);
        }

        Ok(stored_justification(&**chain.db(), &checkpoint.block)?.is_some())
    }

    /// The voters a target's votes count against, when this node can know them: the main
    /// chain's, which also hold for any branch that split off after the checkpoint that fixed
    /// them.
    fn voters_for(&self, chain: &View, target: &Checkpoint) -> Result<Option<Voters>, DbError> {
        if !self.main_checkpoint(chain, target)? {
            let fixed_at = self.checkpoint_height(target.epoch.saturating_sub(self.electorate.fixed_ahead()));
            let Some(block) = chain.block(&target.block)? else {
                return Ok(None);
            };
            let fork = chain.fork_height(&block)?;

            if fork.is_none_or(|fork| fork < fixed_at) {
                return Ok(None);
            }
        }

        self.electorate.voters(&chain.state(), target.epoch)
    }

    /// Records the vote for conflict checks, and returns evidence when it conflicts with one the
    /// same voter signed before.
    fn watch(&mut self, chain: &View, vote: &SignedVote) -> Result<Option<Evidence>, DbError> {
        let voter = vote.vote.voter;
        let mut found = None;

        match self.seen.get(&(voter, vote.vote.target.epoch)) {
            Some(first) if first != vote => {
                found = Some(Evidence {
                    first: *first,
                    second: *vote,
                })
            }
            Some(_) => {}
            None => {
                self.seen.insert((voter, vote.vote.target.epoch), *vote);
            }
        }

        let history = self.history.entry(voter).or_default();

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
            let mut stored = stored_evidence(&**chain.db())?;

            if !stored.contains(&evidence) {
                stored.push(evidence);

                if stored.len() > MAX_EVIDENCE {
                    stored.remove(0);
                }

                let state = State::new(chain.db().clone());
                let bytes = wincode::serialize(&stored).map_err(|err| DbError::Corrupt(err.to_string()))?;
                state.put(Column::Finality, EVIDENCE_KEY.to_vec(), bytes);
                chain.db().commit(state.take_changes())?;
            }
        }

        Ok(found)
    }

    /// Adds a checked vote to its link's tally, or holds it when it cannot be placed yet.
    fn count(&mut self, chain: &View, vote: SignedVote) -> Result<FinalityUpdate, FinalityError> {
        let (source, target) = (vote.vote.source, vote.vote.target);

        if target.epoch <= self.finalized.epoch {
            return Err(FinalityError::Stale);
        }

        let placeable = self.main_checkpoint(chain, &target)?
            && self.main_checkpoint(chain, &source)?
            && self.is_justified(chain, &source)?;
        let voters = if placeable { self.voters_for(chain, &target)? } else { None };
        let Some(voters) = voters else {
            if self.pending.len() >= MAX_PENDING {
                self.pending.pop_front();
            }

            self.pending.push_back(vote);

            return Ok(FinalityUpdate {
                pending: true,
                ..FinalityUpdate::default()
            });
        };
        let member = *voters.set.member(&vote.vote.voter).ok_or(FinalityError::NotValidator)?;
        let already = stored_justification(&**chain.db(), &target.block)?.is_some();
        let tally = self.links.entry((source, target)).or_default();

        if tally.votes.insert(vote.vote.voter, vote).is_none() {
            tally.weight += member.weight;
        }

        if already || tally.weight < voters.needed {
            return Ok(FinalityUpdate::default());
        }

        let votes = tally.votes.values().copied().collect();

        self.justify(chain, Justification { source, target, votes })
    }
}

impl<E: Electorate> Finality for Votes<E> {
    fn name(&self) -> &'static str {
        self.electorate.name()
    }

    fn checkpoint_interval(&self) -> u64 {
        self.params.checkpoint_interval
    }

    fn finalized(&self) -> Checkpoint {
        self.finalized
    }

    fn justified(&self) -> Checkpoint {
        self.justified
    }

    /// Checks the justifications a block carries and pays every voter in them, each checkpoint
    /// once, then lets the electorate update state at a checkpoint.
    fn apply(&self, state: &Arc<State>, block: &Block) -> Result<(), ImportError> {
        let bad = |why: String| ImportError::Invalid(BlockError::Proof(why));

        if block.proofs.len() > self.params.max_proofs_per_block as usize {
            return Err(bad(format!("more than {} proofs", self.params.max_proofs_per_block)));
        }

        let validators = Validators::new(state);
        let on_chain = |checkpoint: &Checkpoint| -> Result<bool, DbError> {
            let height = self.checkpoint_height(checkpoint.epoch);
            let id: Option<Hash> = read_value(&**state, Column::Heights, &height.to_be_bytes())?;

            Ok(height < block.header.height && id == Some(checkpoint.block))
        };

        for proof in &block.proofs {
            let (source, target) = (proof.source, proof.target);

            if source.epoch >= target.epoch || !on_chain(&source)? || !on_chain(&target)? {
                return Err(bad(format!("a proof for epoch {} is not for this branch", target.epoch)));
            }

            if validators.paid(target.epoch)? {
                return Err(bad(format!("epoch {}'s voters are paid already", target.epoch)));
            }

            let voters = self
                .electorate
                .voters(state, target.epoch)?
                .ok_or_else(|| bad(format!("no validator set for epoch {}", target.epoch)))?;
            let mut weight = u256::ZERO;
            let mut previous: Option<PublicKey> = None;
            let mut paid = Vec::new();

            for vote in &proof.votes {
                if previous.is_some_and(|previous| previous >= vote.vote.voter)
                    || vote.vote.source != source
                    || vote.vote.target != target
                    || vote.verify(&self.genesis).is_err()
                {
                    return Err(bad(format!("a bad vote in the proof for epoch {}", target.epoch)));
                }

                let member = voters
                    .set
                    .member(&vote.vote.voter)
                    .ok_or_else(|| bad(format!("a vote by a non-validator for epoch {}", target.epoch)))?;
                weight += member.weight;
                paid.push(member.account);
                previous = Some(vote.vote.voter);
            }

            if weight < voters.needed {
                return Err(bad(format!("the proof for epoch {} falls short of a justification", target.epoch)));
            }

            if self.params.vote_reward > u256::ZERO {
                let accounts = Accounts::new(state);

                for account in paid {
                    accounts.credit(&account, self.params.vote_reward).map_err(|err| match err {
                        RegistryError::Db(err) => ImportError::Db(err),
                        other => bad(other.to_string()),
                    })?;
                }
            }

            validators.mark_paid(target.epoch);
        }

        let height = block.header.height;

        if height.is_multiple_of(self.params.checkpoint_interval) {
            self.electorate.at_checkpoint(state, self.epoch_of(height))?;
        }

        Ok(())
    }

    fn add_vote(&mut self, chain: &View, vote: SignedVote) -> Result<FinalityUpdate, FinalityError> {
        vote.verify(&self.genesis).map_err(FinalityError::Invalid)?;

        if let Some(evidence) = self.watch(chain, &vote)? {
            return Ok(FinalityUpdate {
                evidence: Some(evidence),
                ..FinalityUpdate::default()
            });
        }

        self.count(chain, vote)
    }

    fn retry_votes(&mut self, chain: &View) -> Result<FinalityUpdate, FinalityError> {
        let held: Vec<SignedVote> = self.pending.drain(..).collect();
        let mut update = FinalityUpdate::default();

        for vote in held {
            match self.count(chain, vote) {
                Ok(counted) => update.merge(counted),
                Err(FinalityError::Db(err)) => return Err(FinalityError::Db(err)),
                Err(_) => {}
            }
        }

        Ok(update)
    }

    fn check_justification(&self, chain: &View, justification: &Justification) -> Result<bool, FinalityError> {
        let (source, target) = (justification.source, justification.target);

        if target.epoch <= self.finalized.epoch {
            return Err(FinalityError::Stale);
        }

        if stored_justification(&**chain.db(), &target.block)?.is_some() {
            return Ok(false);
        }

        if !self.is_justified(chain, &source)? {
            return Err(FinalityError::Unverifiable("its source is not justified here"));
        }

        if !self.on_branch(chain, &source, &target)? {
            return Err(FinalityError::Unverifiable("its source is not on its target's branch"));
        }

        let voters = self
            .voters_for(chain, &target)?
            .ok_or(FinalityError::Unverifiable("its validator set is not known here"))?;
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
            weight += voters.set.member(&vote.vote.voter).ok_or(FinalityError::NotValidator)?.weight;
            previous = Some(vote.vote.voter);
        }

        if weight < voters.needed {
            return Err(FinalityError::Unverifiable("its votes fall short of a justification"));
        }

        Ok(true)
    }

    /// Stores a checked justification, moves the latest justified checkpoint, and finalizes the
    /// source when the target is its direct successor.
    fn justify(&mut self, chain: &View, justification: Justification) -> Result<FinalityUpdate, FinalityError> {
        let (source, target) = (justification.source, justification.target);
        let mut update = FinalityUpdate {
            justified: vec![target],
            ..FinalityUpdate::default()
        };
        let state = State::new(chain.db().clone());
        state.put_value(Column::Finality, justification_key(&target.block), &justification);

        if target.epoch > self.justified.epoch {
            self.justified = target;
            state.put_value(Column::Finality, JUSTIFIED_KEY.to_vec(), &target);
        }

        if target.epoch == source.epoch + 1 && source.epoch > self.finalized.epoch {
            self.finalized = source;
            update.finalized = Some(source);
            state.put_value(Column::Finality, FINALIZED_KEY.to_vec(), &source);
            let finalized = source.epoch;
            self.links.retain(|(_, target), _| target.epoch > finalized);
            self.seen.retain(|(_, epoch), _| *epoch > finalized);
        }

        chain.db().commit(state.take_changes())?;
        update.merge(self.retry_votes(chain)?);

        Ok(update)
    }

    fn vote_for(&self, chain: &View, voter: &PublicKey) -> Result<Option<Vote>, DbError> {
        let Some(deep) = chain.tip().height.checked_sub(self.params.vote_depth) else {
            return Ok(None);
        };
        let epoch = self.epoch_of(deep);
        let source = self.justified;
        let own: OwnVotes = read_value(&**chain.db(), Column::Finality, &own_key(voter))?.unwrap_or_default();

        if epoch == 0 || epoch <= own.last_target || epoch <= source.epoch || source.epoch < own.max_source {
            return Ok(None);
        }

        let Some(block) = chain.block_at(self.checkpoint_height(epoch))? else {
            return Ok(None);
        };
        let target = Checkpoint {
            epoch,
            block: block.id(),
        };

        match self.voters_for(chain, &target)? {
            Some(voters) if voters.set.member(voter).is_some() => Ok(Some(Vote {
                source,
                target,
                voter: *voter,
            })),
            _ => Ok(None),
        }
    }

    fn record_own_vote(&self, chain: &View, vote: &Vote) -> Result<(), DbError> {
        let key = own_key(&vote.voter);
        let mut own: OwnVotes = read_value(&**chain.db(), Column::Finality, &key)?.unwrap_or_default();
        own.last_target = own.last_target.max(vote.target.epoch);
        own.max_source = own.max_source.max(vote.source.epoch);
        let state = State::new(chain.db().clone());
        state.put_value(Column::Finality, key, &own);

        chain.db().commit(state.take_changes())
    }

    fn unpaid_proofs(&self, chain: &View) -> Result<Vec<Justification>, DbError> {
        if self.params.vote_reward == u256::ZERO {
            return Ok(Vec::new());
        }

        let state = chain.state();
        let validators = Validators::new(&state);
        let newest = self.justified.epoch;
        let mut proofs = Vec::new();

        for epoch in newest.saturating_sub(UNPAID_WINDOW).max(1)..=newest {
            if proofs.len() >= self.params.max_proofs_per_block as usize {
                break;
            }

            let Some(block) = chain.block_at(self.checkpoint_height(epoch))? else {
                continue;
            };

            if let Some(proof) = stored_justification(&**chain.db(), &block.id())?
                && !validators.paid(epoch)?
                && self.main_checkpoint(chain, &proof.source)?
            {
                proofs.push(proof);
            }
        }

        Ok(proofs)
    }

    fn justifications_from(&self, chain: &View, epoch: u64, count: usize) -> Result<Vec<Justification>, DbError> {
        let last = self.epoch_of(chain.tip().height);
        let mut out = Vec::new();

        for epoch in epoch.max(1)..=last {
            if out.len() >= count {
                break;
            }

            if let Some(block) = chain.block_at(self.checkpoint_height(epoch))?
                && let Some(justification) = stored_justification(&**chain.db(), &block.id())?
            {
                out.push(justification);
            }
        }

        Ok(out)
    }
}
