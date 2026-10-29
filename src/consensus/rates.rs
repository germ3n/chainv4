//! How mining is going, for the node's report and the explorer: each device's hashrate, measured
//! over the last stretch of a few seconds, and the workers mining for the node.

use {
    r#macro::document,
    std::sync::Mutex,
    std::sync::atomic::{AtomicU64, Ordering},
    std::time::{Duration, Instant},
};

/// How long a hashrate is measured over before it is taken again.
const STRETCH: Duration = Duration::from_secs(3);

#[document(
    summary = "A hashrate meter: miners add the nonces they tried, from any thread, and the rate is what they tried over the last few seconds.",
    category = "Consensus",
    see_also = "MiningReport",
)]
#[derive(Debug)]
pub struct Meter {
    tried: AtomicU64,
    /// When the rate was last taken, the count then, and the rate.
    taken: Mutex<(Instant, u64, f64)>,
}

impl Default for Meter {
    fn default() -> Meter {
        Meter {
            tried: AtomicU64::new(0),
            taken: Mutex::new((Instant::now(), 0, 0.0)),
        }
    }
}

impl Meter {
    #[document(parent = "Meter", summary = "Counts nonces tried.")]
    pub fn add(&self, nonces: u64) {
        self.tried.fetch_add(nonces, Ordering::Relaxed);
    }

    #[document(parent = "Meter", summary = "Every nonce tried so far.")]
    pub fn total(&self) -> u64 {
        self.tried.load(Ordering::Relaxed)
    }

    #[document(parent = "Meter", summary = "Hashes a second over the last stretch, taken again once it is a few seconds old.")]
    pub fn rate(&self) -> f64 {
        let mut taken = self.taken.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let elapsed = taken.0.elapsed();

        if elapsed >= STRETCH {
            let total = self.total();
            *taken = (Instant::now(), total, total.saturating_sub(taken.1) as f64 / elapsed.as_secs_f64());
        }

        taken.2
    }
}

#[document(summary = "One device mining: a GPU, or the CPU's threads, with its hashrate.", category = "Consensus", see_also = "MiningReport")]
#[derive(Clone, Debug, PartialEq)]
pub struct DeviceReport {
    #[document(summary = "Its name, such as \"Apple M3 Pro (Metal)\" or \"CPU, 4 threads\".")]
    pub name: String,
    #[document(summary = "Hashes a second, over the last few seconds.")]
    pub rate: f64,
}

#[document(summary = "A worker mining for the node over its mining port.", category = "Consensus", see_also = "MiningReport")]
#[derive(Clone, Debug, PartialEq)]
pub struct WorkerReport {
    #[document(summary = "The name it gave.")]
    pub name: String,
    #[document(summary = "Where it connected from.")]
    pub address: String,
    #[document(summary = "When it connected, in milliseconds since the Unix epoch.")]
    pub since: u64,
    #[document(summary = "Its devices and their hashrates, as it last said.")]
    pub devices: Vec<DeviceReport>,
    #[document(summary = "Seals it found that the node took.")]
    pub found: u64,
    #[document(summary = "Nonces it sent that did not meet the work, or came after the block was found.")]
    pub refused: u64,
    #[document(summary = "Where its share of a pool's blocks goes, as it said: None on a node that runs no pool.")]
    pub payout: Option<crate::structs::address::Address>,
    #[document(summary = "Shares the pool took from it.")]
    pub shares: u64,
    #[document(summary = "The difficulty its shares meet now, as the pool set it for its hashrate; 0 on a node that runs no pool.")]
    pub share_difficulty: u128,
    #[document(summary = "Whether it mines alone: each block it finds pays its payout in full.")]
    pub solo: bool,
}

#[document(summary = "One miner's standing in a pool: its work in the window the next block shares out by, that block's parts it would get, and all it has earned from the pool's blocks so far.", category = "Consensus", see_also = "PoolReport")]
#[derive(Clone, Debug, PartialEq)]
pub struct MinerStanding {
    #[document(summary = "Where its earnings go.")]
    pub to: crate::structs::address::Address,
    #[document(summary = "Its shares' difficulties added up, of those in the window.")]
    pub work: u128,
    #[document(summary = "Its shares in the window.")]
    pub shares: u64,
    #[document(summary = "Its parts of the next block, of PAYOUT_PARTS.")]
    pub parts: u32,
    #[document(summary = "All it has earned from the pool's blocks credited so far, which it claims from the pool's escrow less what it claimed.")]
    pub earned: crate::structs::u256::u256,
}

#[document(summary = "A block the pool's work sealed: each miner's parts of it, and whether it was credited.", category = "Consensus", see_also = "PoolReport")]
#[derive(Clone, Debug, PartialEq)]
pub struct PoolBlock {
    #[document(summary = "Its height.")]
    pub height: u64,
    #[document(summary = "Its id.")]
    pub id: crate::structs::hash::Hash,
    #[document(summary = "When it was sealed, in milliseconds since the Unix epoch.")]
    pub at: u64,
    #[document(summary = "Each miner's parts of its reward and fees, of PAYOUT_PARTS.")]
    pub payouts: Vec<crate::structs::block::Payout>,
    #[document(summary = "None while it may still be undone; then Some(true) once its miners were credited, or Some(false) for a block the chain dropped, which credits nobody.")]
    pub credited: Option<bool>,
}

#[document(
    summary = "Whether a pool's block can no longer be undone: Final once it is finalized, or deeper than the chain's max_reorg_depth, with what it shared out; Dropped once it can no longer be on the main chain; Pending until then.",
    category = "Consensus",
    see_also = "Producer::pool, Chain::pool_settled",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PoolSettled {
    #[document(summary = "It may still be undone, or still come back.")]
    Pending,
    #[document(summary = "It is off the main chain for good.")]
    Dropped,
    #[document(summary = "It is on the main chain for good, and shared out this: its reward and its producer's fees, together.")]
    Final(crate::structs::u256::u256),
}

#[document(
    summary = "A pool's claims at one root: every miner and all it has earned, in address order, the leaves of the root, and the pool's blocks that committed to it. What a miner's Merkle path comes from.",
    category = "Consensus",
    see_also = "PoolReport, Tx::Claim, claim_leaf",
)]
#[derive(Clone, Debug, PartialEq)]
pub struct ClaimTree {
    #[document(summary = "The root.")]
    pub root: crate::structs::hash::Hash,
    #[document(summary = "Each miner and all it has earned, in address order.")]
    pub leaves: Vec<(crate::structs::address::Address, crate::structs::u256::u256)>,
    #[document(summary = "The pool's blocks that carried this root, by height and id, oldest first: a claim names the height of any one on the main chain.")]
    pub committed: Vec<(u64, crate::structs::hash::Hash)>,
}

impl ClaimTree {
    #[document(
        parent = "ClaimTree",
        summary = "The tree over these earnings, in address order, committed to by no block yet.",
    )]
    pub fn new(hash: crate::structs::hash::HashType, leaves: Vec<(crate::structs::address::Address, crate::structs::u256::u256)>) -> ClaimTree {
        let hashes: Vec<_> = leaves.iter().map(|(to, earned)| crate::registry::pools::claim_leaf(hash, to, *earned)).collect();

        ClaimTree { root: crate::crypto::merkle::root(hash, &hashes), leaves, committed: Vec::new() }
    }

    #[document(
        parent = "ClaimTree",
        summary = "A miner's claim: its place, all it earned, and the Merkle path from its leaf to the root. None for a miner the tree does not hold.",
    )]
    pub fn proof(&self, hash: crate::structs::hash::HashType, to: &crate::structs::address::Address) -> Option<(u64, crate::structs::u256::u256, Vec<crate::structs::hash::Hash>)> {
        let index = self.leaves.binary_search_by(|(miner, _)| miner.cmp(to)).ok()?;
        let hashes: Vec<_> = self.leaves.iter().map(|(to, earned)| crate::registry::pools::claim_leaf(hash, to, *earned)).collect();

        Some((index as u64, self.leaves[index].1, crate::crypto::merkle::audit_path(hash, &hashes, index)))
    }

    #[document(
        parent = "ClaimTree",
        summary = "A miner's claim under this tree's root, as the block at this height committed to it, for the miner to keep. None for a miner the tree does not hold.",
        params = { pool = "The pool, whose escrow pays it.", height = "The height of a block that committed to the root.", block = "That block's id." },
    )]
    pub fn claim(
        &self,
        hash: crate::structs::hash::HashType,
        pool: crate::structs::address::Address,
        height: u64,
        block: crate::structs::hash::Hash,
        to: &crate::structs::address::Address,
    ) -> Option<crate::registry::pools::PoolClaim> {
        let (index, earned, path) = self.proof(hash, to)?;

        Some(crate::registry::pools::PoolClaim { pool, height, block, to: *to, earned, index, count: self.leaves.len() as u64, path })
    }
}

#[document(
    summary = "How a node's mining pool stands: its fee, its window, every miner with work in the window and what the next block gives it, and the blocks it sealed lately. Whether each of those blocks is still on the main chain is the chain's to say.",
    category = "Consensus",
    see_also = "MiningReport, MinerStanding, PoolBlock",
)]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PoolReport {
    #[document(summary = "The pool's address: its blocks' producer, which keeps the fee, and whose escrow its miners claim from.")]
    pub producer: Option<crate::structs::address::Address>,
    #[document(summary = "What the pool keeps of each block, in parts of PAYOUT_PARTS.")]
    pub fee: u32,
    #[document(summary = "How many blocks' worth of work the split counts.")]
    pub window: u64,
    #[document(summary = "Whether anyone may join, with only a payout address, rather than those who know the token.")]
    pub open: bool,
    #[document(summary = "The easiest share difficulty it hands out.")]
    pub min_share: u128,
    #[document(summary = "The work in the window, every miner's added up.")]
    pub work: u128,
    #[document(summary = "Every miner with work in the window, the most first.")]
    pub miners: Vec<MinerStanding>,
    #[document(summary = "The blocks it sealed lately, newest first: every one not yet credited, and the latest of the rest.")]
    pub blocks: Vec<PoolBlock>,
    #[document(summary = "All its miners have earned, every credited block's share added up.")]
    pub earned: crate::structs::u256::u256,
    #[document(summary = "Its claim trees, newest first: the one its next block commits to, and the few before, for the paths to roots blocks committed already.")]
    pub claims: Vec<std::sync::Arc<ClaimTree>>,
}

impl PoolReport {
    #[document(
        parent = "PoolReport",
        summary = "The highest height at which the pool's blocks can no longer be undone, as far as its books know: that of the newest block it credited or found dropped. Every height under it is settled too. None before any was.",
        see_also = "SavedClaims::keep",
    )]
    pub fn settled(&self) -> Option<u64> {
        self.blocks.iter().filter(|block| block.credited.is_some()).map(|block| block.height).max()
    }
}

#[document(
    summary = "How a node's mining is going: its own devices, and the workers mining for it, each with its hashrate.",
    category = "Consensus",
    see_also = "Producer::mining",
)]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MiningReport {
    #[document(summary = "The node's own devices.")]
    pub devices: Vec<DeviceReport>,
    #[document(summary = "Where workers connect, when it takes them.")]
    pub listening: Option<String>,
    #[document(summary = "The workers connected now.")]
    pub workers: Vec<WorkerReport>,
    #[document(summary = "Its pool, when it runs one.")]
    pub pool: Option<PoolReport>,
    #[document(summary = "Whether its mining port takes workers mining alone, --mining-solo.")]
    pub solo: bool,
    #[document(summary = "Whether its mining port takes anyone, --mining-open, with the open token.")]
    pub open: bool,
    #[document(summary = "The protocol name its mining port speaks, as --protocol gave it; empty for a node without one.")]
    pub protocol: String,
}

impl MiningReport {
    #[document(parent = "MiningReport", summary = "Every device's hashrate added up, the workers' included.")]
    pub fn rate(&self) -> f64 {
        self.devices.iter().chain(self.workers.iter().flat_map(|worker| &worker.devices)).map(|device| device.rate).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_meter_counts_from_any_thread_and_the_report_adds_up() {
        let meter = std::sync::Arc::new(Meter::default());
        let threads: Vec<_> = (0..4)
            .map(|_| {
                let meter = meter.clone();
                std::thread::spawn(move || (0..1_000).for_each(|_| meter.add(10)))
            })
            .collect();
        threads.into_iter().for_each(|thread| thread.join().unwrap());

        assert_eq!(meter.total(), 40_000);
        assert_eq!(meter.rate(), 0.0, "not a stretch yet");

        let report = MiningReport {
            devices: vec![DeviceReport { name: "CPU".to_string(), rate: 5.0 }],
            listening: None,
            workers: vec![WorkerReport {
                name: "rig".to_string(),
                address: "10.0.0.2:5000".to_string(),
                since: 0,
                devices: vec![DeviceReport { name: "GPU 0".to_string(), rate: 100.0 }, DeviceReport { name: "GPU 1".to_string(), rate: 50.0 }],
                found: 1,
                refused: 0,
                payout: None,
                shares: 0,
                share_difficulty: 0,
                solo: false,
            }],
            pool: None,
            ..Default::default()
        };

        assert_eq!(report.rate(), 155.0);
    }
}
