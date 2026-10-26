//! Governance by finality's own electorate: whoever finalizes the chain changes it. Under FFG an
//! account weighs the stake of the validators it bonded, as the epoch's set fixes them, and two
//! thirds of the set decide, as a justification takes; under authority finality an account weighs
//! the decided signers it speaks for, and change_threshold of them decide, as a signer change
//! takes. Depth finality has no electorate, so genesis refuses the pair.

use {
    super::Governance,
    crate::chain::FinalityConfig,
    crate::db::{DbError, State},
    crate::structs::address::Address,
    crate::structs::u256::u256,
};

/// The electorate kind: what the chain's finality says.
pub(super) struct Electorate {
    finality: FinalityConfig,
}

impl Electorate {
    pub(super) fn new(finality: FinalityConfig) -> Electorate {
        Electorate { finality }
    }
}

impl Governance for Electorate {
    fn name(&self) -> &'static str {
        "electorate"
    }

    #[cfg_attr(not(any(feature = "finality-ffg", feature = "finality-authority")), allow(unused_variables))]
    fn weight(&self, state: &State, epoch: u64, account: &Address) -> Result<Option<u256>, DbError> {
        let weight = match self.finality {
            #[cfg(feature = "finality-ffg")]
            FinalityConfig::Ffg(_) => crate::registry::validators::Validators::new(state)
                .set(epoch)?
                .map(|set| set.members.iter().filter(|member| member.account == *account).fold(u256::ZERO, |sum, member| sum.saturating_add(member.weight)))
                .unwrap_or(u256::ZERO),
            #[cfg(feature = "finality-authority")]
            FinalityConfig::Authority(_) => {
                let decided = crate::registry::signers::Signers::new(state).decided()?;

                u256::from(decided.set.signers.iter().filter(|signer| signer.account == *account).count() as u64)
            }
            #[allow(unreachable_patterns)]
            _ => u256::ZERO,
        };

        Ok((weight > u256::ZERO).then_some(weight))
    }

    #[cfg_attr(not(any(feature = "finality-ffg", feature = "finality-authority")), allow(unused_variables))]
    fn needed(&self, state: &State, epoch: u64) -> Result<u256, DbError> {
        Ok(match self.finality {
            #[cfg(feature = "finality-ffg")]
            FinalityConfig::Ffg(_) => crate::registry::validators::Validators::new(state)
                .set(epoch)?
                .map_or(u256::MAX, |set| set.two_thirds()),
            #[cfg(feature = "finality-authority")]
            FinalityConfig::Authority(_) => u256::from(crate::registry::signers::Signers::new(state).decided()?.set.change_threshold),
            #[allow(unreachable_patterns)]
            _ => u256::MAX,
        })
    }
}
