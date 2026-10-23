use r#macro::document;

#[document(
    summary = "A quantity of the chain's currency.",
    category = "Primitives",
    see_also = "Tx",
)]
pub enum Amount {
    #[document(summary = "An amount held as an unsigned 128-bit integer.")]
    U128(u128),
}