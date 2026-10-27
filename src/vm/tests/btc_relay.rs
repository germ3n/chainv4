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
    let includes = |txid: &[u8], index: u64| relay.call("includes", &[bytes(&sha256d(&block_1)), bytes(txid), number(index), bytes(&[])]).unwrap();

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
