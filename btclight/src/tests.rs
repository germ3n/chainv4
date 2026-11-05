extern crate std;

use {
    crate::*,
    alloc::{collections::BTreeMap, vec, vec::Vec},
    sha2::{Digest, Sha256},
};

struct Native;

impl Crypto for Native {
    fn sha256d(&self, data: &[u8]) -> [u8; 32] {
        Sha256::digest(Sha256::digest(data)).into()
    }
}

const C: Native = Native;

fn hex(text: &str) -> Vec<u8> {
    let text = text.trim_start_matches("0x");

    (0..text.len()).step_by(2).map(|at| u8::from_str_radix(&text[at..at + 2], 16).unwrap()).collect()
}

/// Bitcoin's genesis header and block 1's, and block 1's one transaction, its coinbase.
const GENESIS: &str = "0100000000000000000000000000000000000000000000000000000000000000000000003ba3edfd7a7b12b27ac72c3e67768f617fc81bc3888a51323a9fb8aa4b1e5e4a29ab5f49ffff001d1dac2b7c";
const BLOCK_1: &str = "010000006fe28c0ab6f1b372c1a6a246ae63f74f931e8365e15a089c68d6190000000000982051fd1e4ba744bbbe680e1fee14677ba1a3c3540bf7b1cdb606e857233e0e61bc6649ffff001d01e36299";
const COINBASE_1: &str = "01000000010000000000000000000000000000000000000000000000000000000000000000ffffffff0704ffff001d0104ffffffff0100f2052a0100000043410496b538e853519c726a2c91e61ec11600ae1390813a627c66fb8be7947be63c52da7589379515d4e0a604f8141781e62294721166bf621e73a82cbf2342c858eeac00000000";

const REGTEST: u32 = 0x207f_ffff;

/// A regtest header on a parent with a Merkle root, mined: a nonce until its hash meets the
/// difficulty it names.
fn mined(prev: [u8; 32], merkle: [u8; 32], time: u32, bits: u32) -> Header {
    let target = target(bits).unwrap();

    for nonce in 0u32.. {
        let header = Header { version: 4, prev, merkle, time, bits, nonce };

        if U256::from_le_bytes(C.sha256d(&header.encode())) <= target {
            return header;
        }
    }

    unreachable!("some nonce meets regtest's difficulty")
}

fn hash(header: &Header) -> [u8; 32] {
    C.sha256d(&header.encode())
}

#[test]
fn compact_difficulty_reads_and_writes_as_bitcoin_rounds_it() {
    let limit = target(0x1d00_ffff).unwrap();
    assert_eq!(limit, U256::from(0xffffu64) << 208u32, "mainnet's limit: 0xffff, 26 bytes up");
    assert_eq!(compact(limit), 0x1d00_ffff);
    assert_eq!(target(0x207f_ffff).unwrap(), U256::from(0x7f_ffffu64) << 232u32);
    assert_eq!(compact(target(0x207f_ffff).unwrap()), 0x207f_ffff);

    // Bitcoin Core's arith_uint256 SetCompact vectors.
    assert_eq!(target(0x0112_3456).unwrap(), U256::from(0x12u64));
    assert_eq!(target(0x0212_3456).unwrap(), U256::from(0x1234u64));
    assert_eq!(target(0x0512_3456).unwrap(), U256::from(0x12_3456_0000u64));
    assert_eq!(target(0x0000_0000).unwrap(), U256::ZERO);
    assert_eq!(compact(U256::from(0x1234u64)), 0x0212_3400);
    assert_eq!(compact(U256::from(0x80u64)), 0x0200_8000, "the top bit would read as negative: a byte more");

    assert_eq!(target(0x0492_3456), None, "negative");
    assert_eq!(target(0xff12_3456), None, "past 256 bits");
    assert_eq!(target(0x2101_0000), None, "a byte past 256 bits");
}

#[test]
fn work_is_about_two_to_the_256_over_the_target() {
    assert_eq!(work(target(0x1d00_ffff).unwrap()), U256::from(0x1_0001_0001u64), "a block at mainnet's limit");
    assert_eq!(work(target(REGTEST).unwrap()), U256::from(2u64));
    assert_eq!(work(U256::MAX >> 1u32), U256::from(2u64));
}

#[test]
fn retargets_follow_bitcoin_core_s_own_vectors() {
    let limit = Network::Main.limit();

    // pow_tests.cpp: get_next_work, its pow limit, and both clamps.
    assert_eq!(retarget(0x1d00ffff, 1261130161, 1262152739, limit), Some(0x1d00d86a), "blocks 30240 to 32255");
    assert_eq!(retarget(0x1d00ffff, 1231006505, 1233061996, limit), Some(0x1d00ffff), "never easier than the limit");
    assert_eq!(retarget(0x1c05a3f4, 1279008237, 1279297671, limit), Some(0x1c0168fd), "at most four times harder");
    assert_eq!(retarget(0x1c387f6f, 1263163443, 1269211443, limit), Some(0x1d00e1fd), "at most four times easier");
    assert_eq!(retarget(0x0492_3456, 0, TIMESPAN as u32, limit), None, "a difficulty Bitcoin refuses");
}

#[test]
fn networks_go_by_their_names() {
    for network in Network::ALL {
        assert_eq!(Network::from_name(network.name()), Some(network));
    }

    assert_eq!(Network::from_name("mainnet"), None);
    assert_eq!((Network::Main.limit(), Network::Regtest.limit(), Network::Test.limit()), (0x1d00_ffff, REGTEST, REGTEST));
}

#[test]
fn a_header_reads_and_writes_its_80_bytes() {
    let genesis = hex(GENESIS);
    let header = Header::read(&genesis).unwrap();

    assert_eq!((header.version, header.time, header.bits, header.nonce), (1, 1231006505, 0x1d00_ffff, 2083236893));
    assert_eq!(header.prev, [0; 32]);
    assert_eq!(header.encode().to_vec(), genesis);
    assert_eq!(Header::read(&genesis[..79]), None);

    let mut id = C.sha256d(&genesis);
    id.reverse();
    assert_eq!(id.to_vec(), hex("000000000019d6689c085ae165831e934ff763ae46a2a6c172b3f1b60a8ce26f"), "Bitcoin's genesis block");
}

#[test]
fn the_kept_record_is_laid_out_byte_for_byte_as_the_relay_stores_it() {
    let kept = Kept { prev: [1; 32], merkle: [2; 32], height: 0x0304, work: U256::from(0x0506u64), bits: 0x0708_090a, time: 0x0b0c_0d0e, period_start: 0x0f10_1112, times: vec![7, 8], version: 0x1314_1516, nonce: 0x1718_191a };
    let bytes = kept.encode();

    assert_eq!(bytes.len(), 117 + 4 * 2 + 8);
    assert_eq!(bytes[..32], [1; 32]);
    assert_eq!(bytes[32..64], [2; 32]);
    assert_eq!(bytes[64..72], 0x0304u64.to_le_bytes());
    assert_eq!(bytes[72..74], [0x06, 0x05]);
    assert_eq!(bytes[74..104], [0; 30]);
    assert_eq!(bytes[104..108], 0x0708_090au32.to_le_bytes());
    assert_eq!(bytes[108..112], 0x0b0c_0d0eu32.to_le_bytes());
    assert_eq!(bytes[112..116], 0x0f10_1112u32.to_le_bytes());
    assert_eq!(bytes[116], 2);
    assert_eq!(bytes[117..125], [7, 0, 0, 0, 8, 0, 0, 0]);
    assert_eq!(bytes[125..129], 0x1314_1516u32.to_le_bytes(), "the version, after the times");
    assert_eq!(bytes[129..133], 0x1718_191au32.to_le_bytes(), "and the nonce");

    assert_eq!(Kept::decode(&bytes), Some(kept.clone()));
    assert_eq!(Kept::decode(&bytes[..bytes.len() - 1]), None, "short a byte of the nonce");
    assert_eq!(Kept::decode(&bytes[..125]), None, "as relays from before the version and nonce kept them");
    assert_eq!(kept.header(), Header { version: 0x1314_1516, prev: [1; 32], merkle: [2; 32], time: 0x0b0c_0d0e, bits: 0x0708_090a, nonce: 0x1718_191a });
}

#[test]
fn bitcoin_s_block_1_builds_on_its_genesis_by_main_s_rules() {
    let genesis = Header::read(&hex(GENESIS)).unwrap();
    let block_1 = Header::read(&hex(BLOCK_1)).unwrap();
    let start = Kept::start(&genesis, 0);
    let now = 1_800_000_000;

    assert_eq!((start.work, start.times.clone(), start.period_start), (U256::ZERO, vec![genesis.time], genesis.time));
    assert_eq!(start.header(), genesis, "the start writes whole again");

    let kept = start.child(&block_1, &hash(&block_1), Network::Main, now).unwrap();
    assert_eq!((kept.height, kept.work, kept.prev), (1, U256::from(0x1_0001_0001u64), hash(&genesis)));
    assert_eq!(kept.times, vec![genesis.time, block_1.time]);
    assert_eq!(kept.header(), block_1);

    let weak = Header { nonce: 0, ..block_1 };
    assert_eq!(start.child(&weak, &hash(&weak), Network::Main, now), Err(Error::NoWork));
    assert_eq!(start.child(&block_1, &hash(&block_1), Network::Main, u64::from(block_1.time) - MOST_AHEAD - 1), Err(Error::TooFarAhead), "two hours and a second ahead");
    assert!(start.child(&block_1, &hash(&block_1), Network::Main, u64::from(block_1.time) - MOST_AHEAD).is_ok(), "two hours ahead, just");
}

#[test]
fn each_network_checks_its_own_rules() {
    let start = Kept::start(&mined([0; 32], [0; 32], 1_000, REGTEST), 0);
    let now = 1_000_000;
    let child = |header: &Header, network: Network| start.child(header, &hash(header), network, now);

    let easier = mined(hash(&start.header()), [1; 32], 1_001, 0x2100_ffff);
    assert_eq!(child(&easier, Network::Test), Err(Error::TooEasy), "past even regtest's limit");
    let regtest = mined(hash(&start.header()), [1; 32], 1_001, REGTEST);
    assert_eq!(child(&regtest, Network::Main), Err(Error::TooEasy), "regtest's difficulty on main");
    assert!(child(&regtest, Network::Regtest).is_ok());

    let harder = mined(hash(&start.header()), [1; 32], 1_001, 0x207f_fffe);
    assert_eq!(child(&harder, Network::Regtest), Err(Error::WrongDifficulty), "regtest keeps one difficulty");
    assert!(child(&harder, Network::Test).is_ok(), "test checks only the work named");

    let refused = Header { bits: 0x0492_3456, ..regtest };
    assert_eq!(child(&refused, Network::Test), Err(Error::BadDifficulty));

    let early = mined(hash(&start.header()), [1; 32], 1_000, REGTEST);
    assert_eq!(child(&early, Network::Regtest), Err(Error::TooEarly), "not after the median");
}

#[test]
fn the_median_is_of_the_last_eleven_times() {
    let mut kept = Kept::start(&mined([0; 32], [0; 32], 1_000, REGTEST), 0);

    // Times out of order, as Bitcoin allows within the median.
    for time in [1_010, 1_020, 1_015, 1_030, 1_025, 1_040, 1_035, 1_050, 1_045, 1_060, 1_055] {
        let header = mined(hash(&kept.header()), [3; 32], time, REGTEST);
        kept = kept.child(&header, &hash(&header), Network::Regtest, 1_000_000).unwrap();
    }

    assert_eq!(kept.times.len(), MEDIAN_SPAN, "the start's own time has dropped out");
    assert_eq!(kept.times[0], 1_010);
    assert_eq!(kept.median_time(), 1_035);
    assert_eq!(kept.period_start, 1_000, "a period starts every 2016 blocks");

    let at_median = mined(hash(&kept.header()), [3; 32], 1_035, REGTEST);
    assert_eq!(kept.child(&at_median, &hash(&at_median), Network::Regtest, 1_000_000), Err(Error::TooEarly));
    let after = mined(hash(&kept.header()), [3; 32], 1_036, REGTEST);
    assert!(kept.child(&after, &hash(&after), Network::Regtest, 1_000_000).is_ok());
}

/// A relay's headers and main chain in memory, as the node keeps them.
#[derive(Default)]
struct Memory {
    kept: BTreeMap<[u8; 32], Kept>,
    main: BTreeMap<u64, [u8; 32]>,
    tip: [u8; 32],
}

impl MainChain for Memory {
    fn link(&self, hash: &[u8; 32]) -> Option<(u64, [u8; 32])> {
        self.kept.get(hash).map(|kept| (kept.height, kept.prev))
    }

    fn main_at(&self, height: u64) -> Option<[u8; 32]> {
        self.main.get(&height).copied()
    }

    fn set_main(&mut self, height: u64, hash: &[u8; 32]) {
        self.main.insert(height, *hash);
    }

    fn unset_main(&mut self, height: u64) {
        self.main.remove(&height);
    }
}

impl Memory {
    fn started(header: &Header) -> Memory {
        let mut memory = Memory { tip: hash(header), ..Memory::default() };
        memory.kept.insert(hash(header), Kept::start(header, 0));
        memory.main.insert(0, hash(header));

        memory
    }

    fn add(&mut self, header: &Header) {
        let kept = self.kept[&header.prev].child(header, &hash(header), Network::Regtest, 1_000_000).unwrap();
        let best = self.kept[&self.tip].clone();

        if kept.work > best.work {
            follow(self, &hash(header), kept.height, kept.prev, best.height);
            self.tip = hash(header);
        }

        self.kept.insert(hash(header), kept);
    }
}

#[test]
fn the_main_chain_follows_the_most_work() {
    let start = mined([0; 32], [0; 32], 1_000, REGTEST);
    let mut chain = Memory::started(&start);
    let a1 = mined(hash(&start), [1; 32], 1_001, REGTEST);
    let a2 = mined(hash(&a1), [1; 32], 1_002, REGTEST);
    let a3 = mined(hash(&a2), [1; 32], 1_003, REGTEST);

    for header in [a1, a2, a3] {
        chain.add(&header);
    }

    assert_eq!(chain.main.values().copied().collect::<Vec<_>>(), [hash(&start), hash(&a1), hash(&a2), hash(&a3)]);

    // A branch from a1 as long as a: the same work, so the first one seen stays.
    let b2 = mined(hash(&a1), [2; 32], 1_002, REGTEST);
    let b3 = mined(hash(&b2), [2; 32], 1_003, REGTEST);
    chain.add(&b2);
    chain.add(&b3);
    assert_eq!(chain.tip, hash(&a3));

    // One further: it is the main chain now, down to a1, where they meet.
    let b4 = mined(hash(&b3), [2; 32], 1_004, REGTEST);
    chain.add(&b4);
    assert_eq!(chain.tip, hash(&b4));
    assert_eq!(chain.main.values().copied().collect::<Vec<_>>(), [hash(&start), hash(&a1), hash(&b2), hash(&b3), hash(&b4)]);

    // Back to a, made longer still: from a3 up.
    let mut prev = hash(&a3);
    let mut a = Vec::new();

    for time in 1_004..1_007 {
        let header = mined(prev, [1; 32], time, REGTEST);
        prev = hash(&header);
        a.push(header);
        chain.add(&header);
    }

    assert_eq!(chain.tip, hash(&a[2]));
    assert_eq!(chain.main.len(), 7);
    assert_eq!(chain.main[&2], hash(&a2));
    assert_eq!(chain.main[&4], hash(&a[0]));
}

#[test]
fn a_main_chain_moving_to_a_shorter_branch_with_more_work_drops_the_heights_above() {
    // Test's rules let a harder header carry more work on its own than two easy ones.
    let start = mined([0; 32], [0; 32], 1_000, REGTEST);
    let mut memory = Memory::started(&start);
    let add = |memory: &mut Memory, header: &Header| {
        let kept = memory.kept[&header.prev].child(header, &hash(header), Network::Test, 1_000_000).unwrap();
        let best = memory.kept[&memory.tip].clone();

        if kept.work > best.work {
            follow(memory, &hash(header), kept.height, kept.prev, best.height);
            memory.tip = hash(header);
        }

        memory.kept.insert(hash(header), kept);
    };
    let a1 = mined(hash(&start), [1; 32], 1_001, REGTEST);
    let a2 = mined(hash(&a1), [1; 32], 1_002, REGTEST);
    add(&mut memory, &a1);
    add(&mut memory, &a2);
    let hard = mined(hash(&start), [2; 32], 1_001, 0x2000_ffff);
    add(&mut memory, &hard);

    assert_eq!(memory.tip, hash(&hard));
    assert_eq!(memory.main.values().copied().collect::<Vec<_>>(), [hash(&start), hash(&hard)], "height 2 is a's alone, off the main chain");
}

#[test]
fn a_coinbase_spends_nothing_and_is_never_64_bytes() {
    let coinbase = hex(COINBASE_1);

    assert!(is_coinbase(&coinbase));
    assert!(!is_coinbase(&coinbase[..40]), "too short to hold an input");
    assert!(!is_coinbase(&[0; 64]));

    let mut spends = coinbase.clone();
    spends[5] = 1;
    assert!(!is_coinbase(&spends), "an input spending something");
    let mut two = coinbase.clone();
    two[4] = 2;
    assert!(!is_coinbase(&two), "two inputs");
}

/// CVE-2017-12842: the 64 bytes of two ids hash to an inner node of a block's tree, so with a path
/// one short they pass for a transaction a level up. The coinbase's path says how tall it is.
#[test]
fn an_inner_node_does_not_pass_for_a_transaction() {
    let coinbase = hex(COINBASE_1);
    let leaves = [C.sha256d(&coinbase), [0xa1; 32], [0xa2; 32], [0xa3; 32]];
    let left = C.sha256d(&[leaves[0], leaves[1]].concat());
    let right = C.sha256d(&[leaves[2], leaves[3]].concat());
    let root = C.sha256d(&[left, right].concat());
    let coinbase_path = [leaves[1], right];

    assert!(includes(&C, &root, &leaves[2], 2, &[leaves[3], left], &coinbase, &coinbase_path));
    assert!(includes(&C, &root, &leaves[0], 0, &coinbase_path, &coinbase, &coinbase_path));
    assert!(!includes(&C, &root, &leaves[2], 3, &[leaves[3], left], &coinbase, &coinbase_path), "not at another place");
    assert!(!includes(&C, &root, &leaves[2], 6, &[leaves[3], left], &coinbase, &coinbase_path), "nor past the tree");

    assert!(leads_to(&C, &right, 1, &[left], &root), "the forged path does lead to the root");
    assert!(!includes(&C, &root, &right, 1, &[left], &coinbase, &coinbase_path), "one level short of the coinbase's");
    assert!(!includes(&C, &root, &right, 1, &[left], &coinbase, &[right]), "nor does a shorter coinbase path help");
    assert!(!includes(&C, &root, &right, 1, &[left], &[leaves[0], leaves[1]].concat(), &[right]), "an inner node is no coinbase");
    assert!(!includes(&C, &root, &[0; 32], 0, &[[0; 32]; MAX_PATH + 1], &coinbase, &[[0; 32]; MAX_PATH + 1]), "no tree is that tall");
}

#[test]
fn block_1_s_coinbase_is_its_merkle_root() {
    let block_1 = Header::read(&hex(BLOCK_1)).unwrap();
    let coinbase = hex(COINBASE_1);

    assert!(includes(&C, &block_1.merkle, &C.sha256d(&coinbase), 0, &[], &coinbase, &[]));
    assert!(!includes(&C, &block_1.merkle, &C.sha256d(&coinbase), 1, &[], &coinbase, &[]));
}
