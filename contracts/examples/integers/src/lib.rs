//! Integer math past what a u64 holds, unsigned and signed: U256 and I256 take the operators, with
//! a plain u64 or i64 on either side, and abort the call on overflow; their checked_, saturating_
//! and wrapping_ methods are for when that is not what you want. contracts/lua/integers.lua is the
//! same contract in Lua, with the same layout, so either engine can take over the other's storage.

#![no_std]

extern crate alloc;

use {
    alloc::{string::String, vec, vec::Vec},
    chainv4_sdk::{Address, I256, IntoValue, U256, Value, export, namespace, sender},
};

namespace!(LEDGER, "ledger", { "owner": "address", "balance": "i256", "small": "i128", "big": "u128", "supply": "u256" });

#[export]
fn init() {
    LEDGER.set_address(b"owner", &sender());
}

#[export]
fn check_perms(permission: String, who: Address, _detail: Value) -> bool {
    permission == "upgrade" && LEDGER.get_address(b"owner") == Some(who)
}

/// A fee in basis points, rounding down: amount * bps / 10000.
#[export]
fn fee(amount: U256, bps: u64) -> U256 {
    amount * bps / 10_000
}

/// Adds a change, above zero or below it, to the balance, and returns the balance.
#[export]
fn settle(change: I256) -> I256 {
    let balance = LEDGER.get_i256(b"balance").unwrap_or(I256::ZERO) + change;
    LEDGER.set_i256(b"balance", balance);

    balance
}

/// Adds to the supply, failing with a message of its own rather than "u256 overflow".
#[export]
fn mint(amount: U256) -> Result<U256, &'static str> {
    let supply = LEDGER.get_u256(b"supply").unwrap_or(U256::ZERO).checked_add(amount).ok_or("the supply would overflow")?;
    LEDGER.set_u256(b"supply", supply);

    Ok(supply)
}

/// Keeps an i128 and a u128, and reads them back.
#[export]
fn keep(small: i128, big: u128) -> (i128, u128) {
    LEDGER.set_i128(b"small", small);
    LEDGER.set_u128(b"big", big);

    (LEDGER.get_i128(b"small").unwrap_or(0), LEDGER.get_u128(b"big").unwrap_or(0))
}

/// What everything kept holds: the balance, the supply and the two small ones.
#[export]
fn held() -> (I256, U256, i128, u128) {
    (
        LEDGER.get_i256(b"balance").unwrap_or(I256::ZERO),
        LEDGER.get_u256(b"supply").unwrap_or(U256::ZERO),
        LEDGER.get_i128(b"small").unwrap_or(0),
        LEDGER.get_u128(b"big").unwrap_or(0),
    )
}

/// The edges: checked gives nothing, saturating stops, wrapping wraps, and / and % round toward
/// zero, as Rust's do.
#[export]
fn edges() -> Vec<Value> {
    vec![
        U256::MAX.checked_add(U256::ONE).into_value(),
        U256::ZERO.saturating_sub(U256::ONE).into(),
        U256::MAX.wrapping_add(U256::ONE).into(),
        I256::MIN.checked_neg().into_value(),
        I256::MAX.wrapping_add(I256::ONE).into(),
        (I256::from(-7i64) / 2).into(),
        (I256::from(-7i64) % 2).into(),
        U256::from(10u64).pow(30).into(),
    ]
}

/// Comparisons with plain integers, either way round.
#[export]
fn compare(amount: U256, change: I256) -> (bool, bool, bool, bool) {
    (amount > 1000, 1000 < amount, change < 0, amount == 5)
}

/// The operators never wrap: this aborts with "u256 overflow".
#[export]
fn overflow() -> U256 {
    U256::MAX + 1
}
