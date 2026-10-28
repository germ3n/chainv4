//! Releases (docs/updates.md): a manifest of the newest chainv4, wallet and explorer, each with a
//! binary for every target it is built for and its SHA-256, and the upgrades that stop older nodes
//! of a chain at a height. Release keys built into every binary sign it, and it counts only once a
//! threshold of them have, so no one key, and no node or host relaying it, can push a binary. A
//! node fetches it, checks it, and serves it at GET /release; workers, the apps and chainv4 update
//! check it again themselves.

use {
    crate::crypto::keypair::{PublicKey, Signature},
    crate::structs::hex::{from_hex, to_hex},
    crate::structs::version::{Upgrade, Version},
    r#macro::document,
    serde_json::{Value, json},
    std::fmt,
    wincode::{SchemaRead, SchemaWrite},
};

#[cfg(test)]
mod tests;

#[document(
    summary = "The target triple this build was compiled for, such as aarch64-apple-darwin: which of a release's files is its binary.",
    category = "Releases",
    see_also = "ReleaseFile",
)]
pub const TARGET: &str = env!("CHAINV4_TARGET");

/// The release keys this build trusts: release/keys.json, or the file CHAINV4_RELEASE_KEYS named
/// when it was built.
const BUILT_IN_KEYS: &str = include_str!(concat!(env!("OUT_DIR"), "/release-keys.json"));

/// What a release's signatures are made over, before its encoding: so no other signature a key
/// makes is one over a release.
const DOMAIN: &[u8] = b"chainv4 release\0";

#[document(
    summary = "The products a release names: the node and CLI, the wallet and the explorer.",
    category = "Releases",
    see_also = "Product",
)]
pub const PRODUCTS: [&str; 3] = ["chainv4", "wallet", "explorer"];

#[document(
    summary = "One binary of a product: the target it runs on, where to download it, and its SHA-256 and size, which the signatures cover, so wherever it is fetched from it is the one signed.",
    category = "Releases",
    see_also = "Product, install",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct ReleaseFile {
    #[document(summary = "The target triple, such as x86_64-unknown-linux-gnu.")]
    pub target: String,
    #[document(summary = "Where to download it.")]
    pub url: String,
    #[document(summary = "Its SHA-256.")]
    pub sha256: [u8; 32],
    #[document(summary = "Its size in bytes.")]
    pub size: u64,
}

#[document(
    summary = "A product's newest release: its version, the oldest version that still works with the network, what changed, and its binaries.",
    category = "Releases",
    see_also = "Release, ReleaseFile",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct Product {
    #[document(summary = "chainv4, wallet or explorer.")]
    pub name: String,
    #[document(summary = "The newest version.")]
    pub version: Version,
    #[document(summary = "The oldest version that still works: the apps below it stop and ask to update; a node is stopped by upgrades instead, at a height.")]
    pub least: Version,
    #[document(summary = "What changed, in a line or a few.")]
    pub notes: String,
    #[document(summary = "Its binaries, one for each target, in the order of their targets.")]
    pub files: Vec<ReleaseFile>,
}

impl Product {
    #[document(parent = "Product", summary = "The binary for a target, if it was built for it.")]
    pub fn file(&self, target: &str) -> Option<&ReleaseFile> {
        self.files.iter().find(|file| file.target == target)
    }

    #[document(parent = "Product", summary = "Where a build of this version stands against it.")]
    pub fn standing(&self, ours: Version) -> Standing {
        if ours < self.least {
            Standing::Unsupported { newest: self.version, least: self.least }
        } else if ours < self.version {
            Standing::Behind { newest: self.version }
        } else {
            Standing::Current
        }
    }
}

#[document(
    summary = "Where a build stands against its product's newest release.",
    category = "Releases",
    see_also = "Product::standing",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Standing {
    #[document(summary = "It is the newest, or newer.")]
    Current,
    #[document(summary = "A newer version is out; this one still works.")]
    Behind { newest: Version },
    #[document(summary = "It is older than the oldest that still works: an app stops and asks to update.")]
    Unsupported { newest: Version, least: Version },
}

#[document(
    summary = "An upgrade of one chain the release announces, for chains without governance to announce one with Change::Upgrade: from block from on, nodes of that chain older than least stop.",
    category = "Releases",
    see_also = "Upgrade, Change::Upgrade",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct Breaking {
    #[document(summary = "The chain, by its genesis block's id.")]
    pub chain: Vec<u8>,
    #[document(summary = "The version that runs on past it, and the height older nodes stop at.")]
    pub upgrade: Upgrade,
}

#[document(
    summary = "What the release keys sign: when it was issued, which orders releases so an older one never replaces a newer, each product's newest release, and the upgrades it announces.",
    category = "Releases",
    see_also = "SignedRelease, Product, Breaking",
)]
#[derive(Clone, Debug, Default, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct Release {
    #[document(summary = "When it was issued, in seconds since the Unix epoch.")]
    pub issued: u64,
    #[document(summary = "Each product's newest release, in the order of their names.")]
    pub products: Vec<Product>,
    #[document(summary = "The upgrades it announces, for chains without governance.")]
    pub breaking: Vec<Breaking>,
}

impl Release {
    #[document(parent = "Release", summary = "The bytes its signatures are over: a domain of their own, then its encoding.")]
    pub fn signing_bytes(&self) -> Vec<u8> {
        let mut bytes = DOMAIN.to_vec();
        bytes.extend(wincode::serialize(self).expect("a release encodes"));

        bytes
    }

    #[document(parent = "Release", summary = "A product's newest release, if it names one.")]
    pub fn product(&self, name: &str) -> Option<&Product> {
        self.products.iter().find(|product| product.name == name)
    }

    #[document(
        parent = "Release",
        summary = "The upgrade it announces for a chain that stops a node of this version first, if any.",
        params = {
            chain = "The chain's genesis id, as bytes.",
            ours = "The node's version.",
        },
    )]
    pub fn stopping(&self, chain: &[u8], ours: Version) -> Option<Upgrade> {
        self.breaking
            .iter()
            .filter(|breaking| breaking.chain == chain && breaking.upgrade.stops(ours))
            .map(|breaking| breaking.upgrade)
            .min_by_key(|upgrade| upgrade.from)
    }

    #[document(parent = "Release", summary = "Why it is not a release: products not in order or named twice, a name not in PRODUCTS, a least version above the newest, or a product's targets out of order or repeated.")]
    pub fn problem(&self) -> Option<String> {
        if self.products.windows(2).any(|pair| pair[0].name >= pair[1].name) {
            return Some("its products go in the order of their names, each once".to_string());
        }

        for product in &self.products {
            if !PRODUCTS.contains(&product.name.as_str()) {
                return Some(format!("{} is not a product: chainv4, wallet or explorer", product.name));
            }

            if product.least > product.version {
                return Some(format!("{}'s least version {} is above its newest, {}", product.name, product.least, product.version));
            }

            if product.files.windows(2).any(|pair| pair[0].target >= pair[1].target) {
                return Some(format!("{}'s files go in the order of their targets, each once", product.name));
            }
        }

        None
    }

    #[document(parent = "Release", summary = "As JSON, as the manifest file and GET /release carry it.")]
    pub fn to_json(&self) -> Value {
        json!({
            "issued": self.issued,
            "products": self.products.iter().map(|product| json!({
                "name": product.name,
                "version": product.version.to_string(),
                "least": product.least.to_string(),
                "notes": product.notes,
                "files": product.files.iter().map(|file| json!({
                    "target": file.target,
                    "url": file.url,
                    "sha256": to_hex(&file.sha256),
                    "size": file.size,
                })).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
            "breaking": self.breaking.iter().map(|breaking| json!({
                "chain": to_hex(&breaking.chain),
                "least": breaking.upgrade.least.to_string(),
                "from": breaking.upgrade.from,
            })).collect::<Vec<_>>(),
        })
    }

    #[document(parent = "Release", summary = "Reads the JSON to_json writes.")]
    pub fn from_json(value: &Value) -> Result<Release, String> {
        let text = |value: &Value, what: &str| value.as_str().map(str::to_string).ok_or_else(|| format!("the release's {what} is not text"));
        let number = |value: &Value, what: &str| value.as_u64().ok_or_else(|| format!("the release's {what} is not a whole number"));
        let version = |value: &Value, what: &str| text(value, what)?.parse::<Version>();
        let list = |value: &Value, what: &str| value.as_array().cloned().ok_or_else(|| format!("the release's {what} is not a list"));
        let mut products = Vec::new();

        for product in list(&value["products"], "products")? {
            let mut files = Vec::new();

            for file in list(&product["files"], "files")? {
                let sha256 = from_hex(&text(&file["sha256"], "sha256")?)
                    .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
                    .ok_or("a file's sha256 is not 32 bytes of hex")?;
                files.push(ReleaseFile {
                    target: text(&file["target"], "target")?,
                    url: text(&file["url"], "url")?,
                    sha256,
                    size: number(&file["size"], "size")?,
                });
            }

            products.push(Product {
                name: text(&product["name"], "product name")?,
                version: version(&product["version"], "version")?,
                least: version(&product["least"], "least version")?,
                notes: product["notes"].as_str().unwrap_or_default().to_string(),
                files,
            });
        }

        let mut breaking = Vec::new();

        for entry in value["breaking"].as_array().cloned().unwrap_or_default() {
            breaking.push(Breaking {
                chain: from_hex(&text(&entry["chain"], "chain")?).ok_or("an upgrade's chain is not hex")?,
                upgrade: Upgrade { least: version(&entry["least"], "least version")?, from: number(&entry["from"], "height")? },
            });
        }

        Ok(Release { issued: number(&value["issued"], "issued")?, products, breaking })
    }
}

#[document(
    summary = "The release keys a build trusts and how many must sign: release/keys.json, or the file CHAINV4_RELEASE_KEYS named when it was built, with where its node looks for releases. No keys turns the release side off: nodes fetch nothing and the apps show nothing.",
    category = "Releases",
    see_also = "ReleaseKeys::built_in, SignedRelease::verify",
)]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReleaseKeys {
    #[document(summary = "How many of the keys must sign.")]
    pub threshold: usize,
    #[document(summary = "The keys, each counted once.")]
    pub keys: Vec<PublicKey>,
    #[document(summary = "Where nodes fetch the manifest from unless --updates names another.")]
    pub updates: Option<String>,
}

impl ReleaseKeys {
    #[document(parent = "ReleaseKeys", summary = "The keys built into this binary.", returns = "The keys, or why the file it was built with is not keys.")]
    pub fn built_in() -> Result<ReleaseKeys, String> {
        ReleaseKeys::from_json(&serde_json::from_str(BUILT_IN_KEYS).map_err(|err| format!("the release keys built in are not JSON: {err}"))?)
    }

    #[document(parent = "ReleaseKeys", summary = "Reads keys.json: {\"threshold\":2,\"keys\":[\"ed25519:0x…\",…],\"updates\":\"https://…/manifest.json\"}. A threshold of 0, or above the number of keys, is refused, and so is a key named twice.")]
    pub fn from_json(value: &Value) -> Result<ReleaseKeys, String> {
        let mut keys = Vec::new();

        for key in value["keys"].as_array().cloned().unwrap_or_default() {
            let text = key.as_str().ok_or("each release key is text, such as ed25519:0x…")?;
            let key: PublicKey = text.parse().map_err(|_| format!("{text} is not a public key"))?;

            if keys.contains(&key) {
                return Err(format!("the release key {text} is named twice"));
            }

            keys.push(key);
        }

        let threshold = value["threshold"].as_u64().unwrap_or(0) as usize;

        if !keys.is_empty() && (threshold == 0 || threshold > keys.len()) {
            return Err(format!("a threshold of {threshold} is not between 1 and the {} release keys", keys.len()));
        }

        Ok(ReleaseKeys { threshold, keys, updates: value["updates"].as_str().filter(|url| !url.is_empty()).map(str::to_string) })
    }

    #[document(parent = "ReleaseKeys", summary = "Whether there are any: without them there are no releases to trust.")]
    pub fn on(&self) -> bool {
        !self.keys.is_empty()
    }
}

#[document(
    summary = "Why a release is not taken.",
    category = "Releases",
    see_also = "SignedRelease::verify",
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReleaseError {
    #[document(summary = "This build trusts no release keys.")]
    NoKeys,
    #[document(summary = "Fewer of the keys signed it than the threshold.")]
    TooFewSignatures { valid: usize, threshold: usize },
    #[document(summary = "It is not a release, as Release::problem says.")]
    Malformed(String),
}

impl fmt::Display for ReleaseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReleaseError::NoKeys => write!(f, "this build trusts no release keys"),
            ReleaseError::TooFewSignatures { valid, threshold } => write!(f, "{valid} of the release keys signed it, and it takes {threshold}"),
            ReleaseError::Malformed(why) => write!(f, "not a release: {why}"),
        }
    }
}

impl std::error::Error for ReleaseError {}

#[document(
    summary = "A release with the signatures made over it, as the manifest file holds it.",
    category = "Releases",
    see_also = "Release, ReleaseKeys",
)]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SignedRelease {
    #[document(summary = "The release.")]
    pub release: Release,
    #[document(summary = "Each key's signature over Release::signing_bytes.")]
    pub signatures: Vec<(PublicKey, Signature)>,
}

impl SignedRelease {
    #[document(
        parent = "SignedRelease",
        summary = "Checks that at least the threshold of the trusted keys signed it, each counted once, and that it is a release.",
        returns = "How many of the keys signed it.",
    )]
    pub fn verify(&self, keys: &ReleaseKeys) -> Result<usize, ReleaseError> {
        if !keys.on() {
            return Err(ReleaseError::NoKeys);
        }

        if let Some(problem) = self.release.problem() {
            return Err(ReleaseError::Malformed(problem));
        }

        let bytes = self.release.signing_bytes();
        let valid = keys
            .keys
            .iter()
            .filter(|key| self.signatures.iter().any(|(signer, signature)| signer == *key && key.verify_signature(&bytes, signature).is_ok()))
            .count();

        match valid >= keys.threshold {
            true => Ok(valid),
            false => Err(ReleaseError::TooFewSignatures { valid, threshold: keys.threshold }),
        }
    }

    #[document(parent = "SignedRelease", summary = "Adds a key's signature, replacing one it made before.")]
    pub fn sign(&mut self, key: &crate::crypto::keypair::Keypair) {
        let signer = key.public();
        let signature = key.sign(&self.release.signing_bytes());
        self.signatures.retain(|(other, _)| *other != signer);
        self.signatures.push((signer, signature));
    }

    #[document(parent = "SignedRelease", summary = "As the manifest file's JSON: {\"release\":{…},\"signatures\":[{\"key\":\"ed25519:0x…\",\"signature\":\"0x…\"}]}.")]
    pub fn to_json(&self) -> Value {
        json!({
            "release": self.release.to_json(),
            "signatures": self.signatures.iter().map(|(key, signature)| json!({ "key": key.to_string(), "signature": signature.to_string() })).collect::<Vec<_>>(),
        })
    }

    #[document(parent = "SignedRelease", summary = "Reads the manifest file's JSON. Its signatures are not checked: verify does that.")]
    pub fn from_json(value: &Value) -> Result<SignedRelease, String> {
        let mut signatures = Vec::new();

        for entry in value["signatures"].as_array().cloned().unwrap_or_default() {
            let key: PublicKey = entry["key"].as_str().and_then(|text| text.parse().ok()).ok_or("a signature's key is not a public key")?;
            let bytes = entry["signature"].as_str().and_then(from_hex).and_then(|bytes| <[u8; 64]>::try_from(bytes).ok()).ok_or("a signature is not 64 bytes of hex")?;
            signatures.push((key, signature_for(&key, bytes)?));
        }

        Ok(SignedRelease { release: Release::from_json(&value["release"])?, signatures })
    }
}

/// A signature in the scheme of the key that made it.
fn signature_for(key: &PublicKey, bytes: [u8; 64]) -> Result<Signature, String> {
    match key {
        #[cfg(feature = "ed25519")]
        PublicKey::Ed25519(_) => Ok(Signature::Ed25519(bytes)),
        #[cfg(feature = "edblake")]
        PublicKey::EdBlake(_) => Ok(Signature::EdBlake(bytes)),
        #[cfg(feature = "secp256k1")]
        PublicKey::Secp256k1(_) => Ok(Signature::Secp256k1(bytes)),
        #[cfg(feature = "bitcoin")]
        PublicKey::Bitcoin(_) => Ok(Signature::Bitcoin(bytes)),
        #[allow(unreachable_patterns)]
        _ => Err("a release key of a scheme this build does not know".to_string()),
    }
}

#[document(summary = "The SHA-256 of some bytes: what a release file's sha256 is of.", category = "Releases", see_also = "ReleaseFile")]
pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    <sha2::Sha256 as sha2::Digest>::digest(bytes).into()
}

#[document(
    summary = "Puts a downloaded binary in place of one on disk, once its size and SHA-256 are the release's: written beside it as .new, given the old one's permissions, then renamed over it, so the file is never half written. A running program keeps the old one until it restarts. On Windows, which will not rename over a running program, the old one is moved aside to .old first.",
    category = "Releases",
    params = {
        file = "The release's file, whose size and SHA-256 the bytes must match.",
        bytes = "What was downloaded.",
        path = "The binary to replace, such as std::env::current_exe().",
    },
    see_also = "ReleaseFile",
)]
#[cfg(not(target_arch = "wasm32"))]
pub fn install(file: &ReleaseFile, bytes: &[u8], path: &std::path::Path) -> Result<(), String> {
    if bytes.len() as u64 != file.size || sha256(bytes) != file.sha256 {
        return Err(format!("the download from {} is not the binary the release signed", file.url));
    }

    let fresh = path.with_extension("new");
    std::fs::write(&fresh, bytes).map_err(|err| format!("could not write {}: {err}", fresh.display()))?;

    if let Ok(metadata) = std::fs::metadata(path) {
        std::fs::set_permissions(&fresh, metadata.permissions()).map_err(|err| format!("could not make {} runnable: {err}", fresh.display()))?;
    }

    #[cfg(windows)]
    {
        let old = path.with_extension("old");
        let _ = std::fs::remove_file(&old);
        std::fs::rename(path, &old).map_err(|err| format!("could not move {} aside: {err}", path.display()))?;
    }

    std::fs::rename(&fresh, path).map_err(|err| format!("could not put {} in place: {err}", path.display()))
}
