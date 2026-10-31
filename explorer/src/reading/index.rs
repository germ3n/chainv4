//! What the explorer keeps of the blocks it read: each block but for its state entries and their
//! proofs, which are most of what a block weighs and which nothing here reads. So a block keeps its
//! header and seal, its transactions and receipts, with the events and payments in them, and its
//! finality proofs, which is what a block's page, a transaction's and an address's show, and all that
//! recomputes the block's roots, so a block read back from a device is checked again.
//!
//! It is bounded by bytes, as encoded: past the cap, the oldest go, a bucket of heights at a time, and
//! whoever keeps the reading's place moves the height it knows from up to say so.

use {
    chainv4::light::{BlockRange, CheckedItem, NoProofs, RangeBody, RangeChanges, RangeItem, check_range_item, decode_block_range},
    chainv4::structs::address::Address,
    chainv4::structs::hash::{Hash, HashType},
    chainv4_ui::blocks::touched::{mentioned, parties},
    std::collections::{BTreeMap, BTreeSet, HashMap},
};

/// How many heights go in a bucket: what is written and let go of at a time. About a quarter of a
/// megabyte of empty blocks, so what is being read writes little each time, and few files are read
/// on opening.
pub const BUCKET: u64 = 1_024;

/// A transaction's place: its block's height, and where in it.
pub type TxPlace = (u64, u32);
/// An event's place: its transaction's, and where in its receipt.
pub type EventPlace = (u64, u32, u32);

/// What an event is looked up by.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum EventKey {
    Contract(Address),
    /// Named in what it holds.
    Mentions(Address),
    /// An address's 20 bytes in an EVM log's topics.
    MentionsEvm([u8; 20]),
    Topic([u8; 32]),
}

/// The blocks read, and what finds things in them.
#[derive(Default)]
pub struct Index {
    blocks: BTreeMap<u64, RangeItem>,
    ids: HashMap<Hash, u64>,
    txs: HashMap<Hash, TxPlace>,
    /// Who each transaction names or paid.
    parties: HashMap<Address, BTreeSet<TxPlace>>,
    events: HashMap<EventKey, BTreeSet<EventPlace>>,
    /// How many bytes each bucket comes to, as encoded.
    buckets: BTreeMap<u64, u64>,
    bytes: u64,
    /// How many blocks came without their receipts, or with their header alone.
    no_receipts: u64,
}

/// The bucket a height goes in.
pub fn bucket_of(height: u64) -> u64 {
    height / BUCKET
}

/// A block with its state entries left out, which is all that is kept of it, or None for the part of
/// a block that only goes on through its entries, and so holds nothing to keep.
pub fn kept_of(item: &CheckedItem) -> Option<RangeItem> {
    (!matches!(item.body, RangeBody::Continued)).then(|| RangeItem { sealed: item.sealed.clone(), body: item.body.clone(), changes: RangeChanges::None(NoProofs::NoList) })
}

/// What a block weighs, as it is kept.
pub fn weight(item: &RangeItem) -> u64 {
    wincode::serialized_size(item).unwrap_or(0)
}

/// Blocks as they are kept on a shelf, in the encoding a node sends them in.
pub fn encode(items: Vec<RangeItem>) -> Result<Vec<u8>, String> {
    wincode::serialize(&BlockRange { tip: 0, items, next: None }).map_err(|why| format!("could not write blocks: {why}"))
}

/// Blocks as they came back from a shelf, once each one's body has been checked against its own
/// header and each header links to the next: bytes on a device are read as a node's are, on any bytes.
/// `above` is the height and parent of the lowest block already read back, for the highest here to
/// link up to.
pub fn decode(bytes: &[u8], hash: HashType, above: Option<&(u64, String)>) -> Result<Vec<RangeItem>, String> {
    let range = decode_block_range(bytes).map_err(|why| why.to_string())?;
    let mut items = Vec::with_capacity(range.items.len());

    for item in range.items {
        let sealed = item.sealed.clone();

        items.extend(kept_of(&check_range_item(hash, &sealed, item).map_err(|why| why.to_string())?));
    }

    for pair in items.windows(2) {
        if pair[0].sealed.header.height + 1 == pair[1].sealed.header.height && pair[1].sealed.header.parent != pair[0].sealed.id() {
            return Err("blocks that do not link up".to_string());
        }
    }

    if let (Some((height, parent)), Some(top)) = (above, items.last())
        && top.sealed.header.height + 1 == *height
        && top.sealed.id().to_string() != *parent
    {
        return Err("blocks that do not link up to those above them".to_string());
    }

    Ok(items)
}

impl Index {
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    pub fn no_receipts(&self) -> u64 {
        self.no_receipts
    }

    /// How many blocks it holds.
    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    pub fn lowest(&self) -> Option<u64> {
        self.blocks.keys().next().copied()
    }

    pub fn highest(&self) -> Option<u64> {
        self.blocks.keys().next_back().copied()
    }

    pub fn at(&self, height: u64) -> Option<&RangeItem> {
        self.blocks.get(&height)
    }

    /// The height of a block by its id.
    pub fn height_of(&self, id: &Hash) -> Option<u64> {
        self.ids.get(id).copied()
    }

    /// Where a transaction is, by its id.
    pub fn tx_at(&self, id: &Hash) -> Option<TxPlace> {
        self.txs.get(id).copied()
    }

    /// The newest blocks at or below this height, newest first, as many as asked for.
    pub fn newest_below(&self, before: Option<u64>, count: usize) -> impl Iterator<Item = &RangeItem> {
        self.blocks.range(..before.unwrap_or(u64::MAX)).rev().take(count).map(|(_, item)| item)
    }

    /// The transactions that name or paid an address, newest first, below a place if any.
    pub fn txs_of(&self, address: &Address, below: Option<TxPlace>) -> impl Iterator<Item = TxPlace> + '_ {
        self.parties.get(address).into_iter().flat_map(move |places| places.range(..below.unwrap_or((u64::MAX, u32::MAX))).rev().copied())
    }

    /// The events under a key, newest first, below a place if any.
    pub fn events_of(&self, key: &EventKey, below: Option<EventPlace>) -> impl Iterator<Item = EventPlace> + '_ {
        self.events.get(key).into_iter().flat_map(move |places| places.range(..below.unwrap_or((u64::MAX, u32::MAX, u32::MAX))).rev().copied())
    }

    /// Takes a block in, in place of any it held at that height, and says the bucket it went in.
    pub fn take(&mut self, item: RangeItem, hash: HashType) -> u64 {
        let height = item.sealed.header.height;
        let bucket = bucket_of(height);
        self.remove(height, hash);
        let size = weight(&item);
        *self.buckets.entry(bucket).or_default() += size;
        self.bytes += size;
        self.ids.insert(item.sealed.id(), height);
        self.no_receipts += u64::from(!matches!(&item.body, RangeBody::Whole { receipts: Some(_), .. }));

        if let RangeBody::Whole { txs, receipts, .. } = &item.body {
            let receipts = receipts.as_deref();

            for (at, signed) in txs.iter().enumerate() {
                let place = (height, at as u32);
                let receipt = receipts.and_then(|receipts| receipts.get(at));
                self.txs.insert(receipt.map_or_else(|| signed.id(hash), |receipt| receipt.tx), place);

                for party in parties(signed, receipt) {
                    self.parties.entry(party).or_default().insert(place);
                }

                for (position, event) in receipt.into_iter().flat_map(|receipt| &receipt.events).enumerate() {
                    let found = (height, at as u32, position as u32);

                    for key in keys_of(event) {
                        self.events.entry(key).or_default().insert(found);
                    }
                }
            }
        }

        self.blocks.insert(height, item);

        bucket
    }

    /// Lets go of a block, and everything that finds it.
    fn remove(&mut self, height: u64, hash: HashType) {
        let Some(item) = self.blocks.remove(&height) else {
            return;
        };
        let size = weight(&item);
        let bucket = bucket_of(height);
        self.bytes = self.bytes.saturating_sub(size);
        self.no_receipts = self.no_receipts.saturating_sub(u64::from(!matches!(&item.body, RangeBody::Whole { receipts: Some(_), .. })));
        self.ids.remove(&item.sealed.id());

        if let Some(left) = self.buckets.get_mut(&bucket) {
            *left = left.saturating_sub(size);
        }

        if !self.blocks.range(bucket * BUCKET..(bucket + 1).saturating_mul(BUCKET)).any(|_| true) {
            self.buckets.remove(&bucket);
        }

        if let RangeBody::Whole { txs, receipts, .. } = &item.body {
            let receipts = receipts.as_deref();

            for (at, signed) in txs.iter().enumerate() {
                let place = (height, at as u32);
                let receipt = receipts.and_then(|receipts| receipts.get(at));
                self.txs.remove(&receipt.map_or_else(|| signed.id(hash), |receipt| receipt.tx));

                for party in parties(signed, receipt) {
                    drop_place(&mut self.parties, &party, &place);
                }

                for (position, event) in receipt.into_iter().flat_map(|receipt| &receipt.events).enumerate() {
                    let found = (height, at as u32, position as u32);

                    for key in keys_of(event) {
                        drop_place(&mut self.events, &key, &found);
                    }
                }
            }
        }
    }

    /// Lets go of the oldest bucket, if another is left to hold what came last, and says which it was.
    pub fn drop_oldest_bucket(&mut self, hash: HashType) -> Option<u64> {
        if self.buckets.len() < 2 {
            return None;
        }

        let (&bucket, _) = self.buckets.iter().next()?;
        let heights: Vec<u64> = self.blocks.range(..(bucket + 1).saturating_mul(BUCKET)).map(|(height, _)| *height).collect();

        for height in heights {
            self.remove(height, hash);
        }

        Some(bucket)
    }

    /// The blocks of a bucket, as they are written.
    pub fn bucket_bytes(&self, bucket: u64) -> Result<Vec<u8>, String> {
        encode(self.blocks.range(bucket * BUCKET..(bucket + 1).saturating_mul(BUCKET)).map(|(_, item)| item.clone()).collect())
    }
}

/// Lets go of a place under a key, and of the key once nothing is left under it, so what finds things
/// shrinks with what it finds.
fn drop_place<K: Eq + std::hash::Hash + Clone, P: Ord>(map: &mut HashMap<K, BTreeSet<P>>, key: &K, place: &P) {
    if let Some(places) = map.get_mut(key) {
        places.remove(place);

        if places.is_empty() {
            map.remove(key);
        }
    }
}

/// What an event is found by.
fn keys_of(event: &chainv4::structs::event::Event) -> Vec<EventKey> {
    let (held, evm) = mentioned(event);
    let mut keys = vec![EventKey::Contract(event.contract)];

    keys.extend(held.into_iter().map(EventKey::Mentions));
    keys.extend(evm.into_iter().map(EventKey::MentionsEvm));

    if let Some((topics, _)) = event.evm_log() {
        keys.extend(topics.into_iter().map(EventKey::Topic));
    }

    keys
}
