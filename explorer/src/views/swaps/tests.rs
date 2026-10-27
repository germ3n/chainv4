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
    let (one, two) = (format!("0x{}", "11".repeat(32)), format!("0x{}", "22".repeat(32)));
    // Newest first, as the node gives them.
    let events = [
        event("Claimed", &[&one, "0x5e"], 12),
        event("Locked", &[&two, "ed25519:0x0b", "ed25519:0x0a", "contract:0x07", "300", "1793043200000"], 11),
        event("Locked", &[&one, "ed25519:0x0b", "ed25519:0x0a", "ed25519:0x00", "500", "1793000000000"], 10),
    ];
    let swaps = swaps(&events);

    assert_eq!(swaps.len(), 2);
    assert_eq!((swaps[0].0.as_str(), swaps[0].1.state, swaps[0].1.amount.as_str(), swaps[0].1.height), (one.as_str(), "claimed", "500", 10));
    assert_eq!((swaps[1].1.state, swaps[1].1.deadline, swaps[1].1.token.as_str()), ("locked", 1_793_043_200_000, "contract:0x07"));
    assert_eq!(swaps[1].1.locked_in, format!("0x{:064x}", 11));
}
