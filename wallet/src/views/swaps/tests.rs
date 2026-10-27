use {
    super::*,
    chainv4::crypto::mnemonic::Phrase,
    chainv4::crypto::secp256k1,
    chainv4::swap::evm::Call,
};

fn seed() -> chainv4::crypto::mnemonic::Seed {
    Phrase::parse("abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about").unwrap().seed("")
}

fn bitcoin_key(seed: &chainv4::crypto::mnemonic::Seed, network: Network, index: u32) -> ([u8; 32], [u8; 33]) {
    let key = seed.bitcoin_key(network.coin(), 0, index);
    let (PrivateKey::Bitcoin(secret), PublicKey::Bitcoin(public)) = (key.private().clone(), key.public()) else { unreachable!() };

    (secret, public)
}

const NOW: u64 = 1_793_000_000_000;

/// An offer by Alice, paying bitcoin for this chain's coin, and Bob's answer.
fn offered() -> (Terms, Terms, [u8; 32]) {
    let seed = seed();
    let (secret, hash) = secret().unwrap();
    let alice = Parties { chain: Some(Address::Ed25519([0xa; 32])), bitcoin: Some(bitcoin_key(&seed, Network::Test, 1).1), evm: None };
    let bob = Parties { chain: Some(Address::Ed25519([0xb; 32])), bitcoin: Some(bitcoin_key(&seed, Network::Test, 0).1), evm: None };
    let pays = Leg::Bitcoin(BitcoinLeg { testnet: true, sats: 80_000, payer: None, receiver: None });
    let gets = Leg::Chain(ChainLeg { item: None, contract: Address::Contract([0x5a; 32]), token: None, amount: u256::new(5_000), payer: None, receiver: None });
    let offer = Terms::offer(hash, pays, gets, NOW + FIRST_DEADLINE_MS, NOW + SECOND_DEADLINE_MS, &alice).unwrap();
    let answer = offer.answer(&bob).unwrap();

    (offer, answer, secret)
}

#[test]
fn terms_travel_as_a_code_the_scanner_reads() {
    let (offer, answer, _) = offered();

    assert!(answer.answers(&offer));

    for terms in [offer, answer] {
        let shown = Message::Swap(Box::new(terms.clone()));
        assert_eq!(Message::from_ur(shown.ur_type(), &shown.bytes()).unwrap(), shown);
        assert_eq!(shown.bytes(), terms.to_bytes(), "as the CLI writes them");
    }
}

#[test]
fn a_legs_lock_reads_as_where_it_stands() {
    let (_, terms, _) = offered();
    let value = |text: &str| json!({ "type": "string", "value": text });
    let leg = &terms.second;
    let deadline = terms.second_deadline.to_string();
    let (zero, alice) = (Address::Ed25519([0; 32]).to_string(), Address::Ed25519([0xa; 32]).to_string());
    let locked = |amount: &str| -> Vec<Value> { ["locked", &alice, &alice, &zero, amount, &deadline, ""].iter().map(|text| value(text)).collect() };
    let mut state = SwapsState::default();
    let hash = to_hex(&terms.hash);

    let read = |state: &SwapsState| {
        let text = |value: &Value| value["value"].as_str().unwrap().to_string();
        state.status.get(&hash).cloned().map(|answer| answer.and_then(|values| Lock::from_chain(&values.iter().map(text).collect::<Vec<_>>())))
    };

    assert_eq!(describe(leg, Some(&Err("asking".to_string())), terms.second_deadline), "not known yet: asking");
    state.status.insert(hash.clone(), Ok(vec![value("none")]));
    assert_eq!(describe(leg, read(&state).as_ref(), terms.second_deadline), "nothing locked yet");
    state.status.insert(hash.clone(), Ok(locked("5000")));
    assert_eq!(describe(leg, read(&state).as_ref(), terms.second_deadline), "locked as the terms say, waiting for its claim");
    state.status.insert(hash.clone(), Ok(locked("4000")));
    assert!(describe(leg, read(&state).as_ref(), terms.second_deadline).starts_with("locked, but not as the terms say"), "less than agreed");
    assert!(describe(&terms.first, None, terms.first_deadline).contains("HTLC"));

    assert_eq!(when(1_000 + 90 * 60_000, 1_000), "in 1 h 30 min");
    assert_eq!(when(1_000, 1_000 + 5 * 60_000), "5 min ago");
}

#[test]
fn calls_go_to_the_send_page_as_contract_calls() {
    let prepared = call(Address::Ed25519([1; 32]), Address::Contract([2; 32]), "lock", vec![TxCallArgumentType::I64(5)], u256::new(500), "Lock a swap".to_string());

    match prepared.tx {
        Tx::ContractCall { to, func, amount, data } => {
            assert_eq!(to, Address::Contract([2; 32]));
            assert_eq!(func, Hasher::digest(HashType::Xxh3_64, b"lock"));
            assert_eq!(amount, Amount::U128(500));
            assert_eq!(data, [TxCallArgumentType::I64(5)]);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_claim_signed_here_is_the_one_the_offline_signer_signs() {
    let (_, terms, secret) = offered();
    let seed = seed();
    let (claimer, ours) = bitcoin_key(&seed, Network::Test, 0);
    let form = BitcoinForm {
        funding: format!("{}:1", "cd".repeat(32)),
        sats: "80000".to_string(),
        to: bech32::encode_segwit("tb", &hash160(&ours)),
        fee_rate: "2".to_string(),
        ..BitcoinForm::default()
    };
    let spend = Spend::Claim { secret };

    // With the key here: signed at once.
    let Ok(Signed::Here(here)) = sign(Some(claimer), None, &terms, spend, &form) else { panic!("signed here") };
    assert_eq!(revealed(&here, &terms.hash), Some(secret));

    // Watched from the signer, account 0 of its words: a PSBT it signs, finished here the same.
    let origin = crate::airgap::Origin { account: 0, fingerprint: seed.bitcoin_account(0).master_fingerprint };
    let Ok(Signed::Signer(awaiting)) = sign(None, Some(origin), &terms, spend, &form) else { panic!("asked of the signer") };
    let signed = awaiting.request.psbt.sign(&seed).unwrap();
    assert_eq!(signed.finish_htlc(&terms.htlc().unwrap(), 80_000, spend).unwrap().encode(), here.encode());

    // Nowhere to sign, and coins that do not read.
    assert!(sign(None, None, &terms, spend, &form).is_err_and(|why| why.contains("pair it again")));
    assert!(sign(Some(claimer), None, &terms, spend, &BitcoinForm { funding: "nope".to_string(), ..form }).is_err());
}

fn sepolia_lock() -> Call {
    let leg = EvmLeg { item: None, chain_id: 11155111, contract: [0xc0; 20], token: None, amount: u256::new(7), payer: Some([0xa1; 20]), receiver: Some([0xb2; 20]) };

    leg.lock_calls(&[0x11; 32], NOW).unwrap().remove(0)
}

#[test]
fn an_evm_call_is_priced_from_one_batch_of_answers() {
    let call = sepolia_lock();
    let reply = |id: u64, result: &str| json!({ "jsonrpc": "2.0", "id": id, "result": result });
    let batch = json!([reply(2, "0x5"), reply(1, "0xaa36a7"), reply(3, "0x3b9aca00"), reply(4, "0x2"), reply(5, "0xc350")]);
    let tx = crate::views::evm::price(&call, 11155111, &batch).unwrap();

    assert_eq!((tx.chain_id, tx.nonce, tx.to, tx.value), (11155111, 5, Some([0xc0; 20]), u256::new(7)), "answers in any order, by id");
    assert_eq!((tx.max_price, tx.tip), (u256::new(2_000_000_000), u256::new(2)), "twice the price, the tip suggested");
    assert_eq!(tx.gas_limit, 50_000 + 12_500 + 10_000, "room above the estimate");

    // No tip suggested: one at the whole price.
    let untipped = json!([reply(1, "0xaa36a7"), reply(2, "0x5"), reply(3, "0x64"), { "id": 4, "error": { "message": "no such method" } }, reply(5, "0x5208")]);
    assert_eq!(crate::views::evm::price(&call, 11155111, &untipped).unwrap().tip, u256::new(100));

    // Another chain's RPC, or a call that would fail, is refused.
    assert!(crate::views::evm::price(&call, 1, &batch).unwrap_err().contains("another chain"));
    let failing = json!([reply(1, "0xaa36a7"), reply(2, "0x5"), reply(3, "0x64"), reply(4, "0x1"), { "id": 5, "error": { "message": "execution reverted: lock something" } }]);
    assert!(crate::views::evm::price(&call, 11155111, &failing).unwrap_err().contains("lock something"));
}

#[test]
fn the_signers_signature_completes_the_transaction_only_for_its_address() {
    let tx = sepolia_lock().transaction(11155111, 5, 80_000, u256::new(200), u256::new(2));
    let key = [0x42; 32];
    let from = secp256k1::address(&secp256k1::public_key(&key).unwrap()).unwrap();
    let origin = crate::airgap::Origin { account: 0, fingerprint: [1, 2, 3, 4] };
    let request = crate::hardware::SignRequest::asking(tx.signing_data(), crate::hardware::DataKind::TypedTransaction, Some(tx.chain_id), origin, from, "chainv4 · swap");
    let (rs, parity) = secp256k1::sign_recoverable(&key, &tx.signing_hash());
    let signature = crate::hardware::Signature { id: request.id.clone(), signature: [&rs[..], &[parity]].concat(), origin: None };

    assert_eq!(crate::views::evm::finish(&tx, &request, &signature, from).unwrap().encode(), tx.clone().signed(&key).encode());
    assert!(crate::views::evm::finish(&tx, &request, &signature, [0xee; 20]).unwrap_err().contains("another key"));
    let other = crate::hardware::Signature { id: Some(vec![9; 16]), ..signature };
    assert!(crate::views::evm::finish(&tx, &request, &other, from).unwrap_err().contains("another request"));
}

#[test]
fn an_evm_chains_heads_read_with_or_without_a_finalized_block() {
    let reply = |id: u64, result: Value| json!({ "jsonrpc": "2.0", "id": id, "result": result });

    assert_eq!(evm::heads(&json!([reply(2, json!({ "number": "0x64" })), reply(1, json!("0x70"))])), Ok((0x70, Some(0x64))));
    // A chain without finality names no finalized block: depth is counted instead.
    assert_eq!(evm::heads(&json!([reply(1, json!("0x70")), reply(2, Value::Null)])), Ok((0x70, None)));
    assert!(evm::heads(&json!([])).is_err());
}

#[test]
fn a_leg_locking_an_nft_names_it_and_its_contract_is_asked_which() {
    let form = |asset: Asset, id: &str| LegForm { asset, id: id.to_string(), ..LegForm::new(Kind::Chain) };

    assert_eq!(form(Asset::Fungible, "").item(), Ok(None));
    assert_eq!(form(Asset::Nft, " 7 ").item(), Ok(Some(Item { id: u256::new(7), multi: false })));
    assert_eq!(form(Asset::Multi, "9").item(), Ok(Some(Item { id: u256::new(9), multi: true })));
    assert!(form(Asset::Nft, "seven").item().is_err());

    // The contract's item answer is kept beside get's, by the swap's hash.
    let (_, terms, _) = offered();
    let hash = to_hex(&terms.hash);
    let mut state = SwapsState::default();
    state.asked.insert(hash.clone(), 0.0);
    let asked = |function: &str| Ask::Call(json!({ "contract": "contract:0x5a", "function": function, "args": [{ "type": "string", "value": hash }] }).to_string());
    let returned = Ok(json!({ "status": "returned", "values": [{ "type": "string", "value": "nft" }, { "type": "u256", "value": "7" }] }));

    assert!(state.answered(&asked("item"), &returned));
    assert!(state.items.contains_key(&hash) && !state.status.contains_key(&hash));
    assert!(!state.answered(&asked("lock"), &returned), "not a read");
}
