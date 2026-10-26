//! Mining on the GPU: a thread for each GPU hands the current template's work to it a batch of
//! nonces at a time, through the chainv4-gpu crate, and reports the seal it finds. With several
//! GPUs, each takes every so many batches, so none tries another's nonces. The GPU hashes exactly
//! what the CPU miner hashes, so a seal it finds passes ProofOfWork::check.

use {
    super::pow::{Rig, Shared, work_prefix, work_target},
    super::Producer,
    crate::registry::validators::Member,
    crate::structs::address::Address,
    crate::structs::block::{BlockHeader, Seal},
    crate::structs::hash::{Hash, HashType},
    chainv4_gpu::{BATCH, Gpu, WorkHash},
    r#macro::document,
    std::sync::Arc,
    std::time::Instant,
};

/// How often the thread logs its hashrate.
const REPORT_EVERY: std::time::Duration = std::time::Duration::from_secs(30);

#[document(
    summary = "Mines on the GPU, through wgpu on Vulkan, Metal or WebGPU: every GPU there is, or those picked. seal hands the header to a thread for each GPU, which tries nonces on it a batch at a time, and returns at once; the seal arrives through the callback. A new header replaces the old one, and dropping the miner stops the threads.",
    category = "Consensus",
    see_also = "Miner, ProofOfWork, Producer, Rig",
)]
pub struct GpuMiner {
    address: Address,
    rig: Rig,
}

impl GpuMiner {
    #[document(
        parent = "GpuMiner",
        summary = "Opens the GPUs and starts a mining thread on each, idle until the first header.",
        params = {
            address = "Where rewards for blocks this miner finds go.",
            hash = "The chain's work hash, from genesis.",
            which = "The GPUs to mine on, by their place in chainv4_gpu::list; None for every one.",
            found = "Called from a mining thread with the block id and its seal when a nonce is found.",
        },
        returns = "The miner, or why the GPUs could not be used: none found, one that does not open, a kernel that did not compile, or a work hash the GPU does not mine.",
    )]
    pub fn new(address: Address, hash: HashType, which: Option<&[usize]>, found: impl Fn(Hash, Seal) + Send + Sync + 'static) -> Result<GpuMiner, String> {
        Ok(GpuMiner {
            address,
            rig: Rig::gpu(hash, which, found)?,
        })
    }
}

impl Rig {
    #[document(
        parent = "Rig",
        summary = "Opens the GPUs and starts a mining thread on each, idle until the first work. In a build with the gpu feature.",
        params = {
            hash = "The chain's work hash, from genesis.",
            which = "The GPUs to mine on, by their place in chainv4_gpu::list; None for every one.",
            found = "Called from a mining thread with the block id and its seal when a nonce is found.",
        },
        returns = "The rig, or why the GPUs could not be used: none found, one that does not open, a kernel that did not compile, or a work hash the GPU does not mine.",
    )]
    pub fn gpu(hash: HashType, which: Option<&[usize]>, found: impl Fn(Hash, Seal) + Send + Sync + 'static) -> Result<Rig, String> {
        let work_hash = work_hash(hash).ok_or_else(|| format!("the GPU does not mine {}", hash.name()))?;

        for (index, name) in chainv4_gpu::list().iter().enumerate() {
            log::info!("[gpu] {index}: {name}");
        }

        let gpus = Gpu::open_all(which).map_err(|err| err.to_string())?;
        let (shared, found, count) = (Arc::new(Shared::new()), Arc::new(found), gpus.len() as u64);
        // Built as it goes, so a thread that does not start stops those that did.
        let mut rig = Rig {
            shared,
            threads: Vec::new(),
            devices: Vec::new(),
        };

        for (index, gpu) in gpus.into_iter().enumerate() {
            log::info!("[gpu] mining {} on {}", hash.name(), gpu.name());
            let meter = Arc::new(crate::consensus::Meter::default());
            rig.devices.push((gpu.name().to_string(), meter.clone()));
            let (worker, found) = (rig.shared.clone(), found.clone());
            let thread = std::thread::Builder::new()
                .name(format!("gpu-miner-{index}"))
                .spawn(move || mine(&worker, &gpu, work_hash, index as u64, count, &*found, &meter))
                .map_err(|err| format!("the GPU mining thread did not start: {err}"))?;
            rig.threads.push(thread);
        }

        Ok(rig)
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

    fn mining(&self) -> Option<crate::consensus::MiningReport> {
        Some(crate::consensus::MiningReport {
            devices: self.rig.devices(),
            ..Default::default()
        })
    }
}

/// One GPU's mining thread: for each job, batches of nonces, the index-th of every count from the
/// job's start up, until one is found, the job changes, or the miner stops. A GPU that fails ends its thread;
/// the others go on, and the log says so.
#[allow(clippy::too_many_arguments)]
fn mine(shared: &Shared, gpu: &Gpu, hash: WorkHash, index: u64, count: u64, found: &(dyn Fn(Hash, Seal) + Send + Sync), meter: &crate::consensus::Meter) {
    let mut finished = 0;
    let mut hashed: u64 = 0;
    let mut since = Instant::now();

    while let Some((generation, id, difficulty, first)) = shared.next_job(finished) {
        let prefix = work_prefix(&id);
        let target = work_target(difficulty);
        let mut start = first.wrapping_add(index * u64::from(BATCH));

        loop {
            match gpu.search(hash, &prefix, &target, start, BATCH) {
                Ok(Some(nonce)) => {
                    shared.finish(generation, id, nonce, found);

                    break;
                }
                Ok(None) => {}
                Err(err) => {
                    log::error!("[gpu] {}: {err}; mining on it stops", gpu.name());

                    return;
                }
            }

            hashed += u64::from(BATCH);
            meter.add(u64::from(BATCH));
            start = start.wrapping_add(count * u64::from(BATCH));

            if since.elapsed() >= REPORT_EVERY {
                log::info!("[gpu] {}: {:.1} MH/s", gpu.name(), hashed as f64 / since.elapsed().as_secs_f64() / 1e6);
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
        let mut miner = match GpuMiner::new(Address::Ed25519([1; 32]), HashType::default_chain(), None, move |id, seal| {
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
