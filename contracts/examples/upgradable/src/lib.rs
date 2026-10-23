//! A proxy that is upgradable without the built-in upgrade, the Rust twin of
//! contracts/lua/proxy.lua. Every function it does not export itself reaches fallback, which runs
//! its implementation's code on the proxy's own storage, balance and events. Upgrading is
//! pointing it at a new implementation.

#![no_std]

extern crate alloc;

use chainv4_sdk::{Address, Value, abort, delegate, emit, export, input, output, sender, storage};

// Prefixed, so they cannot clash with an implementation's own keys.
const OWNER: &[u8] = b"proxy:owner";
const IMPLEMENTATION: &[u8] = b"proxy:implementation";

fn stored(key: &[u8]) -> Option<Address> {
    Address::decode(storage::get(key)?.as_slice().try_into().ok()?)
}

fn implementation_or_abort() -> Address {
    stored(IMPLEMENTATION).unwrap_or_else(|| abort("no implementation set"))
}

/// Run once by the chain when the proxy is deployed.
#[export]
fn init() {
    storage::set(OWNER, &sender().encode());
}

/// Points the proxy at new code. Storage stays, so the new code must read the old layout.
#[export]
fn set_implementation() {
    if stored(OWNER) != Some(sender()) {
        abort("only the owner");
    }

    let implementation = input()
        .first()
        .and_then(Value::as_address)
        .unwrap_or_else(|| abort("set_implementation takes an address"));
    storage::set(IMPLEMENTATION, &implementation.encode());
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
