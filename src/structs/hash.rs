use r#macro::document;

#[document(
    summary = "A 32-byte digest, tagged with the algorithm that produced it.",
    category = "Crypto",
    see_also = "Hashable",
)]
pub enum Hash {
    #[document(summary = "A SHA-256 digest.")]
    Sha256([u8; 32]),
    #[document(summary = "A 256-bit BLAKE3 digest.")]
    Blake3_256([u8; 32]),
    #[document(summary = "A BLAKE2s-256 digest.")]
    Blake2s256([u8; 32]),
    #[document(summary = "A BLAKE2b-256 digest.")]
    Blake2b256([u8; 32]),
}

#[document(
    summary = "The algorithm that produced a hash.",
    category = "Crypto",
    see_also = "Hash",
)]
pub enum HashType {
    #[document(summary = "A SHA-256 digest.")]
    Sha256,
    #[document(summary = "A 256-bit BLAKE3 digest.")]
    Blake3_256,
    #[document(summary = "A BLAKE2s-256 digest.")]
    Blake2s256,
    #[document(summary = "A BLAKE2b-256 digest.")]
    Blake2b256,
}