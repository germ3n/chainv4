use super::*;
use crate::db::{MemoryDb, State};
use crate::registry::{Accounts, RegistryError};
use std::sync::Arc;

const DAY: u64 = 24 * 60 * 60 * 1000;

fn account(seed: u8) -> Address {
    Address::Ed25519([seed; 32])
}

fn config() -> NamesConfig {
    NamesConfig {
        suffix: "dev".to_string(),
        prices: vec![u256::from(300u64), u256::from(40u64), u256::from(5u64)],
        grace_ms: 90 * DAY,
        max_years: 10,
    }
}

/// A state where the first three accounts hold 10,000 each.
fn funded() -> State {
    let state = State::new(Arc::new(MemoryDb::default()));

    for seed in 1..=3 {
        Accounts::new(&state).credit(&account(seed), u256::from(10_000u64)).unwrap();
    }

    state
}

fn act(state: &State, sender: u8, name: &str, action: NameAction, now: u64) -> Result<u256, RegistryError> {
    Names::new(state).act(&account(sender), name, &action, now, now / DAY, &config(), &config().prices)
}

fn balance(state: &State, seed: u8) -> u256 {
    Accounts::new(state).get(&account(seed)).unwrap().balance
}

fn refused(result: Result<u256, RegistryError>) -> String {
    match result {
        Err(RegistryError::Name(why)) => why,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn names_are_labels_of_lower_case_letters_digits_and_hyphens() {
    assert_eq!(name_problem("alice"), None);
    assert_eq!(name_problem("pay.alice"), None);
    assert_eq!(name_problem("a.b.alice"), None, "subnames may be one character");
    assert_eq!(name_problem("x-1"), None);
    assert!(name_problem("al").is_some(), "a name holds at least three");
    assert!(name_problem("Alice").is_some(), "upper case is refused, not folded");
    assert!(name_problem("-ab").is_some());
    assert!(name_problem("ab-").is_some());
    assert!(name_problem("al ice").is_some());
    assert!(name_problem("pay..alice").is_some(), "an empty label");
    assert!(name_problem(&"a".repeat(64)).is_some());
    assert!(name_problem(&["a"; 130].join(".")).is_some(), "too long altogether");
    assert!(text_key_problem("com.github").is_none());
    assert!(text_key_problem("URL").is_some());

    let names = config();

    assert_eq!(names.strip("Alice.dev"), "alice");
    assert_eq!(names.strip("pay.alice.dev"), "pay.alice");
    assert_eq!(names.strip("alice"), "alice");
    assert_eq!(names.display("alice"), "alice.dev");
    assert_eq!(names.problem(), None);
    assert!(NamesConfig { suffix: "d.v".into(), ..config() }.problem().is_some());
    assert!(NamesConfig { prices: Vec::new(), ..config() }.problem().is_some());
    assert!(NamesConfig { prices: vec![u256::ONE; MAX_PRICES + 1], ..config() }.problem().is_some());
    assert!(NamesConfig { max_years: 0, ..config() }.problem().is_some());
    assert_eq!(NamesConfig::dev().problem(), None);
}

#[test]
fn a_year_costs_by_length_and_the_last_price_covers_every_longer_name() {
    let prices = config().prices;

    assert_eq!(price(&prices, "abc", 1), Some(u256::from(300u64)));
    assert_eq!(price(&prices, "abcd", 2), Some(u256::from(80u64)));
    assert_eq!(price(&prices, "abcde", 1), Some(u256::from(5u64)));
    assert_eq!(price(&prices, &"a".repeat(63), 3), Some(u256::from(15u64)));
    assert_eq!(price(&[u256::from(7u64)], "abcdef", 2), Some(u256::from(14u64)), "one price is every length's");
    assert_eq!(price(&[u256::MAX], "abc", 2), None, "an overflow is no price");
}

#[test]
fn registering_burns_the_price_and_points_the_name_at_its_owner() {
    let state = funded();
    let names = Names::new(&state);

    assert_eq!(act(&state, 1, "alice", NameAction::Register { years: 2 }, DAY), Ok(u256::from(10u64)));
    assert_eq!(balance(&state, 1), u256::from(9_990u64), "five a year for five characters, burned");
    assert_eq!(names.resolve("alice", DAY, config().grace_ms).unwrap(), Some(account(1)));

    let found = names.lookup("alice", DAY, config().grace_ms).unwrap().unwrap();

    assert_eq!(found.status, NameStatus::Active);
    assert_eq!(found.expires, DAY + 2 * YEAR_MS);
    assert_eq!(found.record.root, 1);

    assert!(refused(act(&state, 2, "alice", NameAction::Register { years: 1 }, DAY)).contains("taken"));
    assert!(refused(act(&state, 1, "alice", NameAction::Register { years: 1 }, DAY)).contains("renew"));
    assert!(refused(act(&state, 2, "bob", NameAction::Register { years: 11 }, DAY)).contains("at most 10 years"));
    assert!(refused(act(&state, 2, "pay.bob", NameAction::Register { years: 1 }, DAY)).contains("subname"));
    assert_eq!(act(&state, 9, "bob", NameAction::Register { years: 1 }, DAY), Err(RegistryError::InsufficientBalance), "nothing to pay with");
    assert_eq!(names.record("bob").unwrap(), None);

    // Three characters cost the most.
    act(&state, 2, "bob", NameAction::Register { years: 1 }, DAY).unwrap();

    assert_eq!(balance(&state, 2), u256::from(9_700u64));
}

#[test]
fn a_name_expires_into_its_grace_period_and_then_anyone_may_take_it() {
    let state = funded();
    let names = Names::new(&state);
    let grace = config().grace_ms;

    act(&state, 1, "alice", NameAction::Register { years: 1 }, 0).unwrap();
    act(&state, 1, "alice", NameAction::Subname { label: "pay".into(), owner: account(3) }, 0).unwrap();
    act(&state, 1, "alice", NameAction::Text { key: "url".into(), value: Some("https://alice.example".into()) }, 0).unwrap();

    // Expired: it resolves to nothing, nor do its subnames, and only its owner may renew it.
    let expired = YEAR_MS + DAY;

    assert_eq!(names.lookup("alice", expired, grace).unwrap().unwrap().status, NameStatus::Grace);
    assert_eq!(names.resolve("alice", expired, grace).unwrap(), None);
    assert_eq!(names.resolve("pay.alice", expired, grace).unwrap(), None);
    assert!(refused(act(&state, 2, "alice", NameAction::Register { years: 1 }, expired)).contains("taken"));
    assert!(refused(act(&state, 2, "alice", NameAction::Renew { years: 1 }, expired)).contains("not yours"));
    assert!(refused(act(&state, 1, "alice", NameAction::Target { target: None }, expired)).contains("expired"));

    // Renewed in its grace period: from when it expired, not from now, and everything comes back.
    act(&state, 1, "alice", NameAction::Renew { years: 1 }, expired).unwrap();

    assert_eq!(names.lookup("alice", expired, grace).unwrap().unwrap().expires, 2 * YEAR_MS);
    assert_eq!(names.resolve("pay.alice", expired, grace).unwrap(), Some(account(3)));

    // Past its grace period it is available, and a new registration starts afresh.
    let gone = 2 * YEAR_MS + grace;

    assert_eq!(names.lookup("alice", gone, grace).unwrap(), None);
    assert!(refused(act(&state, 1, "alice", NameAction::Renew { years: 1 }, gone)).contains("nobody holds"));

    act(&state, 2, "alice", NameAction::Register { years: 1 }, gone).unwrap();

    let taken = names.lookup("alice", gone, grace).unwrap().unwrap();

    assert_eq!((taken.record.owner, taken.record.target), (account(2), Some(account(2))));
    assert!(taken.record.texts.is_empty(), "the old owner's records went with the old registration");
    assert_eq!(names.lookup("pay.alice", gone, grace).unwrap(), None, "and so did its subnames");
}

#[test]
fn renewals_hold_a_name_at_most_max_years_ahead() {
    let state = funded();

    act(&state, 1, "alice", NameAction::Register { years: 6 }, 0).unwrap();

    assert!(refused(act(&state, 1, "alice", NameAction::Renew { years: 5 }, 0)).contains("at most 10 years"));
    assert_eq!(act(&state, 1, "alice", NameAction::Renew { years: 4 }, 0), Ok(u256::from(20u64)));
    assert_eq!(Names::new(&state).lookup("alice", 0, 0).unwrap().unwrap().expires, 10 * YEAR_MS);
    assert!(refused(act(&state, 1, "pay.alice", NameAction::Renew { years: 1 }, 0)).contains("nobody holds"));
}

#[test]
fn owners_point_their_names_give_them_away_and_keep_text_records() {
    let state = funded();
    let names = Names::new(&state);
    let text = |key: &str, value: Option<&str>| NameAction::Text { key: key.into(), value: value.map(str::to_string) };

    act(&state, 1, "alice", NameAction::Register { years: 1 }, 0).unwrap();
    act(&state, 1, "alice", NameAction::Target { target: Some(account(3)) }, 0).unwrap();

    assert_eq!(names.resolve("alice", 0, 0).unwrap(), Some(account(3)));
    assert!(refused(act(&state, 2, "alice", NameAction::Target { target: None }, 0)).contains("not yours"));
    assert!(refused(act(&state, 2, "carol", NameAction::Target { target: None }, 0)).contains("nobody holds"));

    act(&state, 1, "alice", text("url", Some("https://a.example")), 0).unwrap();
    act(&state, 1, "alice", text("avatar", Some("ipfs://x")), 0).unwrap();
    act(&state, 1, "alice", text("url", Some("https://b.example")), 0).unwrap();

    let keys = |state: &State| -> Vec<(String, String)> { Names::new(state).record("alice").unwrap().unwrap().texts.into_iter().map(|text| (text.key, text.value)).collect() };

    assert_eq!(keys(&state), [("avatar".into(), "ipfs://x".into()), ("url".into(), "https://b.example".into())], "sorted, one per key");

    act(&state, 1, "alice", text("avatar", None), 0).unwrap();

    assert_eq!(keys(&state).len(), 1);

    for index in 1..MAX_TEXTS {
        act(&state, 1, "alice", text(&format!("k{index}"), Some("v")), 0).unwrap();
    }

    assert!(refused(act(&state, 1, "alice", text("one-more", Some("v")), 0)).contains("at most 16"));
    act(&state, 1, "alice", text("url", Some("replacing one is fine")), 0).unwrap();

    // Given away, its new owner alone changes it; what it points at stays.
    let listed = |owner: u8| state.get(crate::db::Column::Names, &owned_key(&account(owner), "alice")).unwrap().is_some();

    assert!(listed(1) && !listed(2));
    act(&state, 1, "alice", NameAction::Transfer { to: account(2) }, 0).unwrap();
    assert!(!listed(1) && listed(2), "the owners' listings follow it");

    assert!(refused(act(&state, 1, "alice", NameAction::Target { target: None }, 0)).contains("not yours"));
    assert_eq!(names.resolve("alice", 0, 0).unwrap(), Some(account(3)));
    act(&state, 2, "alice", NameAction::Renew { years: 1 }, 0).unwrap();
}

#[test]
fn subnames_are_free_their_owners_change_them_and_the_name_takes_them_back() {
    let state = funded();
    let names = Names::new(&state);

    act(&state, 1, "alice", NameAction::Register { years: 1 }, 0).unwrap();
    let before = balance(&state, 1);

    assert_eq!(act(&state, 1, "alice", NameAction::Subname { label: "pay".into(), owner: account(2) }, 0), Ok(u256::ZERO));
    assert_eq!(balance(&state, 1), before, "free");
    assert_eq!(names.resolve("pay.alice", 0, 0).unwrap(), Some(account(2)));
    assert!(refused(act(&state, 2, "alice", NameAction::Subname { label: "x".into(), owner: account(2) }, 0)).contains("not yours"));

    // Its owner points it, and makes subnames of its own.
    act(&state, 2, "pay.alice", NameAction::Target { target: Some(account(3)) }, 0).unwrap();
    act(&state, 2, "pay.alice", NameAction::Subname { label: "eu".into(), owner: account(3) }, 0).unwrap();

    assert_eq!(names.resolve("pay.alice", 0, 0).unwrap(), Some(account(3)));
    assert_eq!(names.resolve("eu.pay.alice", 0, 0).unwrap(), Some(account(3)));
    assert_eq!(names.lookup("eu.pay.alice", 0, 0).unwrap().unwrap().expires, YEAR_MS, "a subname lasts as long as its name");
    assert!(refused(act(&state, 1, "pay.alice", NameAction::Target { target: None }, 0)).contains("not yours"));

    // The name's owner takes it back by making it again.
    act(&state, 1, "alice", NameAction::Subname { label: "pay".into(), owner: account(1) }, 0).unwrap();

    assert_eq!(names.resolve("pay.alice", 0, 0).unwrap(), Some(account(1)));
    assert!(refused(act(&state, 1, "x.nobody", NameAction::Target { target: None }, 0)).contains("nobody holds"));
    assert!(refused(act(&state, 1, "alice", NameAction::Subname { label: "-x".into(), owner: account(1) }, 0)).contains("hyphen"));
}

#[test]
fn a_primary_name_must_resolve_to_its_account_and_lapses_when_it_stops() {
    let state = funded();
    let names = Names::new(&state);
    let config = config();

    act(&state, 1, "alice", NameAction::Register { years: 1 }, 0).unwrap();

    assert!(matches!(names.set_primary(&account(2), Some("alice"), 0, &config), Err(RegistryError::Name(why)) if why.contains("does not resolve to")));

    names.set_primary(&account(1), Some("alice"), 0, &config).unwrap();

    assert_eq!(names.primary(&account(1), 0, config.grace_ms).unwrap(), Some("alice".to_string()));

    // Pointed elsewhere, it is nobody's primary name until it points back.
    act(&state, 1, "alice", NameAction::Target { target: Some(account(2)) }, 0).unwrap();

    assert_eq!(names.primary(&account(1), 0, config.grace_ms).unwrap(), None);

    act(&state, 1, "alice", NameAction::Target { target: Some(account(1)) }, 0).unwrap();

    assert_eq!(names.primary(&account(1), 0, config.grace_ms).unwrap(), Some("alice".to_string()));
    assert_eq!(names.primary(&account(1), YEAR_MS, config.grace_ms).unwrap(), None, "nor once it expires");

    names.set_primary(&account(1), None, 0, &config).unwrap();

    assert_eq!(names.primary(&account(1), 0, config.grace_ms).unwrap(), None);
}
