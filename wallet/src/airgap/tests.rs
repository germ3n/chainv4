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
fn base45_reads_and_writes_as_rfc_9285_does() {
    for (bytes, text) in [(&b"AB"[..], "BB8"), (b"Hello!!", "%69 VD92EX0"), (b"base-45", "UJCLQE7W581"), (b"ietf!", "QED8WEX0"), (b"", "")] {
        assert_eq!(base45::encode(bytes), text);
        assert_eq!(base45::decode(text).as_deref(), Some(bytes));
    }

    let every: Vec<u8> = (0..=255).chain((0..=255).rev()).collect();

    assert_eq!(base45::decode(&base45::encode(&every)), Some(every));
    assert_eq!(base45::decode("GGW"), None, "65535 is the most three characters may hold");
    assert_eq!(base45::decode("ab"), None, "lower case is not in the alphabet");
    assert_eq!(base45::decode("A"), None, "one character holds nothing");
}

#[test]
fn every_message_reads_back_from_its_text_and_fits_a_dense_code() {
    let key = key();
    let pairing = Pairing {
        name: "Main".to_string(),
        keys: KeyType::ALL.iter().map(|scheme| Keypair::from_seed([*scheme as u8 + 1; 32]).public()).collect(),
    };
    let signed = sign(&transfer(&key), &key).unwrap();

    for message in [Message::Pair(pairing), Message::Sign(transfer(&key)), Message::Sign(token_transfer(&key)), Message::Signed(signed)] {
        let text = message.to_text();

        assert_eq!(Message::from_text(&text), Ok(message.clone()));
        assert_eq!(Message::from_text(&format!("  {text}\r\n")), Ok(message.clone()), "pasted, with what pasting brings");
        assert!(!text.ends_with(' '), "base45 never ends in its space");
        assert!(text.bytes().all(|byte| b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ $%*+-./:".contains(&byte)), "all in a QR code's alphanumeric mode");
        assert!(text.len() < 700, "{} is {} characters", message.name(), text.len());
    }
}

#[test]
fn text_that_is_not_exactly_one_message_is_refused() {
    let text = Message::Sign(transfer(&key())).to_text();
    // The encoding comes after the third colon; base45 has colons of its own.
    let encoded = text.splitn(4, ':').nth(3).unwrap();
    let mut longer = base45::decode(encoded).unwrap();
    longer.push(0);

    assert!(Message::from_text(&format!("CV4:1:SIGN:{}", base45::encode(&longer))).unwrap_err().contains("more in it"));
    assert!(Message::from_text(&format!("CV4:1:SIGN:{}", &encoded[..encoded.len() - 3])).is_err(), "cut short");
    assert!(Message::from_text(&text.replace("CV4:1:", "CV4:2:")).unwrap_err().contains("not a chainv4"), "another version");
    assert!(Message::from_text(&text.replace(":SIGN:", ":SEND:")).unwrap_err().contains("does not know"));
    assert!(Message::from_text("https://example.com").is_err());
}

#[test]
fn codes_read_off_photos_and_files_alike() {
    let message = Message::Sign(token_transfer(&key()));
    let text = message.to_text();

    // A screenshot, a PNG; a photo as a phone's camera takes it, a big JPEG with the code in the
    // middle; and the text itself, in a file.
    let screenshot = encoded(&photo(&text, 6, 0), image::ImageFormat::Png);
    let camera = encoded(&photo(&text, 18, 3_000), image::ImageFormat::Jpeg);

    assert_eq!(read(&screenshot), Ok(Code::Whole(Box::new(message.clone()))));
    assert_eq!(read(&camera), Ok(Code::Whole(Box::new(message.clone()))));
    assert_eq!(read(text.as_bytes()), Ok(Code::Whole(Box::new(message))));

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
fn a_message_too_long_for_one_code_goes_as_parts_read_in_any_order() {
    let message = long(&key());
    let text = message.to_text();
    assert_eq!(codes(&Message::Sign(transfer(&key())).to_text()).len(), 1, "a transfer fits one code");

    let codes = codes(&text);
    assert_eq!(codes.len(), text.len().div_ceil(PART_SIZE));
    assert!(codes.len() > 2);

    for code in &codes {
        assert!(code.len() <= PART_SIZE + 32, "each about as long as the rest: {}", code.len());
        assert!(code.bytes().all(|byte| b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ $%*+-./:".contains(&byte)), "and as dense: {code}");
    }

    // Backwards, the first one read twice: only the last one in gives the message.
    let mut assembly = Assembly::default();
    let order = [codes.len() - 1].into_iter().chain((0..codes.len()).rev());
    let mut seen = std::collections::BTreeSet::new();

    for index in order {
        let Ok(Code::Part(part)) = Code::from_text(&codes[index]) else {
            panic!("not a part: {}", codes[index]);
        };
        seen.insert(index);
        let got = assembly.add(part).unwrap();

        match seen.len() == codes.len() {
            true => assert_eq!(got, Some(message.clone()), "the last one in"),
            false => {
                assert_eq!(got, None);
                assert_eq!(assembly.missing(), (codes.len(), (1..=codes.len()).filter(|part| !seen.contains(&(part - 1))).collect()));
            }
        }
    }

    assert_eq!(assembly.missing(), (0, Vec::new()), "and it starts afresh after");
}

#[test]
fn parts_of_another_message_start_over_and_a_forged_part_is_caught() {
    let key = key();
    let first = codes(&long(&key).to_text());
    let mut other = long(&key);

    if let Message::Sign(request) = &mut other {
        request.nonce = 99;
    }

    let second = codes(&other.to_text());
    let part = |code: &String| match Code::from_text(code) {
        Ok(Code::Part(part)) => part,
        other => panic!("not a part: {other:?}"),
    };
    let mut assembly = Assembly::default();

    assembly.add(part(&first[0])).unwrap();
    assembly.add(part(&second[1])).unwrap();

    assert_eq!(assembly.missing(), (second.len(), (1..=second.len()).filter(|index| *index != 2).collect()), "the other's, from scratch");

    // A part whose text was changed, but which still names the whole by the same hash.
    let mut assembly = Assembly::default();
    let mut forged = part(&first[1]);
    forged.text = forged.text.chars().rev().collect();

    for (index, code) in first.iter().enumerate() {
        let read = if index == 1 { forged.clone() } else { part(code) };
        let got = assembly.add(read);

        if index + 1 == first.len() {
            assert!(got.unwrap_err().contains("do not make the message"));
        }
    }

    for bad in ["CV4:1:PART:0/3:ABCDEF01:XYZ", "CV4:1:PART:4/3:ABCDEF01:XYZ", "CV4:1:PART:1/1:ABCDEF01:XYZ", "CV4:1:PART:1/3:ABC:XYZ", "CV4:1:PART:1/3:ABCDEF01:", "CV4:1:PART:x/3:ABCDEF01:XYZ"] {
        assert!(Code::from_text(bad).is_err(), "{bad}");
    }
}

#[test]
fn a_part_reads_off_a_photo_like_a_whole_code() {
    let codes = codes(&long(&key()).to_text());
    let photo = encoded(&photo(&codes[1], 14, 2_400), image::ImageFormat::Jpeg);

    assert!(matches!(read(&photo), Ok(Code::Part(Part { index: 2, .. }))));
}
