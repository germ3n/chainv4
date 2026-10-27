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

/// A block's Merkle root over its transactions' ids, in Bitcoin's own byte order: each row's
/// pairs hashed with SHA-256 twice, the last of an odd row paired with itself.
pub fn merkle_root(txids: &[[u8; 32]]) -> [u8; 32] {
    let mut row = txids.to_vec();

    while row.len() > 1 {
        row = row.chunks(2).map(|pair| sha256d(&[pair[0], *pair.last().expect("a pair holds one at least")].concat())).collect();
    }

    row.first().copied().unwrap_or([0; 32])
}

/// A transaction's Merkle path in its block: its sibling in each row, from its own up, as the
/// Bitcoin relay's includes takes them, the place's bits choosing left or right.
pub fn merkle_path(txids: &[[u8; 32]], index: usize) -> Vec<[u8; 32]> {
    let (mut row, mut at, mut path) = (txids.to_vec(), index, Vec::new());

    while row.len() > 1 {
        path.push(*row.get(at ^ 1).unwrap_or(&row[at]));
        row = row.chunks(2).map(|pair| sha256d(&[pair[0], *pair.last().expect("a pair holds one at least")].concat())).collect();
        at /= 2;
    }

    path
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
    #[test]
    fn merkle_paths_lead_to_the_root_and_block_100000_s_root_is_its_own() {
        use super::{merkle_path, merkle_root, sha256d};

        // Block 100,000's four transactions, as explorers write them, and its Merkle root.
        let shown = [
            "8c14f0db3df150123e6f3dbbf30f8b955a8249b62ac1d1ff16284aefa3d06d87",
            "fff2525b8931402dd09222c50775608f75787bd2b87e56995a7bdd30f79702c4",
            "6359f0868171b1d194cbee1af2f16ea598ae8fad666d9b012c8ed2b79a236ec4",
            "e9a66845e05d5abc0ad04ec80f774a7e585c6e8db975962d069a522137b80c1d",
        ];
        let internal = |text: &str| -> [u8; 32] {
            let mut bytes: [u8; 32] = crate::structs::hex::from_hex(&format!("0x{text}")).unwrap().try_into().unwrap();
            bytes.reverse();
            bytes
        };
        let txids: Vec<[u8; 32]> = shown.iter().map(|text| internal(text)).collect();

        assert_eq!(merkle_root(&txids), internal("f3e94742aca4b5ef85488dc37c06c3282295ffec960994b2c0d5ac2a25a95766"));

        // Every place in rows of every size, odd ones paired with themselves, folds to the root.
        for count in 1..=9u8 {
            let txids: Vec<[u8; 32]> = (0..count).map(|byte| sha256d(&[byte])).collect();

            for index in 0..txids.len() {
                let (mut hash, mut at) = (txids[index], index);

                for sibling in merkle_path(&txids, index) {
                    hash = sha256d(&if at % 2 == 0 { [hash, sibling] } else { [sibling, hash] }.concat());
                    at /= 2;
                }

                assert_eq!(hash, merkle_root(&txids), "{count} transactions, the one at {index}");
            }
        }
    }

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
