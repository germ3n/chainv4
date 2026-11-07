use {
    super::*,
    crate::crypto::keypair::{Keypair, PrivateKey},
    crate::structs::address::Address,
    crate::structs::hash::HashType,
};

fn attester(seed: u8) -> ([u8; 32], [u8; 20]) {
    let keypair = Keypair::from_private(PrivateKey::Secp256k1([seed; 32]));
    let Address::Secp256k1(address) = keypair.public().address() else { unreachable!("a secp256k1 key") };

    ([seed; 32], address)
}

fn link() -> AttestedLink {
    let hash = |byte: u8| Hash::from_bytes(HashType::default_chain(), &[byte; 32]).unwrap();

    AttestedLink {
        chain_id: 77,
        source: Checkpoint { epoch: 3, block: hash(1) },
        target: Checkpoint { epoch: 4, block: hash(2) },
        height: 32,
        state_root: hash(3),
        set: [4; 32],
        next_set: [5; 32],
    }
}

#[test]
fn an_attesting_set_lists_attesters_by_address_each_once_and_counts_every_validator_in_its_whole() {
    let (_, first) = attester(1);
    let (_, second) = attester(2);
    let set = AttestingSet::of([(Some(second), u256::new(5)), (None, u256::new(7)), (Some(first), u256::new(3)), (Some(second), u256::new(1))]);
    let mut expected = vec![(first, u256::new(3)), (second, u256::new(6))];
    expected.sort();

    assert_eq!(set, AttestingSet { total: u256::new(16), attesters: expected });
    assert_eq!(set.weight(&second), Some(u256::new(6)));
    assert!(set.enough(u256::new(11)) && !set.enough(u256::new(10)), "two thirds of 16, as 3w >= 2t");

    // Its hash is abi.encode(total, attesters, weights)'s, laid out by hand: the same set, in
    // any order, hashes alike; another weight does not.
    let shuffled = AttestingSet::of([(Some(first), u256::new(3)), (Some(second), u256::new(6)), (None, u256::new(7))]);
    assert_eq!(shuffled.hash(), set.hash());
    assert_ne!(AttestingSet::of([(Some(first), u256::new(4)), (Some(second), u256::new(6)), (None, u256::new(7))]).hash(), set.hash());

    #[cfg(feature = "abi")]
    {
        use crate::abi::{AbiType, AbiValue, encode_values};

        let encoded = encode_values(
            &[AbiType::Uint(256), AbiType::Array(Box::new(AbiType::Address)), AbiType::Array(Box::new(AbiType::Uint(256)))],
            &[
                AbiValue::Uint(set.total),
                AbiValue::Array(set.attesters.iter().map(|(address, _)| AbiValue::Address(*address)).collect()),
                AbiValue::Array(set.attesters.iter().map(|(_, weight)| AbiValue::Uint(*weight)).collect()),
            ],
        )
        .unwrap();
        assert_eq!(crate::crypto::secp256k1::keccak(&encoded), set.hash(), "as the ABI encoder writes it");
        assert_eq!(AttestingSet::default().hash(), crate::crypto::secp256k1::keccak(&encode_values(&[AbiType::Uint(256), AbiType::Array(Box::new(AbiType::Address)), AbiType::Array(Box::new(AbiType::Uint(256)))], &[AbiValue::Uint(u256::ZERO), AbiValue::Array(Vec::new()), AbiValue::Array(Vec::new())]).unwrap()));
    }
}

#[test]
fn an_attestation_recovers_to_its_attester_over_this_link_alone_and_in_one_shape() {
    let (secret, address) = attester(9);
    let link = link();
    let attestation = link.sign(&secret);

    assert!(matches!(attestation[64], 27 | 28));
    assert_eq!(link.attester(&attestation), Some(address));

    // Any field changed is another digest, whose attester is someone else or no one.
    for other in [
        AttestedLink { chain_id: 78, ..link },
        AttestedLink { height: 33, ..link },
        AttestedLink { set: [6; 32], ..link },
        AttestedLink { next_set: [6; 32], ..link },
        AttestedLink { source: Checkpoint { epoch: 2, ..link.source }, ..link },
    ] {
        assert_ne!(other.digest(), link.digest());
        assert_ne!(other.attester(&attestation), Some(address));
    }

    // v as 0 or 1, or s made high, is not the one shape the contract takes.
    let mut zero_based = attestation;
    zero_based[64] -= 27;
    assert_eq!(link.attester(&zero_based), None);

    let order = u256::from_str_radix("115792089237316195423570985008687907852837564279074904382605163141518161494337", 10).unwrap();
    let s = u256::from_be_bytes(attestation[32..64].try_into().unwrap());
    let mut high = attestation;
    high[32..64].copy_from_slice(&(order - s).to_be_bytes());
    high[64] = if attestation[64] == 27 { 28 } else { 27 };
    assert_eq!(link.attester(&high), None);
}

#[test]
fn a_hash_of_another_length_is_its_keccak_as_a_word() {
    let wide = Hash::from_bytes(HashType::default_chain(), &[7; 32]).unwrap();

    assert_eq!(word(&wide), [7; 32]);
}
