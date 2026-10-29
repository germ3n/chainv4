//! The Miner on the desktop and the phones: chainv4's worker, on a thread of its own, reaching the
//! port over TCP, through the wallet's proxy when it has a SOCKS5 one. A thread beside it asks the
//! page to draw again every second while it mines, so the numbers move.
//!
//! A phone mines only while the wallet is in front: Android and iOS stop drawing an app they send
//! to the background, so that thread also watches egui's frames, and stops the worker once none
//! came for a few seconds, or once it finds it was itself asleep that long, as iOS freezes an app
//! in the background. The page sees the gap too as it draws again, and leaves the miner stopped.

use {
    super::{Devices, MineConfig, Port},
    chainv4::consensus::DeviceReport,
    chainv4::mining::worker::{self, Worker, WorkerConfig, WorkerStatus},
    std::sync::atomic::{AtomicBool, Ordering},
    std::sync::{Arc, Mutex, MutexGuard},
    std::thread::JoinHandle,
    std::time::{Duration, Instant},
};

/// How often the page is asked to draw again while mining.
const REPAINT: Duration = Duration::from_secs(1);
/// How long a phone mines without the wallet drawing a frame: past it, the wallet is taken for
/// gone to the background.
pub const UNSEEN: Duration = Duration::from_secs(5);
/// What a phone's miner says once it stopped for the background.
const IN_BACKGROUND: &str = "stopped as the wallet went to the background";

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The worker, and how it stood when it was stopped, should it be.
#[derive(Default)]
struct Running {
    worker: Option<Worker>,
    last: WorkerStatus,
}

/// Mining, until dropped.
pub struct Miner {
    config: MineConfig,
    running: Arc<Mutex<Running>>,
    stop: Arc<AtomicBool>,
    ticking: Option<JoinHandle<()>>,
}

impl Miner {
    /// Starts mining as the config says: an error for a port only a browser reaches, or a worker
    /// that does not start.
    pub fn start(config: MineConfig, ctx: &egui::Context) -> Result<Miner, String> {
        let Port::Tcp { address, proxy } = &config.port else {
            return Err("this app reaches a mining port over TCP, host:port, not a WebSocket".to_string());
        };
        let devices = match &config.devices {
            Devices::Cpu(threads) => worker::Devices::Cpu((*threads).max(1)),
            Devices::Gpu(which) => worker::Devices::Gpu(which.clone()),
        };
        let worker = Worker::start(WorkerConfig {
            node: address.clone(),
            token: config.token.clone(),
            name: config.name.clone(),
            protocol: config.protocol.clone(),
            devices,
            proxy: proxy.clone(),
            payout: Some(config.payout),
            // The page keeps the pool's claims itself, from the pool's node.
            claims: None,
            solo: config.solo,
        })?;
        let running = Arc::new(Mutex::new(Running { worker: Some(worker), last: WorkerStatus::default() }));
        let stop = Arc::new(AtomicBool::new(false));
        let ticking = {
            let (running, stop, ctx) = (running.clone(), stop.clone(), ctx.clone());

            std::thread::Builder::new()
                .name("miner".to_string())
                .spawn(move || tick(&running, &stop, &ctx))
                .map_err(|err| format!("the miner's thread did not start: {err}"))?
        };

        Ok(Miner { config, running, stop, ticking: Some(ticking) })
    }

    /// How the worker is doing; once stopped for the background, how it stood then.
    pub fn status(&self) -> WorkerStatus {
        let running = lock(&self.running);

        running.worker.as_ref().map_or_else(|| running.last.clone(), Worker::status)
    }

    /// Its devices and their hashrates; none before the first work, or once stopped.
    pub fn devices(&self) -> Vec<DeviceReport> {
        lock(&self.running).worker.as_ref().map(Worker::devices).unwrap_or_default()
    }

    /// What it was started with.
    pub fn config(&self) -> &MineConfig {
        &self.config
    }
}

impl Drop for Miner {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);

        if let Some(ticking) = self.ticking.take() {
            let _ = ticking.join();
        }

        // A worker stops once its connection does, which can take a while as it connects through
        // a proxy: it stops on a thread of its own, so the page does not wait.
        if let Some(worker) = lock(&self.running).worker.take() {
            let _ = std::thread::Builder::new().name("miner stopping".to_string()).spawn(move || drop(worker));
        }
    }
}

/// Asks the page to draw again every second while mining, and on a phone stops the worker once the
/// wallet went to the background.
fn tick(running: &Mutex<Running>, stop: &AtomicBool, ctx: &egui::Context) {
    let mut watch = Watch::new(frames(ctx), Instant::now());

    while !stop.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(100));

        if !watch.due(Instant::now()) {
            continue;
        }

        let unseen = watch.unseen(frames(ctx), Instant::now());

        if cfg!(any(target_os = "android", target_os = "ios")) && unseen {
            let mut running = lock(running);

            if let Some(worker) = running.worker.take() {
                log::info!("[miner] {IN_BACKGROUND}");
                running.last = WorkerStatus { connected: false, trouble: Some(IN_BACKGROUND.to_string()), ..worker.status() };
                drop(running);
                drop(worker);
            }

            return;
        }

        ctx.request_repaint();
    }
}

/// The frames egui drew so far.
fn frames(ctx: &egui::Context) -> u64 {
    ctx.cumulative_pass_nr_for(egui::ViewportId::ROOT)
}

/// Whether the wallet is still drawn: the frames it drew, and when the count last moved; and when
/// the watch last looked, to know a second passed, or that the app slept.
struct Watch {
    frames: u64,
    moved: Instant,
    looked: Instant,
}

impl Watch {
    fn new(frames: u64, now: Instant) -> Watch {
        Watch { frames, moved: now, looked: now }
    }

    /// Whether a second passed since it last looked.
    fn due(&self, now: Instant) -> bool {
        now.duration_since(self.looked) >= REPAINT
    }

    /// Looks: whether no frame came for UNSEEN, or the watch itself was asleep that long, as an app
    /// iOS froze in the background wakes to find.
    fn unseen(&mut self, frames: u64, now: Instant) -> bool {
        let slept = now.duration_since(self.looked) >= UNSEEN;
        self.looked = now;

        if frames != self.frames {
            self.frames = frames;
            self.moved = now;
        }

        slept || now.duration_since(self.moved) >= UNSEEN
    }
}

/// The GPUs this device has, by name, as the GPU miner numbers them. Listing them asks the
/// graphics driver, so the page asks once.
pub fn gpus() -> Vec<String> {
    chainv4_gpu::list()
}

/// The CPU threads this device has.
pub fn cpus() -> usize {
    std::thread::available_parallelism().map_or(1, std::num::NonZero::get)
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        chainv4::mining::protocol,
        chainv4::structs::address::Address,
        chainv4::structs::hash::{Hash, HashType},
        chainv4::structs::hex::to_hex,
        serde_json::{Value, json},
        std::io::{BufRead, BufReader, Write},
        std::net::TcpListener,
    };

    const TOKEN: &[u8] = b"a port's token";

    fn config(port: Port) -> MineConfig {
        MineConfig {
            port,
            token: TOKEN.to_vec(),
            protocol: "chainv4".to_string(),
            name: "test".to_string(),
            payout: Address::Ed25519([3; 32]),
            solo: true,
            devices: Devices::Cpu(1),
        }
    }

    /// Waits for the miner's status to show what is asked.
    fn until(miner: &Miner, what: impl Fn(&WorkerStatus) -> bool) -> WorkerStatus {
        let deadline = Instant::now() + Duration::from_secs(20);

        loop {
            let status = miner.status();

            if what(&status) {
                return status;
            }

            assert!(Instant::now() < deadline, "never came to it: {status:?}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn a_miner_meets_a_port_mines_alone_and_sends_what_it_found() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let ctx = egui::Context::default();
        let miner = Miner::start(config(Port::Tcp { address, proxy: None }), &ctx).unwrap();

        // The port's side, by the protocol's pure handshake.
        let (stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
        let mut writer = stream.try_clone().unwrap();
        let mut reader = BufReader::new(stream);
        let line = |reader: &mut BufReader<_>| {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            line.trim_end().to_string()
        };
        let (greeting, challenge) = protocol::greeting("chainv4").unwrap();
        writeln!(writer, "{greeting}").unwrap();
        let hello: Value = serde_json::from_str(&line(&mut reader)).unwrap();
        let (said, answer, mut sealer, mut opener) = protocol::welcome(TOKEN, "chainv4", &challenge, &hello).unwrap();
        writeln!(writer, "{answer}").unwrap();

        assert_eq!((said.payout, said.solo, said.name.as_str()), (Some(Address::Ed25519([3; 32])), true, "test"));
        until(&miner, |status| status.connected);

        writeln!(writer, "{}", sealer.seal(&json!({ "type": "solo" }))).unwrap();
        assert_eq!(until(&miner, |status| status.mode.is_some()).mode, Some(worker::Mode::Solo));

        // So easy any nonce meets it.
        let chain = HashType::default_chain();
        let id = Hash::from_bytes(chain, &[7; 32]).unwrap();
        let work = json!({
            "type": "work",
            "job": 4,
            "chain": chain.name(),
            "id": to_hex(&id.to_bytes()),
            "difficulty": "1",
            "hash": chain.name(),
            "start": 0,
        });
        writeln!(writer, "{}", sealer.seal(&work)).unwrap();
        let status = until(&miner, |status| status.jobs == 1);

        assert_eq!((status.difficulty, status.share_difficulty), (Some(1), None));

        let found = loop {
            let message = opener.open(&line(&mut reader)).unwrap();

            if message["type"] == "found" {
                break message;
            }
        };

        assert_eq!(found["job"], 4);
        assert!(found["nonce"].is_u64());

        writeln!(writer, "{}", sealer.seal(&json!({ "type": "verdict", "job": 4, "took": true, "kind": "block" }))).unwrap();
        until(&miner, |status| status.blocks == 1);
        assert_eq!(miner.config().payout, Address::Ed25519([3; 32]));
    }

    #[test]
    fn a_websocket_is_the_browsers_way_and_refused_here() {
        let ctx = egui::Context::default();

        assert!(Miner::start(config(Port::WebSocket("ws://node/mining/ws".to_string())), &ctx).is_err_and(|why| why.contains("WebSocket")));
    }

    #[test]
    fn a_phone_is_taken_for_in_the_background_once_no_frame_came_for_a_while_or_it_slept() {
        let start = Instant::now();
        let mut watch = Watch::new(10, start);
        let at = |seconds: u64| start + Duration::from_secs(seconds);

        assert!(!watch.due(start + Duration::from_millis(500)) && watch.due(at(1)));
        assert!(!watch.unseen(11, at(1)), "a frame came");
        assert!(!watch.unseen(11, at(2)) && !watch.unseen(11, at(5)), "a moment without frames");
        assert!(watch.unseen(11, at(6)), "none for {UNSEEN:?}");

        let mut frozen = Watch::new(10, start);
        assert!(frozen.unseen(11, at(30)), "asleep, frames or not");
    }

    #[test]
    fn a_device_has_a_thread_at_least() {
        assert!(cpus() >= 1);
    }
}
