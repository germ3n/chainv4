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
    ] })), addresses: vec!["ed25519:0x01".to_string()], ..NftsState::default() };
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
    assert!(matches!(read(b"<svg xmlns='http://www.w3.org/2000/svg'/>", Some("image/svg+xml")), Media::Picture(_)), "one of no size takes SVG's own 100 by 100");
    assert!(matches!(read(b"<svg not closed", Some("image/svg+xml")), Media::Not(why) if why.contains("SVG")));
    assert!(matches!(read(b"not a picture", None), Media::Not(why) if why.contains("PNG, JPEG, WebP, GIF or SVG")));

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

#[test]
fn the_tiles_of_a_row_share_one_top_and_one_height_whatever_they_are_called() {
    let collection = |name: &str| Listed { address: format!("contract:{name}"), kind: CollectionKind::Nft, name: Some(name.to_string()), symbol: None };
    let assurance = chainv4::light::Assurance { block: chainv4::structs::hash::Hash::zero(HashType::default_chain()), height: 1, final_: true };
    let card = |listed: Listed, id: &str| Card { listed, id: id.to_string(), holder: "ed25519:0x01".to_string(), amount: u256::from(1u64), proof: Proof::Light(assurance) };
    let cards = [
        card(collection("Art"), "1"),
        card(collection("A collection whose name runs on far longer than any tile has room for on a phone"), "12345678901234567890"),
    ];
    let ctx = egui::Context::default();
    crate::theme::apply(&ctx, &crate::theme::DARK);
    let mut state = NftsState::default();
    let mut tiles = Vec::new();

    // A frame to settle fonts, then the one measured.
    for _ in 0..2 {
        tiles.clear();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.horizontal_top(|ui| {
                for card in &cards {
                    ui.scope(|ui| {
                        let before = ui.cursor().min;
                        tile(ui, &crate::theme::DARK, &mut state, card, SIDE);
                        tiles.push(egui::Rect::from_min_max(before, ui.min_rect().max));
                    });
                }
            });
        });
        output.textures_delta.clear();
    }

    assert_eq!(tiles.len(), 2);
    assert!((tiles[0].top() - tiles[1].top()).abs() < 0.5, "one top line: {tiles:?}");
    assert!((tiles[0].height() - tiles[1].height()).abs() < 0.5, "one height: {tiles:?}");
    assert!((tiles[0].width() - SIDE).abs() < 0.5 && (tiles[1].width() - SIDE).abs() < 0.5, "a long name stays in its tile: {tiles:?}");
}

const COLLECTION: &str = "contract:0xc5d33df18482280dbf43177286f3e395e7d7493efe180ff1c7b5707ea9811c7b";

fn tokens_of(holder: &str) -> Ask {
    call_ask(COLLECTION, "tokens_of", json!([{ "type": "address", "value": holder }, { "type": "u256", "value": "0" }, { "type": "u256", "value": PAGE.to_string() }]))
}

fn proven<T>(value: T, height: u64) -> crate::chain::Proven<T> {
    crate::chain::Proven { value, assurance: chainv4::light::Assurance { block: chainv4::structs::hash::Hash::zero(HashType::default_chain()), height, final_: true } }
}

/// Each card the page would draw, by the address it is held at, for the test words' account.
fn drawn_cards(world: &mut crate::views::drawn::World, state: &mut NftsState) -> Vec<(String, String)> {
    let mut book = crate::views::drawn::two_phrases();
    let mut holders = crate::keys::open(&book).unwrap();
    let mut found = Vec::new();
    crate::views::drawn::page_in(world, &mut book, &mut holders, (360.0, 740.0), |_, ctx| {
        state.addresses = ctx.addresses();
        found = cards(ctx, state).into_iter().map(|card| (card.id, card.holder)).collect();
    });

    found
}

fn account() -> Vec<Address> {
    crate::keys::open(&crate::views::drawn::two_phrases()).unwrap()[0].addresses()
}

#[test]
fn an_nft_is_shown_at_whichever_of_the_account_s_addresses_holds_it_whatever_its_scheme() {
    let addresses = account();
    let holders = crate::keys::open(&crate::views::drawn::two_phrases()).unwrap();

    assert_eq!(addresses.iter().map(|address| crate::views::scheme(address)).collect::<Vec<_>>().len(), 4, "ed25519, edblake, secp256k1 and bitcoin");

    let mut state = NftsState { collections: listed(&json!({ "collections": [{ "address": COLLECTION, "kind": "nft" }] })), ..NftsState::default() };
    let mut world = crate::views::drawn::following(&holders);

    for (n, address) in addresses.iter().enumerate() {
        let id = (n + 1).to_string();
        assert!(state.answered(&tokens_of(&address.to_string()), &returned(&[json!({ "type": "u256", "value": id })])));
        world.chain.nft_owners.insert((COLLECTION.to_string(), id), proven(Some(*address), 1));
    }

    let found = drawn_cards(&mut world, &mut state);

    assert_eq!(found, addresses.iter().enumerate().map(|(n, address)| ((n + 1).to_string(), address.to_string())).collect::<Vec<_>>());
}

#[test]
fn an_nft_sent_to_another_of_the_account_s_addresses_stays_on_the_page_before_its_list_says_so() {
    let addresses = account();
    let holders = crate::keys::open(&crate::views::drawn::two_phrases()).unwrap();
    let (ed25519, bitcoin) = (addresses[0], addresses[3]);
    let mut state = NftsState { collections: listed(&json!({ "collections": [{ "address": COLLECTION, "kind": "nft" }] })), ..NftsState::default() };
    let mut world = crate::views::drawn::following(&holders);

    // Listed at the Ed25519 address, then sent to the Bitcoin one: the proof says so first, and the
    // Bitcoin address's list has not come, refused as too many asked at once.
    assert!(state.answered(&tokens_of(&ed25519.to_string()), &returned(&[json!({ "type": "u256", "value": "9" })])));
    assert!(state.answered(&tokens_of(&bitcoin.to_string()), &Err(crate::node::Failure { status: 429, message: "too many requests from this address".to_string() })));
    world.chain.nft_owners.insert((COLLECTION.to_string(), "9".to_string()), proven(Some(bitcoin), 5));

    assert_eq!(drawn_cards(&mut world, &mut state), [("9".to_string(), bitcoin.to_string())], "at the Bitcoin address");
    assert!(state.unasked.contains_key(&bitcoin.to_string()), "and the page says whose list did not come");

    // The Ed25519 address's list no longer has it: followed at the Bitcoin one, it stays.
    let mut found = drawn_cards(&mut world, &mut state);
    state.follow(&found.iter().map(|(id, holder)| Card { listed: state.collections[0].clone(), id: id.clone(), holder: holder.clone(), amount: u256::from(1u64), proof: Proof::Light(proven((), 5).assurance) }).collect::<Vec<_>>());
    assert!(state.answered(&tokens_of(&ed25519.to_string()), &returned(&[])));
    found = drawn_cards(&mut world, &mut state);

    assert_eq!(found, [("9".to_string(), bitcoin.to_string())]);

    // Once the Bitcoin address's own list answers, it is that list that counts.
    assert!(state.answered(&tokens_of(&bitcoin.to_string()), &returned(&[json!({ "type": "u256", "value": "9" })])));
    assert!(state.unasked.is_empty());
    assert_eq!(drawn_cards(&mut world, &mut state), [("9".to_string(), bitcoin.to_string())]);
}

#[test]
fn an_nft_proven_at_an_address_of_no_account_here_is_not_shown() {
    let holders = crate::keys::open(&crate::views::drawn::two_phrases()).unwrap();
    let ed25519 = account()[0];
    let mut state = NftsState { collections: listed(&json!({ "collections": [{ "address": COLLECTION, "kind": "nft" }] })), ..NftsState::default() };
    let mut world = crate::views::drawn::following(&holders);
    assert!(state.answered(&tokens_of(&ed25519.to_string()), &returned(&[json!({ "type": "u256", "value": "3" })])));
    world.chain.nft_owners.insert((COLLECTION.to_string(), "3".to_string()), proven(Some(Address::Bitcoin([1; 20])), 5));

    assert!(drawn_cards(&mut world, &mut state).is_empty());
}

#[test]
fn a_multi_token_id_one_address_lists_is_proven_at_every_address_of_the_account() {
    let addresses = account();
    let mut state = NftsState { collections: listed(&json!({ "collections": [{ "address": COLLECTION, "kind": "multi" }] })), addresses: addresses.iter().map(Address::to_string).collect(), ..NftsState::default() };
    assert!(state.answered(&tokens_of(&addresses[0].to_string()), &returned(&[json!({ "type": "u256", "value": "7" })])));

    let mut holders: Vec<String> = state.wanted().into_iter().map(|held| held.holder).collect();
    holders.sort();
    let mut expected: Vec<String> = addresses.iter().map(Address::to_string).collect();
    expected.sort();

    assert_eq!(holders, expected);
}

#[test]
fn an_svg_is_drawn_from_its_bytes_alone_and_an_animated_gif_keeps_its_frames() {
    // Drawn to fit a card, at most four times the size it says it is.
    let square = br#"<svg xmlns="http://www.w3.org/2000/svg" width="64" height="32"><rect width="64" height="32" fill="red"/></svg>"#;
    let Media::Picture(drawn) = read(square, Some("image/svg+xml")) else { panic!("an SVG is drawn") };
    assert_eq!(drawn.size, [256, 128]);
    let large = br#"<svg xmlns="http://www.w3.org/2000/svg" width="2048" height="1024"><rect width="2048" height="1024" fill="red"/></svg>"#;
    assert!(matches!(read(large, None), Media::Picture(picture) if picture.size == [512, 256]), "a big one shrinks to a card's");
    assert_eq!(drawn.pixels[0], egui::Color32::from_rgb(255, 0, 0));

    // An image it points at, by a file or a URL, is never loaded: it draws without it.
    let pointing = br#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="8" height="8"><image href="file:///etc/passwd" width="8" height="8"/><image xlink:href="https://example.org/x.png" width="8" height="8"/></svg>"#;
    let Media::Picture(drawn) = read(pointing, None) else { panic!("drawn, without what it points at") };
    assert!(drawn.pixels.iter().all(|pixel| pixel.a() == 0), "nothing loaded into it");

    // A GIF of two frames animates, each frame for as long as it says.
    let frame = |rgb: [u8; 3], ms: u32| {
        let buffer = image::RgbaImage::from_pixel(4, 4, image::Rgba([rgb[0], rgb[1], rgb[2], 255]));
        image::Frame::from_parts(buffer, 0, 0, image::Delay::from_numer_denom_ms(ms, 1))
    };
    let mut animated = Vec::new();
    {
        let mut encoder = image::codecs::gif::GifEncoder::new(&mut animated);
        encoder.encode_frames([frame([255, 0, 0], 200), frame([0, 0, 255], 5)]).unwrap();
    }
    let Media::Animation(frames) = read(&animated, Some("image/gif")) else { panic!("an animation") };
    assert_eq!(frames.len(), 2);
    assert_eq!((frames[0].1, frames[1].1), (0.2, 0.1), "a frame under 20 ms shows for 100, as browsers do");

    // One frame alone is a picture as any other.
    let mut still = Vec::new();
    image::codecs::gif::GifEncoder::new(&mut still).encode_frames([frame([0, 255, 0], 100)]).unwrap();
    assert!(matches!(read(&still, None), Media::Picture(picture) if picture.size == [4, 4]));
}

#[test]
fn a_batch_packs_each_id_and_amount_and_goes_safely_to_a_contract() {
    let from = Address::Ed25519([1; 32]);
    let collection = Address::Contract([9; 32]);
    let items = vec![("7".to_string(), u256::from(2u64)), ("300".to_string(), u256::from(1u64))];
    let packed = |prepared: &Prepared| match &prepared.tx {
        Tx::ContractCall { func, data, .. } => (*func, data.clone()),
        _ => panic!("a call"),
    };

    let (func, data) = packed(&batch_transfer(from, collection, &items, Address::Secp256k1([2; 20])));
    assert_eq!(func, Hasher::digest(HashType::Xxh3_64, b"batch_transfer_from"));
    let TxCallArgumentType::Bytes(bytes) = &data[2] else { panic!("packed in bytes") };
    assert_eq!(bytes.len(), 128);
    assert_eq!((bytes[31], bytes[63], &bytes[94..96], bytes[127]), (7, 2, &[1u8, 44][..], 1), "big-endian, 32 bytes each");
    assert_eq!(data[0], TxCallArgumentType::Address(from));

    let (func, _) = packed(&batch_transfer(from, collection, &items, Address::Contract([3; 32])));
    assert_eq!(func, Hasher::digest(HashType::Xxh3_64, b"safe_batch_transfer_from"));
}

#[test]
fn a_royalty_is_read_as_a_share_of_ten_thousand_and_none_where_the_collection_keeps_none() {
    let mut state = NftsState::default();
    let ask = |id: &str| call_ask("contract:0x01", "royalty_info", serde_json::json!([{ "type": "u256", "value": id }, { "type": "u256", "value": "10000" }]));

    state.answered(&ask("5"), &returned(&[serde_json::json!({ "type": "address", "value": "ed25519:0x77" }), serde_json::json!({ "type": "u256", "value": "250" })]));
    assert_eq!(state.royalties[&("contract:0x01".to_string(), "5".to_string())], Some(("ed25519:0x77".to_string(), u256::from(250u64))));
    state.answered(&ask("6"), &returned(&[serde_json::json!({ "type": "address", "value": "ed25519:0x00" }), serde_json::json!({ "type": "u256", "value": "0" })]));
    assert_eq!(state.royalties[&("contract:0x01".to_string(), "6".to_string())], None, "nothing owed is none");
    state.answered(&ask("7"), &ok(serde_json::json!({ "status": "failed", "error": "no such function" })));
    assert_eq!(state.royalties[&("contract:0x01".to_string(), "7".to_string())], None);
}

