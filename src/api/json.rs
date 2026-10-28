//! How the API writes chain types as JSON. Amounts and wide integers are decimal strings, since
//! JavaScript numbers lose precision past 2^53; hashes, bytes and signatures are 0x hex.

use {
    crate::governance::Change,
    crate::chain::{CommittedReceipt, IndexedEvent, IndexedTx, TxReceipt, TxStatus},
    crate::crypto::hashable::Hashable,
    crate::db::{Column, StateProof},
    crate::crypto::keypair::PublicKey,
    crate::node::{NodeReport, PeerReport},
    crate::registry::signers::SignerSet,
    crate::registry::validators::ValidatorRecord,
    crate::structs::vote::{Checkpoint, Justification, SignedVote},
    crate::registry::contracts::ContractRecord,
    crate::registry::tokens::Token,
    crate::registry::collections::Collection,
    crate::registry::names::NameAction,
    crate::structs::account::Account,
    crate::structs::address::Address,
    crate::structs::amount::Amount,
    crate::structs::block::{Block, Seal, SealedHeader},
    crate::structs::event::Event,
    crate::structs::code::Code,
    crate::structs::hash::{Hash, HashType},
    crate::structs::hex::{from_hex, to_hex},
    crate::structs::u256::{i256, u256},
    crate::vm::{Declared, Outcome},
    crate::structs::trace::{Hop, HopKind, Trace},
    crate::structs::tx::{FeeOffer, SignedTx, Tx, TxCallArgumentType},
    serde_json::{Value, json},
};

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
    })
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

/// How a node is doing, as GET /node answers.
pub fn node(report: &NodeReport) -> Value {
    let counters = &report.counters;

    json!({
        "started": report.started,
        "now": report.now,
        "uptime_ms": report.now.saturating_sub(report.started),
        "listening": report.listening.iter().map(ToString::to_string).collect::<Vec<_>>(),
        "producing": report.producing,
        "validating": report.validating,
        "mempool": { "count": report.waiting, "capacity": report.capacity },
        "syncing": report.syncing.map(|(peer, from)| json!({ "peer": peer.to_string(), "from": from })),
        "counters": {
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
            "flooders_disconnected": counters.flooders_disconnected,
            "gossip_rejected": counters.gossip_rejected,
            "peers_banned": counters.peers_banned,
        },
        "backlog": {
            "waiting": report.backlog.0,
            "total": report.backlog.1.total,
            "per_peer": report.backlog.1.per_peer,
        },
        "peers": report.peers.iter().map(peer).collect::<Vec<_>>(),
        "mining": report.mining.as_ref().map(mining),
    })
}

/// How a node's mining goes: its devices, the workers mining for it, and the total hashrate.
fn mining(report: &crate::consensus::MiningReport) -> Value {
    let devices = |devices: &[crate::consensus::DeviceReport]| -> Vec<Value> {
        devices.iter().map(|device| json!({ "name": device.name, "rate": device.rate })).collect()
    };

    json!({
        "rate": report.rate(),
        "devices": devices(&report.devices),
        "listening": report.listening,
        "workers": report.workers.iter().map(|worker| json!({
            "name": worker.name,
            "address": worker.address,
            "since": worker.since,
            "rate": worker.devices.iter().map(|device| device.rate).sum::<f64>(),
            "devices": devices(&worker.devices),
            "found": worker.found,
            "refused": worker.refused,
        })).collect::<Vec<_>>(),
    })
}

fn peer(peer: &PeerReport) -> Value {
    json!({
        "id": peer.id.to_string(),
        "address": peer.address.to_string(),
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

/// A fee offer, as POST /offer answers.
pub fn fee_offer(offer: &FeeOffer) -> Value {
    json!({
        "gas_limit": offer.gas_limit,
        "max_price": offer.max_price.to_string(),
        "tip": offer.tip.to_string(),
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
    })
}

/// What a streaming node did: the event's name, and its data.
#[cfg(feature = "stream")]
pub fn node_event(event: &crate::node::NodeEvent, hash: HashType, reward: &crate::chain::RewardSchedule) -> (&'static str, Value) {
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
        NodeEvent::PeerUp { peer, address } => ("peer_up", json!({ "id": peer.to_string(), "address": address.to_string() })),
        NodeEvent::PeerDown(peer) => ("peer_down", json!({ "id": peer.to_string() })),
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
    };
    value["fuel_used"] = receipt.fuel_used.into();
    value["gas_used"] = receipt.gas_used.into();
    value["fee"] = receipt.fee.to_string().into();
    value["events"] = receipt.events.iter().map(event).collect();
    value["payments"] = receipt.payments.iter().map(payment).collect();
    value["trace"] = trace(&receipt.trace);

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

pub fn event(event: &Event) -> Value {
    json!({
        "contract": event.contract.to_string(),
        "name": event.name,
        "data": event.data.iter().map(argument).collect::<Vec<_>>(),
    })
}

pub fn signed_tx(signed: &SignedTx, hash: HashType) -> Value {
    json!({
        "id": signed.id(hash).to_string(),
        "sender": signed.sender().to_string(),
        "nonce": signed.nonce,
        "fee": {
            "gas_limit": signed.fee.gas_limit,
            "max_price": signed.fee.max_price.to_string(),
            "tip": signed.fee.tip.to_string(),
        },
        "signer": signed.signer.to_string(),
        "signature": signed.signature.to_string(),
        "tx": tx(&signed.tx),
    })
}

pub fn tx(tx: &Tx) -> Value {
    match tx {
        Tx::Transfer { from, to, amount } => json!({
            "type": "transfer",
            "from": from.to_string(),
            "to": to.to_string(),
            "amount": decimal(amount),
        }),
        Tx::Mint { to, amount } => json!({
            "type": "mint",
            "to": to.to_string(),
            "amount": decimal(amount),
        }),
        Tx::Burn { from, amount } => json!({
            "type": "burn",
            "from": from.to_string(),
            "amount": decimal(amount),
        }),
        Tx::EvmCall { to, value, input } => json!({
            "type": "evm_call",
            "to": to.to_string(),
            "value": decimal(value),
            "input": crate::structs::hex::to_hex(input),
        }),
        Tx::ContractCall { to, func, amount, data } => json!({
            "type": "contract_call",
            "to": to.to_string(),
            "func": func.to_string(),
            "amount": decimal(amount),
            "data": data.iter().map(argument).collect::<Vec<_>>(),
        }),
        Tx::Deploy { code } | Tx::Upgrade { code, .. } => {
            let mut value = json!({
                "type": if matches!(tx, Tx::Deploy { .. }) { "deploy" } else { "upgrade" },
                "engine": code.engine().name(),
                "code_size": code.bytes().len(),
            });

            if let Tx::Upgrade { contract, .. } = tx {
                value["contract"] = contract.to_string().into();
            }

            // Source is worth showing; a module's bytes are not.
            if let Code::Lua(source) = code {
                value["source"] = source.as_str().into();
            }

            value
        }
        Tx::Bond { voter, amount } => json!({
            "type": "bond",
            "voter": voter.to_string(),
            "amount": decimal(amount),
        }),
        Tx::Unbond { voter } => json!({ "type": "unbond", "voter": voter.to_string() }),
        Tx::Withdraw { voter } => json!({ "type": "withdraw", "voter": voter.to_string() }),
        Tx::Slash { first, second } => json!({
            "type": "slash",
            "voter": first.vote.voter.to_string(),
            "first": vote(first),
            "second": vote(second),
        }),
        Tx::ApproveSigners {
            version,
            signers,
            threshold,
            change_threshold,
        } => {
            let mut value = signer_set(&SignerSet::new(signers.clone(), *threshold, *change_threshold));
            value["type"] = "approve_signers".into();
            value["version"] = (*version).into();

            value
        }
        Tx::WithdrawApproval { version } => json!({ "type": "withdraw_approval", "version": version }),
        Tx::ApproveChange { version, change } => json!({ "type": "approve_change", "version": version, "change": self::change(change) }),
        Tx::WithdrawChange { version } => json!({ "type": "withdraw_change", "version": version }),
        Tx::Name { name, action } => json!({ "type": "name", "name": name, "action": name_action(action) }),
        Tx::PrimaryName { name } => json!({ "type": "primary_name", "name": name }),
        Tx::Ethereum { raw } => ethereum(raw),
    }
}

/// An Ethereum-signed transaction: its bytes and hash, and what they say.
fn ethereum(raw: &[u8]) -> Value {
    let mut value = json!({ "type": "ethereum", "raw": to_hex(raw) });

    #[cfg(feature = "secp256k1")]
    {
        value["hash"] = to_hex(&crate::crypto::ethereum::tx_hash(raw)).into();
    }

    if let Ok(ethereum) = crate::crypto::ethereum::decode(raw) {
        value["kind"] = ethereum.kind.number().into();
        value["chain_id"] = ethereum.chain_id.into();
        value["to"] = ethereum.to.map(|to| Address::Evm(to).to_string()).into();
        value["value"] = ethereum.value.to_string().into();
        value["input"] = to_hex(&ethereum.input).into();
    }

    value
}

/// What a name transaction does: its kind, and what it sets.
pub fn name_action(action: &NameAction) -> Value {
    let mut value = match action {
        NameAction::Register { years } | NameAction::Renew { years } => json!({ "years": years }),
        NameAction::Transfer { to } => json!({ "to": to.to_string() }),
        NameAction::Target { target } => json!({ "target": target.map(|target| target.to_string()) }),
        NameAction::Text { key, value } => json!({ "key": key, "value": value }),
        NameAction::Subname { label, owner } => json!({ "label": label, "owner": owner.to_string() }),
    };
    value["type"] = action.kind().into();

    value
}

/// A change under governance: what it is of, and the new value.
pub fn change(change: &Change) -> Value {
    match change {
        Change::Consensus(consensus) => json!({ "consensus": crate::cli::genesis::consensus_json(consensus, None) }),
        Change::Bootnodes(bootnodes) => json!({ "bootnodes": bootnodes }),
        Change::NamePrices(prices) => json!({ "name_prices": prices.iter().map(u256::to_string).collect::<Vec<_>>() }),
        Change::Collections(functions) => json!({ "collections": { "nft": functions.nft, "multi": functions.multi } }),
        Change::Federation(federation) => json!({ "federation": crate::cli::genesis::federation_json(federation) }),
        Change::Directory(directory) => json!({ "directory": crate::cli::genesis::directory_json(directory) }),
        Change::Fees(fees) => json!({ "fees": crate::cli::genesis::fees_json(fees) }),
        Change::Reward(reward) => json!({ "reward": crate::cli::genesis::reward_json(reward) }),
        Change::Limits(limits) => json!({ "limits": crate::cli::genesis::limits_json(limits) }),
        Change::Finality(finality) => json!({ "finality": crate::cli::genesis::finality_json(finality) }),
        Change::SwitchFinality { epoch, config } => json!({ "switch_finality": { "epoch": epoch, "finality": crate::cli::genesis::finality_json(config) } }),
    }
}

/// A set of authority signers, each with its key and account, with its two thresholds.
pub fn signer_set(set: &SignerSet) -> Value {
    json!({
        "signers": set.signers.iter().map(|signer| json!({ "key": signer.key.to_string(), "account": signer.account.to_string() })).collect::<Vec<_>>(),
        "threshold": set.threshold,
        "change_threshold": set.change_threshold,
    })
}

pub fn checkpoint(checkpoint: &Checkpoint) -> Value {
    json!({ "epoch": checkpoint.epoch, "block": checkpoint.block.to_string() })
}

/// A signed vote, readable, with its encoding for Tx::Slash.
pub fn vote(vote: &SignedVote) -> Value {
    json!({
        "voter": vote.vote.voter.to_string(),
        "source": checkpoint(&vote.vote.source),
        "target": checkpoint(&vote.vote.target),
        "encoded": to_hex(&wincode::serialize(vote).expect("votes always encode")),
    })
}

/// A justification, without its votes' signatures.
pub fn proof(proof: &Justification) -> Value {
    json!({
        "source": checkpoint(&proof.source),
        "target": checkpoint(&proof.target),
        "voters": proof.votes.iter().map(|vote| vote.vote.voter.to_string()).collect::<Vec<_>>(),
    })
}

pub fn validator(voter: &PublicKey, record: &ValidatorRecord) -> Value {
    json!({
        "voter": voter.to_string(),
        "account": record.account.to_string(),
        "bond": record.bond.to_string(),
        "active_from": record.active_from,
        "active_until": record.active_until,
        "withdrawable_from": record.withdrawable_from,
        "slashed": record.slashed,
    })
}

/// A call argument as its type and value. Numbers are JSON numbers, or null when not finite.
pub fn argument(argument: &TxCallArgumentType) -> Value {
    let (kind, value): (&str, Value) = match argument {
        TxCallArgumentType::Address(address) => ("address", address.to_string().into()),
        TxCallArgumentType::Amount(amount) => ("amount", decimal(amount).into()),
        TxCallArgumentType::Bytes(bytes) => ("bytes", to_hex(bytes).into()),
        TxCallArgumentType::Bool(value) => ("bool", (*value).into()),
        TxCallArgumentType::String(value) => ("string", value.as_str().into()),
        TxCallArgumentType::Number(value) => ("number", (*value).into()),
        TxCallArgumentType::I64(value) => ("i64", value.to_string().into()),
        TxCallArgumentType::U64(value) => ("u64", value.to_string().into()),
        TxCallArgumentType::I128(value) => ("i128", value.to_string().into()),
        TxCallArgumentType::U128(value) => ("u128", value.to_string().into()),
        TxCallArgumentType::U256(value) => ("u256", value.to_string().into()),
        TxCallArgumentType::I256(value) => ("i256", value.to_string().into()),
        TxCallArgumentType::Nil => ("nil", Value::Null),
    };

    json!({ "type": kind, "value": value })
}

/// A call argument from its type and value, as argument writes it. Whole numbers may also be
/// JSON numbers.
pub fn parse_argument(value: &Value) -> Result<TxCallArgumentType, String> {
    let kind = value.get("type").and_then(Value::as_str).ok_or("an argument is {\"type\": …, \"value\": …}")?;

    if kind == "nil" {
        return Ok(TxCallArgumentType::Nil);
    }

    let given = value.get("value").ok_or("an argument needs a value")?;
    let text = given.as_str().map(str::to_string).unwrap_or_else(|| given.to_string());
    let article = if kind.starts_with(['a', 'i']) { "an" } else { "a" };
    let bad = || format!("not {article} {kind}: {text}");
    let argument = match kind {
        "address" => TxCallArgumentType::Address(text.parse().map_err(|_| bad())?),
        "amount" => TxCallArgumentType::Amount(Amount::U256(u256::from_str_radix(&text, 10).map_err(|_| bad())?)),
        "bytes" => TxCallArgumentType::Bytes(from_hex(&text).ok_or_else(bad)?),
        "bool" => TxCallArgumentType::Bool(given.as_bool().or_else(|| text.parse().ok()).ok_or_else(bad)?),
        "string" => TxCallArgumentType::String(text),
        "number" => TxCallArgumentType::Number(given.as_f64().or_else(|| text.parse().ok()).ok_or_else(bad)?),
        "i64" => TxCallArgumentType::I64(text.parse().map_err(|_| bad())?),
        "u64" => TxCallArgumentType::U64(text.parse().map_err(|_| bad())?),
        "i128" => TxCallArgumentType::I128(text.parse().map_err(|_| bad())?),
        "u128" => TxCallArgumentType::U128(text.parse().map_err(|_| bad())?),
        "u256" => TxCallArgumentType::U256(u256::from_str_radix(&text, 10).map_err(|_| bad())?),
        "i256" => TxCallArgumentType::I256(i256::from_str_radix(&text, 10).map_err(|_| bad())?),
        other => return Err(format!("unknown argument type {other}")),
    };

    Ok(argument)
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

fn decimal(amount: &Amount) -> String {
    amount.to_u256().to_string()
}
