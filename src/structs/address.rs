use r#macro::document;

#[document(
    summary = "An account on the chain, named by the public key that controls it.",
    category = "Primitives",
    see_also = "Tx, Amount",
)]
pub enum Address {
    #[document(summary = "An address backed by a 32-byte Ed25519 public key.")]
    Ed25519([u8; 32]),
}