use {
    super::{Effect, Failure, Receipt},
    crate::crypto::hashable::Hashable,
    crate::structs::address::Address,
    crate::structs::event::Event,
    crate::structs::hash::Hash,
    crate::structs::receipt::{CommittedOutcome, CommittedReceipt, Payment, StepEffect, StepOutcome},
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
    #[document(summary = "The coin its contracts paid, in order: what the receipts root commits to. Empty when it failed, and for receipts stored before payments were kept.")]
    pub payments: Vec<Payment>,
}

impl Hashable for TxReceipt {}

#[document(
    summary = "A receipt as nodes stored them before payments: what stored_receipt falls back to reading.",
    category = "Chain",
    see_also = "TxReceipt",
)]
#[derive(Clone, Debug, PartialEq, SchemaWrite, SchemaRead)]
pub struct LegacyTxReceipt {
    #[document(summary = "The block it landed in.")]
    pub block: Hash,
    #[document(summary = "Its place among the block's transactions.")]
    pub index: u32,
    #[document(summary = "Fuel its contracts burned.")]
    pub fuel_used: u64,
    #[document(summary = "Gas it used.")]
    pub gas_used: u64,
    #[document(summary = "What its sender paid.")]
    #[wincode(with = "U256Le")]
    pub fee: u256,
    #[document(summary = "What it did.")]
    pub status: TxStatus,
    #[document(summary = "What it emitted.")]
    pub events: Vec<Event>,
    #[document(summary = "Where its contracts went.")]
    pub trace: Trace,
}

impl From<LegacyTxReceipt> for TxReceipt {
    fn from(old: LegacyTxReceipt) -> TxReceipt {
        TxReceipt {
            block: old.block,
            index: old.index,
            fuel_used: old.fuel_used,
            gas_used: old.gas_used,
            fee: old.fee,
            status: old.status,
            events: old.events,
            trace: old.trace,
            payments: Vec::new(),
        }
    }
}

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
    #[document(summary = "A governance approval that was the last one needed: its change is decided, as the chain's decision-th and its kind's version-th.")]
    #[wincode(tag = 5)]
    Decided { decision: u64, kind: crate::governance::ChangeKind, version: u64 },
    #[document(summary = "A batch that went through: what each step did, in order.")]
    #[wincode(tag = 6)]
    Batch(Vec<StepOutcome>),
    #[document(summary = "A batch that failed, undone whole: which step stopped it, counted from 0, and why. It still used up its nonce.")]
    #[wincode(tag = 7)]
    BatchFailed { step: u32, why: String },
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
            payments: receipt.payments.clone(),
        }
    }

    #[document(
        parent = "TxReceipt",
        summary = "What the block's receipts root commits to for it, given its transaction's id.",
    )]
    pub fn committed(&self, tx: Hash) -> CommittedReceipt {
        CommittedReceipt::new(tx, self.fuel_used, self.gas_used, self.fee, outcome(&self.status), self.events.clone(), self.payments.clone())
    }
}

impl TxStatus {
    #[document(parent = "TxStatus", summary = "Whether the transaction failed: it undid all it did, and paid its fee.")]
    pub fn failed(&self) -> bool {
        matches!(self, TxStatus::Failed(_) | TxStatus::BatchFailed { .. })
    }

    #[document(parent = "TxStatus", summary = "Every contract it deployed: one, or each a batch's steps did.")]
    pub fn deployed(&self) -> Vec<Address> {
        match self {
            TxStatus::Deployed(address) => vec![*address],
            TxStatus::Batch(steps) => steps
                .iter()
                .filter_map(|step| match step.effect {
                    StepEffect::Deployed(address) => Some(address),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    /// The stored form of what running a transaction did.
    fn of(receipt: &Receipt) -> TxStatus {
        match &receipt.result {
            Ok(Effect::Done) => TxStatus::Done,
            Ok(Effect::Deployed(address)) => TxStatus::Deployed(*address),
            Ok(Effect::Returned(values)) => TxStatus::Returned(values.clone()),
            Ok(Effect::Upgraded(address)) => TxStatus::Upgraded(*address),
            Ok(Effect::Decided { decision, kind, version }) => TxStatus::Decided { decision: *decision, kind: *kind, version: *version },
            Ok(Effect::Batch(steps)) => TxStatus::Batch(steps.clone()),
            Err(Failure::Step { step, failure }) => TxStatus::BatchFailed { step: *step, why: failure.to_string() },
            Err(failure) => TxStatus::Failed(failure.to_string()),
        }
    }
}

impl CommittedReceipt {
    #[document(
        parent = "CommittedReceipt",
        summary = "What the receipts root commits to for a receipt from running a block.",
    )]
    pub fn of(receipt: &Receipt) -> CommittedReceipt {
        CommittedReceipt::new(receipt.id, receipt.fuel_used, receipt.gas_used, receipt.fee, outcome(&TxStatus::of(receipt)), receipt.events.clone(), receipt.payments.clone())
    }
}

/// What the receipts root commits to for a status: all of it but a failure's message.
fn outcome(status: &TxStatus) -> CommittedOutcome {
    match status {
        TxStatus::Done => CommittedOutcome::Done,
        TxStatus::Deployed(address) => CommittedOutcome::Deployed(*address),
        TxStatus::Returned(values) => CommittedOutcome::Returned(values.clone()),
        TxStatus::Failed(_) => CommittedOutcome::Failed,
        TxStatus::Upgraded(address) => CommittedOutcome::Upgraded(*address),
        TxStatus::Decided { decision, kind, version } => CommittedOutcome::Decided { decision: *decision, kind: *kind, version: *version },
        TxStatus::Batch(steps) => CommittedOutcome::Batch(steps.clone()),
        TxStatus::BatchFailed { step, .. } => CommittedOutcome::BatchFailed { step: *step },
    }
}
