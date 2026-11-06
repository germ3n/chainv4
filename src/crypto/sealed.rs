//! Key files sealed under a passphrase: what `chainv4 keygen --passphrase` and `chainv4 key
//! encrypt` write, and what the node, the CLI, the wallet and the explorer open, asking for the
//! passphrase or reading it from a file. A plain key file stays one line, `scheme:seed`; a sealed
//! one is one line too:
//!
//! `chainv4-sealed-key:1:<scheme>:<salt>:<nonce>:<sealed seed>`, the last three in hex.
//!
//! The passphrase is stretched by Argon2id at 64 MiB and 3 passes (STRONG), and the 32-byte seed
//! sealed by XChaCha20-Poly1305 under it, with the version and the scheme as associated data, so
//! neither can be changed without the seal failing. The scheme stays readable, so a sealed file
//! still says what kind of key it holds. The wallet's vault stretches its password the same way.

use {
    super::keypair::Keypair,
    crate::structs::hex::{from_hex, to_hex},
    argon2::Argon2,
    chacha20poly1305::aead::{Aead, KeyInit, Payload},
    chacha20poly1305::{XChaCha20Poly1305, XNonce},
    r#macro::document,
    zeroize::{Zeroize, Zeroizing},
};

#[document(summary = "What a sealed key file's line starts with, before its version.", category = "Keys", see_also = "seal_key_file")]
pub const SEALED_KEY_PREFIX: &str = "chainv4-sealed-key";

/// The one version there is: Argon2id at STRONG, then XChaCha20-Poly1305.
const VERSION: &str = "1";

#[document(
    summary = "How a passphrase is stretched, for sealed key files and the wallet's vault: Argon2id, version 0x13, 65536 KiB, 3 passes, 1 lane, a 32-byte key. Pinned, never the crate's default, as what it stretches to must never move under what was sealed.",
    category = "Keys",
)]
pub const STRONG: (u32, u32, u32) = (65_536, 3, 1);

#[document(summary = "Argon2id at STRONG.", category = "Keys", see_also = "STRONG")]
pub fn strong_argon2() -> Argon2<'static> {
    let (memory, passes, lanes) = STRONG;
    let params = argon2::Params::new(memory, passes, lanes, Some(32)).expect("the pinned Argon2 parameters are valid");

    Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params)
}

#[document(
    summary = "Whether a key file's text is sealed under a passphrase, so opening it needs one.",
    category = "Keys",
    see_also = "open_key_file",
)]
pub fn is_sealed_key_file(text: &str) -> bool {
    text.trim().strip_prefix(SEALED_KEY_PREFIX).is_some_and(|rest| rest.starts_with(':'))
}

#[document(
    summary = "The scheme a key file holds, sealed or not, without opening it: ed25519, edblake, secp256k1 or bitcoin.",
    category = "Keys",
)]
pub fn key_file_scheme(text: &str) -> Option<&str> {
    let text = text.trim();

    match text.strip_prefix(SEALED_KEY_PREFIX).and_then(|rest| rest.strip_prefix(':')) {
        Some(rest) => rest.split(':').nth(1),
        None => text.split_once(':').map(|(scheme, _)| scheme),
    }
}

#[document(
    summary = "The 32-byte key a passphrase stretches to under a salt, at STRONG: Argon2id's 64 MiB of working memory is the caller's here, wiped once it is done, as the crate's own allocation would be dropped unwiped. The key is wiped as it drops.",
    category = "Keys",
    see_also = "STRONG",
)]
pub fn strong_stretch(passphrase: &[u8], salt: &[u8]) -> Result<Zeroizing<[u8; 32]>, String> {
    let argon2 = strong_argon2();
    let mut key = Zeroizing::new([0; 32]);
    let mut memory = vec![argon2::Block::default(); argon2.params().block_count()];
    let stretched = argon2.hash_password_into_with_memory(passphrase, salt, &mut *key, &mut memory).map_err(|err| format!("could not stretch the passphrase: {err}"));
    memory.zeroize();
    stretched?;

    Ok(key)
}

fn stretch(passphrase: &str, salt: &[u8]) -> Result<Zeroizing<[u8; 32]>, String> {
    strong_stretch(passphrase.as_bytes(), salt)
}

/// What the seal binds besides the seed: the version and the scheme.
fn bound(scheme: &str) -> Vec<u8> {
    format!("{SEALED_KEY_PREFIX}:{VERSION}:{scheme}").into_bytes()
}

#[document(
    summary = "A key file's line for this key, sealed under a passphrase, without its newline. An empty passphrase is refused: a plain key file says the same, honestly.",
    category = "Keys",
    returns = "The sealed line, or why it could not be made.",
    see_also = "open_key_file, Keypair::to_key_file",
)]
pub fn seal_key_file(keypair: &Keypair, passphrase: &str) -> Result<String, String> {
    if passphrase.is_empty() {
        return Err("a sealed key file needs a passphrase; leave it plain instead".to_string());
    }

    let plain = Zeroizing::new(keypair.to_key_file());
    let (scheme, seed) = plain.split_once(':').expect("a key file is scheme:seed");
    let seed = Zeroizing::new(from_hex(seed).expect("a key file's seed is hex"));
    let (mut salt, mut nonce) = ([0u8; 16], [0u8; 24]);
    getrandom::fill(&mut salt).map_err(|err| format!("no randomness for a salt: {err}"))?;
    getrandom::fill(&mut nonce).map_err(|err| format!("no randomness for a nonce: {err}"))?;
    let key = stretch(passphrase, &salt)?;
    let sealed = XChaCha20Poly1305::new_from_slice(&*key)
        .expect("a 32-byte key")
        .encrypt(&XNonce::from(nonce), Payload { msg: &seed, aad: &bound(scheme) })
        .map_err(|_| "sealing failed".to_string())?;

    Ok(format!("{SEALED_KEY_PREFIX}:{VERSION}:{scheme}:{}:{}:{}", to_hex(&salt), to_hex(&nonce), to_hex(&sealed)))
}

#[document(
    summary = "Opens a key file's text: a sealed one with its passphrase, a plain one as it is, whatever passphrase is given.",
    category = "Keys",
    returns = "The keypair; for a sealed file, wrong passphrase when the passphrase is not the one it was sealed under, or it was changed since.",
    see_also = "seal_key_file, Keypair::from_key_file",
)]
pub fn open_key_file(text: &str, passphrase: &str) -> Result<Keypair, String> {
    let text = text.trim();
    let Some(rest) = text.strip_prefix(SEALED_KEY_PREFIX).and_then(|rest| rest.strip_prefix(':')) else {
        return Keypair::from_key_file(text);
    };
    let parts: Vec<&str> = rest.split(':').collect();
    let [version, scheme, salt, nonce, sealed] = parts[..] else {
        return Err("a sealed key file that does not read".to_string());
    };

    if version != VERSION {
        return Err(format!("a sealed key file of version {version}, which this build does not know: update chainv4"));
    }

    let damaged = || "a sealed key file that does not read".to_string();
    let salt = from_hex(salt).filter(|salt| salt.len() == 16).ok_or_else(damaged)?;
    let nonce: [u8; 24] = from_hex(nonce).and_then(|nonce| nonce.try_into().ok()).ok_or_else(damaged)?;
    let sealed = from_hex(sealed).ok_or_else(damaged)?;
    let key = stretch(passphrase, &salt)?;
    let seed = Zeroizing::new(
        XChaCha20Poly1305::new_from_slice(&*key)
            .expect("a 32-byte key")
            .decrypt(&XNonce::from(nonce), Payload { msg: &sealed, aad: &bound(scheme) })
            .map_err(|_| "wrong passphrase".to_string())?,
    );

    // Every copy of the seed in hex is one to wipe: the hex itself, and the line made of it.
    let hex = Zeroizing::new(to_hex(&seed));
    let mut line = Zeroizing::new(String::with_capacity(scheme.len() + 1 + hex.len()));
    line.push_str(scheme);
    line.push(':');
    line.push_str(&hex);

    Keypair::from_key_file(&line)
}

#[cfg(test)]
mod tests {
    use {super::*, crate::crypto::keypair::KeyType};

    #[test]
    fn a_sealed_key_file_opens_with_its_passphrase_alone_and_says_its_scheme() {
        let key = Keypair::from_seed([9; 32]);
        let sealed = seal_key_file(&key, "correct horse").unwrap();

        assert!(is_sealed_key_file(&sealed) && !is_sealed_key_file(&key.to_key_file()));
        assert_eq!(key_file_scheme(&sealed), key_file_scheme(&key.to_key_file()));
        assert!(!sealed.contains(&key.to_key_file().split_once(':').unwrap().1[2..]), "the seed is not in it");
        assert_eq!(open_key_file(&format!("{sealed}\n"), "correct horse").unwrap().public(), key.public());
        assert_eq!(open_key_file(&sealed, "correct horsf").err().as_deref(), Some("wrong passphrase"));
        assert_ne!(seal_key_file(&key, "correct horse").unwrap(), sealed, "a fresh salt and nonce each time");
        assert!(seal_key_file(&key, "").is_err());

        // A plain file opens whatever passphrase is given.
        assert_eq!(open_key_file(&key.to_key_file(), "anything").unwrap().public(), key.public());

        // The scheme is bound: written as another, it fails rather than open as the wrong kind.
        let other = KeyType::ALL.iter().map(|kind| kind.name().to_string()).find(|kind| Some(kind.as_str()) != key_file_scheme(&sealed)).unwrap();
        let swapped = sealed.replacen(&format!(":{}:", key_file_scheme(&sealed).unwrap()), &format!(":{other}:"), 1);
        assert!(open_key_file(&swapped, "correct horse").is_err());
        assert!(open_key_file(&sealed.replacen(":1:", ":2:", 1), "correct horse").err().unwrap().contains("version 2"));
        assert!(open_key_file(&sealed[..sealed.len() - 2], "correct horse").is_err());
    }

    #[test]
    fn the_strong_stretch_is_pinned() {
        let mut key = [0u8; 32];
        strong_argon2().hash_password_into(b"a password", &[7; 16], &mut key).unwrap();

        // Fixed, so a bump of the argon2 crate cannot move what every sealed file opens with; and
        // the stretch with memory of its own comes to the same.
        assert_eq!(to_hex(&key), PINNED);
        assert_eq!(*strong_stretch(b"a password", &[7; 16]).unwrap(), key);
    }

    const PINNED: &str = "0x4ed2c74c9718b8e63a177145f3e563ce64299f2496fa6bdad14ff673e4b287cd";
}
