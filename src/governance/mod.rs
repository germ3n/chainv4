//! Governance: how a running chain changes what genesis set, without a hard fork. Genesis names a
//! kind of governance, or none, and who it lets approve a change: under `electorate` the chain's
//! own finality electorate, the authority signers or the FFG validators by stake; under `council`
//! a list of accounts genesis names. A change is proposed and approved by transaction
//! (`Tx::ApproveChange`), decided once the approvals weigh enough, and fixed in state for the
//! epoch `activation_delay` ahead at the next checkpoint, as finality fixes its voters, so every
//! branch and every light client learns it the same way, from the chain and its light updates.
//!
//! What can change today is the consensus, who produces blocks and how, and the bootnodes, the
//! addresses new nodes dial first. The finality kind, and the parameters that define epochs, stay
//! what genesis says: the electorate that approves is finality's own, and every light client's
//! epoch arithmetic rests on them.

// A build without any kind has an empty GovernanceConfig, whose methods all match on nothing.
#![cfg_attr(not(any(feature = "governance-electorate", feature = "governance-council")), allow(unreachable_code, unused_variables))]

use {
    crate::consensus::ConsensusConfig,
    crate::crypto::hashable::Hashable,
    crate::db::{DbError, State},
    crate::structs::address::Address,
    crate::structs::u256::u256,
    r#macro::document,
    wincode::{SchemaRead, SchemaWrite},
};

#[cfg(all(feature = "governance-council", feature = "node"))]
mod council;
#[cfg(all(feature = "governance-electorate", feature = "node"))]
mod electorate;

#[document(
    summary = "A kind of governance, as the chain uses it to count approvals: who may approve a change, what each approval weighs, and what the approvals for one change must add up to. GovernanceConfig::build makes one from genesis.",
    category = "Governance",
    see_also = "GovernanceConfig, Changes, Tx::ApproveChange",
)]
pub trait Governance: Send + Sync {
    #[document(parent = "Governance", summary = "The kind's name, as genesis files and the API spell it.")]
    fn name(&self) -> &'static str;

    #[document(
        parent = "Governance",
        summary = "What an account's approval weighs in a state, or None when the account may not approve: under the electorate, the stake its validators hold, or how many signers it speaks for; under a council, 1 for a member.",
        params = {
            state = "The state the approval is counted in.",
            epoch = "The epoch it is counted in.",
            account = "The approving account.",
        },
    )]
    fn weight(&self, state: &State, epoch: u64, account: &Address) -> Result<Option<u256>, DbError>;

    #[document(
        parent = "Governance",
        summary = "What the approvals for one change must add up to in a state to decide it: two thirds of the stake, the signers' change_threshold, or the council's threshold.",
    )]
    fn needed(&self, state: &State, epoch: u64) -> Result<u256, DbError>;

    #[document(
        parent = "Governance",
        summary = "The accounts that may approve, when the kind names them: a council's members. None for the electorate, whose voters are finality's and change with it.",
    )]
    fn members(&self) -> Option<&[Address]> {
        None
    }
}

#[document(
    summary = "Which governance a chain runs, and its parameters. Part of genesis, so every node counts approvals alike; None in genesis means nothing changes. Needs finality that votes, whose light updates carry decisions to light clients.",
    category = "Governance",
    see_also = "Genesis::governance, Governance, Change",
    note = "Each kind sits behind its own cargo feature, governance-electorate and governance-council, both on by default. Tags are pinned, so encodings match across builds.",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
#[wincode(tag_encoding = "u8")]
pub enum GovernanceConfig {
    #[document(
        summary = "Finality's own electorate approves: under FFG the validators, each account weighing the stake its validators hold in the epoch's set, two thirds deciding; under authority finality the signers, each account weighing the signers it speaks for, change_threshold of the decided set deciding.",
        params = { approval_epochs = "How many epochs an approval counts for, from the one it was made in." },
    )]
    #[cfg(feature = "governance-electorate")]
    #[wincode(tag = 0)]
    Electorate { approval_epochs: u64 },
    #[document(
        summary = "Accounts genesis names approve, each weighing one.",
        params = {
            members = "The council's accounts, each named once.",
            threshold = "How many of them decide a change: at least one, at most all.",
            approval_epochs = "How many epochs an approval counts for, from the one it was made in.",
        },
    )]
    #[cfg(feature = "governance-council")]
    #[wincode(tag = 1)]
    Council {
        members: Vec<Address>,
        threshold: u32,
        approval_epochs: u64,
    },
}

impl Hashable for GovernanceConfig {
    const TAG_LEN: usize = 1;
}

#[document(
    summary = "A kind of governance by name alone: what a light client needs to know, since a chain with governance may change its consensus, and the client then waits to learn each epoch's from a light update before it checks that epoch's headers.",
    category = "Governance",
    see_also = "LightParams::governance, GovernanceConfig",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
#[wincode(tag_encoding = "u8")]
pub enum GovernanceKind {
    #[document(summary = "Finality's electorate approves.")]
    #[wincode(tag = 0)]
    Electorate,
    #[document(summary = "A council genesis names approves.")]
    #[wincode(tag = 1)]
    Council,
}

impl GovernanceKind {
    #[document(parent = "GovernanceKind", summary = "The kind's name: electorate or council.")]
    pub fn name(&self) -> &'static str {
        match self {
            GovernanceKind::Electorate => "electorate",
            GovernanceKind::Council => "council",
        }
    }
}

impl GovernanceConfig {
    #[document(parent = "GovernanceConfig", summary = "The names of the kinds this build has, as genesis files spell them.")]
    pub const KINDS: &'static [&'static str] = &[
        #[cfg(feature = "governance-electorate")]
        "electorate",
        #[cfg(feature = "governance-council")]
        "council",
    ];

    #[document(parent = "GovernanceConfig", summary = "The kind, by name alone.")]
    pub fn kind(&self) -> GovernanceKind {
        match *self {
            #[cfg(feature = "governance-electorate")]
            GovernanceConfig::Electorate { .. } => GovernanceKind::Electorate,
            #[cfg(feature = "governance-council")]
            GovernanceConfig::Council { .. } => GovernanceKind::Council,
        }
    }

    #[document(parent = "GovernanceConfig", summary = "The kind's name: electorate or council.")]
    pub fn name(&self) -> &'static str {
        self.kind().name()
    }

    #[document(parent = "GovernanceConfig", summary = "How many epochs an approval counts for.")]
    pub fn approval_epochs(&self) -> u64 {
        match *self {
            #[cfg(feature = "governance-electorate")]
            GovernanceConfig::Electorate { approval_epochs } => approval_epochs,
            #[cfg(feature = "governance-council")]
            GovernanceConfig::Council { approval_epochs, .. } => approval_epochs,
        }
    }

    #[document(parent = "GovernanceConfig", summary = "Settings no chain could govern with.")]
    pub fn problem(&self) -> Option<&'static str> {
        if self.approval_epochs() == 0 {
            return Some("governance.approval_epochs must be at least 1");
        }

        match *self {
            #[cfg(feature = "governance-electorate")]
            GovernanceConfig::Electorate { .. } => None,
            #[cfg(feature = "governance-council")]
            GovernanceConfig::Council { ref members, threshold, .. } => {
                let mut sorted = members.clone();
                sorted.sort();
                sorted.dedup();

                if members.is_empty() {
                    Some("governance.members must name at least one account")
                } else if sorted.len() != members.len() {
                    Some("governance.members names an account twice")
                } else if threshold == 0 || threshold as usize > members.len() {
                    Some("governance.threshold must be at least 1 and at most the number of members")
                } else {
                    None
                }
            }
        }
    }

    #[document(
        parent = "GovernanceConfig",
        summary = "The governance these settings describe, counting approvals under the chain's finality.",
        params = { finality = "The chain's finality, whose electorate approves under the electorate kind." },
    )]
    #[cfg(feature = "node")]
    #[cfg_attr(not(feature = "governance-electorate"), allow(unused_variables))]
    pub fn build(&self, finality: &crate::chain::FinalityConfig) -> std::sync::Arc<dyn Governance> {
        match *self {
            #[cfg(feature = "governance-electorate")]
            GovernanceConfig::Electorate { .. } => std::sync::Arc::new(electorate::Electorate::new(finality.clone())),
            #[cfg(feature = "governance-council")]
            GovernanceConfig::Council { ref members, threshold, .. } => std::sync::Arc::new(council::Council::new(members.clone(), threshold)),
        }
    }
}

#[document(
    summary = "What a change proposes: a new consensus, which takes effect at an epoch, as a checkpoint fixes it, so blocks either side of the boundary are checked by different rules; new bootnodes, which nodes dial from the block that decides them on; new prices for names, which hold from that block on too; or what makes a collection, which does as well. Each kind has a ballot and a version of its own, its ChangeKind's: deciding one leaves the others' approvals standing.",
    category = "Governance",
    see_also = "Tx::ApproveChange, Changes",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
#[wincode(tag_encoding = "u8")]
pub enum Change {
    #[document(summary = "A new consensus: who produces blocks, and how. It must fit the chain's finality as genesis's must: rotating needs authority finality, staked needs FFG. A proof-of-work consensus keeps genesis's window, if any.")]
    #[wincode(tag = 0)]
    Consensus(ConsensusConfig),
    #[document(summary = "New bootnodes, replacing genesis's or the last decided: libp2p addresses such as /ip4/203.0.113.7/udp/4001/quic-v1, at most 32, which nodes of the chain dial to find their first peers. Empty leaves the chain with none. Advice to nodes, not a rule blocks are checked by, so it takes effect at once.")]
    #[wincode(tag = 1)]
    Bootnodes(Vec<String>),
    #[document(summary = "New prices for names, replacing genesis's or the last decided: what a year costs by a name's length, as NamesConfig::prices lists them. Registrations and renewals pay them from the block that decides them on; names already held keep the years they paid for.")]
    #[wincode(tag = 2)]
    NamePrices(#[wincode(with = "crate::structs::u256::U256LeList")] Vec<crate::structs::u256::u256>),
    #[document(summary = "What makes a contract a collection, replacing genesis's or the last decided: the NFT and multi-token standards' functions. A chain whose genesis kept no collection registry turns it on this way. Deploys, upgrades and refreshes register by it from the block that decides it on; contracts already deployed join at their next upgrade or refresh.")]
    #[wincode(tag = 3)]
    Collections(crate::registry::collections::CollectionFunctions),
    #[document(summary = "A new federation for the bridges, replacing genesis's or the last decided: its signers, threshold and depths. Its version is one more than the last's. Contracts read it from the block that decides it on; the gateways on other chains follow it once the old signers sign it over.")]
    #[wincode(tag = 4)]
    Federation(crate::registry::federation::Federation),
    #[document(summary = "A new directory, replacing genesis's or the last decided: the contracts the chain announces, the bridges' gateways and Gateway.sol on each EVM chain, the swap contract, the DEX and its router, and the NFT factory. What the apps and tools trust from the block that decides it on; it never changes the federation's version.")]
    #[wincode(tag = 5)]
    Directory(crate::registry::directory::Directory),
    #[document(summary = "New fees, replacing genesis's or the last decided, or turning them on for a chain without: the block gas limit, the burned share, the gas schedule and the base price's rules. They apply from the block after the one that decides them; the base price carries on from where it was.")]
    #[wincode(tag = 6)]
    Fees(crate::params::fees::FeeConfig),
    #[document(summary = "A new block reward, replacing genesis's or the last decided: initial from the block after the one that decides it, halving every halving_interval blocks counted from there.")]
    #[wincode(tag = 7)]
    Reward(crate::params::Reward),
    #[document(summary = "New limits, replacing genesis's or the last decided: a transaction's fuel and memory, and max_block_bytes. They apply from the block after the one that decides them.")]
    #[wincode(tag = 8)]
    Limits(crate::params::BlockLimits),
    #[document(summary = "Finality's tunable settings, as the chain's whole finality config with only those changed: under FFG the vote depth, minimum bond, withdrawal delay, reporter divisor, most validators, vote reward, proofs per block and the leak; under authority the vote depth, most signers, approval epochs, vote reward and proofs per block; under depth the depth. Fixed for the epoch activation_delay + 1 after the one that decides it, so no epoch changes rules halfway.")]
    #[wincode(tag = 9)]
    Finality(#[wincode(with = "crate::structs::unset::Framed<crate::params::finality::FinalityConfig>")] crate::params::finality::FinalityConfig),
    #[document(
        summary = "A switch to the other kind of voting finality, FFG or authority, from an epoch on: the new kind's whole config, with genesis's checkpoint_interval and activation_delay. Into FFG the config names no validators: they bond from the block that decides it, and should none have by the checkpoint that fixes the switch epoch's voters, the switch lapses, the chain stays as it was, and bonds made too late are paid back. Into authority the config's signers vote from the switch epoch, and every FFG validator is unbonding from it. It runs from its epoch, or from activation_delay + 1 after the one that decides it, whichever is later; the justified and finalized checkpoints carry over.",
        params = {
            epoch = "The first epoch the new kind runs, at the earliest.",
            config = "The new kind and its settings.",
        },
    )]
    #[wincode(tag = 10)]
    SwitchFinality {
        epoch: u64,
        #[wincode(with = "crate::structs::unset::Framed<crate::params::finality::FinalityConfig>")]
        config: crate::params::finality::FinalityConfig,
    },
    #[document(summary = "An upgrade: from block from on, nodes older than least stop before importing it and say to update, so a release that changes the rules reaches the chain without old nodes following on alone. from must be ahead of the block deciding it; a later upgrade replaces it. Advice to nodes, never a rule blocks are checked by.")]
    #[wincode(tag = 11)]
    Upgrade(crate::structs::version::Upgrade),
    #[document(summary = "A new name for the chain, replacing genesis's or the last decided, as chain_name_problem allows: what the apps call the network from the block that decides it on. Only displays read it.")]
    #[wincode(tag = 12)]
    Name(String),
}

#[document(summary = "The longest a chain's name may be, in characters.", category = "Governance", see_also = "chain_name_problem")]
pub const MOST_NAME_CHARS: usize = 40;

#[document(
    summary = "Why a name cannot be a chain's: empty, longer than MOST_NAME_CHARS characters, with a control character, or with a space at either end.",
    category = "Governance",
    see_also = "Change::Name, Genesis::name",
)]
pub fn chain_name_problem(name: &str) -> Option<&'static str> {
    if name.is_empty() || name.chars().count() > MOST_NAME_CHARS {
        Some("a chain's name is 1 to 40 characters")
    } else if name.chars().any(char::is_control) {
        Some("a chain's name holds no control characters")
    } else if name.trim() != name {
        Some("a chain's name starts and ends with something other than a space")
    } else {
        None
    }
}

#[document(summary = "The most bootnodes genesis or a change may name.", category = "Governance", see_also = "Change::Bootnodes, Genesis::bootnodes")]
pub const MOST_BOOTNODES: usize = 32;

#[document(
    summary = "Why a list of bootnodes cannot be a chain's: more than MOST_BOOTNODES, one named twice, or one that is empty, longer than 256 bytes, or not a path starting with /. Whether each is an address libp2p dials is for nodes to find, which skip one that is not.",
    category = "Governance",
    see_also = "Change::Bootnodes, Genesis::bootnodes",
)]
pub fn bootnodes_problem(bootnodes: &[String]) -> Option<&'static str> {
    let mut sorted: Vec<&String> = bootnodes.iter().collect();
    sorted.sort();
    sorted.dedup();

    if bootnodes.len() > MOST_BOOTNODES {
        Some("bootnodes holds at most 32 addresses")
    } else if sorted.len() != bootnodes.len() {
        Some("bootnodes names an address twice")
    } else if bootnodes.iter().any(|address| !address.starts_with('/') || address.len() > 256 || address.chars().any(char::is_whitespace)) {
        Some("each bootnode is a libp2p address, such as /ip4/203.0.113.7/udp/4001/quic-v1")
    } else {
        None
    }
}

impl Hashable for Change {
    const TAG_LEN: usize = 1;
}

#[document(
    summary = "Which ballot and version a change is on: each kind of setting its own, but for the consensus, finality's tuning and a switch of finality's kind, which are checked against each other and so share one. A decision of a kind leaves every other kind's approvals standing.",
    category = "Governance",
    see_also = "Change::kind, Changes::approve",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, SchemaWrite, SchemaRead)]
#[wincode(tag_encoding = "u8")]
pub enum ChangeKind {
    #[document(summary = "The consensus, finality's tuning and a switch of finality's kind.")]
    #[wincode(tag = 0)]
    Consensus,
    #[document(summary = "The bootnodes.")]
    #[wincode(tag = 1)]
    Bootnodes,
    #[document(summary = "What names cost.")]
    #[wincode(tag = 2)]
    NamePrices,
    #[document(summary = "What makes a collection.")]
    #[wincode(tag = 3)]
    Collections,
    #[document(summary = "The bridges' federation.")]
    #[wincode(tag = 4)]
    Federation,
    #[document(summary = "The directory.")]
    #[wincode(tag = 5)]
    Directory,
    #[document(summary = "The fees.")]
    #[wincode(tag = 6)]
    Fees,
    #[document(summary = "The block reward.")]
    #[wincode(tag = 7)]
    Reward,
    #[document(summary = "The limits.")]
    #[wincode(tag = 8)]
    Limits,
    #[document(summary = "An upgrade.")]
    #[wincode(tag = 9)]
    Upgrade,
    #[document(summary = "The chain's name.")]
    #[wincode(tag = 10)]
    Name,
}

impl Hashable for ChangeKind {
    const TAG_LEN: usize = 1;
}

impl ChangeKind {
    #[document(parent = "ChangeKind", summary = "Every kind, in tag order.")]
    pub const ALL: [ChangeKind; 11] = [
        ChangeKind::Consensus,
        ChangeKind::Bootnodes,
        ChangeKind::NamePrices,
        ChangeKind::Collections,
        ChangeKind::Federation,
        ChangeKind::Directory,
        ChangeKind::Fees,
        ChangeKind::Reward,
        ChangeKind::Limits,
        ChangeKind::Upgrade,
        ChangeKind::Name,
    ];

    #[document(parent = "ChangeKind", summary = "Its name, as the API and the CLI write it: consensus, bootnodes, name_prices, collections, federation, directory, fees, reward, limits, upgrade or name.")]
    pub fn name(self) -> &'static str {
        match self {
            ChangeKind::Consensus => "consensus",
            ChangeKind::Bootnodes => "bootnodes",
            ChangeKind::NamePrices => "name_prices",
            ChangeKind::Collections => "collections",
            ChangeKind::Federation => "federation",
            ChangeKind::Directory => "directory",
            ChangeKind::Fees => "fees",
            ChangeKind::Reward => "reward",
            ChangeKind::Limits => "limits",
            ChangeKind::Upgrade => "upgrade",
            ChangeKind::Name => "name",
        }
    }
}

impl std::str::FromStr for ChangeKind {
    type Err = String;

    /// Its name, or a change's: finality and switch_finality are the consensus's.
    fn from_str(text: &str) -> Result<ChangeKind, String> {
        match text {
            "finality" | "switch_finality" => Ok(ChangeKind::Consensus),
            text => ChangeKind::ALL.into_iter().find(|kind| kind.name() == text).ok_or_else(|| {
                format!("{text} is not a kind of change; one of {}", ChangeKind::ALL.map(ChangeKind::name).join(", "))
            }),
        }
    }
}

impl Change {
    #[document(parent = "Change", summary = "Which ballot and version it is on.")]
    pub fn kind(&self) -> ChangeKind {
        match self {
            Change::Consensus(_) | Change::Finality(_) | Change::SwitchFinality { .. } => ChangeKind::Consensus,
            Change::Bootnodes(_) => ChangeKind::Bootnodes,
            Change::NamePrices(_) => ChangeKind::NamePrices,
            Change::Collections(_) => ChangeKind::Collections,
            Change::Federation(_) => ChangeKind::Federation,
            Change::Directory(_) => ChangeKind::Directory,
            Change::Fees(_) => ChangeKind::Fees,
            Change::Reward(_) => ChangeKind::Reward,
            Change::Limits(_) => ChangeKind::Limits,
            Change::Upgrade(_) => ChangeKind::Upgrade,
            Change::Name(_) => ChangeKind::Name,
        }
    }

    #[document(parent = "Change", summary = "What kind of change it is, as the API names it: consensus, bootnodes, name_prices, collections, federation, directory, fees, reward, limits, finality, switch_finality, upgrade or name.")]
    pub fn name(&self) -> &'static str {
        match self {
            Change::Consensus(_) => "consensus",
            Change::Bootnodes(_) => "bootnodes",
            Change::NamePrices(_) => "name_prices",
            Change::Collections(_) => "collections",
            Change::Federation(_) => "federation",
            Change::Directory(_) => "directory",
            Change::Fees(_) => "fees",
            Change::Reward(_) => "reward",
            Change::Limits(_) => "limits",
            Change::Finality(_) => "finality",
            Change::SwitchFinality { .. } => "switch_finality",
            Change::Upgrade(_) => "upgrade",
            Change::Name(_) => "name",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "finality-ffg")]
    #[test]
    fn a_change_of_finality_crosses_the_wire_with_its_leak_or_without() {
        use crate::chain::{FfgConfig, FinalityConfig};
        use crate::registry::validators::Leak;
        use crate::structs::tx::{FeeOffer, SignedTx, Tx};

        let config = |leak: Option<Leak>| {
            FinalityConfig::Ffg(FfgConfig {
                checkpoint_interval: 8,
                vote_depth: 1,
                min_bond: u256::new(100),
                activation_delay: 2,
                withdrawal_delay: 3,
                reporter_divisor: 4,
                max_validators: 10,
                vote_reward: u256::ZERO,
                max_proofs_per_block: 4,
                validators: Vec::new(),
                leak,
            })
        };

        // The leak reads only at a message's end; framed, it reads in the middle of a transaction.
        for leak in [None, Some(Leak { after_epochs: 4, permille: 10 })] {
            for change in [Change::Finality(config(leak)), Change::SwitchFinality { epoch: 40, config: config(leak) }] {
                let tx = Tx::ApproveChange { version: 3, change };
                let signed = SignedTx::sign(tx, 7, FeeOffer::NONE, &crate::crypto::keypair::Keypair::development());
                let read: SignedTx = wincode::deserialize(&wincode::serialize(&signed).unwrap()).unwrap();

                assert_eq!(read, signed);
            }
        }
    }

    #[test]
    fn a_chains_bootnodes_are_a_short_list_of_addresses_each_named_once() {
        let address = |port: u16| format!("/ip4/203.0.113.7/udp/{port}/quic-v1");

        assert_eq!(bootnodes_problem(&[]), None);
        assert_eq!(bootnodes_problem(&[address(4001), address(4002)]), None);
        assert_eq!(bootnodes_problem(&[address(4001), address(4001)]), Some("bootnodes names an address twice"));
        assert!(bootnodes_problem(&(0..33).map(address).collect::<Vec<_>>()).is_some());

        for wrong in ["203.0.113.7:4001", "", "/ip4/1.2.3.4 /udp/1", &format!("/{}", "x".repeat(256))] {
            assert!(bootnodes_problem(&[wrong.to_string()]).is_some(), "{wrong:?}");
        }
    }
}
