use {
    crate::crypto::hashable::Hashable,
    crate::crypto::verifiable::Verifiable,
    crate::crypto::verifiable::VerifyError,
    r#macro::document,
    wincode::SchemaRead,
    wincode::SchemaWrite,
};

#[cfg(not(any(
    feature = "sha256",
    feature = "blake3",
    feature = "blake2s",
    feature = "blake2b",
    feature = "xxh3",
    feature = "keccak256",
)))]
compile_error!(
    "enable at least one hash feature: sha256, blake3, blake2s, blake2b, xxh3 or keccak256"
);

// Contracts and chains hash with a secure 256-bit algorithm the genesis picks; XXH3 alone
// only names functions.
#[cfg(all(
    any(feature = "vm", feature = "node"),
    not(any(
        feature = "sha256",
        feature = "blake3",
        feature = "blake2s",
        feature = "blake2b",
        feature = "keccak256",
    ))
))]
compile_error!(
    "contracts and nodes need a secure hash: enable sha256, blake3, blake2s, blake2b or keccak256"
);

#[document(
    summary = "A digest, tagged with the algorithm that produced it.",
    category = "Crypto",
    see_also = "Hashable, HashType",
    note = "Each algorithm sits behind a cargo feature: sha256, blake3, blake2s, blake2b, xxh3 and keccak256, all on by default. Tags are pinned, so turning one off never changes how the others encode.",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, std::hash::Hash, SchemaWrite, SchemaRead)]
#[wincode(tag_encoding = "u8")]
pub enum Hash {
    #[document(summary = "A SHA-256 digest.")]
    #[cfg(feature = "sha256")]
    #[wincode(tag = 0)]
    Sha256([u8; 32]),
    #[document(summary = "A 256-bit BLAKE3 digest.")]
    #[cfg(feature = "blake3")]
    #[wincode(tag = 1)]
    Blake3_256([u8; 32]),
    #[document(summary = "A BLAKE2s-256 digest.")]
    #[cfg(feature = "blake2s")]
    #[wincode(tag = 2)]
    Blake2s256([u8; 32]),
    #[document(summary = "A BLAKE2b-256 digest.")]
    #[cfg(feature = "blake2b")]
    #[wincode(tag = 3)]
    Blake2b256([u8; 32]),
    #[document(
        summary = "A 64-bit XXH3 hash. Fast but not cryptographic: meant for contract function selectors, never for anything an attacker could gain from forging.",
        see_also = "Hasher::digest",
    )]
    #[cfg(feature = "xxh3")]
    #[wincode(tag = 4)]
    Xxh3_64(u64),
    #[document(summary = "A Keccak-256 digest, the hash Ethereum uses. Not the same as SHA3-256, which pads its input differently.")]
    #[cfg(feature = "keccak256")]
    #[wincode(tag = 5)]
    Keccak256([u8; 32]),
}

impl Hash {
    #[document(
        parent = "Hash",
        summary = "The algorithm that produced this digest.",
    )]
    pub fn algorithm(&self) -> HashType {
        match self {
            #[cfg(feature = "sha256")]
            Hash::Sha256(_) => HashType::Sha256,
            #[cfg(feature = "blake3")]
            Hash::Blake3_256(_) => HashType::Blake3_256,
            #[cfg(feature = "blake2s")]
            Hash::Blake2s256(_) => HashType::Blake2s256,
            #[cfg(feature = "blake2b")]
            Hash::Blake2b256(_) => HashType::Blake2b256,
            #[cfg(feature = "xxh3")]
            Hash::Xxh3_64(_) => HashType::Xxh3_64,
            #[cfg(feature = "keccak256")]
            Hash::Keccak256(_) => HashType::Keccak256,
        }
    }

    #[document(
        parent = "Hash",
        summary = "The digest's bytes, without the algorithm: 32 of them, or 8 big-endian for XXH3, the order xxhsum prints.",
    )]
    pub fn to_bytes(&self) -> Vec<u8> {
        match self {
            #[cfg(feature = "sha256")]
            Hash::Sha256(bytes) => bytes.to_vec(),
            #[cfg(feature = "blake3")]
            Hash::Blake3_256(bytes) => bytes.to_vec(),
            #[cfg(feature = "blake2s")]
            Hash::Blake2s256(bytes) => bytes.to_vec(),
            #[cfg(feature = "blake2b")]
            Hash::Blake2b256(bytes) => bytes.to_vec(),
            #[cfg(feature = "xxh3")]
            Hash::Xxh3_64(value) => value.to_be_bytes().to_vec(),
            #[cfg(feature = "keccak256")]
            Hash::Keccak256(bytes) => bytes.to_vec(),
        }
    }
}

/// Hex with a 0x prefix, for logs and messages.
impl std::fmt::Display for Hash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "0x")?;

        for byte in self.to_bytes() {
            write!(f, "{byte:02x}")?;
        }

        Ok(())
    }
}

impl Verifiable for Hash {
    #[document(
        parent = "Hash",
        summary = "Every digest is valid by itself, since any bytes can be a hash. Which algorithm is allowed depends on where the hash is used, so its owner checks that.",
        see_also = "Tx::verify",
    )]
    fn verify(&self) -> Result<(), VerifyError> {
        Ok(())
    }
}

#[document(
    summary = "The algorithm that produced a hash.",
    category = "Crypto",
    see_also = "Hash",
    note = "Each algorithm sits behind the cargo feature of the same name as on Hash. Tags are pinned and match Hash's.",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, std::hash::Hash, SchemaWrite, SchemaRead)]
#[wincode(tag_encoding = "u8")]
pub enum HashType {
    #[document(summary = "A SHA-256 digest.")]
    #[cfg(feature = "sha256")]
    #[wincode(tag = 0)]
    Sha256,
    #[document(summary = "A 256-bit BLAKE3 digest.")]
    #[cfg(feature = "blake3")]
    #[wincode(tag = 1)]
    Blake3_256,
    #[document(summary = "A BLAKE2s-256 digest.")]
    #[cfg(feature = "blake2s")]
    #[wincode(tag = 2)]
    Blake2s256,
    #[document(summary = "A BLAKE2b-256 digest.")]
    #[cfg(feature = "blake2b")]
    #[wincode(tag = 3)]
    Blake2b256,
    #[document(summary = "A 64-bit XXH3 hash. Not cryptographic: for contract function selectors only.")]
    #[cfg(feature = "xxh3")]
    #[wincode(tag = 4)]
    Xxh3_64,
    #[document(summary = "A Keccak-256 digest, the hash Ethereum uses.")]
    #[cfg(feature = "keccak256")]
    #[wincode(tag = 5)]
    Keccak256,
}

impl HashType {
    #[document(
        parent = "HashType",
        summary = "Every algorithm this build has, in tag order.",
    )]
    pub const ALL: &'static [HashType] = &[
        #[cfg(feature = "sha256")]
        HashType::Sha256,
        #[cfg(feature = "blake3")]
        HashType::Blake3_256,
        #[cfg(feature = "blake2s")]
        HashType::Blake2s256,
        #[cfg(feature = "blake2b")]
        HashType::Blake2b256,
        #[cfg(feature = "xxh3")]
        HashType::Xxh3_64,
        #[cfg(feature = "keccak256")]
        HashType::Keccak256,
    ];

    #[document(
        parent = "HashType",
        summary = "The algorithm's short name, as genesis files, the API and the Lua hash library write it: sha256, blake3, blake2s, blake2b, xxh3 or keccak256.",
    )]
    pub fn name(self) -> &'static str {
        match self {
            #[cfg(feature = "sha256")]
            HashType::Sha256 => "sha256",
            #[cfg(feature = "blake3")]
            HashType::Blake3_256 => "blake3",
            #[cfg(feature = "blake2s")]
            HashType::Blake2s256 => "blake2s",
            #[cfg(feature = "blake2b")]
            HashType::Blake2b256 => "blake2b",
            #[cfg(feature = "xxh3")]
            HashType::Xxh3_64 => "xxh3",
            #[cfg(feature = "keccak256")]
            HashType::Keccak256 => "keccak256",
        }
    }

    #[document(
        parent = "HashType",
        summary = "How many bytes a digest has: 32, or 8 for XXH3.",
    )]
    pub fn digest_len(self) -> usize {
        match self {
            #[cfg(feature = "xxh3")]
            HashType::Xxh3_64 => 8,
            #[allow(unreachable_patterns)]
            _ => 32,
        }
    }

    #[document(
        parent = "HashType",
        summary = "Whether a chain can hash with it: 256 bits and cryptographic. Everything but XXH3.",
        see_also = "Genesis",
    )]
    pub fn secure(self) -> bool {
        self.digest_len() == 32
    }

    #[document(
        parent = "HashType",
        summary = "The chain hash development chains use: BLAKE3 when this build has it, otherwise the first other secure algorithm it has.",
    )]
    pub fn default_chain() -> HashType {
        #[cfg(feature = "blake3")]
        return HashType::Blake3_256;

        #[cfg(not(feature = "blake3"))]
        *HashType::ALL
            .iter()
            .find(|algorithm| algorithm.secure())
            .expect("a build with a chain has a secure hash")
    }
}

impl std::str::FromStr for HashType {
    type Err = String;

    fn from_str(name: &str) -> Result<HashType, String> {
        HashType::ALL
            .iter()
            .copied()
            .find(|algorithm| algorithm.name() == name)
            .ok_or_else(|| {
                let known: Vec<&str> = HashType::ALL.iter().map(|algorithm| algorithm.name()).collect();

                format!("unknown hash {name}; this build has {}", known.join(", "))
            })
    }
}

impl Hash {
    #[document(
        parent = "Hash",
        summary = "A digest made of zeros, the stand-in for the hash of nothing, such as genesis's parent.",
    )]
    pub fn zero(algorithm: HashType) -> Hash {
        Hash::from_bytes(algorithm, &vec![0; algorithm.digest_len()]).expect("the right length")
    }

    #[document(
        parent = "Hash",
        summary = "A digest from its bytes, as to_bytes gives them.",
        returns = "The hash, or None when the length does not fit the algorithm.",
    )]
    pub fn from_bytes(algorithm: HashType, bytes: &[u8]) -> Option<Hash> {
        let hash = match algorithm {
            #[cfg(feature = "sha256")]
            HashType::Sha256 => Hash::Sha256(bytes.try_into().ok()?),
            #[cfg(feature = "blake3")]
            HashType::Blake3_256 => Hash::Blake3_256(bytes.try_into().ok()?),
            #[cfg(feature = "blake2s")]
            HashType::Blake2s256 => Hash::Blake2s256(bytes.try_into().ok()?),
            #[cfg(feature = "blake2b")]
            HashType::Blake2b256 => Hash::Blake2b256(bytes.try_into().ok()?),
            #[cfg(feature = "xxh3")]
            HashType::Xxh3_64 => Hash::Xxh3_64(u64::from_be_bytes(bytes.try_into().ok()?)),
            #[cfg(feature = "keccak256")]
            HashType::Keccak256 => Hash::Keccak256(bytes.try_into().ok()?),
        };

        Some(hash)
    }
}

impl Hashable for Hash {
    const TAG_LEN: usize = 1;
}

impl Hashable for HashType {
    const TAG_LEN: usize = 1;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::hasher::Hasher;

    #[test]
    fn every_algorithm_round_trips_through_its_bytes_and_name() {
        for &algorithm in HashType::ALL {
            let digest = Hasher::digest(algorithm, b"abc");

            assert_eq!(Hash::from_bytes(algorithm, &digest.to_bytes()), Some(digest));
            assert_eq!(Hash::zero(algorithm).algorithm(), algorithm);
            assert_eq!(algorithm.name().parse::<HashType>(), Ok(algorithm));
            assert_eq!(digest.to_bytes().len(), algorithm.digest_len());
        }

        assert!(HashType::default_chain().secure());
        assert!("md5".parse::<HashType>().unwrap_err().contains("unknown hash md5"));
        #[cfg(feature = "xxh3")]
        assert!(!HashType::Xxh3_64.secure());
    }
}
