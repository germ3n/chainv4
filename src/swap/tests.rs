use {super::*, crate::crypto::secp256k1};

const NOW: u64 = 1_793_000_000_000;

fn bitcoin_key(byte: u8) -> [u8; 33] {
    secp256k1::public_key(&[byte; 32]).unwrap()
}

fn alice() -> Parties {
    Parties { chain: Some(Address::Ed25519([0xa; 32])), bitcoin: Some(bitcoin_key(1)), evm: Some([0xa1; 20]) }
}

fn bob() -> Parties {
    Parties { chain: Some(Address::Ed25519([0xb; 32])), bitcoin: Some(bitcoin_key(2)), evm: Some([0xb2; 20]) }
}

fn chain_leg() -> Leg {
    Leg::Chain(ChainLeg { contract: Address::Contract([0x5a; 32]), token: None, amount: u256::new(5_000), payer: None, receiver: None })
}

fn bitcoin_leg() -> Leg {
    Leg::Bitcoin(BitcoinLeg { testnet: true, sats: 100_000, payer: None, receiver: None })
}

fn evm_leg(chain_id: u64) -> Leg {
    Leg::Evm(EvmLeg { chain_id, contract: evm::deployed_address(), token: None, amount: u256::new(10u128.pow(16)), payer: None, receiver: None })
}

/// Alice's offer of bitcoin for chainv4's coin, and Bob's answer.
fn offer() -> Terms {
    Terms::offer(sha256(&[0x5e; 32]), bitcoin_leg(), chain_leg(), NOW + FIRST_DEADLINE_MS, NOW + SECOND_DEADLINE_MS, &alice()).unwrap()
}

fn answered() -> Terms {
    offer().answer(&bob()).unwrap()
}

#[test]
fn terms_travel_as_a_code_and_read_back_whole() {
    let ether_for_bitcoin = Terms::offer(offer().hash, evm_leg(11155111), bitcoin_leg(), offer().first_deadline, offer().second_deadline, &alice()).unwrap().answer(&bob()).unwrap();

    for terms in [offer(), answered(), ether_for_bitcoin] {
        let text = terms.to_text();

        assert!(text.starts_with("UR:CHAINV4-SWAP/"), "{text}");
        assert_eq!(Terms::from_text(&text).unwrap(), terms);
        assert_eq!(Terms::from_text(&text.to_ascii_lowercase()).unwrap(), terms, "either case");
    }

    let bytes = answered().to_bytes();
    assert_eq!(bytes[0], 2, "legs");
    assert!(Terms::from_bytes(&[bytes.clone(), vec![0]].concat()).is_err(), "bytes past them");
    assert!(Terms::from_bytes(&[&[9][..], &bytes[1..]].concat()).unwrap_err().contains("does not read"));
    assert!(Terms::from_text("ur:chainv4-pairing/aeae").unwrap_err().contains("not a swap"));
}

#[test]
fn terms_written_before_legs_read_as_the_same_swap() {
    for terms in [offer(), answered(), Terms { first: chain_leg(), second: bitcoin_leg(), ..offer() }] {
        let old = legacy::write(&terms);

        assert!(old[0] <= 1, "the chain's tag came first");
        assert_eq!(Terms::from_bytes(&old).unwrap(), terms);
        assert!(Terms::from_bytes(&[old, vec![0]].concat()).is_err(), "bytes past them");
    }
}

#[test]
fn whoever_starts_pays_the_first_leg_and_the_answer_fills_the_rest() {
    let (offer, answered) = (offer(), answered());

    assert!(!offer.agreed() && answered.agreed());
    assert_eq!(offer.htlc(), None, "no HTLC before both keys are in");
    assert_eq!(offer.mine(&alice()), (Some(Side::First), Some(Side::Second)));
    assert_eq!(answered.mine(&bob()), (Some(Side::Second), Some(Side::First)));
    assert_eq!(answered.mine(&Parties::default()), (None, None));
    assert!(answered.answers(&offer));
    assert!(answered.answer(&bob()).unwrap_err().contains("answered already"));

    let htlc = answered.htlc().unwrap();
    assert_eq!((htlc.claim, htlc.refund, htlc.hash), (bitcoin_key(2), bitcoin_key(1), offer.hash));
    assert_eq!(u64::from(htlc.locktime), (NOW + FIRST_DEADLINE_MS) / 1_000, "Alice's lock, the first, is Bitcoin's");

    // An answer that changes anything but Bob's parts is not one.
    let mut changed = answered.clone();
    changed.second_deadline -= 1;
    assert!(!changed.answers(&offer));
    let mut changed = answered.clone();
    if let Leg::Bitcoin(leg) = &mut changed.first {
        leg.payer = Some(bitcoin_key(3));
    }
    assert!(!changed.answers(&offer), "Alice's key swapped out");
    assert!(!offer.answers(&offer), "not agreed");

    // An answer needs who they are on each leg's chain.
    let no_bitcoin = Parties { bitcoin: None, ..bob() };
    assert!(offer.answer(&no_bitcoin).unwrap_err().contains("Bitcoin key"));
}

#[test]
fn any_two_chains_swap_but_not_one_chain_with_itself() {
    let ethereum_for_base = Terms::offer(offer().hash, evm_leg(1), evm_leg(8453), offer().first_deadline, offer().second_deadline, &alice()).unwrap().answer(&bob()).unwrap();
    assert_eq!(ethereum_for_base.check(NOW), Ok(()));
    assert_eq!(ethereum_for_base.htlc(), None, "no Bitcoin leg");
    assert_eq!((ethereum_for_base.first.chain_name(), ethereum_for_base.second.chain_name()), ("Ethereum".to_string(), "Base".to_string()));

    for (one, two) in [(evm_leg(1), evm_leg(1)), (chain_leg(), chain_leg()), (bitcoin_leg(), Leg::Bitcoin(BitcoinLeg { testnet: false, sats: 1_000, payer: None, receiver: None }))] {
        let terms = Terms { first: one, second: two, ..offer() };
        assert!(terms.check(NOW).unwrap_err().contains("two chains"));
    }
}

#[test]
fn terms_that_do_not_hold_together_are_refused() {
    assert_eq!(answered().check(NOW), Ok(()));
    assert_eq!(offer().check(NOW), Ok(()), "an offer, without the other's parts");

    let refused = |terms: Terms, why: &str| {
        let error = terms.check(NOW).unwrap_err();
        assert!(error.contains(why), "{error}");
    };
    let with_first = |change: &dyn Fn(&mut BitcoinLeg)| {
        let mut terms = answered();
        if let Leg::Bitcoin(leg) = &mut terms.first {
            change(leg);
        }
        terms
    };

    refused(with_first(&|leg| leg.sats = 100), "at least");
    refused(Terms { second: Leg::Chain(ChainLeg { amount: u256::ZERO, ..chain(&answered().second) }), ..answered() }, "more than nothing");
    refused(Terms { second_deadline: NOW - 1, ..answered() }, "passed");
    refused(Terms { second_deadline: NOW + FIRST_DEADLINE_MS - 3_600_000, ..answered() }, "at least 4 hours");
    refused(with_first(&|leg| leg.receiver = Some([0; 33])), "not a key");
    refused(with_first(&|leg| leg.receiver = Some(bitcoin_key(1))), "same party");
    refused(Terms { second: Leg::Chain(ChainLeg { payer: Some(Address::Ed25519([0xa; 32])), ..chain(&answered().second) }), ..answered() }, "same party");

    let mut zero = Terms::offer(offer().hash, evm_leg(1), bitcoin_leg(), offer().first_deadline, offer().second_deadline, &alice()).unwrap();
    if let Leg::Evm(leg) = &mut zero.first {
        leg.token = Some([0; 20]);
    }
    refused(zero, "zero address");
}

fn chain(leg: &Leg) -> ChainLeg {
    match leg {
        Leg::Chain(leg) => leg.clone(),
        _ => panic!("a chainv4 leg"),
    }
}

#[test]
fn a_lock_stands_only_as_the_terms_say() {
    let terms = answered();
    let leg = &terms.second;
    let lock = Lock { state: LockState::Locked, recipient: Some(Party::Chain(Address::Ed25519([0xa; 32]))), token: None, amount: u256::new(5_000), deadline: terms.second_deadline, secret: None };

    assert_eq!(leg.stands(&lock, terms.second_deadline), Ok(()));
    assert_eq!(leg.stands(&Lock { state: LockState::Claimed, ..lock.clone() }, terms.second_deadline), Ok(()));
    assert!(leg.stands(&Lock { state: LockState::None, ..lock.clone() }, terms.second_deadline).unwrap_err().contains("nothing"));
    assert!(leg.stands(&Lock { recipient: Some(Party::Chain(Address::Ed25519([0xc; 32]))), ..lock.clone() }, terms.second_deadline).unwrap_err().contains("someone else"));
    assert!(leg.stands(&Lock { amount: u256::new(4_999), ..lock.clone() }, terms.second_deadline).unwrap_err().contains("not what"));
    assert!(leg.stands(&Lock { token: Some(Party::Chain(Address::Contract([1; 32]))), ..lock.clone() }, terms.second_deadline).unwrap_err().contains("not what"));
    assert!(leg.stands(&Lock { deadline: terms.first_deadline, ..lock.clone() }, terms.second_deadline).unwrap_err().contains("deadline"));
    assert!(terms.first.stands(&lock, terms.first_deadline).unwrap_err().contains("HTLC"));

    // chainv4's contract answers get as text.
    let text = |values: &[&str]| values.iter().map(|value| value.to_string()).collect::<Vec<_>>();
    let zero = Address::Ed25519([0; 32]).to_string();
    let a = Address::Ed25519([0xa; 32]).to_string();
    let deadline = terms.second_deadline.to_string();
    let read = Lock::from_chain(&text(&["locked", &a, &a, &zero, "5000", &deadline, ""])).unwrap();
    assert_eq!(read, Lock { recipient: Some(Party::Chain(Address::Ed25519([0xa; 32]))), ..lock.clone() });
    assert_eq!(Lock::from_chain(&text(&["none"])).unwrap().state, LockState::None);
    assert!(Lock::from_chain(&text(&["locked", &a])).is_err());
}

#[test]
fn a_secret_is_new_each_time_and_its_hash_is_sha256() {
    let (secret, hash) = secret().unwrap();

    assert_eq!(hash, sha256(&secret));
    assert_ne!(super::secret().unwrap().0, secret);
}
