//! A read-only call run over nothing but proven state: how an app gets a contract's answer of its
//! own rather than take a node's. The node sends what the call read, each entry with its proof at a
//! block (POST /call with "prove": true); the app checks each against that block's state root,
//! from a header its light client checked, then runs the call over those entries alone
//! (db::ProvenEntries). An entry the call reads that the node left out fails the run and is named,
//! so the app asks for it and runs again; a call that scans a range of keys cannot be proven so.

use {
    super::{CallContext, Limits, Outcome, Vm, VmError},
    crate::db::{Column, ProvenEntries, State, StateProof},
    crate::registry::Contracts,
    crate::structs::address::Address,
    crate::structs::hash::Hash,
    crate::structs::tx::TxCallArgumentType,
    r#macro::document,
    std::sync::Arc,
};

#[document(summary = "A state entry a node sent with a call's answer: its column, key, value or None for absent, and its proof.", category = "VM", see_also = "run_proven")]
#[derive(Clone, Debug)]
pub struct ProvenRead {
    #[document(summary = "Its column.")]
    pub column: Column,
    #[document(summary = "Its key.")]
    pub key: Vec<u8>,
    #[document(summary = "Its value, or None for none.")]
    pub value: Option<Vec<u8>>,
    #[document(summary = "Its proof against the block's state root.")]
    pub proof: StateProof,
}

#[document(summary = "Why a call could not be run over proven state alone.", category = "VM", see_also = "run_proven")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProvenRunError {
    #[document(summary = "A proof does not hold against the state root: the node sent state that is not the chain's.")]
    Invalid { column: &'static str, key: Vec<u8> },
    #[document(summary = "The call read this entry, which the node did not send: ask for its proof and run again.")]
    Missing { column: Column, key: Vec<u8> },
    #[document(summary = "The call scans a range of keys, which proofs of single entries do not cover.")]
    Scans,
}

impl std::fmt::Display for ProvenRunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProvenRunError::Invalid { column, key } => write!(f, "the proof of {column} {} does not hold: the node sent state that is not the chain's", crate::structs::hex::to_hex(key)),
            ProvenRunError::Missing { column, key } => write!(f, "the call reads {} {}, which the node did not prove", column.name(), crate::structs::hex::to_hex(key)),
            ProvenRunError::Scans => write!(f, "the call lists a range of storage, which cannot be proven entry by entry yet"),
        }
    }
}

#[document(
    summary = "Runs a read-only call over proven entries alone, once every proof holds against root: the answer the app shows as its own, whatever the node said its was.",
    params = {
        vm = "The app's own engines.",
        root = "The state root of the block the entries are proven at, from a header the light client checked.",
        reads = "What the node sent.",
        contract = "The contract called.",
        function = "Its function's selector, as transactions name it.",
        args = "Its arguments.",
        context = "The call's sender, the block it runs at, and what contracts run under there.",
        limits = "What it may use.",
    },
    returns = "Its outcome, as a node's dry run gives it; or why it could not be run so.",
    category = "VM",
    see_also = "ProvenRead, ProvenEntries",
)]
#[allow(clippy::too_many_arguments)]
pub fn run_proven(vm: &Arc<Vm>, root: &Hash, reads: &[ProvenRead], contract: &Address, function: &Hash, args: &[TxCallArgumentType], context: CallContext, limits: Limits) -> Result<Outcome, ProvenRunError> {
    for read in reads {
        if !read.proof.verify(root, read.column, &read.key, read.value.as_deref()) {
            return Err(ProvenRunError::Invalid { column: read.column.name(), key: read.key.clone() });
        }
    }

    let entries = Arc::new(ProvenEntries::new(reads.iter().map(|read| (read.column, read.key.clone(), read.value.clone()))));
    let state = Arc::new(State::new(entries.clone()));
    let missing = |entries: &ProvenEntries| match entries.missing() {
        Some((column, key)) => ProvenRunError::Missing { column, key },
        None => ProvenRunError::Scans,
    };

    let record = match Contracts::new(&state).get(contract) {
        Ok(Some(record)) => record,
        Ok(None) => return Ok(Outcome::failed(VmError::Runtime(format!("no contract at {contract}")))),
        Err(_) => return Err(missing(&entries)),
    };
    let outcome = vm.call(&state, &record, context, function, args, limits);

    if entries.scanned() {
        return Err(ProvenRunError::Scans);
    }

    if entries.missing().is_some() {
        return Err(missing(&entries));
    }

    Ok(outcome)
}
