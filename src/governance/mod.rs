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
    summary = "What a change proposes: a new consensus, which takes effect at an epoch, as a checkpoint fixes it, so blocks either side of the boundary are checked by different rules; new bootnodes, which nodes dial from the block that decides them on; new prices for names, which hold from that block on too; or what makes a collection, which does as well. Every kind shares one ballot and one version.",
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

impl Change {
    #[document(parent = "Change", summary = "What kind of change it is, as the API names it: consensus, bootnodes, name_prices or collections.")]
    pub fn name(&self) -> &'static str {
        match self {
            Change::Consensus(_) => "consensus",
            Change::Bootnodes(_) => "bootnodes",
            Change::NamePrices(_) => "name_prices",
            Change::Collections(_) => "collections",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
