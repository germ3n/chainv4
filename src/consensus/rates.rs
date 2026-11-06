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
    summary = "A pool's claims at one root: every miner and all it has earned, in address order, the leaves of the root, and the pool's blocks that committed to it. What a miner's Merkle path comes from. Its leaves are in memory, or kept in the node's database (Column::PoolTrees), where a path is a few reads however many miners the pool has.",
    category = "Consensus",
    see_also = "PoolReport, Tx::Claim, claim_leaf",
)]
#[derive(Clone)]
pub struct ClaimTree {
    #[document(summary = "The root.")]
    pub root: Hash,
    #[document(summary = "How many leaves it holds: miners with earnings.")]
    pub count: u64,
    #[document(summary = "The pool's blocks that carried this root, by height and id, oldest first: a claim names the height of any one on the main chain.")]
    pub committed: Vec<(u64, Hash)>,
    leaves: Leaves,
}

/// Where a tree's leaves are.
#[derive(Clone)]
enum Leaves {
    Memory(std::sync::Arc<Vec<(Address, u256)>>),
    Kept(std::sync::Arc<dyn crate::db::Database>),
}

impl PartialEq for ClaimTree {
    fn eq(&self, other: &ClaimTree) -> bool {
        (self.root, self.count, &self.committed) == (other.root, other.count, &other.committed)
    }
}

impl std::fmt::Debug for ClaimTree {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kept = matches!(self.leaves, Leaves::Kept(_));

        f.debug_struct("ClaimTree").field("root", &self.root).field("count", &self.count).field("committed", &self.committed).field("kept", &kept).finish()
    }
}

use crate::structs::{address::Address, hash::{Hash, HashType}, u256::u256};

/// How many entries go to the database in one commit while a tree is written.
const TREE_BATCH: usize = 50_000;

/// Where a list of n, at least 2, splits, as RFC 6962's trees split: the largest power of two below n.
fn split(count: u64) -> u64 {
    1 << (u64::BITS - (count - 1).leading_zeros() - 1)
}

fn tree_prefix(root: &Hash) -> Vec<u8> {
    [b"t".as_slice(), &root.to_bytes()].concat()
}

fn leaf_key(root: &Hash, index: u64) -> Vec<u8> {
    [tree_prefix(root), b"l".to_vec(), index.to_be_bytes().to_vec()].concat()
}

fn miner_key(root: &Hash, to: &Address) -> Vec<u8> {
    [tree_prefix(root), b"a".to_vec(), crate::db::address_key(to)].concat()
}

fn node_key(root: &Hash, start: u64, len: u64) -> Vec<u8> {
    [tree_prefix(root), b"n".to_vec(), start.to_be_bytes().to_vec(), len.to_be_bytes().to_vec()].concat()
}

fn meta_key(root: &Hash) -> Vec<u8> {
    [tree_prefix(root), b"m".to_vec()].concat()
}

/// Which trees are kept, newest first.
const KEPT_KEY: &[u8] = b"kept";

fn encode_leaf(to: &Address, earned: u256) -> Vec<u8> {
    [wincode::serialize(to).expect("addresses encode"), earned.to_le_bytes().to_vec()].concat()
}

fn decode_leaf(bytes: &[u8]) -> Option<(Address, u256)> {
    let (address, earned) = bytes.split_at_checked(bytes.len().checked_sub(32)?)?;

    Some((wincode::deserialize(address).ok()?, u256::from_le_bytes(earned.try_into().ok()?)))
}

fn hash_of(hash: HashType, bytes: &[u8]) -> Option<Hash> {
    Hash::from_bytes(hash, bytes)
}

impl ClaimTree {
    #[document(
        parent = "ClaimTree",
        summary = "The tree over these earnings, in address order, committed to by no block yet, in memory.",
    )]
    pub fn new(hash: HashType, leaves: Vec<(Address, u256)>) -> ClaimTree {
        let hashes: Vec<_> = leaves.iter().map(|(to, earned)| crate::registry::pools::claim_leaf(hash, to, *earned)).collect();

        ClaimTree { root: crate::crypto::merkle::root(hash, &hashes), count: leaves.len() as u64, committed: Vec::new(), leaves: Leaves::Memory(std::sync::Arc::new(leaves)) }
    }

    #[document(
        parent = "ClaimTree",
        summary = "The tree over these earnings, written to the node's database: each leaf, each miner's place, and every inner node, so a path is a few reads. Written in batches, then named among the kept trees.",
    )]
    pub fn keep(db: std::sync::Arc<dyn crate::db::Database>, hash: HashType, leaves: &[(Address, u256)]) -> Result<ClaimTree, crate::db::DbError> {
        use crate::db::{Changes, Column};

        let hashes: Vec<Hash> = leaves.iter().map(|(to, earned)| crate::registry::pools::claim_leaf(hash, to, *earned)).collect();
        let root = crate::crypto::merkle::root(hash, &hashes);
        let mut changes = Changes::new();
        let flush = |changes: &mut Changes| db.commit(std::mem::take(changes));

        for (index, (to, earned)) in leaves.iter().enumerate() {
            changes.insert((Column::PoolTrees, leaf_key(&root, index as u64)), Some(encode_leaf(to, *earned)));
            changes.insert((Column::PoolTrees, miner_key(&root, to)), Some((index as u64).to_be_bytes().to_vec()));

            if changes.len() >= TREE_BATCH {
                flush(&mut changes)?;
            }
        }

        // Every inner node, by the range of leaves it covers: its root is the tree's.
        fn nodes(hashes: &[Hash], start: u64, root: &Hash, out: &mut Vec<(Vec<u8>, Hash)>) -> Hash {
            if hashes.len() == 1 {
                return hashes[0];
            }

            let left = split(hashes.len() as u64) as usize;
            let (l, r) = (nodes(&hashes[..left], start, root, out), nodes(&hashes[left..], start + left as u64, root, out));
            let joined = crate::crypto::merkle::join(&l, &r);
            out.push((node_key(root, start, hashes.len() as u64), joined));

            joined
        }

        if hashes.len() > 1 {
            let mut inner = Vec::new();
            nodes(&hashes, 0, &root, &mut inner);

            for (key, node) in inner {
                changes.insert((Column::PoolTrees, key), Some(node.to_bytes()));

                if changes.len() >= TREE_BATCH {
                    flush(&mut changes)?;
                }
            }
        }

        let tree = ClaimTree { root, count: leaves.len() as u64, committed: Vec::new(), leaves: Leaves::Kept(db.clone()) };
        changes.insert((Column::PoolTrees, meta_key(&root)), Some(tree.meta()));
        flush(&mut changes)?;

        Ok(tree)
    }

    fn meta(&self) -> Vec<u8> {
        let committed: Vec<(u64, Vec<u8>)> = self.committed.iter().map(|(height, id)| (*height, id.to_bytes())).collect();

        wincode::serialize(&(self.count, committed)).expect("meta encodes")
    }

    #[document(parent = "ClaimTree", summary = "Writes again which blocks committed to a kept tree; nothing for one in memory.")]
    pub fn keep_committed(&self) -> Result<(), crate::db::DbError> {
        match &self.leaves {
            Leaves::Memory(_) => Ok(()),
            Leaves::Kept(db) => db.commit(crate::db::Changes::from([((crate::db::Column::PoolTrees, meta_key(&self.root)), Some(self.meta()))])),
        }
    }

    #[document(parent = "ClaimTree", summary = "Whether its leaves are in the node's database.")]
    pub fn is_kept(&self) -> bool {
        matches!(self.leaves, Leaves::Kept(_))
    }

    #[document(parent = "ClaimTree", summary = "The trees a database keeps, newest first, as kept_trees last named them, with the blocks that committed to each.")]
    pub fn kept(db: std::sync::Arc<dyn crate::db::Database>, hash: HashType) -> Result<Vec<ClaimTree>, crate::db::DbError> {
        use crate::db::Column;

        let Some(list) = db.read(Column::PoolTrees, KEPT_KEY)? else {
            return Ok(Vec::new());
        };
        let roots: Vec<Vec<u8>> = wincode::deserialize(&list).unwrap_or_default();
        let mut trees = Vec::new();

        for root in roots.iter().filter_map(|bytes| hash_of(hash, bytes)) {
            let Some(meta) = db.read(Column::PoolTrees, &meta_key(&root))? else { continue };
            let Ok((count, committed)) = wincode::deserialize::<(u64, Vec<(u64, Vec<u8>)>)>(&meta) else { continue };
            let committed = committed.into_iter().filter_map(|(height, id)| Some((height, hash_of(hash, &id)?))).collect();
            trees.push(ClaimTree { root, count, committed, leaves: Leaves::Kept(db.clone()) });
        }

        Ok(trees)
    }

    #[document(parent = "ClaimTree", summary = "Names these trees as the ones kept, newest first, and deletes every other tree the database holds.")]
    pub fn kept_trees(db: &std::sync::Arc<dyn crate::db::Database>, trees: &[std::sync::Arc<ClaimTree>]) -> Result<(), crate::db::DbError> {
        use crate::db::{Changes, Column};

        let roots: Vec<Vec<u8>> = trees.iter().filter(|tree| tree.is_kept()).map(|tree| tree.root.to_bytes()).collect();
        let keep: std::collections::HashSet<Vec<u8>> = roots.iter().map(|root| [b"t".as_slice(), root].concat()).collect();
        let mut gone = Vec::new();
        db.scan(Column::PoolTrees, b"t", b"t", &mut |key, _| {
            let prefix_len = keep.iter().next().map_or(1 + 32, Vec::len).min(key.len());

            if !keep.contains(&key[..prefix_len]) {
                gone.push(key.to_vec());
            }

            true
        })?;
        let mut changes = Changes::new();
        changes.insert((Column::PoolTrees, KEPT_KEY.to_vec()), Some(wincode::serialize(&roots).expect("roots encode")));

        for key in gone {
            changes.insert((Column::PoolTrees, key), None);

            if changes.len() >= TREE_BATCH {
                db.commit(std::mem::take(&mut changes))?;
            }
        }

        db.commit(changes)
    }

    /// The leaf at a place.
    fn leaf(&self, index: u64) -> Option<(Address, u256)> {
        match &self.leaves {
            Leaves::Memory(leaves) => leaves.get(usize::try_from(index).ok()?).copied(),
            Leaves::Kept(db) => decode_leaf(&db.read(crate::db::Column::PoolTrees, &leaf_key(&self.root, index)).ok()??),
        }
    }

    /// The root over the leaves from start, len of them, of a kept tree.
    fn range_root(&self, db: &dyn crate::db::Database, hash: HashType, start: u64, len: u64) -> Option<Hash> {
        match len {
            1 => self.leaf(start).map(|(to, earned)| crate::registry::pools::claim_leaf(hash, &to, earned)),
            _ => hash_of(hash, &db.read(crate::db::Column::PoolTrees, &node_key(&self.root, start, len)).ok()??),
        }
    }

    /// A leaf's path in a kept tree, as merkle::audit_path makes it: the deepest sibling first.
    fn kept_path(&self, db: &dyn crate::db::Database, hash: HashType, start: u64, len: u64, index: u64) -> Option<Vec<Hash>> {
        if len <= 1 {
            return Some(Vec::new());
        }

        let left = split(len);
        let mut path = match index < left {
            true => self.kept_path(db, hash, start, left, index)?,
            false => self.kept_path(db, hash, start + left, len - left, index - left)?,
        };
        path.push(match index < left {
            true => self.range_root(db, hash, start + left, len - left)?,
            false => self.range_root(db, hash, start, left)?,
        });

        Some(path)
    }

    #[document(
        parent = "ClaimTree",
        summary = "A miner's claim: its place, all it earned, and the Merkle path from its leaf to the root. None for a miner the tree does not hold.",
    )]
    pub fn proof(&self, hash: HashType, to: &Address) -> Option<(u64, u256, Vec<Hash>)> {
        match &self.leaves {
            Leaves::Memory(leaves) => {
                let index = leaves.binary_search_by(|(miner, _)| miner.cmp(to)).ok()?;
                let hashes: Vec<_> = leaves.iter().map(|(to, earned)| crate::registry::pools::claim_leaf(hash, to, *earned)).collect();

                Some((index as u64, leaves[index].1, crate::crypto::merkle::audit_path(hash, &hashes, index)))
            }
            Leaves::Kept(db) => {
                let index = u64::from_be_bytes(db.read(crate::db::Column::PoolTrees, &miner_key(&self.root, to)).ok()??.try_into().ok()?);
                let (_, earned) = self.leaf(index)?;

                Some((index, earned, self.kept_path(db.as_ref(), hash, 0, self.count, index)?))
            }
        }
    }

    #[document(
        parent = "ClaimTree",
        summary = "Visits each leaf in order, its place, miner and all it earned, until visit says to stop.",
    )]
    pub fn each_leaf(&self, visit: &mut dyn FnMut(u64, &Address, u256) -> bool) {
        match &self.leaves {
            Leaves::Memory(leaves) => {
                for (index, (to, earned)) in leaves.iter().enumerate() {
                    if !visit(index as u64, to, *earned) {
                        return;
                    }
                }
            }
            Leaves::Kept(db) => {
                let prefix = [tree_prefix(&self.root), b"l".to_vec()].concat();
                let mut index = 0;
                let _ = db.scan(crate::db::Column::PoolTrees, &prefix, &prefix, &mut |_, value| match decode_leaf(value) {
                    Some((to, earned)) => {
                        index += 1;
                        visit(index - 1, &to, earned)
                    }
                    None => false,
                });
            }
        }
    }

    #[document(
        parent = "ClaimTree",
        summary = "A miner's claim under this tree's root, as the block at this height committed to it, for the miner to keep. None for a miner the tree does not hold.",
        params = { pool = "The pool, whose escrow pays it.", height = "The height of a block that committed to the root.", block = "That block's id." },
    )]
    pub fn claim(&self, hash: HashType, pool: Address, height: u64, block: Hash, to: &Address) -> Option<crate::registry::pools::PoolClaim> {
        let (index, earned, path) = self.proof(hash, to)?;

        Some(crate::registry::pools::PoolClaim { pool, height, block, to: *to, earned, index, count: self.count, path })
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
