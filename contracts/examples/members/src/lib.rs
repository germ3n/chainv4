//! A club's members, kept in a namespace its layout declares, counted and listed by iterating it:
//! storage in key order, a page at a time.

#![no_std]

extern crate alloc;

use {
    alloc::vec::Vec,
    chainv4_sdk::{Address, Value, abort, block_time, export, namespace, sender, storage},
};

// Each member by address, with when they joined.
namespace!(MEMBERS, "members", "address" => "u64");

/// Joins the club, once.
#[export]
fn join() {
    let me = sender().encode();

    if MEMBERS.get_u64(&me).is_some() {
        abort("a member already");
    }

    MEMBERS.set_u64(&me, block_time());
}

/// Leaves it.
#[export]
fn leave() {
    MEMBERS.delete(&sender().encode());
}

/// How many members there are: every key of the namespace, a page at a time.
#[export]
fn count() -> u64 {
    MEMBERS.iter(&[]).count() as u64
}

/// A page of members, each an address, in the order of their bytes: at most limit, from the
/// member after the one given, or the first. The last one given goes in after to go on.
#[export]
fn list(after: Option<Address>, limit: u64) -> Vec<Value> {
    let start = after.map_or_else(Vec::new, |after| [&after.encode()[..], &[0]].concat());
    let limit = u32::try_from(limit).unwrap_or(storage::MAX_SCAN).clamp(1, storage::MAX_SCAN);

    MEMBERS
        .scan(&[], &start, limit)
        .into_iter()
        .filter_map(|(key, _)| Address::decode(&key.try_into().ok()?))
        .map(Value::Address)
        .collect()
}
