//! The offline signer as MetaMask's QR hardware wallet, as a Keystone is: MetaMask pairs with it
//! by reading its extended public key, and asks it to sign by showing a request it reads back the
//! signature of. The codes are Uniform Resources, as ERC-4527 and Keystone's registry have them:
//! CBOR, in bytewords, and when too long for one QR code, fountain-coded parts that animate.
//!
//! - **Pairing:** `ur:crypto-hdkey`, the public key and chain code at m/44'/60'/0', Ethereum's
//!   account level, and the seed's fingerprint. MetaMask derives accounts 0/n below it, as the
//!   wallet does, and lists them to add.
//! - **A request:** `ur:eth-sign-request`, what to sign and which kind: a legacy or a typed
//!   transaction, unsigned, typed data, or a message; the chain; and the key, by its path below
//!   the seed's fingerprint.
//! - **The signature:** `ur:eth-signature`, r, s and v as Ethereum takes them for that kind, back
//!   under the request's id.
//!
//! The signer reads every request whole before it signs: what it shows is what it signs.

use {
    chainv4::crypto::eip712::typed::TypedData,
    chainv4::crypto::ethereum::{EthereumKind, EthereumTx, decode_unsigned},
    chainv4::crypto::keypair::{KeyType, PrivateKey},
    chainv4::crypto::mnemonic::{ExtendedPublic, Seed},
    chainv4::crypto::secp256k1,
    chainv4::structs::hex::to_hex,
    minicbor::data::{Tag, Type},
    minicbor::{Decoder, Encoder},
};

/// The CBOR tags of the registry's types, as BCR-2020-006 numbers them.
const UUID: u64 = 37;
const COIN_INFO: u64 = 305;
const KEYPATH: u64 = 304;

/// Ethereum's coin type, as SLIP-44 and the registry's coin info have it.
const ETHEREUM: u64 = 60;

/// The UR types this module reads and writes.
pub const HDKEY_TYPE: &str = "crypto-hdkey";
pub const REQUEST_TYPE: &str = "eth-sign-request";
pub const SIGNATURE_TYPE: &str = "eth-signature";

/// What the signer calls itself to MetaMask, as a Keystone calls itself Keystone.
pub const NAME: &str = "chainv4";
/// Which way MetaMask reads the accounts below the key: BIP44's, 0/n.
const STANDARD: &str = "account.standard";

/// One step of a key's path: an index, hardened or not, or any index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Index(u32, bool),
    Any(bool),
}

/// A key's path, as the registry's crypto-keypath has one: its steps, and the fingerprint of the
/// key it starts at.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KeyPath {
    pub steps: Vec<Step>,
    pub fingerprint: Option<[u8; 4]>,
    pub depth: Option<u8>,
}

impl KeyPath {
    /// As wallets write paths: 44'/60'/0'/0/3.
    pub fn text(&self) -> String {
        self.steps
            .iter()
            .map(|step| match step {
                Step::Index(index, true) => format!("{index}'"),
                Step::Index(index, false) => index.to_string(),
                Step::Any(true) => "*'".to_string(),
                Step::Any(false) => "*".to_string(),
            })
            .collect::<Vec<_>>()
            .join("/")
    }

    /// The account this wallet signs with at a path, as MetaMask asks for one: m/44'/60'/0'/0/n.
    pub fn account(&self) -> Option<u32> {
        match self.steps.as_slice() {
            [Step::Index(44, true), Step::Index(60, true), Step::Index(0, true), Step::Index(0, false), Step::Index(account, false)] => Some(*account),
            _ => None,
        }
    }

    fn encode(&self, e: &mut Encoder<Vec<u8>>) -> Result<(), String> {
        let entries = 1 + u64::from(self.fingerprint.is_some()) + u64::from(self.depth.is_some());
        e.tag(Tag::new(KEYPATH)).and_then(|e| e.map(entries)).and_then(|e| e.u8(1)).and_then(|e| e.array(2 * self.steps.len() as u64)).map_err(text)?;

        for step in &self.steps {
            match step {
                Step::Index(index, hardened) => e.u32(*index).and_then(|e| e.bool(*hardened)),
                Step::Any(hardened) => e.array(0).and_then(|e| e.bool(*hardened)),
            }
            .map_err(text)?;
        }

        if let Some(fingerprint) = self.fingerprint {
            e.u8(2).and_then(|e| e.u32(u32::from_be_bytes(fingerprint))).map_err(text)?;
        }

        if let Some(depth) = self.depth {
            e.u8(3).and_then(|e| e.u8(depth)).map_err(text)?;
        }

        Ok(())
    }

    fn decode(d: &mut Decoder) -> Result<KeyPath, String> {
        expect_tag(d, KEYPATH)?;
        let mut path = KeyPath::default();

        for _ in 0..d.map().map_err(text)?.ok_or("a key path of unknown length")? {
            match d.u8().map_err(text)? {
                1 => {
                    let count = d.array().map_err(text)?.ok_or("a key path of unknown length")?;

                    if count % 2 != 0 || count > 64 {
                        return Err("a key path's steps come in pairs".to_string());
                    }

                    for _ in 0..count / 2 {
                        let step = match d.datatype().map_err(text)? {
                            Type::Array => {
                                d.array().map_err(text)?;
                                Step::Any(d.bool().map_err(text)?)
                            }
                            _ => {
                                let index = d.u32().map_err(text)?;
                                Step::Index(index, d.bool().map_err(text)?)
                            }
                        };
                        path.steps.push(step);
                    }
                }
                2 => path.fingerprint = Some(d.u32().map_err(text)?.to_be_bytes()),
                3 => path.depth = Some(d.u8().map_err(text)?),
                _ => d.skip().map_err(text)?,
            }
        }

        Ok(path)
    }
}

/// A key as the registry's crypto-hdkey has one, extended, with where it sits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HdKey {
    pub key: [u8; 33],
    pub chain_code: Option<[u8; 32]>,
    /// The coin and network it is for: Ethereum's coin type, 60, for this wallet's.
    pub coin: Option<(Option<u64>, Option<u64>)>,
    pub origin: Option<KeyPath>,
    pub children: Option<KeyPath>,
    pub parent_fingerprint: Option<[u8; 4]>,
    pub name: Option<String>,
    pub note: Option<String>,
}

impl HdKey {
    /// The account level's extended key, as MetaMask pairs with it: accounts 0/n below it.
    pub fn ethereum(account: &ExtendedPublic) -> HdKey {
        HdKey {
            key: account.public,
            chain_code: Some(account.chain_code),
            coin: Some((Some(ETHEREUM), None)),
            origin: Some(KeyPath {
                steps: vec![Step::Index(44, true), Step::Index(60, true), Step::Index(0, true)],
                fingerprint: Some(account.master_fingerprint),
                depth: Some(account.depth),
            }),
            children: Some(KeyPath {
                steps: vec![Step::Index(0, false), Step::Any(false)],
                fingerprint: None,
                depth: None,
            }),
            parent_fingerprint: Some(account.parent_fingerprint),
            name: Some(NAME.to_string()),
            note: Some(STANDARD.to_string()),
        }
    }

    /// Its CBOR, as the registry writes it: each field there is, in order of its key.
    pub fn to_cbor(&self) -> Result<Vec<u8>, String> {
        let fields = [
            true,
            self.chain_code.is_some(),
            self.coin.is_some(),
            self.origin.is_some(),
            self.children.is_some(),
            self.parent_fingerprint.is_some(),
            self.name.is_some(),
            self.note.is_some(),
        ];
        let mut e = Encoder::new(Vec::new());
        e.map(fields.iter().filter(|there| **there).count() as u64).and_then(|e| e.u8(3)).and_then(|e| e.bytes(&self.key)).map_err(text)?;

        if let Some(chain_code) = &self.chain_code {
            e.u8(4).and_then(|e| e.bytes(chain_code)).map_err(text)?;
        }

        if let Some((coin, network)) = self.coin {
            let entries = u64::from(coin.is_some()) + u64::from(network.is_some());
            e.u8(5).and_then(|e| e.tag(Tag::new(COIN_INFO))).and_then(|e| e.map(entries)).map_err(text)?;

            if let Some(coin) = coin {
                e.u8(1).and_then(|e| e.u64(coin)).map_err(text)?;
            }

            if let Some(network) = network {
                e.u8(2).and_then(|e| e.u64(network)).map_err(text)?;
            }
        }

        if let Some(origin) = &self.origin {
            e.u8(6).map_err(text)?;
            origin.encode(&mut e)?;
        }

        if let Some(children) = &self.children {
            e.u8(7).map_err(text)?;
            children.encode(&mut e)?;
        }

        if let Some(fingerprint) = self.parent_fingerprint {
            e.u8(8).and_then(|e| e.u32(u32::from_be_bytes(fingerprint))).map_err(text)?;
        }

        if let Some(name) = &self.name {
            e.u8(9).and_then(|e| e.str(name)).map_err(text)?;
        }

        if let Some(note) = &self.note {
            e.u8(10).and_then(|e| e.str(note)).map_err(text)?;
        }

        Ok(e.into_writer())
    }
}

/// What a request asks to be signed, as ERC-4527 numbers the kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DataKind {
    /// A legacy transaction, unsigned, with EIP-155's chain id.
    Transaction,
    /// EIP-712 typed data, as JSON.
    TypedData,
    /// A message, as personal_sign signs it.
    Message,
    /// A typed transaction, EIP-2930's or EIP-1559's, unsigned.
    TypedTransaction,
}

/// A request MetaMask shows: eth-sign-request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignRequest {
    /// Its id, a UUID: what the signature goes back under.
    pub id: Option<Vec<u8>>,
    pub data: Vec<u8>,
    pub kind: DataKind,
    pub chain_id: Option<u64>,
    pub path: KeyPath,
    pub address: Option<[u8; 20]>,
    /// Who asks, as it names itself: metamask.
    pub origin: Option<String>,
}

impl SignRequest {
    /// Reads one from its CBOR.
    pub fn from_cbor(bytes: &[u8]) -> Result<SignRequest, String> {
        let mut d = Decoder::new(bytes);
        let (mut id, mut data, mut kind, mut chain_id, mut path, mut address, mut origin) = (None, None, DataKind::Transaction, None, None, None, None);

        for _ in 0..d.map().map_err(text)?.ok_or("a request of unknown length")? {
            match d.u8().map_err(text)? {
                1 => {
                    expect_tag(&mut d, UUID)?;
                    id = Some(d.bytes().map_err(text)?.to_vec());
                }
                2 => data = Some(d.bytes().map_err(text)?.to_vec()),
                3 => {
                    kind = match d.u8().map_err(text)? {
                        1 => DataKind::Transaction,
                        2 => DataKind::TypedData,
                        3 => DataKind::Message,
                        4 => DataKind::TypedTransaction,
                        other => return Err(format!("a kind of data to sign this wallet does not know: {other}")),
                    }
                }
                4 => chain_id = Some(d.u64().map_err(|_| "a chain id this wallet does not read".to_string())?),
                5 => path = Some(KeyPath::decode(&mut d)?),
                6 => address = Some(d.bytes().map_err(text)?.try_into().map_err(|_| "an address is 20 bytes")?),
                7 => origin = Some(d.str().map_err(text)?.to_string()),
                _ => d.skip().map_err(text)?,
            }
        }

        if d.position() != bytes.len() {
            return Err("a request with more in it than it should hold".to_string());
        }

        Ok(SignRequest {
            id,
            data: data.ok_or("a request with nothing to sign")?,
            kind,
            chain_id,
            path: path.ok_or("a request that names no key")?,
            address,
            origin,
        })
    }
}

impl SignRequest {
    /// Its CBOR, as MetaMask writes it: what from_cbor reads.
    pub fn to_cbor(&self) -> Result<Vec<u8>, String> {
        let entries = 3 + [self.id.is_some(), self.chain_id.is_some(), self.address.is_some(), self.origin.is_some()].iter().filter(|there| **there).count() as u64;
        let kind = match self.kind {
            DataKind::Transaction => 1,
            DataKind::TypedData => 2,
            DataKind::Message => 3,
            DataKind::TypedTransaction => 4,
        };
        let mut e = Encoder::new(Vec::new());
        e.map(entries).map_err(text)?;

        if let Some(id) = &self.id {
            e.u8(1).and_then(|e| e.tag(Tag::new(UUID))).and_then(|e| e.bytes(id)).map_err(text)?;
        }

        e.u8(2).and_then(|e| e.bytes(&self.data)).and_then(|e| e.u8(3)).and_then(|e| e.u8(kind)).map_err(text)?;

        if let Some(chain_id) = self.chain_id {
            e.u8(4).and_then(|e| e.u64(chain_id)).map_err(text)?;
        }

        e.u8(5).map_err(text)?;
        self.path.encode(&mut e)?;

        if let Some(address) = &self.address {
            e.u8(6).and_then(|e| e.bytes(address)).map_err(text)?;
        }

        if let Some(origin) = &self.origin {
            e.u8(7).and_then(|e| e.str(origin)).map_err(text)?;
        }

        Ok(e.into_writer())
    }
}

/// What the signer signed, for MetaMask to read back: eth-signature.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signature {
    pub id: Option<Vec<u8>>,
    /// r, s, and v as Ethereum takes it for the kind signed.
    pub signature: Vec<u8>,
    pub origin: Option<String>,
}

impl Signature {
    pub fn to_cbor(&self) -> Result<Vec<u8>, String> {
        let entries = 1 + u64::from(self.id.is_some()) + u64::from(self.origin.is_some());
        let mut e = Encoder::new(Vec::new());
        e.map(entries).map_err(text)?;

        if let Some(id) = &self.id {
            e.u8(1).and_then(|e| e.tag(Tag::new(UUID))).and_then(|e| e.bytes(id)).map_err(text)?;
        }

        e.u8(2).and_then(|e| e.bytes(&self.signature)).map_err(text)?;

        if let Some(origin) = &self.origin {
            e.u8(3).and_then(|e| e.str(origin)).map_err(text)?;
        }

        Ok(e.into_writer())
    }
}

/// A request laid out to check before signing.
#[derive(Clone, Debug)]
pub enum Asked {
    /// A transaction, read from what is signed.
    Transaction(Box<EthereumTx>),
    TypedData(Box<TypedData>, serde_json::Value),
    /// A message: text when it is text.
    Message(Vec<u8>),
}

impl SignRequest {
    /// The account at its path, which must be one this wallet derives: m/44'/60'/0'/0/n.
    pub fn account(&self) -> Result<u32, String> {
        self.path.account().ok_or_else(|| format!("a key at m/{}, which this wallet holds none at: it signs at m/44'/60'/0'/0/n", self.path.text()))
    }

    /// What it asks, read whole: a transaction that does not read, or is for another chain than
    /// the request says, is refused, as is typed data that does not hash.
    pub fn asked(&self) -> Result<Asked, String> {
        match self.kind {
            DataKind::Transaction | DataKind::TypedTransaction => {
                let tx = decode_unsigned(&self.data)?;
                let typed = tx.kind != EthereumKind::Legacy;

                if typed != (self.kind == DataKind::TypedTransaction) {
                    return Err("a transaction of another type than the request says".to_string());
                }

                if self.chain_id.is_some_and(|chain| chain != tx.chain_id) {
                    return Err(format!("a transaction for chain {}, in a request for chain {}", tx.chain_id, self.chain_id.unwrap_or_default()));
                }

                Ok(Asked::Transaction(Box::new(tx)))
            }
            DataKind::TypedData => {
                let json: serde_json::Value = serde_json::from_slice(&self.data).map_err(|_| "typed data that is not JSON")?;
                let data = TypedData::parse(&json)?;
                data.digest()?;

                Ok(Asked::TypedData(Box::new(data), json))
            }
            DataKind::Message => Ok(Asked::Message(self.data.clone())),
        }
    }

    /// The chain it is for: the transaction's, or the request's, or typed data's domain's.
    pub fn chain(&self) -> Option<u64> {
        match self.asked() {
            Ok(Asked::Transaction(tx)) => Some(tx.chain_id),
            Ok(Asked::TypedData(_, json)) => match &json["domain"]["chainId"] {
                serde_json::Value::Number(number) => number.as_u64(),
                serde_json::Value::String(text) => match text.strip_prefix("0x") {
                    Some(hex) => u64::from_str_radix(hex, 16).ok(),
                    None => text.parse().ok(),
                },
                _ => self.chain_id,
            },
            _ => self.chain_id,
        }
    }

    /// Signs it with the seed's key at its path: the signature MetaMask reads back. Refuses a
    /// request for another seed's keys, one whose address is not the key's, and one that does
    /// not read.
    pub fn sign(&self, seed: &Seed) -> Result<Signature, String> {
        let fingerprint = seed.ethereum_account().master_fingerprint;

        if self.path.fingerprint.is_some_and(|asked| asked != fingerprint) {
            return Err("a request for another wallet's keys: pair MetaMask with this one".to_string());
        }

        let account = self.account()?;
        let key = seed.key(KeyType::Secp256k1, account);
        let PrivateKey::Secp256k1(secret) = key.private() else {
            unreachable!("a secp256k1 key");
        };

        if let Some(address) = self.address
            && key.public().address() != chainv4::structs::address::Address::Secp256k1(address)
        {
            return Err(format!("a request to sign as {}, which is not this wallet's account {account}", to_hex(&address)));
        }

        let digest = match self.asked()? {
            Asked::Transaction(tx) => tx.signing_hash(),
            Asked::TypedData(data, _) => data.digest()?,
            Asked::Message(message) => crate::metamask::personal_digest(&message),
        };
        let (rs, parity) = secp256k1::sign_recoverable(secret, &digest);
        // v as Ethereum takes it: a typed transaction's parity alone, a legacy one's EIP-155 v,
        // and 27 or 28 for a message or typed data.
        let v: Vec<u8> = match (self.kind, self.asked()?) {
            (DataKind::TypedTransaction, _) => vec![parity],
            (DataKind::Transaction, Asked::Transaction(tx)) => {
                let v = tx.chain_id * 2 + 35 + u64::from(parity);
                let bytes = v.to_be_bytes();

                bytes[bytes.iter().position(|byte| *byte != 0).unwrap_or(7)..].to_vec()
            }
            _ => vec![27 + parity],
        };

        Ok(Signature {
            id: self.id.clone(),
            signature: [rs.as_slice(), &v].concat(),
            origin: Some(NAME.to_string()),
        })
    }
}

/// A UR as a QR code holds it best: in capitals, which QR's alphanumeric mode packs densest.
pub fn ur(kind: &str, cbor: &[u8]) -> String {
    ur::encode(cbor, &ur::Type::Custom(kind)).to_ascii_uppercase()
}

/// MetaMask's pairing code for a seed: its account level's extended key.
pub fn pairing(seed: &Seed) -> Result<String, String> {
    Ok(ur(HDKEY_TYPE, &HdKey::ethereum(&seed.ethereum_account()).to_cbor()?))
}

/// Reading a UR, whole or in parts, in any order: MetaMask's animated codes are fountain-coded, so
/// any parts enough to cover the message will do.
#[derive(Default)]
pub struct Assembly {
    decoder: ur::Decoder,
    /// How many parts the message was cut into, once one was read.
    count: Option<usize>,
}

impl std::fmt::Debug for Assembly {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Assembly").field("count", &self.count).finish_non_exhaustive()
    }
}

impl Assembly {
    /// Takes a code's text: a UR of the type wanted, whole or a part. Gives its CBOR once it is
    /// all in. A part of another message starts over with it.
    pub fn add(&mut self, text: &str, wanted: &str) -> Result<Option<Vec<u8>>, String> {
        let lower = text.trim().to_ascii_lowercase();
        let kind = lower.strip_prefix("ur:").and_then(|rest| rest.split('/').next()).ok_or("not a UR code")?;

        if kind != wanted {
            return Err(format!("a {kind} code, not the {wanted} this wallet reads here"));
        }

        match ur::decode(&lower).map_err(|err| format!("a UR code that does not read: {err:?}"))? {
            (ur::ur::Kind::SinglePart, cbor) => {
                *self = Assembly::default();

                Ok(Some(cbor))
            }
            (ur::ur::Kind::MultiPart, _) => {
                let count = lower.split('/').nth(1).and_then(|indices| indices.split_once('-')).and_then(|(_, count)| count.parse().ok());

                if self.count.is_some() && self.count != count {
                    *self = Assembly::default();
                }

                self.count = count;

                if let Err(err) = self.decoder.receive(&lower) {
                    // A part of another message: start over with it.
                    *self = Assembly { count, ..Assembly::default() };
                    self.decoder.receive(&lower).map_err(|_| format!("a UR part that does not read: {err:?}"))?;
                }

                match self.decoder.complete() {
                    true => {
                        let message = self.decoder.message().map_err(|err| format!("{err:?}"))?;
                        *self = Assembly::default();

                        Ok(message)
                    }
                    false => Ok(None),
                }
            }
        }
    }

    /// How many of the message's parts are in, of how many; None before the first part.
    pub fn progress(&self) -> Option<(usize, usize)> {
        Some((self.decoder.resolved_fragment_count().unwrap_or(0), self.count?))
    }
}

/// The names of chains a signer meets most: what the review calls a chain by, beside its id.
pub fn chain_name(chain: u64, yours: Option<u64>) -> String {
    match chain {
        chain if Some(chain) == yours => "your chain".to_string(),
        1 => "Ethereum".to_string(),
        11_155_111 => "Sepolia".to_string(),
        17_000 => "Holesky".to_string(),
        10 => "Optimism".to_string(),
        137 => "Polygon".to_string(),
        8_453 => "Base".to_string(),
        42_161 => "Arbitrum One".to_string(),
        56 => "BNB Smart Chain".to_string(),
        other => format!("chain {other}"),
    }
}

fn expect_tag(d: &mut Decoder, tag: u64) -> Result<(), String> {
    match d.tag().map_err(text)?.as_u64() {
        found if found == tag => Ok(()),
        found => Err(format!("CBOR tag {found} where {tag} belongs")),
    }
}

fn text(err: impl std::fmt::Display) -> String {
    format!("a code that does not read: {err}")
}

#[cfg(test)]
mod tests;
