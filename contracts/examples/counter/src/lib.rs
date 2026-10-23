//! Counts. The smallest useful contract: storage, arguments and return values.

#![no_std]

use chainv4_sdk::{Value, abort, export, input, output, sender, storage};

const COUNT: &[u8] = b"count";
const LAST: &[u8] = b"last";

/// Adds a whole number, 1 when none is given, and returns the new count.
#[export]
fn increment() {
    let by = match input().first() {
        Some(value) => value.as_i64().unwrap_or_else(|| abort("increment takes a whole number")),
        None => 1,
    };
    let count = storage::get_i64(COUNT)
        .unwrap_or(0)
        .checked_add(by)
        .unwrap_or_else(|| abort("the count would overflow"));
    storage::set(COUNT, &count.to_le_bytes());
    storage::set(LAST, &sender().encode());
    output(&[Value::I64(count)]);
}

/// The count.
#[export]
fn get() {
    output(&[Value::I64(storage::get_i64(COUNT).unwrap_or(0))]);
}

/// Who incremented last, if anyone.
#[export]
fn last() {
    let last = storage::get(LAST).and_then(|bytes| chainv4_sdk::Address::decode(bytes.as_slice().try_into().ok()?));

    match last {
        Some(address) => output(&[Value::Address(address)]),
        None => output(&[]),
    }
}
