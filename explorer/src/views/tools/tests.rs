use {
    super::*,
    crate::light::Light,
    crate::proof::tests::{chain, proof_json},
    crate::store::Store,
    chainv4::abi::AbiValue,
    chainv4::abi::multisig::EvmBundle,
    chainv4::crypto::keypair::{Keypair, PrivateKey},
    chainv4::db::Column,
    chainv4::registry::multisig::{MultisigBody, MultisigBundle},
    chainv4::structs::address::Address,
    chainv4::structs::amount::Amount,
    chainv4::structs::hash::{Hash, HashType},
    chainv4::structs::hex::to_hex,
    chainv4::structs::tx::{FeeOffer, SignedTx, Tx},
    chainv4::structs::u256::u256,
    chainv4::swap::{BitcoinLeg, BitcoinParty, ChainLeg, Leg, Terms},
    serde_json::json,
};

const HASH: HashType = HashType::Blake3_256;
const WIDE: f32 = 1_200.0;
const PHONE: f32 = 380.0;

fn status(genesis: &Hash) -> crate::model::Status {
    serde_json::from_value(json!({
        "genesis": genesis.to_string(), "tip": "0x00", "height": 1, "timestamp": 0, "difficulty": "0", "work": "1",
        "hash": "blake3", "coin": { "symbol": "DEV", "decimals": 18 }, "finality": null, "fees": null,
    }))
    .unwrap()
}

fn genesis() -> Hash {
    Hash::from_bytes(HASH, &[7; 32]).unwrap()
}

/// A transfer of 1.5 DEV the development key signs.
fn transfer() -> SignedTx {
    let keypair = Keypair::development();
    let tx = Tx::Transfer {
        from: keypair.public().address(),
        to: Address::Ed25519([3; 32]),
        amount: Amount::U128(1_500_000_000_000_000_000),
    };

    SignedTx::sign(tx, 4, FeeOffer::NONE, &keypair)
}

fn bundle() -> MultisigBundle {
    let mut bundle = MultisigBundle {
        genesis: genesis(),
        body: MultisigBody {
            account: Address::Multisig([1; 32]),
            tx: Tx::Burn { from: Address::Multisig([1; 32]), amount: Amount::U128(5) },
            nonce: 2,
            fee: FeeOffer::NONE,
        },
        signatures: Vec::new(),
    };
    bundle.sign(&Keypair::development());

    bundle
}

/// The tools drawn on a tab, at a width.
fn drawn(store: &Store, light: &mut Light, width: f32, state: &mut ToolsState) -> String {
    crate::views::render(store, light, width, |ui, ctx| show(ui, ctx, state)).0
}

// Find events.

#[test]
fn the_find_events_tab_asks_for_what_it_is_given_on_any_screen() {
    for width in [WIDE, PHONE] {
        let mut state = ToolsState { tab: Tab::Events, ..ToolsState::default() };
        let text = drawn(&Store::default(), &mut Light::default(), width, &mut state);

        assert!(text.contains("Find events"), "{text}");
    }

    let query = |state: &find::FindState| state.query();
    let mut state = find::FindState::default();

    state.topic.topic = Some("0xdd".to_string());
    assert_eq!(query(&state), Some(crate::api::EventQuery::topic("0xdd", None)));

    state.contract = " evm:0x05 ".to_string();
    assert_eq!(query(&state), Some(crate::api::EventQuery::topic("0xdd", Some("evm:0x05"))));

    state.topic.topic = None;
    assert_eq!(query(&state), Some(crate::api::EventQuery::contract("evm:0x05", None)));

    state.contract.clear();
    assert_eq!(query(&state), None, "nothing to find");

    state.address = "secp256k1:0x05".to_string();
    assert_eq!(query(&state), Some(crate::api::EventQuery::mentioning("secp256k1:0x05")));
}

// Decode.

#[test]
fn a_raw_signed_transaction_decodes_with_its_kind_and_whether_it_checks() {
    let signed = transfer();
    let raw = to_hex(&wincode::serialize(&signed).unwrap());

    assert_eq!(decode::decode(&raw), Ok(decode::Decoded::Tx(Box::new(signed.clone()))));
    assert_eq!(decode::kind(&signed.tx), "transfer");
    assert_eq!(decode::kind(&Tx::EvmCall { to: Address::Evm([1; 20]), value: Amount::U128(0), input: Vec::new() }), "evm_call");
    assert_eq!(decode::signature_checks(&signed, Some(&genesis())), Ok(()));

    let mut forged = signed.clone();
    forged.nonce = 5;
    assert!(decode::signature_checks(&forged, None).is_err(), "a signature over another nonce");

    assert!(decode::decode(&format!("{raw}00")).unwrap_err().contains("more bytes after it"));
    assert!(decode::decode("0x0102").unwrap_err().starts_with("not a signed transaction"));
    assert!(decode::decode("hello").unwrap_err().starts_with("paste a raw signed transaction"));

    // Shown as the tx page shows one: the node's own JSON for it.
    let view = decode::node_view(&signed, HASH).unwrap();

    assert_eq!(view.id, signed.id(HASH).to_string());
    assert_eq!(view.kind(), "transfer");
    assert_eq!(view.tx["amount"], chainv4::structs::json::tx(&signed.tx)["amount"]);

    // The wallet's code for it, as its offline signer shows it.
    let code = ur::encode(&wincode::serialize(&signed).unwrap(), &ur::Type::Custom("chainv4-signed")).to_ascii_uppercase();

    assert_eq!(decode::decode(&code), Ok(decode::Decoded::Tx(Box::new(signed.clone()))));
    assert!(decode::decode(&ur::encode(&[1, 2], &ur::Type::Custom("chainv4-pairing"))).unwrap_err().contains("chainv4-pairing"), "not one the explorer reads");
}

#[test]
fn swap_codes_and_multisig_bundles_decode_as_the_wallet_writes_them() {
    let terms = Terms {
        hash: [9; 32],
        first: Leg::Chain(ChainLeg { contract: Address::Contract([2; 32]), token: None, item: None, amount: u256::new(5_000), payer: Some(Address::Ed25519([3; 32])), receiver: None }),
        second: Leg::Bitcoin(BitcoinLeg { testnet: true, sats: 100_000, payer: None, receiver: Some(BitcoinParty::Key([2; 33])) }),
        first_deadline: 100_000_000,
        second_deadline: 50_000_000,
    };
    assert_eq!(decode::decode(&terms.to_text()), Ok(decode::Decoded::Swap(Box::new(terms))));

    let bundle = bundle();
    assert_eq!(decode::decode(&bundle.to_text()), Ok(decode::Decoded::Bundle(Box::new(bundle.clone()))));

    // Its code, as the wallet shows it to the other members.
    let code = ur::encode(&wincode::serialize(&bundle).unwrap(), &ur::Type::Custom("chainv4-multisig")).to_ascii_uppercase();

    assert_eq!(decode::decode(&code), Ok(decode::Decoded::Bundle(Box::new(bundle.clone()))));
    assert!(decode::decode("UR:CHAINV4-MULTISIG/AEAE").is_err(), "one that does not read");

    let signatures = decode::bundle_signatures(&bundle);
    assert_eq!(signatures, [(Keypair::development().public().to_string(), Keypair::development().public().address().to_string(), true)]);

    let mut elsewhere = bundle.clone();
    elsewhere.genesis = Hash::from_bytes(HASH, &[8; 32]).unwrap();
    assert!(!decode::bundle_signatures(&elsewhere)[0].2, "signed for another chain");

    let evm = EvmBundle { chain_id: 1, contract: [1; 20], to: [2; 20], value: u256::ZERO, data: Vec::new(), nonce: u256::ZERO, signatures: Vec::new() };
    assert_eq!(decode::decode(&evm.to_text()), Ok(decode::Decoded::EvmBundle(Box::new(evm))));
    assert_eq!(decode::decode("ur:chainv4-pairing/aeae").unwrap_err(), "a chainv4-pairing code, which the explorer does not read");
}

#[test]
fn calldata_and_logs_read_by_the_built_in_abis() {
    let known = chainv4::abi::known::known_abis();
    let token = known.iter().find(|abi| abi.name.starts_with("ERC-20")).unwrap();
    let transfer = token.abi.functions.iter().find(|function| function.name == "transfer").unwrap();
    let input = transfer.encode_call(&[AbiValue::Address([0xab; 20]), AbiValue::Uint(u256::new(5))]).unwrap();
    let read = decode::decode_call(None, &known, &input).unwrap();

    assert_eq!(read.signature, "transfer(address,uint256)");
    assert_eq!(read.shown, format!("transfer(to = {}, value = 5)", to_hex(&[0xab; 20])));
    assert_eq!(read.contract, "read as ERC-20");
    assert!(decode::decode_call(None, &known, &[0xde, 0xad, 0xbe, 0xef]).unwrap_err().contains("0xdeadbeef"));
    assert!(decode::decode_call(None, &known, &[1]).is_err());

    // An ERC-20 Transfer: two addresses in topics, the value in the data.
    let pad = |bytes: [u8; 20]| -> [u8; 32] {
        let mut topic = [0; 32];
        topic[12..].copy_from_slice(&bytes);
        topic
    };
    let topic = hash::selector("Transfer(address,address,uint256)").unwrap().topic;
    let topics = decode::topics(&format!("{topic}\n{}, {}", to_hex(&pad([1; 20])), to_hex(&pad([2; 20])))).unwrap();
    let mut data = [0u8; 32];
    data[31] = 7;
    let log = decode::decode_log(None, &known, &topics, &data).unwrap();

    assert_eq!(log.signature, "Transfer(address,address,uint256)");
    assert_eq!(log.shown, format!("Transfer(from = {}, to = {}, value = 7)", to_hex(&[1; 20]), to_hex(&[2; 20])));
    assert!(decode::topics("0x01").unwrap_err().contains("is not a topic"));

    // An ERC-721 Transfer shares the topic, its id a topic too.
    let mut nft = topics.clone();
    nft.push(data);
    let read = decode::decode_log(None, &known, &nft, &[]).unwrap();
    assert!(read.contract == "read as ERC-721" && read.shown.ends_with("= 7)"), "{read:?}");

    // One pasted reads alone, and says so.
    let pasted = chainv4::abi::Abi::parse(r#"[{"type": "function", "name": "transfer", "inputs": [{"name": "payee", "type": "address"}, {"name": "cents", "type": "uint256"}]}]"#).unwrap();
    let read = decode::decode_call(Some(&pasted), &[], &input).unwrap();

    assert_eq!(read.contract, "read by the ABI kept for this contract");
    assert!(read.shown.contains("cents = 5"), "{read:?}");
    assert!(decode::decode_log(Some(&pasted), &[], &topics, &data).is_err(), "it has no such event, and nothing else is asked");
}

#[test]
fn the_decode_tab_shows_a_transaction_on_any_screen() {
    let store = Store { status: Some(status(&genesis())), ..Store::default() };

    for width in [WIDE, PHONE] {
        let mut state = ToolsState::default();
        state.decode.input = to_hex(&wincode::serialize(&transfer()).unwrap());
        state.decode.calldata = "0xa9059cbb".to_string();
        let text = drawn(&store, &mut Light::default(), width, &mut state);

        for wanted in ["Decode", "Transaction · transfer", "signature checks", "1.5 DEV", "EVM CALLDATA AND LOGS", "no ABI here has a function of selector 0xa9059cbb"] {
            assert!(text.contains(wanted), "{wanted} at {width} wide, in {text}");
        }

        state.decode.input = bundle().to_text();
        let text = drawn(&store, &mut Light::default(), width, &mut state);

        for wanted in ["A multisig's transaction · burn", "this chain", "signatures · 1", "checks"] {
            assert!(text.contains(wanted), "{wanted} at {width} wide, in {text}");
        }
    }
}

// Convert.

#[test]
fn units_convert_both_ways_at_the_chain_s_decimals() {
    assert_eq!(convert::to_whole("1500000000000000000", 18), Ok("1.5".to_string()));
    assert_eq!(convert::to_whole("1,000,000", 6), Ok("1".to_string()));
    assert_eq!(convert::to_whole("1", 18), Ok("0.000000000000000001".to_string()));
    assert_eq!(convert::to_whole("0", 18), Ok("0".to_string()));
    assert!(convert::to_whole("1.5", 18).is_err(), "the smallest unit has no fraction");

    assert_eq!(convert::to_smallest("1.5", 18), Ok("1500000000000000000".to_string()));
    assert_eq!(convert::to_smallest("0.000001", 6), Ok("1".to_string()));
    assert!(convert::to_smallest("0.0000001", 6).unwrap_err().contains("at most 6 decimals"));
    assert_eq!(convert::to_whole(&convert::to_smallest("123.456", 18).unwrap(), 18), Ok("123.456".to_string()), "and back");
}

#[test]
fn an_address_and_its_evm_alias_convert_both_ways() {
    let secp = "secp256k1:0x5aaeb6053f3e94c9b9a09f33669435e7ef1beaed";
    let alias = convert::alias(secp).unwrap();

    assert_eq!(alias.evm, "0x5aaeb6053f3e94c9b9a09f33669435e7ef1beaed");
    assert_eq!(alias.checksummed, "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed", "EIP-55's own example");
    assert_eq!(alias.accounts[0], secp);

    let back = convert::alias("0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed").unwrap();
    assert_eq!(back.accounts, [secp.to_string(), "evm:0x5aaeb6053f3e94c9b9a09f33669435e7ef1beaed".to_string()]);

    // An Edwards account's alias is a hash of its key, one way only.
    let edwards = Address::Ed25519([3; 32]);
    let hashed = convert::alias(&edwards.to_string()).unwrap();
    assert_eq!(hashed.evm, to_hex(&chainv4::abi::evm_address(&edwards)));
    assert!(hashed.accounts.is_empty());

    assert!(convert::alias("0x01").is_err());
    assert!(convert::alias("hello").is_err());
}

#[test]
fn a_public_key_reads_as_the_address_of_each_scheme_it_fits() {
    let edwards = Keypair::development().public();
    let PublicKey32(bytes) = edwards_bytes(&edwards);
    let found = convert::key_addresses(&to_hex(&bytes)).unwrap();

    assert_eq!(found.iter().map(|(scheme, _)| *scheme).collect::<Vec<_>>(), ["ed25519", "edblake"]);
    assert_eq!(found[0].1, Ok(edwards.address().to_string()));

    let secp = Keypair::from_private(PrivateKey::Secp256k1([1; 32])).public();
    let found = convert::key_addresses(&secp.to_string()).unwrap();
    assert_eq!(found, [("secp256k1", Ok(secp.address().to_string()))]);

    let chainv4::crypto::keypair::PublicKey::Secp256k1(compressed) = secp else { unreachable!() };
    let both = convert::key_addresses(&to_hex(&compressed)).unwrap();
    assert_eq!(both.iter().map(|(scheme, _)| *scheme).collect::<Vec<_>>(), ["secp256k1", "bitcoin"]);
    assert!(both[1].1.as_ref().unwrap().starts_with("bc1q"));

    let mut off_curve = [0u8; 33];
    off_curve[0] = 2;
    assert!(convert::key_addresses(&to_hex(&off_curve)).unwrap()[0].1.is_err(), "not a point");
    assert!(convert::key_addresses("0x0102").unwrap_err().contains("2 bytes"));
}

struct PublicKey32([u8; 32]);

fn edwards_bytes(key: &chainv4::crypto::keypair::PublicKey) -> PublicKey32 {
    match key {
        chainv4::crypto::keypair::PublicKey::Ed25519(bytes) => PublicKey32(*bytes),
        _ => unreachable!("the development key is ed25519"),
    }
}

#[test]
fn bech32_and_its_witness_program_convert_both_ways() {
    let read = convert::from_bech32("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4").unwrap();

    assert_eq!(read, convert::Witness { hrp: "bc".to_string(), kind: "P2WPKH", program: "0x751e76e8199196d454941c45d1b3a323f1433bd6".to_string() });
    assert_eq!(convert::to_bech32("bc", &read.program), Ok("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4".to_string()));
    assert_eq!(convert::to_bech32("tb", &read.program), Ok("tb1qw508d6qejxtdg4y5r3zarvary0c5xw7kxpjzsx".to_string()));
    assert_eq!(convert::from_bech32("tb1qw508d6qejxtdg4y5r3zarvary0c5xw7kxpjzsx").unwrap().hrp, "tb");
    assert!(convert::from_bech32("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t5").is_err(), "a bad checksum");
    assert!(convert::to_bech32("bc", "0x0102").unwrap_err().contains("not 2"));
}

#[test]
fn the_convert_tab_shows_its_converters_on_any_screen() {
    let store = Store { status: Some(status(&genesis())), ..Store::default() };

    for width in [WIDE, PHONE] {
        let mut state = ToolsState { tab: Tab::Convert, ..ToolsState::default() };
        state.convert.smallest = "2500000000000000000".to_string();
        state.convert.whole = "0.25".to_string();
        state.convert.address = "secp256k1:0x5aaeb6053f3e94c9b9a09f33669435e7ef1beaed".to_string();
        state.convert.bech32 = "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4".to_string();
        let text = drawn(&store, &mut Light::default(), width, &mut state);

        for wanted in ["UNITS", "2.5 DEV", "250000000000000000", "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed", "PUBLIC KEY TO ADDRESS", "bc · P2WPKH", "0x751e76e8199196d454941c45d1b3a323f1433bd6"] {
            assert!(text.contains(wanted), "{wanted} at {width} wide, in {text}");
        }
    }
}

// Hash & verify.

#[test]
fn hashes_match_known_vectors() {
    let found: std::collections::HashMap<&str, String> = hash::digests(b"abc").into_iter().collect();

    assert_eq!(found["sha256"], "0xba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    assert_eq!(found["keccak256"], "0x4e03657aea45a94fc7d47ba826c8d667c0d1e6e33a64a036ec44f58fa12d6c45");
    assert_eq!(found["blake3"], "0x6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85");
    assert_eq!(found["blake2s"], "0x508c5e8c327c14e2e1a72ba34eeb452f37458b209ed63a294d999b4c86675982");
    assert_eq!(found["blake2b"], "0xbddd813c634239723171ef3fee98579b94964e3bb1cb3e427262c8c068d52319");
    assert_eq!(found["sha256d"], "0x4f8b42c22dd3729b519ba6f68d2da7cc5b2d606d05daed5ad5128cc03e6c6358");

    let empty: std::collections::HashMap<&str, String> = hash::digests(b"").into_iter().collect();
    assert_eq!(empty["keccak256"], "0xc5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470");
    assert_eq!(empty["xxh3"], "0x2d06800538d394c2");

    assert_eq!(hash::bytes_of("0x616263", true), Ok(b"abc".to_vec()));
    assert_eq!(hash::bytes_of("abc", false), Ok(b"abc".to_vec()));
    assert!(hash::bytes_of("0xabc", true).is_err());
}

#[test]
fn a_signature_checks_for_its_key_and_message_and_not_otherwise() {
    for keypair in [Keypair::development(), Keypair::from_private(PrivateKey::Secp256k1([1; 32])), Keypair::from_private(PrivateKey::Bitcoin([2; 32])), Keypair::from_private(PrivateKey::EdBlake([3; 32]))] {
        let key = keypair.public().to_string();
        let signature = keypair.sign(b"hello").to_string();

        assert_eq!(hash::check_signature(&key, b"hello", &signature), Ok(()), "{key}");
        assert_eq!(hash::check_signature(&key, b"hullo", &signature), Err("it does not check: this key did not sign this message".to_string()), "{key}");
    }

    let key = Keypair::development().public().to_string();
    assert!(hash::check_signature(&key, b"hello", "0x01").unwrap_err().starts_with("a signature is 64 bytes"));
    assert!(hash::check_signature("hello", b"hello", "0x01").unwrap_err().starts_with("not a key"));
}

#[test]
fn a_selector_and_a_topic_come_from_a_signature() {
    let transfer = hash::selector("transfer(address,uint256)").unwrap();
    assert_eq!((transfer.canonical.as_str(), transfer.selector.as_str()), ("transfer(address,uint256)", "0xa9059cbb"));

    let event = hash::selector(" Transfer(address, address, uint256) ").unwrap();
    assert_eq!(event.canonical, "Transfer(address,address,uint256)");
    assert_eq!(event.topic, "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef");

    assert_eq!(hash::selector("transfer(address,uint)").unwrap().canonical, "transfer(address,uint256)", "uint alone is 256 bits");
    assert!(hash::selector("(address)").is_err());
    assert!(hash::selector("transfer").is_err());
}

#[test]
fn the_hash_tab_shows_digests_a_signature_check_and_a_selector_on_any_screen() {
    let keypair = Keypair::development();

    for width in [WIDE, PHONE] {
        let mut state = ToolsState { tab: Tab::Hash, ..ToolsState::default() };
        state.hash.input = "abc".to_string();
        state.hash.key = keypair.public().to_string();
        state.hash.message = "hello".to_string();
        state.hash.signature = keypair.sign(b"hello").to_string();
        state.hash.evm = "transfer(address,uint256)".to_string();
        let text = drawn(&Store::default(), &mut Light::default(), width, &mut state);

        for wanted in ["HASHES", "3 bytes", "0xba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad", "checks", "this key signed the message itself", "0xa9059cbb"] {
            assert!(text.contains(wanted), "{wanted} at {width} wide, in {text}");
        }
    }
}

// Proof.

#[test]
fn each_pick_asks_for_its_proof_at_the_block_given() {
    let address = Address::Ed25519([3; 32]);
    let block = Some("0xbb".to_string());

    assert_eq!(
        proof::ask_for(proof::Pick::Account, &address.to_string(), "", block.clone()),
        Ok(crate::api::Ask::Proof { column: "accounts".to_string(), key: to_hex(&chainv4::db::address_key(&address)), block: block.clone() })
    );
    assert_eq!(
        proof::ask_for(proof::Pick::Entry, "0x636F696E3A737570706C79", "validators", None),
        Ok(crate::api::Ask::Proof { column: "validators".to_string(), key: "0x636f696e3a737570706c79".to_string(), block: None })
    );
    assert_eq!(proof::ask_for(proof::Pick::Tx, " 0xAB ", "", block.clone()), Ok(crate::api::Ask::ReceiptProof("0xab".to_string())));
    assert!(proof::ask_for(proof::Pick::Entry, "0x01", "blocks", None).unwrap_err().starts_with("blocks is not a chain-state column"));
    assert!(proof::ask_for(proof::Pick::Account, "hello", "", None).is_err());
    assert!(proof::ask_for(proof::Pick::Paste, "", "", None).is_err());
}

#[test]
fn a_pasted_proof_checks_against_the_light_client_or_says_it_cannot_on_any_screen() {
    let held = chainv4::registry::supply::Supply { minted: u256::new(1_500), burned: u256::new(500) };
    let (client, state) = chain(&[(Column::Validators, chainv4::registry::supply::SUPPLY_KEY, &wincode::serialize(&held).unwrap())]);
    let pasted = proof_json(&client, &state, Column::Validators, chainv4::registry::supply::SUPPLY_KEY).to_string();
    let mut following = Light::started_with(client);

    for width in [WIDE, PHONE] {
        let mut tools = ToolsState { tab: Tab::Proof, ..ToolsState::default() };
        tools.proof.pick = proof::Pick::Paste;
        tools.proof.pasted = pasted.clone();

        let text = drawn(&Store::default(), &mut Light::default(), width, &mut tools);
        for wanted in ["WHAT TO PROVE", "not checked", "can't be checked", "header", "state root", "path", "the coin's supply: minted"] {
            assert!(text.contains(wanted), "{wanted} at {width} wide, in {text}");
        }
        assert!(!text.contains("proven"), "nothing is proven without the light client: {text}");

        let text = drawn(&Store::default(), &mut following, width, &mut tools);
        for wanted in ["proven", "a header the light client checked itself", "the header's"] {
            assert!(text.contains(wanted), "{wanted} at {width} wide, in {text}");
        }
    }

    // The fetch: nothing asked yet, then the button asks at the light client's tip.
    let mut tools = ToolsState { tab: Tab::Proof, ..ToolsState::default() };
    let text = drawn(&Store::default(), &mut Light::default(), WIDE, &mut tools);
    assert!(text.contains("Fetch and check") && text.contains("a proof fetched now can be read, not checked"), "{text}");
}
