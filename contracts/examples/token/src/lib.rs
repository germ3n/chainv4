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

/// Balances live where the token standard says light clients can prove them: the balances
/// namespace, by the holder's address text.
const BALANCES: &[u8] = b"balances";

fn balance(owner: &Address) -> U256 {
    storage::namespace(BALANCES).get(owner.text().as_bytes()).and_then(|bytes| bytes.try_into().ok()).map(U256::from_le_bytes).unwrap_or_default()
}

fn set_balance(owner: &Address, value: U256) {
    storage::namespace(BALANCES).set(owner.text().as_bytes(), &value.to_le_bytes());
}

fn allowance_key(owner: &Address, spender: &Address) -> Vec<u8> {
    key(b"allowance:", &[owner, spender])
}

fn owner() -> Option<Address> {
    Address::decode(storage::get(OWNER)?.as_slice().try_into().ok()?)
}

fn move_tokens(from: &Address, to: &Address, amount: U256) {
    let remaining = balance(from)
        .checked_sub(amount)
        .unwrap_or_else(|| abort("insufficient balance"));
    set_balance(from, remaining);
    let received = balance(to)
        .checked_add(amount)
        .unwrap_or_else(|| abort("balance overflow"));
    set_balance(to, received);
    emit("Transfer", &[Value::Address(*from), Value::Address(*to), amount.into()]);
}

fn mint_to(to: &Address, amount: U256) {
    let supply = get(SUPPLY).checked_add(amount).unwrap_or_else(|| abort("supply overflow"));
    put(SUPPLY, supply);
    let balance = balance(to)
        .checked_add(amount)
        .unwrap_or_else(|| abort("balance overflow"));
    set_balance(to, balance);
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
    balance(&owner)
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
