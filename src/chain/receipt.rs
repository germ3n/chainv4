use {
    super::{Effect, Receipt},
    crate::crypto::hashable::Hashable,
    crate::structs::address::Address,
    crate::structs::event::Event,
    crate::structs::hash::Hash,
    crate::structs::trace::Trace,
    crate::structs::tx::TxCallArgumentType,
    crate::structs::u256::{U256Le, u256},
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
    #[document(summary = "Gas it used, against the block's limit; 0 on a chain without fees.")]
    pub gas_used: u64,
    #[document(summary = "What its sender paid for it; 0 on a chain without fees.")]
    #[wincode(with = "U256Le")]
    pub fee: u256,
    #[document(summary = "What it did, or why it failed.")]
    pub status: TxStatus,
    #[document(summary = "What its contract calls emitted, in order. Empty when it failed.")]
    pub events: Vec<Event>,
    #[document(summary = "Where its contracts went: every call, delegate and transfer, failed ones included. Empty for a transaction that runs no contract.")]
    pub trace: Trace,
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
    #[document(summary = "The contract at this address runs new code.")]
    #[wincode(tag = 4)]
    Upgraded(Address),
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
            Ok(Effect::Upgraded(address)) => TxStatus::Upgraded(*address),
            Err(failure) => TxStatus::Failed(failure.to_string()),
        };

        TxReceipt {
            block,
            index,
            fuel_used: receipt.fuel_used,
            gas_used: receipt.gas_used,
            fee: receipt.fee,
            status,
            events: receipt.events.clone(),
            trace: receipt.trace.clone(),
        }
    }
}
