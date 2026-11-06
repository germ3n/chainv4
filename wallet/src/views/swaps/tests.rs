use {
    super::*,
    crate::bitcoin::Signs,
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
    let alice = Parties { chain: Some(Address::Ed25519([0xa; 32])), bitcoin: Some(chainv4::swap::BitcoinParty::Key(bitcoin_key(&seed, Network::Test, 1).1)), evm: None };
    let bob = Parties { chain: Some(Address::Ed25519([0xb; 32])), bitcoin: Some(chainv4::swap::BitcoinParty::Key(bitcoin_key(&seed, Network::Test, 0).1)), evm: None };
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
    // Each leg is kept by its swap id, the contract's key for it.
    let id = id_of(&terms, Side::Second);
    assert_eq!(id, to_hex(&terms.swap_id(Side::Second).unwrap()));
    assert_eq!(id_of(&terms, Side::First), "", "Bitcoin's HTLC has none");

    let read = |state: &SwapsState| {
        let text = |value: &Value| value["value"].as_str().map_or_else(|| value["value"].to_string(), str::to_string);
        state.status.get(&id).cloned().map(|answer| answer.and_then(|values| Lock::from_chain(&values.iter().map(text).collect::<Vec<_>>())))
    };

    assert_eq!(describe(leg, Some(&Err("asking".to_string())), terms.second_deadline), "not known yet: asking");
    state.status.insert(id.clone(), Ok(vec![value("none")]));
    assert_eq!(describe(leg, read(&state).as_ref(), terms.second_deadline), "nothing locked yet");
    state.status.insert(id.clone(), Ok(locked("5000")));
    assert_eq!(describe(leg, read(&state).as_ref(), terms.second_deadline), "locked as the terms say, waiting for its claim");
    state.status.insert(id.clone(), Ok(locked("4000")));
    assert!(describe(leg, read(&state).as_ref(), terms.second_deadline).starts_with("locked, but not as the terms say"), "less than agreed");
    assert!(describe(&terms.first, None, terms.first_deadline).contains("HTLC"));

    // A claim whose payout the token refused reads as owed, as get's last value says.
    let claimed = ["claimed", &alice, &alice, &zero, "5000", &deadline, &to_hex(&[7; 32]), "9"].iter().map(|text| value(text)).chain([json!({ "type": "bool", "value": true })]).collect();
    state.status.insert(id.clone(), Ok(claimed));
    assert!(describe(leg, read(&state).as_ref(), terms.second_deadline).contains("owed"));

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

    // The contract's item answer is kept beside get's, by the leg's swap id.
    let (_, terms, _) = offered();
    let id = id_of(&terms, Side::Second);
    let mut state = SwapsState::default();
    state.asked.insert(id.clone(), 0.0);
    let asked = |function: &str| Ask::Call(json!({ "contract": "contract:0x5a", "function": function, "args": [{ "type": "string", "value": id }] }).to_string());
    let returned = Ok(json!({ "status": "returned", "values": [{ "type": "string", "value": "nft" }, { "type": "u256", "value": "7" }] }));

    assert!(state.answered(&asked("item"), &returned));
    assert!(state.items.contains_key(&id) && !state.status.contains_key(&id));
    assert!(!state.answered(&asked("lock"), &returned), "not a read");
}

#[test]
fn a_multisig_swaps_with_its_keys_at_the_swap_s_index_and_its_members_sign_the_claim() {
    use chainv4::crypto::bitcoin::multisig::xpub_text;
    use chainv4::swap::BitcoinParty;

    let seeds: Vec<_> = [
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
        "legal winner thank year wave sausage worth useful legal winner thank yellow",
        "letter advice cage absurd amount doctor acoustic avoid letter advice cage above",
    ]
    .iter()
    .map(|words| Phrase::parse(words).unwrap().seed(""))
    .collect();
    let members: Vec<Value> = seeds.iter().map(|seed| json!({ "key": "ed25519:0x00", "bitcoin": xpub_text(&seed.multisig_account(1, 0), Network::Test), "evm": "0x11" })).collect();
    let policy = json!({ "members": members, "threshold": 2, "testnet": true });
    let wallet = super::super::multisig::bitcoin::wallet_of(&policy).unwrap();
    let multisig = Address::Multisig([4; 32]);
    let (secret, hash) = secret().unwrap();

    // Bob, a 2-of-3, answers Alice's bitcoin with its quorum's keys at the swap's index.
    let quorum = BitcoinParty::of_multisig(&wallet, chainv4::swap::multisig_index(&hash)).unwrap();
    let alice = Parties { chain: Some(Address::Ed25519([0xa; 32])), bitcoin: Some(BitcoinParty::Key(bitcoin_key(&seeds[0], Network::Test, 1).1)), evm: None };
    let bob = Parties { chain: Some(multisig), bitcoin: Some(quorum.clone()), evm: None };
    let pays = Leg::Bitcoin(BitcoinLeg { testnet: true, sats: 80_000, payer: None, receiver: None });
    let gets = Leg::Chain(ChainLeg { item: None, contract: Address::Contract([0x5a; 32]), token: None, amount: u256::new(5_000), payer: None, receiver: None });
    let terms = Terms::offer(hash, pays, gets, NOW + FIRST_DEADLINE_MS, NOW + SECOND_DEADLINE_MS, &alice).unwrap().answer(&bob).unwrap();

    assert_eq!(terms.htlc().unwrap().claim, quorum, "the multisig receives the bitcoin, so takes the claim");

    // Its claim is a PSBT for the members; two of them sign with their words, and it finishes.
    let form = BitcoinForm {
        funding: format!("{}:0", "ab".repeat(32)),
        sats: "80000".to_string(),
        to: chainv4::crypto::bitcoin::multisig::bitcoin_address(2, &wallet.xpubs, Network::Test, 0, 0).unwrap(),
        fee_rate: "2".to_string(),
        ..BitcoinForm::default()
    };
    let Ok(Signed::Members(shared)) = for_members(&terms, Spend::Claim { secret }, &form, &wallet) else {
        panic!("a PSBT for the members");
    };
    let mut psbt = shared.psbt.sign(&seeds[1]).unwrap();
    psbt.combine(&shared.psbt.sign(&seeds[2]).unwrap()).unwrap();
    let htlc = terms.htlc().unwrap();
    let claim = psbt.finish_htlc(&htlc, 80_000, Spend::Claim { secret }).unwrap();

    assert_eq!(claim.inputs[0].witness.len(), 6);
    assert!(matches!(for_members(&terms, Spend::Refund, &form, &wallet), Err(why) if why.contains("not the multisig's")), "the refund is Alice's");
}

#[test]
fn a_swap_s_steps_mark_what_is_done_the_one_now_and_the_rest_to_come() {
    use super::Step::{Done, Later, Now};

    assert_eq!(super::marks([true, false, false, false, false, false]), [Done, Now, Later, Later, Later, Later], "offered, waiting for the answer");
    assert_eq!(super::marks([true, true, true, false, false, false]), [Done, Done, Done, Now, Later, Later], "the first lock in");
    assert_eq!(super::marks([true; 6]), [Done; 6], "both claimed");
    // A step seen done out of order, such as a Bitcoin lock the light client cannot see, leaves the
    // earliest open one as now's.
    assert_eq!(super::marks([true, true, false, true, false, false]), [Done, Done, Now, Done, Later, Later]);
}

#[test]
fn a_wallet_that_needs_biometrics_starts_no_swap_as_its_words_would_not_bring_the_secret_back() {
    use crate::{biometrics::fake, guard::{Guard, tagged}, vault::{Book, Vault, fresh_secret}};

    fake::fresh();
    let mut book = crate::views::drawn::two_phrases();
    let mut holders = crate::keys::open(&book).unwrap();
    let secret = fresh_secret().unwrap();
    let vault = Vault::keyed(&secret).unwrap();
    let entry = crate::app::WalletEntry { id: String::new(), name: "Mine".to_string(), vault: vault.seal(&Book::default()).unwrap(), current: String::new(), biometric: tagged(Guard::Biometric, &fake::keep(&secret)), unprotected: false, born: None };
    let mut world = crate::views::drawn::World { settings: crate::app::Settings::with_wallets(vec![entry]), vault: Some(vault), ..crate::views::drawn::World::default() };
    let state = SwapsState::default();
    let mut said = None;

    crate::views::drawn::page_in(&mut world, &mut book, &mut holders, (360.0, 740.0), |_, ctx| said = Some(offer(ctx, &state)));

    assert!(said.unwrap().err().unwrap().contains("words cannot bring back"));
}

#[test]
fn a_swap_is_acted_on_from_the_address_its_terms_name_whatever_is_picked_now() {
    let holder = crate::keys::open(&crate::views::drawn::two_phrases()).unwrap().remove(0);
    let mine = holder.addresses();
    let leg = |amount: u64| Leg::Chain(ChainLeg { item: None, contract: Address::Contract([5; 32]), token: None, amount: u256::from(amount), payer: None, receiver: None });
    let bitcoin_side = Parties { chain: Some(mine[3]), ..Parties::default() };
    let terms = Terms::offer([1; 32], leg(10), leg(20), NOW + 7_200_000, NOW + 3_600_000, &bitcoin_side).unwrap();

    assert_eq!(named_in(&terms, &holder), Some(mine[3]), "offered from the Bitcoin address, so it locks and refunds from there");

    let stranger = Terms::offer([1; 32], leg(10), leg(20), NOW + 7_200_000, NOW + 3_600_000, &Parties { chain: Some(Address::Ed25519([9; 32])), ..Parties::default() }).unwrap();

    assert_eq!(named_in(&stranger, &holder), None);
}

#[test]
fn a_second_lock_on_bitcoin_is_refunded_by_the_wallet_at_a_high_fee_once_its_deadline_passes() {
    let seed = seed();
    let (refunder, ours) = bitcoin_key(&seed, Network::Test, 0);
    let (_, hash) = secret().unwrap();
    let alice = Parties { chain: Some(Address::Ed25519([0xa; 32])), bitcoin: Some(chainv4::swap::BitcoinParty::Key(bitcoin_key(&seed, Network::Test, 1).1)), evm: None };
    let bob = Parties { chain: Some(Address::Ed25519([0xb; 32])), bitcoin: Some(chainv4::swap::BitcoinParty::Key(ours)), evm: None };
    // Alice pays this chain's coin first; Bob, answering, pays bitcoin second.
    let pays = Leg::Chain(ChainLeg { item: None, contract: Address::Contract([0x5a; 32]), token: None, amount: u256::new(5_000), payer: None, receiver: None });
    let gets = Leg::Bitcoin(BitcoinLeg { testnet: true, sats: 80_000, payer: None, receiver: None });
    let terms = Terms::offer(hash, pays, gets, NOW + FIRST_DEADLINE_MS, NOW + SECOND_DEADLINE_MS, &alice).unwrap().answer(&bob).unwrap();
    let deadline = terms.second_deadline;

    // Due for Bob alone, and only from the deadline on: the starter's own lock, the first, has
    // no late claim to race.
    assert!(second_refund_due(&terms, false, deadline - 1_000).is_none());
    assert!(second_refund_due(&terms, true, deadline).is_none(), "the starter pays no second lock");
    let (htlc, network) = second_refund_due(&terms, false, deadline).unwrap();
    assert_eq!((htlc.refund.clone(), network), (chainv4::swap::BitcoinParty::Key(ours), Network::Test));
    let (_, first_on_bitcoin, _) = offered();
    assert!(second_refund_due(&first_on_bitcoin, false, first_on_bitcoin.first_deadline).is_none(), "Bitcoin as the first leg");

    // Signed here at the high fee, back to Bob, with the lock time Bitcoin checks it against.
    let to = bech32::encode_segwit("tb", &hash160(&ours));
    let refund = urgent_refund(&htlc, network, &refunder, format!("{}:0", "cd".repeat(32)), "80000".to_string(), to.clone()).unwrap();
    assert_eq!(refund.lock_time, htlc.locktime);
    assert_eq!(revealed(&refund, &hash), None);
    assert_eq!((80_000 - refund.outputs[0].value) / refund.vsize(), chainv4::swap::htlc::URGENT_FEE_RATE, "the urgent rate, for a witness priced as long as DER makes it");

    // Coins too few for it pay the highest rate they can; none, and it says why.
    let small = urgent_refund(&htlc, network, &refunder, format!("{}:0", "cd".repeat(32)), "2000".to_string(), to.clone()).unwrap();
    assert!(small.outputs[0].value >= chainv4::swap::htlc::DUST);
    assert!(urgent_refund(&htlc, network, &refunder, format!("{}:0", "cd".repeat(32)), "300".to_string(), to).unwrap_err().contains("too few"));
}
