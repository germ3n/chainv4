use {
    super::*,
    chainv4::crypto::mnemonic::Phrase,
    chainv4::structs::hex::from_hex,
    chainv4::structs::u256::u256,
};

/// Hardhat's development phrase: account n's address is every Ethereum wallet's for these words.
fn seed() -> Seed {
    Phrase::parse("test test test test test test test test test test test junk").unwrap().seed("")
}

fn hex(text: &str) -> Vec<u8> {
    from_hex(&format!("0x{text}")).unwrap()
}

/// What a signature recovers to: the address that signed.
fn signer(digest: &[u8; 32], signature: &[u8]) -> String {
    let parity = match signature[64..] {
        [v] if v >= 27 => v - 27,
        [v] => v,
        _ => panic!("a legacy one's v, worked out by its caller"),
    };
    let key = secp256k1::recover_strict(digest, signature[..64].try_into().unwrap(), parity).unwrap();

    to_hex(&secp256k1::address(&key).unwrap())
}

fn request(kind: DataKind, data: Vec<u8>, account: u32) -> SignRequest {
    SignRequest {
        id: Some(vec![0x9b; 16]),
        data,
        kind,
        chain_id: Some(38_262_171_930_895),
        path: KeyPath {
            steps: vec![Step::Index(44, true), Step::Index(60, true), Step::Index(0, true), Step::Index(0, false), Step::Index(account, false)],
            fingerprint: Some(seed().ethereum_account().master_fingerprint),
            depth: None,
        },
        address: None,
        origin: Some("metamask".to_string()),
    }
}

fn transaction(kind: EthereumKind) -> EthereumTx {
    EthereumTx {
        kind,
        chain_id: 38_262_171_930_895,
        nonce: 3,
        max_price: u256::from(2_000_000_000u64),
        tip: u256::from(1_000u64),
        gas_limit: 21_000,
        to: Some([0x35; 20]),
        value: u256::from(10u64).pow(18),
        input: Vec::new(),
        access_list: Vec::new(),
        signature: [0; 64],
        parity: 0,
    }
}

#[test]
fn codes_are_byte_for_byte_keystones() {
    // Keystone's own: an extended key with a coin's network, an origin path and a parent.
    let key = HdKey {
        key: hex("026fe2355745bb2db3630bbc80ef5d58951c963c841f54170ba6e5c12be7fc12a6").try_into().unwrap(),
        chain_code: Some(hex("ced155c72456255881793514edc5bd9447e7f74abb88c6d6b6480fd016ee8c85").try_into().unwrap()),
        coin: Some((None, Some(1))),
        origin: Some(KeyPath {
            steps: vec![Step::Index(44, true), Step::Index(1, true), Step::Index(1, true), Step::Index(0, false), Step::Index(1, false)],
            fingerprint: None,
            depth: None,
        }),
        children: None,
        parent_fingerprint: Some([0xe9, 0x18, 0x1c, 0xf3]),
        name: None,
        note: None,
    };
    let cbor = key.to_cbor().unwrap();

    assert_eq!(
        to_hex(&cbor),
        "0xa5035821026fe2355745bb2db3630bbc80ef5d58951c963c841f54170ba6e5c12be7fc12a6045820ced155c72456255881793514edc5bd9447e7f74abb88c6d6b6480fd016ee8c8505d90131a1020106d90130a1018a182cf501f501f500f401f4081ae9181cf3"
    );
    assert_eq!(
        ur(HDKEY_TYPE, &cbor).to_ascii_lowercase(),
        "ur:crypto-hdkey/onaxhdclaojlvoechgferkdpqdiabdrflawshlhdmdcemtfnlrctghchbdolvwsednvdztbgolaahdcxtottgostdkhfdahdlykkecbbweskrymwflvdylgerkloswtbrpfdbsticmwylklpahtaadehoyaoadamtaaddyoyadlecsdwykadykadykaewkadwkaycywlcscewfihbdaehn"
    );

    // Keystone's request, as MetaMask writes them.
    let read = SignRequest::from_cbor(&hex(
        "a601d825509b1deb4d3b7d4bad9bdd2b0d7b3dcb6d02584bf849808609184e72a00082271094000000000000000000000000000000000000000080a47f74657374320000000000000000000000000000000000000000000000000000006000578080800301040105d90130a2018a182cf501f501f500f401f4021a1234567807686d6574616d61736b",
    ))
    .unwrap();

    assert_eq!(read.path.text(), "44'/1'/1'/0/1");
    assert_eq!(read.path.fingerprint, Some([0x12, 0x34, 0x56, 0x78]));
    assert_eq!((read.kind, read.chain_id, read.origin.as_deref()), (DataKind::Transaction, Some(1), Some("metamask")));
    assert_eq!(read.id, Some(hex("9b1deb4d3b7d4bad9bdd2b0d7b3dcb6d")));
    assert!(read.account().is_err(), "not a path this wallet signs at");
    assert_eq!(
        to_hex(&read.to_cbor().unwrap()),
        "0xa601d825509b1deb4d3b7d4bad9bdd2b0d7b3dcb6d02584bf849808609184e72a00082271094000000000000000000000000000000000000000080a47f74657374320000000000000000000000000000000000000000000000000000006000578080800301040105d90130a2018a182cf501f501f500f401f4021a1234567807686d6574616d61736b",
        "written back as read"
    );

    // Keystone's signature.
    let signature = Signature {
        id: Some(hex("9b1deb4d3b7d4bad9bdd2b0d7b3dcb6d")),
        signature: hex("d4f0a7bcd95bba1fbb1051885054730e3f47064288575aacc102fbbf6a9a14daa066991e360d3e3406c20c00a40973eff37c7d641e5b351ec4a99bfe86f335f713"),
        origin: Some("keystone".to_string()),
    };

    assert_eq!(
        to_hex(&signature.to_cbor().unwrap()),
        "0xa301d825509b1deb4d3b7d4bad9bdd2b0d7b3dcb6d025841d4f0a7bcd95bba1fbb1051885054730e3f47064288575aacc102fbbf6a9a14daa066991e360d3e3406c20c00a40973eff37c7d641e5b351ec4a99bfe86f335f71303686b657973746f6e65"
    );

    // Read back, as the wallet reads a signer's answer for a site.
    assert_eq!(Signature::from_cbor(&signature.to_cbor().unwrap()).unwrap(), signature);
    assert!(Signature::from_cbor(&[signature.to_cbor().unwrap(), vec![0]].concat()).is_err(), "bytes after it");

    // Anything past the request's map is refused.
    let mut longer = hex("a2020103d82550");
    longer.push(0);

    assert!(SignRequest::from_cbor(&longer).is_err());
}

#[test]
fn the_pairing_code_names_the_seed_and_ethereums_account_level() {
    let account = seed().ethereum_account();
    let key = HdKey::ethereum(&account);

    assert_eq!(key.origin.as_ref().unwrap().text(), "44'/60'/0'");
    assert_eq!(key.origin.as_ref().unwrap().fingerprint, Some(account.master_fingerprint));
    assert_eq!(key.children.as_ref().unwrap().text(), "0/*");
    assert_eq!(key.note.as_deref(), Some("account.standard"));

    let code = pairing(&seed()).unwrap();

    assert!(code.starts_with("UR:CRYPTO-HDKEY/"), "{code}");
    assert!(code.len() < 400, "one code: {} characters", code.len());
}

#[test]
fn every_kind_signs_as_metamask_reads_it_back() {
    let account = "0x70997970c51812dc3a010c7d01b50e0d17dc79c8";

    // A typed transaction: v is its parity alone.
    let tx = transaction(EthereumKind::DynamicFee);
    let signed = request(DataKind::TypedTransaction, tx_data(&tx), 1).sign(&seed()).unwrap();

    assert_eq!(signed.id, Some(vec![0x9b; 16]));
    assert_eq!(signed.signature.len(), 65);
    assert!(signed.signature[64] <= 1);
    assert_eq!(signer(&tx.signing_hash(), &signed.signature), account);

    // A legacy one: EIP-155's v, chain id and all, as many bytes as it takes.
    let tx = transaction(EthereumKind::Legacy);
    let signed = request(DataKind::Transaction, tx_data(&tx), 1).sign(&seed()).unwrap();
    let v = signed.signature[64..].iter().fold(0u64, |v, byte| v << 8 | u64::from(*byte));
    let parity = (v - 35 - tx.chain_id * 2) as u8;

    assert!(parity <= 1, "v = {v}");
    assert_eq!(signer(&tx.signing_hash(), &[&signed.signature[..64], &[parity][..]].concat()), account);

    // A message, as personal_sign: 27 or 28.
    let signed = request(DataKind::Message, b"Sign in to chainv4".to_vec(), 1).sign(&seed()).unwrap();

    assert!(signed.signature[64] == 27 || signed.signature[64] == 28);
    assert_eq!(signer(&crate::metamask::personal_digest(b"Sign in to chainv4"), &signed.signature), account);

    // Typed data.
    let mail = serde_json::json!({
        "types": { "EIP712Domain": [{ "name": "name", "type": "string" }, { "name": "chainId", "type": "uint256" }], "Note": [{ "name": "text", "type": "string" }] },
        "primaryType": "Note",
        "domain": { "name": "chainv4", "chainId": 38_262_171_930_895u64 },
        "message": { "text": "hi" },
    });
    let request = request(DataKind::TypedData, mail.to_string().into_bytes(), 0);
    let signed = request.sign(&seed()).unwrap();
    let Ok(Asked::TypedData(data, _)) = request.asked() else { panic!("typed data") };

    assert_eq!(signer(&data.digest().unwrap(), &signed.signature), "0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266");
    assert_eq!(request.chain(), Some(38_262_171_930_895), "the domain's chain");
}

/// A transaction's bytes before it is signed, as MetaMask hands them over.
fn tx_data(tx: &EthereumTx) -> Vec<u8> {
    tx.signing_data()
}

#[test]
fn what_does_not_hold_is_refused_before_anything_is_signed() {
    let tx = transaction(EthereumKind::DynamicFee);
    let good = request(DataKind::TypedTransaction, tx_data(&tx), 1);

    // Another seed's request.
    let mut other = good.clone();
    other.path.fingerprint = Some([1, 2, 3, 4]);

    assert!(other.sign(&seed()).unwrap_err().contains("another wallet"));

    // An address that is not the key's at the path.
    let mut wrong = good.clone();
    wrong.address = Some([7; 20]);

    assert!(wrong.sign(&seed()).is_err());

    // The request names one chain, the transaction another.
    let mut chain = good.clone();
    chain.chain_id = Some(1);

    assert!(chain.asked().unwrap_err().contains("chain"));

    // The kind the request says, against what the bytes are.
    let mut kind = good.clone();
    kind.kind = DataKind::Transaction;

    assert!(kind.asked().is_err());

    // A path this wallet holds no key at.
    let mut path = good.clone();
    path.path.steps[2] = Step::Index(1, true);

    assert!(path.sign(&seed()).unwrap_err().contains("m/44'/60'/1'/0/1"));
    assert_eq!(chain_name(1, None), "Ethereum");
    assert_eq!(chain_name(38_262_171_930_895, Some(38_262_171_930_895)), "your chain");
    assert_eq!(chain_name(5, None), "chain 5");
}

#[test]
fn an_animated_request_is_read_from_any_parts_enough_in_any_order() {
    let tx = EthereumTx { input: vec![0xab; 600], gas_limit: 300_000, ..transaction(EthereumKind::DynamicFee) };
    let request = request(DataKind::TypedTransaction, tx_data(&tx), 1);
    let cbor = request.to_cbor().unwrap();

    assert_eq!(SignRequest::from_cbor(&cbor).unwrap(), request);

    // Cut in parts, as MetaMask animates it; read starting midway, skipping some, in capitals as a
    // QR code holds them, until the fountain's mixed parts fill in what was missed.
    let mut encoder = ur::Encoder::new(&cbor, 100, REQUEST_TYPE).unwrap();
    let parts: Vec<String> = (0..60).map(|_| encoder.next_part().unwrap()).collect();
    let count = encoder.fragment_count();
    let mut assembly = Assembly::default();
    let mut read = None;

    assert!(count > 3);

    for (index, part) in parts.iter().enumerate().skip(2) {
        if index % 3 == 0 {
            continue;
        }

        if let Some((kind, cbor)) = assembly.add(&part.to_ascii_uppercase()).unwrap() {
            assert_eq!(kind, REQUEST_TYPE);
            read = Some(cbor);

            break;
        }

        let (resolved, of) = assembly.progress().unwrap();
        assert!(resolved <= of && of == count);
    }

    assert_eq!(read, Some(cbor.clone()));

    // A whole one, and one of another type.
    assert_eq!(assembly.add(&ur(REQUEST_TYPE, &cbor)).unwrap(), Some((REQUEST_TYPE.to_string(), cbor)));
    assert!(assembly.add("ur:crypto-psbt/abc").is_err(), "not bytewords");
    assert!(assembly.add("CV4:1:PAIR:XYZ").is_err());
}

/// Requests as MetaMask's QR keyring makes them, with Keystone's own library: an EIP-1559
/// transaction, cut into ten fragments and fountain-coded, a legacy one, typed data and a message,
/// each for account 1 of the test words. Made by a script against @keystonehq/bc-ur-registry-eth,
/// ethereumjs and ethers.
const METAMASK: &str = include_str!("metamask.json");

#[test]
fn metamasks_own_requests_read_and_sign_for_the_account_it_asked() {
    let fixture: serde_json::Value = serde_json::from_str(METAMASK).unwrap();
    let seed = seed();

    assert_eq!(fixture["xfp"].as_str().unwrap(), to_hex(&seed.ethereum_account().master_fingerprint).trim_start_matches("0x"), "the same seed, by MetaMask's word");

    let mut written = Vec::new();

    for (name, request) in fixture["requests"].as_object().unwrap() {
        let parts: Vec<&str> = request["parts"].as_array().unwrap().iter().map(|part| part.as_str().unwrap()).collect();
        let mut assembly = Assembly::default();
        // Started late, missing every third part: the fountain's mixed parts make up the rest.
        let cbor = parts
            .iter()
            .enumerate()
            .filter(|(index, _)| parts.len() == 3 || (*index >= 4 && index % 3 != 0))
            .find_map(|(_, part)| assembly.add(&part.to_ascii_uppercase()).unwrap())
            .map(|(_, bytes)| bytes)
            .unwrap_or_else(|| panic!("{name}: every part read"));
        let read = SignRequest::from_cbor(&cbor).unwrap();

        assert_eq!(read.id.as_ref().map(|id| to_hex(id)), Some(format!("0x{}", request["id"].as_str().unwrap())), "{name}");
        assert_eq!(read.address.map(|address| to_hex(&address)), Some(fixture["address"].as_str().unwrap().to_string()), "{name}");
        assert_eq!(read.account().unwrap(), 1, "{name}");
        assert_eq!(read.chain(), Some(38_262_171_930_895), "{name}");

        let signature = read.sign(&seed).unwrap();
        let hash: [u8; 32] = from_hex(request["hash"].as_str().unwrap()).unwrap().try_into().unwrap();
        let parity = match (read.kind, signature.signature[64..].to_vec()) {
            (DataKind::Transaction, v) => (v.iter().fold(0u64, |v, byte| v << 8 | u64::from(*byte)) - 35 - 2 * 38_262_171_930_895) as u8,
            (_, v) if v[0] >= 27 => v[0] - 27,
            (_, v) => v[0],
        };

        assert_eq!(signer(&hash, &[&signature.signature[..64], &[parity][..]].concat()), fixture["address"].as_str().unwrap(), "{name}: the hash MetaMask computed, signed by the key it asked for");
        written.push(serde_json::json!({ "name": name, "ur": ur(SIGNATURE_TYPE, &signature.to_cbor().unwrap()) }));
    }

    // For the script that checks them back with Keystone's library, when asked.
    if let Ok(path) = std::env::var("CHAINV4_METAMASK_OUT") {
        let out = serde_json::json!({ "pairing": pairing(&seed).unwrap(), "signatures": written });
        std::fs::write(path, out.to_string()).unwrap();
    }
}

#[test]
fn the_wallets_own_requests_name_the_signers_key_and_the_site() {
    let seed = seed();
    let origin = crate::airgap::Origin { account: 1, fingerprint: seed.ethereum_account().master_fingerprint };
    let address = secp256k1::address(&secp256k1::public_key(&match seed.key(KeyType::Secp256k1, 1).private() {
        PrivateKey::Secp256k1(secret) => *secret,
        _ => unreachable!(),
    }).unwrap()).unwrap();
    let asked = SignRequest::asking(b"hello".to_vec(), DataKind::Message, Some(1), origin, address, "https://app.example");
    let read = SignRequest::from_cbor(&asked.to_cbor().unwrap()).unwrap();
    let id = asked.id.clone().unwrap();

    assert_eq!(read, asked);
    assert_eq!((read.account(), read.path.fingerprint), (Ok(1), Some(origin.fingerprint)));
    assert_eq!((id.len(), id[6] >> 4, id[8] >> 6), (16, 4, 2), "a version 4 UUID");
    assert_ne!(SignRequest::asking(Vec::new(), DataKind::Message, None, origin, address, "x").id, asked.id, "each its own");
    assert_eq!(read.asker(), "A site asks, through the wallet · https://app.example");
    assert_eq!(request(DataKind::Message, Vec::new(), 0).asker(), "MetaMask asks · metamask");

    // The signer signs it as MetaMask's, for the account it names; the parts come back out.
    let signature = Signature::from_cbor(&read.sign(&seed).unwrap().to_cbor().unwrap()).unwrap();
    let (rs, parity) = signature.parts().unwrap();

    assert_eq!(signature.id, asked.id);
    assert_eq!(signer(&crate::metamask::personal_digest(b"hello"), &[rs.as_slice(), &[parity]].concat()), to_hex(&address));
    assert_eq!(Signature { signature: vec![0; 64], ..signature.clone() }.parts(), None, "no v");
    assert_eq!(Signature { signature: [vec![0; 64], vec![0x01, 0x23]].concat(), ..signature }.parts().map(|(_, parity)| parity), Some(0), "EIP-155's v, 291: 291 - 35 is even");
}
