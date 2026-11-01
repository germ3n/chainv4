use {super::*, crate::model::Argument, serde_json::Value};

fn bytes(hex: &str) -> Argument {
    Argument { kind: "bytes".to_string(), value: Value::String(hex.to_string()) }
}

fn int(value: u64) -> Argument {
    Argument { kind: "i64".to_string(), value: json!(value) }
}

fn text(value: &str) -> Argument {
    Argument { kind: "string".to_string(), value: Value::String(value.to_string()) }
}

fn event(name: &str, data: Vec<Argument>, height: u64) -> FoundEvent {
    FoundEvent {
        contract: "contract:0x6a".to_string(),
        name: name.to_string(),
        data,
        tx: format!("0x{height:064x}"),
        block: format!("0x{:064x}", height + 1),
        height,
        index: 0,
        position: 0,
    }
}

#[test]
fn transfers_read_from_the_gateways_events_each_way() {
    let (out, back, arriving) = (format!("0x{}", "aa".repeat(32)), format!("0x{}", "bb".repeat(32)), format!("0x{}", "cc".repeat(32)));
    let (one, two) = (format!("0x02{}", "11".repeat(32)), format!("0x03{}", "22".repeat(32)));
    // Newest first, as the node gives them.
    let events = [
        event("Attested", vec![bytes(&arriving), bytes(&one)], 14),
        event("Received", vec![int(8453), int(4), bytes(&back)], 13),
        event("Attested", vec![bytes(&back), bytes(&two)], 13),
        event("Attested", vec![bytes(&back), bytes(&one)], 12),
        event("Attested", vec![bytes(&out), bytes(&one)], 11),
        event("Attested", vec![bytes(&out), bytes(&one)], 11),
        event("Sent", vec![int(1), int(8453), bytes(&out), text("ed25519:0x0a")], 10),
    ];
    let found = crossings(&events);

    assert_eq!(found.iter().map(|crossing| crossing.digest.as_str()).collect::<Vec<_>>(), [arriving.as_str(), back.as_str(), out.as_str()], "newest first");
    assert_eq!(found[2].way, Way::Out { nonce: 1, target: 8453 });
    assert_eq!(found[2].signers.len(), 1, "a signer counts once");
    assert_eq!((found[2].state(Some(2)), found[2].state(Some(1))), ("signing, 1 of 2".to_string(), "signed, to relay".to_string()));
    assert_eq!(found[1].way, Way::In { source: 8453, nonce: 4 });
    assert_eq!((found[1].state(Some(2)), found[1].done_in.as_deref(), found[1].height), ("received".to_string(), Some(format!("0x{:064x}", 13).as_str()), 12));
    assert_eq!((&found[0].way, found[0].state(Some(2))), (&Way::Arriving, "signing, 1 of 2".to_string()));
}

#[test]
fn wrapped_assets_say_what_they_stand_for() {
    let usdt = format!("0x{}", "dd".repeat(20));
    let events = [
        event("Sent", vec![int(1), int(1), bytes(&format!("0x{}", "aa".repeat(32)))], 9),
        event("Wrapped", vec![text("1"), int(1), bytes(&usdt), text("contract:0x77")], 8),
        event("Wrapped", vec![text("1"), int(0), text(""), text("contract:0x76")], 7),
    ];
    let made = wrapped(&events);

    assert_eq!(made.len(), 2);
    assert_eq!((made[0].origin, kind_name(made[0].kind), party(&made[0].asset), made[0].here.as_str()), (1, "token", usdt.clone(), "contract:0x77"));
    assert_eq!((kind_name(made[1].kind), party(&made[1].asset)), ("coin", String::new()), "the coin has no address");
}

#[test]
fn bytes_that_came_back_as_text_read_as_their_bytes() {
    assert_eq!(bytes_hex(&bytes("0xABcd")), "0xabcd");
    assert_eq!(bytes_hex(&text("hi")), "0x6869");
    assert_eq!(bytes_hex(&text("")), "0x");

    // 33 bytes are an address here, 20 one on an EVM chain.
    let here = chainv4::structs::address::Address::Contract([5; 32]);
    assert_eq!(party(&to_hex(&here.to_abi())), here.to_string());
    assert_eq!(party(&format!("0x{}", "0e".repeat(20))), format!("0x{}", "0e".repeat(20)));
}

#[test]
fn bitcoin_hashes_show_as_explorers_write_them() {
    let internal = format!("0x01{}ff", "00".repeat(30));

    assert_eq!(shown_hash(&internal), format!("ff{}01", "00".repeat(30)));
    assert_eq!(outpoint(&format!("01{}ff:3", "00".repeat(30))), format!("ff{}01:3", "00".repeat(30)));
    assert_eq!(outpoint("not one"), "not one");
}

#[test]
fn an_inputs_signatures_count_against_its_threshold() {
    let values = |signed: usize| ["0x52ae".to_string(), "2".to_string()].into_iter().chain((0..signed).flat_map(|at| [at.to_string(), "0x5151".to_string()])).collect::<Vec<_>>();

    assert_eq!(signed_input(&values(0)), Some((0, 2)));
    assert_eq!(signed_input(&values(2)), Some((2, 2)));
    assert_eq!(signed_input(&["0x52ae".to_string()]), None);
}

#[test]
fn a_release_and_a_held_output_read_as_the_gateway_gives_them() {
    let release = release_from(&[bytes("0x0200"), bytes(&format!("0x01{}", "00".repeat(31))), text("3,4"), int(1), text("open")]).unwrap();

    assert_eq!(release.raw, vec![2, 0]);
    assert_eq!(release.txid, format!("{}01", "00".repeat(31)));
    assert_eq!((release.withdrawals, release.version, release.state.as_str()), (vec![3, 4], 1, "open"));
    assert_eq!(release_from(&[bytes("0x0200"), bytes("0x00"), text(""), int(0), text("open")]).unwrap().withdrawals, Vec::<u64>::new(), "a sweep pays none");

    let held = held_from(&[text(&format!("01{}:0", "00".repeat(31))), int(50_000), int(0), text(""), text("unspent")]).unwrap();

    assert_eq!((held.outpoint, held.sats, held.state.as_str()), (format!("{}01:0", "00".repeat(31)), 50_000, "unspent"));
    assert_eq!(party(&held.recipient), "", "change is no one's deposit");
}

#[test]
fn a_withdrawal_says_what_it_may_lose_and_then_what_it_lost() {
    let said = |most: &str, share: &str| ["0x00", "20000", "queued", "ed25519:0x", most, share].map(str::to_string);

    assert_eq!(withdrawal_fee(&said("2000", "")), format!("up to {}", btc("2000")));
    assert_eq!(withdrawal_fee(&said("2000", "500")), btc("500"));
    assert_eq!(withdrawal_fee(&["0x00".to_string()]), "—");
}
