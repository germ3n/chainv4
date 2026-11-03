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
pub mod vm;
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
    #[document(summary = "The limits a contract keeps within, or None for genesis's contract_limits, ContractLimits::DEFAULT's when genesis names none.")]
    pub contract: Option<vm::ContractLimits>,
    #[document(summary = "The most bytes one transaction may take, encoded, Ethereum-signed ones' raw bytes; or None for genesis's max_tx_bytes, which unset bounds native transactions by the block alone and Ethereum-signed ones by ETHEREUM_TX_BYTES.")]
    pub max_tx_bytes: Option<u64>,
    #[document(summary = "The most members a native multisig may have, or None for genesis's multisig_members, MULTISIG_MEMBERS when genesis names none. A multisig made under a higher limit keeps its members; it only cannot add more.")]
    pub multisig_members: Option<u32>,
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

        if let Some(problem) = self.contract.as_ref().and_then(vm::ContractLimits::problem) {
            return Some(problem);
        }

        if self.max_tx_bytes.is_some_and(|bytes| !(MIN_TX_BYTES..=MAX_BLOCK_BYTES).contains(&bytes)) {
            return Some("max_tx_bytes must be 64 KiB to 6 MiB, 65536 to 6291456");
        }

        if self.multisig_members.is_some_and(|members| members == 0 || members as usize > crate::registry::multisig::MAX_MEMBERS) {
            return Some("multisig_members must be 1 to 20, CHECKMULTISIG's most, so every multisig keeps its Bitcoin address");
        }

        None
    }
}

#[document(
    summary = "The least max_tx_bytes may be, 64 KiB: room for a governance change or a multisig's bundle.",
    category = "Chain",
    see_also = "BlockLimits::max_tx_bytes",
)]
pub const MIN_TX_BYTES: u64 = 64 * 1024;

#[document(
    summary = "The most members a native multisig had before the limit was governed, and has on a chain whose genesis names no multisig_members.",
    category = "Chain",
    see_also = "BlockLimits::multisig_members, MAX_MEMBERS",
)]
pub const MULTISIG_MEMBERS: u32 = 20;

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
