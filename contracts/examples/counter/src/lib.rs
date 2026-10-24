//! Counts. The smallest useful contract: storage, arguments and return values.

#![no_std]

use chainv4_sdk::{Address, Value, abort, emit, export, sender, storage};

const COUNT: &[u8] = b"count";
const LAST: &[u8] = b"last";

/// Adds a whole number, 1 when none is given, and returns the new count.
#[export]
fn increment(by: Option<i64>) -> i64 {
    let count = storage::get_i64(COUNT)
        .unwrap_or(0)
        .checked_add(by.unwrap_or(1))
        .unwrap_or_else(|| abort("the count would overflow"));
    storage::set(COUNT, &count.to_le_bytes());
    storage::set(LAST, &sender().encode());
    emit("Incremented", &[Value::I64(count), Value::Address(sender())]);

    count
}

/// The count.
#[export]
fn get() -> i64 {
    storage::get_i64(COUNT).unwrap_or(0)
}

/// Who incremented last, or nil before anyone did.
#[export]
fn last() -> Option<Address> {
    storage::get(LAST).and_then(|bytes| Address::decode(bytes.as_slice().try_into().ok()?))
}
