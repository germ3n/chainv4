//! An example NFT collection following the chainv4 NFT standard (docs/nft-standard.md), the Rust
//! twin of contracts/lua/nft.lua: each token has one owner. Whoever deploys it owns the
//! collection, or whoever a factory names when it clones it: they mint, set where token_uri
//! points, and may upgrade it.

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
const SUPPLY: &[u8] = b"supply";
const NEXT: &[u8] = b"next";

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

/// Owners live where the NFT standard says light clients can prove them: the owners namespace,
/// by the token id in decimal, as the owner's address text.
fn owners() -> storage::Namespace<'static> {
    storage::namespace(b"owners")
}

/// What each holder holds, to list it: count:<holder>, at:<holder>:<n> the nth id, index:<id> its n.
fn held() -> storage::Namespace<'static> {
    storage::namespace(b"held")
}

fn approvals() -> storage::Namespace<'static> {
    storage::namespace(b"approvals")
}

fn operators() -> storage::Namespace<'static> {
    storage::namespace(b"operators")
}

fn uris() -> storage::Namespace<'static> {
    storage::namespace(b"uris")
}

fn text(key: &[u8]) -> String {
    storage::own().get_text(key).unwrap_or_default()
}

fn number(key: &[u8]) -> U256 {
    storage::own().get_u256(key).unwrap_or_default()
}

fn holder_of(id: U256) -> Result<Address, &'static str> {
    owners()
        .get_text(key(id).as_bytes())
        .and_then(|text| Address::parse(&text))
        .ok_or("no such token")
}

fn count(holder: &Address) -> U256 {
    held().get_u256(format!("count:{}", holder.text()).as_bytes()).unwrap_or_default()
}

fn hold(holder: &Address, id: U256) {
    let n = count(holder);
    held().set_u256(format!("at:{}:{}", holder.text(), n.to_decimal()).as_bytes(), id);
    held().set_u256(format!("index:{}", key(id)).as_bytes(), n);
    held().set_u256(format!("count:{}", holder.text()).as_bytes(), n + U256::ONE);
}

/// Takes an id out of a holder's list, moving the last one into its place.
fn release(holder: &Address, id: U256) {
    let last = count(holder) - U256::ONE;
    let index = held().get_u256(format!("index:{}", key(id)).as_bytes()).unwrap_or_default();
    let at = |n: U256| format!("at:{}:{}", holder.text(), n.to_decimal());
    let last_id = held().get_u256(at(last).as_bytes()).unwrap_or_default();

    if index != last {
        held().set_u256(at(index).as_bytes(), last_id);
        held().set_u256(format!("index:{}", key(last_id)).as_bytes(), index);
    }

    held().delete(at(last).as_bytes());
    held().delete(format!("index:{}", key(id)).as_bytes());

    match last.is_zero() {
        true => held().delete(format!("count:{}", holder.text()).as_bytes()),
        false => held().set_u256(format!("count:{}", holder.text()).as_bytes(), last),
    }
}

fn is_operator(holder: &Address, operator: &Address) -> bool {
    operators().get(format!("{}|{}", holder.text(), operator.text()).as_bytes()).is_some()
}

fn approved(id: U256) -> Option<Address> {
    approvals().get_text(key(id).as_bytes()).and_then(|text| Address::parse(&text))
}

fn may_move(spender: &Address, holder: &Address, id: U256) -> bool {
    spender == holder || is_operator(holder, spender) || approved(id).as_ref() == Some(spender)
}

/// Moves a token, a mint from zero or a burn to it, forgetting who was approved for it.
fn move_token(from: &Address, to: &Address, id: U256) {
    approvals().delete(key(id).as_bytes());

    if *from == zero() {
        storage::own().set_u256(SUPPLY, number(SUPPLY) + U256::ONE);
    } else {
        release(from, id);
    }

    if *to == zero() {
        owners().delete(key(id).as_bytes());
        storage::own().set_u256(SUPPLY, number(SUPPLY) - U256::ONE);
    } else {
        owners().set_text(key(id).as_bytes(), &to.text());
        hold(to, id);
    }

    emit("Transfer", &[Value::Address(*from), Value::Address(*to), id.into()]);
}

fn checked_transfer(from: &Address, to: &Address, id: U256) -> Result<(), &'static str> {
    let holder = holder_of(id)?;

    if holder != *from {
        return Err("from does not own the token");
    }

    if !may_move(&sender(), &holder, id) {
        return Err("not the owner, approved or an operator");
    }

    if *to == zero() {
        return Err("transfer to the zero address; burn instead");
    }

    move_token(from, to, id);

    Ok(())
}

/// A contract receiving a token by a safe transfer must say it takes it.
fn received(from: &Address, to: &Address, id: U256) -> Result<(), &'static str> {
    if to.kind != AddressKind::Contract {
        return Ok(());
    }

    let args = [Value::Address(sender()), Value::Address(*from), id.into(), U256::ONE.into()];

    match call(to, "on_nft_received", &args).as_deref() {
        Ok([Value::Bool(true)]) => Ok(()),
        _ => Err("the receiver did not take the token"),
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
    own.set_text(NAME, name.as_deref().unwrap_or("Example Collection"));
    own.set_text(SYMBOL, symbol.as_deref().unwrap_or("EXC"));
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

#[export]
fn total_supply() -> U256 {
    number(SUPPLY)
}

#[export]
fn balance_of(holder: Address) -> U256 {
    count(&holder)
}

#[export]
fn owner_of(id: U256) -> Result<Address, &'static str> {
    holder_of(id)
}

#[export]
fn token_uri(id: U256) -> Result<String, &'static str> {
    holder_of(id)?;

    Ok(uris().get_text(key(id).as_bytes()).unwrap_or_else(|| format!("{}{}", text(BASE_URI), key(id))))
}

/// Up to count ids holder holds, from the start-th on, counting from 0: page through them until
/// fewer come back than asked for.
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
fn transfer(to: Address, id: U256) -> Result<bool, &'static str> {
    checked_transfer(&sender(), &to, id)?;

    Ok(true)
}

#[export]
fn transfer_from(from: Address, to: Address, id: U256) -> Result<bool, &'static str> {
    checked_transfer(&from, &to, id)?;

    Ok(true)
}

#[export]
fn safe_transfer(to: Address, id: U256) -> Result<bool, &'static str> {
    checked_transfer(&sender(), &to, id)?;
    received(&sender(), &to, id)?;

    Ok(true)
}

#[export]
fn safe_transfer_from(from: Address, to: Address, id: U256) -> Result<bool, &'static str> {
    checked_transfer(&from, &to, id)?;
    received(&from, &to, id)?;

    Ok(true)
}

/// Lets spender move one token, until it moves; the zero address approves nobody.
#[export]
fn approve(spender: Address, id: U256) -> Result<bool, &'static str> {
    let holder = holder_of(id)?;
    let caller = sender();

    if caller != holder && !is_operator(&holder, &caller) {
        return Err("not the owner or an operator");
    }

    match spender == zero() {
        true => approvals().delete(key(id).as_bytes()),
        false => approvals().set_text(key(id).as_bytes(), &spender.text()),
    }

    emit("Approval", &[Value::Address(holder), Value::Address(spender), id.into()]);

    Ok(true)
}

#[export]
fn get_approved(id: U256) -> Result<Option<Address>, &'static str> {
    holder_of(id)?;

    Ok(approved(id))
}

/// Lets operator move every token the caller holds, now and later, or stops it.
#[export]
fn set_approval_for_all(operator: Address, yes: bool) -> Result<bool, &'static str> {
    let caller = sender();

    if operator == caller {
        return Err("an owner is their own operator already");
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

/// Beyond the standard: the owner mints the next id to whoever they like, or an id of their
/// choosing, as a bridge's wrapped collection mints the ids the other chain's tokens have, with
/// its own URI or the base one, and returns the id.
#[export]
fn mint(to: Address, uri: Option<String>, chosen: Option<U256>) -> Result<U256, &'static str> {
    only_owner()?;

    if to == zero() {
        return Err("mint to the zero address");
    }

    let next = storage::own().get_u256(NEXT).unwrap_or(U256::ONE);
    let id = chosen.unwrap_or(next);

    if owners().get(key(id).as_bytes()).is_some() {
        return Err("that token exists already");
    }

    if id >= next {
        storage::own().set_u256(NEXT, id + U256::ONE);
    }

    if let Some(uri) = uri {
        uris().set_text(key(id).as_bytes(), &uri);
    }

    move_token(&zero(), &to, id);

    Ok(id)
}

/// Its owner, or whoever may move it, burns a token.
#[export]
fn burn(id: U256) -> Result<bool, &'static str> {
    let holder = holder_of(id)?;

    if !may_move(&sender(), &holder, id) {
        return Err("not the owner, approved or an operator");
    }

    uris().delete(key(id).as_bytes());
    move_token(&holder, &zero(), id);

    Ok(true)
}

/// The owner points token_uri somewhere else: base_uri followed by the id, for tokens minted
/// without their own.
#[export]
fn set_base_uri(base_uri: String) -> Result<bool, &'static str> {
    only_owner()?;
    storage::own().set_text(BASE_URI, &base_uri);

    Ok(true)
}

/// Beyond the standard, as ERC-2981 does it: what a sale of an id owes, and to whom, out of its
/// price, set by the owner for every id or for one, in basis points out of 10,000.
fn royalties() -> storage::Namespace<'static> {
    storage::namespace(b"royalties")
}

const WHOLE: u64 = 10_000;

#[export]
fn set_royalty(receiver: Address, bps: U256, id: Option<U256>) -> Result<bool, &'static str> {
    only_owner()?;

    if bps > U256::from(WHOLE) {
        return Err("a royalty is at most 10,000 basis points, the whole price");
    }

    let at = id.map_or_else(|| String::from("*"), key);
    royalties().set_text(at.as_bytes(), &format!("{}|{}", receiver.text(), bps));
    emit("RoyaltySet", &[Value::Address(receiver), bps.into(), id.unwrap_or(U256::ZERO).into()]);

    Ok(true)
}

/// Who a sale of id at price pays, and how much: the zero address and nothing when none is set.
#[export]
fn royalty_info(id: U256, price: U256) -> Result<(Address, U256), &'static str> {
    let kept = royalties().get_text(key(id).as_bytes()).or_else(|| royalties().get_text(b"*"));
    let Some(kept) = kept else {
        return Ok((zero(), U256::ZERO));
    };
    let (receiver, bps) = kept.rsplit_once('|').ok_or("a royalty kept that does not read")?;
    let receiver = Address::parse(receiver).ok_or("a royalty kept that does not read")?;
    let bps = U256::from_decimal(bps).ok_or("a royalty kept that does not read")?;
    let owed = price.checked_mul(bps).ok_or("a price too large to take a royalty of")?.checked_div(U256::from(WHOLE)).unwrap_or(U256::ZERO);

    Ok((receiver, owed))
}

/// Asked by the chain before an upgrade: only the owner may.
#[export]
fn check_perms(permission: String, who: Address, _detail: Value) -> bool {
    permission == "upgrade" && storage::own().get_text(OWNER) == Some(who.text())
}
