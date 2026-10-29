//! Proof-of-work mining for chainv4 on the GPU, through wgpu: on Vulkan, Metal and WebGPU, with
//! one compute kernel per work hash the chain may name. A seal the GPU finds is one the node's CPU
//! check accepts: the same `"chainv4 work"`, block id and nonce, hashed the same way, read as a
//! big-endian number and held to the same target. The tests here hash against the CPU crates.
//!
//! The node does not know wgpu: it hands [`Gpu::search`] the first 44 bytes of the work message
//! and the target, and gets the lowest nonce of a range that meets it, as its own threads would
//! find first.
//!
//! A machine may have several GPUs: [`list`] names each once, by its place in the list, and
//! [`Gpu::open_all`] opens those asked for, or every one.
//!
//! A browser, where nothing may block, has the GPU through WebGPU alone: [`Gpu::open_async`] opens
//! it and [`Gpu::search_async`] searches it, each awaited. The blocking calls wait on these same
//! futures, so both search alike.

use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

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

    /// The work hash a genesis names, by its name; None for one the GPU does not mine.
    pub fn named(name: &str) -> Option<WorkHash> {
        WorkHash::ALL.into_iter().find(|hash| hash.name() == name)
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

/// Every GPU wgpu can mine on, each once: a card reached through several backends, such as Vulkan
/// and OpenGL, under its native one. Discrete cards first, then integrated, then any other;
/// software renderers, which are the CPU, are left out. Not in a browser, which shows one alone.
#[cfg(not(target_arch = "wasm32"))]
fn adapters() -> Vec<wgpu::Adapter> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let mut found: Vec<Option<wgpu::Adapter>> = instance.enumerate_adapters(wgpu::Backends::all()).into_iter().map(Some).collect();
    let infos: Vec<wgpu::AdapterInfo> = found.iter().flatten().map(wgpu::Adapter::get_info).collect();

    chosen(&infos).into_iter().filter_map(|index| found[index].take()).collect()
}

/// Which of these adapters to mine on, by their place, in the order to list them: each GPU once,
/// under its native backend. The same card through several backends sorts its best first and
/// the rest go; OpenGL reports other vendor and device ids than Vulkan, Metal or DX12 for the same
/// card, as Android's does, so an OpenGL adapter named as a native one is the same card and goes
/// too.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn chosen(infos: &[wgpu::AdapterInfo]) -> Vec<usize> {
    let native = |backend: wgpu::Backend| matches!(backend, wgpu::Backend::Vulkan | wgpu::Backend::Metal | wgpu::Backend::Dx12);
    let rank = |backend: wgpu::Backend| match backend {
        _ if native(backend) => 0,
        wgpu::Backend::Gl => 1,
        _ => 2,
    };
    let kind = |device: wgpu::DeviceType| match device {
        wgpu::DeviceType::DiscreteGpu => 0,
        wgpu::DeviceType::IntegratedGpu => 1,
        _ => 2,
    };
    let mut order: Vec<usize> = (0..infos.len()).filter(|&index| infos[index].device_type != wgpu::DeviceType::Cpu).collect();
    order.sort_by_key(|&index| {
        let info = &infos[index];

        (kind(info.device_type), info.vendor, info.device, info.name.clone(), rank(info.backend))
    });
    // After sorting, a card's backends sit side by side, its best first.
    order.dedup_by_key(|index| {
        let info = &infos[*index];

        (info.vendor, info.device, info.name.clone())
    });
    // OpenGL's name for a card is its native name, or that and more, such as "/PCIe/SSE2".
    let same_card = |gl: &str, native: &str| {
        let (gl, native) = (gl.trim().to_lowercase(), native.trim().to_lowercase());

        !native.is_empty() && gl.strip_prefix(&native).is_some_and(|rest| rest.chars().next().is_none_or(|next| !next.is_alphanumeric()))
    };
    let natives: Vec<String> = order.iter().map(|&index| &infos[index]).filter(|info| native(info.backend)).map(|info| info.name.clone()).collect();
    order.retain(|&index| {
        let info = &infos[index];

        info.backend != wgpu::Backend::Gl || !natives.iter().any(|name| same_card(&info.name, name))
    });

    order
}

/// The GPUs this machine has, by their place in the list, as [`Gpu::open_all`] takes it: each
/// one's name and backend, such as "Apple M3 Pro (Metal)". Not in a browser.
#[cfg(not(target_arch = "wasm32"))]
pub fn list() -> Vec<String> {
    adapters().iter().map(|adapter| describe(&adapter.get_info())).collect()
}

fn describe(info: &wgpu::AdapterInfo) -> String {
    format!("{} ({:?})", info.name, info.backend)
}

impl Gpu {
    /// Opens the most powerful GPU wgpu finds and compiles every kernel on it.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn open() -> Result<Gpu, GpuError> {
        pollster::block_on(Gpu::open_async(&WorkHash::ALL))
    }

    /// Opens the most powerful GPU wgpu finds, without blocking: in a browser, the one WebGPU
    /// gives. Compiles the kernels of these work hashes alone, as a browser compiles slowly; a
    /// search for another fails.
    pub async fn open_async(hashes: &[WorkHash]) -> Result<Gpu, GpuError> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                ..Default::default()
            })
            .await
            .ok_or(GpuError::NoAdapter)?;

        Gpu::on(adapter, hashes).await
    }

    /// Opens the GPUs at these places in [`list`], or every one, compiling every kernel on each.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn open_all(which: Option<&[usize]>) -> Result<Vec<Gpu>, GpuError> {
        let mut found: Vec<Option<wgpu::Adapter>> = adapters().into_iter().map(Some).collect();

        if found.is_empty() {
            return Err(GpuError::NoAdapter);
        }

        let indices: Vec<usize> = match which {
            Some(which) => which.to_vec(),
            None => (0..found.len()).collect(),
        };
        let (count, mut opened) = (found.len(), Vec::new());

        for index in indices {
            let adapter = found
                .get_mut(index)
                .ok_or_else(|| GpuError::Device(format!("there is no GPU {index}: this machine has {count}")))?
                .take()
                .ok_or_else(|| GpuError::Device(format!("GPU {index} was asked for twice")))?;
            opened.push(pollster::block_on(Gpu::on(adapter, &WorkHash::ALL))?);
        }

        Ok(opened)
    }

    /// Opens an adapter's device and compiles these hashes' kernels on it. Each kernel compiles in
    /// an error scope, as a browser reports a kernel's errors only once asked.
    async fn on(adapter: wgpu::Adapter, hashes: &[WorkHash]) -> Result<Gpu, GpuError> {
        let info = adapter.get_info();
        let name = describe(&info);
        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("chainv4 miner"),
                    ..Default::default()
                },
                None,
            )
            .await
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

        for &hash in hashes {
            if pipelines.iter().any(|(compiled, _, _)| *compiled == hash) {
                continue;
            }

            device.push_error_scope(wgpu::ErrorFilter::Validation);
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

            if let Some(err) = device.pop_error_scope().await {
                return Err(GpuError::Kernel(format!("{}: {err}", hash.name())));
            }

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
    #[cfg(not(target_arch = "wasm32"))]
    pub fn search(&self, hash: WorkHash, prefix: &[u8; PREFIX_LEN], target: &[u8; 32], start: u64, count: u32) -> Result<Option<u64>, GpuError> {
        pollster::block_on(self.search_async(hash, prefix, target, start, count))
    }

    /// As [`Gpu::search`], without blocking: in a browser, awaiting the GPU lets the page go on
    /// meanwhile. One search at a time; a search dropped part way gives the GPU back.
    pub async fn search_async(&self, hash: WorkHash, prefix: &[u8; PREFIX_LEN], target: &[u8; 32], start: u64, count: u32) -> Result<Option<u64>, GpuError> {
        let count = count.min(BATCH);

        if count == 0 {
            return Ok(None);
        }

        if self.busy.swap(true, Ordering::AcqRel) {
            return Err(GpuError::Device("a search is already running".to_string()));
        }

        let mut searching = Searching { gpu: self, mapping: false };

        searching.run(hash, prefix, target, start, count).await
    }

    /// Writes the job, dispatches the kernel over `count` nonces and copies what it found to the
    /// readback buffer.
    fn submit(&self, hash: WorkHash, prefix: &[u8; PREFIX_LEN], target: &[u8; 32], start: u64, count: u32) -> Result<(), GpuError> {
        let (_, pipeline, bind_group) = self
            .pipelines
            .iter()
            .find(|(named, _, _)| *named == hash)
            .ok_or_else(|| GpuError::Kernel(format!("{}: not compiled on this GPU", hash.name())))?;
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

        Ok(())
    }

    /// The nonce the mapped readback buffer holds, once read and unmapped: None for none found.
    fn read(&self, start: u64) -> Result<Option<u64>, GpuError> {
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

/// A search in flight. Dropped, it frees the GPU for the next: should the search be dropped while
/// the readback is still being mapped, the mapping is undone, or the next search could not map it.
struct Searching<'a> {
    gpu: &'a Gpu,
    mapping: bool,
}

impl Searching<'_> {
    async fn run(&mut self, hash: WorkHash, prefix: &[u8; PREFIX_LEN], target: &[u8; 32], start: u64, count: u32) -> Result<Option<u64>, GpuError> {
        self.gpu.submit(hash, prefix, target, start, count)?;

        let mapped = Mapped::default();
        let done = mapped.0.clone();
        self.mapping = true;
        self.gpu.readback.slice(..).map_async(wgpu::MapMode::Read, move |result| {
            let mut done = done.lock().expect("the mapping's end is never poisoned");
            done.result = Some(result);

            if let Some(waker) = done.waker.take() {
                waker.wake();
            }
        });
        // Natively the mapping ends here, once the GPU is done; a browser ends it by itself, and
        // the future below waits for it.
        if let wgpu::MaintainResult::SubmissionQueueEmpty = self.gpu.device.poll(wgpu::Maintain::Wait) {}

        let result = mapped.await;
        // Mapped, or failing to: either way there is no mapping left to undo.
        self.mapping = false;
        result.map_err(|err| GpuError::Device(err.to_string()))?;

        self.gpu.read(start)
    }
}

impl Drop for Searching<'_> {
    fn drop(&mut self) {
        if self.mapping {
            self.gpu.readback.unmap();
        }

        self.gpu.busy.store(false, Ordering::Release);
    }
}

/// The end of the readback buffer's mapping, awaited: map_async's callback hands in its result and
/// wakes whoever waits.
#[derive(Default)]
struct Mapped(Arc<Mutex<Mapping>>);

/// How a mapping stands: its result once it ended, and who to wake then.
#[derive(Default)]
struct Mapping {
    result: Option<Result<(), wgpu::BufferAsyncError>>,
    waker: Option<Waker>,
}

impl Future for Mapped {
    type Output = Result<(), wgpu::BufferAsyncError>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let mut done = self.0.lock().expect("the mapping's end is never poisoned");

        match done.result.take() {
            Some(result) => Poll::Ready(result),
            None => {
                done.waker = Some(context.waker().clone());

                Poll::Pending
            }
        }
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

    /// An adapter's info as wgpu gives it, for chosen.
    fn info(name: &str, backend: wgpu::Backend, vendor: u32, device: u32, device_type: wgpu::DeviceType) -> wgpu::AdapterInfo {
        wgpu::AdapterInfo { name: name.to_string(), vendor, device, device_type, driver: String::new(), driver_info: String::new(), backend }
    }

    #[test]
    fn each_gpu_is_chosen_once_under_its_native_backend_and_not_again_through_opengl() {
        use wgpu::{Backend, DeviceType};

        // Android: the Mali through Vulkan, and again through OpenGL with ids of its own.
        let phone = [info("Mali-G715", Backend::Gl, 0, 0, DeviceType::IntegratedGpu), info("Mali-G715", Backend::Vulkan, 0x13b5, 0xa8670000, DeviceType::IntegratedGpu)];
        assert_eq!(chosen(&phone), vec![1]);

        // A desktop: OpenGL's longer name for the same card, a second card, a software renderer,
        // and the first card twice under Vulkan and DX12 with the same ids.
        let desktop = [
            info("NVIDIA GeForce RTX 3080/PCIe/SSE2", Backend::Gl, 0x10de, 0, DeviceType::DiscreteGpu),
            info("NVIDIA GeForce RTX 3080", Backend::Vulkan, 0x10de, 0x2206, DeviceType::DiscreteGpu),
            info("NVIDIA GeForce RTX 3080", Backend::Dx12, 0x10de, 0x2206, DeviceType::DiscreteGpu),
            info("NVIDIA GeForce RTX 3080 Ti", Backend::Vulkan, 0x10de, 0x2208, DeviceType::DiscreteGpu),
            info("Intel(R) UHD Graphics 770", Backend::Vulkan, 0x8086, 0x4680, DeviceType::IntegratedGpu),
            info("llvmpipe (LLVM 17.0.6, 256 bits)", Backend::Vulkan, 0x10005, 0, DeviceType::Cpu),
        ];
        let names: Vec<_> = chosen(&desktop).into_iter().map(|index| format!("{} ({:?})", desktop[index].name, desktop[index].backend)).collect();
        assert_eq!(names, ["NVIDIA GeForce RTX 3080 (Vulkan)", "NVIDIA GeForce RTX 3080 Ti (Vulkan)", "Intel(R) UHD Graphics 770 (Vulkan)"]);

        // OpenGL alone, as on a machine without Vulkan: kept.
        let only = [info("Mali-G715", Backend::Gl, 0, 0, DeviceType::IntegratedGpu)];
        assert_eq!(chosen(&only), vec![0]);
    }

    #[test]
    fn a_work_hash_is_found_by_the_name_genesis_gives_it() {
        for hash in WorkHash::ALL {
            assert_eq!(WorkHash::named(hash.name()), Some(hash));
        }

        assert_eq!(WorkHash::named("xxh3"), None, "not secure, so no genesis mines it");
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
    fn every_gpu_listed_opens_once_and_a_missing_one_is_refused() {
        let names = list();

        if names.is_empty() {
            eprintln!("no GPU here: skipping");

            return;
        }

        let opened = Gpu::open_all(None).unwrap();

        assert_eq!(opened.iter().map(|gpu| gpu.name().to_string()).collect::<Vec<_>>(), names);
        assert!(Gpu::open_all(Some(&[names.len()])).is_err(), "past the end");
        assert!(Gpu::open_all(Some(&[0, 0])).is_err(), "twice");
    }

    #[test]
    fn a_batch_is_bounded_and_an_empty_search_finds_nothing() {
        let Some(gpu) = gpu() else { return };

        assert!(!gpu.name().is_empty());
        assert_eq!(gpu.search(WorkHash::Blake3, &prefix(), &[0xff; 32], 9, 0).unwrap(), None);
        assert_eq!(gpu.search(WorkHash::Blake3, &prefix(), &[0xff; 32], 9, u32::MAX).unwrap(), Some(9), "capped at BATCH, still found");
    }
}
