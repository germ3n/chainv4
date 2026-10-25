//! EIP-712: typed data, as Ethereum's wallets sign it, so MetaMask and the hardware wallets it
//! drives can sign chainv4 transactions with keys they already hold. A wallet shows each field by
//! name and signs the keccak256 of `0x19 0x01`, the domain's hash and the message's.
//!
//! The domain is `chainv4`, version `1`, with the genesis id as its salt, so a signature holds on
//! one chain alone. Each transaction has one typed form:
//! - **`Transfer`:** from, to, amount, nonce and fee, for a coin transfer whose amount is in its
//!   narrowest width, as the wallet writes one.
//! - **`TokenTransfer`:** token, to, amount, nonce and fee, for the token standard's transfer call.
//! - **`Transaction`:** its kind and encoded bytes, nonce and fee, for every other.
//!
//! Each form maps back to one transaction alone: a transfer whose amount could be narrower, read
//! as Transfer, would sign the same as the narrow one under another id, so it goes as Transaction.
//! Addresses are their text, `ed25519:0x…`, as EIP-712's own address type holds Ethereum's alone.

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
}

/// A struct's type and values: its name, and each field's name, type and value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Typed {
    pub name: &'static str,
    pub fields: Vec<(&'static str, &'static str, Value)>,
}

impl Typed {
    /// Its type, as `encodeType` writes it: itself, then the structs it holds, by name.
    pub fn encode_type(&self) -> String {
        let mut referenced = std::collections::BTreeMap::new();
        self.referenced(&mut referenced);
        referenced.remove(self.name);

        std::iter::once(self.signature()).chain(referenced.into_values()).collect()
    }

    fn signature(&self) -> String {
        let fields: Vec<String> = self.fields.iter().map(|(name, kind, _)| format!("{kind} {name}")).collect();

        format!("{}({})", self.name, fields.join(","))
    }

    fn referenced(&self, into: &mut std::collections::BTreeMap<&'static str, String>) {
        into.insert(self.name, self.signature());

        for (_, _, value) in &self.fields {
            if let Value::Struct(inner) = value {
                inner.referenced(into);
            }
        }
    }

    /// `hashStruct`: the keccak256 of its type's hash and each field, encoded.
    pub fn hash(&self) -> [u8; 32] {
        let mut encoded = keccak(self.encode_type().as_bytes()).to_vec();

        for (_, _, value) in &self.fields {
            encoded.extend_from_slice(&match value {
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
            });
        }

        keccak(&encoded)
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

/// A transaction's typed form, with its nonce and fee offer.
pub fn message(tx: &Tx, nonce: u64, fee: &FeeOffer) -> Typed {
    let costs = [
        ("nonce", "uint64", Value::Uint(u256::from(nonce))),
        ("gasLimit", "uint64", Value::Uint(u256::from(fee.gas_limit))),
        ("maxPrice", "uint256", Value::Uint(fee.max_price)),
        ("tip", "uint256", Value::Uint(fee.tip)),
    ];
    let (name, mut fields) = match tx {
        Tx::Transfer { from, to, amount } if narrowest(amount) => (
            "Transfer",
            vec![
                ("from", "string", Value::String(from.to_string())),
                ("to", "string", Value::String(to.to_string())),
                ("amount", "uint256", Value::Uint(amount.to_u256())),
            ],
        ),
        // A build without xxh3 runs no contracts, and has no token transfer to name.
        #[cfg(feature = "xxh3")]
        Tx::ContractCall {
            to: token,
            func,
            amount: Amount::U128(0),
            data,
        } if *func == super::hasher::Hasher::digest(crate::structs::hash::HashType::Xxh3_64, b"transfer") => match data.as_slice() {
            [crate::structs::tx::TxCallArgumentType::Address(to), crate::structs::tx::TxCallArgumentType::U256(units)] => (
                "TokenTransfer",
                vec![
                    ("token", "string", Value::String(token.to_string())),
                    ("to", "string", Value::String(to.to_string())),
                    ("amount", "uint256", Value::Uint(*units)),
                ],
            ),
            _ => generic(tx),
        },
        _ => generic(tx),
    };
    fields.extend(costs);

    Typed { name, fields }
}

/// Any transaction, as its kind and encoded bytes.
fn generic(tx: &Tx) -> (&'static str, Vec<(&'static str, &'static str, Value)>) {
    let kind = format!("{tx:?}").split([' ', '{', '(']).next().unwrap_or_default().to_string();

    ("Transaction", vec![("kind", "string", Value::String(kind)), ("data", "bytes", Value::Bytes(tx.hashable_bytes_with_tag()))])
}

/// Whether an amount is written in its narrowest width: as the wallet writes one, and the one
/// encoding a Transfer form stands for.
fn narrowest(amount: &Amount) -> bool {
    match amount {
        Amount::U128(_) => true,
        Amount::U256(wide) => u128::try_from(*wide).is_err(),
    }
}

/// What a key signs for a transaction on this chain, in EIP-712's typed form.
pub fn transaction_digest(tx: &Tx, nonce: u64, fee: &FeeOffer, genesis: &Hash) -> [u8; 32] {
    digest(&domain(genesis), &message(tx, nonce, fee))
}

#[cfg(test)]
mod tests;
