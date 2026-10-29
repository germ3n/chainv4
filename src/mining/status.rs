//! What a worker shows of itself, and the work it is handed: shared by chainv4 worker and the apps
//! that mine, natively or in a browser.

use {
    crate::structs::hash::{Hash, HashType},
    r#macro::document,
};

#[document(
    summary = "What a worker mines for, as the node said once it connected.",
    category = "Mining",
    see_also = "WorkerStatus",
)]
#[derive(Clone, Debug, PartialEq)]
pub enum Mode {
    #[document(summary = "The node itself: its blocks pay its own reward address.")]
    Node,
    #[document(summary = "A pool, which keeps fee (in parts of a billion) of each block and splits the rest by the last window blocks' worth of shares.")]
    Pool { fee: u64, window: u64 },
    #[document(summary = "Alone: each block the worker finds pays its payout in full.")]
    Solo,
}

#[document(
    summary = "How a worker is doing, for an app to show: whether it is connected and to what, the work it has, what the node took and refused, and a pool's word on its standing.",
    category = "Mining",
    see_also = "Worker, Mode",
)]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorkerStatus {
    #[document(summary = "Whether it is past the handshake with the node now.")]
    pub connected: bool,
    #[document(summary = "What it mines for; None before the node said.")]
    pub mode: Option<Mode>,
    #[document(summary = "The jobs the node handed it.")]
    pub jobs: u64,
    #[document(summary = "The block difficulty of the job now.")]
    pub difficulty: Option<u128>,
    #[document(summary = "A pool's share difficulty for this worker now.")]
    pub share_difficulty: Option<u128>,
    #[document(summary = "The shares the pool took.")]
    pub shares: u64,
    #[document(summary = "The blocks it found that the node took.")]
    pub blocks: u64,
    #[document(summary = "The nonces the node refused, mostly for a block found a moment before.")]
    pub refused: u64,
    #[document(summary = "A pool's last standing message: its work and shares in the window, its parts of the next block, all it earned, and its claim.")]
    pub standing: Option<serde_json::Value>,
    #[document(summary = "Why the last connection failed or ended, or why the node refused it; cleared once connected again.")]
    pub trouble: Option<String>,
}

#[document(
    summary = "A work message from the node, read: the job, the block id to find a nonce for and its difficulty, the work hash, where the worker's nonces start, and a pool's share difficulty.",
    category = "Mining",
    see_also = "WorkerStatus",
)]
pub struct Work {
    #[document(summary = "The job's number, which a found message names.")]
    pub job: u64,
    #[document(summary = "The block id to find a nonce for.")]
    pub id: Hash,
    #[document(summary = "The difficulty the block's work hash must meet.")]
    pub difficulty: u128,
    #[document(summary = "The chain's work hash.")]
    pub hash: HashType,
    #[document(summary = "The first nonce of this worker's range.")]
    pub start: u64,
    #[document(summary = "A pool's share difficulty for this worker; None from a node that runs no pool, or mining alone.")]
    pub share: Option<u128>,
}

impl Work {
    #[document(parent = "Work", summary = "Reads a work message; None for one that does not read.")]
    pub fn read(message: &serde_json::Value) -> Option<Work> {
        let chain: HashType = message["chain"].as_str()?.parse().ok()?;
        let id = Hash::from_bytes(chain, &crate::structs::hex::from_hex(message["id"].as_str()?)?)?;

        Some(Work {
            job: message["job"].as_u64()?,
            id,
            difficulty: message["difficulty"].as_str()?.parse().ok()?,
            hash: message["hash"].as_str()?.parse().ok()?,
            start: message["start"].as_u64()?,
            share: message["share"].as_str().and_then(|share| share.parse().ok()),
        })
    }
}

