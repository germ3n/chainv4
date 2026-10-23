//! Which finality a chain runs, as its genesis names it.

// Only FFG keeps anything in chain state, so without it some parameters go unused. A build without
// any kind has an empty FinalityConfig, whose methods all match on nothing.
#![cfg_attr(not(feature = "finality-ffg"), allow(unused_variables))]
#![cfg_attr(
    not(any(feature = "finality-ffg", feature = "finality-authority", feature = "finality-depth")),
    allow(unreachable_code)
)]

#[cfg(feature = "finality-authority")]
use super::authority::{self, AuthorityConfig};
#[cfg(feature = "finality-authority")]
use crate::registry::signers::Signers;
#[cfg(feature = "finality-depth")]
use super::depth::{Depth, DepthConfig};
#[cfg(feature = "finality-ffg")]
use super::ffg::{self, FfgConfig};
#[cfg(any(feature = "finality-ffg", feature = "finality-authority"))]
use super::votes::Votes;
use {
    super::Finality,
    crate::chain::{SignerRules, StakingRules},
    crate::crypto::hashable::Hashable,
    crate::crypto::keypair::PublicKey,
    crate::db::{DbError, Read, State},
    crate::registry::validators::ValidatorSet,
    crate::structs::hash::Hash,
    r#macro::document,
    wincode::{SchemaRead, SchemaWrite},
};

#[document(
    summary = "Which finality a chain runs, and every parameter of it. Part of genesis, so every node agrees on it; nothing about finality is fixed in code.",
    category = "Finality",
    see_also = "Genesis, Finality, FfgConfig, AuthorityConfig, DepthConfig",
    note = "Each kind sits behind its own cargo feature, finality-ffg, finality-authority and finality-depth, all on by default. Tags are pinned, so encodings match across builds.",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
#[wincode(tag_encoding = "u8")]
pub enum FinalityConfig {
    #[document(summary = "Casper FFG-style: validators bond on chain and vote on checkpoints. Two thirds by bond justify, a justified direct child finalizes its parent, and cheaters lose their bond.")]
    #[cfg(feature = "finality-ffg")]
    #[wincode(tag = 0)]
    Ffg(FfgConfig),
    #[document(summary = "The same votes, from signers genesis names, a threshold of them justifying, who change by their own approval. No bonds, and nobody can be slashed.")]
    #[cfg(feature = "finality-authority")]
    #[wincode(tag = 1)]
    Authority(AuthorityConfig),
    #[document(summary = "No votes: a block is final once enough blocks are on top of it on the main chain.")]
    #[cfg(feature = "finality-depth")]
    #[wincode(tag = 2)]
    Depth(DepthConfig),
}

impl Hashable for FinalityConfig {
    const TAG_LEN: usize = 1;
}

impl FinalityConfig {
    #[document(parent = "FinalityConfig", summary = "The names of the kinds this build has, as genesis files spell them.")]
    pub const KINDS: &'static [&'static str] = &[
        #[cfg(feature = "finality-ffg")]
        "ffg",
        #[cfg(feature = "finality-authority")]
        "authority",
        #[cfg(feature = "finality-depth")]
        "depth",
    ];

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

    #[document(parent = "FinalityConfig", summary = "The kind's name: ffg, authority or depth.")]
    pub fn name(&self) -> &'static str {
        match *self {
            #[cfg(feature = "finality-ffg")]
            FinalityConfig::Ffg(_) => "ffg",
            #[cfg(feature = "finality-authority")]
            FinalityConfig::Authority(_) => "authority",
            #[cfg(feature = "finality-depth")]
            FinalityConfig::Depth(_) => "depth",
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
    pub fn signer_rules(&self) -> Option<SignerRules> {
        match *self {
            #[cfg(feature = "finality-ffg")]
            FinalityConfig::Ffg(_) => None,
            #[cfg(feature = "finality-authority")]
            FinalityConfig::Authority(ref config) => Some(config.signer_rules()),
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
        summary = "The finality these settings describe, with what this node learned before.",
        params = {
            db = "The chain's database.",
            genesis = "The genesis block's id.",
        },
    )]
    pub fn build(&self, db: &dyn Read, genesis: Hash) -> Result<Box<dyn Finality>, DbError> {
        Ok(match *self {
            #[cfg(feature = "finality-ffg")]
            FinalityConfig::Ffg(ref config) => Box::new(Votes::load(ffg::Staked::new(config.clone()), db, genesis)?),
            #[cfg(feature = "finality-authority")]
            FinalityConfig::Authority(ref config) => Box::new(Votes::load(authority::Appointed::new(config), db, genesis)?),
            #[cfg(feature = "finality-depth")]
            FinalityConfig::Depth(ref config) => Box::new(Depth::load(config, db, genesis)?),
        })
    }
}
