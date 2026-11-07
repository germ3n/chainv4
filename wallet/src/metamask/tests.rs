use {
    super::*,
    chainv4::crypto::keypair::{KeyType, Keypair, PrivateKey, Signature},
    chainv4::structs::address::Address,
    chainv4::structs::amount::Amount,
    chainv4::structs::hash::HashType,
    chainv4::structs::hex::from_hex,
    chainv4::structs::tx::SignedTx,
    chainv4::structs::u256::u256,
};

/// A 65-byte signature over a hash as MetaMask gives one, with v, from a key of our own.
fn as_metamask(key: &Keypair, hash: &[u8; 32]) -> [u8; 65] {
    let PrivateKey::Secp256k1(secret) = key.private() else {
        panic!("an Ethereum key");
    };
    let rs = secp256k1::sign_hash(secret, hash);

    [27, 28]
        .into_iter()
        .map(|v| {
            let mut signature = [0; 65];
            signature[..64].copy_from_slice(&rs);
            signature[64] = v;
            signature
        })
        .find(|signature| secp256k1::recover(hash, signature).is_some_and(|(recovered, _)| PublicKey::Secp256k1(recovered) == key.public()))
        .unwrap()
}

fn address(key: &Keypair) -> String {
    match key.public().address() {
        Address::Secp256k1(bytes) => to_hex(&bytes),
        _ => panic!("an Ethereum key"),
    }
}

#[test]
fn personal_sign_hashes_as_eip_191_says() {
    assert_eq!(personal_digest(b"hello").to_vec(), from_hex("0x50b2c43fd39106bafbba0da34fc430e1f91e3c96ea2acee2bc34119f92b37750").unwrap());
}

#[test]
fn connecting_learns_the_key_of_the_address_metamask_names_and_no_other() {
    let key = Keypair::generate(KeyType::Secp256k1).unwrap();
    let hash = personal_digest(CONNECT.as_bytes());
    let signed = as_metamask(&key, &hash);

    assert_eq!(key_of(&hash, &signed, &address(&key)).map(|(recovered, _)| PublicKey::Secp256k1(recovered)), Ok(key.public()));
    assert!(key_of(&hash, &signed, &address(&key).to_uppercase().replace("0X", "0x")).is_ok(), "MetaMask's mixed case");

    let other = Keypair::generate(KeyType::Secp256k1).unwrap();

    assert!(key_of(&hash, &signed, &address(&other)).unwrap_err().contains("not"));
}

#[test]
fn what_metamask_signs_of_the_typed_data_is_a_transaction_this_chain_takes() {
    let key = Keypair::generate(KeyType::Secp256k1).unwrap();
    let genesis = Hash::from_bytes(HashType::default_chain(), &[4; 32]).unwrap();
    let tx = Tx::Transfer {
        from: key.public().address(),
        to: Keypair::from_seed([9; 32]).public().address(),
        amount: Amount::U128(1_500_000_000_000_000_000),
    };
    let fee = FeeOffer { gas_limit: 21_000, max_price: u256::new(2_000_000_000), tip: u256::ZERO };
    let typed = typed_data(&tx, &[], 3, &fee, &genesis).unwrap();

    assert_eq!(typed["primaryType"], "Transfer");
    assert_eq!(typed["domain"]["name"], "chainv4");
    assert_eq!(typed["domain"]["salt"], to_hex(&[4; 32]), "the genesis id");
    assert_eq!(typed["message"]["amount"], "1500000000000000000", "wide numbers as decimal text");
    assert_eq!(typed["message"]["to"], tx_to(&tx));
    assert_eq!(typed["types"]["Transfer"][0], serde_json::json!({ "name": "from", "type": "string" }));

    // MetaMask signs the typed hash; recovered and checked, it is what the node takes.
    let hash = eip712::transaction_digest(&tx, &[], 3, &fee, &genesis).unwrap();
    let (recovered, low) = key_of(&hash, &as_metamask(&key, &hash), &address(&key)).unwrap();
    let signed = SignedTx {
        tx,
        nonce: 3,
        fee,
        signer: PublicKey::Secp256k1(recovered),
        signature: Signature::Secp256k1Eip712(low),
        functions: Vec::new(),
    };

    assert_eq!(signed.verify_on(&genesis), Ok(()));
}

fn tx_to(tx: &Tx) -> String {
    match tx {
        Tx::Transfer { to, .. } => to.to_string(),
        _ => unreachable!(),
    }
}

/// Writes typed data for each form, with the hash this wallet and the node sign, for MetaMask's
/// own library to hash and sign: `EIP712_OUT=dir cargo test eip712_for_metamask -- --ignored`.
#[test]
#[ignore = "writes files for a check against @metamask/eth-sig-util"]
fn eip712_for_metamask() {
    let out = std::path::PathBuf::from(std::env::var("EIP712_OUT").expect("EIP712_OUT names a folder"));
    // A key MetaMask's library imports too: 32 bytes of 0x11.
    let key = Keypair::from_key_file(&format!("secp256k1:0x{}", "11".repeat(32))).unwrap();
    let genesis = Hash::from_bytes(HashType::default_chain(), &[4; 32]).unwrap();
    let to = Keypair::from_seed([9; 32]).public().address();
    let fee = FeeOffer { gas_limit: 21_000, max_price: u256::new(2_000_000_000), tip: u256::new(7) };
    let forms = [
        ("transfer", Tx::Transfer { from: key.public().address(), to, amount: Amount::U128(1_500_000_000_000_000_000) }),
        (
            "token",
            Tx::ContractCall {
                to: Address::Contract([0x11; 32]),
                func: chainv4::crypto::hasher::Hasher::digest(HashType::Xxh3_64, b"transfer"),
                amount: Amount::U128(0),
                data: vec![chainv4::structs::tx::TxCallArgumentType::Address(to), chainv4::structs::tx::TxCallArgumentType::U256(u256::new(250))],
            },
        ),
        ("burn", Tx::Burn { from: key.public().address(), amount: Amount::U128(1) }),
        (
            "call",
            Tx::ContractCall {
                to: Address::Contract([0x22; 32]),
                func: chainv4::crypto::hasher::Hasher::digest(HashType::Xxh3_64, b"swap_exact"),
                amount: Amount::U128(9),
                data: vec![chainv4::structs::tx::TxCallArgumentType::Address(to), chainv4::structs::tx::TxCallArgumentType::U64(3)],
            },
        ),
        (
            "batch",
            Tx::Batch {
                steps: vec![
                    Tx::Transfer { from: key.public().address(), to, amount: Amount::U128(5) },
                    Tx::ContractCall { to: Address::Contract([0x22; 32]), func: chainv4::crypto::hasher::Hasher::digest(HashType::Xxh3_64, b"swap_exact"), amount: Amount::U128(0), data: Vec::new() },
                    Tx::Burn { from: key.public().address(), amount: Amount::U128(1) },
                ],
            },
        ),
    ];

    for (name, tx) in forms {
        let functions = eip712::name_calls(&tx, &["swap_exact".to_string()]).unwrap();
        let typed = typed_data(&tx, &functions, 3, &fee, &genesis).unwrap();
        let hash = eip712::transaction_digest(&tx, &functions, 3, &fee, &genesis).unwrap();
        std::fs::write(out.join(format!("{name}.json")), serde_json::to_string_pretty(&typed).unwrap()).unwrap();
        std::fs::write(out.join(format!("{name}.hash")), to_hex(&hash)).unwrap();
    }

    std::fs::write(out.join("address"), address(&key)).unwrap();
}

/// Checks the signatures MetaMask's library made of those files, written back as name.sig, as
/// the node checks them: `EIP712_OUT=dir cargo test eip712_from_metamask -- --ignored`.
#[test]
#[ignore = "reads the signatures @metamask/eth-sig-util made"]
fn eip712_from_metamask() {
    let out = std::path::PathBuf::from(std::env::var("EIP712_OUT").expect("EIP712_OUT names a folder"));
    let key = Keypair::from_key_file(&format!("secp256k1:0x{}", "11".repeat(32))).unwrap();

    for name in ["transfer", "token", "burn", "call", "batch"] {
        let hash: [u8; 32] = from_hex(&std::fs::read_to_string(out.join(format!("{name}.hash"))).unwrap()).unwrap().try_into().unwrap();
        let signed = signature(std::fs::read_to_string(out.join(format!("{name}.sig"))).unwrap().trim()).unwrap();
        let (recovered, _) = key_of(&hash, &signed, &address(&key)).unwrap();

        assert_eq!(PublicKey::Secp256k1(recovered), key.public(), "{name}: MetaMask's signature is the key's, over this wallet's hash");
    }
}
