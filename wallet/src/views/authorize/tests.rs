use {
    super::*,
    crate::views::drawn::{following_shown, two_phrases},
    chainv4::crypto::keypair::Keypair,
    chainv4::structs::hash::{Hash, HashType},
};

const TOKEN: Address = Address::Contract([7; 32]);

fn chain_id() -> u64 {
    chainv4::crypto::ethereum::chain_id(&Hash::zero(HashType::default_chain()))
}

fn shown(state: &mut AuthorizeState) -> Vec<String> {
    let mut book = two_phrases();
    let mut holders = crate::keys::open(&book).unwrap();
    let mut send = SendState::default();
    let (width, _, texts) = following_shown(&mut book, &mut holders, (360.0, 1400.0), |ui, ctx| show(ui, ctx, state, &mut send));

    assert!(width <= 360.0, "it fits a phone: {width}");

    texts.into_iter().map(|(text, _)| text).collect()
}

fn answer(function: &str, value: Value) -> (Ask, Result<Value, crate::node::Failure>) {
    let ask = Ask::Call(json!({ "contract": TOKEN.to_string(), "function": function, "args": [], "purpose": "authorize" }).to_string());

    (ask, Ok(json!({ "status": "returned", "values": [{ "type": "string", "value": value }] })))
}

#[test]
fn the_tokens_answers_are_taken_and_others_left_alone() {
    let mut state = AuthorizeState::default();
    let (ask, result) = answer("nonces", json!("3"));

    assert!(state.answered(&ask, &result));
    assert_eq!(state.nonce, Some(Ok(u256::new(3))));

    let (ask, result) = answer("permit_message", json!("chainv4 token permit"));

    assert!(state.answered(&ask, &result));
    assert_eq!(state.written, Some(Ok("chainv4 token permit".to_string())));

    let theirs = Ask::Call(json!({ "contract": "c", "function": "tokens_of", "purpose": "nfts" }).to_string());

    assert!(!state.answered(&theirs, &Ok(Value::Null)));
}

#[test]
fn an_allowance_signs_at_the_nonce_the_token_said_and_only_text_it_writes_alike() {
    let holders = crate::keys::open(&two_phrases()).unwrap();
    let owner = holders[0].addresses().into_iter().find(|address| holders[0].key(address).is_some()).unwrap();
    let prepared = TokenAuthorization::Permit { token: TOKEN, owner, spender: Address::Ed25519([0xb; 32]), amount: u256::new(300), nonce: u256::ZERO, deadline: 5_000 };
    let mut state = AuthorizeState { prepared: Some(prepared), nonce: Some(Ok(u256::new(2))), ..AuthorizeState::default() };
    let terms = state.terms().unwrap();

    assert!(matches!(terms, TokenAuthorization::Permit { nonce, .. } if nonce == u256::new(2)), "the token's nonce");

    // The token writes the same: shown, and Sign it offered.
    state.written = Some(Ok(terms.message(chain_id())));
    let texts = shown(&mut state);

    assert!(texts.iter().any(|text| text.contains("nonce: 2")), "{texts:?}");
    assert!(texts.iter().any(|text| text == "Sign it"));

    // Anything else is not signed.
    state.written = Some(Ok("chainv4 token permit\nsomething else".to_string()));
    let texts = shown(&mut state);

    assert!(texts.iter().any(|text| text.contains("other text")), "{texts:?}");
    assert!(!texts.iter().any(|text| text == "Sign it"));
}

#[test]
fn a_pasted_authorization_shows_its_terms_and_whether_it_holds_here() {
    let alice = Keypair::from_seed([0xa1; 32]);
    let terms = |deadline| TokenAuthorization::Permit { token: TOKEN, owner: alice.public().address(), spender: Address::Ed25519([0xb; 32]), amount: u256::new(300), nonce: u256::ZERO, deadline };
    let far = u64::MAX / 2;
    let here = SignedAuthorization::sign(terms(far), &alice, chain_id()).unwrap();
    let mut state = AuthorizeState { mode: Mode::Send, pasted: here.to_text(), ..AuthorizeState::default() };
    let texts = shown(&mut state);

    assert!(texts.iter().any(|text| text.contains("Signed by the owner's key")), "{texts:?}");
    assert!(texts.iter().any(|text| text == "Review and send"));

    // Signed for another chain, it does not hold here.
    state.pasted = SignedAuthorization::sign(terms(far), &alice, chain_id() + 1).unwrap().to_text();
    assert!(shown(&mut state).iter().any(|text| text.contains("It does not hold: the signature does not hold")));

    // Past its deadline, it has lapsed.
    state.pasted = SignedAuthorization::sign(terms(0), &alice, chain_id()).unwrap().to_text();
    assert!(shown(&mut state).iter().any(|text| text == "It has lapsed."));

    state.pasted = "not one".to_string();
    assert!(shown(&mut state).iter().any(|text| text.contains("not a signed authorization")));
}
