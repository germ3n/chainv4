use {
    super::{Consensus, ConsensusError, MAX_DRIFT_MS, Producer},
    crate::crypto::hashable::Hashable,
    crate::structs::address::Address,
    crate::structs::block::{BlockHeader, Seal},
    crate::structs::hash::Hash,
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
/// How many nonces a mining thread tries before checking whether its job is still current.
const NONCES_PER_CHECK: u64 = 4_096;

#[document(
    summary = "The hash proof of work is measured with. Part of genesis; new algorithms, such as memory-hard ones, can join under new tags without touching chains that use the old ones.",
    category = "Consensus",
    see_also = "PowConfig",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
#[wincode(tag_encoding = "u8")]
pub enum PowHash {
    #[document(summary = "BLAKE3 of a domain tag, the block id and the nonce.")]
    #[wincode(tag = 0)]
    Blake3,
}

impl Hashable for PowHash {
    const TAG_LEN: usize = 1;
}

#[document(
    summary = "Proof-of-work parameters, from genesis.",
    category = "Consensus",
    see_also = "ProofOfWork, ConsensusConfig::ProofOfWork",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct PowConfig {
    #[document(summary = "The work hash.")]
    pub hash: PowHash,
    #[document(summary = "How far apart blocks should be, in milliseconds. Difficulty rises when they come faster and falls when they come slower.")]
    pub target_block_ms: u64,
    #[document(summary = "The first block's difficulty: the expected number of hashes to find its seal.")]
    pub initial_difficulty: u128,
    #[document(summary = "Difficulty never falls below this.")]
    pub min_difficulty: u128,
}

impl Hashable for PowConfig {}

#[document(
    summary = "The rules of proof of work. Anyone may produce a block whose seal holds a nonce that makes the work hash of the block's id and nonce, read as a big-endian number, at most u256::MAX / difficulty. Difficulty adjusts every block from its parent alone: one step up when a block comes in under the target time, none within one to two targets, and a step down for each further target, at most 32.",
    category = "Consensus",
    see_also = "PowConfig, Miner, Consensus",
    note = "A block weighs its difficulty, so the main chain is the one that took the most work, not the longest.",
)]
pub struct ProofOfWork {
    config: PowConfig,
}

impl ProofOfWork {
    #[document(parent = "ProofOfWork", summary = "The rules for these parameters.")]
    pub fn new(config: PowConfig) -> ProofOfWork {
        ProofOfWork { config }
    }
}

impl Consensus for ProofOfWork {
    fn work(&self, header: &BlockHeader) -> u256 {
        u256::from(header.difficulty)
    }

    fn difficulty(&self, parent: &BlockHeader, timestamp: u64) -> u128 {
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

    fn check(&self, header: &BlockHeader, seal: &Seal, parent: &BlockHeader, now: u64) -> Result<(), ConsensusError> {
        if header.timestamp <= parent.timestamp {
            return Err(ConsensusError::Early);
        }

        if header.timestamp > now + MAX_DRIFT_MS {
            return Err(ConsensusError::FromTheFuture);
        }

        let expected = self.difficulty(parent, header.timestamp);

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
pub fn work_hash(hash: PowHash, id: &Hash, nonce: u64) -> u256 {
    Search::new(hash, id).work(nonce)
}

/// The work hash of one block id, ready to try nonces against.
struct Search {
    prefix: blake3::Hasher,
}

impl Search {
    fn new(hash: PowHash, id: &Hash) -> Search {
        match hash {
            PowHash::Blake3 => {
                let mut prefix = blake3::Hasher::new();
                prefix.update(WORK_DOMAIN);
                prefix.update(&id.to_bytes());

                Search { prefix }
            }
        }
    }

    fn work(&self, nonce: u64) -> u256 {
        let mut hasher = self.prefix.clone();
        hasher.update(&nonce.to_le_bytes());

        u256::from_be_bytes(*hasher.finalize().as_bytes())
    }
}

/// What the mining threads work on.
struct Job {
    /// Bumped for every new header, so threads notice when theirs is stale.
    generation: u64,
    /// The block id and difficulty to find a nonce for; None once one was found.
    work: Option<(Hash, u128)>,
    stopped: bool,
}

struct Shared {
    job: Mutex<Job>,
    changed: Condvar,
}

impl Shared {
    fn lock(&self) -> std::sync::MutexGuard<'_, Job> {
        self.job.lock().expect("mining threads never panic holding the job")
    }
}

#[document(
    summary = "Mines on threads of its own. seal hands the header to every thread, each trying its own share of the nonces, and returns at once; whichever thread finds a nonce first reports it through the callback. A new header replaces the old one, and dropping the miner stops the threads.",
    category = "Consensus",
    see_also = "ProofOfWork, Producer",
)]
pub struct Miner {
    address: Address,
    shared: Arc<Shared>,
    threads: Vec<JoinHandle<()>>,
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
    pub fn new(address: Address, hash: PowHash, threads: usize, found: impl Fn(Hash, Seal) + Send + Sync + 'static) -> Miner {
        let shared = Arc::new(Shared {
            job: Mutex::new(Job {
                generation: 0,
                work: None,
                stopped: false,
            }),
            changed: Condvar::new(),
        });
        let found = Arc::new(found);
        let count = threads.max(1);
        let threads = (0..count)
            .map(|index| {
                let shared = shared.clone();
                let found = found.clone();

                std::thread::Builder::new()
                    .name(format!("miner-{index}"))
                    .spawn(move || mine(&shared, hash, index as u64, count as u64, &*found))
                    .expect("the mining thread starts")
            })
            .collect();

        Miner {
            address,
            shared,
            threads,
        }
    }
}

impl Producer for Miner {
    fn address(&self) -> Address {
        self.address
    }

    /// Timestamps must move forward, so the next millisecond after the parent's.
    fn start_at(&self, parent: &BlockHeader) -> u64 {
        parent.timestamp + 1
    }

    fn seal(&mut self, header: &BlockHeader) -> Option<Seal> {
        let mut job = self.shared.lock();
        job.generation += 1;
        job.work = Some((header.id(), header.difficulty));
        self.shared.changed.notify_all();

        None
    }
}

impl Drop for Miner {
    fn drop(&mut self) {
        self.shared.lock().stopped = true;
        self.shared.changed.notify_all();

        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

/// One mining thread: tries nonces index, index + stride, … on the current job until it finds
/// one, the job changes, or the miner stops.
fn mine(shared: &Shared, hash: PowHash, index: u64, stride: u64, found: &(dyn Fn(Hash, Seal) + Send + Sync)) {
    let mut finished = 0;

    loop {
        let (generation, id, difficulty) = {
            let mut job = shared.lock();

            while !job.stopped && (job.work.is_none() || job.generation == finished) {
                job = shared.changed.wait(job).expect("mining threads never panic holding the job");
            }

            if job.stopped {
                return;
            }

            let (id, difficulty) = job.work.expect("checked above");

            (job.generation, id, difficulty)
        };
        let search = Search::new(hash, &id);
        let mut nonce = index;

        'search: loop {
            for _ in 0..NONCES_PER_CHECK {
                if meets(search.work(nonce), difficulty) {
                    let mut job = shared.lock();

                    // Only the first find of a job counts.
                    if job.generation == generation && job.work.is_some() {
                        job.work = None;
                        drop(job);
                        found(id, Seal::Work { nonce });
                    }

                    break 'search;
                }

                nonce = nonce.wrapping_add(stride);
            }

            let job = shared.lock();

            if job.stopped || job.generation != generation || job.work.is_none() {
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
            hash: PowHash::Blake3,
            target_block_ms: TARGET,
            initial_difficulty: 1_000,
            min_difficulty: min,
        })
    }

    fn header(height: u64, timestamp: u64, difficulty: u128) -> BlockHeader {
        BlockHeader {
            parent: Hash::Blake3_256([height as u8; 32]),
            height,
            timestamp,
            producer: Address::Ed25519([1; 32]),
            difficulty,
            tx_root: Hash::Blake3_256([0; 32]),
        }
    }

    /// Finds a nonce on this thread.
    fn solve(header: &BlockHeader) -> Seal {
        let search = Search::new(PowHash::Blake3, &header.id());

        (0..)
            .find(|nonce| meets(search.work(*nonce), header.difficulty))
            .map(|nonce| Seal::Work { nonce })
            .unwrap()
    }

    #[test]
    fn difficulty_follows_the_block_time() {
        let rules = rules(10);
        let parent = header(5, 10_000, 6_400);

        assert_eq!(rules.difficulty(&header(0, 0, 0), 5), 1_000);
        assert_eq!(rules.difficulty(&parent, 10_000 + TARGET / 2), 6_500);
        assert_eq!(rules.difficulty(&parent, 10_000 + TARGET), 6_400);
        assert_eq!(rules.difficulty(&parent, 10_000 + TARGET * 3), 6_200);
        assert_eq!(rules.difficulty(&parent, 10_000 + TARGET * 1_000), 6_400 - 100 * 32);
        assert_eq!(rules.difficulty(&header(5, 10_000, 12), 10_000 + TARGET * 50), 10);
    }

    #[test]
    fn a_found_nonce_passes_and_anything_else_fails() {
        let rules = rules(1);
        let parent = header(5, 10_000, 64);
        let mut child = header(6, 10_100, 0);
        child.parent = parent.id();
        child.difficulty = rules.difficulty(&parent, child.timestamp);
        let seal = solve(&child);
        let Seal::Work { nonce } = seal else { unreachable!() };
        let wrong = (0..)
            .map(|offset| Seal::Work { nonce: nonce.wrapping_add(offset + 1) })
            .find(|candidate| {
                let Seal::Work { nonce } = candidate else { unreachable!() };
                !meets(Search::new(PowHash::Blake3, &child.id()).work(*nonce), child.difficulty)
            })
            .unwrap();
        let easy = BlockHeader { difficulty: 1, ..child.clone() };

        assert_eq!(rules.check(&child, &seal, &parent, 10_100), Ok(()));
        assert_eq!(rules.check(&child, &wrong, &parent, 10_100), Err(ConsensusError::NotEnoughWork));
        assert!(matches!(rules.check(&easy, &solve(&easy), &parent, 10_100), Err(ConsensusError::WrongDifficulty { .. })));
        assert_eq!(rules.check(&child, &Seal::None, &parent, 10_100), Err(ConsensusError::BadSeal));
        assert_eq!(rules.check(&header(6, 10_000, 0), &seal, &parent, 10_100), Err(ConsensusError::Early));
        assert_eq!(rules.work(&child), u256::from(child.difficulty));
    }

    #[test]
    fn the_miner_finds_seals_and_moves_on_to_new_headers() {
        let (sender, found) = mpsc::channel();
        let sender = Mutex::new(sender);
        let mut miner = Miner::new(Address::Ed25519([1; 32]), PowHash::Blake3, 2, move |id, seal| {
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
            assert!(matches!(rules.check(&child, &seal, &parent, timestamp), Ok(()) | Err(ConsensusError::WrongDifficulty { .. })));
            assert!(meets(Search::new(PowHash::Blake3, &id).work(match seal {
                Seal::Work { nonce } => nonce,
                _ => unreachable!(),
            }), 500));
        }

        drop(miner);
    }
}
