//! The settings a chain's genesis fixes and governance may replace: what a transaction and a block
//! may use, the block reward, the fees (fees.rs) and finality (finality.rs). Plain data, in every
//! build, so a light client decodes a change of any of them as a node does; what a node does with
//! them lives with what uses them, in chain and vm.

use {
    crate::crypto::hashable::Hashable,
    crate::structs::u256::{U256Le, u256},
    r#macro::document,
    wincode::{SchemaRead, SchemaWrite},
};

pub mod fees;
pub mod finality;

#[document(
    summary = "What one call may use. Running out of either stops the contract.",
    category = "VM",
    see_also = "ScriptEngine::run, Host::call",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct Limits {
    #[document(summary = "Instruction budget, charged per instruction by the engine and per byte by host operations that do bulk work. A call to another contract spends from the caller's budget.")]
    pub fuel: u64,
    #[document(summary = "Most bytes one call's engine may allocate. Each contract in a chain of calls gets its own.")]
    pub memory: usize,
}

#[document(
    summary = "What a transaction and a block may use: one deploy's or call's fuel and memory, and the most bytes a block's transactions may take. Genesis sets them, as tx_limits and max_block_bytes; governance replaces them with Change::Limits.",
    category = "Chain",
    see_also = "Change::Limits, Limits",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct BlockLimits {
    #[document(summary = "What one deploy or contract call may use.")]
    pub tx_limits: Limits,
    #[document(summary = "The most bytes a block's transactions may take, encoded, or None for no bound but the gas limit.")]
    pub max_block_bytes: Option<u64>,
}

impl Hashable for BlockLimits {}

#[document(
    summary = "What each block mints for its producer, credited after its transactions run. A fixed reward has no halving interval; with one, the reward halves every that many blocks until it reaches zero.",
    category = "Chain",
    see_also = "Genesis",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct Reward {
    #[document(summary = "The reward for the first blocks.")]
    #[wincode(with = "U256Le")]
    pub initial: u256,
    #[document(summary = "Blocks between halvings, or None for a reward that never changes.")]
    pub halving_interval: Option<u64>,
}

impl Hashable for Reward {}
