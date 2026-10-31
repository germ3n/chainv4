use {
    super::*,
    chainv4::crypto::keypair::Keypair,
    chainv4::registry::names::NameAction,
    chainv4::structs::amount::Amount,
};

fn me() -> Address {
    Keypair::from_seed([7; 32]).public().address()
}

fn someone() -> Address {
    Keypair::from_seed([8; 32]).public().address()
}

fn dev() -> Shown {
    Shown { symbol: "DEV".to_string(), decimals: 18 }
}

fn laid(steps: &[Tx]) -> Steps {
    of(steps, &dev(), &|_| None, None)
}

const COIN: u128 = 1_000_000_000_000_000_000;

#[test]
fn the_total_is_all_that_every_kind_of_step_sends_of_the_coin() {
    let contract = Address::Contract([3; 32]);
    let steps = laid(&[
        Tx::Transfer { from: me(), to: someone(), amount: Amount::U128(COIN) },
        Tx::Burn { from: me(), amount: Amount::U128(COIN) },
        Tx::ContractCall { to: contract, func: Hasher::digest(HashType::Xxh3_64, b"mint"), amount: Amount::U128(COIN), data: Vec::new() },
        Tx::EvmCall { to: Address::Evm([1; 20]), value: Amount::U128(COIN), input: Vec::new() },
        // Moves a token, not the coin.
        Tx::ContractCall { to: contract, func: Hasher::digest(HashType::Xxh3_64, b"transfer"), amount: Amount::U128(0), data: vec![TxCallArgumentType::Address(someone()), TxCallArgumentType::U256(u256::new(5))] },
        Tx::PrimaryName { name: None },
    ]);

    assert_eq!(steps.count, 6);
    assert_eq!(steps.coin, u256::new(4 * COIN));
    assert_eq!(steps.total.0, "4 DEV");
    assert_eq!(steps.total.1, (4 * COIN).to_string());
    assert!(steps.runs_code);
    assert!(!steps.burns_names);
}

#[test]
fn a_total_too_large_to_add_up_stays_as_large_as_there_is_rather_than_wrapping_round() {
    let huge = Tx::Transfer { from: me(), to: someone(), amount: Amount::U256(u256::MAX) };

    assert_eq!(laid(&[huge.clone(), huge]).coin, u256::MAX);
}

#[test]
fn each_kind_says_what_it_does_and_a_token_goes_by_its_name_only_where_it_is_known() {
    let token = Address::Contract([3; 32]);
    let transfer = Tx::ContractCall { to: token, func: Hasher::digest(HashType::Xxh3_64, b"transfer"), amount: Amount::U128(0), data: vec![TxCallArgumentType::Address(someone()), TxCallArgumentType::U256(u256::new(250))] };

    let unknown = laid(std::slice::from_ref(&transfer));

    assert_eq!(unknown.steps[0].kind, "Send a token");
    assert!(unknown.steps[0].rows.iter().any(|row| row.said == Said::Text("250 units of the token".to_string())), "{:?}", unknown.steps[0].rows);

    let gold = |_: &Address| Some(Shown { symbol: "GOLD".to_string(), decimals: 2 });
    let named = of(std::slice::from_ref(&transfer), &dev(), &gold, None);

    assert_eq!(named.steps[0].kind, "Send GOLD");
    assert!(named.steps[0].rows.iter().any(|row| row.said == Said::Text("2.5 GOLD · 250 units".to_string())), "{:?}", named.steps[0].rows);

    let renamed = laid(&[Tx::Name { name: "alice".to_string(), action: NameAction::Register { years: 2 } }]);

    assert!(renamed.burns_names);
    assert_eq!(renamed.steps[0].kind, "Register a name");
    assert_eq!(renamed.steps[0].rows[0].said, Said::Text("alice, for 2 years".to_string()));

    let call = laid(&[Tx::ContractCall { to: token, func: Hasher::digest(HashType::Xxh3_64, b"approve"), amount: Amount::U128(COIN), data: vec![TxCallArgumentType::I64(-5), TxCallArgumentType::Nil] }]);

    assert_eq!(call.steps[0].kind, "Call approve");
    assert_eq!(call.steps[0].coin.as_ref().map(|(shown, _)| shown.as_str()), Some("1 DEV"));
    assert_eq!(call.steps[0].rows[1].said, Said::Text("-5".to_string()));

    let evm = laid(&[Tx::EvmCall { to: Address::Evm([1; 20]), value: Amount::U128(0), input: vec![1, 2, 3, 4, 5] }]);

    assert_eq!(evm.steps[0].rows[1].said, Said::Text("0x01020304 · 5 bytes".to_string()));
    assert_eq!(evm.steps[0].evm.as_ref().map(|evm| evm.input.len()), Some(5));
}

#[test]
fn what_no_batch_may_hold_is_named_and_marked() {
    let steps = laid(&[Tx::Mint { to: me(), amount: Amount::U128(1) }, Tx::Batch { steps: Vec::new() }]);

    assert!(steps.steps.iter().all(|step| !step.allowed));
    assert!(steps.steps[0].kind.contains("Not allowed in a batch: Mint"), "{}", steps.steps[0].kind);
    assert!(steps.steps[1].kind.contains("Batch"));
}

#[test]
fn deploys_say_where_they_land_each_apart_and_where_nothing_says_the_chain_s_hash_they_do_not() {
    let code = || Tx::Deploy { code: Code::Lua("return {}".to_string()) };
    let evm = || Tx::Deploy { code: Code::Evm(vec![0]) };
    let hash = HashType::default_chain();
    let landing = Landing { sender: me(), nonce: 7, hash };
    let steps = of(&[code(), Tx::PrimaryName { name: None }, code(), evm()], &dev(), &|_| None, Some(&landing));
    let lands = |index: usize| steps.steps[index].rows.iter().find(|row| row.label == "Lands at").map(|row| row.said.clone());

    assert_eq!(lands(0), Some(Said::Address(contract_address(&me(), 7, hash).to_string())));
    assert_eq!(lands(2), Some(Said::Address(contract_address(&me(), batch_deploy_nonce(7, 1), hash).to_string())));
    assert_eq!(lands(3), Some(Said::Address(Address::Evm(evm_create_address(evm_address(&me()), batch_deploy_nonce(7, 2))).to_string())));
    assert!(laid(&[code()]).steps[0].rows.iter().all(|row| row.label != "Lands at"), "an offline signer knows no chain's hash");
}

#[test]
fn plain_text_stays_on_its_line_says_what_it_hides_and_is_cut_short() {
    assert_eq!(plain("hello"), "hello");
    assert_eq!(plain("a\nb\tc"), "a\\nb\\tc");
    assert_eq!(plain("say \"hi\""), "say \"hi\"");
    assert!(plain("x\u{202e}y").contains("\\u{202e}"), "what reverses text is written, not obeyed");
    assert!(plain("x\u{200b}y").contains("\\u{200b}"), "what cannot be seen is written");
    assert!(!plain("\u{1b}[31m").contains('\u{1b}'));

    let cut = plain(&"W".repeat(1_000));

    assert_eq!(cut.chars().count(), MAX_TEXT + 1);
    assert!(cut.ends_with('…'));
    assert_eq!(plain(&"W".repeat(MAX_TEXT)).chars().count(), MAX_TEXT, "nothing to cut");
}

#[test]
fn a_coin_or_token_named_to_deceive_keeps_to_its_field() {
    let lying = Shown { symbol: "DEV\nStep 9 of 9\u{202e}".to_string(), decimals: 0 };
    let token = Address::Contract([3; 32]);
    let steps = [
        Tx::Transfer { from: me(), to: someone(), amount: Amount::U128(5) },
        Tx::ContractCall { to: token, func: Hasher::digest(HashType::Xxh3_64, b"transfer"), amount: Amount::U128(0), data: vec![TxCallArgumentType::Address(someone()), TxCallArgumentType::U256(u256::new(5))] },
    ];
    let laid = of(&steps, &lying, &|_| Some(lying.clone()), None);
    let everything = format!("{laid:?}");

    assert!(!everything.contains("Step 9 of 9\u{202e}") || everything.contains("\\n"), "{everything}");
    assert!(laid.total.0.lines().count() == 1 && !laid.total.0.contains('\u{202e}'), "{}", laid.total.0);
    assert!(laid.steps.iter().all(|step| step.kind.lines().count() == 1 && !step.kind.contains('\u{202e}')), "{:?}", laid.steps);
    assert!(laid.steps[0].coin.iter().all(|(shown, _)| shown.lines().count() == 1));
    assert!(laid.steps[1].rows.iter().all(|row| matches!(&row.said, Said::Address(_)) || matches!(&row.said, Said::Text(text) if text.lines().count() == 1 && !text.contains('\u{202e}'))), "{:?}", laid.steps[1].rows);
}

