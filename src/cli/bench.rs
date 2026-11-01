//! `chainv4 bench`: how many transactions a second the chain takes. `bench chain` times building
//! and importing full blocks of transfers on an in-process chain, with no network and no API, so
//! it shows what one machine can execute. `bench load` signs transfers from many funded keys and
//! submits them to running nodes, at a rate it raises step by step, and reports what they took:
//! where the API, gossip and blocks keep up, and where they fall behind.

use {
    super::CliResult,
    super::send::{TlsArgs, ok, pooled},
    crate::chain::{Allocation, Chain, Genesis},
    crate::consensus::{AuthorityProducer, DevAuthority, Producer},
    crate::crypto::keypair::{KeyType, Keypair},
    crate::db::{Database, MemoryDb, RocksDb},
    crate::structs::amount::Amount,
    crate::structs::hex::to_hex,
    crate::structs::tx::{FeeOffer, SignedTx, Tx},
    crate::structs::u256::u256,
    serde_json::{Value, json},
    std::path::PathBuf,
    std::sync::Arc,
    std::time::{Duration, Instant},
};

#[derive(clap::Args)]
pub struct BenchArgs {
    #[command(subcommand)]
    pub what: BenchWhat,
}

#[derive(clap::Subcommand)]
pub enum BenchWhat {
    /// Builds and imports full blocks of transfers on an in-process chain, and times each part.
    Chain {
        /// How many blocks to fill.
        #[arg(long, default_value_t = 5)]
        blocks: u32,
        /// Funded senders, each sending one transfer a block, so none waits on another's nonce.
        /// The block's gas limit caps what fits, about 13,800 Ed25519 transfers.
        #[arg(long, default_value_t = 16_000)]
        senders: u32,
        /// Keep the chain in RocksDB in this directory, synced as a node keeps it, rather than in
        /// memory.
        #[arg(long)]
        db: Option<PathBuf>,
        /// Write the results as JSON to this file too.
        #[arg(long)]
        json: Option<PathBuf>,
    },
    /// Submits signed transfers to running nodes at a rising rate, and reports what they took.
    Load {
        /// A node's HTTP API; repeat to spread the load over several.
        #[arg(long, required = true)]
        node: Vec<String>,
        #[command(flatten)]
        tls: TlsArgs,
        /// The key file holding the coins the senders are funded from.
        #[arg(long)]
        key: Option<PathBuf>,
        /// How many senders to fund and send from.
        #[arg(long, default_value_t = 200)]
        senders: u32,
        /// The rates to try, in transfers a second, each for --step.
        #[arg(long, value_delimiter = ',', default_value = "50,100,200,400,800,1600")]
        rates: Vec<u32>,
        /// How long each rate runs, in seconds.
        #[arg(long, default_value_t = 20)]
        step: u64,
        /// How many requests are in flight at once.
        #[arg(long, default_value_t = 32)]
        threads: usize,
        #[arg(long)]
        json: Option<PathBuf>,
    },
}

pub fn bench(args: BenchArgs) -> CliResult<()> {
    match args.what {
        BenchWhat::Chain { blocks, senders, db, json } => {
            let results = chain(blocks, senders, db)?;
            print_chain(&results);
            write_json(json, &json!(results.iter().map(BlockTimes::json).collect::<Vec<_>>()))
        }
        BenchWhat::Load { node, tls, key, senders, rates, step, threads, json } => {
            let results = load(&node, &tls, key.as_deref(), senders, &rates, Duration::from_secs(step), threads)?;
            write_json(json, &json!(results.iter().map(Step::json).collect::<Vec<_>>()))
        }
    }
}

fn write_json(path: Option<PathBuf>, value: &Value) -> CliResult<()> {
    if let Some(path) = path {
        std::fs::write(&path, serde_json::to_string_pretty(value)?)?;
        println!("wrote {}", path.display());
    }

    Ok(())
}

/// One block's timings, in milliseconds.
#[derive(Clone, Debug)]
pub struct BlockTimes {
    pub txs: usize,
    pub sign: f64,
    pub build: f64,
    pub import: f64,
}

impl BlockTimes {
    /// Transactions a second a node could take, importing blocks this fast.
    pub fn import_tps(&self) -> f64 {
        self.txs as f64 * 1_000.0 / self.import.max(f64::EPSILON)
    }

    fn json(&self) -> Value {
        json!({ "txs": self.txs, "sign_ms": self.sign, "build_ms": self.build, "import_ms": self.import, "import_tps": self.import_tps() })
    }
}

/// Builds blocks of transfers on one chain, as their producer, and imports each on a second, as
/// every other node does: what each takes. The senders' keys are the build's first scheme's,
/// Ed25519's by default.
pub fn chain(blocks: u32, senders: u32, db: Option<PathBuf>) -> CliResult<Vec<BlockTimes>> {
    const SLOT: u64 = 2_000;
    let authority = DevAuthority::dev_keypair();
    let keys: Vec<Keypair> = (0..senders)
        .map(|index| {
            let mut seed = [0x5a; 32];
            seed[..4].copy_from_slice(&index.to_le_bytes());

            Keypair::from_seed(seed)
        })
        .collect();
    let mut genesis = Genesis {
        max_block_bytes: Some(crate::chain::MAX_BLOCK_BYTES),
        ..Genesis::dev_authority(authority.public())
    };
    genesis.allocations.extend(keys.iter().map(|key| Allocation { address: key.public().address(), amount: u256::new(10u128.pow(21)) }));
    let open = |name: &str| -> CliResult<Arc<dyn Database>> {
        Ok(match &db {
            Some(dir) => Arc::new(RocksDb::open(dir.join(name))?),
            None => Arc::new(MemoryDb::default()),
        })
    };
    let (mut producer_chain, mut follower) = (Chain::open(open("producer")?, &genesis)?, Chain::open(open("follower")?, &genesis)?);
    let mut producer = AuthorityProducer::new(authority, SLOT);
    let mut results = Vec::new();

    let chain = producer_chain.genesis();

    for nonce in 0..u64::from(blocks) {
        let base = producer_chain.next_base_price();
        let gas_limit = 4_000;
        let started = Instant::now();
        let txs: Vec<SignedTx> = keys
            .iter()
            .enumerate()
            .map(|(index, key)| {
                let to = keys[(index + 1) % keys.len()].public().address();
                let fee = FeeOffer { gas_limit, max_price: base * u256::new(100), tip: u256::ZERO };

                SignedTx::sign(Tx::Transfer { from: key.public().address(), to, amount: Amount::U128(1) }, nonce, fee, key, &chain)
            })
            .collect();
        let sign = ms(started);

        let now = producer_chain.tip().timestamp + SLOT;
        let started = Instant::now();
        let mut block = producer_chain.build(txs, Vec::new(), producer.address(&[]), now)?;
        block.seal = producer.seal(&block.header).ok_or("the producer could not seal")?;
        let build = ms(started);
        producer_chain.import(&block, now).map_err(|err| format!("the producer refused its own block: {err}"))?;

        let started = Instant::now();
        follower.import(&block, now).map_err(|err| format!("the block did not import: {err}"))?;
        let import = ms(started);

        results.push(BlockTimes { txs: block.txs.len(), sign, build, import });
    }

    Ok(results)
}

fn ms(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1_000.0
}

fn print_chain(results: &[BlockTimes]) {
    println!("{:>6}  {:>10}  {:>10}  {:>10}  {:>12}", "txs", "sign ms", "build ms", "import ms", "import tps");

    for times in results {
        println!("{:>6}  {:>10.1}  {:>10.1}  {:>10.1}  {:>12.0}", times.txs, times.sign, times.build, times.import, times.import_tps());
    }

    if let Some(best) = results.iter().map(BlockTimes::import_tps).reduce(f64::max) {
        let block_ms = 2_000.0;
        println!("best import: {best:.0} transfers a second; a 2 s block of them takes {:.0}% of its slot to import", 100.0 * results.iter().map(|times| times.import).fold(0.0, f64::max) / block_ms);
    }
}

/// One rate tried against the nodes.
#[derive(Clone, Debug)]
pub struct Step {
    pub rate: u32,
    pub sent: u64,
    pub taken: u64,
    pub refused: u64,
    pub failed: u64,
    /// Transfers that landed in blocks during the step, a second.
    pub landed_tps: f64,
    /// The median and 99th-percentile time of a submission, in milliseconds.
    pub p50: f64,
    pub p99: f64,
}

impl Step {
    fn json(&self) -> Value {
        json!({ "rate": self.rate, "sent": self.sent, "taken": self.taken, "refused": self.refused, "failed": self.failed, "landed_tps": self.landed_tps, "p50_ms": self.p50, "p99_ms": self.p99 })
    }
}

/// Funds senders from the key, then submits transfers among them at each rate in turn, counting
/// what the nodes took and what landed in blocks.
pub fn load(nodes: &[String], tls: &TlsArgs, key: Option<&std::path::Path>, senders: u32, rates: &[u32], step: Duration, threads: usize) -> CliResult<Vec<Step>> {
    let agent = pooled(tls, threads)?;
    let nodes: Vec<String> = nodes.iter().map(|node| node.trim_end_matches('/').to_string()).collect();
    let funder = super::signer(key)?;
    let status = ok(agent.get(format!("{}/status", nodes[0])).call())?;
    let genesis = super::send::genesis_of(&status)?;
    let base: u256 = status["fees"]["base_price"].as_str().and_then(|text| u256::from_str_radix(text, 10).ok()).unwrap_or(u256::ZERO);
    let fee = FeeOffer { gas_limit: 4_000, max_price: (base * u256::new(4)).max(u256::ONE), tip: u256::ZERO };
    let keys: Vec<Keypair> = (0..senders).map(|_| Keypair::generate(KeyType::ALL[0])).collect::<Result<_, _>>()?;

    // Funding: one transfer each from the funder, then wait for the last to land.
    println!("funding {senders} senders from {}", funder.public().address());
    let mut nonce = nonce_of(&agent, &nodes[0], &funder)?;
    let each = u256::new(10u128.pow(18));

    for key in &keys {
        let tx = SignedTx::sign(Tx::Transfer { from: funder.public().address(), to: key.public().address(), amount: Amount::U256(each) }, nonce, fee, &funder, &genesis);
        submit(&agent, &nodes[0], &tx).map_err(|why| format!("funding refused: {why}"))?;
        nonce += 1;
    }

    let funded = Instant::now();

    while nonce_of(&agent, &nodes[0], &funder)? < nonce {
        if funded.elapsed() > Duration::from_secs(120) {
            return Err("the funding did not land within two minutes".into());
        }

        std::thread::sleep(Duration::from_millis(500));
    }

    let mut nonces = vec![0u64; keys.len()];
    let mut results = Vec::new();
    println!("{:>6}  {:>8}  {:>8}  {:>8}  {:>8}  {:>10}  {:>8}  {:>8}", "rate", "sent", "taken", "refused", "failed", "landed/s", "p50 ms", "p99 ms");

    for &rate in rates {
        let height = |agent: &ureq::Agent| -> CliResult<u64> { Ok(ok(agent.get(format!("{}/status", nodes[0])).call())?["height"].as_u64().unwrap_or(0)) };
        let (from_height, started) = (height(&agent)?, Instant::now());
        let total = (u64::from(rate) * step.as_secs()) as usize;
        let mut txs = Vec::with_capacity(total);

        for index in 0..total {
            let (sender, to) = (index % keys.len(), (index + 1) % keys.len());
            txs.push(SignedTx::sign(Tx::Transfer { from: keys[sender].public().address(), to: keys[to].public().address(), amount: Amount::U128(1) }, nonces[sender], fee, &keys[sender], &genesis));
            nonces[sender] += 1;
        }

        let (taken, refused, failed, mut latencies) = send_at(&agent, &nodes, txs, rate, threads);
        // What landed: transactions in the blocks made while the step ran, and a slot after.
        std::thread::sleep(Duration::from_secs(3));
        let to_height = height(&agent)?;
        let landed = (from_height + 1..=to_height).map(|height| block_txs(&agent, &nodes[0], height)).sum::<u64>();
        // A second of the step's own, the settling after it left out.
        let elapsed = (started.elapsed().as_secs_f64() - 3.0).max(step.as_secs_f64());
        latencies.sort_by(f64::total_cmp);
        let percentile = |p: f64| latencies.get(((latencies.len() as f64 - 1.0) * p).round() as usize).copied().unwrap_or(0.0);
        let step = Step { rate, sent: total as u64, taken, refused, failed, landed_tps: landed as f64 / elapsed, p50: percentile(0.5), p99: percentile(0.99) };
        println!("{:>6}  {:>8}  {:>8}  {:>8}  {:>8}  {:>10.0}  {:>8.1}  {:>8.1}", step.rate, step.sent, step.taken, step.refused, step.failed, step.landed_tps, step.p50, step.p99);
        let saturated = step.taken < step.sent * 9 / 10;
        results.push(step);

        if saturated {
            println!("the nodes took less than nine in ten: saturated at {rate} a second");
            break;
        }
    }

    Ok(results)
}

fn nonce_of(agent: &ureq::Agent, node: &str, key: &Keypair) -> CliResult<u64> {
    Ok(ok(agent.get(format!("{node}/accounts/{}", key.public().address())).call())?["nonce"].as_u64().unwrap_or(0))
}

/// How many transactions a block holds. A full one's JSON passes 10 MB, ureq's usual limit.
fn block_txs(agent: &ureq::Agent, node: &str, height: u64) -> u64 {
    agent
        .get(format!("{node}/blocks/{height}"))
        .call()
        .ok()
        .and_then(|mut answer| answer.body_mut().with_config().limit(64 << 20).read_to_vec().ok())
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|block| block["tx_count"].as_u64())
        .unwrap_or(0)
}

fn submit(agent: &ureq::Agent, node: &str, tx: &SignedTx) -> Result<(), String> {
    let body = json!({ "tx": to_hex(&wincode::serialize(tx).map_err(|err| err.to_string())?) }).to_string();
    let mut answer = agent.post(format!("{node}/txs")).header("content-type", "application/json").send(body).map_err(|err| err.to_string())?;
    // Read whole, so the connection goes back to the pool for the next request.
    let _ = answer.body_mut().read_to_vec();

    match answer.status().as_u16() {
        200..300 => Ok(()),
        status => Err(format!("{status}")),
    }
}

/// Sends the transactions spread over the nodes at about rate a second from threads at once:
/// how many were taken, refused and failed, and each submission's time in milliseconds.
fn send_at(agent: &ureq::Agent, nodes: &[String], txs: Vec<SignedTx>, rate: u32, threads: usize) -> (u64, u64, u64, Vec<f64>) {
    let queue = Arc::new(std::sync::Mutex::new(txs.into_iter().enumerate().collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>()));
    let start = Instant::now();
    let gap = Duration::from_secs_f64(1.0 / f64::from(rate.max(1)));
    let workers: Vec<_> = (0..threads.max(1))
        .map(|_| {
            let (queue, agent, nodes) = (queue.clone(), agent.clone(), nodes.to_vec());

            std::thread::spawn(move || {
                let (mut taken, mut refused, mut failed, mut latencies) = (0, 0, 0, Vec::new());

                loop {
                    let Some((index, tx)) = queue.lock().expect("never poisoned").pop() else {
                        break;
                    };
                    // Each transaction has its moment, so the rate holds however many threads.
                    let due = start + gap * index as u32;

                    if let Some(wait) = due.checked_duration_since(Instant::now()) {
                        std::thread::sleep(wait);
                    }

                    let sent = Instant::now();

                    match submit(&agent, &nodes[index % nodes.len()], &tx) {
                        Ok(()) => taken += 1,
                        Err(status) if status.len() == 3 => refused += 1,
                        Err(_) => failed += 1,
                    }

                    latencies.push(ms(sent));
                }

                (taken, refused, failed, latencies)
            })
        })
        .collect();

    workers.into_iter().filter_map(|worker| worker.join().ok()).fold((0, 0, 0, Vec::new()), |(t, r, f, mut l), (t2, r2, f2, l2)| {
        l.extend(l2);
        (t + t2, r + r2, f + f2, l)
    })
}
