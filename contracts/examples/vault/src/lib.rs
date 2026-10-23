//! Holds funds for whoever sends them, and pays each depositor back on request.

#![no_std]

use chainv4_sdk::{Amount, Value, abort, amount, balance, export, input, output, sender, storage, this, transfer, value};

/// What a depositor has in the vault, kept under their address.
fn held(key: &[u8]) -> u128 {
    storage::get(key)
        .and_then(|bytes| bytes.try_into().ok())
        .map(u128::from_le_bytes)
        .unwrap_or(0)
}

fn small(amount: [u8; 32]) -> u128 {
    if amount[16..].iter().any(|byte| *byte != 0) {
        abort("amounts past u128 are not supported");
    }

    u128::from_le_bytes(amount[..16].try_into().expect("16 bytes"))
}

/// Keeps what the call sent and returns what the sender now holds.
#[export]
fn deposit() {
    let key = sender().encode();
    let total = held(&key)
        .checked_add(small(value()))
        .unwrap_or_else(|| abort("too much"));
    storage::set(&key, &total.to_le_bytes());
    output(&[Value::U128(total)]);
}

/// Sends an amount of the sender's deposit back to them and returns what is left.
#[export]
fn withdraw() {
    let wanted = input()
        .first()
        .and_then(Value::as_amount)
        .map(small)
        .unwrap_or_else(|| abort("withdraw takes an amount"));
    let key = sender().encode();
    let left = held(&key)
        .checked_sub(wanted)
        .unwrap_or_else(|| abort("more than was deposited"));
    storage::set(&key, &left.to_le_bytes());

    if let Err(err) = transfer(&sender(), amount(wanted)) {
        abort(&err);
    }

    output(&[Value::U128(left)]);
}

/// Everything the vault holds.
#[export]
fn total() {
    output(&[Value::Amount(Amount::U256(balance(&this())))]);
}
