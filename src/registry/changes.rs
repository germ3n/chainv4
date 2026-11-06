//! Governance's records, in chain state: the consensus decided last, the bootnodes and name prices
//! decided last, and each kind of change's version and ballot, in Column::Validators beside the
//! signers' records, and the
//! consensus fixed for each epoch, in Column::Epochs beside each epoch's voters, which is what a
//! light client asks a proof of.

#[cfg(feature = "governance")]
use {super::RegistryError, crate::governance::Governance};
use {
    crate::consensus::ConsensusConfig,
    crate::crypto::hashable::Hashable,
    crate::db::{Column, DbError, State},
    crate::governance::{Change, ChangeKind, ChangeVote},
    crate::structs::address::Address,
    crate::structs::u256::u256,
    r#macro::document,
    wincode::{SchemaRead, SchemaWrite},
};

const DECIDED: &[u8] = b"change:decided";
/// Each kind's ballot and version are kept under these, followed by the kind's name.
const BALLOT: &[u8] = b"change:ballot:";
const VERSION: &[u8] = b"change:version:";
const BOOTNODES: &[u8] = b"change:bootnodes";
const NAMES: &[u8] = b"change:names";
const TOKEN_FUNCTIONS: &[u8] = b"change:token-functions";
const EVM_SPEC: &[u8] = b"change:evm-spec";
const DEX: &[u8] = b"change:dex";
const ANCHORING: &[u8] = b"change:anchoring";
const COLLECTIONS: &[u8] = b"change:collections";
const FEDERATION: &[u8] = crate::registry::federation::FEDERATION_KEY;
const DIRECTORY: &[u8] = crate::registry::directory::DIRECTORY_KEY;
const FEES: &[u8] = b"change:fees";
const REWARD: &[u8] = b"change:reward";
const LIMITS: &[u8] = b"change:limits";
const FINALITY: &[u8] = b"change:finality";
const UPGRADE: &[u8] = b"change:upgrade";
const NAME: &[u8] = b"change:name";
const GOVERNANCE: &[u8] = b"change:governance";
const REJECTED: &[u8] = b"change:rejected";
const LOCKS: &[u8] = b"change:locks";

/// How many changes of one kind were decided, as a state value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, SchemaWrite, SchemaRead)]
struct KindVersion {
    version: u64,
}

impl Hashable for KindVersion {}

/// Where a kind's ballot is kept.
fn ballot_key(kind: ChangeKind) -> Vec<u8> {
    [BALLOT, kind.name().as_bytes()].concat()
}

/// Where a kind's version is kept.
fn version_key(kind: ChangeKind) -> Vec<u8> {
    [VERSION, kind.name().as_bytes()].concat()
}

/// The chain's name in force, as a state value.
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
struct ChainName {
    name: String,
}

impl Hashable for ChainName {}

#[document(
    summary = "Where an approval is made: its block's height and epoch, and how many epochs ahead finality fixes what is decided, which Change::Finality takes effect after.",
    category = "Governance",
    see_also = "Changes::vote",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DecidedAt {
    #[document(summary = "The block's height.")]
    pub height: u64,
    #[document(summary = "Its epoch.")]
    pub epoch: u64,
    #[document(summary = "Finality's activation_delay.")]
    pub activation_delay: u64,
    #[document(summary = "The block's time, in milliseconds since the Unix epoch: what a decided federation is kept with.")]
    pub time: u64,
}

#[document(
    summary = "The block reward governance decided last, and the height it took effect at: the block after the one that decided it, where its halvings count from.",
    category = "Governance",
    see_also = "Change::Reward, Reward::since",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct DecidedReward {
    #[document(summary = "The reward.")]
    pub reward: crate::params::Reward,
    #[document(summary = "The height it took effect at.")]
    pub from: u64,
}

impl Hashable for DecidedReward {}

#[document(
    summary = "Finality's settings as governance decided them, each with the epoch it took effect from, oldest first: what an epoch runs with is the last that took effect by it, or genesis's before any.",
    category = "Governance",
    see_also = "Change::Finality, Changes::finality_at",
)]
#[derive(Clone, Debug, Default, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct FinalityEras {
    #[document(summary = "Each decided config, with the epoch it took effect from.")]
    pub eras: Vec<FinalityEra>,
}

impl Hashable for FinalityEras {}

#[document(summary = "One finality config governance decided, and the epoch it took effect from.", category = "Governance", see_also = "FinalityEras")]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct FinalityEra {
    #[document(summary = "The first epoch it runs.")]
    pub from: u64,
    #[document(summary = "The config.")]
    #[wincode(with = "crate::structs::unset::Framed<crate::params::finality::FinalityConfig>")]
    pub config: crate::params::finality::FinalityConfig,
}

#[document(
    summary = "The consensus decided last, and how many changes of any kind came before it.",
    category = "Governance",
    see_also = "Changes",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct DecidedChange {
    #[document(summary = "How many changes have been decided since genesis. An approval names the version it changes, so approvals for an older decision never count.")]
    pub version: u64,
    #[document(summary = "The consensus itself, which the next checkpoints fix for the epochs ahead.")]
    pub consensus: ConsensusConfig,
}

impl Hashable for DecidedChange {}

#[document(
    summary = "The bootnodes governance decided last, which replace genesis's.",
    category = "Governance",
    see_also = "Changes::bootnodes, Change::Bootnodes",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct DecidedBootnodes {
    #[document(summary = "The addresses, as the change named them.")]
    pub bootnodes: Vec<String>,
}

impl Hashable for DecidedBootnodes {}

#[document(
    summary = "The EVM specs governance decided, each with the height it runs from, oldest first: a block runs the last that took effect by its height, or genesis's before any.",
    category = "Governance",
    see_also = "Change::EvmSpec, Changes::evm_spec_at",
)]
#[derive(Clone, Debug, Default, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct EvmSpecs {
    #[document(summary = "Each decided spec and the first height it runs.")]
    pub eras: Vec<(u64, crate::params::vm::EvmSpec)>,
}

impl Hashable for EvmSpecs {}

/// The token functions in force, as a state value.
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
struct TokenFunctions {
    functions: Vec<String>,
}

impl Hashable for TokenFunctions {}

#[document(
    summary = "One account's vote on a change.",
    category = "Governance",
    see_also = "ChangeBallot, ChangeVote",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct CastVote {
    #[document(summary = "The voting account.")]
    pub account: Address,
    #[document(summary = "The change it votes on.")]
    pub change: Change,
    #[document(summary = "Yes, No or Abstain.")]
    pub vote: ChangeVote,
    #[document(summary = "The epoch it was cast in. It counts for approval_epochs epochs from there, and voting again starts it afresh.")]
    pub epoch: u64,
}

impl CastVote {
    #[document(parent = "CastVote", summary = "Whether it still counts in an epoch, when votes count for approval_epochs epochs.")]
    pub fn counts(&self, epoch: u64, approval_epochs: u64) -> bool {
        epoch < self.epoch.saturating_add(approval_epochs)
    }
}

#[document(
    summary = "The votes on changing one kind of setting from what was decided last: at most one per account on each change, and at most one Yes per account in the kind, so an account backs one change of each kind at once. A new vote replaces the account's old one on that change, and Yes on another change of the kind moves its Yes there. A vote stops counting approval_epochs epochs after the one it was cast in, and what it weighs is counted afresh whenever the ballot is: an account that lost its standing since weighs nothing. The changes rejected at the kind's version stay listed, so none comes straight back.",
    category = "Governance",
    see_also = "Changes::vote",
)]
#[derive(Clone, Debug, Default, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct ChangeBallot {
    #[document(summary = "The kind's version the votes change.")]
    pub version: u64,
    #[document(summary = "The votes, in the order they came.")]
    pub votes: Vec<CastVote>,
    #[document(summary = "The changes rejected at this version, in the order they were.")]
    pub rejected: Vec<Change>,
}

impl Hashable for ChangeBallot {}

impl ChangeBallot {
    #[document(
        parent = "ChangeBallot",
        summary = "The ballot as it stands in an epoch: only the votes that still count.",
        params = {
            epoch = "The epoch to count in.",
            approval_epochs = "How many epochs a vote counts for.",
        },
    )]
    pub fn counting(mut self, epoch: u64, approval_epochs: u64) -> ChangeBallot {
        self.votes.retain(|vote| vote.counts(epoch, approval_epochs));
        self
    }

    #[document(parent = "ChangeBallot", summary = "Who voted a way on a change.")]
    pub fn voters(&self, change: &Change, vote: ChangeVote) -> Vec<Address> {
        self.votes.iter().filter(|cast| cast.change == *change && cast.vote == vote).map(|cast| cast.account).collect()
    }

    #[document(parent = "ChangeBallot", summary = "Who voted Yes on a change.")]
    pub fn approvers(&self, change: &Change) -> Vec<Address> {
        self.voters(change, ChangeVote::Yes)
    }

    #[document(parent = "ChangeBallot", summary = "The votes on a change, in the order they came.")]
    pub fn votes_on<'b>(&'b self, change: &'b Change) -> impl Iterator<Item = &'b CastVote> + 'b {
        self.votes.iter().filter(move |cast| cast.change == *change)
    }

    #[document(parent = "ChangeBallot", summary = "The changes voted on, each once, in the order their first vote came.")]
    pub fn changes(&self) -> Vec<Change> {
        let mut changes: Vec<Change> = Vec::new();

        for vote in &self.votes {
            if !changes.contains(&vote.change) {
                changes.push(vote.change.clone());
            }
        }

        changes
    }
}

#[document(
    summary = "How the counting votes on one change stand: what Yes, No and Abstain weigh, what every voter weighs together, what Yes needs, and the quorum's weight, which Yes, No and Abstain together must reach before Yes decides.",
    category = "Governance",
    see_also = "Changes::tally, ChangeVote",
)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    #[document(summary = "What the Yes votes weigh.")]
    pub yes: u256,
    #[document(summary = "What the No votes weigh.")]
    pub no: u256,
    #[document(summary = "What the Abstain votes weigh.")]
    pub abstain: u256,
    #[document(summary = "What every account that may vote weighs together.")]
    pub total: u256,
    #[document(summary = "What Yes must weigh to decide it.")]
    pub needed: u256,
    #[document(summary = "What the votes of any kind must weigh together before Yes decides it: the quorum's share of total, rounded up.")]
    pub quorum: u256,
}

impl Tally {
    #[document(parent = "Tally", summary = "What every vote weighs together, Abstain included: what the quorum is measured by.")]
    pub fn voted(&self) -> u256 {
        self.yes.saturating_add(self.no).saturating_add(self.abstain)
    }

    #[document(parent = "Tally", summary = "Whether it decides the change: Yes weighs what is needed, and the quorum has voted.")]
    pub fn decides(&self) -> bool {
        self.yes >= self.needed && self.voted() >= self.quorum
    }

    #[document(parent = "Tally", summary = "Whether the change can still pass: whether what has not voted No, Yes, Abstain and those yet to vote, could still reach what is needed.")]
    pub fn can_pass(&self) -> bool {
        self.total.saturating_sub(self.no) >= self.needed
    }

    #[document(parent = "Tally", summary = "How much more Yes needs to weigh, 0 once it weighs enough.")]
    pub fn more_needed(&self) -> u256 {
        self.needed.saturating_sub(self.yes)
    }
}

#[document(
    summary = "What a vote did.",
    category = "Governance",
    see_also = "Changes::vote",
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChangeVoted {
    #[document(summary = "It counted, and the change's votes now stand so.")]
    Counted(Tally),
    #[document(summary = "It decided the change, as the chain's decision-th and its kind's version-th.")]
    Decided { decision: u64, kind: ChangeKind, version: u64 },
    #[document(summary = "It left the change unable to pass, so the change is rejected: the chain's rejection-th, at its kind's version, which stays where it was.")]
    Rejected { rejection: u64, kind: ChangeKind, version: u64 },
}

#[document(
    summary = "How votes count: for how many epochs, and the quorum. Governance's settings, genesis's or those decided last.",
    category = "Governance",
    see_also = "Changes::vote, GovernanceConfig",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BallotRules {
    #[document(summary = "How many epochs a vote counts for.")]
    pub approval_epochs: u64,
    #[document(summary = "The share of the voters' weight, in thousandths, that must vote on a change before Yes decides it.")]
    pub quorum_permille: u32,
}

#[document(
    summary = "Governance's settings as decided last, and the first block that counts votes by them: the one after the block that decided them.",
    category = "Governance",
    see_also = "Change::Governance, Changes::governance",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct DecidedGovernance {
    #[document(summary = "The settings.")]
    pub config: crate::governance::GovernanceConfig,
    #[document(summary = "The first block they count votes in.")]
    pub from: u64,
}

impl Hashable for DecidedGovernance {}

/// The locks that stand, as a state value: genesis's and those decided since, in order.
#[derive(Clone, Debug, Default, PartialEq, Eq, SchemaWrite, SchemaRead)]
struct Locks {
    paths: Vec<String>,
}

impl Hashable for Locks {}

#[document(
    summary = "What genesis set of each setting governance changes: what Changes::in_force gives for a setting while governance decided none of it. Built once from genesis, with the chain's governance rules.",
    category = "Governance",
    see_also = "Changes::in_force, Genesis::governance_settings",
)]
#[derive(Clone, Debug, PartialEq)]
pub struct GenesisSettings {
    #[document(summary = "The consensus, which genesis fixes for its first epochs.")]
    pub consensus: ConsensusConfig,
    #[document(summary = "The bootnodes.")]
    pub bootnodes: Vec<String>,
    #[document(summary = "The names' prices, grace and most years, on a chain with names.")]
    pub names: Option<crate::registry::names::NameSettings>,
    #[document(summary = "What makes a token.")]
    pub token_functions: Vec<String>,
    #[document(summary = "What makes a collection, if genesis keeps a registry.")]
    pub collections: Option<crate::registry::collections::CollectionFunctions>,
    #[document(summary = "The EVM's spec, Prague while genesis names none.")]
    pub evm_spec: crate::params::vm::EvmSpec,
    #[document(summary = "The DEX's fee.")]
    pub dex: crate::params::vm::DexSettings,
    #[document(summary = "Anchoring's settings, all off while genesis names none.")]
    pub anchoring: crate::params::anchoring::AnchorSettings,
    #[document(summary = "The bridges' federation, if any.")]
    pub federation: Option<crate::registry::federation::Federation>,
    #[document(summary = "The fees, on a chain with them.")]
    pub fees: Option<crate::params::fees::FeeConfig>,
    #[document(summary = "The block reward.")]
    pub reward: crate::params::Reward,
    #[document(summary = "The limits, as a change of limits carries them.")]
    pub limits: crate::params::BlockLimits,
}

/// How many changes were rejected, as a state value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, SchemaWrite, SchemaRead)]
struct Rejections {
    count: u64,
}

impl Hashable for Rejections {}

#[document(
    summary = "Governance's records in chain state: the consensus fixed for each epoch, the one decided last, and the ballot for changing it. A decided change is fixed for epochs activation_delay on at the checkpoints that follow, like finality's voters, so every branch that split off after the checkpoint that fixed an epoch's consensus checks that epoch's blocks alike.",
    category = "Governance",
    see_also = "Governance, Change, Tx::VoteChange",
)]
pub struct Changes<'a> {
    state: &'a State,
}

impl<'a> Changes<'a> {
    #[document(parent = "Changes", summary = "The records in a state layer.")]
    pub fn new(state: &'a State) -> Changes<'a> {
        Changes { state }
    }

    #[document(
        parent = "Changes",
        summary = "Writes genesis's consensus as the one decided, and fixes it for the epochs genesis already decides.",
    )]
    pub fn write_genesis(&self, consensus: &ConsensusConfig, activation_delay: u64) -> Result<(), DbError> {
        self.state.put_value(Column::Validators, DECIDED.to_vec(), &DecidedChange {
            version: 0,
            consensus: consensus.clone(),
        });

        for epoch in 0..=activation_delay {
            self.fix(epoch)?;
        }

        Ok(())
    }

    #[document(parent = "Changes", summary = "The consensus fixed for an epoch, if any is yet.")]
    pub fn consensus_for(&self, epoch: u64) -> Result<Option<ConsensusConfig>, DbError> {
        self.state.get_value(Column::Epochs, &Changes::consensus_key(epoch))
    }

    #[document(parent = "Changes", summary = "Where an epoch's consensus is kept, in Column::Epochs: what a light client asks a proof of.")]
    pub fn consensus_key(epoch: u64) -> Vec<u8> {
        let mut key = b"consensus:".to_vec();
        key.extend_from_slice(&epoch.to_be_bytes());

        key
    }

    #[document(parent = "Changes", summary = "Fixes the decided consensus for an epoch. Each checkpoint fixes the epoch activation_delay ahead.")]
    pub fn fix(&self, epoch: u64) -> Result<(), DbError> {
        let decided = self.decided()?;
        self.state.put_value(Column::Epochs, Changes::consensus_key(epoch), &decided.consensus);

        Ok(())
    }

    #[document(
        parent = "Changes",
        summary = "The bootnodes decided last, or None while governance never decided any, when genesis's stand.",
    )]
    pub fn bootnodes(&self) -> Result<Option<Vec<String>>, DbError> {
        Ok(self.state.get_value::<DecidedBootnodes>(Column::Validators, BOOTNODES)?.map(|decided| decided.bootnodes))
    }

    #[document(
        parent = "Changes",
        summary = "The name settings decided last, or None while governance never decided any, when genesis's stand.",
    )]
    pub fn names(&self) -> Result<Option<crate::registry::names::NameSettings>, DbError> {
        self.state.get_value(Column::Validators, NAMES)
    }

    #[document(parent = "Changes", summary = "The token functions decided last, or None while governance never decided any, when genesis's stand.")]
    pub fn token_functions(&self) -> Result<Option<Vec<String>>, DbError> {
        Ok(self.state.get_value::<TokenFunctions>(Column::Validators, TOKEN_FUNCTIONS)?.map(|decided| decided.functions))
    }

    #[document(parent = "Changes", summary = "Every EVM spec governance decided, with the height each runs from.")]
    pub fn evm_specs(&self) -> Result<EvmSpecs, DbError> {
        Ok(self.state.get_value(Column::Validators, EVM_SPEC)?.unwrap_or_default())
    }

    #[document(
        parent = "Changes",
        summary = "The EVM spec a block at a height runs: the last decided to run by it, or genesis's.",
        params = { height = "The block's height.", genesis = "Genesis's spec." },
    )]
    pub fn evm_spec_at(&self, height: u64, genesis: crate::params::vm::EvmSpec) -> Result<crate::params::vm::EvmSpec, DbError> {
        Ok(self.evm_specs()?.eras.into_iter().rev().find(|(from, _)| *from <= height).map_or(genesis, |(_, spec)| spec))
    }

    #[document(parent = "Changes", summary = "Anchoring's settings decided last, or None while governance never decided any, when genesis's stand.")]
    pub fn anchoring(&self) -> Result<Option<crate::params::anchoring::AnchorSettings>, DbError> {
        self.state.get_value(Column::Validators, ANCHORING)
    }

    #[document(parent = "Changes", summary = "The DEX's settings decided last, or None while governance never decided any, when genesis's stand.")]
    pub fn dex(&self) -> Result<Option<crate::params::vm::DexSettings>, DbError> {
        self.state.get_value(Column::Validators, DEX)
    }

    #[document(
        parent = "Changes",
        summary = "What makes a collection, as decided last, or None while governance never decided it, when genesis's stands.",
    )]
    pub fn collections(&self) -> Result<Option<crate::registry::collections::CollectionFunctions>, DbError> {
        self.state.get_value(Column::Validators, COLLECTIONS)
    }

    #[document(
        parent = "Changes",
        summary = "The bridges' federation in force, with its version: genesis's as 0, written as the chain starts, or the one governance decided last; None on a chain with neither.",
    )]
    pub fn federation(&self) -> Result<Option<crate::registry::federation::DecidedFederation>, DbError> {
        self.state.get_value(Column::Validators, FEDERATION)
    }

    #[document(
        parent = "Changes",
        summary = "The directory in force: genesis's, written as the chain starts, or the one governance decided last; None while there is none. One state entry either way, so a light client proves it.",
    )]
    pub fn directory(&self) -> Result<Option<crate::registry::directory::Directory>, DbError> {
        self.state.get_value(Column::Validators, DIRECTORY)
    }

    #[document(parent = "Changes", summary = "Writes genesis's federation as the one in force, version 0, as the chain starts, and keeps it for good as decided at height 0, at genesis's time.")]
    pub fn write_genesis_federation(&self, federation: &crate::registry::federation::Federation, time: u64) {
        let decided = crate::registry::federation::DecidedFederation { version: 0, federation: federation.clone() };

        self.write_federation(&decided, crate::registry::federation::DecidedWhen { height: 0, time });
    }

    #[document(
        parent = "Changes",
        summary = "Writes a federation as the one in force, and keeps it for good by its version with the block that decided it, so contracts read it once another replaced it: what genesis and a decided Change::Federation both write.",
    )]
    pub fn write_federation(&self, decided: &crate::registry::federation::DecidedFederation, when: crate::registry::federation::DecidedWhen) {
        use crate::registry::federation::{FederationRecord, federation_history_key};

        self.state.put_value(Column::Validators, FEDERATION.to_vec(), decided);
        self.state.put_value(Column::Validators, federation_history_key(decided.version), &FederationRecord { federation: decided.federation.clone(), decided: when });
    }

    #[document(parent = "Changes", summary = "The federation of a version as the chain keeps it for good, with the block that decided it; None for one never decided.")]
    pub fn federation_record(&self, version: u64) -> Result<Option<crate::registry::federation::FederationRecord>, DbError> {
        self.state.get_value(Column::Validators, &crate::registry::federation::federation_history_key(version))
    }

    #[document(
        parent = "Changes",
        summary = "The federation of a version, in force or past, with the block that decided it and the one that decided the next, once one did: what federation.at reads. One in force that was written without its record, as on a chain from before records were kept, is given alone, with neither block. None for a version never decided.",
    )]
    pub fn federation_at(&self, version: u64) -> Result<Option<crate::registry::federation::FederationAt>, DbError> {
        use crate::registry::federation::FederationAt;

        let replaced = match version.checked_add(1) {
            Some(next) => self.federation_record(next)?.map(|record| record.decided),
            None => None,
        };

        if let Some(record) = self.federation_record(version)? {
            return Ok(Some(FederationAt { version, federation: record.federation, decided: Some(record.decided), replaced }));
        }

        Ok(self.federation()?.filter(|decided| decided.version == version).map(|decided| FederationAt { version, federation: decided.federation, decided: None, replaced: None }))
    }

    #[document(parent = "Changes", summary = "The fees governance decided last, or None while it decided none, when genesis's stand.")]
    pub fn fees(&self) -> Result<Option<crate::params::fees::FeeConfig>, DbError> {
        self.state.get_value(Column::Validators, FEES)
    }

    #[document(parent = "Changes", summary = "The block reward governance decided last, with the height it took effect at, or None while it decided none, when genesis's stands.")]
    pub fn reward(&self) -> Result<Option<DecidedReward>, DbError> {
        self.state.get_value(Column::Validators, REWARD)
    }

    #[document(parent = "Changes", summary = "The chain's name in force: genesis's, written as the chain starts, or the one governance decided last; None for a chain with neither.")]
    pub fn name(&self) -> Result<Option<String>, DbError> {
        Ok(self.state.get_value::<ChainName>(Column::Validators, NAME)?.map(|chain| chain.name))
    }

    #[document(parent = "Changes", summary = "Writes a name as the chain's in force: genesis's as the chain starts, or one governance decided.")]
    pub fn write_name(&self, name: &str) {
        self.state.put_value(Column::Validators, NAME.to_vec(), &ChainName { name: name.to_string() });
    }

    #[document(parent = "Changes", summary = "The upgrade governance decided last, or None while it decided none: from its height on, nodes older than its version stop.")]
    pub fn upgrade(&self) -> Result<Option<crate::structs::version::Upgrade>, DbError> {
        self.state.get_value(Column::Validators, UPGRADE)
    }

    #[document(parent = "Changes", summary = "The limits governance decided last, or None while it decided none, when genesis's stand.")]
    pub fn limits(&self) -> Result<Option<crate::params::BlockLimits>, DbError> {
        self.state.get_value(Column::Validators, LIMITS)
    }

    #[document(parent = "Changes", summary = "Every finality config governance decided, with the epoch each took effect from.")]
    pub fn finality_eras(&self) -> Result<FinalityEras, DbError> {
        Ok(self.state.get_value(Column::Validators, FINALITY)?.unwrap_or_default())
    }

    #[document(
        parent = "Changes",
        summary = "The finality config an epoch runs with, as governance decided it: the last that took effect by the epoch, or None before any, when genesis's stands.",
    )]
    pub fn finality_at(&self, epoch: u64) -> Result<Option<crate::params::finality::FinalityConfig>, DbError> {
        Ok(self.finality_eras()?.eras.into_iter().rev().find(|era| era.from <= epoch).map(|era| era.config))
    }

    #[document(
        parent = "Changes",
        summary = "The finality an epoch runs: the config governance decided for it, or genesis's.",
        params = { genesis = "The chain's finality from genesis." },
    )]
    pub fn finality_in(&self, epoch: u64, genesis: &crate::params::finality::FinalityConfig) -> Result<crate::params::finality::FinalityConfig, DbError> {
        Ok(self.finality_at(epoch)?.unwrap_or_else(|| genesis.clone()))
    }

    #[document(
        parent = "Changes",
        summary = "The finality decided last, whether or not it runs yet: what a new change of finality is checked against.",
        params = { genesis = "The chain's finality from genesis." },
    )]
    pub fn finality_decided(&self, genesis: &crate::params::finality::FinalityConfig) -> Result<crate::params::finality::FinalityConfig, DbError> {
        Ok(self.finality_eras()?.eras.pop().map_or_else(|| genesis.clone(), |era| era.config))
    }

    #[document(
        parent = "Changes",
        summary = "A decided switch of finality's kind that has not taken effect by an epoch: the epoch it runs from and the new config, or None.",
        params = { genesis = "The chain's finality from genesis." },
    )]
    pub fn switch_ahead(&self, epoch: u64, genesis: &crate::params::finality::FinalityConfig) -> Result<Option<FinalityEra>, DbError> {
        let running = self.finality_in(epoch, genesis)?;

        Ok(self.finality_eras()?.eras.into_iter().find(|era| era.from > epoch && era.config.name() != running.name()))
    }

    #[document(
        parent = "Changes",
        summary = "Drops the finality decided to run from an epoch, and whatever was decided after it: a switch into FFG that lapsed, no validator having bonded.",
    )]
    pub fn lapse_finality(&self, from: u64) -> Result<(), DbError> {
        let mut eras = self.finality_eras()?;
        eras.eras.retain(|era| era.from < from);

        match eras.eras.is_empty() {
            true => self.state.delete(Column::Validators, FINALITY.to_vec()),
            false => self.state.put_value(Column::Validators, FINALITY.to_vec(), &eras),
        }

        Ok(())
    }

    #[document(parent = "Changes", summary = "Writes a directory as the one in force: genesis's as the chain starts, or one governance decided.")]
    pub fn write_directory(&self, directory: &crate::registry::directory::Directory) {
        self.state.put_value(Column::Validators, DIRECTORY.to_vec(), directory);
    }

    #[document(parent = "Changes", summary = "The consensus decided last, and its version: the count of every change decided, of any kind.")]
    pub fn decided(&self) -> Result<DecidedChange, DbError> {
        self.state
            .get_value(Column::Validators, DECIDED)?
            .ok_or_else(|| DbError::Corrupt("no decided consensus".to_string()))
    }

    #[document(parent = "Changes", summary = "How many changes of a kind were decided: what an approval of one names.")]
    pub fn version_of(&self, kind: ChangeKind) -> Result<u64, DbError> {
        Ok(self.state.get_value::<KindVersion>(Column::Validators, &version_key(kind))?.unwrap_or_default().version)
    }

    #[document(
        parent = "Changes",
        summary = "The votes so far on changing a kind of setting from what was decided last, expired ones included: ChangeBallot::counting leaves only those that count.",
    )]
    pub fn ballot(&self, kind: ChangeKind) -> Result<ChangeBallot, DbError> {
        let version = self.version_of(kind)?;
        let ballot: ChangeBallot = self.state.get_value(Column::Validators, &ballot_key(kind))?.unwrap_or_default();

        // Approvals for an older version never count.
        Ok(match ballot.version == version {
            true => ballot,
            false => ChangeBallot { version, ..ChangeBallot::default() },
        })
    }

    #[document(parent = "Changes", summary = "Every kind's ballot with a vote in it, expired ones included, in kind order.")]
    pub fn ballots(&self) -> Result<Vec<(ChangeKind, ChangeBallot)>, DbError> {
        let mut ballots = Vec::new();

        for kind in ChangeKind::ALL {
            let ballot = self.ballot(kind)?;

            if !ballot.votes.is_empty() {
                ballots.push((kind, ballot));
            }
        }

        Ok(ballots)
    }

    #[document(parent = "Changes", summary = "Governance's settings as decided last, with the block they count votes from, or None while governance never changed itself, when genesis's stand.")]
    pub fn governance(&self) -> Result<Option<DecidedGovernance>, DbError> {
        self.state.get_value(Column::Validators, GOVERNANCE)
    }

    #[document(parent = "Changes", summary = "Where the locks are kept, in Column::Validators: change:locks, the one entry a light client proves them by.")]
    pub fn locks_key() -> &'static [u8] {
        LOCKS
    }

    #[document(parent = "Changes", summary = "The locks a change:locks entry holds, as a light client proved its bytes: None for bytes that are no locks.")]
    pub fn decode_locks(bytes: &[u8]) -> Option<Vec<String>> {
        wincode::deserialize::<Locks>(bytes).ok().map(|locks| locks.paths)
    }

    #[document(parent = "Changes", summary = "The locks that stand, in order: genesis's, written as the chain starts, and every one decided since. None is ever taken away.")]
    pub fn locks(&self) -> Result<Vec<String>, DbError> {
        Ok(self.state.get_value::<Locks>(Column::Validators, LOCKS)?.unwrap_or_default().paths)
    }

    #[document(parent = "Changes", summary = "Adds locks to those that stand, keeping them in order, each once: genesis's as the chain starts, or a decided Change::Locks's. One state entry, change:locks, so a light client proves them.")]
    pub fn add_locks(&self, paths: &[String]) -> Result<(), DbError> {
        let mut locks = self.locks()?;
        locks.extend_from_slice(paths);
        locks.sort();
        locks.dedup();
        self.state.put_value(Column::Validators, LOCKS.to_vec(), &Locks { paths: locks });

        Ok(())
    }

    #[cfg(feature = "governance")]
    #[document(
        parent = "Changes",
        summary = "Every setting governance changes, as it stands in this state: the consensus fixed for the epoch, the finality it runs, and of every other setting the last decided, or genesis's while none was, with the locks; what GET /governance shows as in_force and what a lock compares a change with. A setting with neither, such as a chain's first directory, is left out.",
        params = {
            epoch = "The epoch it is read for.",
            genesis = "What genesis set.",
            finality = "Genesis's finality.",
            governance = "Governance's own settings in force.",
        },
    )]
    pub fn in_force(&self, epoch: u64, genesis: &GenesisSettings, finality: &crate::params::finality::FinalityConfig, governance: &crate::governance::GovernanceConfig) -> Result<Vec<Change>, DbError> {
        let specs = self.evm_specs()?;
        let collections = self.collections()?.or_else(|| genesis.collections.clone());
        let settings = [
            Some(Change::Consensus(self.consensus_for(epoch)?.unwrap_or_else(|| genesis.consensus.clone()))),
            self.name()?.map(Change::Name),
            Some(Change::Bootnodes(self.bootnodes()?.unwrap_or_else(|| genesis.bootnodes.clone()))),
            self.names()?.or_else(|| genesis.names.clone()).map(Change::Names),
            Some(Change::TokenFunctions(self.token_functions()?.unwrap_or_else(|| genesis.token_functions.clone()))),
            Some(Change::EvmSpec { spec: specs.eras.last().map_or(genesis.evm_spec, |(_, spec)| *spec), from: specs.eras.last().map_or(0, |(from, _)| *from) }),
            Some(Change::Dex(self.dex()?.unwrap_or(genesis.dex))),
            Some(Change::Anchoring(self.anchoring()?.unwrap_or_else(|| genesis.anchoring.clone()))),
            collections.map(Change::Collections),
            self.federation()?.map(|decided| decided.federation).or_else(|| genesis.federation.clone()).map(Change::Federation),
            self.directory()?.map(Change::Directory),
            self.fees()?.or_else(|| genesis.fees.clone()).map(Change::Fees),
            Some(Change::Reward(self.reward()?.map_or(genesis.reward, |decided| decided.reward))),
            Some(Change::Limits(self.limits()?.unwrap_or(genesis.limits))),
            Some(Change::Finality(self.finality_in(epoch, finality)?)),
            self.upgrade()?.map(Change::Upgrade),
            Some(Change::Governance(governance.clone())),
            Some(Change::Locks(self.locks()?)),
        ];

        Ok(settings.into_iter().flatten().collect())
    }

    #[document(parent = "Changes", summary = "How many changes were rejected since genesis, of every kind: the count a light client follows without gaps, as it does the decisions.")]
    pub fn rejections(&self) -> Result<u64, DbError> {
        Ok(self.state.get_value::<Rejections>(Column::Validators, REJECTED)?.unwrap_or_default().count)
    }

    #[cfg(feature = "governance")]
    #[document(
        parent = "Changes",
        summary = "How the counting votes on a change stand in this state: each voter's weight as governance counts it now, which a voter that lost its standing since no longer adds to, beside what Yes needs, the whole electorate's weight and the quorum's.",
        params = {
            ballot = "The ballot, already left to the votes that count.",
            change = "The change.",
            epoch = "The epoch it is counted in.",
            quorum_permille = "The quorum, in thousandths of the whole.",
            governance = "Who may vote, and what it takes.",
        },
    )]
    pub fn tally(&self, ballot: &ChangeBallot, change: &Change, epoch: u64, quorum_permille: u32, governance: &dyn Governance) -> Result<Tally, DbError> {
        let mut tally = Tally {
            total: governance.total(self.state, epoch)?,
            needed: governance.needed(self.state, epoch)?,
            ..Tally::default()
        };

        for cast in ballot.votes_on(change) {
            let weight = governance.weight(self.state, epoch, &cast.account)?.unwrap_or(u256::ZERO);
            let side = match cast.vote {
                ChangeVote::Yes => &mut tally.yes,
                ChangeVote::No => &mut tally.no,
                ChangeVote::Abstain => &mut tally.abstain,
            };
            *side = side.saturating_add(weight);
        }

        // The quorum's share of the whole, rounded up, so a quorum of any share asks at least one
        // vote's weight once there is any.
        let thousand = u256::from(1000u64);
        tally.quorum = tally.total.saturating_mul(u256::from(quorum_permille)).saturating_add(thousand - u256::ONE) / thousand;

        Ok(tally)
    }

    #[cfg(feature = "governance")]
    #[document(
        parent = "Changes",
        summary = "Counts an account's vote on a change, replacing its old vote on it. Once Yes weighs what governance needs and the quorum has voted, the change is decided, and the checkpoints that follow fix it for later epochs; once what has not voted No could no longer reach what governance needs, it is rejected instead, its votes cleared, and the same change refused at the kind's version from then on.",
        params = {
            sender = "Who voted: an account governance lets vote.",
            version = "The version of the change's kind it changes, which must be the decided one's.",
            change = "The change it votes on, checked to fit the chain already.",
            vote = "Yes, No or Abstain.",
            at = "Where the vote is cast: the block, its epoch and finality's activation_delay.",
            rules = "How long votes count, and the quorum.",
            governance = "Who may vote, and what it takes.",
        },
        returns = "Whether that decided or rejected it. NotGoverning for a sender that may not vote, StaleChange for an older version, SameChange for what stands already, RejectedChange for a change rejected at this version, BadChange in the block that changed governance itself.",
    )]
    #[allow(clippy::too_many_arguments)]
    pub fn vote(&self, sender: &Address, version: u64, change: Change, vote: ChangeVote, at: DecidedAt, rules: BallotRules, governance: &dyn Governance) -> Result<ChangeVoted, RegistryError> {
        let epoch = at.epoch;
        let kind = change.kind();
        let decided = self.standing(sender, kind, version, at, governance)?;
        let same = match &change {
            Change::Consensus(consensus) => *consensus == decided.consensus,
            Change::Bootnodes(bootnodes) => self.bootnodes()?.as_ref() == Some(bootnodes),
            Change::Names(settings) => self.names()?.as_ref() == Some(settings),
            Change::TokenFunctions(functions) => self.token_functions()?.as_ref() == Some(functions),
            Change::EvmSpec { spec, .. } => self.evm_specs()?.eras.last().map(|(_, last)| last) == Some(spec),
            Change::Dex(dex) => self.dex()?.as_ref() == Some(dex),
            Change::Anchoring(settings) => self.anchoring()?.as_ref() == Some(settings),
            Change::Collections(functions) => self.collections()?.as_ref() == Some(functions),
            Change::Federation(federation) => self.federation()?.map(|decided| decided.federation).as_ref() == Some(federation),
            Change::Directory(directory) => self.directory()?.as_ref() == Some(directory),
            Change::Fees(fees) => self.fees()?.as_ref() == Some(fees),
            Change::Reward(reward) => self.reward()?.map(|decided| decided.reward).as_ref() == Some(reward),
            Change::Limits(limits) => self.limits()?.as_ref() == Some(limits),
            Change::Finality(config) | Change::SwitchFinality { config, .. } => self.finality_eras()?.eras.last().map(|era| &era.config) == Some(config),
            Change::Upgrade(upgrade) => self.upgrade()?.as_ref() == Some(upgrade),
            Change::Name(name) => self.name()?.as_ref() == Some(name),
            Change::Governance(config) => self.governance()?.map(|decided| decided.config).as_ref() == Some(config),
            Change::Locks(paths) => {
                let standing = self.locks()?;
                paths.iter().all(|path| standing.contains(path))
            }
        };

        if same {
            return Err(RegistryError::SameChange);
        }

        let mut ballot = self.ballot(kind)?.counting(epoch, rules.approval_epochs);

        if ballot.rejected.contains(&change) {
            return Err(RegistryError::RejectedChange);
        }

        // One vote an account on each change, and one Yes an account in the kind.
        ballot.votes.retain(|cast| cast.account != *sender || (cast.change != change && (vote != ChangeVote::Yes || cast.vote != ChangeVote::Yes)));
        ballot.votes.push(CastVote {
            account: *sender,
            change: change.clone(),
            vote,
            epoch,
        });
        let tally = self.tally(&ballot, &change, epoch, rules.quorum_permille, governance)?;

        if !tally.decides() {
            if tally.can_pass() {
                self.state.put_value(Column::Validators, ballot_key(kind), &ballot);

                return Ok(ChangeVoted::Counted(tally));
            }

            // It can no longer pass: its votes go, and it stays rejected while the kind's version
            // does. The version itself stays, as nothing it names changed, so every other change
            // of the kind keeps its votes and every vote in flight still counts.
            let rejection = self.rejections()? + 1;
            ballot.votes.retain(|cast| cast.change != change);
            ballot.rejected.push(change);
            self.state.put_value(Column::Validators, ballot_key(kind), &ballot);
            self.state.put_value(Column::Validators, REJECTED.to_vec(), &Rejections { count: rejection });

            return Ok(ChangeVoted::Rejected { rejection, kind, version });
        }

        // The chain's count of decisions, and the kind's own: only its ballot empties.
        let decision = decided.version + 1;
        let version = version + 1;
        self.state.put_value(Column::Validators, version_key(kind), &KindVersion { version });
        let consensus = match change {
            Change::Consensus(consensus) => consensus,
            // Bootnodes take effect at once: nothing checks blocks by them.
            Change::Bootnodes(bootnodes) => {
                self.state.put_value(Column::Validators, BOOTNODES.to_vec(), &DecidedBootnodes { bootnodes });

                decided.consensus
            }
            // Names, tokens and the DEX's fee are read for each block from its parent's state, so
            // they hold from the next block.
            Change::Names(settings) => {
                self.state.put_value(Column::Validators, NAMES.to_vec(), &settings);

                decided.consensus
            }
            Change::TokenFunctions(functions) => {
                self.state.put_value(Column::Validators, TOKEN_FUNCTIONS.to_vec(), &TokenFunctions { functions });

                decided.consensus
            }
            Change::Dex(dex) => {
                self.state.put_value(Column::Validators, DEX.to_vec(), &dex);

                decided.consensus
            }
            // Read for each block from its parent's state, as the DEX's fee is.
            Change::Anchoring(settings) => {
                self.state.put_value(Column::Validators, ANCHORING.to_vec(), &settings);

                decided.consensus
            }
            // An EVM spec runs from the height it names, which is ahead; one decided later
            // replaces what was to run from its height on.
            Change::EvmSpec { spec, from } => {
                let mut specs = self.evm_specs()?;
                specs.eras.retain(|(at, _)| *at < from);
                specs.eras.push((from, spec));
                self.state.put_value(Column::Validators, EVM_SPEC.to_vec(), &specs);

                decided.consensus
            }
            // And what makes a collection: deploys read it as they run.
            Change::Collections(functions) => {
                self.state.put_value(Column::Validators, COLLECTIONS.to_vec(), &functions);

                decided.consensus
            }
            // And the federation: contracts read it as they run, and its version counts the ones
            // decided, after genesis's 0.
            // Each is kept for good too, with the block that decided it, which is when the one
            // before was replaced: the Bitcoin gateway counts an old federation's grace from it.
            Change::Federation(federation) => {
                let version = self.federation()?.map_or(1, |decided| decided.version + 1);
                let when = crate::registry::federation::DecidedWhen { height: at.height, time: at.time };
                self.write_federation(&crate::registry::federation::DecidedFederation { version, federation }, when);

                decided.consensus
            }
            // And the directory: only the apps and tools read it.
            Change::Directory(directory) => {
                self.write_directory(&directory);

                decided.consensus
            }
            // Fees, the reward and limits apply from the next block: the chain reads them from
            // state for each block it runs.
            Change::Fees(fees) => {
                self.state.put_value(Column::Validators, FEES.to_vec(), &fees);

                decided.consensus
            }
            Change::Reward(reward) => {
                self.state.put_value(Column::Validators, REWARD.to_vec(), &DecidedReward { reward, from: at.height + 1 });

                decided.consensus
            }
            Change::Limits(limits) => {
                self.state.put_value(Column::Validators, LIMITS.to_vec(), &limits);

                decided.consensus
            }
            // A name only tells the apps what to call the chain.
            Change::Name(name) => {
                self.write_name(&name);

                decided.consensus
            }
            // An upgrade only tells nodes when to stop: blocks are never checked by it.
            Change::Upgrade(upgrade) => {
                self.state.put_value(Column::Validators, UPGRADE.to_vec(), &upgrade);

                decided.consensus
            }
            // Finality's settings wait for an epoch far enough ahead that every node has fixed
            // it the same way, as the consensus does.
            Change::Finality(config) => {
                let mut eras = self.finality_eras()?;
                let from = epoch + at.activation_delay + 1;
                eras.eras.retain(|era| era.from < from);
                eras.eras.push(FinalityEra { from, config });
                self.state.put_value(Column::Validators, FINALITY.to_vec(), &eras);

                decided.consensus
            }
            // A switch of kind runs from the epoch it names, or the first one far enough ahead.
            Change::SwitchFinality { epoch: named, config } => {
                let mut eras = self.finality_eras()?;
                let from = named.max(epoch + at.activation_delay + 1);
                eras.eras.retain(|era| era.from < from);
                eras.eras.push(FinalityEra { from, config });
                self.state.put_value(Column::Validators, FINALITY.to_vec(), &eras);

                decided.consensus
            }
            // Locks hold at once, for every vote after this one: they are only ever added to.
            Change::Locks(paths) => {
                self.add_locks(&paths)?;

                decided.consensus
            }
            // Governance's own settings count votes from the next block. Every vote open now was
            // cast for, and weighed by, the settings that go, so none carries over: each kind's
            // ballot empties, while its version stays, since nothing it names changed.
            Change::Governance(config) => {
                self.state.put_value(Column::Validators, GOVERNANCE.to_vec(), &DecidedGovernance { config, from: at.height + 1 });

                for other in ChangeKind::ALL {
                    self.state.delete(Column::Validators, ballot_key(other));
                }

                decided.consensus
            }
        };
        self.state.put_value(Column::Validators, DECIDED.to_vec(), &DecidedChange { version: decision, consensus });
        self.state.delete(Column::Validators, ballot_key(kind));

        Ok(ChangeVoted::Decided { decision, kind, version })
    }

    #[cfg(feature = "governance")]
    #[document(
        parent = "Changes",
        summary = "Clears an account's vote on a change, while it still counts, so the account has no say on it until it votes again.",
        params = {
            sender = "The account.",
            version = "The kind's version its vote was for, which must be the decided one's.",
            change = "The change it voted on.",
            at = "Where it is cleared: the block and its epoch.",
            approval_epochs = "How many epochs a vote counts for.",
            governance = "Who may vote.",
        },
        returns = "NotGoverning for a sender that may not vote, StaleChange for an older version, NoVote when it has no vote on the change that still counts.",
    )]
    pub fn clear(&self, sender: &Address, version: u64, change: &Change, at: DecidedAt, approval_epochs: u64, governance: &dyn Governance) -> Result<(), RegistryError> {
        let kind = change.kind();
        self.standing(sender, kind, version, at, governance)?;
        let mut ballot = self.ballot(kind)?.counting(at.epoch, approval_epochs);
        let before = ballot.votes.len();
        ballot.votes.retain(|cast| cast.account != *sender || cast.change != *change);

        if ballot.votes.len() == before {
            return Err(RegistryError::NoVote);
        }

        match ballot.votes.is_empty() && ballot.rejected.is_empty() {
            true => self.state.delete(Column::Validators, ballot_key(kind)),
            false => self.state.put_value(Column::Validators, ballot_key(kind), &ballot),
        }

        Ok(())
    }

    #[cfg(feature = "governance")]
    /// What was decided last, for a sender with standing, at a version that is its kind's, in a
    /// block that did not change governance itself: the rest of that one counts by the old
    /// settings, and its votes would be cleared by the new.
    fn standing(&self, sender: &Address, kind: ChangeKind, version: u64, at: DecidedAt, governance: &dyn Governance) -> Result<DecidedChange, RegistryError> {
        if governance.weight(self.state, at.epoch, sender)?.is_none() {
            return Err(RegistryError::NotGoverning);
        }

        if self.governance()?.is_some_and(|decided| decided.from > at.height) {
            return Err(RegistryError::BadChange("governance changed itself in this block; vote from the next"));
        }

        let decided = self.version_of(kind)?;

        if version != decided {
            return Err(RegistryError::StaleChange(decided));
        }

        Ok(self.decided()?)
    }
}

#[cfg(all(test, feature = "governance"))]
mod tests {
    use super::*;

    /// An approval made at an epoch, finality fixing what is decided two epochs ahead.
    fn at(epoch: u64) -> DecidedAt {
        DecidedAt { height: epoch * 10, epoch, activation_delay: 2, time: epoch * 10_000 }
    }
    use crate::db::MemoryDb;
    use crate::structs::address::Address;
    use std::sync::Arc;

    /// Three accounts, the first weighing two, needing three of the four.
    struct Weighted;

    fn member(seed: u8) -> Address {
        Address::Ed25519([seed; 32])
    }

    impl Governance for Weighted {
        fn name(&self) -> &'static str {
            "weighted"
        }

        fn weight(&self, _state: &State, _epoch: u64, account: &Address) -> Result<Option<u256>, DbError> {
            Ok(match account {
                account if *account == member(1) => Some(u256::from(2u64)),
                account if *account == member(2) || *account == member(3) => Some(u256::ONE),
                _ => None,
            })
        }

        fn needed(&self, _state: &State, _epoch: u64) -> Result<u256, DbError> {
            Ok(u256::from(3u64))
        }

        fn total(&self, _state: &State, _epoch: u64) -> Result<u256, DbError> {
            Ok(u256::from(4u64))
        }

        fn voters(&self, _state: &State, _epoch: u64) -> Result<Vec<(Address, u256)>, DbError> {
            Ok(vec![(member(1), u256::from(2u64)), (member(2), u256::ONE), (member(3), u256::ONE)])
        }
    }

    /// Votes counting for some epochs, with no quorum.
    fn rules(approval_epochs: u64) -> BallotRules {
        BallotRules { approval_epochs, quorum_permille: 0 }
    }

    /// A vote that counted, Yes then weighing this much of the three needed, out of four.
    fn counted(yes: u256) -> Result<ChangeVoted, RegistryError> {
        Ok(ChangeVoted::Counted(Tally { yes, total: u256::from(4u64), needed: u256::from(3u64), ..Tally::default() }))
    }

    fn pow() -> ConsensusConfig {
        ConsensusConfig::ProofOfWork(crate::consensus::PowConfig {
            hash: crate::structs::hash::HashType::default_chain(),
            target_block_ms: 1_000,
            initial_difficulty: 1,
            min_difficulty: 1,
            tuning: None,
            window: None,
        })
    }

    fn staked() -> Change {
        Change::Consensus(ConsensusConfig::Staked {
            slot_ms: 2_000,
            schedule: crate::consensus::Schedule::Draw,
        })
    }

    #[test]
    fn approvals_weigh_up_to_a_decision_and_the_checkpoints_fix_it() {
        let state = State::new(Arc::new(MemoryDb::default()));
        let changes = Changes::new(&state);
        changes.write_genesis(&pow(), 2).unwrap();

        assert_eq!(changes.decided().unwrap(), DecidedChange { version: 0, consensus: pow() });
        assert_eq!(changes.consensus_for(2).unwrap(), Some(pow()));
        assert_eq!(changes.consensus_for(3).unwrap(), None);

        // A stranger may not; the same consensus is no change; a stale version is refused.
        assert_eq!(changes.vote(&member(9), 0, staked(), ChangeVote::Yes, at(1), rules(4), &Weighted), Err(RegistryError::NotGoverning));
        assert_eq!(changes.vote(&member(2), 0, Change::Consensus(pow()), ChangeVote::Yes, at(1), rules(4), &Weighted), Err(RegistryError::SameChange));
        assert_eq!(changes.vote(&member(2), 1, staked(), ChangeVote::Yes, at(1), rules(4), &Weighted), Err(RegistryError::StaleChange(0)));

        // One of weight one, then the one of weight two decide it: three of three.
        assert_eq!(
            changes.vote(&member(2), 0, staked(), ChangeVote::Yes, at(1), rules(4), &Weighted),
            counted(u256::ONE)
        );
        assert_eq!(changes.ballot(ChangeKind::Consensus).unwrap().approvers(&staked()), [member(2)]);
        assert_eq!(changes.ballot(ChangeKind::Consensus).unwrap().changes(), [staked()]);
        assert_eq!(changes.vote(&member(1), 0, staked(), ChangeVote::Yes, at(2), rules(4), &Weighted), Ok(ChangeVoted::Decided { decision: 1, kind: ChangeKind::Consensus, version: 1 }));

        let Change::Consensus(consensus) = staked() else { unreachable!() };

        assert_eq!(changes.decided().unwrap(), DecidedChange { version: 1, consensus: consensus.clone() });
        assert!(changes.ballot(ChangeKind::Consensus).unwrap().votes.is_empty());

        // The next checkpoints fix it ahead; the epochs fixed before keep the old one.
        changes.fix(5).unwrap();

        assert_eq!(changes.consensus_for(5).unwrap(), Some(consensus));
        assert_eq!(changes.consensus_for(2).unwrap(), Some(pow()));
    }

    #[test]
    fn a_name_is_decided_on_the_ballot_in_place_of_genesis_s() {
        let state = State::new(Arc::new(MemoryDb::default()));
        let changes = Changes::new(&state);
        changes.write_genesis(&pow(), 2).unwrap();
        changes.write_name("chainv4 testnet");
        let renamed = Change::Name("chainv4 mainnet".to_string());

        assert_eq!(changes.vote(&member(1), 0, Change::Name("chainv4 testnet".to_string()), ChangeVote::Yes, at(1), rules(4), &Weighted), Err(RegistryError::SameChange));

        changes.vote(&member(1), 0, renamed.clone(), ChangeVote::Yes, at(1), rules(4), &Weighted).unwrap();
        assert_eq!(changes.name().unwrap().as_deref(), Some("chainv4 testnet"), "genesis's stands until decided");
        assert_eq!(changes.vote(&member(2), 0, renamed, ChangeVote::Yes, at(1), rules(4), &Weighted), Ok(ChangeVoted::Decided { decision: 1, kind: ChangeKind::Name, version: 1 }));
        assert_eq!(changes.name().unwrap().as_deref(), Some("chainv4 mainnet"));
        assert_eq!(changes.decided().unwrap().consensus, pow(), "the consensus stays");
    }

    #[test]
    fn every_federation_is_kept_for_good_with_the_block_that_decided_it_and_the_one_that_replaced_it() {
        use crate::registry::federation::{DecidedWhen, Federation};

        let federation = |threshold: u32| Federation { signers: vec![[2; 33], [3; 33]], threshold, chain_depth: 20, evm_depth: 12, bitcoin_depth: 6, btc: None };
        let state = State::new(Arc::new(MemoryDb::default()));
        let changes = Changes::new(&state);
        changes.write_genesis(&pow(), 2).unwrap();

        assert_eq!(changes.federation_at(0).unwrap(), None, "none until genesis or governance names one");
        changes.write_genesis_federation(&federation(1), 5_000);
        let genesis = changes.federation_at(0).unwrap().unwrap();
        assert_eq!((genesis.federation.threshold, genesis.decided, genesis.replaced), (1, Some(DecidedWhen { height: 0, time: 5_000 }), None));

        // Decided by the ballot: in force from then, and genesis's replaced by that block.
        changes.vote(&member(1), 0, Change::Federation(federation(2)), ChangeVote::Yes, at(3), rules(4), &Weighted).unwrap();
        changes.vote(&member(2), 0, Change::Federation(federation(2)), ChangeVote::Yes, at(3), rules(4), &Weighted).unwrap();
        let third = DecidedWhen { height: 30, time: 30_000 };
        assert_eq!(changes.federation().unwrap().map(|decided| decided.version), Some(1));
        assert_eq!(changes.federation_at(0).unwrap().unwrap().replaced, Some(third));
        let now = changes.federation_at(1).unwrap().unwrap();
        assert_eq!((now.federation.threshold, now.decided, now.replaced), (2, Some(third), None));
        assert_eq!(changes.federation_at(2).unwrap(), None, "a version never decided");
        assert_eq!(changes.federation_at(u64::MAX).unwrap(), None);
    }

    #[test]
    fn each_kind_has_its_ballot_and_version_and_a_decision_leaves_the_others_standing() {
        let state = State::new(Arc::new(MemoryDb::default()));
        let changes = Changes::new(&state);
        changes.write_genesis(&pow(), 2).unwrap();
        let bootnodes = Change::Bootnodes(vec!["/ip4/203.0.113.7/udp/4001/quic-v1".to_string()]);

        assert_eq!(changes.bootnodes().unwrap(), None, "genesis's stand until governance decides");

        // One account backs a change of each kind at once.
        changes.vote(&member(2), 0, bootnodes.clone(), ChangeVote::Yes, at(1), rules(4), &Weighted).unwrap();
        changes.vote(&member(2), 0, staked(), ChangeVote::Yes, at(1), rules(4), &Weighted).unwrap();

        assert_eq!(changes.ballot(ChangeKind::Bootnodes).unwrap().approvers(&bootnodes), [member(2)]);
        assert_eq!(changes.ballot(ChangeKind::Consensus).unwrap().approvers(&staked()), [member(2)]);
        assert_eq!(changes.ballots().unwrap().iter().map(|(kind, _)| *kind).collect::<Vec<_>>(), [ChangeKind::Consensus, ChangeKind::Bootnodes]);
        assert_eq!(
            changes.vote(&member(1), 0, bootnodes.clone(), ChangeVote::Yes, at(1), rules(4), &Weighted),
            Ok(ChangeVoted::Decided { decision: 1, kind: ChangeKind::Bootnodes, version: 1 })
        );
        assert_eq!(changes.bootnodes().unwrap(), Some(vec!["/ip4/203.0.113.7/udp/4001/quic-v1".to_string()]));
        assert_eq!(changes.decided().unwrap(), DecidedChange { version: 1, consensus: pow() }, "the consensus stays, the count of decisions moves");
        assert_eq!((changes.version_of(ChangeKind::Bootnodes).unwrap(), changes.version_of(ChangeKind::Consensus).unwrap()), (1, 0));
        assert!(changes.ballot(ChangeKind::Bootnodes).unwrap().votes.is_empty(), "its own ballot empties");
        assert_eq!(changes.ballot(ChangeKind::Consensus).unwrap().approvers(&staked()), [member(2)], "the other kind's approvals stand");

        // A stale version is the kind's own; the other kind's stays at 0.
        assert_eq!(changes.vote(&member(1), 0, Change::Bootnodes(Vec::new()), ChangeVote::Yes, at(1), rules(4), &Weighted), Err(RegistryError::StaleChange(1)));
        assert_eq!(changes.vote(&member(1), 1, bootnodes, ChangeVote::Yes, at(1), rules(4), &Weighted), Err(RegistryError::SameChange));
        assert_eq!(
            changes.vote(&member(1), 0, staked(), ChangeVote::Yes, at(1), rules(4), &Weighted),
            Ok(ChangeVoted::Decided { decision: 2, kind: ChangeKind::Consensus, version: 1 }),
            "the consensus decided second, as the chain's second decision and its kind's first"
        );

        // An empty list is a change too: the chain has no bootnodes from then on.
        changes.vote(&member(1), 1, Change::Bootnodes(Vec::new()), ChangeVote::Yes, at(1), rules(4), &Weighted).unwrap();
        changes.vote(&member(2), 1, Change::Bootnodes(Vec::new()), ChangeVote::Yes, at(1), rules(4), &Weighted).unwrap();

        assert_eq!(changes.bootnodes().unwrap(), Some(Vec::new()));
        assert_eq!((changes.decided().unwrap().version, changes.version_of(ChangeKind::Bootnodes).unwrap()), (3, 2));
    }

    #[test]
    fn finality_and_its_switch_share_the_consensus_s_ballot() {
        assert_eq!(staked().kind(), ChangeKind::Consensus);
        assert_eq!("finality".parse::<ChangeKind>(), Ok(ChangeKind::Consensus));
        assert_eq!("switch_finality".parse::<ChangeKind>(), Ok(ChangeKind::Consensus));
        assert_eq!("fees".parse::<ChangeKind>(), Ok(ChangeKind::Fees));
        assert!("colour".parse::<ChangeKind>().is_err());
        assert!(ChangeKind::ALL.iter().all(|kind| kind.name().parse::<ChangeKind>() == Ok(*kind)));
    }

    #[test]
    fn approvals_expire_move_and_withdraw() {
        let state = State::new(Arc::new(MemoryDb::default()));
        let changes = Changes::new(&state);
        changes.write_genesis(&pow(), 2).unwrap();
        let other = Change::Consensus(ConsensusConfig::Rotating { slot_ms: 1_000 });

        changes.vote(&member(2), 0, staked(), ChangeVote::Yes, at(1), rules(2), &Weighted).unwrap();
        changes.vote(&member(3), 0, staked(), ChangeVote::Yes, at(1), rules(2), &Weighted).unwrap();

        // Moving an approval takes it from the other change.
        changes.vote(&member(3), 0, other.clone(), ChangeVote::Yes, at(1), rules(2), &Weighted).unwrap();

        assert_eq!(changes.ballot(ChangeKind::Consensus).unwrap().approvers(&staked()), [member(2)]);
        assert_eq!(changes.ballot(ChangeKind::Consensus).unwrap().approvers(&other), [member(3)]);

        // Withdrawing needs an approval that still counts.
        assert_eq!(changes.clear(&member(3), 0, &other, at(1), 2, &Weighted), Ok(()));
        assert_eq!(changes.clear(&member(3), 0, &other, at(1), 2, &Weighted), Err(RegistryError::NoVote));
        assert_eq!(changes.clear(&member(9), 0, &other, at(1), 2, &Weighted), Err(RegistryError::NotGoverning));

        // An approval from epoch 1 counts through epoch 2 and not in 3: the weight-two account
        // alone does not decide then.
        assert_eq!(changes.ballot(ChangeKind::Consensus).unwrap().counting(2, 2).approvers(&staked()), [member(2)]);
        assert!(changes.ballot(ChangeKind::Consensus).unwrap().counting(3, 2).approvers(&staked()).is_empty());
        assert_eq!(
            changes.vote(&member(1), 0, staked(), ChangeVote::Yes, at(3), rules(2), &Weighted),
            counted(u256::from(2u64))
        );
    }

    /// The votes on a change as they stand in an epoch.
    fn tallied(changes: &Changes, change: &Change, epoch: u64, rules: BallotRules) -> Tally {
        let ballot = changes.ballot(change.kind()).unwrap().counting(epoch, rules.approval_epochs);

        changes.tally(&ballot, change, epoch, rules.quorum_permille, &Weighted).unwrap()
    }

    #[test]
    fn yes_no_and_abstain_weigh_apart_and_a_change_that_can_no_longer_pass_is_rejected() {
        let state = State::new(Arc::new(MemoryDb::default()));
        let changes = Changes::new(&state);
        changes.write_genesis(&pow(), 2).unwrap();
        let rotating = Change::Consensus(ConsensusConfig::Rotating { slot_ms: 1_000 });

        // One No of the four leaves three that could still say Yes, as many as it needs.
        assert_eq!(
            changes.vote(&member(2), 0, staked(), ChangeVote::No, at(1), rules(4), &Weighted),
            Ok(ChangeVoted::Counted(Tally { no: u256::ONE, total: u256::from(4u64), needed: u256::from(3u64), ..Tally::default() }))
        );
        changes.vote(&member(3), 0, staked(), ChangeVote::Abstain, at(1), rules(4), &Weighted).unwrap();
        changes.vote(&member(3), 0, rotating.clone(), ChangeVote::Yes, at(1), rules(4), &Weighted).unwrap();

        let tally = tallied(&changes, &staked(), 1, rules(4));
        assert_eq!((tally.yes, tally.no, tally.abstain, tally.voted()), (u256::ZERO, u256::ONE, u256::ONE, u256::from(2u64)));
        assert!(tally.can_pass() && !tally.decides());
        assert_eq!(tally.more_needed(), u256::from(3u64));

        // The weight-two No leaves one that has not said No: it can no longer reach three.
        assert_eq!(changes.vote(&member(1), 0, staked(), ChangeVote::No, at(1), rules(4), &Weighted), Ok(ChangeVoted::Rejected { rejection: 1, kind: ChangeKind::Consensus, version: 0 }));
        assert_eq!(changes.rejections().unwrap(), 1);

        let ballot = changes.ballot(ChangeKind::Consensus).unwrap();
        assert_eq!(ballot.rejected, [staked()]);
        assert!(ballot.votes_on(&staked()).next().is_none(), "its votes go with it");
        assert_eq!(ballot.approvers(&rotating), [member(3)], "another change of the kind keeps its votes");
        assert_eq!(changes.version_of(ChangeKind::Consensus).unwrap(), 0, "nothing it names changed");

        // It stays rejected at this version, whoever votes on it and however.
        assert_eq!(changes.vote(&member(1), 0, staked(), ChangeVote::Yes, at(1), rules(4), &Weighted), Err(RegistryError::RejectedChange));

        // Once a change of the kind is decided, it may come back.
        changes.vote(&member(1), 0, rotating, ChangeVote::Yes, at(1), rules(4), &Weighted).unwrap();
        assert_eq!(changes.vote(&member(2), 1, staked(), ChangeVote::Yes, at(1), rules(4), &Weighted), counted(u256::ONE));
    }

    #[test]
    fn a_quorum_holds_yes_back_until_enough_have_voted_abstain_and_no_included() {
        let state = State::new(Arc::new(MemoryDb::default()));
        let changes = Changes::new(&state);
        changes.write_genesis(&pow(), 2).unwrap();
        let everyone = BallotRules { approval_epochs: 4, quorum_permille: 1000 };

        // Yes weighs three of the three needed, but only three of the four have voted.
        changes.vote(&member(1), 0, staked(), ChangeVote::Yes, at(1), everyone, &Weighted).unwrap();
        assert_eq!(
            changes.vote(&member(2), 0, staked(), ChangeVote::Yes, at(1), everyone, &Weighted),
            Ok(ChangeVoted::Counted(Tally { yes: u256::from(3u64), total: u256::from(4u64), needed: u256::from(3u64), quorum: u256::from(4u64), ..Tally::default() }))
        );

        // An abstention makes the quorum, and so decides it.
        assert_eq!(changes.vote(&member(3), 0, staked(), ChangeVote::Abstain, at(1), everyone, &Weighted), Ok(ChangeVoted::Decided { decision: 1, kind: ChangeKind::Consensus, version: 1 }));

        // A quorum's share rounds up: a tenth of four is one vote's weight.
        let tenth = BallotRules { approval_epochs: 4, quorum_permille: 100 };
        changes.vote(&member(2), 1, Change::Consensus(pow()), ChangeVote::Abstain, at(1), tenth, &Weighted).unwrap();
        assert_eq!(tallied(&changes, &Change::Consensus(pow()), 1, tenth).quorum, u256::ONE);
    }

    #[test]
    fn a_new_vote_replaces_the_old_and_clearing_takes_it_back() {
        let state = State::new(Arc::new(MemoryDb::default()));
        let changes = Changes::new(&state);
        changes.write_genesis(&pow(), 2).unwrap();
        let side = |changes: &Changes| {
            let tally = tallied(changes, &staked(), 1, rules(4));
            (tally.yes, tally.no, tally.abstain)
        };

        changes.vote(&member(2), 0, staked(), ChangeVote::Yes, at(1), rules(4), &Weighted).unwrap();
        assert_eq!(side(&changes), (u256::ONE, u256::ZERO, u256::ZERO));
        changes.vote(&member(2), 0, staked(), ChangeVote::No, at(1), rules(4), &Weighted).unwrap();
        assert_eq!(side(&changes), (u256::ZERO, u256::ONE, u256::ZERO));
        changes.vote(&member(2), 0, staked(), ChangeVote::Abstain, at(1), rules(4), &Weighted).unwrap();
        assert_eq!(side(&changes), (u256::ZERO, u256::ZERO, u256::ONE));
        assert_eq!(changes.ballot(ChangeKind::Consensus).unwrap().votes.len(), 1, "one vote an account on a change");

        // No on one change and Yes on another stand together; only a second Yes moves.
        let rotating = Change::Consensus(ConsensusConfig::Rotating { slot_ms: 1_000 });
        changes.vote(&member(2), 0, rotating.clone(), ChangeVote::Yes, at(1), rules(4), &Weighted).unwrap();
        assert_eq!(side(&changes), (u256::ZERO, u256::ZERO, u256::ONE));

        assert_eq!(changes.clear(&member(2), 0, &staked(), at(1), 4, &Weighted), Ok(()));
        assert_eq!(side(&changes), (u256::ZERO, u256::ZERO, u256::ZERO));
        assert_eq!(changes.ballot(ChangeKind::Consensus).unwrap().approvers(&rotating), [member(2)], "clearing one leaves the other");
        assert_eq!(changes.clear(&member(2), 0, &staked(), at(1), 4, &Weighted), Err(RegistryError::NoVote));
        assert_eq!(changes.clear(&member(2), 1, &rotating, at(1), 4, &Weighted), Err(RegistryError::StaleChange(0)));
    }

    #[test]
    fn a_no_stops_counting_when_its_epochs_run_out() {
        let state = State::new(Arc::new(MemoryDb::default()));
        let changes = Changes::new(&state);
        changes.write_genesis(&pow(), 2).unwrap();

        changes.vote(&member(2), 0, staked(), ChangeVote::No, at(1), rules(2), &Weighted).unwrap();
        assert_eq!(tallied(&changes, &staked(), 2, rules(2)).no, u256::ONE);
        assert_eq!(tallied(&changes, &staked(), 3, rules(2)).no, u256::ZERO);

        // So in epoch 3 the weight-two No alone leaves two that could say Yes: rejected; in epoch
        // 2 the expired one would have made it so as well.
        assert_eq!(changes.vote(&member(1), 0, staked(), ChangeVote::No, at(3), rules(2), &Weighted), Ok(ChangeVoted::Rejected { rejection: 1, kind: ChangeKind::Consensus, version: 0 }));
    }

    #[cfg(feature = "governance-council")]
    #[test]
    fn a_governance_decision_clears_every_open_vote_and_counts_from_the_next_block() {
        let state = State::new(Arc::new(MemoryDb::default()));
        let changes = Changes::new(&state);
        changes.write_genesis(&pow(), 2).unwrap();
        let bootnodes = Change::Bootnodes(vec!["/ip4/203.0.113.7/udp/4001/quic-v1".to_string()]);
        let council = crate::governance::GovernanceConfig::Council { members: vec![member(1), member(4)], threshold: 2, approval_epochs: 8, quorum_permille: 0 };
        let change = Change::Governance(council.clone());

        changes.vote(&member(2), 0, bootnodes.clone(), ChangeVote::Yes, at(1), rules(4), &Weighted).unwrap();
        changes.vote(&member(2), 0, staked(), ChangeVote::No, at(1), rules(4), &Weighted).unwrap();
        changes.vote(&member(1), 0, change.clone(), ChangeVote::Yes, at(1), rules(4), &Weighted).unwrap();
        assert_eq!(changes.vote(&member(3), 0, change, ChangeVote::Yes, at(1), rules(4), &Weighted), Ok(ChangeVoted::Decided { decision: 1, kind: ChangeKind::Governance, version: 1 }));

        assert_eq!(changes.governance().unwrap(), Some(DecidedGovernance { config: council, from: 11 }));
        assert!(changes.ballots().unwrap().is_empty(), "every kind's votes were cast under the old settings");
        assert_eq!(changes.version_of(ChangeKind::Bootnodes).unwrap(), 0, "other kinds keep their versions");

        // The rest of the deciding block counts by nobody; the next by the new settings.
        assert_eq!(
            changes.vote(&member(2), 0, bootnodes.clone(), ChangeVote::Yes, at(1), rules(4), &Weighted),
            Err(RegistryError::BadChange("governance changed itself in this block; vote from the next"))
        );
        let next = DecidedAt { height: 11, epoch: 1, activation_delay: 2, time: 11_000 };
        assert_eq!(changes.vote(&member(2), 0, bootnodes, ChangeVote::Yes, next, rules(4), &Weighted), counted(u256::ONE));
    }
}
