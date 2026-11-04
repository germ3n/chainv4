//! Anchored sync's judgement: which history Bitcoin saw first. Each peer shows its chain's anchor
//! list, the newest entry proven by its Bitcoin transaction, whose OP_RETURN holds the root of every
//! entry before it, so the whole list is the chain's own. Two lists that agree up to a place and
//! differ there are two histories, and the one whose entry at that place sits lower in Bitcoin's
//! main chain was there first: a long-range fork made later from old keys cannot have anchored its
//! checkpoints in Bitcoin's past. Each entry compared there is proven too, as the fork's own relay
//! could say anything of its own blocks.

use {
    super::{AnchorEntry, AnchorsAnswer, BtcProof, Payload, bitcoin::HeaderChain, chain_tag, list_root},
    crate::structs::hash::Hash,
    r#macro::document,
    std::collections::BTreeMap,
};

#[document(
    summary = "A peer's anchor list once checked: every entry, and which of them are proven to sit in Bitcoin's main chain where they say.",
    category = "Anchoring",
    see_also = "AnchorsAnswer, compare_histories",
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct History {
    #[document(summary = "Every entry, oldest first.")]
    pub entries: Vec<AnchorEntry>,
    #[document(summary = "The places proven, with their Bitcoin heights.")]
    pub proven: BTreeMap<u64, u64>,
}

impl History {
    #[document(parent = "History", summary = "Its newest anchored checkpoint: where a node syncing by it starts.")]
    pub fn newest(&self) -> &AnchorEntry {
        self.entries.last().expect("a history holds an entry")
    }
}

/// Checks one proof of the entry at a place: it carries this chain's anchor of that entry's
/// checkpoint, with the root of every entry before it, in a block of the main chain as deep as
/// asked, where the entry says.
fn check_proof(entries: &[AnchorEntry], index: usize, proof: &BtcProof, headers: &HeaderChain, genesis: &Hash, depth: u64) -> Result<u64, String> {
    let entry = &entries[index];
    let payload = Payload::find(&proof.raw)?;

    if payload.chain != chain_tag(genesis) || payload.epoch != entry.epoch || payload.block[..] != entry.block.to_bytes()[..] {
        return Err(format!("anchor {index}'s transaction is not of its checkpoint"));
    }

    if payload.root != list_root(&entries[..index]) {
        return Err(format!("anchor {index}'s root is not that of the entries before it"));
    }

    let Some((height, merkle)) = headers.block(&proof.block) else {
        return Err(format!("anchor {index}'s Bitcoin block is in no header shown"));
    };

    if proof.block != entry.btc_block || height != entry.btc_height {
        return Err(format!("anchor {index}'s proof is of another Bitcoin block than its entry"));
    }

    if !proof.leads_to(&merkle) {
        return Err(format!("anchor {index}'s Merkle paths do not lead to its block's root"));
    }

    if headers.depth(&proof.block) < depth {
        return Err(format!("anchor {index}'s Bitcoin block is off the main chain, or not {depth} deep"));
    }

    Ok(height)
}

#[document(
    summary = "Checks a peer's anchor list against Bitcoin's headers: the newest entry's proof must hold, and with it the root of every entry before it, and each other proof the peer sent must hold too.",
    params = {
        answer = "What the peer sent.",
        headers = "Bitcoin's headers, the most work any peer showed.",
        genesis = "This chain's genesis id.",
        depth = "How deep an anchor's Bitcoin block must be.",
    },
    returns = "The history, or why the answer does not check.",
    category = "Anchoring",
    see_also = "AnchorsAnswer, History",
)]
pub fn check_anchors(answer: &AnchorsAnswer, headers: &HeaderChain, genesis: &Hash, depth: u64) -> Result<History, String> {
    let Some(newest) = answer.entries.len().checked_sub(1) else {
        return Err("no anchors".to_string());
    };

    if !answer.proofs.iter().any(|(index, _)| *index == newest as u64) {
        return Err("no proof of the newest anchor".to_string());
    }

    let mut proven = BTreeMap::new();

    for (index, proof) in &answer.proofs {
        let place = usize::try_from(*index).ok().filter(|place| *place <= newest).ok_or_else(|| format!("a proof of anchor {index}, past the list"))?;
        proven.insert(*index, check_proof(&answer.entries, place, proof, headers, genesis, depth)?);
    }

    Ok(History { entries: answer.entries.clone(), proven })
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
    #[document(summary = "They part at a place, and one's entry there sits lower in Bitcoin: that one was first. True when the second was.")]
    Earlier(bool),
    #[document(summary = "They part at a place whose entries are not both proven yet: ask each peer for its proof of that place.")]
    NeedProofs(u64),
    #[document(summary = "They part at a place whose entries sit in the same Bitcoin block: two histories anchored at once, which no rule picks between.")]
    Tie(u64),
}

/// Whether two entries name the same checkpoint: the same history up to it, whatever Bitcoin
/// transaction each chain proved it by.
fn same(a: &AnchorEntry, b: &AnchorEntry) -> bool {
    a.epoch == b.epoch && a.block == b.block
}

#[document(
    summary = "Compares two checked histories: where their entries first part, and which Bitcoin saw first.",
    category = "Anchoring",
    see_also = "AnchorVerdict, check_anchors",
)]
pub fn compare_histories(a: &History, b: &History) -> AnchorVerdict {
    let parted = a.entries.iter().zip(&b.entries).position(|(a, b)| !same(a, b));

    let Some(at) = parted else {
        return AnchorVerdict::Extends(b.entries.len() > a.entries.len());
    };

    let at = at as u64;

    match (a.proven.get(&at), b.proven.get(&at)) {
        (Some(first), Some(second)) if first == second => AnchorVerdict::Tie(at),
        (Some(first), Some(second)) => AnchorVerdict::Earlier(second < first),
        _ => AnchorVerdict::NeedProofs(at),
    }
}
