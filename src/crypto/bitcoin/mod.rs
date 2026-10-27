//! Bitcoin's keys: ECDSA on secp256k1, as Ethereum's, but hashing as Bitcoin does. A key signs the
//! double SHA-256 of what it signs, deterministically (RFC 6979) and with a low s, as Bitcoin
//! requires; verifying refuses a high s. Its address is the HASH160 of its compressed key,
//! RIPEMD-160 over SHA-256, which Bitcoin's native segwit addresses (P2WPKH) write as bc1q…, so
//! the same key has the same bc1q address here as in Bitcoin's wallets.
//!
//! Keys are secp256k1's: a 32-byte scalar, and a public key compressed in 33 bytes.

use {
    crate::crypto::secp256k1,
    sha2::{Digest, Sha256},
};

pub mod tx;

/// SHA-256 once: what a P2WSH output commits its script by, and a swap's hash lock its secret.
pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

/// SHA-256 twice, as Bitcoin hashes what it signs.
pub fn sha256d(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(Sha256::digest(bytes)).into()
}

/// RIPEMD-160 of SHA-256, as Bitcoin hashes a key into its address.
pub fn hash160(bytes: &[u8]) -> [u8; 20] {
    <ripemd::Ripemd160 as ripemd::Digest>::digest(Sha256::digest(bytes)).into()
}

/// Signs a message: the double SHA-256 of it, 64 bytes, r then s, s always low.
///
/// Panics on bytes that are not a private key, which a PrivateKey never holds.
pub fn sign(secret: &[u8; 32], message: &[u8]) -> [u8; 64] {
    secp256k1::sign_hash(secret, &sha256d(message))
}

/// Whether a signature was made over a message by this public key's private key. A high s is
/// refused.
pub fn verify(key: &[u8; 33], message: &[u8], signature: &[u8; 64]) -> bool {
    secp256k1::verify_hash(key, &sha256d(message), signature)
}

/// A public key's address: the HASH160 of its compressed form. None for bytes that are not a point.
pub fn address(key: &[u8; 33]) -> Option<[u8; 20]> {
    secp256k1::valid_key(key).then(|| hash160(key))
}

#[cfg(test)]
mod tests {
    use {super::*, crate::structs::bech32::encode_segwit, crate::structs::hex::from_hex};

    fn bytes<const N: usize>(hex: &str) -> [u8; N] {
        from_hex(hex).unwrap().try_into().unwrap()
    }

    #[test]
    fn addresses_are_bitcoins() {
        // BIP173's example key and its P2WPKH address.
        let key = bytes::<33>("0x0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798");

        assert_eq!(encode_segwit("bc", &address(&key).unwrap()), "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4");
        assert_eq!(address(&[0; 33]), None, "no point, no address");
    }

    #[test]
    fn signatures_check_only_for_their_key_message_and_hash() {
        let secret = bytes::<32>("0x4c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f362318");
        let key = secp256k1::public_key(&secret).unwrap();
        let signature = sign(&secret, b"hello");

        assert!(verify(&key, b"hello", &signature));
        assert!(!verify(&key, b"hellp", &signature));
        assert!(!secp256k1::verify(&key, b"hello", &signature), "Ethereum's keccak256 is another message");
        assert_eq!(sha256d(b"hello"), bytes::<32>("0x9595c9df90075148eb06860365df33584b75bff782a510c6cd4883a419833d50"));
    }
}
