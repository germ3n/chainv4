//! What a block's receipts root commits to, outside the node so light clients can check a
//! receipt without one.

use {
    crate::crypto::hashable::Hashable,
    crate::crypto::merkle,
    crate::structs::address::Address,
    crate::structs::event::Event,
    crate::structs::hash::{Hash, HashType},
    crate::structs::tx::TxCallArgumentType,
    crate::structs::u256::{U256Le, u256},
    r#macro::document,
    wincode::{SchemaRead, SchemaWrite},
};

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
    #[document(summary = "The coin its contracts paid, in the order they paid it: what a contract sent from its own balance, with a call or a transfer, natively or in the EVM. Not what its sender sent, which the transaction says. Empty when it failed.")]
    pub payments: Vec<Payment>,
}

impl Hashable for CommittedReceipt {}

#[document(
    summary = "Coin a contract paid while a transaction ran, as the receipts root commits to it: so a wallet proves, line by line, what a contract paid it, as it proves a receipt.",
    category = "Chain",
    see_also = "CommittedReceipt",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct Payment {
    #[document(summary = "The contract it left.")]
    pub from: Address,
    #[document(summary = "Who got it: an account, or a contract.")]
    pub to: Address,
    #[document(summary = "How much, in the coin's smallest unit.")]
    #[wincode(with = "U256Le")]
    pub amount: u256,
}

impl Hashable for Payment {}

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
    #[document(summary = "A governance approval that was the last one needed: its change is decided, as the chain's decision-th, the count light clients follow without gaps, and its kind's version-th. What lets a light client prove a decision from the receipt alone.")]
    #[wincode(tag = 5)]
    Decided { decision: u64, kind: crate::governance::ChangeKind, version: u64 },
    #[document(summary = "A batch that went through: what each step did, in order.")]
    #[wincode(tag = 6)]
    Batch(Vec<StepOutcome>),
    #[document(summary = "A batch that failed, undone whole: which step stopped it, counted from 0. Why is not committed to, as no failure's message is.")]
    #[wincode(tag = 7)]
    BatchFailed { step: u32 },
}

#[document(
    summary = "What one step of a batch that went through did, and how much of the receipt's events and payments are its: each step's come after those of the steps before it, so the counts say where each starts.",
    category = "Chain",
    see_also = "CommittedOutcome, Tx::Batch",
)]
#[derive(Clone, Debug, PartialEq, SchemaWrite, SchemaRead)]
pub struct StepOutcome {
    #[document(summary = "What it did.")]
    pub effect: StepEffect,
    #[document(summary = "How many of the receipt's events it emitted.")]
    pub events: u32,
    #[document(summary = "How many of the receipt's payments its contracts made.")]
    pub payments: u32,
}

#[document(
    summary = "What a batch's step did: what a transaction a batch may hold can do.",
    category = "Chain",
    see_also = "StepOutcome",
)]
#[derive(Clone, Debug, PartialEq, SchemaWrite, SchemaRead)]
#[wincode(tag_encoding = "u8")]
pub enum StepEffect {
    #[document(summary = "A transfer, burn, name or claim went through.")]
    #[wincode(tag = 0)]
    Done,
    #[document(summary = "A contract was deployed at this address.")]
    #[wincode(tag = 1)]
    Deployed(Address),
    #[document(summary = "A contract call returned these values.")]
    #[wincode(tag = 2)]
    Returned(Vec<TxCallArgumentType>),
    #[document(summary = "The contract at this address runs new code.")]
    #[wincode(tag = 3)]
    Upgraded(Address),
}

impl StepEffect {
    #[document(parent = "StepEffect", summary = "The same thing as a whole transaction's outcome, for what describes one either way.")]
    pub fn outcome(&self) -> CommittedOutcome {
        match self {
            StepEffect::Done => CommittedOutcome::Done,
            StepEffect::Deployed(address) => CommittedOutcome::Deployed(*address),
            StepEffect::Returned(values) => CommittedOutcome::Returned(values.clone()),
            StepEffect::Upgraded(address) => CommittedOutcome::Upgraded(*address),
        }
    }
}

impl CommittedOutcome {
    #[document(parent = "CommittedOutcome", summary = "Whether the transaction failed: it undid all it did, and paid its fee.")]
    pub fn failed(&self) -> bool {
        matches!(self, CommittedOutcome::Failed | CommittedOutcome::BatchFailed { .. })
    }
}

impl CommittedReceipt {
    #[document(parent = "CommittedReceipt", summary = "A committed receipt from its parts.")]
    pub fn new(tx: Hash, fuel_used: u64, gas_used: u64, fee: u256, outcome: CommittedOutcome, events: Vec<Event>, payments: Vec<Payment>) -> CommittedReceipt {
        CommittedReceipt {
            tx,
            fuel_used,
            gas_used,
            fee,
            outcome,
            events,
            payments,
        }
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
