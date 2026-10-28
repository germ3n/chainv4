use {
    super::*,
    crate::crypto::keypair::Keypair,
};

fn key(seed: u8) -> Keypair {
    Keypair::from_seed([seed; 32])
}

fn keys(threshold: usize, seeds: &[u8]) -> ReleaseKeys {
    ReleaseKeys { threshold, keys: seeds.iter().map(|seed| key(*seed).public()).collect(), updates: None }
}

fn release() -> Release {
    let binary = b"the new chainv4";

    Release {
        issued: 1_800_000_000,
        products: vec![
            Product {
                name: "chainv4".to_string(),
                version: Version::new(0, 2, 0),
                least: Version::new(0, 1, 0),
                notes: "pools".to_string(),
                files: vec![ReleaseFile { target: TARGET.to_string(), url: "https://example.org/chainv4".to_string(), sha256: sha256(binary), size: binary.len() as u64 }],
            },
            Product { name: "wallet".to_string(), version: Version::new(0, 3, 0), least: Version::new(0, 3, 0), notes: String::new(), files: Vec::new() },
        ],
        breaking: vec![Breaking { chain: vec![7; 32], upgrade: Upgrade { least: Version::new(0, 2, 0), from: 500 } }],
    }
}

#[test]
fn a_release_counts_once_a_threshold_of_the_keys_built_in_signed_it() {
    let mut signed = SignedRelease { release: release(), signatures: Vec::new() };
    let trusted = keys(2, &[1, 2, 3]);

    assert_eq!(signed.verify(&trusted), Err(ReleaseError::TooFewSignatures { valid: 0, threshold: 2 }));

    signed.sign(&key(1));
    signed.sign(&key(1));
    signed.sign(&key(9));
    assert_eq!(signed.verify(&trusted), Err(ReleaseError::TooFewSignatures { valid: 1, threshold: 2 }), "a key signing twice, or one not trusted, counts no more");

    signed.sign(&key(3));
    assert_eq!(signed.verify(&trusted), Ok(2));
    assert_eq!(signed.verify(&ReleaseKeys::default()), Err(ReleaseError::NoKeys));

    let mut changed = signed.clone();
    changed.release.products[0].files[0].url = "https://elsewhere.example/chainv4".to_string();
    assert!(changed.verify(&trusted).is_err(), "the signatures cover every file's address and hash");
}

#[test]
fn a_manifest_reads_back_what_it_wrote_and_still_verifies() {
    let mut signed = SignedRelease { release: release(), signatures: Vec::new() };
    signed.sign(&key(1));
    signed.sign(&key(2));
    let text = serde_json::to_string_pretty(&signed.to_json()).unwrap();
    let back = SignedRelease::from_json(&serde_json::from_str(&text).unwrap()).unwrap();

    assert_eq!(back, signed);
    assert_eq!(back.verify(&keys(2, &[1, 2])), Ok(2));
}

#[test]
fn products_stand_behind_or_unsupported_and_upgrades_stop_older_nodes_of_their_chain() {
    let release = release();
    let node = release.product("chainv4").unwrap();

    assert_eq!(node.standing(Version::new(0, 2, 0)), Standing::Current);
    assert_eq!(node.standing(Version::new(0, 1, 5)), Standing::Behind { newest: Version::new(0, 2, 0) });
    assert_eq!(release.product("wallet").unwrap().standing(Version::new(0, 2, 9)), Standing::Unsupported { newest: Version::new(0, 3, 0), least: Version::new(0, 3, 0) });
    assert!(node.file(TARGET).is_some() && node.file("riscv-unknown").is_none());

    assert_eq!(release.stopping(&[7; 32], Version::new(0, 1, 0)), Some(Upgrade { least: Version::new(0, 2, 0), from: 500 }));
    assert_eq!(release.stopping(&[7; 32], Version::new(0, 2, 0)), None);
    assert_eq!(release.stopping(&[8; 32], Version::new(0, 1, 0)), None, "another chain's upgrade");
}

#[test]
fn releases_out_of_order_or_naming_unknown_products_are_refused() {
    let mut swapped = release();
    swapped.products.reverse();
    assert!(swapped.problem().is_some());

    let mut unknown = release();
    unknown.products[1].name = "zzz".to_string();
    assert!(unknown.problem().is_some());

    let mut least = release();
    least.products[0].least = Version::new(9, 0, 0);
    assert!(least.problem().is_some());
}

#[test]
fn keys_json_needs_a_threshold_its_keys_can_meet() {
    let one = key(1).public().to_string();
    let two = key(2).public().to_string();

    assert_eq!(ReleaseKeys::from_json(&json!({ "threshold": 2, "keys": [one, two], "updates": "https://example.org/manifest.json" })).map(|keys| (keys.threshold, keys.keys.len(), keys.updates)), Ok((2, 2, Some("https://example.org/manifest.json".to_string()))));
    assert!(ReleaseKeys::from_json(&json!({ "threshold": 3, "keys": [one, two] })).is_err());
    assert!(ReleaseKeys::from_json(&json!({ "threshold": 0, "keys": [one] })).is_err());
    assert!(ReleaseKeys::from_json(&json!({ "threshold": 1, "keys": [one, one] })).is_err());
    assert!(ReleaseKeys::built_in().is_ok(), "the keys this build was made with read");
}

#[test]
fn a_binary_goes_in_place_only_if_it_is_the_one_signed() {
    let dir = std::env::temp_dir().join(format!("chainv4-install-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("chainv4");
    std::fs::write(&path, b"the old chainv4").unwrap();
    let file = release().products[0].files[0].clone();

    assert!(install(&file, b"something else", &path).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"the old chainv4");

    install(&file, b"the new chainv4", &path).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"the new chainv4");
    assert!(!path.with_extension("new").exists());

    std::fs::remove_dir_all(&dir).unwrap();
}
