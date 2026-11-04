//! Bitcoin's headers checked by a node on its own, as contracts/examples/btc_relay checks them on
//! chain: proof of work, the retarget every 2016 blocks on main, the median time past, and the most
//! work wins. Anchored sync feeds it every peer's relay headers from anchoring's btc_start, so the
//! Bitcoin chain it compares anchors on is the one with the most work any peer showed, whatever a
//! fork's relay claims.

use {
    crate::{crypto::bitcoin::sha256d, params::anchoring::{BtcNetworkRules, BtcStart}, structs::u256::u256},
    r#macro::document,
    std::collections::HashMap,
};

/// Blocks between difficulty changes, and the time they are meant to take, in seconds.
const INTERVAL: u64 = 2016;
const TIMESPAN: u64 = 14 * 24 * 60 * 60;
/// How many past blocks' times the median is taken over.
const MEDIAN_SPAN: usize = 11;
/// How far ahead of the node's clock a header's time may be, in seconds.
const MOST_AHEAD: u64 = 2 * 60 * 60;

fn limit(network: BtcNetworkRules) -> u32 {
    match network {
        BtcNetworkRules::Main => 0x1d00_ffff,
        BtcNetworkRules::Regtest | BtcNetworkRules::Test => 0x207f_ffff,
    }
}

#[document(
    summary = "A compact difficulty as the target it stands for, or None for one Bitcoin refuses: negative, or past 256 bits.",
    category = "Anchoring",
)]
pub fn target(bits: u32) -> Option<u256> {
    let (size, mantissa) = ((bits >> 24) as usize, bits & 0x007f_ffff);

    if bits & 0x0080_0000 != 0 || (mantissa != 0 && size > 34) {
        return None;
    }

    let mut bytes = [0u8; 32];

    for (index, byte) in mantissa.to_le_bytes()[..3].iter().enumerate() {
        match (size + index).checked_sub(3) {
            Some(at) if at < 32 => bytes[at] = *byte,
            Some(_) if *byte != 0 => return None,
            _ => {}
        }
    }

    Some(u256::from_le_bytes(bytes))
}

#[document(
    summary = "A target as compact difficulty, as Bitcoin rounds it.",
    category = "Anchoring",
)]
pub fn compact_bits(target: u256) -> u32 {
    let bytes = target.to_le_bytes();
    let size = bytes.iter().rposition(|byte| *byte != 0).map_or(0, |top| top + 1);
    let mantissa = |size: usize| -> u32 { (0..3).fold(0, |mantissa, index| mantissa << 8 | size.checked_sub(1 + index).map_or(0, |at| u32::from(bytes[at]))) };
    let (mut size, mut value) = (size, mantissa(size));

    if value & 0x0080_0000 != 0 {
        value >>= 8;
        size += 1;
    }

    value | (size as u32) << 24
}

#[document(
    summary = "The work a target stands for: about 2^256 / (target + 1).",
    category = "Anchoring",
)]
pub fn work(target: u256) -> u256 {
    (u256::MAX - target).checked_div(target + u256::ONE).unwrap_or(u256::ZERO) + u256::ONE
}

/// The difficulty after a period, as Bitcoin retargets.
fn retarget(bits: u32, first: u32, last: u32, network: BtcNetworkRules) -> Option<u32> {
    let span = u64::from(last.saturating_sub(first)).clamp(TIMESPAN / 4, TIMESPAN * 4);
    let next = target(bits)?.checked_mul(u256::from(span))?.checked_div(u256::from(TIMESPAN))?;

    Some(compact_bits(next.min(target(limit(network))?)))
}

/// What the chain keeps of each header.
#[derive(Clone, Debug)]
struct Kept {
    prev: [u8; 32],
    merkle: [u8; 32],
    height: u64,
    work: u256,
    bits: u32,
    time: u32,
    period_start: u32,
    times: Vec<u32>,
}

impl Kept {
    fn median_time(&self) -> u32 {
        let mut sorted = self.times.clone();
        sorted.sort_unstable();

        sorted[sorted.len() / 2]
    }
}

#[document(
    summary = "Bitcoin's headers from a start, every branch any peer showed, checked as Bitcoin's nodes check them, the one with the most work the main chain.",
    category = "Anchoring",
    see_also = "BtcStart, History",
)]
pub struct HeaderChain {
    network: BtcNetworkRules,
    kept: HashMap<[u8; 32], Kept>,
    tip: [u8; 32],
    /// The main chain by height, from the start, rebuilt when the tip moves.
    main: Vec<[u8; 32]>,
    start_height: u64,
}

impl HeaderChain {
    #[document(parent = "HeaderChain", summary = "The chain at its start, which it trusts.", returns = "The chain, or why the start is no header.")]
    pub fn new(start: &BtcStart) -> Result<HeaderChain, String> {
        let header: &[u8; 80] = start.header.as_slice().try_into().map_err(|_| "a header is 80 bytes".to_string())?;
        let (hash, word) = (sha256d(header), |at: usize| u32::from_le_bytes(header[at..at + 4].try_into().expect("4 bytes")));
        let time = word(68);
        let kept = Kept { prev: header[4..36].try_into().expect("32 bytes"), merkle: header[36..68].try_into().expect("32 bytes"), height: start.height, work: u256::ZERO, bits: word(72), time, period_start: time, times: vec![time] };

        Ok(HeaderChain { network: start.network, kept: HashMap::from([(hash, kept)]), tip: hash, main: vec![hash], start_height: start.height })
    }

    #[document(
        parent = "HeaderChain",
        summary = "Takes a header building on one it holds, checked by the network's rules; a branch with more work becomes the main chain.",
        params = { header = "80 bytes.", now = "The node's clock, in seconds since the Unix epoch, which no header may be two hours past." },
        returns = "Whether it was new, or why it breaks a rule.",
    )]
    pub fn add(&mut self, header: &[u8], now: u64) -> Result<bool, String> {
        let header: &[u8; 80] = header.try_into().map_err(|_| "a header is 80 bytes".to_string())?;
        let hash = sha256d(header);

        if self.kept.contains_key(&hash) {
            return Ok(false);
        }

        let word = |at: usize| u32::from_le_bytes(header[at..at + 4].try_into().expect("4 bytes"));
        let (prev_hash, merkle, time, bits): ([u8; 32], [u8; 32], u32, u32) = (header[4..36].try_into().expect("32 bytes"), header[36..68].try_into().expect("32 bytes"), word(68), word(72));
        let prev = self.kept.get(&prev_hash).ok_or("a header that builds on none it holds")?;
        let height = prev.height + 1;
        let claimed = target(bits).ok_or("a difficulty Bitcoin refuses")?;

        if u256::from_le_bytes(hash) > claimed {
            return Err("a header without the work its difficulty asks".to_string());
        }

        if claimed > target(limit(self.network)).expect("the limit is a target") {
            return Err("a difficulty easier than the network allows".to_string());
        }

        let expected = match self.network {
            BtcNetworkRules::Main if height.is_multiple_of(INTERVAL) => Some(retarget(prev.bits, prev.period_start, prev.time, self.network).ok_or("a retarget out of range")?),
            BtcNetworkRules::Main | BtcNetworkRules::Regtest => Some(prev.bits),
            BtcNetworkRules::Test => None,
        };

        if expected.is_some_and(|expected| expected != bits) {
            return Err("a difficulty other than the network's rules give".to_string());
        }

        if time <= prev.median_time() {
            return Err("a time not after the median of the last eleven".to_string());
        }

        if u64::from(time) > now + MOST_AHEAD {
            return Err("a time more than two hours ahead".to_string());
        }

        let mut times = prev.times.clone();
        times.push(time);

        if times.len() > MEDIAN_SPAN {
            times.remove(0);
        }

        let kept = Kept {
            prev: prev_hash,
            merkle,
            height,
            work: prev.work + work(claimed),
            bits,
            time,
            period_start: if height.is_multiple_of(INTERVAL) { time } else { prev.period_start },
            times,
        };
        let better = kept.work > self.kept[&self.tip].work;
        self.kept.insert(hash, kept);

        if better {
            self.follow(hash);
        }

        Ok(true)
    }

    fn follow(&mut self, tip: [u8; 32]) {
        let mut chain = Vec::new();
        let mut at = tip;

        while let Some(kept) = self.kept.get(&at) {
            chain.push(at);

            if kept.height == self.start_height {
                break;
            }

            at = kept.prev;
        }

        chain.reverse();
        self.tip = tip;
        self.main = chain;
    }

    #[document(parent = "HeaderChain", summary = "The main chain's tip: its hash, height and work from the start.")]
    pub fn tip(&self) -> ([u8; 32], u64, u256) {
        let kept = &self.kept[&self.tip];

        (self.tip, kept.height, kept.work)
    }

    #[document(parent = "HeaderChain", summary = "The main chain's block at a height.")]
    pub fn main_at(&self, height: u64) -> Option<[u8; 32]> {
        self.main.get(usize::try_from(height.checked_sub(self.start_height)?).ok()?).copied()
    }

    #[document(parent = "HeaderChain", summary = "How deep a block is in the main chain: 1 for the tip, 0 off it or unknown.")]
    pub fn depth(&self, hash: &[u8; 32]) -> u64 {
        match self.kept.get(hash) {
            Some(kept) if self.main_at(kept.height) == Some(*hash) => self.kept[&self.tip].height - kept.height + 1,
            _ => 0,
        }
    }

    #[document(parent = "HeaderChain", summary = "A block's height and Merkle root, on any branch it holds.")]
    pub fn block(&self, hash: &[u8; 32]) -> Option<(u64, [u8; 32])> {
        self.kept.get(hash).map(|kept| (kept.height, kept.merkle))
    }

    #[document(parent = "HeaderChain", summary = "Whether it holds a block, on any branch.")]
    pub fn holds(&self, hash: &[u8; 32]) -> bool {
        self.kept.contains_key(hash)
    }
}
