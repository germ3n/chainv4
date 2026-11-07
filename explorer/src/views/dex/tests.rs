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
    DexPool { a: COIN, b: token(), fee_bps: 30, reserve_a: u256::new(1_000_000), reserve_b: u256::new(4_000_000), share: Address::Contract([9; 32]), venue: chainv4::dex::Venue::Native }
}

#[test]
fn the_native_dexs_events_read_as_trades_adds_and_removals_in_the_pools_order() {
    let (coin, token) = (COIN.to_string(), token().to_string());
    let swap_in_token = event("Swap", vec![token.clone(), coin.clone(), "ed25519:0x01".into(), "40000".into(), "9900".into(), "30".into()], 20);
    // From before tiers: no tier, so the pair's pool at any.
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
    assert_eq!(fees(&trades, &pool(), 20, 5), u256::new(30), "40,000 tokens are 10,000 of the coin at 4 to 1");

    // The same pair's pool at another tier: the trade at 0.3% is not its, the add from before tiers is.
    let other = DexPool { fee_bps: 100, ..pool() };
    assert_eq!(in_pool(&other, &trades[0]), None);
    assert!(in_pool(&other, &trades[1]).is_some());
    assert_eq!(fees(&trades, &other, 20, 5), u256::ZERO);
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

#[test]
fn a_pools_time_weighted_price_comes_from_two_reads_an_hour_apart() {
    use crate::store::Store;
    use chainv4::abi::{AbiType, AbiValue, encode_values};

    let mut store = Store::default();
    // The tip at 7,200; with no block times known, blocks are taken as 2 seconds apart, so the
    // earlier read is an hour back, at 5,400.
    store.main.insert(7_000, "0x01".to_string());
    store.main.insert(7_200, "0x02".to_string());
    let native = "contract:0x0d".to_string();
    // Two tokens neither of whose decimals is known: raw units, as they are.
    let pool = DexPool { a: Address::Contract([5; 32]), ..pool() };
    let args = json!([{ "type": "address", "value": pool.a.to_string() }, { "type": "address", "value": pool.b.to_string() }, { "type": "i64", "value": pool.fee_bps.to_string() }]);
    let one = u256::from(10u64).pow(18);
    let answer = |cumulative: u256, time: u64| json!({ "status": "returned", "values": [{}, {}, {}, { "type": "u256", "value": cumulative.to_string() }], "time": time });
    let at = |height: u64| Ask::CallAt(json!({ "contract": native, "function": "pool", "args": args, "block": height }).to_string());

    // 3 a millisecond, 10^18 fixed point, over an hour: an average of 3.
    store.calls.insert(at(5_400), answer(one, 1_000));
    store.calls.insert(at(7_200), answer(one + one * u256::from(3u64) * u256::from(3_600_000u64), 3_601_000));
    let mut found = None;

    crate::views::render(&store, &mut crate::light::Light::default(), 1_200.0, |_, ctx| {
        found = twap(ctx, &mut DexState::default(), &pool, &Some(native.clone()), false);
    });

    let (average, span) = found.expect("both reads answered");
    assert_eq!(span, 3_600_000);
    assert!((average - 3.0).abs() < 1e-9, "{average}");

    // An EVM pair: its price0CumulativeLast brought up to each block's time from its reserves.
    let pair = [0x2a; 20];
    let evm_pool = DexPool { a: Address::Evm([5; 20]), b: Address::Evm([6; 20]), share: Address::Evm(pair), venue: chainv4::dex::Venue::Evm, ..pool.clone() };
    let contract = format!("evm:{}", to_hex(&pair));
    let input = |signature: &str| to_hex(&keccak(signature.as_bytes())[..4]);
    let evm_at = |signature: &str, height: u64| Ask::CallAt(json!({ "contract": contract, "input": input(signature), "block": height }).to_string());
    let word = |value: u256| to_hex(&value.to_be_bytes());
    let reserves = |last_s: u64| to_hex(&encode_values(&[AbiType::Uint(112), AbiType::Uint(112), AbiType::Uint(32)], &[AbiValue::Uint(u256::new(1_000)), AbiValue::Uint(u256::new(2_000)), AbiValue::Uint(u256::from(last_s))]).unwrap());

    // Last changed at second 1; read at seconds 1 and 3,601: an average of 2.
    store.calls.insert(evm_at("price0CumulativeLast()", 5_400), json!({ "status": "returned", "output": word(u256::ZERO), "time": 1_000 }));
    store.calls.insert(evm_at("getReserves()", 5_400), json!({ "status": "returned", "output": reserves(1), "time": 1_000 }));
    store.calls.insert(evm_at("price0CumulativeLast()", 7_200), json!({ "status": "returned", "output": word(u256::ZERO), "time": 3_601_000 }));
    store.calls.insert(evm_at("getReserves()", 7_200), json!({ "status": "returned", "output": reserves(1), "time": 3_601_000 }));
    let mut found = None;

    crate::views::render(&store, &mut crate::light::Light::default(), 1_200.0, |_, ctx| {
        found = twap(ctx, &mut DexState::default(), &evm_pool, &None, true);
    });

    let (average, _) = found.expect("all four reads answered");
    assert!((average - 2.0).abs() < 1e-9, "{average}");
}
