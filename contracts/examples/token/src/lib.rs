//! An example token following the chainv4 token standard (docs/token-standard.md), the Rust twin
//! of contracts/lua/token.lua. Whoever deploys it owns it: they receive the initial supply, may
//! mint more, and may upgrade it.

#![no_std]

extern crate alloc;

use {
    alloc::{string::String, vec::Vec},
    alloc::format,
    chainv4_sdk::{Address, AddressKind, U256, Value, abort, block_time, chain_id, emit, export, key_address, sender, storage, this, verify},
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

/// Signed by an owner off the chain, and sent by anyone (docs/token-standard.md, "Signed
/// authorizations"): the owner's key, which must be the one the owner's address is of, and its
/// signature over the text below, by its scheme's own rules.
fn signed_by(owner: &Address, key: &[u8], message: &str, signature: &[u8]) {
    let scheme = match owner.kind {
        AddressKind::Ed25519 => "ed25519",
        AddressKind::EdBlake => "edblake",
        AddressKind::Secp256k1 => "secp256k1",
        AddressKind::Bitcoin => "bitcoin",
        AddressKind::Contract | AddressKind::Evm => abort("only a key's account signs"),
    };

    if key_address(scheme, key) != *owner {
        abort("the key is not the owner's");
    }

    if !verify(scheme, key, message.as_bytes(), signature) {
        abort("the signature does not hold");
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn permit_key(owner: &Address) -> Vec<u8> {
    key(b"permit:", &[owner])
}

fn used_key(owner: &Address, nonce: &[u8]) -> Vec<u8> {
    let mut key = key(b"used:", &[owner]);
    key.extend_from_slice(nonce);

    key
}

fn permits(owner: &Address) -> u64 {
    storage::get(&permit_key(owner)).and_then(|bytes| bytes.try_into().ok()).map(u64::from_le_bytes).unwrap_or(0)
}

fn permit_text(owner: &Address, spender: &Address, amount: U256, deadline: u64) -> String {
    format!(
        "chainv4 token permit\nchain: {}\ntoken: {}\nowner: {}\nspender: {}\namount: {}\nnonce: {}\ndeadline: {deadline}",
        chain_id(),
        this().text(),
        owner.text(),
        spender.text(),
        amount.to_decimal(),
        permits(owner),
    )
}

/// What an owner signs to let a spender move its tokens: the chain, this token, both parties, the
/// allowance, the owner's next permit nonce and when it lapses, in milliseconds.
#[export]
fn permit_message(owner: Address, spender: Address, amount: U256, deadline: u64) -> String {
    permit_text(&owner, &spender, amount, deadline)
}

fn transfer_text(from: &Address, to: &Address, amount: U256, valid_after: u64, valid_before: u64, nonce: &[u8]) -> String {
    format!(
        "chainv4 token transfer\nchain: {}\ntoken: {}\nfrom: {}\nto: {}\namount: {}\nvalid after: {valid_after}\nvalid before: {valid_before}\nnonce: 0x{}",
        chain_id(),
        this().text(),
        from.text(),
        to.text(),
        amount.to_decimal(),
        hex(nonce),
    )
}

/// What an owner signs to send tokens a relayer submits: the chain, this token, both parties, the
/// amount, the window it holds in, after one time and before another, and a nonce of 32 bytes the
/// owner picks, used once.
#[export]
fn transfer_message(from: Address, to: Address, amount: U256, valid_after: u64, valid_before: u64, nonce: Vec<u8>) -> String {
    transfer_text(&from, &to, amount, valid_after, valid_before, &nonce)
}

/// How many permits an owner has had taken: the nonce its next one signs.
#[export]
fn nonces(owner: Address) -> U256 {
    U256::from(permits(&owner))
}

/// Sets an allowance an owner signed, as approve would from the owner.
#[export]
fn permit(owner: Address, spender: Address, amount: U256, deadline: u64, key: Vec<u8>, signature: Vec<u8>) -> Result<bool, &'static str> {
    if block_time() > deadline {
        return Err("the permit has lapsed");
    }

    signed_by(&owner, &key, &permit_text(&owner, &spender, amount, deadline), &signature);
    storage::set(&permit_key(&owner), &(permits(&owner) + 1).to_le_bytes());
    put(&allowance_key(&owner, &spender), amount);
    emit("Approval", &[Value::Address(owner), Value::Address(spender), amount.into()]);

    Ok(true)
}

fn used(owner: &Address, nonce: &[u8]) -> bool {
    storage::get(&used_key(owner, nonce)).is_some()
}

/// Whether a transfer's nonce was used, or not yet.
#[export]
fn authorization_state(owner: Address, nonce: Vec<u8>) -> bool {
    used(&owner, &nonce)
}

/// Sends tokens their owner signed for, as transfer would from the owner, once.
#[export]
#[allow(clippy::too_many_arguments)]
fn transfer_with_authorization(from: Address, to: Address, amount: U256, valid_after: u64, valid_before: u64, nonce: Vec<u8>, key: Vec<u8>, signature: Vec<u8>) -> Result<bool, &'static str> {
    if nonce.len() != 32 {
        return Err("a nonce is 32 bytes");
    }

    let now = block_time();

    if now <= valid_after {
        return Err("the authorization is not valid yet");
    }

    if now >= valid_before {
        return Err("the authorization has lapsed");
    }

    if used(&from, &nonce) {
        return Err("the authorization was used");
    }

    signed_by(&from, &key, &transfer_text(&from, &to, amount, valid_after, valid_before, &nonce), &signature);
    storage::set(&used_key(&from, &nonce), &[1]);
    emit("AuthorizationUsed", &[Value::Address(from), Value::Bytes(nonce)]);
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
