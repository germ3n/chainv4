use {super::*, crate::model::Argument, chainv4::structs::address::Address};

fn event(name: &str, data: Vec<String>, height: u64) -> FoundEvent {
    FoundEvent {
        contract: "contract:0x01".to_string(),
        name: name.to_string(),
        data: data.into_iter().map(|value| Argument { kind: "string".to_string(), value: Value::String(value) }).collect(),
        tx: format!("0x{height:064x}"),
        block: format!("0x{:064x}", height + 1),
        height,
        index: 0,
        position: 0,
    }
}

fn token() -> Address {
    Address::Contract([7; 32])
}

fn pool() -> DexPool {
    DexPool { a: COIN, b: token(), reserve_a: u256::new(1_000_000), reserve_b: u256::new(4_000_000), share: Address::Contract([9; 32]) }
}

#[test]
fn the_native_dexs_events_read_as_trades_adds_and_removals_in_the_pools_order() {
    let (coin, token) = (COIN.to_string(), token().to_string());
    let swap_in_token = event("Swap", vec![token.clone(), coin.clone(), "ed25519:0x01".into(), "40000".into(), "9900".into()], 20);
    let added = event("Mint", vec![coin.clone(), token.clone(), "ed25519:0x01".into(), "100".into(), "400".into(), "200".into()], 10);
    let trade = native_activity(&swap_in_token).unwrap();
    let add = native_activity(&added).unwrap();

    // Token in, coin out: in the pool's order, coin first.
    assert_eq!(in_pool(&pool(), &trade), Some([u256::ZERO, u256::new(40_000), u256::new(9_900), u256::ZERO]));
    assert_eq!(in_pool(&pool(), &add), Some([u256::new(100), u256::new(400), u256::ZERO, u256::ZERO]));
    assert!(native_activity(&event("Sync", vec![coin, token, "1".into(), "2".into()], 5)).is_none(), "a Sync is no activity");

    // Volume in the coin: what came out of it; fees: 0.3% of the token in, at the pool's price.
    let trades = [trade, add];
    assert_eq!(volume(&trades, &pool(), 20, 5), u256::new(9_900));
    assert_eq!(volume(&trades, &pool(), 100, 5), u256::ZERO, "outside the window");
    assert_eq!(fees(&trades, &pool(), 20, 5, 30), u256::new(30), "40,000 tokens are 10,000 of the coin at 4 to 1");
    assert_eq!(fees(&trades, &pool(), 20, 5, 100), u256::new(100), "at the fee the chain sets");
}

#[test]
fn an_evm_pair_s_logs_are_asked_for_by_the_name_the_node_keeps_them_under() {
    // The node's index compares names by bytes, and calls every EVM log "log": spelled any other
    // way, a pair's trades and prices come back empty. The chain's own tests check the node's side.
    let path = crate::api::Ask::Events { query: pair_logs(&pool()), cursor: None }.path();

    assert!(path.contains("&name=log&"), "{path}");
    assert_eq!(chainv4::structs::event::EVM_LOG, "log");
}

#[test]
fn an_evm_pairs_logs_read_as_trades() {
    let words = |values: [u64; 4]| to_hex(&values.map(word).concat());
    let topic = to_hex(&keccak(b"Swap(address,uint256,uint256,uint256,uint256,address)"));
    let log = event("log", vec![topic, "0x00".into(), "0x00".into(), words([0, 5_000, 1_200, 0])], 7);
    let trade = evm_activity(&pool(), &log).unwrap();

    assert_eq!((trade.kind, in_pool(&pool(), &trade)), ("trade", Some([u256::ZERO, u256::new(5_000), u256::new(1_200), u256::ZERO])));
    let other = event("log", vec![to_hex(&keccak(b"Approval(address,address,uint256)")), words([1, 2, 3, 4])], 7);
    assert!(evm_activity(&pool(), &other).is_none());
}
