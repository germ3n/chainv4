use {
    crate::crypto::hashable::Hashable,
    crate::crypto::verifiable::Verifiable,
    crate::crypto::verifiable::VerifyError,
    ed25519_dalek::VerifyingKey,
    r#macro::document,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    zeroize::Zeroize,
};

#[cfg(feature = "ed25519")]
use ed25519_dalek::{Signer, SigningKey};

#[cfg(feature = "edblake")]
use crate::crypto::edblake;

#[cfg(not(any(feature = "ed25519", feature = "edblake")))]
compile_error!("enable at least one key feature: ed25519 or edblake");

#[document(
    summary = "A signature scheme.",
    category = "Keys",
    see_also = "Keypair, PublicKey, Signature",
    note = "Each scheme sits behind a cargo feature of the same name, ed25519 and edblake, both on by default. Tags are pinned.",
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
}

#[document(
    summary = "A public key, tagged with its scheme. Anyone can check a signature against it.",
    category = "Keys",
    see_also = "Keypair, Signature",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
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
}

impl PublicKey {
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
        summary = "Checks that the key is a point on the curve and not a weak small-order point.",
        see_also = "VerifyError::InvalidPublicKey",
    )]
    fn verify(&self) -> Result<(), VerifyError> {
        match self {
            #[cfg(feature = "ed25519")]
            PublicKey::Ed25519(key) => curve_point(key),
            #[cfg(feature = "edblake")]
            PublicKey::EdBlake(key) => curve_point(key),
        }
    }
}

impl Hashable for PublicKey {
    const TAG_LEN: usize = 1;
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
        }
    }
}

#[document(
    summary = "A signature, tagged with the scheme that made it.",
    category = "Keys",
    see_also = "PublicKey::verify_signature, Keypair::sign",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
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
}

impl Hashable for Signature {
    const TAG_LEN: usize = 1;
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

impl Keypair {
    #[document(
        parent = "Keypair",
        summary = "Makes a new key from the operating system's random number generator.",
        params = { key_type = "The scheme to use." },
        returns = "The keypair, or the error if the system could not supply randomness.",
    )]
    pub fn generate(key_type: KeyType) -> Result<Keypair, getrandom::Error> {
        let mut seed = [0; 32];
        getrandom::fill(&mut seed)?;
        let private = match key_type {
            #[cfg(feature = "ed25519")]
            KeyType::Ed25519 => PrivateKey::Ed25519(seed),
            #[cfg(feature = "edblake")]
            KeyType::EdBlake => PrivateKey::EdBlake(seed),
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
        super::PublicKey,
        super::Signature,
        crate::crypto::verifiable::Verifiable,
        crate::crypto::verifiable::VerifyError,
    };

    /// Every scheme this build has.
    const KEY_TYPES: &[KeyType] = &[
        #[cfg(feature = "ed25519")]
        KeyType::Ed25519,
        #[cfg(feature = "edblake")]
        KeyType::EdBlake,
    ];

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
