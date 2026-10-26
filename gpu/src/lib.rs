//! Proof-of-work mining for chainv4 on the GPU, through wgpu: on Vulkan, Metal and WebGPU, with
//! one compute kernel per work hash the chain may name. A seal the GPU finds is one the node's CPU
//! check accepts: the same `"chainv4 work"`, block id and nonce, hashed the same way, read as a
//! big-endian number and held to the same target. The tests here hash against the CPU crates.
//!
//! The node does not know wgpu: it hands [`Gpu::search`] the first 44 bytes of the work message
//! and the target, and gets the lowest nonce of a range that meets it, as its own threads would
//! find first.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// How many nonces one dispatch tries: 64 invocations a workgroup, 16,384 workgroups.
pub const BATCH: u32 = 1 << 20;

/// What a work message is before its nonce: `"chainv4 work"` and the block's 32-byte id.
pub const PREFIX_LEN: usize = 44;

/// The work hashes the GPU mines, every secure hash a genesis may name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WorkHash {
    Sha256,
    Blake3,
    Blake2s,
    Blake2b,
    Keccak256,
}

impl WorkHash {
    /// Every work hash.
    pub const ALL: [WorkHash; 5] = [WorkHash::Sha256, WorkHash::Blake3, WorkHash::Blake2s, WorkHash::Blake2b, WorkHash::Keccak256];

    /// The hash's name, as genesis files spell it.
    pub fn name(self) -> &'static str {
        match self {
            WorkHash::Sha256 => "sha256",
            WorkHash::Blake3 => "blake3",
            WorkHash::Blake2s => "blake2s",
            WorkHash::Blake2b => "blake2b",
            WorkHash::Keccak256 => "keccak256",
        }
    }

    /// The hash's kernel: the shared search over nonces, and its digest.
    fn source(self) -> String {
        let kernel = match self {
            WorkHash::Sha256 => include_str!("kernels/sha256.wgsl").to_string(),
            WorkHash::Blake3 => include_str!("kernels/blake3.wgsl").to_string(),
            WorkHash::Blake2s => include_str!("kernels/blake2s.wgsl").to_string(),
            WorkHash::Blake2b => format!("{}\n{}", include_str!("kernels/u64.wgsl"), include_str!("kernels/blake2b.wgsl")),
            WorkHash::Keccak256 => format!("{}\n{}", include_str!("kernels/u64.wgsl"), include_str!("kernels/keccak256.wgsl")),
        };

        format!("{}\n{kernel}", include_str!("kernels/prelude.wgsl"))
    }
}

/// Why the GPU could not be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GpuError {
    /// No GPU this build's backends can drive: no Vulkan, Metal or WebGPU device.
    NoAdapter,
    /// The device could not be opened, or failed while searching.
    Device(String),
    /// A kernel did not compile on this device.
    Kernel(String),
}

impl std::fmt::Display for GpuError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GpuError::NoAdapter => write!(f, "no GPU found: none of Vulkan, Metal or WebGPU gave an adapter"),
            GpuError::Device(why) => write!(f, "the GPU failed: {why}"),
            GpuError::Kernel(why) => write!(f, "a kernel did not compile: {why}"),
        }
    }
}

impl std::error::Error for GpuError {}

/// A GPU ready to search for nonces, with a compiled kernel for every work hash.
pub struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    name: String,
    pipelines: Vec<(WorkHash, wgpu::ComputePipeline, wgpu::BindGroup)>,
    job: wgpu::Buffer,
    found: wgpu::Buffer,
    readback: wgpu::Buffer,
    /// Set when the device reports an error, so the next search fails rather than returns nothing.
    failed: Arc<Mutex<Option<String>>>,
    /// Whether a search is in flight: one at a time, the buffers being shared.
    busy: AtomicBool,
}

/// The job as the kernels read it: 11 prefix words, 8 target words, the first nonce, the count.
const JOB_BYTES: u64 = (11 + 8 + 4) * 4;

impl Gpu {
    /// Opens the most powerful GPU wgpu finds and compiles every kernel on it.
    pub fn open() -> Result<Gpu, GpuError> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            ..Default::default()
        }))
        .ok_or(GpuError::NoAdapter)?;
        let info = adapter.get_info();
        let name = format!("{} ({:?})", info.name, info.backend);
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("chainv4 miner"),
                ..Default::default()
            },
            None,
        ))
        .map_err(|err| GpuError::Device(err.to_string()))?;
        let failed = Arc::new(Mutex::new(None));
        let report = failed.clone();
        device.on_uncaptured_error(Box::new(move |err| {
            // The first error says what went wrong; the ones that follow are its consequences.
            report.lock().expect("the error report is never poisoned").get_or_insert_with(|| err.to_string());
        }));
        let job = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("job"),
            size: JOB_BYTES,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let found = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("found"),
            size: 4,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: 4,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut pipelines = Vec::new();

        for hash in WorkHash::ALL {
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(hash.name()),
                source: wgpu::ShaderSource::Wgsl(hash.source().into()),
            });
            let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(hash.name()),
                layout: None,
                module: &module,
                entry_point: Some("main"),
                compilation_options: Default::default(),
                cache: None,
            });

            if let Some(why) = failed.lock().expect("the error report is never poisoned").take() {
                return Err(GpuError::Kernel(format!("{}: {why}", hash.name())));
            }

            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(hash.name()),
                layout: &pipeline.get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: job.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: found.as_entire_binding(),
                    },
                ],
            });

            if let Some(why) = failed.lock().expect("the error report is never poisoned").take() {
                return Err(GpuError::Kernel(format!("{}: {why}", hash.name())));
            }

            pipelines.push((hash, pipeline, bind_group));
        }

        Ok(Gpu {
            device,
            queue,
            name,
            pipelines,
            job,
            found,
            readback,
            failed,
            busy: AtomicBool::new(false),
        })
    }

    /// The device's name and backend, such as "Apple M3 Pro (Metal)".
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Tries `count` nonces from `start` on, at most [`BATCH`], and returns the lowest whose work
    /// hash is at most the target.
    ///
    /// - `prefix`: the work message before the nonce, `"chainv4 work"` and the block id.
    /// - `target`: the most the hash may be, as 32 big-endian bytes: `u256::MAX / difficulty`.
    pub fn search(&self, hash: WorkHash, prefix: &[u8; PREFIX_LEN], target: &[u8; 32], start: u64, count: u32) -> Result<Option<u64>, GpuError> {
        let count = count.min(BATCH);

        if count == 0 {
            return Ok(None);
        }

        if self.busy.swap(true, Ordering::AcqRel) {
            return Err(GpuError::Device("a search is already running".to_string()));
        }

        let result = self.run(hash, prefix, target, start, count);
        self.busy.store(false, Ordering::Release);

        result
    }

    fn run(&self, hash: WorkHash, prefix: &[u8; PREFIX_LEN], target: &[u8; 32], start: u64, count: u32) -> Result<Option<u64>, GpuError> {
        let (_, pipeline, bind_group) = self
            .pipelines
            .iter()
            .find(|(named, _, _)| *named == hash)
            .expect("every work hash has a pipeline");
        let mut job = Vec::with_capacity(JOB_BYTES as usize);

        for word in prefix.chunks(4) {
            job.extend_from_slice(word);
        }

        for word in target.chunks(4) {
            job.extend_from_slice(&u32::from_be_bytes(word.try_into().expect("four bytes")).to_le_bytes());
        }

        job.extend_from_slice(&(start as u32).to_le_bytes());
        job.extend_from_slice(&((start >> 32) as u32).to_le_bytes());
        job.extend_from_slice(&count.to_le_bytes());
        job.extend_from_slice(&0u32.to_le_bytes());
        self.queue.write_buffer(&self.job, 0, &job);
        self.queue.write_buffer(&self.found, 0, &u32::MAX.to_le_bytes());

        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("search") });

        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("search"),
                timestamp_writes: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, bind_group, &[]);
            pass.dispatch_workgroups(count.div_ceil(64), 1, 1);
        }

        encoder.copy_buffer_to_buffer(&self.found, 0, &self.readback, 0, 4);
        self.queue.submit([encoder.finish()]);

        let (sender, receiver) = std::sync::mpsc::channel();
        self.readback.slice(..).map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        if let wgpu::MaintainResult::SubmissionQueueEmpty = self.device.poll(wgpu::Maintain::Wait) {}

        receiver
            .recv()
            .map_err(|_| GpuError::Device("the search never finished".to_string()))?
            .map_err(|err| GpuError::Device(err.to_string()))?;
        let index = {
            let view = self.readback.slice(..).get_mapped_range();

            u32::from_le_bytes(view[..4].try_into().expect("four bytes"))
        };
        self.readback.unmap();

        if let Some(why) = self.failed.lock().expect("the error report is never poisoned").take() {
            return Err(GpuError::Device(why));
        }

        Ok(match index {
            u32::MAX => None,
            found => Some(start + u64::from(found - 1)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The work hash of a message on the CPU, as the chain computes it, as a big-endian number.
    fn cpu(hash: WorkHash, message: &[u8]) -> [u8; 32] {
        match hash {
            WorkHash::Sha256 => <sha2::Sha256 as sha2::Digest>::digest(message).into(),
            WorkHash::Blake3 => *blake3::hash(message).as_bytes(),
            WorkHash::Blake2s => <blake2::Blake2s256 as blake2::Digest>::digest(message).into(),
            WorkHash::Blake2b => <blake2::Blake2b256 as blake2::Digest>::digest(message).into(),
            WorkHash::Keccak256 => <sha3::Keccak256 as sha3::Digest>::digest(message).into(),
        }
    }

    fn message(prefix: &[u8; PREFIX_LEN], nonce: u64) -> Vec<u8> {
        let mut message = prefix.to_vec();
        message.extend_from_slice(&nonce.to_le_bytes());

        message
    }

    fn prefix() -> [u8; PREFIX_LEN] {
        let mut prefix = [0; PREFIX_LEN];
        prefix[..12].copy_from_slice(b"chainv4 work");
        prefix[12..].iter_mut().enumerate().for_each(|(index, byte)| *byte = (index * 7 + 3) as u8);

        prefix
    }

    /// The GPU, or None on a machine without one, where these tests have nothing to run on.
    fn gpu() -> Option<Gpu> {
        match Gpu::open() {
            Ok(gpu) => Some(gpu),
            Err(GpuError::NoAdapter) => {
                eprintln!("no GPU here: skipping");

                None
            }
            Err(err) => panic!("{err}"),
        }
    }

    #[test]
    fn every_kernel_hashes_as_the_cpu_does() {
        let Some(gpu) = gpu() else { return };
        let prefix = prefix();

        for hash in WorkHash::ALL {
            // A target equal to one nonce's own hash finds that nonce, or an earlier one whose
            // hash is lower: exactly what the CPU finds first.
            for nonce in [0u64, 1, 7, 63, 64, 1_000, u64::from(u32::MAX) + 5] {
                let target = cpu(hash, &message(&prefix, nonce));
                let start = nonce.saturating_sub(100);
                let expected = (start..=nonce).find(|candidate| cpu(hash, &message(&prefix, *candidate)) <= target);

                assert_eq!(gpu.search(hash, &prefix, &target, start, 200).unwrap(), expected, "{} at {nonce}", hash.name());
            }

            // Nothing meets a target of zero; everything meets one of all ones.
            assert_eq!(gpu.search(hash, &prefix, &[0; 32], 0, 1_000).unwrap(), None, "{}", hash.name());
            assert_eq!(gpu.search(hash, &prefix, &[0xff; 32], 5, 1_000).unwrap(), Some(5), "{}", hash.name());
        }
    }

    #[test]
    fn a_difficulty_is_met_where_the_cpu_meets_it() {
        let Some(gpu) = gpu() else { return };
        let prefix = prefix();
        // u256::MAX / 4_000: about one nonce in four thousand.
        let mut target = [0xff; 32];
        let quotient = u128::MAX / 4_000;
        target[..16].copy_from_slice(&quotient.to_be_bytes());

        for hash in WorkHash::ALL {
            let expected = (0u64..).find(|nonce| cpu(hash, &message(&prefix, *nonce)) <= target).unwrap();
            let mut found = None;
            let mut start = 0;

            while found.is_none() {
                found = gpu.search(hash, &prefix, &target, start, BATCH).unwrap();
                start += u64::from(BATCH);
            }

            assert_eq!(found, Some(expected), "{}", hash.name());
        }
    }

    #[test]
    fn a_batch_is_bounded_and_an_empty_search_finds_nothing() {
        let Some(gpu) = gpu() else { return };

        assert!(!gpu.name().is_empty());
        assert_eq!(gpu.search(WorkHash::Blake3, &prefix(), &[0xff; 32], 9, 0).unwrap(), None);
        assert_eq!(gpu.search(WorkHash::Blake3, &prefix(), &[0xff; 32], 9, u32::MAX).unwrap(), Some(9), "capped at BATCH, still found");
    }
}
