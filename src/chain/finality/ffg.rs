//! Casper FFG-style finality. Validators bond on chain, with a voting key of their own, and vote on
//! checkpoints; two thirds of the bonded weight voting for one link justifies its target. Validator
//! sets are chain state, fixed activation_delay epochs ahead at each checkpoint, so every branch
//! that split off after that checkpoint counts votes with the same weights. Conflicting votes cost
//! the whole bond, part of it paid to whoever reports them.

use {
    super::votes::{Electorate, VoteParams, Voters},
    crate::crypto::hashable::Hashable,
    crate::crypto::keypair::PublicKey,
    crate::db::{DbError, State},
    crate::registry::validators::{Staking, ValidatorRecord, Validators},
    crate::structs::address::Address,
    crate::structs::u256::{U256Le, u256},
    r#macro::document,
    std::sync::Arc,
    wincode::{SchemaRead, SchemaWrite},
};

#[document(
    summary = "A validator bonded from genesis.",
    category = "Finality",
    see_also = "FfgConfig",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct GenesisValidator {
    #[document(summary = "The account that owns the bond.")]
    pub account: Address,
    #[document(summary = "The key that signs its votes.")]
    pub voter: PublicKey,
    #[document(summary = "Its bond, minted at genesis.")]
    #[wincode(with = "U256Le")]
    pub bond: u256,
}

impl Hashable for GenesisValidator {}

#[document(
    summary = "Every parameter of FFG finality, set in genesis.",
    category = "Finality",
    see_also = "FinalityConfig, Staking, Vote",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct FfgConfig {
    #[document(summary = "Blocks per epoch. Each epoch's first block is its checkpoint.")]
    pub checkpoint_interval: u64,
    #[document(summary = "How many blocks on top of a checkpoint before validators vote for it, so they rarely vote for a block that is about to lose a fork.")]
    pub vote_depth: u64,
    #[document(summary = "The smallest bond.")]
    #[wincode(with = "U256Le")]
    pub min_bond: u256,
    #[document(summary = "Epochs before a bond, unbond or slash changes who votes. At least 1; larger keeps validator sets agreeing across deeper forks.")]
    pub activation_delay: u64,
    #[document(summary = "Epochs a bond stays locked, and slashable, after its validator stops voting.")]
    pub withdrawal_delay: u64,
    #[document(summary = "A reporter of cheating gets the bond divided by this; the rest is burned.")]
    pub reporter_divisor: u64,
    #[document(summary = "The most validators registered at once.")]
    pub max_validators: u32,
    #[document(summary = "What every voter in a justification gets when a block carries it. With none, blocks carry no proofs.")]
    #[wincode(with = "U256Le")]
    pub vote_reward: u256,
    #[document(summary = "The most justifications one block may carry.")]
    pub max_proofs_per_block: u32,
    #[document(summary = "Validators bonded from the start. With none, nothing finalizes until someone bonds.")]
    pub validators: Vec<GenesisValidator>,
}

impl Hashable for FfgConfig {}

/// Coins at 18 decimals.
fn coins(count: u64) -> u256 {
    u256::from(count) * u256::from(10u64).pow(18)
}

impl FfgConfig {
    #[document(
        parent = "FfgConfig",
        summary = "The development settings: an epoch every 8 blocks, votes 2 blocks deep, changes after 2 epochs, bonds locked 64 epochs after leaving, a 100-coin minimum bond, 1 coin per vote paid, a sixteenth of a slashed bond to its reporter, and one validator from genesis.",
        params = { validator = "The genesis validator's key, which both owns the bond and votes." },
    )]
    pub fn dev(validator: PublicKey) -> FfgConfig {
        FfgConfig {
            checkpoint_interval: 8,
            vote_depth: 2,
            min_bond: coins(100),
            activation_delay: 2,
            withdrawal_delay: 64,
            reporter_divisor: 16,
            max_validators: 1_000,
            vote_reward: coins(1),
            max_proofs_per_block: 4,
            validators: vec![GenesisValidator {
                account: validator.address(),
                voter: validator,
                bond: coins(10_000),
            }],
        }
    }

    #[document(parent = "FfgConfig", summary = "The staking rules these settings make.")]
    pub fn staking(&self) -> Staking {
        Staking {
            min_bond: self.min_bond,
            activation_delay: self.activation_delay,
            withdrawal_delay: self.withdrawal_delay,
            reporter_divisor: self.reporter_divisor,
            max_validators: self.max_validators,
        }
    }

    /// Settings no chain could finalize with.
    pub(super) fn problem(&self) -> Option<&'static str> {
        if self.checkpoint_interval == 0 {
            return Some("checkpoint_interval must be at least 1");
        }

        if self.activation_delay == 0 {
            return Some("activation_delay must be at least 1");
        }

        if self.reporter_divisor == 0 {
            return Some("reporter_divisor must be at least 1");
        }

        if self.validators.len() > self.max_validators as usize {
            return Some("more genesis validators than max_validators");
        }

        None
    }
}

/// Registers genesis validators and fixes the validator sets genesis already decides.
pub(super) fn write_genesis(state: &State, config: &FfgConfig) -> Result<(), DbError> {
    let validators = Validators::new(state);
    let mut voters = validators.voters()?;

    for validator in &config.validators {
        validators.register(&validator.voter, ValidatorRecord {
            account: validator.account,
            bond: validator.bond,
            active_from: 0,
            active_until: None,
            withdrawable_from: None,
            slashed: false,
        }, &mut voters);
    }

    for epoch in 0..=config.activation_delay {
        validators.fix_set(epoch)?;
    }

    Ok(())
}

/// Bonded validators, weighted by bond, two thirds of the weight justifying.
pub(super) struct Staked {
    config: FfgConfig,
}

impl Staked {
    pub(super) fn new(config: FfgConfig) -> Staked {
        Staked { config }
    }
}

impl Electorate for Staked {
    fn name(&self) -> &'static str {
        "ffg"
    }

    fn params(&self) -> VoteParams {
        VoteParams {
            checkpoint_interval: self.config.checkpoint_interval,
            vote_depth: self.config.vote_depth,
            vote_reward: self.config.vote_reward,
            max_proofs_per_block: self.config.max_proofs_per_block,
        }
    }

    /// Two thirds of the bonded weight: the smallest weight w with 3w >= 2 total, and never
    /// nothing, so an empty set justifies nothing.
    fn voters(&self, state: &State, epoch: u64) -> Result<Option<Voters>, DbError> {
        Ok(Validators::new(state).set(epoch)?.map(|set| Voters {
            needed: set.two_thirds(),
            set,
        }))
    }

    fn fixed_ahead(&self) -> u64 {
        self.config.activation_delay
    }

    /// Each checkpoint fixes the validator set activation_delay epochs ahead.
    fn at_checkpoint(&self, state: &Arc<State>, epoch: u64) -> Result<(), DbError> {
        Validators::new(state).fix_set(epoch + self.config.activation_delay).map(|_| ())
    }
}
