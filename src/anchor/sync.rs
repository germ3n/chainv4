//! Anchored sync's judgement: which history the chains holding its anchors saw first. Each peer
//! shows its chain's anchor list, the newest entry proven by its anchor's transaction, whose
//! payload holds the root of every entry before it, so the whole list is the chain's own. Two lists
//! that agree up to a place and differ there are two histories, and the one anchored earlier after
//! that place was there first: a long-range fork made later from old keys cannot have anchored its
//! checkpoints in Bitcoin's past, nor in Ethereum's finalized one. Bitcoin decides where both
//! histories have Bitcoin anchors after they part; Ethereum fills the gaps. Each entry compared is
//! proven too, as the fork's own relay could say anything of its own blocks.

use {
    super::{AnchorEntry, AnchorsAnswer, Payload, chain_tag, list_root},
    crate::{
        params::anchoring::{AnchorChain, AnchorSource},
        structs::{hash::Hash, tx::AnchorProof},
    },
    r#macro::document,
    std::collections::BTreeMap,
};

#[cfg(feature = "anchoring-btc")]
use super::bitcoin::HeaderChain;
#[cfg(feature = "anchoring-eth")]
use super::ethereum::EthCommittees;

#[document(
    summary = "A peer's anchor list once checked: every entry, and which of them are proven to sit where they say in the chain holding them.",
    category = "Anchoring",
    see_also = "AnchorsAnswer, compare_histories",
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct History {
    #[document(summary = "Every entry, oldest first.")]
    pub entries: Vec<AnchorEntry>,
    #[document(summary = "The places proven, with where their proofs showed them: a Bitcoin block and height, or an Ethereum root and slot.")]
    pub proven: BTreeMap<u64, AnchorSource>,
    #[document(summary = "The chains the node could check this history's anchors in: Bitcoin once a peer showed its headers, Ethereum once a beacon API showed the committees from the peer's eth_start. Only a chain both histories were checked in decides between them.")]
    pub readable: Vec<AnchorChain>,
}

impl History {
    #[document(parent = "History", summary = "Its newest anchored checkpoint: where a node syncing by it starts.")]
    pub fn newest(&self) -> &AnchorEntry {
        self.entries.last().expect("a history holds an entry")
    }

    /// The place of its first entry in a chain, at or after a place.
    fn first_in(&self, chain: AnchorChain, from: usize) -> Option<u64> {
        self.entries.iter().enumerate().skip(from).find(|(_, entry)| entry.source.chain() == chain).map(|(index, _)| index as u64)
    }

    /// Where a place is proven to sit in a chain, if it is.
    fn proven_in(&self, chain: AnchorChain, index: u64) -> Option<u64> {
        self.proven.get(&index).filter(|source| source.chain() == chain).map(AnchorSource::position)
    }
}

#[document(
    summary = "What anchored sync checks anchors' proofs against: the headers of each chain it has heard from its peers. A chain it has none of proves nothing.",
    category = "Anchoring",
    see_also = "check_anchors",
)]
#[derive(Clone, Copy, Default)]
pub struct Sources<'a> {
    #[cfg(feature = "anchoring-btc")]
    #[document(summary = "Bitcoin's headers, the most work any peer showed.")]
    pub bitcoin: Option<&'a HeaderChain>,
    #[cfg(feature = "anchoring-eth")]
    #[document(summary = "Ethereum's sync committees, followed from each eth_start a peer named through the node's beacon APIs: none without --beacon, when Ethereum anchors prove nothing.")]
    pub ethereum: &'a [EthCommittees],
    #[document(summary = "Holds the lifetime in a build that reads neither chain.")]
    pub marker: std::marker::PhantomData<&'a ()>,
}

impl Sources<'_> {
    #[document(parent = "Sources", summary = "Whether anchors in a chain can be checked for a peer naming this Ethereum start: Bitcoin's once a peer showed its headers, Ethereum's once the committees from that start were followed.")]
    pub fn reads(&self, chain: AnchorChain, eth_start: Option<&crate::params::anchoring::EthStart>) -> bool {
        match chain {
            #[cfg(feature = "anchoring-btc")]
            AnchorChain::Bitcoin => self.bitcoin.is_some(),
            #[cfg(feature = "anchoring-eth")]
            AnchorChain::Ethereum => eth_start.is_some_and(|start| self.ethereum.iter().any(|committees| committees.start == *start)),
            #[allow(unreachable_patterns)]
            _ => {
                let _ = eth_start;

                false
            }
        }
    }
}

#[document(
    summary = "The newest entry of a peer's list in a chain the node can check: where a list whose newest anchors sit in a chain it cannot, Ethereum's without --beacon, is checked up to, and those after it decide nothing.",
    category = "Anchoring",
    see_also = "check_anchors, Sources::reads",
)]
pub fn newest_readable(answer: &AnchorsAnswer, sources: &Sources<'_>) -> Option<u64> {
    answer.entries.iter().rposition(|entry| sources.reads(entry.source.chain(), answer.eth_start.as_ref())).map(|place| place as u64)
}

#[document(
    summary = "A peer's list cut after a place: its entries to it, and the proofs it sent of them.",
    category = "Anchoring",
    see_also = "newest_readable",
)]
pub fn cut_at(answer: &AnchorsAnswer, place: u64) -> AnchorsAnswer {
    AnchorsAnswer {
        entries: answer.entries.iter().take(place as usize + 1).cloned().collect(),
        proofs: answer.proofs.iter().filter(|(index, _)| *index <= place).cloned().collect(),
        ..answer.clone()
    }
}

/// Checks one proof of the entry at a place: it carries this chain's anchor of that entry's
/// checkpoint, with the root of every entry before it, where the entry says, as deep as asked.
fn check_proof(answer: &AnchorsAnswer, index: usize, proof: &AnchorProof, sources: &Sources<'_>, genesis: &Hash, depth: u64) -> Result<AnchorSource, String> {
    let entries = &answer.entries[..];
    let entry = &entries[index];

    if proof.chain() != entry.source.chain() {
        return Err(format!("anchor {index}'s proof is in another chain than its entry"));
    }

    match proof {
        #[cfg(feature = "anchoring-btc")]
        AnchorProof::Bitcoin(proof) => {
            check_payload(entries, index, &Payload::find(&proof.raw)?, genesis)?;

            let Some(headers) = sources.bitcoin else {
                return Err(format!("anchor {index} is in Bitcoin, and no peer showed Bitcoin's headers"));
            };

            super::bitcoin::check_shown(proof, &entry.source, headers, index, depth)?;

            Ok(entry.source)
        }
        #[cfg(not(feature = "anchoring-btc"))]
        AnchorProof::Bitcoin(_) => {
            let _ = (sources, genesis, depth);

            Err(format!("anchor {index} is in Bitcoin, which this build reads nothing of"))
        }
        #[cfg(feature = "anchoring-eth")]
        AnchorProof::Ethereum(proof) => {
            let Some(start) = &answer.eth_start else {
                return Err(format!("anchor {index} is in Ethereum, and the peer names no Ethereum start"));
            };
            let Some(committees) = sources.ethereum.iter().find(|committees| committees.start == *start) else {
                return Err(format!("anchor {index} is in Ethereum, and no beacon API (--beacon) showed its sync committees"));
            };
            let (payload, _) = super::ethereum::check_shown(proof, &entry.source, committees, index)?;
            check_payload(entries, index, &payload, genesis)?;

            Ok(entry.source)
        }
        #[cfg(not(feature = "anchoring-eth"))]
        AnchorProof::Ethereum(_) => {
            let _ = (sources, genesis, &answer.eth_start);

            Err(format!("anchor {index} is in Ethereum, which this build reads nothing of"))
        }
    }
}

/// Checks an anchor's payload is of the entry at a place: this chain's, its checkpoint, and the
/// root of every entry before it.
#[cfg_attr(not(any(feature = "anchoring-btc", feature = "anchoring-eth")), allow(dead_code))]
fn check_payload(entries: &[AnchorEntry], index: usize, payload: &Payload, genesis: &Hash) -> Result<(), String> {
    let entry = &entries[index];

    if payload.chain != chain_tag(genesis) || payload.epoch != entry.epoch || payload.block[..] != entry.block.to_bytes()[..] {
        return Err(format!("anchor {index}'s transaction is not of its checkpoint"));
    }

    if payload.root != list_root(&entries[..index]) {
        return Err(format!("anchor {index}'s root is not that of the entries before it"));
    }

    Ok(())
}

#[document(
    summary = "Checks a peer's anchor list against the chains holding it: the newest entry's proof must hold, and with it the root of every entry before it, and each other proof the peer sent must hold too. A Bitcoin proof holds against the most-work headers peers showed, an Ethereum one against the sync committees followed from the peer's eth_start; one in a chain the node heard nothing of holds nothing.",
    params = {
        answer = "What the peer sent.",
        sources = "What was heard of each chain: Bitcoin's headers, the most work any peer showed, and Ethereum's committees.",
        genesis = "This chain's genesis id.",
        depth = "How deep an anchor's Bitcoin block must be.",
    },
    returns = "The history, or why the answer does not check.",
    category = "Anchoring",
    see_also = "AnchorsAnswer, History, Sources",
)]
pub fn check_anchors(answer: &AnchorsAnswer, sources: &Sources<'_>, genesis: &Hash, depth: u64) -> Result<History, String> {
    let Some(newest) = answer.entries.len().checked_sub(1) else {
        return Err("no anchors".to_string());
    };

    if !answer.proofs.iter().any(|(index, _)| *index == newest as u64) {
        return Err("no proof of the newest anchor".to_string());
    }

    let mut proven = BTreeMap::new();

    for (index, proof) in &answer.proofs {
        let place = usize::try_from(*index).ok().filter(|place| *place <= newest).ok_or_else(|| format!("a proof of anchor {index}, past the list"))?;
        proven.insert(*index, check_proof(answer, place, proof, sources, genesis, depth)?);
    }

    let readable = [AnchorChain::Bitcoin, AnchorChain::Ethereum].into_iter().filter(|chain| sources.reads(*chain, answer.eth_start.as_ref())).collect();

    Ok(History { entries: answer.entries.clone(), proven, readable })
}

#[document(
    summary = "What comparing two histories found.",
    category = "Anchoring",
    see_also = "compare_histories",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnchorVerdict {
    #[document(summary = "One is the other's start: the same history, the longer reaching further. True when the second is the longer.")]
    Extends(bool),
    #[document(summary = "They part at a place, and the entries deciding sit lower in their chain in one: that one was first. True when the second was.")]
    Earlier(bool),
    #[document(summary = "They part, and the entries deciding are not both proven yet: ask the first peer for its proof of the first place, and the second for the second. The places may differ, as each history's first entry in the deciding chain may sit at another place.")]
    NeedProofs(u64, u64),
    #[document(summary = "They part at a place, and the entries deciding sit in the same block of their chain: two histories anchored at once, which no rule picks between.")]
    Tie(u64),
    #[document(summary = "They part at a place after which no chain holds an anchor of both: nothing decides, which the node treats as a tie.")]
    Undecided(u64),
}

/// Whether two entries name the same checkpoint: the same history up to it, whatever transaction
/// each chain proved it by.
fn same(a: &AnchorEntry, b: &AnchorEntry) -> bool {
    a.epoch == b.epoch && a.block == b.block
}

/// The verdict of one chain where two histories part: by each one's first entry in it at or after
/// the place, if both have one.
#[cfg_attr(not(any(feature = "anchoring-btc", feature = "anchoring-eth")), allow(dead_code))]
fn decide(a: &History, b: &History, at: usize, chain: AnchorChain) -> Option<AnchorVerdict> {
    if !a.readable.contains(&chain) || !b.readable.contains(&chain) {
        return None;
    }

    let (first, second) = (a.first_in(chain, at)?, b.first_in(chain, at)?);

    Some(match (a.proven_in(chain, first), b.proven_in(chain, second)) {
        (Some(one), Some(other)) if one == other => AnchorVerdict::Tie(at as u64),
        (Some(one), Some(other)) => AnchorVerdict::Earlier(other < one),
        _ => AnchorVerdict::NeedProofs(first, second),
    })
}

#[document(
    summary = "Compares two checked histories: where their entries first part, and which was anchored first after it. Bitcoin decides where both have a Bitcoin anchor at or after the place, by each one's first; Ethereum, by slot, where they do not; and where neither does, nothing decides. A build reading only one of the chains goes by that one alone.",
    category = "Anchoring",
    see_also = "AnchorVerdict, check_anchors",
)]
pub fn compare_histories(a: &History, b: &History) -> AnchorVerdict {
    let parted = a.entries.iter().zip(&b.entries).position(|(a, b)| !same(a, b));

    let Some(at) = parted else {
        return AnchorVerdict::Extends(b.entries.len() > a.entries.len());
    };

    #[cfg(feature = "anchoring-btc")]
    if let Some(verdict) = decide(a, b, at, AnchorChain::Bitcoin) {
        return verdict;
    }

    #[cfg(feature = "anchoring-eth")]
    if let Some(verdict) = decide(a, b, at, AnchorChain::Ethereum) {
        return verdict;
    }

    AnchorVerdict::Undecided(at as u64)
}
