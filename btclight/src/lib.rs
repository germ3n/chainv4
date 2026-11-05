//! Bitcoin's header rules, as Bitcoin's nodes check headers, cut down to what a light client
//! needs: what lets a chain that runs no Bitcoin node follow Bitcoin's most-work chain from a
//! checkpoint, and prove that a transaction is in one of its blocks.
//!
//! - [`target`], [`compact`], [`work`] and [`retarget`]: compact difficulty, the work a target
//!   stands for, and the difficulty every 2016 blocks.
//! - [`Network`]: main, regtest and test, and the rules each checks.
//! - [`Header`]: the 80 bytes Bitcoin writes.
//! - [`Kept`]: what a relay keeps of each header, byte for byte as the btc_relay contract stores
//!   it, and [`Kept::child`], which checks a header building on one by every rule its network has:
//!   proof of work, the retarget, the median time past and the two hours ahead.
//! - [`MainChain`] and [`follow`]: the most-work chain, by height, moved to a new tip.
//! - [`is_coinbase`], [`leads_to`] and [`includes`]: a transaction in a block by its Merkle path,
//!   with the coinbase's path showing how tall the tree is (CVE-2017-12842).
//!
//! No hashing is built in: a [`Crypto`] passes SHA-256 twice in, so the btc_relay contract
//! (contracts/examples/btc_relay) runs it as a host call, and the node runs it natively, by the same
//! rules from this one source.

#![no_std]

extern crate alloc;

use alloc::vec::Vec;

pub use ethnum::U256;

#[cfg(test)]
mod tests;

/// Blocks between difficulty changes.
pub const INTERVAL: u64 = 2016;

/// The time a difficulty period is meant to take, in seconds: two weeks.
pub const TIMESPAN: u64 = 14 * 24 * 60 * 60;

/// How many past blocks' times the median is taken over.
pub const MEDIAN_SPAN: usize = 11;

/// How far ahead of the clock a header's time may be, in seconds, as Bitcoin's nodes allow.
pub const MOST_AHEAD: u64 = 2 * 60 * 60;

/// The longest Merkle path a block's tree can have: 32 levels.
pub const MAX_PATH: usize = 32;

/// A header's length as Bitcoin writes it.
pub const HEADER_LEN: usize = 80;

/// What the rules need done that they do not do themselves.
pub trait Crypto {
    /// SHA-256 twice, as Bitcoin hashes headers, transactions and Merkle nodes.
    fn sha256d(&self, data: &[u8]) -> [u8; 32];
}

/// Why a header was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// A compact difficulty Bitcoin refuses: negative, or past 256 bits.
    BadDifficulty,
    /// A hash above the target its difficulty names.
    NoWork,
    /// A difficulty easier than the network's limit.
    TooEasy,
    /// A retarget that does not fit 256 bits.
    RetargetOutOfRange,
    /// A difficulty other than the network's rules give at its height.
    WrongDifficulty,
    /// A time not after the median of the last eleven.
    TooEarly,
    /// A time more than two hours ahead of the clock.
    TooFarAhead,
}

impl Error {
    /// What went wrong, in a sentence a contract can fail with.
    pub fn message(self) -> &'static str {
        match self {
            Error::BadDifficulty => "a difficulty Bitcoin refuses",
            Error::NoWork => "a header without the work its difficulty asks",
            Error::TooEasy => "a difficulty easier than the network allows",
            Error::RetargetOutOfRange => "a retarget out of range",
            Error::WrongDifficulty => "a difficulty other than the network's rules give",
            Error::TooEarly => "a time not after the median of the last eleven",
            Error::TooFarAhead => "a time more than two hours ahead",
        }
    }
}

impl core::fmt::Display for Error {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(self.message())
    }
}

/// Which Bitcoin network headers follow, and so which rules they are checked by. Main checks every
/// rule; regtest keeps one difficulty, as Bitcoin's regtest does; test checks only that each
/// header's work meets the difficulty it names, which testnet4 and signet allow too loosely for
/// more, and is for testing alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Network {
    Main,
    Regtest,
    Test,
}

impl Network {
    /// Every network.
    pub const ALL: [Network; 3] = [Network::Main, Network::Regtest, Network::Test];

    /// Its name, as a relay is started with: main, regtest or test.
    pub fn name(self) -> &'static str {
        match self {
            Network::Main => "main",
            Network::Regtest => "regtest",
            Network::Test => "test",
        }
    }

    /// The network by its name.
    pub fn from_name(name: &str) -> Option<Network> {
        match name {
            "main" => Some(Network::Main),
            "regtest" => Some(Network::Regtest),
            "test" => Some(Network::Test),
            _ => None,
        }
    }

    /// The easiest difficulty its blocks may have, compact.
    pub fn limit(self) -> u32 {
        match self {
            Network::Main => 0x1d00_ffff,
            Network::Regtest | Network::Test => 0x207f_ffff,
        }
    }
}

/// A compact difficulty as the target it stands for, or None for one Bitcoin refuses: negative,
/// or past 256 bits.
pub fn target(bits: u32) -> Option<U256> {
    let (size, mantissa) = ((bits >> 24) as usize, bits & 0x007f_ffff);

    if bits & 0x0080_0000 != 0 || (mantissa != 0 && size > 34) {
        return None;
    }

    let mut bytes = [0u8; 32];

    // The mantissa's three bytes, the highest at size - 1.
    for (index, byte) in mantissa.to_le_bytes()[..3].iter().enumerate() {
        match (size + index).checked_sub(3) {
            Some(at) if at < 32 => bytes[at] = *byte,
            Some(_) if *byte != 0 => return None,
            _ => {}
        }
    }

    Some(U256::from_le_bytes(bytes))
}

/// A target as compact difficulty, as Bitcoin rounds it.
pub fn compact(target: U256) -> u32 {
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
pub fn work(target: U256) -> U256 {
    (U256::MAX - target).checked_div(target + U256::ONE).unwrap_or(U256::ZERO) + U256::ONE
}

/// The difficulty after a period, as Bitcoin retargets: its target scaled by how long the period
/// took, from its first block's time to its last's, against two weeks, at most four times either
/// way, and never easier than the limit. None for a difficulty Bitcoin refuses, or a product past
/// 256 bits.
pub fn retarget(bits: u32, first: u32, last: u32, limit: u32) -> Option<u32> {
    let span = u64::from(last.saturating_sub(first)).clamp(TIMESPAN / 4, TIMESPAN * 4);
    let next = target(bits)?.checked_mul(U256::from(span))?.checked_div(U256::from(TIMESPAN))?;

    Some(compact(next.min(target(limit)?)))
}

/// A header as Bitcoin writes it, 80 bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub version: u32,
    pub prev: [u8; 32],
    pub merkle: [u8; 32],
    pub time: u32,
    pub bits: u32,
    pub nonce: u32,
}

impl Header {
    /// A header from its 80 bytes, or None for any other length.
    pub fn read(bytes: &[u8]) -> Option<Header> {
        let word = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().expect("4 bytes"));

        (bytes.len() == HEADER_LEN).then(|| Header {
            version: word(0),
            prev: bytes[4..36].try_into().expect("32 bytes"),
            merkle: bytes[36..68].try_into().expect("32 bytes"),
            time: word(68),
            bits: word(72),
            nonce: word(76),
        })
    }

    /// Its 80 bytes.
    pub fn encode(&self) -> [u8; HEADER_LEN] {
        let mut out = [0u8; HEADER_LEN];
        out[..4].copy_from_slice(&self.version.to_le_bytes());
        out[4..36].copy_from_slice(&self.prev);
        out[36..68].copy_from_slice(&self.merkle);
        out[68..72].copy_from_slice(&self.time.to_le_bytes());
        out[72..76].copy_from_slice(&self.bits.to_le_bytes());
        out[76..].copy_from_slice(&self.nonce.to_le_bytes());

        out
    }
}

/// What a relay keeps of each header it took. Its record, as [`Kept::encode`] writes it and the
/// btc_relay contract stores it under the header's hash, is, little-endian throughout:
///
/// | Bytes | What |
/// | --- | --- |
/// | 0..32 | the parent's hash |
/// | 32..64 | the Merkle root |
/// | 64..72 | the height |
/// | 72..104 | the work from the start up to and including it |
/// | 104..108 | the compact difficulty |
/// | 108..112 | the time |
/// | 112..116 | the time of its difficulty period's first block |
/// | 116 | how many times follow, 1 to 11 |
/// | 117.. | the last times up to its own, oldest first, 4 bytes each |
/// | then 4 | the version |
/// | then 4 | the nonce |
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Kept {
    pub prev: [u8; 32],
    pub merkle: [u8; 32],
    pub height: u64,
    /// Work from the start up to and including it.
    pub work: U256,
    pub bits: u32,
    pub time: u32,
    /// The time of the first block of its difficulty period.
    pub period_start: u32,
    /// The times of the last MEDIAN_SPAN blocks up to it, oldest first.
    pub times: Vec<u32>,
    /// The header's version and nonce, kept after the times so a node can write the header whole
    /// again and serve it to a peer checking anchors.
    pub version: u32,
    pub nonce: u32,
}

impl Kept {
    /// The record of the header a relay starts at, which it trusts: no work before it, and its own
    /// time as its period's first and the only one the median is taken over.
    pub fn start(header: &Header, height: u64) -> Kept {
        Kept {
            prev: header.prev,
            merkle: header.merkle,
            height,
            work: U256::ZERO,
            bits: header.bits,
            time: header.time,
            period_start: header.time,
            times: alloc::vec![header.time],
            version: header.version,
            nonce: header.nonce,
        }
    }

    /// Its record's bytes.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(125 + 4 * self.times.len());
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

        out.extend_from_slice(&self.version.to_le_bytes());
        out.extend_from_slice(&self.nonce.to_le_bytes());

        out
    }

    /// One from its record's bytes, or None for bytes too short to be one.
    pub fn decode(bytes: &[u8]) -> Option<Kept> {
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
            version: word(117 + 4 * count)?,
            nonce: word(121 + 4 * count)?,
        })
    }

    /// The header it was kept from, whole again.
    pub fn header(&self) -> Header {
        Header { version: self.version, prev: self.prev, merkle: self.merkle, time: self.time, bits: self.bits, nonce: self.nonce }
    }

    /// The median of its last times: what the next block's time must be after.
    pub fn median_time(&self) -> u32 {
        let mut sorted = self.times.clone();
        sorted.sort_unstable();

        sorted[sorted.len() / 2]
    }

    /// The record of a header building on this one, once it checks by every rule its network has:
    /// the work its difficulty asks, never easier than the limit, the retarget every 2016 blocks
    /// on main and one difficulty on regtest, a time after the median of the last eleven, and none
    /// more than two hours past now.
    ///
    /// - `hash`: the header's SHA-256 twice, in Bitcoin's byte order.
    /// - `now`: the clock, in seconds since the Unix epoch.
    pub fn child(&self, header: &Header, hash: &[u8; 32], network: Network, now: u64) -> Result<Kept, Error> {
        let height = self.height + 1;
        let claimed = target(header.bits).ok_or(Error::BadDifficulty)?;

        if U256::from_le_bytes(*hash) > claimed {
            return Err(Error::NoWork);
        }

        if claimed > target(network.limit()).expect("the limit is a target") {
            return Err(Error::TooEasy);
        }

        let expected = match network {
            Network::Main if height.is_multiple_of(INTERVAL) => Some(retarget(self.bits, self.period_start, self.time, network.limit()).ok_or(Error::RetargetOutOfRange)?),
            Network::Main | Network::Regtest => Some(self.bits),
            Network::Test => None,
        };

        if expected.is_some_and(|bits| bits != header.bits) {
            return Err(Error::WrongDifficulty);
        }

        if header.time <= self.median_time() {
            return Err(Error::TooEarly);
        }

        if u64::from(header.time) > now + MOST_AHEAD {
            return Err(Error::TooFarAhead);
        }

        let mut times = self.times.clone();
        times.push(header.time);

        if times.len() > MEDIAN_SPAN {
            times.remove(0);
        }

        Ok(Kept {
            prev: header.prev,
            merkle: header.merkle,
            height,
            work: self.work + work(claimed),
            bits: header.bits,
            time: header.time,
            period_start: if height.is_multiple_of(INTERVAL) { header.time } else { self.period_start },
            times,
            version: header.version,
            nonce: header.nonce,
        })
    }
}

/// Where a relay keeps its headers and its main chain: storage for the contract, memory for the
/// node.
pub trait MainChain {
    /// The height and parent of a header it holds.
    fn link(&self, hash: &[u8; 32]) -> Option<(u64, [u8; 32])>;

    /// The main chain's block at a height.
    fn main_at(&self, height: u64) -> Option<[u8; 32]>;

    /// Makes a block the main chain's at its height.
    fn set_main(&mut self, height: u64, hash: &[u8; 32]);

    /// Leaves the main chain without a block at a height.
    fn unset_main(&mut self, height: u64);
}

/// Makes a header, by its hash, height and parent, the main chain's tip, the old tip at
/// old_height: the main chain is its ancestors, down to where they meet the old one. Call it with
/// a header whose work is more than the old tip's; keeping the tip itself is the caller's.
pub fn follow<M: MainChain + ?Sized>(chain: &mut M, hash: &[u8; 32], height: u64, prev: [u8; 32], old_height: u64) {
    for above in height + 1..=old_height {
        chain.unset_main(above);
    }

    let (mut at, mut height, mut prev) = (*hash, height, prev);

    while chain.main_at(height) != Some(at) {
        chain.set_main(height, &at);

        let Some((parent_height, parent_prev)) = chain.link(&prev) else {
            break;
        };

        (at, height, prev) = (prev, parent_height, parent_prev);
    }
}

/// Whether bytes are a coinbase, as a block's first transaction is, without its witness: one input,
/// spending nothing. Never 64 bytes, as an inner node of a Merkle tree is.
pub fn is_coinbase(raw: &[u8]) -> bool {
    raw.len() != 64 && raw.len() >= 41 && raw[4] == 1 && raw[5..37] == [0; 32] && raw[37..41] == [0xff; 4]
}

/// Whether a leaf at a place, by its path, the siblings from its own row up, comes to a Merkle
/// root.
pub fn leads_to<C: Crypto + ?Sized>(crypto: &C, leaf: &[u8; 32], index: u64, path: &[[u8; 32]], root: &[u8; 32]) -> bool {
    let (mut at, mut index) = (*leaf, index);

    for sibling in path {
        let mut pair = [0u8; 64];

        match index & 1 {
            0 => (pair[..32].copy_from_slice(&at), pair[32..].copy_from_slice(sibling)),
            _ => (pair[..32].copy_from_slice(sibling), pair[32..].copy_from_slice(&at)),
        };

        at = crypto.sha256d(&pair);
        index >>= 1;
    }

    index == 0 && at == *root
}

/// Whether a transaction, by its id in Bitcoin's byte order, is the index-th under a block's
/// Merkle root, by its path. A block's header says nothing of how tall its tree is, so an inner
/// node, the 64 bytes of two ids, could pass for a transaction a level up (CVE-2017-12842). So the
/// block's coinbase comes too, the transaction itself and its path: it shows how tall the tree is,
/// and the path must be just as long, at most MAX_PATH.
pub fn includes<C: Crypto + ?Sized>(crypto: &C, root: &[u8; 32], txid: &[u8; 32], index: u64, path: &[[u8; 32]], coinbase: &[u8], coinbase_path: &[[u8; 32]]) -> bool {
    path.len() <= MAX_PATH && path.len() == coinbase_path.len() && is_coinbase(coinbase) && leads_to(crypto, &crypto.sha256d(coinbase), 0, coinbase_path, root) && leads_to(crypto, txid, index, path, root)
}
