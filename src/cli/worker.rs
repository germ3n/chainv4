//! The worker subcommand: mines for a node over its mining port, with no chain of its own.

use {
    super::CliResult,
    crate::mining::worker::{Devices, Worker, WorkerConfig},
    std::path::PathBuf,
};

#[derive(clap::Args)]
pub struct WorkerArgs {
    /// The node's mining port, as host:port, such as 203.0.113.7:4002.
    #[arg(long)]
    node: String,
    /// The file holding the token the node was given with --mining-token.
    #[arg(long)]
    token: PathBuf,
    /// Mine on the GPUs instead of CPU threads: every one, or those --gpus picks. Needs a build
    /// with the gpu feature.
    #[arg(long)]
    gpu: bool,
    /// The GPUs to mine on, by their number in chainv4 gpus, such as 0,2.
    #[arg(long, requires = "gpu", value_delimiter = ',')]
    gpus: Vec<usize>,
    /// How many CPU threads to mine on, without --gpu.
    #[arg(long, default_value_t = 1, conflicts_with = "gpu", value_parser = clap::value_parser!(u32).range(1..))]
    threads: u32,
    /// The node's protocol name, as its --protocol gives it.
    #[arg(long, default_value = crate::net::DEFAULT_PROTOCOL, value_parser = crate::net::protocol_name)]
    protocol: String,
    /// What to call this worker in the node's report and on the explorer; the host name by default.
    #[arg(long)]
    name: Option<String>,
    /// Reach the node through a SOCKS5 proxy, socks5h://host:port: Tor's, socks5h://127.0.0.1:9050,
    /// for a node's onion, --node ADDRESS.onion:PORT.
    #[arg(long)]
    proxy: Option<String>,
}

/// Mines for the node until the process is stopped.
pub fn worker(args: WorkerArgs) -> CliResult<()> {
    let token = crate::mining::read_token(&args.token)?;
    let devices = match args.gpu {
        true => Devices::Gpu((!args.gpus.is_empty()).then_some(args.gpus)),
        false => Devices::Cpu(args.threads as usize),
    };

    #[cfg(not(feature = "gpu"))]
    if args.gpu {
        return Err("--gpu needs a build with the gpu feature: cargo build --features gpu".into());
    }

    let name = args.name.unwrap_or_else(host_name);
    log::info!("[worker] {name}: mining for {}", args.node);

    Worker::start(WorkerConfig {
        node: args.node,
        token,
        name,
        protocol: args.protocol,
        devices,
        proxy: args.proxy,
    })?
    .wait();

    Ok(())
}

/// The machine's name, for a worker not given one.
fn host_name() -> String {
    ["HOSTNAME", "HOST", "COMPUTERNAME"]
        .iter()
        .find_map(|name| std::env::var(name).ok())
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok())
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "worker".to_string())
}

/// Lists the GPUs this machine has, by the numbers --gpus takes.
#[cfg(feature = "gpu")]
pub fn gpus() -> CliResult<()> {
    let gpus = chainv4_gpu::list();

    if gpus.is_empty() {
        return Err("no GPU found".into());
    }

    for (index, name) in gpus.iter().enumerate() {
        println!("{index}: {name}");
    }

    Ok(())
}
