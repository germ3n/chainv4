//! A mining pool's books. Its workers send shares, nonces that meet a difficulty far easier than a
//! block's, set for each worker so it finds one every ten seconds or so; a share is worth its
//! difficulty, the hashes it takes on average. The books keep the last window of them, some
//! blocks' worth of work (PPLNS: pay per last N shares), and every block the pool seals shares its
//! reward and fees among the miners in the window by their work, less the pool's fee, which its
//! producer keeps. The miners' share goes to the pool's escrow on chain, in the block itself.
//!
//! Once a block can no longer be undone, the books credit each of its miners with its part, and
//! build a claim tree over all each miner has earned. The pool's next block commits to its root,
//! and a miner claims from the escrow with a path to its leaf (Tx::Claim): any number of miners,
//! in 44 bytes a block, and the pool never holds what it owes.
//!
//! The books are kept in a file, so a restart loses no miner's work in the window or earnings.

use {
    crate::consensus::{ClaimTree, MinerStanding, PoolBlock, PoolReport, PoolSettled},
    crate::structs::address::Address,
    crate::structs::block::{PAYOUT_PARTS, Payout, PoolCommitment},
    crate::structs::hash::{Hash, HashType},
    crate::structs::u256::u256,
    r#macro::document,
    serde_json::{Value, json},
    std::collections::{BTreeMap, HashSet, VecDeque},
    std::path::PathBuf,
    std::sync::Arc,
    std::time::{Duration, Instant},
};

#[document(
    summary = "How often a worker should find a share, in seconds: what each one's share difficulty is set for.",
    category = "Mining",
    see_also = "share_difficulty",
)]
pub const SHARE_EVERY: f64 = 10.0;

/// The most shares the books keep, whatever the window: a bound on their memory and file.
const MOST_SHARES: usize = 1 << 20;
/// How many settled blocks the books remember; those still waiting are all kept.
const KEPT_BLOCKS: usize = 100;
/// How many claim trees the books keep, for paths to the roots blocks committed already.
const KEPT_TREES: usize = 4;
/// How long a worker's shares are counted over for the hashrate they show.
const SHOWN_OVER: Duration = Duration::from_secs(60);
/// How many recent jobs' share difficulties a worker keeps, as many as the pool takes shares for.
pub const RECENT_JOBS: usize = 8;

#[document(
    summary = "How a node runs its mining pool: --pool and the flags beside it.",
    category = "Mining",
    see_also = "Pool, MiningPort",
)]
#[derive(Clone, Debug, PartialEq)]
pub struct PoolConfig {
    #[document(summary = "What the pool keeps of each block, in parts of PAYOUT_PARTS: --pool-fee's percent.")]
    pub fee: u32,
    #[document(summary = "How many blocks' worth of work the split counts: --pool-window, 2 unless given.")]
    pub window: u64,
    #[document(summary = "The easiest share difficulty handed out: --pool-min-share.")]
    pub min_share: u128,
    #[document(summary = "Whether anyone may join, with a payout address and no token: --mining-open.")]
    pub open: bool,
    #[document(summary = "Where the books are kept: pool.json in the node's data directory; None keeps them in memory alone.")]
    pub file: Option<PathBuf>,
}

impl Default for PoolConfig {
    fn default() -> PoolConfig {
        PoolConfig { fee: 0, window: 2, min_share: 1_000, open: false, file: None }
    }
}

#[document(
    summary = "Reads a pool fee in percent, such as 1.5, as parts of PAYOUT_PARTS: from 0 to 100, with at most seven decimals, which is a part.",
    category = "Mining",
    see_also = "PoolConfig::fee",
)]
pub fn fee_parts(percent: &str) -> Result<u32, String> {
    let wrong = || format!("the pool fee {percent} is not a percent from 0 to 100, such as 1.5");
    let (whole, fraction) = percent.trim().split_once('.').unwrap_or((percent.trim(), ""));

    if whole.is_empty() || fraction.len() > 7 || !whole.bytes().chain(fraction.bytes()).all(|byte| byte.is_ascii_digit()) {
        return Err(wrong());
    }

    let whole: u64 = whole.parse().map_err(|_| wrong())?;
    let fraction: u64 = format!("{fraction:0<7}").parse().map_err(|_| wrong())?;
    let parts = whole.checked_mul(10_000_000).and_then(|parts| parts.checked_add(fraction)).ok_or_else(wrong)?;

    if parts > u64::from(PAYOUT_PARTS) {
        return Err(wrong());
    }

    Ok(parts as u32)
}

#[document(
    summary = "The share difficulty for a worker: what it finds about every SHARE_EVERY seconds at its hashrate, the higher of what it says it does and what its shares show, between the pool's easiest and the block's own. Saying less than it does gains a worker nothing: its shares come faster and show it.",
    category = "Mining",
    params = {
        said = "The hashrate it last said, in hashes a second.",
        shown = "The hashrate its recent shares show.",
        min_share = "The pool's easiest share difficulty.",
        block = "The block's difficulty, which no share needs to pass.",
    },
)]
pub fn share_difficulty(said: f64, shown: f64, min_share: u128, block: u128) -> u128 {
    let rate = said.max(shown);
    // A cast saturates: a rate past u128 is the most, and NaN fails the comparison.
    let wanted = if rate > 0.0 { (rate * SHARE_EVERY) as u128 } else { 0 };

    wanted.max(min_share).min(block.max(1))
}

/// A share the pool took.
#[derive(Clone, Debug, PartialEq)]
struct Share {
    to: Address,
    work: u128,
    at: u64,
}

#[document(
    summary = "A mining pool's books: the shares in the window and the split they make of the next block; the blocks the pool sealed, each crediting its miners once it can no longer be undone; all each miner has earned; and the claim trees over those earnings, whose roots the pool's blocks commit to.",
    category = "Mining",
    see_also = "PoolConfig, Pool, ClaimTree",
)]
pub struct Ledger {
    config: PoolConfig,
    hash: HashType,
    shares: VecDeque<Share>,
    /// Every share's work in the window, added up.
    work: u128,
    /// The latest block difficulty, which sets how much work the window holds.
    difficulty: u128,
    /// The blocks sealed, newest first: every one not yet settled, and the latest of the rest.
    blocks: VecDeque<PoolBlock>,
    /// All each miner has earned from the blocks credited.
    earned: BTreeMap<Address, u256>,
    /// The claim trees, newest first.
    trees: VecDeque<Arc<ClaimTree>>,
}

impl Ledger {
    #[document(
        parent = "Ledger",
        summary = "The books, read from the config's file when it has one and it reads; empty otherwise.",
        params = { hash = "The chain's hash, which the claim trees are built with." },
    )]
    pub fn open(config: PoolConfig, hash: HashType) -> Ledger {
        let mut ledger = Ledger {
            config,
            hash,
            shares: VecDeque::new(),
            work: 0,
            difficulty: 0,
            blocks: VecDeque::new(),
            earned: BTreeMap::new(),
            trees: VecDeque::from([Arc::new(ClaimTree::new(hash, Vec::new()))]),
        };

        if let Some(file) = ledger.config.file.clone() {
            match std::fs::read_to_string(&file) {
                Ok(text) => match serde_json::from_str::<Value>(&text).ok().and_then(|value| ledger.read(&value)) {
                    Some(()) => log::info!("[pool] {} shares, {} blocks and {} miners' earnings read from {}", ledger.shares.len(), ledger.blocks.len(), ledger.earned.len(), file.display()),
                    None => log::warn!("[pool] {} does not read; starting with empty books", file.display()),
                },
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => log::warn!("[pool] {}: {err}; starting with empty books", file.display()),
            }
        }

        ledger
    }

    #[document(parent = "Ledger", summary = "How the pool runs.")]
    pub fn config(&self) -> &PoolConfig {
        &self.config
    }

    #[document(
        parent = "Ledger",
        summary = "Takes a share, worth its difficulty, and drops the oldest shares the window no longer holds.",
        params = {
            to = "Where its miner's earnings go.",
            work = "The share's difficulty.",
            at = "When it came, in milliseconds since the Unix epoch.",
            block = "The block's difficulty now, which sets how much work the window holds.",
        },
    )]
    pub fn add(&mut self, to: Address, work: u128, at: u64, block: u128) {
        self.shares.push_back(Share { to, work, at });
        self.work = self.work.saturating_add(work);
        self.difficulty = block;
        self.trim();
    }

    #[document(
        parent = "Ledger",
        summary = "Takes a new block difficulty, and drops the oldest shares the window no longer holds at it.",
    )]
    pub fn at_difficulty(&mut self, block: u128) {
        self.difficulty = block;
        self.trim();
    }

    /// Drops the oldest shares while the rest still hold the window's work, or past the most kept.
    fn trim(&mut self) {
        let window = self.difficulty.saturating_mul(u128::from(self.config.window.max(1)));

        while let Some(oldest) = self.shares.front() {
            let without = self.work - oldest.work;

            if without < window && self.shares.len() <= MOST_SHARES {
                break;
            }

            self.work = without;
            self.shares.pop_front();
        }
    }

    /// Each miner's work and shares in the window.
    fn by_miner(&self) -> BTreeMap<Address, (u128, u64)> {
        let mut miners: BTreeMap<Address, (u128, u64)> = BTreeMap::new();

        for share in &self.shares {
            let (work, shares) = miners.entry(share.to).or_default();
            *work = work.saturating_add(share.work);
            *shares += 1;
        }

        miners
    }

    #[document(
        parent = "Ledger",
        summary = "How the next block shares out its reward: every miner with work in the window, each with its part of what the fee leaves, by its work, in address order. Empty with no shares.",
    )]
    pub fn split(&self) -> Vec<Payout> {
        let miners = self.by_miner();
        let total = miners.values().fold(u256::ZERO, |total, (work, _)| total + u256::from(*work));

        if total == u256::ZERO {
            return Vec::new();
        }

        let shared = u256::from(PAYOUT_PARTS - self.config.fee.min(PAYOUT_PARTS));

        miners
            .into_iter()
            .map(|(to, (work, _))| Payout { to, parts: (shared * u256::from(work) / total).as_u32() })
            .filter(|payout| payout.parts > 0)
            .collect()
    }

    #[document(
        parent = "Ledger",
        summary = "Credits the miners of every sealed block that can no longer be undone, as settled says, and builds a new claim tree when any was.",
        params = { settled = "Whether a block, by id and height, is settled: what the chain says of it." },
    )]
    pub fn credit(&mut self, settled: &dyn Fn(&Hash, u64) -> PoolSettled) {
        let mut credited = false;

        for block in self.blocks.iter_mut().filter(|block| block.credited.is_none()) {
            match settled(&block.id, block.height) {
                PoolSettled::Pending => {}
                PoolSettled::Dropped => block.credited = Some(false),
                PoolSettled::Final(total) => {
                    for payout in &block.payouts {
                        let earned = self.earned.entry(payout.to).or_default();
                        *earned = earned.saturating_add(payout.of(total));
                    }

                    block.credited = Some(true);
                    credited = true;
                }
            }
        }

        if credited {
            let leaves = self.earned.iter().map(|(to, earned)| (*to, *earned)).collect();
            self.trees.push_front(Arc::new(ClaimTree::new(self.hash, leaves)));
            self.trees.truncate(KEPT_TREES);
        }

        self.forget();
    }

    /// Forgets the oldest settled blocks past the most kept; one still waiting is never forgotten.
    fn forget(&mut self) {
        while self.blocks.len() > KEPT_BLOCKS && self.blocks.back().is_some_and(|block| block.credited.is_some()) {
            self.blocks.pop_back();
        }
    }

    #[document(
        parent = "Ledger",
        summary = "What the next block commits to, with the split it shares out by: the parts the fee leaves, sent to the escrow, and the newest claim tree's root. None with no shares and nothing earned yet, when the block is the producer's alone.",
    )]
    pub fn commitment(&self) -> (Option<PoolCommitment>, Vec<Payout>) {
        let split = self.split();
        let tree = self.trees.front().expect("the books always hold a tree");

        if split.is_empty() && tree.leaves.is_empty() {
            return (None, split);
        }

        let parts = split.iter().map(|payout| payout.parts).sum();

        (Some(PoolCommitment { parts, root: tree.root, count: tree.leaves.len() as u64 }), split)
    }

    #[document(
        parent = "Ledger",
        summary = "Remembers a block the pool sealed, with its split and the root it committed to.",
    )]
    pub fn sealed(&mut self, height: u64, id: Hash, at: u64, split: Vec<Payout>, root: Option<Hash>) {
        self.blocks.push_front(PoolBlock { height, id, at, payouts: split, credited: None });

        if let Some(root) = root
            && let Some(tree) = self.trees.iter_mut().find(|tree| tree.root == root)
        {
            Arc::make_mut(tree).committed.push((height, id));
        }

        self.forget();
    }

    #[document(parent = "Ledger", summary = "How the pool stands, for the node's report and GET /pool.")]
    pub fn report(&self) -> PoolReport {
        let split: BTreeMap<Address, u32> = self.split().into_iter().map(|payout| (payout.to, payout.parts)).collect();
        let window = self.by_miner();
        let everyone: std::collections::BTreeSet<Address> = window.keys().chain(self.earned.keys()).copied().collect();
        let mut miners: Vec<MinerStanding> = everyone
            .into_iter()
            .map(|to| {
                let (work, shares) = window.get(&to).copied().unwrap_or_default();

                MinerStanding { to, work, shares, parts: split.get(&to).copied().unwrap_or(0), earned: self.earned.get(&to).copied().unwrap_or_default() }
            })
            .collect();
        miners.sort_by(|a, b| b.work.cmp(&a.work).then(b.earned.cmp(&a.earned)).then(a.to.cmp(&b.to)));

        PoolReport {
            producer: None,
            fee: self.config.fee,
            window: self.config.window,
            open: self.config.open,
            min_share: self.config.min_share,
            work: self.work,
            miners,
            blocks: self.blocks.iter().cloned().collect(),
            earned: self.earned.values().fold(u256::ZERO, |total, earned| total.saturating_add(*earned)),
            claims: self.trees.iter().cloned().collect(),
        }
    }

    #[document(parent = "Ledger", summary = "Writes the books to the config's file, through a file beside it renamed over it, so a crash leaves the old or the new.")]
    pub fn save(&self) {
        let Some(file) = &self.config.file else {
            return;
        };
        let next = file.with_extension("json.new");
        let written = std::fs::write(&next, self.write().to_string()).and_then(|()| std::fs::rename(&next, file));

        if let Err(err) = written {
            log::warn!("[pool] the books did not save to {}: {err}", file.display());
        }
    }

    fn write(&self) -> Value {
        let payouts = |payouts: &[Payout]| -> Vec<Value> { payouts.iter().map(|payout| json!({ "to": payout.to.to_string(), "parts": payout.parts })).collect() };
        let id = |id: &Hash| json!({ "chain": id.algorithm().name(), "id": crate::structs::hex::to_hex(&id.to_bytes()) });

        json!({
            "difficulty": self.difficulty.to_string(),
            "shares": self.shares.iter().map(|share| json!({ "to": share.to.to_string(), "work": share.work.to_string(), "at": share.at })).collect::<Vec<_>>(),
            "blocks": self.blocks.iter().map(|block| json!({
                "height": block.height,
                "id": id(&block.id),
                "at": block.at,
                "payouts": payouts(&block.payouts),
                "credited": block.credited,
            })).collect::<Vec<_>>(),
            "earned": self.earned.iter().map(|(to, earned)| json!({ "to": to.to_string(), "earned": earned.to_string() })).collect::<Vec<_>>(),
            // The newest tree is the earnings above; which blocks committed to it is all to keep.
            "committed": self.trees.front().map_or(Vec::new(), |tree| tree.committed.iter().map(|(height, block)| json!({ "height": height, "id": id(block) })).collect()),
        })
    }

    fn read(&mut self, value: &Value) -> Option<()> {
        let payouts = |value: &Value| -> Option<Vec<Payout>> {
            value.as_array()?.iter().map(|payout| Some(Payout { to: payout["to"].as_str()?.parse().ok()?, parts: u32::try_from(payout["parts"].as_u64()?).ok()? })).collect()
        };
        let id = |value: &Value| -> Option<Hash> {
            let chain: HashType = value["chain"].as_str()?.parse().ok()?;

            Hash::from_bytes(chain, &crate::structs::hex::from_hex(value["id"].as_str()?)?)
        };
        let difficulty: u128 = value["difficulty"].as_str()?.parse().ok()?;
        let shares: VecDeque<Share> = value["shares"]
            .as_array()?
            .iter()
            .map(|share| Some(Share { to: share["to"].as_str()?.parse().ok()?, work: share["work"].as_str()?.parse().ok()?, at: share["at"].as_u64()? }))
            .collect::<Option<_>>()?;
        let blocks: VecDeque<PoolBlock> = value["blocks"]
            .as_array()?
            .iter()
            .map(|block| {
                Some(PoolBlock {
                    height: block["height"].as_u64()?,
                    id: id(&block["id"])?,
                    at: block["at"].as_u64()?,
                    payouts: payouts(&block["payouts"])?,
                    credited: block["credited"].as_bool(),
                })
            })
            .collect::<Option<_>>()?;
        let earned: BTreeMap<Address, u256> = value["earned"]
            .as_array()?
            .iter()
            .map(|miner| Some((miner["to"].as_str()?.parse().ok()?, u256::from_str_radix(miner["earned"].as_str()?, 10).ok()?)))
            .collect::<Option<_>>()?;
        let committed: Vec<(u64, Hash)> = value["committed"].as_array()?.iter().map(|block| Some((block["height"].as_u64()?, id(&block["id"])?))).collect::<Option<_>>()?;

        let mut tree = ClaimTree::new(self.hash, earned.iter().map(|(to, earned)| (*to, *earned)).collect());
        tree.committed = committed;
        self.work = shares.iter().fold(0u128, |work, share| work.saturating_add(share.work));
        self.shares = shares;
        self.blocks = blocks;
        self.earned = earned;
        self.trees = VecDeque::from([Arc::new(tree)]);
        self.difficulty = difficulty;
        self.trim();

        Some(())
    }
}

#[document(
    summary = "A pool's view of one worker: where its payouts go, the hashrate it said and the shares it sent lately, the share difficulty it got for each recent job, and the shares it sent for them, so none counts twice.",
    category = "Mining",
    see_also = "share_difficulty, Ledger",
)]
pub struct PoolWorker {
    #[document(summary = "Where its payouts go.")]
    pub payout: Address,
    said: f64,
    since: Instant,
    /// The shares it sent within SHOWN_OVER: when, and their difficulty.
    recent: VecDeque<(Instant, u128)>,
    /// Each recent job, newest first, with the share difficulty this worker was handed for it.
    jobs: VecDeque<(u64, u128)>,
    /// The shares taken for those jobs, by job and nonce.
    seen: HashSet<(u64, u64)>,
}

impl PoolWorker {
    #[document(parent = "PoolWorker", summary = "A worker just connected, paying to payout.")]
    pub fn new(payout: Address) -> PoolWorker {
        PoolWorker { payout, said: 0.0, since: Instant::now(), recent: VecDeque::new(), jobs: VecDeque::new(), seen: HashSet::new() }
    }

    #[document(parent = "PoolWorker", summary = "Takes the hashrate it said, all its devices' added up.")]
    pub fn said(&mut self, rate: f64) {
        self.said = rate;
    }

    /// The hashrate its shares show: their work over SHOWN_OVER, or since it connected if later,
    /// once ten seconds have gone.
    fn shown(&mut self) -> f64 {
        let now = Instant::now();
        self.recent.retain(|(at, _)| now.duration_since(*at) < SHOWN_OVER);
        let over = now.duration_since(self.since).min(SHOWN_OVER).as_secs_f64();

        match over >= SHARE_EVERY {
            true => self.recent.iter().map(|(_, work)| *work as f64).sum::<f64>() / over,
            false => 0.0,
        }
    }

    #[document(
        parent = "PoolWorker",
        summary = "The share difficulty for a new job, from share_difficulty, kept with the job so the shares sent for it are checked against it.",
    )]
    pub fn job(&mut self, number: u64, min_share: u128, block: u128) -> u128 {
        let shown = self.shown();
        let difficulty = share_difficulty(self.said, shown, min_share, block);

        if self.jobs.front().is_none_or(|(newest, _)| *newest != number) {
            self.jobs.push_front((number, difficulty));
            self.jobs.truncate(RECENT_JOBS);
            let kept: HashSet<u64> = self.jobs.iter().map(|(job, _)| *job).collect();
            self.seen.retain(|(job, _)| kept.contains(job));
        }

        difficulty
    }

    #[document(parent = "PoolWorker", summary = "The share difficulty it was handed for a job, while the job is recent.")]
    pub fn difficulty(&self, number: u64) -> Option<u128> {
        self.jobs.iter().find(|(job, _)| *job == number).map(|(_, difficulty)| *difficulty)
    }

    #[document(parent = "PoolWorker", summary = "The share difficulty of its newest job: 0 before its first.")]
    pub fn current(&self) -> u128 {
        self.jobs.front().map_or(0, |(_, difficulty)| *difficulty)
    }

    #[document(
        parent = "PoolWorker",
        summary = "Counts a share for a job, unless it sent that nonce for it already.",
        returns = "Whether it is new.",
    )]
    pub fn took(&mut self, number: u64, nonce: u64, work: u128) -> bool {
        if !self.seen.insert((number, nonce)) {
            return false;
        }

        self.recent.push_back((Instant::now(), work));

        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn miner(byte: u8) -> Address {
        Address::Ed25519([byte; 32])
    }

    #[test]
    fn a_fee_reads_as_parts_of_a_billion() {
        assert_eq!(fee_parts("0"), Ok(0));
        assert_eq!(fee_parts("1.5"), Ok(15_000_000));
        assert_eq!(fee_parts("100"), Ok(PAYOUT_PARTS));
        assert_eq!(fee_parts("0.0000001"), Ok(1));
        assert!(fee_parts("100.1").is_err());
        assert!(fee_parts("-1").is_err());
        assert!(fee_parts("1.00000001").is_err(), "finer than a part");
        assert!(fee_parts("").is_err());
        assert!(fee_parts("1e3").is_err());
    }

    #[test]
    fn share_difficulty_follows_the_faster_rate_between_the_floor_and_the_block() {
        assert_eq!(share_difficulty(0.0, 0.0, 1_000, 1_000_000), 1_000, "nothing said: the floor");
        assert_eq!(share_difficulty(5_000.0, 0.0, 1_000, 1_000_000), 50_000);
        assert_eq!(share_difficulty(10.0, 5_000.0, 1_000, 1_000_000), 50_000, "saying less than its shares show gains nothing");
        assert_eq!(share_difficulty(1e12, 0.0, 1_000, 1_000_000), 1_000_000, "never past the block");
        assert_eq!(share_difficulty(f64::NAN, f64::INFINITY, 1_000, 1_000_000), 1_000_000);
    }

    fn open(config: PoolConfig) -> Ledger {
        Ledger::open(config, HashType::default_chain())
    }

    fn block(byte: u8) -> Hash {
        Hash::from_bytes(HashType::default_chain(), &[byte; 32]).unwrap()
    }

    #[test]
    fn the_split_follows_work_in_the_window_less_the_fee() {
        let mut ledger = open(PoolConfig { fee: 10_000_000, window: 1, ..PoolConfig::default() });
        assert!(ledger.split().is_empty());
        assert_eq!(ledger.commitment(), (None, Vec::new()), "no shares, nothing earned: the producer keeps it all");

        ledger.add(miner(1), 300, 0, 1_000);
        ledger.add(miner(2), 100, 1, 1_000);
        let split = ledger.split();

        assert_eq!(split, vec![Payout { to: miner(1), parts: 742_500_000 }, Payout { to: miner(2), parts: 247_500_000 }], "three quarters and a quarter of the 99% the 1% fee leaves");
        let (commitment, _) = ledger.commitment();
        assert_eq!(commitment.map(|commitment| (commitment.parts, commitment.count)), Some((990_000_000, 0)), "nothing credited yet: an empty root");
    }

    #[test]
    fn the_window_keeps_its_blocks_worth_of_work_and_drops_the_oldest() {
        let mut ledger = open(PoolConfig { window: 2, ..PoolConfig::default() });

        for at in 0..10 {
            ledger.add(miner(1), 100, at, 200);
        }

        ledger.add(miner(2), 300, 10, 200);

        // 2 blocks of 200: the newest shares holding 400, the 300 and one of 100.
        assert_eq!(ledger.report().work, 400);
        assert_eq!(ledger.split(), vec![Payout { to: miner(1), parts: 250_000_000 }, Payout { to: miner(2), parts: 750_000_000 }]);
    }

    #[test]
    fn settled_blocks_credit_their_miners_and_the_next_root_holds_every_earning() {
        let hash = HashType::default_chain();
        let mut ledger = open(PoolConfig::default());
        ledger.add(miner(1), 300, 0, 1_000);
        ledger.add(miner(2), 100, 0, 1_000);
        let (commitment, split) = ledger.commitment();
        ledger.sealed(1, block(1), 0, split.clone(), commitment.map(|commitment| commitment.root));
        ledger.sealed(2, block(2), 0, split.clone(), None);
        ledger.sealed(3, block(3), 0, split, None);

        // Block 1 is final, having shared out 1,000; block 2 was dropped; block 3 may yet be undone.
        let settled = |id: &Hash, _: u64| match id.to_bytes()[0] {
            1 => PoolSettled::Final(u256::new(1_000)),
            2 => PoolSettled::Dropped,
            _ => PoolSettled::Pending,
        };
        ledger.credit(&settled);

        let report = ledger.report();
        assert_eq!(report.blocks.iter().map(|block| block.credited).collect::<Vec<_>>(), vec![None, Some(false), Some(true)]);
        assert_eq!(report.earned, u256::new(1_000));
        assert_eq!(report.miners.iter().map(|miner| (miner.to, miner.earned)).collect::<Vec<_>>(), vec![(miner(1), u256::new(750)), (miner(2), u256::new(250))]);

        // The next block commits to the new root, and each miner's path leads to it.
        let (commitment, _) = ledger.commitment();
        let commitment = commitment.unwrap();
        let tree = &report.claims[0];
        assert_eq!((commitment.root, commitment.count), (tree.root, 2));
        let (index, earned, path) = tree.proof(hash, &miner(2)).unwrap();
        assert!(crate::crypto::merkle::verify(&commitment.root, &crate::registry::pools::claim_leaf(hash, &miner(2), earned), index as usize, 2, &path));
        assert_eq!(earned, u256::new(250));
        assert!(tree.proof(hash, &miner(3)).is_none());

        // Crediting again credits nothing twice.
        ledger.credit(&settled);
        assert_eq!(ledger.report().earned, u256::new(1_000));
    }

    #[test]
    fn a_pool_of_any_size_fits_one_commitment() {
        let mut ledger = open(PoolConfig { window: 1_000, ..PoolConfig::default() });

        for index in 0..5_000u64 {
            let mut key = [0; 32];
            key[..8].copy_from_slice(&index.to_le_bytes());
            ledger.add(Address::Ed25519(key), 1_000, 0, 1_000_000);
        }

        let (commitment, split) = ledger.commitment();
        assert_eq!(split.len(), 5_000, "every miner in the split");
        ledger.sealed(1, block(1), 0, split, commitment.map(|commitment| commitment.root));
        ledger.credit(&|_, _| PoolSettled::Final(u256::new(5_000_000)));

        let (commitment, _) = ledger.commitment();
        assert_eq!(commitment.unwrap().count, 5_000, "all of them in a root of 32 bytes");
        assert!(ledger.report().miners.iter().all(|miner| miner.earned == u256::new(1_000)));
    }

    #[test]
    fn a_worker_counts_each_share_once_and_keeps_its_recent_jobs() {
        let mut worker = PoolWorker::new(miner(1));
        worker.said(500.0);
        assert_eq!(worker.job(1, 1_000, 1_000_000), 5_000);
        assert_eq!(worker.job(1, 1_000, 1_000_000), 5_000, "the same job again changes nothing");

        assert!(worker.took(1, 42, 5_000));
        assert!(!worker.took(1, 42, 5_000), "the same nonce twice");
        assert!(worker.took(1, 43, 5_000));

        for job in 2..=(RECENT_JOBS as u64 + 1) {
            worker.job(job, 1_000, 1_000_000);
        }

        assert_eq!(worker.difficulty(1), None, "too old to take shares for");
        assert!(worker.difficulty(2).is_some());
        assert_eq!(worker.current(), 5_000);
    }

    #[test]
    fn the_books_survive_a_restart() {
        let dir = std::env::temp_dir().join(format!("chainv4-pool-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let config = PoolConfig { fee: 5, file: Some(dir.join("pool.json")), ..PoolConfig::default() };
        let mut ledger = open(config.clone());
        ledger.add(miner(1), 500, 7, 10_000);
        let (commitment, split) = ledger.commitment();
        ledger.sealed(4, block(9), 8, split.clone(), commitment.map(|commitment| commitment.root));
        ledger.credit(&|_, _| PoolSettled::Final(u256::new(10_000)));
        let (commitment, _) = ledger.commitment();
        ledger.sealed(5, block(10), 9, split, commitment.map(|commitment| commitment.root));
        ledger.save();

        let again = open(config);
        let (before, after) = (ledger.report(), again.report());
        assert_eq!((&after.miners, &after.blocks, after.earned), (&before.miners, &before.blocks, before.earned));
        assert_eq!(after.claims[0], before.claims[0], "the newest tree, and the block that committed to it");
        assert_eq!(after.claims[0].committed, vec![(5, block(10))]);
        assert_eq!(again.commitment(), ledger.commitment());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
