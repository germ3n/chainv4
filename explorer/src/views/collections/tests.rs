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
