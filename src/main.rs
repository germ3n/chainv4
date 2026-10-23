#[cfg(feature = "http")]
pub mod api;
#[cfg(feature = "node")]
pub mod chain;
#[cfg(feature = "node")]
pub mod consensus;
pub mod crypto;
pub mod db;
#[cfg(feature = "node")]
pub mod mempool;
#[cfg(feature = "node")]
pub mod net;
#[cfg(feature = "node")]
pub mod node;
pub mod registry;
pub mod structs;
pub mod vm;

#[cfg(feature = "node")]
#[derive(clap::Parser)]
#[command(about = "Runs a chainv4 node")]
struct Args {
    /// Where the chain database lives.
    #[arg(long, default_value = "chain.db")]
    db: std::path::PathBuf,
    /// An address to listen on. Repeat to listen on several.
    #[arg(long, default_value = "/ip4/0.0.0.0/udp/0/quic-v1")]
    listen: Vec<libp2p::Multiaddr>,
    /// A peer to dial at startup. Repeat to dial several.
    #[arg(long)]
    peer: Vec<libp2p::Multiaddr>,
    /// Produce blocks with the built-in development authority key.
    #[arg(long)]
    produce: bool,
    /// Serve the JSON HTTP API on this address, such as 127.0.0.1:8080.
    #[cfg(feature = "http")]
    #[arg(long)]
    http: Option<std::net::SocketAddr>,
}

#[cfg(feature = "node")]
fn main() -> std::process::ExitCode {
    use clap::Parser;

    // libp2p's own info lines are mostly noise for an operator; RUST_LOG still overrides this.
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info,libp2p=warn"))
        .format_timestamp_millis()
        .init();

    match run(Args::parse()) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(err) => {
            log::error!("{err}");

            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(feature = "node")]
fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    use {
        chain::Genesis,
        consensus::DevAuthority,
        db::{Database, RocksDb},
        net::NetConfig,
        node::Node,
        std::sync::Arc,
    };

    let authority = DevAuthority::dev_keypair();
    let genesis = Genesis::dev(authority.public());
    let db: Arc<dyn Database> = Arc::new(RocksDb::open(&args.db)?);
    let network: String = genesis.block().id().to_bytes()[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    log::info!(
        "[node] chainv4 {} on network {network}, database {}",
        env!("CARGO_PKG_VERSION"),
        args.db.display()
    );

    if args.produce {
        log::info!("[node] producing blocks as {}", authority.public());
    }

    let net = net::spawn(NetConfig {
        listen: args.listen,
        peers: args.peer,
        network,
    })?;
    let consensus = DevAuthority::new(genesis.authority, genesis.slot_ms, args.produce.then_some(authority));
    let node = Node::new(db.clone(), &genesis, Box::new(consensus), net)?;

    #[cfg(feature = "http")]
    if let Some(address) = args.http {
        let address = api::start(db, address)?;
        log::info!("[http] listening on {address}");
    }

    node.run()?;

    Ok(())
}

#[cfg(not(feature = "node"))]
fn main() {
    eprintln!("chainv4 was built without the node feature");
}
