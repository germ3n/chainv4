//! Anchoring the chain into other chains (docs/anchoring.md). Anyone writes a finalized checkpoint
//! into a Bitcoin transaction's OP_RETURN, or an Ethereum transaction, then shows it here with
//! Tx::Anchor: in Bitcoin, the transaction, its Merkle path and its block's coinbase, checked
//! against the Bitcoin relay contract's headers, as the Bitcoin gateway checks a deposit. The chain
//! records each anchored checkpoint in one list, whichever chain holds it, whose running root every
//! later anchor writes as well, so the list a peer shows can be checked against those chains
//! themselves; and nodes refuse any block that would replace a checkpoint anchored while the
//! settings enforced it.
//!
//! What a node does with the list on its own, picking the history the other chains saw first, is in
//! sync.rs; Bitcoin's proofs, its relay's records and its headers in bitcoin.rs; Ethereum's proofs,
//! its relay's records and the sync committees a syncing node follows in ethereum.rs.

use {
    crate::{
        crypto::hashable::Hashable,
        db::{Column, DbError, State},
        params::anchoring::{AnchorSettings, AnchorSource},
        structs::{address::Address, hash::Hash, tx::AnchorProof, u256::u256},
    },
    r#macro::document,
    sha2::{Digest, Sha256},
};

pub use crate::params::anchoring::{AnchorEntry, AnchorsAnswer};

#[cfg(feature = "anchoring-btc")]
pub mod bitcoin;
#[cfg(feature = "anchoring-eth")]
pub mod ethereum;
pub mod sync;

#[cfg(feature = "anchoring-btc")]
pub use bitcoin::{Relay, RelayHeader};
#[cfg(feature = "anchoring-eth")]
pub use ethereum::{EthCommittees, EthRelay};

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
    summary = "The anchor list's root after one more entry: SHA-256 of the root before it and the entry's encoding. The empty list's root is all zeros.",
    category = "Anchoring",
    see_also = "AnchorEntry, Payload",
)]
pub fn next_root(prev: &[u8; 32], entry: &AnchorEntry) -> [u8; 32] {
    Sha256::digest([&prev[..], &entry.hashable_bytes_with_tag()].concat()).into()
}

#[document(
    summary = "The root of a list of entries, as next_root folds them from all zeros.",
    category = "Anchoring",
    see_also = "next_root",
)]
pub fn list_root(entries: &[AnchorEntry]) -> [u8; 32] {
    entries.iter().fold([0; 32], |root, entry| next_root(&root, entry))
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
    pub fn proof(&self, index: u64) -> Result<Option<AnchorProof>, DbError> {
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
    fn record(&self, entry: &AnchorEntry, proof: &AnchorProof, height: u64) -> Result<u64, DbError> {
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
    #[document(summary = "The block's time, in milliseconds since the Unix epoch: no Ethereum finality update may be signed after the slot it falls in.")]
    pub time: u64,
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

/// The chain's checkpoint interval, which an anchor's epoch counts in.
#[cfg_attr(not(any(feature = "anchoring-btc", feature = "anchoring-eth")), allow(dead_code))]
fn interval(rules: &ProveRules<'_>) -> Result<u64, ProveError> {
    rules.checkpoint_interval.filter(|interval| *interval > 0).ok_or_else(|| ProveError::Refused("this chain's finality has no checkpoints to anchor".to_string()))
}

/// Checks what an anchor says, whichever chain holds it: this chain's tag, the block at the epoch's
/// checkpoint height on this branch, an epoch not anchored yet, and the list's root now. The
/// checkpoint's block and height.
#[cfg_attr(not(any(feature = "anchoring-btc", feature = "anchoring-eth")), allow(dead_code))]
fn check_payload(state: &State, rules: &ProveRules<'_>, interval: u64, payload: &Payload) -> Result<(Hash, u64), ProveError> {
    let fail = |why: &str| Err(ProveError::Refused(why.to_string()));

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

    // The root of every entry before this one, so the list a peer shows checks against the chain
    // holding it whole: an anchor written while another was proven in between is written again.
    if payload.root != anchors.root()? {
        return fail("the anchor's list root is not the list's root now: another anchor was proven since it was written");
    }

    Ok((block, height))
}

/// What a proof showed: where the anchor sits, and the checkpoint it anchors, by its epoch, block
/// and height.
#[cfg_attr(not(any(feature = "anchoring-btc", feature = "anchoring-eth")), allow(dead_code))]
struct Shown {
    source: AnchorSource,
    epoch: u64,
    block: Hash,
    height: u64,
}

/// What a proof shows, by the chain it is in.
fn check(state: &State, rules: &ProveRules<'_>, proof: &AnchorProof) -> Result<Shown, ProveError> {
    match proof {
        #[cfg(feature = "anchoring-btc")]
        AnchorProof::Bitcoin(proof) => bitcoin::check(state, rules, proof),
        #[cfg(not(feature = "anchoring-btc"))]
        AnchorProof::Bitcoin(_) => {
            let _ = (state, rules);

            Err(ProveError::Refused("this build reads no Bitcoin, so it takes no Bitcoin anchors".to_string()))
        }
        #[cfg(feature = "anchoring-eth")]
        AnchorProof::Ethereum(proof) => ethereum::check(state, rules, proof),
        #[cfg(not(feature = "anchoring-eth"))]
        AnchorProof::Ethereum(_) => {
            let _ = (state, rules);

            Err(ProveError::Refused("this build reads no Ethereum, so it takes no Ethereum anchors".to_string()))
        }
    }
}

#[document(
    summary = "Runs a Tx::Anchor: checks the proof shows this chain's anchor of a checkpoint on this branch, with a root the list held, in a block the other chain holds deep enough, and records it, paying the reward when it is due. A Bitcoin proof is checked against the Bitcoin relay; an Ethereum one against the Ethereum relay and the sync committee that signed its finality update.",
    params = {
        state = "The transaction's layer.",
        rules = "What it runs under.",
        sender = "Who sent it, and earns the reward.",
        proof = "The anchor's transaction and its proof.",
    },
    returns = "What it did, or why it fails.",
    category = "Anchoring",
    see_also = "Tx::Anchor, Anchors",
)]
pub fn prove(state: &State, rules: &ProveRules<'_>, sender: &Address, proof: &AnchorProof) -> Result<Proven, ProveError> {
    let settings = rules.settings;
    let Shown { source, epoch, block, height } = check(state, rules, proof)?;
    let anchors = Anchors::new(state);
    let entry = AnchorEntry { epoch, block, source, enforced: settings.enforce };
    let index = anchors.record(&entry, proof, height)?;
    let due = anchors.rewarded()?.is_none_or(|last| epoch >= last.saturating_add(settings.every));
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
        put_plain(state, REWARDED.to_vec(), &epoch);
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
