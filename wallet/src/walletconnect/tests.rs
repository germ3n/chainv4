use super::*;

fn bytes(text: &str) -> [u8; 32] {
    key32(text).unwrap()
}

#[test]
fn x25519_is_rfc_7748s_and_both_sides_derive_one_key_and_topic() {
    // RFC 7748, section 6.1.
    let alice = bytes("77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a");
    let bob = bytes("5dab087e624a8a4b79e17f8b83800ee66f3bb1292618b6fd1c2f8b27ff88e0eb");
    assert_eq!(hex(&x25519_public(&alice)), "8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a");
    assert_eq!(hex(&x25519_public(&bob)), "de9edb7d7b7dc1b4d35b61c2ece435373f8343c85b78674dadfc7e146f882b4f");
    assert_eq!(hex(&MontgomeryPoint(x25519_public(&bob)).mul_clamped(alice).to_bytes()), "4a5d9d5ba4ce2de1728e3bf480350f25e07e21c947d19e3376f09b3c1e161742");

    // The key is HKDF-SHA256 of that secret, no salt, no info; the topic its SHA-256.
    let key = sym_key(&alice, &x25519_public(&bob));
    assert_eq!(key, sym_key(&bob, &x25519_public(&alice)));
    let mut expected = [0; 32];
    Hkdf::<Sha256>::new(None, &bytes("4a5d9d5ba4ce2de1728e3bf480350f25e07e21c947d19e3376f09b3c1e161742")).expand(&[], &mut expected).unwrap();
    assert_eq!(key, expected);
    assert_eq!(topic_of(&key), hex(&Sha256::digest(key)));
    assert_eq!(topic_of(&key).len(), 64);
}

#[test]
fn a_type_0_envelope_is_a_zero_the_nonce_and_the_box_and_opens_with_its_key_alone() {
    let key = [7; 32];
    let iv = [9; 12];
    let sealed = seal(&key, r#"{"id":1}"#, iv);
    let raw = base64::engine::general_purpose::STANDARD.decode(&sealed).unwrap();

    assert_eq!(raw[0], 0);
    assert_eq!(raw[1..13], iv);
    assert_eq!(raw.len(), 1 + 12 + 8 + 16, "the message and the tag");
    assert_eq!(open(&key, &sealed).unwrap(), r#"{"id":1}"#);
    assert!(open(&[8; 32], &sealed).unwrap_err().contains("not sealed with the key"));

    // A type 1 envelope, and junk, are refused.
    let mut typed = raw.clone();
    typed[0] = 1;
    assert!(open(&key, &base64::engine::general_purpose::STANDARD.encode(typed)).unwrap_err().contains("type 1"));
    assert!(open(&key, "not base64!").is_err());
    assert!(open(&key, "AA==").is_err());
}

#[test]
fn the_relay_token_is_an_eddsa_jwt_naming_the_key_as_did_key() {
    use chainv4::crypto::keypair::{Keypair, PrivateKey, PublicKey, Signature};

    let seed = [3; 32];
    let token = relay_token(&seed, "abc", RELAY, 1_000);
    let parts: Vec<&str> = token.split('.').collect();
    assert_eq!(parts.len(), 3);
    let url = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let header: Value = serde_json::from_slice(&url.decode(parts[0]).unwrap()).unwrap();
    let payload: Value = serde_json::from_slice(&url.decode(parts[1]).unwrap()).unwrap();

    assert_eq!(header, json!({ "alg": "EdDSA", "typ": "JWT" }));
    assert_eq!((payload["sub"].as_str(), payload["aud"].as_str(), payload["iat"].as_u64(), payload["exp"].as_u64()), (Some("abc"), Some(RELAY), Some(1_000), Some(87_400)));

    // did:key:z, then base58 of 0xed 0x01 and the public key: "z6Mk…" for every Ed25519 key.
    let key = Keypair::from_private(PrivateKey::Ed25519(seed));
    let PublicKey::Ed25519(public) = key.public() else { unreachable!("an Ed25519 key") };
    let iss = payload["iss"].as_str().unwrap();
    assert!(iss.starts_with("did:key:z6Mk"), "{iss}");
    let decoded = bs58::decode(&iss["did:key:z".len()..]).into_vec().unwrap();
    assert_eq!((&decoded[..2], &decoded[2..]), (&[0xed, 0x01][..], &public[..]));

    let signature: [u8; 64] = url.decode(parts[2]).unwrap().try_into().unwrap();
    assert!(key.public().verify_signature(format!("{}.{}", parts[0], parts[1]).as_bytes(), &Signature::Ed25519(signature)).is_ok());

    let url = relay_url(RELAY, &token, "p123");
    assert!(url.starts_with("wss://relay.walletconnect.org?auth=") && url.ends_with("&projectId=p123") && url.contains("&ua=wc-2/"));
}

#[test]
fn a_code_names_its_topic_key_and_expiry_and_nothing_else_reads_as_one() {
    let topic = "ab".repeat(32);
    let code = format!("wc:{topic}@2?relay-protocol=irn&symKey={}&expiryTimestamp=1700000300&methods=[wc_sessionPropose]", "cd".repeat(32));
    assert_eq!(Pairing::parse(&code).unwrap(), Pairing { topic: topic.clone(), key: [0xcd; 32], expiry: Some(1_700_000_300) });

    for bad in [
        format!("wc:{topic}@1?bridge=https://x&key={}", "cd".repeat(32)),
        format!("wc:{topic}@2?relay-protocol=waku&symKey={}", "cd".repeat(32)),
        format!("wc:{topic}@2?relay-protocol=irn"),
        format!("wc:abcd@2?symKey={}", "cd".repeat(32)),
        "https://example.com".to_string(),
    ] {
        assert!(Pairing::parse(&bad).is_err(), "{bad}");
    }
}

fn proposal_params(public: &[u8; 32], required: Value, optional: Value) -> Value {
    json!({
        "relays": [{ "protocol": "irn" }],
        "proposer": { "publicKey": hex(public), "metadata": { "name": "Uni\u{0007}swap", "url": "https://app.example", "icons": ["https://app.example/i.png"] } },
        "requiredNamespaces": required,
        "optionalNamespaces": optional,
    })
}

#[test]
fn a_proposal_is_had_on_this_chain_alone_and_approving_it_gives_the_site_the_same_key() {
    let site_secret = [5; 32];
    let site_public = x25519_public(&site_secret);
    let optional = json!({ "eip155": { "chains": ["eip155:8908", "eip155:1"], "methods": ["personal_sign"], "events": [] } });
    let proposal = Proposal::read(42, "pt", &proposal_params(&site_public, json!({}), optional)).unwrap();

    assert_eq!(proposal.metadata.name, "Uniswap", "control characters dropped");
    assert_eq!(proposal.metadata.origin(), "https://app.example (by WalletConnect, as it says)");
    assert_eq!(proposal.refused_on(8908), None, "chains it would take need not be this one");

    let required = Proposal::read(1, "pt", &proposal_params(&site_public, json!({ "eip155": { "chains": ["eip155:1"] } }), json!({}))).unwrap();
    assert!(required.refused_on(8908).unwrap().contains("eip155:1"));
    let solana = Proposal::read(1, "pt", &proposal_params(&site_public, json!({ "solana": { "chains": ["solana:x"] } }), json!({}))).unwrap();
    assert!(solana.refused_on(8908).is_some());
    let chain_named = Proposal::read(1, "pt", &proposal_params(&site_public, json!({ "eip155:8908": {} }), json!({}))).unwrap();
    assert_eq!(chain_named.refused_on(8908), None);

    let wallet_secret = [6; 32];
    let approved = approve(&proposal, &wallet_secret, 8908, vec![format!("0x{}", "ab".repeat(20))], 1_000, 77);

    // The site reads the wallet's key from the answer and derives the session's key from it.
    let responder = key32(approved.answer["result"]["responderPublicKey"].as_str().unwrap()).unwrap();
    let theirs = sym_key(&site_secret, &responder);
    assert_eq!(hex(&theirs), approved.session.key);
    assert_eq!(topic_of(&theirs), approved.session.topic);
    assert_eq!(approved.answer["id"], 42);
    assert_eq!(approved.settle["method"], "wc_sessionSettle");
    assert_eq!(approved.settle["id"], 77);
    assert_eq!(approved.settle["params"]["expiry"], 1_000 + SESSION_EXPIRY);
    assert_eq!(approved.settle["params"]["controller"]["publicKey"], hex(&responder));
    assert_eq!(approved.settle["params"]["namespaces"]["eip155"]["accounts"], json!([format!("eip155:8908:0x{}", "ab".repeat(20))]));
    assert_eq!(approved.settle["params"]["namespaces"]["eip155"]["chains"], json!(["eip155:8908"]));
}

#[test]
fn a_sessions_request_reads_as_the_extensions_and_what_it_may_not_ask_is_answered_at_once() {
    let account = format!("0x{}", "ab".repeat(20));
    let session = Session { topic: "t".to_string(), key: hex(&[1; 32]), peer: Metadata { name: "S".to_string(), url: "https://s.example".to_string(), ..Metadata::default() }, accounts: vec![account.clone()], chain: 8908, expiry: 9 };
    let asked = |method: &str, params: Value, chain: &str| json!({ "request": { "method": method, "params": params }, "chainId": chain });

    let read = request_for(&session, 5, &asked("personal_sign", json!(["0x6869", account.to_uppercase().replace("0X", "0x")]), "eip155:8908")).unwrap();
    assert_eq!(read, json!({ "id": 5, "origin": "https://s.example (by WalletConnect, as it says)", "method": "personal_sign", "params": ["0x6869", account.to_uppercase().replace("0X", "0x")] }));

    assert_eq!(request_for(&session, 5, &asked("personal_sign", json!(["0x6869", format!("0x{}", "cd".repeat(20))]), "eip155:8908")).unwrap_err().0, 4100);
    assert_eq!(request_for(&session, 5, &asked("eth_sendTransaction", json!([{ "from": format!("0x{}", "cd".repeat(20)) }]), "eip155:8908")).unwrap_err().0, 4100);
    assert_eq!(request_for(&session, 5, &asked("eth_sign", json!([]), "eip155:8908")).unwrap_err().0, 4200);
    assert_eq!(request_for(&session, 5, &asked("personal_sign", json!([]), "eip155:1")).unwrap_err().0, 5100);
    assert!(request_for(&session, 5, &asked("wallet_switchEthereumChain", json!([{ "chainId": "0x1" }]), "eip155:1")).is_ok(), "a switch names the chain it is on");
}
