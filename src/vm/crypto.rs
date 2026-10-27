//! What contracts check and hash beyond the hash library, the same for every engine: signatures of
//! every scheme the build has, secp256k1 recovery as Ethereum's ecrecover does, the address a key
//! controls, Bitcoin's hashes, and chainv4's own proofs, a state entry or a Merkle path against a
//! root. Lua reaches them through its crypto library, WebAssembly through host calls, and the EVM
//! through the crypto precompile. Each engine charges the cost:: fuel named for it.

use {
    crate::crypto::keypair::{PublicKey, Signature},
    crate::db::{Column, StateProof},
    crate::structs::address::Address,
    crate::structs::hash::{Hash, HashType},
    r#macro::document,
};

#[document(
    summary = "The schemes signatures are checked for, by name, as contracts name them: those this build has, of ed25519, edblake, secp256k1 and bitcoin.",
    category = "VM",
    see_also = "check_signature",
)]
pub const SCHEMES: &[&str] = &[
    #[cfg(feature = "ed25519")]
    "ed25519",
    #[cfg(feature = "edblake")]
    "edblake",
    #[cfg(feature = "secp256k1")]
    "secp256k1",
    #[cfg(feature = "bitcoin")]
    "bitcoin",
];

/// A key of a scheme, from its bytes: 32 for Ed25519 and EdBlake, 33 compressed for secp256k1 and
/// Bitcoin.
fn parse_key(scheme: &str, key: &[u8]) -> Result<PublicKey, String> {
    let wrong = || format!("a {scheme} key is not {} bytes", key.len());

    match scheme {
        #[cfg(feature = "ed25519")]
        "ed25519" => Ok(PublicKey::Ed25519(key.try_into().map_err(|_| wrong())?)),
        #[cfg(feature = "edblake")]
        "edblake" => Ok(PublicKey::EdBlake(key.try_into().map_err(|_| wrong())?)),
        #[cfg(feature = "secp256k1")]
        "secp256k1" => Ok(PublicKey::Secp256k1(key.try_into().map_err(|_| wrong())?)),
        #[cfg(feature = "bitcoin")]
        "bitcoin" => Ok(PublicKey::Bitcoin(key.try_into().map_err(|_| wrong())?)),
        other => Err(format!("{other} is not a scheme this chain checks: {}", SCHEMES.join(", "))),
    }
}

#[document(
    summary = "Whether a signature over a message is the key's, by the scheme's own rules, as chainv4 signs: strict Ed25519 and EdBlake, low-s ECDSA over the message's keccak256 for secp256k1, as Ethereum's keys sign, and over its double SHA-256 for Bitcoin.",
    params = {
        scheme = "ed25519, edblake, secp256k1 or bitcoin.",
        key = "The key: 32 bytes, or 33 compressed for secp256k1 and Bitcoin.",
        message = "The bytes signed.",
        signature = "64 bytes.",
    },
    returns = "Whether it holds; an error for a scheme the build lacks, or a key of the wrong length. A signature of the wrong length, or a key that is no point, simply does not hold.",
    category = "VM",
    see_also = "PublicKey::verify_signature",
)]
pub fn check_signature(scheme: &str, key: &[u8], message: &[u8], signature: &[u8]) -> Result<bool, String> {
    let key = parse_key(scheme, key)?;
    let Ok(signature) = <[u8; 64]>::try_from(signature) else {
        return Ok(false);
    };
    let signature = match key {
        #[cfg(feature = "ed25519")]
        PublicKey::Ed25519(_) => Signature::Ed25519(signature),
        #[cfg(feature = "edblake")]
        PublicKey::EdBlake(_) => Signature::EdBlake(signature),
        #[cfg(feature = "secp256k1")]
        PublicKey::Secp256k1(_) => Signature::Secp256k1(signature),
        #[cfg(feature = "bitcoin")]
        PublicKey::Bitcoin(_) => Signature::Bitcoin(signature),
    };

    Ok(key.verify_signature(message, &signature).is_ok())
}

#[document(
    summary = "The secp256k1 key that made a signature over a 32-byte digest, as Ethereum's ecrecover finds it, compressed: r, s and v, 65 bytes, v 0, 1, 27 or 28. High-s signatures, which Ethereum refuses too, recover nothing.",
    returns = "The key, or None for a signature that recovers none.",
    category = "VM",
)]
#[cfg(feature = "secp256k1")]
pub fn recover_key(digest: &[u8; 32], signature: &[u8]) -> Option<[u8; 33]> {
    let [rs @ .., v] = signature else {
        return None;
    };
    let rs: &[u8; 64] = rs.try_into().ok()?;
    let parity = match v {
        0 | 27 => 0,
        1 | 28 => 1,
        _ => return None,
    };

    crate::crypto::secp256k1::recover_strict(digest, rs, parity)
}

#[document(
    summary = "The account a key controls, as chainv4 writes it: ed25519:…, edblake:…, secp256k1:… (Ethereum's address) or a bc1q… Bitcoin address.",
    returns = "The address; an error as check_signature gives one.",
    category = "VM",
)]
pub fn key_address(scheme: &str, key: &[u8]) -> Result<Address, String> {
    Ok(parse_key(scheme, key)?.address())
}

#[document(
    summary = "The hashes contracts have here beside the hash library's, by name: ripemd160, 20 bytes; hash160, Bitcoin's RIPEMD-160 of SHA-256, 20 bytes; and sha256d, Bitcoin's SHA-256 twice, 32 bytes, as its block and transaction ids are made.",
    returns = "The digest, or None for a name this build lacks.",
    category = "VM",
)]
pub fn digest(name: &str, data: &[u8]) -> Option<Vec<u8>> {
    match name {
        #[cfg(feature = "bitcoin")]
        "ripemd160" => Some(<ripemd::Ripemd160 as ripemd::Digest>::digest(data).to_vec()),
        #[cfg(feature = "bitcoin")]
        "hash160" => Some(crate::crypto::bitcoin::hash160(data).to_vec()),
        #[cfg(feature = "bitcoin")]
        "sha256d" => Some(crate::crypto::bitcoin::sha256d(data).to_vec()),
        _ => None,
    }
}

#[document(
    summary = "Whether a state proof shows a chain-state entry holds a value under a state root, or with None, nothing: what a light client checks values with, for contracts that take another chainv4 chain's state, or this one's, on a header's word.",
    params = {
        hash = "The hash the root was made with: the chain's.",
        root = "The state root, 32 bytes, from a header the contract trusts.",
        column = "The column by name, such as accounts or namespaces.",
        key = "The entry's key.",
        value = "What it holds, or None for nothing.",
        proof = "The StateProof, as GET /proofs encodes it.",
    },
    returns = "Whether it shows that; false for a proof that does not decode, or a column that holds no chain state.",
    category = "VM",
    see_also = "StateProof::verify",
)]
pub fn check_state(hash: HashType, root: &[u8], column: &str, key: &[u8], value: Option<&[u8]>, proof: &[u8]) -> bool {
    let (Some(root), Some(column)) = (Hash::from_bytes(hash, root), Column::ALL.into_iter().find(|known| known.name() == column)) else {
        return false;
    };
    let Ok(proof) = wincode::deserialize::<StateProof>(proof) else {
        return false;
    };

    proof.verify(&root, column, key, value)
}

#[document(
    summary = "Whether data is the index-th of count leaves under a Merkle root, by its audit path: how a header's tx_root and receipts_root commit to its transactions and receipts.",
    params = {
        hash = "The hash the tree was made with: the chain's.",
        root = "The root, 32 bytes.",
        data = "The leaf's bytes, as the tree took them: a transaction's, or a receipt's.",
        path = "The siblings, 32 bytes each, from the leaf up, back to back.",
    },
    returns = "Whether it is so; false for a path that is not whole hashes.",
    category = "VM",
    see_also = "check_state",
)]
pub fn check_merkle(hash: HashType, root: &[u8], data: &[u8], index: u64, count: u64, path: &[u8]) -> bool {
    let Some(root) = Hash::from_bytes(hash, root) else {
        return false;
    };

    if !path.len().is_multiple_of(32) {
        return false;
    }

    let Some(path) = path.chunks(32).map(|sibling| Hash::from_bytes(hash, sibling)).collect::<Option<Vec<_>>>() else {
        return false;
    };
    let (Ok(index), Ok(count)) = (usize::try_from(index), usize::try_from(count)) else {
        return false;
    };

    crate::crypto::merkle::verify(&root, &crate::crypto::merkle::leaf(hash, data), index, count, &path)
}

#[cfg(test)]
mod tests;
