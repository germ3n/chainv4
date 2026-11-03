//! Which finality a chain runs, as its genesis names it.

// Only FFG keeps anything in chain state, so without it some parameters go unused. A build without
// any kind has an empty FinalityConfig, whose methods all match on nothing.
#![cfg_attr(not(feature = "finality-ffg"), allow(unused_variables))]
#![cfg_attr(
    not(any(feature = "finality-ffg", feature = "finality-authority", feature = "finality-depth")),
    allow(unreachable_code)
)]

#[cfg(feature = "finality-authority")]
use super::authority::{self};
#[cfg(feature = "finality-authority")]
use crate::registry::signers::Signers;
#[cfg(feature = "finality-depth")]
use super::depth::Depth;
#[cfg(feature = "finality-ffg")]
use super::ffg::{self, FfgConfig};
#[cfg(any(feature = "finality-ffg", feature = "finality-authority"))]
use super::votes::Votes;
use {
    super::{Finality, View},
    crate::chain::{SignerRules, StakingRules},
    crate::crypto::keypair::PublicKey,
    crate::db::{DbError, State},
    crate::registry::validators::ValidatorSet,
    crate::structs::hash::Hash,
    r#macro::document,
};

pub use crate::params::finality::FinalityConfig;

impl FinalityConfig {
    #[document(
        parent = "FinalityConfig",
        summary = "The development chains' finality: FFG with the dev key as the one validator, or none in a build without FFG.",
        params = { validator = "The genesis validator's key, which both owns the bond and votes." },
    )]
    #[cfg(feature = "finality-ffg")]
    pub fn dev(validator: PublicKey) -> Option<FinalityConfig> {
        Some(FinalityConfig::Ffg(FfgConfig::dev(validator)))
    }

    #[cfg(not(feature = "finality-ffg"))]
    pub fn dev(_validator: PublicKey) -> Option<FinalityConfig> {
        None
    }

    #[document(
        parent = "FinalityConfig",
        summary = "The account a genesis signer's key speaks through, under authority finality: what a rotating producer with that key names as its blocks' producer. None under any other kind, or for a key genesis does not name; a signer added since has its account in the decided set.",
    )]
    #[cfg_attr(not(feature = "finality-authority"), allow(unused_variables))]
    pub fn signer_account(&self, key: &PublicKey) -> Option<crate::structs::address::Address> {
        match *self {
            #[cfg(feature = "finality-authority")]
            FinalityConfig::Authority(ref config) => config.signers.iter().find(|signer| signer.key == *key).map(|signer| signer.account),
            #[allow(unreachable_patterns)]
            _ => None,
        }
    }

    #[document(parent = "FinalityConfig", summary = "Whether validators vote: true for ffg and authority, false for depth.")]
    pub fn votes(&self) -> bool {
        match *self {
            #[cfg(feature = "finality-ffg")]
            FinalityConfig::Ffg(_) => true,
            #[cfg(feature = "finality-authority")]
            FinalityConfig::Authority(_) => true,
            #[cfg(feature = "finality-depth")]
            FinalityConfig::Depth(_) => false,
        }
    }

    #[document(
        parent = "FinalityConfig",
        summary = "Blocks per epoch. Each epoch's first block is its checkpoint; under depth finality, every block is one.",
    )]
    pub fn checkpoint_interval(&self) -> u64 {
        match *self {
            #[cfg(feature = "finality-ffg")]
            FinalityConfig::Ffg(ref config) => config.checkpoint_interval,
            #[cfg(feature = "finality-authority")]
            FinalityConfig::Authority(ref config) => config.checkpoint_interval,
            #[cfg(feature = "finality-depth")]
            FinalityConfig::Depth(_) => 1,
        }
    }

    /// What its votes run by: blocks per epoch, the vote depth, the vote reward and proofs per
    /// block; None for a kind without votes.
    #[cfg(any(feature = "finality-ffg", feature = "finality-authority"))]
    pub(super) fn vote_params(&self) -> Option<super::votes::VoteParams> {
        match *self {
            #[cfg(feature = "finality-ffg")]
            FinalityConfig::Ffg(ref config) => Some(super::votes::VoteParams {
                checkpoint_interval: config.checkpoint_interval,
                vote_depth: config.vote_depth,
                vote_reward: config.vote_reward,
                max_proofs_per_block: config.max_proofs_per_block,
            }),
            #[cfg(feature = "finality-authority")]
            FinalityConfig::Authority(ref config) => Some(super::votes::VoteParams {
                checkpoint_interval: config.checkpoint_interval,
                vote_depth: config.vote_depth,
                vote_reward: config.vote_reward,
                max_proofs_per_block: config.max_proofs_per_block,
            }),
            #[cfg(feature = "finality-depth")]
            FinalityConfig::Depth(_) => None,
        }
    }

    /// FFG's inactivity leak, if it has one; no other kind leaks.
    #[cfg(any(feature = "finality-ffg", feature = "finality-authority"))]
    pub(super) fn leak(&self) -> Option<crate::registry::validators::Leak> {
        match *self {
            #[cfg(feature = "finality-ffg")]
            FinalityConfig::Ffg(ref config) => config.leak,
            #[allow(unreachable_patterns)]
            _ => None,
        }
    }

    #[document(parent = "FinalityConfig", summary = "Epochs before what is decided for voters takes effect: activation_delay under ffg and authority, 0 under depth.")]
    pub fn activation_delay(&self) -> u64 {
        match *self {
            #[cfg(feature = "finality-ffg")]
            FinalityConfig::Ffg(ref config) => config.activation_delay,
            #[cfg(feature = "finality-authority")]
            FinalityConfig::Authority(ref config) => config.activation_delay,
            #[cfg(feature = "finality-depth")]
            FinalityConfig::Depth(_) => 0,
        }
    }

    #[document(parent = "FinalityConfig", summary = "The epoch a height falls in.")]
    pub fn epoch_of(&self, height: u64) -> u64 {
        height / self.checkpoint_interval().max(1)
    }

    #[document(parent = "FinalityConfig", summary = "The height of an epoch's checkpoint.")]
    pub fn checkpoint_height(&self, epoch: u64) -> u64 {
        epoch.saturating_mul(self.checkpoint_interval())
    }

    #[document(
        parent = "FinalityConfig",
        summary = "The rules staking transactions run under, for kinds with bonds.",
        params = { chain = "The genesis id, which votes are signed over." },
    )]
    pub fn staking(&self, chain: Hash) -> Option<StakingRules> {
        match *self {
            #[cfg(feature = "finality-ffg")]
            FinalityConfig::Ffg(ref config) => Some(StakingRules {
                staking: config.staking(),
                checkpoint_interval: config.checkpoint_interval,
                chain,
                bonding: true,
            }),
            #[cfg(feature = "finality-authority")]
            FinalityConfig::Authority(_) => None,
            #[cfg(feature = "finality-depth")]
            FinalityConfig::Depth(_) => None,
        }
    }

    #[document(
        parent = "FinalityConfig",
        summary = "The rules signer changes run under, for authority finality.",
    )]
    #[cfg_attr(not(feature = "finality-authority"), allow(unused_variables))]
    pub fn signer_rules(&self, chain: Hash) -> Option<SignerRules> {
        match *self {
            #[cfg(feature = "finality-ffg")]
            FinalityConfig::Ffg(_) => None,
            #[cfg(feature = "finality-authority")]
            FinalityConfig::Authority(ref config) => Some(config.signer_rules(chain)),
            #[cfg(feature = "finality-depth")]
            FinalityConfig::Depth(_) => None,
        }
    }

    #[document(
        parent = "FinalityConfig",
        summary = "Who votes in an epoch, as a state knows it: the bonded set for FFG, the signers fixed for it under authority finality, and none for depth.",
    )]
    pub fn voters(&self, state: &State, epoch: u64) -> Result<Option<ValidatorSet>, DbError> {
        match *self {
            #[cfg(feature = "finality-ffg")]
            FinalityConfig::Ffg(_) => crate::registry::validators::Validators::new(state).set(epoch),
            #[cfg(feature = "finality-authority")]
            FinalityConfig::Authority(_) => Ok(Signers::new(state).set(epoch)?.map(|set| set.validators())),
            #[cfg(feature = "finality-depth")]
            FinalityConfig::Depth(_) => Ok(None),
        }
    }

    #[document(
        parent = "FinalityConfig",
        summary = "Who votes in an epoch by the kind that epoch runs: the one governance switched to, or this one, genesis's.",
    )]
    pub fn voters_in(&self, state: &State, epoch: u64) -> Result<Option<ValidatorSet>, DbError> {
        crate::registry::changes::Changes::new(state).finality_in(epoch, self)?.voters(state, epoch)
    }

    #[document(
        parent = "FinalityConfig",
        summary = "Why governance cannot put this in place of the finality decided last, as Change::Finality: another kind, which Change::SwitchFinality switches to, another checkpoint_interval or activation_delay, which the epochs and the fixed-ahead sets rest on, other starting validators or signers, which history keeps, or other thresholds, which the signers change by their own approval; or settings no chain could finalize with.",
        params = { decided = "The finality decided last: genesis's, or what governance switched to or retuned since." },
    )]
    pub fn retune_problem(&self, decided: &FinalityConfig) -> Option<&'static str> {
        let kept = match (self, decided) {
            #[cfg(feature = "finality-ffg")]
            (FinalityConfig::Ffg(new), FinalityConfig::Ffg(old)) => {
                new.checkpoint_interval == old.checkpoint_interval && new.activation_delay == old.activation_delay && new.validators == old.validators
            }
            #[cfg(feature = "finality-authority")]
            (FinalityConfig::Authority(new), FinalityConfig::Authority(old)) => {
                new.checkpoint_interval == old.checkpoint_interval
                    && new.activation_delay == old.activation_delay
                    && new.signers == old.signers
                    && new.threshold == old.threshold
                    && new.change_threshold == old.change_threshold
            }
            #[cfg(feature = "finality-depth")]
            (FinalityConfig::Depth(_), FinalityConfig::Depth(_)) => true,
            #[allow(unreachable_patterns)]
            _ => return Some("a change of finality keeps its kind; a switch changes it"),
        };

        match kept {
            false => Some("a change keeps checkpoint_interval, activation_delay, the starting validators or signers and the thresholds as they are"),
            true => self.problem(),
        }
    }

    /// Settings no chain could finalize with.
    pub(in crate::chain) fn problem(&self) -> Option<&'static str> {
        match *self {
            #[cfg(feature = "finality-ffg")]
            FinalityConfig::Ffg(ref config) => config.problem(),
            #[cfg(feature = "finality-authority")]
            FinalityConfig::Authority(ref config) => config.problem(),
            #[cfg(feature = "finality-depth")]
            FinalityConfig::Depth(_) => None,
        }
    }

    /// Writes what the kind keeps in chain state from the start, such as genesis validators.
    pub(in crate::chain) fn write_genesis(&self, state: &State) -> Result<(), DbError> {
        match *self {
            #[cfg(feature = "finality-ffg")]
            FinalityConfig::Ffg(ref config) => ffg::write_genesis(state, config),
            #[cfg(feature = "finality-authority")]
            FinalityConfig::Authority(ref config) => authority::write_genesis(state, config),
            #[cfg(feature = "finality-depth")]
            FinalityConfig::Depth(_) => Ok(()),
        }
    }

    #[document(
        parent = "FinalityConfig",
        summary = "The finality these settings describe, with what this node learned before: the votes it counted or held back are counted again.",
        params = { chain = "The chain as it was opened." },
    )]
    pub fn build(&self, chain: &View) -> Result<Box<dyn Finality>, DbError> {
        Ok(match *self {
            #[cfg(feature = "finality-ffg")]
            FinalityConfig::Ffg(_) => Box::new(Votes::load(super::switch::Elected::new(self.clone()), chain)?),
            #[cfg(feature = "finality-authority")]
            FinalityConfig::Authority(_) => Box::new(Votes::load(super::switch::Elected::new(self.clone()), chain)?),
            #[cfg(feature = "finality-depth")]
            FinalityConfig::Depth(ref config) => Box::new(Depth::load(config, &**chain.db(), chain.genesis())?),
        })
    }
}
