//! Bridges for the apps and the federation's tools (docs/bridges.md): a transfer as both
//! gateways, contracts/lua/gateway.lua here and contracts/evm/Gateway.sol on EVM chains, hash it,
//! and as the federation's signers sign it.

use {
    crate::abi::{AbiType, AbiValue, encode_values},
    crate::crypto::hasher::Hasher,
    crate::structs::address::Address,
    crate::structs::hash::HashType,
    crate::structs::tx::TxCallArgumentType,
    crate::structs::u256::u256,
    r#macro::document,
};

#[cfg(feature = "bitcoin")]
#[document(
    summary = "A Bitcoin multisig script, as the Bitcoin gateway writes it: the threshold, each key, their count and OP_CHECKMULTISIG. Bare, it is a federation's change; under a recipient, its deposit script.",
    category = "Bridges",
    see_also = "btc_deposit_script",
)]
pub fn btc_multisig(threshold: u32, keys: &[[u8; 33]]) -> Vec<u8> {
    // Numbers to 16 as their own opcodes, past it as a one-byte push, as the gateway does.
    let small = |number: usize| match number {
        0..=16 => vec![0x50 + number as u8],
        _ => vec![1, number as u8],
    };
    let mut script = small(threshold as usize);

    for key in keys {
        script.push(33);
        script.extend_from_slice(key);
    }

    script.extend(small(keys.len()));
    script.push(0xae);

    script
}

#[cfg(feature = "bitcoin")]
#[document(
    summary = "An address's deposit script under a federation, as the Bitcoin gateway writes it: its 33 bytes, dropped; the chain's id and the gateway, dropped; then the multisig. Paid as its P2WSH, the gateway mints wBTC to the address for it, on this chain alone, as a proof of a payment outlives a chain; computed here, so a wallet never takes the address to pay from a node.",
    params = {
        recipient = "Who the deposit is for.",
        chain_id = "The chain's id, crypto::ethereum::chain_id of its genesis.",
        gateway = "The Bitcoin gateway.",
        threshold = "How many of the federation must sign.",
        keys = "The federation's keys.",
    },
    category = "Bridges",
    see_also = "btc_multisig",
)]
pub fn btc_deposit_script(recipient: &Address, chain_id: u64, gateway: &Address, threshold: u32, keys: &[[u8; 33]]) -> Vec<u8> {
    [&[33][..], &recipient.to_abi(), &[0x75, 41], &chain_id.to_le_bytes(), &gateway.to_abi(), &[0x75], &btc_multisig(threshold, keys)].concat()
}

#[cfg(feature = "bitcoin")]
#[document(
    summary = "What a federation signer signs to vote to cancel a release on the Bitcoin gateway: double SHA-256 of a domain, the chain's id, the gateway, the release and its transaction's id, as the gateway makes it.",
    category = "Bridges",
    see_also = "sign_digest",
)]
pub fn btc_cancel_digest(chain_id: u64, gateway: &Address, proposal: u64, txid: &[u8; 32]) -> [u8; 32] {
    crate::crypto::bitcoin::sha256d(&[b"chainv4-btc-cancel".as_slice(), &chain_id.to_le_bytes(), &gateway.to_abi(), &proposal.to_le_bytes(), txid].concat())
}

#[cfg(feature = "bitcoin")]
#[document(
    summary = "What a federation signer signs to propose a release on the Bitcoin gateway: double SHA-256 of a domain, the chain's id, the gateway, the number the release will have, its transaction's id and the withdrawals it pays, as the gateway makes it. The number makes a signature good once: a release cancelled takes signing again to come back.",
    params = {
        chain_id = "The chain's id.",
        gateway = "The Bitcoin gateway.",
        proposal = "The number it will have: proposal_count and one.",
        raw = "Its transaction without witnesses.",
        ids = "The withdrawals it pays, in order.",
    },
    category = "Bridges",
    see_also = "sign_digest, btc_cancel_digest",
)]
pub fn btc_propose_digest(chain_id: u64, gateway: &Address, proposal: u64, raw: &[u8], ids: &[u64]) -> [u8; 32] {
    let named: Vec<u8> = ids.iter().flat_map(|id| id.to_le_bytes()).collect();

    crate::crypto::bitcoin::sha256d(&[b"chainv4-btc-propose".as_slice(), &chain_id.to_le_bytes(), &gateway.to_abi(), &proposal.to_le_bytes(), &crate::crypto::bitcoin::sha256d(raw), &(ids.len() as u64).to_le_bytes(), &named].concat())
}

#[cfg(feature = "bitcoin")]
#[document(
    summary = "What a federation signer signs to propose a bump on the Bitcoin gateway: double SHA-256 of a domain, the chain's id, the gateway, the number the bump will have, the proposal it replaces or pays for, and its transaction's id, as the gateway makes it. A replacement's domain is chainv4-btc-replace and a child's chainv4-btc-child, so neither passes for the other, for a release or for a vote.",
    params = {
        domain = "BTC_REPLACE_DOMAIN or BTC_CHILD_DOMAIN.",
        chain_id = "The chain's id.",
        gateway = "The Bitcoin gateway.",
        proposal = "The number it will have: proposal_count and one.",
        of = "What it replaces, or for a child what it pays for.",
        raw = "Its transaction without witnesses.",
    },
    category = "Bridges",
    see_also = "btc_propose_digest, sign_digest",
)]
pub fn btc_bump_digest(domain: &str, chain_id: u64, gateway: &Address, proposal: u64, of: u64, raw: &[u8]) -> [u8; 32] {
    crate::crypto::bitcoin::sha256d(&[domain.as_bytes(), &chain_id.to_le_bytes(), &gateway.to_abi(), &proposal.to_le_bytes(), &of.to_le_bytes(), &crate::crypto::bitcoin::sha256d(raw)].concat())
}

#[cfg(feature = "bitcoin")]
#[document(summary = "What a replacement's proposal is signed under: see btc_bump_digest.", category = "Bridges")]
pub const BTC_REPLACE_DOMAIN: &str = "chainv4-btc-replace";

#[cfg(feature = "bitcoin")]
#[document(summary = "What a child's proposal is signed under: see btc_bump_digest.", category = "Bridges")]
pub const BTC_CHILD_DOMAIN: &str = "chainv4-btc-child";

#[cfg(feature = "bitcoin")]
#[document(summary = "Each input's sequence in a release, a replacement and a child: 0xfffffffd, which signals BIP125 so a version paying more replaces it in Bitcoin's mempools, and whose top bit leaves it no BIP68 delay.", category = "Bridges")]
pub const BTC_SEQUENCE: u32 = 0xffff_fffd;

#[cfg(feature = "bitcoin")]
#[document(summary = "What a replacement pays past every version it replaces, in satoshis a virtual byte: Bitcoin's incremental relay fee, which its nodes ask before they take a replacement. The gateway asks the same.", category = "Bridges")]
pub const BTC_LEAST_BUMP_RATE: u64 = 1;

#[cfg(feature = "bitcoin")]
#[document(
    summary = "The most virtual bytes a release can be once signed, as the Bitcoin gateway reckons it to hold its fee to the floor: its bytes without witnesses, and for each input, a threshold of signatures of at most 73 bytes and its witness script.",
    params = {
        base = "The length of the transaction without its witnesses.",
        inputs = "Each input's threshold, and the length of its witness script.",
    },
    category = "Bridges",
)]
pub fn btc_release_vbytes(base: usize, inputs: &[(u32, usize)]) -> u64 {
    let witness = |(threshold, script): &(u32, usize)| 2 + *threshold as usize * 74 + if *script < 0xfd { 1 } else { 3 } + *script;
    let weight = 4 * base + 2 + inputs.iter().map(witness).sum::<usize>();

    weight.div_ceil(4) as u64
}

#[cfg(feature = "bitcoin")]
#[document(
    summary = "A withdrawal a release names, as the Bitcoin gateway says it stands.",
    category = "Bridges",
    see_also = "check_btc_release",
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NamedWithdrawal {
    pub id: u64,
    #[document(summary = "The output script it asked to be paid to.")]
    pub script: Vec<u8>,
    #[document(summary = "What it burned: it arrives less its share of its release's fee.")]
    pub sats: u64,
    #[document(summary = "The most it agreed to lose to that share, as the gateway judges a release proposed now.")]
    pub most_fee: u64,
    #[document(summary = "queued, proposed or paid.")]
    pub state: String,
    #[document(summary = "The outpoint its release must spend, after one paying it was cancelled.")]
    pub anchor: Option<String>,
    #[document(summary = "A lower most fee its owner asked for that does not hold yet, and the height it holds from while it is queued: BTC_LOWER_MOST_FEE_AFTER blocks after it was asked for.")]
    pub lowering: Option<(u64, u64)>,
}

#[cfg(feature = "bitcoin")]
#[document(
    summary = "How many blocks a lowered most fee waits before it holds, as the Bitcoin gateway has it: a release proposed against the old limit, with up to 50 withdrawals, lands under it, rather than one owner undoing it by lowering theirs. A raised one holds at once.",
    category = "Bridges",
    see_also = "NamedWithdrawal::most_fee_at",
)]
pub const BTC_LOWER_MOST_FEE_AFTER: u64 = 30;

#[cfg(feature = "bitcoin")]
impl NamedWithdrawal {
    #[document(
        parent = "NamedWithdrawal",
        summary = "The most it may lose to fees in a release proposed at a height, as the gateway judges it: a lowering holds from its height while it is queued, and one in a release keeps the limit that release was proposed under, for its bumps too.",
    )]
    pub fn most_fee_at(&self, height: u64) -> u64 {
        match self.lowering {
            Some((lowered, from)) if self.state == "queued" && height >= from => lowered,
            _ => self.most_fee,
        }
    }

    #[document(
        parent = "NamedWithdrawal",
        summary = "The most it may lose in a release however late that lands: most_fee_at once any lowering holds. What the planner goes by, as it cannot know the block a proposal lands in, and a release within the lower limit passes the higher one too.",
    )]
    pub fn most_fee_ahead(&self) -> u64 {
        self.most_fee_at(u64::MAX)
    }
}

#[cfg(feature = "bitcoin")]
#[document(
    summary = "An output the Bitcoin gateway holds that a release spends, as it says it stands.",
    category = "Bridges",
    see_also = "check_btc_release",
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpentOutput {
    pub sats: u64,
    #[document(summary = "The federation it was paid to.")]
    pub version: u64,
    #[document(summary = "unspent, pending or spent.")]
    pub state: String,
    #[document(summary = "How many of that federation must sign.")]
    pub threshold: u32,
    #[document(summary = "The length of the witness script that spends it.")]
    pub script_len: usize,
}

#[cfg(feature = "bitcoin")]
#[document(
    summary = "Everything a federation signer checks a release against, each read from the Bitcoin gateway's state rather than taken from what proposed it.",
    category = "Bridges",
    see_also = "check_btc_release",
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseFacts {
    #[document(summary = "The transaction as the gateway keeps it, without witnesses.")]
    pub raw: Vec<u8>,
    #[document(summary = "The transaction's id the gateway keeps.")]
    pub txid: Vec<u8>,
    #[document(summary = "The federation it was proposed under.")]
    pub version: u64,
    #[document(summary = "The withdrawals it names, in order.")]
    pub withdrawals: Vec<NamedWithdrawal>,
    #[document(summary = "What each input spends, in order, with its outpoint as the gateway writes it.")]
    pub inputs: Vec<(String, SpentOutput)>,
    #[document(summary = "The bare multisig of the federation it was proposed under.")]
    pub change_script: Vec<u8>,
    #[document(summary = "The most fee a release may pay, in satoshis.")]
    pub fee_cap: u64,
    #[document(summary = "The least it may pay a virtual byte.")]
    pub fee_floor: u64,
    #[document(summary = "The most a virtual byte this signer signs for, from btc_most_fee_rate: what a release pays past it is burned.")]
    pub most_rate: u64,
    #[document(summary = "What it repays of what children cost the backing, and the policy that goes by.")]
    pub repay: ReleaseRepay,
}

#[cfg(feature = "bitcoin")]
#[document(
    summary = "What a release repays of what children cost the backing, as the gateway says it stands, with the federation's policy for it: what a signer checks it against, and what the planner works it out from.",
    category = "Bridges",
    see_also = "btc_repays, check_btc_release",
)]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReleaseRepay {
    #[document(summary = "What each of its withdrawals repays, in order; empty for none.")]
    pub shares: Vec<u64>,
    #[document(summary = "The budget those were worked out of: the least of what was owed and pct of its fee.")]
    pub budget: u64,
    #[document(summary = "To a signer, what children cost the backing that is not repaid yet, shortfall(), which no release repays more than; to the planner, what is left of that past what open releases repay.")]
    pub owed: u64,
    #[document(summary = "btc.repay_pct: the most of its fee a release repays, as a percent.")]
    pub pct: u64,
    #[document(summary = "btc.repay_most: the most one withdrawal repays.")]
    pub most: u64,
}

#[cfg(feature = "bitcoin")]
impl ReleaseRepay {
    /// What each of this many withdrawals repays, none past the shares given.
    fn each(&self, count: usize) -> Vec<u64> {
        (0..count).map(|at| self.shares.get(at).copied().unwrap_or(0)).collect()
    }
}

#[cfg(feature = "bitcoin")]
#[document(
    summary = "What each withdrawal of a release repays of what children cost the backing, as the gateway works it out when it is proposed: of a budget, the least of what is owed past what open releases repay and pct of the release's fee, each an even share, at most most, and only what its limit leaves past its share of the fee, so one at its limit repays nothing and waits for nothing.",
    params = {
        owed = "What is owed past what open releases repay.",
        fee = "The release's fee.",
        pct = "btc.repay_pct.",
        most = "btc.repay_most.",
        rooms = "What each withdrawal's limit leaves past its share of the fee, in order.",
    },
    returns = "The budget, and what each repays.",
    category = "Bridges",
    see_also = "ReleaseRepay",
)]
pub fn btc_repays(owed: u64, fee: u64, pct: u64, most: u64, rooms: &[u64]) -> (u64, Vec<u64>) {
    let budget = owed.min(fee.saturating_mul(pct) / 100);
    let count = rooms.len() as u64;

    (budget, rooms.iter().enumerate().map(|(at, room)| btc_fee_share(budget, count, at).min(most).min(*room)).collect())
}

#[cfg(feature = "bitcoin")]
#[document(
    summary = "Why a signer will not sign a release. Each comes from reading all of what the gateway says of it, so none is a read that failed, which never gets this far: what a signer does with one is btc_release_step.",
    category = "Bridges",
    see_also = "check_btc_release, btc_release_step",
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReleaseRefusal {
    #[document(summary = "It should never have been proposed: nothing about it changes with time.")]
    Wrong(String),
    #[document(summary = "An input was shown spent by another release, so Bitcoin can never mine this one, however many signed it.")]
    Dead(String),
    #[document(summary = "It pays past the most a virtual byte this signer signs for now.")]
    Overpriced(String),
    #[document(summary = "A bump of what is not stuck, as far as this signer's Esplora says: mined, or paying what the network asks. Another signer's may say otherwise, so it gets a vote only past the age limit.")]
    Needless(String),
}

#[cfg(feature = "bitcoin")]
impl std::fmt::Display for ReleaseRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReleaseRefusal::Wrong(why) | ReleaseRefusal::Dead(why) | ReleaseRefusal::Overpriced(why) | ReleaseRefusal::Needless(why) => formatter.write_str(why),
        }
    }
}

#[cfg(feature = "bitcoin")]
#[document(summary = "How long a release may stand before its age says something, unless --cancel-after says otherwise, in milliseconds of the chain's block time: 6 hours. Short of its signatures it is then voted down, and with all of them it is warned about.", category = "Bridges")]
pub const BTC_CANCEL_AFTER_MS: u64 = 6 * 3_600_000;

#[cfg(feature = "bitcoin")]
#[document(
    summary = "Where a release stands for one signer: what btc_release_step goes by. A read that failed never gets this far, as nothing can be said of a release half read.",
    category = "Bridges",
    see_also = "btc_release_step",
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseStanding {
    #[document(summary = "Why check_btc_release refuses it, if it does.")]
    pub refusal: Option<ReleaseRefusal>,
    #[document(summary = "Whether this signer has no fee rate to go by, neither Esplora's nor its operator's, so what it refuses as overpriced may be fine.")]
    pub blind: bool,
    #[document(summary = "Whether this signer signed any input of it.")]
    pub signed_by_me: bool,
    #[document(summary = "Whether every input holds its threshold of signatures: anyone can then finish it, and the gateway refuses a vote.")]
    pub signed_enough: bool,
    #[document(summary = "Whether it is older than the limit by the chain's block time; None if that could not be read.")]
    pub past_limit: Option<bool>,
}

#[cfg(feature = "bitcoin")]
#[document(summary = "What a signer's pass does about one open release.", category = "Bridges", see_also = "btc_release_step")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReleaseStep {
    #[document(summary = "Signs each input it has not.")]
    Sign,
    #[document(summary = "Votes to cancel it, for this reason.")]
    Vote(String),
    #[document(summary = "Leaves it, for this reason.")]
    Leave(String),
    #[document(summary = "Nothing: it holds all its signatures and stands, so one more would only cost.")]
    Nothing,
}

#[cfg(feature = "bitcoin")]
#[document(
    summary = "What a signer does about an open release. Bitcoin can never mine one with an input shown spent, so that takes a vote whoever signed it. One with all its signatures can be finished by anyone and the gateway refuses a vote, so it is left. Past the age limit it takes a vote even from a signer that signed it, or a release whose other signers went away could never be cancelled: the gateway counts that vote, and each withdrawal's anchor is what keeps its public signature from paying twice. Otherwise a refusal takes a vote at once, but for three: a bump this signer's Esplora says is needless, which another's may not, one overpriced by a limit with no fee rate behind it, which may be fine, and one this signer signed already, as a signature cannot be taken back and what it would now refuse is usually only the rate having moved, which the age limit settles.",
    returns = "The step.",
    category = "Bridges",
    see_also = "check_btc_release",
)]
pub fn btc_release_step(standing: &ReleaseStanding) -> ReleaseStep {
    match &standing.refusal {
        Some(ReleaseRefusal::Dead(why)) => ReleaseStep::Vote(why.clone()),
        Some(why) if standing.signed_enough => ReleaseStep::Leave(format!("{why}; it holds all its signatures, so it cannot be cancelled")),
        None if standing.signed_enough => ReleaseStep::Nothing,
        _ if standing.past_limit == Some(true) => ReleaseStep::Vote("it is short of its signatures past the age limit".to_string()),
        None => ReleaseStep::Sign,
        Some(why @ ReleaseRefusal::Needless(_)) => ReleaseStep::Leave(format!("{why}; another signer's Bitcoin may say otherwise, so it gets a vote only past the age limit")),
        Some(why @ ReleaseRefusal::Overpriced(_)) if standing.blind => ReleaseStep::Leave(format!("{why}; with no fee rate to go by that may pass, so it gets no vote")),
        Some(why) if standing.signed_by_me => ReleaseStep::Leave(format!("{why}; this signer signed it, so it gets a vote only past the age limit")),
        Some(why) => ReleaseStep::Vote(why.to_string()),
    }
}

#[cfg(feature = "bitcoin")]
#[document(
    summary = "Whether a signer may propose a withdrawal again at a rate, after a release paying it was cancelled at another. Cancelled and proposed again as it was would go round every pass, for as long as the network asks what some signer refuses: so until that release is older than the limit, by the chain's block time, only a rate at least a quarter under the one cancelled goes, which is a different question to put to the signers. A release that was cancelled by its age has passed the limit already, so it holds nothing up.",
    params = {
        rate = "The rate it would be proposed at, in satoshis a virtual byte.",
        cancelled_rate = "What the cancelled release paid.",
        age = "How old that release is, in milliseconds of block time; None if that could not be read, as when the node keeps no event index.",
        limit = "--cancel-after.",
    },
    category = "Bridges",
    see_also = "btc_release_step",
)]
pub fn btc_may_propose_again(rate: u64, cancelled_rate: u64, age: Option<u64>, limit: u64) -> bool {
    age.is_some_and(|age| age >= limit) || rate.saturating_mul(4) <= cancelled_rate.saturating_mul(3)
}

#[cfg(feature = "bitcoin")]
#[document(
    summary = "What a release pays a virtual byte, reckoned as the gateway reckons its size, each signature at its largest.",
    returns = "The rate, rounded down, or why it does not read.",
    category = "Bridges",
)]
pub fn btc_release_rate(facts: &ReleaseFacts) -> Result<u64, String> {
    let (tx, _) = crate::crypto::bitcoin::tx::Transaction::read(&facts.raw)?;
    let spent = facts.inputs.iter().map(|(_, held)| held.sats).sum::<u64>();
    let fee = spent.checked_sub(tx.outputs.iter().map(|output| output.value).sum::<u64>()).ok_or("it pays more than it spends")?;
    let sizes: Vec<(u32, usize)> = facts.inputs.iter().map(|(_, held)| (held.threshold, held.script_len)).collect();

    Ok(fee / btc_release_vbytes(facts.raw.len(), &sizes).max(1))
}

#[cfg(feature = "bitcoin")]
#[document(
    summary = "The line a pass logs for a release with all its signatures that Bitcoin has not mined past the age limit: its id, its newest version Bitcoin holds, what it pays against what the network asks, and how it is bumped.",
    params = {
        id = "The release.",
        txid = "Its transaction, as explorers write it.",
        age = "How old it is, in milliseconds of block time.",
        rate = "What it pays a virtual byte.",
        asked = "What the network asks to mine it soon, if Esplora says.",
    },
    category = "Bridges",
)]
pub fn btc_stuck_warning(id: u64, txid: &str, age: u64, rate: u64, asked: Option<u64>) -> String {
    let asked = asked.map_or("Esplora gives no rate for now".to_string(), |asked| format!("the network asks {asked}"));

    format!("WARNING: release {id}, txid {txid}, has all its signatures and is unconfirmed after {} hours. It pays {rate} satoshis a virtual byte, and {asked}. It cannot be cancelled, as anyone could still broadcast it: the signers' passes bump it, by a replacement or a child-pays-for-parent, while it pays well under that; one its withdrawals' limits hold back waits for their owners to raise them with set-most-fee, or for fees to fall", age / 3_600_000)
}

#[cfg(feature = "bitcoin")]
#[document(
    summary = "Whether a federation signer should sign a release. The gateway checks all of this when it is proposed; a signer goes over it again, as what it signs pays real bitcoin and a fault in the contract should not be enough to take it: each withdrawal named once, paid what it burned less its share of the fee, within what it agreed to lose, and still waiting; each input once, held, pending; only change besides, of the size and to the federation it should be; a fee between the floor and the cap, and at no more a virtual byte than this signer takes; and nothing Bitcoin would refuse or delay.",
    returns = "Why not, or nothing when it may sign.",
    category = "Bridges",
    see_also = "btc_release_step",
)]
pub fn check_btc_release(facts: &ReleaseFacts) -> Result<(), ReleaseRefusal> {
    if let Some((outpoint, _)) = facts.inputs.iter().find(|(_, held)| held.state == "spent") {
        return Err(ReleaseRefusal::Dead(format!("the gateway says its input {outpoint} is spent: Bitcoin can never mine it")));
    }

    let (fee, vbytes) = sound_btc_release(facts).map_err(ReleaseRefusal::Wrong)?;

    if fee > vbytes.saturating_mul(facts.most_rate) {
        return Err(ReleaseRefusal::Overpriced(format!("its fee of {fee} is {} satoshis a virtual byte, past the {} this signer signs for now: give --max-fee-rate to sign it, or the federation cancels it", fee / vbytes.max(1), facts.most_rate)));
    }

    Ok(())
}

/// All of check_btc_release that does not change with time: why not, or its fee and size.
#[cfg(feature = "bitcoin")]
fn sound_btc_release(facts: &ReleaseFacts) -> Result<(u64, u64), String> {
    use crate::crypto::bitcoin::tx::{Transaction, p2wsh};

    let (tx, witnessed) = Transaction::read(&facts.raw)?;

    if witnessed || tx.base() != facts.raw {
        return Err("its transaction is not written as Bitcoin writes it".to_string());
    }

    if crate::crypto::bitcoin::sha256d(&facts.raw).as_slice() != facts.txid.as_slice() {
        return Err("its transaction is not the id the gateway keeps".to_string());
    }

    if tx.version != 2 || tx.lock_time != 0 {
        return Err("it is not version 2 with no lock time".to_string());
    }

    if facts.raw.len() < 65 {
        return Err("it is under the 65 bytes Bitcoin's nodes relay".to_string());
    }

    if tx.inputs.len() != facts.inputs.len() {
        return Err("the gateway's account of its inputs does not match it".to_string());
    }

    let mut spends = std::collections::HashSet::new();
    let mut spent = 0;

    for (input, (outpoint, held)) in tx.inputs.iter().zip(&facts.inputs) {
        if !spends.insert(outpoint) {
            return Err(format!("it spends {outpoint} twice"));
        }

        if input.sequence != BTC_SEQUENCE || !input.script_sig.is_empty() {
            return Err(format!("its input {outpoint} has a delay or a script"));
        }

        if held.state != "pending" {
            return Err(format!("the gateway says its input {outpoint} is {}", held.state));
        }

        if facts.withdrawals.is_empty() && held.version >= facts.version {
            return Err("a sweep of what is no older federation's".to_string());
        }

        spent += held.sats;
    }

    let paid = tx.outputs.iter().map(|output| output.value).sum::<u64>();
    let fee = spent.checked_sub(paid).ok_or("it pays more than it spends")?;
    let mut named = std::collections::HashSet::new();

    if let Some(twice) = facts.withdrawals.iter().find(|withdrawal| !named.insert(withdrawal.id)) {
        return Err(format!("it names withdrawal {} twice", twice.id));
    }

    let change = match &tx.outputs[facts.withdrawals.len().min(tx.outputs.len())..] {
        [] if facts.withdrawals.is_empty() => return Err("it pays nothing".to_string()),
        [] => return Err("it keeps no change to its federation, which a child needs to pay for it".to_string()),
        [change] if change.script == p2wsh(&facts.change_script) && change.value >= BTC_DUST => change.value,
        [_] => return Err("its last output is not change to its federation".to_string()),
        _ => return Err("it pays more than its withdrawals and change".to_string()),
    };
    let burned: u64 = facts.withdrawals.iter().map(|withdrawal| withdrawal.sats).sum();
    let count = facts.withdrawals.len();
    let repays = facts.repay.each(count);
    let repaid: u64 = repays.iter().sum();

    if facts.repay.shares.len() > count {
        return Err("the gateway's account of what it repays does not match it".to_string());
    }

    // What its withdrawals lose between them: the fee, and what topped its change up to dust.
    let cost = match facts.withdrawals.is_empty() {
        true => 0,
        false => {
            let over = spent.checked_sub(burned).ok_or("it spends less than its withdrawals burned")?;

            if change != over.max(BTC_DUST) + repaid {
                return Err("its change is not all it spends past what its withdrawals burned, or dust where that is less, with what they repay".to_string());
            }

            fee + over.max(BTC_DUST) - over
        }
    };

    // What it repays of what children cost the backing goes to the federation, so it could only take
    // from the withdrawals: each within its even share of a budget of at most pct of the fee and of what
    // is owed, and at most most.
    if facts.repay.budget > fee.saturating_mul(facts.repay.pct) / 100 || repaid > facts.repay.owed.min(facts.repay.budget) {
        return Err(format!("it repays {repaid} of a budget of {}, past {} percent of its fee of {fee}, or past what children cost the backing, {}", facts.repay.budget, facts.repay.pct, facts.repay.owed));
    }

    if let Some(at) = (0..count).find(|at| repays[*at] > btc_fee_share(facts.repay.budget, count as u64, *at).min(facts.repay.most)) {
        return Err(format!("withdrawal {} repays {}, past its share of the budget or the {} one withdrawal repays at most", facts.withdrawals[at].id, repays[at], facts.repay.most));
    }

    for (at, withdrawal) in facts.withdrawals.iter().enumerate() {
        if withdrawal.state != "proposed" {
            return Err(format!("withdrawal {} is {}", withdrawal.id, withdrawal.state));
        }

        let output = tx.outputs.get(at).ok_or_else(|| format!("it does not pay withdrawal {}", withdrawal.id))?;

        let share = btc_fee_share(cost, facts.withdrawals.len() as u64, at) + repays[at];

        if share > withdrawal.most_fee_ahead() {
            return Err(format!("its share of the fee is more than withdrawal {} agreed to lose", withdrawal.id));
        }

        if output.script != withdrawal.script || Some(output.value) != withdrawal.sats.checked_sub(share) {
            return Err(format!("it does not pay withdrawal {} as asked", withdrawal.id));
        }

        if let Some(anchor) = &withdrawal.anchor
            && !spends.contains(anchor)
        {
            return Err(format!("withdrawal {} was in a cancelled release this does not conflict with", withdrawal.id));
        }
    }

    let sizes: Vec<(u32, usize)> = facts.inputs.iter().map(|(_, held)| (held.threshold, held.script_len)).collect();
    let least = btc_release_vbytes(facts.raw.len(), &sizes) * facts.fee_floor;

    if fee == 0 || fee > facts.fee_cap || fee < least {
        return Err(format!("its fee of {fee} is not from {least}, the floor for its size, to the cap of {}", facts.fee_cap));
    }

    Ok((fee, btc_release_vbytes(facts.raw.len(), &sizes)))
}

#[cfg(feature = "bitcoin")]
#[document(summary = "How many times the fastest fee rate Bitcoin's network asks a signer signs for: a release proposed a moment ago, or as the network calms, stays well under it.", category = "Bridges")]
pub const BTC_RATE_TIMES_FASTEST: u64 = 3;

#[cfg(feature = "bitcoin")]
#[document(summary = "What a signer signs for whatever the network asks, in sat/vB, so a quiet one, where the fastest rate is 1, does not refuse a release proposed at a normal fee.", category = "Bridges")]
pub const BTC_RATE_ALWAYS_SIGNED: u64 = 25;

#[cfg(feature = "bitcoin")]
#[document(summary = "The most a signer signs for not knowing what the network asks, in sat/vB: a fee rate past any but a rush, so fees going unread neither halt withdrawals nor sign just anything.", category = "Bridges")]
pub const BTC_RATE_BLIND: u64 = 200;

#[cfg(feature = "bitcoin")]
#[document(
    summary = "The most a virtual byte a signer signs a release for: what the operator told it; else a few times the network's fastest rate, at least BTC_RATE_ALWAYS_SIGNED and the gateway's floor; else, with Esplora out of reach, BTC_RATE_BLIND, or the floor should it be higher.",
    params = {
        told = "--max-fee-rate, if given.",
        fastest = "The network's fastest rate from Esplora, if it said one.",
        fee_floor = "The least the gateway lets a release pay a virtual byte.",
    },
    category = "Bridges",
    see_also = "check_btc_release",
)]
pub fn btc_most_fee_rate(told: Option<u64>, fastest: Option<f64>, fee_floor: u64) -> u64 {
    match (told, fastest) {
        (Some(told), _) => told,
        (None, Some(fastest)) => ((fastest.ceil() as u64).saturating_mul(BTC_RATE_TIMES_FASTEST)).max(BTC_RATE_ALWAYS_SIGNED).max(fee_floor),
        (None, None) => BTC_RATE_BLIND.max(fee_floor),
    }
}

#[cfg(feature = "bitcoin")]
#[document(summary = "How much a release proposed from the queue pays.", category = "Bridges", see_also = "plan_btc_release")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReleaseFee {
    #[document(summary = "This many satoshis, as an operator told: it holds or nothing is proposed.")]
    Fixed(u64),
    #[document(summary = "This many satoshis a virtual byte of whatever it comes to, at least the gateway's floor.")]
    Rate(u64),
}

#[cfg(feature = "bitcoin")]
#[document(summary = "What to propose of the queue, and what waits and why.", category = "Bridges", see_also = "plan_btc_release")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleasePlan {
    #[document(summary = "The release, unsigned, if any can be made now.")]
    pub tx: Option<crate::crypto::bitcoin::tx::Transaction>,
    #[document(summary = "The withdrawals it pays, in the order of its outputs.")]
    pub ids: Vec<u64>,
    #[document(summary = "What it pays in fees.")]
    pub fee: u64,
    #[document(summary = "What its withdrawals lose between them: the fee, and what topped its change up to dust, should the gateway hold too little past what they burned.")]
    pub cost: u64,
    #[document(summary = "What each withdrawal it pays repays of what children cost the backing, in order, and the budget that came of.")]
    pub repay: ReleaseRepay,
    #[document(summary = "Each withdrawal it cannot pay now, with why.")]
    pub waiting: Vec<(u64, String)>,
}

/// The most withdrawals one release pays, as the gateway allows.
#[cfg(feature = "bitcoin")]
const BTC_MOST_WITHDRAWALS: usize = 50;
/// The most virtual bytes one release may be, as the gateway allows.
#[cfg(feature = "bitcoin")]
const BTC_MOST_VBYTES: u64 = 100_000;

#[cfg(feature = "bitcoin")]
#[document(
    summary = "What spending one output the gateway holds adds to a release once signed, in virtual bytes: its outpoint, empty script and sequence, and its witness at its largest.",
    params = { threshold = "How many of its federation must sign.", script_len = "The length of the witness script that spends it." },
    category = "Bridges",
    see_also = "btc_release_vbytes",
)]
pub fn btc_input_vbytes(threshold: u32, script_len: usize) -> u64 {
    let witness = 2 + threshold as usize * 74 + if script_len < 0xfd { 1 } else { 3 } + script_len;

    (4 * 41 + witness).div_ceil(4) as u64
}

/// What pays these withdrawals and leaves change of at least dust: what their anchors require, then
/// what the gateway holds, largest first, leaving out what is worth no more than it costs to spend at
/// the rate. Taken in the order held, dust paid to a deposit address ahead of the rest would drag
/// hundreds of inputs into every release, past the cap. Should it hold no more than they burned and
/// less than dust past it, all it holds of worth, the withdrawals topping its change up to dust.
/// None if it holds less than they burned.
#[cfg(feature = "bitcoin")]
fn btc_inputs<'a>(chosen: &[&NamedWithdrawal], held: &'a [(String, SpentOutput)], rate: u64) -> Option<Vec<&'a (String, SpentOutput)>> {
    let burned: u64 = chosen.iter().map(|withdrawal| withdrawal.sats).sum();
    let required = |outpoint: &str| chosen.iter().any(|withdrawal| withdrawal.anchor.as_deref() == Some(outpoint));
    let mut inputs: Vec<_> = held.iter().filter(|(outpoint, _)| required(outpoint)).collect();
    let mut taken: u64 = inputs.iter().map(|(_, output)| output.sats).sum();
    let worth = |output: &SpentOutput| output.sats > btc_input_vbytes(output.threshold, output.script_len).saturating_mul(rate);
    // Largest first, and among equals in the order held, so every signer picks the same.
    let mut rest: Vec<_> = held.iter().filter(|(outpoint, output)| !required(outpoint) && worth(output)).collect();
    rest.sort_by_key(|(_, output)| std::cmp::Reverse(output.sats));

    for output in rest {
        if taken >= burned + BTC_DUST {
            break;
        }

        inputs.push(output);
        taken += output.1.sats;
    }

    (taken >= burned).then_some(inputs)
}

#[cfg(feature = "bitcoin")]
#[document(
    summary = "The release to propose from what is queued, oldest first, and what the gateway holds, largest first, leaving out an output worth no more than it costs to spend at the rate, so dust paid to a deposit address never drags a release past the cap. It is worked out from the gateway's state and the fee alone, so signers doing it at once build the same one, and the first to land wins. A withdrawal it cannot pay now is left out and said why, never holding up the rest: one whose anchor is in another release, one past what the gateway holds, one that agreed to lose less than its share of the fee, by the lowest its limit comes to however late the release lands. A release past the cap at this rate pays fewer, for another to pay the rest.",
    params = {
        queued = "What is queued, oldest first.",
        held = "What the gateway holds unspent, in the order it took it.",
        change = "The output script of the federation in force.",
        fee_cap = "The most fee a release may pay.",
        fee_floor = "The least it may pay a virtual byte.",
        fee = "What it pays.",
        repay = "What is owed past what open releases repay, as owed, and the policy for repaying it.",
    },
    returns = "The plan, or why a fee told cannot be paid.",
    category = "Bridges",
    see_also = "btc_propose_digest, check_btc_release",
)]
#[allow(clippy::too_many_arguments)]
pub fn plan_btc_release(queued: &[NamedWithdrawal], held: &[(String, SpentOutput)], change: &[u8], fee_cap: u64, fee_floor: u64, fee: ReleaseFee, repay: &ReleaseRepay) -> Result<ReleasePlan, String> {
    use crate::crypto::bitcoin::tx::{Transaction, TxIn, TxOut};

    let mut waiting = Vec::new();
    let mut out: std::collections::HashSet<u64> = std::collections::HashSet::new();

    for withdrawal in queued {
        if let Some(anchor) = withdrawal.anchor.as_deref().filter(|anchor| !held.iter().any(|(at, _)| at == anchor)) {
            out.insert(withdrawal.id);
            waiting.push((withdrawal.id, format!("the input it must spend, {anchor}, is in another release")));
        }
    }

    // How many one release may pay: fewer once one this large is past the cap, and those left go in
    // the next, so nothing is said of them.
    let mut most = BTC_MOST_WITHDRAWALS;

    loop {
        let chosen: Vec<&NamedWithdrawal> = queued.iter().filter(|withdrawal| !out.contains(&withdrawal.id)).take(most).collect();

        if chosen.is_empty() {
            return Ok(ReleasePlan { tx: None, ids: Vec::new(), fee: 0, cost: 0, repay: ReleaseRepay::default(), waiting });
        }

        // The rate each input costs at: a fee told pays at least the floor, and may be no more.
        let rate = match fee {
            ReleaseFee::Fixed(_) => fee_floor,
            ReleaseFee::Rate(rate) => rate.max(fee_floor),
        };
        let Some(inputs) = btc_inputs(&chosen, held, rate) else {
            let largest = chosen.iter().rev().max_by_key(|withdrawal| withdrawal.sats).expect("there is one").id;
            out.insert(largest);
            waiting.push((largest, "the gateway holds too little unspent to pay it, for now".to_string()));

            continue;
        };
        let burned: u64 = chosen.iter().map(|withdrawal| withdrawal.sats).sum();
        let taken: u64 = inputs.iter().map(|(_, output)| output.sats).sum();
        let spends = inputs
            .iter()
            .map(|(outpoint, _)| {
                let (txid, vout) = outpoint.split_once(':').ok_or("an outpoint that does not read")?;
                // The gateway keys what it holds by the id in Bitcoin's own byte order.
                let txid: [u8; 32] = crate::structs::hex::from_hex(&format!("0x{txid}")).and_then(|bytes| bytes.try_into().ok()).ok_or("an outpoint that does not read")?;

                Ok(TxIn { txid, vout: vout.parse().map_err(|_| "an outpoint that does not read")?, script_sig: Vec::new(), sequence: BTC_SEQUENCE, witness: Vec::new() })
            })
            .collect::<Result<Vec<_>, &str>>()?;
        let mut outputs: Vec<TxOut> = chosen.iter().map(|withdrawal| TxOut { value: withdrawal.sats, script: withdrawal.script.clone() }).collect();

        // Withdrawals pay their own fee, so all the rest of what it spends is change, and the fee
        // comes off no backing. Change is kept even where that is less than dust, topped up to it,
        // so a child can always spend it to pay for the release.
        let topped = BTC_DUST.saturating_sub(taken - burned);
        outputs.push(TxOut { value: (taken - burned).max(BTC_DUST), script: change.to_vec() });

        let mut tx = Transaction { version: 2, inputs: spends, outputs, lock_time: 0 };
        let sizes: Vec<(u32, usize)> = inputs.iter().map(|(_, output)| (output.threshold, output.script_len)).collect();
        let vbytes = btc_release_vbytes(tx.base().len(), &sizes);
        let paid = match fee {
            ReleaseFee::Fixed(fee) if fee < vbytes.saturating_mul(fee_floor) => return Err(format!("a fee of {fee} is under the floor: this release pays at least {}", vbytes.saturating_mul(fee_floor))),
            ReleaseFee::Fixed(fee) if fee > fee_cap => return Err(format!("a fee of {fee} is past the gateway's cap of {fee_cap}")),
            ReleaseFee::Fixed(fee) => fee,
            ReleaseFee::Rate(rate) => vbytes.saturating_mul(rate.max(fee_floor)),
        };

        if paid > fee_cap || vbytes > BTC_MOST_VBYTES {
            most = chosen.len() - 1;

            if most == 0 {
                waiting.push((chosen[0].id, format!("a release of it alone comes to {vbytes} virtual bytes and a fee of {paid}, past the gateway's limits, until fees fall")));
            }

            continue;
        }

        let count = chosen.len() as u64;
        let cost = paid + topped;
        let short = chosen
            .iter()
            .enumerate()
            .filter(|(at, withdrawal)| btc_fee_share(cost, count, *at) > withdrawal.most_fee_ahead())
            .min_by_key(|(at, withdrawal)| (withdrawal.most_fee_ahead(), std::cmp::Reverse(*at)))
            .map(|(_, withdrawal)| withdrawal);

        if let Some(short) = short {
            out.insert(short.id);
            waiting.push((short.id, format!("it agreed to lose {}, less than its share of a fee of {cost}: its owner raises that with set-most-fee, or it waits for fees to fall", short.most_fee_ahead())));

            continue;
        }

        // What each repays comes out of what its limit leaves, and goes back to the federation's change.
        let rooms: Vec<u64> = chosen.iter().enumerate().map(|(at, withdrawal)| withdrawal.most_fee_ahead() - btc_fee_share(cost, count, at)).collect();
        let (budget, repays) = btc_repays(repay.owed, paid, repay.pct, repay.most, &rooms);

        for (at, output) in tx.outputs.iter_mut().take(chosen.len()).enumerate() {
            output.value -= btc_fee_share(cost, count, at) + repays[at];
        }

        let change_at = chosen.len();
        tx.outputs[change_at].value += repays.iter().sum::<u64>();

        return Ok(ReleasePlan { tx: Some(tx), ids: chosen.iter().map(|withdrawal| withdrawal.id).collect(), fee: paid, cost, repay: ReleaseRepay { shares: repays, budget, ..repay.clone() }, waiting });
    }
}

#[cfg(feature = "bitcoin")]
#[document(
    summary = "A withdrawal's share of its release's fee, as the Bitcoin gateway reckons it: the fee split evenly, the first taking what will not divide.",
    params = { fee = "The release's fee.", count = "How many withdrawals it pays.", at = "This one's place among them, from 0." },
    category = "Bridges",
)]
pub fn btc_fee_share(fee: u64, count: u64, at: usize) -> u64 {
    fee / count + if at == 0 { fee % count } else { 0 }
}

#[cfg(feature = "bitcoin")]
#[document(summary = "What change is at least, in satoshis: Bitcoin's dust for a P2WSH output, under which its nodes relay nothing. Every release keeps change of at least this, its withdrawals topping it up where it would be less, so a child can always spend it to pay for the release.", category = "Bridges")]
pub const BTC_DUST: u64 = 330;

#[cfg(feature = "bitcoin")]
#[document(summary = "A bump to propose, unsigned: a replacement of a release or of a child, or a child paying for a release.", category = "Bridges", see_also = "plan_btc_replacement, plan_btc_child")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BumpPlan {
    #[document(summary = "Its transaction, without witnesses.")]
    pub tx: crate::crypto::bitcoin::tx::Transaction,
    #[document(summary = "What it pays in fees.")]
    pub fee: u64,
    #[document(summary = "What it comes to a virtual byte, rounded down: a child's with what it pays for, as Bitcoin's miners reckon a package.")]
    pub rate: u64,
    #[document(summary = "The withdrawals whose limit kept it under the rate asked: their owners may raise it with set-most-fee while the release is open.")]
    pub held_back: Vec<u64>,
}

/// What a version weighs and pays, from what the gateway says it spends: its fee, what its
/// withdrawals lose between them, and its size once signed.
#[cfg(feature = "bitcoin")]
fn btc_figures(facts: &ReleaseFacts) -> Result<(crate::crypto::bitcoin::tx::Transaction, u64, u64, u64), String> {
    let (tx, _) = crate::crypto::bitcoin::tx::Transaction::read(&facts.raw)?;
    let spent: u64 = facts.inputs.iter().map(|(_, held)| held.sats).sum();
    let fee = spent.checked_sub(tx.outputs.iter().map(|output| output.value).sum()).ok_or("it pays more than it spends")?;
    let burned: u64 = facts.withdrawals.iter().map(|withdrawal| withdrawal.sats).sum();
    let paid: u64 = tx.outputs.iter().take(facts.withdrawals.len()).map(|output| output.value).sum();
    let repaid: u64 = facts.repay.each(facts.withdrawals.len()).iter().sum();
    let sizes: Vec<(u32, usize)> = facts.inputs.iter().map(|(_, held)| (held.threshold, held.script_len)).collect();

    Ok((tx, fee, burned.saturating_sub(paid).saturating_sub(repaid), btc_release_vbytes(facts.raw.len(), &sizes)))
}

#[cfg(feature = "bitcoin")]
#[document(
    summary = "What a release, a replacement or a child pays in fees and weighs once signed, in virtual bytes, as the gateway reckons it: what Bitcoin's miners weigh a child and what it pays for by together.",
    returns = "Its fee and size, or why it does not read.",
    category = "Bridges",
    see_also = "btc_release_rate",
)]
pub fn btc_fee_and_vbytes(facts: &ReleaseFacts) -> Result<(u64, u64), String> {
    let (_, fee, _, vbytes) = btc_figures(facts)?;

    Ok((fee, vbytes))
}

/// The largest of what withdrawals lose between them, up to a limit, whose even share keeps each
/// within its own: every share but the first is the cost over their count, and the first also takes
/// what will not divide, so a few steps down from the bound the others set reach one it takes.
#[cfg(feature = "bitcoin")]
fn btc_most_cost(limits: &[u64], upto: u64) -> u64 {
    let Some(first) = limits.first() else {
        return upto;
    };
    let count = limits.len() as u64;
    let others = limits[1..].iter().min().map_or(u64::MAX, |least| least.saturating_mul(count).saturating_add(count - 1));
    let mut cost = upto.min(others).min(first.saturating_mul(count).saturating_add(count - 1));

    while cost > 0 && limits.iter().enumerate().any(|(at, limit)| btc_fee_share(cost, count, at) > *limit) {
        cost -= 1;
    }

    cost
}

/// A multisig script's threshold, as btc_multisig writes it.
#[cfg(feature = "bitcoin")]
fn btc_threshold(script: &[u8]) -> u32 {
    match script {
        [small @ 0x51..=0x60, ..] => u32::from(small - 0x50),
        [1, number, ..] => u32::from(*number),
        _ => 0,
    }
}

#[cfg(feature = "bitcoin")]
#[document(
    summary = "The change of a release, or of a replacement of one, as a child spends it: its outpoint, as the gateway writes it, and what the gateway says of it before it is shown, paid to the federation the release was proposed under.",
    returns = "The outpoint and the output, or why its transaction does not read.",
    category = "Bridges",
    see_also = "plan_btc_child",
)]
pub fn btc_change_of(parent: &ReleaseFacts) -> Result<(String, SpentOutput), String> {
    let (tx, _) = crate::crypto::bitcoin::tx::Transaction::read(&parent.raw)?;
    let at = parent.withdrawals.len();
    let change = tx.outputs.get(at).ok_or("it keeps no change")?;
    let txid = crate::crypto::bitcoin::sha256d(&parent.raw);

    Ok((
        format!("{}:{at}", crate::structs::hex::to_hex(&txid).trim_start_matches("0x")),
        SpentOutput { sats: change.value, version: parent.version, state: "unshown".to_string(), threshold: btc_threshold(&parent.change_script), script_len: parent.change_script.len() },
    ))
}

#[cfg(feature = "bitcoin")]
#[document(
    summary = "A replacement of a version of a release, or of a child, at a rate: the same inputs and output scripts, more fee. For a release, the change stays and its withdrawals pay what it adds, evenly, each within what it agreed to lose; should one's limit not take its share at that rate, it goes as high as the tightest limit lets it and says which held it back, as a replacement pays exactly the same withdrawals and splits evenly. A sweep's fee comes from its change. It pays past every open version by Bitcoin's incremental rate, and never past the cap.",
    params = {
        facts = "The version it replaces, as the gateway says it stands.",
        most_open_fee = "The most fee any open version of it pays.",
        rate = "What it should pay a virtual byte: what the network asks now.",
    },
    returns = "The replacement, or why none can go.",
    category = "Bridges",
    see_also = "check_btc_replacement, btc_bump_digest",
)]
pub fn plan_btc_replacement(facts: &ReleaseFacts, most_open_fee: u64, rate: u64) -> Result<BumpPlan, String> {
    let (mut tx, fee, cost, vbytes) = btc_figures(facts)?;
    let least = most_open_fee.saturating_add(vbytes * BTC_LEAST_BUMP_RATE).max(vbytes.saturating_mul(facts.fee_floor));
    let wanted = vbytes.saturating_mul(rate).max(least).min(facts.fee_cap);

    if least > facts.fee_cap {
        return Err(format!("it pays {most_open_fee} already: a replacement pays at least {least}, past the cap of {}", facts.fee_cap));
    }

    let count = facts.withdrawals.len();
    let mut held_back = Vec::new();
    let paying = match count {
        // A sweep's fee is the federation's, from its change.
        0 => {
            let change = tx.outputs[0].value;
            let paying = wanted.min((fee + change).saturating_sub(BTC_DUST));
            tx.outputs[0].value = fee + change - paying;

            paying
        }
        _ => {
            // What tops its change up to dust stays as it was, and so does its change.
            // What each repays stays as it was too, the release's, within its limit with the rest.
            let topped = cost.saturating_sub(fee);
            let repays = facts.repay.each(count);
            let limits: Vec<u64> = facts.withdrawals.iter().zip(&repays).map(|(withdrawal, repay)| withdrawal.most_fee.saturating_sub(*repay)).collect();
            let new_cost = btc_most_cost(&limits, wanted + topped);
            held_back = facts.withdrawals.iter().zip(&limits).enumerate().filter(|(at, (_, limit))| btc_fee_share(wanted + topped, count as u64, *at) > **limit).map(|(_, (withdrawal, _))| withdrawal.id).collect();

            for (at, withdrawal) in facts.withdrawals.iter().enumerate() {
                tx.outputs[at].value = withdrawal.sats - btc_fee_share(new_cost, count as u64, at) - repays[at];
            }

            new_cost.saturating_sub(topped)
        }
    };

    if paying < least {
        let who = held_back.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(", ");

        return Err(format!("withdrawal {who} agreed to lose too little for any bump of it: their owners may raise that with set-most-fee"));
    }

    Ok(BumpPlan { tx, fee: paying, rate: paying / vbytes.max(1), held_back })
}

/// What a child may add to what it pays for: each of that one's withdrawals' room under its limit
/// past its share there, and what the cap leaves past that one's fee.
#[cfg(feature = "bitcoin")]
fn btc_child_room(parent: &ReleaseFacts) -> Result<(Vec<u64>, u64, u64, u64), String> {
    let (_, fee, cost, vbytes) = btc_figures(parent)?;
    let count = parent.withdrawals.len() as u64;
    let repays = parent.repay.each(parent.withdrawals.len());
    let room = parent.withdrawals.iter().enumerate().map(|(at, withdrawal)| withdrawal.most_fee.saturating_sub(btc_fee_share(cost, count, at) + repays[at])).collect();

    Ok((room, parent.fee_cap.saturating_sub(fee), fee, vbytes))
}

/// A child's fee at a rate for it and what it pays for together, within what may be charged: that
/// one's fee and size are given as a pair.
#[cfg(feature = "bitcoin")]
fn btc_child_fee(parent: &ReleaseFacts, room: &[u64], most: u64, (parent_fee, parent_vbytes): (u64, u64), vbytes: u64, rate: u64, least: u64) -> (u64, Vec<u64>) {
    let wanted = rate.saturating_mul(parent_vbytes + vbytes).saturating_sub(parent_fee).max(vbytes.saturating_mul(parent.fee_floor)).max(least).max(1);
    let fee = btc_most_cost(room, wanted).min(most);
    let count = room.len() as u64;
    let held_back = parent.withdrawals.iter().zip(room).enumerate().filter(|(at, (_, room))| btc_fee_share(wanted, count, *at) > **room).map(|(_, (withdrawal, _))| withdrawal.id).collect();

    (fee, held_back)
}

#[cfg(feature = "bitcoin")]
#[document(
    summary = "A child paying for a version of a release at a rate for the two together, as Bitcoin's miners reckon a package. It spends that version's change, and only should that not pay its fee and leave dust, what the gateway holds unspent, in the order it took it; and pays the rest to the federation in force. Its fee is charged to the release's withdrawals, evenly, each within what it agreed to lose past its share of the release's own, and the two fees stay under the cap: should a limit hold it under the rate, it goes as high as that lets it and says which.",
    params = {
        parent = "The version it pays for, as the gateway says it stands.",
        held = "What the gateway holds unspent, in the order it took it.",
        change = "The output script of the federation in force.",
        rate = "What the two together should pay a virtual byte.",
    },
    returns = "The child, or why none can go.",
    category = "Bridges",
    see_also = "check_btc_child, btc_change_of, btc_bump_digest",
)]
pub fn plan_btc_child(parent: &ReleaseFacts, held: &[(String, SpentOutput)], change: &[u8], rate: u64) -> Result<BumpPlan, String> {
    use crate::crypto::bitcoin::tx::{Transaction, TxIn, TxOut};

    let (room, most, parent_fee, parent_vbytes) = btc_child_room(parent)?;
    let change_of = btc_change_of(parent)?;
    let mut others = held.iter().filter(|(outpoint, held)| held.state == "unspent" && *outpoint != change_of.0);
    let mut inputs = vec![change_of.clone()];

    loop {
        let spends = inputs
            .iter()
            .map(|(outpoint, _)| {
                let (txid, vout) = outpoint.split_once(':').ok_or("an outpoint that does not read")?;
                let txid: [u8; 32] = crate::structs::hex::from_hex(&format!("0x{txid}")).and_then(|bytes| bytes.try_into().ok()).ok_or("an outpoint that does not read")?;

                Ok(TxIn { txid, vout: vout.parse().map_err(|_| "an outpoint that does not read")?, script_sig: Vec::new(), sequence: BTC_SEQUENCE, witness: Vec::new() })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let spent: u64 = inputs.iter().map(|(_, output)| output.sats).sum();
        let mut tx = Transaction { version: 2, inputs: spends, outputs: vec![TxOut { value: 0, script: change.to_vec() }], lock_time: 0 };
        let sizes: Vec<(u32, usize)> = inputs.iter().map(|(_, output)| (output.threshold, output.script_len)).collect();
        let vbytes = btc_release_vbytes(tx.base().len(), &sizes);
        let (fee, held_back) = btc_child_fee(parent, &room, most, (parent_fee, parent_vbytes), vbytes, rate, 0);

        if fee < vbytes.saturating_mul(parent.fee_floor).max(1) {
            return Err(format!("what may be charged to its withdrawals, {fee}, is under the floor for a child of {vbytes} virtual bytes: their owners may raise their limits with set-most-fee"));
        }

        if spent >= fee + BTC_DUST {
            tx.outputs[0].value = spent - fee;

            return Ok(BumpPlan { tx, fee, rate: (parent_fee + fee) / (parent_vbytes + vbytes), held_back });
        }

        inputs.push(others.next().ok_or("its change and all the gateway holds unspent are too little to pay a child")?.clone());
    }
}

#[cfg(feature = "bitcoin")]
#[document(
    summary = "A replacement of a child, at a rate for it and what it pays for together: the same inputs and output, less to the federation, charged as the child is, and past every open version of the child by Bitcoin's incremental rate.",
    params = {
        parent = "The version the child pays for.",
        child = "The version of the child it replaces.",
        most_open_fee = "The most fee any open version of the child pays.",
        rate = "What the two together should pay a virtual byte.",
    },
    returns = "The replacement, or why none can go.",
    category = "Bridges",
    see_also = "plan_btc_child, check_btc_child",
)]
pub fn plan_btc_child_replacement(parent: &ReleaseFacts, child: &ReleaseFacts, most_open_fee: u64, rate: u64) -> Result<BumpPlan, String> {
    let (room, most, parent_fee, parent_vbytes) = btc_child_room(parent)?;
    let (mut tx, _, _, vbytes) = btc_figures(child)?;
    let spent: u64 = child.inputs.iter().map(|(_, held)| held.sats).sum();
    let least = most_open_fee + vbytes * BTC_LEAST_BUMP_RATE;
    let (fee, held_back) = btc_child_fee(parent, &room, most.min(spent.saturating_sub(BTC_DUST)), (parent_fee, parent_vbytes), vbytes, rate, least);

    if fee < least {
        return Err(format!("a replacement of the child pays at least {least}, past what may be charged to its withdrawals or the cap"));
    }

    tx.outputs[0].value = spent - fee;

    Ok(BumpPlan { tx, fee, rate: (parent_fee + fee) / (parent_vbytes + vbytes), held_back })
}

#[cfg(feature = "bitcoin")]
#[document(
    summary = "Whether a federation signer should sign a replacement: what check_btc_release asks of any release, and that it spends just what the version it replaces does, in the same order, pays the same scripts, keeps a release's change as it was, and pays past every open version by Bitcoin's incremental rate. Whether the release is stuck at all is the signer's to ask Bitcoin.",
    params = {
        prior = "The version it replaces.",
        next = "The replacement.",
        most_open_fee = "The most fee any other open version of it pays.",
    },
    returns = "Why not, or nothing when it may sign.",
    category = "Bridges",
    see_also = "plan_btc_replacement, btc_release_step",
)]
pub fn check_btc_replacement(prior: &ReleaseFacts, next: &ReleaseFacts, most_open_fee: u64) -> Result<(), ReleaseRefusal> {
    check_btc_release(next)?;

    let wrong = |why: &str| Err(ReleaseRefusal::Wrong(why.to_string()));
    let (old, _, _, _) = btc_figures(prior).map_err(ReleaseRefusal::Wrong)?;
    let (new, fee, _, vbytes) = btc_figures(next).map_err(ReleaseRefusal::Wrong)?;
    let spends = |tx: &crate::crypto::bitcoin::tx::Transaction| tx.inputs.iter().map(|input| (input.txid, input.vout)).collect::<Vec<_>>();
    let scripts = |tx: &crate::crypto::bitcoin::tx::Transaction| tx.outputs.iter().map(|output| output.script.clone()).collect::<Vec<_>>();

    if spends(&old) != spends(&new) || scripts(&old) != scripts(&new) || prior.withdrawals.iter().map(|withdrawal| withdrawal.id).ne(next.withdrawals.iter().map(|withdrawal| withdrawal.id)) {
        return wrong("it does not spend what it replaces, or pay the same outputs");
    }

    if !next.withdrawals.is_empty() && old.outputs.last().map(|output| output.value) != new.outputs.last().map(|output| output.value) {
        return wrong("it does not keep the change of what it replaces");
    }

    if prior.repay.each(prior.withdrawals.len()) != next.repay.each(next.withdrawals.len()) || prior.repay.budget != next.repay.budget {
        return wrong("it does not repay what the release it replaces repays");
    }

    if fee < most_open_fee + vbytes * BTC_LEAST_BUMP_RATE {
        return Err(ReleaseRefusal::Wrong(format!("its fee of {fee} is not past every version it replaces, {most_open_fee}, by a satoshi a virtual byte")));
    }

    Ok(())
}

#[cfg(feature = "bitcoin")]
#[document(
    summary = "Whether a federation signer should sign a child, or a replacement of one: written as Bitcoin writes it, with the id the gateway keeps; version 2, no lock time, each input signalling BIP125; the version it pays for's change first, then held outputs only should that change not pay its fee and leave dust, each once and pending; one output, to the federation in force, of at least dust; a fee at the floor for its size, past every open version of it by Bitcoin's incremental rate, under the cap with what it pays for's, and chargeable to that one's withdrawals within what each agreed to lose; and the two together at no more a virtual byte than this signer takes.",
    params = {
        parent = "The version it pays for.",
        child = "The child, as the gateway says it stands, with what it spends and the federation it pays.",
        most_open_fee = "For a replacement of a child, the most fee any other open version of it pays.",
    },
    returns = "Why not, or nothing when it may sign.",
    category = "Bridges",
    see_also = "plan_btc_child, btc_release_step",
)]
pub fn check_btc_child(parent: &ReleaseFacts, child: &ReleaseFacts, most_open_fee: Option<u64>) -> Result<(), ReleaseRefusal> {
    use crate::crypto::bitcoin::tx::{Transaction, p2wsh};

    if let Some((outpoint, _)) = child.inputs.iter().find(|(_, held)| held.state == "spent") {
        return Err(ReleaseRefusal::Dead(format!("the gateway says its input {outpoint} is spent: Bitcoin can never mine it")));
    }

    let wrong = |why: String| Err(ReleaseRefusal::Wrong(why));
    let (tx, witnessed) = Transaction::read(&child.raw).map_err(ReleaseRefusal::Wrong)?;

    if witnessed || tx.base() != child.raw || crate::crypto::bitcoin::sha256d(&child.raw).as_slice() != child.txid.as_slice() {
        return wrong("its transaction is not written as Bitcoin writes it, or not the id the gateway keeps".to_string());
    }

    if tx.version != 2 || tx.lock_time != 0 || child.raw.len() < 65 || !child.withdrawals.is_empty() {
        return wrong("it is not a child as the gateway takes one: version 2, no lock time, paying no withdrawal".to_string());
    }

    let (change, _) = btc_change_of(parent).map_err(ReleaseRefusal::Wrong)?;
    let mut spends = std::collections::HashSet::new();

    if tx.inputs.len() != child.inputs.len() || child.inputs.first().map(|(outpoint, _)| outpoint) != Some(&change) {
        return wrong(format!("it does not spend {change}, the change of what it pays for, first"));
    }

    for (input, (outpoint, held)) in tx.inputs.iter().zip(&child.inputs) {
        if !spends.insert(outpoint) || input.sequence != BTC_SEQUENCE || !input.script_sig.is_empty() || held.state != "pending" {
            return wrong(format!("its input {outpoint} is spent twice, has a delay or a script, or is {}", held.state));
        }
    }

    let spent: u64 = child.inputs.iter().map(|(_, held)| held.sats).sum();
    let paid = match tx.outputs.as_slice() {
        [output] if output.script == p2wsh(&child.change_script) && output.value >= BTC_DUST => output.value,
        _ => return wrong("it does not pay the federation in force alone, at least dust".to_string()),
    };
    let fee = spent.checked_sub(paid).filter(|fee| *fee > 0).ok_or(ReleaseRefusal::Wrong("it pays no fee".to_string()))?;

    if child.inputs.len() > 1 && child.inputs[0].1.sats >= fee + BTC_DUST {
        return wrong("it spends more than the change it pays for with, which pays its fee alone".to_string());
    }

    let sizes: Vec<(u32, usize)> = child.inputs.iter().map(|(_, held)| (held.threshold, held.script_len)).collect();
    let vbytes = btc_release_vbytes(child.raw.len(), &sizes);
    let (room, most, parent_fee, parent_vbytes) = btc_child_room(parent).map_err(ReleaseRefusal::Wrong)?;

    if fee < vbytes * child.fee_floor || fee > most {
        return Err(ReleaseRefusal::Wrong(format!("its fee of {fee} is under the floor for its size, or past the cap with what it pays for's")));
    }

    if btc_most_cost(&room, fee) < fee {
        return wrong(format!("its fee of {fee} is more than what it pays for's withdrawals agreed to lose, with their shares of that one's"));
    }

    if let Some(most_open_fee) = most_open_fee
        && fee < most_open_fee + vbytes * BTC_LEAST_BUMP_RATE
    {
        return Err(ReleaseRefusal::Wrong(format!("its fee of {fee} is not past every version it replaces, {most_open_fee}, by a satoshi a virtual byte")));
    }

    let rate = (parent_fee + fee) / (parent_vbytes + vbytes);

    if parent_fee + fee > (parent_vbytes + vbytes).saturating_mul(child.most_rate) {
        return Err(ReleaseRefusal::Overpriced(format!("with what it pays for it pays {rate} satoshis a virtual byte, past the {} this signer signs for now", child.most_rate)));
    }

    Ok(())
}

#[cfg(feature = "bitcoin")]
#[document(
    summary = "Whether a release with all its signatures is stuck, so a signer's pass bumps it: unmined, as the caller knows, for at least the delay since its newest bump, or since it was proposed, by the chain's block time; and paying, a child's with what it pays for, under the percent of what the network asks that the federation's policy says. With no age, or no rate from the network, nothing says it is stuck, and nothing is bumped by policy: an operator still may by hand.",
    params = {
        age = "How long since its newest version or child was proposed, in milliseconds of block time; None if that could not be read.",
        rate = "What it pays a virtual byte, with any child of it in Bitcoin's mempool.",
        asked = "What the network asks now, from Esplora, if it says.",
        bump_after_ms = "btc.bump_after_ms.",
        bump_below_pct = "btc.bump_below_pct.",
    },
    category = "Bridges",
    see_also = "btc_bump_kind",
)]
pub fn btc_is_stuck(age: Option<u64>, rate: u64, asked: Option<u64>, bump_after_ms: u64, bump_below_pct: u64) -> bool {
    age.is_some_and(|age| age >= bump_after_ms) && asked.is_some_and(|asked| rate.saturating_mul(100) < asked.saturating_mul(bump_below_pct))
}

#[cfg(feature = "bitcoin")]
#[document(summary = "Which bump a stuck release gets.", category = "Bridges", see_also = "btc_bump_kind")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BumpKind {
    #[document(summary = "A replacement of its newest version, paying more: what Bitcoin's nodes take by default.")]
    Replace,
    #[document(summary = "A child spending the change of the version in Bitcoin's mempool, paying for both.")]
    Child,
    #[document(summary = "A replacement of the child paying for it, paying more.")]
    ReplaceChild,
}

#[cfg(feature = "bitcoin")]
#[document(
    summary = "Which bump a stuck release gets. Replace-by-fee first, as it costs the federation nothing; a child once Esplora refused the replacement, as when a recipient pinned it with a large child of their own, or once a replacement was proposed and is stuck in turn; and a child already paying for it is replaced, as a second child of the same change would conflict with it.",
    params = {
        child_open = "Whether a child pays for the version in Bitcoin's mempool.",
        replaced = "Whether that version is itself a replacement.",
        refused = "Whether Esplora refused its newest replacement.",
    },
    category = "Bridges",
    see_also = "btc_is_stuck",
)]
pub fn btc_bump_kind(child_open: bool, replaced: bool, refused: bool) -> BumpKind {
    match (child_open, replaced || refused) {
        (true, _) => BumpKind::ReplaceChild,
        (false, true) => BumpKind::Child,
        (false, false) => BumpKind::Replace,
    }
}

#[cfg(feature = "bitcoin")]
#[document(summary = "The least one turn is, in milliseconds, however fast the chain: on one faster than this, a block passes before the first signer's transaction reaches the mempool.", category = "Bridges", see_also = "btc_turn_delay")]
pub const BTC_LEAST_TURN_MS: u64 = 1_000;

#[cfg(feature = "bitcoin")]
#[document(
    summary = "How long a signer waits before it sends what any one signer can, a released, a vote to cancel, a proposal or a bump, in milliseconds: its place among the federation's signers times one block of the chain, at least BTC_LEAST_TURN_MS. The first sends at once, and each after it checks again a turn later, so what the one before sent has landed or waits in the mempool, and only one of them pays for it, unless those before it are down.",
    params = { place = "Its place among the federation in force's signers, from 0.", block_ms = "The chain's block time, from btc_block_ms." },
    category = "Bridges",
    see_also = "btc_block_ms",
)]
pub fn btc_turn_delay(place: u64, block_ms: u64) -> u64 {
    place.saturating_mul(block_ms.max(BTC_LEAST_TURN_MS))
}

#[cfg(feature = "bitcoin")]
#[document(
    summary = "The chain's block time, in milliseconds, from two of its blocks: the time between them over the blocks between them, at least a millisecond and at most ten minutes; five seconds should they be the same block.",
    params = { newer = "The newer block's height and time.", older = "The older block's height and time." },
    category = "Bridges",
    see_also = "btc_turn_delay",
)]
pub fn btc_block_ms(newer: (u64, u64), older: (u64, u64)) -> u64 {
    match newer.0.checked_sub(older.0).filter(|blocks| *blocks > 0) {
        Some(blocks) => (newer.1.saturating_sub(older.1) / blocks).clamp(1, 600_000),
        None => 5_000,
    }
}

#[document(summary = "What both gateways hash a transfer's digest under.", category = "Bridges")]
pub const DOMAIN: &str = "chainv4-gateway";

#[document(summary = "What the gateways hash a void's digest under.", category = "Bridges")]
pub const VOID_DOMAIN: &str = "chainv4-gateway-void";

#[document(summary = "What the gateways hash a refund's digest under.", category = "Bridges")]
pub const REFUND_DOMAIN: &str = "chainv4-gateway-refund";

#[document(summary = "Gateway.sol's gateway, as solc wrote it for Paris: what chainv4's tools deploy on an EVM chain, its constructor's arguments after it.", category = "Bridges")]
pub const GATEWAY_CODE: &str = include_str!("../contracts/evm/Gateway.bin");

#[document(
    summary = "The templates the EVM gateway clones each wrapped asset from, deployed once before it: a wrapped ERC-20, ERC-721 and ERC-1155.",
    category = "Bridges",
)]
pub const WRAPPED_CODES: [&str; 3] = [
    include_str!("../contracts/evm/WrappedToken.bin"),
    include_str!("../contracts/evm/WrappedNft.bin"),
    include_str!("../contracts/evm/WrappedMulti.bin"),
];

#[cfg(feature = "bitcoin")]
#[document(
    summary = "Which Bitcoin a gateway's relay follows, as its network_name says: what its addresses are written for, and the only addresses a withdrawal may pay. Main's are bc1…, the test networks' tb1… and regtest's bcrt1….",
    category = "Bridges",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BtcNetwork {
    #[document(summary = "Bitcoin itself.")]
    Main,
    #[document(summary = "Testnet3, testnet4 and signet, which write addresses alike.")]
    Test,
    #[document(summary = "A regtest network of one's own.")]
    Regtest,
}

#[cfg(feature = "bitcoin")]
impl BtcNetwork {
    #[document(parent = "BtcNetwork", summary = "The network a relay's network_name names: main, test or regtest.")]
    pub fn from_name(name: &str) -> Option<BtcNetwork> {
        match name.trim() {
            "main" => Some(BtcNetwork::Main),
            "test" => Some(BtcNetwork::Test),
            "regtest" => Some(BtcNetwork::Regtest),
            _ => None,
        }
    }

    #[document(parent = "BtcNetwork", summary = "Its name, as from_name reads it.")]
    pub fn name(self) -> &'static str {
        match self {
            BtcNetwork::Main => "main",
            BtcNetwork::Test => "test",
            BtcNetwork::Regtest => "regtest",
        }
    }

    #[document(parent = "BtcNetwork", summary = "The human-readable part of its segwit addresses: bc, tb or bcrt.")]
    pub fn hrp(self) -> &'static str {
        match self {
            BtcNetwork::Main => "bc",
            BtcNetwork::Test => "tb",
            BtcNetwork::Regtest => "bcrt",
        }
    }

    #[document(parent = "BtcNetwork", summary = "An output script's address on this network. Regtest's witness programs are bcrt1…, its others as the test networks write them.")]
    pub fn address(self, script: &[u8]) -> String {
        use crate::crypto::bitcoin::tx::{Network, address};

        match (self, script) {
            (BtcNetwork::Main, _) => address(script, Network::Main),
            (BtcNetwork::Regtest, [0x00, size @ (0x14 | 0x20), program @ ..]) if program.len() == usize::from(*size) => crate::structs::bech32::encode_segwit("bcrt", program),
            (BtcNetwork::Regtest, [version @ 0x51..=0x60, size @ 2..=40, program @ ..]) if program.len() == usize::from(*size) => crate::structs::bech32::encode_witness("bcrt", version - 0x50, program),
            _ => address(script, Network::Test),
        }
    }

    #[document(
        parent = "BtcNetwork",
        summary = "The output script a withdrawal pays, from a native segwit address on this network or a script given as 0x and its hex. An address on another network is refused, as bitcoin sent to it would be paid where no one meant.",
    )]
    pub fn output_script(self, text: &str) -> Result<Vec<u8>, String> {
        let text = text.trim();

        if text.starts_with("0x") {
            return crate::structs::hex::from_hex(text).filter(|script| (1..=80).contains(&script.len())).ok_or_else(|| "the script is not hex, or not 1 to 80 bytes".to_string());
        }

        let (hrp, program) = crate::structs::bech32::decode_segwit(text).ok_or_else(|| format!("give a native segwit address, {}1q…, or an output script, 0x…", self.hrp()))?;

        if hrp != self.hrp() {
            let theirs = [BtcNetwork::Main, BtcNetwork::Test, BtcNetwork::Regtest].into_iter().find(|network| network.hrp() == hrp).map_or_else(|| format!("{hrp}1… is no Bitcoin network's address"), |network| format!("{hrp}1… is an address on Bitcoin's {} network", network.name()));

            return Err(format!("{theirs}, and the gateway pays out on Bitcoin's {} network, whose addresses are {}1…", self.name(), self.hrp()));
        }

        Ok([&[0x00, program.len() as u8][..], &program].concat())
    }
}

#[document(
    summary = "What a transfer moves.",
    category = "Bridges",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssetKind {
    #[document(summary = "A chain's coin: DEV here, ETH on Ethereum.")]
    Coin = 0,
    #[document(summary = "A fungible token: a token-standard token here, an ERC-20 there.")]
    Token = 1,
    #[document(summary = "One NFT: an NFT collection's here, an ERC-721's there.")]
    Nft = 2,
    #[document(summary = "An amount of one id: a multi-token collection's here, an ERC-1155's there.")]
    Multi = 3,
}

impl AssetKind {
    #[document(parent = "AssetKind", summary = "The kind its number names, as the gateways write it.")]
    pub fn from_number(number: u64) -> Option<AssetKind> {
        Some(match number {
            0 => AssetKind::Coin,
            1 => AssetKind::Token,
            2 => AssetKind::Nft,
            3 => AssetKind::Multi,
            _ => return None,
        })
    }
}

#[document(
    summary = "A transfer between two gateways, as each writes it and the federation signs it. Chains are named by their EVM chain ids, chainv4's included; addresses on chainv4 as their 33 bytes (Address::to_abi), an EVM chain's as their 20, and a gateway on chainv4 as its identity().",
    category = "Bridges",
    see_also = "Transfer::digest",
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transfer {
    #[document(summary = "The gateway it leaves from, on the source chain: 20 bytes on an EVM chain, a gateway here as its identity(), its 33 bytes and when it was set up.")]
    pub from: Vec<u8>,
    #[document(summary = "The gateway it arrives at, on the target chain, written as from is.")]
    pub to: Vec<u8>,
    pub source: u64,
    pub target: u64,
    #[document(summary = "The source gateway's count of transfers it sent, from 1.")]
    pub nonce: u64,
    pub kind: AssetKind,
    #[document(summary = "The chain the asset is native to.")]
    pub origin: u64,
    #[document(summary = "The asset's address on its origin chain; empty for a coin.")]
    pub asset: Vec<u8>,
    #[document(summary = "An NFT's or multi-token id's id; 0 otherwise.")]
    pub id: u256,
    #[document(summary = "How much: 1 for an NFT.")]
    pub amount: u256,
    #[document(summary = "Who gets it on the target chain.")]
    pub recipient: Vec<u8>,
    pub name: String,
    pub symbol: String,
    pub decimals: u8,
    #[document(summary = "An NFT's or multi-token id's URI on its origin chain; empty otherwise.")]
    pub uri: String,
}

fn keccak(bytes: &[u8]) -> [u8; 32] {
    Hasher::digest(HashType::Keccak256, bytes).to_bytes().try_into().expect("32 bytes")
}

impl Transfer {
    #[document(
        parent = "Transfer",
        summary = "Whether it is for this gateway on this chain, from its peer: what Gateway.sol's logs still hold after a chain's reset is for another, and no signer's business.",
        params = { here = "This chain's id.", identity = "The gateway here, as its identity() writes it.", peer = "The gateway it left, on its chain." },
    )]
    pub fn arrives_at(&self, here: u64, identity: &[u8], peer: &[u8]) -> bool {
        self.target == here && self.to == identity && self.from == peer
    }

    #[document(
        parent = "Transfer",
        summary = "The key Gateway.sol marks it delivered by: keccak256 of its source chain, its source gateway's hash and its nonce.",
    )]
    pub fn delivered_key(&self) -> Vec<u8> {
        let values = [AbiValue::Uint(u256::from(self.source)), AbiValue::FixedBytes(keccak(&self.from).to_vec()), AbiValue::Uint(u256::from(self.nonce))];

        keccak(&encode_values(&[AbiType::Uint(64), AbiType::FixedBytes(32), AbiType::Uint(64)], &values).expect("a key encodes")).to_vec()
    }

    #[document(
        parent = "Transfer",
        summary = "What the federation signs: keccak256 of the domain, the head (gateways, chains, nonce, kind and origin, hashed) and the body (asset, id, amount, recipient and names, hashed), ABI-encoded, as both gateways compute it.",
    )]
    pub fn digest(&self) -> [u8; 32] {
        let number = |value: u64| AbiValue::Uint(u256::from(value));
        let head = encode_values(
            &[AbiType::Bytes, AbiType::Bytes, AbiType::Uint(64), AbiType::Uint(64), AbiType::Uint(64), AbiType::Uint(8), AbiType::Uint(64)],
            &[
                AbiValue::Bytes(self.from.clone()),
                AbiValue::Bytes(self.to.clone()),
                number(self.source),
                number(self.target),
                number(self.nonce),
                number(self.kind as u64),
                number(self.origin),
            ],
        )
        .expect("a head encodes");
        let body = encode_values(
            &[AbiType::Bytes, AbiType::Uint(256), AbiType::Uint(256), AbiType::Bytes, AbiType::String, AbiType::String, AbiType::Uint(8), AbiType::String],
            &[
                AbiValue::Bytes(self.asset.clone()),
                AbiValue::Uint(self.id),
                AbiValue::Uint(self.amount),
                AbiValue::Bytes(self.recipient.clone()),
                AbiValue::String(self.name.clone()),
                AbiValue::String(self.symbol.clone()),
                number(u64::from(self.decimals)),
                AbiValue::String(self.uri.clone()),
            ],
        )
        .expect("a body encodes");
        let outer = encode_values(
            &[AbiType::String, AbiType::FixedBytes(32), AbiType::FixedBytes(32)],
            &[AbiValue::String(DOMAIN.to_string()), AbiValue::FixedBytes(keccak(&head).to_vec()), AbiValue::FixedBytes(keccak(&body).to_vec())],
        )
        .expect("a digest encodes");

        keccak(&outer)
    }

    #[document(
        parent = "Transfer",
        summary = "What the federation signs to void it, at the gateway it was going to, so it can never be delivered: keccak256 of its own domain, its two gateways and chains, its nonce and its digest, as both gateways compute it. Its domain and layout are its own, so no signature over a transfer or a signer set passes for one, or one for them.",
    )]
    pub fn void_digest(&self) -> [u8; 32] {
        self.signed_over(VOID_DOMAIN, None)
    }

    #[document(
        parent = "Transfer",
        summary = "What the federation signs to refund it, at the gateway it left, once its signers read it voided where it was going: as void_digest, under its own domain, and with whom that gateway says sent it, as that chain writes an address.",
    )]
    pub fn refund_digest(&self, sender: &[u8]) -> [u8; 32] {
        self.signed_over(REFUND_DOMAIN, Some(sender))
    }

    fn signed_over(&self, domain: &str, sender: Option<&[u8]>) -> [u8; 32] {
        let number = |value: u64| AbiValue::Uint(u256::from(value));
        let mut layout = vec![AbiType::String, AbiType::Bytes, AbiType::Bytes, AbiType::Uint(64), AbiType::Uint(64), AbiType::Uint(64), AbiType::FixedBytes(32)];
        let mut values = vec![AbiValue::String(domain.to_string()), AbiValue::Bytes(self.from.clone()), AbiValue::Bytes(self.to.clone()), number(self.source), number(self.target), number(self.nonce), AbiValue::FixedBytes(self.digest().to_vec())];

        if let Some(sender) = sender {
            layout.push(AbiType::Bytes);
            values.push(AbiValue::Bytes(sender.to_vec()));
        }

        keccak(&encode_values(&layout, &values).expect("a void or a refund encodes"))
    }

    #[document(
        parent = "Transfer",
        summary = "The arguments the gateway here's attest_void takes for it, the signature last.",
    )]
    pub fn attest_void_args(&self, signature: &[u8; 65]) -> Vec<TxCallArgumentType> {
        vec![TxCallArgumentType::U64(self.source), TxCallArgumentType::U64(self.target), TxCallArgumentType::U64(self.nonce), TxCallArgumentType::Bytes(self.digest().to_vec()), TxCallArgumentType::Bytes(signature.to_vec())]
    }

    #[document(
        parent = "Transfer",
        summary = "The arguments the gateway here's attest_refund takes for it, whom it says sent it and the signature last.",
    )]
    pub fn attest_refund_args(&self, sender: &[u8], signature: &[u8; 65]) -> Vec<TxCallArgumentType> {
        vec![TxCallArgumentType::U64(self.source), TxCallArgumentType::U64(self.target), TxCallArgumentType::U64(self.nonce), TxCallArgumentType::Bytes(self.digest().to_vec()), TxCallArgumentType::Bytes(sender.to_vec()), TxCallArgumentType::Bytes(signature.to_vec())]
    }

    #[document(
        parent = "Transfer",
        summary = "A signer's signature over its digest: r, s and v, 65 bytes, v 27 or 28, as ecrecover takes it and the gateway here recovers.",
    )]
    pub fn sign(&self, secret: &[u8; 32]) -> [u8; 65] {
        sign_digest(secret, &self.digest())
    }

    #[document(
        parent = "Transfer",
        summary = "The arguments the gateway here's attest takes for it, the signature last: what a signer's transaction carries.",
    )]
    pub fn attest_args(&self, signature: &[u8; 65]) -> Vec<TxCallArgumentType> {
        let number = |value: u64| TxCallArgumentType::U64(value);

        vec![
            number(self.source),
            number(self.target),
            number(self.nonce),
            number(self.kind as u64),
            number(self.origin),
            TxCallArgumentType::Bytes(self.asset.clone()),
            TxCallArgumentType::U256(self.id),
            TxCallArgumentType::U256(self.amount),
            TxCallArgumentType::Bytes(self.recipient.clone()),
            TxCallArgumentType::String(self.name.clone()),
            TxCallArgumentType::String(self.symbol.clone()),
            number(u64::from(self.decimals)),
            TxCallArgumentType::String(self.uri.clone()),
            TxCallArgumentType::Bytes(signature.to_vec()),
        ]
    }
}

#[document(summary = "A transfer as Gateway.sol's functions take and give it: one ABI tuple.", category = "Bridges")]
pub const TRANSFER_TUPLE: &str = "(bytes,bytes,uint64,uint64,uint64,uint8,uint64,bytes,uint256,uint256,bytes,string,string,uint8,string)";

fn selector(signature: &str) -> Vec<u8> {
    keccak(signature.as_bytes())[..4].to_vec()
}

fn types(signature: &str) -> Vec<AbiType> {
    crate::abi::parse_signature(signature).expect("the gateway's signatures parse").1
}

impl Transfer {
    #[document(parent = "Transfer", summary = "It as Gateway.sol's Transfer tuple.")]
    pub fn evm_tuple(&self) -> AbiValue {
        let number = |value: u64| AbiValue::Uint(u256::from(value));

        AbiValue::Tuple(vec![
            AbiValue::Bytes(self.from.clone()),
            AbiValue::Bytes(self.to.clone()),
            number(self.source),
            number(self.target),
            number(self.nonce),
            number(self.kind as u64),
            number(self.origin),
            AbiValue::Bytes(self.asset.clone()),
            AbiValue::Uint(self.id),
            AbiValue::Uint(self.amount),
            AbiValue::Bytes(self.recipient.clone()),
            AbiValue::String(self.name.clone()),
            AbiValue::String(self.symbol.clone()),
            number(u64::from(self.decimals)),
            AbiValue::String(self.uri.clone()),
        ])
    }

    #[document(parent = "Transfer", summary = "It from Gateway.sol's Transfer tuple, as its sent(nonce) returns it.", returns = "The transfer, or None for values that are not one.")]
    pub fn from_evm(value: &AbiValue) -> Option<Transfer> {
        let AbiValue::Tuple(fields) = value else {
            return None;
        };
        let bytes = |index: usize| match fields.get(index)? {
            AbiValue::Bytes(bytes) => Some(bytes.clone()),
            _ => None,
        };
        let wide = |index: usize| match fields.get(index)? {
            AbiValue::Uint(number) => Some(*number),
            _ => None,
        };
        let small = |index: usize| wide(index).and_then(|number| u64::try_from(number).ok());
        let text = |index: usize| match fields.get(index)? {
            AbiValue::String(text) => Some(text.clone()),
            _ => None,
        };

        Some(Transfer {
            from: bytes(0)?,
            to: bytes(1)?,
            source: small(2)?,
            target: small(3)?,
            nonce: small(4)?,
            kind: AssetKind::from_number(small(5)?)?,
            origin: small(6)?,
            asset: bytes(7)?,
            id: wide(8)?,
            amount: wide(9)?,
            recipient: bytes(10)?,
            name: text(11)?,
            symbol: text(12)?,
            decimals: u8::try_from(small(13)?).ok()?,
            uri: text(14)?,
        })
    }

    #[document(parent = "Transfer", summary = "Calldata voiding it at Gateway.sol with the federation's signatures over void_digest: what a relayer sends.")]
    pub fn void_input(&self, signatures: &[[u8; 65]]) -> Vec<u8> {
        let signature = "voidTransfer(uint64,bytes,uint64,bytes32,bytes[])";
        let values = [
            AbiValue::Uint(u256::from(self.source)),
            AbiValue::Bytes(self.from.clone()),
            AbiValue::Uint(u256::from(self.nonce)),
            AbiValue::FixedBytes(self.digest().to_vec()),
            AbiValue::Array(signatures.iter().map(|signature| AbiValue::Bytes(signature.to_vec())).collect()),
        ];

        [selector(signature), encode_values(&types(signature), &values).expect("a void encodes")].concat()
    }

    #[document(parent = "Transfer", summary = "Calldata delivering it to Gateway.sol with the federation's signatures: what a relayer sends.")]
    pub fn deliver_input(&self, signatures: &[[u8; 65]]) -> Vec<u8> {
        let signature = format!("deliver({TRANSFER_TUPLE},bytes[])");
        let values = [self.evm_tuple(), AbiValue::Array(signatures.iter().map(|signature| AbiValue::Bytes(signature.to_vec())).collect())];

        [selector(&signature), encode_values(&types(&signature), &values).expect("a delivery encodes")].concat()
    }

    #[document(
        parent = "Transfer",
        summary = "Calldata delivering it to Gateway.sol by a validity proof: its path in the withdrawals tree of the proven range ending at end_block, as ChainV4Finality.sol holds the range's root, with the federation's signatures too when chainv4's governance says both, or none.",
        see_also = "withdrawal_proof",
    )]
    pub fn deliver_proven_input(&self, end_block: [u8; 32], path: &[[u8; 32]], signatures: &[[u8; 65]]) -> Vec<u8> {
        let signature = format!("deliverProven({TRANSFER_TUPLE},bytes32,bytes32[],bytes[])");
        let values = [
            self.evm_tuple(),
            AbiValue::FixedBytes(end_block.to_vec()),
            AbiValue::Array(path.iter().map(|node| AbiValue::FixedBytes(node.to_vec())).collect()),
            AbiValue::Array(signatures.iter().map(|signature| AbiValue::Bytes(signature.to_vec())).collect()),
        ];

        [selector(&signature), encode_values(&types(&signature), &values).expect("a delivery encodes")].concat()
    }
}

#[document(summary = "Calldata refunding what left Gateway.sol at a nonce, with the federation's signatures over Transfer::refund_digest: what and to whom are what it recorded, so that is all it takes.", category = "Bridges")]
pub fn refund_input(nonce: u64, signatures: &[[u8; 65]]) -> Vec<u8> {
    let signature = "refund(uint64,bytes[])";
    let values = [AbiValue::Uint(u256::from(nonce)), AbiValue::Array(signatures.iter().map(|signature| AbiValue::Bytes(signature.to_vec())).collect())];

    [selector(signature), encode_values(&types(signature), &values).expect("a refund encodes")].concat()
}

#[document(
    summary = "Calldata sending something through Gateway.sol to another chain: the coin with the call, or an ERC-20, ERC-721 or ERC-1155 the gateway may move.",
    params = {
        target = "The chain it goes to.",
        recipient = "Who gets it there: a chainv4 address's 33 bytes.",
        kind = "What it is.",
        asset = "The token or collection; zero for the coin.",
        id = "An NFT's or multi-token id's id.",
        amount = "How much; 1 for an NFT.",
    },
    category = "Bridges",
)]
pub fn send_input(target: u64, recipient: &[u8], kind: AssetKind, asset: [u8; 20], id: u256, amount: u256) -> Vec<u8> {
    let signature = "send(uint64,bytes,uint8,address,uint256,uint256)";
    let values = [
        AbiValue::Uint(u256::from(target)),
        AbiValue::Bytes(recipient.to_vec()),
        AbiValue::Uint(u256::from(kind as u64)),
        AbiValue::Address(asset),
        AbiValue::Uint(id),
        AbiValue::Uint(amount),
    ];

    [selector(signature), encode_values(&types(signature), &values).expect("a send encodes")].concat()
}

#[document(
    summary = "Gateway.sol's constructor arguments, to follow its code: the federation as Ethereum addresses, its threshold and version, what the chain's coin is called, and the wrapped templates.",
    category = "Bridges",
)]
pub fn gateway_constructor(signers: &[[u8; 20]], threshold: u64, version: u64, coin: (&str, &str), templates: [[u8; 20]; 3]) -> Vec<u8> {
    encode_values(
        &types("(address[],uint256,uint64,string,string,address[3])"),
        &[
            AbiValue::Array(signers.iter().map(|signer| AbiValue::Address(*signer)).collect()),
            AbiValue::Uint(u256::from(threshold)),
            AbiValue::Uint(u256::from(version)),
            AbiValue::String(coin.0.to_string()),
            AbiValue::String(coin.1.to_string()),
            AbiValue::Array(templates.iter().map(|template| AbiValue::Address(*template)).collect()),
        ],
    )
    .expect("a constructor encodes")
}

#[document(
    summary = "What the federation in force signs to hand Gateway.sol the next one: the gateway, its chain, the next version, signers and threshold.",
    category = "Bridges",
)]
pub fn signers_digest(gateway: [u8; 20], chain_id: u64, version: u64, signers: &[[u8; 20]], threshold: u64) -> [u8; 32] {
    keccak(
        &encode_values(
            &types("(string,address,uint256,uint64,address[],uint256)"),
            &[
                AbiValue::String("chainv4-gateway-signers".to_string()),
                AbiValue::Address(gateway),
                AbiValue::Uint(u256::from(chain_id)),
                AbiValue::Uint(u256::from(version)),
                AbiValue::Array(signers.iter().map(|signer| AbiValue::Address(*signer)).collect()),
                AbiValue::Uint(u256::from(threshold)),
            ],
        )
        .expect("a signer set encodes"),
    )
}

#[document(summary = "Calldata handing Gateway.sol the next federation, with the signatures of the one in force over signers_digest.", category = "Bridges")]
pub fn set_signers_input(signers: &[[u8; 20]], threshold: u64, version: u64, signatures: &[[u8; 65]]) -> Vec<u8> {
    let signature = "setSigners(address[],uint256,uint64,bytes[])";
    let values = [
        AbiValue::Array(signers.iter().map(|signer| AbiValue::Address(*signer)).collect()),
        AbiValue::Uint(u256::from(threshold)),
        AbiValue::Uint(u256::from(version)),
        AbiValue::Array(signatures.iter().map(|signature| AbiValue::Bytes(signature.to_vec())).collect()),
    ];

    [selector(signature), encode_values(&types(signature), &values).expect("a signer set encodes")].concat()
}

#[document(summary = "A 32-byte digest signed as the federation signs: r, s and v, v 27 or 28.", category = "Bridges")]
pub fn sign_digest(secret: &[u8; 32], digest: &[u8; 32]) -> [u8; 65] {
    let (rs, parity) = crate::crypto::secp256k1::sign_recoverable(secret, digest);
    let mut signature = [0; 65];
    signature[..64].copy_from_slice(&rs);
    signature[64] = 27 + parity;

    signature
}

#[document(
    summary = "An address on chainv4 as the gateways write it: its 33 bytes, its kind's tag first.",
    category = "Bridges",
)]
pub fn chain_bytes(address: &Address) -> Vec<u8> {
    address.to_abi().to_vec()
}

#[cfg(test)]
mod tests;
