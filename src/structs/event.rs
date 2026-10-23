use {
    crate::crypto::hashable::Hashable,
    crate::structs::address::Address,
    crate::structs::tx::TxCallArgumentType,
    r#macro::document,
    wincode::SchemaRead,
    wincode::SchemaWrite,
};

#[document(
    summary = "Something a contract announced while it ran, kept in its transaction's receipt for clients to follow. Events from a call that failed are dropped with everything else it did.",
    category = "Transactions",
    see_also = "TxReceipt",
)]
#[derive(Clone, Debug, PartialEq, SchemaWrite, SchemaRead)]
pub struct Event {
    #[document(summary = "The contract that emitted it.")]
    pub contract: Address,
    #[document(summary = "What happened, such as Transfer. At most 64 bytes.")]
    pub name: String,
    #[document(summary = "The details, as values a contract call could return.")]
    pub data: Vec<TxCallArgumentType>,
}

impl Hashable for Event {}
