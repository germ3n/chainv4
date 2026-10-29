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
#[cfg(feature = "json")]
pub mod json;

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

#[document(
    summary = "Why a reward cannot be a chain's: a halving interval of 0.",
    category = "Chain",
    see_also = "Reward, Change::Reward",
)]
pub fn reward_problem(reward: &Reward) -> Option<&'static str> {
    (reward.halving_interval == Some(0)).then_some("a reward halves every 1 block or more, or never")
}

impl BlockLimits {
    #[document(
        parent = "BlockLimits",
        summary = "Why they cannot be a chain's: fuel below 100,000 or memory below 1 MiB, which no contract call fits in, or max_block_bytes outside 1 to 6 MiB, as genesis's.",
    )]
    pub fn problem(&self) -> Option<&'static str> {
        if self.tx_limits.fuel < 100_000 || self.tx_limits.memory < 1024 * 1024 {
            return Some("a transaction needs at least 100,000 fuel and 1 MiB of memory");
        }

        if self.max_block_bytes.is_some_and(|bytes| !(MIN_BLOCK_BYTES..=MAX_BLOCK_BYTES).contains(&bytes)) {
            return Some("max_block_bytes must be 1 MiB to 6 MiB, 1048576 to 6291456");
        }

        None
    }
}

#[document(
    summary = "The least max_block_bytes a genesis may set, 1 MiB, so a block still holds the largest transaction there is: a deploy of a 512 KiB WebAssembly module.",
    category = "Chain",
    see_also = "Genesis::max_block_bytes",
)]
pub const MIN_BLOCK_BYTES: u64 = 1024 * 1024;

#[document(
    summary = "The most max_block_bytes a genesis may set, 6 MiB, so a full block, with its header and finality proofs, still fits one gossip message of 8 MiB.",
    category = "Chain",
    see_also = "Genesis::max_block_bytes",
)]
pub const MAX_BLOCK_BYTES: u64 = 6 * 1024 * 1024;
