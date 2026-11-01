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
    #[document(summary = "The most it agreed to lose to that share.")]
    pub most_fee: u64,
    #[document(summary = "queued, proposed or paid.")]
    pub state: String,
    #[document(summary = "The outpoint its release must spend, after one paying it was cancelled.")]
    pub anchor: Option<String>,
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
}

#[cfg(feature = "bitcoin")]
#[document(
    summary = "Whether a federation signer should sign a release. The gateway checks all of this when it is proposed; a signer goes over it again, as what it signs pays real bitcoin and a fault in the contract should not be enough to take it: each withdrawal named once, paid what it burned less its share of the fee, within what it agreed to lose, and still waiting; each input once, held, pending; only change besides, of the size and to the federation it should be; a fee between the floor and the cap, and at no more a virtual byte than this signer takes; and nothing Bitcoin would refuse or delay.",
    returns = "Why not, or nothing when it may sign.",
    category = "Bridges",
)]
pub fn check_btc_release(facts: &ReleaseFacts) -> Result<(), String> {
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

        if input.sequence != 0xffff_ffff || !input.script_sig.is_empty() {
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

    match &tx.outputs[facts.withdrawals.len().min(tx.outputs.len())..] {
        [] if facts.withdrawals.is_empty() => return Err("it pays nothing".to_string()),
        [] => {}
        [change] if change.script == p2wsh(&facts.change_script) && change.value >= BTC_DUST => {}
        [_] => return Err("its last output is not change to its federation".to_string()),
        _ => return Err("it pays more than its withdrawals and change".to_string()),
    }

    for (at, withdrawal) in facts.withdrawals.iter().enumerate() {
        if withdrawal.state != "proposed" {
            return Err(format!("withdrawal {} is {}", withdrawal.id, withdrawal.state));
        }

        let output = tx.outputs.get(at).ok_or_else(|| format!("it does not pay withdrawal {}", withdrawal.id))?;

        let share = btc_fee_share(fee, facts.withdrawals.len() as u64, at);

        if share > withdrawal.most_fee {
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

    let vbytes = btc_release_vbytes(facts.raw.len(), &sizes);

    if fee > vbytes.saturating_mul(facts.most_rate) {
        return Err(format!("its fee of {fee} is {} satoshis a virtual byte, past the {} this signer signs for now: give --max-fee-rate to sign it, or the federation cancels it", fee / vbytes.max(1), facts.most_rate));
    }

    Ok(())
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
    #[document(summary = "Each withdrawal it cannot pay now, with why.")]
    pub waiting: Vec<(u64, String)>,
}

/// The most withdrawals one release pays, as the gateway allows.
#[cfg(feature = "bitcoin")]
const BTC_MOST_WITHDRAWALS: usize = 50;
/// The most virtual bytes one release may be, as the gateway allows.
#[cfg(feature = "bitcoin")]
const BTC_MOST_VBYTES: u64 = 100_000;

/// What pays these withdrawals, spending just what they burned or leaving change of at least dust:
/// what their anchors require, then what the gateway holds in the order it took it. None if it holds
/// too little.
#[cfg(feature = "bitcoin")]
fn btc_inputs<'a>(chosen: &[&NamedWithdrawal], held: &'a [(String, SpentOutput)]) -> Option<Vec<&'a (String, SpentOutput)>> {
    let burned: u64 = chosen.iter().map(|withdrawal| withdrawal.sats).sum();
    let required = |outpoint: &str| chosen.iter().any(|withdrawal| withdrawal.anchor.as_deref() == Some(outpoint));
    let mut inputs: Vec<_> = held.iter().filter(|(outpoint, _)| required(outpoint)).collect();
    let enough = |taken: u64| taken == burned || taken >= burned + BTC_DUST;
    let mut taken: u64 = inputs.iter().map(|(_, output)| output.sats).sum();

    for output in held.iter().filter(|(outpoint, _)| !required(outpoint)) {
        if enough(taken) {
            break;
        }

        inputs.push(output);
        taken += output.1.sats;
    }

    enough(taken).then_some(inputs)
}

#[cfg(feature = "bitcoin")]
#[document(
    summary = "The release to propose from what is queued and what the gateway holds, oldest first. It is worked out from the gateway's state and the fee alone, so signers doing it at once build the same one, and the first to land wins. A withdrawal it cannot pay now is left out and said why, never holding up the rest: one whose anchor is in another release, one past what the gateway holds, one that agreed to lose less than its share of the fee. A release past the cap at this rate pays fewer, for another to pay the rest.",
    params = {
        queued = "What is queued, oldest first.",
        held = "What the gateway holds unspent, in the order it took it.",
        change = "The output script of the federation in force.",
        fee_cap = "The most fee a release may pay.",
        fee_floor = "The least it may pay a virtual byte.",
        fee = "What it pays.",
    },
    returns = "The plan, or why a fee told cannot be paid.",
    category = "Bridges",
    see_also = "btc_propose_digest, check_btc_release",
)]
pub fn plan_btc_release(queued: &[NamedWithdrawal], held: &[(String, SpentOutput)], change: &[u8], fee_cap: u64, fee_floor: u64, fee: ReleaseFee) -> Result<ReleasePlan, String> {
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
            return Ok(ReleasePlan { tx: None, ids: Vec::new(), fee: 0, waiting });
        }

        let Some(inputs) = btc_inputs(&chosen, held) else {
            let largest = chosen.iter().rev().max_by_key(|withdrawal| withdrawal.sats).expect("there is one").id;
            out.insert(largest);
            waiting.push((largest, "the gateway holds too little unspent to pay it and leave change of at least dust, for now".to_string()));

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

                Ok(TxIn { txid, vout: vout.parse().map_err(|_| "an outpoint that does not read")?, script_sig: Vec::new(), sequence: 0xffff_ffff, witness: Vec::new() })
            })
            .collect::<Result<Vec<_>, &str>>()?;
        let mut outputs: Vec<TxOut> = chosen.iter().map(|withdrawal| TxOut { value: withdrawal.sats, script: withdrawal.script.clone() }).collect();

        // Withdrawals pay their own fee, so all the rest of what it spends is change, and the fee
        // comes off no backing.
        if taken > burned {
            outputs.push(TxOut { value: taken - burned, script: change.to_vec() });
        }

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
        let short = chosen
            .iter()
            .enumerate()
            .filter(|(at, withdrawal)| btc_fee_share(paid, count, *at) > withdrawal.most_fee)
            .min_by_key(|(at, withdrawal)| (withdrawal.most_fee, std::cmp::Reverse(*at)))
            .map(|(_, withdrawal)| withdrawal);

        if let Some(short) = short {
            out.insert(short.id);
            waiting.push((short.id, format!("it agreed to lose {}, less than its share of a fee of {paid}: its owner raises that with set-most-fee, or it waits for fees to fall", short.most_fee)));

            continue;
        }

        for (at, output) in tx.outputs.iter_mut().take(chosen.len()).enumerate() {
            output.value -= btc_fee_share(paid, count, at);
        }

        return Ok(ReleasePlan { tx: Some(tx), ids: chosen.iter().map(|withdrawal| withdrawal.id).collect(), fee: paid, waiting });
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
#[document(summary = "What change is at least, in satoshis: Bitcoin's nodes relay nothing with an output under its dust.", category = "Bridges")]
pub const BTC_DUST: u64 = 546;

#[document(summary = "What both gateways hash a transfer's digest under.", category = "Bridges")]
pub const DOMAIN: &str = "chainv4-gateway";

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

    #[document(parent = "Transfer", summary = "Calldata delivering it to Gateway.sol with the federation's signatures: what a relayer sends.")]
    pub fn deliver_input(&self, signatures: &[[u8; 65]]) -> Vec<u8> {
        let signature = format!("deliver({TRANSFER_TUPLE},bytes[])");
        let values = [self.evm_tuple(), AbiValue::Array(signatures.iter().map(|signature| AbiValue::Bytes(signature.to_vec())).collect())];

        [selector(&signature), encode_values(&types(&signature), &values).expect("a delivery encodes")].concat()
    }
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
