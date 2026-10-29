//! Transactions and what they carry as JSON, as the API writes them: amounts and wide integers as
//! decimal strings, since JavaScript numbers lose precision past 2^53, and hashes, bytes and
//! signatures as 0x hex. Apart from the node, so the apps write a transaction they decoded as the
//! node would.

use {
    crate::registry::names::NameAction,
    crate::registry::signers::SignerSet,
    crate::structs::address::Address,
    crate::structs::amount::Amount,
    crate::structs::code::Code,
    crate::structs::event::Event,
    crate::structs::hash::HashType,
    crate::structs::hex::{from_hex, to_hex},
    crate::structs::tx::{FeeOffer, SignedTx, Tx, TxCallArgumentType},
    crate::structs::u256::{i256, u256},
    crate::structs::vote::{Checkpoint, SignedVote},
    serde_json::{Value, json},
};

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
        "fee": fee_offer(&signed.fee),
        "signer": signed.signer.to_string(),
        "signature": signed.signature.to_string(),
        "tx": tx(&signed.tx),
    })
}

pub fn tx(transaction: &Tx) -> Value {
    match transaction {
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
                "type": if matches!(transaction, Tx::Deploy { .. }) { "deploy" } else { "upgrade" },
                "engine": code.engine().name(),
                "code_size": code.bytes().len(),
            });

            if let Tx::Upgrade { contract, .. } = transaction {
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
        Tx::ApproveChange { version, change } => json!({ "type": "approve_change", "version": version, "change": crate::params::json::change_json(change) }),
        Tx::WithdrawChange { kind, version } => json!({ "type": "withdraw_change", "kind": kind.name(), "version": version }),
        Tx::SetDirectory { directory } => json!({ "type": "set_directory", "directory": crate::params::json::directory_json(directory) }),
        Tx::HandDirectory { to } => json!({ "type": "hand_directory", "to": to.map(|to| to.to_string()) }),
        Tx::Name { name, action } => json!({ "type": "name", "name": name, "action": name_action(action) }),
        Tx::PrimaryName { name } => json!({ "type": "primary_name", "name": name }),
        Tx::Ethereum { raw } => ethereum(raw),
        Tx::Multisig { account, tx: inner, signatures } => json!({
            "type": "multisig",
            "account": account.to_string(),
            "tx": tx(inner),
            "signatures": signatures.iter().map(|signed| json!({ "key": signed.key.to_string(), "signature": signed.signature.to_string() })).collect::<Vec<_>>(),
        }),
        Tx::CreateMultisig { members, threshold, testnet } => json!({ "type": "create_multisig", "members": members.iter().map(|member| multisig_member(member, *testnet)).collect::<Vec<_>>(), "threshold": threshold, "testnet": testnet }),
        Tx::ChangeMultisig { members, threshold, testnet } => json!({ "type": "change_multisig", "members": members.iter().map(|member| multisig_member(member, *testnet)).collect::<Vec<_>>(), "threshold": threshold, "testnet": testnet }),
        Tx::ApproveMultisig { account, nonce, fee, tx: inner } => json!({
            "type": "approve_multisig",
            "account": account.to_string(),
            "nonce": nonce,
            "fee": fee_offer(fee),
            "tx": tx(inner),
        }),
        Tx::WithdrawMultisig { account, proposal } => json!({ "type": "withdraw_multisig", "account": account.to_string(), "proposal": proposal.to_string() }),
        Tx::Claim { height, to, earned, index, path } => json!({
            "type": "claim",
            "height": height,
            "to": to.to_string(),
            "earned": earned.to_string(),
            "index": index,
            "path": path.iter().map(ToString::to_string).collect::<Vec<_>>(),
        }),
    }
}

/// A multisig's member: its key here, its Bitcoin xpub with its origin as descriptors write it,
/// and its EVM address in hex, each or null.
pub fn multisig_member(member: &crate::registry::multisig::MultisigMember, testnet: bool) -> Value {
    #[cfg(feature = "bitcoin")]
    let bitcoin = member.bitcoin.map(|xpub| crate::crypto::bitcoin::multisig::xpub_text(&xpub, network(testnet)));
    #[cfg(not(feature = "bitcoin"))]
    let bitcoin = member.bitcoin.map(|xpub| { let _ = testnet; to_hex(&xpub.public) });

    json!({
        "key": member.key.to_string(),
        "bitcoin": bitcoin,
        "evm": member.evm.map(|address| to_hex(&address)),
    })
}

/// The Bitcoin network a multisig's xpubs are for.
#[cfg(feature = "bitcoin")]
pub fn network(testnet: bool) -> crate::crypto::bitcoin::tx::Network {
    match testnet {
        true => crate::crypto::bitcoin::tx::Network::Test,
        false => crate::crypto::bitcoin::tx::Network::Main,
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

/// A fee offer, as POST /offer answers.
pub fn fee_offer(offer: &FeeOffer) -> Value {
    json!({
        "gas_limit": offer.gas_limit,
        "max_price": offer.max_price.to_string(),
        "tip": offer.tip.to_string(),
    })
}

pub(crate) fn decimal(amount: &Amount) -> String {
    amount.to_u256().to_string()
}
