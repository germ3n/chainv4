use {super::*, chainv4::abi::{decode_values, parse_signature}};

fn pool(a: Address, b: Address, reserve_a: u128, reserve_b: u128) -> DexPool {
    DexPool { a, b, reserve_a: u256::new(reserve_a), reserve_b: u256::new(reserve_b), share: Address::Contract([0x55; 32]) }
}

#[test]
fn a_pools_prices_come_from_its_sync_events_oldest_first() {
    let token = Address::Contract([7; 32]);
    let other = Address::Contract([8; 32]);
    let sync = |height: u64, a: &Address, b: &Address, reserve_a: u64, reserve_b: u64| {
        json!({ "height": height, "data": [
            { "type": "address", "value": a.to_string() },
            { "type": "address", "value": b.to_string() },
            { "type": "u256", "value": reserve_a.to_string() },
            { "type": "u256", "value": reserve_b.to_string() },
        ] })
    };
    // Newest first, as the node gives them, one of another pool among them.
    let events = [sync(9, &COIN, &token, 100, 400), sync(8, &COIN, &other, 1, 1), sync(5, &COIN, &token, 100, 200)];

    assert_eq!(prices(&events, &pool(COIN, token, 100, 400), 0, 0), [(5, 2.0), (9, 4.0)]);
    // Either order of the pair: priced in the chosen pool's.
    assert_eq!(prices(&events, &pool(token, COIN, 400, 100), 0, 0), [(5, 0.5), (9, 0.25)]);
}

#[test]
fn trades_go_to_the_send_page_as_calls_to_the_dex_or_the_router() {
    let me = Address::Ed25519([1; 32]);
    let dex = Address::Contract([0xd; 32]);
    let prepared = native_call(me, dex, "swap", vec![TxCallArgumentType::Address(COIN)], u256::new(500), "Trade".to_string());

    match prepared.tx {
        Tx::ContractCall { to, func, amount, data } => {
            assert_eq!((to, func, amount), (dex, Hasher::digest(HashType::Xxh3_64, b"swap"), Amount::U128(500)));
            assert_eq!(data, [TxCallArgumentType::Address(COIN)]);
        }
        other => panic!("{other:?}"),
    }

    // The router's calls, in Solidity's ABI.
    let signature = "swapExactDEVForTokens(uint256,address[],address,uint256)";
    let path = AbiValue::Array(vec![AbiValue::Address([0xa; 20]), AbiValue::Address([0xb; 20])]);
    let input = calldata(signature, &[AbiType::Uint(256), AbiType::Array(Box::new(AbiType::Address)), AbiType::Address, AbiType::Uint(256)], &[AbiValue::Uint(u256::new(9)), path.clone(), AbiValue::Address([0xc; 20]), AbiValue::Uint(u256::new(60))]);
    let (selector, types) = parse_signature(signature).unwrap();
    assert_eq!(input[..4], selector.unwrap());
    assert_eq!(decode_values(&types, &input[4..]).unwrap()[1], path);
    let sent = evm_call(me, [0xe; 20], input, u256::new(9), "Trade".to_string());
    assert!(matches!(sent.tx, Tx::EvmCall { to: Address::Evm(to), value: Amount::U128(9), .. } if to == [0xe; 20]));
    assert_eq!(approve_input([0xd; 20], u256::new(5))[..4], keccak(b"approve(address,uint256)")[..4]);
}

#[test]
fn the_evm_dexs_answers_list_its_pairs_with_wdev_as_the_coin() {
    let mut state = TradeState { venue: Venue::Evm, ..TradeState::default() };
    let answer = |state: &mut TradeState, read: Read, words: Vec<[u8; 32]>| {
        let id = state.next_id;
        state.next_id += 1;
        state.reads.insert(id, read);
        let body = json!({ "id": id }).to_string();
        assert!(state.answered_rpc(&body, &Ok(json!({ "result": to_hex(&words.concat()) }))));
    };
    let address = |byte: u8| {
        let mut word = [0; 32];
        word[12..].copy_from_slice(&[byte; 20]);
        word
    };
    let number = |value: u64| u256::from(value).to_be_bytes();

    answer(&mut state, Read::Wdev, vec![address(0xee)]);
    answer(&mut state, Read::Factory, vec![address(0xfa)]);
    answer(&mut state, Read::Pair(0), vec![address(0x11)]);
    answer(&mut state, Read::Token0([0x11; 20]), vec![address(0x0a)]);
    answer(&mut state, Read::Token1([0x11; 20]), vec![address(0xee)]);
    answer(&mut state, Read::Reserves([0x11; 20]), vec![number(300), number(700), number(0)]);

    assert_eq!(state.listed(), [DexPool { a: Address::Evm([0x0a; 20]), b: COIN, reserve_a: u256::new(300), reserve_b: u256::new(700), share: Address::Evm([0x11; 20]) }]);
    assert!(!state.answered_rpc(&json!({ "id": 5 }).to_string(), &Ok(json!({}))), "the dApp page's");
}

#[test]
fn slippage_is_read_in_percent() {
    let at = |text: &str| slippage(&TradeState { slippage: text.to_string(), ..TradeState::default() });

    assert_eq!((at("0.5"), at("1"), at("nope"), at("80")), (50, 100, 50, 50));
}
