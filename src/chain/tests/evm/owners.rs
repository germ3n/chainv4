//! MultiSig.sol's owners of every kind, through whole blocks: Ed25519 owners checked by chainv4's
//! crypto precompile at home and by Ed25519.sol everywhere else, Ed25519.sol and its SHA-512 held to
//! RFC 8032 and to Rust's, and one MultiSig owning another through ERC-1271.

use {
    super::*,
    crate::abi::multisig::{EvmBundle, multisig_init_code, nested_signature, sign_execute},
    crate::abi::{AbiType, AbiValue, encode_values},
    crate::crypto::keypair::{PrivateKey, Signature},
    crate::registry::multisig::{EvmKind, EvmOwner, EvmStart, ed25519_owner},
};

/// Deploys init code as the authority; its address and what deploying it took.
fn create(chain: &mut Chain, init: Vec<u8>) -> Address {
    let deployer = authority();
    let at = evm::create_address(&deployer.public().address(), nonce(chain, &deployer));
    let deploy = sign(chain, &deployer, Tx::Deploy { code: Code::Evm(init) });
    assert_eq!(advance(chain, vec![deploy])[0].result, Ok(Effect::Deployed(at)));

    at
}

/// A call's receipt, from the authority.
fn send(chain: &mut Chain, to: Address, input: Vec<u8>) -> Receipt {
    let sent = call(chain, &authority(), to, 0, input);

    advance(chain, vec![sent]).remove(0)
}

fn bytes20(address: Address) -> [u8; 20] {
    match address {
        Address::Evm(bytes) => bytes,
        other => panic!("{other:?}"),
    }
}

fn ed25519(seed: u8) -> (Keypair, [u8; 32]) {
    let keypair = Keypair::from_private(PrivateKey::Ed25519([seed; 32]));
    let crate::crypto::keypair::PublicKey::Ed25519(key) = keypair.public() else { unreachable!() };

    (keypair, key)
}

fn ed25519_sign(keypair: &Keypair, message: &[u8]) -> [u8; 64] {
    match keypair.sign(message) {
        Signature::Ed25519(signature) => signature,
        _ => unreachable!(),
    }
}

/// Ed25519Check's verify, through a block: its answer, and the gas it took.
fn solidity_verify(chain: &mut Chain, check: Address, key: &[u8; 32], signature: &[u8; 64], message: &[u8]) -> (bool, u64) {
    let types = [AbiType::FixedBytes(32), AbiType::FixedBytes(32), AbiType::FixedBytes(32), AbiType::Bytes];
    let values = [AbiValue::FixedBytes(key.to_vec()), AbiValue::FixedBytes(signature[..32].to_vec()), AbiValue::FixedBytes(signature[32..].to_vec()), AbiValue::Bytes(message.to_vec())];
    let receipt = send(chain, check, calldata("verify(bytes32,bytes32,bytes32,bytes)", &[encode_values(&types, &values).unwrap()]));

    (returned(&receipt) == word(1), receipt.fuel_used)
}

#[test]
fn ed25519_in_solidity_holds_to_rfc_8032_and_to_rust() {
    let db = Arc::new(MemoryDb::default());
    let mut chain = open(&db);
    let check = create(&mut chain, crate::structs::hex::from_hex(&format!("0x{}", include_str!("../../../../contracts/evm/Ed25519Check.bin").trim())).unwrap());

    // SHA-512, any length, the block boundaries round 111 and 112 bytes included.
    for length in [0usize, 3, 111, 112, 127, 128, 129, 300] {
        let message: Vec<u8> = (0..length).map(|at| at as u8).collect();
        let types = [AbiType::Bytes];
        let receipt = send(&mut chain, check, calldata("sha512(bytes)", &[encode_values(&types, &[AbiValue::Bytes(message.clone())]).unwrap()]));
        let answer = returned(&receipt);
        let expected = <sha2::Sha512 as sha2::Digest>::digest(&message);

        assert_eq!(answer[64..128], expected[..], "{length} bytes");
    }

    // RFC 8032's first three test vectors: secret, public key, message, signature.
    let vectors = [
        ("9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60", "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a", "", "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b"),
        ("4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb", "3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c", "72", "92a009a9f0d4cab8720e820b5f642540a2b27b5416503f8fb3762223ebdb69da085ac1e43e15996e458f3613d0f11d8c387b2eaeb4302aeeb00d291612bb0c00"),
        ("c5aa8df43f9f837bedb7442f31dcb7b166d38535076f094b85ce3a2e0b4458f7", "fc51cd8e6218a1a38da47ed00230f0580816ed13ba3303ac5deb911548908025", "af82", "6291d657deec24024827e69c3abe01a30ce548a284743a445e3680d7db5ac3ac18ff9b538d16f290ae67f760984dc6594a7c15e9716ed28dc027beceea1ec40a"),
    ];
    let hex = |text: &str| crate::structs::hex::from_hex(&format!("0x{text}")).unwrap();

    for (secret, public, message, signature) in vectors {
        let keypair = Keypair::from_private(PrivateKey::Ed25519(hex(secret).try_into().unwrap()));
        let key: [u8; 32] = hex(public).try_into().unwrap();
        let signature: [u8; 64] = hex(signature).try_into().unwrap();

        assert_eq!(keypair.public(), crate::crypto::keypair::PublicKey::Ed25519(key), "the vector's key");
        assert_eq!(ed25519_sign(&keypair, &hex(message)), signature, "Rust signs as the RFC");
        assert!(solidity_verify(&mut chain, check, &key, &signature, &hex(message)).0, "Solidity checks the RFC's");
    }

    // A digest as MultiSig.sol checks one, signed by Rust, under several keys; and what must not hold.
    for seed in [1u8, 7, 200] {
        let (keypair, key) = ed25519(seed);
        let digest = crate::crypto::secp256k1::keccak(&[seed]);
        let signature = ed25519_sign(&keypair, &digest);
        let (holds, gas) = solidity_verify(&mut chain, check, &key, &signature, &digest);

        assert!(holds, "seed {seed}");
        // Fuel is gas on a chain without fees.
        assert!(gas > 100_000 && gas < 3_000_000, "{gas} gas: within any chain's block");
        println!("Ed25519 in Solidity: {gas} gas");

        let mut other = digest;
        other[0] ^= 1;
        assert!(!solidity_verify(&mut chain, check, &key, &signature, &other).0, "another message");
        let mut forged = signature;
        forged[10] ^= 1;
        assert!(!solidity_verify(&mut chain, check, &key, &forged, &digest).0, "another R");
        let mut big = signature;
        big[63] |= 0xf0;
        assert!(!solidity_verify(&mut chain, check, &key, &big, &digest).0, "S past the group's order");
        let (_, stranger) = ed25519(seed.wrapping_add(1));
        assert!(!solidity_verify(&mut chain, check, &stranger, &signature, &digest).0, "another key");
        assert!(!solidity_verify(&mut chain, check, &[0xff; 32], &signature, &digest).0, "no point at all");
    }
}

/// A MultiSig.sol whose owners are an EVM key and an Ed25519 key, 2 of 2, deployed with this home.
fn mixed(chain: &mut Chain, home: u64) -> (Address, EvmStart, Keypair) {
    let (keypair, key) = ed25519(9);
    let mut owners = vec![key_owner(&[1; 32]), EvmOwner { address: ed25519_owner(&key), kind: EvmKind::Ed25519(key) }];
    owners.sort();
    let start = EvmStart { owners, threshold: 2 };
    let contract = create(chain, multisig_init_code(&Address::Multisig([4; 32]), &start, home));
    let fund = call(chain, &authority(), contract, 1_000, Vec::new());
    advance(chain, vec![fund]);

    (contract, start, keypair)
}

#[test]
fn an_ed25519_owner_signs_by_the_precompile_at_home_and_by_solidity_elsewhere() {
    let db = Arc::new(MemoryDb::default());
    let mut chain = open(&db);
    let here = evm::chain_id(&chain.genesis());
    let payee = evm_owner(&[9; 32]);
    let mut gas = Vec::new();

    // One whose home is this chain, and one whose home is another: the second checks in Solidity.
    for home in [here, here + 1] {
        let (contract, start, keypair) = mixed(&mut chain, home);
        let mut bundle = EvmBundle::new(here, bytes20(contract), payee, u256::new(100), Vec::new(), u256::ZERO);
        bundle.sign(&[1; 32]);
        let PublicKey::Ed25519(key) = keypair.public() else { unreachable!() };

        // The EVM key alone is one of two; a wrong Ed25519 signature counts for nothing.
        assert!(send(&mut chain, contract, bundle.execute_input(&start.owners)).result.is_err());
        let mut wrong = bundle.clone();
        wrong.signatures.push(crate::abi::multisig::OwnerSignature { owner: ed25519_owner(&key), signature: vec![7; 64] });
        assert!(send(&mut chain, contract, wrong.execute_input(&start.owners)).result.is_err(), "home {home}");

        bundle.add_ed25519(&key, &ed25519_sign(&keypair, &bundle.digest())).unwrap();
        let receipt = send(&mut chain, contract, bundle.execute_input(&start.owners));
        assert!(receipt.result.is_ok(), "home {home}: {:?}", receipt.result);
        gas.push(receipt.fuel_used);
    }

    assert_eq!(balance(&chain, &Address::Secp256k1(payee)), u256::new(200));
    // The rest of execute, the EVM key's check and the payment among it, costs both the same.
    assert!(gas[0] * 3 < gas[1], "the precompile, {}, far cheaper than Solidity, {}", gas[0], gas[1]);
}

#[test]
fn one_multisig_owns_another_and_approves_for_it_through_erc_1271() {
    let db = Arc::new(MemoryDb::default());
    let mut chain = open(&db);
    let home = evm::chain_id(&chain.genesis());

    // The inner one, 2 of its 2 EVM keys; the outer one, 2 of the inner contract and one key.
    let mut inner_owners = vec![key_owner(&[1; 32]), key_owner(&[2; 32])];
    inner_owners.sort();
    let inner = create(&mut chain, multisig_init_code(&Address::Multisig([5; 32]), &EvmStart { owners: inner_owners, threshold: 2 }, home));
    let mut outer_owners = vec![EvmOwner { address: bytes20(inner), kind: EvmKind::Contract }, key_owner(&[3; 32])];
    outer_owners.sort();
    let start = EvmStart { owners: outer_owners, threshold: 2 };
    let outer = create(&mut chain, multisig_init_code(&Address::Multisig([6; 32]), &start, home));
    let fund = call(&chain, &authority(), outer, 1_000, Vec::new());
    advance(&mut chain, vec![fund]);
    let payee = evm_owner(&[9; 32]);
    let mut bundle = EvmBundle::new(home, bytes20(outer), payee, u256::new(250), Vec::new(), u256::ZERO);
    bundle.sign(&[3; 32]);
    let approve = |bundle: &mut EvmBundle, secret: &[u8; 32]| {
        let digest = bundle.nested_digest(&bytes20(inner));
        let signature = sign_execute(secret, &digest);
        bundle.add_nested(bytes20(inner), crate::abi::multisig::execute_signer(&digest, &signature).unwrap(), signature.to_vec());
    };

    // One of the inner contract's two approvals is not its signature.
    approve(&mut bundle, &[1; 32]);
    assert!(send(&mut chain, outer, bundle.execute_input(&start.owners)).result.is_err());

    // Its owners' signatures over the outer digest itself, not wrapped in its domain, are none.
    let mut bare: Vec<([u8; 20], Vec<u8>)> = [[1u8; 32], [2; 32]].iter().map(|secret| (evm_owner(secret), sign_execute(secret, &bundle.digest()).to_vec())).collect();
    bare.sort();
    let mut unwrapped = bundle.clone();
    unwrapped.add_contract(bytes20(inner), nested_signature(&bare));
    assert!(send(&mut chain, outer, unwrapped.execute_input(&start.owners)).result.is_err(), "no replay of execute signatures");

    approve(&mut bundle, &[2; 32]);
    let receipt = send(&mut chain, outer, bundle.execute_input(&start.owners));
    assert!(receipt.result.is_ok(), "{:?}", receipt.result);
    assert_eq!(balance(&chain, &Address::Secp256k1(payee)), u256::new(250));

    // It says it answers ERC-1271.
    let asked = send(&mut chain, inner, calldata("supportsInterface(bytes4)", &[[vec![0x16, 0x26, 0xba, 0x7e], vec![0; 28]].concat()]));
    assert_eq!(returned(&asked), word(1));
}
