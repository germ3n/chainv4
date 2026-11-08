use {
    super::*,
    crate::structs::hash::{Hash, HashType},
    crate::structs::u256::u256,
};

fn hash(byte: u8) -> Hash {
    Hash::from_bytes(HashType::default_chain(), &[byte; 32]).unwrap()
}

fn range() -> ProvenRange {
    ProvenRange {
        chain_id: 77,
        genesis: hash(1),
        start: RangeEnd { height: 39, block: hash(2), state_root: hash(3) },
        end: RangeEnd { height: 48, block: hash(4), state_root: hash(5) },
        withdrawals: [6; 32],
        program: [7; 32],
        gateway: GatewayMode::Both,
    }
}

#[test]
fn a_ranges_public_values_are_eleven_words_as_abi_encode_lays_them_and_read_back() {
    let range = range();
    let values = range.public_values();

    assert_eq!(values.len(), ProvenRange::WORDS * 32);
    assert_eq!(values[24..32], 77u64.to_be_bytes(), "the chain id, a uint256");
    assert_eq!(values[32..64], [1; 32], "the genesis, as its bytes");
    assert_eq!(values[10 * 32 + 31], 1, "both, as 1");
    assert_eq!(ProvenRange::from_public_values(HashType::default_chain(), &values), Some(range));
    assert_eq!(ProvenRange::from_public_values(HashType::default_chain(), &values[..320]), None, "short");

    let mut odd = values.clone();
    odd[10 * 32 + 31] = 3;
    assert_eq!(ProvenRange::from_public_values(HashType::default_chain(), &odd), None, "no fourth mode");

    #[cfg(feature = "abi")]
    {
        use crate::abi::{AbiType, AbiValue, encode_values};

        let word = |bytes: [u8; 32]| AbiValue::FixedBytes(bytes.to_vec());
        let number = |value: u64| AbiValue::Uint(u256::from(value));
        let types = [AbiType::Uint(256), AbiType::FixedBytes(32), AbiType::Uint(256), AbiType::FixedBytes(32), AbiType::FixedBytes(32), AbiType::Uint(256), AbiType::FixedBytes(32), AbiType::FixedBytes(32), AbiType::FixedBytes(32), AbiType::FixedBytes(32), AbiType::Uint(256)];
        let encoded = encode_values(&types, &[number(77), word([1; 32]), number(39), word([2; 32]), word([3; 32]), number(48), word([4; 32]), word([5; 32]), word([6; 32]), word([7; 32]), number(1)]).unwrap();
        assert_eq!(encoded, values, "as the ABI encoder writes them");
    }
}

#[cfg(feature = "secp256k1")]
#[test]
fn every_withdrawal_proves_against_its_ranges_root_and_nothing_else_does() {
    for count in [1usize, 2, 3, 5, 8] {
        let digests: Vec<[u8; 32]> = (0..count).map(|at| [at as u8 + 1; 32]).collect();
        let root = withdrawals_root(&digests);

        for (index, digest) in digests.iter().enumerate() {
            let proof = withdrawal_proof(&digests, index).unwrap();
            assert!(verify_withdrawal(&root, digest, &proof), "{index} of {count}");
            assert!(!verify_withdrawal(&root, &[0xee; 32], &proof), "another digest, {index} of {count}");
        }

        assert_eq!(withdrawal_proof(&digests, count), None);
    }

    assert_eq!(withdrawals_root(&[]), [0; 32], "none");
    assert!(!verify_withdrawal(&[0; 32], &[0; 32], &[]), "an empty tree holds nothing");

    // One alone is its own leaf, keccak256 of the digest, as Gateway.sol hashes it.
    assert_eq!(withdrawals_root(&[[9; 32]]), crate::crypto::secp256k1::keccak(&[9; 32]));
}

#[cfg(feature = "chain")]
#[test]
fn proof_settings_cross_json_and_a_program_of_zeros_names_none() {
    let settings = ProofSettings { kinds: vec![ProofKind::Sp1Groth16, ProofKind::Sp1Stark], program: [3; 32], reward_per_block: u256::new(1_000), gateway: GatewayMode::ProofOrFederation };
    let json = crate::params::json::proof_settings_json(&settings);

    assert_eq!(json["kinds"], serde_json::json!(["sp1-groth16", "sp1-stark"]));
    assert_eq!(json["gateway"], "proof-or-federation");
    assert_eq!(crate::params::json::proof_settings_from_json(&json).as_ref(), Ok(&settings));
    assert_eq!(settings.problem(), None);
    assert!(settings.takes(ProofKind::Sp1Stark) && !settings.takes(ProofKind::Sp1Plonk));
    assert!(ProofSettings { program: [0; 32], ..settings.clone() }.problem().is_some());
    assert!(ProofSettings { kinds: vec![], ..settings.clone() }.problem().is_some(), "no kind");
    assert!(ProofSettings { kinds: vec![ProofKind::Sp1Stark, ProofKind::Sp1Groth16], ..settings.clone() }.problem().is_some(), "out of order");
    assert!(ProofSettings { kinds: vec![ProofKind::Sp1Plonk, ProofKind::Sp1Plonk], ..settings.clone() }.problem().is_some(), "twice");

    // JSON takes them in any order, and writes them in ALL's.
    let mut reordered = json.clone();
    reordered["kinds"] = serde_json::json!(["sp1-stark", "sp1-groth16"]);
    assert_eq!(crate::params::json::proof_settings_from_json(&reordered), Ok(settings));
    let refused = |kinds: serde_json::Value| crate::params::json::proof_settings_from_json(&serde_json::json!({ "kinds": kinds, "program": format!("0x{}", "03".repeat(32)), "reward_per_block": "0", "gateway": "both" })).is_err();
    assert!(refused(serde_json::json!(["groth17"])));
    assert!(refused(serde_json::json!([])));
    assert!(refused(serde_json::json!(["sp1-plonk", "sp1-plonk"])));
    assert!(refused(serde_json::json!("sp1-plonk")), "a list, not one name");
}

#[test]
fn every_gateway_mode_crosses_the_public_values_and_its_name() {
    for mode in GatewayMode::ALL {
        let proven = ProvenRange { gateway: mode, ..range() };
        assert_eq!(ProvenRange::from_public_values(HashType::default_chain(), &proven.public_values()), Some(proven));
        assert_eq!(GatewayMode::parse(mode.name()), Some(mode));
    }
    assert_eq!(GatewayMode::parse("federation-only"), Some(GatewayMode::FederationOnly));
}

#[test]
fn every_kind_has_a_pinned_tag_a_name_and_a_size() {
    for (kind, tag) in ProofKind::ALL.into_iter().zip([0u8, 1, 2]) {
        assert_eq!(wincode::serialize(&kind).unwrap(), vec![tag], "{}", kind.name());
        assert_eq!(ProofKind::parse(kind.name()), Some(kind));
    }
    assert!(ProofKind::Sp1Groth16.on_evm() && ProofKind::Sp1Plonk.on_evm() && !ProofKind::Sp1Stark.on_evm());
    assert!(ProofKind::Sp1Stark.max_bytes() > ProofKind::Sp1Groth16.max_bytes());
}

#[test]
fn a_stark_key_is_the_program_id_unpacked() {
    // Eight words packed 31 bits apart, the first highest, as SP1's koalabears_to_bn254 packs them.
    let words: [u32; 8] = [1, 2, 3, 0x7f00_0000, 5, 6, 7, 8];
    let mut packed = u256::ZERO;
    for word in words {
        packed = (packed << 31u32) | u256::from(word);
    }
    let key = crate::proofs::sp1_stark_key(&packed.to_be_bytes()).unwrap();

    assert_eq!(key, words.iter().flat_map(|word| word.to_le_bytes()).collect::<Vec<_>>());
    // A word at the prime or over, or bits past the eighth word, pack no key.
    assert_eq!(crate::proofs::sp1_stark_key(&(u256::from(0x7f00_0001u32) << 31u32).to_be_bytes()), None);
    assert_eq!(crate::proofs::sp1_stark_key(&[0xff; 32]), None);
}
