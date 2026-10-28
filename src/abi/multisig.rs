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
    super::keccak,
    crate::crypto::hashable::Hashable,
    crate::crypto::keypair::PublicKey,
    crate::registry::multisig::EvmStart,
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

/// A list of addresses as the ABI writes one after its offset: its length, then each.
fn addresses(owners: &[[u8; 20]]) -> Vec<u8> {
    std::iter::once(number(u256::from(owners.len() as u64))).chain(owners.iter().map(|owner| word(owner))).flatten().collect()
}

/// Bytes as the ABI writes them after their offset: their length, then them, padded to a word.
fn padded(bytes: &[u8]) -> Vec<u8> {
    let mut out = number(u256::from(bytes.len() as u64)).to_vec();
    out.extend(bytes);
    out.resize(32 + bytes.len().div_ceil(32) * 32, 0);

    out
}

#[document(
    summary = "MultiSig.sol's init code for a multisig: its code, then its constructor's arguments, the chainv4 multisig's address, the owners, sorted, and the threshold it starts with.",
    category = "Multisig",
)]
pub fn multisig_init_code(multisig: &Address, start: &EvmStart) -> Vec<u8> {
    let mut init = from_hex(&format!("0x{}", MULTISIG_CODE.trim())).expect("MultiSig.bin is hex");
    init.extend(word(multisig.bytes()));
    init.extend(number(u256::from(96u64)));
    init.extend(number(u256::from(u64::from(start.threshold))));
    init.extend(addresses(&start.owners));

    init
}

#[document(
    summary = "Where a multisig's MultiSig.sol sits on every EVM chain: CREATE2's address for the deterministic deployment proxy, no salt, and its init code. The same before it is deployed as after.",
    category = "Multisig",
    see_also = "multisig_init_code, EvmStart",
)]
pub fn multisig_contract(multisig: &Address, start: &EvmStart) -> [u8; 20] {
    let hashed = keccak(&[&[0xff][..], &PROXY, &[0; 32], &keccak(&multisig_init_code(multisig, start))].concat());

    hashed[12..].try_into().expect("20 bytes")
}

#[document(summary = "What deploys a multisig's MultiSig.sol through the proxy: the call's data, no salt then its init code. Anyone may send it.", category = "Multisig")]
pub fn multisig_deployment(multisig: &Address, start: &EvmStart) -> Vec<u8> {
    [vec![0; 32], multisig_init_code(multisig, start)].concat()
}

#[document(
    summary = "What a MultiSig.sol's owners sign for a call: EIP-712's digest of Execute(address to, uint256 value, bytes data, uint256 nonce), under the domain chainv4 MultiSig, version 1, of the contract on its chain.",
    category = "Multisig",
)]
pub fn execute_digest(chain_id: u64, contract: &[u8; 20], to: &[u8; 20], value: u256, data: &[u8], nonce: u256) -> [u8; 32] {
    let domain_type = keccak(b"EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)");
    let domain = keccak(&[domain_type, keccak(b"chainv4 MultiSig"), keccak(b"1"), number(u256::from(chain_id)), word(contract)].concat());
    let call = keccak(&[keccak(b"Execute(address to,uint256 value,bytes data,uint256 nonce)"), word(to), number(value), keccak(data), number(nonce)].concat());

    keccak(&[&b"\x19\x01"[..], &domain, &call].concat())
}

#[document(summary = "An owner's signature over a digest, as MultiSig.sol takes it: r, s, low, and v, 27 or 28.", category = "Multisig")]
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
    summary = "execute's calldata: the call, and the owners' signatures one after another, by their addresses ascending, as MultiSig.sol checks them.",
    category = "Multisig",
)]
pub fn multisig_execute_input(to: &[u8; 20], value: u256, data: &[u8], signatures: &[[u8; 65]]) -> Vec<u8> {
    let data = padded(data);
    let joined: Vec<u8> = signatures.iter().flatten().copied().collect();
    let mut input = selector("execute(address,uint256,bytes,bytes)").to_vec();
    input.extend(word(to));
    input.extend(number(value));
    input.extend(number(u256::from(128u64)));
    input.extend(number(u256::from(128 + data.len() as u64)));
    input.extend(data);
    input.extend(padded(&joined));

    input
}

#[document(summary = "changeOwners's calldata, for a call the contract makes on itself through execute: the owners, sorted, and the threshold.", category = "Multisig")]
pub fn change_owners_input(next: &EvmStart) -> Vec<u8> {
    let mut input = selector("changeOwners(address[],uint256)").to_vec();
    input.extend(number(u256::from(64u64)));
    input.extend(number(u256::from(u64::from(next.threshold))));
    input.extend(addresses(&next.owners));

    input
}

#[document(summary = "The calls that read a MultiSig.sol: its owners, its threshold and its nonce.", category = "Multisig")]
pub fn multisig_reads() -> [Vec<u8>; 3] {
    [selector("getOwners()").to_vec(), selector("threshold()").to_vec(), selector("nonce()").to_vec()]
}

#[document(
    summary = "What the reads answered, read back: the owners and threshold, and the nonce; None for answers that are not a MultiSig.sol's.",
    category = "Multisig",
)]
pub fn multisig_read(owners: &[u8], threshold: &[u8], nonce: &[u8]) -> Option<(EvmStart, u256)> {
    let at = |bytes: &[u8], word: usize| bytes.get(word * 32..(word + 1) * 32).map(|word| u256::from_be_bytes(word.try_into().expect("32 bytes")));
    let count = usize::try_from(u64::try_from(at(owners, 1)?).ok()?).ok()?;
    let list = (0..count).map(|index| owners.get((2 + index) * 32 + 12..(3 + index) * 32).and_then(|bytes| bytes.try_into().ok())).collect::<Option<Vec<[u8; 20]>>>()?;
    let threshold = u32::try_from(u64::try_from(at(threshold, 0)?).ok()?).ok()?;

    Some((EvmStart { owners: list, threshold }, at(nonce, 0)?))
}

#[document(
    summary = "A call a multisig's EVM contract is asked to make, on its way round the owners: the chain, the contract, the call and the nonce it runs at, and the signatures so far. Passed as text or a code until enough signed, then sent as execute.",
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
    #[document(summary = "The owners' signatures so far, each 65 bytes.")]
    pub signatures: Vec<[u8; 65]>,
}

impl Hashable for EvmBundle {}

/// What an EVM bundle's text starts with.
const EVM_BUNDLE_PREFIX: &str = "chainv4-multisig-evm:";

impl EvmBundle {
    #[document(parent = "EvmBundle", summary = "What its owners sign.")]
    pub fn digest(&self) -> [u8; 32] {
        execute_digest(self.chain_id, &self.contract, &self.to, self.value, &self.data, self.nonce)
    }

    #[document(
        parent = "EvmBundle",
        summary = "What its owners sign, as eth_signTypedData_v4 takes it: the EIP-712 JSON whose digest is digest's. A hardware wallet, MetaMask or the offline signer shows it and signs it.",
    )]
    pub fn typed_data(&self) -> serde_json::Value {
        serde_json::json!({
            "types": {
                "EIP712Domain": [
                    { "name": "name", "type": "string" },
                    { "name": "version", "type": "string" },
                    { "name": "chainId", "type": "uint256" },
                    { "name": "verifyingContract", "type": "address" },
                ],
                "Execute": [
                    { "name": "to", "type": "address" },
                    { "name": "value", "type": "uint256" },
                    { "name": "data", "type": "bytes" },
                    { "name": "nonce", "type": "uint256" },
                ],
            },
            "primaryType": "Execute",
            "domain": { "name": "chainv4 MultiSig", "version": "1", "chainId": self.chain_id, "verifyingContract": to_hex(&self.contract) },
            "message": { "to": to_hex(&self.to), "value": self.value.to_string(), "data": to_hex(&self.data), "nonce": self.nonce.to_string() },
        })
    }

    #[document(parent = "EvmBundle", summary = "Adds a signature made elsewhere, over its digest, such as a hardware wallet's over typed_data; refused when it recovers nobody or has a high s.")]
    pub fn add(&mut self, signature: [u8; 65]) -> Result<[u8; 20], String> {
        let digest = self.digest();
        let signer = execute_signer(&digest, &signature).ok_or("a signature that recovers no key, or not as MultiSig.sol takes it")?;
        self.signatures.retain(|held| execute_signer(&digest, held) != Some(signer));
        self.signatures.push(signature);

        Ok(signer)
    }

    #[document(parent = "EvmBundle", summary = "Adds an owner's signature, replacing any earlier one by the same key.")]
    pub fn sign(&mut self, secret: &[u8; 32]) {
        let digest = self.digest();
        let signature = sign_execute(secret, &digest);
        let signer = execute_signer(&digest, &signature);
        self.signatures.retain(|held| execute_signer(&digest, held) != signer);
        self.signatures.push(signature);
    }

    #[document(parent = "EvmBundle", summary = "Who signed it, each once, by address ascending, with their signatures: what execute takes. Signatures that recover nobody are left out.")]
    pub fn signers(&self) -> Vec<([u8; 20], [u8; 65])> {
        let digest = self.digest();
        let mut signed: Vec<([u8; 20], [u8; 65])> = self.signatures.iter().filter_map(|signature| execute_signer(&digest, signature).map(|signer| (signer, *signature))).collect();
        signed.sort_by_key(|(signer, _)| *signer);
        signed.dedup_by_key(|(signer, _)| *signer);

        signed
    }

    #[document(parent = "EvmBundle", summary = "execute's calldata, with the signatures of these owners alone, in order: what a member sends to the contract once enough signed.")]
    pub fn execute_input(&self, owners: &[[u8; 20]]) -> Vec<u8> {
        let signatures: Vec<[u8; 65]> = self.signers().into_iter().filter(|(signer, _)| owners.contains(signer)).map(|(_, signature)| signature).collect();

        multisig_execute_input(&self.to, self.value, &self.data, &signatures)
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

    fn start() -> EvmStart {
        let mut owners: Vec<[u8; 20]> = (1..=3u8)
            .map(|seed| match PublicKey::Secp256k1(crate::crypto::secp256k1::public_key(&[seed; 32]).unwrap()).address() {
                Address::Secp256k1(address) => address,
                _ => unreachable!(),
            })
            .collect();
        owners.sort();

        EvmStart { owners, threshold: 2 }
    }

    #[test]
    fn its_address_commits_to_the_multisig_and_who_it_starts_with() {
        let multisig = Address::Multisig([4; 32]);
        let here = multisig_contract(&multisig, &start());

        assert_eq!(here, multisig_contract(&multisig, &start()), "the same everywhere");
        assert_ne!(here, multisig_contract(&Address::Multisig([5; 32]), &start()));
        assert_ne!(here, multisig_contract(&multisig, &EvmStart { threshold: 1, ..start() }));
        assert_eq!(multisig_deployment(&multisig, &start())[..32], [0; 32], "no salt");
    }

    #[cfg(feature = "swap")]
    #[test]
    fn it_is_where_the_proxy_puts_it() {
        use crate::swap::evm::{DEPLOYMENT_PROXY, create2};

        let multisig = Address::Multisig([4; 32]);

        assert_eq!(PROXY, DEPLOYMENT_PROXY);
        assert_eq!(multisig_contract(&multisig, &start()), create2(&DEPLOYMENT_PROXY, &[0; 32], &multisig_init_code(&multisig, &start())));
    }

    #[test]
    fn owners_sign_a_call_and_execute_takes_them_by_address() {
        let mut bundle = EvmBundle { chain_id: 11155111, contract: [9; 20], to: [7; 20], value: u256::new(5), data: vec![1, 2, 3], nonce: u256::ZERO, signatures: Vec::new() };
        bundle.sign(&[3; 32]);
        bundle.sign(&[1; 32]);
        bundle.sign(&[1; 32]);

        let signers: Vec<[u8; 20]> = bundle.signers().into_iter().map(|(signer, _)| signer).collect();

        assert_eq!(signers.len(), 2, "one each");
        assert!(signers[0] < signers[1]);
        assert!(start().owners.contains(&signers[0]) && start().owners.contains(&signers[1]));
        assert_eq!(EvmBundle::from_text(&bundle.to_text()), Some(bundle.clone()));
        assert_eq!(bundle.execute_input(&start().owners)[..4], selector("execute(address,uint256,bytes,bytes)"));
    }

    #[cfg(feature = "keccak256")]
    #[test]
    fn its_typed_data_is_what_the_owners_sign_and_a_signature_from_elsewhere_is_added() {
        let mut bundle = EvmBundle { chain_id: 1, contract: [9; 20], to: [7; 20], value: u256::new(10).pow(18), data: vec![0xa9, 0x05, 0x9c, 0xbb], nonce: u256::new(3), signatures: Vec::new() };
        let typed = crate::crypto::eip712::typed::TypedData::parse(&bundle.typed_data()).unwrap();

        assert_eq!(typed.digest().unwrap(), bundle.digest());

        let signature = sign_execute(&[2; 32], &bundle.digest());
        let signer = bundle.add(signature).unwrap();
        assert_eq!(bundle.signers(), [(signer, signature)]);
        assert!(bundle.add([0; 65]).is_err());
    }
}
