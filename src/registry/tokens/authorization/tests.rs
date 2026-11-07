use {super::*, crate::crypto::keypair::PrivateKey};

fn permit(owner: Address) -> TokenAuthorization {
    TokenAuthorization::Permit { token: Address::Contract([7; 32]), owner, spender: Address::Ed25519([0xb; 32]), amount: u256::new(300), nonce: u256::new(2), deadline: 5_000 }
}

#[test]
fn the_text_is_the_reference_tokens_line_by_line() {
    let owner = Address::Ed25519([0xa; 32]);

    assert_eq!(
        permit(owner).message(42),
        format!("chainv4 token permit\nchain: 42\ntoken: {}\nowner: {owner}\nspender: {}\namount: 300\nnonce: 2\ndeadline: 5000", Address::Contract([7; 32]), Address::Ed25519([0xb; 32]))
    );

    let transfer = TokenAuthorization::Transfer { token: Address::Contract([7; 32]), from: owner, to: Address::Ed25519([0xb; 32]), amount: u256::new(250), valid_after: 1_000, valid_before: 9_000, nonce: [7; 32] };

    assert!(transfer.message(42).ends_with(&format!("valid after: 1000\nvalid before: 9000\nnonce: 0x{}", "07".repeat(32))));
    assert_eq!(transfer.message_call().0, "transfer_message");
    assert_eq!(transfer.message_call().1.len(), 6);
}

#[test]
fn a_signed_one_holds_for_its_owner_on_its_chain_and_reads_back_from_its_text() {
    for keypair in [Keypair::from_seed([0xa1; 32]), Keypair::from_private(PrivateKey::Secp256k1([0xd4; 32]))] {
        let signed = SignedAuthorization::sign(permit(keypair.public().address()), &keypair, 42).unwrap();

        assert_eq!(signed.check(42), Ok(()), "{}", keypair.public().address().kind());
        assert_eq!(signed.check(43), Err("the signature does not hold".to_string()), "bound to its chain");
        assert_eq!(SignedAuthorization::from_text(&signed.to_text()), Ok(signed.clone()));

        // Its call: the terms, then the key and the signature.
        let Tx::ContractCall { to, data, .. } = signed.call() else { panic!("a call") };

        assert_eq!(to, Address::Contract([7; 32]));
        assert_eq!(data.len(), 6);
        assert_eq!(data[4], TxCallArgumentType::Bytes(signed.key.clone()));
    }

    // Another's key, or another's terms under the same signature, do not hold.
    let alice = Keypair::from_seed([0xa1; 32]);

    assert!(SignedAuthorization::sign(permit(Address::Ed25519([0xc; 32])), &alice, 42).is_err());

    let signed = SignedAuthorization::sign(permit(alice.public().address()), &alice, 42).unwrap();
    let other = Keypair::from_seed([0xb2; 32]);
    let theirs = SignedAuthorization { key: other.public().address().to_string().into_bytes(), ..signed.clone() };

    assert_eq!(theirs.check(42), Err("the key is not the owner's".to_string()));

    let mut changed = signed.clone();
    if let TokenAuthorization::Permit { amount, .. } = &mut changed.authorization {
        *amount = u256::new(301);
    }

    assert_eq!(changed.check(42), Err("the signature does not hold".to_string()));

    let by_contract = SignedAuthorization { authorization: permit(Address::Contract([1; 32])), ..signed };

    assert_eq!(by_contract.check(42), Err("only a key's account signs".to_string()));
    assert!(SignedAuthorization::from_text("{\"kind\": \"something else\"}").unwrap_err().contains("something else"));
}
