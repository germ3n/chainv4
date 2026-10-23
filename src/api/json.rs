//! How the API writes chain types as JSON. Amounts and wide integers are decimal strings, since
//! JavaScript numbers lose precision past 2^53; hashes, bytes and signatures are 0x hex.

use {
    crate::registry::contracts::ContractRecord,
    crate::structs::account::Account,
    crate::structs::address::Address,
    crate::structs::amount::Amount,
    crate::structs::block::Block,
    crate::structs::code::Code,
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
        "tx_root": header.tx_root.to_string(),
        "tx_count": block.txs.len(),
    })
}

/// The summary, the seal and every transaction.
pub fn block(block: &Block) -> Value {
    let mut value = summary(block);
    value["seal"] = block.seal.map(|seal| seal.to_string()).into();
    value["txs"] = block.txs.iter().map(signed_tx).collect();

    value
}

pub fn signed_tx(signed: &SignedTx) -> Value {
    json!({
        "id": signed.id().to_string(),
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
        Tx::Deploy { code } => {
            let mut value = json!({
                "type": "deploy",
                "engine": code.engine().name(),
                "code_size": code.bytes().len(),
            });

            // Source is worth showing; a module's bytes are not.
            if let Code::Lua(source) = code {
                value["source"] = source.as_str().into();
            }

            value
        }
    }
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
        "reentrant": record.reentrant,
        "code_size": record.code.len(),
    })
}

fn decimal(amount: &Amount) -> String {
    amount.to_u256().to_string()
}
