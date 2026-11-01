//! The Miner in the browser: the mining port's protocol over the node's WebSocket, GET
//! /mining/ws, a line to a message, and the nonces tried on the browser's GPU through WebGPU, or
//! on Web Workers, one a thread, where the CPU was picked or there is no WebGPU. All of it runs on
//! the page's thread between its frames: the GPU is awaited a batch at a time, and each Web
//! Worker is handed a chunk of nonces at a time, so new work reaches them soon. It mines while the
//! page is open, connects again whenever the connection goes, and stops once dropped.

// Hashing a chunk is the Web Worker's half, built from the same file.
#[allow(dead_code)]
mod work;

use {
    super::{Devices, MineConfig, Port},
    chainv4::consensus::DeviceReport,
    chainv4::mining::protocol::Hello,
    chainv4::mining::status::{Work, WorkerStatus},
    chainv4::structs::hash::HashType,
    chainv4_gpu::{BATCH, Gpu, WorkHash},
    serde_json::{Value, json},
    std::cell::RefCell,
    std::rc::{Rc, Weak},
    std::time::Duration,
    wasm_bindgen::{JsCast, JsValue, closure::Closure},
    web_sys::{CloseEvent, MessageEvent, WebSocket, Worker},
    web_time::Instant,
    work::{FIRST_CHUNK, Job, Meter, READY, Searched, Session, Taken},
};

/// How often the miner sends its hashrates when due, and asks the page to draw again.
const TICK_MS: i32 = 1_000;
/// How often it sends its hashrates: the node takes a worker that says nothing for a minute for
/// gone. A hidden tab's timers slow down, so the GPU's batches and the Web Workers' chunks send
/// them when due as well.
const RATE_EVERY: Duration = Duration::from_secs(5);
/// How long it waits before connecting again.
const RECONNECT_MS: i32 = 5_000;
/// The browser's GPU, as gpus() names it and the node's report shows it: WebGPU does not say
/// which GPU it is.
const GPU: &str = "This browser's GPU (WebGPU)";
/// What starts a Web Worker, beside the page: miner-worker-boot.js loads the worker's wasm.
const WORKER: &str = "./miner-worker-boot.js";

/// Mining, until dropped.
pub struct Miner {
    config: MineConfig,
    state: Rc<RefCell<State>>,
}

impl Miner {
    /// Starts mining as the config says: an error for a port only TCP reaches, GPUs the browser
    /// does not have, or a WebSocket address the browser does not take.
    pub fn start(config: MineConfig, ctx: &egui::Context) -> Result<Miner, String> {
        let Port::WebSocket { url, .. } = &config.port else {
            return Err("a browser reaches a mining port over its node's WebSocket, ws:// or wss://…/mining/ws, not TCP".to_string());
        };

        if !url.starts_with("ws://") && !url.starts_with("wss://") {
            return Err(format!("{url} is not a WebSocket's address, ws:// or wss://"));
        }

        let no_gpu = match &config.devices {
            Devices::Cpu(_) => None,
            Devices::Gpu(Some(which)) if which.is_empty() => return Err("no GPU was picked to mine on".to_string()),
            Devices::Gpu(Some(which)) if which.iter().any(|index| *index != 0) => {
                return Err("a browser has one GPU to mine on, its own, through WebGPU".to_string());
            }
            Devices::Gpu(_) if gpus().is_empty() => Some("this browser has no WebGPU".to_string()),
            Devices::Gpu(_) => None,
        };
        let state = Rc::new(RefCell::new(State {
            config: config.clone(),
            ctx: ctx.clone(),
            status: WorkerStatus::default(),
            socket: None,
            session: Session::Closed,
            why: None,
            job: None,
            generation: 0,
            rated: Instant::now(),
            rig: Rig::None,
            loops: 0,
            no_gpu,
            tick: None,
            reconnect: None,
            stopped: false,
        }));

        connect(&state)?;
        let tick = handler(Rc::downgrade(&state), |state, _| state.borrow_mut().tick());
        state.borrow_mut().tick = window().and_then(|window| window.set_interval_with_callback_and_timeout_and_arguments_0(&tick, TICK_MS).ok());

        Ok(Miner { config, state })
    }

    /// How it is doing now.
    pub fn status(&self) -> WorkerStatus {
        self.state.borrow().status.clone()
    }

    /// Its devices and their hashrates; none before the first work.
    pub fn devices(&self) -> Vec<DeviceReport> {
        self.state.borrow_mut().devices()
    }

    /// What it was started with.
    pub fn config(&self) -> &MineConfig {
        &self.config
    }
}

impl Drop for Miner {
    fn drop(&mut self) {
        self.state.borrow_mut().stop();
    }
}

/// The GPUs a browser may mine on: its own, when it has WebGPU.
pub fn gpus() -> Vec<String> {
    let has = window().is_some_and(|window| js_sys::Reflect::get(&window.navigator(), &JsValue::from_str("gpu")).is_ok_and(|gpu| !gpu.is_undefined() && !gpu.is_null()));

    if has { vec![GPU.to_string()] } else { Vec::new() }
}

/// The CPU threads the browser says this device has.
pub fn cpus() -> usize {
    window().map_or(1, |window| window.navigator().hardware_concurrency() as usize).max(1)
}

fn window() -> Option<web_sys::Window> {
    web_sys::window()
}

/// What a JavaScript error says.
fn said(value: &JsValue) -> String {
    value
        .as_string()
        .or_else(|| value.dyn_ref::<js_sys::Error>().map(|err| String::from(err.message())))
        .unwrap_or_else(|| format!("{value:?}"))
}

/// A JavaScript function calling f with the miner's state while there is one: it holds the state
/// weakly, so dropping the miner frees it, and the browser frees the function once nothing calls
/// it.
fn handler(state: Weak<RefCell<State>>, f: impl Fn(&Rc<RefCell<State>>, JsValue) + 'static) -> js_sys::Function {
    Closure::<dyn FnMut(JsValue)>::new(move |event: JsValue| {
        if let Some(state) = state.upgrade() {
            f(&state, event);
        }
    })
    .into_js_value()
    .unchecked_into()
}

/// What it mines on, made once the first work says which hash.
enum Rig {
    None,
    /// WebGPU opening, for this work hash.
    Opening(HashType),
    /// The GPU, and the loop searching it now, by its number.
    Gpu { gpu: Rc<Gpu>, hash: HashType, meter: Meter, running: Option<u64> },
    /// The Web Workers, one a thread, metered as one device as chainv4's CPU threads are.
    Cpu { name: String, slots: Vec<Slot>, meter: Meter },
}

/// A Web Worker: whether it said it is ready, the chunk it has, and how big its next one is.
struct Slot {
    worker: Worker,
    ready: bool,
    /// When its chunk went out, and how many nonces it holds; None while it waits for one.
    busy: Option<(Instant, u64)>,
    count: u64,
}

/// A batch for the GPU: what it searches, and for which generation of the job.
struct Batch {
    gpu: Rc<Gpu>,
    hash: WorkHash,
    prefix: [u8; 44],
    target: [u8; 32],
    start: u64,
    count: u32,
    generation: u64,
}

/// Everything the miner keeps, on the page's thread.
struct State {
    config: MineConfig,
    ctx: egui::Context,
    status: WorkerStatus,
    socket: Option<WebSocket>,
    session: Session,
    /// Why the connection is closing, as found before it closed.
    why: Option<String>,
    job: Option<Job>,
    /// Bumped for each work, and when the connection goes, so what comes back for older work is
    /// told apart.
    generation: u64,
    /// When the hashrates were last sent.
    rated: Instant,
    rig: Rig,
    /// The GPU loops started, so one that was replaced knows to stop.
    loops: u64,
    /// Why it does not mine on WebGPU though asked to, once found: none here, or it failed.
    no_gpu: Option<String>,
    /// The tick's interval, and the timeout that connects again.
    tick: Option<i32>,
    reconnect: Option<i32>,
    stopped: bool,
}

/// Opens a WebSocket to the node, its handshake to follow as the node greets it.
fn connect(state: &Rc<RefCell<State>>) -> Result<(), String> {
    let Port::WebSocket { url, .. } = state.borrow().config.port.clone() else {
        return Err("not a WebSocket".to_string());
    };
    let socket = WebSocket::new(&url).map_err(|err| format!("{url}: {}", said(&err)))?;
    let weak = Rc::downgrade(state);
    socket.set_onmessage(Some(&handler(weak.clone(), |state, event| {
        if let Some(text) = event.unchecked_into::<MessageEvent>().data().as_string() {
            take(state, &text);
        }
    })));
    socket.set_onerror(Some(&handler(weak.clone(), |state, _| {
        // A browser says nothing more of why, to keep pages from probing the network.
        state.borrow_mut().why.get_or_insert_with(|| "the WebSocket failed: the node is not reached, or takes no workers".to_string());
    })));
    socket.set_onclose(Some(&handler(weak, |state, event| closed(state, &event.unchecked_into()))));

    let mut state = state.borrow_mut();
    (state.socket, state.session, state.why) = (Some(socket), Session::Greeting, None);

    Ok(())
}

/// Takes a message from the node: work goes to the devices; one that does not check, or a node
/// that refused this miner, closes the connection.
fn take(state: &Rc<RefCell<State>>, text: &str) {
    let heard = state.borrow_mut().take(text);

    match heard {
        Ok(Some((work, span))) => mine(state, &work, span),
        Ok(None) => {}
        Err(why) => state.borrow_mut().fail(why),
    }

    state.borrow().ctx.request_repaint();
}

/// The connection closed: says why, rests the devices, and connects again in a while.
fn closed(state: &Rc<RefCell<State>>, event: &CloseEvent) {
    let mut guard = state.borrow_mut();

    if guard.stopped {
        return;
    }

    let was_open = guard.session.open();
    let why = guard.why.take().unwrap_or_else(|| match (was_open, event.reason()) {
        (true, reason) if reason.is_empty() => "the node closed the connection".to_string(),
        (true, reason) => format!("the node closed the connection: {reason}"),
        (false, _) => format!("the WebSocket closed before the handshake ended, with code {}", event.code()),
    });
    // Asked again it would say the same, or mine for someone else: this needs a person.
    let stopping = why.starts_with(chainv4::mining::protocol::PAYOUT_DIFFERS);
    log::info!("[miner] {why}; {}", if stopping { "stopping".to_string() } else { format!("connecting again in {}s", RECONNECT_MS / 1_000) });
    (guard.socket, guard.session) = (None, Session::Closed);
    (guard.status.connected, guard.status.trouble) = (false, Some(why));
    // Nonces found from here on belong to no job the next connection knows.
    guard.job = None;
    guard.generation += 1;
    guard.ctx.request_repaint();
    drop(guard);

    if !stopping {
        again(state);
    }
}

/// Connects again once RECONNECT_MS passed, and again after that while it fails.
fn again(state: &Rc<RefCell<State>>) {
    let reconnect = handler(Rc::downgrade(state), |state, _| {
        state.borrow_mut().reconnect = None;

        if let Err(why) = connect(state) {
            state.borrow_mut().status.trouble = Some(why);
            again(state);
        }
    });
    state.borrow_mut().reconnect = window().and_then(|window| window.set_timeout_with_callback_and_timeout_and_arguments_0(&reconnect, RECONNECT_MS).ok());
}

/// Mines new work: the devices drop what they had and start on it, made first should this be the
/// first work, or its hash another.
fn mine(state: &Rc<RefCell<State>>, work: &Work, span: u64) {
    let (hash, on_gpu) = {
        let mut state = state.borrow_mut();
        state.generation += 1;
        let job = Job::new(work, span, state.generation);
        let on_gpu = matches!(state.config.devices, Devices::Gpu(_)) && job.prefix().is_some() && WorkHash::named(job.hash.name()).is_some();
        state.job = Some(job);

        (work.hash, on_gpu)
    };

    mount(state, hash, on_gpu);
    feed(state);
}

/// Makes the devices for a work hash: WebGPU when asked for and there, or else the Web Workers,
/// saying why it is not the GPU.
fn mount(state: &Rc<RefCell<State>>, hash: HashType, on_gpu: bool) {
    let mut guard = state.borrow_mut();

    if on_gpu && guard.no_gpu.is_none() {
        match guard.rig {
            Rig::Opening(mounted) | Rig::Gpu { hash: mounted, .. } if mounted == hash => return,
            _ => {}
        }

        guard.unmount();
        guard.rig = Rig::Opening(hash);
        let weak = Rc::downgrade(state);

        wasm_bindgen_futures::spawn_local(async move {
            let opened = Gpu::open_async(&[WorkHash::named(hash.name()).expect("checked before")]).await;
            let Some(state) = weak.upgrade() else {
                return;
            };

            {
                let mut guard = state.borrow_mut();

                // Stopped, or another hash's work came while it opened.
                if guard.stopped || !matches!(guard.rig, Rig::Opening(opening) if opening == hash) {
                    return;
                }

                match opened {
                    Ok(gpu) => {
                        log::info!("[miner] mining {} on {GPU}", hash.name());
                        guard.rig = Rig::Gpu { gpu: Rc::new(gpu), hash, meter: Meter::new(Instant::now()), running: None };
                    }
                    Err(err) => {
                        guard.no_gpu = Some(format!("WebGPU did not open: {err}"));
                        guard.rig = Rig::None;
                    }
                }
            }

            mount(&state, hash, true);
            feed(&state);
        });

        return;
    }

    if matches!(guard.rig, Rig::Cpu { .. }) {
        // Web Workers hash whatever work comes.
        return;
    }

    guard.unmount();
    let threads = match guard.config.devices {
        Devices::Cpu(threads) => threads.max(1),
        // In the GPU's place, every thread but the page's own.
        Devices::Gpu(_) => cpus().saturating_sub(1).max(1),
    };

    match workers(state, threads) {
        Ok(slots) => {
            let name = format!("CPU, {threads} Web Worker{}", if threads == 1 { "" } else { "s" });
            log::info!("[miner] mining {} on {name}", hash.name());
            guard.rig = Rig::Cpu { name, slots, meter: Meter::new(Instant::now()) };

            if let Some(why) = guard.no_gpu.clone().filter(|_| matches!(guard.config.devices, Devices::Gpu(_))) {
                log::warn!("[miner] {why}: mining on the CPU instead");
                guard.status.trouble = Some(format!("{why}: mining on the CPU instead"));
            }
        }
        Err(why) => {
            log::warn!("[miner] {why}");
            guard.status.trouble = Some(why);
        }
    }
}

/// Starts this many Web Workers, each to say it is ready before it is handed a chunk.
fn workers(state: &Rc<RefCell<State>>, threads: usize) -> Result<Vec<Slot>, String> {
    let mut slots = Vec::new();

    for index in 0..threads {
        let worker = match Worker::new(WORKER) {
            Ok(worker) => worker,
            Err(err) => {
                slots.iter().for_each(|slot: &Slot| slot.worker.terminate());

                return Err(format!("a Web Worker did not start: {}", said(&err)));
            }
        };
        let weak = Rc::downgrade(state);
        worker.set_onmessage(Some(&handler(weak.clone(), move |state, event| {
            if let Some(text) = event.unchecked_into::<MessageEvent>().data().as_string() {
                state.borrow_mut().searched(index, &text);
            }
        })));
        worker.set_onerror(Some(&handler(weak, |state, event| {
            // An ErrorEvent for an error in its script; a bare Event for a script that did not load.
            let message = js_sys::Reflect::get(&event, &JsValue::from_str("message")).ok().and_then(|message| message.as_string());
            let why = format!("a Web Worker failed: {}", message.unwrap_or_else(|| format!("{WORKER} did not load")));
            log::warn!("[miner] {why}");
            let mut state = state.borrow_mut();
            state.status.trouble = Some(why);
            state.ctx.request_repaint();
        })));
        slots.push(Slot { worker, ready: false, busy: None, count: FIRST_CHUNK });
    }

    Ok(slots)
}

/// Sets the devices on the job: a loop on the GPU, should none run, or a chunk to each Web Worker
/// waiting for one.
fn feed(state: &Rc<RefCell<State>>) {
    let mut guard = state.borrow_mut();
    let (has_job, loops) = (guard.job.is_some(), guard.loops + 1);

    match &mut guard.rig {
        Rig::Gpu { running, .. } if running.is_none() && has_job => {
            *running = Some(loops);
            guard.loops = loops;
            wasm_bindgen_futures::spawn_local(search(Rc::downgrade(state), loops));
        }
        Rig::Cpu { slots, .. } => {
            let count = slots.len();
            (0..count).for_each(|index| guard.hand(index));
        }
        _ => {}
    }
}

/// The GPU's loop: a batch at a time from the job's nonces, each awaited, so the page goes on
/// between them, until the job is done or gone, another loop took the GPU, or the miner stopped.
async fn search(state: Weak<RefCell<State>>, me: u64) {
    loop {
        let Some(batch) = state.upgrade().and_then(|state| state.borrow_mut().batch(me)) else {
            return;
        };
        let result = batch.gpu.search_async(batch.hash, &batch.prefix, &batch.target, batch.start, batch.count).await;
        let Some(state) = state.upgrade() else {
            return;
        };
        let mut guard = state.borrow_mut();

        if guard.stopped {
            return;
        }

        if let Rig::Gpu { meter, .. } = &mut guard.rig {
            meter.add(u64::from(batch.count));
        }

        match result {
            // A batch gives its lowest nonce alone, as on the desktop: shares come seconds apart,
            // batches far faster.
            Ok(Some(nonce)) => {
                if guard.job.as_ref().is_some_and(|job| job.generation == batch.generation && !job.meets(nonce)) {
                    log::warn!("[miner] the GPU found {nonce}, which does not meet the work: not sent");
                } else {
                    guard.found(batch.generation, &[nonce]);
                }
            }
            Ok(None) => {}
            Err(err) => {
                guard.no_gpu = Some(format!("the GPU failed: {err}"));
                guard.unmount();
                let hash = guard.job.as_ref().map(|job| job.hash);
                drop(guard);

                if let Some(hash) = hash {
                    mount(&state, hash, true);
                    feed(&state);
                }

                return;
            }
        }

        guard.rate_if_due();
    }
}

impl State {
    /// A line from the node, as far as the handshake is: work it hands, with its span of nonces.
    fn take(&mut self, text: &str) -> Result<Option<(Work, u64)>, String> {
        let me = Hello { name: self.config.name.clone(), payout: Some(self.config.payout), solo: self.config.solo };

        match self.session.take(text, &self.config.token, &self.config.protocol, &me)? {
            Taken::Hello(hello) => self.send(&hello)?,
            Taken::Joined => {
                work::joined(&mut self.status);
                self.rated = Instant::now();
                log::info!("[miner] mining as {}{}", self.config.name, if self.config.solo { ", alone" } else { "" });
            }
            Taken::Message(message) => {
                if let Some(why) = chainv4::mining::protocol::pays_another(&message, Some(&self.config.payout)) {
                    return Err(why);
                }

                let heard = work::heard(&mut self.status, &message)?;
                // Pings come every 20 seconds, however a hidden tab slows its timers.
                self.rate_if_due();

                return Ok(heard);
            }
            Taken::Nothing => {}
        }

        Ok(None)
    }

    /// Sends a line as it is: the handshake's.
    fn send(&self, line: &str) -> Result<(), String> {
        let socket = self.socket.as_ref().ok_or("not connected")?;

        socket.send_with_str(line).map_err(|err| format!("sending failed: {}", said(&err)))
    }

    /// Sends a message sealed, past the handshake; before it, nothing.
    fn send_sealed(&mut self, message: &Value) -> Result<(), String> {
        match self.session.seal(message) {
            Some(line) => self.send(&line),
            None => Ok(()),
        }
    }

    /// Ends the connection for a reason, which the status keeps once it closed.
    fn fail(&mut self, why: String) {
        log::warn!("[miner] {why}");
        self.why.get_or_insert(why);

        if let Some(socket) = &self.socket {
            let _ = socket.close();
        }
    }

    /// Sends the nonces found for a generation of the job that it still wants.
    fn found(&mut self, generation: u64, nonces: &[u64]) {
        let Some(job) = self.job.as_mut() else {
            return;
        };
        let (number, shares) = (job.number, job.shares);

        for nonce in job.found(generation, nonces) {
            if !shares {
                log::info!("[miner] found a nonce for job {number}");
            }

            if let Err(why) = self.send_sealed(&json!({ "type": "found", "job": number, "nonce": nonce })) {
                return self.fail(why);
            }
        }

        self.ctx.request_repaint();
    }

    /// Each second: the hashrates when due, and the page drawn again, so the numbers move.
    fn tick(&mut self) {
        self.rate_if_due();
        self.ctx.request_repaint();
    }

    /// Sends the devices' hashrates once RATE_EVERY passed since last.
    fn rate_if_due(&mut self) {
        if !self.session.open() || self.rated.elapsed() < RATE_EVERY {
            return;
        }

        self.rated = Instant::now();
        let devices: Vec<Value> = self.devices().iter().map(|device| json!({ "name": device.name, "rate": device.rate })).collect();

        if let Err(why) = self.send_sealed(&json!({ "type": "rate", "devices": devices })) {
            self.fail(why);
        }
    }

    /// The devices and their hashrates.
    fn devices(&mut self) -> Vec<DeviceReport> {
        let now = Instant::now();

        match &mut self.rig {
            Rig::Gpu { meter, .. } => vec![DeviceReport { name: GPU.to_string(), rate: meter.rate(now) }],
            Rig::Cpu { name, meter, .. } => vec![DeviceReport { name: name.clone(), rate: meter.rate(now) }],
            Rig::None | Rig::Opening(_) => Vec::new(),
        }
    }

    /// The GPU's next batch, for the loop numbered me while it is the one running; None once there
    /// is none, and that loop ends.
    fn batch(&mut self, me: u64) -> Option<Batch> {
        let State { rig: Rig::Gpu { gpu, hash, running, .. }, job, stopped: false, .. } = self else {
            return None;
        };

        if *running != Some(me) {
            return None;
        }

        let next = job.as_mut().filter(|job| job.hash == *hash).and_then(|job| {
            let (prefix, target, generation) = (job.prefix()?, job.target(), job.generation);
            let (start, count) = job.take(u64::from(BATCH))?;

            Some(Batch {
                gpu: gpu.clone(),
                hash: WorkHash::named(hash.name())?,
                prefix,
                target,
                start,
                count: count as u32,
                generation,
            })
        });

        if next.is_none() {
            *running = None;
        }

        next
    }

    /// What a Web Worker posted: that it is ready, or what it found in its chunk. Either way it
    /// is handed the next.
    fn searched(&mut self, index: usize, text: &str) {
        if self.stopped {
            return;
        }

        let Rig::Cpu { slots, meter, .. } = &mut self.rig else {
            return;
        };
        let Some(slot) = slots.get_mut(index) else {
            return;
        };

        if text == READY {
            slot.ready = true;
        } else if let Some(searched) = Searched::read(text) {
            meter.add(searched.tried);

            // A chunk cut short by a block's nonce says nothing of how fast the worker is.
            if let Some((since, count)) = slot.busy.take()
                && searched.tried == count
            {
                slot.count = work::next_count(count, since.elapsed().as_secs_f64() * 1e3);
            }

            self.found(searched.generation, &searched.found);
            self.rate_if_due();
        }

        self.hand(index);
    }

    /// Hands a Web Worker its next chunk, if it waits for one and the job has nonces left.
    fn hand(&mut self, index: usize) {
        let State { rig: Rig::Cpu { slots, .. }, job: Some(job), stopped: false, .. } = self else {
            return;
        };
        let Some(slot) = slots.get_mut(index).filter(|slot| slot.ready && slot.busy.is_none()) else {
            return;
        };
        let Some(chunk) = job.chunk(slot.count) else {
            return;
        };

        match slot.worker.post_message(&JsValue::from_str(&chunk.to_json())) {
            Ok(()) => slot.busy = Some((Instant::now(), chunk.count)),
            Err(err) => log::warn!("[miner] a Web Worker was not handed its chunk: {}", said(&err)),
        }
    }

    /// Stops the devices: the Web Workers end, and the GPU's loop stops after its batch.
    fn unmount(&mut self) {
        if let Rig::Cpu { slots, .. } = &self.rig {
            for slot in slots {
                slot.worker.set_onmessage(None);
                slot.worker.set_onerror(None);
                slot.worker.terminate();
            }
        }

        self.rig = Rig::None;
    }

    /// Stops everything: the connection, the timers and the devices.
    fn stop(&mut self) {
        self.stopped = true;

        if let Some(socket) = self.socket.take() {
            socket.set_onmessage(None);
            socket.set_onerror(None);
            socket.set_onclose(None);
            let _ = socket.close();
        }

        if let Some(window) = window() {
            if let Some(tick) = self.tick.take() {
                window.clear_interval_with_handle(tick);
            }

            if let Some(reconnect) = self.reconnect.take() {
                window.clear_timeout_with_handle(reconnect);
            }
        }

        self.unmount();
        self.job = None;
        self.status.connected = false;
    }
}
