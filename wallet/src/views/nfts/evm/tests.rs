use super::*;

const ART: &str = "evm:0x1111111111111111111111111111111111111111";
const ITEMS: &str = "evm:0x2222222222222222222222222222222222222222";

/// An EVM log as /events gives it: its topics, then its data, each bytes.
fn log(topics: &[Vec<u8>], data: Vec<u8>) -> Value {
    let parts: Vec<Value> = topics.iter().chain([&data]).map(|part| json!({ "type": "bytes", "value": to_hex(part) })).collect();

    json!({ "contract": ART, "name": "log", "data": parts })
}

fn padded(form: &[u8; 20]) -> Vec<u8> {
    [vec![0; 12], form.to_vec()].concat()
}

fn listed(address: &str, kind: CollectionKind) -> Listed {
    Listed { address: address.to_string(), kind, name: None, symbol: None }
}

fn read(state: &mut EvmState, what: &str, collection: &str, id: &str, from: &str, output: Vec<u8>, proven: Option<u64>) {
    let body = json!({ "contract": collection, "input": "0x", "from": from, "purpose": "nfts-evm", "collection": collection, "id": id, "what": what }).to_string();
    let checked = match proven {
        Some(height) => json!({ "here": true, "height": height }),
        None => json!({ "here": false }),
    };

    assert!(state.answered(&Ask::Call(body), &Ok(json!({ "status": "returned", "output": to_hex(&output), "checked": checked }))));
}

#[test]
fn the_ids_an_evm_collections_logs_name_are_each_kinds_own() {
    let (me, them) = ([0xab; 20], [0xcd; 20]);
    let transfer = keccak(TRANSFER.as_bytes()).to_vec();
    let single = keccak(TRANSFER_SINGLE.as_bytes()).to_vec();
    let batch = keccak(TRANSFER_BATCH.as_bytes()).to_vec();
    let ids = encode_values(
        &[AbiType::Array(Box::new(AbiType::Uint(256))), AbiType::Array(Box::new(AbiType::Uint(256)))],
        &[AbiValue::Array(vec![AbiValue::Uint(u256::new(8)), AbiValue::Uint(u256::new(9))]), AbiValue::Array(vec![AbiValue::Uint(u256::ONE), AbiValue::Uint(u256::ONE)])],
    )
    .unwrap();
    let events = [
        // An ERC-721's Transfer: the id is its last topic.
        log(&[transfer.clone(), padded(&them), padded(&me), word(u256::new(7))], Vec::new()),
        // An ERC-20's Transfer has three topics, the amount in its data: no id.
        log(&[transfer, padded(&them), padded(&me)], word(u256::new(500))),
        log(&[single, padded(&me), padded(&them), padded(&me)], [word(u256::new(3)), word(u256::new(40))].concat()),
        log(&[batch, padded(&me), padded(&them), padded(&me)], ids),
        // The same id again, and a log of another event: nothing new.
        log(&[keccak(TRANSFER.as_bytes()).to_vec(), padded(&me), padded(&them), word(u256::new(7))], Vec::new()),
        log(&[keccak(b"Approval(address,address,uint256)").to_vec(), padded(&me), padded(&them), word(u256::new(11))], Vec::new()),
    ];

    assert_eq!(ids_in(&events), ["7", "3", "8", "9"]);
}

#[test]
fn logs_go_on_a_page_at_a_time_and_say_when_the_node_keeps_no_index() {
    let mut state = EvmState::default();
    let ask = Ask::Logs { contract: ART.to_string(), topic: topic(&[0xab; 20]), cursor: None };
    let page = json!({ "events": [log(&[keccak(TRANSFER.as_bytes()).to_vec(), padded(&[0; 20]), padded(&[0xab; 20]), word(u256::new(5))], Vec::new())], "next": "0x0102" });

    state.pages.insert((ART.to_string(), topic(&[0xab; 20])), 1);
    let next = state.logs(&ask, &Ok(page.clone()));

    assert!(matches!(&next, Some(Ask::Logs { cursor: Some(cursor), .. }) if cursor == "0x0102"));
    assert_eq!(state.ids[ART], ["5"]);

    // Past the last page it stops, however many the node has.
    state.pages.insert((ART.to_string(), topic(&[0xab; 20])), PAGES);
    assert!(state.logs(&ask, &Ok(page)).is_none());

    let missing = crate::node::Failure { status: 404, message: "this node keeps no event index".to_string() };

    assert!(state.logs(&ask, &Err(missing)).is_none());
    assert!(state.unlisted.as_deref().is_some_and(|why| why.contains("no event index")));
    assert!(!state.pages.contains_key(&(ART.to_string(), topic(&[0xab; 20]))), "asked again on the next look");
}

#[test]
fn reads_are_kept_by_what_they_are_for_and_others_left_alone() {
    let mut state = EvmState::default();
    let uri = encode_values(&[AbiType::String], &[AbiValue::String("ipfs://items/{id}.json".to_string())]).unwrap();
    let royalty = encode_values(&[AbiType::Address, AbiType::Uint(256)], &[AbiValue::Address([0x99; 20]), AbiValue::Uint(u256::new(250))]).unwrap();

    read(&mut state, "owner", ART, "7", "0xab", padded(&[0xab; 20]), Some(9));
    read(&mut state, "uri", ITEMS, "10", "0xab", uri, None);
    read(&mut state, "royalty", ART, "7", "0xab", royalty, Some(9));

    assert_eq!(state.held[&(ART.to_string(), "7".to_string(), String::new())], Read { answer: Ok(padded(&[0xab; 20])), proven: Some(9) });
    assert_eq!(state.uris[&(ITEMS.to_string(), "10".to_string())], format!("ipfs://items/{}.json", "0".repeat(62) + "0a"), "ERC-1155's {{id}} as 64 hex digits");
    assert_eq!(state.royalties[&(ART.to_string(), "7".to_string())], Some((Address::Evm([0x99; 20]).to_string(), u256::new(250))));

    let native = json!({ "contract": "contract:0x01", "function": "tokens_of", "purpose": "nfts" }).to_string();

    assert!(!state.answered(&Ask::Call(native), &Ok(Value::Null)), "a native collection's");
}

#[test]
fn an_evm_token_is_sent_by_safe_transfer_from_from_the_holders_evm_form() {
    let from = Address::Ed25519([1; 32]);
    let to = Address::Evm([0x22; 20]);
    let nft = transfer_input(CollectionKind::Nft, &from, &to, u256::new(7), u256::ONE);
    let multi = transfer_input(CollectionKind::Multi, &from, &to, u256::new(7), u256::new(3));
    let batch = batch_input(&from, &to, &[(u256::new(1), u256::new(2)), (u256::new(3), u256::new(4))]);

    assert_eq!(nft[..4], [0x42, 0x84, 0x2e, 0x0e], "ERC-721's safeTransferFrom(address,address,uint256)");
    assert_eq!(nft[4..36], padded(&evm_address(&from)), "from the holder's EVM form");
    assert_eq!(nft[36..68], padded(&[0x22; 20]));
    assert_eq!(nft[68..], word(u256::new(7)));
    assert_eq!(multi[..4], [0xf2, 0x42, 0x43, 0x2a], "ERC-1155's safeTransferFrom with an amount and data");
    assert_eq!(batch[..4], [0x2e, 0xb2, 0xc2, 0xd6], "safeBatchTransferFrom");

    let read = decode_values(&[AbiType::Address, AbiType::Address, AbiType::Array(Box::new(AbiType::Uint(256))), AbiType::Array(Box::new(AbiType::Uint(256))), AbiType::Bytes], &batch[4..]).unwrap();

    assert_eq!(read[2], AbiValue::Array(vec![AbiValue::Uint(u256::new(1)), AbiValue::Uint(u256::new(3))]));
    assert_eq!(read[3], AbiValue::Array(vec![AbiValue::Uint(u256::new(2)), AbiValue::Uint(u256::new(4))]));
}

#[test]
fn what_the_account_is_proven_to_hold_is_a_card_at_the_address_whose_form_holds_it() {
    let mut book = crate::views::drawn::two_phrases();
    let mut holders = crate::keys::open(&book).unwrap();
    let mut world = crate::views::drawn::following(&holders);
    let collections = [listed(ART, CollectionKind::Nft), listed(ITEMS, CollectionKind::Multi)];
    let mut state = EvmState::default();
    let mut drawn = Vec::new();

    crate::views::drawn::page_in(&mut world, &mut book, &mut holders, (360.0, 740.0), |_, ctx| {
        let forms = forms(ctx);
        let (second, form) = forms[1];
        state.ids.insert(ART.to_string(), vec!["7".to_string(), "8".to_string()]);
        state.ids.insert(ITEMS.to_string(), vec!["3".to_string()]);

        // 7 is the second address's, 8 someone else's; 3 the second's, 40 of it, the node's word.
        read(&mut state, "owner", ART, "7", "", padded(&form), Some(12));
        read(&mut state, "owner", ART, "8", "", padded(&[0xee; 20]), Some(12));
        read(&mut state, "amount", ITEMS, "3", &to_hex(&form), word(u256::new(40)), None);
        read(&mut state, "amount", ITEMS, "3", &to_hex(&forms[0].1), word(u256::ZERO), Some(12));

        drawn = state.cards(ctx, &collections).into_iter().map(|card| (card.id, card.holder == second.to_string(), card.amount, matches!(card.proof, Proof::Rerun(Some(12))))).collect();
    });

    assert_eq!(drawn, [("7".to_string(), true, u256::ONE, true), ("3".to_string(), true, u256::new(40), false)]);
}
