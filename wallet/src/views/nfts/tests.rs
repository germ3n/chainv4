use super::*;

fn ok(value: Value) -> Result<Value, crate::node::Failure> {
    Ok(value)
}

fn returned(values: &[Value]) -> Result<Value, crate::node::Failure> {
    ok(json!({ "status": "returned", "values": values }))
}

#[test]
fn the_registry_s_list_is_read_and_strangers_left_out() {
    let found = listed(&json!({ "collections": [
        { "address": "contract:0xaa", "kind": "nft", "name": "Art", "symbol": "ART" },
        { "address": "contract:0xbb", "kind": "multi", "name": null, "symbol": "ITM" },
        { "address": "contract:0xcc", "kind": "fungible" },
    ] }));

    assert_eq!(found.len(), 2);
    assert_eq!((found[0].kind, found[0].title()), (CollectionKind::Nft, "Art".to_string()));
    assert_eq!((found[1].kind, found[1].title()), (CollectionKind::Multi, "ITM".to_string()));
}

#[test]
fn what_is_held_is_asked_then_proven_by_kind() {
    let mut state = NftsState { collections: listed(&json!({ "collections": [
        { "address": "contract:0xaa", "kind": "nft" },
        { "address": "contract:0xbb", "kind": "multi" },
    ] })), ..NftsState::default() };
    let ask = |contract: &str, function: &str, first: &str| call_ask(contract, function, json!([{ "type": "address", "value": first }]));
    let id = |id: &str| json!({ "type": "u256", "value": id });

    assert!(state.answered(&ask("contract:0xaa", "tokens_of", "ed25519:0x01"), &returned(&[id("1"), id("4")])));
    assert!(state.answered(&ask("contract:0xbb", "tokens_of", "ed25519:0x01"), &returned(&[id("7")])));
    assert!(state.answered(&call_ask("contract:0xaa", "token_uri", json!([id("4")])), &returned(&[json!({ "type": "string", "value": "ipfs://bafy/4" })])));
    assert!(!state.answered(&Ask::Call(json!({ "purpose": "trade" }).to_string()), &returned(&[])), "another page's");

    let mut wanted = state.wanted();
    wanted.sort_by(|a, b| (&a.collection, &a.id).cmp(&(&b.collection, &b.id)));

    assert_eq!(wanted.iter().map(|held| (held.id.as_str(), held.multi)).collect::<Vec<_>>(), [("1", false), ("4", false), ("7", true)]);
    assert_eq!(state.uris.get(&("contract:0xaa".to_string(), "4".to_string())).map(String::as_str), Some("ipfs://bafy/4"));
}

#[test]
fn bytes_become_metadata_a_picture_or_a_reason() {
    assert!(matches!(read(br#"{"name":"Ape","image":"ipfs://x"}"#, None), Media::Metadata(metadata) if metadata.name.as_deref() == Some("Ape")));
    assert!(matches!(read(b"<svg xmlns='http://www.w3.org/2000/svg'/>", Some("image/svg+xml")), Media::Not(why) if why.contains("SVG")));
    assert!(matches!(read(b"not a picture", None), Media::Not(why) if why.contains("PNG, JPEG or WebP")));

    let mut png = Vec::new();
    image::RgbaImage::from_pixel(3, 2, image::Rgba([9, 8, 7, 255])).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();

    assert!(matches!(read(&png, Some("image/png")), Media::Picture(picture) if picture.size == [3, 2]));
}

#[test]
fn what_was_fetched_lands_on_every_uri_that_led_there() {
    let mut state = NftsState::default();
    state.media.insert("ipfs://bafy/1".to_string(), Media::Asked);
    state.media.insert("https://example.com/2".to_string(), Media::Asked);
    state.fetched("https://gw.example/ipfs/bafy/1", Ok(br#"{"name":"One"}"#.to_vec()), "https://gw.example/ipfs/");
    state.fetched("https://example.com/2", Err("404 Not Found".to_string()), "https://gw.example/ipfs/");

    assert!(matches!(state.media.get("ipfs://bafy/1"), Some(Media::Metadata(metadata)) if metadata.name.as_deref() == Some("One")));
    assert!(matches!(state.media.get("https://example.com/2"), Some(Media::Not(why)) if why == "404 Not Found"));
}

#[test]
fn a_contract_is_sent_to_safely_and_a_key_plainly() {
    let (me, collection) = (Address::Ed25519([1; 32]), Address::Contract([2; 32]));
    let function = |prepared: &Prepared| match &prepared.tx {
        Tx::ContractCall { func, data, .. } => (*func, data.len()),
        other => panic!("{other:?}"),
    };
    let named = |name: &str| Hasher::digest(HashType::Xxh3_64, name.as_bytes());

    let to_key = transfer(me, collection, CollectionKind::Nft, "4", Address::Ed25519([3; 32]), u256::from(1u64));
    let to_contract = transfer(me, collection, CollectionKind::Multi, "7", Address::Contract([4; 32]), u256::from(5u64));

    assert_eq!(function(&to_key), (named("transfer"), 2));
    assert_eq!(function(&to_contract), (named("safe_transfer"), 3));
    assert_eq!((to_key.what.as_str(), to_contract.what.as_str()), ("Send NFT #4", "Send 5 of #7"));
    assert_eq!(to_key.coin, u256::ZERO);
}
