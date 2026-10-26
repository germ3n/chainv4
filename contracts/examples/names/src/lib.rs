//! Looks up the chain's names, such as alice.dev, and pays them: what a wallet's address book does,
//! inside a contract.

#![no_std]

extern crate alloc;

use {
    alloc::string::String,
    chainv4_sdk::{Address, abort, export, name_of, resolve_name, transfer, value},
};

/// Where a name points now, or nil.
#[export]
fn lookup(name: String) -> Option<Address> {
    resolve_name(&name)
}

/// What an address is called, such as alice.dev, or nil.
#[export]
fn whois(account: Address) -> Option<String> {
    name_of(&account)
}

/// Pays what the call sent to whatever a name resolves to, and returns who that is.
#[export]
fn pay(name: String) -> Address {
    let to = resolve_name(&name).unwrap_or_else(|| abort("no such name"));
    transfer(&to, value()).unwrap_or_else(|why| abort(&why));

    to
}
