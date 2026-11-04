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
    #[document(summary = "What the Bitcoin gateway lets releases pay and how its federation works, or None for what each gateway was set up with. Governance changes it with the federation, and the gateway reads it as it runs.")]
    pub btc: Option<BtcPolicy>,
}

impl Hashable for Federation {}

#[document(
    summary = "The federation's Bitcoin policy, which the Bitcoin gateway and its signers go by from the block a change of federation decides it: the most fee a release may pay, the least it pays a virtual byte, how long a retired federation's deposit addresses stay open, how long a release waits before its signers vote it down, the share of a withdrawal its fee may take unless the withdrawal says, when a release with all its signatures that Bitcoin has not mined is bumped, and how later releases repay what a child cost the backing.",
    category = "Bridges",
    see_also = "Federation::btc, Change::Federation",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct BtcPolicy {
    #[document(summary = "The most fee a release may pay, in satoshis. It must pay the smallest release at the floor.")]
    pub fee_cap: u64,
    #[document(summary = "The least a release pays a virtual byte, in satoshis. At least 1.")]
    pub fee_floor: u64,
    #[document(summary = "How long a retired federation's deposit addresses stay open, in milliseconds.")]
    pub grace_ms: u64,
    #[document(summary = "How long a release waits for Bitcoin before the federation's signers vote to cancel it, in milliseconds: what each signer once chose for itself with --cancel-after.")]
    pub cancel_after_ms: u64,
    #[document(summary = "A withdrawal that names no most fee may lose at most its sats divided by this to its release's fee: 10, a tenth, by default. At least 1.")]
    pub fee_part: u64,
    #[document(summary = "How long, in milliseconds of block time, a release with all its signatures may go unmined before its signers bump it, and its bump again before they bump that: BUMP_AFTER_MS unless set.")]
    pub bump_after_ms: u64,
    #[document(summary = "How far under what the network asks a release must pay to be bumped, as a percent of that rate: BUMP_BELOW_PCT unless set. 1 to 100.")]
    pub bump_below_pct: u64,
    #[document(summary = "The most a release repays of what children cost the backing, as a percent of its own fee: REPAY_PCT unless set. 0 to 100; 0 never repays.")]
    pub repay_pct: u64,
    #[document(summary = "The most any one withdrawal repays of it, in satoshis: REPAY_MOST unless set. It counts within the withdrawal's most fee.")]
    pub repay_most: u64,
}

impl Hashable for BtcPolicy {}

impl BtcPolicy {
    #[document(parent = "BtcPolicy", summary = "The smallest release there is, in virtual bytes: one input under 20 of 20 signers, paying a withdrawal and change. The cap must pay it at the floor.")]
    pub const ONE_INPUT_VBYTES: u64 = 710;

    #[document(
        parent = "BtcPolicy",
        summary = "How long a release with all its signatures waits for Bitcoin before it is bumped, unless the policy says: an hour, twice the three blocks its rate was asked for, so one mined in the usual time is never bumped, and well inside the six hours a release short of its signatures waits before it is voted down.",
    )]
    pub const BUMP_AFTER_MS: u64 = 3_600_000;

    #[document(
        parent = "BtcPolicy",
        summary = "How far under the network's rate a release must pay to be bumped, unless the policy says: under 80 percent of it. Estimates wander by a tenth or so from one minute to the next, and a bump costs its withdrawals the whole fee again at the new rate, so a release only a little under waits for the next block or two rather than pay for that.",
    )]
    pub const BUMP_BELOW_PCT: u64 = 80;

    #[document(
        parent = "BtcPolicy",
        summary = "How much of its own fee a release repays at most of what children cost the backing, unless the policy says: a fifth. A child's fee is about what pulls one release up to the network's rate, so some five releases repay it, each withdrawer paying little.",
    )]
    pub const REPAY_PCT: u64 = 20;

    #[document(
        parent = "BtcPolicy",
        summary = "The most one withdrawal repays of it, unless the policy says: 500 satoshis, half of what the least withdrawal agrees to lose by default, so it never takes most of anyone's limit.",
    )]
    pub const REPAY_MOST: u64 = 500;

    #[document(parent = "BtcPolicy", summary = "Why no gateway could go by it: a floor of 0, a cap that pays no release at the floor, no fee share, no wait before a cancel or a bump, or a bump threshold or repayment that is no percent.")]
    pub fn problem(&self) -> Option<&'static str> {
        if self.fee_floor == 0 {
            Some("btc.fee_floor is at least 1 satoshi a virtual byte")
        } else if self.fee_cap < self.fee_floor.saturating_mul(BtcPolicy::ONE_INPUT_VBYTES) {
            Some("btc.fee_cap is under what the floor asks of the smallest release: it would allow none")
        } else if self.fee_part == 0 {
            Some("btc.fee_part is at least 1")
        } else if self.cancel_after_ms == 0 {
            Some("btc.cancel_after_ms is at least 1")
        } else if self.bump_after_ms == 0 {
            Some("btc.bump_after_ms is at least 1")
        } else if !(1..=100).contains(&self.bump_below_pct) {
            Some("btc.bump_below_pct is 1 to 100")
        } else if self.repay_pct > 100 {
            Some("btc.repay_pct is 0 to 100")
        } else {
            None
        }
    }
}

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

        self.btc.as_ref().and_then(BtcPolicy::problem)
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

#[cfg(all(test, feature = "secp256k1"))]
mod tests {
    use super::*;

    fn key(byte: u8) -> [u8; 33] {
        crate::crypto::secp256k1::public_key(&[byte; 32]).unwrap()
    }

    #[test]
    fn a_federation_holds_together_or_says_why_not() {
        let good = Federation { signers: vec![key(1), key(2), key(3)], threshold: 2, chain_depth: 20, evm_depth: 12, bitcoin_depth: 6, btc: None };
        let policy = BtcPolicy { fee_cap: 50_000, fee_floor: 2, grace_ms: 3_600_000, cancel_after_ms: 3_600_000, fee_part: 10, bump_after_ms: 3_600_000, bump_below_pct: 80, repay_pct: 20, repay_most: 500 };

        assert_eq!(good.problem(), None);
        assert_eq!(Federation { btc: Some(policy), ..good.clone() }.problem(), None);

        for (bad, why) in [
            (Federation { signers: Vec::new(), ..good.clone() }, "1 to 20"),
            (Federation { signers: (1..=21).map(key).collect(), ..good.clone() }, "1 to 20"),
            (Federation { signers: vec![key(1), key(1)], ..good.clone() }, "once"),
            (Federation { signers: vec![key(1), [5; 33]], ..good.clone() }, "secp256k1"),
            (Federation { threshold: 0, ..good.clone() }, "threshold"),
            (Federation { threshold: 4, ..good.clone() }, "threshold"),
            (Federation { bitcoin_depth: 0, ..good.clone() }, "at least one block"),
            (Federation { btc: Some(BtcPolicy { fee_floor: 0, ..policy }), ..good.clone() }, "fee_floor"),
            (Federation { btc: Some(BtcPolicy { fee_cap: 1_000, ..policy }), ..good.clone() }, "allow none"),
            (Federation { btc: Some(BtcPolicy { fee_part: 0, ..policy }), ..good.clone() }, "fee_part"),
            (Federation { btc: Some(BtcPolicy { bump_after_ms: 0, ..policy }), ..good.clone() }, "bump_after_ms"),
            (Federation { btc: Some(BtcPolicy { bump_below_pct: 0, ..policy }), ..good.clone() }, "bump_below_pct"),
            (Federation { btc: Some(BtcPolicy { bump_below_pct: 101, ..policy }), ..good.clone() }, "bump_below_pct"),
            (Federation { btc: Some(BtcPolicy { repay_pct: 101, ..policy }), ..good.clone() }, "repay_pct"),
        ] {
            assert!(bad.problem().is_some_and(|problem| problem.contains(why)), "{bad:?}");
        }
    }
}
