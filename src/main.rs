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
    /// Makes a key and writes it to a file, and prints its public key and address.
    Keygen(cli::KeygenArgs),
    /// Shows a key file's public key and addresses, never its secret.
    Key(cli::KeyArgs),
    /// Signs a transaction and submits it to a node's HTTP API.
    #[cfg(feature = "client")]
    Send(Box<cli::SendArgs>),
    /// A native multisig's transactions: shows one, signs a bundle send --multisig wrote, approves
    /// it on chain, withdraws an approval, and submits it once enough members back it.
    #[cfg(feature = "client")]
    Multisig(cli::MultisigArgs),
    /// Claims a miner's earnings from a mining pool's escrow, with the path the pool's node gives.
    #[cfg(feature = "client")]
    Claim(cli::ClaimArgs),
    /// Asks a node's faucet for a drip, for a dev or test chain, doing the proof of work it wants.
    #[cfg(all(feature = "client", feature = "sha256"))]
    Faucet(cli::FaucetArgs),
    /// Atomic swaps between chainv4, Bitcoin and EVM chains: makes and answers a swap's terms,
    /// locks, claims and refunds on chainv4 and EVM chains, signs Bitcoin's claim or refund of its
    /// HTLC, and deploys Swap.sol.
    #[cfg(all(feature = "client", feature = "swap"))]
    Swap(cli::SwapArgs),
    /// The chain's DEXes: deploys and sets one up, lists its pools, quotes a trade through the
    /// best route, and trades, adds and takes out, on the native DEX or with --evm the router.
    #[cfg(all(feature = "client", feature = "dex", feature = "evm"))]
    Dex(cli::DexArgs),
    /// The chain's NFT and multi-token collections: lists the registry, deploys a factory and
    /// creates collections through it, and mints, sends, shows and lists what is held.
    #[cfg(all(feature = "client", feature = "nft"))]
    Nft(cli::NftArgs),
    /// The bridges: deploys and joins the gateways here and on EVM chains, sends the coin, tokens
    /// and NFTs across, and runs a federation signer's signing and relaying.
    #[cfg(all(feature = "client", feature = "bridge", feature = "swap"))]
    Bridge(cli::BridgeArgs),
    /// How many transactions a second the chain takes: blocks built and imported in-process
    /// (bench chain), or transfers submitted to running nodes at a rising rate (bench load).
    #[cfg(feature = "client")]
    Bench(cli::BenchArgs),
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
    /// Mines for a node over its mining port, on this machine's CPU threads or GPUs, with no chain
    /// of its own: for rigs that add their hashrate to one node.
    Worker(cli::WorkerArgs),
    /// Lists this machine's GPUs, by the numbers --gpus takes.
    #[cfg(feature = "gpu")]
    Gpus,
    /// Puts the newest chainv4 in place of this binary: from a node's GET /release or the
    /// manifest, once a threshold of the release keys built in signed it.
    #[cfg(feature = "updates")]
    Update(cli::UpdateArgs),
    /// Publishes releases: writes a product's release and its binaries for a static host, signs
    /// the manifest with a release key, and checks one.
    #[cfg(feature = "updates")]
    Release(cli::ReleaseArgs),
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
    /// A peer to dial at startup, and again every 15 seconds while no connection to it stands.
    /// Repeat to dial several. A /p2p/ part, the peer's id, is checked when given; none is needed.
    #[arg(long)]
    peer: Vec<libp2p::Multiaddr>,
    /// The name every protocol and gossip topic starts with, beside the genesis id: give chains
    /// built from this code a name of their own so their nodes never mix, even on one network.
    /// Peers under another name, or another genesis, are disconnected. Workers give the same.
    #[arg(long, default_value = chainv4::net::DEFAULT_PROTOCOL, value_parser = chainv4::net::protocol_name)]
    protocol: String,
    /// A bootnode of this node's own, beside the chain's from genesis and governance: dialed while
    /// the node has fewer peers than --target-peers, at most once a minute. Repeat for several.
    #[arg(long)]
    bootnode: Vec<libp2p::Multiaddr>,
    /// How many peers to look for: while it has fewer, the node dials bootnodes and peers it
    /// learns of from the others, a few at a time. Peers that dial in are taken regardless.
    #[arg(long, default_value_t = chainv4::net::DEFAULT_TARGET_PEERS as u32, value_parser = clap::value_parser!(u32).range(1..=1000))]
    target_peers: u32,
    /// Do not look for peers on the local network with mDNS: for a server, whose network holds no
    /// nodes of yours, so peers come from --peer and Kademlia alone.
    #[arg(long)]
    no_mdns: bool,
    /// The most gossiped transactions and votes, and submitted transactions, that may wait for
    /// the node at once. More are dropped until it catches up; submissions get 503.
    #[arg(long, default_value_t = 10_000, value_parser = clap::value_parser!(u32).range(1..))]
    max_gossip_backlog: u32,
    /// The most of those any one peer's may be. More are dropped, and a peer that goes over
    /// 10 times in a minute is disconnected.
    #[arg(long, default_value_t = 1_000, value_parser = clap::value_parser!(u32).range(1..))]
    max_gossip_per_peer: u32,
    /// The node's network key file, which its peer id is of, so the id is the same each start:
    /// made at the first start, readable by its owner alone. net.key beside --db unless given.
    #[arg(long)]
    net_key: Option<std::path::PathBuf>,
    /// How long a peer is banned once what it sent that breaks the rules adds up, such as two
    /// invalid blocks or ten bad signatures within ten minutes: 1h, 30m, 90s or 2d.
    #[arg(long, default_value = "1h", value_parser = cli::parse_duration)]
    ban_for: u64,
    /// Ban a peer's IP as well as its peer id, so a new id from the same machine is refused too.
    /// It can catch honest nodes behind the same NAT or VPN.
    #[arg(long)]
    ban_ips: bool,
    /// Tor's SOCKS5 proxy, socks5h://host:port: /onion3 peers and bootnodes are dialed through it,
    /// everything else directly. Listen on TCP too (--listen /ip4/0.0.0.0/tcp/4001) for tor to
    /// publish as the node's onion.
    #[arg(long)]
    tor_proxy: Option<String>,
    /// Every dial through --tor-proxy and no QUIC, so the node never shows its IP: it reaches onion
    /// peers, and TCP ones through Tor's exits. Its own listeners should be on 127.0.0.1, for tor.
    #[arg(long, requires = "tor_proxy")]
    tor_only: bool,
    /// The file tor writes the node's onion into, as a multiaddr, /onion3/…:PORT: read until it is
    /// there, then told to peers as the node's address.
    #[arg(long)]
    onion_file: Option<std::path::PathBuf>,
    /// Where to look for releases every hour: a manifest's URL, or off. Defaults to the one the
    /// release keys built in name; a build with no release keys looks nowhere. The node warns
    /// while it is behind, stops at an upgrade a release announces for its chain, and serves the
    /// release at GET /release.
    #[cfg(feature = "updates")]
    #[arg(long, value_name = "URL")]
    updates: Option<String>,
    /// Once a release names a newer chainv4 with a binary for this machine: download it, check it
    /// is the one signed, put it in place of this binary and restart on it, with the same flags.
    /// Not for a container, whose image is what updates it.
    #[cfg(feature = "updates")]
    #[arg(long)]
    auto_update: bool,
    /// Do not wait for the disk on each commit: faster, but a power cut can lose the last few
    /// blocks, which peers give back. For benchmarks and tests.
    #[arg(long)]
    no_fsync: bool,
    /// The database's block cache, in MiB, which every column shares: more keeps more of the
    /// state in memory between blocks.
    #[arg(long, default_value_t = chainv4::db::DEFAULT_CACHE_MB)]
    db_cache: usize,
    /// How many blocks back to keep state to read and prove at: balances, calls and proofs at a
    /// past block, over the API and JSON-RPC.
    #[arg(long, default_value_t = chainv4::chain::DEFAULT_STATE_HISTORY, value_parser = clap::value_parser!(u64).range(1..))]
    state_history: u64,
    /// Keep the state of every height from now on, never pruning it: an archive node.
    #[arg(long, conflicts_with = "state_history")]
    archive: bool,
    /// The most transactions a block this node produces may hold. Without it, blocks fill to the
    /// chain's max_block_bytes and gas limit, which every node checks; this only caps this node's
    /// own blocks.
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..))]
    max_block_txs: Option<u32>,
    /// How many milliseconds this node spends running transactions for each block it produces.
    /// Once they are up, what ran by then is the block and the rest wait for the next. A brake for
    /// a slow machine, so it does not miss its slot; without it, blocks fill to the chain's limits
    /// whatever they take to run.
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    max_build_ms: Option<u64>,
    /// Which development chain to run: pow, mined by anyone; authority, where the built-in
    /// development key produces every block; rotating, where the authority finality signers
    /// produce in turn, the development key being the one signer; or staked, where the FFG
    /// validators produce by stake, the development key being the one validator.
    #[arg(long, value_enum, default_value_t = cli::genesis::DevChain::Pow)]
    consensus: cli::genesis::DevChain,
    /// Run the chain a genesis file describes instead of a development chain.
    #[arg(long, conflicts_with = "consensus")]
    genesis: Option<std::path::PathBuf>,
    /// Mine blocks (pow chains, and chains whose governance may move to pow). With --produce
    /// too, the node does whichever the chain runs.
    #[arg(long)]
    mine: bool,
    /// Where mining rewards go. Defaults to the development key's address.
    #[arg(long, requires = "mine")]
    reward_to: Option<structs::address::Address>,
    /// How many threads to mine on; 0 for none, when workers mine for the node over --mining.
    #[arg(long, default_value_t = 1, requires = "mine")]
    mine_threads: usize,
    /// Mine on the GPUs instead of threads: through wgpu, on Vulkan, Metal or WebGPU, every GPU
    /// there is unless --gpus picks. Needs a build with the gpu feature.
    #[arg(long, requires = "mine", conflicts_with = "mine_threads")]
    mine_gpu: bool,
    /// The GPUs to mine on, by their number in chainv4 gpus, such as 0,2.
    #[arg(long, requires = "mine_gpu", value_delimiter = ',')]
    gpus: Vec<usize>,
    /// Take workers on this address, such as 0.0.0.0:4002: machines running chainv4 worker that
    /// mine for this node with no chain of their own. Each gets a range of nonces apart.
    #[arg(long, requires = "mine")]
    mining: Option<std::net::SocketAddr>,
    /// The file holding the token workers must know, for --mining: at least 32 characters, such as
    /// openssl rand -hex 32 makes. An open pool, --mining-open, takes none.
    #[arg(long, requires = "mining")]
    mining_token: Option<std::path::PathBuf>,
    /// Run a mining pool on the mining port: workers send shares at a difficulty set for each,
    /// and every block splits its reward among the last --pool-window blocks' worth of them,
    /// each worker's to the --payout address it gives. Their books are kept in pool.json beside
    /// the database.
    #[cfg(feature = "pool")]
    #[arg(long, requires = "mining")]
    pool: bool,
    /// What the pool keeps of each block, in percent, such as 1.5: its producer's, --reward-to.
    #[cfg(feature = "pool")]
    #[arg(long, requires = "pool", default_value = "0", value_parser = chainv4::mining::shares::fee_parts)]
    pool_fee: u32,
    /// How many blocks' worth of the latest shares each block splits by: more evens out luck,
    /// fewer pays sooner.
    #[cfg(feature = "pool")]
    #[arg(long, requires = "pool", default_value_t = 2, value_parser = clap::value_parser!(u64).range(1..=1000))]
    pool_window: u64,
    /// The easiest share difficulty the pool hands out, however slow the worker.
    #[cfg(feature = "pool")]
    #[arg(long, requires = "pool", default_value_t = 1_000)]
    pool_min_share: u128,
    /// Let anyone join the pool with only a payout address, without the token.
    #[cfg(feature = "pool")]
    #[arg(long, requires = "pool", conflicts_with = "mining_token")]
    mining_open: bool,
    /// Claim for the pool's miners once their unclaimed earnings reach this, in the coin's
    /// smallest unit, from --pool-push-key, which pays the fees: miners who never claim still get
    /// paid. Up to 64 claims a minute.
    #[cfg(feature = "pool")]
    #[arg(long, requires = "pool", requires = "pool_push_key")]
    pool_push: Option<String>,
    /// The key file that sends --pool-push's claims and pays their fees.
    #[cfg(feature = "pool")]
    #[arg(long, requires = "pool_push")]
    pool_push_key: Option<std::path::PathBuf>,
    /// Produce blocks (authority, rotating and staked chains), with the development key unless
    /// --producer-key names the authority's, or a signer's or validator's voting, key file.
    #[arg(long)]
    produce: bool,
    /// The authority's, or the signer's or validator's voting, key file, for --produce.
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
    /// How many threads the API answers on, side by side: one a core unless given.
    #[cfg(feature = "http")]
    #[arg(long, requires = "http", default_value_t = 0)]
    http_threads: usize,
    /// How many requests one client, by IP or the one a local proxy forwards for, may have in
    /// flight at once: more get 429.
    #[cfg(feature = "http")]
    #[arg(long, requires = "http", default_value_t = chainv4::api::DEFAULT_PER_IP)]
    http_per_ip: usize,
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
    /// Run a faucet at the API's POST /faucet, for a dev or test chain, paying from this key file's
    /// account: fund it first. Each address, and each IP, gets a drip once every --faucet-every.
    #[cfg(feature = "faucet")]
    #[arg(long, requires = "http")]
    faucet_key: Option<std::path::PathBuf>,
    /// Each drip, in whole coins, such as 10 or 0.5.
    #[cfg(feature = "faucet")]
    #[arg(long, requires = "faucet_key", default_value = "10")]
    faucet_amount: String,
    /// How long an address, and an IP, wait between drips: 24h, 30m, 90s or 2d.
    #[cfg(feature = "faucet")]
    #[arg(long, requires = "faucet_key", default_value = "24h", value_parser = cli::parse_duration)]
    faucet_every: u64,
    /// How many zero bits the proof of work each request carries starts with: each one doubles the
    /// asker's hashing. 20 is about a second in a browser.
    #[cfg(feature = "faucet")]
    #[arg(long, requires = "faucet_key", default_value_t = 20, value_parser = clap::value_parser!(u32).range(0..=40))]
    faucet_work: u32,
    /// The atomic swap contract on this chain, contracts/lua/swap.lua as deployed here: GET /status
    /// names it, for the wallet's Swaps page and chainv4 swap to use unless told another.
    #[cfg(feature = "swap")]
    #[arg(long, requires = "http")]
    swap_contract: Option<structs::address::Address>,
    /// The DEX on this chain, contracts/lua/dex.lua as deployed here: GET /status names it, for
    /// the wallet's Trade page to use when the name dex does not resolve.
    #[cfg(feature = "dex")]
    #[arg(long, requires = "http")]
    dex_contract: Option<structs::address::Address>,
    /// The EVM DEX's router on this chain, DexRouter from contracts/evm/Dex.sol, by its 20 bytes
    /// as 0x and 40 hex digits: GET /status names it, for the Trade page's EVM pools.
    #[cfg(feature = "dex")]
    #[arg(long, requires = "http")]
    dex_router: Option<String>,
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
    /// With --snapshot, a finalized checkpoint's block id, as 0x hex, from someone you trust: the
    /// node fetches no header below it, holds it as final, and syncs on from there, so a
    /// long-range fork below it cannot reach the node. GET /status of a node you trust shows one
    /// under finality.finalized.block.
    #[arg(long, requires = "snapshot")]
    checkpoint: Option<String>,
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
        Some(Command::Key(args)) => cli::key(args),
        #[cfg(feature = "client")]
        Some(Command::Send(args)) => cli::send(*args),
        #[cfg(all(feature = "client", feature = "sha256"))]
        Some(Command::Faucet(args)) => cli::faucet(args),
        #[cfg(feature = "client")]
        Some(Command::Multisig(args)) => cli::multisig(args),
        #[cfg(feature = "client")]
        Some(Command::Claim(args)) => cli::claim(args),
        #[cfg(all(feature = "client", feature = "swap"))]
        Some(Command::Swap(args)) => cli::swap(args),
        #[cfg(all(feature = "client", feature = "dex", feature = "evm"))]
        Some(Command::Dex(args)) => cli::dex(args),
        #[cfg(all(feature = "client", feature = "nft"))]
        Some(Command::Nft(args)) => cli::nft(args),
        #[cfg(all(feature = "client", feature = "bridge", feature = "swap"))]
        Some(Command::Bridge(args)) => cli::bridge(args),
        #[cfg(feature = "client")]
        Some(Command::Bench(args)) => cli::bench(args),
        Some(Command::Genesis(args)) => cli::genesis::write(args),
        Some(Command::Layout(args)) => cli::layout(args),
        Some(Command::Db(args)) => cli::db(args),
        #[cfg(feature = "client")]
        Some(Command::Light(args)) => cli::light(args),
        Some(Command::Worker(args)) => cli::worker(args),
        #[cfg(feature = "gpu")]
        Some(Command::Gpus) => cli::gpus(),
        #[cfg(feature = "updates")]
        Some(Command::Update(args)) => cli::update(args),
        #[cfg(feature = "updates")]
        Some(Command::Release(args)) => cli::release(args),
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
        chainv4::consensus::{ConsensusConfig, DevAuthority},
        chainv4::db::{Database, RocksDb},
        chainv4::net::{self, BacklogLimits, NetConfig},
        chainv4::node::{Mining, MiningPort, Node, Producing},
        std::sync::Arc,
    };

    let dev = DevAuthority::dev_keypair();
    let genesis: Genesis = match &args.genesis {
        Some(path) => cli::genesis::read(path)?,
        None => cli::genesis::dev(args.consensus),
    };
    let rocks = match args.no_fsync {
        true => RocksDb::open_with(&args.db, args.db_cache)?.unsynced(),
        false => RocksDb::open_with(&args.db, args.db_cache)?,
    };
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

    // The network key, beside the database's directory unless --net-key names its file.
    let net_key = args.net_key.clone().unwrap_or_else(|| {
        let parent = args.db.parent().filter(|parent| !parent.as_os_str().is_empty()).unwrap_or(std::path::Path::new("."));

        parent.join(net::NET_KEY_FILE)
    });
    // Tor carries no UDP: under --tor-only the node listens on TCP alone, for its onion.
    if args.tor_only && args.listen.iter().any(|address| !address.iter().any(|part| matches!(part, libp2p::multiaddr::Protocol::Tcp(_)))) {
        return Err("--tor-only listens on TCP alone, for tor to publish: give --listen /ip4/127.0.0.1/tcp/4001".into());
    }

    let net = net::spawn(NetConfig {
        listen: args.listen,
        peers: args.peer,
        bootnodes: args.bootnode,
        target_peers: args.target_peers as usize,
        mdns: !args.no_mdns,
        hash: genesis.hash,
        network,
        protocol: args.protocol.clone(),
        backlog: BacklogLimits {
            total: args.max_gossip_backlog as usize,
            per_peer: args.max_gossip_per_peer as usize,
        },
        key: Some(net::net_key(&net_key)?),
        remembered: chainv4::node::remembered(db.as_ref())?,
        bans: net::BanLimits {
            ban_for: std::time::Duration::from_millis(args.ban_for),
            ips: args.ban_ips,
        },
        tor: net::tor::TorConfig {
            proxy: args.tor_proxy.clone(),
            only: args.tor_only,
            onion_file: args.onion_file.clone(),
        },
    })?;
    // A container stops its node with SIGTERM, which a process with no handler ignores as PID 1;
    // Ctrl-C sends SIGINT. Either stops the loop, and the database closes as the node is dropped.
    // An update put in place stops it too, to start it again on the new binary.
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let restart = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    // Taken before an update can replace it: Linux names a replaced binary "… (deleted)".
    let binary = std::env::current_exe()?;
    #[cfg(feature = "updates")]
    let releases = {
        let keys = chainv4::release::ReleaseKeys::built_in()?;
        let url = match args.updates.as_deref() {
            Some("off") => None,
            Some(url) => Some(url.to_string()),
            None => keys.updates.clone(),
        };
        let on = keys.on();
        let releases = Arc::new(chainv4::node::Releases::new(keys, Some(beside(&args.db, "release.json"))));

        match (on, url) {
            (true, Some(url)) => {
                // Over Tor whenever the node keeps its IP to itself, and for an onion's manifest.
                let proxy = (args.tor_only || url.contains(".onion/")).then(|| args.tor_proxy.clone()).flatten();
                log::info!("[updates] looking for releases at {url} every hour{}", if args.auto_update { ", updating on its own" } else { "" });
                let config = chainv4::node::UpdateConfig { url, proxy, auto: args.auto_update, binary: binary.clone() };
                chainv4::node::spawn_updates(config, releases.clone(), stop.clone(), restart.clone())?;
            }
            (false, _) if args.updates.is_some() || args.auto_update => log::warn!("[updates] this build trusts no release keys, so it looks for no releases"),
            _ => {}
        }

        releases
    };
    // What this node produces with: a miner's settings, a signing key, or both. The node builds
    // the producer for whatever consensus the chain runs, and again when governance changes it.
    #[cfg(feature = "pool")]
    let open = args.mining_open;
    #[cfg(not(feature = "pool"))]
    let open = false;
    let port = match (args.mining, &args.mining_token) {
        (Some(listen), token) => Some(MiningPort {
            listen,
            token: match (token, open) {
                (Some(token), _) => chainv4::mining::read_token(token)?,
                (None, true) => chainv4::mining::pool::OPEN_TOKEN.to_vec(),
                (None, false) => return Err("--mining needs --mining-token, or --pool --mining-open for a pool anyone may join".into()),
            },
            protocol: args.protocol.clone(),
            #[cfg(feature = "pool")]
            pool: args.pool.then(|| chainv4::mining::shares::PoolConfig {
                fee: args.pool_fee,
                window: args.pool_window,
                min_share: args.pool_min_share.max(1),
                open,
                file: Some(beside(&args.db, "pool.json")),
            }),
            #[cfg(feature = "updates")]
            releases: Some(releases.clone()),
        }),
        _ => None,
    };

    if args.mine && args.mine_threads == 0 && !args.mine_gpu && port.is_none() {
        return Err("--mine-threads 0 mines nothing without --mining for workers to mine for the node".into());
    }

    let mine = args.mine.then(|| Mining {
        reward_to: args.reward_to.unwrap_or(dev.public().address()),
        threads: args.mine_threads,
        gpu: args.mine_gpu,
        gpus: (!args.gpus.is_empty()).then_some(args.gpus),
        port,
    });
    let sign = match args.produce {
        true => Some(cli::signer(args.producer_key.as_deref())?),
        false => None,
    };
    let pow = matches!(genesis.consensus, ConsensusConfig::ProofOfWork(_));

    if mine.is_some() && !pow && genesis.governance.is_none() {
        return Err("--mine needs a proof-of-work chain, or one whose governance may make it one".into());
    }

    if sign.is_some() && pow && genesis.governance.is_none() {
        return Err("--produce needs an authority, rotating or staked chain, or one whose governance may make it one".into());
    }

    let producing = (mine.is_some() || sign.is_some()).then_some(Producing { mine, sign });
    let mut node = Node::new(db.clone(), &genesis, None, net)?
        .max_block_txs(args.max_block_txs.map(|most| most as usize))
        .max_build_ms(args.max_build_ms);

    #[cfg(feature = "updates")]
    {
        node = node.releases(releases.clone());
    }

    if let Some(producing) = producing {
        node = node.producing(producing);
    }

    #[cfg(feature = "pool")]
    if let (Some(least), Some(key)) = (&args.pool_push, &args.pool_push_key) {
        let least = chainv4::structs::u256::u256::from_str_radix(least, 10).map_err(|_| format!("--pool-push {least} is not a whole number of the coin's smallest unit"))?;
        node = node.pushing(chainv4::node::PoolPush { key: cli::signer(Some(key))?, least });
    }

    if args.snapshot {
        let checkpoint = match &args.checkpoint {
            Some(text) => {
                let bytes = chainv4::structs::hex::from_hex(text).ok_or("--checkpoint is not hex")?;

                Some(chainv4::structs::hash::Hash::from_bytes(genesis.hash, &bytes).ok_or("--checkpoint is not as long as the chain's ids")?)
            }
            None => None,
        };

        match node.sync_from_snapshot(checkpoint)? {
            true if checkpoint.is_some() => log::info!("[node] syncing from a snapshot, starting at the checkpoint"),
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

    node = node.keep_state_history((!args.archive).then_some(args.state_history));

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
        #[cfg(feature = "faucet")]
        let faucet = match &args.faucet_key {
            Some(path) => {
                let text = std::fs::read_to_string(path).map_err(|err| format!("could not read the faucet's key {}: {err}", path.display()))?;
                let key = chainv4::crypto::keypair::Keypair::from_key_file(&text).map_err(|err| format!("the faucet's key {}: {err}", path.display()))?;
                let amount = cli::parse_coins(&args.faucet_amount, genesis.coin.decimals)?;
                log::info!("[faucet] {} {} a drip, from {}, every {} ms", args.faucet_amount, genesis.coin.symbol, key.public().address(), args.faucet_every);

                Some(api::FaucetConfig { key, amount, every_ms: args.faucet_every, work_bits: args.faucet_work })
            }
            None => None,
        };
        let options = api::ApiOptions {
            cors: args.cors,
            explorer: args.explorer,
            wallet: args.wallet,
            #[cfg(feature = "faucet")]
            faucet,
            #[cfg(feature = "swap")]
            swap_contract: args.swap_contract,
            #[cfg(feature = "dex")]
            dex_contract: args.dex_contract,
            #[cfg(feature = "dex")]
            dex_router: match args.dex_router.as_deref().map(evm_bytes).transpose() {
                Ok(router) => router,
                Err(why) => return Err(format!("--dex-router: {why}").into()),
            },
            threads: args.http_threads,
            per_ip: args.http_per_ip,
            #[cfg(feature = "updates")]
            releases: Some(releases.clone()),
        };
        let address = api::start(db, Some(node.handle()), &genesis, address, options)?;
        log::info!("[http] listening on {address}");
    }

    for signal in [signal_hook::consts::SIGTERM, signal_hook::consts::SIGINT] {
        signal_hook::flag::register(signal, stop.clone())?;
    }

    node.run(stop)?;
    log::info!("[node] stopped");

    if restart.load(std::sync::atomic::Ordering::Relaxed) {
        return start_again(&binary);
    }

    Ok(())
}

/// Starts the binary an update put in place with this one's flags: in this process on Unix, so
/// whatever supervises it sees no exit, and as a new one elsewhere.
#[cfg(feature = "node")]
fn start_again(binary: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    let mut command = std::process::Command::new(binary);
    command.args(std::env::args_os().skip(1));
    log::info!("[updates] starting {} again", binary.display());

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;

        Err(format!("could not start {} again: {}", binary.display(), command.exec()).into())
    }
    #[cfg(not(unix))]
    {
        command.spawn()?;

        Ok(())
    }
}

/// A file beside the database's directory, as pool.json and release.json are.
#[cfg(all(feature = "node", any(feature = "pool", feature = "updates")))]
fn beside(db: &std::path::Path, name: &str) -> std::path::PathBuf {
    db.parent().filter(|parent| !parent.as_os_str().is_empty()).unwrap_or(std::path::Path::new(".")).join(name)
}

#[cfg(not(feature = "node"))]
fn main() {
    eprintln!("chainv4 was built without the node feature");
}

/// 20 bytes of EVM address, from 0x and 40 hex digits.
#[cfg(all(feature = "dex", feature = "http"))]
fn evm_bytes(text: &str) -> Result<[u8; 20], String> {
    match text.trim().strip_prefix("0x").filter(|hex| hex.len() == 40) {
        Some(_) => structs::hex::from_hex(text.trim()).and_then(|bytes| bytes.try_into().ok()).ok_or_else(|| format!("{text} is not hex")),
        None => Err(format!("{text} is not 0x and 40 hex digits")),
    }
}
