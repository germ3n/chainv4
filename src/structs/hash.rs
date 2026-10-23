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

#[document(
    summary = "A digest, tagged with the algorithm that produced it.",
    category = "Crypto",
    see_also = "Hashable, HashType",
    note = "Each algorithm sits behind a cargo feature: sha256, blake3, blake2s, blake2b, xxh3 and keccak256, all on by default. Tags are pinned, so turning one off never changes how the others encode.",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
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

impl Hashable for Hash {
    const TAG_LEN: usize = 1;
}

impl Hashable for HashType {
    const TAG_LEN: usize = 1;
}
