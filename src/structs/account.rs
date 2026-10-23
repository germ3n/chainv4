use {
    crate::crypto::hashable::Hashable,
    crate::structs::u256::U256Le,
    crate::structs::u256::u256,
    r#macro::document,
    wincode::SchemaRead,
    wincode::SchemaWrite,
};

#[document(
    summary = "What the chain knows about an address. An address that was never written to has the default: nonce 0 and nothing in it.",
    category = "Primitives",
    see_also = "Accounts, Address",
)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct Account {
    #[document(summary = "How many transactions the address has sent. Each one must carry the next value, so none can be replayed.")]
    pub nonce: u64,
    #[document(summary = "The balance.")]
    #[wincode(with = "U256Le")]
    pub balance: u256,
}

impl Hashable for Account {}
