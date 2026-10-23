//! Ed25519 with BLAKE3 in place of SHA-512: the scheme chainv3's ed25519-dalek-blake3 fork
//! implements, rebuilt on ed25519-dalek's hazmat API so keys and signatures stay byte-identical.

use {
    curve25519_dalek::edwards::CompressedEdwardsY,
    digest::consts::U64,
    digest::{FixedOutput, HashMarker, Output, OutputSizeUser, Update},
    ed25519_dalek::VerifyingKey,
    ed25519_dalek::hazmat::{ExpandedSecretKey, raw_sign, raw_verify},
};

/// BLAKE3 read out to the 64 bytes Ed25519 needs from its hash, as the fork does.
#[derive(Clone, Default)]
pub struct Blake3_512(blake3::Hasher);

impl HashMarker for Blake3_512 {}

impl OutputSizeUser for Blake3_512 {
    type OutputSize = U64;
}

impl Update for Blake3_512 {
    fn update(&mut self, data: &[u8]) {
        self.0.update(data);
    }
}

impl FixedOutput for Blake3_512 {
    fn finalize_into(self, out: &mut Output<Self>) {
        self.0.finalize_xof().fill(out);
    }
}

fn expand(seed: &[u8; 32]) -> ExpandedSecretKey {
    let mut hash = [0; 64];
    let mut hasher = blake3::Hasher::new();
    hasher.update(seed);
    hasher.finalize_xof().fill(&mut hash);

    ExpandedSecretKey::from_bytes(&hash)
}

pub fn public_key(seed: &[u8; 32]) -> [u8; 32] {
    VerifyingKey::from(&expand(seed)).to_bytes()
}

pub fn sign(seed: &[u8; 32], message: &[u8]) -> [u8; 64] {
    let key = expand(seed);
    let public = VerifyingKey::from(&key);

    raw_sign::<Blake3_512>(&key, message, &public).to_bytes()
}

/// The same checks as ed25519-dalek's verify_strict: besides the equation itself, a small-order
/// key or R is rejected, since either lets one signature pass for many messages.
pub fn verify(key: &[u8; 32], message: &[u8], signature: &[u8; 64]) -> bool {
    let Ok(key) = VerifyingKey::from_bytes(key) else {
        return false;
    };
    let mut r = [0; 32];
    r.copy_from_slice(&signature[..32]);
    let small_r = CompressedEdwardsY(r)
        .decompress()
        .is_none_or(|point| point.is_small_order());

    if small_r || key.is_weak() {
        return false;
    }

    let signature = ed25519_dalek::Signature::from_bytes(signature);

    raw_verify::<Blake3_512>(&key, message, &signature).is_ok()
}
