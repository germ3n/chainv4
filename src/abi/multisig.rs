//! A native multisig's side on EVM chains: contracts/evm/MultiSig.sol, which its members' EVM keys
//! control, M of N. It sits at one address on every EVM chain, deployed through the deterministic
//! deployment proxy with no salt, its init code naming the chainv4 multisig and the owners and
//! threshold it starts with, so anyone works out the address before it exists, and coins sent there
//! first wait for it.
//!
//! What it runs, it runs on threshold of its owners' signatures over EIP-712's digest of the call
//! and its nonce, under the contract's own domain on its chain. An EvmBundle carries a call round
//! the owners as they sign, and becomes execute's calldata once enough have.

use {
    super::{AbiType, AbiValue, decode_values, encode_values, keccak},
    crate::crypto::hashable::Hashable,
    crate::crypto::keypair::PublicKey,
    crate::registry::multisig::{EvmKind, EvmOwner, EvmStart},
    crate::structs::address::Address,
    crate::structs::hex::{from_hex, to_hex},
    crate::structs::u256::{U256Le, u256},
    r#macro::document,
    wincode::{SchemaRead, SchemaWrite},
};

#[document(summary = "MultiSig.sol's init code, as solc wrote it for Paris.", category = "Multisig")]
pub const MULTISIG_CODE: &str = include_str!("../../contracts/evm/MultiSig.bin");

/// The deterministic deployment proxy, at one address on most EVM chains.
const PROXY: [u8; 20] = [0x4e, 0x59, 0xb4, 0x48, 0x47, 0xb3, 0x79, 0x57, 0x85, 0x88, 0x92, 0x0c, 0xa7, 0x8f, 0xbf, 0x26, 0xc0, 0xb4, 0x95, 0x6c];

fn word(bytes: &[u8]) -> [u8; 32] {
    let mut word = [0; 32];
    word[32 - bytes.len()..].copy_from_slice(bytes);

    word
}

fn number(value: u256) -> [u8; 32] {
    value.to_be_bytes()
}

fn selector(signature: &str) -> [u8; 4] {
    keccak(signature.as_bytes())[..4].try_into().expect("4 bytes")
}

/// What MultiSig.sol takes its owners as: each one's kind, then its key, in their order.
fn owner_lists(owners: &[EvmOwner]) -> [AbiValue; 2] {
    let (kinds, keys): (Vec<AbiValue>, Vec<AbiValue>) = owners
        .iter()
        .map(|owner| {
            let (kind, key) = owner.kind.contract_key(&owner.address);

            (AbiValue::Uint(u256::from(u64::from(kind))), AbiValue::FixedBytes(key.to_vec()))
        })
        .unzip();

    [AbiValue::Array(kinds), AbiValue::Array(keys)]
}

fn kinds_type() -> AbiType {
    AbiType::Array(Box::new(AbiType::Uint(8)))
}

fn keys_type() -> AbiType {
    AbiType::Array(Box::new(AbiType::FixedBytes(32)))
}

#[document(
    summary = "MultiSig.sol's init code for a multisig: its code, then its constructor's arguments, the chainv4 multisig's address, chainv4's chain id, the owners' kinds and keys, sorted by address, and the threshold it starts with.",
    category = "Multisig",
    params = { multisig = "The chainv4 multisig.", start = "The owners and threshold it starts with.", home = "chainv4's EVM chain id, ethereum::chain_id of its genesis: where the contract trusts chainv4's crypto precompile to check Ed25519." },
)]
pub fn multisig_init_code(multisig: &Address, start: &EvmStart, home: u64) -> Vec<u8> {
    let mut init = from_hex(&format!("0x{}", MULTISIG_CODE.trim())).expect("MultiSig.bin is hex");
    let [kinds, keys] = owner_lists(&start.owners);
    let types = [AbiType::FixedBytes(32), AbiType::Uint(256), kinds_type(), keys_type(), AbiType::Uint(256)];
    let values = [
        AbiValue::FixedBytes(multisig.bytes().to_vec()),
        AbiValue::Uint(u256::from(home)),
        kinds,
        keys,
        AbiValue::Uint(u256::from(u64::from(start.threshold))),
    ];
    init.extend(encode_values(&types, &values).expect("the constructor's arguments encode"));

    init
}

#[document(
    summary = "Where a multisig's MultiSig.sol sits on every EVM chain: CREATE2's address for the deterministic deployment proxy, no salt, and its init code. The same before it is deployed as after.",
    category = "Multisig",
    see_also = "multisig_init_code, EvmStart",
)]
pub fn multisig_contract(multisig: &Address, start: &EvmStart, home: u64) -> [u8; 20] {
    let hashed = keccak(&[&[0xff][..], &PROXY, &[0; 32], &keccak(&multisig_init_code(multisig, start, home))].concat());

    hashed[12..].try_into().expect("20 bytes")
}

#[document(summary = "What deploys a multisig's MultiSig.sol through the proxy: the call's data, no salt then its init code. Anyone may send it.", category = "Multisig")]
pub fn multisig_deployment(multisig: &Address, start: &EvmStart, home: u64) -> Vec<u8> {
    [vec![0; 32], multisig_init_code(multisig, start, home)].concat()
}

/// A MultiSig.sol's EIP-712 domain on a chain.
fn domain(chain_id: u64, contract: &[u8; 20]) -> [u8; 32] {
    let domain_type = keccak(b"EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)");

    keccak(&[domain_type, keccak(b"chainv4 MultiSig"), keccak(b"1"), number(u256::from(chain_id)), word(contract)].concat())
}

#[document(
    summary = "What a MultiSig.sol's owners sign for a call: EIP-712's digest of Execute(address to, uint256 value, bytes data, uint256 nonce), under the domain chainv4 MultiSig, version 1, of the contract on its chain.",
    category = "Multisig",
)]
pub fn execute_digest(chain_id: u64, contract: &[u8; 20], to: &[u8; 20], value: u256, data: &[u8], nonce: u256) -> [u8; 32] {
    let call = keccak(&[keccak(b"Execute(address to,uint256 value,bytes data,uint256 nonce)"), word(to), number(value), keccak(data), number(nonce)].concat());

    keccak(&[&b"\x19\x01"[..], &domain(chain_id, contract), &call].concat())
}

#[document(
    summary = "What a MultiSig.sol's owners sign for its isValidSignature to hold a hash, as ERC-1271 asks it when it owns another contract: EIP-712's digest of Message(bytes32 hash), under its own domain, so no execute's signatures are ever one.",
    category = "Multisig",
    params = { chain_id = "The chain, by its EIP-155 id.", contract = "The MultiSig.sol that is the owner.", hash = "What it is asked to hold: the other contract's digest." },
)]
pub fn message_digest(chain_id: u64, contract: &[u8; 20], hash: &[u8; 32]) -> [u8; 32] {
    let message = keccak(&[keccak(b"Message(bytes32 hash)"), *hash].concat());

    keccak(&[&b"\x19\x01"[..], &domain(chain_id, contract), &message].concat())
}

#[document(summary = "An EVM key owner's signature over a digest, as MultiSig.sol takes it: r, s, low, and v, 27 or 28.", category = "Multisig")]
pub fn sign_execute(secret: &[u8; 32], digest: &[u8; 32]) -> [u8; 65] {
    let (signature, parity) = crate::crypto::secp256k1::sign_recoverable(secret, digest);
    let mut out = [0; 65];
    out[..64].copy_from_slice(&signature);
    out[64] = 27 + parity;

    out
}

#[document(summary = "The EVM address that made a signature over a digest, as MultiSig.sol recovers it; None for one that recovers none, or with a high s.", category = "Multisig")]
pub fn execute_signer(digest: &[u8; 32], signature: &[u8; 65]) -> Option<[u8; 20]> {
    let key = crate::crypto::secp256k1::recover_strict(digest, signature[..64].try_into().ok()?, signature[64].checked_sub(27)?)?;

    match PublicKey::Secp256k1(key).address() {
        Address::Secp256k1(address) => Some(address),
        _ => None,
    }
}

#[document(
    summary = "Whether an Ed25519 owner's 64-byte signature is its key's over a digest, as MultiSig.sol checks it.",
    category = "Multisig",
)]
pub fn ed25519_holds(key: &[u8; 32], digest: &[u8; 32], signature: &[u8]) -> bool {
    crate::vm::crypto::check_signature("ed25519", key, digest, signature).unwrap_or(false)
}

#[document(
    summary = "execute's calldata: the call, and the signers, by address ascending, each with its signature as its kind takes it, as MultiSig.sol checks them.",
    category = "Multisig",
)]
pub fn multisig_execute_input(to: &[u8; 20], value: u256, data: &[u8], signed: &[([u8; 20], Vec<u8>)]) -> Vec<u8> {
    let types = [AbiType::Address, AbiType::Uint(256), AbiType::Bytes, AbiType::Array(Box::new(AbiType::Address)), AbiType::Array(Box::new(AbiType::Bytes))];
    let values = [
        AbiValue::Address(*to),
        AbiValue::Uint(value),
        AbiValue::Bytes(data.to_vec()),
        AbiValue::Array(signed.iter().map(|(signer, _)| AbiValue::Address(*signer)).collect()),
        AbiValue::Array(signed.iter().map(|(_, signature)| AbiValue::Bytes(signature.clone())).collect()),
    ];

    [selector("execute(address,uint256,bytes,address[],bytes[])").to_vec(), encode_values(&types, &values).expect("execute's arguments encode")].concat()
}

#[document(
    summary = "A MultiSig.sol's signature as an owner of another contract, what its isValidSignature takes: its own signers, by address ascending, and their signatures over message_digest, ABI-encoded as (address[], bytes[]).",
    category = "Multisig",
)]
pub fn nested_signature(signed: &[([u8; 20], Vec<u8>)]) -> Vec<u8> {
    let types = [AbiType::Array(Box::new(AbiType::Address)), AbiType::Array(Box::new(AbiType::Bytes))];
    let values = [
        AbiValue::Array(signed.iter().map(|(signer, _)| AbiValue::Address(*signer)).collect()),
        AbiValue::Array(signed.iter().map(|(_, signature)| AbiValue::Bytes(signature.clone())).collect()),
    ];

    encode_values(&types, &values).expect("the signers encode")
}

#[document(summary = "changeOwners's calldata, for a call the contract makes on itself through execute: the owners' kinds and keys, sorted by address, and the threshold.", category = "Multisig")]
pub fn change_owners_input(next: &EvmStart) -> Vec<u8> {
    let [kinds, keys] = owner_lists(&next.owners);
    let types = [kinds_type(), keys_type(), AbiType::Uint(256)];
    let values = [kinds, keys, AbiValue::Uint(u256::from(u64::from(next.threshold)))];

    [selector("changeOwners(uint8[],bytes32[],uint256)").to_vec(), encode_values(&types, &values).expect("the owners encode")].concat()
}

#[document(summary = "The calls that read a MultiSig.sol: its owners' kinds and keys, its threshold and its nonce.", category = "Multisig")]
pub fn multisig_reads() -> [Vec<u8>; 3] {
    [selector("getOwnerKeys()").to_vec(), selector("threshold()").to_vec(), selector("nonce()").to_vec()]
}

#[document(summary = "What isValidSignature answers for a signature it holds, ERC-1271's magic value: what a contract owner's answer is checked against.", category = "Multisig")]
pub const ERC1271_VALID: [u8; 4] = [0x16, 0x26, 0xba, 0x7e];

#[document(summary = "isValidSignature's calldata: asks a contract owner whether it holds a signature over a hash.", category = "Multisig")]
pub fn is_valid_signature_input(hash: &[u8; 32], signature: &[u8]) -> Vec<u8> {
    let types = [AbiType::FixedBytes(32), AbiType::Bytes];
    let values = [AbiValue::FixedBytes(hash.to_vec()), AbiValue::Bytes(signature.to_vec())];

    [selector("isValidSignature(bytes32,bytes)").to_vec(), encode_values(&types, &values).expect("the question encodes")].concat()
}

#[document(
    summary = "What the reads answered, read back: the owners, each its address worked out from its kind and key as the contract works it out, the threshold, and the nonce; None for answers that are not a MultiSig.sol's.",
    category = "Multisig",
)]
pub fn multisig_read(owners: &[u8], threshold: &[u8], nonce: &[u8]) -> Option<(EvmStart, u256)> {
    let at = |bytes: &[u8], word: usize| bytes.get(word * 32..(word + 1) * 32).map(|word| u256::from_be_bytes(word.try_into().expect("32 bytes")));
    let lists = decode_values(&[kinds_type(), keys_type()], owners).ok()?;
    let (AbiValue::Array(kinds), AbiValue::Array(keys)) = (&lists[0], &lists[1]) else {
        return None;
    };
    let mut list = Vec::new();

    for (kind, key) in kinds.iter().zip(keys) {
        let (AbiValue::Uint(kind), AbiValue::FixedBytes(key)) = (kind, key) else {
            return None;
        };
        let key: [u8; 32] = key.as_slice().try_into().ok()?;
        let address: [u8; 20] = key[12..].try_into().expect("20 bytes");
        list.push(match u64::try_from(*kind).ok()? {
            0 => EvmOwner { address, kind: EvmKind::Key },
            1 => EvmOwner { address, kind: EvmKind::Contract },
            2 => EvmOwner { address: crate::registry::multisig::ed25519_owner(&key), kind: EvmKind::Ed25519(key) },
            _ => return None,
        });
    }

    let threshold = u32::try_from(u64::try_from(at(threshold, 0)?).ok()?).ok()?;

    Some((EvmStart { owners: list, threshold }, at(nonce, 0)?))
}

/// An owner's signature over a digest as its kind of key makes one, and its address as an owner:
/// an EVM key's 65 bytes, or an Ed25519 key's 64.
fn owner_signature(keypair: &crate::crypto::keypair::Keypair, digest: &[u8; 32]) -> Result<([u8; 20], Vec<u8>), String> {
    use crate::crypto::keypair::{PrivateKey, Signature};

    match (keypair.private(), keypair.public()) {
        (PrivateKey::Secp256k1(secret), _) => {
            let signature = sign_execute(secret, digest);

            Ok((execute_signer(digest, &signature).ok_or("the key's signature recovers nobody")?, signature.to_vec()))
        }
        (_, PublicKey::Ed25519(key)) => match keypair.sign(digest) {
            Signature::Ed25519(signature) => Ok((crate::registry::multisig::ed25519_owner(&key), signature.to_vec())),
            _ => Err("an Ed25519 key signs as Ed25519".to_string()),
        },
        _ => Err("a MultiSig.sol owner signs with an EVM key or an Ed25519 key".to_string()),
    }
}

#[document(
    summary = "What the offline signer is asked to sign of a multisig's EVM call: the bundle, which of its keys signs, and, when it approves as an owner of another MultiSig.sol, that contract. It reads one back with the signature added.",
    category = "Multisig",
    see_also = "EvmBundle",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct EvmRequest {
    #[document(summary = "The call, with the signatures so far.")]
    pub bundle: EvmBundle,
    #[document(summary = "The key that signs: an EVM key or an Ed25519 key of the signer's.")]
    pub signer: PublicKey,
    #[document(summary = "The owner contract it approves for, when it signs as one of that contract's owners; None to sign as an owner of the bundle's own contract.")]
    pub owner: Option<[u8; 20]>,
}

impl Hashable for EvmRequest {}

impl EvmRequest {
    #[document(parent = "EvmRequest", summary = "Signs it with the key it names: the bundle with the signature in.")]
    pub fn sign(&self, keypair: &crate::crypto::keypair::Keypair) -> Result<EvmBundle, String> {
        if keypair.public() != self.signer {
            return Err("another key than the one asked".to_string());
        }

        let mut bundle = self.bundle.clone();

        match self.owner {
            Some(owner) => bundle.approve_with(owner, keypair)?,
            None => bundle.sign_with(keypair)?,
        };

        Ok(bundle)
    }
}

#[document(
    summary = "One owner's signature in a bundle: who, and the bytes its kind takes, 65 for an EVM key, 64 for Ed25519, whatever a contract owner's isValidSignature reads.",
    category = "Multisig",
    see_also = "EvmBundle",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct OwnerSignature {
    #[document(summary = "The owner's address on the contract.")]
    pub owner: [u8; 20],
    #[document(summary = "Its signature.")]
    pub signature: Vec<u8>,
}

impl Hashable for OwnerSignature {}

#[document(
    summary = "A contract owner that is another MultiSig.sol, collecting its own owners' signatures for a bundle: their signatures over message_digest of the bundle's digest under its domain, packed by nested_signature once enough signed.",
    category = "Multisig",
    see_also = "EvmBundle, message_digest",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct Nested {
    #[document(summary = "The owner: the other MultiSig.sol's address.")]
    pub owner: [u8; 20],
    #[document(summary = "Its own owners' signatures so far.")]
    pub signatures: Vec<OwnerSignature>,
}

impl Hashable for Nested {}

#[document(
    summary = "A call a multisig's EVM contract is asked to make, on its way round the owners: the chain, the contract, the call and the nonce it runs at, and the signatures so far, its contract owners' own owners' among them. Passed as text or a code until enough signed, then sent as execute.",
    category = "Multisig",
    see_also = "execute_digest, multisig_execute_input",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct EvmBundle {
    #[document(summary = "The chain, by its EIP-155 id.")]
    pub chain_id: u64,
    #[document(summary = "The multisig's MultiSig.sol there.")]
    pub contract: [u8; 20],
    #[document(summary = "Whom it calls.")]
    pub to: [u8; 20],
    #[document(summary = "The coin it sends, in wei.")]
    #[wincode(with = "U256Le")]
    pub value: u256,
    #[document(summary = "The call's data.")]
    pub data: Vec<u8>,
    #[document(summary = "The contract's nonce it runs at.")]
    #[wincode(with = "U256Le")]
    pub nonce: u256,
    #[document(summary = "The owners' signatures so far, one each.")]
    pub signatures: Vec<OwnerSignature>,
    #[document(summary = "The contract owners that are other MultiSig.sols, gathering their own owners' signatures.")]
    pub nested: Vec<Nested>,
}

impl Hashable for EvmBundle {}

/// What an EVM bundle's text starts with.
const EVM_BUNDLE_PREFIX: &str = "chainv4-multisig-evm:";

/// EIP-712's typed data for what a MultiSig.sol's owners sign: its domain, and the one message.
fn typed(chain_id: u64, contract: &[u8; 20], primary: &str, fields: serde_json::Value, message: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "types": {
            "EIP712Domain": [
                { "name": "name", "type": "string" },
                { "name": "version", "type": "string" },
                { "name": "chainId", "type": "uint256" },
                { "name": "verifyingContract", "type": "address" },
            ],
            primary: fields,
        },
        "primaryType": primary,
        "domain": { "name": "chainv4 MultiSig", "version": "1", "chainId": chain_id, "verifyingContract": to_hex(contract) },
        "message": message,
    })
}

impl EvmBundle {
    #[document(parent = "EvmBundle", summary = "A call with no signatures yet.")]
    pub fn new(chain_id: u64, contract: [u8; 20], to: [u8; 20], value: u256, data: Vec<u8>, nonce: u256) -> EvmBundle {
        EvmBundle { chain_id, contract, to, value, data, nonce, signatures: Vec::new(), nested: Vec::new() }
    }

    #[document(parent = "EvmBundle", summary = "What its owners sign.")]
    pub fn digest(&self) -> [u8; 32] {
        execute_digest(self.chain_id, &self.contract, &self.to, self.value, &self.data, self.nonce)
    }

    #[document(parent = "EvmBundle", summary = "What the owners of a contract owner that is another MultiSig.sol sign for it: message_digest of this bundle's digest under that contract's domain.")]
    pub fn nested_digest(&self, owner: &[u8; 20]) -> [u8; 32] {
        message_digest(self.chain_id, owner, &self.digest())
    }

    #[document(
        parent = "EvmBundle",
        summary = "What its owners sign, as eth_signTypedData_v4 takes it: the EIP-712 JSON whose digest is digest's. A hardware wallet, MetaMask or the offline signer shows it and signs it.",
    )]
    pub fn typed_data(&self) -> serde_json::Value {
        let fields = serde_json::json!([
            { "name": "to", "type": "address" },
            { "name": "value", "type": "uint256" },
            { "name": "data", "type": "bytes" },
            { "name": "nonce", "type": "uint256" },
        ]);
        let message = serde_json::json!({ "to": to_hex(&self.to), "value": self.value.to_string(), "data": to_hex(&self.data), "nonce": self.nonce.to_string() });

        typed(self.chain_id, &self.contract, "Execute", fields, message)
    }

    #[document(parent = "EvmBundle", summary = "What a contract owner's own owners sign for it, as eth_signTypedData_v4 takes it: Message(bytes32 hash) under its domain, whose digest is nested_digest's.")]
    pub fn nested_typed_data(&self, owner: &[u8; 20]) -> serde_json::Value {
        let fields = serde_json::json!([{ "name": "hash", "type": "bytes32" }]);

        typed(self.chain_id, owner, "Message", fields, serde_json::json!({ "hash": to_hex(&self.digest()) }))
    }

    /// Puts one owner's signature in a list, in place of any earlier one by it.
    fn put(list: &mut Vec<OwnerSignature>, owner: [u8; 20], signature: Vec<u8>) {
        list.retain(|held| held.owner != owner);
        list.push(OwnerSignature { owner, signature });
    }

    #[document(parent = "EvmBundle", summary = "Adds an EVM key owner's signature made elsewhere, over its digest, such as a hardware wallet's over typed_data; refused when it recovers nobody or has a high s.")]
    pub fn add(&mut self, signature: [u8; 65]) -> Result<[u8; 20], String> {
        let signer = execute_signer(&self.digest(), &signature).ok_or("a signature that recovers no key, or not as MultiSig.sol takes it")?;
        Self::put(&mut self.signatures, signer, signature.to_vec());

        Ok(signer)
    }

    #[document(parent = "EvmBundle", summary = "Adds an EVM key owner's signature, replacing any earlier one by the same key.")]
    pub fn sign(&mut self, secret: &[u8; 32]) {
        let signature = sign_execute(secret, &self.digest());

        if let Some(signer) = execute_signer(&self.digest(), &signature) {
            Self::put(&mut self.signatures, signer, signature.to_vec());
        }
    }

    #[document(parent = "EvmBundle", summary = "Adds an Ed25519 owner's 64-byte signature over its digest; refused when it is not the key's.")]
    pub fn add_ed25519(&mut self, key: &[u8; 32], signature: &[u8]) -> Result<[u8; 20], String> {
        if !ed25519_holds(key, &self.digest(), signature) {
            return Err("not this Ed25519 key's signature over the call".to_string());
        }

        let owner = crate::registry::multisig::ed25519_owner(key);
        Self::put(&mut self.signatures, owner, signature.to_vec());

        Ok(owner)
    }

    #[document(parent = "EvmBundle", summary = "Adds a contract owner's signature as given, what its isValidSignature reads: the chain asks the contract when the bundle is sent.")]
    pub fn add_contract(&mut self, owner: [u8; 20], signature: Vec<u8>) {
        self.nested.retain(|nested| nested.owner != owner);
        Self::put(&mut self.signatures, owner, signature);
    }

    #[document(parent = "EvmBundle", summary = "Adds the signature of one of a nested MultiSig.sol's owners over nested_digest, by that owner's address there: an EVM key's 65 bytes, an Ed25519 key's 64, checked by the caller against that contract's owners.")]
    pub fn add_nested(&mut self, owner: [u8; 20], signer: [u8; 20], signature: Vec<u8>) {
        let at = match self.nested.iter().position(|nested| nested.owner == owner) {
            Some(at) => at,
            None => {
                self.nested.push(Nested { owner, signatures: Vec::new() });
                self.nested.len() - 1
            }
        };

        Self::put(&mut self.nested[at].signatures, signer, signature);
    }

    #[document(parent = "EvmBundle", summary = "Signs it as one of its owners, with an EVM key or an Ed25519 key; the owner's address there.")]
    pub fn sign_with(&mut self, keypair: &crate::crypto::keypair::Keypair) -> Result<[u8; 20], String> {
        let (owner, signature) = owner_signature(keypair, &self.digest())?;

        match keypair.public() {
            PublicKey::Ed25519(key) => self.add_ed25519(&key, &signature),
            _ => {
                Self::put(&mut self.signatures, owner, signature);

                Ok(owner)
            }
        }
    }

    #[document(parent = "EvmBundle", summary = "Approves it for an owner that is another MultiSig.sol, as one of that contract's own owners: signs nested_digest with an EVM key or an Ed25519 key; the signer's address on that contract.")]
    pub fn approve_with(&mut self, owner: [u8; 20], keypair: &crate::crypto::keypair::Keypair) -> Result<[u8; 20], String> {
        let (signer, signature) = owner_signature(keypair, &self.nested_digest(&owner))?;
        self.add_nested(owner, signer, signature);

        Ok(signer)
    }

    #[document(parent = "EvmBundle", summary = "Takes in another copy's signatures and nested approvals, for the same call; refused for another call, chain or nonce.")]
    pub fn merge(&mut self, other: &EvmBundle) -> Result<(), String> {
        if other.digest() != self.digest() {
            return Err("that bundle is another call, or for another chain or nonce".to_string());
        }

        for held in &other.signatures {
            Self::put(&mut self.signatures, held.owner, held.signature.clone());
        }

        for nested in &other.nested {
            for held in &nested.signatures {
                self.add_nested(nested.owner, held.owner, held.signature.clone());
            }
        }

        Ok(())
    }

    #[document(parent = "EvmBundle", summary = "How many of these owners signed it, as far as can be told without the chain: an EVM key's and an Ed25519 key's signatures checked, a contract owner's counted as it is given. The chain asks a contract owner itself.")]
    pub fn signed_by(&self, owners: &EvmStart) -> usize {
        let digest = self.digest();

        self.signers()
            .into_iter()
            .filter(|(signer, signature)| match owners.owners.iter().find(|owner| owner.address == *signer).map(|owner| owner.kind) {
                Some(EvmKind::Key) => <[u8; 65]>::try_from(signature.as_slice()).ok().and_then(|signature| execute_signer(&digest, &signature)) == Some(*signer),
                Some(EvmKind::Ed25519(key)) => ed25519_holds(&key, &digest, signature),
                Some(EvmKind::Contract) => true,
                None => false,
            })
            .count()
    }

    #[document(parent = "EvmBundle", summary = "Who signed it, each once, by address ascending, with their signatures: its own, and each nested owner's, packed, once that owner's own owners among these signed. What execute takes.")]
    pub fn signers(&self) -> Vec<([u8; 20], Vec<u8>)> {
        let mut signed: Vec<([u8; 20], Vec<u8>)> = self.signatures.iter().map(|held| (held.owner, held.signature.clone())).collect();

        for nested in &self.nested {
            if !signed.iter().any(|(owner, _)| *owner == nested.owner) && !nested.signatures.is_empty() {
                let mut inner: Vec<([u8; 20], Vec<u8>)> = nested.signatures.iter().map(|held| (held.owner, held.signature.clone())).collect();
                inner.sort_by_key(|(owner, _)| *owner);
                signed.push((nested.owner, nested_signature(&inner)));
            }
        }

        signed.sort_by_key(|(owner, _)| *owner);
        signed.dedup_by_key(|(owner, _)| *owner);

        signed
    }

    #[document(parent = "EvmBundle", summary = "execute's calldata, with the signatures of these owners alone, in order: what a member sends to the contract once enough signed.")]
    pub fn execute_input(&self, owners: &[EvmOwner]) -> Vec<u8> {
        let signed: Vec<([u8; 20], Vec<u8>)> = self.signers().into_iter().filter(|(signer, _)| owners.iter().any(|owner| owner.address == *signer)).collect();

        multisig_execute_input(&self.to, self.value, &self.data, &signed)
    }

    #[document(parent = "EvmBundle", summary = "Its text: chainv4-multisig-evm: and its encoding in hex.")]
    pub fn to_text(&self) -> String {
        format!("{EVM_BUNDLE_PREFIX}{}", to_hex(&self.hashable_bytes_with_tag()))
    }

    #[document(parent = "EvmBundle", summary = "Reads its text back; None for text that is not one.")]
    pub fn from_text(text: &str) -> Option<EvmBundle> {
        wincode::deserialize(&from_hex(text.trim().strip_prefix(EVM_BUNDLE_PREFIX)?)?).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A test chain id for chainv4 itself.
    const HOME: u64 = 0x1234;

    fn start() -> EvmStart {
        let mut owners: Vec<EvmOwner> = (1..=3u8)
            .map(|seed| match PublicKey::Secp256k1(crate::crypto::secp256k1::public_key(&[seed; 32]).unwrap()).address() {
                Address::Secp256k1(address) => EvmOwner { address, kind: EvmKind::Key },
                _ => unreachable!(),
            })
            .collect();
        owners.sort();

        EvmStart { owners, threshold: 2 }
    }

    #[test]
    fn its_address_commits_to_the_multisig_its_home_and_who_it_starts_with() {
        let multisig = Address::Multisig([4; 32]);
        let here = multisig_contract(&multisig, &start(), HOME);

        assert_eq!(here, multisig_contract(&multisig, &start(), HOME), "the same everywhere");
        assert_ne!(here, multisig_contract(&Address::Multisig([5; 32]), &start(), HOME));
        assert_ne!(here, multisig_contract(&multisig, &EvmStart { threshold: 1, ..start() }, HOME));
        assert_ne!(here, multisig_contract(&multisig, &start(), HOME + 1), "another chainv4's");
        let mut contract_owned = start();
        contract_owned.owners[0].kind = EvmKind::Contract;
        assert_ne!(here, multisig_contract(&multisig, &contract_owned, HOME), "the same address as a contract");
        assert_eq!(multisig_deployment(&multisig, &start(), HOME)[..32], [0; 32], "no salt");
    }

    #[cfg(feature = "swap")]
    #[test]
    fn it_is_where_the_proxy_puts_it() {
        use crate::swap::evm::{DEPLOYMENT_PROXY, create2};

        let multisig = Address::Multisig([4; 32]);

        assert_eq!(PROXY, DEPLOYMENT_PROXY);
        assert_eq!(multisig_contract(&multisig, &start(), HOME), create2(&DEPLOYMENT_PROXY, &[0; 32], &multisig_init_code(&multisig, &start(), HOME)));
    }

    #[test]
    fn owners_sign_a_call_and_execute_takes_them_by_address() {
        let mut bundle = EvmBundle::new(11155111, [9; 20], [7; 20], u256::new(5), vec![1, 2, 3], u256::ZERO);
        bundle.sign(&[3; 32]);
        bundle.sign(&[1; 32]);
        bundle.sign(&[1; 32]);

        let signers: Vec<[u8; 20]> = bundle.signers().into_iter().map(|(signer, _)| signer).collect();

        assert_eq!(signers.len(), 2, "one each");
        assert!(signers[0] < signers[1]);
        assert!(start().owners.iter().any(|owner| owner.address == signers[0]) && start().owners.iter().any(|owner| owner.address == signers[1]));
        assert_eq!(EvmBundle::from_text(&bundle.to_text()), Some(bundle.clone()));
        assert_eq!(bundle.execute_input(&start().owners)[..4], selector("execute(address,uint256,bytes,address[],bytes[])"));
    }

    #[cfg(feature = "ed25519")]
    #[test]
    fn an_ed25519_owner_signs_the_digest_and_a_nested_multisig_its_wrapped_one() {
        use crate::crypto::keypair::{Keypair, PrivateKey, Signature};

        let keypair = Keypair::from_private(PrivateKey::Ed25519([8; 32]));
        let PublicKey::Ed25519(key) = keypair.public() else { unreachable!() };
        let mut bundle = EvmBundle::new(1, [9; 20], [7; 20], u256::ZERO, Vec::new(), u256::ZERO);
        let Signature::Ed25519(signature) = keypair.sign(&bundle.digest()) else { unreachable!() };

        assert!(bundle.add_ed25519(&key, &[0; 64]).is_err(), "not its signature");
        assert_eq!(bundle.add_ed25519(&key, &signature).unwrap(), crate::registry::multisig::ed25519_owner(&key));

        // Another MultiSig.sol, an owner: its own owners sign the call wrapped in its domain.
        let inner = [5; 20];
        assert_ne!(bundle.nested_digest(&inner), bundle.digest());
        let typed = crate::crypto::eip712::typed::TypedData::parse(&bundle.nested_typed_data(&inner)).unwrap();
        assert_eq!(typed.digest().unwrap(), bundle.nested_digest(&inner));
        let wrapped = sign_execute(&[2; 32], &bundle.nested_digest(&inner));
        let signer = execute_signer(&bundle.nested_digest(&inner), &wrapped).unwrap();
        bundle.add_nested(inner, signer, wrapped.to_vec());

        let signed = bundle.signers();
        assert_eq!(signed.len(), 2);
        let packed = &signed.iter().find(|(owner, _)| *owner == inner).unwrap().1;
        assert_eq!(packed, &nested_signature(&[(signer, wrapped.to_vec())]), "packed as its isValidSignature reads it");
        assert_eq!(EvmBundle::from_text(&bundle.to_text()), Some(bundle));
    }

    #[test]
    fn what_a_contract_reads_back_names_each_owner_as_the_contract_does() {
        let mut owners = start().owners;
        owners[1].kind = EvmKind::Contract;
        let key = [6; 32];
        owners.push(EvmOwner { address: crate::registry::multisig::ed25519_owner(&key), kind: EvmKind::Ed25519(key) });
        let lists = owner_lists(&owners);
        let encoded = encode_values(&[kinds_type(), keys_type()], &lists).unwrap();

        let (read, nonce) = multisig_read(&encoded, &number(u256::new(2)), &number(u256::new(7))).unwrap();
        assert_eq!((read.owners, read.threshold, nonce), (owners, 2, u256::new(7)));
    }

    #[cfg(feature = "keccak256")]
    #[test]
    fn its_typed_data_is_what_the_owners_sign_and_a_signature_from_elsewhere_is_added() {
        let mut bundle = EvmBundle::new(1, [9; 20], [7; 20], u256::new(10).pow(18), vec![0xa9, 0x05, 0x9c, 0xbb], u256::new(3));
        let typed = crate::crypto::eip712::typed::TypedData::parse(&bundle.typed_data()).unwrap();

        assert_eq!(typed.digest().unwrap(), bundle.digest());

        let signature = sign_execute(&[2; 32], &bundle.digest());
        let signer = bundle.add(signature).unwrap();
        assert_eq!(bundle.signers(), [(signer, signature.to_vec())]);
        assert!(bundle.add([0; 65]).is_err());
    }
}
