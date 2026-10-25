//! Version 3 of contracts/lua/points, in Rust: the same contract moved to the other engine by an
//! upgrade, as docs/contract-storage.md does it. It keeps version 2's layout as it was, every
//! namespace and key with its kinds and units, and reads and writes the text kinds Lua wrote with
//! get_decimal, get_address_text and get_text, so nothing is retired and nothing moves.

#![no_std]

extern crate alloc;

use {
    alloc::{format, string::String},
    chainv4_sdk::{Address, U256, abort, export, namespace, retired, sender},
};

// Version 2's layout, line for line.
namespace!(POINTS, "points", "address-text" => "decimal:points");
namespace!(BADGES, "badges", "address-text" => "text");
namespace!(CONFIG, "config", { "owner": "address-text", "title": "text", "max_award": "decimal:points" });
retired!("config", "name");

fn owner() -> Address {
    CONFIG.get_address_text(b"owner").unwrap_or_else(|| abort("points has no owner"))
}

fn only_the_owner() {
    if sender() != owner() {
        abort("only the owner may");
    }
}

/// Awards points to someone, at most max_award at once, and returns what they hold now.
#[export]
fn award(who: Address, amount: U256) -> U256 {
    only_the_owner();

    let most = CONFIG.get_decimal(b"max_award").unwrap_or(U256::MAX);

    if amount > most {
        abort(&format!("one award is at most {} points", most.to_decimal()));
    }

    let key = who.text();
    let now = POINTS
        .get_decimal(key.as_bytes())
        .unwrap_or(U256::ZERO)
        .checked_add(amount)
        .unwrap_or_else(|| abort("that is more points than there are"));
    POINTS.set_decimal(key.as_bytes(), now);

    now
}

/// Gives someone a badge, in place of any they had.
#[export]
fn give_badge(who: Address, badge: String) {
    only_the_owner();
    BADGES.set_text(who.text().as_bytes(), &badge);
}

#[export]
fn points_of(who: Address) -> U256 {
    POINTS.get_decimal(who.text().as_bytes()).unwrap_or(U256::ZERO)
}

#[export]
fn badge_of(who: Address) -> Option<String> {
    BADGES.get_text(who.text().as_bytes())
}

#[export]
fn title() -> Option<String> {
    CONFIG.get_text(b"title")
}

#[export]
fn check_perms(permission: String, who: Address) -> bool {
    permission == "upgrade" && who == owner()
}
