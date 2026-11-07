//! Attestations: what lets Ethereum follow chainv4's finality cheaply. Beside its vote, a validator
//! may sign a keccak256 commitment of the link it votes for with a secp256k1 key of its own, its
//! attester, registered on chain apart from its voting key. The commitment names the link, the
//! target's height and state root, and the hashes of the attesting sets of the target's epoch and
//! the next, so a contract on Ethereum (ChainV4Finality.sol) checks a justification with
//! `ecrecover` alone and follows the set as it changes.
//!
//! Everything here is laid out as Solidity's `abi.encode` lays it out, so the contract computes the
//! same digest and set hash from the same values.

use {
    crate::crypto::secp256k1::{keccak, recover_strict, sign_recoverable},
    crate::structs::hash::Hash,
    crate::structs::u256::u256,
    crate::structs::vote::Checkpoint,
    r#macro::document,
};

#[document(
    summary = "A validator's attestation: r, s and v, which is 27 or 28, over an AttestedLink's digest, s always low, as ecrecover takes it.",
    category = "Finality",
    see_also = "AttestedLink, SignedVote::attestation",
)]
pub type Attestation = [u8; 65];

/// Separates attestations from anything else an attester's key might sign.
const DOMAIN: &[u8] = b"chainv4 attestation";

#[document(
    summary = "A 32-byte word for a chainv4 hash, as the contract holds one: its bytes when it has 32, else their keccak256.",
    category = "Finality",
)]
pub fn word(hash: &Hash) -> [u8; 32] {
    let bytes = hash.to_bytes();

    bytes.as_slice().try_into().unwrap_or_else(|_| keccak(&bytes))
}

#[document(
    summary = "An epoch's attesting set: the members with an attester, by attester address ascending, each address once with the weight of every member naming it, and the whole set's weight, attesters or not, which two thirds of is what a justification needs.",
    category = "Finality",
    see_also = "AttestedLink",
)]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AttestingSet {
    #[document(summary = "The weight of every validator in the set.")]
    pub total: u256,
    #[document(summary = "Each attester and its weight, by address ascending.")]
    pub attesters: Vec<([u8; 20], u256)>,
}

impl AttestingSet {
    #[document(
        parent = "AttestingSet",
        summary = "The attesting set of members, each its attester, if it named one, and its weight.",
        params = { members = "Each member's attester and weight." },
    )]
    pub fn of(members: impl IntoIterator<Item = (Option<[u8; 20]>, u256)>) -> AttestingSet {
        let mut total = u256::ZERO;
        let mut attesters: Vec<([u8; 20], u256)> = Vec::new();

        for (attester, weight) in members {
            total = total.saturating_add(weight);

            if let Some(attester) = attester {
                match attesters.iter_mut().find(|(address, _)| *address == attester) {
                    Some((_, summed)) => *summed = summed.saturating_add(weight),
                    None => attesters.push((attester, weight)),
                }
            }
        }

        attesters.sort_by_key(|(address, _)| *address);

        AttestingSet { total, attesters }
    }

    #[document(parent = "AttestingSet", summary = "Its hash, as the contract makes it: keccak256(abi.encode(total, attesters, weights)).")]
    pub fn hash(&self) -> [u8; 32] {
        let count = self.attesters.len();
        let mut encoded = Vec::with_capacity(32 * (5 + 2 * count));
        let number = |value: u256| value.to_be_bytes();
        // The head: total, then where each array starts.
        encoded.extend_from_slice(&number(self.total));
        encoded.extend_from_slice(&number(u256::from(96u64)));
        encoded.extend_from_slice(&number(u256::from(128 + 32 * count as u64)));
        encoded.extend_from_slice(&number(u256::from(count as u64)));

        for (address, _) in &self.attesters {
            let mut word = [0; 32];
            word[12..].copy_from_slice(address);
            encoded.extend_from_slice(&word);
        }

        encoded.extend_from_slice(&number(u256::from(count as u64)));

        for (_, weight) in &self.attesters {
            encoded.extend_from_slice(&number(*weight));
        }

        keccak(&encoded)
    }

    #[document(parent = "AttestingSet", summary = "The weight of an attester, if it is one.")]
    pub fn weight(&self, attester: &[u8; 20]) -> Option<u256> {
        self.attesters.iter().find(|(address, _)| address == attester).map(|(_, weight)| *weight)
    }

    #[document(parent = "AttestingSet", summary = "Whether some weight is two thirds of the whole set's or more, as a justification needs and the contract checks: weight × 3 ≥ total × 2.")]
    pub fn enough(&self, weight: u256) -> bool {
        weight.saturating_mul(u256::from(3u64)) >= self.total.saturating_mul(u256::from(2u64))
    }
}

#[document(
    summary = "What an attestation commits to: the chain, by its EVM chain id; the link voted for; the target's height and state root; and the hashes of the attesting sets of the target's epoch and the next. What ChainV4Finality.sol checks a justification by.",
    category = "Finality",
    see_also = "Attestation, AttestingSet",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AttestedLink {
    #[document(summary = "The chain's id, as its EVM's CHAINID gives it.")]
    pub chain_id: u64,
    #[document(summary = "The link's source.")]
    pub source: Checkpoint,
    #[document(summary = "Its target.")]
    pub target: Checkpoint,
    #[document(summary = "The target's height.")]
    pub height: u64,
    #[document(summary = "The target's state root.")]
    pub state_root: Hash,
    #[document(summary = "The hash of the target epoch's attesting set: the set that signs it.")]
    pub set: [u8; 32],
    #[document(summary = "The hash of the next epoch's attesting set: the set the contract moves to.")]
    pub next_set: [u8; 32],
}

impl AttestedLink {
    #[document(
        parent = "AttestedLink",
        summary = "What an attester signs: keccak256(abi.encode(keccak256(\"chainv4 attestation\"), chain id, source epoch, source block, target epoch, target block, height, state root, set, next set)).",
    )]
    pub fn digest(&self) -> [u8; 32] {
        let number = |value: u64| u256::from(value).to_be_bytes();
        let words: [[u8; 32]; 10] = [
            keccak(DOMAIN),
            number(self.chain_id),
            number(self.source.epoch),
            word(&self.source.block),
            number(self.target.epoch),
            word(&self.target.block),
            number(self.height),
            word(&self.state_root),
            self.set,
            self.next_set,
        ];

        keccak(&words.concat())
    }

    #[document(parent = "AttestedLink", summary = "Signs it with an attester's secret key: r, s low, and v as 27 or 28.")]
    pub fn sign(&self, secret: &[u8; 32]) -> Attestation {
        let (signature, parity) = sign_recoverable(secret, &self.digest());
        let mut attestation = [0; 65];
        attestation[..64].copy_from_slice(&signature);
        attestation[64] = 27 + parity;

        attestation
    }

    #[document(
        parent = "AttestedLink",
        summary = "The attester that signed an attestation of it: the address its signature recovers to, only from one with v 27 or 28 and a low s, so no attestation has two shapes.",
    )]
    pub fn attester(&self, attestation: &Attestation) -> Option<[u8; 20]> {
        let parity = attestation[64].checked_sub(27)?;
        let signature: [u8; 64] = attestation[..64].try_into().ok()?;
        let key = recover_strict(&self.digest(), &signature, parity)?;

        crate::crypto::secp256k1::address(&key)
    }
}

#[cfg(test)]
mod tests;
