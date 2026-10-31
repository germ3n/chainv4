use {
    crate::crypto::hashable::Hashable,
    crate::structs::address::Address,
    crate::structs::tx::TxCallArgumentType,
    r#macro::document,
    wincode::SchemaRead,
    wincode::SchemaWrite,
};

/// What an EVM log is called as an event: every log has this name, and its topics and data as its
/// values. Asking the node's index for any other spelling finds nothing, as names match by bytes.
pub const EVM_LOG: &str = "log";

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

impl Event {
    #[document(
        parent = "Event",
        summary = "An EVM log's topics and data, which the event holds as bytes, topics first and the data last. None for any other event, whatever it is called: only an EVM contract emits logs.",
    )]
    pub fn evm_log(&self) -> Option<(Vec<[u8; 32]>, &[u8])> {
        if !matches!(self.contract, Address::Evm(_)) || self.name != EVM_LOG {
            return None;
        }

        let (data, topics) = self.data.split_last()?;
        let TxCallArgumentType::Bytes(data) = data else {
            return None;
        };
        let topics = topics
            .iter()
            .map(|topic| match topic {
                TxCallArgumentType::Bytes(topic) => <[u8; 32]>::try_from(topic.as_slice()).ok(),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()?;

        Some((topics, data))
    }
}
