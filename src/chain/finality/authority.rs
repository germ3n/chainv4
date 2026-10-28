//! Authority finality: the votes of FFG, from signers genesis names, each counting once. A
//! threshold of them voting for one link justifies its target, and justifying the very next
//! checkpoint finalizes the source, as in FFG. There are no bonds to take: conflicting votes are
//! evidence that throws the signer out instead.
//!
//! The signers change by their own approval: once change_threshold of them approve the same new
//! set (Tx::ApproveSigners), it is decided, and each checkpoint fixes the decided set for the epoch
//! activation_delay ahead, as FFG fixes its validator sets. Both thresholds must be more than half
//! the signers, so two rival checkpoints, or two rival changes, cannot both have enough. A signer
//! that signs conflicting votes is thrown out by the evidence (Tx::Slash), with no bond to take.

use {
    super::votes::{Electorate, VoteParams, Voters},
    crate::chain::SignerRules,
    crate::crypto::keypair::PublicKey,
    crate::db::{DbError, State},
    crate::registry::signers::{Signer, SignerSet, Signers},
    crate::structs::hash::Hash,
    crate::structs::u256::u256,
    r#macro::document,
    std::sync::Arc,
};

pub use crate::params::finality::AuthorityConfig;

impl AuthorityConfig {
    #[document(
        parent = "AuthorityConfig",
        summary = "Development settings: one signer, an epoch every 8 blocks, votes 2 blocks deep, changes after 2 epochs, approvals counting for 64 epochs, up to 100 signers, and no rewards.",
        params = { signer = "The one key that votes, which is its own account too." },
    )]
    pub fn dev(signer: PublicKey) -> AuthorityConfig {
        AuthorityConfig {
            checkpoint_interval: 8,
            vote_depth: 2,
            activation_delay: 2,
            max_signers: 100,
            signers: vec![Signer::own(signer)],
            threshold: 1,
            change_threshold: 1,
            approval_epochs: 64,
            vote_reward: u256::ZERO,
            max_proofs_per_block: 4,
        }
    }

    #[document(parent = "AuthorityConfig", summary = "The signers genesis starts with, as a set.")]
    pub fn genesis_set(&self) -> SignerSet {
        SignerSet::new(self.signers.clone(), self.threshold, self.change_threshold)
    }

    #[document(
        parent = "AuthorityConfig",
        summary = "The rules signer changes and ejections run under.",
        params = { chain = "The genesis id, which votes are signed over." },
    )]
    pub fn signer_rules(&self, chain: Hash) -> SignerRules {
        SignerRules {
            max_signers: self.max_signers,
            checkpoint_interval: self.checkpoint_interval,
            approval_epochs: self.approval_epochs,
            activation_delay: self.activation_delay,
            chain,
        }
    }

    /// Settings no chain could finalize with, or safely.
    pub(super) fn problem(&self) -> Option<&'static str> {
        if self.checkpoint_interval == 0 {
            return Some("checkpoint_interval must be at least 1");
        }

        if self.activation_delay == 0 {
            return Some("activation_delay must be at least 1");
        }

        if self.approval_epochs == 0 {
            return Some("approval_epochs must be at least 1");
        }

        self.genesis_set().problem(self.max_signers)
    }
}

/// Writes the genesis signers and fixes them for the epochs genesis already decides.
pub(super) fn write_genesis(state: &State, config: &AuthorityConfig) -> Result<(), DbError> {
    Signers::new(state).write_genesis(&config.genesis_set(), config.activation_delay)
}

/// The signers fixed for each epoch, a threshold of them justifying.
pub(super) struct Appointed {
    params: VoteParams,
    activation_delay: u64,
}

impl Appointed {
    pub(super) fn new(config: &AuthorityConfig) -> Appointed {
        Appointed {
            params: VoteParams {
                checkpoint_interval: config.checkpoint_interval,
                vote_depth: config.vote_depth,
                vote_reward: config.vote_reward,
                max_proofs_per_block: config.max_proofs_per_block,
            },
            activation_delay: config.activation_delay,
        }
    }
}

impl Electorate for Appointed {
    fn name(&self) -> &'static str {
        "authority"
    }

    fn params(&self) -> VoteParams {
        self.params
    }

    fn voters(&self, state: &State, epoch: u64) -> Result<Option<Voters>, DbError> {
        Ok(Signers::new(state).set(epoch)?.map(|set| Voters {
            set: set.validators(),
            needed: u256::from(set.threshold),
        }))
    }

    fn fixed_ahead(&self) -> u64 {
        self.activation_delay
    }

    /// Each checkpoint fixes the decided signers activation_delay epochs ahead.
    fn at_checkpoint(&self, state: &Arc<State>, epoch: u64) -> Result<(), DbError> {
        Signers::new(state).fix_set(epoch + self.activation_delay)
    }
}
