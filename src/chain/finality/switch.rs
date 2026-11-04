//! Finality whose kind governance may switch, FFG to authority or back, from an epoch on
//! (Change::SwitchFinality). The epochs stay genesis's, so only who votes changes: each epoch's
//! voters are fixed activation_delay ahead by the kind that epoch runs, and the justified and
//! finalized checkpoints carry over, the new electorate's first links starting from the old one's
//! last justified checkpoint.
//!
//! - Into FFG: bonding opens once the switch is decided, under the new config's rules. The
//!   checkpoint that fixes the switch epoch's voters fixes the validators bonded by then; should
//!   there be none, the switch lapses: that checkpoint fixes the signers instead, as if it had never
//!   been decided, and pays the bonds made for it, too late, back.
//! - Into authority: the checkpoint that fixes the switch epoch's voters makes the config's signers
//!   the decided set and fixes them, and every validator is unbonding from the switch epoch, its
//!   bond withdrawable withdrawal_delay epochs later and slashable until then.
//!
//! Every chain whose finality votes runs through Elected, which asks each epoch's kind; one that
//! never switches runs genesis's throughout. An epoch only ever has one kind's voters fixed, which
//! is what lets a light client prove which kind it runs.

#![cfg_attr(not(all(feature = "finality-ffg", feature = "finality-authority")), allow(unused_variables, unused_imports, dead_code))]
// A node built without any kind has an empty FinalityConfig, which nothing here can reach.
#![cfg_attr(
    not(any(feature = "finality-ffg", feature = "finality-authority", feature = "finality-depth")),
    allow(unreachable_code)
)]

#[cfg(any(feature = "finality-ffg", feature = "finality-authority"))]
use super::votes::{Electorate, VoteParams, Voters};
use {
    super::FinalityConfig,
    crate::chain::{SignerRules, StakingRules},
    crate::db::{Column, DbError, State},
    crate::registry::changes::Changes,
    crate::registry::signers::Signers,
    crate::registry::validators::{Leak, Validators},
    crate::structs::hash::Hash,
    std::sync::Arc,
};

/// The voters of whichever kind each epoch runs.
#[cfg(any(feature = "finality-ffg", feature = "finality-authority"))]
pub(super) struct Elected {
    genesis: FinalityConfig,
}

#[cfg(any(feature = "finality-ffg", feature = "finality-authority"))]
impl Elected {
    pub(super) fn new(genesis: FinalityConfig) -> Elected {
        Elected { genesis }
    }
}

#[cfg(any(feature = "finality-ffg", feature = "finality-authority"))]
impl Electorate for Elected {
    fn name(&self) -> &'static str {
        self.genesis.name()
    }

    fn params(&self) -> VoteParams {
        self.genesis.vote_params().expect("only a kind that votes has an electorate")
    }

    fn voters(&self, state: &State, epoch: u64) -> Result<Option<Voters>, DbError> {
        match Changes::new(state).finality_in(epoch, &self.genesis)? {
            #[cfg(feature = "finality-ffg")]
            FinalityConfig::Ffg(_) => super::ffg::voters(state, epoch),
            #[cfg(feature = "finality-authority")]
            FinalityConfig::Authority(_) => super::authority::voters(state, epoch),
            #[allow(unreachable_patterns)]
            _ => Ok(None),
        }
    }

    fn fixed_ahead(&self) -> u64 {
        self.genesis.activation_delay()
    }

    fn leak(&self) -> Option<Leak> {
        self.genesis.leak()
    }

    /// Under FFG, long enough to cover a bond's slashable window as the epoch's settings have it:
    /// withdrawal_delay, with vote_depth to spare. Signers have no bonds, so they keep the least.
    fn history(&self, state: &State, epoch: u64) -> Result<u64, DbError> {
        match Changes::new(state).finality_in(epoch, &self.genesis)? {
            #[cfg(feature = "finality-ffg")]
            FinalityConfig::Ffg(config) => Ok(super::votes::history_for(config.withdrawal_delay, config.vote_depth)),
            #[allow(unreachable_patterns)]
            _ => Ok(super::votes::MAX_HISTORY as u64),
        }
    }

    /// Leaks stalled bonds under FFG, then fixes the voters activation_delay ahead by the kind
    /// that epoch runs, switching there when the epoch before it ran the other.
    fn at_checkpoint(&self, state: &Arc<State>, epoch: u64) -> Result<(), DbError> {
        let changes = Changes::new(state);

        #[cfg(feature = "finality-ffg")]
        #[allow(irrefutable_let_patterns)]
        if let FinalityConfig::Ffg(config) = changes.finality_in(epoch, &self.genesis)? {
            super::ffg::leak_at(state, &config, epoch)?;
        }

        let target = epoch + self.fixed_ahead();
        let before = changes.finality_in(target.saturating_sub(1), &self.genesis)?;

        match (before, changes.finality_in(target, &self.genesis)?) {
            #[cfg(all(feature = "finality-ffg", feature = "finality-authority"))]
            (FinalityConfig::Authority(_), FinalityConfig::Ffg(_)) => into_ffg(state, target),
            #[cfg(all(feature = "finality-ffg", feature = "finality-authority"))]
            (FinalityConfig::Ffg(old), FinalityConfig::Authority(new)) => {
                let signers = Signers::new(state);
                signers.start(&new.genesis_set())?;
                signers.fix_set(target)?;

                Validators::new(state).unbond_all(target, old.withdrawal_delay)
            }
            #[cfg(feature = "finality-ffg")]
            (_, FinalityConfig::Ffg(_)) => Validators::new(state).fix_set(target).map(|_| ()),
            #[cfg(feature = "finality-authority")]
            (_, FinalityConfig::Authority(_)) => Signers::new(state).fix_set(target),
            #[allow(unreachable_patterns)]
            _ => Ok(()),
        }
    }
}

/// Fixes the first FFG epoch's validators, those bonded by now; with none, the switch lapses, the
/// signers carry on, and those who bonded for it, too late, get their bonds back.
#[cfg(all(feature = "finality-ffg", feature = "finality-authority"))]
fn into_ffg(state: &State, target: u64) -> Result<(), DbError> {
    let validators = Validators::new(state);

    if !validators.fix_set(target)?.members.is_empty() {
        return Ok(());
    }

    state.delete(Column::Epochs, Validators::set_key(target));
    Changes::new(state).lapse_finality(target)?;
    validators.refund_bonded()?;

    Signers::new(state).fix_set(target)
}

/// Why a switch to config, from the epoch named, cannot be approved in an epoch: a kind that does
/// not vote or the one decided already, a change of finality or consensus still to take effect, a
/// consensus the new kind does not fit, other epochs than genesis's, validators named for FFG, too
/// soon, or settings no chain could finalize with.
pub(crate) fn switch_problem(
    state: &State,
    genesis: &FinalityConfig,
    epoch: u64,
    named: u64,
    config: &FinalityConfig,
) -> Result<Option<&'static str>, DbError> {
    let changes = Changes::new(state);
    let decided = changes.finality_decided(genesis)?;
    let delay = genesis.activation_delay();

    let problem = if !config.votes() || !decided.votes() {
        Some("a switch goes between FFG and authority finality")
    } else if config.name() == decided.name() {
        Some("a switch goes to the other kind of finality; a change of finality retunes this one")
    } else if changes.finality_eras()?.eras.last().is_some_and(|era| era.from > epoch) {
        Some("a change of finality decided earlier has yet to take effect")
    } else if !consensus_settled(&changes, epoch, delay)? {
        Some("a change of consensus decided earlier has yet to be fixed")
    } else if config.checkpoint_interval() != genesis.checkpoint_interval() || config.activation_delay() != delay {
        Some("a switch keeps genesis's checkpoint_interval and activation_delay")
    } else if named < epoch + delay + 1 {
        Some("a switch runs from activation_delay + 1 epochs after the one it is approved in, at the earliest")
    } else {
        crate::chain::consensus_problem(&changes.decided()?.consensus, Some(config)).or_else(|| named_validators(config)).or_else(|| config.problem())
    };

    Ok(problem)
}

/// A switch into FFG names no validators: they bond for it, from their own balances.
fn named_validators(config: &FinalityConfig) -> Option<&'static str> {
    match config {
        #[cfg(feature = "finality-ffg")]
        FinalityConfig::Ffg(ffg) if !ffg.validators.is_empty() => Some("a switch into FFG names no validators: they bond once it is decided"),
        _ => None,
    }
}

/// Whether the consensus decided last is the one fixed for the furthest epoch fixed so far: no
/// change of consensus is on its way.
fn consensus_settled(changes: &Changes, epoch: u64, delay: u64) -> Result<bool, DbError> {
    let fixed = match changes.consensus_for(epoch + delay)? {
        Some(fixed) => Some(fixed),
        None => changes.consensus_for((epoch + delay).saturating_sub(1))?,
    };

    let decided = changes.decided()?.consensus;

    Ok(fixed.is_none_or(|fixed| fixed == decided))
}

/// Whether a change of consensus or finality decided earlier may still be overtaken: a switch of
/// finality's kind that has yet to take effect, which every other change of either waits for.
pub(crate) fn switching(state: &State, genesis: &FinalityConfig, epoch: u64) -> Result<bool, DbError> {
    Ok(Changes::new(state).switch_ahead(epoch, genesis)?.is_some())
}

/// The staking and signer rules a block in an epoch runs under: those of the kind the epoch runs;
/// under authority, staking too while validators may still bond for a switch into FFG, or withdraw
/// and be slashed after one out of it, with bonding closed.
pub(crate) fn rules_at(
    state: &State,
    genesis: &FinalityConfig,
    epoch: u64,
    chain: Hash,
) -> Result<(Option<StakingRules>, Option<SignerRules>), DbError> {
    let changes = Changes::new(state);
    let running = changes.finality_in(epoch, genesis)?;

    if let Some(staking) = running.staking(chain) {
        return Ok((Some(staking), running.signer_rules(chain)));
    }

    let staking = match changes.switch_ahead(epoch, genesis)? {
        Some(ahead) => ahead.config.staking(chain),
        None => last_ffg(&changes, genesis, epoch)?.and_then(|config| config.staking(chain)).map(|rules| StakingRules { bonding: false, ..rules }),
    };

    Ok((staking, running.signer_rules(chain)))
}

/// The FFG config run last by an epoch, if the chain ever ran FFG.
fn last_ffg(changes: &Changes, genesis: &FinalityConfig, epoch: u64) -> Result<Option<FinalityConfig>, DbError> {
    let eras = changes.finality_eras()?.eras;
    let ran = eras.into_iter().rev().filter(|era| era.from <= epoch).map(|era| era.config);

    Ok(ran.chain(std::iter::once(genesis.clone())).find(|config| config.name() == "ffg"))
}
