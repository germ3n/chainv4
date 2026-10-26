//! Ethereum's own transactions, as MetaMask, Foundry and ethers sign them: legacy ones under
//! EIP-155, EIP-2930's with an access list, and EIP-1559's with a priority fee. Each is its RLP
//! encoding, signed over its keccak256, bound to one chain by its chain id. chainv4 carries the
//! signed bytes as they are, in Tx::Ethereum, and checks them here: the chain id, a low s, and the
//! key the signature recovers, which is the sender.
//!
//! Only what every node must agree on lives here, so any build with secp256k1 checks them alike;
//! running them is the EVM's.

#[cfg(feature = "secp256k1")]
use super::secp256k1::{keccak, recover_strict, sign_recoverable};
use {crate::structs::u256::u256, r#macro::document};

#[document(summary = "The most bytes an Ethereum-signed transaction may take: 128 KiB, as Ethereum's nodes take.", category = "Ethereum", see_also = "Tx::Ethereum")]
pub const MAX_RAW: usize = 128 * 1024;

#[document(summary = "Which of Ethereum's transaction types a transaction is: its first byte, or legacy.", category = "Ethereum", see_also = "EthereumTx")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EthereumKind {
    #[document(summary = "A legacy transaction, with its chain id in v as EIP-155 puts it.")]
    Legacy,
    #[document(summary = "EIP-2930's, type 1: a gas price and an access list.")]
    AccessList,
    #[document(summary = "EIP-1559's, type 2: a most per gas, a priority fee and an access list.")]
    DynamicFee,
}

impl EthereumKind {
    #[document(parent = "EthereumKind", summary = "The type as JSON-RPC names it: 0, 1 or 2.")]
    pub fn number(self) -> u8 {
        match self {
            EthereumKind::Legacy => 0,
            EthereumKind::AccessList => 1,
            EthereumKind::DynamicFee => 2,
        }
    }
}

#[document(summary = "One entry of an access list: an address, and the storage slots it reads.", category = "Ethereum", see_also = "EthereumTx")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccessItem {
    #[document(summary = "The address.")]
    pub address: [u8; 20],
    #[document(summary = "Its slots.")]
    pub slots: Vec<[u8; 32]>,
}

#[document(
    summary = "An Ethereum transaction, its fields read from its signed bytes.",
    category = "Ethereum",
    see_also = "Tx::Ethereum, decode",
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EthereumTx {
    #[document(summary = "Its type.")]
    pub kind: EthereumKind,
    #[document(summary = "The chain it is for.")]
    pub chain_id: u64,
    #[document(summary = "The sender's nonce.")]
    pub nonce: u64,
    #[document(summary = "The most it pays per gas: a legacy or EIP-2930 transaction's gas price, an EIP-1559 one's max fee per gas.")]
    pub max_price: u256,
    #[document(summary = "What it adds per gas for the producer: an EIP-1559 transaction's max priority fee, or for the others their whole gas price.")]
    pub tip: u256,
    #[document(summary = "The most gas it may use.")]
    pub gas_limit: u64,
    #[document(summary = "Whom it calls or pays, or None for one that creates a contract.")]
    pub to: Option<[u8; 20]>,
    #[document(summary = "What it sends, in the coin's smallest units: wei.")]
    pub value: u256,
    #[document(summary = "Its calldata, or a create's init code.")]
    pub input: Vec<u8>,
    #[document(summary = "What it says it reads ahead of time: none for a legacy transaction.")]
    pub access_list: Vec<AccessItem>,
    #[document(summary = "r and s, s low.")]
    pub signature: [u8; 64],
    #[document(summary = "The parity that recovers the key: 0 or 1.")]
    pub parity: u8,
}

/// An RLP item, as read: bytes, or a list of items.
#[derive(Debug, PartialEq, Eq)]
enum Item<'a> {
    Bytes(&'a [u8]),
    List(Vec<Item<'a>>),
}

/// Reads one item from the front of the bytes, refusing any encoding but the one RLP writes:
/// lengths at their shortest, and single low bytes as themselves.
fn item(bytes: &[u8]) -> Result<(Item<'_>, &[u8]), String> {
    let (&first, rest) = bytes.split_first().ok_or("RLP ends early")?;
    let long = |rest: &[u8], of: usize| -> Result<(usize, usize), String> {
        let digits = rest.get(..of).ok_or("RLP ends early")?;

        if digits.first() == Some(&0) {
            return Err("an RLP length with a leading zero".to_string());
        }

        let len = digits.iter().try_fold(0usize, |len, digit| len.checked_mul(256).map(|len| len + usize::from(*digit))).ok_or("an RLP length too long")?;

        if len < 56 {
            return Err("an RLP length written long that fits short".to_string());
        }

        Ok((of, len))
    };

    match first {
        0x00..=0x7f => Ok((Item::Bytes(&bytes[..1]), rest)),
        0x80..=0xb7 => {
            let (body, after) = take(rest, 0, usize::from(first - 0x80))?;

            if body.len() == 1 && body[0] < 0x80 {
                return Err("a single low byte written as a string".to_string());
            }

            Ok((Item::Bytes(body), after))
        }
        0xb8..=0xbf => {
            let (skip, len) = long(rest, usize::from(first - 0xb7))?;
            let (body, after) = take(rest, skip, len)?;

            Ok((Item::Bytes(body), after))
        }
        0xc0..=0xf7 => {
            let (body, after) = take(rest, 0, usize::from(first - 0xc0))?;

            Ok((Item::List(items(body)?), after))
        }
        0xf8..=0xff => {
            let (skip, len) = long(rest, usize::from(first - 0xf7))?;
            let (body, after) = take(rest, skip, len)?;

            Ok((Item::List(items(body)?), after))
        }
    }
}

/// The len bytes after skip, and what follows them.
fn take(rest: &[u8], skip: usize, len: usize) -> Result<(&[u8], &[u8]), String> {
    let end = skip.checked_add(len).ok_or("RLP length overflows")?;
    let body = rest.get(skip..end).ok_or("RLP ends early")?;

    Ok((body, &rest[end..]))
}

fn items(mut body: &[u8]) -> Result<Vec<Item<'_>>, String> {
    let mut found = Vec::new();

    while !body.is_empty() {
        let (next, rest) = item(body)?;
        found.push(next);
        body = rest;
    }

    Ok(found)
}

fn bytes<'a>(item: &Item<'a>, what: &str) -> Result<&'a [u8], String> {
    match item {
        Item::Bytes(bytes) => Ok(bytes),
        Item::List(_) => Err(format!("{what} is a list, not bytes")),
    }
}

/// A whole number, as RLP writes one: big-endian, no leading zeros, empty for zero.
fn number(item: &Item, what: &str) -> Result<u256, String> {
    let digits = bytes(item, what)?;

    if digits.len() > 32 {
        return Err(format!("{what} is more than 256 bits"));
    }

    if digits.first() == Some(&0) {
        return Err(format!("{what} has a leading zero"));
    }

    let mut word = [0; 32];
    word[32 - digits.len()..].copy_from_slice(digits);

    Ok(u256::from_be_bytes(word))
}

fn small(item: &Item, what: &str) -> Result<u64, String> {
    u64::try_from(number(item, what)?).map_err(|_| format!("{what} is more than 64 bits"))
}

fn word(item: &Item, what: &str) -> Result<[u8; 32], String> {
    let value = number(item, what)?;

    Ok(value.to_be_bytes())
}

fn to_field(item: &Item) -> Result<Option<[u8; 20]>, String> {
    match bytes(item, "to")? {
        [] => Ok(None),
        address => Ok(Some(address.try_into().map_err(|_| "to is not 20 bytes")?)),
    }
}

fn access_list(item: &Item) -> Result<Vec<AccessItem>, String> {
    let Item::List(entries) = item else {
        return Err("the access list is not a list".to_string());
    };

    entries
        .iter()
        .map(|entry| match entry {
            Item::List(pair) if pair.len() == 2 => {
                let address = bytes(&pair[0], "an access list address")?.try_into().map_err(|_| "an access list address is not 20 bytes")?;
                let Item::List(slots) = &pair[1] else {
                    return Err("an access list's slots are not a list".to_string());
                };
                let slots = slots
                    .iter()
                    .map(|slot| bytes(slot, "a slot")?.try_into().map_err(|_| "a slot is not 32 bytes".to_string()))
                    .collect::<Result<_, _>>()?;

                Ok(AccessItem { address, slots })
            }
            _ => Err("an access list entry is not an address and its slots".to_string()),
        })
        .collect()
}

#[document(
    summary = "Reads a signed Ethereum transaction from its bytes, as eth_sendRawTransaction takes them: a legacy one, which must carry its chain id as EIP-155 says, or a type 1 or 2 one. Checks only that it reads; verify checks its signature.",
    category = "Ethereum",
    returns = "The transaction, or why its bytes are not one.",
)]
pub fn decode(raw: &[u8]) -> Result<EthereumTx, String> {
    if raw.len() > MAX_RAW {
        return Err(format!("an Ethereum transaction takes at most {MAX_RAW} bytes"));
    }

    let (kind, body) = match raw.first() {
        Some(1) => (EthereumKind::AccessList, &raw[1..]),
        Some(2) => (EthereumKind::DynamicFee, &raw[1..]),
        Some(first) if *first >= 0xc0 => (EthereumKind::Legacy, raw),
        _ => return Err("not an Ethereum transaction this chain takes: legacy, or of type 1 or 2".to_string()),
    };
    let (Item::List(fields), []) = item(body)? else {
        return Err("an Ethereum transaction is one RLP list".to_string());
    };
    let signature = |r: &Item, s: &Item| -> Result<[u8; 64], String> { Ok([word(r, "r")?, word(s, "s")?].concat().try_into().expect("64 bytes")) };

    match (kind, fields.as_slice()) {
        (EthereumKind::Legacy, [nonce, price, gas, to, value, input, v, r, s]) => {
            let v = small(v, "v")?;

            if v < 35 {
                return Err("a legacy transaction must carry its chain id, as EIP-155 says: replayable on any chain otherwise".to_string());
            }

            let price = number(price, "the gas price")?;

            Ok(EthereumTx {
                kind,
                chain_id: (v - 35) / 2,
                nonce: small(nonce, "the nonce")?,
                max_price: price,
                tip: price,
                gas_limit: small(gas, "the gas limit")?,
                to: to_field(to)?,
                value: number(value, "the value")?,
                input: bytes(input, "the input")?.to_vec(),
                access_list: Vec::new(),
                signature: signature(r, s)?,
                parity: ((v - 35) % 2) as u8,
            })
        }
        (EthereumKind::AccessList, [chain, nonce, price, gas, to, value, input, list, parity, r, s]) => {
            let price = number(price, "the gas price")?;

            Ok(EthereumTx {
                kind,
                chain_id: small(chain, "the chain id")?,
                nonce: small(nonce, "the nonce")?,
                max_price: price,
                tip: price,
                gas_limit: small(gas, "the gas limit")?,
                to: to_field(to)?,
                value: number(value, "the value")?,
                input: bytes(input, "the input")?.to_vec(),
                access_list: access_list(list)?,
                signature: signature(r, s)?,
                parity: u8::try_from(small(parity, "the parity")?).map_err(|_| "the parity is 0 or 1")?,
            })
        }
        (EthereumKind::DynamicFee, [chain, nonce, tip, max, gas, to, value, input, list, parity, r, s]) => Ok(EthereumTx {
            kind,
            chain_id: small(chain, "the chain id")?,
            nonce: small(nonce, "the nonce")?,
            max_price: number(max, "the max fee per gas")?,
            tip: number(tip, "the max priority fee per gas")?,
            gas_limit: small(gas, "the gas limit")?,
            to: to_field(to)?,
            value: number(value, "the value")?,
            input: bytes(input, "the input")?.to_vec(),
            access_list: access_list(list)?,
            signature: signature(r, s)?,
            parity: u8::try_from(small(parity, "the parity")?).map_err(|_| "the parity is 0 or 1")?,
        }),
        _ => Err("an Ethereum transaction with the wrong number of fields for its type".to_string()),
    }
}

/// RLP's encoding of bytes.
fn put_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    match bytes {
        [low] if *low < 0x80 => out.push(*low),
        _ => {
            put_length(out, bytes.len(), 0x80);
            out.extend_from_slice(bytes);
        }
    }
}

fn put_length(out: &mut Vec<u8>, len: usize, base: u8) {
    if len < 56 {
        out.push(base + len as u8);
    } else {
        let digits: Vec<u8> = len.to_be_bytes().into_iter().skip_while(|digit| *digit == 0).collect();
        out.push(base + 55 + digits.len() as u8);
        out.extend(digits);
    }
}

fn put_number(out: &mut Vec<u8>, value: u256) {
    let word = value.to_be_bytes();
    let first = word.iter().position(|digit| *digit != 0).unwrap_or(32);

    put_bytes(out, &word[first..]);
}

fn put_list(out: &mut Vec<u8>, body: &[u8]) {
    put_length(out, body.len(), 0xc0);
    out.extend_from_slice(body);
}

impl EthereumTx {
    /// The fields every type shares and signs, in its order, before any signature.
    fn fields(&self, signed: bool) -> Vec<u8> {
        let mut body = Vec::new();
        let number = |body: &mut Vec<u8>, value: u64| put_number(body, u256::from(value));
        let list = |body: &mut Vec<u8>| {
            let mut entries = Vec::new();

            for entry in &self.access_list {
                let mut pair = Vec::new();
                put_bytes(&mut pair, &entry.address);
                let mut slots = Vec::new();

                for slot in &entry.slots {
                    put_bytes(&mut slots, slot);
                }

                put_list(&mut pair, &slots);
                put_list(&mut entries, &pair);
            }

            put_list(body, &entries);
        };
        let to = self.to.map(|to| to.to_vec()).unwrap_or_default();
        let (r, s) = (u256::from_be_bytes(self.signature[..32].try_into().expect("32")), u256::from_be_bytes(self.signature[32..].try_into().expect("32")));

        match self.kind {
            EthereumKind::Legacy => {
                number(&mut body, self.nonce);
                put_number(&mut body, self.max_price);
                number(&mut body, self.gas_limit);
                put_bytes(&mut body, &to);
                put_number(&mut body, self.value);
                put_bytes(&mut body, &self.input);

                match signed {
                    true => {
                        number(&mut body, self.chain_id * 2 + 35 + u64::from(self.parity));
                        put_number(&mut body, r);
                        put_number(&mut body, s);
                    }
                    false => {
                        number(&mut body, self.chain_id);
                        number(&mut body, 0);
                        number(&mut body, 0);
                    }
                }
            }
            EthereumKind::AccessList | EthereumKind::DynamicFee => {
                number(&mut body, self.chain_id);
                number(&mut body, self.nonce);

                match self.kind {
                    EthereumKind::DynamicFee => {
                        put_number(&mut body, self.tip);
                        put_number(&mut body, self.max_price);
                    }
                    _ => put_number(&mut body, self.max_price),
                }

                number(&mut body, self.gas_limit);
                put_bytes(&mut body, &to);
                put_number(&mut body, self.value);
                put_bytes(&mut body, &self.input);
                list(&mut body);

                if signed {
                    number(&mut body, u64::from(self.parity));
                    put_number(&mut body, r);
                    put_number(&mut body, s);
                }
            }
        }

        let mut out = match self.kind {
            EthereumKind::Legacy => Vec::new(),
            kind => vec![kind.number()],
        };
        put_list(&mut out, &body);

        out
    }

    #[cfg(feature = "secp256k1")]
    #[document(parent = "EthereumTx", summary = "What its signature signs: the keccak256 of its fields, typed as its type says, with EIP-155's chain id for a legacy one.")]
    pub fn signing_hash(&self) -> [u8; 32] {
        keccak(&self.fields(false))
    }

    #[document(parent = "EthereumTx", summary = "Its signed bytes, as eth_sendRawTransaction takes them: what decode reads.")]
    pub fn encode(&self) -> Vec<u8> {
        self.fields(true)
    }

    #[cfg(feature = "secp256k1")]
    #[document(parent = "EthereumTx", summary = "The compressed public key its signature recovers: its sender's. None for a signature that recovers none, or has a high s.")]
    pub fn signer(&self) -> Option<[u8; 33]> {
        recover_strict(&self.signing_hash(), &self.signature, self.parity)
    }

    #[cfg(feature = "secp256k1")]
    #[document(parent = "EthereumTx", summary = "Signs it with a secp256k1 key, as a wallet does, for tests and tools: what was in its signature is replaced.")]
    pub fn signed(mut self, secret: &[u8; 32]) -> EthereumTx {
        let (signature, parity) = sign_recoverable(secret, &self.signing_hash());
        self.signature = signature;
        self.parity = parity;

        self
    }
}

#[document(
    summary = "The chain id Ethereum transactions are signed for, EIP-155's: the first six bytes of the genesis id, so every chain has its own and MetaMask, which holds ids to 53 bits, takes it.",
    category = "Ethereum",
    see_also = "EthereumTx",
)]
pub fn chain_id(genesis: &crate::structs::hash::Hash) -> u64 {
    let bytes = genesis.to_bytes();
    let mut id = [0; 8];
    id[2..].copy_from_slice(&bytes[..6]);

    u64::from_be_bytes(id).max(1)
}

#[cfg(feature = "secp256k1")]
#[document(summary = "A signed Ethereum transaction's hash, as Ethereum's tools name it: the keccak256 of its bytes.", category = "Ethereum", see_also = "decode")]
pub fn tx_hash(raw: &[u8]) -> [u8; 32] {
    keccak(raw)
}

#[cfg(all(test, feature = "secp256k1"))]
mod tests;
