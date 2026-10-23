#[cfg(feature = "http")]
pub mod api;
#[cfg(feature = "node")]
pub mod chain;
#[cfg(feature = "node")]
pub mod cli;
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
#[cfg(feature = "vm")]
pub mod vm;

#[cfg(feature = "node")]
#[derive(clap::Parser)]
#[command(
    about = "Runs a chainv4 node, or with a subcommand, works as a client of one",
    args_conflicts_with_subcommands = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    #[command(flatten)]
    node: NodeArgs,
}

#[cfg(feature = "node")]
#[derive(clap::Subcommand)]
enum Command {
    /// Makes a key and writes it to a file.
    Keygen(cli::KeygenArgs),
    /// Signs a transaction and submits it to a node's HTTP API.
    #[cfg(feature = "client")]
    Send(cli::SendArgs),
    /// Writes a genesis file with the development defaults, to edit and run with --genesis.
    Genesis(cli::genesis::GenesisArgs),
}

#[cfg(feature = "node")]
#[derive(clap::Args)]
struct NodeArgs {
    /// Where the chain database lives.
    #[arg(long, default_value = "chain.db")]
    db: std::path::PathBuf,
    /// An address to listen on. Repeat to listen on several.
    #[arg(long, default_value = "/ip4/0.0.0.0/udp/0/quic-v1")]
    listen: Vec<libp2p::Multiaddr>,
    /// A peer to dial at startup. Repeat to dial several.
    #[arg(long)]
    peer: Vec<libp2p::Multiaddr>,
    /// Which development chain to run: pow, mined by anyone, or authority, where the built-in
    /// development key produces every block.
    #[arg(long, value_enum, default_value_t = cli::genesis::DevChain::Pow)]
    consensus: cli::genesis::DevChain,
    /// Run the chain a genesis file describes instead of a development chain.
    #[arg(long, conflicts_with = "consensus")]
    genesis: Option<std::path::PathBuf>,
    /// Mine blocks (pow chains).
    #[arg(long, conflicts_with = "produce")]
    mine: bool,
    /// Where mining rewards go. Defaults to the development key's address.
    #[arg(long, requires = "mine")]
    reward_to: Option<structs::address::Address>,
    /// How many threads to mine on.
    #[arg(long, default_value_t = 1, requires = "mine")]
    mine_threads: usize,
    /// Produce blocks (authority chains), with the development key unless --producer-key names
    /// the authority's key file.
    #[arg(long)]
    produce: bool,
    /// The authority's key file, for --produce.
    #[arg(long, requires = "produce")]
    producer_key: Option<std::path::PathBuf>,
    /// Vote on finality, with the development key unless --validator-key names a voting key file.
    #[arg(long)]
    validate: bool,
    /// The voting key file named in the validator's bond, for --validate.
    #[arg(long, requires = "validate")]
    validator_key: Option<std::path::PathBuf>,
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

    let cli = Cli::parse();
    let result = match cli.command {
        Some(Command::Keygen(args)) => cli::keygen(args),
        #[cfg(feature = "client")]
        Some(Command::Send(args)) => cli::send(args),
        Some(Command::Genesis(args)) => cli::genesis::write(args),
        None => run(cli.node),
    };

    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(err) => {
            log::error!("{err}");

            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(feature = "node")]
fn run(args: NodeArgs) -> Result<(), Box<dyn std::error::Error>> {
    use {
        chain::Genesis,
        consensus::{AuthorityProducer, ConsensusConfig, DevAuthority, Miner, Producer},
        db::{Database, RocksDb},
        net::{NetConfig, NetEvent},
        node::Node,
        std::sync::Arc,
    };

    let dev = DevAuthority::dev_keypair();
    let genesis: Genesis = match &args.genesis {
        Some(path) => cli::genesis::read(path)?,
        None => cli::genesis::dev(args.consensus),
    };
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

    log::info!(
        "[node] {} consensus, {} hashes; contracts run on {}",
        genesis.consensus.name(),
        genesis.hash.name(),
        genesis.engines.iter().map(|engine| engine.name()).collect::<Vec<_>>().join(" and ")
    );

    let net = net::spawn(NetConfig {
        listen: args.listen,
        peers: args.peer,
        hash: genesis.hash,
        network,
    })?;
    let producer: Option<Box<dyn Producer>> = match (&genesis.consensus, args.mine, args.produce) {
        (_, false, false) => None,
        (ConsensusConfig::Authority { key, slot_ms }, false, true) => {
            let keypair = cli::signer(args.producer_key.as_deref())?;

            if keypair.public() != *key {
                return Err(format!("the chain's authority is {key}, not {}", keypair.public()).into());
            }

            log::info!("[node] producing blocks as {}", keypair.public().address());

            Some(Box::new(AuthorityProducer::new(keypair, *slot_ms)))
        }
        (ConsensusConfig::ProofOfWork(config), true, false) => {
            let reward_to = args.reward_to.unwrap_or(dev.public().address());
            let inbox = net.inbox.clone();
            log::info!("[node] mining on {} threads, rewards to {reward_to}", args.mine_threads.max(1));

            Some(Box::new(Miner::new(reward_to, config.hash, args.mine_threads, move |block, seal| {
                let _ = inbox.send(NetEvent::Sealed { block, seal });
            })))
        }
        (ConsensusConfig::Authority { .. }, true, _) => return Err("--mine needs a proof-of-work chain".into()),
        (ConsensusConfig::ProofOfWork(_), _, true) => return Err("--produce needs an authority chain".into()),
    };
    let mut node = Node::new(db.clone(), &genesis, producer, net)?;

    if args.validate {
        if genesis.finality.is_none() {
            return Err("--validate needs a chain with finality".into());
        }

        let voter = cli::signer(args.validator_key.as_deref())?;
        log::info!("[node] validating with voting key {}", voter.public());
        node = node.validate(voter);
    }

    #[cfg(feature = "http")]
    if let Some(address) = args.http {
        let address = api::start(db, Some(node.submitter()), &genesis, address)?;
        log::info!("[http] listening on {address}");
    }

    node.run()?;

    Ok(())
}

#[cfg(not(feature = "node"))]
fn main() {
    eprintln!("chainv4 was built without the node feature");
}
