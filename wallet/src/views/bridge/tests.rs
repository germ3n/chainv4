use {super::*, chainv4::structs::hex::to_hex};

/// A value as the node's /call writes it.
fn typed(kind: &str, value: Value) -> Value {
    json!({ "type": kind, "value": value })
}

#[test]
fn a_transfer_out_is_read_back_only_as_the_gateway_signed_it() {
    let identity = [&Address::Contract([7; 32]).to_abi()[..], b"1800000000000"].concat();
    let transfer = Transfer {
        from: identity.clone(),
        to: vec![0xee; 20],
        source: 1337,
        target: 11155111,
        nonce: 4,
        kind: AssetKind::Token,
        origin: 1337,
        asset: Address::Contract([9; 32]).to_abi().to_vec(),
        id: u256::ZERO,
        amount: u256::from(250u64),
        recipient: vec![0xb0; 20],
        name: "Dev Token".into(),
        symbol: "DVT".into(),
        decimals: 6,
        uri: String::new(),
    };
    let values = |digest: &[u8]| {
        vec![
            typed("i64", json!(11155111)),
            typed("i64", json!(1)),
            typed("i64", json!(1337)),
            typed("bytes", json!(to_hex(&transfer.asset))),
            typed("u256", json!("0")),
            typed("u256", json!("250")),
            typed("bytes", json!(to_hex(&transfer.recipient))),
            typed("string", json!("Dev Token")),
            typed("string", json!("DVT")),
            typed("i64", json!(6)),
            typed("string", json!("")),
            typed("bytes", json!(to_hex(digest))),
            typed("address", json!(Address::Ed25519([1; 32]).to_string())),
        ]
    };

    assert_eq!(left(&values(&transfer.digest()), &identity, [0xee; 20], 1337, 4), Some(transfer.clone()));
    // A node that changed any field, or the digest, is caught: the signers signed another.
    assert_eq!(left(&values(&[0; 32]), &identity, [0xee; 20], 1337, 4), None);
    assert_eq!(left(&values(&transfer.digest()), &identity, [0xee; 20], 1337, 5), None, "another nonce");
    assert_eq!(left(&values(&transfer.digest()), &identity, [0xdd; 20], 1337, 4), None, "another Gateway.sol");
    assert_eq!(left(&values(&transfer.digest()), &Address::Contract([7; 32]).to_abi(), [0xee; 20], 1337, 4), None, "the gateway as it was set up another time");

    // Gateway.sol's delivered key differs by nonce, and by the gateway it came from: a chain
    // restarted from the identical genesis counts its nonces from 1 again.
    assert_ne!(transfer.delivered_key(), Transfer { nonce: 5, ..transfer.clone() }.delivered_key());
    assert_ne!(transfer.delivered_key(), Transfer { from: Address::Contract([7; 32]).to_abi().to_vec(), ..transfer.clone() }.delivered_key());
}

#[test]
fn a_voided_transfer_says_what_became_of_it_rather_than_offering_a_delivery() {
    assert_eq!(refund_state(false), "voided, refund to come");
    assert_eq!(refund_state(true), "refunded");
}

#[test]
fn approvals_name_what_each_kind_lets_the_gateway_move() {
    let spender = [0x11; 20];

    assert_eq!(evm_approval(AssetKind::Coin, spender, u256::ZERO, u256::from(5u64)), None);
    assert_eq!(evm_approval(AssetKind::Token, spender, u256::ZERO, u256::from(5u64)).unwrap()[..4], [0x09, 0x5e, 0xa7, 0xb3], "approve(address,uint256)");
    assert_eq!(evm_approval(AssetKind::Nft, spender, u256::from(7u64), u256::from(1u64)).unwrap()[4 + 32..], encode_values(&[AbiType::Uint(256)], &[AbiValue::Uint(u256::from(7u64))]).unwrap()[..], "the id, not the amount");
    assert_eq!(evm_approval(AssetKind::Multi, spender, u256::ZERO, u256::ZERO).unwrap()[..4], [0xa2, 0x2c, 0xb4, 0x65], "setApprovalForAll(address,bool)");
}

#[test]
fn withdrawal_addresses_read_as_their_scripts() {
    // BIP173's P2WPKH and P2WSH examples.
    assert_eq!(output_script("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4").map(|script| to_hex(&script)), Some("0x0014751e76e8199196d454941c45d1b3a323f1433bd6".to_string()));
    assert_eq!(output_script("tb1qrp33g0q5c5txsp9arysrx4k6zdkfs4nce4xj0gdcccefvpysxf3q0sl5k7").map(|script| script.len()), Some(34));
    assert_eq!(output_script("0x5120").map(|script| script.len()), Some(2), "any script, as 0x…");
    assert_eq!(output_script("bc1p5d7rjq7g6rdk2yhzks9smlaqtedr4dekq08ge8ztwac72sfr9rusxg3297"), None, "Taproot only as its script");
    assert_eq!(output_script(""), None);
}

#[test]
fn esplora_s_proof_is_given_in_bitcoin_s_own_byte_order_without_the_witness() {
    use chainv4::crypto::bitcoin::tx::{TxIn, TxOut};

    let tx = Transaction {
        version: 2,
        inputs: vec![TxIn { txid: [3; 32], vout: 0, script_sig: Vec::new(), sequence: 0xffff_ffff, witness: vec![vec![1, 2, 3]] }],
        outputs: vec![TxOut { value: 5_000, script: vec![0x51] }],
        lock_time: 0,
    };
    let shown = |bytes: [u8; 32]| {
        let mut reversed = bytes;
        reversed.reverse();
        to_hex(&reversed).trim_start_matches("0x").to_string()
    };
    let merkle = json!({ "block_height": 9, "merkle": [shown([0xaa; 32]), shown([0x01; 32])], "pos": 2 });
    let of_coinbase = json!({ "block_height": 9, "merkle": [shown([0xcc; 32]), shown([0x02; 32])], "pos": 0 });
    let hex = to_hex(&tx.encode());
    let hex = hex.trim_start_matches("0x");
    let proof = esplora_proof(hex, &merkle, &shown([0x0b; 32]), hex, &of_coinbase).unwrap();

    assert_eq!(proof, Proof { block: vec![0x0b; 32], raw: tx.base(), index: 2, path: [[0xaa; 32], [0x01; 32]].concat(), coinbase: tx.base(), coinbase_path: [[0xcc; 32], [0x02; 32]].concat() });
    assert!(esplora_proof("00", &merkle, &shown([0; 32]), hex, &of_coinbase).is_err());
    assert!(esplora_proof(hex, &merkle, &shown([0; 32]), "00", &of_coinbase).is_err());
    assert!(esplora_proof(hex, &json!({ "pos": 1 }), &shown([0; 32]), hex, &of_coinbase).unwrap_err().contains("Merkle"));
    assert!(esplora_proof(hex, &merkle, &shown([0; 32]), hex, &json!({ "pos": 0 })).unwrap_err().contains("Merkle"));

    // What a withdrawal says it arrives as: all but the tenth the wallet lets it lose.
    assert_eq!(least_arriving(10_000), 9_000);
    assert_eq!(least_arriving(123_456), 111_111);
}

#[test]
fn the_deposit_address_is_the_federation_s_under_the_account() {
    use chainv4::registry::federation::Federation;

    let federation = |secrets: &[u8]| DecidedFederation {
        version: 0,
        federation: Federation {
            signers: secrets.iter().map(|byte| chainv4::crypto::secp256k1::public_key(&[*byte; 32]).unwrap()).collect(),
            threshold: 2,
            chain_depth: 20,
            evm_depth: 12,
            bitcoin_depth: 2,
            btc: None,
        },
    };
    let alice = Address::Ed25519([0xa; 32]);
    let gateway = Address::Contract([7; 32]);
    let output = deposit_output(&alice, 1337, &gateway, &federation(&[21, 22, 23]));

    assert_eq!(output[..2], [0, 32], "a P2WSH");
    assert_ne!(output, deposit_output(&Address::Ed25519([0xb; 32]), 1337, &gateway, &federation(&[21, 22, 23])), "each account its own");
    assert_ne!(output, deposit_output(&alice, 1337, &gateway, &federation(&[31, 32, 33])), "each federation its own");
    assert_ne!(output, deposit_output(&alice, 1338, &gateway, &federation(&[21, 22, 23])), "each chain its own");
    assert_ne!(output, deposit_output(&alice, 1337, &Address::Contract([8; 32]), &federation(&[21, 22, 23])), "each gateway its own");
}

#[cfg(all(feature = "bitcoin-light", not(target_arch = "wasm32")))]
#[test]
fn every_address_of_every_account_has_its_deposit_watched() {
    use chainv4::registry::federation::Federation;

    let federation = DecidedFederation {
        version: 0,
        federation: Federation {
            signers: [21u8, 22, 23].iter().map(|byte| chainv4::crypto::secp256k1::public_key(&[*byte; 32]).unwrap()).collect(),
            threshold: 2,
            chain_depth: 20,
            evm_depth: 12,
            bitcoin_depth: 2,
            btc: None,
        },
    };
    let gateway = Address::Contract([7; 32]);
    let assurance = chainv4::light::Assurance { block: chainv4::structs::hash::Hash::zero(HashType::default_chain()), height: 1, final_: true };
    let genesis = chainv4::structs::hash::Hash::zero(HashType::default_chain());
    let mut chain = crate::chain::Chain::default();
    chain.genesis = Some(genesis);
    chain.directory = Some(crate::chain::Proven { value: Some(Directory { btc_gateway: Some(gateway), ..Directory::default() }), assurance });
    chain.federation = Some(crate::chain::Proven { value: Some(federation.clone()), assurance });
    let holders = crate::keys::open(&crate::views::drawn::two_phrases()).unwrap();
    let watched = deposit_outputs(&chain, &holders);
    let chain_id = chainv4::crypto::ethereum::chain_id(&genesis);

    assert_eq!(watched.len(), holders.iter().map(|holder| holder.addresses().len()).sum::<usize>());

    for address in holders.iter().flat_map(crate::keys::Holder::addresses) {
        assert!(watched.contains(&deposit_output(&address, chain_id, &gateway, &federation)), "{address}'s deposit address");
    }
}

#[test]
fn the_bridge_offers_every_address_and_shows_the_deposit_address_of_the_one_picked_on_every_screen() {
    use chainv4::registry::federation::Federation;

    let federation = DecidedFederation {
        version: 0,
        federation: Federation {
            signers: [21u8, 22, 23].iter().map(|byte| chainv4::crypto::secp256k1::public_key(&[*byte; 32]).unwrap()).collect(),
            threshold: 2,
            chain_depth: 20,
            evm_depth: 12,
            bitcoin_depth: 2,
            btc: None,
        },
    };
    let assurance = chainv4::light::Assurance { block: chainv4::structs::hash::Hash::zero(HashType::default_chain()), height: 1, final_: true };
    let mut book = crate::views::drawn::two_phrases();
    let mut holders = crate::keys::open(&book).unwrap();
    let mine = holders[0].addresses();

    for size in crate::views::drawn::SCREENS {
        for (picked, scheme) in [(None, "Ed25519"), (Some(mine[3]), "Bitcoin")] {
            let mut world = crate::views::drawn::following(&holders);
            world.chain.directory = Some(crate::chain::Proven { value: Some(Directory { btc_gateway: Some(Address::Contract([7; 32])), ..Directory::default() }), assurance });
            world.chain.federation = Some(crate::chain::Proven { value: Some(federation.clone()), assurance });
            let mut state = BridgeState { way: Way::Bitcoin, from: picked, ..BridgeState::default() };
            let mut send = SendState::default();
            let mut draw = |ui: &mut Ui, ctx: &mut Ctx| {
                if let Some(picked) = picked {
                    crate::views::from::remember(ctx.wake, picked);
                }
                show(ui, ctx, &mut state, &mut send);
            };
            let (width, _) = crate::views::drawn::page_in(&mut world, &mut book, &mut holders, (size.0, 4_000.0), &mut draw);
            let said = crate::views::drawn::said_in(&mut world, &mut book, &mut holders, (size.0, 4_000.0), &mut draw);

            assert!(width <= size.0 + 1.0, "{width} on {size:?}");
            assert!(said.contains("FROM"), "the picker: {said}");
            assert!(said.contains(&format!("minted as wBTC to {scheme}")), "{scheme} on {size:?}: {said}");
        }
    }
}
