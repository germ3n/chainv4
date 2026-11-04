use {
    super::*,
    crate::consensus::{Consensus, Miner, PowConfig, ProofOfWork},
    crate::mining::worker::{Devices, Worker, WorkerConfig},
    crate::structs::u256::u256,
    std::sync::mpsc,
    std::time::Instant,
};

const TOKEN: &[u8] = b"0123456789abcdef0123456789abcdef";

/// A header on a chain hashed with the build's default.
fn header(height: u64, timestamp: u64, difficulty: u128) -> BlockHeader {
    let chain = HashType::default_chain();

    BlockHeader {
        parent: Hash::from_bytes(chain, &[height as u8; 32]).unwrap(),
        height,
        timestamp,
        producer: Address::Ed25519([1; 32]),
        difficulty,
        tx_root: Hash::zero(chain),
        proof_root: Hash::zero(chain),
        pool_root: Hash::zero(chain),
        receipts_root: Hash::zero(chain),
        state_root: Hash::zero(chain),
        gas_used: 0,
        base_price: u256::ZERO,
    }
}

fn port() -> MiningPort {
    MiningPort { listen: "127.0.0.1:0".parse().unwrap(), token: TOKEN.to_vec(), protocol: "chainv4".to_string(), #[cfg(feature = "pool")] pool: None, #[cfg(feature = "updates")] releases: None, solo: false, limits: PortLimits::DEFAULT }
}

/// A pool on a port of its own, its seals coming out of the channel.
fn pool() -> (Pool, mpsc::Receiver<(Hash, Seal)>) {
    let (sender, seals) = mpsc::channel();
    let sender = Mutex::new(sender);
    let found: Found = Arc::new(move |id, seal| {
        let _ = State::lock(&sender).send((id, seal));
    });

    (Pool::new(Address::Ed25519([1; 32]), HashType::default_chain(), port(), found).unwrap(), seals)
}

/// A worker's side of a connection, past the handshake.
fn raw(pool: &Pool, name: &str) -> (Sender, Receiver) {
    let stream = TcpStream::connect(pool.listening()).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();

    protocol::connect(stream, TOKEN, "chainv4", name, None).unwrap()
}

/// The next message of a type, past pings.
fn next(receiver: &mut Receiver, kind: &str) -> Value {
    loop {
        let message = receiver.receive().unwrap();

        if message["type"] == kind {
            return message;
        }
    }
}

/// Waits for the pool's report to show what is asked.
fn until(pool: &Pool, what: impl Fn(&MiningReport) -> bool) -> MiningReport {
    let deadline = Instant::now() + Duration::from_secs(10);

    loop {
        let report = pool.mining().unwrap();

        if what(&report) {
            return report;
        }

        assert!(Instant::now() < deadline, "the report never showed it: {report:?}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn rules() -> ProofOfWork {
    ProofOfWork::new(
        PowConfig {
            hash: HashType::default_chain(),
            target_block_ms: 1_000,
            initial_difficulty: 1_000,
            min_difficulty: 1,
            tuning: None,
            window: None,
        },
        None,
    )
}

/// The child of a parent at the difficulty the rules give it.
fn child(parent: &BlockHeader, timestamp: u64) -> BlockHeader {
    let mut child = header(parent.height + 1, timestamp, 0);
    child.parent = parent.id();
    child.difficulty = rules().difficulty(std::slice::from_ref(parent), timestamp);

    child
}

#[test]
fn a_worker_on_another_thread_seals_blocks_the_rules_accept() {
    let (mut pool, seals) = pool();
    let worker = Worker::start(WorkerConfig {
        node: pool.listening().to_string(),
        token: TOKEN.to_vec(),
        name: "rig 1".to_string(),
        protocol: "chainv4".to_string(),
        devices: Devices::Cpu(2),
        proxy: None,
        payout: None,
        claims: None,
        solo: false,
        insecure_open_port: false,
        ca_cert: None,
    })
    .unwrap();
    let parent = header(5, 10_000, 500);

    until(&pool, |report| report.workers.len() == 1);

    for timestamp in [10_100, 10_200] {
        let child = child(&parent, timestamp);

        assert_eq!(pool.start_at(&parent, &[]), 10_001);
        assert_eq!(pool.seal(&child), None);

        let (id, seal) = seals.recv_timeout(Duration::from_secs(30)).unwrap();
        let Seal::Work { nonce } = seal else { panic!("not a work seal") };

        assert_eq!(id, child.id());
        assert!(nonce >= FIRST_RANGE, "a worker's nonce is in its range, above the node's own");
        assert_eq!(rules().check(&child, &seal, std::slice::from_ref(&parent), &[], timestamp), Ok(()));
    }

    let report = until(&pool, |report| report.workers.first().is_some_and(|worker| worker.found == 2 && !worker.devices.is_empty()));
    let worker_report = &report.workers[0];

    assert_eq!((worker_report.name.as_str(), worker_report.refused), ("rig 1", 0));
    assert_eq!(worker_report.devices[0].name, "CPU, 2 threads");
    assert_eq!(report.listening, Some(pool.listening().to_string()));
    assert!(report.devices.is_empty(), "the pool has no devices of its own");
    assert_eq!(worker.devices().len(), 1);

    drop(worker);
    until(&pool, |report| report.workers.is_empty());
}

#[test]
fn a_worker_without_the_token_gets_no_work() {
    let (mut pool, _seals) = pool();
    pool.seal(&header(1, 1, 1_000));
    let stream = TcpStream::connect(pool.listening()).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();

    assert_eq!(protocol::connect(stream, b"a guess at the token, long enough", "chainv4", "thief", None).err().as_deref(), Some("the node refused the token"));
    assert!(pool.mining().unwrap().workers.is_empty());
}

#[test]
fn a_worker_of_another_protocol_name_is_told_apart() {
    let (pool, _seals) = pool();
    let stream = TcpStream::connect(pool.listening()).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();

    assert_eq!(
        protocol::connect(stream, TOKEN, "otherchain", "stray", None).err().as_deref(),
        Some("a node speaking \"chainv4-mining/2\", not otherchain-mining/2")
    );
    assert!(pool.mining().unwrap().workers.is_empty());
}

#[test]
fn every_worker_gets_a_range_of_its_own_and_new_work_with_each_header() {
    let (mut pool, _seals) = pool();
    let (_to_first, mut first) = raw(&pool, "first");
    let (_to_second, mut second) = raw(&pool, "second");
    let one = header(1, 1, 1_000);
    pool.seal(&one);

    let (a, b) = (next(&mut first, "work"), next(&mut second, "work"));
    let starts = [a["start"].as_u64().unwrap(), b["start"].as_u64().unwrap()];

    assert_eq!(a["id"], crate::structs::hex::to_hex(&one.id().to_bytes()));
    assert_eq!((a["difficulty"].as_str(), a["span"].as_u64()), (Some("1000"), Some(SPAN)));
    assert_eq!(a["hash"], HashType::default_chain().name());
    assert_eq!(starts[0].abs_diff(starts[1]), SPAN, "{starts:?}");
    assert!(starts.iter().all(|start| *start >= FIRST_RANGE));

    let two = header(2, 2, 2_000);
    pool.seal(&two);
    let again = next(&mut first, "work");

    assert_eq!(again["id"], crate::structs::hex::to_hex(&two.id().to_bytes()));
    assert_eq!(again["job"].as_u64(), a["job"].as_u64().map(|job| job + 1));
    assert_eq!(again["start"].as_u64(), Some(starts[0]), "a worker keeps its range");
}

#[test]
fn nonces_that_miss_or_come_late_are_refused_and_counted() {
    let (mut pool, seals) = pool();
    let (mut to_node, mut from_node) = raw(&pool, "sloppy");
    // So hard no nonce meets it.
    pool.seal(&header(1, 1, u128::MAX));
    let job = next(&mut from_node, "work")["job"].as_u64().unwrap();

    to_node.send(&json!({ "type": "found", "job": job, "nonce": 1 })).unwrap();
    let short = next(&mut from_node, "verdict");
    to_node.send(&json!({ "type": "found", "job": job - 1, "nonce": 1 })).unwrap();
    let stale = next(&mut from_node, "verdict");

    assert_eq!((short["took"].clone(), short["why"].clone()), (json!(false), json!("it does not meet the difficulty")));
    assert_eq!(stale["took"], false);
    assert!(seals.try_recv().is_err());
    until(&pool, |report| report.workers.first().is_some_and(|worker| worker.refused == 2 && worker.found == 0));
}

#[test]
fn a_workers_rates_show_in_the_report_and_add_up() {
    let (pool, _seals) = pool();
    let (mut to_node, _from_node) = raw(&pool, "rig");

    to_node
        .send(&json!({ "type": "rate", "devices": [{ "name": "GPU 0", "rate": 200e6 }, { "name": "GPU 1\u{7}", "rate": -5.0 }] }))
        .unwrap();

    let report = until(&pool, |report| report.workers.first().is_some_and(|worker| worker.devices.len() == 2));

    assert_eq!(report.workers[0].devices[1], DeviceReport { name: "GPU 1".to_string(), rate: 0.0 }, "names are cleaned and rates kept sane");
    assert_eq!(report.rate(), 200e6);
}

#[test]
fn the_nodes_own_miner_mines_beside_the_workers_and_ends_their_work() {
    let (pool, seals) = pool();
    let local = Miner::new(Address::Ed25519([1; 32]), HashType::default_chain(), 1, pool.finder());
    let mut pool = pool.mining_too(Box::new(local));
    let (mut to_node, mut from_node) = raw(&pool, "late");
    let parent = header(5, 10_000, 500);
    let child = child(&parent, 10_100);
    pool.seal(&child);
    // The pool's first template is job 1. The worker may never hear of it: the node's own miner
    // can find the seal before the pool hands the job out, and a found job is handed to no one.
    let job = 1;

    let (id, seal) = seals.recv_timeout(Duration::from_secs(30)).unwrap();
    let Seal::Work { nonce } = seal else { panic!("not a work seal") };

    assert_eq!(id, child.id());
    assert!(nonce < FIRST_RANGE, "the node's own nonces start at 0");
    assert_eq!(pool.mining().unwrap().devices[0].name, "CPU, 1 thread");

    // The worker's copy of the same nonce comes after the node found it.
    to_node.send(&json!({ "type": "found", "job": job, "nonce": nonce })).unwrap();

    assert_eq!(next(&mut from_node, "verdict")["took"], false);
}

#[test]
fn dropping_the_pool_closes_its_port_and_its_workers() {
    let (pool, _seals) = pool();
    let listening = pool.listening();
    let (_to_node, mut from_node) = raw(&pool, "rig");
    until(&pool, |report| report.workers.len() == 1);
    drop(pool);

    // Past the pings sent before, the connection closed.
    assert!((0..10).any(|_| from_node.receive().is_err()), "the worker's connection closed");
    TcpListener::bind(listening).expect("the port is free again");
}

/// A pool that runs its books, in memory, with an easy floor for shares.
#[cfg(feature = "pool")]
fn books(fee: u32) -> (Pool, mpsc::Receiver<(Hash, Seal)>) {
    let (sender, seals) = mpsc::channel();
    let sender = Mutex::new(sender);
    let found: Found = Arc::new(move |id, seal| {
        let _ = State::lock(&sender).send((id, seal));
    });
    let port = MiningPort { pool: Some(super::super::shares::PoolConfig { fee, min_share: 16, ..Default::default() }), ..port() };

    (Pool::new(Address::Ed25519([1; 32]), HashType::default_chain(), port, found).unwrap(), seals)
}

/// A pool worker's side of a connection, past the handshake and the pool's terms.
#[cfg(feature = "pool")]
fn paid(pool: &Pool, name: &str, payout: Address) -> (Sender, Receiver) {
    let stream = TcpStream::connect(pool.listening()).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let (sender, mut receiver) = protocol::connect(stream, TOKEN, "chainv4", name, Some(&payout)).unwrap();
    assert_eq!(next(&mut receiver, "pool")["window"], 2);

    (sender, receiver)
}

/// The first nonces from start whose work hash for id meets a difficulty.
#[cfg(feature = "pool")]
fn nonces(id: &Hash, difficulty: u128, start: u64, count: usize) -> Vec<u64> {
    (start..).filter(|nonce| meets(work_hash(HashType::default_chain(), id, *nonce), difficulty)).take(count).collect()
}

#[cfg(feature = "pool")]
#[test]
fn a_pool_counts_each_workers_shares_once_and_its_blocks_commit_to_their_split() {
    use crate::structs::block::PAYOUT_PARTS;

    let (mut pool, seals) = books(10_000_000);
    let (alice, bob) = (Address::Ed25519([8; 32]), Address::Ed25519([9; 32]));
    let (mut to_alice, mut from_alice) = paid(&pool, "alice's rig", alice);
    let (mut to_bob, mut from_bob) = paid(&pool, "bob's rig", bob);
    until(&pool, |report| report.workers.len() == 2);
    let pending = |_: &Hash, _: u64| crate::consensus::PoolSettled::Pending;
    assert_eq!(pool.pool(&pending), None, "no shares yet, nothing earned: the producer keeps it all");

    // A block far too hard to find here: every nonce sent is a share at most.
    let hard = header(1, 1, 1 << 100);
    pool.seal(&hard);
    let (work_alice, work_bob) = (next(&mut from_alice, "work"), next(&mut from_bob, "work"));
    assert_eq!(work_alice["share"], "16", "nothing said of its hashrate yet: the pool's floor");
    let job = work_alice["job"].as_u64().unwrap();
    let id = hard.id();

    let verdict = |to: &mut Sender, from: &mut Receiver, nonce: u64| {
        to.send(&json!({ "type": "found", "job": job, "nonce": nonce })).unwrap();

        next(from, "verdict")
    };
    let alices = nonces(&id, 16, work_alice["start"].as_u64().unwrap(), 1);
    let bobs = nonces(&id, 16, work_bob["start"].as_u64().unwrap(), 3);

    assert_eq!(verdict(&mut to_alice, &mut from_alice, alices[0])["kind"], "share");
    let twice = verdict(&mut to_alice, &mut from_alice, alices[0]);
    assert_eq!((twice["took"].as_bool(), twice["why"].as_str()), (Some(false), Some("it was sent already")));
    let short = (0..).find(|nonce| !meets(work_hash(HashType::default_chain(), &id, *nonce), 16)).unwrap();
    assert_eq!(verdict(&mut to_alice, &mut from_alice, short)["why"], "it does not meet the difficulty");

    for nonce in &bobs {
        assert_eq!(verdict(&mut to_bob, &mut from_bob, *nonce)["kind"], "share");
    }

    // A quarter and three quarters of the 99% a 1% fee leaves, in address order, to the escrow.
    let commitment = pool.pool(&pending).unwrap();
    assert_eq!((commitment.parts, commitment.count), (990_000_000, 0), "nothing credited yet: an empty root");
    let split = vec![Payout { to: alice, parts: 247_500_000 }, Payout { to: bob, parts: 742_500_000 }];
    assert!(commitment.parts < PAYOUT_PARTS);
    let report = until(&pool, |report| report.workers.iter().map(|worker| worker.shares).sum::<u64>() == 4);
    assert_eq!(report.pool.as_ref().unwrap().miners.iter().map(|miner| (miner.to, miner.shares)).collect::<Vec<_>>(), vec![(bob, 3), (alice, 1)]);

    // A block bob finds goes out with the split given for it, and into the books.
    let easy = header(2, 2, 16);
    pool.seal(&easy);
    let work = next(&mut from_bob, "work");
    let nonce = nonces(&easy.id(), 16, work["start"].as_u64().unwrap(), 1)[0];
    to_bob.send(&json!({ "type": "found", "job": work["job"], "nonce": nonce })).unwrap();
    assert_eq!(next(&mut from_bob, "verdict")["kind"], "block");
    assert_eq!(seals.recv_timeout(Duration::from_secs(10)).unwrap().0, easy.id());

    let report = pool.mining().unwrap().pool.unwrap();
    assert_eq!((report.blocks[0].height, report.blocks[0].id, &report.blocks[0].payouts, report.blocks[0].credited), (2, easy.id(), &split, None));
    assert_eq!(report.claims[0].committed, vec![(2, easy.id())], "the block committed to the newest root");

    // Once the chain says it is final, having shared out 1,000, its miners are credited, and the
    // next block commits to a root that holds them.
    let commitment = pool.pool(&|id: &Hash, _| if *id == easy.id() { crate::consensus::PoolSettled::Final(u256::new(1_000)) } else { crate::consensus::PoolSettled::Pending }).unwrap();
    let report = pool.mining().unwrap().pool.unwrap();
    assert_eq!(commitment.count, 2);
    assert_eq!(report.claims[0].root, commitment.root);
    assert_eq!(report.miners.iter().map(|miner| (miner.to, miner.earned)).collect::<Vec<_>>(), vec![(bob, u256::new(742)), (alice, u256::new(247))]);

    // The block after, bob's again, commits to that root, and each worker's standing then
    // carries its own claim under it, to keep, with the height the pool's blocks settled to.
    let after = header(3, 3, 16);
    pool.seal(&after);
    let work = next(&mut from_bob, "work");
    let nonce = nonces(&after.id(), 16, work["start"].as_u64().unwrap(), 1)[0];
    to_bob.send(&json!({ "type": "found", "job": work["job"], "nonce": nonce })).unwrap();
    assert_eq!(next(&mut from_bob, "verdict")["kind"], "block");
    let range = State::lock(&pool.state.workers).iter().find(|(_, worker)| worker.name == "bob's rig").map(|(range, _)| *range).unwrap();
    let standing = pool.state.standing(range).unwrap();
    let claim = crate::mining::claims::claim_from_json(&standing["claim"], None).unwrap();
    let root = crate::registry::pools::PoolRoot { pool: Address::Ed25519([1; 32]), root: commitment.root, count: 2 };

    assert_eq!((claim.to, claim.earned, claim.height, claim.block), (bob, u256::new(742), 3, after.id()));
    assert!(claim.holds(&root, HashType::default_chain()), "it holds under the root on chain");
    assert_eq!(standing["settled"], 2, "block 2 was credited");
}

#[cfg(feature = "pool")]
#[test]
fn a_pool_turns_away_a_worker_with_nowhere_to_pay() {
    let (pool, _seals) = books(0);
    let (_sender, mut receiver) = raw(&pool, "anonymous");

    assert_eq!(next(&mut receiver, "refused")["why"], "this node runs a pool: give the address to pay, --payout ADDRESS");
    assert!(pool.mining().unwrap().workers.is_empty());
}

#[cfg(feature = "pool")]
#[test]
fn a_worker_mining_for_a_pool_sends_share_after_share_on_one_job() {
    let (mut pool, _seals) = books(0);
    let payout = Address::Ed25519([8; 32]);
    let worker = Worker::start(WorkerConfig {
        node: pool.listening().to_string(),
        token: TOKEN.to_vec(),
        name: "rig".to_string(),
        protocol: "chainv4".to_string(),
        devices: Devices::Cpu(1),
        proxy: None,
        payout: Some(payout),
        claims: None,
        solo: false,
        insecure_open_port: false,
        ca_cert: None,
    })
    .unwrap();
    until(&pool, |report| report.workers.len() == 1);
    pool.seal(&header(1, 1, 1 << 100));

    // One job, and the search goes on past each share.
    let report = until(&pool, |report| report.workers[0].shares >= 3);
    assert_eq!((report.workers[0].payout, report.workers[0].found), (Some(payout), 0));
    let commitment = pool.pool(&|_, _| crate::consensus::PoolSettled::Pending).unwrap();
    assert_eq!(commitment.parts, crate::structs::block::PAYOUT_PARTS, "no fee: all of it to the escrow");

    drop(worker);
}

/// A solo worker's side of a connection, past the handshake, and the first message after it.
fn alone(pool: &Pool, payout: Option<Address>) -> (Sender, Receiver, Value) {
    let stream = TcpStream::connect(pool.listening()).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let (sender, mut receiver) = protocol::connect_as(stream, TOKEN, "chainv4", &protocol::Hello { name: "alone".to_string(), payout, solo: true }).unwrap();
    let first = receiver.receive().unwrap();

    (sender, receiver, first)
}

#[test]
fn a_solo_worker_mines_its_own_block_and_its_seal_ends_the_job_for_everyone() {
    let (sender, seals) = mpsc::channel();
    let sender = Mutex::new(sender);
    let found: Found = Arc::new(move |id, seal| {
        let _ = State::lock(&sender).send((id, seal));
    });
    let mut pool = Pool::new(Address::Ed25519([1; 32]), HashType::default_chain(), MiningPort { solo: true, ..port() }, found).unwrap();
    let payout = Address::Ed25519([4; 32]);
    let (mut solo_sender, mut solo, first) = alone(&pool, Some(payout));
    let (_, mut other) = raw(&pool, "for the node");

    assert_eq!(first["type"], "solo");
    until(&pool, |report| report.workers.len() == 2);
    assert_eq!(pool.solo(), vec![payout], "a template wanted for its payout");

    // The node's block, and the solo worker's, which pays it; difficulty 1, so any nonce meets it.
    let main = header(1, 1, 1);
    let mine = BlockHeader { producer: payout, ..header(1, 1, 1) };
    assert_eq!(pool.seal_with_solo(&main, std::slice::from_ref(&mine)), None);
    let work = next(&mut solo, "work");
    let theirs = next(&mut other, "work");

    assert_eq!(work["id"], crate::structs::hex::to_hex(&mine.id().to_bytes()), "its own block");
    assert_eq!(theirs["id"], crate::structs::hex::to_hex(&main.id().to_bytes()));
    assert!(work["share"].is_null(), "no shares mining alone");

    solo_sender.send(&json!({ "type": "found", "job": work["job"], "nonce": work["start"] })).unwrap();
    let verdict = next(&mut solo, "verdict");

    assert_eq!((verdict["took"].as_bool(), verdict["kind"].as_str()), (Some(true), Some("block")));
    assert_eq!(seals.recv_timeout(Duration::from_secs(10)).unwrap().0, mine.id(), "the solo block sealed");

    // The node's workers are too late now: the job is over.
    let report = pool.mining().unwrap();
    assert!(report.solo && report.workers.iter().any(|worker| worker.solo && worker.payout == Some(payout) && worker.found == 1));
    assert_eq!(State::lock(&pool.state.job).work, None);
}

#[test]
fn a_solo_worker_is_turned_away_by_a_port_that_takes_none_or_without_a_payout() {
    let (pool, _seals) = pool();
    let (_, _, refused) = alone(&pool, Some(Address::Ed25519([4; 32])));

    assert_eq!(refused["type"], "refused");
    assert!(refused["why"].as_str().unwrap().contains("--mining-solo"), "{refused}");

    let found: Found = Arc::new(|_, _| {});
    let solo = Pool::new(Address::Ed25519([1; 32]), HashType::default_chain(), MiningPort { solo: true, ..port() }, found).unwrap();
    let (_, _, refused) = alone(&solo, None);

    assert!(refused["why"].as_str().unwrap().contains("--payout"), "{refused}");
    assert!(solo.solo().is_empty());
}

#[test]
fn a_port_tuned_to_one_payout_mining_alone_refuses_a_second() {
    let found: Found = Arc::new(|_, _| {});
    let limits = PortLimits { solo: 1, ..PortLimits::DEFAULT };
    let pool = Pool::new(Address::Ed25519([1; 32]), HashType::default_chain(), MiningPort { solo: true, limits, ..port() }, found).unwrap();
    let (_first_sender, _first, first) = alone(&pool, Some(Address::Ed25519([4; 32])));
    let (_again_sender, _again, again) = alone(&pool, Some(Address::Ed25519([4; 32])));
    let (_, _, second) = alone(&pool, Some(Address::Ed25519([5; 32])));

    assert_eq!((first["type"].as_str(), again["type"].as_str()), (Some("solo"), Some("solo")), "one payout, mined for twice");
    assert_eq!(second["type"], "refused");
    assert!(second["why"].as_str().unwrap().contains("for 1 payouts mining alone already"), "{second}");
}

#[test]
fn a_port_tuned_to_few_connections_turns_the_next_away() {
    let found: Found = Arc::new(|_, _| {});
    let limits = PortLimits { connections: 2, ..PortLimits::DEFAULT };
    let pool = Pool::new(Address::Ed25519([1; 32]), HashType::default_chain(), MiningPort { limits, ..port() }, found).unwrap();
    let _one = raw(&pool, "one");
    let _two = raw(&pool, "two");
    until(&pool, |report| report.workers.len() == 2);

    // The third connects at TCP, but the port closes it without a handshake.
    let stream = TcpStream::connect(pool.listening()).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    assert!(protocol::connect(stream, TOKEN, "chainv4", "three", None).is_err());
}
