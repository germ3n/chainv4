#[cfg(all(feature = "ed25519", feature = "edblake", feature = "secp256k1", feature = "bitcoin"))]
use crate::crypto::keypair::{Keypair, PrivateKey};
use {
    super::*,
    crate::db::{MemoryDb, State, StateTree},
    std::sync::Arc,
};

#[cfg(all(feature = "ed25519", feature = "edblake", feature = "secp256k1", feature = "bitcoin"))]
fn signed(private: PrivateKey, message: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let keypair = Keypair::from_private(private);
    let key = match keypair.public() {
        #[cfg(feature = "ed25519")]
        PublicKey::Ed25519(key) => key.to_vec(),
        #[cfg(feature = "edblake")]
        PublicKey::EdBlake(key) => key.to_vec(),
        #[cfg(feature = "secp256k1")]
        PublicKey::Secp256k1(key) => key.to_vec(),
        #[cfg(feature = "bitcoin")]
        PublicKey::Bitcoin(key) => key.to_vec(),
    };
    let signature = match keypair.sign(message) {
        #[cfg(feature = "ed25519")]
        Signature::Ed25519(bytes) => bytes,
        #[cfg(feature = "edblake")]
        Signature::EdBlake(bytes) => bytes,
        #[cfg(feature = "secp256k1")]
        Signature::Secp256k1(bytes) => bytes,
        #[cfg(feature = "bitcoin")]
        Signature::Bitcoin(bytes) => bytes,
        other => panic!("a key signs with its scheme's signature, not {other:?}"),
    };

    (key, signature.to_vec())
}

#[cfg(all(feature = "ed25519", feature = "edblake", feature = "secp256k1", feature = "bitcoin"))]
#[test]
fn every_scheme_s_signatures_check_as_chainv4_signs_them() {
    let message = b"pay alice 5";

    for (scheme, private) in [
        ("ed25519", PrivateKey::Ed25519([1; 32])),
        ("edblake", PrivateKey::EdBlake([2; 32])),
        ("secp256k1", PrivateKey::Secp256k1([3; 32])),
        ("bitcoin", PrivateKey::Bitcoin([4; 32])),
    ] {
        let (key, signature) = signed(private.clone(), message);

        assert_eq!(check_signature(scheme, &key, message, &signature), Ok(true), "{scheme}");
        assert_eq!(check_signature(scheme, &key, b"pay alice 6", &signature), Ok(false), "{scheme}: another message");
        assert_eq!(check_signature(scheme, &key, message, &signature[..63]), Ok(false), "{scheme}: a short signature");
        assert_eq!(key_address(scheme, &key), Ok(Keypair::from_private(private).public().address()), "{scheme}");
    }

    // A key of another length, or a scheme the chain does not check, is an error, not a no.
    assert!(check_signature("ed25519", &[1; 33], message, &[0; 64]).is_err());
    assert!(check_signature("rsa", &[1; 32], message, &[0; 64]).unwrap_err().contains("not a scheme"));
    assert!(check_signature("secp256k1", &[1; 33], message, &[0; 64]) == Ok(false), "a key that is no point holds nothing");
}

#[cfg(feature = "secp256k1")]
#[test]
fn recovery_finds_the_key_as_ecrecover_does() {
    let secret = [7; 32];
    let digest = [9; 32];
    let (rs, parity) = crate::crypto::secp256k1::sign_recoverable(&secret, &digest);
    let key = crate::crypto::secp256k1::public_key(&secret).unwrap();

    for v in [parity, parity + 27] {
        assert_eq!(recover_key(&digest, &[rs.as_slice(), &[v]].concat()), Some(key), "v {v}");
    }

    assert_ne!(recover_key(&digest, &[rs.as_slice(), &[1 - parity]].concat()), Some(key), "the other parity is another key");
    assert_eq!(recover_key(&digest, &rs), None, "64 bytes, no v");
    assert_eq!(recover_key(&digest, &[rs.as_slice(), &[5]].concat()), None);
}

#[cfg(feature = "bitcoin")]
#[test]
fn bitcoin_s_hashes_are_its_own() {
    // RIPEMD-160 and SHA-256 of the empty string, as everyone writes them.
    assert_eq!(crate::structs::hex::to_hex(&digest("ripemd160", b"").unwrap()), "0x9c1185a5c5e9fc54612808977ee8f548b2258d31");
    assert_eq!(digest("hash160", b"abc").unwrap(), crate::crypto::bitcoin::hash160(b"abc"));
    assert_eq!(digest("sha256d", b"abc").unwrap(), crate::crypto::bitcoin::sha256d(b"abc"));
    assert_eq!(digest("md5", b"abc"), None);
}

#[test]
fn proofs_check_against_their_root_alone() {
    let hash = HashType::default_chain();
    let state = Arc::new(State::new(Arc::new(MemoryDb::default())));
    state.put(Column::Accounts, b"alice".to_vec(), b"rich".to_vec());
    state.put(Column::Accounts, b"bob".to_vec(), b"poor".to_vec());
    let tree = StateTree::new(&state, hash);
    let root = tree.follow().unwrap().to_bytes();
    let proof = wincode::serialize(&tree.prove(Column::Accounts, b"alice").unwrap()).unwrap();
    let absent = wincode::serialize(&tree.prove(Column::Accounts, b"carol").unwrap()).unwrap();

    assert!(check_state(hash, &root, "accounts", b"alice", Some(b"rich"), &proof));
    assert!(!check_state(hash, &root, "accounts", b"alice", Some(b"poor"), &proof));
    assert!(check_state(hash, &root, "accounts", b"carol", None, &absent), "nothing, proven");
    assert!(!check_state(hash, &root, "names", b"alice", Some(b"rich"), &proof), "another column");
    assert!(!check_state(hash, &root, "accounts", b"alice", Some(b"rich"), b"junk"));
    assert!(!check_state(hash, &root, "nowhere", b"alice", Some(b"rich"), &proof));

    // A Merkle path, as a header's roots take their transactions and receipts.
    let leaves: Vec<Hash> = [b"one".as_slice(), b"two", b"three"].iter().map(|data| crate::crypto::merkle::leaf(hash, data)).collect();
    let root = crate::crypto::merkle::root(hash, &leaves).to_bytes();
    let path: Vec<u8> = crate::crypto::merkle::audit_path(hash, &leaves, 1).iter().flat_map(|sibling| sibling.to_bytes()).collect();

    assert!(check_merkle(hash, &root, b"two", 1, 3, &path));
    assert!(!check_merkle(hash, &root, b"two", 0, 3, &path));
    assert!(!check_merkle(hash, &root, b"two!", 1, 3, &path));
    assert!(!check_merkle(hash, &root, b"two", 1, 3, &path[1..]), "not whole hashes");
}

/// Times the BLS check against check_signature, whose 3,000 fuel is the EVM's ecrecover, to set
/// bls_verify and bls_key in proportion. Run in a release build:
/// `cargo test --release --lib vm::crypto::tests::bls_costs -- --ignored --nocapture`.
#[cfg(all(feature = "bls12-381", feature = "secp256k1", feature = "ed25519"))]
#[test]
#[ignore]
fn bls_costs_are_measured_against_check_signature() {
    use std::time::Instant;

    let time = |runs: u32, work: &dyn Fn()| {
        work();
        let start = Instant::now();

        for _ in 0..runs {
            work();
        }

        start.elapsed().as_secs_f64() * 1e6 / f64::from(runs)
    };
    let check = |private: crate::crypto::keypair::PrivateKey| {
        let keypair = crate::crypto::keypair::Keypair::from_private(private);
        let key = match keypair.public() {
            PublicKey::Secp256k1(key) => key.to_vec(),
            PublicKey::Ed25519(key) => key.to_vec(),
            _ => unreachable!(),
        };
        let signature = match keypair.sign(b"pay alice 5") {
            Signature::Secp256k1(bytes) | Signature::Ed25519(bytes) => bytes.to_vec(),
            _ => unreachable!(),
        };
        let scheme = if key.len() == 33 { "secp256k1" } else { "ed25519" };

        time(2_000, &|| assert!(check_signature(scheme, &key, b"pay alice 5", &signature).unwrap()))
    };
    let secp = check(crate::crypto::keypair::PrivateKey::Secp256k1([7; 32]));
    let ed = check(crate::crypto::keypair::PrivateKey::Ed25519([7; 32]));

    let secrets: Vec<bls12_381::Scalar> = (1..=512u64).map(|n| bls12_381::Scalar::from(n * 7919)).collect();
    let keys: Vec<u8> = secrets.iter().flat_map(ethlight::testing::public_key).collect();
    let message = [0x42; 32];
    let signed = |count: usize| ethlight::testing::sign(&secrets[..count].iter().fold(bls12_381::Scalar::zero(), |sum, secret| sum + secret), &message);
    let (one, all) = (signed(1), signed(512));
    let single = time(200, &|| assert!(fast_aggregate_verify(&keys[..48], &message, &one).unwrap()));
    let full = time(50, &|| assert!(fast_aggregate_verify(&keys, &message, &all).unwrap()));
    let per_key = (full - single) / 511.0;
    let base = single - per_key;
    let fuel_per_us = 3_000.0 / secp;

    println!("secp256k1 verify {secp:.1} µs, ed25519 verify {ed:.1} µs: {fuel_per_us:.1} fuel a µs");
    println!("BLS: one key {single:.1} µs, 512 keys {full:.1} µs: base {base:.1} µs, {per_key:.2} µs a key");
    println!("in proportion: bls_verify {:.0}, bls_key {:.0}", base * fuel_per_us, per_key * fuel_per_us);
}
