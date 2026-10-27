use {
    super::*,
    crate::db::{Database, MemoryDb},
    crate::registry::tokens::{Token, Tokens, stored_tokens},
    std::sync::Arc,
};

fn collection(byte: u8, kind: CollectionKind, symbol: Option<&str>) -> Collection {
    Collection {
        address: Address::Contract([byte; 32]),
        kind,
        name: None,
        symbol: symbol.map(str::to_string),
    }
}

fn symbols(db: &MemoryDb, symbol: Option<&str>) -> Vec<Option<String>> {
    stored_collections(db, symbol, None, 100).unwrap().0.into_iter().map(|collection| collection.symbol).collect()
}

#[test]
fn collections_are_listed_by_symbol_apart_from_tokens() {
    let db = Arc::new(MemoryDb::default());
    let state = State::new(db.clone());
    let collections = Collections::new(&state);

    for (byte, kind, symbol) in [
        (1, CollectionKind::Nft, Some("apes")),
        (2, CollectionKind::Multi, Some("ITEMS")),
        (3, CollectionKind::Nft, None),
        (4, CollectionKind::Nft, Some("Ape")),
    ] {
        collections.register(&collection(byte, kind, symbol)).unwrap();
    }

    // A token beside them, with a symbol they share: neither listing shows the other.
    Tokens::new(&state)
        .register(&Token { address: Address::Contract([9; 32]), name: None, symbol: Some("APE".to_string()), decimals: None })
        .unwrap();
    db.commit(state.take_changes()).unwrap();
    let some = |symbols: &[&str]| symbols.iter().map(|symbol| Some(symbol.to_string())).collect::<Vec<_>>();

    assert_eq!(symbols(&db, None), [vec![None], some(&["Ape", "apes", "ITEMS"])].concat());
    assert_eq!(symbols(&db, Some("ap")), some(&["Ape", "apes"]));
    assert_eq!(stored_tokens(&*db, None, None, 10).unwrap().0.len(), 1);

    // A page at a time.
    let (first, next) = stored_collections(&*db, None, None, 2).unwrap();
    let (rest, after) = stored_collections(&*db, None, next.as_deref(), 2).unwrap();

    assert_eq!((first.len(), rest.len(), after), (2, 2, None));

    // Registering again moves its listing; taking it off removes both its keys.
    let state = State::new(db.clone());
    let collections = Collections::new(&state);
    collections.register(&collection(1, CollectionKind::Nft, Some("ZOO"))).unwrap();
    collections.unregister(&Address::Contract([2; 32])).unwrap();
    db.commit(state.take_changes()).unwrap();

    assert_eq!(symbols(&db, None), [vec![None], some(&["Ape", "ZOO"])].concat());
    assert_eq!(Collections::new(&State::new(db.clone())).get(&Address::Contract([2; 32])).unwrap(), None);
    assert_eq!(
        Collections::new(&State::new(db.clone())).get(&Address::Contract([1; 32])).unwrap().map(|found| found.kind),
        Some(CollectionKind::Nft)
    );
}

#[test]
fn the_nft_standard_comes_before_the_multi_token_one() {
    let functions = CollectionFunctions::standard();
    let exporting = |names: &[&str]| {
        let names = names.to_vec();

        functions.kind(move |function| names.contains(&function))
    };

    assert_eq!(exporting(&STANDARD_NFT_FUNCTIONS), Some(CollectionKind::Nft));
    assert_eq!(exporting(&STANDARD_MULTI_FUNCTIONS), Some(CollectionKind::Multi));
    assert_eq!(exporting(&STANDARD_MULTI_FUNCTIONS[1..]), None);
    // The NFT list holds the multi-token one, so short of owner_of an NFT is taken for multi.
    let without_owner_of: Vec<&str> = STANDARD_NFT_FUNCTIONS.into_iter().filter(|name| *name != "owner_of").collect();

    assert_eq!(exporting(&without_owner_of), Some(CollectionKind::Multi));

    // An empty list keeps none of its kind.
    let none = CollectionFunctions { nft: Vec::new(), multi: Vec::new() };

    assert_eq!(none.kind(|_| true), None);
    assert_eq!(none.problem(), None);
    assert!(CollectionFunctions { nft: vec!["a".into(), "a".into()], multi: Vec::new() }.problem().is_some());
    assert!(CollectionFunctions { nft: Vec::new(), multi: vec![String::new()] }.problem().is_some());
}

#[test]
fn answers_become_a_collection_only_within_their_limits() {
    let address = Address::Contract([1; 32]);
    let text = |text: &str| vec![TxCallArgumentType::String(text.to_string())];
    let found = Collection::from_answers(address, CollectionKind::Multi, Some(&text("Items")), Some(&text("ITM"))).unwrap();

    assert_eq!((found.name.as_deref(), found.symbol.as_deref()), (Some("Items"), Some("ITM")));
    assert_eq!(Collection::from_answers(address, CollectionKind::Nft, None, None).unwrap().name, None);

    for (name, symbol) in [(Some(text("")), None), (None, Some(text("SEVENTEEN-LETTERS"))), (Some(text("a\nb")), None), (None, Some(vec![TxCallArgumentType::U64(1)]))] {
        assert!(Collection::from_answers(address, CollectionKind::Nft, name.as_deref(), symbol.as_deref()).is_err());
    }
}
