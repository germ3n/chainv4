//! Counts. The smallest useful contract: storage, arguments and return values, and a storage
//! layout, which new versions of it must keep.

#![no_std]

use chainv4_sdk::{Address, Value, abort, emit, export, namespace, sender};

// The count, as a little-endian i64, and who counted last. Declaring the namespace puts it in
// the contract's storage layout: new code must declare it too, with the same kind.
namespace!(COUNTER, "counter", "counter-v1");

const COUNT: &[u8] = b"count";
const LAST: &[u8] = b"last";

/// Adds a whole number, 1 when none is given, and returns the new count.
#[export]
fn increment(by: Option<i64>) -> i64 {
    let count = COUNTER
        .get_i64(COUNT)
        .unwrap_or(0)
        .checked_add(by.unwrap_or(1))
        .unwrap_or_else(|| abort("the count would overflow"));
    COUNTER.set(COUNT, &count.to_le_bytes());
    COUNTER.set(LAST, &sender().encode());
    emit("Incremented", &[Value::I64(count), Value::Address(sender())]);

    count
}

/// The count.
#[export]
fn get() -> i64 {
    COUNTER.get_i64(COUNT).unwrap_or(0)
}

/// Who incremented last, or nil before anyone did.
#[export]
fn last() -> Option<Address> {
    COUNTER.get(LAST).and_then(|bytes| Address::decode(bytes.as_slice().try_into().ok()?))
}
