//! The crypto precompile: chainv4's signatures, hashes and proofs for EVM contracts, which
//! Ethereum's own precompiles lack. Solidity calls it like any contract, through an interface:
//!
//! ```solidity
//! interface Crypto {
//!     function verify(string calldata scheme, bytes calldata key, bytes calldata message, bytes calldata signature) external view returns (bool);
//!     function keyAddress(string calldata scheme, bytes calldata key) external view returns (bytes memory);
//!     function blake3(bytes calldata data) external view returns (bytes32);
//!     function blake2s(bytes calldata data) external view returns (bytes32);
//!     function blake2b(bytes calldata data) external view returns (bytes32);
//!     function xxh3(bytes calldata data) external view returns (bytes8);
//!     function hash160(bytes calldata data) external view returns (bytes20);
//!     function sha256d(bytes calldata data) external view returns (bytes32);
//!     function checkState(bytes32 root, string calldata column, bytes calldata key, bytes calldata value, bool present, bytes calldata proof) external view returns (bool);
//!     function checkMerkle(bytes32 root, bytes calldata data, uint256 index, uint256 count, bytes calldata path) external view returns (bool);
//! }
//! ```
//!
//! It lives at CRYPTO_PRECOMPILE on every chain that runs the EVM, reads no state, and costs gas
//! as the native engines charge fuel for the same work.

use {
    crate::abi::{AbiType, AbiValue, decode_values, encode_values},
    crate::crypto::hasher::Hasher,
    crate::structs::hash::HashType,
    crate::vm::{cost, crypto},
    r#macro::document,
    revm::precompile::{PrecompileHalt, PrecompileOutput},
    revm::primitives::{Bytes, keccak256},
};

#[document(
    summary = "Where the crypto precompile lives: 0x…4e56, NV in ASCII, beside the names and native precompiles.",
    category = "VM",
)]
pub const CRYPTO_PRECOMPILE: [u8; 20] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x4e, 0x56];

/// What the precompile knows of the chain: its hash, which its proofs are made with.
#[derive(Clone, Copy)]
pub(super) struct CryptoAt {
    pub hash: HashType,
}

/// What a call does, from its signature.
enum Called {
    Verify,
    KeyAddress,
    Hash(HashType),
    Digest(&'static str),
    CheckState,
    CheckMerkle,
}

const FUNCTIONS: &[(&str, &str)] = &[
    ("verify(string,bytes,bytes,bytes)", "(string,bytes,bytes,bytes)"),
    ("keyAddress(string,bytes)", "(string,bytes)"),
    ("blake3(bytes)", "(bytes)"),
    ("blake2s(bytes)", "(bytes)"),
    ("blake2b(bytes)", "(bytes)"),
    ("xxh3(bytes)", "(bytes)"),
    ("hash160(bytes)", "(bytes)"),
    ("sha256d(bytes)", "(bytes)"),
    ("checkState(bytes32,string,bytes,bytes,bool,bytes)", "(bytes32,string,bytes,bytes,bool,bytes)"),
    ("checkMerkle(bytes32,bytes,uint256,uint256,bytes)", "(bytes32,bytes,uint256,uint256,bytes)"),
];

fn called(name: &str) -> Option<Called> {
    Some(match name {
        "verify" => Called::Verify,
        "keyAddress" => Called::KeyAddress,
        #[cfg(feature = "blake3")]
        "blake3" => Called::Hash(HashType::Blake3_256),
        #[cfg(feature = "blake2s")]
        "blake2s" => Called::Hash(HashType::Blake2s256),
        #[cfg(feature = "blake2b")]
        "blake2b" => Called::Hash(HashType::Blake2b256),
        #[cfg(feature = "xxh3")]
        "xxh3" => Called::Hash(HashType::Xxh3_64),
        "hash160" => Called::Digest("hash160"),
        "sha256d" => Called::Digest("sha256d"),
        "checkState" => Called::CheckState,
        "checkMerkle" => Called::CheckMerkle,
        _ => return None,
    })
}

fn word(bool: bool) -> Vec<u8> {
    encode_values(&[AbiType::Bool], &[AbiValue::Bool(bool)]).expect("a bool encodes")
}

/// A fixed-width value, left-aligned in its word, as bytesN returns are.
fn fixed(bytes: &[u8]) -> Vec<u8> {
    let mut out = vec![0; 32];
    out[..bytes.len().min(32)].copy_from_slice(&bytes[..bytes.len().min(32)]);

    out
}

impl CryptoAt {
    /// Answers one call. Bad input, or a scheme the chain lacks, reverts; too little gas halts.
    pub fn answer(&self, input: &[u8], gas_limit: u64, reservoir: u64) -> PrecompileOutput {
        let found = input.get(..4).and_then(|selector| {
            FUNCTIONS.iter().find(|(signature, _)| keccak256(signature.as_bytes())[..4] == *selector)
        });
        let Some((signature, types)) = found else {
            return PrecompileOutput::revert(0, Bytes::new(), reservoir);
        };
        let name = signature.split('(').next().unwrap_or_default();
        let (Some(called), Ok((_, types))) = (called(name), crate::abi::parse_signature(types)) else {
            return PrecompileOutput::revert(0, Bytes::new(), reservoir);
        };
        let Ok(values) = decode_values(&types, &input[4..]) else {
            return PrecompileOutput::revert(0, Bytes::new(), reservoir);
        };
        let gas = self.gas(&called, input.len() as u64);

        if gas > gas_limit {
            return PrecompileOutput::halt(PrecompileHalt::OutOfGas, reservoir);
        }

        match self.run(called, &values) {
            Some(output) => PrecompileOutput::new(gas, Bytes::from(output), reservoir),
            None => PrecompileOutput::revert(gas, Bytes::new(), reservoir),
        }
    }

    /// What a call costs, as the native engines charge for the same work, by the input's size.
    fn gas(&self, called: &Called, len: u64) -> u64 {
        match called {
            Called::Verify => cost::VERIFY + len / cost::HASH_BYTES_PER_FUEL,
            Called::KeyAddress => cost::KEY_ADDRESS,
            Called::Hash(_) | Called::Digest(_) => cost::HASH + len / cost::HASH_BYTES_PER_FUEL,
            Called::CheckState | Called::CheckMerkle => cost::CHECK_PROOF + cost::PROOF_SIBLING * (len / 32),
        }
    }

    fn run(&self, called: Called, values: &[AbiValue]) -> Option<Vec<u8>> {
        let bytes = |index: usize| match values.get(index)? {
            AbiValue::Bytes(bytes) | AbiValue::FixedBytes(bytes) => Some(bytes.as_slice()),
            _ => None,
        };
        let text = |index: usize| match values.get(index)? {
            AbiValue::String(text) => Some(text.as_str()),
            _ => None,
        };
        let whole = |index: usize| match values.get(index)? {
            AbiValue::Uint(number) => u64::try_from(*number).ok(),
            _ => None,
        };

        Some(match called {
            Called::Verify => word(crypto::check_signature(text(0)?, bytes(1)?, bytes(2)?, bytes(3)?).ok()?),
            Called::KeyAddress => {
                let address = crypto::key_address(text(0)?, bytes(1)?).ok()?;

                encode_values(&[AbiType::Bytes], &[AbiValue::Bytes(address.to_abi().to_vec())]).ok()?
            }
            Called::Hash(algorithm) => fixed(&Hasher::digest(algorithm, bytes(0)?).to_bytes()),
            Called::Digest(name) => fixed(&crypto::digest(name, bytes(0)?)?),
            Called::CheckState => {
                let present = matches!(values.get(4)?, AbiValue::Bool(true));

                word(crypto::check_state(self.hash, bytes(0)?, text(1)?, bytes(2)?, present.then_some(bytes(3)?), bytes(5)?))
            }
            Called::CheckMerkle => word(crypto::check_merkle(self.hash, bytes(0)?, bytes(1)?, whole(2)?, whole(3)?, bytes(4)?)),
        })
    }
}

#[cfg(test)]
mod tests;
