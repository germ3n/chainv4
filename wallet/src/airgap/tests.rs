use {
    super::*,
    chainv4::crypto::keypair::KeyType,
    chainv4::structs::address::Address,
    chainv4::structs::amount::Amount,
    qrcode::{Color, EcLevel, QrCode},
};

fn key() -> Keypair {
    Keypair::from_seed([7; 32])
}

/// The chain the requests here are for, and another a hostile sender might name.
fn chain() -> Hash {
    Hasher::digest(HashType::default_chain(), b"the chain the sender is on")
}

fn another_chain() -> Hash {
    Hasher::digest(HashType::default_chain(), b"another chain of the same keys")
}

/// Someone else's address, a real key's, as an Ed25519 address must be.
fn someone(seed: u8) -> Address {
    Keypair::from_seed([seed; 32]).public().address()
}

fn transfer(key: &Keypair) -> Request {
    Request {
        tx: Tx::Transfer {
            from: key.public().address(),
            to: someone(0xab),
            amount: Amount::U128(1_500_000_000_000_000_000),
        },
        nonce: 3,
        fee: FeeOffer {
            gas_limit: 21_000,
            max_price: u256::new(2_000_000_000),
            tip: u256::ZERO,
        },
        signer: key.public(),
        coin: Shown {
            symbol: "DEV".to_string(),
            decimals: 18,
        },
        token: None,
        collection: None,
        genesis: chain(),
    }
}

fn token_transfer(key: &Keypair) -> Request {
    Request {
        tx: Tx::ContractCall {
            to: Address::Contract([0x11; 32]),
            func: Hasher::digest(HashType::Xxh3_64, b"transfer"),
            amount: Amount::U128(0),
            data: vec![TxCallArgumentType::Address(someone(0xcd)), TxCallArgumentType::U256(u256::new(250))],
        },
        token: Some(Shown {
            symbol: "GOLD".to_string(),
            decimals: 2,
        }),
        ..transfer(key)
    }
}

/// A QR code of the text, as a photo of a screen would hold it: dark on white, some pixels a
/// module, with a white margin, and the rest of the frame grey.
fn photo(text: &str, module: u32, frame: u32) -> image::GrayImage {
    let code = QrCode::with_error_correction_level(text.as_bytes(), EcLevel::M).unwrap();
    let width = code.width() as u32;
    let colors = code.to_colors();
    let side = (width + 8) * module;
    let offset = (frame.saturating_sub(side)) / 2;

    image::GrayImage::from_fn(frame.max(side), frame.max(side), |x, y| {
        let (x, y) = (x.wrapping_sub(offset), y.wrapping_sub(offset));

        if x >= side || y >= side {
            return image::Luma([150]);
        }

        let (cx, cy) = (x / module, y / module);
        let dark = (4..width + 4).contains(&cx) && (4..width + 4).contains(&cy) && colors[((cy - 4) * width + cx - 4) as usize] == Color::Dark;

        image::Luma([if dark { 20 } else { 240 }])
    })
}

fn encoded(image: &image::GrayImage, format: image::ImageFormat) -> Vec<u8> {
    let mut bytes = std::io::Cursor::new(Vec::new());
    image.write_to(&mut bytes, format).unwrap();

    bytes.into_inner()
}

#[test]
fn every_message_reads_back_from_its_ur_and_a_short_one_fits_one_code() {
    let key = key();
    let pairing = Pairing {
        name: "Main".to_string(),
        keys: KeyType::ALL.iter().map(|scheme| Keypair::from_seed([*scheme as u8 + 1; 32]).public()).collect(),
        origin: Some(crate::airgap::Origin { account: 3, fingerprint: [0x73, 0xc5, 0xda, 0x0a] }),
    };
    let signed = sign(&transfer(&key), &key, None).unwrap();
    let account = chainv4::structs::address::Address::Multisig([5; 32]);
    let mut bundle = chainv4::registry::multisig::MultisigBundle {
        genesis: chainv4::structs::hash::Hash::zero(chainv4::structs::hash::HashType::default_chain()),
        body: chainv4::registry::multisig::MultisigBody { account, tx: transfer(&key).tx, nonce: 3, fee: FeeOffer::NONE },
        signatures: Vec::new(),
        functions: Vec::new(),
    };
    bundle.sign(&key);

    // A bundle, with a signature, is longer than one code: it reads back all the same.
    let multisig = Message::Multisig(Box::new(bundle));

    assert!(multisig.to_text().starts_with("UR:CHAINV4-MULTISIG/"));
    assert_eq!(Message::from_text(&multisig.to_text()), Ok(multisig.clone()));
    assert_eq!(Message::from_ur(multisig.ur_type(), &multisig.bytes()), Ok(multisig.clone()));

    for (message, kind) in [
        (Message::Pair(pairing), "UR:CHAINV4-PAIRING/"),
        (Message::Sign(transfer(&key)), "UR:CHAINV4-SIGN-REQUEST/"),
        (Message::Sign(token_transfer(&key)), "UR:CHAINV4-SIGN-REQUEST/"),
        (Message::Signed(signed), "UR:CHAINV4-SIGNED/"),
    ] {
        let text = message.to_text();

        assert!(text.starts_with(kind), "{text}");
        assert_eq!(Message::from_text(&text), Ok(message.clone()));
        assert_eq!(Message::from_text(&format!("  {}\r\n", text.to_ascii_lowercase())), Ok(message.clone()), "pasted, in either case, with what pasting brings");
        assert!(text.bytes().all(|byte| b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ $%*+-./:".contains(&byte)), "all in a QR code's alphanumeric mode");
        assert_eq!(Message::from_ur(message.ur_type(), &message.bytes()), Ok(message.clone()));
        assert!(message.bytes().len() <= ONE_CODE, "{} fits one code: {} bytes", message.name(), message.bytes().len());
    }
}

#[test]
fn bytes_that_are_not_exactly_one_message_are_refused() {
    let message = Message::Sign(transfer(&key()));
    let mut longer = message.bytes();
    longer.push(0);

    assert!(Message::from_ur(SIGN_TYPE, &longer).unwrap_err().contains("more in it"));
    assert!(Message::from_ur(SIGN_TYPE, &message.bytes()[..10]).is_err(), "cut short");
    assert!(Message::from_ur(PAIR_TYPE, &message.bytes()).is_err(), "a request read as a pairing");
    assert!(Message::from_ur("chainv4-send", &message.bytes()).unwrap_err().contains("does not read"));

    let text = message.to_text();

    assert!(Message::from_text(&text[..text.len() - 4]).is_err(), "its checksum caught");
    assert!(Message::from_text("CV4:1:SIGN:ABC").unwrap_err().contains("start ur:"), "the codes from before");
    assert!(Message::from_text("https://example.com").is_err());

    let part = ur::Encoder::new(&long(&key()).bytes(), FRAGMENT, SIGN_TYPE).unwrap().next_part().unwrap();

    assert!(Message::from_text(&part).unwrap_err().contains("one part"));
}

#[test]
fn codes_read_off_photos_and_files_alike() {
    let message = Message::Sign(token_transfer(&key()));
    let text = message.to_text();

    // A screenshot, a PNG; a photo as a phone's camera takes it, a big JPEG with the code in the
    // middle; and the text itself, in a file.
    let screenshot = encoded(&photo(&text, 6, 0), image::ImageFormat::Png);
    let camera = encoded(&photo(&text, 18, 3_000), image::ImageFormat::Jpeg);

    assert_eq!(read(&screenshot), Ok(message.clone()));
    assert_eq!(read(&camera), Ok(message.clone()));
    assert_eq!(read(text.as_bytes()), Ok(message));

    let other = encoded(&photo("https://example.com", 6, 0), image::ImageFormat::Png);
    let blank = encoded(&image::GrayImage::from_pixel(400, 300, image::Luma([200])), image::ImageFormat::Png);

    assert!(read(&other).unwrap_err().contains("not one of this wallet's"));
    assert!(read(&blank).unwrap_err().contains("no QR code found"));
    assert!(read(b"\x00\x01 not an image").unwrap_err().contains("not an image"));
}

#[test]
fn the_signer_signs_only_with_the_key_asked_for_and_only_what_the_chain_takes() {
    let key = key();
    let request = transfer(&key);
    let signed = sign(&request, &key, None).unwrap();

    assert_eq!(signed, SignedTx::sign(request.tx.clone(), 3, request.fee, &key, &chain()));
    assert!(sign(&request, &Keypair::from_seed([8; 32]), None).unwrap_err().contains("not the one"));

    // A transfer from someone else's address, signed with this key, is one the chain refuses.
    let mut theirs = transfer(&key);
    theirs.tx = Tx::Transfer {
        from: someone(1),
        to: someone(2),
        amount: Amount::U128(1),
    };

    assert!(sign(&theirs, &key, None).unwrap_err().contains("chain would refuse"));
}

#[test]
fn the_sender_takes_back_only_the_transaction_it_asked_for() {
    let key = key();
    let request = transfer(&key);
    let signed = sign(&request, &key, None).unwrap();

    assert_eq!(check(&request, &signed), Ok(()));

    let mut other_nonce = request.clone();
    other_nonce.nonce = 4;
    let mut forged = signed.clone();
    forged.signature = Signature::Ed25519([9; 64]);
    let other_signer = sign(&Request { signer: Keypair::from_seed([8; 32]).public(), ..request.clone() }, &Keypair::from_seed([8; 32]), None);

    assert!(check(&other_nonce, &signed).unwrap_err().contains("not the transaction"));
    assert!(check(&request, &forged).unwrap_err().contains("signature"));
    assert!(other_signer.is_err(), "its transfer is from the first key's address");
    assert!(check(&token_transfer(&key), &signed).is_err());
}

#[test]
fn a_probe_has_a_signature_of_the_signers_scheme_and_signs_nothing() {
    for scheme in KeyType::ALL {
        let key = Keypair::from_seed([*scheme as u8 + 3; 32]);
        let probe = probe(transfer(&key).tx, 5, key.public());
        let real = SignedTx::sign(probe.tx.clone(), 5, FeeOffer::NONE, &key, &chain());

        assert_eq!(wincode::serialized_size(&probe).unwrap(), wincode::serialized_size(&real).unwrap(), "{scheme:?}: priced as the real one");
        assert!(probe.verify_on(&chain()).is_err(), "and never valid");
    }
}

#[test]
fn a_fee_past_what_fits_shows_as_the_most_there_is_and_not_as_what_it_wraps_to() {
    let key = key();
    let fee = FeeOffer { gas_limit: u64::MAX, max_price: u256::MAX, tip: u256::ZERO };
    let shown = describe(&Request { fee, ..transfer(&key) });

    assert!(shown.fee.contains("115,792,089,237,316,195,423,570,985,008,687,907,853,269,984,665,640,564,039,457"), "{}", shown.fee);
}

#[test]
fn a_token_transfers_request_naming_its_chain_reads_off_one_code_in_a_photo() {
    let message = Message::Sign(token_transfer(&key()));

    assert!(message.bytes().len() > FRAGMENT, "denser than a part");
    assert!(message.bytes().len() <= ONE_CODE);
    assert_eq!(read(&encoded(&photo(&message.to_text(), 3, 1_400), image::ImageFormat::Png)), Ok(message));
}

#[test]
fn a_request_carries_its_chain_and_what_is_signed_holds_there_alone() {
    let key = key();
    let request = transfer(&key);
    let bytes = Message::Sign(request.clone()).bytes();
    let Message::Sign(read) = Message::from_ur(SIGN_TYPE, &bytes).unwrap() else { unreachable!() };

    assert_eq!(read.genesis, chain(), "the chain comes through the code");
    assert_eq!(describe(&read).chain, chainv4::crypto::ethereum::chain_id(&chain()), "and shows by the id Settings takes");

    let signed = sign(&read, &key, None).unwrap();

    assert_eq!(signed.verify_on(&chain()), Ok(()));
    assert_eq!(signed.verify_on(&another_chain()), Err(chainv4::crypto::verifiable::VerifyError::InvalidSignature));

    // What a sender asked for on one chain is not what it takes back from a signer told another.
    let elsewhere = Request { genesis: another_chain(), ..request.clone() };

    assert!(check(&elsewhere, &signed).unwrap_err().contains("signature"));
    assert_eq!(check(&request, &signed), Ok(()));
}

#[test]
fn a_signer_told_its_chain_refuses_a_request_for_another_and_a_signer_told_none_signs_what_it_shows() {
    let key = key();
    let request = transfer(&key);
    let ours = chainv4::crypto::ethereum::chain_id(&chain());
    let theirs = chainv4::crypto::ethereum::chain_id(&another_chain());

    assert!(sign(&request, &key, Some(ours)).is_ok());
    assert!(sign(&request, &key, None).is_ok(), "told none, it has nothing to hold it against");

    let refused = sign(&request, &key, Some(theirs)).unwrap_err();

    assert!(refused.contains(&ours.to_string()) && refused.contains(&theirs.to_string()), "{refused}");
    assert_eq!(wrong_chain(&chain(), Some(ours)), None);
    assert_eq!(wrong_chain(&chain(), None), None);
    assert!(wrong_chain(&chain(), Some(theirs)).is_some());
}

/// A request as it was before it named its chain, as a signer of that age reads and writes one.
#[derive(Clone, Debug, PartialEq, SchemaWrite, SchemaRead)]
struct OlderRequest {
    tx: Tx,
    nonce: u64,
    fee: FeeOffer,
    signer: PublicKey,
    coin: Shown,
    token: Option<Shown>,
}

#[test]
fn a_request_from_before_it_named_its_chain_is_refused_and_so_is_a_new_one_on_an_older_signer() {
    let key = key();
    let request = transfer(&key);
    let older = OlderRequest { tx: request.tx.clone(), nonce: request.nonce, fee: request.fee, signer: request.signer, coin: request.coin.clone(), token: None };
    let bytes = wincode::serialize(&older).unwrap();

    assert_eq!(Message::from_ur(SIGN_TYPE, &bytes).unwrap_err(), "a code that does not decode", "this signer, on an older sender's code");

    // An older signer reads every field it knew, and finds the chain left over.
    assert_eq!(whole::<OlderRequest>(&Message::Sign(request).bytes()).unwrap_err(), "a code with more in it than it should hold");
}

#[test]
fn hostile_bytes_in_a_request_never_panic_and_what_does_read_can_be_shown() {
    let key = key();
    let good = Message::Sign(transfer(&key)).bytes();
    let read = |bytes: &[u8]| {
        if let Ok(Message::Sign(request)) = Message::from_ur(SIGN_TYPE, bytes) {
            let _ = describe(&request);
            let _ = wrong_chain(&request.genesis, Some(1));
            let _ = sign(&request, &key, None);
        }
    };

    // Cut short anywhere, and each byte in turn set to what might mislead a length or a tag.
    for length in 0..good.len() {
        read(&good[..length]);
    }

    for at in 0..good.len() {
        for byte in [0, 1, 0x7f, 0x80, 0xff] {
            let mut changed = good.clone();
            changed[at] = byte;
            read(&changed);
        }
    }

    // Noise, the same every run.
    let mut state = 0x9e37_79b9_7f4a_7c15u64;

    for size in 0..400 {
        let noise: Vec<u8> = (0..size)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;

                state as u8
            })
            .collect();

        read(&noise);
    }
}

#[test]
fn the_signer_lays_out_what_it_signs() {
    let key = key();
    let coin = describe(&transfer(&key));

    assert_eq!(coin.kind, "Send DEV");
    assert_eq!(coin.what, Some(("1.5 DEV".to_string(), "1500000000000000000".to_string())));
    assert_eq!(coin.to.as_deref(), Some(someone(0xab).to_string().as_str()));
    assert_eq!(coin.from, key.public().address().to_string());
    assert_eq!(coin.fee, "0.000042 DEV");
    assert_eq!(coin.nonce, 3);

    let token = describe(&token_transfer(&key));

    assert_eq!(token.kind, "Send GOLD");
    assert_eq!(token.what, Some(("2.5 GOLD".to_string(), "250".to_string())));
    assert_eq!(token.to.as_deref(), Some(someone(0xcd).to_string().as_str()));
    assert_eq!(token.token.as_deref(), Some(Address::Contract([0x11; 32]).to_string().as_str()));

    // The same call with no token named may as well send an NFT, whose id is no amount: it is
    // shown as the call it is, its contract known as neither.
    let unnamed_request = Request { token: None, ..token_transfer(&key) };
    let unnamed = describe(&unnamed_request);

    assert_eq!((unnamed.kind.as_str(), unnamed.what.is_none()), ("Call a contract", true));
    assert!(laid(&unnamed_request).steps[0].rows.iter().any(|row| row.said == crate::steps::Said::Text(crate::steps::UNKNOWN_CONTRACT.to_string())));

    let mut bond = transfer(&key);
    bond.tx = Tx::Bond {
        voter: key.public(),
        amount: Amount::U128(10),
    };
    let described = describe(&bond);

    assert_eq!((described.kind.as_str(), described.what.is_none()), ("Bond transaction", true));
    assert!(described.everything.contains("Bond"), "everything it holds, for kinds it lays out no better");
}

/// A request too long for one code: a call with a kilobyte and a half of bytes.
fn long(key: &Keypair) -> Message {
    Message::Sign(Request {
        tx: Tx::ContractCall {
            to: Address::Contract([0x22; 32]),
            func: Hasher::digest(HashType::Xxh3_64, b"store"),
            amount: Amount::U128(0),
            data: vec![TxCallArgumentType::Bytes((0..1_500u32).map(|byte| byte as u8).collect())],
        },
        ..transfer(key)
    })
}

#[test]
fn a_long_message_reads_off_a_camera_from_any_parts_enough() {
    let message = long(&key());
    let bytes = message.bytes();
    let mut encoder = ur::Encoder::new(&bytes, FRAGMENT, message.ur_type()).unwrap();
    let parts: Vec<String> = (0..20).map(|_| encoder.next_part().unwrap().to_ascii_uppercase()).collect();
    let mut assembly = crate::hardware::Assembly::default();
    let mut read = None;

    assert!(encoder.fragment_count() > 2, "long enough to animate");
    assert!(parts.iter().all(|part| part.len() < 700), "each about as long as a code a camera reads well");

    // As a camera sees a screen going through them: frames of 1280 by 720, the code in the middle
    // at about the size a phone held up to it shows; starting late, and missing every third.
    for (index, text) in parts.iter().enumerate().skip(1) {
        if index % 3 == 0 {
            continue;
        }

        let frame = photo(text, 4, 720);
        let mut wide = image::GrayImage::from_pixel(1_280, 720, image::Luma([150]));
        image::imageops::overlay(&mut wide, &frame, 280, 0);
        let found = code_in_frame(1_280, 720, wide.as_raw()).expect("a code in every frame");

        assert_eq!(&found, text);

        if let Some((kind, bytes)) = assembly.add(&found).unwrap() {
            read = Some(Message::from_ur(&kind, &bytes).unwrap());

            break;
        }
    }

    assert_eq!(read, Some(message), "any enough parts, in the order they came");

    // A frame with no code in it, or a part cut off, reads as nothing.
    assert_eq!(code_in_frame(640, 480, &vec![150; 640 * 480]), None);
    assert_eq!(code_in_frame(640, 480, &[0; 10]), None, "a frame shorter than it says");
}

#[test]
fn a_request_names_its_collection_after_its_encoding_and_only_when_it_has_one() {
    let key = Keypair::from_seed([5; 32]);
    let plain = transfer(&key);
    let nft = Request {
        tx: Tx::ContractCall {
            to: Address::Contract([0x22; 32]),
            func: Hasher::digest(HashType::Xxh3_64, b"safe_transfer"),
            amount: Amount::U128(0),
            data: vec![TxCallArgumentType::Address(someone(0xcd)), TxCallArgumentType::U256(u256::new(7))],
        },
        collection: Some(NamedCollection { name: "Test Pixels".to_string(), symbol: "PIX".to_string(), multi: false }),
        ..transfer(&key)
    };

    // Without one, the very bytes a request always had: a signer from before reads it.
    assert_eq!(Message::Sign(plain.clone()).bytes(), wincode::serialize(&plain).unwrap());
    assert_eq!(Message::from_ur(SIGN_TYPE, &Message::Sign(plain.clone()).bytes()), Ok(Message::Sign(plain)));

    // With one, it comes after, and reads back whole; a signer from before refuses it.
    let bytes = Message::Sign(nft.clone()).bytes();
    assert_eq!(Message::from_ur(SIGN_TYPE, &bytes), Ok(Message::Sign(nft.clone())));
    assert!(whole::<Request>(&bytes).is_err(), "as a signer from before reads it: bytes left over");
    assert!(Message::from_ur(SIGN_TYPE, &[bytes.as_slice(), &[0]].concat()).is_err(), "and nothing past the collection");

    // The signer says what it sends: the collection's name, the symbol and the id.
    let said = describe(&nft);
    assert_eq!(said.kind, "Send an NFT from Test Pixels");
    assert_eq!(said.what, Some(("PIX #7".to_string(), "1".to_string())));
    assert_eq!(said.to, Some(someone(0xcd).to_string()));

    let items = Request {
        tx: Tx::ContractCall {
            to: Address::Contract([0x23; 32]),
            func: Hasher::digest(HashType::Xxh3_64, b"transfer"),
            amount: Amount::U128(0),
            data: vec![TxCallArgumentType::Address(someone(0xcd)), TxCallArgumentType::U256(u256::new(3)), TxCallArgumentType::U256(u256::new(12))],
        },
        collection: Some(NamedCollection { name: "Test Items".to_string(), symbol: "ITEM".to_string(), multi: true }),
        ..transfer(&key)
    };
    assert_eq!(describe(&items).what, Some(("12 of ITEM #3".to_string(), "12".to_string())));
    assert_eq!(describe(&items).kind, "Send items from Test Items");
}

#[test]
fn the_signer_names_the_evm_collection_a_transfer_is_from_and_the_items_it_sends() {
    use chainv4::abi::{AbiType, AbiValue, encode_values};

    enum CollectionKind {
        Nft,
        Multi,
    }
    let call = |signature: &str, types: &[AbiType], values: &[AbiValue]| [chainv4::crypto::secp256k1::keccak(signature.as_bytes())[..4].to_vec(), encode_values(types, values).unwrap()].concat();
    let evm = |address: &Address| AbiValue::Address(chainv4::crypto::secp256k1::keccak(&address.to_string().into_bytes())[12..].try_into().unwrap());
    // As the NFTs page writes them (views::nfts::evm).
    let transfer_input = |kind: CollectionKind, from: &Address, to: &Address, id: u256, amount: u256| match kind {
        CollectionKind::Nft => call("safeTransferFrom(address,address,uint256)", &[AbiType::Address, AbiType::Address, AbiType::Uint(256)], &[evm(from), evm(to), AbiValue::Uint(id)]),
        CollectionKind::Multi => call(
            "safeTransferFrom(address,address,uint256,uint256,bytes)",
            &[AbiType::Address, AbiType::Address, AbiType::Uint(256), AbiType::Uint(256), AbiType::Bytes],
            &[evm(from), evm(to), AbiValue::Uint(id), AbiValue::Uint(amount), AbiValue::Bytes(Vec::new())],
        ),
    };
    let batch_input = |from: &Address, to: &Address, items: &[(u256, u256)]| {
        let list = |values: Vec<u256>| AbiValue::Array(values.into_iter().map(AbiValue::Uint).collect());
        let uints = || AbiType::Array(Box::new(AbiType::Uint(256)));

        call(
            "safeBatchTransferFrom(address,address,uint256[],uint256[],bytes)",
            &[AbiType::Address, AbiType::Address, uints(), uints(), AbiType::Bytes],
            &[evm(from), evm(to), list(items.iter().map(|(id, _)| *id).collect()), list(items.iter().map(|(_, amount)| *amount).collect()), AbiValue::Bytes(Vec::new())],
        )
    };

    let key = key();
    let me = key.public().address();
    let collection = Address::Evm([0x31; 20]);
    let request = |input: Vec<u8>, multi: bool| Request {
        tx: Tx::EvmCall { to: collection, value: Amount::U128(0), input },
        collection: Some(NamedCollection { name: "Evm Pixels".to_string(), symbol: "EPX".to_string(), multi }),
        ..transfer(&key)
    };

    let nft = describe(&request(transfer_input(CollectionKind::Nft, &me, &someone(0xcd), u256::new(7), u256::ONE), false));
    assert_eq!((nft.kind.as_str(), nft.what), ("Send an NFT from Evm Pixels", Some(("EPX #7".to_string(), "1".to_string()))));
    assert_eq!(nft.to.as_deref().map(|to| to.starts_with("evm:0x")), Some(true));
    assert_eq!(nft.token, Some(collection.to_string()));

    let items = describe(&request(transfer_input(CollectionKind::Multi, &me, &someone(0xcd), u256::new(3), u256::new(12)), true));
    assert_eq!((items.kind.as_str(), items.what), ("Send items from Evm Pixels", Some(("12 of EPX #3".to_string(), "12".to_string()))));

    let batch = describe(&request(batch_input(&me, &someone(0xcd), &[(u256::new(1), u256::new(2)), (u256::new(5), u256::new(4))]), true));
    assert_eq!(batch.what, Some(("EPX: 2 of #1, 4 of #5".to_string(), "6".to_string())));

    // The calldata of the other standard, or of anything else, is not named as a transfer from it.
    assert!(describe(&request(transfer_input(CollectionKind::Multi, &me, &someone(0xcd), u256::new(3), u256::new(12)), false)).kind != "Send an NFT from Evm Pixels");
    assert_eq!(describe(&request(vec![1, 2, 3, 4], false)).what, None);
}

#[test]
fn an_authorization_goes_to_the_signer_and_comes_back_signed_in_codes_of_their_own() {
    use chainv4::registry::tokens::authorization::{SignedAuthorization, TokenAuthorization};

    let key = key();
    let terms = TokenAuthorization::Permit { token: Address::Contract([0x44; 32]), owner: key.public().address(), spender: someone(0xcd), amount: u256::new(300), nonce: u256::ONE, deadline: 1_900_000_000_000 };
    let asked = Message::Authorize(Box::new(AuthorizeRequest { authorization: terms.clone(), chain_id: 77, token: Some(Shown { symbol: "TOK".to_string(), decimals: 2 }) }));

    assert_eq!(asked.ur_type(), AUTHORIZE_TYPE);
    assert_eq!(Message::from_text(&asked.to_text()), Ok(asked.clone()));

    let signed = SignedAuthorization::sign(terms, &key, 77).unwrap();
    let answer = Message::Authorized(Box::new(signed.clone()));

    assert_eq!(Message::from_text(&answer.to_text()), Ok(answer));
    assert!(Message::from_ur(AUTHORIZE_TYPE, b"{\"kind\":\"chainv4 token permit\"}").is_err());
    assert!(Message::from_ur(AUTHORIZED_TYPE, &[0xff]).is_err());

    // Without the token named, it reads back as the terms alone.
    let bare = AuthorizeRequest { authorization: signed.authorization, chain_id: 77, token: None };
    assert_eq!(Message::from_ur(AUTHORIZE_TYPE, &Message::Authorize(Box::new(bare.clone())).bytes()), Ok(Message::Authorize(Box::new(bare))));
}
