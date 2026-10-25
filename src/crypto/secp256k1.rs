//! Ethereum's keys: ECDSA on secp256k1. A key signs the keccak256 of what it signs, as Ethereum
//! hashes what it signs, deterministically (RFC 6979), and always with a low s, so no signature can
//! be reshaped into a second valid one; verifying refuses a high s. Its address is the last 20
//! bytes of the keccak256 of its public point, as Ethereum's are, so the same key has the same 0x
//! address here as there.
//!
//! The private key is a 32-byte scalar, as the Edwards schemes' seeds are 32 bytes; a public key is
//! written compressed, in 33 bytes.

use {
    k256::ecdsa::signature::hazmat::{PrehashSigner, PrehashVerifier},
    k256::ecdsa::{Signature, SigningKey, VerifyingKey},
    sha3::{Digest, Keccak256},
};

/// What a key signs: the keccak256 of the message.
fn prehash(message: &[u8]) -> [u8; 32] {
    Keccak256::digest(message).into()
}

/// Whether 32 bytes are a private key: a scalar above zero and below the group's order.
pub fn valid_secret(secret: &[u8; 32]) -> bool {
    SigningKey::from_slice(secret).is_ok()
}

/// The compressed public key of a private key, or None for bytes that are not one.
pub fn public_key(secret: &[u8; 32]) -> Option<[u8; 33]> {
    let key = SigningKey::from_slice(secret).ok()?;

    key.verifying_key().to_sec1_point(true).as_bytes().try_into().ok()
}

/// Signs a message: 64 bytes, r then s, s always low.
///
/// Panics on bytes that are not a private key, which a PrivateKey never holds.
pub fn sign(secret: &[u8; 32], message: &[u8]) -> [u8; 64] {
    let key = SigningKey::from_slice(secret).expect("a secp256k1 private key is a valid scalar");
    let signature: Signature = key.sign_prehash(&prehash(message)).expect("signing a 32-byte hash never fails");

    signature.normalize_s().to_bytes().into()
}

/// Whether a signature was made over a message by this public key's private key. A high s is
/// refused.
pub fn verify(key: &[u8; 33], message: &[u8], signature: &[u8; 64]) -> bool {
    let (Ok(key), Ok(signature)) = (VerifyingKey::from_sec1_bytes(key), Signature::from_slice(signature)) else {
        return false;
    };

    key.verify_prehash(&prehash(message), &signature).is_ok()
}

/// keccak256, as Ethereum hashes: what EIP-712 hashes with.
pub fn keccak(bytes: &[u8]) -> [u8; 32] {
    Keccak256::digest(bytes).into()
}

/// Signs a 32-byte hash as it is, as EIP-712 has a key sign its typed hash: r then s, s always low.
///
/// Panics on bytes that are not a private key, which a PrivateKey never holds.
pub fn sign_hash(secret: &[u8; 32], hash: &[u8; 32]) -> [u8; 64] {
    let key = SigningKey::from_slice(secret).expect("a secp256k1 private key is a valid scalar");
    let signature: Signature = key.sign_prehash(hash).expect("signing a 32-byte hash never fails");

    signature.normalize_s().to_bytes().into()
}

/// Whether a signature was made over a 32-byte hash, as it is, by this public key's private key. A
/// high s is refused.
pub fn verify_hash(key: &[u8; 33], hash: &[u8; 32], signature: &[u8; 64]) -> bool {
    let (Ok(key), Ok(signature)) = (VerifyingKey::from_sec1_bytes(key), Signature::from_slice(signature)) else {
        return false;
    };

    key.verify_prehash(hash, &signature).is_ok()
}

/// The public key that made a signature over a 32-byte hash, from the 65 bytes Ethereum's wallets
/// give: r, s and v, which is 27 or 28, or 0 or 1. With it, the signature as chainv4 takes it: its
/// s made low, which signs the same. None for one that recovers no key.
pub fn recover(hash: &[u8; 32], signature: &[u8; 65]) -> Option<([u8; 33], [u8; 64])> {
    let parity = match signature[64] {
        27 | 28 => signature[64] - 27,
        0 | 1 => signature[64],
        _ => return None,
    };
    let id = k256::ecdsa::RecoveryId::from_byte(parity)?;
    let signed = Signature::from_slice(&signature[..64]).ok()?;
    let key = VerifyingKey::recover_from_prehash(hash, &signed, id).ok()?;

    Some((key.to_sec1_point(true).as_bytes().try_into().ok()?, signed.normalize_s().to_bytes().into()))
}

/// Whether 33 bytes are a compressed point on the curve.
pub fn valid_key(key: &[u8; 33]) -> bool {
    VerifyingKey::from_sec1_bytes(key).is_ok()
}

/// A public key's address, as Ethereum makes it: the last 20 bytes of the keccak256 of the
/// uncompressed point, without its 0x04 prefix. None for bytes that are not a point.
pub fn address(key: &[u8; 33]) -> Option<[u8; 20]> {
    let point = VerifyingKey::from_sec1_bytes(key).ok()?.to_sec1_point(false);
    let hash = Keccak256::digest(&point.as_bytes()[1..]);

    hash[12..].try_into().ok()
}

#[cfg(test)]
mod tests {
    use {super::*, crate::structs::hex::from_hex};

    fn bytes<const N: usize>(hex: &str) -> [u8; N] {
        from_hex(hex).unwrap().try_into().unwrap()
    }

    #[test]
    fn addresses_are_ethereums() {
        // A widely used test key: private key 1 has the address 0x7e5f…2bdf.
        let mut one = [0; 32];
        one[31] = 1;
        let key = public_key(&one).unwrap();

        assert_eq!(key, bytes::<33>("0x0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"));
        assert_eq!(address(&key).unwrap(), bytes::<20>("0x7e5f4552091a69125d5dfcb7b8c2659029395bdf"));
        // And the key in the go-ethereum tests, with its address.
        let secret = bytes::<32>("0x4c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f362318");

        assert_eq!(address(&public_key(&secret).unwrap()).unwrap(), bytes::<20>("0x2c7536e3605d9c16a7a3d7b1898e529396a65c23"));
    }

    #[test]
    fn signatures_check_only_for_their_key_message_and_low_s() {
        let secret = bytes::<32>("0x4c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f362318");
        let key = public_key(&secret).unwrap();
        let signature = sign(&secret, b"hello");

        assert_eq!(signature, sign(&secret, b"hello"), "deterministic");
        assert!(verify(&key, b"hello", &signature));
        assert!(!verify(&key, b"hellp", &signature));
        assert!(!verify(&public_key(&[7; 32]).unwrap(), b"hello", &signature));

        // The same signature with s mirrored, n - s, checks under plain ECDSA but not here.
        let high = Signature::from_slice(&signature).unwrap();
        let (r, s) = high.split_scalars();
        let mirrored = Signature::from_scalars(r, -*s).unwrap();

        assert!(!verify(&key, b"hello", &mirrored.to_bytes().into()), "a high s is refused");
        assert!(!valid_secret(&[0; 32]), "zero is no key");
        assert!(!valid_secret(&[0xff; 32]), "nor is anything past the order");
    }
}
