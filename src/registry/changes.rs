//! Governance's records, in chain state: the consensus decided last, the bootnodes and name prices
//! decided last, and the ballot for changing any of them, in Column::Validators beside the signers' records, and the
//! consensus fixed for each epoch, in Column::Epochs beside each epoch's voters, which is what a
//! light client asks a proof of.

#[cfg(feature = "governance")]
use {super::RegistryError, crate::governance::Governance};
use {
    crate::consensus::ConsensusConfig,
    crate::crypto::hashable::Hashable,
    crate::db::{Column, DbError, State},
    crate::governance::Change,
    crate::structs::address::Address,
    crate::structs::u256::u256,
    r#macro::document,
    wincode::{SchemaRead, SchemaWrite},
};

const DECIDED: &[u8] = b"change:decided";
const BALLOT: &[u8] = b"change:ballot";
const BOOTNODES: &[u8] = b"change:bootnodes";
const NAME_PRICES: &[u8] = b"change:name-prices";
const COLLECTIONS: &[u8] = b"change:collections";
const FEDERATION: &[u8] = crate::registry::federation::FEDERATION_KEY;
const DIRECTORY: &[u8] = crate::registry::directory::DIRECTORY_KEY;
const FEES: &[u8] = b"change:fees";
const REWARD: &[u8] = b"change:reward";
const LIMITS: &[u8] = b"change:limits";
const FINALITY: &[u8] = b"change:finality";
const UPGRADE: &[u8] = b"change:upgrade";
const NAME: &[u8] = b"change:name";

/// The chain's name in force, as a state value.
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
struct ChainName {
    name: String,
}

impl Hashable for ChainName {}

#[document(
    summary = "Where an approval is made: its block's height and epoch, and how many epochs ahead finality fixes what is decided, which Change::Finality takes effect after.",
    category = "Governance",
    see_also = "Changes::approve",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DecidedAt {
    #[document(summary = "The block's height.")]
    pub height: u64,
    #[document(summary = "Its epoch.")]
    pub epoch: u64,
    #[document(summary = "Finality's activation_delay.")]
    pub activation_delay: u64,
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
    summary = "The name prices governance decided last, which replace genesis's.",
    category = "Governance",
    see_also = "Changes::name_prices, Change::NamePrices",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct DecidedNamePrices {
    #[document(summary = "What a year costs by a name's length, as NamesConfig::prices lists them.")]
    #[wincode(with = "crate::structs::u256::U256LeList")]
    pub prices: Vec<u256>,
}

impl Hashable for DecidedNamePrices {}

#[document(
    summary = "One account's approval of a change.",
    category = "Governance",
    see_also = "ChangeBallot",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct ChangeApproval {
    #[document(summary = "The approving account.")]
    pub account: Address,
    #[document(summary = "The change it approves.")]
    pub change: Change,
    #[document(summary = "The epoch it was made in. It counts for approval_epochs epochs from there, and approving again starts it afresh.")]
    pub epoch: u64,
}

impl ChangeApproval {
    #[document(parent = "ChangeApproval", summary = "Whether it still counts in an epoch, when approvals count for approval_epochs epochs.")]
    pub fn counts(&self, epoch: u64, approval_epochs: u64) -> bool {
        epoch < self.epoch.saturating_add(approval_epochs)
    }
}

#[document(
    summary = "The approvals for changing what was decided last, at most one per account. An account that approves another change moves its approval there, and one that withdraws takes it back. An approval stops counting approval_epochs epochs after the one it was made in, and what it weighs is counted afresh whenever the ballot is: an account that lost its standing since weighs nothing.",
    category = "Governance",
    see_also = "Changes::approve",
)]
#[derive(Clone, Debug, Default, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct ChangeBallot {
    #[document(summary = "The version the approvals change.")]
    pub version: u64,
    #[document(summary = "The approvals, in the order they came.")]
    pub approvals: Vec<ChangeApproval>,
}

impl Hashable for ChangeBallot {}

impl ChangeBallot {
    #[document(
        parent = "ChangeBallot",
        summary = "The ballot as it stands in an epoch: only the approvals that still count.",
        params = {
            epoch = "The epoch to count in.",
            approval_epochs = "How many epochs an approval counts for.",
        },
    )]
    pub fn counting(mut self, epoch: u64, approval_epochs: u64) -> ChangeBallot {
        self.approvals.retain(|approval| approval.counts(epoch, approval_epochs));
        self
    }

    #[document(parent = "ChangeBallot", summary = "Who approved a change.")]
    pub fn approvers(&self, change: &Change) -> Vec<Address> {
        self.approvals.iter().filter(|approval| approval.change == *change).map(|approval| approval.account).collect()
    }

    #[document(parent = "ChangeBallot", summary = "The changes approved, each once, in the order their first approval came.")]
    pub fn changes(&self) -> Vec<Change> {
        let mut changes: Vec<Change> = Vec::new();

        for approval in &self.approvals {
            if !changes.contains(&approval.change) {
                changes.push(approval.change.clone());
            }
        }

        changes
    }
}

#[document(
    summary = "What an approval did.",
    category = "Governance",
    see_also = "Changes::approve",
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChangeApproved {
    #[document(summary = "It counted, and the change's approvals now weigh this much of what they need.")]
    Counted { weight: u256, needed: u256 },
    #[document(summary = "It was the last one needed: the change is decided, as this version.")]
    DecidedChange(u64),
}

#[document(
    summary = "Governance's records in chain state: the consensus fixed for each epoch, the one decided last, and the ballot for changing it. A decided change is fixed for epochs activation_delay on at the checkpoints that follow, like finality's voters, so every branch that split off after the checkpoint that fixed an epoch's consensus checks that epoch's blocks alike.",
    category = "Governance",
    see_also = "Governance, Change, Tx::ApproveChange",
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
        summary = "The name prices decided last, or None while governance never decided any, when genesis's stand.",
    )]
    pub fn name_prices(&self) -> Result<Option<Vec<u256>>, DbError> {
        Ok(self.state.get_value::<DecidedNamePrices>(Column::Validators, NAME_PRICES)?.map(|decided| decided.prices))
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

    #[document(parent = "Changes", summary = "Writes genesis's federation as the one in force, version 0, as the chain starts.")]
    pub fn write_genesis_federation(&self, federation: &crate::registry::federation::Federation) {
        self.state.put_value(Column::Validators, FEDERATION.to_vec(), &crate::registry::federation::DecidedFederation { version: 0, federation: federation.clone() });
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

    #[document(
        parent = "Changes",
        summary = "The approvals so far for changing what was decided last, expired ones included: ChangeBallot::counting leaves only those that count.",
    )]
    pub fn ballot(&self) -> Result<ChangeBallot, DbError> {
        let decided = self.decided()?;
        let ballot: ChangeBallot = self.state.get_value(Column::Validators, BALLOT)?.unwrap_or_default();

        // Approvals for an older version never count.
        Ok(match ballot.version == decided.version {
            true => ballot,
            false => ChangeBallot {
                version: decided.version,
                approvals: Vec::new(),
            },
        })
    }

    #[cfg(feature = "governance")]
    #[document(
        parent = "Changes",
        summary = "What a change's approvals weigh now: each approver's weight as governance counts it in this state, which an approver that lost its standing since no longer adds to.",
    )]
    pub fn weight_of(&self, ballot: &ChangeBallot, change: &Change, epoch: u64, governance: &dyn Governance) -> Result<u256, DbError> {
        let mut weight = u256::ZERO;

        for account in ballot.approvers(change) {
            weight = weight.saturating_add(governance.weight(self.state, epoch, &account)?.unwrap_or(u256::ZERO));
        }

        Ok(weight)
    }

    #[cfg(feature = "governance")]
    #[document(
        parent = "Changes",
        summary = "Counts an account's approval of a change. Once the approvals that still count for the same change weigh what governance needs, it is decided, and the checkpoints that follow fix it for later epochs.",
        params = {
            sender = "Who sent the approval: an account governance lets approve.",
            version = "The version it changes, which must be the decided one's.",
            change = "The change it approves, checked to fit the chain already.",
            at = "Where the approval is made: the block, its epoch and finality's activation_delay.",
            approval_epochs = "How many epochs an approval counts for.",
            governance = "Who may approve, and what it takes.",
        },
        returns = "Whether that decided it. NotGoverning for a sender that may not approve, StaleChange for an older version, SameChange for the consensus, bootnodes, name prices, collection standards, federation or directory already decided.",
    )]
    pub fn approve(&self, sender: &Address, version: u64, change: Change, at: DecidedAt, approval_epochs: u64, governance: &dyn Governance) -> Result<ChangeApproved, RegistryError> {
        let epoch = at.epoch;
        let decided = self.standing(sender, version, epoch, governance)?;
        let same = match &change {
            Change::Consensus(consensus) => *consensus == decided.consensus,
            Change::Bootnodes(bootnodes) => self.bootnodes()?.as_ref() == Some(bootnodes),
            Change::NamePrices(prices) => self.name_prices()?.as_ref() == Some(prices),
            Change::Collections(functions) => self.collections()?.as_ref() == Some(functions),
            Change::Federation(federation) => self.federation()?.map(|decided| decided.federation).as_ref() == Some(federation),
            Change::Directory(directory) => self.directory()?.as_ref() == Some(directory),
            Change::Fees(fees) => self.fees()?.as_ref() == Some(fees),
            Change::Reward(reward) => self.reward()?.map(|decided| decided.reward).as_ref() == Some(reward),
            Change::Limits(limits) => self.limits()?.as_ref() == Some(limits),
            Change::Finality(config) | Change::SwitchFinality { config, .. } => self.finality_eras()?.eras.last().map(|era| &era.config) == Some(config),
            Change::Upgrade(upgrade) => self.upgrade()?.as_ref() == Some(upgrade),
            Change::Name(name) => self.name()?.as_ref() == Some(name),
        };

        if same {
            return Err(RegistryError::SameChange);
        }

        let mut ballot = self.ballot()?.counting(epoch, approval_epochs);
        ballot.approvals.retain(|approval| approval.account != *sender);
        ballot.approvals.push(ChangeApproval {
            account: *sender,
            change: change.clone(),
            epoch,
        });
        let weight = self.weight_of(&ballot, &change, epoch, governance)?;
        let needed = governance.needed(self.state, epoch)?;

        if weight < needed {
            self.state.put_value(Column::Validators, BALLOT.to_vec(), &ballot);

            return Ok(ChangeApproved::Counted { weight, needed });
        }

        let version = decided.version + 1;
        let consensus = match change {
            Change::Consensus(consensus) => consensus,
            // Bootnodes take effect at once: nothing checks blocks by them.
            Change::Bootnodes(bootnodes) => {
                self.state.put_value(Column::Validators, BOOTNODES.to_vec(), &DecidedBootnodes { bootnodes });

                decided.consensus
            }
            // So do name prices: what a name costs is read when one is registered or renewed.
            Change::NamePrices(prices) => {
                self.state.put_value(Column::Validators, NAME_PRICES.to_vec(), &DecidedNamePrices { prices });

                decided.consensus
            }
            // And what makes a collection: deploys read it as they run.
            Change::Collections(functions) => {
                self.state.put_value(Column::Validators, COLLECTIONS.to_vec(), &functions);

                decided.consensus
            }
            // And the federation: contracts read it as they run, and its version counts the ones
            // decided, after genesis's 0.
            Change::Federation(federation) => {
                let version = self.federation()?.map_or(1, |decided| decided.version + 1);
                self.state.put_value(Column::Validators, FEDERATION.to_vec(), &crate::registry::federation::DecidedFederation { version, federation });

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
        };
        self.state.put_value(Column::Validators, DECIDED.to_vec(), &DecidedChange { version, consensus });
        self.state.delete(Column::Validators, BALLOT.to_vec());

        Ok(ChangeApproved::DecidedChange(version))
    }

    #[cfg(feature = "governance")]
    #[document(
        parent = "Changes",
        summary = "Takes back an account's approval, while it still counts, so it backs no change.",
        params = {
            sender = "The account.",
            version = "The version its approval was for, which must be the decided one's.",
            epoch = "The epoch it is withdrawn in.",
            approval_epochs = "How many epochs an approval counts for.",
            governance = "Who may approve.",
        },
        returns = "NotGoverning for a sender that may not approve, StaleChange for an older version, NoApproval when it has no approval that still counts.",
    )]
    pub fn withdraw(&self, sender: &Address, version: u64, epoch: u64, approval_epochs: u64, governance: &dyn Governance) -> Result<(), RegistryError> {
        self.standing(sender, version, epoch, governance)?;
        let mut ballot = self.ballot()?.counting(epoch, approval_epochs);
        let before = ballot.approvals.len();
        ballot.approvals.retain(|approval| approval.account != *sender);

        if ballot.approvals.len() == before {
            return Err(RegistryError::NoApproval);
        }

        match ballot.approvals.is_empty() {
            true => self.state.delete(Column::Validators, BALLOT.to_vec()),
            false => self.state.put_value(Column::Validators, BALLOT.to_vec(), &ballot),
        }

        Ok(())
    }

    #[cfg(feature = "governance")]
    /// The decision a sender with standing changes, at a version that is the decided one's.
    fn standing(&self, sender: &Address, version: u64, epoch: u64, governance: &dyn Governance) -> Result<DecidedChange, RegistryError> {
        if governance.weight(self.state, epoch, sender)?.is_none() {
            return Err(RegistryError::NotGoverning);
        }

        let decided = self.decided()?;

        if version != decided.version {
            return Err(RegistryError::StaleChange(decided.version));
        }

        Ok(decided)
    }
}

#[cfg(all(test, feature = "governance"))]
mod tests {
    use super::*;

    /// An approval made at an epoch, finality fixing what is decided two epochs ahead.
    fn at(epoch: u64) -> DecidedAt {
        DecidedAt { height: epoch * 10, epoch, activation_delay: 2 }
    }
    use crate::db::MemoryDb;
    use crate::structs::address::Address;
    use std::sync::Arc;

    /// Three accounts, the first weighing two, needing three.
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
    }

    fn pow() -> ConsensusConfig {
        ConsensusConfig::ProofOfWork(crate::consensus::PowConfig {
            hash: crate::structs::hash::HashType::default_chain(),
            target_block_ms: 1_000,
            initial_difficulty: 1,
            min_difficulty: 1,
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
        assert_eq!(changes.approve(&member(9), 0, staked(), at(1), 4, &Weighted), Err(RegistryError::NotGoverning));
        assert_eq!(changes.approve(&member(2), 0, Change::Consensus(pow()), at(1), 4, &Weighted), Err(RegistryError::SameChange));
        assert_eq!(changes.approve(&member(2), 1, staked(), at(1), 4, &Weighted), Err(RegistryError::StaleChange(0)));

        // One of weight one, then the one of weight two decide it: three of three.
        assert_eq!(
            changes.approve(&member(2), 0, staked(), at(1), 4, &Weighted),
            Ok(ChangeApproved::Counted { weight: u256::ONE, needed: u256::from(3u64) })
        );
        assert_eq!(changes.ballot().unwrap().approvers(&staked()), [member(2)]);
        assert_eq!(changes.ballot().unwrap().changes(), [staked()]);
        assert_eq!(changes.approve(&member(1), 0, staked(), at(2), 4, &Weighted), Ok(ChangeApproved::DecidedChange(1)));

        let Change::Consensus(consensus) = staked() else { unreachable!() };

        assert_eq!(changes.decided().unwrap(), DecidedChange { version: 1, consensus: consensus.clone() });
        assert!(changes.ballot().unwrap().approvals.is_empty());

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

        assert_eq!(changes.approve(&member(1), 0, Change::Name("chainv4 testnet".to_string()), at(1), 4, &Weighted), Err(RegistryError::SameChange));

        changes.approve(&member(1), 0, renamed.clone(), at(1), 4, &Weighted).unwrap();
        assert_eq!(changes.name().unwrap().as_deref(), Some("chainv4 testnet"), "genesis's stands until decided");
        assert_eq!(changes.approve(&member(2), 0, renamed, at(1), 4, &Weighted), Ok(ChangeApproved::DecidedChange(1)));
        assert_eq!(changes.name().unwrap().as_deref(), Some("chainv4 mainnet"));
        assert_eq!(changes.decided().unwrap().consensus, pow(), "the consensus stays");
    }

    #[test]
    fn bootnodes_are_decided_on_the_same_ballot_and_leave_the_consensus_be() {
        let state = State::new(Arc::new(MemoryDb::default()));
        let changes = Changes::new(&state);
        changes.write_genesis(&pow(), 2).unwrap();
        let bootnodes = Change::Bootnodes(vec!["/ip4/203.0.113.7/udp/4001/quic-v1".to_string()]);

        assert_eq!(changes.bootnodes().unwrap(), None, "genesis's stand until governance decides");

        changes.approve(&member(2), 0, bootnodes.clone(), at(1), 4, &Weighted).unwrap();
        // Bootnodes and a consensus on one ballot: an account backs one change at a time.
        changes.approve(&member(3), 0, staked(), at(1), 4, &Weighted).unwrap();

        assert_eq!(changes.ballot().unwrap().changes(), [bootnodes.clone(), staked()]);
        assert_eq!(changes.approve(&member(1), 0, bootnodes.clone(), at(1), 4, &Weighted), Ok(ChangeApproved::DecidedChange(1)));
        assert_eq!(changes.bootnodes().unwrap(), Some(vec!["/ip4/203.0.113.7/udp/4001/quic-v1".to_string()]));
        assert_eq!(changes.decided().unwrap(), DecidedChange { version: 1, consensus: pow() }, "the consensus stays, the version moves");
        assert!(changes.ballot().unwrap().approvals.is_empty(), "the other change's approvals were for version 0");
        assert_eq!(changes.approve(&member(1), 1, bootnodes, at(1), 4, &Weighted), Err(RegistryError::SameChange));

        // An empty list is a change too: the chain has no bootnodes from then on.
        changes.approve(&member(1), 1, Change::Bootnodes(Vec::new()), at(1), 4, &Weighted).unwrap();
        changes.approve(&member(2), 1, Change::Bootnodes(Vec::new()), at(1), 4, &Weighted).unwrap();

        assert_eq!(changes.bootnodes().unwrap(), Some(Vec::new()));
        assert_eq!(changes.decided().unwrap().version, 2);
    }

    #[test]
    fn approvals_expire_move_and_withdraw() {
        let state = State::new(Arc::new(MemoryDb::default()));
        let changes = Changes::new(&state);
        changes.write_genesis(&pow(), 2).unwrap();
        let other = Change::Consensus(ConsensusConfig::Rotating { slot_ms: 1_000 });

        changes.approve(&member(2), 0, staked(), at(1), 2, &Weighted).unwrap();
        changes.approve(&member(3), 0, staked(), at(1), 2, &Weighted).unwrap();

        // Moving an approval takes it from the other change.
        changes.approve(&member(3), 0, other.clone(), at(1), 2, &Weighted).unwrap();

        assert_eq!(changes.ballot().unwrap().approvers(&staked()), [member(2)]);
        assert_eq!(changes.ballot().unwrap().approvers(&other), [member(3)]);

        // Withdrawing needs an approval that still counts.
        assert_eq!(changes.withdraw(&member(3), 0, 1, 2, &Weighted), Ok(()));
        assert_eq!(changes.withdraw(&member(3), 0, 1, 2, &Weighted), Err(RegistryError::NoApproval));
        assert_eq!(changes.withdraw(&member(9), 0, 1, 2, &Weighted), Err(RegistryError::NotGoverning));

        // An approval from epoch 1 counts through epoch 2 and not in 3: the weight-two account
        // alone does not decide then.
        assert_eq!(changes.ballot().unwrap().counting(2, 2).approvers(&staked()), [member(2)]);
        assert!(changes.ballot().unwrap().counting(3, 2).approvers(&staked()).is_empty());
        assert_eq!(
            changes.approve(&member(1), 0, staked(), at(3), 2, &Weighted),
            Ok(ChangeApproved::Counted { weight: u256::from(2u64), needed: u256::from(3u64) })
        );
    }
}
