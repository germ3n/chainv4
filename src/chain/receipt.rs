use {
    super::{Effect, Receipt},
    crate::crypto::hashable::Hashable,
    crate::structs::address::Address,
    crate::structs::event::Event,
    crate::structs::hash::Hash,
    crate::structs::tx::TxCallArgumentType,
    r#macro::document,
    wincode::{SchemaRead, SchemaWrite},
};

#[document(
    summary = "What the chain keeps about a transaction on the main chain: where it landed, what it did and what it emitted. Stored by transaction id; a reorg moves or removes it along with everything else.",
    category = "Chain",
    see_also = "Receipt, Event, Chain::receipt",
)]
#[derive(Clone, Debug, PartialEq, SchemaWrite, SchemaRead)]
pub struct TxReceipt {
    #[document(summary = "The block it landed in.")]
    pub block: Hash,
    #[document(summary = "Its place among the block's transactions.")]
    pub index: u32,
    #[document(summary = "Fuel a contract call or deploy burned; 0 for everything else.")]
    pub fuel_used: u64,
    #[document(summary = "What it did, or why it failed.")]
    pub status: TxStatus,
    #[document(summary = "What its contract calls emitted, in order. Empty when it failed.")]
    pub events: Vec<Event>,
}

impl Hashable for TxReceipt {}

#[document(
    summary = "What a transaction did, as stored. A failure keeps only its message.",
    category = "Chain",
    see_also = "TxReceipt",
)]
#[derive(Clone, Debug, PartialEq, SchemaWrite, SchemaRead)]
#[wincode(tag_encoding = "u8")]
pub enum TxStatus {
    #[document(summary = "A transfer, mint or burn went through.")]
    #[wincode(tag = 0)]
    Done,
    #[document(summary = "A contract was deployed at this address.")]
    #[wincode(tag = 1)]
    Deployed(Address),
    #[document(summary = "A contract call returned these values.")]
    #[wincode(tag = 2)]
    Returned(Vec<TxCallArgumentType>),
    #[document(summary = "It ran but failed. It still used up its nonce.")]
    #[wincode(tag = 3)]
    Failed(String),
}

impl TxReceipt {
    #[document(
        parent = "TxReceipt",
        summary = "The stored form of a receipt from running a block.",
    )]
    pub fn new(block: Hash, index: u32, receipt: &Receipt) -> TxReceipt {
        let status = match &receipt.result {
            Ok(Effect::Done) => TxStatus::Done,
            Ok(Effect::Deployed(address)) => TxStatus::Deployed(*address),
            Ok(Effect::Returned(values)) => TxStatus::Returned(values.clone()),
            Err(failure) => TxStatus::Failed(failure.to_string()),
        };

        TxReceipt {
            block,
            index,
            fuel_used: receipt.fuel_used,
            status,
            events: receipt.events.clone(),
        }
    }
}
