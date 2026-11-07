//! How the API writes chain types as JSON. Amounts and wide integers are decimal strings, since
//! JavaScript numbers lose precision past 2^53; hashes, bytes and signatures are 0x hex.

use {
    crate::chain::{CommittedReceipt, IndexedEvent, IndexedTx, TxReceipt, TxStatus},
    crate::crypto::hashable::Hashable,
    crate::db::{Column, StateProof},
    crate::crypto::keypair::PublicKey,
    crate::node::{NodeReport, PeerReport},
    super::sharing::{ReportField, Sharing},
    crate::registry::validators::ValidatorRecord,
    crate::structs::vote::Justification,
    crate::registry::contracts::ContractRecord,
    crate::registry::tokens::Token,
    crate::registry::collections::Collection,
    crate::structs::account::Account,
    crate::structs::address::Address,
    crate::structs::block::{Block, Seal, SealedHeader},
    crate::structs::hash::{Hash, HashType},
    crate::structs::hex::to_hex,
    crate::structs::u256::u256,
    crate::vm::{Declared, Outcome},
    crate::structs::trace::{Hop, HopKind, Trace},
    crate::structs::tx::SignedTx,
    serde_json::{Value, json},
};

pub use crate::structs::json::{argument, checkpoint, event, fee_offer, multisig_member, parse_argument, signed_tx, signer_set, tx, vote};
#[cfg(feature = "bitcoin")]
pub use crate::structs::json::network;

/// A block's id and header, how many transactions it holds, and what it minted for its producer.
pub fn summary(block: &Block, reward: &crate::chain::RewardSchedule) -> Value {
    let header = &block.header;

    json!({
        "id": block.id().to_string(),
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
        "reward": minted(reward, header.height).to_string(),
        "tx_count": block.txs.len(),
        "proof_count": block.proofs.len(),
        "pool_root": header.pool_root.to_string(),
        "pool": block.pool.map(|pool| json!({
            "parts": pool.parts,
            "percent": percent(pool.parts),
            "root": pool.root.to_string(),
            "count": pool.count,
        })),
    })
}

/// Parts of PAYOUT_PARTS as a percent.
pub fn percent(parts: u32) -> f64 {
    f64::from(parts) / f64::from(crate::structs::block::PAYOUT_PARTS) * 100.0
}

/// Each miner's parts of a pool's block, and what they came to once what it shared out is known.
#[cfg(feature = "pool")]
pub fn payouts(payouts: &[crate::structs::block::Payout], shared: Option<u256>) -> Vec<Value> {
    payouts
        .iter()
        .map(|payout| {
            json!({
                "to": payout.to.to_string(),
                "parts": payout.parts,
                "percent": percent(payout.parts),
                "amount": shared.map(|shared| payout.of(shared).to_string()),
            })
        })
        .collect()
}

/// What a block at a height minted for its producer: the reward schedule's figure, and nothing
/// for genesis, which no producer made.
pub fn minted(reward: &crate::chain::RewardSchedule, height: u64) -> u256 {
    match height {
        0 => u256::ZERO,
        height => reward.at(height),
    }
}

/// A proof of one chain-state entry against a block's state root, with what it proves: the entry's
/// value, or null for none. The proof comes encoded too, for light clients to decode.
pub fn state_proof(block: &SealedHeader, column: Column, key: &[u8], value: Option<&[u8]>, proof: &StateProof) -> Value {
    json!({
        "block": block.id().to_string(),
        "height": block.header.height,
        "state_root": block.header.state_root.to_string(),
        "proof": to_hex(&wincode::serialize(proof).expect("proofs always encode")),
        "column": column.name(),
        "key": to_hex(key),
        "value": value.map(to_hex),
        "siblings": proof.siblings.iter().map(ToString::to_string).collect::<Vec<_>>(),
        "leaf": proof.leaf.map(|(path, value)| json!({"path": to_hex(&path), "value_hash": value.to_string()})),
    })
}

/// A proof that a receipt is at its place in a block's receipts root, and its transaction at the
/// same place in tx_root: each encoded, with the Merkle path from its leaf.
pub fn receipt_proof(block: &Block, receipt: &CommittedReceipt, index: usize, count: usize, path: &[Hash], tx_path: &[Hash]) -> Value {
    json!({
        "tx": receipt.tx.to_string(),
        "block": block.id().to_string(),
        "height": block.header.height,
        "receipts_root": block.header.receipts_root.to_string(),
        "index": index,
        "count": count,
        "receipt": to_hex(&receipt.hashable_bytes_with_tag()),
        "path": path.iter().map(ToString::to_string).collect::<Vec<_>>(),
        "tx_root": block.header.tx_root.to_string(),
        "signed": to_hex(&block.txs[index].hashable_bytes_with_tag()),
        "tx_path": tx_path.iter().map(ToString::to_string).collect::<Vec<_>>(),
    })
}

/// A block this node keeps the header of alone, as one that synced from a snapshot does for those
/// before it: the summary but for what the body holds, marked pruned, with its seal.
pub fn pruned(sealed: &SealedHeader, reward: &crate::chain::RewardSchedule) -> Value {
    let header = &sealed.header;

    json!({
        "id": sealed.id().to_string(),
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
        "reward": minted(reward, header.height).to_string(),
        "tx_count": Value::Null,
        "proof_count": Value::Null,
        "pruned": true,
        "seal": seal(&sealed.seal),
        "txs": [],
        "proofs": [],
        "main": true,
    })
}

/// The summary, the seal and every transaction, each with its receipt when the block is on the
/// main chain.
pub fn block(block: &Block, receipts: &[Option<TxReceipt>], hash: HashType, reward: &crate::chain::RewardSchedule) -> Value {
    let mut value = summary(block, reward);
    value["seal"] = seal(&block.seal);
    value["txs"] = block
        .txs
        .iter()
        .enumerate()
        .map(|(index, tx)| {
            let mut value = signed_tx(tx, hash);
            value["receipt"] = receipts.get(index).and_then(Option::as_ref).map(outcome).into();

            value
        })
        .collect();
    value["proofs"] = block.proofs.iter().map(proof).collect();

    value
}

/// How a node is doing, as GET /node answers, less what it keeps to itself: each such field stays,
/// empty, so apps from before still read the answer, and `hidden` names them. Its limits are every
/// value its operator may tune, by key, times in milliseconds and sizes in bytes, null for one
/// unset.
pub fn node(report: &NodeReport, sharing: Sharing, tuning: &crate::node::Tuning) -> Value {
    let counters = &report.counters;
    let shown = |field: ReportField| !sharing.hides(field);
    let uptime = shown(ReportField::Uptime);
    let role = shown(ReportField::Role);

    json!({
        "hidden": sharing.hidden_names(),
        "started": if uptime { report.started } else { 0 },
        "now": report.now,
        "uptime_ms": if uptime { report.now.saturating_sub(report.started) } else { 0 },
        "listening": if shown(ReportField::Listening) { report.listening.iter().map(ToString::to_string).collect::<Vec<_>>() } else { Vec::new() },
        "producing": role && report.producing,
        "validating": role && report.validating,
        "mempool": { "count": report.waiting, "capacity": report.capacity },
        "syncing": report.syncing.filter(|_| shown(ReportField::Syncing)).map(|(peer, from)| json!({ "peer": if shown(ReportField::PeerIds) { peer.to_string() } else { String::new() }, "from": from })),
        "counters": if !shown(ReportField::Counters) { json!({}) } else { json!({
            "blocks_heard": counters.blocks_heard,
            "blocks_imported": counters.blocks_imported,
            "side_blocks": counters.side_blocks,
            "reorgs": counters.reorgs,
            "blocks_rejected": counters.blocks_rejected,
            "blocks_produced": counters.blocks_produced,
            "txs_heard": counters.txs_heard,
            "txs_submitted": counters.txs_submitted,
            "txs_refused": counters.txs_refused,
            "votes_heard": counters.votes_heard,
            "votes_cast": counters.votes_cast,
            "requests_served": counters.requests_served,
            "requests_sent": counters.requests_sent,
            "requests_failed": counters.requests_failed,
            "connections": counters.connections,
            "gossip_dropped": counters.gossip_dropped,
            "gossip_unpublished": counters.gossip_unpublished,
            "flooders_disconnected": counters.flooders_disconnected,
            "gossip_rejected": counters.gossip_rejected,
            "peers_banned": counters.peers_banned,
        }) },
        "backlog": shown(ReportField::Backlog).then(|| json!({
            "waiting": report.backlog.0,
            "total": report.backlog.1.total,
            "per_peer": report.backlog.1.per_peer,
            "per_ip": report.backlog.1.per_ip,
            "requests": report.backlog.1.requests,
            "request_burst": report.backlog.1.request_burst,
            "strikes": report.backlog.1.strikes,
            "strike_window_ms": report.backlog.1.strike_window.as_millis() as u64,
        })),
        "limits": match shown(ReportField::Limits) {
            true => Value::Object(tuning.values().into_iter().map(|(key, value)| (key.to_string(), value.map_or(Value::Null, Value::from))).collect()),
            false => json!({}),
        },
        "peers": if shown(ReportField::Peers) { report.peers.iter().map(|one| peer(one, sharing)).collect::<Vec<_>>() } else { Vec::new() },
        "mining": report.mining.as_ref().filter(|_| shown(ReportField::Mining)).map(|report| mining(report, sharing)),
    })
}

/// How a node's mining goes: its devices, the workers mining for it, and the total hashrate.
fn mining(report: &crate::consensus::MiningReport, sharing: Sharing) -> Value {
    let shown = |field: ReportField| !sharing.hides(field);
    let devices = |devices: &[crate::consensus::DeviceReport]| -> Vec<Value> {
        match shown(ReportField::Devices) {
            true => devices.iter().map(|device| json!({ "name": device.name, "rate": device.rate })).collect(),
            false => Vec::new(),
        }
    };
    let workers: &[crate::consensus::WorkerReport] = if shown(ReportField::Workers) { &report.workers } else { &[] };

    json!({
        "rate": report.rate(),
        "devices": devices(&report.devices),
        "listening": report.listening.as_ref().filter(|_| shown(ReportField::Listening)),
        "workers": workers.iter().enumerate().map(|(index, worker)| json!({
            "name": worker_name(&worker.name, index, sharing),
            "address": if shown(ReportField::WorkerAddresses) { worker.address.as_str() } else { "" },
            "since": worker.since,
            "rate": worker.devices.iter().map(|device| device.rate).sum::<f64>(),
            "devices": devices(&worker.devices),
            "found": worker.found,
            "refused": worker.refused,
            "payout": worker.payout.filter(|_| shown(ReportField::WorkerPayouts)).map(|payout| payout.to_string()),
            "shares": worker.shares,
            "share_difficulty": worker.share_difficulty.to_string(),
            "solo": worker.solo,
        })).collect::<Vec<_>>(),
        "solo": report.solo,
        "pool": report.pool.as_ref().map(|pool| json!({
            "fee": pool.fee,
            "window": pool.window,
            "open": pool.open,
            "miners": pool.miners.len(),
            "blocks": pool.blocks.len(),
        })),
    })
}

/// What a worker goes by: its own name, or by its place when its name is kept to the node.
pub(super) fn worker_name(name: &str, index: usize, sharing: Sharing) -> String {
    match sharing.hides(ReportField::WorkerNames) {
        true => format!("worker {}", index + 1),
        false => name.to_string(),
    }
}

fn peer(peer: &PeerReport, sharing: Sharing) -> Value {
    json!({
        "id": if sharing.hides(ReportField::PeerIds) { String::new() } else { peer.id.to_string() },
        "address": if sharing.hides(ReportField::PeerAddresses) { String::new() } else { peer.address.to_string() },
        "since": peer.since,
        "heard": { "blocks": peer.heard.blocks, "txs": peer.heard.txs, "votes": peer.heard.votes },
        "dropped": peer.dropped,
        "chain": peer.chain.map(|(height, work, justified)| json!({
            "height": height,
            "work": work.to_string(),
            "justified": justified,
        })),
    })
}

/// What a dry run found, as POST /offer answers it: the gas and fuel it used, and whether it
/// would go through, or why not.
pub fn dry_run(ran: &crate::chain::DryRun) -> Value {
    json!({
        "gas_used": ran.gas_used,
        "fuel_used": ran.fuel_used,
        "status": if ran.failure.is_some() { "failed" } else { "ok" },
        "error": ran.failure,
        "returned": match &ran.effect {
            Some(crate::chain::Effect::Returned(values)) => values.iter().map(argument).collect::<Vec<_>>().into(),
            _ => Value::Null,
        },
        "deployed": match &ran.effect {
            Some(crate::chain::Effect::Deployed(address)) => address.to_string().into(),
            _ => Value::Null,
        },
        "steps": match &ran.effect {
            Some(crate::chain::Effect::Batch(steps)) => steps.iter().map(step_outcome).collect::<Vec<_>>().into(),
            _ => Value::Null,
        },
    })
}

/// What a streaming node did: the event's name, and its data.
#[cfg(feature = "stream")]
pub fn node_event(event: &crate::node::NodeEvent, hash: HashType, reward: &crate::chain::RewardSchedule, sharing: Sharing) -> (&'static str, Value) {
    use crate::node::NodeEvent;

    match event {
        NodeEvent::Block { block, main } => {
            let mut value = summary(block, reward);
            value["main"] = (*main).into();

            ("block", value)
        }
        NodeEvent::Reorganized { undone, applied } => (
            "reorganized",
            json!({
                "undone": undone.iter().map(ToString::to_string).collect::<Vec<_>>(),
                "applied": applied.iter().map(|block| summary(block, reward)).collect::<Vec<_>>(),
            }),
        ),
        NodeEvent::Justified(checkpoint) => ("justified", self::checkpoint(checkpoint)),
        NodeEvent::Finalized(checkpoint) => ("finalized", self::checkpoint(checkpoint)),
        NodeEvent::Queued(tx) => ("queued", signed_tx(tx, hash)),
        NodeEvent::Left(left) => ("left", json!({
            "id": left.tx.id(hash).to_string(),
            "sender": left.tx.sender().to_string(),
            "nonce": left.tx.nonce,
            "why": left.why.as_str(),
        })),
        NodeEvent::PeerUp { peer, address } => ("peer_up", json!({
            "id": if sharing.hides(ReportField::PeerIds) { String::new() } else { peer.to_string() },
            "address": if sharing.hides(ReportField::PeerAddresses) { String::new() } else { address.to_string() },
        })),
        NodeEvent::PeerDown(peer) => ("peer_down", json!({ "id": if sharing.hides(ReportField::PeerIds) { String::new() } else { peer.to_string() } })),
    }
}

pub fn seal(seal: &Seal) -> Value {
    match seal {
        Seal::None => Value::Null,
        Seal::Signature(signature) => json!({ "type": "signature", "signature": signature.to_string() }),
        Seal::Work { nonce } => json!({ "type": "work", "nonce": nonce.to_string() }),
    }
}

/// Where a transaction landed, how deep it is, and what it did.
pub fn receipt(id: &str, receipt: &TxReceipt, height: u64, confirmations: u64) -> Value {
    let mut value = outcome(receipt);
    value["id"] = id.into();
    value["block"] = receipt.block.to_string().into();
    value["height"] = height.into();
    value["index"] = receipt.index.into();
    value["confirmations"] = confirmations.into();

    value
}

/// What a transaction did and emitted.
fn outcome(receipt: &TxReceipt) -> Value {
    let mut value = match &receipt.status {
        TxStatus::Done => json!({ "status": "done" }),
        TxStatus::Deployed(address) => json!({ "status": "deployed", "contract": address.to_string() }),
        TxStatus::Upgraded(address) => json!({ "status": "upgraded", "contract": address.to_string() }),
        TxStatus::Returned(values) => json!({
            "status": "returned",
            "values": values.iter().map(argument).collect::<Vec<_>>(),
        }),
        TxStatus::Failed(error) => json!({ "status": "failed", "error": error }),
        TxStatus::Decided { decision, kind, version } => json!({ "status": "decided", "version": decision, "kind": kind.name(), "kind_version": version }),
        TxStatus::Rejected { rejection, kind, version } => json!({ "status": "rejected", "rejection": rejection, "kind": kind.name(), "kind_version": version }),
        TxStatus::Batch(steps) => json!({ "status": "batch", "steps": steps.iter().map(step_outcome).collect::<Vec<_>>() }),
        // Failed as any failure says it, so what reads no further sees that; which step is extra.
        TxStatus::BatchFailed { step, why } => json!({ "status": "failed", "failed_step": step, "error": format!("step {} failed, undoing every step: {why}", step + 1) }),
    };
    value["fuel_used"] = receipt.fuel_used.into();
    value["gas_used"] = receipt.gas_used.into();
    value["fee"] = receipt.fee.to_string().into();
    value["events"] = receipt.events.iter().map(event).collect();
    value["payments"] = receipt.payments.iter().map(payment).collect();
    value["trace"] = trace(&receipt.trace);

    value
}

/// What a batch's step did, as a whole transaction says it, and how many of the receipt's events and
/// payments are its.
fn step_outcome(step: &crate::structs::receipt::StepOutcome) -> Value {
    use crate::structs::receipt::StepEffect;

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

/// Coin a contract paid while a transaction ran.
pub fn payment(payment: &crate::structs::receipt::Payment) -> Value {
    json!({ "from": payment.from.to_string(), "to": payment.to.to_string(), "amount": payment.amount.to_string() })
}

/// Where a transaction's contracts went, hop by hop, depth first.
pub fn trace(trace: &Trace) -> Value {
    json!({
        "hops": trace.hops.iter().map(hop).collect::<Vec<_>>(),
        "dropped": trace.dropped,
    })
}

fn hop(hop: &Hop) -> Value {
    let kind = match hop.kind {
        HopKind::Call => "call",
        HopKind::Delegate => "delegate",
        HopKind::Transfer => "transfer",
        HopKind::Clone => "clone",
        HopKind::Create => "create",
    };

    json!({
        "depth": hop.depth,
        "kind": kind,
        "from": hop.from.to_string(),
        "to": hop.to.to_string(),
        "function": hop.function,
        "value": hop.value.to_string(),
        "fuel_used": hop.fuel_used,
        "error": hop.error,
        "events": hop.events,
    })
}

/// An indexed event: the event, and the transaction and block it came from.
pub fn indexed_event(found: &IndexedEvent) -> Value {
    let mut value = event(&found.event);
    value["tx"] = found.tx.to_string().into();
    value["block"] = found.block.to_string().into();
    value["height"] = found.height.into();
    value["index"] = found.index.into();
    value["position"] = found.position.into();

    value
}

/// A transaction the address index found, with where it landed and what it did.
pub fn indexed_tx(found: &IndexedTx, signed: &SignedTx, receipt: &TxReceipt, hash: HashType) -> Value {
    let mut value = signed_tx(signed, hash);
    value["block"] = found.block.to_string().into();
    value["height"] = found.height.into();
    value["index"] = found.index.into();
    value["receipt"] = outcome(receipt);

    value
}

/// A change under governance: what it is of, and the new value.
#[cfg(feature = "governance")]
pub fn change(change: &crate::governance::Change) -> Value {
    crate::params::json::change_json(change)
}

/// A justification, without its votes' signatures.
pub fn proof(proof: &Justification) -> Value {
    json!({
        "source": checkpoint(&proof.source),
        "target": checkpoint(&proof.target),
        "voters": proof.votes.iter().map(|vote| vote.vote.voter.to_string()).collect::<Vec<_>>(),
    })
}

pub fn validator(voter: &PublicKey, record: &ValidatorRecord, restake: Option<crate::registry::validators::Restake>) -> Value {
    json!({
        "voter": voter.to_string(),
        "account": record.account.to_string(),
        "bond": record.bond.to_string(),
        "active_from": record.active_from,
        "active_until": record.active_until,
        "withdrawable_from": record.withdrawable_from,
        "slashed": record.slashed,
        "attester": record.attester.map(|attester| crate::structs::hex::to_hex(&attester)),
        "restake": restake.map(|restake| json!({ "on": restake.on, "cap": restake.cap.to_string() })),
    })
}

/// What a read-only call did: its values or why it failed, and what it would have emitted.
pub fn call(outcome: &Outcome) -> Value {
    let mut value = match &outcome.result {
        Ok(values) => json!({ "status": "returned", "values": values.iter().map(argument).collect::<Vec<_>>() }),
        Err(err) => json!({ "status": "failed", "error": err.to_string() }),
    };
    value["fuel_used"] = outcome.fuel_used.into();
    value["events"] = outcome.events.iter().map(event).collect();
    value["trace"] = trace(&outcome.trace);

    value
}

pub fn account(address: &Address, account: &Account) -> Value {
    json!({
        "address": address.to_string(),
        "nonce": account.nonce,
        "balance": account.balance.to_string(),
    })
}

/// A contract, with what the token registry holds for it: null when it is not a token.
pub fn contract(address: &Address, record: &ContractRecord, token: Option<&Token>, collection: Option<&Collection>) -> Value {
    json!({
        "address": address.to_string(),
        "deployer": record.deployer.to_string(),
        "engine": record.engine.name(),
        "functions": record.functions,
        "upgradable": record.functions.iter().any(|name| name == crate::vm::CHECK_PERMS),
        "reentrant": record.reentrant,
        "layout": record.layout.namespaces.iter().map(declared).collect::<Vec<_>>(),
        "code_size": record.code.len(),
        "token": token.map(self::token),
        "collection": collection.map(self::collection),
    })
}

/// One namespace of a contract's storage layout: a map with its kind and unit, and its keys' kind
/// and unit, null for any key; a record with its keys, each with its kind and unit; or retired.
fn declared(declared: &Declared) -> Value {
    let shape = &declared.shape;

    match (shape, shape.map(), shape.record()) {
        (_, Some((key, value)), _) => json!({
            "name": declared.name,
            "shape": "map",
            "kind": value.kind.name(),
            "unit": value.unit,
            "key_kind": key.as_ref().map(|key| key.kind.name()),
            "key_unit": key.and_then(|key| key.unit),
        }),
        (_, None, Some(keys)) => json!({
            "name": declared.name,
            "shape": "record",
            "keys": keys.iter().map(|field| json!({
                "key": field.key,
                "kind": field.value.as_ref().map(|value| value.kind.name()),
                "unit": field.value.as_ref().and_then(|value| value.unit.clone()),
                "retired": field.value.is_none(),
            })).collect::<Vec<_>>(),
        }),
        _ => json!({"name": declared.name, "shape": "retired"}),
    }
}

/// A registered token: its address, and its name, symbol and decimals, each null when it exports
/// none.
/// A registered collection: its address, its kind, nft or multi, its name and its symbol.
pub fn collection(collection: &Collection) -> Value {
    json!({
        "address": collection.address.to_string(),
        "kind": collection.kind.as_str(),
        "name": collection.name,
        "symbol": collection.symbol,
    })
}

pub fn token(token: &Token) -> Value {
    json!({
        "address": token.address.to_string(),
        "name": token.name,
        "symbol": token.symbol,
        "decimals": token.decimals,
    })
}

