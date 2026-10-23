use {
    crate::crypto::hashable::Hashable,
    crate::crypto::verifiable::Verifiable,
    crate::crypto::verifiable::VerifyError,
    crate::structs::u256::U256Le,
    crate::structs::u256::u256,
    r#macro::document,
    wincode::SchemaRead,
    wincode::SchemaWrite,
};

#[document(
    summary = "A quantity of the chain's currency.",
    category = "Primitives",
    see_also = "Tx",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
#[wincode(tag_encoding = "u8")]
pub enum Amount {
    #[document(summary = "An amount held as an unsigned 128-bit integer.")]
    #[wincode(tag = 0)]
    U128(u128),
    #[document(summary = "An amount held as an unsigned 256-bit integer.")]
    #[wincode(tag = 1)]
    U256(#[wincode(with = "U256Le")] u256),
}

impl Verifiable for Amount {
    #[document(
        parent = "Amount",
        summary = "Every amount is valid by itself. Whether zero is allowed depends on the transaction, so Tx checks that.",
        see_also = "Tx::verify",
    )]
    fn verify(&self) -> Result<(), VerifyError> {
        match self {
            Amount::U128(_) | Amount::U256(_) => Ok(()),
        }
    }
}

impl Hashable for Amount {
    const TAG_LEN: usize = 1;
}
