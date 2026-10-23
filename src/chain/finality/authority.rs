//! Authority finality: the votes of FFG, from a fixed set of keys genesis names, each counting
//! once. A threshold of them voting for one link justifies its target, and justifying the very
//! next checkpoint finalizes the source, as in FFG. There are no bonds: conflicting votes are kept
//! as evidence, but nobody can be slashed, and changing the signers takes a new chain.
//!
//! The threshold must be more than half the signers. Then two conflicting checkpoints can only
//! both be finalized if some signer voted against itself, and its votes show it.

use {
    super::votes::{Electorate, VoteParams},
    crate::crypto::hashable::Hashable,
    crate::crypto::keypair::PublicKey,
    crate::db::{DbError, State},
    crate::registry::validators::{Member, ValidatorSet},
    crate::structs::u256::{U256Le, u256},
    r#macro::document,
    wincode::{SchemaRead, SchemaWrite},
};

#[document(
    summary = "Every parameter of authority finality, set in genesis.",
    category = "Finality",
    see_also = "FinalityConfig, Vote",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct AuthorityConfig {
    #[document(summary = "Blocks per epoch. Each epoch's first block is its checkpoint.")]
    pub checkpoint_interval: u64,
    #[document(summary = "How many blocks on top of a checkpoint before signers vote for it.")]
    pub vote_depth: u64,
    #[document(summary = "The keys that vote, each counting once. They cannot change.")]
    pub signers: Vec<PublicKey>,
    #[document(summary = "How many signers voting for one link justify it: more than half of them, and at most all.")]
    pub threshold: u32,
    #[document(summary = "What every signer in a justification gets when a block carries it, paid to its key's address. With none, blocks carry no proofs.")]
    #[wincode(with = "U256Le")]
    pub vote_reward: u256,
    #[document(summary = "The most justifications one block may carry.")]
    pub max_proofs_per_block: u32,
}

impl Hashable for AuthorityConfig {}

impl AuthorityConfig {
    #[document(
        parent = "AuthorityConfig",
        summary = "Development settings: one signer, an epoch every 8 blocks, votes 2 blocks deep, and no rewards.",
        params = { signer = "The one key that votes." },
    )]
    pub fn dev(signer: PublicKey) -> AuthorityConfig {
        AuthorityConfig {
            checkpoint_interval: 8,
            vote_depth: 2,
            signers: vec![signer],
            threshold: 1,
            vote_reward: u256::ZERO,
            max_proofs_per_block: 4,
        }
    }

    /// Settings no chain could finalize with, or safely.
    pub(super) fn problem(&self) -> Option<&'static str> {
        if self.checkpoint_interval == 0 {
            return Some("checkpoint_interval must be at least 1");
        }

        if self.signers.is_empty() {
            return Some("authority finality needs at least one signer");
        }

        let mut sorted = self.signers.clone();
        sorted.sort();
        sorted.dedup();

        if sorted.len() != self.signers.len() {
            return Some("a signer is named twice");
        }

        let count = self.signers.len() as u64;
        let threshold = u64::from(self.threshold);

        if threshold * 2 <= count || threshold > count {
            return Some("threshold must be more than half the signers, and at most all of them");
        }

        None
    }
}

/// The signers as a validator set: each weighs one, and is paid at its own address.
pub(super) fn set(config: &AuthorityConfig) -> ValidatorSet {
    let mut members: Vec<Member> = config
        .signers
        .iter()
        .map(|signer| Member {
            voter: *signer,
            account: signer.address(),
            weight: u256::ONE,
        })
        .collect();
    members.sort_by_key(|member| member.voter);

    ValidatorSet { members }
}

/// The fixed signers, a threshold of them justifying.
pub(super) struct Fixed {
    set: ValidatorSet,
    threshold: u32,
    params: VoteParams,
}

impl Fixed {
    pub(super) fn new(config: &AuthorityConfig) -> Fixed {
        Fixed {
            set: set(config),
            threshold: config.threshold,
            params: VoteParams {
                checkpoint_interval: config.checkpoint_interval,
                vote_depth: config.vote_depth,
                vote_reward: config.vote_reward,
                max_proofs_per_block: config.max_proofs_per_block,
            },
        }
    }
}

impl Electorate for Fixed {
    fn name(&self) -> &'static str {
        "authority"
    }

    fn params(&self) -> VoteParams {
        self.params
    }

    fn set(&self, _state: &State, _epoch: u64) -> Result<Option<ValidatorSet>, DbError> {
        Ok(Some(self.set.clone()))
    }

    fn fixed_ahead(&self) -> Option<u64> {
        None
    }

    fn enough(&self, _set: &ValidatorSet, weight: u256) -> bool {
        weight >= u256::from(self.threshold)
    }
}
