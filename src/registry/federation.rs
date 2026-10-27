//! The bridges' federation (docs/bridges.md): the signers who vouch for what crossed from another
//! chain and sign what leaves for one, and how many of them it takes. Genesis may name one, and
//! governance replaces it with Change::Federation; contracts read the one in force, with its
//! version, through the host. Its keys are secp256k1, so the same signers hold a Bitcoin multisig
//! and are checked by an EVM chain's ecrecover.

use {
    crate::crypto::hashable::Hashable,
    r#macro::document,
    wincode::{SchemaRead, SchemaWrite},
};

#[document(
    summary = "Where the federation in force is kept, in the Validators column, with its version: genesis's, written as the chain starts, or the one governance decided last, so a light client proves it.",
    category = "Bridges",
    see_also = "DecidedFederation, Changes::federation",
)]
pub const FEDERATION_KEY: &[u8] = b"change:federation";

#[document(
    summary = "The most signers a federation may have: what one Bitcoin multisig script holds.",
    category = "Bridges",
    see_also = "Federation",
)]
pub const MOST_SIGNERS: usize = 20;

#[document(
    summary = "The bridges' signers: who vouches for a deposit made on another chain, and who signs what leaves for one; how many it takes; and how long each side waits for a lock to be safe before acting on it.",
    category = "Bridges",
    see_also = "Genesis::federation, Change::Federation",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct Federation {
    #[document(summary = "The signers' secp256k1 keys, compressed, 33 bytes each: 1 to MOST_SIGNERS, each once.")]
    pub signers: Vec<[u8; 33]>,
    #[document(summary = "How many of them must sign: 1 to as many as there are.")]
    pub threshold: u32,
    #[document(summary = "How deep a lock on chainv4 must be before the signers vouch for it, on a chain without finality; one with finality waits for it to be final.")]
    pub chain_depth: u64,
    #[document(summary = "How deep a deposit on an EVM chain must be, where its RPC names no finalized block.")]
    pub evm_depth: u64,
    #[document(summary = "How many confirmations a Bitcoin deposit needs before it mints.")]
    pub bitcoin_depth: u32,
}

impl Hashable for Federation {}

impl Federation {
    #[document(
        parent = "Federation",
        summary = "Why it cannot be a chain's: no signers or more than MOST_SIGNERS, one named twice or not a key, a threshold of none or of more than there are, or a depth of 0.",
    )]
    pub fn problem(&self) -> Option<&'static str> {
        if self.signers.is_empty() || self.signers.len() > MOST_SIGNERS {
            return Some("a federation has 1 to 20 signers");
        }

        let mut sorted = self.signers.clone();
        sorted.sort_unstable();

        if sorted.windows(2).any(|pair| pair[0] == pair[1]) {
            return Some("a federation names each signer once");
        }

        #[cfg(feature = "secp256k1")]
        if !self.signers.iter().all(crate::crypto::secp256k1::valid_key) {
            return Some("a federation's signers are compressed secp256k1 keys");
        }

        if self.threshold == 0 || self.threshold as usize > self.signers.len() {
            return Some("a federation's threshold is 1 to as many signers as it has");
        }

        if self.chain_depth == 0 || self.evm_depth == 0 || self.bitcoin_depth == 0 {
            return Some("a federation waits at least one block on each chain");
        }

        None
    }
}

#[document(
    summary = "The federation in force and its version: 0 for genesis's, and one more each time governance decides another, so what was signed for one is told from what was signed for the next.",
    category = "Bridges",
    see_also = "Federation, Changes::federation",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct DecidedFederation {
    #[document(summary = "Its version.")]
    pub version: u64,
    #[document(summary = "The federation.")]
    pub federation: Federation,
}

impl Hashable for DecidedFederation {}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "secp256k1")]
    fn key(byte: u8) -> [u8; 33] {
        crate::crypto::secp256k1::public_key(&[byte; 32]).unwrap()
    }

    #[cfg(feature = "secp256k1")]
    #[test]
    fn a_federation_holds_together_or_says_why_not() {
        let good = Federation { signers: vec![key(1), key(2), key(3)], threshold: 2, chain_depth: 20, evm_depth: 12, bitcoin_depth: 6 };

        assert_eq!(good.problem(), None);

        for (bad, why) in [
            (Federation { signers: Vec::new(), ..good.clone() }, "1 to 20"),
            (Federation { signers: (1..=21).map(key).collect(), ..good.clone() }, "1 to 20"),
            (Federation { signers: vec![key(1), key(1)], ..good.clone() }, "once"),
            (Federation { signers: vec![key(1), [5; 33]], ..good.clone() }, "secp256k1"),
            (Federation { threshold: 0, ..good.clone() }, "threshold"),
            (Federation { threshold: 4, ..good.clone() }, "threshold"),
            (Federation { bitcoin_depth: 0, ..good.clone() }, "at least one block"),
        ] {
            assert!(bad.problem().is_some_and(|problem| problem.contains(why)), "{bad:?}");
        }
    }
}
