use {
    super::*,
    crate::structs::hex::from_hex,
    bls12_381::Scalar,
    ethlight::testing,
};

/// The consensus spec tests' first private key, as a scalar, and its public key.
fn spec_key() -> (Scalar, Vec<u8>) {
    let secret: [u8; 32] = from_hex("0x263dbd792f5b1be47ed85f8938c0f29586af0d3ac7b977f21c278fe1462040e3").unwrap().into_iter().rev().collect::<Vec<_>>().try_into().unwrap();

    (Scalar::from_bytes(&secret).unwrap(), from_hex("0xa491d1b0ecd9bb917989f0e74f0dea0422eac4a873e5e2644f368dffb9a6e20fd6e10c1b77654d067c0618f6e5a7f79a").unwrap())
}

#[test]
fn ethereum_s_spec_signature_over_zero_bytes_holds_and_nothing_else_does() {
    let (_, key) = spec_key();
    let signature = from_hex("0xb6ed936746e01f8ecf281f020953fbf1f01debd5657c4a383940b020b26507f6076334f91e2366c96e9ab279fb5158090352ea1c5b0c9274504f4f0e7053af24802e51e4568d164fe986834f41e55c8e850ce1f98458c0cfc9ab380b55285a55").unwrap();

    assert!(fast_aggregate_verify(&key, &[0; 32], &signature));
    assert!(!fast_aggregate_verify(&key, &[0; 31], &signature), "another message");
    assert!(!fast_aggregate_verify(&[key.clone(), key.clone()].concat(), &[0; 32], &signature), "the key twice");
}

#[test]
fn many_keys_aggregate_and_any_one_missing_fails() {
    let secrets: Vec<Scalar> = (1..=20u64).map(|n| Scalar::from(n * 7919)).collect();
    let keys: Vec<u8> = secrets.iter().flat_map(testing::public_key).collect();
    let message = b"a sync committee's signing root";
    let total = secrets.iter().fold(Scalar::zero(), |sum, secret| sum + secret);
    let aggregate = testing::sign(&total, message);

    assert!(fast_aggregate_verify(&keys, message, &aggregate));
    assert!(!fast_aggregate_verify(&keys[48..], message, &aggregate), "one key short");
    assert!(!fast_aggregate_verify(&keys, b"another message", &aggregate));

    // Each member's own signature checks alone, as ethlight's native check has it too.
    let one = testing::sign(&secrets[3], message);
    assert!(fast_aggregate_verify(&keys[3 * 48..4 * 48], message, &one));
    assert!(testing::fast_aggregate_verify(&[keys[3 * 48..4 * 48].try_into().unwrap()], message, &one));
}

#[test]
fn wrong_lengths_infinity_and_points_off_the_group_are_refused() {
    let (secret, key) = spec_key();
    let message = [9u8; 32];
    let signature = testing::sign(&secret, &message);
    assert!(fast_aggregate_verify(&key, &message, &signature));

    // Lengths: no keys, part of a key, a signature a byte short or long.
    assert!(!fast_aggregate_verify(&[], &message, &signature));
    assert!(!fast_aggregate_verify(&key[..47], &message, &signature));
    assert!(!fast_aggregate_verify(&[key.clone(), vec![0]].concat(), &message, &signature));
    assert!(!fast_aggregate_verify(&key, &message, &signature[..95]));
    assert!(!fast_aggregate_verify(&key, &message, &[signature.as_slice(), &[0]].concat()));

    // The points at infinity, as keys and as the signature.
    let mut infinity_key = [0u8; 48];
    infinity_key[0] = 0xc0;
    let mut infinity_signature = [0u8; 96];
    infinity_signature[0] = 0xc0;
    assert!(public_key(&infinity_key).is_none() && super::signature(&infinity_signature).is_none());
    assert!(!fast_aggregate_verify(&[key.as_slice(), &infinity_key].concat(), &message, &signature));
    assert!(!fast_aggregate_verify(&infinity_key, &message, &infinity_signature));

    // Keys that sum to infinity, with the signature at infinity they would need.
    let negated = G1Affine::from(-G1Projective::from(public_key(&key).unwrap())).to_compressed();
    assert!(!fast_aggregate_verify(&[key.as_slice(), &negated].concat(), &message, &infinity_signature));

    // An x past the field's modulus, and a point on the curve outside the prime-order group.
    assert!(public_key(&[0xff; 48]).is_none());
    let outside = (0u8..=255)
        .map(|x| {
            let mut bytes = [0u8; 48];
            bytes[0] = 0x80;
            bytes[47] = x;
            bytes
        })
        .find(|bytes| bool::from(G1Affine::from_compressed_unchecked(bytes).is_some()) && bool::from(G1Affine::from_compressed(bytes).is_none()))
        .expect("most points on the curve are outside the group");
    assert!(!fast_aggregate_verify(&outside, &message, &signature));

    let mut bad_signature = signature;
    bad_signature[0] &= 0x7f;
    assert!(!fast_aggregate_verify(&key, &message, &bad_signature), "a signature without its compression flag");
}
