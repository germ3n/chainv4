//! EIP-712: typed data, as Ethereum's wallets sign it, so MetaMask and the hardware wallets it
//! drives can sign chainv4 transactions with keys they already hold. A wallet shows each field by
//! name and signs the keccak256 of `0x19 0x01`, the domain's hash and the message's.
//!
//! The domain is `chainv4`, version `1`, with the genesis id as its salt, so a signature holds on
//! one chain alone. Each transaction has one typed form:
//! - **`Transfer`:** from, to, amount, nonce and fee, for a coin transfer whose amount is in its
//!   narrowest width, as the wallet writes one.
//! - **`TokenTransfer`:** token, to, amount, nonce and fee, for the token standard's transfer call.
//! - **`Call`:** the contract, the function by its name, the amount sent, and each argument as an
//!   `Argument`, its type's name and its value written out, for any other contract call.
//! - **`Batch`:** its steps, each a `Step`: a transfer, call or EVM call written out as those are,
//!   anything else as its kind and encoded bytes.
//! - **`Transaction`:** its kind and encoded bytes, nonce and fee, for every other.
//!
//! A call holds its function's XXH3-64 hash, not its name, so the name travels beside the
//! signature, in SignedTx::functions, one for each call the form writes out, in order: the node
//! checks each hashes to its call's and builds the same form. A multisig's batch has no form
//! anyone may sign, as its steps sit inside the multisig's opaque transaction.
//!
//! Each form maps back to one transaction alone: a transfer or call whose amount could be
//! narrower, read as Transfer or Call, would sign the same as the narrow one under another id, so
//! it goes as Transaction, or in a batch as its encoded bytes. Addresses are their text,
//! `ed25519:0x…`, as EIP-712's own address type holds Ethereum's alone.

use {
    super::hashable::Hashable,
    super::secp256k1::keccak,
    crate::structs::amount::Amount,
    crate::structs::hash::Hash,
    crate::structs::tx::{FeeOffer, Tx},
    crate::structs::u256::u256,
};

/// The domain's name and version.
pub const NAME: &str = "chainv4";
pub const VERSION: &str = "1";

/// The longest function name a typed call carries.
pub const MAX_FUNCTION_NAME: usize = 256;

/// A field's value, as EIP-712 encodes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    /// `string`, hashed.
    String(String),
    /// `uint64`, `uint256` and the like: 32 bytes, big-endian.
    Uint(u256),
    /// `bytes`, hashed.
    Bytes(Vec<u8>),
    /// `bytes32`, as it is.
    Bytes32([u8; 32]),
    /// `address`: 20 bytes, left-padded to 32.
    Address([u8; 20]),
    /// Another struct, by its hash.
    Struct(Typed),
    /// An array, `Type[]`: the keccak256 of its items' encodings, one after another.
    Array(Vec<Value>),
}

/// A struct's type and values: its name, and each field's name, type and value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Typed {
    pub name: &'static str,
    pub fields: Vec<(&'static str, &'static str, Value)>,
}

/// The structs the forms hold in arrays, by name, each field's name and type: a call's argument
/// and a batch's step. An empty array names its type all the same.
fn definition(name: &str) -> Option<(&'static str, &'static [(&'static str, &'static str)])> {
    match name {
        "Argument" => Some(("Argument", &[("type", "string"), ("value", "string")])),
        "Step" => Some(("Step", &[("kind", "string"), ("target", "string"), ("function", "string"), ("amount", "uint256"), ("arguments", "Argument[]"), ("data", "bytes")])),
        _ => None,
    }
}

/// A type without its array suffixes: Step for Step[].
fn base(kind: &str) -> &str {
    kind.find('[').map_or(kind, |open| &kind[..open])
}

impl Typed {
    /// Its type, as `encodeType` writes it: itself, then the structs it holds, by name.
    pub fn encode_type(&self) -> String {
        let mut referenced = self.definitions();
        let own = referenced.remove(self.name).unwrap_or_default();

        std::iter::once(signature(self.name, &own)).chain(referenced.iter().map(|(name, fields)| signature(name, fields))).collect()
    }

    /// Every struct type it reaches, itself first among them, each its fields' names and types:
    /// what eth_signTypedData_v4's types are.
    pub fn definitions(&self) -> std::collections::BTreeMap<&'static str, Vec<(&'static str, &'static str)>> {
        let mut into = std::collections::BTreeMap::new();
        self.reach(&mut into);

        into
    }

    fn reach(&self, into: &mut std::collections::BTreeMap<&'static str, Vec<(&'static str, &'static str)>>) {
        into.insert(self.name, self.fields.iter().map(|(name, kind, _)| (*name, *kind)).collect());

        for (_, kind, value) in &self.fields {
            if let Value::Struct(inner) = value {
                inner.reach(into);
            }

            reach_defined(base(kind), into);
        }
    }

    /// `hashStruct`: the keccak256 of its type's hash and each field, encoded.
    pub fn hash(&self) -> [u8; 32] {
        let mut encoded = keccak(self.encode_type().as_bytes()).to_vec();

        for (_, _, value) in &self.fields {
            encoded.extend_from_slice(&encode(value));
        }

        keccak(&encoded)
    }
}

fn signature(name: &str, fields: &[(&str, &str)]) -> String {
    let fields: Vec<String> = fields.iter().map(|(name, kind)| format!("{kind} {name}")).collect();

    format!("{name}({})", fields.join(","))
}

fn reach_defined(name: &str, into: &mut std::collections::BTreeMap<&'static str, Vec<(&'static str, &'static str)>>) {
    let Some((name, fields)) = definition(name) else { return };

    if into.insert(name, fields.to_vec()).is_none() {
        for (_, kind) in fields {
            reach_defined(base(kind), into);
        }
    }
}

/// One value's 32 bytes, as a struct's field.
fn encode(value: &Value) -> [u8; 32] {
    match value {
        Value::String(text) => keccak(text.as_bytes()),
        Value::Bytes(bytes) => keccak(bytes),
        Value::Uint(number) => number.to_be_bytes(),
        Value::Bytes32(bytes) => *bytes,
        Value::Address(address) => {
            let mut word = [0; 32];
            word[12..].copy_from_slice(address);
            word
        }
        Value::Struct(inner) => inner.hash(),
        Value::Array(items) => keccak(&items.iter().flat_map(encode).collect::<Vec<u8>>()),
    }
}

/// What a wallet signs: the keccak256 of `0x19 0x01`, the domain's hash and the message's.
pub fn digest(domain: &Typed, message: &Typed) -> [u8; 32] {
    let mut bytes = vec![0x19, 0x01];
    bytes.extend_from_slice(&domain.hash());
    bytes.extend_from_slice(&message.hash());

    keccak(&bytes)
}

/// The domain of a chain's transactions: chainv4, version 1, salted with its genesis id, or that
/// id's keccak256 when it is not 32 bytes long.
pub fn domain(genesis: &Hash) -> Typed {
    let id = genesis.to_bytes();
    let salt = id.as_slice().try_into().unwrap_or_else(|_| keccak(&id));

    Typed {
        name: "EIP712Domain",
        fields: vec![
            ("name", "string", Value::String(NAME.to_string())),
            ("version", "string", Value::String(VERSION.to_string())),
            ("salt", "bytes32", Value::Bytes32(salt)),
        ],
    }
}

/// A transaction's typed form, with its nonce and fee offer, and the names of the calls it writes
/// out, in order.
///
/// Returns None when the names are not those calls': too few, too many, or one that does not hash
/// to its call's function, is empty or is longer than MAX_FUNCTION_NAME.
pub fn message(tx: &Tx, functions: &[String], nonce: u64, fee: &FeeOffer) -> Option<Typed> {
    let costs = [
        ("nonce", "uint64", Value::Uint(u256::from(nonce))),
        ("gasLimit", "uint64", Value::Uint(u256::from(fee.gas_limit))),
        ("maxPrice", "uint256", Value::Uint(fee.max_price)),
        ("tip", "uint256", Value::Uint(fee.tip)),
    ];
    let mut names = functions.iter();
    let (name, mut fields) = match tx {
        Tx::Transfer { from, to, amount } if narrowest(amount) => (
            "Transfer",
            vec![
                ("from", "string", Value::String(from.to_string())),
                ("to", "string", Value::String(to.to_string())),
                ("amount", "uint256", Value::Uint(amount.to_u256())),
            ],
        ),
        Tx::ContractCall { .. } if token_transfer(tx).is_some() => {
            let (token, to, units) = token_transfer(tx)?;

            (
                "TokenTransfer",
                vec![
                    ("token", "string", Value::String(token.to_string())),
                    ("to", "string", Value::String(to.to_string())),
                    ("amount", "uint256", Value::Uint(units)),
                ],
            )
        }
        Tx::ContractCall { to, amount, data, .. } if narrowest(amount) => (
            "Call",
            vec![
                ("contract", "string", Value::String(to.to_string())),
                ("function", "string", Value::String(named(tx, names.next()?)?)),
                ("amount", "uint256", Value::Uint(amount.to_u256())),
                ("arguments", "Argument[]", Value::Array(data.iter().map(argument).collect())),
            ],
        ),
        Tx::Batch { steps } => ("Batch", vec![("steps", "Step[]", Value::Array(steps.iter().map(|step| self::step(step, &mut names)).collect::<Option<_>>()?))]),
        _ => generic(tx),
    };

    // Every name stands for a call it writes out.
    if names.next().is_some() {
        return None;
    }

    fields.extend(costs);

    Some(Typed { name, fields })
}

/// The token standard's transfer call: the token, to whom and how many units.
fn token_transfer(tx: &Tx) -> Option<(&crate::structs::address::Address, &crate::structs::address::Address, u256)> {
    #[cfg(feature = "xxh3")]
    use crate::structs::tx::TxCallArgumentType;

    match tx {
        // A build without xxh3 runs no contracts, and has no token transfer to name.
        #[cfg(feature = "xxh3")]
        Tx::ContractCall { to: token, func, amount: Amount::U128(0), data } if *func == super::hasher::Hasher::digest(crate::structs::hash::HashType::Xxh3_64, b"transfer") => match data.as_slice() {
            [TxCallArgumentType::Address(to), TxCallArgumentType::U256(units)] => Some((token, to, *units)),
            _ => None,
        },
        _ => None,
    }
}

/// A call's function name, when it is the one the call holds by hash.
fn named(tx: &Tx, name: &String) -> Option<String> {
    #[cfg(feature = "xxh3")]
    if let Tx::ContractCall { func, .. } = tx
        && !name.is_empty()
        && name.len() <= MAX_FUNCTION_NAME
        && *func == super::hasher::Hasher::digest(crate::structs::hash::HashType::Xxh3_64, name.as_bytes())
    {
        return Some(name.clone());
    }

    let _ = (tx, name);

    None
}

/// One step of a batch: a transfer, call or EVM call written out, its amount in its narrowest
/// width; anything else as its kind and encoded bytes, with no target, which a written-out step
/// always has.
fn step<'a>(step: &Tx, names: &mut impl Iterator<Item = &'a String>) -> Option<Value> {
    let written = |kind: &str, target: String, function: String, amount: u256, arguments: Vec<Value>, data: Vec<u8>| {
        Value::Struct(Typed {
            name: "Step",
            fields: vec![
                ("kind", "string", Value::String(kind.to_string())),
                ("target", "string", Value::String(target)),
                ("function", "string", Value::String(function)),
                ("amount", "uint256", Value::Uint(amount)),
                ("arguments", "Argument[]", Value::Array(arguments)),
                ("data", "bytes", Value::Bytes(data)),
            ],
        })
    };

    Some(match step {
        Tx::Transfer { to, amount, .. } if narrowest(amount) => written("Transfer", to.to_string(), String::new(), amount.to_u256(), Vec::new(), Vec::new()),
        Tx::ContractCall { to, amount, data, .. } if narrowest(amount) => {
            written("Call", to.to_string(), named(step, names.next()?)?, amount.to_u256(), data.iter().map(argument).collect(), Vec::new())
        }
        Tx::EvmCall { to, value, input } if narrowest(value) => written("EvmCall", to.to_string(), String::new(), value.to_u256(), Vec::new(), input.clone()),
        other => written(kind(other).as_str(), String::new(), String::new(), u256::ZERO, Vec::new(), other.hashable_bytes_with_tag()),
    })
}

/// A call's argument as its type's name and its value written out: an amount wider than it needs
/// as amount256, so each writing stands for one argument alone.
pub fn argument(argument: &crate::structs::tx::TxCallArgumentType) -> Value {
    use crate::structs::tx::TxCallArgumentType;

    let (kind, value) = match argument {
        TxCallArgumentType::Address(address) => ("address", address.to_string()),
        TxCallArgumentType::Amount(amount) if narrowest(amount) => ("amount", amount.to_u256().to_string()),
        TxCallArgumentType::Amount(amount) => ("amount256", amount.to_u256().to_string()),
        TxCallArgumentType::Bytes(bytes) => ("bytes", crate::structs::hex::to_hex(bytes)),
        TxCallArgumentType::Bool(value) => ("bool", value.to_string()),
        TxCallArgumentType::String(value) => ("string", value.clone()),
        // Only finite numbers are an argument, and Rust writes each the shortest way that reads
        // back to it, -0 apart from 0.
        TxCallArgumentType::Number(value) => ("number", value.to_string()),
        TxCallArgumentType::I64(value) => ("i64", value.to_string()),
        TxCallArgumentType::U64(value) => ("u64", value.to_string()),
        TxCallArgumentType::I128(value) => ("i128", value.to_string()),
        TxCallArgumentType::U128(value) => ("u128", value.to_string()),
        TxCallArgumentType::U256(value) => ("u256", value.to_string()),
        TxCallArgumentType::I256(value) => ("i256", value.to_string()),
        TxCallArgumentType::Nil => ("nil", String::new()),
    };

    Value::Struct(Typed {
        name: "Argument",
        fields: vec![("type", "string", Value::String(kind.to_string())), ("value", "string", Value::String(value))],
    })
}

/// A transaction's kind, as its variant is named.
fn kind(tx: &Tx) -> String {
    format!("{tx:?}").split([' ', '{', '(']).next().unwrap_or_default().to_string()
}

/// Any transaction, as its kind and encoded bytes.
fn generic(tx: &Tx) -> (&'static str, Vec<(&'static str, &'static str, Value)>) {
    ("Transaction", vec![("kind", "string", Value::String(kind(tx))), ("data", "bytes", Value::Bytes(tx.hashable_bytes_with_tag()))])
}

/// Whether an amount is written in its narrowest width: as the wallet writes one, and the one
/// encoding a Transfer form stands for.
fn narrowest(amount: &Amount) -> bool {
    match amount {
        Amount::U128(_) => true,
        Amount::U256(wide) => u128::try_from(*wide).is_err(),
    }
}

/// How many function names a transaction's typed form carries: one for each call it writes out.
pub fn calls_named(tx: &Tx) -> usize {
    let call = |tx: &Tx| matches!(tx, Tx::ContractCall { amount, .. } if narrowest(amount)) && token_transfer(tx).is_none();

    match tx {
        Tx::Batch { steps } => steps.iter().filter(|step| matches!(step, Tx::ContractCall { amount, .. } if narrowest(amount))).count(),
        tx => usize::from(call(tx)),
    }
}

/// The names of the calls a transaction's typed form writes out, in order, each found among names
/// its sender knows by its hash: None when one is not among them.
pub fn name_calls(tx: &Tx, known: &[String]) -> Option<Vec<String>> {
    let calls: Vec<&Tx> = match tx {
        Tx::Batch { steps } => steps.iter().collect(),
        tx => vec![tx],
    };

    calls
        .into_iter()
        .filter(|call| matches!(call, Tx::ContractCall { amount, .. } if narrowest(amount)) && (matches!(tx, Tx::Batch { .. }) || token_transfer(call).is_none()))
        .map(|call| known.iter().find_map(|name| named(call, name)))
        .collect()
}

/// What a key signs for a transaction on this chain, in EIP-712's typed form, with the names of
/// the calls it writes out: None when they are not those calls'.
pub fn transaction_digest(tx: &Tx, functions: &[String], nonce: u64, fee: &FeeOffer, genesis: &Hash) -> Option<[u8; 32]> {
    Some(digest(&domain(genesis), &message(tx, functions, nonce, fee)?))
}

/// Typed data as eth_signTypedData_v4 takes it, JSON: every struct type the message reaches, its
/// primary type, the domain and the message, numbers as decimal text, bytes as 0x hex.
#[cfg(feature = "json")]
pub fn json(domain: &Typed, message: &Typed) -> serde_json::Value {
    let mut types = serde_json::Map::new();

    for typed in [domain, message] {
        for (name, fields) in typed.definitions() {
            types.insert(name.to_string(), fields.iter().map(|(name, kind)| serde_json::json!({ "name": name, "type": kind })).collect());
        }
    }

    serde_json::json!({
        "types": types,
        "primaryType": message.name,
        "domain": json_value(&Value::Struct(domain.clone())),
        "message": json_value(&Value::Struct(message.clone())),
    })
}

#[cfg(feature = "json")]
fn json_value(value: &Value) -> serde_json::Value {
    use crate::structs::hex::to_hex;

    match value {
        Value::String(text) => text.as_str().into(),
        // Decimal text, as wide numbers are too wide for JSON's.
        Value::Uint(number) => number.to_string().into(),
        Value::Bytes(bytes) => to_hex(bytes).into(),
        Value::Bytes32(bytes) => to_hex(bytes).into(),
        Value::Address(bytes) => to_hex(bytes).into(),
        Value::Struct(inner) => inner.fields.iter().map(|(name, _, value)| (name.to_string(), json_value(value))).collect::<serde_json::Map<_, _>>().into(),
        Value::Array(items) => items.iter().map(json_value).collect(),
    }
}

#[cfg(feature = "abi")]
pub mod typed;

#[cfg(test)]
mod tests;
