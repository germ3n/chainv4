//! Mining from the wallet, alone or in a pool, for the account picked: a worker of the node's
//! mining port (`chainv4::mining`), on this device's GPU or CPU. The desktop and the phones speak
//! the port's protocol over TCP, through the wallet's proxy when it has one (`native`); a browser,
//! which opens no TCP connections, speaks it over the node's WebSocket, GET /mining/ws, and mines
//! on WebGPU or on Web Workers (`web`). Both are the same Miner to the Mining page.
//!
//! Whoever mines, the node checks every nonce before it seals a block with it, and a pool's or a
//! solo block's reward goes where the chain's rules send it: the wallet holds no key to mine.

use chainv4::{consensus::DeviceReport, mining::status::WorkerStatus, structs::address::Address};

#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(target_arch = "wasm32")]
mod web;

#[cfg(not(target_arch = "wasm32"))]
pub use native::{Miner, cpus, gpus};
#[cfg(target_arch = "wasm32")]
pub use web::{Miner, cpus, gpus};

/// Where the mining port is reached.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Port {
    /// Its TCP address, host:port: the desktop's and the phones' way, through `proxy` when set,
    /// which the browser refuses.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    Tcp { address: String, proxy: Option<String> },
    /// A node's WebSocket for it, ws:// or wss://…/mining/ws: the browser's way, which the
    /// desktop and the phones refuse.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    WebSocket(String),
}

/// What the wallet mines on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Devices {
    /// This many CPU threads: on the web, Web Workers.
    Cpu(usize),
    /// The GPUs, by their place in gpus(), or every one for None: on the web, the browser's, over
    /// WebGPU.
    Gpu(Option<Vec<usize>>),
}

/// How to mine: where, for whom, as what, and on what.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MineConfig {
    pub port: Port,
    /// The port's token: the open token for a port anyone may join.
    pub token: Vec<u8>,
    /// The port's protocol name, as GET /mining says it.
    pub protocol: String,
    /// What the node's report and the explorer call this worker.
    pub name: String,
    /// The account a pool's shares, or a solo block, pay.
    pub payout: Address,
    /// Mine alone: each block found pays the payout in full; otherwise in the port's pool.
    pub solo: bool,
    pub devices: Devices,
}

// What each side gives the page, checked here so both keep to it:
// - `Miner::start(config, ctx) -> Result<Miner, String>`: connects and mines until dropped,
//   connecting again whenever the connection goes, and asks ctx to repaint as its status changes.
// - `Miner::status(&self) -> WorkerStatus` and `Miner::devices(&self) -> Vec<DeviceReport>`.
// - `Miner::config(&self) -> &MineConfig`: what it was started with.
// - `gpus() -> Vec<String>`: the GPUs it may mine on, by name; `cpus() -> usize`: the threads.
#[allow(dead_code, clippy::type_complexity)]
fn _keeps_to_the_interface(config: MineConfig, ctx: &egui::Context) -> Result<(WorkerStatus, Vec<DeviceReport>, MineConfig, Vec<String>, usize), String> {
    let miner = Miner::start(config, ctx)?;

    Ok((miner.status(), miner.devices(), miner.config().clone(), gpus(), cpus()))
}
