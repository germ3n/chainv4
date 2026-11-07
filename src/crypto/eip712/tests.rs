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
    let transfer = message(&Tx::Transfer { from, to, amount: Amount::U128(5) }, &[], 3, &fee).unwrap();

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

        assert_eq!(names(&message(&token, &[], 3, &fee).unwrap()), ["token", "to", "amount", "nonce", "gasLimit", "maxPrice", "tip"]);
        assert_eq!(message(&token, &[], 3, &fee).unwrap().name, "TokenTransfer");
    }

    // Anything else, and a transfer whose amount could be narrower, go as their bytes: no two
    // encodings share a typed form.
    let burn = Tx::Burn { from, amount: Amount::U128(1) };
    let wide = Tx::Transfer { from, to, amount: Amount::U256(u256::new(5)) };

    assert_eq!(names(&message(&burn, &[], 0, &fee).unwrap())[..2], ["kind", "data"]);
    assert_eq!(message(&burn, &[], 0, &fee).unwrap().fields[0].2, Value::String("Burn".to_string()));
    assert_eq!(message(&wide, &[], 3, &fee).unwrap().name, "Transaction");
    assert_ne!(message(&wide, &[], 3, &fee).unwrap().hash(), transfer.hash());
    assert_eq!(message(&Tx::Transfer { from, to, amount: Amount::U256(u256::MAX) }, &[], 3, &fee).unwrap().name, "Transfer", "too wide for 128 bits");
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

    assert_ne!(transaction_digest(&tx, &[], 0, &FeeOffer::NONE, &ours), transaction_digest(&tx, &[], 0, &FeeOffer::NONE, &theirs));
    assert_eq!(domain(&ours).fields[2].2, Value::Bytes32([1; 32]), "the genesis id is the salt");
}

#[test]
fn a_transaction_signed_as_typed_data_holds_on_its_chain_and_nowhere_else() {
    use crate::crypto::keypair::Signature;
    use crate::crypto::verifiable::VerifyError;
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
    let signed = SignedTx::sign_eip712(transfer.clone(), Vec::new(), 0, fee, &key, &ours).unwrap();

    assert_eq!(signed.verify_on(&ours), Ok(()));
    assert_eq!(signed.verify_on(&theirs), Err(VerifyError::InvalidSignature), "bound to its chain");

    let mut changed = signed.clone();
    changed.nonce = 1;

    assert_eq!(changed.verify_on(&ours), Err(VerifyError::InvalidSignature));

    // The same signature as a native one does not hold, nor a native one as typed.
    let Signature::Secp256k1Eip712(bytes) = signed.signature else { unreachable!() };
    let native = SignedTx { signature: Signature::Secp256k1(bytes), ..signed.clone() };

    assert_eq!(native.verify_on(&ours), Err(VerifyError::InvalidSignature));

    let own = SignedTx::sign(transfer.clone(), 0, fee, &key, &ours);

    assert_eq!(own.verify_on(&ours), Ok(()), "chainv4's own signatures check as ever");
    assert_eq!(own.verify_on(&theirs), Err(VerifyError::InvalidSignature), "and are bound to their chain alike");

    // Only Ethereum's keys sign typed data, and a transfer from another's address is refused.
    #[cfg(feature = "ed25519")]
    assert!(SignedTx::sign_eip712(transfer, Vec::new(), 0, fee, &Keypair::generate(KeyType::Ed25519).unwrap(), &ours).is_none());

    let theirs_from = Tx::Transfer { from: Keypair::from_seed([9; 32]).public().address(), to: key.public().address(), amount: Amount::U128(5) };

    assert!(SignedTx::sign_eip712(theirs_from, Vec::new(), 0, fee, &key, &ours).unwrap().verify_on(&ours).is_err());
}

#[test]
fn a_batch_holds_under_a_typed_signature_step_by_step_but_a_multisigs_never() {
    use crate::crypto::keypair::Signature;
    use crate::crypto::verifiable::VerifyError;
    use crate::structs::tx::SignedTx;

    let key = Keypair::generate(KeyType::Secp256k1).unwrap();
    let me = key.public().address();
    let genesis = Hash::zero(HashType::default_chain());
    let batch = Tx::Batch { steps: vec![Tx::Transfer { from: me, to: Address::Ed25519([1; 32]), amount: Amount::U128(1) }, Tx::Burn { from: me, amount: Amount::U128(2) }] };
    let signed = SignedTx::sign_eip712(batch.clone(), Vec::new(), 0, FeeOffer::NONE, &key, &genesis).unwrap();

    assert_eq!(signed.verify_on(&genesis), Ok(()));

    let typed = message(&batch, &[], 0, &FeeOffer::NONE).unwrap();
    let Value::Array(steps) = &typed.fields[0].2 else { panic!("steps as an array") };
    let field = |step: &Value, at: usize| match step {
        Value::Struct(step) => step.fields[at].2.clone(),
        _ => panic!("a step is a struct"),
    };

    assert_eq!(typed.encode_type(), "Batch(Step[] steps,uint64 nonce,uint64 gasLimit,uint256 maxPrice,uint256 tip)Argument(string type,string value)Step(string kind,string target,string function,uint256 amount,Argument[] arguments,bytes data)");
    assert_eq!(field(&steps[0], 0), Value::String("Transfer".to_string()));
    assert_eq!(field(&steps[0], 3), Value::Uint(u256::ONE), "a transfer written out");
    assert_eq!(field(&steps[1], 0), Value::String("Burn".to_string()));
    assert_eq!(field(&steps[1], 1), Value::String(String::new()), "anything else with no target, as its bytes");
    assert_eq!(field(&steps[1], 5), Value::Bytes(Tx::Burn { from: me, amount: Amount::U128(2) }.hashable_bytes_with_tag()));

    // No name stands beside it, as it calls nothing; one there is refused.
    let named = SignedTx { functions: vec!["transfer".to_string()], ..signed.clone() };

    assert_eq!(named.verify_on(&genesis), Err(VerifyError::FunctionNames));

    // A multisig's, carried or approved, has no form: its steps sit inside the multisig's own.
    let secret = match key.private() {
        crate::crypto::keypair::PrivateKey::Secp256k1(secret) => secret,
        #[allow(unreachable_patterns)]
        _ => unreachable!(),
    };
    let forged = |tx: Tx| {
        let digest = transaction_digest(&tx, &[], 0, &FeeOffer::NONE, &genesis).unwrap();

        SignedTx { tx, nonce: 0, fee: FeeOffer::NONE, signer: key.public(), signature: Signature::Secp256k1Eip712(secp256k1::sign_hash(secret, &digest)), functions: Vec::new() }
    };
    let account = Address::Multisig([6; 32]);
    let its = Tx::Batch { steps: vec![Tx::Burn { from: account, amount: Amount::U128(1) }] };
    let carried = Tx::Multisig { account, tx: Box::new(its.clone()), signatures: Vec::new() };

    assert_eq!(SignedTx::sign_eip712(carried.clone(), Vec::new(), 0, FeeOffer::NONE, &key, &genesis), None, "nothing to sign it with");
    assert_eq!(forged(carried).verify_on(&genesis), Err(VerifyError::BatchNotTyped));
    assert_eq!(forged(Tx::ApproveMultisig { account, nonce: 0, fee: FeeOffer::NONE, tx: Box::new(its) }).verify_on(&genesis), Err(VerifyError::BatchNotTyped));

    // Names beside chainv4's own signature are refused too: they would change the id alone.
    let own = SignedTx::sign(batch, 0, FeeOffer::NONE, &key, &genesis);

    assert_eq!(own.verify_on(&genesis), Ok(()));
    assert_eq!(SignedTx { functions: vec!["x".to_string()], ..own }.verify_on(&genesis), Err(VerifyError::FunctionNames));
}

#[cfg(feature = "xxh3")]
#[test]
fn a_call_reads_by_its_functions_name_and_each_argument_written_out() {
    use crate::crypto::hasher::Hasher;
    use crate::crypto::verifiable::VerifyError;
    use crate::structs::tx::{SignedTx, TxCallArgumentType};

    let key = Keypair::generate(KeyType::Secp256k1).unwrap();
    let genesis = Hash::zero(HashType::default_chain());
    let pool = Address::Contract([0x22; 32]);
    let call = |function: &str, amount: Amount, data: Vec<TxCallArgumentType>| Tx::ContractCall { to: pool, func: Hasher::digest(HashType::Xxh3_64, function.as_bytes()), amount, data };
    let args = vec![
        TxCallArgumentType::Address(Address::Ed25519([3; 32])),
        TxCallArgumentType::Amount(Amount::U128(7)),
        TxCallArgumentType::Amount(Amount::U256(u256::new(7))),
        TxCallArgumentType::Number(-0.0),
        TxCallArgumentType::Bytes(vec![0xab]),
        TxCallArgumentType::I256(crate::structs::u256::i256::from(-4)),
        TxCallArgumentType::Nil,
    ];
    let swap = call("swap_exact", Amount::U128(9), args.clone());
    let typed = message(&swap, &["swap_exact".to_string()], 1, &FeeOffer::NONE).unwrap();
    let Value::Array(written) = &typed.fields[3].2 else { panic!("arguments as an array") };
    let pairs: Vec<(String, String)> = written
        .iter()
        .map(|argument| match argument {
            Value::Struct(Typed { fields, .. }) => match (&fields[0].2, &fields[1].2) {
                (Value::String(kind), Value::String(value)) => (kind.clone(), value.clone()),
                _ => panic!("type and value as strings"),
            },
            _ => panic!("an argument is a struct"),
        })
        .collect();

    assert_eq!(typed.name, "Call");
    assert_eq!(typed.encode_type(), "Call(string contract,string function,uint256 amount,Argument[] arguments,uint64 nonce,uint64 gasLimit,uint256 maxPrice,uint256 tip)Argument(string type,string value)");
    assert_eq!(typed.fields[1].2, Value::String("swap_exact".to_string()));
    assert_eq!(
        pairs,
        [
            ("address", Address::Ed25519([3; 32]).to_string()),
            ("amount", "7".to_string()),
            ("amount256", "7".to_string()),
            ("number", "-0".to_string()),
            ("bytes", "0xab".to_string()),
            ("i256", "-4".to_string()),
            ("nil", String::new()),
        ]
        .map(|(kind, value)| (kind.to_string(), value))
    );

    // The name must be the call's: none, another, or one too many is no form at all.
    assert_eq!(message(&swap, &[], 1, &FeeOffer::NONE), None);
    assert_eq!(message(&swap, &["swap".to_string()], 1, &FeeOffer::NONE), None);
    assert_eq!(message(&swap, &["swap_exact".to_string(), "swap_exact".to_string()], 1, &FeeOffer::NONE), None);
    assert_eq!(name_calls(&swap, &["mint".to_string(), "swap_exact".to_string()]), Some(vec!["swap_exact".to_string()]));
    assert_eq!(name_calls(&swap, &["mint".to_string()]), None);

    // Signed, it holds; with its name changed, or dropped, it does not.
    let signed = SignedTx::sign_eip712(swap.clone(), vec!["swap_exact".to_string()], 0, FeeOffer::NONE, &key, &genesis).unwrap();

    assert_eq!(signed.verify_on(&genesis), Ok(()));
    assert_eq!(SignedTx { functions: Vec::new(), ..signed.clone() }.verify_on(&genesis), Err(VerifyError::FunctionNames));
    assert_eq!(SignedTx::sign_eip712(swap, Vec::new(), 0, FeeOffer::NONE, &key, &genesis), None, "a call needs its name");

    // A call sending an amount wider than it needs goes as its bytes, needing no name.
    let wide = call("swap_exact", Amount::U256(u256::new(9)), args);

    assert_eq!(message(&wide, &[], 0, &FeeOffer::NONE).unwrap().name, "Transaction");

    // In a batch: one name for each call, in order, a token transfer's too.
    let me = key.public().address();
    let transfer = Tx::ContractCall {
        to: pool,
        func: Hasher::digest(HashType::Xxh3_64, b"transfer"),
        amount: Amount::U128(0),
        data: vec![TxCallArgumentType::Address(me), TxCallArgumentType::U256(u256::ONE)],
    };
    let batch = Tx::Batch { steps: vec![call("approve", Amount::U128(0), Vec::new()), Tx::EvmCall { to: Address::Evm([5; 20]), value: Amount::U128(3), input: vec![1, 2] }, transfer] };
    let names = name_calls(&batch, &["transfer".to_string(), "approve".to_string()]).unwrap();

    assert_eq!(names, ["approve", "transfer"]);
    assert_eq!(calls_named(&batch), 2);

    let signed = SignedTx::sign_eip712(batch, names, 0, FeeOffer::NONE, &key, &genesis).unwrap();

    assert_eq!(signed.verify_on(&genesis), Ok(()));
    assert_eq!(SignedTx { functions: vec!["transfer".to_string(), "approve".to_string()], ..signed }.verify_on(&genesis), Err(VerifyError::FunctionNames), "in order");
}

#[cfg(all(feature = "abi", feature = "xxh3"))]
#[test]
fn calls_and_batches_hash_as_metamask_hashes_their_json() {
    use crate::crypto::hasher::Hasher;
    use crate::structs::tx::TxCallArgumentType;

    let genesis = Hash::from_bytes(HashType::default_chain(), &[4; 32]).unwrap();
    let fee = FeeOffer { gas_limit: 90_000, max_price: u256::new(3), tip: u256::ONE };
    let call = |function: &str, data| Tx::ContractCall { to: Address::Contract([8; 32]), func: Hasher::digest(HashType::Xxh3_64, function.as_bytes()), amount: Amount::U128(4), data };
    let batch = Tx::Batch {
        steps: vec![
            call("mint", vec![TxCallArgumentType::U64(3), TxCallArgumentType::String("hi".to_string())]),
            call("burn", Vec::new()),
            Tx::Burn { from: Address::Ed25519([1; 32]), amount: Amount::U128(1) },
        ],
    };
    let cases = [(call("mint", vec![TxCallArgumentType::Bool(true)]), vec!["mint".to_string()]), (call("mint", Vec::new()), vec!["mint".to_string()]), (batch, vec!["mint".to_string(), "burn".to_string()])];

    for (tx, functions) in cases {
        let message = message(&tx, &functions, 2, &fee).unwrap();
        let json = json(&domain(&genesis), &message);
        let read = typed::TypedData::parse(&json).unwrap();

        assert_eq!(read.digest().unwrap(), transaction_digest(&tx, &functions, 2, &fee, &genesis).unwrap(), "{}", message.name);
    }
}
