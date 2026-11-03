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
    let word = |crossing: &Crossing, threshold: u64| crossing.standing(Some(threshold)).word();

    assert_eq!(found.iter().map(|crossing| crossing.digest.as_str()).collect::<Vec<_>>(), [arriving.as_str(), back.as_str(), out.as_str()], "newest first");
    assert_eq!(found[2].way, Way::Out { nonce: 1, target: Some(8453) });
    assert_eq!(found[2].signers.len(), 1, "a signer counts once");
    assert_eq!((word(&found[2], 2), word(&found[2], 1)), ("signing, 1 of 2".to_string(), "signed, to deliver".to_string()));
    assert_eq!(found[1].way, Way::In { source: 8453, nonce: 4 });
    assert_eq!((word(&found[1], 2), found[1].done_in.as_deref(), found[1].height), ("arrived".to_string(), Some(format!("0x{:064x}", 13).as_str()), 12));
    assert_eq!((&found[0].way, word(&found[0], 2)), (&Way::Arriving, "signing, 1 of 2".to_string()));
}

/// Every order a list of events can come in.
fn orders(events: &[FoundEvent]) -> Vec<Vec<FoundEvent>> {
    match events {
        [] => vec![Vec::new()],
        _ => (0..events.len())
            .flat_map(|first| {
                let rest: Vec<FoundEvent> = events.iter().enumerate().filter(|(at, _)| *at != first).map(|(_, event)| event.clone()).collect();

                orders(&rest).into_iter().map(move |mut order| {
                    order.insert(0, events[first].clone());
                    order
                })
            })
            .collect(),
    }
}

fn address(value: &str) -> Argument {
    Argument { kind: "address".to_string(), value: Value::String(value.to_string()) }
}

/// The digests of a transfer out, and of its void and refund, and two signers' keys.
struct Digests {
    out: String,
    void: String,
    refund: String,
    one: String,
    two: String,
}

fn digests() -> Digests {
    Digests {
        out: format!("0x{}", "aa".repeat(32)),
        void: format!("0x{}", "a1".repeat(32)),
        refund: format!("0x{}", "a2".repeat(32)),
        one: format!("0x02{}", "11".repeat(32)),
        two: format!("0x03{}", "22".repeat(32)),
    }
}

/// What the gateway, asked, says the void and refund digests are for.
fn over(digests: &Digests) -> HashMap<String, (String, Over)> {
    HashMap::from([(digests.void.clone(), (digests.out.clone(), Over::Void)), (digests.refund.clone(), (digests.out.clone(), Over::Refund))])
}

#[test]
fn a_transfer_out_reads_as_signed_then_voiding_then_voided_then_refunded() {
    let d = digests();
    // As it happened, oldest first: sent, signed, signed to void there, signed to refund here, paid back.
    let story = [
        event("Sent", vec![int(1), int(8453), bytes(&d.out), address("ed25519:0x0a")], 10),
        event("Attested", vec![bytes(&d.out), bytes(&d.one)], 11),
        event("Attested", vec![bytes(&d.out), bytes(&d.two)], 12),
        event("Attested", vec![bytes(&d.void), bytes(&d.one)], 13),
        event("Attested", vec![bytes(&d.void), bytes(&d.two)], 14),
        event("Attested", vec![bytes(&d.refund), bytes(&d.one)], 15),
        event("Attested", vec![bytes(&d.refund), bytes(&d.two)], 16),
        event("Refunded", vec![int(1), bytes(&d.out), address("ed25519:0x0a")], 16),
    ];
    // What the first so many of them come to, given newest first as the node gives them.
    let so_far = |many: usize| place(crossings(&story[..many].iter().rev().cloned().collect::<Vec<_>>()), &over(&d), &HashSet::new());
    let standing = |many: usize| {
        let found = so_far(many);
        assert_eq!(found.len(), 1, "signatures to void or refund it are no transfer of their own: {found:?}");

        found[0].standing(Some(2)).word()
    };

    assert_eq!(standing(3), "signed, to deliver");
    assert_eq!(standing(4), "voiding, 1 of 2");
    assert_eq!(standing(5), "void signed, to deliver");
    assert_eq!(standing(6), "voided, refund to come", "its signers sign a refund only once they read it voided there");
    assert_eq!(standing(8), "refunded");

    let found = so_far(8);
    assert_eq!((found[0].void_signers.len(), found[0].refund_signers.len(), found[0].signers.len()), (2, 2, 2));
    assert_eq!(found[0].refunded, Some((format!("0x{:064x}", 16), "ed25519:0x0a".to_string())));
    assert_eq!((found[0].latest_tx(), &found[0].tx), (format!("0x{:064x}", 16).as_str(), &format!("0x{:064x}", 10)), "where it showed is its Sent");
}

#[test]
fn a_transfers_events_in_any_order_come_to_the_same_standing() {
    let d = digests();
    let out = [
        event("Sent", vec![int(1), int(8453), bytes(&d.out), address("ed25519:0x0a")], 10),
        event("Attested", vec![bytes(&d.out), bytes(&d.one)], 11),
        event("Attested", vec![bytes(&d.void), bytes(&d.one)], 12),
        event("Attested", vec![bytes(&d.refund), bytes(&d.one)], 13),
        event("Attested", vec![bytes(&d.refund), bytes(&d.two)], 14),
        event("Refunded", vec![int(1), bytes(&d.out), address("ed25519:0x0a")], 14),
    ];

    for order in orders(&out) {
        let found = place(crossings(&order), &over(&d), &HashSet::new());

        assert_eq!(found.len(), 1, "{order:?}");
        assert_eq!((found[0].standing(Some(2)), &found[0].way, found[0].height), (Standing::Refunded, &Way::Out { nonce: 1, target: Some(8453) }, 10), "{order:?}");
        assert_eq!((found[0].void_signers.len(), found[0].refund_signers.len()), (1, 2));
    }

    // One on its way here, voided once enough signed: its signatures to deliver it stop counting.
    let back = format!("0x{}", "bb".repeat(32));
    let into = [event("Attested", vec![bytes(&back), bytes(&d.one)], 20), event("Attested", vec![bytes(&d.void), bytes(&d.one)], 21), event("Voided", vec![int(8453), int(4), bytes(&back)], 22)];
    let over = HashMap::from([(d.void.clone(), (back.clone(), Over::Void))]);

    for order in orders(&into) {
        let found = place(crossings(&order), &over, &HashSet::new());

        assert_eq!(found.len(), 1, "{order:?}");
        assert_eq!((found[0].standing(Some(2)).word(), &found[0].way), ("voided, refund to come".to_string(), &Way::In { source: 8453, nonce: 4 }), "{order:?}");
        assert_eq!(found[0].latest_tx(), format!("0x{:064x}", 22));
    }

    // A refund older than the Sent the events reach still says it left, and was paid back.
    let found = crossings(&[event("Refunded", vec![int(7), bytes(&d.out), address("ed25519:0x0a")], 30)]);
    assert_eq!((&found[0].way, found[0].standing(Some(2))), (&Way::Out { nonce: 7, target: None }, Standing::Refunded));
}

#[test]
fn signatures_no_lookup_places_stay_as_an_arriving_transfer() {
    let d = digests();
    let events = [event("Attested", vec![bytes(&d.void), bytes(&d.one)], 12), event("Sent", vec![int(1), int(8453), bytes(&d.out), address("ed25519:0x0a")], 10)];
    let found = place(crossings(&events), &HashMap::new(), &HashSet::new());

    assert_eq!(found.len(), 2);
    assert_eq!(found[0].way, Way::Arriving);

    // Nor does a digest placed onto a transfer the events do not hold go anywhere.
    let found = place(crossings(&events[..1]), &over(&d), &HashSet::new());
    assert_eq!((found.len(), &found[0].way), (1, &Way::Arriving));
}

#[test]
fn a_transfer_that_left_for_a_peer_since_replaced_is_stuck() {
    let d = digests();
    let events = [event("Attested", vec![bytes(&d.out), bytes(&d.one)], 11), event("Sent", vec![int(1), int(8453), bytes(&d.out), address("ed25519:0x0a")], 10)];
    let found = place(crossings(&events), &HashMap::new(), &HashSet::from([d.out.clone()]));

    assert_eq!(found[0].standing(Some(2)).word(), "stuck: its peer is gone");
    assert!(standing_note(&found[0], found[0].standing(Some(2)), chainv4::swap::evm::chain_name).is_some_and(|note| note.contains("unless that peer is named again")));
}

#[test]
fn the_gateways_events_read_in_words() {
    let d = digests();
    let chain = |id: u64| format!("chain {id}");
    let said = |name: &str, data: Vec<Argument>| event_words(name, &data, chain);

    assert_eq!(said("Sent", vec![int(1), int(8453), bytes(&d.out), address("ed25519:0x0a")]), Some(("transfer #1 left for chain 8453, sent by".to_string(), Some("ed25519:0x0a".to_string()), d.out.clone())));
    assert_eq!(said("Voided", vec![int(8453), int(4), bytes(&d.out)]), Some(("transfer #4 from chain 8453 voided, refused here for good: it is refunded where it left".to_string(), None, d.out.clone())));
    assert_eq!(said("Refunded", vec![int(1), bytes(&d.out), address("ed25519:0x0a")]), Some(("transfer #1 paid back, to".to_string(), Some("ed25519:0x0a".to_string()), d.out.clone())));
    assert_eq!(said("Received", vec![int(8453), int(4), bytes(&d.out)]).map(|(words, ..)| words), Some("transfer #4 from chain 8453 arrived and was done".to_string()));
    assert_eq!(said("Attested", vec![bytes(&d.void), bytes(&d.one)]).map(|(_, _, digest)| digest), Some(d.void.clone()), "a signature links what it signs, which opens the transfer it is for");
    assert_eq!(said("Wrapped", vec![]), None);
}

mod page {
    use {
        super::*,
        crate::api::Api,
        crate::light::Light,
        crate::model::EventView,
        crate::store::{EventList, Store},
        crate::views::{Look, Settings},
        chainv4::bridge::{AssetKind, Transfer},
        chainv4::structs::u256::u256,
    };

    const HERE: u64 = 1337;
    const THERE: u64 = 8453;
    /// The Gateway.sol the directory names there now, and the one it named before.
    const PEER: [u8; 20] = [0xbb; 20];
    const GONE: [u8; 20] = [0xaa; 20];

    fn gateway() -> String {
        Address::Contract([6; 32]).to_string()
    }

    fn sender() -> Address {
        Address::Ed25519([1; 32])
    }

    /// 250 of a token out, as the gateway here wrote it, toward a Gateway.sol there.
    fn transfer(nonce: u64, peer: [u8; 20]) -> Transfer {
        Transfer {
            from: [&Address::Contract([6; 32]).to_abi()[..], b"1800000000000"].concat(),
            to: peer.to_vec(),
            source: HERE,
            target: THERE,
            nonce,
            kind: AssetKind::Token,
            origin: HERE,
            asset: Address::Contract([9; 32]).to_abi().to_vec(),
            id: u256::ZERO,
            amount: u256::from(250u64),
            recipient: vec![0xb0; 20],
            name: "Dev Token".into(),
            symbol: "DVT".into(),
            decimals: 6,
            uri: String::new(),
        }
    }

    fn call(function: &str, args: Value) -> Ask {
        Ask::Call { contract: gateway(), function: function.to_string(), args: args.to_string() }
    }

    fn returned(values: Vec<Value>) -> Value {
        json!({ "status": "returned", "values": values })
    }

    /// What sent(nonce) gives for a transfer.
    fn sent(transfer: &Transfer) -> Value {
        let typed = |kind: &str, value: Value| json!({ "type": kind, "value": value });

        returned(vec![
            typed("i64", json!(transfer.target)),
            typed("i64", json!(1)),
            typed("i64", json!(transfer.origin)),
            typed("bytes", json!(to_hex(&transfer.asset))),
            typed("u256", json!("0")),
            typed("u256", json!("250")),
            typed("bytes", json!(to_hex(&transfer.recipient))),
            typed("string", json!("Dev Token")),
            typed("string", json!("DVT")),
            typed("i64", json!(6)),
            typed("string", json!("")),
            typed("bytes", json!(to_hex(&transfer.digest()))),
            typed("address", json!(sender().to_string())),
        ])
    }

    /// A chain whose gateway sent #1, voided there and refunded here, and #2 for a peer since
    /// replaced, and voided one coming from there.
    fn store() -> (Store, String, String) {
        let (refunded, stuck) = (transfer(1, PEER), transfer(2, GONE));
        let (d1, d2) = (to_hex(&refunded.digest()), to_hex(&stuck.digest()));
        let (void, refund) = (to_hex(&refunded.void_digest()), to_hex(&refunded.refund_digest(&sender().to_abi())));
        let (back, void_back) = (format!("0x{}", "cc".repeat(32)), format!("0x{}", "c1".repeat(32)));
        let (one, two) = (format!("0x02{}", "11".repeat(32)), format!("0x03{}", "22".repeat(32)));
        let who = address(&sender().to_string());
        let events = vec![
            event("Voided", vec![int(THERE), int(4), bytes(&back)], 31),
            event("Attested", vec![bytes(&void_back), bytes(&two)], 31),
            event("Attested", vec![bytes(&void_back), bytes(&one)], 30),
            event("Attested", vec![bytes(&back), bytes(&one)], 29),
            event("Attested", vec![bytes(&d2), bytes(&one)], 21),
            event("Sent", vec![int(2), int(THERE), bytes(&d2), who.clone()], 20),
            event("Refunded", vec![int(1), bytes(&d1), who.clone()], 16),
            event("Attested", vec![bytes(&refund), bytes(&two)], 16),
            event("Attested", vec![bytes(&refund), bytes(&one)], 15),
            event("Attested", vec![bytes(&void), bytes(&two)], 14),
            event("Attested", vec![bytes(&void), bytes(&one)], 13),
            event("Attested", vec![bytes(&d1), bytes(&two)], 12),
            event("Attested", vec![bytes(&d1), bytes(&one)], 11),
            event("Sent", vec![int(1), int(THERE), bytes(&d1), who], 10),
        ]
        .into_iter()
        .map(|event| FoundEvent { contract: gateway(), ..event })
        .collect();
        let status = json!({
            "genesis": "0x00", "tip": "0x00", "height": 40, "timestamp": 0, "difficulty": "0", "work": "1",
            "hash": "blake3", "coin": { "symbol": "DEV", "decimals": 18 }, "finality": null, "fees": null, "chain_id": HERE,
            "federation": { "version": 1, "threshold": 2, "signers": [one, two], "chain_depth": 3, "evm_depth": 12, "bitcoin_depth": 6 },
            "directory": { "gateway": gateway(), "btc_gateway": null, "evm_gateways": [{ "chain_id": THERE, "gateway": to_hex(&PEER) }] },
        });
        let mut store = Store { status: Some(serde_json::from_value(status).unwrap()), ..Store::default() };
        let mut list = EventList::default();
        (list.items, list.refreshed) = (events, 2_000);
        store.events.insert(EventQuery::contract(&gateway(), None).limited(EVENTS), list);
        let identity = json!({ "type": "bytes", "value": to_hex(&refunded.from) });
        store.calls.insert(call("identity", json!([])), returned(vec![identity]));
        store.calls.insert(call("sent", json!([whole(1)])), sent(&refunded));
        store.calls.insert(call("sent", json!([whole(2)])), sent(&stuck));
        let asked = json!([whole(THERE), whole(HERE), whole(4), { "type": "bytes", "value": back }]);
        store.calls.insert(call("void_digest", asked), returned(vec![json!({ "type": "bytes", "value": void_back })]));

        (store, d1, d2)
    }

    /// A page drawn on a screen of a size, a few frames, as it settles: every text drawn and where.
    fn drawn_at(store: &Store, width: f32, height: f32, mut draw: impl FnMut(&mut Ui, &mut Ctx, &mut BridgeState)) -> Vec<(String, egui::Rect)> {
        let mut api = Api::new("http://127.0.0.1:1", Err("offline".to_string()), egui::Context::default());
        let (mut light, mut settings, mut go, mut state) = (Light::default(), Settings::default(), None, BridgeState::default());
        let palette = crate::theme::DARK;
        let look = Look { palette, roles: crate::theme::Roles::of(&palette), narrow: chainv4_ui::layout::is_narrow(width) };
        let reader = chainv4::abi::known::AbiReader::default();
        let context = egui::Context::default();
        let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, height))), ..Default::default() };
        let mut shown = Vec::new();

        for _ in 0..3 {
            let mut output = context.run_ui(input.clone(), |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    let mut reading = crate::reading::Reading::default();
                    let mut ctx = Ctx { store, reader: &reader, light: &mut light, reading: &mut reading, api: &mut api, go: &mut go, settings: &mut settings, now: 2_000, look };
                    draw(ui, &mut ctx, &mut state);
                });
            });
            output.textures_delta.clear();
            shown = output
                .shapes
                .iter()
                .filter_map(|clipped| match &clipped.shape {
                    egui::epaint::Shape::Text(text) => Some((text.galley.text().to_string(), text.visual_bounding_rect())),
                    _ => None,
                })
                .collect();
        }

        shown
    }

    #[test]
    fn the_bridge_page_shows_each_transfers_true_state_on_a_phone_upright_and_on_its_side_and_a_desktop() {
        let (store, ..) = store();

        for (width, height) in [(360.0, 740.0), (740.0, 360.0), (1200.0, 800.0)] {
            // The screen, and the whole page scrolled through, as only what is in view is drawn.
            for tall in [height, 3_000.0] {
                let shown = drawn_at(&store, width, tall, show);

                for (text, rect) in shown.iter().filter(|(text, _)| !text.trim().is_empty()) {
                    assert!(rect.right() <= width + 1.0, "{text:?} at {rect:?} runs past {width}x{height}");
                }
            }

            let shown = drawn_at(&store, width, 3_000.0, show);
            let texts: Vec<&str> = shown.iter().map(|(text, _)| text.as_str()).collect();

            for state in ["refunded", "stuck: its peer is gone", "voided, refund to come"] {
                assert_eq!(texts.iter().filter(|text| **text == state).count(), 1, "{state} at {width}x{height}: {texts:?}");
            }

            assert!(!texts.iter().any(|text| text.starts_with("signing")), "signatures to void or refund show as no transfer of their own: {texts:?}");
        }
    }

    #[test]
    fn a_transfer_whose_peer_is_gone_says_it_is_stuck_and_why_on_its_page() {
        let (store, d1, d2) = store();
        let shown = drawn_at(&store, 360.0, 740.0, |ui, ctx, state| super::super::transfer(ui, ctx, state, &d2));
        let texts: Vec<&str> = shown.iter().map(|(text, _)| text.as_str()).collect();

        assert!(texts.contains(&"stuck: its peer is gone"), "{texts:?}");
        assert!(texts.iter().any(|text| text.contains("unless that peer is named again")), "{texts:?}");

        let shown = drawn_at(&store, 1200.0, 800.0, |ui, ctx, state| super::super::transfer(ui, ctx, state, &d1));
        let texts: Vec<&str> = shown.iter().map(|(text, _)| text.as_str()).collect();

        for said in ["refunded", "refunded in", "refunded to", "To void it · 2 of 2", "To refund it · 2 of 2"] {
            assert!(texts.iter().any(|text| text.eq_ignore_ascii_case(said)), "{said}: {texts:?}");
        }

        // Its refund's digest, as a signature's event links it, opens it too.
        let refund = to_hex(&transfer(1, PEER).refund_digest(&sender().to_abi()));
        let shown = drawn_at(&store, 1200.0, 800.0, |ui, ctx, state| super::super::transfer(ui, ctx, state, &refund));
        assert!(shown.iter().any(|(text, _)| text == "refunded"), "{shown:?}");
    }

    #[test]
    fn the_transaction_and_account_pages_say_the_gateways_events_in_words() {
        let (store, d1, _) = store();
        let events = [
            EventView { contract: gateway(), name: "Refunded".to_string(), data: vec![int(1), bytes(&d1), address(&sender().to_string())] },
            EventView { contract: gateway(), name: "Voided".to_string(), data: vec![int(THERE), int(4), bytes(&d1)] },
            EventView { contract: "contract:0x77".to_string(), name: "Voided".to_string(), data: vec![int(THERE), int(4), bytes(&d1)] },
        ];

        for width in [360.0, 1200.0] {
            let shown = drawn_at(&store, width, 800.0, |ui, ctx, _| crate::views::tx::events(ui, ctx, &events));
            let texts: Vec<&str> = shown.iter().map(|(text, _)| text.as_str()).collect();

            assert!(texts.contains(&"transfer #1 paid back, to"), "{texts:?}");
            assert_eq!(texts.iter().filter(|text| text.contains("voided, refused here for good")).count(), 1, "only the gateway's own reads so: {texts:?}");
        }
    }
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
