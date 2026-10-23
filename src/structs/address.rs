use {
    crate::crypto::hashable::Hashable,
    crate::crypto::keypair::curve_point,
    crate::crypto::verifiable::Verifiable,
    crate::crypto::verifiable::VerifyError,
    r#macro::document,
    wincode::SchemaRead,
    wincode::SchemaWrite,
};

#[document(
    summary = "An account on the chain, named by the public key that controls it.",
    category = "Primitives",
    see_also = "Tx, Amount",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
#[wincode(tag_encoding = "u8")]
pub enum Address {
    #[document(summary = "An address backed by a 32-byte Ed25519 public key.")]
    #[wincode(tag = 0)]
    Ed25519([u8; 32]),
    #[document(
        summary = "A deployed contract. The bytes are derived from the deployer and their nonce, so no key controls it.",
        see_also = "Contracts::deploy",
    )]
    #[wincode(tag = 1)]
    Contract([u8; 32]),
}

impl Address {
    #[document(
        parent = "Address",
        summary = "The 32 bytes behind the address, without saying which kind it is.",
    )]
    pub fn bytes(&self) -> &[u8; 32] {
        match self {
            Address::Ed25519(bytes) | Address::Contract(bytes) => bytes,
        }
    }
}

impl Verifiable for Address {
    #[document(
        parent = "Address",
        summary = "Checks that an Ed25519 key is a point on the curve and not a weak small-order point, which would accept one signature for almost any message. A contract address is always valid.",
        see_also = "VerifyError::InvalidPublicKey",
    )]
    fn verify(&self) -> Result<(), VerifyError> {
        match self {
            Address::Ed25519(key) => curve_point(key),
            Address::Contract(_) => Ok(()),
        }
    }
}

impl Hashable for Address {
    const TAG_LEN: usize = 1;
}