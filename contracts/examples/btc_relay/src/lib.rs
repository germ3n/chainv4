//! A Bitcoin light client as a contract (docs/bridges.md): anyone submits Bitcoin's block
//! headers, and it keeps the chain with the most work from a checkpoint its deployer set, checked
//! as Bitcoin's nodes check headers: proof of work, the difficulty every 2016 blocks, and the
//! median time past. Contracts then ask it how deep a block is, and whether a transaction is in
//! it, by its Merkle path: how the bridges' Bitcoin gateway (contracts/lua/btc_gateway.lua) mints
//! wBTC for a deposit no one has to vouch for.
//!
//! Networks: main checks every rule; regtest keeps one difficulty, as Bitcoin's regtest does; test
//! checks only that each header's work meets the difficulty it names, which testnet4 and signet
//! allow too loosely for more, and is for testing alone.

#![no_std]

extern crate alloc;

use {
    alloc::{string::String, vec::Vec},
    chainv4_sdk::{Address, U256, Value, block_time, emit, export, sender, sha256d, storage},
};

/// Blocks between difficulty changes, and the time they are meant to take, in seconds.
const INTERVAL: u64 = 2016;
const TIMESPAN: u64 = 14 * 24 * 60 * 60;
/// How many past blocks' times the median is taken over.
const MEDIAN_SPAN: usize = 11;
/// How far ahead of the chain's block time a header's may be, in seconds, as Bitcoin's nodes
/// allow against their clocks.
const MOST_AHEAD: u64 = 2 * 60 * 60;
/// The most headers one submission takes.
const MOST_HEADERS: usize = 2_000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Network {
    Main,
    Regtest,
    Test,
}

impl Network {
    fn from_name(name: &str) -> Option<Network> {
        match name {
            "main" => Some(Network::Main),
            "regtest" => Some(Network::Regtest),
            "test" => Some(Network::Test),
            _ => None,
        }
    }

    /// The easiest difficulty its blocks may have, compact.
    fn limit(self) -> u32 {
        match self {
            Network::Main => 0x1d00_ffff,
            Network::Regtest | Network::Test => 0x207f_ffff,
        }
    }
}

/// A header as Bitcoin writes it, 80 bytes.
struct Header {
    prev: [u8; 32],
    merkle: [u8; 32],
    time: u32,
    bits: u32,
}

impl Header {
    fn read(bytes: &[u8]) -> Option<Header> {
        let word = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().expect("4 bytes"));

        (bytes.len() == 80).then(|| Header {
            prev: bytes[4..36].try_into().expect("32 bytes"),
            merkle: bytes[36..68].try_into().expect("32 bytes"),
            time: word(68),
            bits: word(72),
        })
    }
}

/// What the relay keeps of each header it took.
struct Kept {
    prev: [u8; 32],
    merkle: [u8; 32],
    height: u64,
    /// Work from the checkpoint up to and including it.
    work: U256,
    bits: u32,
    time: u32,
    /// The time of the first block of its difficulty period.
    period_start: u32,
    /// The times of the last MEDIAN_SPAN blocks up to it, oldest first.
    times: Vec<u32>,
}

impl Kept {
    fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(120);
        out.extend_from_slice(&self.prev);
        out.extend_from_slice(&self.merkle);
        out.extend_from_slice(&self.height.to_le_bytes());
        out.extend_from_slice(&self.work.to_le_bytes());
        out.extend_from_slice(&self.bits.to_le_bytes());
        out.extend_from_slice(&self.time.to_le_bytes());
        out.extend_from_slice(&self.period_start.to_le_bytes());
        out.push(self.times.len() as u8);

        for time in &self.times {
            out.extend_from_slice(&time.to_le_bytes());
        }

        out
    }

    fn decode(bytes: &[u8]) -> Option<Kept> {
        let word = |at: usize| Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?));
        let count = usize::from(*bytes.get(116)?);

        Some(Kept {
            prev: bytes.get(..32)?.try_into().ok()?,
            merkle: bytes.get(32..64)?.try_into().ok()?,
            height: u64::from_le_bytes(bytes.get(64..72)?.try_into().ok()?),
            work: U256::from_le_bytes(bytes.get(72..104)?.try_into().ok()?),
            bits: word(104)?,
            time: word(108)?,
            period_start: word(112)?,
            times: (0..count).map(|index| word(117 + 4 * index)).collect::<Option<Vec<_>>>()?,
        })
    }

    /// The median of its last times: what the next block's time must be after.
    fn median_time(&self) -> u32 {
        let mut sorted = self.times.clone();
        sorted.sort_unstable();

        sorted[sorted.len() / 2]
    }
}

fn headers() -> storage::Namespace<'static> {
    storage::namespace(b"headers")
}

/// The chain with the most work, by height.
fn main_chain() -> storage::Namespace<'static> {
    storage::namespace(b"main")
}

fn kept(hash: &[u8; 32]) -> Option<Kept> {
    headers().get(hash).and_then(|bytes| Kept::decode(&bytes))
}

fn network() -> Option<Network> {
    storage::own().get(b"network").and_then(|bytes| Network::from_name(core::str::from_utf8(&bytes).ok()?))
}

fn tip_hash() -> Option<[u8; 32]> {
    storage::own().get(b"tip").and_then(|bytes| bytes.try_into().ok())
}

fn main_at(height: u64) -> Option<[u8; 32]> {
    main_chain().get(&height.to_be_bytes()).and_then(|bytes| bytes.try_into().ok())
}

/// A compact difficulty as the target it stands for, or None for one Bitcoin refuses: negative,
/// or past 256 bits.
fn target(bits: u32) -> Option<U256> {
    let (size, mantissa) = ((bits >> 24) as usize, bits & 0x007f_ffff);

    if bits & 0x0080_0000 != 0 || (mantissa != 0 && size > 34) {
        return None;
    }

    let mut bytes = [0u8; 32];
    let mantissa = mantissa.to_le_bytes();

    // The mantissa's three bytes, the highest at size - 1.
    for (index, byte) in mantissa[..3].iter().enumerate() {
        let at = (size + index).checked_sub(3);

        match at {
            Some(at) if at < 32 => bytes[at] = *byte,
            Some(_) if *byte != 0 => return None,
            _ => {}
        }
    }

    Some(U256::from_le_bytes(bytes))
}

/// A target as compact difficulty, as Bitcoin rounds it.
fn compact(target: U256) -> u32 {
    let bytes = target.to_le_bytes();
    let size = bytes.iter().rposition(|byte| *byte != 0).map_or(0, |top| top + 1);
    let mantissa = |size: usize| -> u32 { (0..3).fold(0, |mantissa, index| mantissa << 8 | size.checked_sub(1 + index).map_or(0, |at| u32::from(bytes[at]))) };
    let (mut size, mut value) = (size, mantissa(size));

    // The top bit means negative: a byte more, and the mantissa moves down.
    if value & 0x0080_0000 != 0 {
        value >>= 8;
        size += 1;
    }

    value | (size as u32) << 24
}

/// The work a target stands for: about 2^256 / (target + 1).
fn work(target: U256) -> U256 {
    (U256::MAX - target).checked_div(target + U256::ONE).unwrap_or(U256::ZERO) + U256::ONE
}

/// The difficulty after a period, as Bitcoin retargets: its target scaled by how long the period
/// took against two weeks, at most four times either way, and never easier than the limit.
fn retarget(bits: u32, first: u32, last: u32, limit: u32) -> Option<u32> {
    let span = u64::from(last.saturating_sub(first)).clamp(TIMESPAN / 4, TIMESPAN * 4);
    let next = target(bits)?.checked_mul(U256::from(span))?.checked_div(U256::from(TIMESPAN))?;

    Some(compact(next.min(target(limit)?)))
}

fn checkpoint_owner() -> Option<Address> {
    Address::decode(storage::own().get(b"owner")?.as_slice().try_into().ok()?)
}

/// Run once by the chain: whoever deploys sets the checkpoint.
#[export]
fn init() {
    storage::own().set(b"owner", &sender().encode());
}

/// Starts the relay at a header its deployer trusts, at its height, on a network: main, regtest or
/// test. On main, the height is a difficulty period's first, so the next retarget can be checked.
#[export]
fn start(header: Vec<u8>, height: u64, network_name: String) -> Result<bool, &'static str> {
    if checkpoint_owner() != Some(sender()) {
        return Err("only the deployer sets the checkpoint");
    }

    if tip_hash().is_some() {
        return Err("the relay has started already");
    }

    let network = Network::from_name(&network_name).ok_or("the network is main, regtest or test")?;
    let parsed = Header::read(&header).ok_or("a header is 80 bytes")?;

    if network == Network::Main && !height.is_multiple_of(INTERVAL) {
        return Err("on main, start at a difficulty period's first block: a height divisible by 2016");
    }

    let hash = sha256d(&header);
    let record = Kept { prev: parsed.prev, merkle: parsed.merkle, height, work: U256::ZERO, bits: parsed.bits, time: parsed.time, period_start: parsed.time, times: alloc::vec![parsed.time] };
    headers().set(&hash, &record.encode());
    main_chain().set(&height.to_be_bytes(), &hash);
    storage::own().set(b"tip", &hash);
    storage::own().set(b"network", network_name.as_bytes());

    Ok(true)
}

/// Takes headers, 80 bytes each back to back, each building on one the relay holds: those it
/// held already are skipped, and one that breaks a rule fails the submission. The chain with the
/// most work becomes the main one. How many were new.
#[export]
fn submit(headers_bytes: Vec<u8>) -> Result<u64, &'static str> {
    let network = network().ok_or("the relay has not started")?;

    if headers_bytes.is_empty() || !headers_bytes.len().is_multiple_of(80) || headers_bytes.len() / 80 > MOST_HEADERS {
        return Err("1 to 2,000 headers, 80 bytes each");
    }

    let mut taken = 0;

    for header in headers_bytes.chunks(80) {
        let hash = sha256d(header);

        if headers().get(&hash).is_some() {
            continue;
        }

        let parsed = Header::read(header).ok_or("a header is 80 bytes")?;
        let prev = kept(&parsed.prev).ok_or("a header that builds on none the relay holds")?;
        let height = prev.height + 1;
        let claimed = target(parsed.bits).ok_or("a difficulty Bitcoin refuses")?;

        if U256::from_le_bytes(hash) > claimed {
            return Err("a header without the work its difficulty asks");
        }

        if claimed > target(network.limit()).expect("the limit is a target") {
            return Err("a difficulty easier than the network allows");
        }

        let expected = match network {
            Network::Main if height.is_multiple_of(INTERVAL) => Some(retarget(prev.bits, prev.period_start, prev.time, network.limit()).ok_or("a retarget out of range")?),
            Network::Main | Network::Regtest => Some(prev.bits),
            Network::Test => None,
        };

        if expected.is_some_and(|bits| bits != parsed.bits) {
            return Err("a difficulty other than the network's rules give");
        }

        if parsed.time <= prev.median_time() {
            return Err("a time not after the median of the last eleven");
        }

        if u64::from(parsed.time) > block_time() / 1_000 + MOST_AHEAD {
            return Err("a time more than two hours ahead");
        }

        let mut times = prev.times.clone();
        times.push(parsed.time);

        if times.len() > MEDIAN_SPAN {
            times.remove(0);
        }

        let record = Kept {
            prev: parsed.prev,
            merkle: parsed.merkle,
            height,
            work: prev.work + work(claimed),
            bits: parsed.bits,
            time: parsed.time,
            period_start: if height.is_multiple_of(INTERVAL) { parsed.time } else { prev.period_start },
            times,
        };
        headers().set(&hash, &record.encode());
        taken += 1;

        let best = tip_hash().and_then(|tip| kept(&tip)).ok_or("the relay lost its tip")?;

        if record.work > best.work {
            follow(&hash, &record, best.height);
        }
    }

    Ok(taken)
}

/// Makes a header the tip: the main chain is its ancestors, down to where they meet the old one.
fn follow(hash: &[u8; 32], record: &Kept, old_height: u64) {
    for height in record.height + 1..=old_height {
        main_chain().delete(&height.to_be_bytes());
    }

    let (mut at, mut height, mut prev) = (*hash, record.height, record.prev);

    while main_at(height) != Some(at) {
        main_chain().set(&height.to_be_bytes(), &at);

        let Some(parent) = kept(&prev) else {
            break;
        };

        (at, height, prev) = (prev, parent.height, parent.prev);
    }

    storage::own().set(b"tip", hash);
    emit("Tip", &[Value::Bytes(hash.to_vec()), Value::U64(record.height)]);
}

/// The main chain's tip: its hash, in Bitcoin's byte order, and its height.
#[export]
fn tip() -> Result<(Vec<u8>, u64), &'static str> {
    let hash = tip_hash().ok_or("the relay has not started")?;

    Ok((hash.to_vec(), kept(&hash).ok_or("the relay lost its tip")?.height))
}

/// The main chain's block at a height, if it holds one.
#[export]
fn block_at(height: u64) -> Option<Vec<u8>> {
    main_at(height).map(|hash| hash.to_vec())
}

/// How deep a block is in the main chain: 1 for the tip, 0 for one off it or unknown.
fn depth(hash: &[u8; 32]) -> u64 {
    let (Some(record), Some(tip)) = (kept(hash), tip_hash().and_then(|tip| kept(&tip))) else {
        return 0;
    };

    match main_at(record.height) == Some(*hash) {
        true => tip.height - record.height + 1,
        false => 0,
    }
}

/// How deep a block is in the main chain: 1 for the tip, 0 for one off it or unknown.
#[export]
fn confirmations(hash: Vec<u8>) -> u64 {
    <[u8; 32]>::try_from(hash).map_or(0, |hash| depth(&hash))
}

/// Whether a transaction, by its id in Bitcoin's byte order, is the index-th in a main-chain
/// block, by its Merkle path: the siblings, 32 bytes each, from the transaction up.
#[export]
fn includes(block: Vec<u8>, txid: Vec<u8>, index: u64, path: Vec<u8>) -> bool {
    let (Ok(block), Ok(txid)) = (<[u8; 32]>::try_from(block), <[u8; 32]>::try_from(txid)) else {
        return false;
    };

    if depth(&block) == 0 || !path.len().is_multiple_of(32) || path.len() / 32 > 32 {
        return false;
    }

    let Some(record) = kept(&block) else {
        return false;
    };
    let mut at = txid;
    let mut index = index;

    for sibling in path.chunks(32) {
        let mut pair = [0u8; 64];

        match index & 1 {
            0 => (pair[..32].copy_from_slice(&at), pair[32..].copy_from_slice(sibling)),
            _ => (pair[..32].copy_from_slice(sibling), pair[32..].copy_from_slice(&at)),
        };

        at = sha256d(&pair);
        index >>= 1;
    }

    index == 0 && at == record.merkle
}

/// The difficulty Bitcoin's rules give after a period that started at one time and ended at
/// another, from its difficulty: what submit checks every 2016th block by.
#[export]
fn next_bits(bits: u64, first: u64, last: u64) -> Result<u64, &'static str> {
    let bits = u32::try_from(bits).map_err(|_| "a difficulty is 32 bits")?;
    let (first, last) = (u32::try_from(first).map_err(|_| "a time is 32 bits")?, u32::try_from(last).map_err(|_| "a time is 32 bits")?);

    retarget(bits, first, last, Network::Main.limit()).map(u64::from).ok_or("a retarget out of range")
}
