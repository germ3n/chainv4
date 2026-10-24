//! An example token following the chainv4 token standard (docs/token-standard.md), the Rust twin
//! of contracts/lua/token.lua. Whoever deploys it owns it: they receive the initial supply, may
//! mint more, and may upgrade it.

#![no_std]

extern crate alloc;

use {
    alloc::{string::String, vec::Vec},
    chainv4_sdk::{Address, AddressKind, U256, Value, abort, emit, export, sender, storage},
};

const NAME: &str = "Example Token";
const SYMBOL: &str = "EXT";
const DECIMALS: u64 = 18;
/// A million tokens at 18 decimals.
const INITIAL_SUPPLY: u128 = 1_000_000 * 10u128.pow(18);
const OWNER: &[u8] = b"owner";
const SUPPLY: &[u8] = b"supply";

/// The other side of a mint or a burn.
fn zero() -> Address {
    Address {
        kind: AddressKind::Ed25519,
        bytes: [0; 32],
    }
}

fn key(prefix: &[u8], addresses: &[&Address]) -> Vec<u8> {
    let mut key = prefix.to_vec();

    for address in addresses {
        key.extend_from_slice(&address.encode());
    }

    key
}

fn get(key: &[u8]) -> U256 {
    storage::get(key)
        .and_then(|bytes| bytes.try_into().ok())
        .map(U256::from_le_bytes)
        .unwrap_or_default()
}

fn put(key: &[u8], value: U256) {
    storage::set(key, &value.to_le_bytes());
}

fn balance_key(owner: &Address) -> Vec<u8> {
    key(b"balance:", &[owner])
}

fn allowance_key(owner: &Address, spender: &Address) -> Vec<u8> {
    key(b"allowance:", &[owner, spender])
}

fn owner() -> Option<Address> {
    Address::decode(storage::get(OWNER)?.as_slice().try_into().ok()?)
}

fn move_tokens(from: &Address, to: &Address, amount: U256) {
    let remaining = get(&balance_key(from))
        .checked_sub(amount)
        .unwrap_or_else(|| abort("insufficient balance"));
    put(&balance_key(from), remaining);
    let received = get(&balance_key(to))
        .checked_add(amount)
        .unwrap_or_else(|| abort("balance overflow"));
    put(&balance_key(to), received);
    emit("Transfer", &[Value::Address(*from), Value::Address(*to), amount.into()]);
}

fn mint_to(to: &Address, amount: U256) {
    let supply = get(SUPPLY).checked_add(amount).unwrap_or_else(|| abort("supply overflow"));
    put(SUPPLY, supply);
    let balance = get(&balance_key(to))
        .checked_add(amount)
        .unwrap_or_else(|| abort("balance overflow"));
    put(&balance_key(to), balance);
    emit("Transfer", &[Value::Address(zero()), Value::Address(*to), amount.into()]);
}

/// Run once by the chain when the token is deployed.
#[export]
fn init() {
    let deployer = sender();
    storage::set(OWNER, &deployer.encode());
    mint_to(&deployer, U256::from(INITIAL_SUPPLY));
}

#[export]
fn name() -> &'static str {
    NAME
}

#[export]
fn symbol() -> &'static str {
    SYMBOL
}

#[export]
fn decimals() -> u64 {
    DECIMALS
}

#[export]
fn total_supply() -> U256 {
    get(SUPPLY)
}

#[export]
fn balance_of(owner: Address) -> U256 {
    get(&balance_key(&owner))
}

#[export]
fn allowance(owner: Address, spender: Address) -> U256 {
    get(&allowance_key(&owner, &spender))
}

#[export]
fn transfer(to: Address, amount: U256) -> bool {
    move_tokens(&sender(), &to, amount);

    true
}

#[export]
fn approve(spender: Address, amount: U256) -> bool {
    let owner = sender();
    put(&allowance_key(&owner, &spender), amount);
    emit("Approval", &[Value::Address(owner), Value::Address(spender), amount.into()]);

    true
}

#[export]
fn transfer_from(from: Address, to: Address, amount: U256) -> Result<bool, &'static str> {
    let key = allowance_key(&from, &sender());
    let allowed = get(&key).checked_sub(amount).ok_or("insufficient allowance")?;
    put(&key, allowed);
    move_tokens(&from, &to, amount);

    Ok(true)
}

/// Beyond the standard: only the owner mints more.
#[export]
fn mint(to: Address, amount: U256) -> Result<bool, &'static str> {
    if owner() != Some(sender()) {
        return Err("only the owner mints");
    }

    mint_to(&to, amount);

    Ok(true)
}

/// Asked by the chain before an upgrade: only the owner may.
#[export]
fn check_perms(permission: String, who: Address) -> bool {
    permission == "upgrade" && owner() == Some(who)
}
