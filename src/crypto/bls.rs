//! BLS signatures as Ethereum's consensus layer makes them: keys on G1, 48 bytes compressed,
//! signatures on G2, 96 bytes compressed, messages hashed to G2 by the ciphersuite
//! BLS_SIG_BLS12381G2_XMD:SHA-256_SSWU_RO_POP_. What a contract following Ethereum's sync
//! committees checks their signatures with (contracts/examples/eth_relay), on zkcrypto's
//! bls12_381, in pure Rust.
//!
//! Only FastAggregateVerify, over keys whose proof of possession the beacon chain checked when
//! they joined. Strict and the same on every node: a key or signature that does not decompress to
//! a point of its prime-order group, or decompresses to the point at infinity, refuses the whole
//! check, as does an aggregate key at infinity.

use bls12_381::{
    G1Affine, G1Projective, G2Affine, G2Prepared, G2Projective, Gt,
    hash_to_curve::{ExpandMsgXmd, HashToCurve},
    multi_miller_loop,
};

/// The ciphersuite's domain separation tag: proof-of-possession signatures on G2.
pub const DST: &[u8] = b"BLS_SIG_BLS12381G2_XMD:SHA-256_SSWU_RO_POP_";

/// A public key's bytes, compressed.
pub const PUBLIC_KEY_LEN: usize = 48;

/// A signature's bytes, compressed.
pub const SIGNATURE_LEN: usize = 96;

/// A public key: on the curve, in the group, not at infinity, and its encoding canonical.
pub fn public_key(bytes: &[u8]) -> Option<G1Affine> {
    let point = Option::<G1Affine>::from(G1Affine::from_compressed(bytes.try_into().ok()?))?;

    (!bool::from(point.is_identity())).then_some(point)
}

/// A signature: as a public key is checked, on G2.
pub fn signature(bytes: &[u8]) -> Option<G2Affine> {
    let point = Option::<G2Affine>::from(G2Affine::from_compressed(bytes.try_into().ok()?))?;

    (!bool::from(point.is_identity())).then_some(point)
}

/// A message hashed to G2, by the ciphersuite.
pub fn hash_to_g2(message: &[u8]) -> G2Affine {
    G2Affine::from(<G2Projective as HashToCurve<ExpandMsgXmd<sha2_010::Sha256>>>::hash_to_curve([message], DST))
}

/// FastAggregateVerify: whether a signature is the aggregate of each key's signature over one
/// message. pubkeys is the keys' compressed bytes back to back.
///
/// False for no keys, for bytes that are not whole keys, for a signature that is not 96 bytes, for
/// any key or signature that is no point of its group or is the point at infinity, and for keys
/// that sum to infinity.
pub fn fast_aggregate_verify(pubkeys: &[u8], message: &[u8], signature_bytes: &[u8]) -> bool {
    if pubkeys.is_empty() || !pubkeys.len().is_multiple_of(PUBLIC_KEY_LEN) {
        return false;
    }

    let mut aggregate = G1Projective::identity();

    for key in pubkeys.as_chunks::<PUBLIC_KEY_LEN>().0 {
        let Some(key) = public_key(key) else {
            return false;
        };
        aggregate += key;
    }

    let Some(signature) = signature(signature_bytes) else {
        return false;
    };

    if bool::from(aggregate.is_identity()) {
        return false;
    }

    // e(aggregate, H(m)) == e(g1, signature), as one product that must come to 1.
    let terms = [(&G1Affine::from(aggregate), &G2Prepared::from(hash_to_g2(message))), (&-G1Affine::generator(), &G2Prepared::from(signature))];

    multi_miller_loop(&terms).final_exponentiation() == Gt::identity()
}

#[cfg(test)]
mod tests;
