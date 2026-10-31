//! What the index answers, written as the node's API writes it, so that a page reads one form whichever
//! gave it: blocks, transactions, what touched an address and events, as far as they were read. What
//! was not read says so, in the words the page shows. Each is a plain function of what is held, tested
//! without a screen.
//!
//! What a block commits to leaves two things out of what a node's own answer has: why a transaction
//! failed, and the hops its contracts made. Neither is consensus, so neither can be checked, and neither
//! is shown here.

use {
    super::index::{EventKey, EventPlace, Index, TxPlace},
    crate::api::{EventQuery, Failure},
    chainv4::light::{RangeBody, RangeItem},
    chainv4::structs::address::Address,
    chainv4::structs::block::Seal,
    chainv4::structs::hash::{Hash, HashType},
    chainv4::structs::hex::from_hex,
    chainv4::structs::json::{argument, checkpoint, event as event_json, signed_tx},
    chainv4::structs::receipt::{CommittedOutcome, CommittedReceipt, StepEffect, StepOutcome},
    chainv4::structs::tx::SignedTx,
    chainv4::structs::vote::Justification,
    chainv4_ui::filters::{Facts, TxFilter},
    serde_json::{Value, json},
};

/// How many entries a page lists unless it says, and the most any page looks through before it says it
/// stopped there: an address that touches everything still answers within a frame.
const PAGE: usize = 50;
const MOST_LOOKED_AT: u64 = 2_000;
/// What a failed transaction says of why, as a receipt's message is no part of what a block commits to.
const FAILED: &str = "a block commits to that it failed, not why";

/// Where a reading stands, for what it answers to say.
#[derive(Clone, Copy, Debug)]
pub struct Reach {
    pub known_from: u64,
    pub high: u64,
    /// The height it is still reading down to, if it is.
    pub reading_to: Option<u64>,
    pub hash: HashType,
}

fn not_found(message: String) -> Failure {
    Failure { status: 404, message }
}

/// A block's summary, as GET /blocks lists it. What a block minted is in no header, so it goes unsaid.
pub fn summary(item: &RangeItem) -> Value {
    let header = &item.sealed.header;
    let (txs, proofs, pool) = match &item.body {
        RangeBody::Whole { txs, proofs, pool, .. } => (Some(txs.len()), Some(proofs.len()), *pool),
        _ => (None, None, None),
    };

    json!({
        "id": item.sealed.id().to_string(),
        "height": header.height,
        "parent": header.parent.to_string(),
        "timestamp": header.timestamp,
        "producer": header.producer.to_string(),
        "difficulty": header.difficulty.to_string(),
        "tx_root": header.tx_root.to_string(),
        "receipts_root": header.receipts_root.to_string(),
        "state_root": header.state_root.to_string(),
        "gas_used": header.gas_used,
        "base_price": header.base_price.to_string(),
        "reward": "",
        "tx_count": txs,
        "proof_count": proofs,
        "main": true,
        "pruned": txs.is_none(),
        "pool_root": header.pool_root.to_string(),
        "pool": pool.map(|pool| json!({
            "parts": pool.parts,
            "root": pool.root.to_string(),
            "count": pool.count,
        })),
    })
}

fn seal(seal: &Seal) -> Value {
    match seal {
        Seal::None => Value::Null,
        Seal::Signature(signature) => json!({ "type": "signature", "signature": signature.to_string() }),
        Seal::Work { nonce } => json!({ "type": "work", "nonce": nonce.to_string() }),
    }
}

fn proof(proof: &Justification) -> Value {
    json!({
        "source": checkpoint(&proof.source),
        "target": checkpoint(&proof.target),
        "voters": proof.votes.iter().map(|vote| vote.vote.voter.to_string()).collect::<Vec<_>>(),
    })
}

fn step(step: &StepOutcome) -> Value {
    let mut value = match &step.effect {
        StepEffect::Done => json!({ "status": "done" }),
        StepEffect::Deployed(address) => json!({ "status": "deployed", "contract": address.to_string() }),
        StepEffect::Upgraded(address) => json!({ "status": "upgraded", "contract": address.to_string() }),
        StepEffect::Returned(values) => json!({ "status": "returned", "values": values.iter().map(argument).collect::<Vec<_>>() }),
    };
    value["events"] = step.events.into();
    value["payments"] = step.payments.into();

    value
}

/// What a transaction did and emitted, as far as a block commits to it.
pub fn outcome(receipt: &CommittedReceipt) -> Value {
    let mut value = match &receipt.outcome {
        CommittedOutcome::Done => json!({ "status": "done" }),
        CommittedOutcome::Deployed(address) => json!({ "status": "deployed", "contract": address.to_string() }),
        CommittedOutcome::Upgraded(address) => json!({ "status": "upgraded", "contract": address.to_string() }),
        CommittedOutcome::Returned(values) => json!({ "status": "returned", "values": values.iter().map(argument).collect::<Vec<_>>() }),
        CommittedOutcome::Failed => json!({ "status": "failed", "error": FAILED }),
        CommittedOutcome::Decided { decision, kind, version } => json!({ "status": "decided", "version": decision, "kind": kind.name(), "kind_version": version }),
        CommittedOutcome::Batch(steps) => json!({ "status": "batch", "steps": steps.iter().map(step).collect::<Vec<_>>() }),
        CommittedOutcome::BatchFailed { step } => json!({ "status": "failed", "failed_step": step, "error": format!("step {} failed, undoing every step: {FAILED}", step + 1) }),
    };
    value["fuel_used"] = receipt.fuel_used.into();
    value["gas_used"] = receipt.gas_used.into();
    value["fee"] = receipt.fee.to_string().into();
    value["events"] = receipt.events.iter().map(event_json).collect();
    value["payments"] = receipt.payments.iter().map(|payment| json!({ "from": payment.from.to_string(), "to": payment.to.to_string(), "amount": payment.amount.to_string() })).collect();

    value
}

/// A transaction and what it did.
fn tx_json(signed: &SignedTx, receipt: Option<&CommittedReceipt>, hash: HashType) -> Value {
    let mut value = signed_tx(signed, hash);
    value["receipt"] = receipt.map(outcome).into();

    value
}

/// A block, as GET /blocks/{block} answers it.
pub fn block(item: &RangeItem, hash: HashType) -> Value {
    let mut value = summary(item);
    value["seal"] = seal(&item.sealed.seal);
    value["proofs"] = Value::Array(Vec::new());
    value["txs"] = Value::Array(Vec::new());

    if let RangeBody::Whole { txs, proofs, receipts, .. } = &item.body {
        value["proofs"] = proofs.iter().map(proof).collect();
        value["txs"] = txs.iter().enumerate().map(|(at, signed)| tx_json(signed, receipts.as_deref().and_then(|receipts| receipts.get(at)), hash)).collect();
    }

    value
}

/// What to say of a height that was not read.
pub fn outside_height(height: u64, reach: &Reach) -> String {
    match (height > reach.high, height >= reach.known_from) {
        (true, _) => format!("Block {height} has not been read: this explorer has read up to block {}, and reads final blocks only", reach.high),
        (false, true) => format!("Block {height} is not among those read yet, or the node holds nothing of it: this explorer has read blocks {} to {}", reach.known_from, reach.high),
        (false, false) => format!("Block {height} is older than what was read: this explorer knows from block {}", reach.known_from),
    }
}

/// Blocks by height or id, among those read.
pub fn block_by<'a>(index: &'a Index, wanted: &str, hash: HashType) -> Option<&'a RangeItem> {
    let height = match wanted.parse::<u64>() {
        Ok(height) => height,
        Err(_) => index.height_of(&Hash::from_bytes(hash, &from_hex(wanted)?)?)?,
    };

    index.at(height)
}

/// GET /blocks/{block}.
pub fn block_answer(found: Option<&RangeItem>, wanted: &str, reach: &Reach) -> Result<Value, Failure> {
    match (found, wanted.parse::<u64>()) {
        (Some(item), _) => Ok(block(item, reach.hash)),
        (None, Ok(height)) => Err(not_found(outside_height(height, reach))),
        (None, Err(_)) => Err(not_found(format!("No block with this id among those read: this explorer knows blocks from {} to {}, the final ones", reach.known_from, reach.high))),
    }
}

/// GET /blocks.
pub fn blocks_answer(index: &Index, before: Option<u64>, limit: u64) -> Value {
    json!({ "blocks": index.newest_below(before, limit as usize).map(summary).collect::<Vec<_>>() })
}

/// What a transaction's place holds, if it does.
fn tx_at(index: &Index, place: TxPlace) -> Option<(&RangeItem, &SignedTx, Option<&CommittedReceipt>)> {
    let item = index.at(place.0)?;
    let RangeBody::Whole { txs, receipts, .. } = &item.body else {
        return None;
    };
    let at = place.1 as usize;

    Some((item, txs.get(at)?, receipts.as_deref().and_then(|receipts| receipts.get(at))))
}

/// GET /txs/{id}.
pub fn tx_answer(index: &Index, id: &str, reach: &Reach) -> Result<Value, Failure> {
    let outside = || not_found(format!("Not in the blocks read, {} to {}: it may be older, or not final yet", reach.known_from, reach.high));
    let found = from_hex(id).and_then(|bytes| Hash::from_bytes(reach.hash, &bytes)).and_then(|id| index.tx_at(&id)).ok_or_else(outside)?;
    let (item, _, receipt) = tx_at(index, found).ok_or_else(outside)?;
    let receipt = receipt.ok_or_else(|| not_found(format!("Block {} came without its receipts, so what this transaction did is not known", found.0)))?;
    let mut value = outcome(receipt);
    value["id"] = id.into();
    value["block"] = item.sealed.id().to_string().into();
    value["height"] = found.0.into();
    value["index"] = found.1.into();
    value["confirmations"] = reach.high.saturating_sub(found.0).into();
    value["final"] = true.into();

    Ok(value)
}

/// One value of a query string, %-encoding undone.
fn query_value(query: &str, name: &str) -> Option<String> {
    let raw = query.split('&').find_map(|pair| pair.strip_prefix(name)?.strip_prefix('='))?;
    let bytes = raw.as_bytes();
    let mut text = Vec::with_capacity(bytes.len());
    let mut at = 0;

    while at < bytes.len() {
        match (bytes[at], raw.get(at + 1..at + 3).and_then(|hex| u8::from_str_radix(hex, 16).ok())) {
            (b'%', Some(byte)) => {
                text.push(byte);
                at += 3;
            }
            (byte, _) => {
                text.push(byte);
                at += 1;
            }
        }
    }

    String::from_utf8(text).ok()
}

/// What a query says to look for, as chainv4_ui::filters writes one.
fn filter_of(query: &str) -> (TxFilter, Option<u64>, Option<u64>) {
    use chainv4_ui::filters::Direction;

    let filter = TxFilter {
        kind: query_value(query, "kind"),
        direction: query_value(query, "direction").and_then(|direction| match direction.as_str() {
            "in" => Some(Direction::In),
            "out" => Some(Direction::Out),
            _ => None,
        }),
        counterparty: query_value(query, "counterparty").unwrap_or_default(),
        token_typed: query_value(query, "token").unwrap_or_default(),
        ..TxFilter::default()
    };

    (filter, query_value(query, "from").and_then(|from| from.parse().ok()), query_value(query, "until").and_then(|until| until.parse().ok()))
}

/// Whether a transaction passes a filter, as a node's own filter takes it, by what it says of itself.
fn passes(filter: &TxFilter, tx: &Value, sender_is_address: bool, time: u64, from: Option<u64>, until: Option<u64>) -> bool {
    let inner = tx["tx"].get("tx").filter(|_| tx["tx"]["type"] == "multisig").unwrap_or(&tx["tx"]);
    let named = |field: &str| inner.get(field).and_then(Value::as_str);
    let kind = inner.get("type").and_then(Value::as_str);
    let steps = crate::model::steps_of(&tx["tx"]).filter(|_| tx["tx"]["type"] != "approve_multisig").unwrap_or_default();
    let of_step = |step: &'_ Value| step.get("type").and_then(Value::as_str).map(str::to_string);
    let step_kinds: Vec<String> = steps.iter().filter_map(of_step).collect();
    let payments = tx["receipt"]["payments"].as_array().is_some_and(|paid| !paid.is_empty());
    let moves_coin = matches!(kind, Some("transfer" | "mint" | "burn" | "claim")) || payments || step_kinds.iter().any(|kind| matches!(kind.as_str(), "transfer" | "burn" | "claim"));
    let called = named("to").filter(|_| matches!(kind, Some("contract_call" | "evm_call")));
    let called_in_steps = steps.iter().filter(|step| matches!(step.get("type").and_then(Value::as_str), Some("contract_call" | "evm_call"))).filter_map(|step| step.get("to").and_then(Value::as_str));
    let events = tx["receipt"]["events"].as_array().into_iter().flatten().filter_map(|event| event["contract"].as_str());
    let named_in_steps = steps.iter().flat_map(|step| ["to", "from", "contract"].map(|field| step.get(field).and_then(Value::as_str))).flatten();
    let facts = Facts {
        kinds: [tx["tx"]["type"].as_str(), kind].into_iter().flatten().chain(step_kinds.iter().map(String::as_str)).collect(),
        sent: sender_is_address,
        took_part: !sender_is_address,
        parties: [tx["sender"].as_str(), named("to"), named("from"), named("contract"), named("account")].into_iter().flatten().chain(named_in_steps).collect(),
        tokens: moves_coin.then_some(chainv4_ui::filters::COIN).into_iter().chain(called).chain(called_in_steps).chain(events).collect(),
        time: None,
    };

    filter.passes(&facts, 0) && from.is_none_or(|from| time >= from) && until.is_none_or(|until| time <= until)
}

/// GET /accounts/{address}/txs, under a filter's query, from a cursor. The cursor is the place to go on
/// below, as this alone reads it.
pub fn account_txs(index: &Index, address: &str, query: &str, cursor: Option<&str>, reach: &Reach) -> Result<Value, Failure> {
    let parsed: Address = address.parse().map_err(|why| Failure { status: 400, message: format!("{why}") })?;
    let below = cursor.and_then(|cursor| {
        let (height, at) = cursor.split_once('-')?;

        Some((height.parse().ok()?, at.parse().ok()?))
    });
    let (filter, from, until) = filter_of(query);
    let filtered = !query.is_empty();
    let (mut txs, mut next, mut scanned, mut limited) = (Vec::new(), None, 0, false);

    for place in index.txs_of(&parsed, below) {
        if txs.len() == PAGE || scanned == MOST_LOOKED_AT {
            limited = scanned == MOST_LOOKED_AT && txs.len() < PAGE;
            break;
        }

        scanned += 1;
        next = Some(format!("{}-{}", place.0, place.1));

        let Some((item, signed, receipt)) = tx_at(index, place) else {
            continue;
        };
        let mut value = tx_json(signed, receipt, reach.hash);
        let time = item.sealed.header.timestamp;

        if filtered && !passes(&filter, &value, signed.sender() == parsed, time, from, until) {
            continue;
        }

        value["block"] = item.sealed.id().to_string().into();
        value["height"] = place.0.into();
        value["index"] = place.1.into();
        value["time"] = time.into();
        txs.push(value);
    }

    // Nothing left past what was looked at, it has no next.
    let last = next.as_deref().and_then(|text| {
        let (height, at) = text.split_once('-')?;

        Some((height.parse::<u64>().ok()?, at.parse::<u32>().ok()?))
    });
    let more = last.is_some_and(|last| index.txs_of(&parsed, Some(last)).next().is_some());

    Ok(json!({
        "txs": txs,
        "next": next.filter(|_| more),
        "indexed_from": reach.known_from,
        "catching_up_to": reach.reading_to,
        "applied": filtered.then(|| query.split('&').filter_map(|pair| pair.split('=').next()).collect::<Vec<_>>()),
        "scanned": scanned,
        "limited": limited,
    }))
}

/// One event, as GET /events lists it.
fn event_at(index: &Index, place: EventPlace) -> Option<Value> {
    let (item, _, receipt) = tx_at(index, (place.0, place.1))?;
    let receipt = receipt?;
    let mut value = event_json(receipt.events.get(place.2 as usize)?);
    value["tx"] = receipt.tx.to_string().into();
    value["block"] = item.sealed.id().to_string().into();
    value["height"] = place.0.into();
    value["index"] = place.1.into();
    value["position"] = place.2.into();

    Some(value)
}

/// What a query looks through: what it names most narrowly, as what else it names is tested on each.
fn keys_of(query: &EventQuery) -> Result<Vec<EventKey>, Failure> {
    let refused = |what: &str| Failure { status: 400, message: format!("{what} that does not read") };
    let address = |text: &str| text.parse::<Address>().map_err(|_| refused("an address"));

    if let Some(topic) = &query.topic {
        let topic: [u8; 32] = from_hex(topic).and_then(|bytes| bytes.try_into().ok()).ok_or_else(|| refused("a topic"))?;

        return Ok(vec![EventKey::Topic(topic)]);
    }

    if let Some(mentioning) = &query.address {
        let mentioning = address(mentioning)?;

        return Ok(vec![EventKey::Mentions(mentioning), EventKey::MentionsEvm(chainv4::abi::evm_address(&mentioning))]);
    }

    match &query.contract {
        Some(contract) => Ok(vec![EventKey::Contract(address(contract)?)]),
        None => Err(Failure { status: 400, message: "name a contract, an address or a topic".to_string() }),
    }
}

/// GET /events, from a cursor, which is the place to go on below, as this alone reads it.
pub fn events_answer(index: &Index, query: &EventQuery, cursor: Option<&str>, reach: &Reach) -> Result<Value, Failure> {
    let below: Option<EventPlace> = cursor.and_then(|cursor| {
        let mut parts = cursor.split('-');

        Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
    });
    let limit = query.limit.map_or(PAGE, |limit| limit as usize);
    let keys = keys_of(query)?;
    let contract = query.contract.as_deref();
    let name = query.name.as_deref();
    let mut places: Vec<EventPlace> = keys.iter().flat_map(|key| index.events_of(key, below)).collect();

    places.sort_unstable_by(|a, b| b.cmp(a));
    places.dedup();

    let mut events = Vec::new();
    let mut next = None;

    for place in places {
        if events.len() == limit {
            next = Some(place);
            break;
        }

        let Some(event) = event_at(index, place) else {
            continue;
        };

        // What the place was found by named one thing: what else was asked is tested here.
        if contract.is_some_and(|contract| event["contract"].as_str() != Some(contract)) || name.is_some_and(|name| event["name"].as_str() != Some(name)) {
            continue;
        }

        events.push(event);
    }

    // The next page goes on from just above the place it found no room for.
    let next = next.map(|(height, at, position): EventPlace| {
        let (height, at, position) = match (position, at) {
            (u32::MAX, _) => (height, at, position),
            _ => (height, at, position + 1),
        };

        format!("{height}-{at}-{position}")
    });

    Ok(json!({
        "events": events,
        "next": next,
        "indexed_from": reach.known_from,
        "catching_up_to": reach.reading_to,
        "topics": true,
    }))
}
