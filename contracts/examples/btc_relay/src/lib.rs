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
    btclight::{Header, Kept, MainChain, Network},
    chainv4_sdk::{Address, Value, block_time, emit, export, sender, sha256d, storage},
};

/// The most headers one submission takes.
const MOST_HEADERS: usize = 2_000;

/// SHA-256 twice through the host, for the rules in btclight.
struct Host;

impl btclight::Crypto for Host {
    fn sha256d(&self, data: &[u8]) -> [u8; 32] {
        sha256d(data)
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

/// The relay's storage, as btclight follows the most work through it.
struct Stored;

impl MainChain for Stored {
    fn link(&self, hash: &[u8; 32]) -> Option<(u64, [u8; 32])> {
        kept(hash).map(|kept| (kept.height, kept.prev))
    }

    fn main_at(&self, height: u64) -> Option<[u8; 32]> {
        main_at(height)
    }

    fn set_main(&mut self, height: u64, hash: &[u8; 32]) {
        main_chain().set(&height.to_be_bytes(), hash);
    }

    fn unset_main(&mut self, height: u64) {
        main_chain().delete(&height.to_be_bytes());
    }
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

    if network == Network::Main && !height.is_multiple_of(btclight::INTERVAL) {
        return Err("on main, start at a difficulty period's first block: a height divisible by 2016");
    }

    let hash = sha256d(&header);
    headers().set(&hash, &Kept::start(&parsed, height).encode());
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
        let record = prev.child(&parsed, &hash, network, block_time() / 1_000).map_err(btclight::Error::message)?;
        headers().set(&hash, &record.encode());
        taken += 1;

        let best = tip_hash().and_then(|tip| kept(&tip)).ok_or("the relay lost its tip")?;

        if record.work > best.work {
            btclight::follow(&mut Stored, &hash, record.height, record.prev, best.height);
            storage::own().set(b"tip", &hash);
            emit("Tip", &[Value::Bytes(hash.to_vec()), Value::U64(record.height)]);
        }
    }

    Ok(taken)
}

/// The main chain's tip: its hash, in Bitcoin's byte order, and its height.
#[export]
fn tip() -> Result<(Vec<u8>, u64), &'static str> {
    let hash = tip_hash().ok_or("the relay has not started")?;

    Ok((hash.to_vec(), kept(&hash).ok_or("the relay lost its tip")?.height))
}

/// The network it follows, as it was started: main, regtest or test; none before it starts.
#[export]
fn network_name() -> Option<String> {
    storage::own().get(b"network").and_then(|bytes| String::from_utf8(bytes).ok())
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

/// Whether a transaction, by its id in Bitcoin's byte order, is the index-th in a main-chain block,
/// by its Merkle path: the siblings, 32 bytes each, from the transaction up. A block's header says
/// nothing of how tall its tree is, so an inner node, the 64 bytes of two ids, could pass for a
/// transaction a level up (CVE-2017-12842). So its coinbase comes too, the transaction itself and
/// its path: it shows how tall the tree is, and the path must be just as long.
#[export]
fn includes(block: Vec<u8>, txid: Vec<u8>, index: u64, path: Vec<u8>, coinbase: Vec<u8>, coinbase_path: Vec<u8>) -> bool {
    let (Ok(block), Ok(txid)) = (<[u8; 32]>::try_from(block), <[u8; 32]>::try_from(txid)) else {
        return false;
    };

    let ((path, []), (coinbase_path, [])) = (path.as_chunks::<32>(), coinbase_path.as_chunks::<32>()) else {
        return false;
    };

    if depth(&block) == 0 {
        return false;
    }

    let Some(record) = kept(&block) else {
        return false;
    };

    btclight::includes(&Host, &record.merkle, &txid, index, path, &coinbase, coinbase_path)
}

/// The difficulty Bitcoin's rules give after a period that started at one time and ended at
/// another, from its difficulty: what submit checks every 2016th block by.
#[export]
fn next_bits(bits: u64, first: u64, last: u64) -> Result<u64, &'static str> {
    let bits = u32::try_from(bits).map_err(|_| "a difficulty is 32 bits")?;
    let (first, last) = (u32::try_from(first).map_err(|_| "a time is 32 bits")?, u32::try_from(last).map_err(|_| "a time is 32 bits")?);

    btclight::retarget(bits, first, last, Network::Main.limit()).map(u64::from).ok_or("a retarget out of range")
}
