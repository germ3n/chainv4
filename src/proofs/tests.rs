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
    odd[10 * 32 + 31] = 2;
    assert_eq!(ProvenRange::from_public_values(HashType::default_chain(), &odd), None, "no third mode");

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
    let settings = ProofSettings { kind: ProofKind::Sp1Groth16, program: [3; 32], reward_per_block: u256::new(1_000), gateway: GatewayMode::ProofOrFederation };
    let json = crate::params::json::proof_settings_json(&settings);

    assert_eq!(json["kind"], "sp1-groth16");
    assert_eq!(json["gateway"], "proof-or-federation");
    assert_eq!(crate::params::json::proof_settings_from_json(&json), Ok(settings));
    assert_eq!(settings.problem(), None);
    assert!(ProofSettings { program: [0; 32], ..settings }.problem().is_some());
    assert!(crate::params::json::proof_settings_from_json(&serde_json::json!({ "kind": "groth17", "program": "0x", "reward_per_block": "0", "gateway": "both" })).is_err());
}
