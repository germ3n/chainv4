use {super::*, crate::model::Argument, serde_json::Value};

fn event(name: &str, data: &[&str], height: u64) -> FoundEvent {
    FoundEvent {
        contract: "contract:0x5a".to_string(),
        name: name.to_string(),
        data: data.iter().map(|value| Argument { kind: "string".to_string(), value: Value::String(value.to_string()) }).collect(),
        tx: format!("0x{height:064x}"),
        block: format!("0x{:064x}", height + 1),
        height,
        index: 0,
        position: 0,
    }
}

#[test]
fn a_swaps_events_read_as_its_lock_and_what_came_of_it() {
    let (one, two, three) = (format!("0x{}", "11".repeat(32)), format!("0x{}", "22".repeat(32)), format!("0x{}", "44".repeat(32)));
    let hash = format!("0x{}", "aa".repeat(32));
    // Newest first, as the node gives them. Two swaps share a hash, each by its own id.
    let events = [
        event("Withdrawn", &[&three], 14),
        event("Owed", &[&three, "transfers to you are paused"], 13),
        event("Claimed", &[&three, "0x5e"], 13),
        event("Claimed", &[&one, "0x5e"], 12),
        event("Locked", &[&three, &hash, "ed25519:0x0c", "ed25519:0x0a", "contract:0x07", "1", "1793043200000"], 11),
        event("Locked", &[&two, &hash, "ed25519:0x0b", "ed25519:0x0a", "contract:0x07", "300", "1793043200000"], 11),
        event("Locked", &[&one, "0xbb", "ed25519:0x0b", "ed25519:0x0a", "ed25519:0x00", "500", "1793000000000"], 10),
    ];
    let swaps = swaps(&events);

    assert_eq!(swaps.len(), 3);
    assert_eq!((swaps[0].0.as_str(), swaps[0].1.state, swaps[0].1.amount.as_str(), swaps[0].1.height), (one.as_str(), "claimed", "500", 10));
    assert_eq!((swaps[1].1.state, swaps[1].1.deadline, swaps[1].1.token.as_str(), swaps[1].1.hash.as_str()), ("locked", 1_793_043_200_000, "contract:0x07", hash.as_str()));
    assert_eq!(swaps[1].1.locked_in, format!("0x{:064x}", 11));
    assert_eq!((swaps[2].1.state, swaps[2].1.hash.as_str()), ("claimed", hash.as_str()), "owed, then paid out");
    assert_eq!(super::swaps(&events[1..]).iter().find(|(id, _)| *id == three).unwrap().1.state, "claimed, payout owed");
}

#[test]
fn an_nft_lock_says_which_nft_it_holds() {
    let hash = format!("0x{}", "33".repeat(32));
    let events = [
        event("LockedItem", &[&hash, "nft", "7"], 20),
        event("Locked", &[&hash, "0xaa", "ed25519:0x0b", "ed25519:0x0a", "contract:0xc1", "1", "1793043200000"], 20),
    ];
    let swaps = swaps(&events);

    assert_eq!(swaps[0].1.item, Some(("nft".to_string(), "7".to_string())));
    assert_eq!(swaps[0].1.token, "contract:0xc1", "the collection");
}
