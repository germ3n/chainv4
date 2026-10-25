#[cfg(feature = "node")]
use chainv4::{chain::{IndexKind, Indexing}, cli, structs};
#[cfg(feature = "http")]
use chainv4::api;

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
    Send(Box<cli::SendArgs>),
    /// Writes a genesis file with the development defaults, to edit and run with --genesis.
    Genesis(cli::genesis::GenesisArgs),
    /// Shows the storage layout contract code declares, and with --from, checks it keeps an
    /// older version's, as an upgrade or a proxy repointing would.
    Layout(cli::LayoutArgs),
    /// Which format a database is kept in, and migrating it to another: up, as a node does as it
    /// starts, or back down to an older chainv4's.
    Db(cli::DbArgs),
    /// A light client: follows a node's chain from the genesis it trusts alone, checking every
    /// header and justification, and answers only with what it proved.
    #[cfg(feature = "client")]
    Light(cli::LightArgs),
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
    /// The most gossiped transactions and votes, and submitted transactions, that may wait for
    /// the node at once. More are dropped until it catches up; submissions get 503.
    #[arg(long, default_value_t = 10_000, value_parser = clap::value_parser!(u32).range(1..))]
    max_gossip_backlog: u32,
    /// The most of those any one peer's may be. More are dropped, and a peer that goes over
    /// 10 times in a minute is disconnected.
    #[arg(long, default_value_t = 1_000, value_parser = clap::value_parser!(u32).range(1..))]
    max_gossip_per_peer: u32,
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
    /// Let web pages from this origin call the API, such as http://localhost:8081, or * for any.
    /// Repeat to allow several.
    #[cfg(feature = "http")]
    #[arg(long, requires = "http")]
    cors: Vec<String>,
    /// Serve the explorer web app built in this directory (trunk's dist) from the API's /.
    #[cfg(feature = "http")]
    #[arg(long, requires = "http")]
    explorer: Option<std::path::PathBuf>,
    /// Serve the wallet web app built in this directory (trunk's dist) from the API's /wallet/.
    #[cfg(feature = "http")]
    #[arg(long, requires = "http")]
    wallet: Option<std::path::PathBuf>,
    /// Tell clients what the node does as it happens, on the API's GET /stream.
    #[cfg(feature = "stream")]
    #[arg(long, requires = "http")]
    stream: bool,
    /// Keep an index of events, by contract, by name and by the addresses in them, for the API's
    /// GET /events. FROM is which blocks to keep: genesis (the default), a height, or latest-N, a
    /// window of the latest N blocks. Older blocks are indexed in the background. Once on, the
    /// database keeps it on, keeping the same blocks until told otherwise; off deletes it.
    #[arg(long, value_name = "FROM", num_args = 0..=1, require_equals = true, default_missing_value = "genesis")]
    index_events: Option<Indexing>,
    /// Keep an index of transactions, by every address they touch, for the API's GET
    /// /accounts/{address}/txs: what a wallet lists its history from. FROM is as for
    /// --index-events, and it is kept, and dropped, the same way.
    #[arg(long, value_name = "FROM", num_args = 0..=1, require_equals = true, default_missing_value = "genesis")]
    index_addresses: Option<Indexing>,
    /// On a database that holds only genesis, sync from a snapshot: check every header peers have,
    /// take the state of a block 16 below the best tip from them, and run only the blocks after
    /// it. The node then keeps headers alone for the blocks before it.
    #[arg(long)]
    snapshot: bool,
}

/// How many events a slow /stream client may fall behind by before it misses some.
#[cfg(feature = "stream")]
const STREAM_BACKLOG: usize = 1_024;

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
        Some(Command::Send(args)) => cli::send(*args),
        Some(Command::Genesis(args)) => cli::genesis::write(args),
        Some(Command::Layout(args)) => cli::layout(args),
        Some(Command::Db(args)) => cli::db(args),
        #[cfg(feature = "client")]
        Some(Command::Light(args)) => cli::light(args),
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
        chainv4::chain::{self, Genesis},
        chainv4::consensus::{AuthorityProducer, ConsensusConfig, DevAuthority, Miner, Producer},
        chainv4::db::{Database, RocksDb},
        chainv4::net::{self, BacklogLimits, NetConfig, NetEvent},
        chainv4::node::Node,
        std::sync::Arc,
    };

    let dev = DevAuthority::dev_keypair();
    let genesis: Genesis = match &args.genesis {
        Some(path) => cli::genesis::read(path)?,
        None => cli::genesis::dev(args.consensus),
    };
    let rocks = RocksDb::open(&args.db)?;
    // An older database is moved up to this node's format, after a backup, before anything reads it.
    cli::db::on_start(&rocks, &args.db)?;
    let db: Arc<dyn Database> = Arc::new(rocks);
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
        backlog: BacklogLimits {
            total: args.max_gossip_backlog as usize,
            per_peer: args.max_gossip_per_peer as usize,
        },
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

    if args.snapshot {
        match node.sync_from_snapshot()? {
            true => log::info!("[node] syncing from a snapshot"),
            false => log::info!("[node] the database holds more than genesis already: syncing as usual, not from a snapshot"),
        }
    }

    if args.validate {
        if !genesis.finality.as_ref().is_some_and(chain::FinalityConfig::votes) {
            return Err("--validate needs a chain whose finality takes votes".into());
        }

        let voter = cli::signer(args.validator_key.as_deref())?;
        log::info!("[node] validating with voting key {}", voter.public());
        node = node.validate(voter);
    }

    #[cfg(feature = "stream")]
    if args.stream {
        node = node.stream(STREAM_BACKLOG);
    }

    for (kind, asked) in [(IndexKind::Events, args.index_events), (IndexKind::Addresses, args.index_addresses)] {
        match asked {
            Some(Indexing::Off) => node = node.drop_index(kind)?,
            Some(Indexing::Keep(keep)) => node = node.keep_index(kind, Some(keep))?,
            None if node.chain().index_window(kind).is_some() => node = node.keep_index(kind, None)?,
            None => {}
        }
    }

    #[cfg(feature = "http")]
    if let Some(address) = args.http {
        let options = api::ApiOptions {
            cors: args.cors,
            explorer: args.explorer,
            wallet: args.wallet,
        };
        let address = api::start(db, Some(node.handle()), &genesis, address, options)?;
        log::info!("[http] listening on {address}");
    }

    node.run()?;

    Ok(())
}

#[cfg(not(feature = "node"))]
fn main() {
    eprintln!("chainv4 was built without the node feature");
}
