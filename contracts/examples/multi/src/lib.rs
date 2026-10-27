//! An example multi-token collection following the chainv4 multi-token standard
//! (docs/nft-standard.md), the Rust twin of contracts/lua/multi.lua: holders hold amounts of each
//! id, as ERC-1155's do. Whoever deploys it owns the collection, or whoever a factory names when it
//! clones it: they mint, set where token_uri points, and may upgrade it.

#![no_std]

extern crate alloc;

use {
    alloc::{format, string::String, vec::Vec},
    chainv4_sdk::{Address, AddressKind, U256, Value, call, emit, export, sender, storage},
};

/// The most ids tokens_of returns at once.
const MOST: u64 = 100;
const OWNER: &[u8] = b"owner";
const NAME: &[u8] = b"name";
const SYMBOL: &[u8] = b"symbol";
const BASE_URI: &[u8] = b"base_uri";

/// The other side of a mint or a burn.
fn zero() -> Address {
    Address {
        kind: AddressKind::Ed25519,
        bytes: [0; 32],
    }
}

fn key(id: U256) -> String {
    id.to_decimal()
}

/// Amounts live where the multi-token standard says light clients can prove them: the balances
/// namespace, by the id in decimal, a colon and the holder's address text, 32 bytes
/// little-endian, as the token standard's Rust balances are.
fn balances() -> storage::Namespace<'static> {
    storage::namespace(b"balances")
}

/// The ids each holder holds some of, to list them: count:<holder>, at:<holder>:<n> the nth id,
/// index:<holder>:<id> its n.
fn held() -> storage::Namespace<'static> {
    storage::namespace(b"held")
}

fn operators() -> storage::Namespace<'static> {
    storage::namespace(b"operators")
}

fn uris() -> storage::Namespace<'static> {
    storage::namespace(b"uris")
}

fn supplies() -> storage::Namespace<'static> {
    storage::namespace(b"supplies")
}

fn text(key: &[u8]) -> String {
    storage::own().get_text(key).unwrap_or_default()
}

fn balance(holder: &Address, id: U256) -> U256 {
    balances().get_u256(format!("{}:{}", key(id), holder.text()).as_bytes()).unwrap_or_default()
}

fn count(holder: &Address) -> U256 {
    held().get_u256(format!("count:{}", holder.text()).as_bytes()).unwrap_or_default()
}

fn hold(holder: &Address, id: U256) {
    let n = count(holder);
    held().set_u256(format!("at:{}:{}", holder.text(), n.to_decimal()).as_bytes(), id);
    held().set_u256(format!("index:{}:{}", holder.text(), key(id)).as_bytes(), n);
    held().set_u256(format!("count:{}", holder.text()).as_bytes(), n + U256::ONE);
}

/// Takes an id out of a holder's list, moving the last one into its place.
fn release(holder: &Address, id: U256) {
    let last = count(holder) - U256::ONE;
    let index_of = |id: U256| format!("index:{}:{}", holder.text(), key(id));
    let at = |n: U256| format!("at:{}:{}", holder.text(), n.to_decimal());
    let index = held().get_u256(index_of(id).as_bytes()).unwrap_or_default();
    let last_id = held().get_u256(at(last).as_bytes()).unwrap_or_default();

    if index != last {
        held().set_u256(at(index).as_bytes(), last_id);
        held().set_u256(index_of(last_id).as_bytes(), index);
    }

    held().delete(at(last).as_bytes());
    held().delete(index_of(id).as_bytes());

    match last.is_zero() {
        true => held().delete(format!("count:{}", holder.text()).as_bytes()),
        false => held().set_u256(format!("count:{}", holder.text()).as_bytes(), last),
    }
}

fn set_balance(holder: &Address, id: U256, amount: U256) {
    let before = balance(holder, id);
    let slot = format!("{}:{}", key(id), holder.text());

    match amount.is_zero() {
        true => balances().delete(slot.as_bytes()),
        false => balances().set_u256(slot.as_bytes(), amount),
    }

    if before.is_zero() && !amount.is_zero() {
        hold(holder, id);
    } else if !before.is_zero() && amount.is_zero() {
        release(holder, id);
    }
}

fn supply(id: U256) -> U256 {
    supplies().get_u256(key(id).as_bytes()).unwrap_or_default()
}

fn is_operator(holder: &Address, operator: &Address) -> bool {
    operators().get(format!("{}|{}", holder.text(), operator.text()).as_bytes()).is_some()
}

/// Moves an amount of an id, a mint from zero or a burn to it.
fn move_tokens(from: &Address, to: &Address, id: U256, amount: U256) -> Result<(), &'static str> {
    if *from == zero() {
        supplies().set_u256(key(id).as_bytes(), supply(id).checked_add(amount).ok_or("supply overflow")?);
    } else {
        let left = balance(from, id).checked_sub(amount).ok_or("insufficient balance")?;
        set_balance(from, id, left);
    }

    if *to == zero() {
        supplies().set_u256(key(id).as_bytes(), supply(id) - amount);
    } else {
        set_balance(to, id, balance(to, id).checked_add(amount).ok_or("balance overflow")?);
    }

    emit("TransferSingle", &[Value::Address(sender()), Value::Address(*from), Value::Address(*to), id.into(), amount.into()]);

    Ok(())
}

fn checked_transfer(from: &Address, to: &Address, id: U256, amount: U256) -> Result<(), &'static str> {
    let caller = sender();

    if caller != *from && !is_operator(from, &caller) {
        return Err("not the holder or an operator");
    }

    if *to == zero() {
        return Err("transfer to the zero address; burn instead");
    }

    move_tokens(from, to, id, amount)
}

/// A contract receiving tokens by a safe transfer must say it takes them.
fn received(from: &Address, to: &Address, id: U256, amount: U256) -> Result<(), &'static str> {
    if to.kind != AddressKind::Contract {
        return Ok(());
    }

    let args = [Value::Address(sender()), Value::Address(*from), id.into(), amount.into()];

    match call(to, "on_nft_received", &args).as_deref() {
        Ok([Value::Bool(true)]) => Ok(()),
        _ => Err("the receiver did not take the tokens"),
    }
}

fn only_owner() -> Result<(), &'static str> {
    match storage::own().get_text(OWNER) == Some(sender().text()) {
        true => Ok(()),
        false => Err("only the collection's owner may"),
    }
}

/// Run once by the chain when the collection is deployed, or by a factory's clone, which names
/// its owner; every argument may be left out.
#[export]
fn init(name: Option<String>, symbol: Option<String>, base_uri: Option<String>, owner: Option<Address>) {
    let own = storage::own();
    own.set_text(NAME, name.as_deref().unwrap_or("Example Items"));
    own.set_text(SYMBOL, symbol.as_deref().unwrap_or("EXI"));
    own.set_text(BASE_URI, base_uri.as_deref().unwrap_or(""));
    own.set_text(OWNER, &owner.unwrap_or_else(sender).text());
}

#[export]
fn name() -> String {
    text(NAME)
}

#[export]
fn symbol() -> String {
    text(SYMBOL)
}

/// How much of an id exists.
#[export]
fn total_supply(id: U256) -> U256 {
    supply(id)
}

#[export]
fn balance_of(holder: Address, id: U256) -> U256 {
    balance(&holder, id)
}

#[export]
fn token_uri(id: U256) -> String {
    uris().get_text(key(id).as_bytes()).unwrap_or_else(|| format!("{}{}", text(BASE_URI), key(id)))
}

/// Up to count ids holder holds some of, from the start-th on, counting from 0: page through them
/// until fewer come back than asked for.
#[export]
fn tokens_of(holder: Address, start: Option<U256>, wanted: Option<U256>) -> Vec<Value> {
    let from = start.unwrap_or_default();
    let upto = from.saturating_add(wanted.unwrap_or(U256::from(MOST)).min(U256::from(MOST))).min(count(&holder));
    let mut ids = Vec::new();
    let mut n = from;

    while n < upto {
        ids.push(held().get_u256(format!("at:{}:{}", holder.text(), n.to_decimal()).as_bytes()).unwrap_or_default().into());
        n += U256::ONE;
    }

    ids
}

#[export]
fn transfer(to: Address, id: U256, amount: U256) -> Result<bool, &'static str> {
    checked_transfer(&sender(), &to, id, amount)?;

    Ok(true)
}

#[export]
fn transfer_from(from: Address, to: Address, id: U256, amount: U256) -> Result<bool, &'static str> {
    checked_transfer(&from, &to, id, amount)?;

    Ok(true)
}

#[export]
fn safe_transfer(to: Address, id: U256, amount: U256) -> Result<bool, &'static str> {
    checked_transfer(&sender(), &to, id, amount)?;
    received(&sender(), &to, id, amount)?;

    Ok(true)
}

#[export]
fn safe_transfer_from(from: Address, to: Address, id: U256, amount: U256) -> Result<bool, &'static str> {
    checked_transfer(&from, &to, id, amount)?;
    received(&from, &to, id, amount)?;

    Ok(true)
}

/// Lets operator move every id the caller holds, now and later, or stops it.
#[export]
fn set_approval_for_all(operator: Address, yes: bool) -> Result<bool, &'static str> {
    let caller = sender();

    if operator == caller {
        return Err("a holder is their own operator already");
    }

    let pair = format!("{}|{}", caller.text(), operator.text());

    match yes {
        true => operators().set(pair.as_bytes(), b"1"),
        false => operators().delete(pair.as_bytes()),
    }

    emit("ApprovalForAll", &[Value::Address(caller), Value::Address(operator), Value::Bool(yes)]);

    Ok(true)
}

#[export]
fn is_approved_for_all(holder: Address, operator: Address) -> bool {
    is_operator(&holder, &operator)
}

/// Beyond the standard: the owner mints an amount of an id, giving the id its own URI the first
/// time if they like.
#[export]
fn mint(to: Address, id: U256, amount: U256, uri: Option<String>) -> Result<bool, &'static str> {
    only_owner()?;

    if to == zero() {
        return Err("mint to the zero address");
    }

    if let Some(uri) = uri
        && uris().get(key(id).as_bytes()).is_none()
    {
        uris().set_text(key(id).as_bytes(), &uri);
    }

    move_tokens(&zero(), &to, id, amount)?;

    Ok(true)
}

/// A holder, or their operator, burns an amount of an id.
#[export]
fn burn(from: Address, id: U256, amount: U256) -> Result<bool, &'static str> {
    let caller = sender();

    if caller != from && !is_operator(&from, &caller) {
        return Err("not the holder or an operator");
    }

    move_tokens(&from, &zero(), id, amount)?;

    Ok(true)
}

#[export]
fn set_base_uri(base_uri: String) -> Result<bool, &'static str> {
    only_owner()?;
    storage::own().set_text(BASE_URI, &base_uri);

    Ok(true)
}

/// Asked by the chain before an upgrade: only the owner may.
#[export]
fn check_perms(permission: String, who: Address) -> bool {
    permission == "upgrade" && storage::own().get_text(OWNER) == Some(who.text())
}
