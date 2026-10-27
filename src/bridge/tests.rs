use super::*;

fn transfer() -> Transfer {
    Transfer {
        from: vec![1; 33],
        to: vec![2; 20],
        source: 0x1234_5678,
        target: 11155111,
        nonce: 7,
        kind: AssetKind::Token,
        origin: 0x1234_5678,
        asset: vec![3; 33],
        id: u256::ZERO,
        amount: u256::from(5_000u64),
        recipient: vec![4; 20],
        name: "Example Token".to_string(),
        symbol: "EXT".to_string(),
        decimals: 18,
        uri: String::new(),
    }
}

#[test]
fn every_field_changes_the_digest() {
    let base = transfer().digest();
    let changed = [
        Transfer { from: vec![9; 33], ..transfer() },
        Transfer { to: vec![9; 20], ..transfer() },
        Transfer { source: 1, ..transfer() },
        Transfer { target: 1, ..transfer() },
        Transfer { nonce: 8, ..transfer() },
        Transfer { kind: AssetKind::Coin, ..transfer() },
        Transfer { origin: 1, ..transfer() },
        Transfer { asset: Vec::new(), ..transfer() },
        Transfer { id: u256::from(1u64), ..transfer() },
        Transfer { amount: u256::from(5_001u64), ..transfer() },
        Transfer { recipient: vec![5; 20], ..transfer() },
        Transfer { name: "Other".to_string(), ..transfer() },
        Transfer { symbol: "OTH".to_string(), ..transfer() },
        Transfer { decimals: 6, ..transfer() },
        Transfer { uri: "ipfs://x".to_string(), ..transfer() },
    ];

    for (index, other) in changed.iter().enumerate() {
        assert_ne!(other.digest(), base, "field {index}");
    }
}

#[test]
fn a_signature_recovers_its_signer_as_ecrecover_would() {
    let secret = [7; 32];
    let signature = transfer().sign(&secret);

    assert!(matches!(signature[64], 27 | 28));
    let rs: [u8; 64] = signature[..64].try_into().unwrap();

    assert_eq!(crate::crypto::secp256k1::recover_strict(&transfer().digest(), &rs, signature[64] - 27), crate::crypto::secp256k1::public_key(&secret));
    assert_eq!(transfer().attest_args(&signature).len(), 14);
    assert_eq!(AssetKind::from_number(3), Some(AssetKind::Multi));
    assert_eq!(AssetKind::from_number(4), None);
}
