//! contracts/examples/btc_relay, the Bitcoin light client as a contract: Bitcoin's own first
//! blocks and its transaction, Bitcoin Core's retarget vectors, and regtest headers mined here.

use {super::*, crate::crypto::bitcoin::sha256d, crate::structs::hex::from_hex};

/// A time after every header here, in milliseconds: the chain's block time the relay judges by.
const NOW: u64 = 1_800_000_000_000;

/// Bitcoin's genesis header and block 1's, and block 1's one transaction, its coinbase.
const GENESIS: &str = "0x0100000000000000000000000000000000000000000000000000000000000000000000003ba3edfd7a7b12b27ac72c3e67768f617fc81bc3888a51323a9fb8aa4b1e5e4a29ab5f49ffff001d1dac2b7c";
const BLOCK_1: &str = "0x010000006fe28c0ab6f1b372c1a6a246ae63f74f931e8365e15a089c68d6190000000000982051fd1e4ba744bbbe680e1fee14677ba1a3c3540bf7b1cdb606e857233e0e61bc6649ffff001d01e36299";
const COINBASE_1: &str = "0x01000000010000000000000000000000000000000000000000000000000000000000000000ffffffff0704ffff001d0104ffffffff0100f2052a0100000043410496b538e853519c726a2c91e61ec11600ae1390813a627c66fb8be7947be63c52da7589379515d4e0a604f8141781e62294721166bf621e73a82cbf2342c858eeac00000000";

fn bytes(value: &[u8]) -> TxCallArgumentType {
    TxCallArgumentType::Bytes(value.to_vec())
}

fn number(value: u64) -> TxCallArgumentType {
    TxCallArgumentType::U64(value)
}

pub(super) struct Relay {
    pub world: World,
    pub relay: Address,
}

impl Relay {
    pub fn new() -> Relay {
        let mut world = World::new();
        let relay = world.deploy(Code::Wasm(example("btc_relay")));

        Relay { world, relay }
    }

    pub fn call(&self, function: &str, args: &[TxCallArgumentType]) -> Result<Vec<TxCallArgumentType>, String> {
        self.world.call_at(DEPLOYER, self.relay, function, args, 0, NOW).result.map_err(|err| err.to_string())
    }

    fn tip(&self) -> (Vec<u8>, u64) {
        match &self.call("tip", &[]).unwrap()[..] {
            [TxCallArgumentType::Bytes(hash), TxCallArgumentType::U64(height)] => (hash.clone(), *height),
            other => panic!("{other:?}"),
        }
    }

    fn depth(&self, hash: &[u8]) -> u64 {
        match self.call("confirmations", &[bytes(hash)]).unwrap()[..] {
            [TxCallArgumentType::U64(depth)] => depth,
            ref other => panic!("{other:?}"),
        }
    }
}

/// A coinbase, without its witness, one of its own for each stamp: one input spending nothing.
pub(super) fn coinbase(stamp: u32) -> Vec<u8> {
    crate::crypto::bitcoin::tx::Transaction {
        version: 1,
        inputs: vec![crate::crypto::bitcoin::tx::TxIn { txid: [0; 32], vout: u32::MAX, script_sig: stamp.to_le_bytes().to_vec(), sequence: u32::MAX, witness: Vec::new() }],
        outputs: vec![crate::crypto::bitcoin::tx::TxOut { value: 50_0000_0000, script: vec![0x51; 20] }],
        lock_time: 0,
    }
    .base()
}

/// A regtest header on prev with a Merkle root, mined at regtest's difficulty: a nonce until its
/// hash meets it.
pub(super) fn mined(prev: &[u8; 32], merkle: &[u8; 32], time: u32, bits: u32) -> Vec<u8> {
    let mut target = [0u8; 32];
    let size = (bits >> 24) as usize;
    target[size - 3..size].copy_from_slice(&(bits & 0x7f_ffff).to_le_bytes()[..3]);
    let target = u256::from_le_bytes(target);

    for nonce in 0u32.. {
        let header = [&1u32.to_le_bytes()[..], prev, merkle, &time.to_le_bytes(), &bits.to_le_bytes(), &nonce.to_le_bytes()].concat();

        if u256::from_le_bytes(sha256d(&header)) <= target {
            return header;
        }
    }

    unreachable!("some nonce meets regtest's difficulty")
}

/// Regtest's difficulty.
pub(super) const REGTEST: u32 = 0x207f_ffff;

#[test]
fn bitcoin_s_own_first_blocks_and_transaction_check() {
    let relay = Relay::new();
    let (genesis, block_1) = (from_hex(GENESIS).unwrap(), from_hex(BLOCK_1).unwrap());
    let main = TxCallArgumentType::String("main".into());

    // Only the deployer starts it, once, on main at a difficulty period's first block.
    assert!(relay.world.call_at(Address::Ed25519([9; 32]), relay.relay, "start", &[bytes(&genesis), number(0), main.clone()], 0, NOW).result.is_err());
    assert!(relay.call("start", &[bytes(&genesis), number(5), main.clone()]).unwrap_err().contains("divisible by 2016"));
    relay.call("start", &[bytes(&genesis), number(0), main.clone()]).unwrap();
    assert!(relay.call("start", &[bytes(&genesis), number(0), main]).unwrap_err().contains("started already"));

    // Block 1, with mainnet's real work, builds on it.
    assert_eq!(relay.call("submit", &[bytes(&block_1)]).unwrap(), [TxCallArgumentType::U64(1)]);
    assert_eq!(relay.call("submit", &[bytes(&block_1)]).unwrap(), [TxCallArgumentType::U64(0)], "held already");
    assert_eq!(relay.tip(), (sha256d(&block_1).to_vec(), 1));
    assert_eq!((relay.depth(&sha256d(&genesis)), relay.depth(&sha256d(&block_1))), (2, 1));

    // Its coinbase is its one transaction: the Merkle root is its id, the path empty.
    let txid = sha256d(&from_hex(COINBASE_1).unwrap());
    let coinbase_1 = from_hex(COINBASE_1).unwrap();
    let includes = |txid: &[u8], index: u64| relay.call("includes", &[bytes(&sha256d(&block_1)), bytes(txid), number(index), bytes(&[]), bytes(&coinbase_1), bytes(&[])]).unwrap();

    assert_eq!(includes(&txid, 0), [TxCallArgumentType::Bool(true)]);
    assert_eq!(includes(&[0; 32], 0), [TxCallArgumentType::Bool(false)]);
    assert_eq!(includes(&txid, 1), [TxCallArgumentType::Bool(false)], "not at another place");

    // A header with less work than mainnet asks is refused.
    let mut weak = block_1.clone();
    weak[76..80].copy_from_slice(&0u32.to_le_bytes());

    assert!(relay.call("submit", &[bytes(&weak)]).unwrap_err().contains("without the work"));
}

#[test]
fn retargets_follow_bitcoin_core_s_own_vectors() {
    let relay = Relay::new();
    let next = |bits: u64, first: u64, last: u64| match relay.call("next_bits", &[number(bits), number(first), number(last)]).unwrap()[..] {
        [TxCallArgumentType::U64(bits)] => bits,
        ref other => panic!("{other:?}"),
    };

    // pow_tests.cpp: get_next_work, its pow limit, and both clamps.
    assert_eq!(next(0x1d00ffff, 1261130161, 1262152739), 0x1d00d86a, "blocks 30240 to 32255");
    assert_eq!(next(0x1d00ffff, 1231006505, 1233061996), 0x1d00ffff, "never easier than the limit");
    assert_eq!(next(0x1c05a3f4, 1279008237, 1279297671), 0x1c0168fd, "at most four times harder");
    assert_eq!(next(0x1c387f6f, 1263163443, 1269211443), 0x1d00e1fd, "at most four times easier");
}

#[test]
fn regtest_headers_follow_the_most_work_and_the_time_rules() {
    let relay = Relay::new();
    let start = mined(&[0; 32], &[0; 32], 1_700_000_000, REGTEST);
    assert_eq!(relay.call("network_name", &[]).unwrap(), [TxCallArgumentType::Nil]);
    relay.call("start", &[bytes(&start), number(0), TxCallArgumentType::String("regtest".into())]).unwrap();
    assert_eq!(relay.call("network_name", &[]).unwrap(), [TxCallArgumentType::String("regtest".into())]);

    // Chain a: three headers on the start, a minute apart.
    let mut a = vec![sha256d(&start)];
    let mut submitted = Vec::new();

    for step in 1..=3u32 {
        let header = mined(a.last().unwrap(), &[1; 32], 1_700_000_000 + 60 * step, REGTEST);
        a.push(sha256d(&header));
        submitted.extend(header);
    }

    assert_eq!(relay.call("submit", &[bytes(&submitted)]).unwrap(), [TxCallArgumentType::U64(3)]);
    assert_eq!(relay.tip(), (a[3].to_vec(), 3));

    // Chain b splits at a's first and goes one further: more work, so it is the main chain now.
    let mut b = vec![a[1]];

    for step in 2..=4u32 {
        let header = mined(b.last().unwrap(), &[2; 32], 1_700_000_000 + 60 * step + 7, REGTEST);
        b.push(sha256d(&header));
        relay.call("submit", &[bytes(&header)]).unwrap();
    }

    assert_eq!(relay.tip(), (b[3].to_vec(), 4));
    assert_eq!((relay.depth(&a[3]), relay.depth(&a[2]), relay.depth(&a[1]), relay.depth(&b[1])), (0, 0, 4, 3), "a's own blocks are off it now");
    assert_eq!(relay.call("block_at", &[number(2)]).unwrap(), [bytes(&b[1])]);

    // What breaks the rules is refused: another difficulty, a time not after the median, a parent
    // it does not hold, a time far ahead, a header that is not 80 bytes.
    let other_bits = mined(&b[3], &[3; 32], 1_700_001_000, 0x207f_fffe);
    let early = mined(&b[3], &[3; 32], 1_700_000_000, REGTEST);
    let orphan = mined(&[7; 32], &[3; 32], 1_700_001_000, REGTEST);
    let ahead = mined(&b[3], &[3; 32], (NOW / 1_000 + 3 * 3_600) as u32, REGTEST);

    assert!(relay.call("submit", &[bytes(&other_bits)]).unwrap_err().contains("difficulty other than"));
    assert!(relay.call("submit", &[bytes(&early)]).unwrap_err().contains("median"));
    assert!(relay.call("submit", &[bytes(&orphan)]).unwrap_err().contains("builds on none"));
    assert!(relay.call("submit", &[bytes(&ahead)]).unwrap_err().contains("two hours ahead"));
    assert!(relay.call("submit", &[bytes(&[0; 79])]).unwrap_err().contains("80 bytes each"));
}

/// CVE-2017-12842: the 64 bytes of two ids hash to an inner node of a block's tree, so with a path
/// one short they pass for a transaction a level up, and could be made to read as one that pays
/// anyone. Nothing in a header says how tall its tree is; a coinbase's path does.
#[test]
fn an_inner_node_does_not_pass_for_a_transaction() {
    let relay = Relay::new();
    let start = mined(&[0; 32], &[0; 32], 1_700_000_000, REGTEST);
    relay.call("start", &[bytes(&start), number(0), TxCallArgumentType::String("regtest".into())]).unwrap();

    // A block of four: its coinbase and three more, the last two under one inner node.
    let coinbase_tx = coinbase(7);
    let leaves = [sha256d(&coinbase_tx), [0xa1; 32], [0xa2; 32], [0xa3; 32]];
    let left = sha256d(&[leaves[0], leaves[1]].concat());
    let inner = [leaves[2], leaves[3]].concat();
    let right = sha256d(&inner);
    let root = sha256d(&[left, right].concat());
    let block = mined(&sha256d(&start), &root, 1_700_000_600, REGTEST);
    relay.call("submit", &[bytes(&block)]).unwrap();
    let hash = sha256d(&block);
    let includes = |txid: &[u8], index: u64, path: &[u8], coinbase: &[u8], coinbase_path: &[u8]| relay.call("includes", &[bytes(&hash), bytes(txid), number(index), bytes(path), bytes(coinbase), bytes(coinbase_path)]).unwrap() == [TxCallArgumentType::Bool(true)];
    let coinbase_path = [leaves[1], right].concat();

    // A real transaction, with the coinbase beside it: taken.
    assert!(includes(&leaves[2], 2, &[leaves[3], left].concat(), &coinbase_tx, &coinbase_path));
    assert!(includes(&leaves[0], 0, &coinbase_path, &coinbase_tx, &coinbase_path));

    // The inner node as a transaction at place 1, a path of one: its path reaches the root, but the
    // tree is two tall, as the coinbase's says.
    assert_eq!(sha256d(&[left, sha256d(&inner)].concat()), root, "the forged path does lead to the root");
    assert!(!includes(&right, 1, &left, &coinbase_tx, &coinbase_path), "one level short of the coinbase's");

    // Nor does a shorter coinbase path help: the coinbase's own id does not lead there in one step.
    assert!(!includes(&right, 1, &left, &coinbase_tx, &right));

    // And an inner node cannot stand in for the coinbase: 64 bytes, and no coinbase's shape.
    assert!(!includes(&right, 1, &left, &[leaves[0], leaves[1]].concat(), &right), "the inner node's own bytes");
    let not_one = pays_nothing();
    assert!(!includes(&right, 1, &left, &not_one, &right), "any other transaction");
    assert!(!includes(&leaves[2], 2, &[leaves[3], left].concat(), &not_one, &coinbase_path));
    assert!(!includes(&leaves[2], 2, &[leaves[3], left].concat(), &[], &coinbase_path), "no coinbase at all");
    assert!(!includes(&leaves[2], 2, &[leaves[3], left].concat(), &coinbase(8), &coinbase_path), "another block's");
}

/// A transaction that is no coinbase: its input spends something.
fn pays_nothing() -> Vec<u8> {
    crate::crypto::bitcoin::tx::Transaction {
        version: 2,
        inputs: vec![crate::crypto::bitcoin::tx::TxIn { txid: [3; 32], vout: 0, script_sig: Vec::new(), sequence: u32::MAX, witness: Vec::new() }],
        outputs: vec![crate::crypto::bitcoin::tx::TxOut { value: 1, script: vec![0x51; 20] }],
        lock_time: 0,
    }
    .base()
}

/// What the chain reads of the relay natively, for anchoring, is what the relay keeps: its tip,
/// depths, a header written whole again from its record, which its peers' syncing nodes check by
/// work, and a proof of an anchor checked against it.
#[cfg(all(feature = "node", feature = "anchoring-btc"))]
#[test]
fn anchoring_reads_the_relay_s_own_records_and_proves_an_anchor_by_them() {
    use crate::anchor::{Anchors, Payload, ProveRules, Relay as Records, chain_tag, prove};
    use crate::structs::tx::{AnchorProof, BtcAnchorProof};
    use crate::params::anchoring::{AnchorSettings, BtcNetworkRules, BtcStart};

    let relay = Relay::new();
    let start = mined(&[0; 32], &[0; 32], 1_700_000_000, REGTEST);
    relay.call("start", &[bytes(&start), number(0), TxCallArgumentType::String("regtest".into())]).unwrap();

    let genesis = crate::crypto::hasher::Hasher::digest(HashType::default_chain(), b"a chain");
    let checkpoint = crate::crypto::hasher::Hasher::digest(HashType::default_chain(), b"its checkpoint at epoch 2");
    let payload = Payload { chain: chain_tag(&genesis), epoch: 2, block: checkpoint.to_bytes().try_into().unwrap(), root: [0; 32] };
    let anchor = crate::crypto::bitcoin::tx::Transaction {
        version: 2,
        inputs: vec![crate::crypto::bitcoin::tx::TxIn { txid: [5; 32], vout: 1, script_sig: Vec::new(), sequence: u32::MAX, witness: Vec::new() }],
        outputs: vec![crate::crypto::bitcoin::tx::TxOut { value: 0, script: payload.script() }],
        lock_time: 0,
    }
    .base();
    let coinbase = coinbase(1);
    let ids = [sha256d(&coinbase), sha256d(&anchor)];
    let mut headers = vec![mined(&sha256d(&start), &crate::crypto::bitcoin::merkle_root(&ids), 1_700_000_060, REGTEST)];

    for step in 2..=3u32 {
        headers.push(mined(&sha256d(headers.last().unwrap()), &[step as u8; 32], 1_700_000_000 + 60 * step, REGTEST));
    }

    relay.call("submit", &[bytes(&headers.concat())]).unwrap();
    let records = Records::new(&relay.world.state, relay.relay);
    let block = sha256d(&headers[0]);

    assert_eq!(records.network().unwrap(), Some(BtcNetworkRules::Regtest));
    assert_eq!(records.tip().unwrap().map(|(_, height)| height), Some(3));
    assert_eq!(records.depth(&block).unwrap(), 3);
    assert_eq!(records.raw_header(&block).unwrap().map(|header| header.to_vec()), Some(headers[0].clone()));
    assert_eq!(records.raw_header(&sha256d(&start)).unwrap().map(|header| header.to_vec()), Some(start.clone()), "the start too");

    relay.world.state.put_value(crate::db::Column::Heights, 16u64.to_be_bytes().to_vec(), &checkpoint);
    let settings = AnchorSettings {
        bitcoin: true,
        enforce: true,
        relay: Some(relay.relay),
        btc_start: Some(BtcStart { header: start, height: 0, network: BtcNetworkRules::Regtest }),
        depth: 3,
        ..AnchorSettings::OFF
    };
    let rules = ProveRules { settings: &settings, genesis, checkpoint_interval: Some(8), pooled: false };
    let proof = AnchorProof::Bitcoin(BtcAnchorProof {
        block,
        raw: anchor,
        index: 1,
        path: crate::crypto::bitcoin::merkle_path(&ids, 1),
        coinbase,
        coinbase_path: crate::crypto::bitcoin::merkle_path(&ids, 0),
    });

    prove(&relay.world.state, &rules, &Address::Ed25519([1; 32]), &proof).unwrap();
    assert_eq!(Anchors::new(&relay.world.state).enforced_height().unwrap(), Some(16));
}
