//! The proof of work a faucet asks with each request: a nonce whose SHA-256, over the chain's
//! genesis id, the address to pay, the time window and the nonce, starts with as many zero bits
//! as the faucet says. It costs the asker a moment of hashing, which the apps do for them, and
//! costs whoever scripts many addresses as many moments. Bound to one chain, one address and one
//! window, a solution is good for that drip alone, and only for a while.
//!
//! The node's side, POST /faucet, is in the API; this is what it and every client share.

use {
    crate::crypto::hashable::Hashable,
    crate::crypto::hasher::Hasher,
    crate::structs::address::Address,
    crate::structs::hash::{Hash, HashType},
    r#macro::document,
};

#[document(
    summary = "How long a faucet's time window is, in milliseconds: ten minutes. A solution names the window it was worked out in, and the faucet takes the current one and the one before.",
    category = "Faucet",
    see_also = "faucet_hash",
)]
pub const FAUCET_WINDOW_MS: u64 = 10 * 60 * 1_000;

#[document(
    summary = "The window a time falls in.",
    category = "Faucet",
    params = { now = "Milliseconds since the Unix epoch." },
)]
pub fn faucet_window(now: u64) -> u64 {
    now / FAUCET_WINDOW_MS
}

#[document(
    summary = "The hash a solution is judged by: SHA-256 of a fixed label, the genesis id, the address as the chain encodes it, and the window and the nonce, big-endian.",
    category = "Faucet",
)]
pub fn faucet_hash(genesis: &Hash, to: &Address, window: u64, nonce: u64) -> [u8; 32] {
    let mut bytes = b"chainv4 faucet".to_vec();
    bytes.extend(genesis.to_bytes());
    bytes.extend(to.hashable_bytes_with_tag());
    bytes.extend(window.to_be_bytes());
    bytes.extend(nonce.to_be_bytes());

    Hasher::digest(HashType::Sha256, &bytes).to_bytes().try_into().expect("SHA-256 is 32 bytes")
}

#[document(
    summary = "Whether a hash starts with at least this many zero bits.",
    category = "Faucet",
)]
pub fn enough_work(hash: &[u8; 32], bits: u32) -> bool {
    let zeros = hash.iter().position(|byte| *byte != 0).map_or(256, |at| at as u32 * 8 + hash[at].leading_zeros());

    zeros >= bits
}

#[document(
    summary = "Looks for a solution among these nonces, from first, so an app can spread the search over its frames: about 2^bits tries in all, on average.",
    category = "Faucet",
    returns = "The nonce that meets the bits, or None when none of these do.",
)]
pub fn solve_faucet(genesis: &Hash, to: &Address, window: u64, bits: u32, first: u64, tries: u64) -> Option<u64> {
    (first..first.saturating_add(tries)).find(|nonce| enough_work(&faucet_hash(genesis, to, window, *nonce), bits))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solutions_hold_for_their_chain_address_and_window_alone() {
        let genesis = Hash::Sha256([7; 32]);
        let to = Address::Ed25519([1; 32]);
        let nonce = solve_faucet(&genesis, &to, 5, 12, 0, 1 << 20).expect("one in about 4096");

        assert!(enough_work(&faucet_hash(&genesis, &to, 5, nonce), 12));
        assert_ne!(faucet_hash(&genesis, &Address::Ed25519([2; 32]), 5, nonce), faucet_hash(&genesis, &to, 5, nonce), "another address wants its own");
        assert_ne!(faucet_hash(&genesis, &to, 6, nonce), faucet_hash(&genesis, &to, 5, nonce), "another window too");
        assert_ne!(faucet_hash(&Hash::Sha256([8; 32]), &to, 5, nonce), faucet_hash(&genesis, &to, 5, nonce), "and another chain");
        assert!(enough_work(&[0; 32], 256));
        assert!(enough_work(&[0b0000_1111; 32], 4) && !enough_work(&[0b0000_1111; 32], 5));
        assert!(enough_work(&[0, 0b0100_0000, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], 9));
        assert_eq!(faucet_window(FAUCET_WINDOW_MS * 3 + 1), 3);
        assert_eq!(solve_faucet(&genesis, &to, 5, 64, 0, 10), None, "not in ten tries");
    }
}
