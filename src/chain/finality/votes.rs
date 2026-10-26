//! Vote-based finality, shared by ffg and authority: votes on links between checkpoints, counted
//! until enough of an epoch's voters agree, justifications stored and served, the finalized
//! checkpoint moved, validators who contradict themselves caught, and voters paid through proofs
//! in blocks. What the two kinds differ in is their Electorate: who votes, with what weight, and
//! how much it takes.
//!
//! Every vote counted or held back is also stored in the Finality column, by target epoch and
//! voter. On load they are counted again, which brings back the tallies, the held votes and what
//! double-vote detection compares against, so a restart forgets nothing it had.

use {
    super::{
        EVIDENCE_KEY, Evidence, FINALIZED_KEY, Finality, FinalityError, FinalityUpdate, JUSTIFIED_KEY, View, Workers,
        justification_key, stored_evidence, stored_justification,
    },
    crate::chain::{BlockError, ImportError, read_value},
    crate::crypto::hashable::Hashable,
    crate::crypto::keypair::PublicKey,
    crate::db::{Column, Database, DbError, State},
    crate::registry::validators::{ValidatorSet, Validators},
    crate::registry::{Accounts, RegistryError},
    crate::structs::block::Block,
    crate::structs::hash::Hash,
    crate::structs::u256::u256,
    crate::chain::{Leak, REWARD_POOL, VoteRewardSource},
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
pub(super) const MAX_HISTORY: usize = 64;
/// How far back a producer looks for justifications whose voters are still unpaid.
const UNPAID_WINDOW: u64 = 64;
/// Counted votes, by target epoch and voter. Kept MAX_HISTORY epochs behind the finalized
/// checkpoint, so a vote surrounding one of them is still caught after a restart.
pub(super) const COUNTED: &[u8] = b"v:";
/// Votes held back to count later, by target epoch and the whole vote, as many as the node holds
/// in memory.
pub(super) const HELD: &[u8] = b"p:";

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

    /// The inactivity leak, for kinds with bonds to leak: blocks then carry proofs whether or not
    /// they pay, so state knows when finality stalls, and bundles of votes that fall short while
    /// it does, so state knows who still votes.
    fn leak(&self) -> Option<Leak> {
        None
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

/// Where a counted vote lives: by target epoch, so a scan visits them oldest first, then voter.
/// A voter has at most one vote counted per epoch; a second is evidence instead.
fn counted_key(vote: &SignedVote) -> Vec<u8> {
    let mut key = epoch_key(COUNTED, vote.vote.target.epoch);
    key.extend_from_slice(&vote.vote.voter.hashable_bytes_with_tag());

    key
}

/// Where a held vote lives: by target epoch, then the whole vote. Nothing is known yet about who
/// signed it, so two conflicting ones are both held until counting them shows which is evidence.
fn held_key(vote: &SignedVote) -> Vec<u8> {
    let mut key = epoch_key(HELD, vote.vote.target.epoch);
    key.extend_from_slice(&vote.hashable_bytes_with_tag());

    key
}

fn epoch_key(prefix: &[u8], epoch: u64) -> Vec<u8> {
    let mut key = prefix.to_vec();
    key.extend_from_slice(&epoch.to_be_bytes());

    key
}

/// The target epoch in a stored vote's key.
fn epoch_in(prefix: &[u8], key: &[u8]) -> u64 {
    let bytes = key.get(prefix.len()..prefix.len() + 8).and_then(|bytes| bytes.try_into().ok());

    bytes.map_or(0, u64::from_be_bytes)
}

/// The stored votes of one kind, oldest target first, from an epoch on.
pub(super) fn stored_votes(db: &dyn Database, prefix: &[u8], from: u64) -> Result<Vec<SignedVote>, DbError> {
    let mut votes = Vec::new();
    let mut corrupt = None;
    db.scan(Column::Finality, prefix, &epoch_key(prefix, from), &mut |key, value| {
        match wincode::deserialize(value) {
            Ok(vote) => votes.push(vote),
            Err(err) => corrupt = Some(DbError::Corrupt(format!("stored vote {key:?}: {err}"))),
        }

        corrupt.is_none()
    })?;

    match corrupt {
        Some(err) => Err(err),
        None => Ok(votes),
    }
}

fn own_key(voter: &PublicKey) -> Vec<u8> {
    let mut key = b"own:".to_vec();
    key.extend_from_slice(&voter.hashable_bytes_with_tag());

    key
}

/// Vote-based finality, with whatever electorate the kind has.
pub(super) struct Votes<E> {
    electorate: E,
    params: VoteParams,
    /// Where rewards come from: minted, or paid from the reward pool.
    source: VoteRewardSource,
    genesis: Hash,
    /// The chain's threads, which check a proof's or a justification's signatures all at once.
    workers: Arc<Workers>,
    justified: Checkpoint,
    finalized: Checkpoint,
    links: HashMap<(Checkpoint, Checkpoint), Tally>,
    pending: VecDeque<SignedVote>,
    seen: HashMap<(PublicKey, u64), SignedVote>,
    history: HashMap<PublicKey, VecDeque<SignedVote>>,
}

impl<E: Electorate> Votes<E> {
    /// The finality an electorate makes, with everything this node learned before: the
    /// checkpoints it justified and finalized, and the votes it counted or held back, which it
    /// counts again.
    pub(super) fn load(electorate: E, chain: &View, source: VoteRewardSource) -> Result<Votes<E>, DbError> {
        let db = &**chain.db();
        let start = Checkpoint {
            epoch: 0,
            block: chain.genesis(),
        };
        let mut votes = Votes {
            params: electorate.params(),
            electorate,
            source,
            genesis: chain.genesis(),
            workers: chain.workers().clone(),
            justified: read_value(db, Column::Finality, JUSTIFIED_KEY)?.unwrap_or(start),
            finalized: read_value(db, Column::Finality, FINALIZED_KEY)?.unwrap_or(start),
            links: HashMap::new(),
            pending: VecDeque::new(),
            seen: HashMap::new(),
            history: HashMap::new(),
        };
        let finalized = votes.finalized.epoch;
        let counted = stored_votes(db, COUNTED, finalized.saturating_sub(MAX_HISTORY as u64))?;
        let (past, open): (Vec<_>, Vec<_>) = counted.into_iter().partition(|vote| vote.vote.target.epoch <= finalized);

        // Votes for final epochs only feed double-vote detection; the rest are counted again,
        // which remembers them too.
        for vote in &past {
            votes.remember(vote);
        }

        votes.pending.extend(stored_votes(db, HELD, 0)?);

        for vote in open {
            match votes.count(chain, vote) {
                Err(FinalityError::Db(err)) => return Err(err),
                // Every stored vote counted once already; one that no longer does is dropped.
                Err(_) => forget(chain, counted_key(&vote))?,
                Ok(_) => {}
            }
        }

        Ok(votes)
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

        Ok(chain.header_at(height)?.is_some_and(|header| header.id() == checkpoint.block))
    }

    /// Whether source is target's ancestor checkpoint, on whichever branch target is.
    fn on_branch(&self, chain: &View, source: &Checkpoint, target: &Checkpoint) -> Result<bool, DbError> {
        if source.epoch >= target.epoch {
            return Ok(false);
        }

        let Some(header) = chain.header(&target.block)? else {
            return Ok(false);
        };

        if header.height != self.checkpoint_height(target.epoch) {
            return Ok(false);
        }

        chain.descends(&header, &source.block, self.checkpoint_height(source.epoch))
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
            let Some(header) = chain.header(&target.block)? else {
                return Ok(None);
            };
            let fork = chain.fork_height(&header)?;

            if fork.is_none_or(|fork| fork < fixed_at) {
                return Ok(None);
            }
        }

        self.electorate.voters(&chain.state(), target.epoch)
    }

    /// Remembers a counted vote, to compare its voter's later ones against. Only counted votes
    /// are remembered, so a key that does not vote takes up no room.
    fn remember(&mut self, vote: &SignedVote) {
        let voter = vote.vote.voter;

        if vote.vote.target.epoch > self.finalized.epoch {
            self.seen.entry((voter, vote.vote.target.epoch)).or_insert(*vote);
        }

        let history = self.history.entry(voter).or_default();

        if !history.contains(vote) {
            history.push_back(*vote);

            if history.len() > MAX_HISTORY {
                history.pop_front();
            }
        }
    }

    /// Evidence against a vote's voter, when the vote conflicts with one of its counted ones.
    fn conflict(&self, vote: &SignedVote) -> Option<Evidence> {
        let voter = vote.vote.voter;
        let earlier = self.seen.get(&(voter, vote.vote.target.epoch)).filter(|first| *first != vote).or_else(|| {
            self.history
                .get(&voter)
                .and_then(|history| history.iter().find(|earlier| earlier.vote.conflicts(&vote.vote)))
        });

        earlier.map(|first| Evidence {
            first: *first,
            second: *vote,
        })
    }

    /// Evidence against a voter just shown to vote, when this vote conflicts with one of its held
    /// ones, which nobody could tell were a voter's until now.
    fn held_conflict(&self, vote: &SignedVote) -> Option<Evidence> {
        self.pending
            .iter()
            .find(|held| held.vote.conflicts(&vote.vote))
            .map(|first| Evidence {
                first: *first,
                second: *vote,
            })
    }

    /// Keeps evidence for operators to report, and lets go of the vote it caught if it was held.
    fn record(&self, chain: &View, evidence: Evidence) -> Result<(), DbError> {
        let state = State::new(chain.db().clone());
        let mut stored = stored_evidence(&**chain.db())?;

        if !stored.contains(&evidence) {
            stored.push(evidence);

            if stored.len() > MAX_EVIDENCE {
                stored.remove(0);
            }

            let bytes = wincode::serialize(&stored).map_err(|err| DbError::Corrupt(err.to_string()))?;
            state.put(Column::Finality, EVIDENCE_KEY.to_vec(), bytes);
        }

        state.delete(Column::Finality, held_key(&evidence.second));

        chain.db().commit(state.take_changes())
    }

    /// Adds a signed vote to its link's tally, holds it when it cannot be placed yet, or returns
    /// evidence when it contradicts a vote its voter had counted or held. Whether its key votes at
    /// all is only known once it can be placed, so until then it is held but not remembered, and
    /// a key that does not vote can neither fill memory nor the evidence log.
    fn count(&mut self, chain: &View, vote: SignedVote) -> Result<FinalityUpdate, FinalityError> {
        let (source, target) = (vote.vote.source, vote.vote.target);

        if let Some(evidence) = self.conflict(&vote) {
            self.record(chain, evidence)?;

            return Ok(FinalityUpdate {
                evidence: Some(evidence),
                ..FinalityUpdate::default()
            });
        }

        if target.epoch <= self.finalized.epoch {
            return Err(FinalityError::Stale);
        }

        let placeable = self.main_checkpoint(chain, &target)?
            && self.main_checkpoint(chain, &source)?
            && self.is_justified(chain, &source)?;
        let voters = if placeable { self.voters_for(chain, &target)? } else { None };
        let Some(voters) = voters else {
            if !self.pending.contains(&vote) {
                let state = State::new(chain.db().clone());

                if self.pending.len() >= MAX_PENDING
                    && let Some(dropped) = self.pending.pop_front()
                {
                    state.delete(Column::Finality, held_key(&dropped));
                }

                self.pending.push_back(vote);
                state.put_value(Column::Finality, held_key(&vote), &vote);
                state.delete(Column::Finality, counted_key(&vote));
                chain.db().commit(state.take_changes())?;
            }

            return Ok(FinalityUpdate {
                pending: true,
                ..FinalityUpdate::default()
            });
        };
        let member = *voters.set.member(&vote.vote.voter).ok_or(FinalityError::NotValidator)?;

        if let Some(evidence) = self.held_conflict(&vote) {
            self.record(chain, evidence)?;

            return Ok(FinalityUpdate {
                evidence: Some(evidence),
                ..FinalityUpdate::default()
            });
        }

        self.remember(&vote);
        let state = State::new(chain.db().clone());
        state.put_value(Column::Finality, counted_key(&vote), &vote);
        state.delete(Column::Finality, held_key(&vote));
        chain.db().commit(state.take_changes())?;
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

/// Drops a stored vote.
fn forget(chain: &View, key: Vec<u8>) -> Result<(), DbError> {
    let state = State::new(chain.db().clone());
    state.delete(Column::Finality, key);

    chain.db().commit(state.take_changes())
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
    /// once, then lets the electorate update state at a checkpoint. Under an inactivity leak,
    /// while no checkpoint has been proven for after_epochs epochs, a proof may fall short of a
    /// justification: a bundle of votes, which pays nothing and proves nothing, but records who
    /// still votes, for the leak to spare.
    fn apply(&self, state: &Arc<State>, block: &Block) -> Result<(), ImportError> {
        let bad = |why: String| ImportError::Invalid(BlockError::Proof(why));

        if block.proofs.len() > self.params.max_proofs_per_block as usize {
            return Err(bad(format!("more than {} proofs", self.params.max_proofs_per_block)));
        }

        let validators = Validators::new(state);
        // Bundles are taken from after_epochs epochs without a proven checkpoint, one epoch before
        // the leak starts, so the first leak already sees who voted.
        let leak = self.electorate.leak();
        let bundling = match leak {
            Some(leak) => self.epoch_of(block.header.height).saturating_sub(validators.proven()?) >= leak.after_epochs,
            None => false,
        };
        // Every vote's signature, in every proof, checked at once.
        let genesis = self.genesis;
        let votes: Vec<&SignedVote> = block.proofs.iter().flat_map(|proof| &proof.votes).collect();
        let mut signed = self.workers.map(&votes, |vote| vote.verify(&genesis).is_ok()).into_iter();
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
                let well_signed = signed.next().unwrap_or(false);

                if previous.is_some_and(|previous| previous >= vote.vote.voter)
                    || vote.vote.source != source
                    || vote.vote.target != target
                    || !well_signed
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

            if leak.is_some() {
                for vote in &proof.votes {
                    validators.mark_participation(&vote.vote.voter, target.epoch)?;
                }
            }

            if weight < voters.needed {
                if !bundling {
                    return Err(bad(format!("the proof for epoch {} falls short of a justification", target.epoch)));
                }

                // A bundle: who voted is recorded, and that is all.
                continue;
            }

            if leak.is_some() {
                validators.mark_proven(target.epoch)?;
            }

            if self.params.vote_reward > u256::ZERO {
                let accounts = Accounts::new(state);
                let registry = |err| match err {
                    RegistryError::Db(err) => ImportError::Db(err),
                    other => bad(other.to_string()),
                };

                // Voters in the proof's order, which is by key: from the pool, each gets as much
                // of its reward as the pool still holds; minted, each gets it whole.
                for account in paid {
                    let reward = match self.source {
                        VoteRewardSource::Mint => self.params.vote_reward,
                        VoteRewardSource::Fees => {
                            let pool = accounts.get(&REWARD_POOL).map_err(ImportError::Db)?.balance;
                            let reward = self.params.vote_reward.min(pool);

                            if reward > u256::ZERO {
                                accounts.debit(&REWARD_POOL, reward).map_err(registry)?;
                            }

                            reward
                        }
                    };

                    if reward > u256::ZERO {
                        accounts.credit(&account, reward).map_err(registry)?;
                    }
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

        self.count(chain, vote)
    }

    fn retry_votes(&mut self, chain: &View) -> Result<FinalityUpdate, FinalityError> {
        let held: Vec<SignedVote> = self.pending.drain(..).collect();
        let mut update = FinalityUpdate::default();

        for vote in held {
            match self.count(chain, vote) {
                Ok(counted) => update.merge(counted),
                Err(FinalityError::Db(err)) => return Err(FinalityError::Db(err)),
                // Stale, or by someone who is no voter after all: it never will count.
                Err(_) => forget(chain, held_key(&vote))?,
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
        // Every signature checked at once; what is wrong with the first bad vote is still what
        // comes back.
        let genesis = self.genesis;
        let signed = self.workers.map(&justification.votes, |vote| vote.verify(&genesis));

        for (vote, signed) in justification.votes.iter().zip(signed) {
            if previous.is_some_and(|previous| previous >= vote.vote.voter) {
                return Err(FinalityError::Unverifiable("its votes are not sorted by voter"));
            }

            if vote.vote.source != source || vote.vote.target != target {
                return Err(FinalityError::Unverifiable("a vote is for another link"));
            }

            signed.map_err(FinalityError::Invalid)?;
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

            // Counted votes stay a while, to catch one vote surrounding another; then they go.
            if let Some(oldest) = finalized.checked_sub(MAX_HISTORY as u64) {
                chain.db().scan(Column::Finality, COUNTED, COUNTED, &mut |key, _| {
                    let old = epoch_in(COUNTED, key) <= oldest;

                    if old {
                        state.delete(Column::Finality, key.to_vec());
                    }

                    old
                })?;
            }
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

        let Some(header) = chain.header_at(self.checkpoint_height(epoch))? else {
            return Ok(None);
        };
        let target = Checkpoint {
            epoch,
            block: header.id(),
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
        let leak = self.electorate.leak();

        if self.params.vote_reward == u256::ZERO && leak.is_none() {
            return Ok(Vec::new());
        }

        let state = chain.state();
        let validators = Validators::new(&state);
        let newest = self.justified.epoch;
        let proven = validators.proven()?;
        let mut proofs = Vec::new();

        for epoch in newest.saturating_sub(UNPAID_WINDOW).max(1)..=newest {
            if proofs.len() >= self.params.max_proofs_per_block as usize {
                break;
            }

            let Some(header) = chain.header_at(self.checkpoint_height(epoch))? else {
                continue;
            };
            // Unpaid, under a reward; or unproven, under a leak, so state sees finality move.
            let wanted = (self.params.vote_reward > u256::ZERO && !validators.paid(epoch)?) || (leak.is_some() && epoch > proven);

            if wanted
                && let Some(proof) = stored_justification(&**chain.db(), &header.id())?
                && self.main_checkpoint(chain, &proof.source)?
            {
                proofs.push(proof);
            }
        }

        // While finality stalls, bundles of the votes this node counted for checkpoints not yet
        // proven, whenever they hold a vote state has not seen: who still votes is what the
        // leak spares.
        let tip_epoch = self.epoch_of(chain.tip().height);

        if let Some(leak) = leak
            && tip_epoch.saturating_sub(proven) >= leak.after_epochs
        {
            let mut links: Vec<_> = self.links.iter().filter(|((_, target), _)| target.epoch > proven).collect();
            links.sort_by_key(|((_, target), _)| std::cmp::Reverse(target.epoch));

            for ((source, target), tally) in links {
                if proofs.len() >= self.params.max_proofs_per_block as usize {
                    break;
                }

                // A link that justified is a proof, carried above or stored already; never a bundle.
                if proofs.iter().any(|proof| proof.target == *target)
                    || stored_justification(&**chain.db(), &target.block)?.is_some()
                    || !self.main_checkpoint(chain, target)?
                    || !self.main_checkpoint(chain, source)?
                    || !self.is_justified(chain, source)?
                {
                    continue;
                }

                let mut news = false;

                for voter in tally.votes.keys() {
                    news |= validators.participation(voter)? < target.epoch;
                }

                if news {
                    let votes = tally.votes.values().copied().collect();
                    proofs.push(Justification { source: *source, target: *target, votes });
                }
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

            if let Some(header) = chain.header_at(self.checkpoint_height(epoch))?
                && let Some(justification) = stored_justification(&**chain.db(), &header.id())?
            {
                out.push(justification);
            }
        }

        Ok(out)
    }
}
