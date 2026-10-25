use {
    super::*,
    crate::crypto::keypair::{KeyType, Keypair},
    crate::crypto::secp256k1,
    crate::structs::address::Address,
    crate::structs::hash::HashType,
    crate::structs::hex::from_hex,
};

fn bytes<const N: usize>(hex: &str) -> [u8; N] {
    from_hex(hex).unwrap().try_into().unwrap()
}

/// EIP-712's own example: Cow mails Bob.
fn mail() -> (Typed, Typed) {
    let person = |name: &str, wallet: &str| {
        Value::Struct(Typed {
            name: "Person",
            fields: vec![("name", "string", Value::String(name.to_string())), ("wallet", "address", Value::Address(bytes(wallet)))],
        })
    };
    let domain = Typed {
        name: "EIP712Domain",
        fields: vec![
            ("name", "string", Value::String("Ether Mail".to_string())),
            ("version", "string", Value::String("1".to_string())),
            ("chainId", "uint256", Value::Uint(u256::ONE)),
            ("verifyingContract", "address", Value::Address(bytes("0xCcCCccccCCCCcCCCCCCcCcCccCcCCCcCcccccccC"))),
        ],
    };
    let message = Typed {
        name: "Mail",
        fields: vec![
            ("from", "Person", person("Cow", "0xCD2a3d9F938E13CD947Ec05AbC7FE734Df8DD826")),
            ("to", "Person", person("Bob", "0xbBbBBBBbbBBBbbbBbbBbbbbBBbBbbbbBbBbbBBbB")),
            ("contents", "string", Value::String("Hello, Bob!".to_string())),
        ],
    };

    (domain, message)
}

#[test]
fn eip_712s_own_example_hashes_as_the_standard_says() {
    let (domain, message) = mail();

    assert_eq!(message.encode_type(), "Mail(Person from,Person to,string contents)Person(string name,address wallet)");
    assert_eq!(domain.hash(), bytes::<32>("0xf2cee375fa42b42143804025fc449deafd50cc031ca257e0b194a650a912090f"));
    assert_eq!(message.hash(), bytes::<32>("0xc52c0ee5d84264471806290a3f2c4cecfc5490626bf912d01f240d7a274b371e"));
    assert_eq!(digest(&domain, &message), bytes::<32>("0xbe609aee343fb3c4b28e1df9e632fca64fcfaede20f02e86244efddf30957bd2"));
}

#[test]
fn a_wallets_signature_recovers_its_key_and_checks_as_chainv4_takes_it() {
    // Cow's key in the standard's example is the keccak256 of "cow"; its signature, r, s and v.
    let (domain, message) = mail();
    let hash = digest(&domain, &message);
    let mut signature = [0; 65];
    signature[..32].copy_from_slice(&bytes::<32>("0x4355c47d63924e8a72e509b65029052eb6c299d53a04e167c5775fd466751c9d"));
    signature[32..64].copy_from_slice(&bytes::<32>("0x07299936d304c153f6443dfa05f40ff007d72911b6f72307f996231605b91562"));
    signature[64] = 28;

    let (key, low) = secp256k1::recover(&hash, &signature).unwrap();

    assert_eq!(secp256k1::address(&key), Some(bytes("0xCD2a3d9F938E13CD947Ec05AbC7FE734Df8DD826")), "Cow's");
    assert_eq!(secp256k1::public_key(&secp256k1::keccak(b"cow")), Some(key));
    assert!(secp256k1::verify_hash(&key, &hash, &low));
    assert_eq!(secp256k1::sign_hash(&secp256k1::keccak(b"cow"), &hash), low, "deterministic, as RFC 6979 is");

    signature[64] = 3;

    assert_eq!(secp256k1::recover(&hash, &signature), None, "no such v");
}

#[test]
fn transfers_and_token_transfers_read_field_by_field_and_the_rest_as_bytes() {
    let fee = FeeOffer { gas_limit: 21_000, max_price: u256::new(2_000_000_000), tip: u256::ONE };
    let from = Keypair::from_seed([7; 32]).public().address();
    let to = Address::Ed25519([0xab; 32]);
    let names = |typed: &Typed| typed.fields.iter().map(|(name, _, _)| *name).collect::<Vec<_>>();
    let transfer = message(&Tx::Transfer { from, to, amount: Amount::U128(5) }, 3, &fee);

    assert_eq!(transfer.encode_type(), "Transfer(string from,string to,uint256 amount,uint64 nonce,uint64 gasLimit,uint256 maxPrice,uint256 tip)");
    assert_eq!(transfer.fields[1].2, Value::String(to.to_string()), "addresses as their text");

    #[cfg(feature = "xxh3")]
    {
        use crate::{crypto::hasher::Hasher, structs::tx::TxCallArgumentType};

        let token = Tx::ContractCall {
            to: Address::Contract([0x11; 32]),
            func: Hasher::digest(HashType::Xxh3_64, b"transfer"),
            amount: Amount::U128(0),
            data: vec![TxCallArgumentType::Address(to), TxCallArgumentType::U256(u256::new(250))],
        };

        assert_eq!(names(&message(&token, 3, &fee)), ["token", "to", "amount", "nonce", "gasLimit", "maxPrice", "tip"]);
        assert_eq!(message(&token, 3, &fee).name, "TokenTransfer");
    }

    // Anything else, and a transfer whose amount could be narrower, go as their bytes: no two
    // encodings share a typed form.
    let burn = Tx::Burn { from, amount: Amount::U128(1) };
    let wide = Tx::Transfer { from, to, amount: Amount::U256(u256::new(5)) };

    assert_eq!(names(&message(&burn, 0, &fee))[..2], ["kind", "data"]);
    assert_eq!(message(&burn, 0, &fee).fields[0].2, Value::String("Burn".to_string()));
    assert_eq!(message(&wide, 3, &fee).name, "Transaction");
    assert_ne!(message(&wide, 3, &fee).hash(), transfer.hash());
    assert_eq!(message(&Tx::Transfer { from, to, amount: Amount::U256(u256::MAX) }, 3, &fee).name, "Transfer", "too wide for 128 bits");
}

#[test]
fn a_signature_holds_on_its_chain_alone() {
    let key = Keypair::generate(KeyType::Secp256k1).unwrap();
    let tx = Tx::Transfer {
        from: key.public().address(),
        to: Address::Ed25519([0xab; 32]),
        amount: Amount::U128(5),
    };
    let ours = Hash::from_bytes(HashType::default_chain(), &[1; 32]).unwrap();
    let theirs = Hash::from_bytes(HashType::default_chain(), &[2; 32]).unwrap();

    assert_ne!(transaction_digest(&tx, 0, &FeeOffer::NONE, &ours), transaction_digest(&tx, 0, &FeeOffer::NONE, &theirs));
    assert_eq!(domain(&ours).fields[2].2, Value::Bytes32([1; 32]), "the genesis id is the salt");
}

#[test]
fn a_transaction_signed_as_typed_data_holds_on_its_chain_and_nowhere_else() {
    use crate::crypto::keypair::Signature;
    use crate::crypto::verifiable::{Verifiable, VerifyError};
    use crate::structs::tx::SignedTx;

    let key = Keypair::generate(KeyType::Secp256k1).unwrap();
    let ours = Hash::from_bytes(HashType::default_chain(), &[1; 32]).unwrap();
    let theirs = Hash::from_bytes(HashType::default_chain(), &[2; 32]).unwrap();
    let transfer = Tx::Transfer {
        from: key.public().address(),
        to: Keypair::from_seed([9; 32]).public().address(),
        amount: Amount::U128(5),
    };
    let fee = FeeOffer { gas_limit: 21_000, max_price: u256::ONE, tip: u256::ZERO };
    let signed = SignedTx::sign_eip712(transfer.clone(), 0, fee, &key, &ours).unwrap();

    assert_eq!(signed.verify_on(&ours), Ok(()));
    assert_eq!(signed.verify_on(&theirs), Err(VerifyError::InvalidSignature), "bound to its chain");
    assert_eq!(signed.verify(), Err(VerifyError::InvalidSignature), "and never checked without one");

    let mut changed = signed.clone();
    changed.nonce = 1;

    assert_eq!(changed.verify_on(&ours), Err(VerifyError::InvalidSignature));

    // The same signature as a native one does not hold, nor a native one as typed.
    let Signature::Secp256k1Eip712(bytes) = signed.signature else { unreachable!() };
    let native = SignedTx { signature: Signature::Secp256k1(bytes), ..signed.clone() };

    assert_eq!(native.verify_on(&ours), Err(VerifyError::InvalidSignature));

    let own = SignedTx::sign(transfer.clone(), 0, fee, &key);

    assert_eq!(own.verify_on(&ours), Ok(()), "chainv4's own signatures check as ever");

    // Only Ethereum's keys sign typed data, and a transfer from another's address is refused.
    #[cfg(feature = "ed25519")]
    assert!(SignedTx::sign_eip712(transfer, 0, fee, &Keypair::generate(KeyType::Ed25519).unwrap(), &ours).is_none());

    let theirs_from = Tx::Transfer { from: Keypair::from_seed([9; 32]).public().address(), to: key.public().address(), amount: Amount::U128(5) };

    assert!(SignedTx::sign_eip712(theirs_from, 0, fee, &key, &ours).unwrap().verify_on(&ours).is_err());
}
