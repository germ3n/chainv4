use {super::*, crate::crypto::secp256k1};

const NOW: u64 = 1_793_000_000_000;

fn bitcoin_key(byte: u8) -> [u8; 33] {
    secp256k1::public_key(&[byte; 32]).unwrap()
}

/// An offer by someone paying bitcoin, and its answer.
fn offer() -> Terms {
    Terms {
        chain: Counterparty::BitcoinTestnet,
        hash: sha256(&[0x5e; 32]),
        sats: 100_000,
        token: None,
        amount: u256::new(5_000),
        starter: Pays::Bitcoin,
        bitcoin_payer: Some(bitcoin_key(1)),
        bitcoin_receiver: None,
        chain_payer: None,
        chain_receiver: Some(Address::Ed25519([0xa; 32])),
        first_deadline: NOW + FIRST_DEADLINE_MS,
        second_deadline: NOW + SECOND_DEADLINE_MS,
    }
}

fn answered() -> Terms {
    Terms {
        bitcoin_receiver: Some(bitcoin_key(2)),
        chain_payer: Some(Address::Ed25519([0xb; 32])),
        ..offer()
    }
}

#[test]
fn terms_travel_as_a_code_and_read_back_whole() {
    for terms in [offer(), answered(), Terms { token: Some(Address::Contract([7; 32])), starter: Pays::Chain, ..answered() }] {
        let text = terms.to_text();

        assert!(text.starts_with("UR:CHAINV4-SWAP/"), "{text}");
        assert_eq!(Terms::from_text(&text).unwrap(), terms);
        assert_eq!(Terms::from_text(&text.to_ascii_lowercase()).unwrap(), terms, "either case");
    }

    let bytes = wincode::serialize(&answered()).unwrap();
    assert!(Terms::from_bytes(&[bytes.clone(), vec![0]].concat()).is_err(), "bytes past them");
    assert!(Terms::from_text("ur:chainv4-pairing/aeae").unwrap_err().contains("not a swap"));
}

#[test]
fn whoever_starts_locks_first_and_longer() {
    let paying_bitcoin = answered();
    let htlc = paying_bitcoin.htlc().unwrap();

    assert!(!offer().agreed() && paying_bitcoin.agreed());
    assert_eq!(offer().htlc(), None, "no HTLC before both keys are in");
    assert_eq!((htlc.claim, htlc.refund, htlc.hash), (bitcoin_key(2), bitcoin_key(1), paying_bitcoin.hash));
    assert_eq!(u64::from(htlc.locktime), (NOW + FIRST_DEADLINE_MS) / 1_000, "the starter's lock is Bitcoin's, the first");
    assert_eq!(paying_bitcoin.chain_deadline(), NOW + SECOND_DEADLINE_MS);

    let paying_chain = Terms { starter: Pays::Chain, ..answered() };
    assert_eq!(paying_chain.chain_deadline(), NOW + FIRST_DEADLINE_MS);
    assert_eq!(u64::from(paying_chain.htlc().unwrap().locktime), (NOW + SECOND_DEADLINE_MS) / 1_000);
}

#[test]
fn terms_that_do_not_hold_together_are_refused() {
    assert_eq!(answered().check(NOW), Ok(()));
    assert_eq!(offer().check(NOW), Ok(()), "an offer, without the other's keys");

    let refused = |terms: Terms, why: &str| {
        let error = terms.check(NOW).unwrap_err();
        assert!(error.contains(why), "{error}");
    };

    refused(Terms { sats: 100, ..answered() }, "at least");
    refused(Terms { amount: u256::ZERO, ..answered() }, "more than nothing");
    refused(Terms { second_deadline: NOW - 1, ..answered() }, "passed");
    refused(Terms { second_deadline: NOW + FIRST_DEADLINE_MS - 3_600_000, ..answered() }, "at least 4 hours");
    refused(Terms { bitcoin_receiver: Some([0; 33]), ..answered() }, "not a key");
    refused(Terms { bitcoin_receiver: Some(bitcoin_key(1)), ..answered() }, "same Bitcoin key");
    refused(Terms { chain_payer: Some(Address::Ed25519([0xa; 32])), ..answered() }, "same chainv4 address");
}

#[test]
fn a_secret_is_new_each_time_and_its_hash_is_sha256() {
    let (secret, hash) = secret().unwrap();

    assert_eq!(hash, sha256(&secret));
    assert_ne!(super::secret().unwrap().0, secret);
}
