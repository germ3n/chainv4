use {
    super::*,
    chainv4::crypto::mnemonic::Phrase,
};

fn seed() -> chainv4::crypto::mnemonic::Seed {
    Phrase::parse("abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about").unwrap().seed("")
}

fn bitcoin_key(seed: &chainv4::crypto::mnemonic::Seed, network: Network, index: u32) -> ([u8; 32], [u8; 33]) {
    let key = seed.bitcoin_key(network.coin(), 0, index);
    let (PrivateKey::Bitcoin(secret), PublicKey::Bitcoin(public)) = (key.private().clone(), key.public()) else { unreachable!() };

    (secret, public)
}

/// An offer by Alice, paying bitcoin, and Bob's answer.
fn offered() -> (Terms, Terms, [u8; 32]) {
    let seed = seed();
    let (secret, hash) = secret().unwrap();
    let now = 1_793_000_000_000;
    let offer = Terms {
        chain: Counterparty::BitcoinTestnet,
        contract: Address::Contract([0x5a; 32]),
        hash,
        sats: 80_000,
        token: None,
        amount: u256::new(5_000),
        starter: Pays::Bitcoin,
        bitcoin_payer: Some(bitcoin_key(&seed, Network::Test, 1).1),
        bitcoin_receiver: None,
        chain_payer: None,
        chain_receiver: Some(Address::Ed25519([0xa; 32])),
        first_deadline: now + FIRST_DEADLINE_MS,
        second_deadline: now + SECOND_DEADLINE_MS,
    };
    let answer = Terms {
        bitcoin_receiver: Some(bitcoin_key(&seed, Network::Test, 0).1),
        chain_payer: Some(Address::Ed25519([0xb; 32])),
        ..offer.clone()
    };

    (offer, answer, secret)
}

#[test]
fn an_answer_keeps_every_term_of_the_offer() {
    let (offer, answer, _) = offered();

    assert!(answers(&offer, &answer));
    assert!(!answers(&offer, &offer), "an offer answers nothing");
    assert!(!answers(&offer, &Terms { sats: 79_999, ..answer.clone() }), "less bitcoin");
    assert!(!answers(&offer, &Terms { chain_receiver: Some(Address::Ed25519([0xc; 32])), ..answer.clone() }), "paying someone else");
    assert!(!answers(&offer, &Terms { first_deadline: answer.first_deadline + 1, ..answer.clone() }));
    assert!(!answers(&offer, &Terms { contract: Address::Contract([0x66; 32]), ..answer.clone() }), "another contract");

    // Terms travel as a code the scanner reads.
    let shown = Message::Swap(Box::new(answer.clone()));
    assert_eq!(Message::from_ur(shown.ur_type(), &shown.bytes()).unwrap(), shown);
}

#[test]
fn the_contracts_answer_reads_as_where_the_swap_stands() {
    let value = |text: &str| json!({ "type": "string", "value": text });

    assert!(matches!(chain_state(None), ChainState::Unknown(_)));
    assert!(matches!(chain_state(Some(&Ok(vec![value("none")]))), ChainState::None));
    assert!(matches!(chain_state(Some(&Ok(vec![value("locked")]))), ChainState::Locked));
    let claimed = vec![value("claimed"), value(""), value(""), value(""), value(""), value(""), value("0x5e")];
    assert!(matches!(chain_state(Some(&Ok(claimed))), ChainState::Claimed { secret } if secret == "0x5e"));
    assert!(matches!(chain_state(Some(&Err("down".to_string()))), ChainState::Unknown(why) if why == "down"));

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
