use {super::*, crate::model::Argument, serde_json::Value};

fn event(name: &str, data: &[&str]) -> FoundEvent {
    FoundEvent {
        contract: "contract:0xc1".to_string(),
        name: name.to_string(),
        data: data.iter().map(|value| Argument { kind: "string".to_string(), value: Value::String(value.to_string()) }).collect(),
        tx: "0x01".to_string(),
        block: "0x02".to_string(),
        height: 9,
        index: 0,
        position: 0,
    }
}

#[test]
fn transfers_read_by_the_collection_s_standard() {
    let nft = moved(&event("Transfer", &["ed25519:0x0a", "ed25519:0x0b", "7"]), false).unwrap();

    assert_eq!((nft.from.as_str(), nft.to.as_str(), nft.id.as_str(), nft.amount.as_str(), nft.height), ("ed25519:0x0a", "ed25519:0x0b", "7", "1", 9));
    let multi = moved(&event("TransferSingle", &["ed25519:0x0c", "ed25519:0x0a", "ed25519:0x0b", "7", "20"]), true).unwrap();

    assert_eq!((multi.from.as_str(), multi.to.as_str(), multi.amount.as_str()), ("ed25519:0x0a", "ed25519:0x0b", "20"), "the operator left out");

    // Another standard's event, or another event, is no transfer of this one.
    assert_eq!(moved(&event("TransferSingle", &["a", "b", "c", "7", "20"]), false), None);
    assert_eq!(moved(&event("Approval", &["a", "b", "7"]), false), None);
    assert_eq!(moved(&event("Transfer", &["a", "b"]), false), None, "too few values");
}

#[test]
fn the_registry_reads_and_names_each_collection() {
    let page: crate::model::CollectionsPage = serde_json::from_value(serde_json::json!({ "collections": [
        { "address": "contract:0xc1", "kind": "nft", "name": "Art", "symbol": "ART" },
        { "address": "contract:0xc2", "kind": "multi", "name": null, "symbol": "ITM" },
        { "address": "contract:0xc3", "kind": "nft", "name": null, "symbol": null },
    ], "next": null })).unwrap();

    assert_eq!(page.collections.iter().map(|collection| (collection.multi(), collection.title())).collect::<Vec<_>>(), [
        (false, Some("Art".to_string())),
        (true, Some("ITM".to_string())),
        (false, None),
    ]);
    assert_eq!(kind(&page.collections[1]), "multi-token");
}

#[test]
fn an_evm_collections_logs_read_as_transfers_by_its_standard() {
    use chainv4::crypto::secp256k1::keccak;
    use chainv4::structs::hex::to_hex;

    let word = |byte: u8| to_hex(&[[0; 12].as_slice(), &[byte; 20]].concat());
    let number = |value: u64| to_hex(&chainv4::structs::u256::u256::from(value).to_be_bytes());
    let log = |parts: &[String]| event("log", &parts.iter().map(String::as_str).collect::<Vec<_>>());
    let transfer = to_hex(&keccak(b"Transfer(address,address,uint256)"));
    let single = to_hex(&keccak(b"TransferSingle(address,address,address,uint256,uint256)"));
    let nft = evm_moved(&log(&[transfer.clone(), word(0), word(0xbb), number(7), "0x".to_string()]), false).unwrap();

    assert_eq!((nft.from.as_str(), nft.id.as_str(), nft.amount.as_str()), (chainv4::structs::address::Address::Evm([0; 20]).to_string().as_str(), "7", "1"), "minted, from the zero address");
    assert_eq!(nft.to, chainv4::structs::address::Address::Evm([0xbb; 20]).to_string());

    let data = format!("{}{}", number(3), &number(40)[2..]);
    let multi = evm_moved(&log(&[single, word(0xcc), word(0xaa), word(0xbb), data]), true).unwrap();

    assert_eq!((multi.id.as_str(), multi.amount.as_str()), ("3", "40"));
    assert_eq!(multi.from, chainv4::structs::address::Address::Evm([0xaa; 20]).to_string(), "the operator left out");

    // An ERC-20's Transfer has three topics: no token of a collection.
    assert_eq!(evm_moved(&log(&[transfer, word(1), word(2), number(500)]), false), None);
}

#[test]
fn an_erc_1155_uri_names_its_id_in_64_hex_digits() {
    use chainv4::abi::{AbiType, AbiValue, encode_values};

    let output = encode_values(&[AbiType::String], &[AbiValue::String("ipfs://x/{id}.json".to_string())]).unwrap();

    assert_eq!(uri_of(&output, "10").unwrap(), format!("ipfs://x/{}0a.json", "0".repeat(62)));
    assert_eq!(uri_of(&[1, 2], "10"), None, "not a string");
}
