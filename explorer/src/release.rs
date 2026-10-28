//! The explorer's own releases (docs/updates.md): what the manifest a node relays says of this
//! explorer, once the release keys built into it have checked it. The node only relays it, so its
//! word counts for nothing here: a manifest counts once a threshold of those keys signed it.

use {
    chainv4::release::{ReleaseError, ReleaseFile, ReleaseKeys, SignedRelease, Standing, TARGET},
    chainv4::structs::version::Version,
    serde_json::Value,
};

/// The name a release gives the explorer among its products.
const PRODUCT: &str = "explorer";

/// This explorer's version, from its own Cargo.toml: not the chainv4 library's, which
/// Version::ours is.
pub fn ours() -> Version {
    env!("CARGO_PKG_VERSION").parse().expect("Cargo.toml's version is major.minor.patch")
}

/// A newer explorer than this one, from a manifest enough of the release keys signed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Update {
    /// Behind, while this one still works, or Unsupported, once it no longer does; never Current.
    pub standing: Standing,
    /// What changed, in a line or a few.
    pub notes: String,
    /// The binary for the target this build was compiled for, if the release has one.
    pub file: Option<ReleaseFile>,
}

impl Update {
    /// The newest explorer's version.
    pub fn newest(&self) -> Version {
        match self.standing {
            Standing::Behind { newest } | Standing::Unsupported { newest, .. } => newest,
            Standing::Current => ours(),
        }
    }

    /// Whether this explorer no longer works with the network, and stops until it is updated.
    pub fn blocks(&self) -> bool {
        matches!(self.standing, Standing::Unsupported { .. })
    }
}

/// What to show of the manifest a node relays, as GET /release's manifest carries it: an update,
/// when it names a newer explorer than this version, or None, when the node relays none, the
/// release names no explorer, or this one is the newest.
///
/// A manifest this build has no keys to check, or one too few of them signed, is an error, and so
/// is one that does not read as a release: either way it shows nothing.
pub fn update(manifest: &Value, keys: &ReleaseKeys, ours: Version) -> Result<Option<Update>, ReleaseError> {
    if !keys.on() {
        return Err(ReleaseError::NoKeys);
    }

    if manifest.is_null() {
        return Ok(None);
    }

    let signed = SignedRelease::from_json(manifest).map_err(ReleaseError::Malformed)?;
    signed.verify(keys)?;

    let Some(product) = signed.release.product(PRODUCT) else {
        return Ok(None);
    };

    Ok(match product.standing(ours) {
        Standing::Current => None,
        standing => Some(Update {
            standing,
            notes: product.notes.clone(),
            file: product.file(TARGET).cloned(),
        }),
    })
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        chainv4::crypto::keypair::Keypair,
        chainv4::release::{Product, Release, sha256},
    };

    fn key(seed: u8) -> Keypair {
        Keypair::from_seed([seed; 32])
    }

    fn keys(threshold: usize, seeds: &[u8]) -> ReleaseKeys {
        ReleaseKeys {
            threshold,
            keys: seeds.iter().map(|seed| key(*seed).public()).collect(),
            updates: None,
        }
    }

    /// A manifest naming explorer 0.2.0, which works back to least, with a binary for target,
    /// signed by the keys of these seeds.
    fn manifest(least: Version, target: &str, signers: &[u8]) -> Value {
        let binary = b"the new explorer";
        let mut signed = SignedRelease {
            release: Release {
                issued: 1_800_000_000,
                products: vec![Product {
                    name: "explorer".to_string(),
                    version: Version::new(0, 2, 0),
                    least,
                    notes: "update notices".to_string(),
                    files: vec![ReleaseFile {
                        target: target.to_string(),
                        url: "https://example.org/explorer".to_string(),
                        sha256: sha256(binary),
                        size: binary.len() as u64,
                    }],
                }],
                breaking: Vec::new(),
            },
            signatures: Vec::new(),
        };

        for seed in signers {
            signed.sign(&key(*seed));
        }

        signed.to_json()
    }

    #[test]
    fn a_build_without_release_keys_shows_nothing() {
        let signed = manifest(Version::new(0, 1, 0), TARGET, &[1, 2]);

        assert_eq!(update(&signed, &ReleaseKeys::default(), Version::new(0, 1, 0)), Err(ReleaseError::NoKeys));
        assert_eq!(update(&signed, &ReleaseKeys::built_in().unwrap(), Version::new(0, 1, 0)), Err(ReleaseError::NoKeys), "release/keys.json is empty");
        assert_eq!(update(&Value::Null, &keys(1, &[1]), Version::new(0, 1, 0)), Ok(None), "a node that relays none");
    }

    #[test]
    fn a_manifest_too_few_of_the_keys_signed_shows_nothing() {
        let signed = manifest(Version::new(0, 1, 0), TARGET, &[1, 9]);

        assert_eq!(
            update(&signed, &keys(2, &[1, 2, 3]), Version::new(0, 1, 0)),
            Err(ReleaseError::TooFewSignatures { valid: 1, threshold: 2 })
        );
        assert!(matches!(update(&serde_json::json!({ "release": {} }), &keys(1, &[1]), Version::new(0, 1, 0)), Err(ReleaseError::Malformed(_))));
    }

    #[test]
    fn a_newer_explorer_that_still_works_is_an_update_with_this_target_s_binary() {
        let found = update(&manifest(Version::new(0, 1, 0), TARGET, &[1, 2]), &keys(2, &[1, 2, 3]), Version::new(0, 1, 0)).unwrap().unwrap();

        assert_eq!(found.standing, Standing::Behind { newest: Version::new(0, 2, 0) });
        assert_eq!((found.newest(), found.blocks(), found.notes.as_str()), (Version::new(0, 2, 0), false, "update notices"));
        assert_eq!(found.file.map(|file| file.url), Some("https://example.org/explorer".to_string()));
    }

    #[test]
    fn an_explorer_below_the_least_version_is_unsupported_and_blocks() {
        let found = update(&manifest(Version::new(0, 2, 0), TARGET, &[1]), &keys(1, &[1]), Version::new(0, 1, 9)).unwrap().unwrap();

        assert_eq!(found.standing, Standing::Unsupported { newest: Version::new(0, 2, 0), least: Version::new(0, 2, 0) });
        assert!(found.blocks());
    }

    #[test]
    fn the_newest_explorer_or_a_newer_one_is_current_and_shows_nothing() {
        let signed = manifest(Version::new(0, 1, 0), TARGET, &[1]);

        assert_eq!(update(&signed, &keys(1, &[1]), Version::new(0, 2, 0)), Ok(None));
        assert_eq!(update(&signed, &keys(1, &[1]), Version::new(0, 3, 0)), Ok(None));
    }

    #[test]
    fn a_release_without_a_binary_for_this_target_is_still_an_update_with_none_to_install() {
        let found = update(&manifest(Version::new(0, 1, 0), "nowhere-unknown-none", &[1]), &keys(1, &[1]), Version::new(0, 1, 0)).unwrap().unwrap();

        assert_eq!((found.newest(), found.file), (Version::new(0, 2, 0), None));
    }
}
