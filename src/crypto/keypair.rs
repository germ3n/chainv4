use {
    crate::crypto::hashable::Hashable,
    crate::crypto::verifiable::Verifiable,
    crate::crypto::verifiable::VerifyError,
    crate::structs::address::Address,
    crate::structs::hex::{from_hex, to_hex},
    ed25519_dalek::VerifyingKey,
    r#macro::document,
    std::fmt,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    zeroize::Zeroize,
};

#[cfg(feature = "ed25519")]
use ed25519_dalek::{Signer, SigningKey};

#[cfg(feature = "edblake")]
use crate::crypto::edblake;

#[cfg(feature = "secp256k1")]
use crate::crypto::secp256k1;

#[cfg(not(any(feature = "ed25519", feature = "edblake", feature = "secp256k1")))]
compile_error!("enable at least one key feature: ed25519, edblake or secp256k1");

#[document(
    summary = "A signature scheme.",
    category = "Keys",
    see_also = "Keypair, PublicKey, Signature",
    note = "Each scheme sits behind a cargo feature of the same name, ed25519, edblake and secp256k1, all on by default. Tags are pinned.",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
#[wincode(tag_encoding = "u8")]
pub enum KeyType {
    #[document(summary = "Standard Ed25519 (RFC 8032), hashing with SHA-512.")]
    #[cfg(feature = "ed25519")]
    #[wincode(tag = 0)]
    Ed25519,
    #[document(summary = "Ed25519 hashing with BLAKE3 read out to 64 bytes instead of SHA-512. Keys and signatures match chainv3's ed25519-dalek-blake3.")]
    #[cfg(feature = "edblake")]
    #[wincode(tag = 1)]
    EdBlake,
    #[document(summary = "Ethereum's: ECDSA on secp256k1 over keccak256, with a low s, and addresses of keccak256's last 20 bytes.")]
    #[cfg(feature = "secp256k1")]
    #[wincode(tag = 2)]
    Secp256k1,
}

impl KeyType {
    #[document(parent = "KeyType", summary = "Every scheme this build has.")]
    pub const ALL: &[KeyType] = &[
        #[cfg(feature = "ed25519")]
        KeyType::Ed25519,
        #[cfg(feature = "edblake")]
        KeyType::EdBlake,
        #[cfg(feature = "secp256k1")]
        KeyType::Secp256k1,
    ];

    #[document(parent = "KeyType", summary = "Its name, as key files and addresses begin: ed25519, edblake or secp256k1.")]
    pub fn name(self) -> &'static str {
        match self {
            #[cfg(feature = "ed25519")]
            KeyType::Ed25519 => "ed25519",
            #[cfg(feature = "edblake")]
            KeyType::EdBlake => "edblake",
            #[cfg(feature = "secp256k1")]
            KeyType::Secp256k1 => "secp256k1",
        }
    }
}

/// Reads a scheme's name.
impl std::str::FromStr for KeyType {
    type Err = String;

    fn from_str(text: &str) -> Result<KeyType, String> {
        KeyType::ALL
            .iter()
            .copied()
            .find(|scheme| scheme.name() == text)
            .ok_or_else(|| format!("unknown scheme {text}: this build has {}", KeyType::ALL.iter().map(|scheme| scheme.name()).collect::<Vec<_>>().join(", ")))
    }
}

#[document(
    summary = "A public key, tagged with its scheme. Anyone can check a signature against it.",
    category = "Keys",
    see_also = "Keypair, Signature",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, std::hash::Hash, SchemaWrite, SchemaRead)]
#[wincode(tag_encoding = "u8")]
pub enum PublicKey {
    #[document(summary = "An Ed25519 public key.")]
    #[cfg(feature = "ed25519")]
    #[wincode(tag = 0)]
    Ed25519([u8; 32]),
    #[document(summary = "An EdBlake public key.")]
    #[cfg(feature = "edblake")]
    #[wincode(tag = 1)]
    EdBlake([u8; 32]),
    #[document(summary = "A secp256k1 public key, compressed.")]
    #[cfg(feature = "secp256k1")]
    #[wincode(tag = 2)]
    Secp256k1([u8; 33]),
}

impl PublicKey {
    #[document(
        parent = "PublicKey",
        summary = "The account this key controls: under Ed25519 and EdBlake the key itself, under secp256k1 the last 20 bytes of the keccak256 of its point, as Ethereum's; each tagged with its scheme.",
    )]
    pub fn address(&self) -> Address {
        match self {
            #[cfg(feature = "ed25519")]
            PublicKey::Ed25519(key) => Address::Ed25519(*key),
            #[cfg(feature = "edblake")]
            PublicKey::EdBlake(key) => Address::EdBlake(*key),
            // A key that is no point has no account; it never verifies anything either.
            #[cfg(feature = "secp256k1")]
            PublicKey::Secp256k1(key) => Address::Secp256k1(secp256k1::address(key).unwrap_or_default()),
        }
    }

    #[document(
        parent = "PublicKey",
        summary = "The scheme this key belongs to.",
    )]
    pub fn key_type(&self) -> KeyType {
        match self {
            #[cfg(feature = "ed25519")]
            PublicKey::Ed25519(_) => KeyType::Ed25519,
            #[cfg(feature = "edblake")]
            PublicKey::EdBlake(_) => KeyType::EdBlake,
            #[cfg(feature = "secp256k1")]
            PublicKey::Secp256k1(_) => KeyType::Secp256k1,
        }
    }

    #[document(
        parent = "PublicKey",
        summary = "Checks that signature was made over message by this key's private key. Verification is strict: small-order keys and signature points, which let one signature pass for many messages, are rejected.",
        params = {
            message = "The bytes that were signed.",
            signature = "The signature to check. A signature from a different scheme never matches.",
        },
        returns = "Ok when the signature is valid.",
        see_also = "Keypair::sign",
    )]
    pub fn verify_signature(&self, message: &[u8], signature: &Signature) -> Result<(), VerifyError> {
        let valid = match (self, signature) {
            #[cfg(feature = "ed25519")]
            (PublicKey::Ed25519(key), Signature::Ed25519(signature)) => {
                let signature = ed25519_dalek::Signature::from_bytes(signature);

                VerifyingKey::from_bytes(key)
                    .and_then(|key| key.verify_strict(message, &signature))
                    .is_ok()
            }
            #[cfg(feature = "edblake")]
            (PublicKey::EdBlake(key), Signature::EdBlake(signature)) => {
                edblake::verify(key, message, signature)
            }
            #[cfg(feature = "secp256k1")]
            (PublicKey::Secp256k1(key), Signature::Secp256k1(signature)) => {
                secp256k1::verify(key, message, signature)
            }
            #[allow(unreachable_patterns)]
            _ => false,
        };

        if !valid {
            return Err(VerifyError::InvalidSignature);
        }

        Ok(())
    }
}

impl Verifiable for PublicKey {
    #[document(
        parent = "PublicKey",
        summary = "Checks that the key is a point on its curve, and for the Edwards schemes not a weak small-order point.",
        see_also = "VerifyError::InvalidPublicKey",
    )]
    fn verify(&self) -> Result<(), VerifyError> {
        match self {
            #[cfg(feature = "ed25519")]
            PublicKey::Ed25519(key) => curve_point(key),
            #[cfg(feature = "edblake")]
            PublicKey::EdBlake(key) => curve_point(key),
            #[cfg(feature = "secp256k1")]
            PublicKey::Secp256k1(key) => match secp256k1::valid_key(key) {
                true => Ok(()),
                false => Err(VerifyError::InvalidPublicKey),
            },
        }
    }
}

impl Hashable for PublicKey {
    const TAG_LEN: usize = 1;
}

/// The scheme, a colon and the key in hex: the same text as the key's address under the Edwards
/// schemes, such as ed25519:0x3b6a…, and the compressed point under secp256k1, which is 33 bytes
/// where its address is 20.
impl fmt::Display for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            #[cfg(feature = "secp256k1")]
            PublicKey::Secp256k1(key) => write!(f, "secp256k1:{}", to_hex(key)),
            #[allow(unreachable_patterns)]
            _ => self.address().fmt(f),
        }
    }
}

/// Reads the text Display writes. A contract address, or a secp256k1 address, is not a key.
impl std::str::FromStr for PublicKey {
    type Err = crate::structs::address::ParseAddressError;

    fn from_str(text: &str) -> Result<PublicKey, Self::Err> {
        #[cfg(feature = "secp256k1")]
        if let Some(hex) = text.strip_prefix("secp256k1:")
            && let Some(key) = from_hex(hex).and_then(|bytes| <[u8; 33]>::try_from(bytes).ok())
        {
            return Ok(PublicKey::Secp256k1(key));
        }

        match text.parse::<Address>()? {
            #[cfg(feature = "ed25519")]
            Address::Ed25519(key) => Ok(PublicKey::Ed25519(key)),
            #[cfg(feature = "edblake")]
            Address::EdBlake(key) => Ok(PublicKey::EdBlake(key)),
            _ => Err(crate::structs::address::ParseAddressError),
        }
    }
}

/// Both schemes put their keys on the same curve, and so does an address.
pub(crate) fn curve_point(key: &[u8; 32]) -> Result<(), VerifyError> {
    match VerifyingKey::from_bytes(key) {
        Ok(key) if !key.is_weak() => Ok(()),
        _ => Err(VerifyError::InvalidPublicKey),
    }
}

#[document(
    summary = "A private key: the 32-byte seed everything else about the key is derived from. It is wiped from memory when dropped and has no Debug output, so it cannot end up in a log.",
    category = "Keys",
    see_also = "Keypair, PublicKey",
)]
#[derive(Clone, SchemaWrite, SchemaRead)]
#[wincode(tag_encoding = "u8")]
pub enum PrivateKey {
    #[document(summary = "An Ed25519 seed.")]
    #[cfg(feature = "ed25519")]
    #[wincode(tag = 0)]
    Ed25519([u8; 32]),
    #[document(summary = "An EdBlake seed.")]
    #[cfg(feature = "edblake")]
    #[wincode(tag = 1)]
    EdBlake([u8; 32]),
    #[document(summary = "A secp256k1 scalar: above zero, and below the group's order.")]
    #[cfg(feature = "secp256k1")]
    #[wincode(tag = 2)]
    Secp256k1([u8; 32]),
}

impl PrivateKey {
    #[document(
        parent = "PrivateKey",
        summary = "The scheme this key belongs to.",
    )]
    pub fn key_type(&self) -> KeyType {
        match self {
            #[cfg(feature = "ed25519")]
            PrivateKey::Ed25519(_) => KeyType::Ed25519,
            #[cfg(feature = "edblake")]
            PrivateKey::EdBlake(_) => KeyType::EdBlake,
            #[cfg(feature = "secp256k1")]
            PrivateKey::Secp256k1(_) => KeyType::Secp256k1,
        }
    }

    #[document(
        parent = "PrivateKey",
        summary = "Derives the public key.",
    )]
    pub fn public_key(&self) -> PublicKey {
        match self {
            #[cfg(feature = "ed25519")]
            PrivateKey::Ed25519(seed) => {
                PublicKey::Ed25519(SigningKey::from_bytes(seed).verifying_key().to_bytes())
            }
            #[cfg(feature = "edblake")]
            PrivateKey::EdBlake(seed) => PublicKey::EdBlake(edblake::public_key(seed)),
            #[cfg(feature = "secp256k1")]
            PrivateKey::Secp256k1(secret) => {
                PublicKey::Secp256k1(secp256k1::public_key(secret).expect("a secp256k1 private key is a valid scalar"))
            }
        }
    }

    #[document(
        parent = "PrivateKey",
        summary = "Signs message. Signing is deterministic: the same key and message always give the same signature.",
        params = { message = "The bytes to sign." },
        see_also = "PublicKey::verify_signature",
    )]
    pub fn sign(&self, message: &[u8]) -> Signature {
        match self {
            #[cfg(feature = "ed25519")]
            PrivateKey::Ed25519(seed) => {
                Signature::Ed25519(SigningKey::from_bytes(seed).sign(message).to_bytes())
            }
            #[cfg(feature = "edblake")]
            PrivateKey::EdBlake(seed) => Signature::EdBlake(edblake::sign(seed, message)),
            #[cfg(feature = "secp256k1")]
            PrivateKey::Secp256k1(secret) => Signature::Secp256k1(secp256k1::sign(secret, message)),
        }
    }
}

impl Drop for PrivateKey {
    fn drop(&mut self) {
        match self {
            #[cfg(feature = "ed25519")]
            PrivateKey::Ed25519(seed) => seed.zeroize(),
            #[cfg(feature = "edblake")]
            PrivateKey::EdBlake(seed) => seed.zeroize(),
            #[cfg(feature = "secp256k1")]
            PrivateKey::Secp256k1(secret) => secret.zeroize(),
        }
    }
}

#[document(
    summary = "A signature, tagged with the scheme that made it.",
    category = "Keys",
    see_also = "PublicKey::verify_signature, Keypair::sign",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, std::hash::Hash, SchemaWrite, SchemaRead)]
#[wincode(tag_encoding = "u8")]
pub enum Signature {
    #[document(summary = "An Ed25519 signature.")]
    #[cfg(feature = "ed25519")]
    #[wincode(tag = 0)]
    Ed25519([u8; 64]),
    #[document(summary = "An EdBlake signature.")]
    #[cfg(feature = "edblake")]
    #[wincode(tag = 1)]
    EdBlake([u8; 64]),
    #[document(summary = "A secp256k1 ECDSA signature: r, then s, which is always low.")]
    #[cfg(feature = "secp256k1")]
    #[wincode(tag = 2)]
    Secp256k1([u8; 64]),
}

impl Hashable for Signature {
    const TAG_LEN: usize = 1;
}

/// The 64 bytes in hex. The scheme is left out: it is always the signer's.
impl fmt::Display for Signature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            #[cfg(feature = "ed25519")]
            Signature::Ed25519(bytes) => f.write_str(&to_hex(bytes)),
            #[cfg(feature = "edblake")]
            Signature::EdBlake(bytes) => f.write_str(&to_hex(bytes)),
            #[cfg(feature = "secp256k1")]
            Signature::Secp256k1(bytes) => f.write_str(&to_hex(bytes)),
        }
    }
}

impl Hashable for KeyType {
    const TAG_LEN: usize = 1;
}

#[document(
    summary = "A private key together with its public key, ready to sign.",
    category = "Keys",
    see_also = "PrivateKey, PublicKey, Signature",
)]
#[derive(Clone)]
pub struct Keypair {
    private: PrivateKey,
    public: PublicKey,
}

#[document(
    summary = "The well-known development key's seed. Public on purpose: anyone can run a dev network with it, and nothing of value should ever depend on it.",
    category = "Keys",
    see_also = "Keypair::development",
)]
pub const DEV_SEED: [u8; 32] = *b"chainv4 dev authority, not safe!";

impl Keypair {
    #[document(
        parent = "Keypair",
        summary = "The well-known development key: Ed25519 when built in, else EdBlake, else secp256k1. Never use it for anything of value: its seed is in the source.",
        see_also = "DEV_SEED, DevAuthority::dev_keypair, Keypair::from_seed",
    )]
    pub fn development() -> Keypair {
        Keypair::from_seed(DEV_SEED)
    }

    #[document(
        parent = "Keypair",
        summary = "A key from a fixed 32-byte seed, in the first scheme this build has: Ed25519, else EdBlake, else secp256k1. For the development key and tests, never for keys of value: a seed is a key only when it is secret and random.",
        params = { seed = "The seed. For secp256k1 it must be a valid secret: not zero, and below the group's order, as every seed of 32 equal bytes but 0x00 and 0xff is." },
        see_also = "Keypair::development, Keypair::generate",
    )]
    pub fn from_seed(seed: [u8; 32]) -> Keypair {
        let private = match KeyType::ALL[0] {
            #[cfg(feature = "ed25519")]
            KeyType::Ed25519 => PrivateKey::Ed25519(seed),
            #[cfg(feature = "edblake")]
            KeyType::EdBlake => PrivateKey::EdBlake(seed),
            #[cfg(feature = "secp256k1")]
            KeyType::Secp256k1 => PrivateKey::Secp256k1(seed),
        };

        Keypair::from_private(private)
    }

    #[document(
        parent = "Keypair",
        summary = "Makes a new key from the operating system's random number generator.",
        params = { key_type = "The scheme to use." },
        returns = "The keypair, or the error if the system could not supply randomness.",
    )]
    pub fn generate(key_type: KeyType) -> Result<Keypair, getrandom::Error> {
        let mut seed = [0; 32];
        getrandom::fill(&mut seed)?;

        // A secp256k1 key must be below the group's order: all but one in 2^128 draws are.
        #[cfg(feature = "secp256k1")]
        while key_type == KeyType::Secp256k1 && !secp256k1::valid_secret(&seed) {
            getrandom::fill(&mut seed)?;
        }

        let private = match key_type {
            #[cfg(feature = "ed25519")]
            KeyType::Ed25519 => PrivateKey::Ed25519(seed),
            #[cfg(feature = "edblake")]
            KeyType::EdBlake => PrivateKey::EdBlake(seed),
            #[cfg(feature = "secp256k1")]
            KeyType::Secp256k1 => PrivateKey::Secp256k1(seed),
        };
        seed.zeroize();

        Ok(Keypair::from_private(private))
    }

    #[document(
        parent = "Keypair",
        summary = "Rebuilds a keypair from a stored private key.",
        params = { private = "The private key." },
    )]
    pub fn from_private(private: PrivateKey) -> Keypair {
        let public = private.public_key();

        Keypair { private, public }
    }

    #[document(
        parent = "Keypair",
        summary = "Reads a key file: one line holding the scheme, a colon and the 32-byte seed in hex, as chainv4 keygen writes it.",
        returns = "The keypair, or what is wrong with the text.",
        see_also = "Keypair::to_key_file",
    )]
    pub fn from_key_file(text: &str) -> Result<Keypair, String> {
        let (scheme, seed) = text.trim().split_once(':').ok_or("not a key file")?;
        let mut seed: [u8; 32] = from_hex(seed)
            .and_then(|bytes| bytes.try_into().ok())
            .ok_or("it does not hold a 32-byte seed")?;
        let private = match scheme {
            #[cfg(feature = "ed25519")]
            "ed25519" => Ok(PrivateKey::Ed25519(seed)),
            #[cfg(feature = "edblake")]
            "edblake" => Ok(PrivateKey::EdBlake(seed)),
            #[cfg(feature = "secp256k1")]
            "secp256k1" if secp256k1::valid_secret(&seed) => Ok(PrivateKey::Secp256k1(seed)),
            #[cfg(feature = "secp256k1")]
            "secp256k1" => Err("it is not a secp256k1 key: zero, or past the group's order".to_string()),
            other => Err(format!("unknown scheme {other}")),
        };
        seed.zeroize();

        private.map(Keypair::from_private)
    }

    #[document(
        parent = "Keypair",
        summary = "The key file's line for this key, without its newline.",
        see_also = "Keypair::from_key_file",
    )]
    pub fn to_key_file(&self) -> String {
        let (scheme, seed) = match &self.private {
            #[cfg(feature = "ed25519")]
            PrivateKey::Ed25519(seed) => ("ed25519", seed),
            #[cfg(feature = "edblake")]
            PrivateKey::EdBlake(seed) => ("edblake", seed),
            #[cfg(feature = "secp256k1")]
            PrivateKey::Secp256k1(secret) => ("secp256k1", secret),
        };

        format!("{scheme}:{}", to_hex(seed))
    }

    #[document(
        parent = "Keypair",
        summary = "The private key, for storing it.",
    )]
    pub fn private(&self) -> &PrivateKey {
        &self.private
    }

    #[document(
        parent = "Keypair",
        summary = "The public key.",
    )]
    pub fn public(&self) -> PublicKey {
        self.public
    }

    #[document(
        parent = "Keypair",
        summary = "Signs message with the private key.",
        params = { message = "The bytes to sign." },
        see_also = "PublicKey::verify_signature",
    )]
    pub fn sign(&self, message: &[u8]) -> Signature {
        self.private.sign(message)
    }
}

#[cfg(test)]
mod tests {
    use {
        super::KeyType,
        super::Keypair,
        super::PrivateKey,
        super::Signature,
        crate::crypto::verifiable::Verifiable,
        crate::crypto::verifiable::VerifyError,
    };
    // Only the Edwards schemes' known-answer tests name a public key.
    #[cfg(any(feature = "ed25519", feature = "edblake"))]
    use super::PublicKey;

    /// Every scheme this build has.
    const KEY_TYPES: &[KeyType] = KeyType::ALL;

    #[cfg(any(feature = "ed25519", feature = "edblake"))]
    fn hex<const N: usize>(text: &str) -> [u8; N] {
        assert_eq!(text.len(), N * 2);
        let mut out = [0; N];

        for (index, byte) in out.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).unwrap();
        }

        out
    }

    /// The same signature with one bit flipped.
    fn tampered(signature: Signature) -> Signature {
        match signature {
            #[cfg(feature = "ed25519")]
            Signature::Ed25519(mut bytes) => {
                bytes[40] ^= 1;
                Signature::Ed25519(bytes)
            }
            #[cfg(feature = "edblake")]
            Signature::EdBlake(mut bytes) => {
                bytes[40] ^= 1;
                Signature::EdBlake(bytes)
            }
            #[cfg(feature = "secp256k1")]
            Signature::Secp256k1(mut bytes) => {
                bytes[40] ^= 1;
                Signature::Secp256k1(bytes)
            }
        }
    }

    #[cfg(feature = "ed25519")]
    #[test]
    fn ed25519_matches_rfc_8032() {
        // RFC 8032, section 7.1, test 1: the empty message.
        let keypair = Keypair::from_private(PrivateKey::Ed25519(hex(
            "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60",
        )));
        let signature = keypair.sign(b"");

        assert_eq!(
            keypair.public(),
            PublicKey::Ed25519(hex(
                "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a"
            ))
        );
        assert_eq!(
            signature,
            Signature::Ed25519(hex(
                "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b"
            ))
        );
        assert_eq!(keypair.public().verify_signature(b"", &signature), Ok(()));
    }

    #[cfg(feature = "edblake")]
    #[test]
    fn edblake_matches_chainv3() {
        // Made by chainv3's ed25519-dalek-blake3 fork from the same seed and message.
        let keypair = Keypair::from_private(PrivateKey::EdBlake([7; 32]));
        let signature = keypair.sign(b"chainv4 edblake");

        assert_eq!(
            keypair.public(),
            PublicKey::EdBlake(hex(
                "2e7254e05d0f02a56e282a7da19ee3acd70f71810ac6a78e48986d211320ad06"
            ))
        );
        assert_eq!(
            signature,
            Signature::EdBlake(hex(
                "c2bd941dc2773a2ef7231d4d84c9698e7f0276bea936d8156cce9af7c9ec656a9a32ac7ce96f643351ab48bb236e88724b4eb84938807df1b4c9ec85ae7c0805"
            ))
        );
        assert_eq!(
            keypair.public().verify_signature(b"chainv4 edblake", &signature),
            Ok(())
        );
    }

    #[test]
    fn signatures_only_match_their_key_and_message() {
        for &key_type in KEY_TYPES {
            let keypair = Keypair::generate(key_type).unwrap();
            let other = Keypair::generate(key_type).unwrap();
            let signature = keypair.sign(b"pay 5");
            let public = keypair.public();

            assert_eq!(public.verify_signature(b"pay 5", &signature), Ok(()));
            assert_eq!(
                public.verify_signature(b"pay 6", &signature),
                Err(VerifyError::InvalidSignature)
            );
            assert_eq!(
                other.public().verify_signature(b"pay 5", &signature),
                Err(VerifyError::InvalidSignature)
            );
            assert_eq!(
                public.verify_signature(b"pay 5", &tampered(signature)),
                Err(VerifyError::InvalidSignature)
            );
        }
    }

    #[cfg(all(feature = "ed25519", feature = "edblake"))]
    #[test]
    fn schemes_do_not_accept_each_other() {
        let ed25519 = Keypair::from_private(PrivateKey::Ed25519([9; 32]));
        let edblake = Keypair::from_private(PrivateKey::EdBlake([9; 32]));
        let signature = edblake.sign(b"message");

        assert_ne!(ed25519.public(), edblake.public());
        assert_eq!(
            ed25519.public().verify_signature(b"message", &signature),
            Err(VerifyError::InvalidSignature)
        );

        // Relabelling an EdBlake key and signature as Ed25519 fails too: the hash differs.
        let (PublicKey::EdBlake(key), Signature::EdBlake(bytes)) = (edblake.public(), signature)
        else {
            unreachable!()
        };

        assert_eq!(
            PublicKey::Ed25519(key).verify_signature(b"message", &Signature::Ed25519(bytes)),
            Err(VerifyError::InvalidSignature)
        );
    }

    #[test]
    fn generated_keys_are_valid_and_stored_keys_reload() {
        for &key_type in KEY_TYPES {
            let keypair = Keypair::generate(key_type).unwrap();
            let stored = wincode::serialize(keypair.private()).unwrap();
            let restored: PrivateKey = wincode::deserialize(&stored).unwrap();

            assert_eq!(keypair.public().key_type(), key_type);
            assert_eq!(keypair.public().verify(), Ok(()));
            assert_eq!(Keypair::from_private(restored).public(), keypair.public());
        }
    }
}
