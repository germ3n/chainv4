use {
    super::{Effect, Receipt},
    crate::crypto::hashable::Hashable,
    crate::crypto::merkle,
    crate::structs::address::Address,
    crate::structs::event::Event,
    crate::structs::hash::{Hash, HashType},
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
        TxReceipt {
            block,
            index,
            fuel_used: receipt.fuel_used,
            gas_used: receipt.gas_used,
            fee: receipt.fee,
            status: TxStatus::of(receipt),
            events: receipt.events.clone(),
            trace: receipt.trace.clone(),
        }
    }

    #[document(
        parent = "TxReceipt",
        summary = "What the block's receipts root commits to for it, given its transaction's id.",
    )]
    pub fn committed(&self, tx: Hash) -> CommittedReceipt {
        CommittedReceipt::new(tx, self.fuel_used, self.gas_used, self.fee, &self.status, &self.events)
    }
}

impl TxStatus {
    /// The stored form of what running a transaction did.
    fn of(receipt: &Receipt) -> TxStatus {
        match &receipt.result {
            Ok(Effect::Done) => TxStatus::Done,
            Ok(Effect::Deployed(address)) => TxStatus::Deployed(*address),
            Ok(Effect::Returned(values)) => TxStatus::Returned(values.clone()),
            Ok(Effect::Upgraded(address)) => TxStatus::Upgraded(*address),
            Err(failure) => TxStatus::Failed(failure.to_string()),
        }
    }
}

#[document(
    summary = "What a block's receipts root commits to for one transaction: its id, what it burned, used and paid, what it did and what it emitted. Not a failure's message, nor the trace, neither of which is consensus, so either can change wording or depth without splitting the chain.",
    category = "Chain",
    see_also = "receipts_root, TxReceipt::committed",
)]
#[derive(Clone, Debug, PartialEq, SchemaWrite, SchemaRead)]
pub struct CommittedReceipt {
    #[document(summary = "The transaction's id.")]
    pub tx: Hash,
    #[document(summary = "Fuel its contracts burned.")]
    pub fuel_used: u64,
    #[document(summary = "Gas it used.")]
    pub gas_used: u64,
    #[document(summary = "What its sender paid.")]
    #[wincode(with = "U256Le")]
    pub fee: u256,
    #[document(summary = "What it did.")]
    pub outcome: CommittedOutcome,
    #[document(summary = "What it emitted, in order.")]
    pub events: Vec<Event>,
}

impl Hashable for CommittedReceipt {}

#[document(
    summary = "What a transaction did, as the receipts root commits to it: a TxStatus without a failure's message.",
    category = "Chain",
    see_also = "CommittedReceipt",
)]
#[derive(Clone, Debug, PartialEq, SchemaWrite, SchemaRead)]
#[wincode(tag_encoding = "u8")]
pub enum CommittedOutcome {
    #[document(summary = "A transfer, mint or burn went through.")]
    #[wincode(tag = 0)]
    Done,
    #[document(summary = "A contract was deployed at this address.")]
    #[wincode(tag = 1)]
    Deployed(Address),
    #[document(summary = "A contract call returned these values.")]
    #[wincode(tag = 2)]
    Returned(Vec<TxCallArgumentType>),
    #[document(summary = "It ran but failed.")]
    #[wincode(tag = 3)]
    Failed,
    #[document(summary = "The contract at this address runs new code.")]
    #[wincode(tag = 4)]
    Upgraded(Address),
}

impl CommittedReceipt {
    fn new(tx: Hash, fuel_used: u64, gas_used: u64, fee: u256, status: &TxStatus, events: &[Event]) -> CommittedReceipt {
        let outcome = match status {
            TxStatus::Done => CommittedOutcome::Done,
            TxStatus::Deployed(address) => CommittedOutcome::Deployed(*address),
            TxStatus::Returned(values) => CommittedOutcome::Returned(values.clone()),
            TxStatus::Failed(_) => CommittedOutcome::Failed,
            TxStatus::Upgraded(address) => CommittedOutcome::Upgraded(*address),
        };

        CommittedReceipt {
            tx,
            fuel_used,
            gas_used,
            fee,
            outcome,
            events: events.to_vec(),
        }
    }

    #[document(
        parent = "CommittedReceipt",
        summary = "What the receipts root commits to for a receipt from running a block.",
    )]
    pub fn of(receipt: &Receipt) -> CommittedReceipt {
        CommittedReceipt::new(receipt.id, receipt.fuel_used, receipt.gas_used, receipt.fee, &TxStatus::of(receipt), &receipt.events)
    }

    #[document(
        parent = "CommittedReceipt",
        summary = "Its leaf in the receipts root: its encoding, hashed as a Merkle leaf.",
        see_also = "leaf",
    )]
    pub fn leaf(&self, hash: HashType) -> Hash {
        merkle::leaf(hash, &self.hashable_bytes_with_tag())
    }
}

#[document(
    summary = "A block's receipts root: the Merkle root, as RFC 6962 builds one, over its receipts' leaves in order; the zero hash for a block without transactions.",
    params = {
        hash = "The chain's hash.",
        receipts = "The block's receipts, in order.",
    },
    category = "Chain",
    see_also = "CommittedReceipt, root, audit_path",
)]
pub fn receipts_root(hash: HashType, receipts: &[CommittedReceipt]) -> Hash {
    let leaves: Vec<Hash> = receipts.iter().map(|receipt| receipt.leaf(hash)).collect();

    merkle::root(hash, &leaves)
}
