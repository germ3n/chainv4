use {
    super::*,
    crate::crypto::keypair::{Keypair, PrivateKey, PublicKey, Signature},
};

fn call(signature: &str, values: &[AbiValue]) -> Vec<u8> {
    let (_, types) = crate::abi::parse_signature(&format!("({})", signature.split_once('(').unwrap().1.trim_end_matches(')'))).unwrap();

    [keccak256(signature.as_bytes())[..4].to_vec(), encode_values(&types, values).unwrap()].concat()
}

fn answered(input: &[u8]) -> PrecompileOutput {
    CryptoAt { hash: HashType::default_chain() }.answer(input, 1_000_000, 0)
}

fn output(input: &[u8]) -> Vec<u8> {
    let answer = answered(input);

    assert!(answer.is_success(), "{answer:?}");

    answer.bytes.to_vec()
}

#[cfg(feature = "ed25519")]
#[test]
fn solidity_checks_chainv4_s_signatures() {
    let keypair = Keypair::from_private(PrivateKey::Ed25519([5; 32]));
    let (PublicKey::Ed25519(key), Signature::Ed25519(signature)) = (keypair.public(), keypair.sign(b"hello")) else { unreachable!() };
    let verify = |message: &[u8]| {
        output(&call("verify(string,bytes,bytes,bytes)", &[
            AbiValue::String("ed25519".into()),
            AbiValue::Bytes(key.to_vec()),
            AbiValue::Bytes(message.to_vec()),
            AbiValue::Bytes(signature.to_vec()),
        ]))
    };

    assert_eq!(verify(b"hello"), word(true));
    assert_eq!(verify(b"hullo"), word(false));

    // A scheme the chain lacks reverts, as bad input does.
    let unknown = call("verify(string,bytes,bytes,bytes)", &[AbiValue::String("rsa".into()), AbiValue::Bytes(key.to_vec()), AbiValue::Bytes(Vec::new()), AbiValue::Bytes(Vec::new())]);

    assert!(answered(&unknown).is_revert());

    // The account the key controls, as its 33 bytes.
    let address = output(&call("keyAddress(string,bytes)", &[AbiValue::String("ed25519".into()), AbiValue::Bytes(key.to_vec())]));

    assert_eq!(decode_values(&[AbiType::Bytes], &address).unwrap(), [AbiValue::Bytes(keypair.public().address().to_abi().to_vec())]);
}

#[test]
fn hashes_and_proofs_answer_as_the_native_engines_do() {
    #[cfg(feature = "blake3")]
    assert_eq!(output(&call("blake3(bytes)", &[AbiValue::Bytes(b"abc".to_vec())])), Hasher::digest(HashType::Blake3_256, b"abc").to_bytes());
    #[cfg(feature = "xxh3")]
    assert_eq!(output(&call("xxh3(bytes)", &[AbiValue::Bytes(b"abc".to_vec())]))[..8], Hasher::digest(HashType::Xxh3_64, b"abc").to_bytes()[..]);
    #[cfg(feature = "bitcoin")]
    assert_eq!(output(&call("hash160(bytes)", &[AbiValue::Bytes(b"abc".to_vec())]))[..20], crate::crypto::bitcoin::hash160(b"abc"));

    let hash = HashType::default_chain();
    let leaves: Vec<_> = [b"one".as_slice(), b"two"].iter().map(|data| crate::crypto::merkle::leaf(hash, data)).collect();
    let root = crate::crypto::merkle::root(hash, &leaves).to_bytes();
    let path: Vec<u8> = crate::crypto::merkle::audit_path(hash, &leaves, 1).iter().flat_map(|sibling| sibling.to_bytes()).collect();
    let merkle = |data: &[u8]| {
        output(&call("checkMerkle(bytes32,bytes,uint256,uint256,bytes)", &[
            AbiValue::FixedBytes(root.clone()),
            AbiValue::Bytes(data.to_vec()),
            AbiValue::Uint(1u64.into()),
            AbiValue::Uint(2u64.into()),
            AbiValue::Bytes(path.clone()),
        ]))
    };

    assert_eq!(merkle(b"two"), word(true));
    assert_eq!(merkle(b"one"), word(false));
}

#[test]
fn what_it_does_not_know_reverts_and_too_little_gas_halts() {
    assert!(answered(&[1, 2, 3, 4]).is_revert(), "no such function");
    assert!(answered(&keccak256(b"sha256d(bytes)")[..4]).is_revert(), "no arguments");

    let costly = call("verify(string,bytes,bytes,bytes)", &[AbiValue::String("ed25519".into()), AbiValue::Bytes(vec![0; 32]), AbiValue::Bytes(Vec::new()), AbiValue::Bytes(vec![0; 64])]);

    assert!(CryptoAt { hash: HashType::default_chain() }.answer(&costly, 100, 0).is_halt());
}
