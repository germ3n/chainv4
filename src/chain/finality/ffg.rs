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
    #[document(summary = "The inactivity leak, or None for none: once no checkpoint has been proven in a block for after_epochs epochs, each checkpoint takes permille thousandths of the bond of every validator that has not voted since, until those that do vote hold two thirds again. Added after chains launched: None adds nothing to genesis's encoding, so chains from before keep their ids.")]
    #[wincode(with = "UnsetLeak")]
    pub leak: Option<Leak>,
}

impl Hashable for FfgConfig {}

pub use crate::registry::validators::Leak;

/// Encodes FfgConfig::leak as nothing while None, and as a 1 and the leak once set, so a genesis
/// from before the field hashes as it did. Only as a last field; see Unset.
pub struct UnsetLeak;

// SAFETY: size_of is exactly what write writes: nothing for None, seventeen bytes for Some.
unsafe impl<C: wincode::config::ConfigCore> SchemaWrite<C> for UnsetLeak {
    type Src = Option<Leak>;

    fn size_of(src: &Option<Leak>) -> wincode::WriteResult<usize> {
        Ok(if src.is_some() { 17 } else { 0 })
    }

    fn write(writer: impl wincode::io::Writer, src: &Option<Leak>) -> wincode::WriteResult<()> {
        let Some(leak) = src else {
            return Ok(());
        };
        let mut bytes = [1; 17];
        bytes[1..9].copy_from_slice(&leak.after_epochs.to_le_bytes());
        bytes[9..].copy_from_slice(&leak.permille.to_le_bytes());

        <[u8; 17] as SchemaWrite<C>>::write(writer, &bytes)
    }
}

// SAFETY: read initializes dst before it returns Ok. An exhausted reader is None, which is how an
// encoding from before the field reads; so this only ever ends a message.
unsafe impl<'de, C: wincode::config::ConfigCore> SchemaRead<'de, C> for UnsetLeak {
    type Dst = Option<Leak>;

    fn read(mut reader: impl wincode::io::Reader<'de>, dst: &mut std::mem::MaybeUninit<Option<Leak>>) -> wincode::ReadResult<()> {
        let value = match reader.take_byte() {
            Ok(1) => {
                let rest = reader.take_array::<16>()?;

                Some(Leak {
                    after_epochs: u64::from_le_bytes(rest[..8].try_into().expect("eight bytes")),
                    permille: u64::from_le_bytes(rest[8..].try_into().expect("eight bytes")),
                })
            }
            _ => None,
        };
        dst.write(value);

        Ok(())
    }
}

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
            leak: None,
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

        if self.leak.is_some_and(|leak| leak.after_epochs == 0) {
            return Some("leak.after_epochs must be at least 1");
        }

        if self.leak.is_some_and(|leak| leak.permille > 1000) {
            return Some("leak.permille must be at most 1000");
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

    fn leak(&self) -> Option<Leak> {
        self.config.leak
    }

    /// Each checkpoint leaks the bonds of validators that stopped voting while the chain is
    /// stalled, then fixes the validator set activation_delay epochs ahead.
    fn at_checkpoint(&self, state: &Arc<State>, epoch: u64) -> Result<(), DbError> {
        let validators = Validators::new(state);

        if let Some(leak) = self.config.leak
            && epoch.saturating_sub(validators.proven()?) > leak.after_epochs
        {
            validators.leak(epoch, &leak, &self.config.staking())?;
        }

        validators.fix_set(epoch + self.config.activation_delay).map(|_| ())
    }
}
