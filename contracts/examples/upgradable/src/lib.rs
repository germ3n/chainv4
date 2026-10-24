//! A proxy that is upgradable without the built-in upgrade, the Rust twin of
//! contracts/lua/proxy.lua. Every function it does not export itself reaches fallback, which runs
//! its implementation's code on the proxy's own storage, balance and events. Upgrading is
//! pointing it at a new implementation.

#![no_std]

extern crate alloc;

use chainv4_sdk::{Address, Value, abort, check_layout, delegate, emit, export, input, output, sender, storage};

// Kept in the proxy's own space, apart from every key an implementation uses. Implementations
// may read them, but only the proxy's own code can change them.
const OWNER: &[u8] = b"owner";
const IMPLEMENTATION: &[u8] = b"implementation";

fn stored(key: &[u8]) -> Option<Address> {
    Address::decode(storage::own().get(key)?.as_slice().try_into().ok()?)
}

fn implementation_or_abort() -> Address {
    stored(IMPLEMENTATION).unwrap_or_else(|| abort("no implementation set"))
}

/// Run once by the chain when the proxy is deployed.
#[export]
fn init() {
    storage::own().set(OWNER, &sender().encode());
}

/// Points the proxy at new code. Storage stays, so the new code must keep the old code's storage
/// layout, as an upgrade's would.
#[export]
fn set_implementation() {
    if stored(OWNER) != Some(sender()) {
        abort("only the owner");
    }

    let implementation = input()
        .first()
        .and_then(Value::as_address)
        .unwrap_or_else(|| abort("set_implementation takes an address"));

    if let Some(current) = stored(IMPLEMENTATION) {
        check_layout(&current, &implementation).unwrap_or_else(|err| abort(&err));
    }

    storage::own().set(IMPLEMENTATION, &implementation.encode());
    emit("Upgraded", &[Value::Address(implementation)]);
}

#[export]
fn implementation() {
    output(&[Value::Address(implementation_or_abort())]);
}

/// Run by the chain for every function the proxy does not export: the selector comes first.
#[export]
fn fallback() {
    let mut args = input();
    let selector = match args.first() {
        Some(Value::U64(selector)) => *selector,
        _ => abort("fallback takes a selector"),
    };
    let rest = args.split_off(1);

    match delegate(&implementation_or_abort(), selector, &rest) {
        Ok(returns) => output(&returns),
        Err(err) => abort(&err),
    }
}
