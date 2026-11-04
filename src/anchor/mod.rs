//! Anchoring the chain into Bitcoin (docs/anchoring.md). Anyone writes a finalized checkpoint into
//! a Bitcoin transaction's OP_RETURN, then shows it here with Tx::Anchor: the transaction, its
//! Merkle path and its block's coinbase, checked against the Bitcoin relay contract's headers, as
//! the Bitcoin gateway checks a deposit. The chain records each anchored checkpoint in a list whose
//! running root every later anchor writes into Bitcoin as well, so the list a peer shows can be
//! checked against Bitcoin itself; and nodes refuse any block that would replace a checkpoint
//! anchored while the settings enforced it.
//!
//! What a node does with the list on its own, checking Bitcoin's headers by their work and picking
//! the history Bitcoin saw first, is in sync.rs; the header rules in bitcoin.rs.

use {
    crate::{
        crypto::{bitcoin::{sha256, sha256d, tx::Transaction}, hashable::Hashable},
        db::{Column, DbError, State, namespace_key},
        params::anchoring::{AnchorSettings, BtcNetworkRules},
        structs::{address::Address, hash::Hash, tx::{AnchorProof, MAX_ANCHOR_PATH, MAX_ANCHOR_TX}, u256::u256},
    },
    r#macro::document,
};

pub use crate::params::anchoring::{AnchorEntry, AnchorsAnswer};

pub mod bitcoin;
pub mod sync;

#[cfg(test)]
mod tests;

#[document(
    summary = "What an anchor's OP_RETURN starts with: cv4a.",
    category = "Anchoring",
    see_also = "Payload",
)]
pub const MAGIC: [u8; 4] = *b"cv4a";

#[document(
    summary = "An anchor's OP_RETURN data, 80 bytes, the most Bitcoin's nodes relay by default: MAGIC, the chain's tag, the checkpoint's epoch and block, and the anchor list's root.",
    category = "Anchoring",
    see_also = "Payload",
)]
pub const PAYLOAD_LEN: usize = 80;

#[document(
    summary = "What an anchor writes into Bitcoin, as the 80 bytes of an OP_RETURN: MAGIC, the first 4 bytes of the chain's genesis id, the checkpoint's epoch big-endian, its block id, and the root of the anchor list as the chain held it when the anchorer wrote it, all zeros for an empty list.",
    category = "Anchoring",
    see_also = "Tx::Anchor, AnchorEntry, next_root",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Payload {
    #[document(summary = "The chain's tag: its genesis id's first 4 bytes.")]
    pub chain: [u8; 4],
    #[document(summary = "The checkpoint's epoch.")]
    pub epoch: u64,
    #[document(summary = "The checkpoint's block id.")]
    pub block: [u8; 32],
    #[document(summary = "The anchor list's root.")]
    pub root: [u8; 32],
}

impl Payload {
    #[document(parent = "Payload", summary = "Its 80 bytes.")]
    pub fn encode(&self) -> [u8; PAYLOAD_LEN] {
        let mut out = [0u8; PAYLOAD_LEN];
        out[..4].copy_from_slice(&MAGIC);
        out[4..8].copy_from_slice(&self.chain);
        out[8..16].copy_from_slice(&self.epoch.to_be_bytes());
        out[16..48].copy_from_slice(&self.block);
        out[48..].copy_from_slice(&self.root);

        out
    }

    #[document(parent = "Payload", summary = "One from its 80 bytes, or None for bytes that are no anchor's.")]
    pub fn decode(bytes: &[u8]) -> Option<Payload> {
        (bytes.len() == PAYLOAD_LEN && bytes[..4] == MAGIC).then(|| Payload {
            chain: bytes[4..8].try_into().expect("4 bytes"),
            epoch: u64::from_be_bytes(bytes[8..16].try_into().expect("8 bytes")),
            block: bytes[16..48].try_into().expect("32 bytes"),
            root: bytes[48..].try_into().expect("32 bytes"),
        })
    }

    #[document(parent = "Payload", summary = "The output script that carries it: OP_RETURN, OP_PUSHDATA1 80, then its bytes.")]
    pub fn script(&self) -> Vec<u8> {
        [&[0x6a, 0x4c, PAYLOAD_LEN as u8][..], &self.encode()].concat()
    }

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
                [0x6a, 0x4c, len, data @ ..] if usize::from(*len) == PAYLOAD_LEN => Payload::decode(data),
                _ => None,
            })
            .ok_or_else(|| "the transaction carries no anchor: no OP_RETURN of cv4a's 80 bytes".to_string())
    }
}

#[document(
    summary = "A chain's tag in its anchors: the first 4 bytes of its genesis id.",
    category = "Anchoring",
    see_also = "Payload",
)]
pub fn chain_tag(genesis: &Hash) -> [u8; 4] {
    genesis.to_bytes()[..4].try_into().expect("a chain's ids are 32 bytes")
}

#[document(
    summary = "What showed an anchor: Tx::Anchor's proof, kept with the entry so any node can show it to a peer.",
    category = "Anchoring",
    see_also = "AnchorProof, AnchorEntry",
)]
pub type BtcProof = AnchorProof;

impl AnchorProof {
    #[document(parent = "AnchorProof", summary = "Why it could never prove anything, before any header is read: bytes too many or 64, paths too long or of different lengths, or a coinbase that is none.")]
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

        (!is_coinbase(&self.coinbase)).then_some("the coinbase is no coinbase: one input, spending nothing")
    }

    #[document(parent = "AnchorProof", summary = "Whether it leads to a Merkle root, the coinbase's path and the transaction's both.")]
    pub fn leads_to(&self, merkle: &[u8; 32]) -> bool {
        self.problem().is_none() && leads_to(&sha256d(&self.coinbase), 0, &self.coinbase_path, merkle) && leads_to(&sha256d(&self.raw), self.index, &self.path, merkle)
    }
}

/// Whether bytes are a coinbase without its witness: one input, spending nothing. Never 64 bytes.
fn is_coinbase(raw: &[u8]) -> bool {
    raw.len() != 64 && raw.len() >= 41 && raw[4] == 1 && raw[5..37] == [0; 32] && raw[37..41] == [0xff; 4]
}

/// Whether a leaf at a place, by its path, comes to a Merkle root, as btc_relay's includes checks.
fn leads_to(leaf: &[u8; 32], index: u64, path: &[[u8; 32]], root: &[u8; 32]) -> bool {
    let (mut at, mut index) = (*leaf, index);

    for sibling in path {
        at = match index & 1 {
            0 => sha256d(&[at, *sibling].concat()),
            _ => sha256d(&[*sibling, at].concat()),
        };
        index >>= 1;
    }

    index == 0 && at == *root
}

#[document(
    summary = "The anchor list's root after one more entry: SHA-256 of the root before it and the entry's encoding. The empty list's root is all zeros.",
    category = "Anchoring",
    see_also = "AnchorEntry, Payload",
)]
pub fn next_root(prev: &[u8; 32], entry: &AnchorEntry) -> [u8; 32] {
    sha256(&[&prev[..], &entry.hashable_bytes_with_tag()].concat())
}

#[document(
    summary = "The root of a list of entries, as next_root folds them from all zeros.",
    category = "Anchoring",
    see_also = "next_root",
)]
pub fn list_root(entries: &[AnchorEntry]) -> [u8; 32] {
    entries.iter().fold([0; 32], |root, entry| next_root(&root, entry))
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
    summary = "A Bitcoin relay contract's records in a state, read as the chain reads its own: no call into it, so nothing it runs can change what a proof shows.",
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

    #[document(parent = "Relay", summary = "A header it took, by its hash in Bitcoin's byte order.")]
    pub fn header(&self, hash: &[u8; 32]) -> Result<Option<RelayHeader>, DbError> {
        let bytes = self.state.get(Column::Namespaces, &namespace_key(&self.address, b"headers", hash))?;

        Ok(bytes.filter(|bytes| bytes.len() >= 117).map(|bytes| RelayHeader {
            merkle: bytes[32..64].try_into().expect("32 bytes"),
            height: u64::from_le_bytes(bytes[64..72].try_into().expect("8 bytes")),
        }))
    }

    #[document(parent = "Relay", summary = "A header it took, written whole again, 80 bytes, from what it keeps: None for one it does not hold, or keeps without its version and nonce, as relays from before anchoring did.")]
    pub fn raw_header(&self, hash: &[u8; 32]) -> Result<Option<[u8; 80]>, DbError> {
        let Some(bytes) = self.state.get(Column::Namespaces, &namespace_key(&self.address, b"headers", hash))? else {
            return Ok(None);
        };
        let Some(count) = bytes.get(116).map(|count| usize::from(*count)) else {
            return Ok(None);
        };
        let Some(tail) = bytes.get(117 + 4 * count..125 + 4 * count) else {
            return Ok(None);
        };
        let mut header = [0u8; 80];
        header[..4].copy_from_slice(&tail[..4]);
        header[4..68].copy_from_slice(&bytes[..64]);
        header[68..72].copy_from_slice(&bytes[108..112]);
        header[72..76].copy_from_slice(&bytes[104..108]);
        header[76..].copy_from_slice(&tail[4..]);

        Ok((sha256d(&header) == *hash).then_some(header))
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

/// Keys in Column::Validators, beside governance's own.
const COUNT: &[u8] = b"anchor:count";
const ENTRY: &[u8] = b"anchor:i:";
const ROOT: &[u8] = b"anchor:r:";
const PROOF: &[u8] = b"anchor:p:";
const BY_EPOCH: &[u8] = b"anchor:e:";
const HEIGHT: &[u8] = b"anchor:height";
const REWARDED: &[u8] = b"anchor:rewarded";

fn keyed(prefix: &[u8], key: &[u8]) -> Vec<u8> {
    [prefix, key].concat()
}

/// Stores a plain value, a number or a root, as wincode writes it.
fn put_plain<T: wincode::SchemaWrite<wincode::config::DefaultConfig, Src = T>>(state: &State, key: Vec<u8>, value: &T) {
    state.put(Column::Validators, key, wincode::serialize(value).expect("a plain value always encodes"));
}

#[document(
    summary = "The anchor list in a state: each anchored checkpoint in the order the chain recorded them, the running root after each, the proof that showed each, and the highest checkpoint an enforced one holds. Chain state, kept in Column::Validators.",
    category = "Anchoring",
    see_also = "AnchorEntry, Tx::Anchor, next_root",
)]
pub struct Anchors<'a> {
    state: &'a State,
}

impl<'a> Anchors<'a> {
    #[document(parent = "Anchors", summary = "The list in a state.")]
    pub fn new(state: &'a State) -> Anchors<'a> {
        Anchors { state }
    }

    #[document(parent = "Anchors", summary = "How many anchors it holds.")]
    pub fn count(&self) -> Result<u64, DbError> {
        Ok(self.state.get_value(Column::Validators, COUNT)?.unwrap_or(0))
    }

    #[document(parent = "Anchors", summary = "The entry at a place, counted from 0.")]
    pub fn entry(&self, index: u64) -> Result<Option<AnchorEntry>, DbError> {
        self.state.get_value(Column::Validators, &keyed(ENTRY, &index.to_be_bytes()))
    }

    #[document(parent = "Anchors", summary = "Entries from a place on, at most limit of them.")]
    pub fn entries(&self, from: u64, limit: u64) -> Result<Vec<AnchorEntry>, DbError> {
        let end = self.count()?.min(from.saturating_add(limit));

        (from..end).map(|index| self.entry(index)?.ok_or_else(|| DbError::Corrupt(format!("anchor {index} is missing")))).collect()
    }

    #[document(parent = "Anchors", summary = "The root after the entry at a place.")]
    pub fn root_at(&self, index: u64) -> Result<Option<[u8; 32]>, DbError> {
        self.state.get_value(Column::Validators, &keyed(ROOT, &index.to_be_bytes()))
    }

    #[document(parent = "Anchors", summary = "The list's root now: all zeros while it is empty.")]
    pub fn root(&self) -> Result<[u8; 32], DbError> {
        match self.count()? {
            0 => Ok([0; 32]),
            count => self.root_at(count - 1)?.ok_or_else(|| DbError::Corrupt("the anchor list's root is missing".to_string())),
        }
    }

    #[document(parent = "Anchors", summary = "The proof that showed the entry at a place.")]
    pub fn proof(&self, index: u64) -> Result<Option<BtcProof>, DbError> {
        self.state.get_value(Column::Validators, &keyed(PROOF, &index.to_be_bytes()))
    }

    #[document(parent = "Anchors", summary = "Where an epoch's checkpoint is in the list, if anchored.")]
    pub fn by_epoch(&self, epoch: u64) -> Result<Option<u64>, DbError> {
        self.state.get_value(Column::Validators, &keyed(BY_EPOCH, &epoch.to_be_bytes()))
    }

    #[document(parent = "Anchors", summary = "The highest checkpoint an enforced anchor holds, by height: no block at or under it may replace the chain's.")]
    pub fn enforced_height(&self) -> Result<Option<u64>, DbError> {
        self.state.get_value(Column::Validators, HEIGHT)
    }

    #[document(parent = "Anchors", summary = "The last epoch whose anchor earned the reward.")]
    pub fn rewarded(&self) -> Result<Option<u64>, DbError> {
        self.state.get_value(Column::Validators, REWARDED)
    }

    /// Appends an entry with its proof, at its checkpoint's height.
    fn record(&self, entry: &AnchorEntry, proof: &BtcProof, height: u64) -> Result<u64, DbError> {
        let index = self.count()?;
        let root = next_root(&self.root()?, entry);
        let at = index.to_be_bytes();

        self.state.put_value(Column::Validators, keyed(ENTRY, &at), entry);
        put_plain(self.state, keyed(ROOT, &at), &root);
        self.state.put_value(Column::Validators, keyed(PROOF, &at), proof);
        put_plain(self.state, keyed(BY_EPOCH, &entry.epoch.to_be_bytes()), &index);
        put_plain(self.state, COUNT.to_vec(), &(index + 1));

        if entry.enforced && self.enforced_height()?.is_none_or(|highest| height > highest) {
            put_plain(self.state, HEIGHT.to_vec(), &height);
        }

        Ok(index)
    }
}

#[document(
    summary = "What one Tx::Anchor needs to know of the chain: its settings, its genesis id, its checkpoint interval and where its reward comes from.",
    category = "Anchoring",
    see_also = "prove",
)]
pub struct ProveRules<'a> {
    #[document(summary = "The anchoring settings in force.")]
    pub settings: &'a AnchorSettings,
    #[document(summary = "The chain's genesis id.")]
    pub genesis: Hash,
    #[document(summary = "Blocks per epoch, or None on a chain whose finality has no checkpoints.")]
    pub checkpoint_interval: Option<u64>,
    #[document(summary = "Whether rewards come from REWARD_POOL rather than being minted.")]
    pub pooled: bool,
}

#[document(
    summary = "What a Tx::Anchor that ran did: where its entry went, and what it earned the sender.",
    category = "Anchoring",
    see_also = "prove",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Proven {
    #[document(summary = "The entry's place in the list.")]
    pub index: u64,
    #[document(summary = "What the sender earned.")]
    pub reward: u256,
}

#[document(
    summary = "Runs a Tx::Anchor: checks the Bitcoin transaction carries this chain's anchor of a checkpoint on this branch, with a root the list held, in a block the relay holds deep enough, and records it, paying the reward when it is due.",
    params = {
        state = "The transaction's layer.",
        rules = "What it runs under.",
        sender = "Who sent it, and earns the reward.",
        proof = "The Bitcoin transaction and its proof.",
    },
    returns = "What it did, or why it fails.",
    category = "Anchoring",
    see_also = "Tx::Anchor, Anchors",
)]
pub fn prove(state: &State, rules: &ProveRules<'_>, sender: &Address, proof: &BtcProof) -> Result<Proven, ProveError> {
    let settings = rules.settings;
    let fail = |why: &str| Err(ProveError::Refused(why.to_string()));

    if !settings.bitcoin {
        return fail("anchoring to Bitcoin is off on this chain");
    }

    let (Some(relay), Some(start)) = (settings.relay, &settings.btc_start) else {
        return fail("anchoring has no relay or start");
    };
    let Some(interval) = rules.checkpoint_interval.filter(|interval| *interval > 0) else {
        return fail("this chain's finality has no checkpoints to anchor");
    };

    if let Some(problem) = proof.problem() {
        return fail(problem);
    }

    let relay = Relay::new(state, relay);

    if relay.network()? != Some(start.network) || relay.header(&sha256d(&start.header))?.map(|header| header.height) != Some(start.height) {
        return fail("the relay did not start where anchoring's btc_start says");
    }

    let payload = Payload::find(&proof.raw).map_err(ProveError::Refused)?;

    if payload.chain != chain_tag(&rules.genesis) {
        return fail("the anchor is another chain's");
    }

    let height = payload.epoch.checked_mul(interval).ok_or_else(|| ProveError::Refused("an epoch past the chain's heights".to_string()))?;
    let block = Hash::from_bytes(rules.genesis.algorithm(), &payload.block).expect("a chain's ids are 32 bytes");

    if state.get_value::<Hash>(Column::Heights, &height.to_be_bytes())? != Some(block) {
        return fail("the anchored block is not this branch's checkpoint at that epoch");
    }

    let anchors = Anchors::new(state);

    if anchors.by_epoch(payload.epoch)?.is_some() {
        return fail("that epoch's checkpoint is anchored already");
    }

    // The root of every entry before this one, so the list a peer shows checks against Bitcoin
    // whole: an anchor written while another was proven in between is written again.
    if payload.root != anchors.root()? {
        return fail("the anchor's list root is not the list's root now: another anchor was proven since it was written");
    }

    let Some(header) = relay.header(&proof.block)? else {
        return fail("the relay holds no such Bitcoin block");
    };

    if relay.depth(&proof.block)? < settings.depth {
        return fail("the Bitcoin block is not yet as deep as anchoring.depth, or off the relay's main chain");
    }

    if !proof.leads_to(&header.merkle) {
        return fail("the Merkle paths do not lead to the block's root");
    }

    let entry = AnchorEntry { epoch: payload.epoch, block, btc_block: proof.block, btc_height: header.height, enforced: settings.enforce };
    let index = anchors.record(&entry, proof, height)?;
    let due = anchors.rewarded()?.is_none_or(|last| payload.epoch >= last.saturating_add(settings.every));
    let mut reward = u256::ZERO;

    if due && settings.reward > u256::ZERO {
        let accounts = crate::registry::accounts::Accounts::new(state);

        reward = match rules.pooled {
            true => {
                let pool = accounts.get(&crate::chain::REWARD_POOL)?.balance;
                let reward = settings.reward.min(pool);
                accounts.debit(&crate::chain::REWARD_POOL, reward).map_err(ProveError::from_registry)?;

                reward
            }
            false => {
                crate::registry::supply::Supply::mint(state, settings.reward)?;

                settings.reward
            }
        };
        accounts.credit(sender, reward).map_err(ProveError::from_registry)?;
    }

    if due {
        put_plain(state, REWARDED.to_vec(), &payload.epoch);
    }

    Ok(Proven { index, reward })
}

#[document(
    summary = "Why a Tx::Anchor failed: the proof does not show what it must, or the node could not read its state.",
    category = "Anchoring",
    see_also = "prove",
)]
#[derive(Debug)]
pub enum ProveError {
    #[document(summary = "What the proof fails to show.")]
    Refused(String),
    #[document(summary = "The database failed.")]
    Db(DbError),
}

impl ProveError {
    fn from_registry(err: crate::registry::RegistryError) -> ProveError {
        match err {
            crate::registry::RegistryError::Db(err) => ProveError::Db(err),
            err => ProveError::Refused(err.to_string()),
        }
    }
}

impl From<DbError> for ProveError {
    fn from(err: DbError) -> ProveError {
        ProveError::Db(err)
    }
}

impl std::fmt::Display for ProveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProveError::Refused(why) => write!(f, "{why}"),
            ProveError::Db(err) => write!(f, "{err}"),
        }
    }
}
