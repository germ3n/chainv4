//! How the API writes chain types as JSON. Amounts and wide integers are decimal strings, since
//! JavaScript numbers lose precision past 2^53; hashes, bytes and signatures are 0x hex.

use {
    crate::chain::{TxReceipt, TxStatus},
    crate::crypto::keypair::PublicKey,
    crate::registry::validators::ValidatorRecord,
    crate::structs::vote::{Checkpoint, Justification, SignedVote},
    crate::registry::contracts::ContractRecord,
    crate::structs::account::Account,
    crate::structs::address::Address,
    crate::structs::amount::Amount,
    crate::structs::block::{Block, Seal},
    crate::structs::event::Event,
    crate::structs::code::Code,
    crate::structs::hash::HashType,
    crate::structs::hex::to_hex,
    crate::structs::tx::{SignedTx, Tx, TxCallArgumentType},
    serde_json::{Value, json},
};

/// A block's id and header, and how many transactions it holds.
pub fn summary(block: &Block) -> Value {
    let header = &block.header;

    json!({
        "id": block.id().to_string(),
        "height": header.height,
        "parent": header.parent.to_string(),
        "timestamp": header.timestamp,
        "producer": header.producer.to_string(),
        "difficulty": header.difficulty.to_string(),
        "tx_root": header.tx_root.to_string(),
        "tx_count": block.txs.len(),
        "proof_count": block.proofs.len(),
    })
}

/// The summary, the seal and every transaction, each with its receipt when the block is on the
/// main chain.
pub fn block(block: &Block, receipts: &[Option<TxReceipt>], hash: HashType) -> Value {
    let mut value = summary(block);
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
    value["events"] = receipt.events.iter().map(event).collect();

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
    }
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
    };

    json!({ "type": kind, "value": value })
}

pub fn account(address: &Address, account: &Account) -> Value {
    json!({
        "address": address.to_string(),
        "nonce": account.nonce,
        "balance": account.balance.to_string(),
    })
}

pub fn contract(address: &Address, record: &ContractRecord) -> Value {
    json!({
        "address": address.to_string(),
        "deployer": record.deployer.to_string(),
        "engine": record.engine.name(),
        "functions": record.functions,
        "upgradable": record.functions.iter().any(|name| name == crate::vm::CHECK_PERMS),
        "reentrant": record.reentrant,
        "code_size": record.code.len(),
    })
}

fn decimal(amount: &Amount) -> String {
    amount.to_u256().to_string()
}
