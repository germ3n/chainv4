use {
    super::{Consensus, ConsensusError, MAX_DRIFT_MS, Producer},
    crate::crypto::hashable::Hashable,
    crate::registry::validators::Member,
    crate::structs::address::Address,
    crate::structs::block::{BlockHeader, Seal},
    crate::crypto::hasher::Hasher,
    crate::structs::hash::{Hash, HashType},
    crate::structs::u256::u256,
    r#macro::document,
    std::sync::{Arc, Condvar, Mutex},
    std::thread::JoinHandle,
    wincode::{SchemaRead, SchemaWrite},
};

/// Separates work hashes from every other hash the chain takes.
const WORK_DOMAIN: &[u8] = b"chainv4 work";
/// Each step moves difficulty by its parent's divided by this.
const ADJUSTMENT: u128 = 64;
/// The most steps difficulty falls in one block, so one long gap cannot undo it all.
const MAX_STEPS_DOWN: u64 = 32;
/// Under a window, how far one interval between blocks may count from the target either way: a
/// block that came in a quarter of the target, or four times it, counts as that, so one gap or one
/// timestamp moves difficulty by a bounded share of the window.
const MAX_SWING: u64 = 4;
/// How many nonces a mining thread tries before checking whether its job is still current.
const NONCES_PER_CHECK: u64 = 4_096;

#[document(
    summary = "Proof-of-work parameters, from genesis.",
    category = "Consensus",
    see_also = "ProofOfWork, ConsensusConfig::ProofOfWork",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct PowConfig {
    #[document(summary = "The work hash: any secure algorithm this build has, such as blake3 or sha256. Independent of the chain's own hash.")]
    pub hash: HashType,
    #[document(summary = "How far apart blocks should be, in milliseconds. Difficulty rises when they come faster and falls when they come slower.")]
    pub target_block_ms: u64,
    #[document(summary = "The first block's difficulty: the expected number of hashes to find its seal.")]
    pub initial_difficulty: u128,
    #[document(summary = "Difficulty never falls below this.")]
    pub min_difficulty: u128,
}

impl Hashable for PowConfig {}

#[document(
    summary = "The rules of proof of work. Anyone may produce a block whose seal holds a nonce that makes the work hash of the block's id and nonce, read as a big-endian number, at most u256::MAX / difficulty. Without a window, difficulty adjusts every block from its parent alone: one step up when a block comes in under the target time, none within one to two targets, and a step down for each further target, at most 32. With one, it retargets every block over the window: the work of the window's blocks over the time they took, scaled to the target, each interval counting as between a quarter of the target and four times it, so one block's timestamp moves it by a bounded share of the window; and a block's timestamp must be after the median of the window's, as in Bitcoin, so a producer who sets its clock ahead does not drag the chain's time along.",
    category = "Consensus",
    see_also = "PowConfig, Miner, Consensus, Genesis::pow_window",
    note = "A block weighs its difficulty, so the main chain is the one that took the most work, not the longest.",
)]
pub struct ProofOfWork {
    config: PowConfig,
    /// How many ancestors the window holds, or None for the parent-only rule.
    window: Option<u64>,
}

impl ProofOfWork {
    #[document(
        parent = "ProofOfWork",
        summary = "The rules for these parameters.",
        params = { window = "The retarget and median-time window in blocks, at least 2, or None for the parent-only rule." },
    )]
    pub fn new(config: PowConfig, window: Option<u64>) -> ProofOfWork {
        ProofOfWork { config, window }
    }

    /// Difficulty from the parent's alone, by steps of a 64th.
    fn stepped(&self, parent: &BlockHeader, timestamp: u64) -> u128 {
        if parent.height == 0 {
            return self.config.initial_difficulty.max(self.config.min_difficulty).max(1);
        }

        let step = (parent.difficulty / ADJUSTMENT).max(1);
        let targets = timestamp.saturating_sub(parent.timestamp) / self.config.target_block_ms.max(1);
        let difficulty = match targets {
            0 => parent.difficulty.saturating_add(step),
            _ => parent
                .difficulty
                .saturating_sub(step.saturating_mul(u128::from((targets - 1).min(MAX_STEPS_DOWN)))),
        };

        difficulty.max(self.config.min_difficulty).max(1)
    }

    /// The window's ancestors: those given, at most window of them, and never genesis, whose
    /// timestamp is whatever the genesis file says.
    fn windowed<'a>(&self, ancestors: &'a [BlockHeader], window: u64) -> &'a [BlockHeader] {
        let most = usize::try_from(window).unwrap_or(usize::MAX).min(ancestors.len());
        let above_genesis = ancestors.iter().position(|header| header.height == 0).unwrap_or(ancestors.len());

        &ancestors[..most.min(above_genesis)]
    }

    /// Difficulty from the work the window's blocks took over the time they took, scaled to the
    /// target: the hashrate the window shows, times the target time. Each interval between two
    /// blocks counts as at least a quarter of the target and at most four times it, so one long
    /// gap, or one timestamp set ahead, moves difficulty by a bounded share of the window, and a
    /// timestamp before its parent's, which the median rule allows, counts as a quick block. The
    /// first two blocks, with no interval to measure, take the initial difficulty.
    fn retargeted(&self, ancestors: &[BlockHeader], window: u64) -> u128 {
        let window = self.windowed(ancestors, window);
        let floor = self.config.min_difficulty.max(1);

        if window.len() < 2 {
            return self.config.initial_difficulty.max(floor);
        }

        let target = u128::from(self.config.target_block_ms.max(1));
        let span: u128 = window
            .windows(2)
            .map(|pair| u128::from(pair[0].timestamp.saturating_sub(pair[1].timestamp)).clamp(target / u128::from(MAX_SWING), target * u128::from(MAX_SWING)))
            .sum();
        // The blocks produced during the span: every one but the oldest, which starts it.
        let work: u256 = window[..window.len() - 1].iter().map(|header| u256::from(header.difficulty)).fold(u256::ZERO, |sum, next| sum + next);
        let difficulty = work * u256::from(target) / u256::from(span.max(1));

        u128::try_from(difficulty).unwrap_or(u128::MAX).max(floor)
    }

    /// The median timestamp of the window's ancestors: what a block's must be after.
    fn median_time(&self, ancestors: &[BlockHeader], window: u64) -> Option<u64> {
        let window = self.windowed(ancestors, window);

        if window.is_empty() {
            return None;
        }

        let mut times: Vec<u64> = window.iter().map(|header| header.timestamp).collect();
        times.sort_unstable();

        Some(times[times.len() / 2])
    }
}

impl Consensus for ProofOfWork {
    fn work(&self, header: &BlockHeader) -> u256 {
        u256::from(header.difficulty)
    }

    fn lookback(&self) -> usize {
        self.window.map_or(1, |window| usize::try_from(window).unwrap_or(usize::MAX).max(1))
    }

    fn difficulty(&self, ancestors: &[BlockHeader], timestamp: u64) -> u128 {
        let Some(parent) = ancestors.first() else {
            return self.config.initial_difficulty.max(self.config.min_difficulty).max(1);
        };

        match self.window {
            Some(window) => self.retargeted(ancestors, window),
            None => self.stepped(parent, timestamp),
        }
    }

    fn check(&self, header: &BlockHeader, seal: &Seal, ancestors: &[BlockHeader], _producers: &[Member], now: u64) -> Result<(), ConsensusError> {
        let Some(parent) = ancestors.first() else {
            return Err(ConsensusError::NoAncestors);
        };
        // Without a window, after the parent; with one, after the median of the window, or the
        // parent's for the first block, whose only ancestor is genesis.
        let after = match self.window {
            Some(window) => self.median_time(ancestors, window).unwrap_or(parent.timestamp),
            None => parent.timestamp,
        };

        if header.timestamp <= after {
            return Err(ConsensusError::Early);
        }

        if header.timestamp > now + MAX_DRIFT_MS {
            return Err(ConsensusError::FromTheFuture);
        }

        let expected = self.difficulty(ancestors, header.timestamp);

        if header.difficulty != expected {
            return Err(ConsensusError::WrongDifficulty {
                expected,
                got: header.difficulty,
            });
        }

        let Seal::Work { nonce } = seal else {
            return Err(ConsensusError::BadSeal);
        };

        if !meets(Search::new(self.config.hash, &header.id()).work(*nonce), header.difficulty) {
            return Err(ConsensusError::NotEnoughWork);
        }

        Ok(())
    }
}

#[document(
    summary = "Whether a work hash meets a difficulty: at most u256::MAX / difficulty.",
    category = "Consensus",
)]
pub fn meets(work: u256, difficulty: u128) -> bool {
    work <= u256::MAX / u256::from(difficulty.max(1))
}

#[document(
    summary = "The work hash of a block id and nonce, as a big-endian number. A seal is valid when this meets the block's difficulty.",
    category = "Consensus",
    see_also = "meets",
)]
pub fn work_hash(hash: HashType, id: &Hash, nonce: u64) -> u256 {
    Search::new(hash, id).work(nonce)
}

/// The work hash of one block id, ready to try nonces against: the domain and id are hashed once,
/// and each nonce continues from a copy.
struct Search {
    prefix: Hasher,
}

impl Search {
    fn new(hash: HashType, id: &Hash) -> Search {
        let mut prefix = Hasher::new(hash);
        prefix.update(WORK_DOMAIN);
        prefix.update(&id.to_bytes());

        Search { prefix }
    }

    fn work(&self, nonce: u64) -> u256 {
        let mut hasher = self.prefix.clone();
        hasher.update(&nonce.to_le_bytes());
        let digest: [u8; 32] = hasher
            .finalize()
            .to_bytes()
            .try_into()
            .expect("genesis only allows 256-bit work hashes");

        u256::from_be_bytes(digest)
    }
}

/// What the mining threads work on.
pub(super) struct Job {
    /// Bumped for every new header, so threads notice when theirs is stale.
    pub(super) generation: u64,
    /// The block id and difficulty to find a nonce for; None once one was found.
    pub(super) work: Option<(Hash, u128)>,
    /// The first nonce to try: 0 for a node's own miner, and the start of its range for a worker.
    pub(super) start: u64,
    /// Whether every nonce that meets the difficulty is reported, a pool's shares, rather than
    /// the first alone.
    pub(super) shares: bool,
    pub(super) stopped: bool,
}

pub(super) struct Shared {
    pub(super) job: Mutex<Job>,
    pub(super) changed: Condvar,
}

impl Shared {
    pub(super) fn new() -> Shared {
        Shared {
            job: Mutex::new(Job {
                generation: 0,
                work: None,
                start: 0,
                shares: false,
                stopped: false,
            }),
            changed: Condvar::new(),
        }
    }

    pub(super) fn lock(&self) -> std::sync::MutexGuard<'_, Job> {
        self.job.lock().expect("mining threads never panic holding the job")
    }

    /// Waits for a job newer than the one finished, and returns its generation, block id,
    /// difficulty and first nonce; None once the miner stopped.
    pub(super) fn next_job(&self, finished: u64) -> Option<(u64, Hash, u128, u64)> {
        let mut job = self.lock();

        while !job.stopped && (job.work.is_none() || job.generation == finished) {
            job = self.changed.wait(job).expect("mining threads never panic holding the job");
        }

        if job.stopped {
            return None;
        }

        let (id, difficulty) = job.work.expect("checked above");

        Some((job.generation, id, difficulty, job.start))
    }

    /// Whether a job is still the current one, so a thread goes on with it.
    pub(super) fn current(&self, generation: u64) -> bool {
        let job = self.lock();

        !job.stopped && job.generation == generation && job.work.is_some()
    }

    /// Hands in a seal for a job, if it is still the current one: only the first find counts,
    /// unless the job wants shares, when each does and the search goes on. Whether it goes on.
    pub(super) fn finish(&self, generation: u64, id: Hash, nonce: u64, found: &(dyn Fn(Hash, Seal) + Send + Sync)) -> bool {
        let mut job = self.lock();

        if job.generation != generation || job.work.is_none() {
            return false;
        }

        let shares = job.shares;

        if !shares {
            job.work = None;
        }

        drop(job);
        found(id, Seal::Work { nonce });

        shares
    }
}

/// The work message before its nonce: the domain and the block id, 44 bytes.
#[cfg_attr(not(all(feature = "gpu-mining", not(target_arch = "wasm32"))), allow(dead_code))]
pub(super) fn work_prefix(id: &Hash) -> [u8; 44] {
    let mut prefix = [0; 44];
    prefix[..WORK_DOMAIN.len()].copy_from_slice(WORK_DOMAIN);
    prefix[WORK_DOMAIN.len()..].copy_from_slice(&id.to_bytes());

    prefix
}

/// The most a work hash may be at a difficulty, as 32 big-endian bytes.
#[cfg_attr(not(all(feature = "gpu-mining", not(target_arch = "wasm32"))), allow(dead_code))]
pub(super) fn work_target(difficulty: u128) -> [u8; 32] {
    (u256::MAX / u256::from(difficulty.max(1))).to_be_bytes()
}

#[document(
    summary = "The devices mining on threads of their own: a CPU's threads, or GPUs. work hands every thread a block id to find a nonce for, each trying its own share of the nonces, and returns at once; whichever finds one first reports it through the callback. New work replaces the old, and dropping the rig stops the threads. Miner and GpuMiner mine on one for the node, and a worker on one for the node it mines for.",
    category = "Consensus",
    see_also = "Miner, GpuMiner, DeviceReport",
)]
pub struct Rig {
    pub(super) shared: Arc<Shared>,
    pub(super) threads: Vec<JoinHandle<()>>,
    /// Each device's name and the nonces it tried.
    pub(super) devices: Vec<(String, Arc<super::Meter>)>,
}

impl Rig {
    #[document(
        parent = "Rig",
        summary = "Starts mining threads on the CPU, idle until the first work.",
        params = {
            hash = "The chain's work hash, from genesis.",
            threads = "How many threads to mine on; at least one.",
            found = "Called from a mining thread with the block id and its seal when a nonce is found.",
        },
    )]
    pub fn cpu(hash: HashType, threads: usize, found: impl Fn(Hash, Seal) + Send + Sync + 'static) -> Rig {
        let shared = Arc::new(Shared::new());
        let found = Arc::new(found);
        let meter = Arc::new(super::Meter::default());
        let count = threads.max(1);
        let threads = (0..count)
            .map(|index| {
                let (shared, found, meter) = (shared.clone(), found.clone(), meter.clone());

                std::thread::Builder::new()
                    .name(format!("miner-{index}"))
                    .spawn(move || mine(&shared, hash, index as u64, count as u64, &*found, &meter))
                    .expect("the mining thread starts")
            })
            .collect();
        let name = format!("CPU, {count} thread{}", if count == 1 { "" } else { "s" });

        Rig {
            shared,
            threads,
            devices: vec![(name, meter)],
        }
    }

    #[document(
        parent = "Rig",
        summary = "Mines a block id: every thread drops what it had and starts on this.",
        params = {
            id = "The block id to find a nonce for.",
            difficulty = "What its work hash must meet.",
            start = "The first nonce to try; the threads go up from it.",
        },
    )]
    pub fn work(&self, id: Hash, difficulty: u128, start: u64) {
        self.set(id, difficulty, start, false);
    }

    #[document(
        parent = "Rig",
        summary = "Mines a block id for a pool's shares: as work, but every nonce that meets the difficulty, the worker's share difficulty, is reported, and the search goes on until new work comes.",
        params = {
            id = "The block id to find nonces for.",
            difficulty = "What a share's work hash must meet: at most the block's own.",
            start = "The first nonce to try; the threads go up from it.",
        },
    )]
    pub fn shares(&self, id: Hash, difficulty: u128, start: u64) {
        self.set(id, difficulty, start, true);
    }

    fn set(&self, id: Hash, difficulty: u128, start: u64, shares: bool) {
        let mut job = self.shared.lock();
        job.generation += 1;
        job.work = Some((id, difficulty));
        job.start = start;
        job.shares = shares;
        self.shared.changed.notify_all();
    }

    #[document(parent = "Rig", summary = "Stops mining until the next work, as when the node it mines for is gone.")]
    pub fn rest(&self) {
        let mut job = self.shared.lock();
        job.generation += 1;
        job.work = None;
        self.shared.changed.notify_all();
    }

    #[document(parent = "Rig", summary = "Each device and its hashrate.")]
    pub fn devices(&self) -> Vec<super::DeviceReport> {
        self.devices.iter().map(|(name, meter)| super::DeviceReport { name: name.clone(), rate: meter.rate() }).collect()
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.shared.lock().stopped = true;
        self.shared.changed.notify_all();

        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

#[document(
    summary = "Mines on threads of its own. seal hands the header to every thread, each trying its own share of the nonces, and returns at once; whichever thread finds a nonce first reports it through the callback. A new header replaces the old one, and dropping the miner stops the threads.",
    category = "Consensus",
    see_also = "ProofOfWork, Producer, Rig",
)]
pub struct Miner {
    address: Address,
    rig: Rig,
}

impl Miner {
    #[document(
        parent = "Miner",
        summary = "Starts the mining threads, idle until the first header.",
        params = {
            address = "Where rewards for blocks this miner finds go.",
            hash = "The chain's work hash, from genesis.",
            threads = "How many threads to mine on; at least one.",
            found = "Called from a mining thread with the block id and its seal when a nonce is found.",
        },
    )]
    pub fn new(address: Address, hash: HashType, threads: usize, found: impl Fn(Hash, Seal) + Send + Sync + 'static) -> Miner {
        Miner {
            address,
            rig: Rig::cpu(hash, threads, found),
        }
    }
}

impl Producer for Miner {
    fn address(&self, _producers: &[Member]) -> Address {
        self.address
    }

    /// Timestamps must move forward, so the next millisecond after the parent's.
    fn start_at(&self, parent: &BlockHeader, _producers: &[Member]) -> u64 {
        parent.timestamp + 1
    }

    fn seal(&mut self, header: &BlockHeader) -> Option<Seal> {
        self.rig.work(header.id(), header.difficulty, 0);

        None
    }

    fn mining(&self) -> Option<super::MiningReport> {
        Some(super::MiningReport {
            devices: self.rig.devices(),
            ..Default::default()
        })
    }
}

/// One mining thread: tries nonces start + index, start + index + stride, … on the current job
/// until it finds one, or for shares on and on, until the job changes or the miner stops.
fn mine(shared: &Shared, hash: HashType, index: u64, stride: u64, found: &(dyn Fn(Hash, Seal) + Send + Sync), meter: &super::Meter) {
    let mut finished = 0;

    while let Some((generation, id, difficulty, start)) = shared.next_job(finished) {
        let search = Search::new(hash, &id);
        let mut nonce = start.wrapping_add(index);

        'search: loop {
            for _ in 0..NONCES_PER_CHECK {
                if meets(search.work(nonce), difficulty) && !shared.finish(generation, id, nonce, found) {
                    break 'search;
                }

                nonce = nonce.wrapping_add(stride);
            }

            meter.add(NONCES_PER_CHECK);

            if !shared.current(generation) {
                break;
            }
        }

        finished = generation;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    const TARGET: u64 = 1_000;

    fn rules(min: u128) -> ProofOfWork {
        ProofOfWork::new(PowConfig {
            hash: HashType::default_chain(),
            target_block_ms: TARGET,
            initial_difficulty: 1_000,
            min_difficulty: min,
        }, None)
    }

    fn windowed(min: u128, window: u64) -> ProofOfWork {
        ProofOfWork::new(PowConfig {
            hash: HashType::default_chain(),
            target_block_ms: TARGET,
            initial_difficulty: 1_000,
            min_difficulty: min,
        }, Some(window))
    }

    /// A header on a chain hashed with the build's default; its algorithm comes from the chain
    /// and could be any secure one.
    fn header(height: u64, timestamp: u64, difficulty: u128) -> BlockHeader {
        let chain = HashType::default_chain();

        BlockHeader {
            parent: Hash::from_bytes(chain, &[height as u8; 32]).unwrap(),
            height,
            timestamp,
            producer: Address::Ed25519([1; 32]),
            difficulty,
            tx_root: Hash::zero(chain),
            proof_root: Hash::zero(chain),
            pool_root: Hash::zero(chain),
            receipts_root: Hash::zero(chain),
            state_root: Hash::zero(chain),
            gas_used: 0,
            base_price: u256::ZERO,
        }
    }

    /// Finds a nonce on this thread.
    fn solve(header: &BlockHeader) -> Seal {
        let search = Search::new(HashType::default_chain(), &header.id());

        (0..)
            .find(|nonce| meets(search.work(*nonce), header.difficulty))
            .map(|nonce| Seal::Work { nonce })
            .unwrap()
    }

    #[test]
    fn difficulty_follows_the_block_time() {
        let rules = rules(10);
        let parent = header(5, 10_000, 6_400);

        assert_eq!(rules.difficulty(&[header(0, 0, 0)], 5), 1_000);
        assert_eq!(rules.difficulty(std::slice::from_ref(&parent), 10_000 + TARGET / 2), 6_500);
        assert_eq!(rules.difficulty(std::slice::from_ref(&parent), 10_000 + TARGET), 6_400);
        assert_eq!(rules.difficulty(std::slice::from_ref(&parent), 10_000 + TARGET * 3), 6_200);
        assert_eq!(rules.difficulty(std::slice::from_ref(&parent), 10_000 + TARGET * 1_000), 6_400 - 100 * 32);
        assert_eq!(rules.difficulty(&[header(5, 10_000, 12)], 10_000 + TARGET * 50), 10);
        assert_eq!(rules.lookback(), 1);
    }

    /// Ancestors newest first: block heights down from top, this many milliseconds apart, at one
    /// difficulty, and genesis under them at time 0.
    fn lineage(top: u64, gap: u64, difficulty: u128) -> Vec<BlockHeader> {
        (0..=top).rev().map(|height| header(height, height * gap, if height == 0 { 0 } else { difficulty })).collect()
    }

    #[test]
    fn a_window_retargets_over_its_blocks_and_bounds_timestamps_by_their_median() {
        let rules = windowed(10, 5);

        assert_eq!(rules.lookback(), 5);

        // Genesis alone, or block 1 alone: nothing to measure, so the initial difficulty.
        assert_eq!(rules.difficulty(&lineage(0, TARGET, 0), TARGET), 1_000);
        assert_eq!(rules.difficulty(&lineage(1, TARGET, 777), 2 * TARGET), 1_000);

        // Blocks on target keep their difficulty, whatever the new block's own timestamp says.
        let on_target = lineage(9, TARGET, 6_400);

        assert_eq!(rules.difficulty(&on_target, 9 * TARGET + 1), 6_400);
        assert_eq!(rules.difficulty(&on_target, 9 * TARGET + 100 * TARGET), 6_400);

        // Twice as fast doubles it; twice as slow halves it; the window is the 5 newest only.
        assert_eq!(rules.difficulty(&lineage(9, TARGET / 2, 6_400), 0), 12_800);
        assert_eq!(rules.difficulty(&lineage(9, TARGET * 2, 6_400), 0), 3_200);

        let mut mixed = lineage(9, TARGET, 6_400);
        mixed[5..].iter_mut().for_each(|header| header.timestamp = 0);
        assert_eq!(rules.difficulty(&mixed, 0), 6_400, "only the 5 newest count");

        // One block 20 targets late in a window of 4 intervals counts as 4 targets late: the span
        // is 7 targets instead of 4, so difficulty falls to four sevenths, not a sixth.
        let mut gap = lineage(9, TARGET, 6_400);
        gap[0].timestamp += 20 * TARGET;
        assert_eq!(rules.difficulty(&gap, 0), 4 * 6_400 / 7);

        // A block timestamped before its parent counts as a quarter-target block.
        let mut back = lineage(9, TARGET, 6_400);
        back[0].timestamp = back[1].timestamp - 1;
        assert_eq!(rules.difficulty(&back, 0), 4 * 6_400 * 4 / 13);

        // Never under the minimum, and the first blocks of a window that has genesis in it
        // measure from block 1, not from genesis's arbitrary time.
        assert_eq!(rules.difficulty(&lineage(9, TARGET * 1_000, 6_400), 0), 1_600, "every interval clamped to four targets: a quarter");
        assert_eq!(windowed(5_000, 5).difficulty(&lineage(9, TARGET * 4, 6_400), 0), 5_000);
        let mut early = lineage(2, TARGET, 1_000);
        early[2].timestamp = 1_000_000_000;
        assert_eq!(rules.difficulty(&early, 0), 1_000, "two blocks, one interval on target");

        // The median time: a block must come after the median of the window's timestamps, so one
        // producer's clock far ahead does not drag the next block's along.
        let mut ahead = lineage(9, TARGET, 6_400);
        ahead[0].timestamp += 100 * TARGET;
        let median = ahead[2].timestamp;
        let mut child = header(10, median, 0);
        child.parent = ahead[0].id();
        child.difficulty = rules.difficulty(&ahead, child.timestamp);
        let seal = Seal::Work { nonce: 0 };

        assert_eq!(rules.check(&child, &seal, &ahead, &[], child.timestamp + 1), Err(ConsensusError::Early), "at the median is too early");
        child.timestamp = median + 1;
        child.difficulty = rules.difficulty(&ahead, child.timestamp);
        assert!(!matches!(rules.check(&child, &seal, &ahead, &[], child.timestamp), Err(ConsensusError::Early)), "just after it is in time, though before its parent");
        assert_eq!(rules.check(&child, &seal, &[], &[], child.timestamp), Err(ConsensusError::NoAncestors));
    }

    #[test]
    fn a_window_settles_after_a_stall_without_collapsing() {
        // Blocks at difficulty 6,400 every target, then one after a long stall. Mining goes on at
        // the hashrate the chain had; difficulty falls while the stall is in the window and then
        // comes back, never below a quarter, and never runs away.
        let rules = windowed(1, 11);
        let mut chain = lineage(20, TARGET, 6_400);
        chain.reverse();
        let last = chain.last().unwrap().clone();
        let mut timestamp = last.timestamp + 100 * TARGET;
        let mut seen = Vec::new();

        for height in 21..60 {
            let ancestors: Vec<BlockHeader> = chain.iter().rev().take(11).cloned().collect();
            let difficulty = rules.difficulty(&ancestors, timestamp);
            seen.push(difficulty);
            chain.push(header(height, timestamp, difficulty));
            // At the old hashrate of 6,400 per target, a block takes this long.
            timestamp += TARGET * difficulty as u64 / 6_400;
        }

        // The stalled block's own timestamp does not lower its difficulty; the next block sees the
        // stall in its window as four targets, so 10 blocks' work over 13 targets, and difficulty
        // stays within a third of where it was until the stall leaves the window.
        assert_eq!(seen[..2], [6_400, 10 * 6_400 / 13]);
        assert!(seen.iter().all(|&difficulty| (4_400..=6_700).contains(&difficulty)), "{seen:?}");
        assert!((6_300..=6_500).contains(seen.last().unwrap()), "{seen:?}");
    }

    #[test]
    fn a_found_nonce_passes_and_anything_else_fails() {
        let rules = rules(1);
        let parent = header(5, 10_000, 64);
        let mut child = header(6, 10_100, 0);
        child.parent = parent.id();
        child.difficulty = rules.difficulty(std::slice::from_ref(&parent), child.timestamp);
        let seal = solve(&child);
        let Seal::Work { nonce } = seal else { unreachable!() };
        let wrong = (0..)
            .map(|offset| Seal::Work { nonce: nonce.wrapping_add(offset + 1) })
            .find(|candidate| {
                let Seal::Work { nonce } = candidate else { unreachable!() };
                !meets(Search::new(HashType::default_chain(), &child.id()).work(*nonce), child.difficulty)
            })
            .unwrap();
        let easy = BlockHeader { difficulty: 1, ..child.clone() };

        assert_eq!(rules.check(&child, &seal, std::slice::from_ref(&parent), &[], 10_100), Ok(()));
        assert_eq!(rules.check(&child, &wrong, std::slice::from_ref(&parent), &[], 10_100), Err(ConsensusError::NotEnoughWork));
        assert!(matches!(rules.check(&easy, &solve(&easy), std::slice::from_ref(&parent), &[], 10_100), Err(ConsensusError::WrongDifficulty { .. })));
        assert_eq!(rules.check(&child, &Seal::None, std::slice::from_ref(&parent), &[], 10_100), Err(ConsensusError::BadSeal));
        assert_eq!(rules.check(&header(6, 10_000, 0), &seal, std::slice::from_ref(&parent), &[], 10_100), Err(ConsensusError::Early));
        assert_eq!(rules.check(&child, &seal, &[], &[], 10_100), Err(ConsensusError::NoAncestors));
        assert_eq!(rules.work(&child), u256::from(child.difficulty));
    }

    #[test]
    fn the_miner_finds_seals_and_moves_on_to_new_headers() {
        let (sender, found) = mpsc::channel();
        let sender = Mutex::new(sender);
        let mut miner = Miner::new(Address::Ed25519([1; 32]), HashType::default_chain(), 2, move |id, seal| {
            let _ = sender.lock().unwrap().send((id, seal));
        });
        let rules = rules(1);
        let parent = header(5, 10_000, 500);

        for timestamp in [10_100, 10_200] {
            let mut child = header(6, timestamp, 0);
            child.parent = parent.id();
            child.difficulty = 500;

            assert_eq!(miner.seal(&child), None);

            let (id, seal) = found.recv_timeout(Duration::from_secs(10)).unwrap();

            assert_eq!(id, child.id());
            assert!(matches!(rules.check(&child, &seal, std::slice::from_ref(&parent), &[], timestamp), Ok(()) | Err(ConsensusError::WrongDifficulty { .. })));
            assert!(meets(Search::new(HashType::default_chain(), &id).work(match seal {
                Seal::Work { nonce } => nonce,
                _ => unreachable!(),
            }), 500));
        }

        drop(miner);
    }

    #[test]
    fn any_secure_work_hash_seals_and_checks() {
        let parent = header(5, 10_000, 64);
        let mut works = Vec::new();

        for &algorithm in HashType::ALL.iter().filter(|algorithm| algorithm.secure()) {
            let rules = ProofOfWork::new(PowConfig {
                hash: algorithm,
                target_block_ms: TARGET,
                initial_difficulty: 1_000,
                min_difficulty: 1,
            }, None);
            let mut child = header(6, 10_100, 0);
            child.parent = parent.id();
            child.difficulty = rules.difficulty(std::slice::from_ref(&parent), child.timestamp);
            let search = Search::new(algorithm, &child.id());
            let nonce = (0..).find(|nonce| meets(search.work(*nonce), child.difficulty)).unwrap();

            assert_eq!(rules.check(&child, &Seal::Work { nonce }, std::slice::from_ref(&parent), &[], 10_100), Ok(()), "{}", algorithm.name());
            assert_eq!(search.work(nonce), work_hash(algorithm, &child.id(), nonce));
            works.push(search.work(0));
        }

        works.sort();
        works.dedup();
        assert_eq!(works.len(), HashType::ALL.iter().filter(|algorithm| algorithm.secure()).count());
    }
}

