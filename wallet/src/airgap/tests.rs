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
    let signed = sign(&transfer(&key), &key).unwrap();
    let account = chainv4::structs::address::Address::Multisig([5; 32]);
    let mut bundle = chainv4::registry::multisig::MultisigBundle {
        genesis: chainv4::structs::hash::Hash::zero(chainv4::structs::hash::HashType::default_chain()),
        body: chainv4::registry::multisig::MultisigBody { account, tx: transfer(&key).tx, nonce: 3, fee: FeeOffer::NONE },
        signatures: Vec::new(),
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
        assert!(message.bytes().len() <= FRAGMENT, "{} fits one code: {} bytes", message.name(), message.bytes().len());
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
    let signed = sign(&request, &key).unwrap();

    assert_eq!(signed, SignedTx::sign(request.tx.clone(), 3, request.fee, &key));
    assert!(sign(&request, &Keypair::from_seed([8; 32])).unwrap_err().contains("not the one"));

    // A transfer from someone else's address, signed with this key, is one the chain refuses.
    let mut theirs = transfer(&key);
    theirs.tx = Tx::Transfer {
        from: someone(1),
        to: someone(2),
        amount: Amount::U128(1),
    };

    assert!(sign(&theirs, &key).unwrap_err().contains("chain would refuse"));
}

#[test]
fn the_sender_takes_back_only_the_transaction_it_asked_for() {
    let key = key();
    let request = transfer(&key);
    let signed = sign(&request, &key).unwrap();

    assert_eq!(check(&request, &signed), Ok(()));

    let mut other_nonce = request.clone();
    other_nonce.nonce = 4;
    let mut forged = signed.clone();
    forged.signature = Signature::Ed25519([9; 64]);
    let other_signer = sign(&Request { signer: Keypair::from_seed([8; 32]).public(), ..request.clone() }, &Keypair::from_seed([8; 32]));

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
        let real = SignedTx::sign(probe.tx.clone(), 5, FeeOffer::NONE, &key);

        assert_eq!(wincode::serialized_size(&probe).unwrap(), wincode::serialized_size(&real).unwrap(), "{scheme:?}: priced as the real one");
        assert!(probe.verify().is_err(), "and never valid");
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

    let unnamed = describe(&Request { token: None, ..token_transfer(&key) });

    assert_eq!(unnamed.what.as_ref().map(|what| what.1.as_str()), Some("250"), "the raw units, whatever it is called");

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
