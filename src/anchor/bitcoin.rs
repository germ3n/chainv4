//! Anchors in Bitcoin. A Bitcoin anchor's proof is checked against the Bitcoin relay contract's
//! records, read natively; and anchored sync checks Bitcoin's headers on its own, as
//! contracts/examples/btc_relay checks them on chain, by the rules both take from btclight: proof
//! of work, the retarget every 2016 blocks on main, the median time past, and the most work wins.
//! Anchored sync feeds it every peer's relay headers from anchoring's btc_start, so the Bitcoin
//! chain it compares anchors on is the one with the most work any peer showed, whatever a fork's
//! relay claims.

use {
    super::{Payload, ProveError, ProveRules, Shown},
    crate::{
        crypto::bitcoin::{sha256d, tx::Transaction},
        db::{Column, DbError, State, namespace_key},
        params::anchoring::{AnchorSource, BtcNetworkRules, BtcStart},
        structs::{address::Address, tx::{BtcAnchorProof, MAX_ANCHOR_PATH, MAX_ANCHOR_TX}, u256::u256},
    },
    btclight::{Header, Kept, MainChain},
    r#macro::document,
    std::collections::{BTreeMap, HashMap},
};

#[cfg(test)]
mod tests;

/// SHA-256 twice, natively, for btclight's rules.
struct Native;

impl btclight::Crypto for Native {
    fn sha256d(&self, data: &[u8]) -> [u8; 32] {
        sha256d(data)
    }
}

/// The rules a network's headers are checked by.
fn rules(network: BtcNetworkRules) -> btclight::Network {
    match network {
        BtcNetworkRules::Main => btclight::Network::Main,
        BtcNetworkRules::Regtest => btclight::Network::Regtest,
        BtcNetworkRules::Test => btclight::Network::Test,
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
    /// The main chain by height, from the start, moved when the tip moves.
    main: BTreeMap<u64, [u8; 32]>,
}

impl MainChain for HeaderChain {
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

impl HeaderChain {
    #[document(parent = "HeaderChain", summary = "The chain at its start, which it trusts.", returns = "The chain, or why the start is no header.")]
    pub fn new(start: &BtcStart) -> Result<HeaderChain, String> {
        let header = Header::read(&start.header).ok_or("a header is 80 bytes")?;
        let hash = sha256d(&start.header);

        Ok(HeaderChain { network: start.network, kept: HashMap::from([(hash, Kept::start(&header, start.height))]), tip: hash, main: BTreeMap::from([(start.height, hash)]) })
    }

    #[document(
        parent = "HeaderChain",
        summary = "Takes a header building on one it holds, checked by the network's rules; a branch with more work becomes the main chain.",
        params = { header = "80 bytes.", now = "The node's clock, in seconds since the Unix epoch, which no header may be two hours past." },
        returns = "Whether it was new, or why it breaks a rule.",
    )]
    pub fn add(&mut self, header: &[u8], now: u64) -> Result<bool, String> {
        let parsed = Header::read(header).ok_or("a header is 80 bytes")?;
        let hash = sha256d(header);

        if self.kept.contains_key(&hash) {
            return Ok(false);
        }

        let prev = self.kept.get(&parsed.prev).ok_or("a header that builds on none it holds")?;
        let kept = prev.child(&parsed, &hash, rules(self.network), now).map_err(|err| err.message().to_string())?;
        let best = &self.kept[&self.tip];

        if kept.work > best.work {
            let old_height = best.height;
            btclight::follow(self, &hash, kept.height, kept.prev, old_height);
            self.tip = hash;
        }

        self.kept.insert(hash, kept);

        Ok(true)
    }

    #[document(parent = "HeaderChain", summary = "The main chain's tip: its hash, height and work from the start.")]
    pub fn tip(&self) -> ([u8; 32], u64, u256) {
        let kept = &self.kept[&self.tip];

        (self.tip, kept.height, kept.work)
    }

    #[document(parent = "HeaderChain", summary = "The main chain's block at a height.")]
    pub fn main_at(&self, height: u64) -> Option<[u8; 32]> {
        MainChain::main_at(self, height)
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

impl BtcAnchorProof {
    #[document(parent = "BtcAnchorProof", summary = "Why it could never prove anything, before any header is read: bytes too many or 64, paths too long or of different lengths, or a coinbase that is none.")]
    pub fn problem(&self) -> Option<&'static str> {
        if self.raw.len() > MAX_ANCHOR_TX || self.coinbase.len() > MAX_ANCHOR_TX {
            return Some("an anchor's transaction and coinbase are at most 100,000 bytes each");
        }

        if self.raw.len() == 64 {
            return Some("a transaction of 64 bytes, which a Merkle tree's inner node could pass for");
        }

        if self.path.len() > MAX_ANCHOR_PATH || self.path.len() != self.coinbase_path.len() {
            return Some("the paths are as long as each other, at most 32");
        }

        (!btclight::is_coinbase(&self.coinbase)).then_some("the coinbase is no coinbase: one input, spending nothing")
    }

    #[document(parent = "BtcAnchorProof", summary = "Whether it leads to a Merkle root, the coinbase's path and the transaction's both, as the relay's own includes checks (CVE-2017-12842).")]
    pub fn leads_to(&self, merkle: &[u8; 32]) -> bool {
        self.problem().is_none() && btclight::includes(&Native, merkle, &sha256d(&self.raw), self.index, &self.path, &self.coinbase, &self.coinbase_path)
    }
}

impl Payload {
    #[document(
        parent = "Payload",
        summary = "The anchor a Bitcoin transaction carries: the first output whose script is one. The transaction comes without its witnesses, as its id hashes it.",
        returns = "The payload, or why the bytes hold none.",
    )]
    pub fn find(raw: &[u8]) -> Result<Payload, String> {
        let (tx, witnessed) = Transaction::read(raw)?;

        if witnessed {
            return Err("an anchor's transaction comes without its witnesses, as its id hashes it".to_string());
        }

        tx.outputs
            .iter()
            .find_map(|output| match output.script.as_slice() {
                [0x6a, 0x4c, len, data @ ..] if usize::from(*len) == super::PAYLOAD_LEN => Payload::decode(data),
                _ => None,
            })
            .ok_or_else(|| "the transaction carries no anchor: no OP_RETURN of cv4a's 80 bytes".to_string())
    }
}

#[document(
    summary = "What the Bitcoin relay keeps of one header, read from its namespaces as contracts/examples/btc_relay lays it out.",
    category = "Anchoring",
    see_also = "Relay",
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelayHeader {
    #[document(summary = "The header's Merkle root.")]
    pub merkle: [u8; 32],
    #[document(summary = "Its height.")]
    pub height: u64,
}

#[document(
    summary = "A Bitcoin relay contract's records in a state, read as the chain reads its own: no call into it, so nothing it runs can change what a proof shows. Each header's record decodes as btclight's Kept, the layout the relay stores.",
    category = "Anchoring",
    see_also = "RelayHeader, AnchorSettings::relay",
)]
pub struct Relay<'a> {
    state: &'a State,
    address: Address,
}

impl<'a> Relay<'a> {
    #[document(parent = "Relay", summary = "The relay at an address.")]
    pub fn new(state: &'a State, address: Address) -> Relay<'a> {
        Relay { state, address }
    }

    fn own(&self, key: &[u8]) -> Result<Option<Vec<u8>>, DbError> {
        self.state.get(Column::Namespaces, &namespace_key(&self.address, &[], key))
    }

    fn kept(&self, hash: &[u8; 32]) -> Result<Option<Kept>, DbError> {
        Ok(self.state.get(Column::Namespaces, &namespace_key(&self.address, b"headers", hash))?.and_then(|bytes| Kept::decode(&bytes)))
    }

    #[document(parent = "Relay", summary = "A header it took, by its hash in Bitcoin's byte order.")]
    pub fn header(&self, hash: &[u8; 32]) -> Result<Option<RelayHeader>, DbError> {
        Ok(self.kept(hash)?.map(|kept| RelayHeader { merkle: kept.merkle, height: kept.height }))
    }

    #[document(parent = "Relay", summary = "A header it took, written whole again, 80 bytes, from what it keeps: None for one it does not hold, or whose record does not write the header it is kept under.")]
    pub fn raw_header(&self, hash: &[u8; 32]) -> Result<Option<[u8; 80]>, DbError> {
        Ok(self.kept(hash)?.map(|kept| kept.header().encode()).filter(|header| sha256d(header) == *hash))
    }

    #[document(parent = "Relay", summary = "Its main chain's block at a height.")]
    pub fn main_at(&self, height: u64) -> Result<Option<[u8; 32]>, DbError> {
        let bytes = self.state.get(Column::Namespaces, &namespace_key(&self.address, b"main", &height.to_be_bytes()))?;

        Ok(bytes.and_then(|bytes| bytes.try_into().ok()))
    }

    #[document(parent = "Relay", summary = "Its main chain's tip: its hash and height.")]
    pub fn tip(&self) -> Result<Option<([u8; 32], u64)>, DbError> {
        let Some(hash) = self.own(b"tip")?.and_then(|bytes| <[u8; 32]>::try_from(bytes).ok()) else {
            return Ok(None);
        };

        Ok(self.header(&hash)?.map(|header| (hash, header.height)))
    }

    #[document(parent = "Relay", summary = "The network it was started on.")]
    pub fn network(&self) -> Result<Option<BtcNetworkRules>, DbError> {
        Ok(self.own(b"network")?.and_then(|bytes| BtcNetworkRules::from_name(std::str::from_utf8(&bytes).ok()?)))
    }

    #[document(parent = "Relay", summary = "How deep a block is in its main chain: 1 for the tip, 0 off it or unknown.")]
    pub fn depth(&self, hash: &[u8; 32]) -> Result<u64, DbError> {
        let (Some(header), Some((_, tip))) = (self.header(hash)?, self.tip()?) else {
            return Ok(0);
        };

        Ok(match self.main_at(header.height)? == Some(*hash) {
            true => tip - header.height + 1,
            false => 0,
        })
    }
}

/// Checks a Bitcoin anchor's proof: Bitcoin anchoring on and set up, the relay the settings name,
/// this chain's anchor of a checkpoint on this branch with the list's root now, in a block the
/// relay holds deep enough, by Merkle paths that lead there.
pub(super) fn check(state: &State, rules: &ProveRules<'_>, proof: &BtcAnchorProof) -> Result<Shown, ProveError> {
    let settings = rules.settings;
    let fail = |why: &str| Err(ProveError::Refused(why.to_string()));

    if !settings.bitcoin {
        return fail("anchoring to Bitcoin is off on this chain");
    }

    let (Some(relay), Some(start)) = (settings.relay, &settings.btc_start) else {
        return fail("anchoring has no relay or start");
    };
    let interval = super::interval(rules)?;

    if let Some(problem) = proof.problem() {
        return fail(problem);
    }

    let relay = Relay::new(state, relay);

    if relay.network()? != Some(start.network) || relay.header(&sha256d(&start.header))?.map(|header| header.height) != Some(start.height) {
        return fail("the relay did not start where anchoring's btc_start says");
    }

    let payload = Payload::find(&proof.raw).map_err(ProveError::Refused)?;
    let (block, height) = super::check_payload(state, rules, interval, &payload)?;

    let Some(found) = relay.header(&proof.block)? else {
        return fail("the relay holds no such Bitcoin block");
    };

    if relay.depth(&proof.block)? < settings.depth {
        return fail("the Bitcoin block is not yet as deep as anchoring.depth, or off the relay's main chain");
    }

    if !proof.leads_to(&found.merkle) {
        return fail("the Merkle paths do not lead to the block's root");
    }

    Ok(Shown { source: AnchorSource::Bitcoin { block: proof.block, height: found.height }, epoch: payload.epoch, block, height })
}

/// Checks a Bitcoin anchor's proof the way anchored sync does, against headers rather than the
/// relay: the block in the headers shown, the one the entry names, the paths leading to its root,
/// and as deep as asked in the main chain. Its height.
pub(super) fn check_shown(proof: &BtcAnchorProof, source: &AnchorSource, headers: &HeaderChain, index: usize, depth: u64) -> Result<u64, String> {
    let Some((height, merkle)) = headers.block(&proof.block) else {
        return Err(format!("anchor {index}'s Bitcoin block is in no header shown"));
    };

    if *source != (AnchorSource::Bitcoin { block: proof.block, height }) {
        return Err(format!("anchor {index}'s proof is of another Bitcoin block than its entry"));
    }

    if !proof.leads_to(&merkle) {
        return Err(format!("anchor {index}'s Merkle paths do not lead to its block's root"));
    }

    if headers.depth(&proof.block) < depth {
        return Err(format!("anchor {index}'s Bitcoin block is off the main chain, or not {depth} deep"));
    }

    Ok(height)
}
