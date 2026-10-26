//! The name registry: names such as alice.dev that point at an address, chainv4's own ENS. Genesis
//! turns it on with a suffix, the price of a year by a label's length, a grace period and the most
//! years a name is held ahead; under governance, Change::NamePrices changes the prices.
//!
//! A name is registered for whole years, its price burned. It points at an address, holds text
//! records, and has subnames its owner creates for free, which last as long as it does. When it
//! expires it stops resolving, its owner alone may renew it for the grace period, and after that
//! anyone may register it afresh: everything it held, subnames included, is gone then, since each
//! subname remembers the registration it was made under. An account may name one of the names
//! pointing at it as its primary name, which reverse lookups give.
//!
//! Records are kept by name, without the suffix: "alice", "pay.alice". Primary names by account.

use {
    crate::crypto::hashable::Hashable,
    crate::db::address_key,
    crate::structs::address::Address,
    crate::structs::u256::{U256LeList, u256},
    r#macro::document,
    wincode::{SchemaRead, SchemaWrite},
};

#[document(summary = "A year, as names count it: 365 days, in milliseconds.", category = "Names", see_also = "NamesConfig")]
pub const YEAR_MS: u64 = 365 * 24 * 60 * 60 * 1000;

#[document(summary = "The shortest name that may be registered: 3 characters. Subnames may be shorter.", category = "Names", see_also = "NamesConfig::prices")]
pub const MIN_LABEL: usize = 3;

#[document(summary = "The longest label, a name or subname's part between dots: 63 characters, as in DNS.", category = "Names")]
pub const MAX_LABEL: usize = 63;

#[document(summary = "The longest whole name, subnames and suffix included: 253 characters, as in DNS.", category = "Names")]
pub const MAX_NAME_LENGTH: usize = 253;

#[document(summary = "The most prices a list holds: one for each length a name may have.", category = "Names", see_also = "NamesConfig::prices")]
pub const MAX_PRICES: usize = MAX_LABEL - MIN_LABEL + 1;

#[document(summary = "The most text records a name holds.", category = "Names", see_also = "NameAction::Text")]
pub const MAX_TEXTS: usize = 16;

#[document(summary = "The longest text record key: 32 characters.", category = "Names", see_also = "NameAction::Text")]
pub const MAX_TEXT_KEY: usize = 32;

#[document(summary = "The longest text record value: 512 bytes.", category = "Names", see_also = "NameAction::Text")]
pub const MAX_TEXT_VALUE: usize = 512;

#[document(
    summary = "A chain's names, as genesis sets them: the suffix every name ends in, what a year costs, the grace period and how far ahead a name may be held.",
    category = "Names",
    see_also = "Names, Genesis::names, Change::NamePrices",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct NamesConfig {
    #[document(summary = "What every name ends in, such as dev for alice.dev: one label, as names are made of.")]
    pub suffix: String,
    #[document(summary = "What one year of a name costs, in the coin's smallest units, by its length: the first for names of MIN_LABEL characters, the next for one more, and the last for every length from its own on. One to MAX_PRICES of them. Burned. Governance's Change::NamePrices replaces them.")]
    #[wincode(with = "U256LeList")]
    pub prices: Vec<u256>,
    #[document(summary = "How long after a name expires its owner alone may still renew it, in milliseconds; then anyone may register it.")]
    pub grace_ms: u64,
    #[document(summary = "The most years a registration or renewal may hold a name ahead of now.")]
    pub max_years: u32,
}

impl Hashable for NamesConfig {}

impl NamesConfig {
    #[document(
        parent = "NamesConfig",
        summary = "A development chain's names: alice.dev, a year costing 640 coins for three characters, 160 for four and 5 for five or more, at 18 decimals, as ENS prices them in dollars; 90 days' grace, and up to 100 years ahead.",
    )]
    pub fn dev() -> NamesConfig {
        let coins = |count: u64| u256::from(count) * u256::from(10u64).pow(18);

        NamesConfig {
            suffix: "dev".to_string(),
            prices: vec![coins(640), coins(160), coins(5)],
            grace_ms: 90 * 24 * 60 * 60 * 1000,
            max_years: 100,
        }
    }

    #[document(parent = "NamesConfig", summary = "Why the settings cannot be a chain's: a suffix that is no label, no prices or too many, or no years at all.")]
    pub fn problem(&self) -> Option<&'static str> {
        if label_problem(&self.suffix, 1).is_some() {
            return Some("names.suffix must be one label: 1 to 63 of a to z, 0 to 9 and hyphens, such as dev");
        }

        if let Some(problem) = prices_problem(&self.prices) {
            return Some(problem);
        }

        if self.max_years == 0 {
            return Some("names.max_years must be at least 1");
        }

        None
    }

    #[document(parent = "NamesConfig", summary = "A name as people read it: with the suffix, alice.dev for alice.")]
    pub fn display(&self, name: &str) -> String {
        format!("{name}.{}", self.suffix)
    }

    #[document(parent = "NamesConfig", summary = "A name as records keep it: without the suffix, if it ends in it, and in lower case: alice for Alice.dev.")]
    pub fn strip(&self, text: &str) -> String {
        let lower = text.trim().to_ascii_lowercase();

        match lower.strip_suffix(&format!(".{}", self.suffix)) {
            Some(name) => name.to_string(),
            None => lower,
        }
    }
}

#[document(
    summary = "Why a list of prices cannot be a chain's: empty, or more than MAX_PRICES.",
    category = "Names",
    see_also = "NamesConfig::prices",
)]
pub fn prices_problem(prices: &[u256]) -> Option<&'static str> {
    match prices.len() {
        0 => Some("names.prices needs at least one price, for every length"),
        len if len > MAX_PRICES => Some("names.prices holds at most 61 prices, one for each length from 3 to 63"),
        _ => None,
    }
}

#[document(
    summary = "What some years of a name cost under a list of prices: its length's price times the years. None when that overflows a u256.",
    category = "Names",
    see_also = "NamesConfig::prices",
)]
pub fn price(prices: &[u256], name: &str, years: u32) -> Option<u256> {
    let index = name.len().saturating_sub(MIN_LABEL).min(prices.len().checked_sub(1)?);

    prices[index].checked_mul(u256::from(years))
}

#[document(
    summary = "Why text cannot be a label: empty, shorter than shortest or longer than MAX_LABEL, holding anything but a to z, 0 to 9 and hyphens, or starting or ending with a hyphen.",
    category = "Names",
)]
pub fn label_problem(label: &str, shortest: usize) -> Option<String> {
    if label.len() < shortest.max(1) {
        return Some(match shortest {
            0 | 1 => "a label holds at least one character".to_string(),
            _ => format!("a name holds at least {shortest} characters"),
        });
    }

    if label.len() > MAX_LABEL {
        return Some(format!("a label holds at most {MAX_LABEL} characters"));
    }

    if !label.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-') {
        return Some("a label holds only a to z, 0 to 9 and hyphens".to_string());
    }

    if label.starts_with('-') || label.ends_with('-') {
        return Some("a label neither starts nor ends with a hyphen".to_string());
    }

    None
}

#[document(
    summary = "Why text cannot be a name, as records keep it, without the suffix: labels joined by dots, the last one at least MIN_LABEL long, all of it at most MAX_NAME_LENGTH.",
    category = "Names",
    see_also = "label_problem",
)]
pub fn name_problem(name: &str) -> Option<String> {
    if name.len() > MAX_NAME_LENGTH {
        return Some(format!("a name holds at most {MAX_NAME_LENGTH} characters"));
    }

    let mut labels = name.split('.').rev();
    let top = labels.next().unwrap_or_default();

    label_problem(top, MIN_LABEL).or_else(|| labels.find_map(|label| label_problem(label, 1)))
}

#[document(
    summary = "Why text cannot be a text record's key: empty, longer than MAX_TEXT_KEY, or holding anything but a to z, 0 to 9, dots, hyphens and underscores.",
    category = "Names",
    see_also = "NameAction::Text",
)]
pub fn text_key_problem(key: &str) -> Option<String> {
    if key.is_empty() || key.len() > MAX_TEXT_KEY {
        return Some(format!("a text key holds 1 to {MAX_TEXT_KEY} characters"));
    }

    if !key.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'-' | b'_')) {
        return Some("a text key holds only a to z, 0 to 9, dots, hyphens and underscores, such as url or com.github".to_string());
    }

    None
}

#[document(
    summary = "What a name transaction does to its name.",
    category = "Names",
    see_also = "Tx::Name, Names",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
#[wincode(tag_encoding = "u8")]
pub enum NameAction {
    #[document(
        summary = "Registers a name nobody holds, or one past its grace period, to the sender, pointing at the sender. Its price for the years is burned.",
        params = { years = "How many years: 1 to the chain's max_years." },
    )]
    #[wincode(tag = 0)]
    Register { years: u32 },
    #[document(
        summary = "Holds the sender's name for more years from when it expires, at its price for them, burned: before it expires, or in its grace period. Not past max_years from now.",
        params = { years = "How many years more." },
    )]
    #[wincode(tag = 1)]
    Renew { years: u32 },
    #[document(
        summary = "Gives the name, or subname, to another account. What it points at and its records stay.",
        params = { to = "The new owner." },
    )]
    #[wincode(tag = 2)]
    Transfer { to: Address },
    #[document(
        summary = "Points the name at an address, or at nothing.",
        params = { target = "The address it resolves to, or None for none." },
    )]
    #[wincode(tag = 3)]
    Target { target: Option<Address> },
    #[document(
        summary = "Sets one of the name's text records, such as url, avatar or description, or clears it.",
        params = {
            key = "The record's key: 1 to MAX_TEXT_KEY of a to z, 0 to 9, dots, hyphens and underscores.",
            value = "Its value, up to MAX_TEXT_VALUE bytes, or None to clear it.",
        },
    )]
    #[wincode(tag = 4)]
    Text { key: String, value: Option<String> },
    #[document(
        summary = "Creates a subname, label.name, for nothing, owned by and pointing at an account, or takes one back, replacing it. It lasts as long as the name does.",
        params = {
            label = "The subname's own label: 1 to MAX_LABEL of a to z, 0 to 9 and hyphens.",
            owner = "Who holds it.",
        },
    )]
    #[wincode(tag = 5)]
    Subname { label: String, owner: Address },
}

impl Hashable for NameAction {
    const TAG_LEN: usize = 1;
}

impl NameAction {
    #[document(parent = "NameAction", summary = "The action's name, as the API and receipts name it: register, renew, transfer, target, text or subname.")]
    pub fn kind(&self) -> &'static str {
        match self {
            NameAction::Register { .. } => "register",
            NameAction::Renew { .. } => "renew",
            NameAction::Transfer { .. } => "transfer",
            NameAction::Target { .. } => "target",
            NameAction::Text { .. } => "text",
            NameAction::Subname { .. } => "subname",
        }
    }

    #[document(parent = "NameAction", summary = "Why the action can never go through, whatever the chain holds: no years, a bad key, a value too long, or a bad label.")]
    pub fn problem(&self) -> Option<String> {
        match self {
            NameAction::Register { years: 0 } | NameAction::Renew { years: 0 } => Some("a name is held for at least a year".to_string()),
            NameAction::Text { key, value } => text_key_problem(key).or_else(|| match value {
                Some(value) if value.len() > MAX_TEXT_VALUE => Some(format!("a text value holds at most {MAX_TEXT_VALUE} bytes")),
                _ => None,
            }),
            NameAction::Subname { label, .. } => label_problem(label, 1),
            _ => None,
        }
    }
}

#[document(summary = "One text record of a name.", category = "Names", see_also = "NameRecord")]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct TextRecord {
    #[document(summary = "Its key, such as url.")]
    pub key: String,
    #[document(summary = "Its value.")]
    pub value: String,
}

#[document(
    summary = "A name or subname as the registry keeps it.",
    category = "Names",
    see_also = "Names",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct NameRecord {
    #[document(summary = "Who holds it, and alone may change it.")]
    pub owner: Address,
    #[document(summary = "The address it resolves to, or None.")]
    pub target: Option<Address>,
    #[document(summary = "When it expires, in milliseconds since the Unix epoch, for a name; 0 for a subname, which lasts as long as its name.")]
    pub expires: u64,
    #[document(summary = "The height its name was registered at, which tells a registration from a later one of the same name: a subname made under an earlier one is gone.")]
    pub root: u64,
    #[document(summary = "Its text records, sorted by key.")]
    pub texts: Vec<TextRecord>,
}

impl Hashable for NameRecord {}

#[document(
    summary = "An account's primary name as the registry keeps it: the name it chose, without the suffix. It counts only while the name resolves to the account.",
    category = "Names",
    see_also = "Tx::PrimaryName, primary_key",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct PrimaryName {
    #[document(summary = "The name.")]
    pub name: String,
}

impl Hashable for PrimaryName {}

#[document(summary = "Where a name stands in time.", category = "Names", see_also = "standing")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameStatus {
    #[document(summary = "Held, and resolving.")]
    Active,
    #[document(summary = "Expired, in its grace period: it resolves to nothing, and only its owner may renew it.")]
    Grace,
    #[document(summary = "Past its grace period, or never registered: anyone may register it.")]
    Available,
}

impl NameStatus {
    #[document(parent = "NameStatus", summary = "The status as the API names it: active, grace or available.")]
    pub fn name(self) -> &'static str {
        match self {
            NameStatus::Active => "active",
            NameStatus::Grace => "grace",
            NameStatus::Available => "available",
        }
    }
}

#[document(summary = "A name that stands, as standing finds it.", category = "Names", see_also = "standing")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FoundName {
    #[document(summary = "Its record.")]
    pub record: NameRecord,
    #[document(summary = "When it expires: its own time, or for a subname its name's.")]
    pub expires: u64,
    #[document(summary = "Whether it resolves, or is in its grace period.")]
    pub status: NameStatus,
}

/// Key kinds: a name's record by name, an account's primary name by its address key, and an
/// owner's names by its address key and the name.
const RECORD: u8 = b'n';
const PRIMARY: u8 = b'p';
const OWNED: u8 = b'o';

#[document(summary = "The key a name's record is kept under, in Column::Names: what a proof of it asks for.", category = "Names", see_also = "NameRecord")]
pub fn record_key(name: &str) -> Vec<u8> {
    [&[RECORD][..], name.as_bytes()].concat()
}

#[document(summary = "The key an account's primary name is kept under, in Column::Names.", category = "Names", see_also = "PrimaryName")]
pub fn primary_key(address: &Address) -> Vec<u8> {
    [vec![PRIMARY], address_key(address)].concat()
}

#[document(
    summary = "Where the names an account owns are listed, in Column::Names: this prefix, then each name. The listing holds every name and subname whose record names the account its owner, standing or not.",
    category = "Names",
    see_also = "owned_key",
)]
pub fn owned_prefix(owner: &Address) -> Vec<u8> {
    let key = address_key(owner);

    [vec![OWNED, key.len() as u8], key].concat()
}

#[document(summary = "The key that lists one name under its owner, in Column::Names.", category = "Names", see_also = "owned_prefix")]
pub fn owned_key(owner: &Address, name: &str) -> Vec<u8> {
    [owned_prefix(owner), name.as_bytes().to_vec()].concat()
}

#[document(
    summary = "A name and every name above it, to the one registered: x.pay.alice, pay.alice, alice. What standing needs the records of, in this order.",
    category = "Names",
    see_also = "standing",
)]
pub fn lineage(name: &str) -> Vec<String> {
    let labels: Vec<&str> = name.split('.').collect();

    (0..labels.len()).map(|start| labels[start..].join(".")).collect()
}

#[document(
    summary = "Whether a name stands at a time, from the records of its lineage: registered, not past its grace period, and for a subname, made under its name's current registration, as is every subname between. A node reads the records from its state; a light client from proofs.",
    params = {
        records = "The record of each name in the lineage, the name's first, None where there is none.",
        now = "The time, in milliseconds since the Unix epoch.",
        grace_ms = "The chain's grace period.",
    },
    returns = "The name, or None when it is available.",
    category = "Names",
    see_also = "lineage",
)]
pub fn standing(records: &[Option<NameRecord>], now: u64, grace_ms: u64) -> Option<FoundName> {
    let (Some(Some(record)), Some(Some(top))) = (records.first(), records.last()) else {
        return None;
    };

    // Every name between, from the subname up, must be of the same registration.
    if !records.iter().all(|between| between.as_ref().is_some_and(|between| between.root == top.root)) {
        return None;
    }

    let status = match now {
        now if now < top.expires => NameStatus::Active,
        now if now < top.expires.saturating_add(grace_ms) => NameStatus::Grace,
        _ => return None,
    };

    Some(FoundName {
        record: record.clone(),
        expires: top.expires,
        status,
    })
}

#[cfg(feature = "names")]
mod registry;
#[cfg(feature = "names")]
pub use registry::{Names, stored_owned_names};

#[cfg(all(test, feature = "names"))]
mod tests;
