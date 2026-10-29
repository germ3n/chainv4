//! The coin's supply (docs/api.md#get-supply): every coin ever made, at genesis, as block rewards
//! and by the minter, and every coin destroyed, by fees' burned share, Tx::Burn, names' prices and
//! slashing. Two counters in state, so a light client proves the supply as it proves a balance.

use {
    crate::crypto::hashable::Hashable,
    crate::db::{Column, DbError, State},
    crate::structs::u256::{U256Le, u256},
    r#macro::document,
    wincode::{SchemaRead, SchemaWrite},
};

#[document(
    summary = "Where the coin's supply is kept, in the Validators column: one entry, so a light client proves it with GET /proofs/validators/{key}.",
    category = "Chain",
    see_also = "Supply",
)]
pub const SUPPLY_KEY: &[u8] = b"coin:supply";

#[document(
    summary = "Every coin made and every coin destroyed since genesis. What there is, held, bonded or in the reward pool, is minted less burned.",
    category = "Chain",
    see_also = "SUPPLY_KEY",
)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct Supply {
    #[document(summary = "Made: genesis's allocations and bonds, block rewards, and what the minter minted.")]
    #[wincode(with = "U256Le")]
    pub minted: u256,
    #[document(summary = "Destroyed: fees' burned share, Tx::Burn, names' prices and what slashing took.")]
    #[wincode(with = "U256Le")]
    pub burned: u256,
}

impl Hashable for Supply {}

impl Supply {
    #[document(parent = "Supply", summary = "What there is now: minted less burned.")]
    pub fn supply(&self) -> u256 {
        self.minted.saturating_sub(self.burned)
    }

    #[document(parent = "Supply", summary = "The supply in a state: none made yet on a chain from before it was kept.")]
    pub fn of(state: &State) -> Result<Supply, DbError> {
        Ok(state.get_value(Column::Validators, SUPPLY_KEY)?.unwrap_or_default())
    }

    #[document(parent = "Supply", summary = "Counts coin made, in a state.")]
    pub fn mint(state: &State, amount: u256) -> Result<(), DbError> {
        Supply::change(state, amount, |supply| supply.minted = supply.minted.saturating_add(amount))
    }

    #[document(parent = "Supply", summary = "Counts coin destroyed, in a state.")]
    pub fn burn(state: &State, amount: u256) -> Result<(), DbError> {
        Supply::change(state, amount, |supply| supply.burned = supply.burned.saturating_add(amount))
    }

    /// Nothing is written for nothing made or destroyed.
    fn change(state: &State, amount: u256, change: impl FnOnce(&mut Supply)) -> Result<(), DbError> {
        if amount == u256::ZERO {
            return Ok(());
        }

        let mut supply = Supply::of(state)?;
        change(&mut supply);
        state.put_value(Column::Validators, SUPPLY_KEY.to_vec(), &supply);

        Ok(())
    }
}

#[document(
    parent = "Supply",
    summary = "Whether a state's supply adds up: minted less burned is every balance, the reward pool's and contracts' among them, and every bond. What the tests that make and destroy coin check after.",
    returns = "What the counters say and what the state holds, when they differ.",
)]
#[cfg(any(test, feature = "node"))]
pub fn adds_up(state: &State) -> Result<Result<(), (u256, u256)>, DbError> {
    use crate::db::Committed;

    let mut held = u256::ZERO;
    state.scan(Column::Accounts, &[], &[], &mut |key, value| {
        if let Some((_, account)) = crate::registry::accounts::account_entry(key, value) {
            held = held.saturating_add(account.balance);
        }

        true
    })?;

    #[cfg(feature = "staking")]
    {
        let validators = crate::registry::validators::Validators::new(state);

        for voter in validators.voters()? {
            if let Some(record) = validators.get(&voter).map_err(|err| DbError::Corrupt(err.to_string()))? {
                held = held.saturating_add(record.bond);
            }
        }
    }

    let counted = Supply::of(state)?.supply();

    Ok(if counted == held { Ok(()) } else { Err((counted, held)) })
}
