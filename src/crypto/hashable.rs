use {
    crate::structs::hash::Hash,
    r#macro::document,
    std::borrow::Cow,
};

#[document(
    summary = "A value with a canonical byte form that can be hashed.",
    category = "Crypto",
    see_also = "Hash",
)]
pub trait Hashable {
    #[document(
        parent = "Hashable",
        summary = "The digest of this value, normally taken over hashable_bytes.",
        returns = "The digest, tagged with its algorithm.",
        see_also = "Hashable::hashable_bytes",
    )]
    fn hash(&self) -> Hash;

    #[document(
        parent = "Hashable",
        summary = "The canonical bytes this value is hashed from.",
        returns = "Borrowed when the value already holds its bytes, such as a key or a digest; owned when they have to be built, such as an encoded transaction.",
    )]
    fn hashable_bytes(&self) -> Cow<'_, [u8]>;
}