use {
    super::*,
    crate::crypto::keypair::Keypair,
};

fn key(seed: u8) -> Keypair {
    Keypair::from_seed([seed; 32])
}

fn keys(threshold: usize, seeds: &[u8]) -> ReleaseKeys {
    ReleaseKeys { threshold, keys: seeds.iter().map(|seed| key(*seed).public()).collect(), updates: None, floor: 0 }
}

fn release() -> Release {
    let binary = b"the new chainv4";

    Release {
        issued: 1_800_000_000,
        expires: 1_800_000_000 + DEFAULT_VALID_DAYS * DAY,
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

const ISSUED: u64 = 1_800_000_000;

fn issued(at: u64) -> SignedRelease {
    let mut signed = SignedRelease { release: Release { issued: at, expires: at + DEFAULT_VALID_DAYS * DAY, ..release() }, signatures: Vec::new() };
    signed.sign(&key(1));

    signed
}

#[test]
fn nothing_counts_past_its_expires_and_changing_that_breaks_its_signatures() {
    let signed = issued(ISSUED);
    let trusted = keys(1, &[1]);
    let expires = signed.release.expires;

    assert_eq!(signed.check(&trusted, expires - 1), Ok(1));
    assert_eq!(signed.check(&trusted, expires), Err(ReleaseError::Expired { expires }));
    assert_eq!(signed.check(&trusted, expires + 1), Err(ReleaseError::Expired { expires }));
    // Signed in: moved along by a host, it no longer verifies.
    let mut moved = signed.clone();
    moved.release.expires += 1;
    assert_eq!(moved.verify(&trusted), Err(ReleaseError::TooFewSignatures { valid: 0, threshold: 1 }));
    // It survives JSON, as the manifest carries it.
    assert_eq!(SignedRelease::from_json(&signed.to_json()).unwrap(), signed);
    let mut stripped = signed.to_json();
    stripped["release"].as_object_mut().unwrap().remove("expires");
    assert!(SignedRelease::from_json(&stripped).unwrap_err().contains("expires"), "a manifest without one is none");
}

#[test]
fn a_release_lapses_after_it_is_issued_and_not_too_long_after() {
    let with = |expires: u64| Release { expires, ..release() }.problem();

    assert!(with(ISSUED).unwrap().contains("not after it is issued"));
    assert!(with(0).is_some());
    assert_eq!(with(ISSUED + MOST_VALID_DAYS * DAY), None);
    assert!(with(ISSUED + MOST_VALID_DAYS * DAY + 1).unwrap().contains("366 days"));
}

#[test]
fn a_build_takes_nothing_issued_before_its_floor() {
    let trusted = ReleaseKeys { floor: ISSUED, ..keys(1, &[1]) };

    assert_eq!(issued(ISSUED).check(&trusted, ISSUED), Ok(1));
    assert_eq!(issued(ISSUED - 1).check(&trusted, ISSUED), Err(ReleaseError::BelowFloor { issued: ISSUED - 1, floor: ISSUED }));
    let mut kept = None;
    assert_eq!(take_newest(&trusted, &mut kept, issued(ISSUED - 1), ISSUED), Err(ReleaseError::BelowFloor { issued: ISSUED - 1, floor: ISSUED }));
    assert_eq!(kept, None);
    assert!(kept_release(&issued(ISSUED - 1).to_json().to_string(), &trusted).is_none(), "nor one kept from before the build");

    assert_eq!(ReleaseKeys::from_json(&json!({ "threshold": 1, "keys": [key(1).public().to_string()], "floor": 5 })).unwrap().floor, 5);
    assert_eq!(ReleaseKeys::from_json(&json!({ "threshold": 1, "keys": [key(1).public().to_string()] })).unwrap().floor, 0);
}

#[test]
fn a_host_cannot_go_back_and_what_was_seen_stands_while_it_counts() {
    let trusted = keys(1, &[1]);
    let mut kept = None;

    assert_eq!(take_newest(&trusted, &mut kept, issued(ISSUED), ISSUED), Ok(true));
    assert_eq!(take_newest(&trusted, &mut kept, issued(ISSUED), ISSUED), Ok(false), "the same one");
    assert_eq!(take_newest(&trusted, &mut kept, issued(ISSUED - 10), ISSUED), Ok(false), "an older one, as a stale host serves");
    assert_eq!(kept.as_ref().unwrap().release.issued, ISSUED);
    assert_eq!(current_release(&kept, ISSUED).unwrap().release.issued, ISSUED);

    // Across a restart, by what was kept.
    let mut kept = kept_release(&kept.unwrap().to_json().to_string(), &trusted);
    assert_eq!(take_newest(&trusted, &mut kept, issued(ISSUED - 10), ISSUED), Ok(false));

    // Unsigned, it changes nothing.
    let mut forged = issued(ISSUED + 10);
    forged.signatures.clear();
    assert!(take_newest(&trusted, &mut kept, forged, ISSUED).is_err());
    assert_eq!(kept.as_ref().unwrap().release.issued, ISSUED);

    // What cannot be read leaves the floor alone.
    assert_eq!(kept_release("not json", &trusted), None);
    assert_eq!(kept_release("{}", &trusted), None);
    assert_eq!(kept_release(&issued(ISSUED).to_json().to_string(), &keys(1, &[2])), None, "another build's keys");
}

/// A wrong clock costs nothing that lasts: what is kept comes from what was signed, never the clock.
#[test]
fn a_wrong_clock_stops_or_lets_through_for_a_while_and_is_put_right_by_setting_it() {
    let trusted = keys(1, &[1]);
    let right = ISSUED + DAY;
    let mut kept = None;

    // Far ahead: what is true now looks expired, nothing is acted on, and it is kept all the same.
    let ahead = right + 400 * DAY;
    let expires = issued(ISSUED).release.expires;
    assert_eq!(take_newest(&trusted, &mut kept, issued(ISSUED), ahead), Err(ReleaseError::Expired { expires }));
    assert!(current_release(&kept, ahead).is_none());
    assert_eq!(current_release(&kept, right).unwrap().release.issued, ISSUED, "the clock put right, it counts");

    // Far behind: an old one looks current, but nothing older than what was seen comes back.
    let behind = ISSUED - 400 * DAY;
    assert_eq!(take_newest(&trusted, &mut kept, issued(ISSUED - 100 * DAY), behind), Ok(false));
    assert_eq!(kept.as_ref().unwrap().release.issued, ISSUED);
    assert!(current_release(&kept, behind).is_some(), "the newest seen, however early it seems");

    // A newer one is taken whatever the clock says of it, and nothing is ever stuck on a clock.
    assert_eq!(take_newest(&trusted, &mut kept, issued(ISSUED + 10), behind), Ok(true));
}

#[test]
fn a_kept_manifest_is_written_whole_and_read_back() {
    let dir = std::env::temp_dir().join(format!("chainv4-kept-{}", std::process::id()));
    let file = dir.join("nested").join("release.json");
    write_kept(&file, &issued(ISSUED)).unwrap();

    assert_eq!(kept_release(&std::fs::read_to_string(&file).unwrap(), &keys(1, &[1])).unwrap().release.issued, ISSUED);
    assert!(!file.with_extension("new").exists());
    std::fs::remove_dir_all(&dir).unwrap();
}
