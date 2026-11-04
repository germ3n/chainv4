use {super::*, crate::swap::Leg};

fn leg(token: Option<[u8; 20]>) -> EvmLeg {
    EvmLeg { item: None, chain_id: 11155111, contract: [0xc0; 20], token, amount: u256::new(7), payer: Some([0xa1; 20]), receiver: Some([0xb2; 20]) }
}

#[test]
fn calls_are_swap_sols_as_solc_names_them() {
    let hash = [0x11; 32];
    let calls = leg(None).lock_calls(&hash, 1_793_043_200_999).unwrap();

    // solc --hashes Swap.sol
    assert_eq!(calls.len(), 1);
    assert_eq!((calls[0].to, calls[0].value), ([0xc0; 20], u256::new(7)), "the coin goes with the lock");
    assert_eq!(calls[0].input[..4], [0x3a, 0x22, 0xa2, 0x5c]);
    assert_eq!(calls[0].input[4..36], hash);
    assert_eq!(calls[0].input[36..68], address_word(&[0xb2; 20]));
    assert_eq!(calls[0].input[68..], number_word(u256::new(1_793_043_200)), "whole seconds");
    let id = [0x33; 32];
    assert_eq!(leg(None).claim_call(&id, &[0x22; 32]).input, [&[0x84, 0xcc, 0x9d, 0xfb][..], &id, &[0x22; 32]].concat());
    assert_eq!(leg(None).refund_call(&id).input, [&[0x72, 0x49, 0xfb, 0xb6][..], &id].concat());
    assert_eq!(leg(None).withdraw_call(&id).input, [&[0x8e, 0x19, 0x89, 0x9e][..], &id].concat());
    assert_eq!(get_input(&id)[..4], [0x8e, 0xaa, 0x6a, 0xc0]);
    assert_eq!(item_input(&id)[..4], [0x1d, 0x37, 0x6d, 0x9b]);

    // A token is approved first, then locked, sending no coin.
    let calls = leg(Some([0x70; 20])).lock_calls(&hash, 1_793_043_200_000).unwrap();
    assert_eq!(calls.iter().map(|call| (call.to, call.value, call.input[..4].to_vec())).collect::<Vec<_>>(), [
        ([0x70; 20], u256::ZERO, vec![0x09, 0x5e, 0xa7, 0xb3]),
        ([0xc0; 20], u256::ZERO, vec![0x6b, 0xb3, 0xe8, 0x57]),
    ]);
    assert_eq!(calls[0].input[4..], [address_word(&[0xc0; 20]), number_word(u256::new(7))].concat(), "the contract, for the amount");
    assert!(EvmLeg { item: None, receiver: None, ..leg(None) }.lock_calls(&hash, 0).is_err());
}

#[test]
fn swap_sol_sits_where_create2_puts_it_through_the_proxy() {
    // EIP-1014's first example.
    assert_eq!(create2(&[0; 20], &[0; 32], &[0]), parse_address("0x4d1a2e2bb4f88f0250f26ffff098b0b30b26bf38").unwrap());
    assert_eq!(deployed_address(), create2(&DEPLOYMENT_PROXY, &[0; 32], &init_code()));
    assert_eq!(proxy_deployment()[32..], init_code());
    assert_eq!(show_address(&DEPLOYMENT_PROXY), "0x4e59b44847b379578588920ca78fbf26c0b4956c");
}

#[test]
fn get_answers_read_as_locks() {
    let words = |state: u8, token: [u8; 20], secret: [u8; 32], owed: u8| {
        [
            number_word(u256::from(state)),
            address_word(&[0xa1; 20]),
            address_word(&[0xb2; 20]),
            address_word(&token),
            number_word(u256::new(7)),
            number_word(u256::new(1_793_043_200)),
            secret,
            number_word(u256::new(4_242)),
            number_word(u256::from(owed)),
        ]
        .concat()
    };

    let locked = read_lock(&words(1, [0; 20], [0; 32], 0)).unwrap();
    assert_eq!(locked, Lock { state: LockState::Locked, recipient: Some(Party::Evm([0xb2; 20])), token: None, amount: u256::new(7), deadline: 1_793_043_200_000, secret: None, height: Some(4_242), item: None, owed: false });
    assert_eq!(Leg::Evm(leg(None)).stands(&locked, 1_793_043_200_999), Ok(()), "seconds, as Swap.sol keeps them");

    let claimed = read_lock(&words(2, [0x70; 20], [0x22; 32], 1)).unwrap();
    assert_eq!((claimed.token, claimed.secret, claimed.owed), (Some(Party::Evm([0x70; 20])), Some([0x22; 32]), true));
    assert_eq!(read_lock(&words(0, [0; 20], [0; 32], 0)).unwrap().state, LockState::None);
    assert!(read_lock(&words(4, [0; 20], [0; 32], 0)).is_err());
    assert!(read_lock(&words(2, [0; 20], [0; 32], 2)).is_err(), "owed is a bool");
    assert!(read_lock(&words(1, [0; 20], [0; 32], 0)[..8 * 32]).is_err(), "a Swap.sol from before claims paid by pull");
    assert!(read_lock(&[0; 32]).is_err());
}

#[test]
fn chains_are_named_by_their_ids() {
    assert_eq!((chain_name(1), chain_name(11155111), chain_name(7)), ("Ethereum".to_string(), "Sepolia".to_string(), "chain 7".to_string()));
    assert_eq!((symbol(137), symbol(7)), ("POL", "ETH"));
    assert!(parse_address("0x12").is_err());
}
