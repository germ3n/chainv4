#[cfg(feature = "node")]
use chainv4::{chain::{IndexKind, Indexing}, cli, node::tuning, structs};
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
    Bridge(Box<cli::BridgeArgs>),
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
    /// learns of from the others, a few at a time. Peers that dial in are taken up to
    /// --max-peers-in.
    #[arg(long, default_value_t = chainv4::net::DEFAULT_TARGET_PEERS as u32, value_parser = clap::value_parser!(u32).range(1..=1000))]
    target_peers: u32,
    /// The most connections peers that dial in may hold at once, 1 to 10,000: past it a new one is
    /// refused as it is made. Peers this node dials are not counted. Without it, every one is
    /// taken.
    #[arg(long, value_parser = tuning::flag("net.max-peers-in"))]
    max_peers_in: Option<u64>,
    /// Do not look for peers on the local network with mDNS: for a server, whose network holds no
    /// nodes of yours, so peers come from --peer and Kademlia alone.
    #[arg(long)]
    no_mdns: bool,
    /// The most gossiped transactions and votes, and submitted transactions, that may wait for
    /// the node at once. More are dropped until it catches up; submissions get 503.
    #[arg(long, default_value_t = 10_000, value_parser = clap::value_parser!(u32).range(1..))]
    max_gossip_backlog: u32,
    /// The most of those any one peer's may be. More are dropped, and a peer that goes over
    /// --max-strikes times within --strike-window is disconnected.
    #[arg(long, default_value_t = 1_000, value_parser = clap::value_parser!(u32).range(1..))]
    max_gossip_per_peer: u32,
    /// How many times a peer may go over its share of the backlog within --strike-window before it
    /// is disconnected, 1 to 10,000: raise it for peers that relay bursts.
    #[arg(long, default_value = tuning::default_text("backlog.strikes"), value_parser = tuning::flag("backlog.strikes"))]
    max_strikes: u64,
    /// The window --max-strikes are counted in, 1s to 1d.
    #[arg(long, default_value = tuning::default_text("backlog.strike-window"), value_parser = tuning::flag("backlog.strike-window"))]
    strike_window: u64,
    /// The most that the peers of one public IP may have waiting together, so one host with many
    /// peer ids gets no more than a few shares. Loopback, as every peer over Tor is, and private
    /// addresses keep each peer to its own share alone. Never below --max-gossip-per-peer.
    #[arg(long, default_value = tuning::default_text("backlog.per-ip"), value_parser = tuning::flag("backlog.per-ip"))]
    max_gossip_per_ip: u64,
    /// Sync requests a second a peer may make, past --sync-burst: more go unanswered, and a peer
    /// that keeps at it is struck and disconnected as a flood is.
    #[arg(long, default_value = tuning::default_text("requests.rate"), value_parser = tuning::flag("requests.rate"))]
    sync_rate: u64,
    /// Sync requests a peer may make at once before --sync-rate holds it back.
    #[arg(long, default_value = tuning::default_text("requests.burst"), value_parser = tuning::flag("requests.burst"))]
    sync_burst: u64,
    /// The node's network key file, which its peer id is of, so the id is the same each start:
    /// made at the first start, readable by its owner alone. net.key beside --db unless given.
    #[arg(long)]
    net_key: Option<std::path::PathBuf>,
    /// How long a peer is banned once what it sent that breaks the rules adds up, such as two
    /// invalid blocks or ten bad signatures within ten minutes: 1h, 30m, 90s or 2d.
    #[arg(long, default_value = "1h", value_parser = cli::parse_duration)]
    ban_for: u64,
    /// How much a peer's faults may add up to within --ban-window before it is banned, 10 to
    /// 1,000,000: an invalid block or bytes that do not decode weigh 50, a flood 25, a bad
    /// signature 10.
    #[arg(long, default_value = tuning::default_text("ban.score"), value_parser = tuning::flag("ban.score"))]
    ban_score: u64,
    /// The window a peer's faults add up in, 1s to 7d: one with none for this long starts again
    /// from nothing.
    #[arg(long, default_value = tuning::default_text("ban.window"), value_parser = tuning::flag("ban.window"))]
    ban_window: u64,
    /// Ban a peer's IP as well as its peer id, so a new id from the same machine is refused too.
    /// It can catch honest nodes behind the same NAT or VPN.
    #[arg(long)]
    ban_ips: bool,
    /// Lift every ban this node kept from before, by peer id and by IP, as it starts. Bans are kept
    /// in the database until they end, so a restart alone lifts none.
    #[arg(long)]
    unban_all: bool,
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
    /// How long a dial through --tor-proxy may take, 5s to 10m: Tor builds a circuit to an onion
    /// first, which on a slow relay can take a while.
    #[arg(long, default_value = tuning::default_text("tor.timeout"), value_parser = tuning::flag("tor.timeout"))]
    tor_timeout: u64,
    /// Where to look for releases every --update-every: a manifest's URL, or off. Defaults to the one the
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
    /// How often to look for releases, 1m to 7d.
    #[cfg(feature = "updates")]
    #[arg(long, default_value = tuning::default_text("updates.every"), value_parser = tuning::flag("updates.every"))]
    update_every: u64,
    /// Do not wait for the disk on each commit: faster, but a power cut can lose the last few
    /// blocks, which peers give back. For benchmarks and tests.
    #[arg(long)]
    no_fsync: bool,
    /// The database's block cache, in MiB, which every column shares: more keeps more of the
    /// state in memory between blocks.
    #[arg(long, default_value_t = chainv4::db::DEFAULT_CACHE_MB)]
    db_cache: usize,
    /// How many pristine Lua states to keep ready for contract calls, 1 to 1,024: more takes
    /// memory for calls back to back. It changes no call's outcome.
    #[arg(long, default_value = tuning::default_text("lua.states"), value_parser = tuning::flag("lua.states"))]
    lua_states: u64,
    /// How many threads build those states, 1 to 64.
    #[arg(long, default_value = tuning::default_text("lua.builders"), value_parser = tuning::flag("lua.builders"))]
    lua_builders: u64,
    /// The WebAssembly module cache, in MiB of modules, 1 to 4,096: more keeps more contracts
    /// compiled between calls.
    #[arg(long, default_value = tuning::default_text("wasm.cache-mb"), value_parser = tuning::flag("wasm.cache-mb"))]
    wasm_cache_mb: u64,
    /// The most modules that cache holds, 1 to 1,000,000.
    #[arg(long, default_value = tuning::default_text("wasm.cache-modules"), value_parser = tuning::flag("wasm.cache-modules"))]
    wasm_cache_modules: u64,
    /// How far back to keep state to read and prove at: balances, calls and proofs at a past
    /// block, over the API and JSON-RPC. In blocks, or as a time, such as 14h, turned into blocks
    /// at the chain's block time as the node starts.
    #[arg(long, default_value = "10000")]
    state_history: chainv4::node::Span,
    /// Keep the state of every height from now on, never pruning it: an archive node.
    #[arg(long, conflicts_with = "state_history")]
    archive: bool,
    /// Prune old blocks: keep the bodies of only this many blocks below the finalized checkpoint
    /// (and never within the reorg depth of the tip), in blocks or as a time such as 7d. Older
    /// blocks' transactions, receipts and index entries go; headers, seals and justifications stay.
    /// Off by default: every block is kept.
    #[arg(long, conflicts_with = "archive")]
    prune_blocks: Option<chainv4::node::Span>,
    /// The most transactions a block this node produces may hold. Without it, blocks fill to the
    /// chain's max_block_bytes and gas limit, which every node checks; on a chain that bounds
    /// neither, 1,000. This only caps this node's own blocks.
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..))]
    max_block_txs: Option<u32>,
    /// How many milliseconds this node spends running transactions for each block it produces.
    /// Once they are up, what ran by then is the block and the rest wait for the next. A brake for
    /// a slow machine, so it does not miss its slot; without it, blocks fill to the chain's limits
    /// whatever they take to run.
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    max_build_ms: Option<u64>,
    /// The most transactions the mempool holds at once. Full, a transaction that can run and pays
    /// more pushes out the least paid one that is not its sender's next; one that pays no more is
    /// refused.
    #[arg(long, default_value_t = chainv4::mempool::MempoolLimits::DEFAULT.capacity as u32, value_parser = clap::value_parser!(u32).range(1..))]
    mempool_size: u32,
    /// The most transactions one sender may have waiting in the mempool, so no one key fills it.
    /// Raise it for a key of this node's own that sends many a block, such as a busy faucet's.
    #[arg(long, default_value_t = chainv4::mempool::MempoolLimits::DEFAULT.per_sender as u32, value_parser = clap::value_parser!(u32).range(1..))]
    mempool_per_sender: u32,
    /// How far past its sender's nonce at the tip a transaction's nonce may be, for the mempool to
    /// take it: 0 takes only the next.
    #[arg(long, default_value_t = chainv4::mempool::MempoolLimits::DEFAULT.nonce_gap)]
    max_nonce_gap: u64,
    /// How much, in percent, a transaction must raise both its max price and its tip to replace
    /// one of its sender's waiting at its nonce.
    #[arg(long, default_value_t = chainv4::mempool::MempoolLimits::DEFAULT.replace_bump, value_parser = clap::value_parser!(u32).range(0..=1_000))]
    replace_bump_percent: u32,
    /// How long a transaction may offer less than the base price before the mempool drops it: in
    /// blocks, 32 unless given, or as a time, such as 3m, turned into blocks at the chain's block
    /// time as the node starts. 1 to 100,000 blocks.
    #[arg(long)]
    underpriced_for: Option<chainv4::node::Span>,
    /// The most bytes of blocks this node answers a syncing peer's request with, past its first
    /// block: it sends fewer blocks, and the peer asks again from there. 1 MiB to 32 MiB.
    #[arg(long, default_value_t = chainv4::net::SYNC_BATCH_BYTES, value_parser = clap::value_parser!(u64).range(1024 * 1024..=chainv4::net::MOST_SYNC_BATCH_BYTES))]
    sync_batch_bytes: u64,
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
    /// Let anyone join the pool, or mine alone with --mining-solo, with only a payout address,
    /// without the token.
    #[arg(long, requires = "mining", conflicts_with = "mining_token")]
    mining_open: bool,
    /// Take workers mining alone on the mining port: each is handed a block of its own, whose
    /// reward and fees go to the --payout it gives, in full. A template is built for each payout,
    /// up to 16 of them.
    #[arg(long, requires = "mining")]
    mining_solo: bool,
    /// What the node shares of itself with whoever asks its API: public keeps the addresses of its
    /// peers, workers and itself, and its workers' names; full shares everything; minimal its own
    /// state alone. Asked from this machine, or with --report-token, it shares everything.
    #[cfg(feature = "http")]
    #[arg(long, default_value = "public")]
    report: chainv4::api::sharing::ReportPreset,
    /// Keep these fields to the node on top of --report, such as counters,devices: peer-addresses,
    /// peer-ids, peers, listening, worker-addresses, worker-names, worker-payouts, workers, devices,
    /// mining, counters, backlog, syncing, uptime, role, limits.
    #[cfg(feature = "http")]
    #[arg(long, value_delimiter = ',')]
    hide: Vec<chainv4::api::sharing::ReportField>,
    /// Share these fields on top of --report, such as peer-addresses.
    #[cfg(feature = "http")]
    #[arg(long, value_delimiter = ',')]
    show: Vec<chainv4::api::sharing::ReportField>,
    /// The file holding a token that shows whoever bears it everything, as Authorization: Bearer
    /// TOKEN: at least 32 characters, such as openssl rand -hex 32 makes. Send it over HTTPS or
    /// Tor alone.
    #[cfg(feature = "http")]
    #[arg(long)]
    report_token: Option<std::path::PathBuf>,
    /// A reverse proxy whose X-Forwarded-For header the API believes, so it counts the client it
    /// names against the per-IP limits rather than the proxy: its address, or a host's name, which
    /// is looked up again every few seconds, for a container whose address may change. Loopback is
    /// always believed, which covers a proxy on the same host. Without this, a proxy anywhere else
    /// makes every visitor one client. Name only what you run, as whatever is named can claim to
    /// be anyone. Repeat for several.
    #[cfg(feature = "http")]
    #[arg(long, value_name = "IP_OR_HOST")]
    trusted_proxy: Vec<String>,
    /// Where workers reach the mining port from outside, such as 203.0.113.7:4002 or
    /// ADDRESS.onion:4002, as GET /status and GET /pool tell the apps; by default they take the
    /// API's host and the port --mining listens on.
    #[arg(long, requires = "mining")]
    mining_public: Option<String>,
    /// Where workers reach the mining port through Tor, ADDRESS.onion:4002, as GET /mining tells
    /// the apps that reach the node at its onion: they take this rather than --mining-public.
    #[arg(long, requires = "mining", conflicts_with = "mining_onion_port")]
    mining_onion: Option<String>,
    /// The port the node's onion offers the mining port on, for GET /mining to name it once tor has
    /// made the onion, as read from --onion-file: for when the onion is not known at the start.
    #[arg(long, requires = "mining", requires = "onion_file")]
    mining_onion_port: Option<u16>,
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
    /// A file holding the passphrase of the node's key files sealed with keygen --seal: the
    /// producer's, the validator's, the pool's push key and the faucet's. Without one, a sealed key
    /// is read from the file CHAINV4_KEY_PASSPHRASE_FILE names, else asked for on a terminal, and a
    /// node with none to ask stops.
    #[arg(long)]
    key_passphrase_file: Option<std::path::PathBuf>,
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
    /// How many requests one client may make a second, 0 to 100,000, beside --http-per-ip: a
    /// burst of up to a second's worth goes through, then 429 with Retry-After. 0 counts nothing.
    #[cfg(feature = "http")]
    #[arg(long, requires = "http", default_value = tuning::default_text("http.rate"), value_parser = tuning::flag("http.rate"))]
    http_rate: u64,
    /// The most any listing returns at once, 10 to 10,000, in place of each listing's own
    /// maximum, 100 to 1,000; the apps page on past it. Without it, each listing keeps its own.
    #[cfg(feature = "http")]
    #[arg(long, requires = "http", value_parser = tuning::flag("http.max-page"))]
    api_max_page: Option<u64>,
    /// How many times the next base price a suggested fee offers, 1 to 100: what eth_gasPrice says,
    /// and the max price POST /offer fills in when left open, under a base fee. More outlasts a
    /// faster rise of the base price, and holds more of the sender's funds back while it waits.
    #[cfg(feature = "http")]
    #[arg(long, requires = "http", default_value = tuning::default_text("http.suggest-multiplier"), value_parser = tuning::flag("http.suggest-multiplier"))]
    suggest_multiplier: u64,
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
    /// The atomic swap contract on this chain, contracts/lua/swap.lua as deployed here, when the
    /// chain's directory names none: GET /status names the directory's first, for the wallet's
    /// Swaps page and chainv4 swap, and shows this apart under operator.
    #[cfg(feature = "swap")]
    #[arg(long, requires = "http")]
    swap_contract: Option<structs::address::Address>,
    /// The DEX on this chain, contracts/lua/dex.lua as deployed here, when the chain's directory
    /// names none: GET /status names the directory's first, for the wallet's Trade page, and shows
    /// this apart under operator.
    #[cfg(feature = "dex")]
    #[arg(long, requires = "http")]
    dex_contract: Option<structs::address::Address>,
    /// The EVM DEX's router on this chain, DexRouter from contracts/evm/Dex.sol, by its 20 bytes
    /// as 0x and 40 hex digits, when the chain's directory names none: GET /status names the
    /// directory's first, for the Trade page's EVM pools, and shows this apart under operator.
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
    /// Keep an index of multisigs by their members, for the API's GET
    /// /accounts/{address}/multisigs: what a wallet lists every multisig its accounts belong to
    /// from. It follows the multisig registry whole, so it takes no FROM: on a chain already
    /// running it is built from the registry at once. Once on, the database keeps it on; off
    /// deletes it.
    #[arg(long, value_name = "on|off", num_args = 0..=1, require_equals = true, default_missing_value = "on", value_parser = on_or_off)]
    index_multisigs: Option<bool>,
    /// On a database that holds only genesis, sync from a snapshot: check every header peers have,
    /// take the state of a block --snapshot-depth below the best tip from them, and run only the
    /// blocks after it. The node then keeps headers alone for the blocks before it.
    #[arg(long)]
    snapshot: bool,
    /// How far below the best tip --snapshot takes its state: 16 blocks unless given, at most half
    /// the chain's max_reorg_depth. Peers serve any block their --state-history still holds, so
    /// they need not agree on it. A chain shorter than twice it is synced by running its blocks.
    #[arg(long, requires = "snapshot", value_parser = tuning::flag("snapshot.depth"))]
    snapshot_depth: Option<u64>,
    /// With --snapshot, a finalized checkpoint's block id, as 0x hex, from someone you trust: the
    /// node fetches no header below it, holds it as final, and syncs on from there, so a
    /// long-range fork below it cannot reach the node. GET /status of a node you trust shows one
    /// under finality.finalized.block.
    #[arg(long, requires = "snapshot")]
    checkpoint: Option<String>,
    /// With --snapshot, start at the checkpoint the anchors pick rather than one you are told:
    /// hear what each peer's chain anchored in Bitcoin, check it against the Bitcoin headers their
    /// relays hold, the most work winning, and take the newest checkpoint of the history Bitcoin
    /// saw first (docs/anchoring.md). Hears from anchors.peers peers first, 2 unless tuned.
    #[arg(long, requires = "snapshot", conflicts_with = "checkpoint")]
    anchored: bool,
    /// With --anchored, an Ethereum beacon API serving the light client routes, such as
    /// https://lodestar-sepolia.chainsafe.io, to follow Ethereum's sync committees from the
    /// chain's eth_start through, so Ethereum anchors are checked by their signatures here; give
    /// it again for others to fall back to. Nothing it serves is trusted. Without it, Ethereum
    /// anchors decide nothing.
    #[arg(long, requires = "anchored")]
    beacon: Vec<String>,
    /// The login of each --beacon, by order: file:PATH or env:NAME, holding user:password or a
    /// token.
    #[arg(long, requires = "beacon")]
    beacon_auth: Vec<String>,
    /// A SOCKS5 proxy for the node's outside connections, --beacon's: socks5://host:port, or
    /// socks5h://host:port to resolve names there too, as Tor wants.
    #[arg(long, requires = "beacon")]
    proxy: Option<String>,
    /// Tune what the node takes on and how fast, as KEY=VALUE, repeated or comma-separated, such
    /// as sync.headers=512,gossip.wait=20ms: the keys are sync., gossip., dial., peers., node.,
    /// mining., http., range., rpc., filter., finality. and chain., in docs/node.md with their
    /// defaults and bounds. None of it is consensus; unset, each is what every node did before.
    #[arg(long, value_name = "KEY=VALUE", value_delimiter = ',', value_parser = tuning::setting)]
    tune: Vec<(&'static str, u64)>,
}

/// How many events a slow /stream client may fall behind by before it misses some.
#[cfg(feature = "stream")]
const STREAM_BACKLOG: usize = 1_024;

#[cfg(feature = "node")]
fn main() -> std::process::ExitCode {
    use clap::Parser;

    // libp2p's own info lines are mostly noise for an operator; RUST_LOG still overrides this.
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info,libp2p=warn,libp2p_gossipsub::behaviour=error"))
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
        Some(Command::Bridge(args)) => cli::bridge(*args),
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
        chainv4::net::{self, NetConfig},
        chainv4::node::{Mining, MiningPort, Node, Producing},
        std::sync::Arc,
    };

    let dev = DevAuthority::dev_keypair();
    // Everything tuned past the plain flags, which each part of the node takes its piece of.
    let mut tuned = chainv4::node::Tuning::new().with(&args.tune);
    tuned.set("ban.score", args.ban_score).set("ban.window", args.ban_window).set("backlog.strikes", args.max_strikes).set("backlog.strike-window", args.strike_window).set("tor.timeout", args.tor_timeout);
    tuned.set("backlog.per-ip", args.max_gossip_per_ip).set("requests.rate", args.sync_rate).set("requests.burst", args.sync_burst);
    tuned.set("lua.states", args.lua_states).set("lua.builders", args.lua_builders).set("wasm.cache-mb", args.wasm_cache_mb).set("wasm.cache-modules", args.wasm_cache_modules);

    if let Some(most) = args.max_peers_in {
        tuned.set("net.max-peers-in", most);
    }

    if let Some(depth) = args.snapshot_depth {
        tuned.set("snapshot.depth", depth);
    }

    #[cfg(feature = "updates")]
    tuned.set("updates.every", args.update_every);
    #[cfg(feature = "http")]
    {
        tuned.set("http.rate", args.http_rate).set("http.suggest-multiplier", args.suggest_multiplier);

        if let Some(most) = args.api_max_page {
            tuned.set("http.max-page", most);
        }
    }

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

    // The snapshot's depth is the syncing node's alone, but held to half the chain's reorg depth,
    // as the block whose state it takes must stay where the chain can no longer roll back past.
    if let Some(depth) = args.snapshot_depth
        && depth > (genesis.max_reorg_depth / 2).max(1)
    {
        return Err(format!("--snapshot-depth {depth} is past half this chain's max_reorg_depth, {}", (genesis.max_reorg_depth / 2).max(1)).into());
    }

    if !tuned.changed().is_empty() {
        let changed: Vec<String> = tuned.changed().into_iter().map(|(key, value)| format!("{key}={value}")).collect();
        log::info!("[node] tuned {}", changed.join(", "));
    }

    // The network key, beside the database's directory unless --net-key names its file.
    let net_key = args.net_key.clone().unwrap_or_else(|| {
        let parent = args.db.parent().filter(|parent| !parent.as_os_str().is_empty()).unwrap_or(std::path::Path::new("."));

        parent.join(net::NET_KEY_FILE)
    });
    // Tor carries no UDP: under --tor-only the node listens on TCP alone, for its onion.
    if args.tor_only && args.listen.iter().any(|address| !address.iter().any(|part| matches!(part, libp2p::multiaddr::Protocol::Tcp(_)))) {
        return Err("--tor-only listens on TCP alone, for tor to publish: give --listen /ip4/127.0.0.1/tcp/4001".into());
    }

    // The bans kept from before hold for what they have left, unless lifted as the node starts.
    if args.unban_all {
        let lifted = chainv4::node::lift_bans(db.as_ref(), net::Unban::All)?;
        log::info!("lifted {lifted} kept ban(s), as --unban-all says");
    }

    let banned = chainv4::node::kept_bans(db.as_ref(), chainv4::node::now_ms())?;

    if !banned.is_empty() {
        log::info!("{} ban(s) kept from before hold until they end: --unban-all lifts them", banned.len());
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
        backlog: tuned.backlog(args.max_gossip_backlog as usize, args.max_gossip_per_peer as usize),
        key: Some(net::net_key(&net_key)?),
        remembered: chainv4::node::remembered(db.as_ref())?,
        bans: tuned.bans(std::time::Duration::from_millis(args.ban_for), args.ban_ips),
        banned,
        tor: net::tor::TorConfig {
            proxy: args.tor_proxy.clone(),
            only: args.tor_only,
            onion_file: args.onion_file.clone(),
            timeout: tuned.tor_timeout(),
        },
        tuning: tuned.net(),
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
                log::info!("[updates] looking for releases at {url} every {} s{}", args.update_every / 1_000, if args.auto_update { ", updating on its own" } else { "" });
                let config = chainv4::node::UpdateConfig { url, proxy, auto: args.auto_update, binary: binary.clone(), every_ms: tuned.update_every_ms() };
                chainv4::node::spawn_updates(config, releases.clone(), stop.clone(), restart.clone())?;
            }
            (false, _) if args.updates.is_some() || args.auto_update => log::warn!("[updates] this build trusts no release keys, so it looks for no releases"),
            _ => {}
        }

        releases
    };
    // What this node produces with: a miner's settings, a signing key, or both. The node builds
    // the producer for whatever consensus the chain runs, and again when governance changes it.
    let open = args.mining_open;

    #[cfg(feature = "pool")]
    let pooled = args.pool;
    #[cfg(not(feature = "pool"))]
    let pooled = false;

    if open && !pooled && !args.mining_solo {
        return Err("--mining-open opens a pool or mining alone to anyone: it needs --pool or --mining-solo".into());
    }

    let port = match (args.mining, &args.mining_token) {
        (Some(listen), token) => Some(MiningPort {
            listen,
            token: match (token, open) {
                (Some(token), _) => chainv4::mining::read_token(token)?,
                (None, true) => chainv4::mining::pool::OPEN_TOKEN.to_vec(),
                (None, false) => return Err("--mining needs --mining-token, or --mining-open with --pool or --mining-solo for a port anyone may join".into()),
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
            solo: args.mining_solo,
            limits: tuned.port(),
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
        true => Some(cli::signer_with(args.producer_key.as_deref(), args.key_passphrase_file.as_deref())?),
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
    let mut node = Node::new_with(db.clone(), &genesis, None, net, tuned.chain())?;
    // What is counted in blocks but may be given as a time goes by the chain's block time now.
    let block_ms = node.chain().consensus_config_at(node.chain().tip().height + 1)?.block_ms();
    let underpriced = match args.underpriced_for {
        Some(span) => span.blocks(block_ms),
        None => u64::from(chainv4::mempool::MAX_UNDERPRICED_BLOCKS),
    };

    if !(1..=100_000).contains(&underpriced) {
        return Err(format!("--underpriced-for is {underpriced} blocks at {block_ms} ms a block: give 1 to 100,000 blocks").into());
    }

    let state_history = args.state_history.blocks(block_ms);

    if state_history == 0 {
        return Err("--state-history keeps at least one block".into());
    }

    node = node
        .max_block_txs(args.max_block_txs.map(|most| most as usize))
        .max_build_ms(args.max_build_ms)
        .mempool_limits(chainv4::mempool::MempoolLimits {
            capacity: args.mempool_size as usize,
            per_sender: args.mempool_per_sender as usize,
            nonce_gap: args.max_nonce_gap,
            replace_bump: args.replace_bump_percent,
            underpriced_blocks: underpriced as u32,
        })
        .sync_batch_bytes(args.sync_batch_bytes)
        .tuning(tuned.node())
        .ahead(tuned.ahead());

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
        node = node.pushing(chainv4::node::PoolPush { key: cli::signer_with(Some(key), args.key_passphrase_file.as_deref())?, least });
    }

    if args.snapshot {
        let checkpoint = match &args.checkpoint {
            Some(text) => {
                let bytes = chainv4::structs::hex::from_hex(text).ok_or("--checkpoint is not hex")?;

                Some(chainv4::structs::hash::Hash::from_bytes(genesis.hash, &bytes).ok_or("--checkpoint is not as long as the chain's ids")?)
            }
            None => None,
        };

        #[cfg(feature = "anchoring")]
        if args.anchored {
            match node.sync_from_anchors()? {
                true => log::info!("[node] syncing from a snapshot, at the checkpoint the anchors pick"),
                false => log::info!("[node] the database holds more than genesis already: syncing as usual, not from a snapshot"),
            }
        }

        #[cfg(all(feature = "anchoring-eth", feature = "client"))]
        if !args.beacon.is_empty() {
            let http: std::sync::Arc<dyn chainv4::remote::Http> = std::sync::Arc::new(chainv4::remote::UreqHttp::new(args.proxy.as_deref())?);
            let endpoints = chainv4::remote::Endpoints::from_flags(&args.beacon, &args.beacon_auth, "beacon", false)?;
            log::info!("[node] Ethereum anchors are checked by sync committees followed through {}", endpoints.hosts().join(", "));
            node.anchor_beacon(std::sync::Arc::new(chainv4::remote::beacon::Beacon::new(http, endpoints)));
        }

        #[cfg(not(all(feature = "anchoring-eth", feature = "client")))]
        if !args.beacon.is_empty() {
            return Err("--beacon needs a build with the anchoring-eth and client features".into());
        }

        #[cfg(not(feature = "anchoring"))]
        if args.anchored {
            return Err("--anchored needs a build with the anchoring feature".into());
        }

        if !args.anchored {
            match node.sync_from_snapshot(checkpoint)? {
                true if checkpoint.is_some() => log::info!("[node] syncing from a snapshot, starting at the checkpoint"),
                true => log::info!("[node] syncing from a snapshot"),
                false => log::info!("[node] the database holds more than genesis already: syncing as usual, not from a snapshot"),
            }
        }
    }

    if args.validate {
        if !genesis.finality.as_ref().is_some_and(chain::FinalityConfig::votes) {
            return Err("--validate needs a chain whose finality takes votes".into());
        }

        let voter = cli::signer_with(args.validator_key.as_deref(), args.key_passphrase_file.as_deref())?;
        log::info!("[node] validating with voting key {}", voter.public());
        node = node.validate(voter);
    }

    #[cfg(feature = "stream")]
    if args.stream {
        node = node.stream(STREAM_BACKLOG);
    }

    node = node.keep_state_history((!args.archive).then_some(state_history));

    if let Some(prune) = args.prune_blocks {
        let keep = prune.blocks(block_ms);

        if keep == 0 {
            return Err("--prune-blocks keeps at least one block".into());
        }

        node = node.keep_blocks(Some(keep));
    }

    for (kind, asked) in [(IndexKind::Events, args.index_events), (IndexKind::Addresses, args.index_addresses)] {
        match asked {
            Some(Indexing::Off) => node = node.drop_index(kind)?,
            Some(Indexing::Keep(keep)) => node = node.keep_index(kind, Some(keep))?,
            None if node.chain().index_window(kind).is_some() => node = node.keep_index(kind, None)?,
            None => {}
        }
    }

    match args.index_multisigs {
        Some(false) => node = node.drop_member_index()?,
        Some(true) => node = node.keep_member_index()?,
        None if node.chain().member_index() => node = node.keep_member_index()?,
        None => {}
    }

    #[cfg(feature = "http")]
    if let Some(address) = args.http {
        #[cfg(feature = "faucet")]
        let faucet = match &args.faucet_key {
            Some(path) => {
                let key = cli::signer_with(Some(path), args.key_passphrase_file.as_deref()).map_err(|err| format!("the faucet's key: {err}"))?;
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
            mining_public: args.mining_public.clone(),
            mining_onion: match (&args.mining_onion, args.mining_onion_port, &args.onion_file) {
                (Some(at), _, _) => Some(api::MiningOnion::At(at.clone())),
                (None, Some(port), Some(file)) => Some(api::MiningOnion::InFile { file: file.clone(), port }),
                _ => None,
            },
            sharing: api::sharing::Sharing::new(args.report, &args.hide, &args.show)?,
            report_token: args.report_token.as_deref().map(chainv4::mining::read_token).transpose()?,
            trusted_proxies: args.trusted_proxy.clone(),
            limits: tuned.api(),
            pools: tuned.pools(),
            tuning: tuned.clone(),
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

/// A switch as --index-multisigs takes it: on, or off to drop what is kept.
#[cfg(feature = "node")]
fn on_or_off(text: &str) -> Result<bool, String> {
    match text.trim().to_ascii_lowercase().as_str() {
        "on" => Ok(true),
        "off" => Ok(false),
        other => Err(format!("{other} is not on or off")),
    }
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

#[cfg(all(test, feature = "node", feature = "http"))]
mod tests {
    use {super::*, clap::Parser};

    fn parse(flags: &[&str]) -> Result<NodeArgs, String> {
        Cli::try_parse_from(std::iter::once("chainv4").chain(flags.iter().copied())).map(|cli| cli.node).map_err(|err| err.to_string())
    }

    #[test]
    fn a_node_with_none_of_the_new_flags_takes_every_default_as_before() {
        let args = parse(&[]).unwrap();

        assert_eq!((args.ban_score, args.ban_window, args.max_strikes, args.strike_window), (100, 600_000, 10, 60_000));
        assert_eq!((args.tor_timeout, args.http_rate, args.suggest_multiplier), (90_000, 0, 2));
        assert_eq!((args.lua_states, args.lua_builders, args.wasm_cache_mb, args.wasm_cache_modules), (16, 2, 32, 1_024));
        assert_eq!((args.max_peers_in, args.api_max_page, args.snapshot_depth, args.underpriced_for), (None, None, None, None));
        assert_eq!(args.state_history, chainv4::node::Span::Blocks(chainv4::chain::DEFAULT_STATE_HISTORY));
        assert_eq!((args.prune_blocks, args.key_passphrase_file), (None, None), "every block kept, as before");
        #[cfg(feature = "updates")]
        assert_eq!(args.update_every, chainv4::node::CHECK_EVERY_MS);
        assert!(args.tune.is_empty());
    }

    #[test]
    fn the_new_flags_read_their_units_and_refuse_what_would_break_the_node() {
        let args = parse(&["--max-peers-in", "40", "--ban-score", "300", "--ban-window", "30m", "--max-strikes", "50", "--strike-window", "5m", "--tor-timeout", "3m", "--lua-states", "64", "--wasm-cache-mb", "256", "--state-history", "14h", "--underpriced-for", "3m"]).unwrap();

        assert_eq!((args.max_peers_in, args.ban_score, args.ban_window, args.max_strikes, args.strike_window), (Some(40), 300, 1_800_000, 50, 300_000));
        assert_eq!((args.tor_timeout, args.lua_states, args.wasm_cache_mb), (180_000, 64, 256));
        assert_eq!((args.state_history, args.underpriced_for), (chainv4::node::Span::Ms(14 * 3_600_000), Some(chainv4::node::Span::Ms(180_000))));

        assert_eq!(parse(&["--prune-blocks", "7d"]).unwrap().prune_blocks, Some(chainv4::node::Span::Ms(7 * 86_400_000)));
        assert_eq!(parse(&["--prune-blocks", "50000"]).unwrap().prune_blocks, Some(chainv4::node::Span::Blocks(50_000)));
        assert!(parse(&["--prune-blocks", "50000", "--archive"]).is_err(), "an archive keeps every block");

        let http = parse(&["--http", "127.0.0.1:0", "--http-rate", "20", "--api-max-page", "50", "--suggest-multiplier", "3"]).unwrap();
        assert_eq!((http.http_rate, http.api_max_page, http.suggest_multiplier), (20, Some(50), 3));

        for refused in [
            &["--max-peers-in", "0"][..],
            &["--ban-score", "0"],
            &["--ban-window", "soon"],
            &["--max-strikes", "0"],
            &["--tor-timeout", "1s"],
            &["--lua-states", "0"],
            &["--lua-builders", "100"],
            &["--wasm-cache-mb", "0"],
            &["--wasm-cache-modules", "0"],
            &["--http", "127.0.0.1:0", "--suggest-multiplier", "0"],
            &["--http", "127.0.0.1:0", "--api-max-page", "5"],
            &["--http-rate", "5"],
            &["--snapshot-depth", "4"],
            &["--snapshot", "--snapshot-depth", "0"],
            &["--state-history", "later"],
        ] {
            assert!(parse(refused).is_err(), "{refused:?}");
        }
    }

    #[test]
    fn tune_takes_keys_repeated_or_comma_separated_and_refuses_what_it_does_not_know() {
        let args = parse(&["--tune", "sync.headers=512,gossip.wait=20ms", "--tune", "rpc.batch=200"]).unwrap();

        assert_eq!(args.tune, vec![("sync.headers", 512), ("gossip.wait", 20), ("rpc.batch", 200)]);
        let tuned = chainv4::node::Tuning::new().with(&args.tune);
        assert_eq!((tuned.node().sync_headers, tuned.net().tx_batch_wait, tuned.api().rpc.batch), (512, std::time::Duration::from_millis(20), 200));

        for refused in ["sync.headers=2048", "nothing=1", "rpc.batch", "ban.score=200", "gossip.batch=1000"] {
            assert!(parse(&["--tune", refused]).is_err(), "{refused}");
        }
    }
}
