use {
    super::*,
    crate::keys::{Holder, derive},
    chainv4::crypto::keypair::{KeyType, Keypair},
    chainv4::crypto::mnemonic::{Phrase, Seed},
};

/// The offline signer's words: Hardhat's, so account 1 is 0x7099….
fn seed() -> Seed {
    Phrase::parse("test test test test test test test test test test test junk").unwrap().seed("")
}

fn origin() -> Origin {
    Origin { account: 1, fingerprint: seed().ethereum_account().master_fingerprint }
}

fn key() -> Keypair {
    seed().key(KeyType::Secp256k1, 1)
}

fn secret() -> [u8; 32] {
    match key().private() {
        PrivateKey::Secp256k1(secret) => *secret,
        _ => unreachable!(),
    }
}

fn account() -> [u8; 20] {
    match key().public().address() {
        Address::Secp256k1(bytes) => bytes,
        _ => unreachable!(),
    }
}

fn shown(method: &str, params: Value) -> Shown {
    let request = Request::read(&json!({ "id": 7, "origin": "https://app.example", "method": method, "params": params })).unwrap();

    Shown {
        request,
        chosen: HashSet::new(),
        nonce: Some(4),
        gas: Some(21_000),
        price: Some(u256::from(2_000_000_000u64)),
        asked: true,
        sending: false,
        trouble: None,
        offline: None,
    }
}

/// What the offline signer shows back for the code shown: read off it, signed, and read back.
fn signed_on_the_signer(offline: &Offline) -> hardware::Signature {
    let shown = Message::MetaMask(Box::new(offline.request.clone()));
    let Message::MetaMask(read) = Message::from_ur(shown.ur_type(), &shown.bytes()).unwrap() else {
        unreachable!();
    };
    let answer = Message::EthSignature(Box::new(read.sign(&seed()).unwrap()));
    let Message::EthSignature(signature) = Message::from_ur(answer.ur_type(), &answer.bytes()).unwrap() else {
        unreachable!();
    };

    *signature
}

#[test]
fn a_message_signed_on_the_signer_is_what_signing_here_gives() {
    let shown = shown("personal_sign", json!(["0x68656c6c6f", to_hex(&account())]));
    let offline = ask_offline(&shown.request.asked, &shown, origin(), account(), Some(1)).unwrap();

    assert_eq!(offline.request.account(), Ok(1));
    assert_eq!(offline.request.origin.as_deref(), Some("chainv4 · https://app.example"));

    match finish(&offline, &signed_on_the_signer(&offline), Some(account())).unwrap() {
        Finished::Signature(signature) => assert_eq!(signature, dapp::signature(&secret(), &dapp::personal_digest(b"hello"))),
        Finished::Send(_) => panic!("a message sends nothing"),
    }
}

#[test]
fn typed_data_signed_on_the_signer_is_what_signing_here_gives() {
    let typed = json!({
        "types": { "EIP712Domain": [{ "name": "name", "type": "string" }, { "name": "chainId", "type": "uint256" }], "Mail": [{ "name": "contents", "type": "string" }] },
        "primaryType": "Mail",
        "domain": { "name": "Ether Mail", "chainId": 1 },
        "message": { "contents": "Hello, Bob!" },
    });
    let shown = shown("eth_signTypedData_v4", json!([to_hex(&account()), typed.to_string()]));
    let offline = ask_offline(&shown.request.asked, &shown, origin(), account(), Some(1)).unwrap();
    let Asked::Typed { data, .. } = &shown.request.asked else {
        panic!("typed data");
    };

    match finish(&offline, &signed_on_the_signer(&offline), Some(account())).unwrap() {
        Finished::Signature(signature) => assert_eq!(signature, dapp::signature(&secret(), &data.digest().unwrap())),
        Finished::Send(_) => panic!("typed data sends nothing"),
    }
}

#[test]
fn a_send_signed_on_the_signer_is_the_transaction_signing_here_makes() {
    let shown = shown("eth_sendTransaction", json!([{ "from": to_hex(&account()), "to": to_hex(&[0x35; 20]), "value": "0xde0b6b3a7640000" }]));
    let Asked::Send(wanted) = &shown.request.asked else {
        panic!("a send");
    };
    let chain = Some(38_262_171_930_895);
    let offline = ask_offline(&shown.request.asked, &shown, origin(), account(), chain).unwrap();

    assert_eq!((offline.request.kind, offline.request.chain_id), (DataKind::TypedTransaction, chain));

    match finish(&offline, &signed_on_the_signer(&offline), Some(account())).unwrap() {
        Finished::Send(tx) => assert_eq!(tx.encode(), transaction(wanted, &shown, chain).unwrap().signed(&secret()).encode()),
        Finished::Signature(_) => panic!("a send is a transaction"),
    }

    // Not priced yet, nothing to ask.
    let unpriced = Shown { gas: None, ..shown };
    assert!(ask_offline(&unpriced.request.asked, &unpriced, origin(), account(), chain).is_err());
}

#[test]
fn another_requests_or_another_keys_signature_is_refused() {
    let shown = shown("personal_sign", json!(["0x68656c6c6f", to_hex(&account())]));
    let offline = ask_offline(&shown.request.asked, &shown, origin(), account(), Some(1)).unwrap();
    let signature = signed_on_the_signer(&offline);

    let other = hardware::Signature { id: Some(vec![1; 16]), ..signature.clone() };
    assert!(finish(&offline, &other, Some(account())).err().unwrap().contains("another request"));
    assert!(finish(&offline, &signature, Some([9; 20])).err().unwrap().contains("not this account's"));

    // A pairing that names the wrong number: the signer refuses, as the address is not its key's there.
    let misnumbered = ask_offline(&shown.request.asked, &shown, Origin { account: 2, ..origin() }, account(), Some(1)).unwrap();
    assert!(misnumbered.request.sign(&seed()).is_err());
}

#[test]
fn sites_are_offered_the_signers_accounts_that_say_where_it_derives_them() {
    let keys = derive(&seed(), 1);
    let publics: Vec<_> = keys.iter().map(Keypair::public).collect();
    let holders = vec![
        Holder::watched("Cold".to_string(), publics.clone(), Some(origin())),
        Holder::watched("Old pairing".to_string(), derive(&seed(), 2).iter().map(Keypair::public).collect(), None),
        Holder::metamask("MetaMask".to_string(), derive(&seed(), 3)[2].public()),
    ];

    assert_eq!(offered(&holders), vec![("Cold".to_string(), account())], "a pairing that said nothing of where, and MetaMask's, are not");
}

#[test]
fn a_genesis_id_kept_as_hex_gives_the_chain_id_its_chain_has() {
    let genesis = chainv4::structs::hash::Hash::Sha256([0x12; 32]);
    assert_eq!(chain_of_genesis(&genesis.to_string()), Some(chain_id(&genesis)));
    assert_eq!(chain_of_genesis("0x1234"), None, "too short");
    assert_eq!(chain_of_genesis("not hex"), None);
}
