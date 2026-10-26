//! Mining on the GPU: one thread hands the current template's work to the GPU a batch of nonces
//! at a time, through the chainv4-gpu crate, and reports the seal it finds. The GPU hashes exactly
//! what the CPU miner hashes, so a seal it finds passes ProofOfWork::check.

use {
    super::pow::{Shared, work_prefix, work_target},
    super::Producer,
    crate::registry::signers::Signer,
    crate::structs::address::Address,
    crate::structs::block::{BlockHeader, Seal},
    crate::structs::hash::{Hash, HashType},
    chainv4_gpu::{BATCH, Gpu, WorkHash},
    r#macro::document,
    std::sync::Arc,
    std::thread::JoinHandle,
    std::time::Instant,
};

/// How often the thread logs its hashrate.
const REPORT_EVERY: std::time::Duration = std::time::Duration::from_secs(30);

#[document(
    summary = "Mines on the GPU, through wgpu on Vulkan, Metal or WebGPU. seal hands the header to a thread that tries nonces on the GPU a batch at a time, and returns at once; the seal arrives through the callback. A new header replaces the old one, and dropping the miner stops the thread.",
    category = "Consensus",
    see_also = "Miner, ProofOfWork, Producer",
)]
pub struct GpuMiner {
    address: Address,
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl GpuMiner {
    #[document(
        parent = "GpuMiner",
        summary = "Opens the GPU and starts the mining thread, idle until the first header.",
        params = {
            address = "Where rewards for blocks this miner finds go.",
            hash = "The chain's work hash, from genesis.",
            found = "Called from the mining thread with the block id and its seal when a nonce is found.",
        },
        returns = "The miner, or why the GPU could not be used: no device, a kernel that did not compile, or a work hash the GPU does not mine.",
    )]
    pub fn new(address: Address, hash: HashType, found: impl Fn(Hash, Seal) + Send + Sync + 'static) -> Result<GpuMiner, String> {
        let work_hash = work_hash(hash).ok_or_else(|| format!("the GPU does not mine {}", hash.name()))?;
        let gpu = Gpu::open().map_err(|err| err.to_string())?;
        log::info!("[gpu] mining {} on {}", hash.name(), gpu.name());
        let shared = Arc::new(Shared::new());
        let worker = shared.clone();
        let thread = std::thread::Builder::new()
            .name("gpu-miner".to_string())
            .spawn(move || mine(&worker, &gpu, work_hash, &found))
            .map_err(|err| format!("the GPU mining thread did not start: {err}"))?;

        Ok(GpuMiner {
            address,
            shared,
            thread: Some(thread),
        })
    }
}

/// The GPU crate's name for a work hash; None for one it does not mine, which no genesis names.
fn work_hash(hash: HashType) -> Option<WorkHash> {
    Some(match hash.name() {
        "sha256" => WorkHash::Sha256,
        "blake3" => WorkHash::Blake3,
        "blake2s" => WorkHash::Blake2s,
        "blake2b" => WorkHash::Blake2b,
        "keccak256" => WorkHash::Keccak256,
        _ => return None,
    })
}

impl Producer for GpuMiner {
    fn address(&self) -> Address {
        self.address
    }

    /// Timestamps must move forward, so the next millisecond after the parent's.
    fn start_at(&self, parent: &BlockHeader, _producers: &[Signer]) -> u64 {
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

impl Drop for GpuMiner {
    fn drop(&mut self) {
        self.shared.lock().stopped = true;
        self.shared.changed.notify_all();

        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The mining thread: for each job, batches of nonces on the GPU from 0 up, until one is found,
/// the job changes, or the miner stops. A GPU that fails ends the thread, and the node mines no
/// more until restarted; the log says so.
fn mine(shared: &Shared, gpu: &Gpu, hash: WorkHash, found: &(dyn Fn(Hash, Seal) + Send + Sync)) {
    let mut finished = 0;
    let mut hashed: u64 = 0;
    let mut since = Instant::now();

    while let Some((generation, id, difficulty)) = shared.next_job(finished) {
        let prefix = work_prefix(&id);
        let target = work_target(difficulty);
        let mut start = 0u64;

        loop {
            match gpu.search(hash, &prefix, &target, start, BATCH) {
                Ok(Some(nonce)) => {
                    shared.finish(generation, id, nonce, found);

                    break;
                }
                Ok(None) => {}
                Err(err) => {
                    log::error!("[gpu] {err}; mining on the GPU stops");

                    return;
                }
            }

            hashed += u64::from(BATCH);
            start = start.wrapping_add(u64::from(BATCH));

            if since.elapsed() >= REPORT_EVERY {
                log::info!("[gpu] {:.1} MH/s", hashed as f64 / since.elapsed().as_secs_f64() / 1e6);
                hashed = 0;
                since = Instant::now();
            }

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
    use crate::consensus::{Consensus, PowConfig, ProofOfWork};
    use crate::structs::u256::u256;
    use std::sync::{Mutex, mpsc};
    use std::time::Duration;

    /// A header on a chain hashed with the build's default.
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
            receipts_root: Hash::zero(chain),
            state_root: Hash::zero(chain),
            gas_used: 0,
            base_price: u256::ZERO,
        }
    }

    #[test]
    fn the_gpu_miner_finds_seals_the_rules_accept() {
        let (sender, found) = mpsc::channel();
        let sender = Mutex::new(sender);
        let mut miner = match GpuMiner::new(Address::Ed25519([1; 32]), HashType::default_chain(), move |id, seal| {
            let _ = sender.lock().unwrap().send((id, seal));
        }) {
            Ok(miner) => miner,
            Err(why) if why.contains("no GPU found") => {
                eprintln!("{why}: skipping");

                return;
            }
            Err(why) => panic!("{why}"),
        };
        let rules = ProofOfWork::new(PowConfig {
            hash: HashType::default_chain(),
            target_block_ms: 1_000,
            initial_difficulty: 1_000,
            min_difficulty: 1,
        }, None);
        let parent = header(5, 10_000, 500);

        for timestamp in [10_100, 10_200] {
            let mut child = header(6, timestamp, 0);
            child.parent = parent.id();
            child.difficulty = rules.difficulty(std::slice::from_ref(&parent), timestamp);

            assert_eq!(miner.start_at(&parent, &[]), 10_001);
            assert_eq!(miner.seal(&child), None);

            let (id, seal) = found.recv_timeout(Duration::from_secs(30)).unwrap();

            assert_eq!(id, child.id());
            assert_eq!(rules.check(&child, &seal, std::slice::from_ref(&parent), &[], timestamp), Ok(()));
        }

        drop(miner);
    }
}
