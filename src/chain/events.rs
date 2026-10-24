//! The event index: every event on the main chain, findable by its contract, by its contract and
//! name, and by any address in its data. Each event is stored once, under its contract; the other
//! two keys only point at it. Keys sort newest first, so a listing reads forward from where the
//! last page stopped.
//!
//! A node keeps the index only when started with --index-events, and once on it stays on. It
//! holds the events of every main-chain block from its low end to the tip. New blocks' entries go
//! into the same commit as their receipts, so a reorg takes them off with the block.
//!
//! Which blocks it keeps is set apart from where its low end is. The node moves the low end a
//! step at a time: down by reading older receipts back, newest first, or up by dropping the
//! oldest blocks' events, as the latest-N window moves with the tip. Each step commits the low
//! end with its entries, so the index never has gaps.

use {
    super::{Undo, UndoEntry, id_key, read_value, stored_block_at, stored_receipt},
    crate::crypto::hashable::Hashable,
    crate::db::{Column, Database, DbError, Read, State, address_key},
    crate::structs::address::Address,
    crate::structs::event::Event,
    crate::structs::hash::{Hash, HashType},
    crate::structs::tx::TxCallArgumentType,
    r#macro::document,
    std::collections::BTreeSet,
    std::fmt,
    std::str::FromStr,
    std::sync::Arc,
    wincode::{SchemaRead, SchemaWrite},
};

/// Where in Meta the index's low end is kept.
pub(super) const FROM_KEY: &[u8] = b"events_from";
/// Where in Meta which blocks it keeps is kept.
pub(super) const KEEP_KEY: &[u8] = b"events_keep";

/// The most blocks, and transactions, one step of catching up reads before it commits.
const STEP_BLOCKS: u64 = 500;
const STEP_TXS: usize = 2_000;

/// Key kinds: the event itself by contract, and pointers to it by name and by address.
const BY_CONTRACT: u8 = b'c';
const BY_NAME: u8 = b'n';
const BY_ADDRESS: u8 = b'a';

#[document(
    summary = "An event, with where it happened: what the event index keeps, and GET /events lists.",
    category = "Chain",
    see_also = "stored_events, Event",
)]
#[derive(Clone, Debug, PartialEq, SchemaWrite, SchemaRead)]
pub struct IndexedEvent {
    #[document(summary = "The transaction that emitted it.")]
    pub tx: Hash,
    #[document(summary = "The block it landed in.")]
    pub block: Hash,
    #[document(summary = "That block's height.")]
    pub height: u64,
    #[document(summary = "The transaction's place in its block.")]
    pub index: u32,
    #[document(summary = "The event's place among the transaction's events.")]
    pub position: u32,
    #[document(summary = "The event.")]
    pub event: Event,
}

impl Hashable for IndexedEvent {}

/// The index's low end.
#[derive(Clone, Copy, Debug, SchemaWrite, SchemaRead)]
pub(super) struct IndexedFrom {
    pub(super) height: u64,
}

impl Hashable for IndexedFrom {}

#[document(
    summary = "Which blocks the event index keeps the events of, as --index-events takes it: genesis, a height, or latest-N.",
    category = "Chain",
    see_also = "EventIndex, Chain::index_events",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
#[wincode(tag_encoding = "u8")]
pub enum EventsFrom {
    #[document(summary = "Every block from this height on; 0 for every block, from genesis.")]
    #[wincode(tag = 0)]
    Height(u64),
    #[document(summary = "Only the latest this many blocks: as the chain grows, the oldest blocks' events go.")]
    #[wincode(tag = 1)]
    Latest(u64),
}

impl Hashable for EventsFrom {}

impl EventsFrom {
    #[document(
        parent = "EventsFrom",
        summary = "The lowest height the index should hold, with the chain's tip at this height.",
    )]
    pub fn lowest(self, tip: u64) -> u64 {
        match self {
            EventsFrom::Height(height) => height,
            EventsFrom::Latest(blocks) => (tip + 1).saturating_sub(blocks),
        }
    }
}

impl FromStr for EventsFrom {
    type Err = String;

    fn from_str(text: &str) -> Result<EventsFrom, String> {
        let text = text.trim().to_ascii_lowercase();

        if text == "genesis" {
            return Ok(EventsFrom::Height(0));
        }

        if let Some(blocks) = text.strip_prefix("latest-") {
            return match blocks.parse() {
                Ok(0) => Err("latest-0 would keep no events".to_string()),
                Ok(blocks) => Ok(EventsFrom::Latest(blocks)),
                Err(_) => Err(format!("{blocks} is not a number of blocks")),
            };
        }

        text.parse()
            .map(EventsFrom::Height)
            .map_err(|_| format!("{text} is not genesis, a height or latest-N, such as latest-100000"))
    }
}

impl fmt::Display for EventsFrom {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EventsFrom::Height(0) => write!(f, "every block"),
            EventsFrom::Height(height) => write!(f, "blocks from height {height}"),
            EventsFrom::Latest(blocks) => write!(f, "the latest {blocks} blocks"),
        }
    }
}

#[document(
    summary = "What a node's event index holds, and what it keeps.",
    category = "Chain",
    see_also = "EventsFrom, stored_event_index",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EventIndex {
    #[document(summary = "Its low end: it holds the events of every main-chain block from this height to the tip.")]
    pub from: u64,
    #[document(summary = "Which blocks it keeps, which the node moves the low end toward.")]
    pub keep: EventsFrom,
}

impl EventIndex {
    #[document(
        parent = "EventIndex",
        summary = "Whether the low end is where it should be, with the chain's tip at this height.",
    )]
    pub fn caught_up(self, tip: u64) -> bool {
        self.from == self.keep.lowest(tip)
    }
}

#[document(
    summary = "Which events to list.",
    category = "Chain",
    see_also = "stored_events",
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EventFilter {
    #[document(summary = "Every event a contract emitted.")]
    Contract(Address),
    #[document(summary = "A contract's events of one name, such as Transfer.")]
    Named(Address, String),
    #[document(summary = "Every event with this address in its data, from any contract.")]
    Mentions(Address),
}

impl EventFilter {
    fn prefix(&self) -> Vec<u8> {
        match self {
            EventFilter::Contract(contract) => [&[BY_CONTRACT][..], &address_key(contract)].concat(),
            EventFilter::Named(contract, name) => named_prefix(contract, name),
            EventFilter::Mentions(address) => [&[BY_ADDRESS][..], &address_key(address)].concat(),
        }
    }
}

fn named_prefix(contract: &Address, name: &str) -> Vec<u8> {
    // The name's length goes first, so no name's keys can run into a longer one's.
    let length = u8::try_from(name.len()).unwrap_or(u8::MAX);

    [&[BY_NAME][..], &address_key(contract), &[length], name.as_bytes()].concat()
}

/// Where an event sits, newest first: the height and both positions inverted.
fn place(height: u64, index: u32, position: u32) -> Vec<u8> {
    [
        &(u64::MAX - height).to_be_bytes()[..],
        &(u32::MAX - index).to_be_bytes(),
        &(u32::MAX - position).to_be_bytes(),
    ]
    .concat()
}

/// An event's keys: the one under its contract, and those that point at it.
fn keys(event: &Event, place: &[u8]) -> (Vec<u8>, Vec<Vec<u8>>) {
    let primary = [&[BY_CONTRACT][..], &address_key(&event.contract), place].concat();
    let mentioned: BTreeSet<Address> = event
        .data
        .iter()
        .filter_map(|value| match value {
            TxCallArgumentType::Address(address) => Some(*address),
            _ => None,
        })
        .collect();
    let mut pointers = vec![[named_prefix(&event.contract, &event.name), place.to_vec()].concat()];
    pointers.extend(mentioned.iter().map(|address| [&[BY_ADDRESS][..], &address_key(address), place].concat()));

    (primary, pointers)
}

/// Indexes one transaction's events, in the layer its block commits.
pub(super) fn index(layer: &State, block: Hash, height: u64, tx: Hash, index: u32, events: &[Event]) {
    for (position, event) in events.iter().enumerate() {
        let position = u32::try_from(position).unwrap_or(u32::MAX);
        let (primary, pointers) = keys(event, &place(height, index, position));

        for pointer in pointers {
            layer.put(Column::Events, pointer, primary.clone());
        }

        layer.put_value(Column::Events, primary, &IndexedEvent {
            tx,
            block,
            height,
            index,
            position,
            event: event.clone(),
        });
    }
}

/// Takes one transaction's events back out of the index.
fn unindex(layer: &State, height: u64, index: u32, events: &[Event]) {
    for (position, event) in events.iter().enumerate() {
        let position = u32::try_from(position).unwrap_or(u32::MAX);
        let (primary, pointers) = keys(event, &place(height, index, position));

        for key in pointers.into_iter().chain([primary]) {
            layer.delete(Column::Events, key);
        }
    }
}

/// Moves the index's low end a step toward where it keeps, in one commit: indexing the blocks
/// below it newest first, or dropping its oldest. Only blocks up to the tip are read; above it
/// there is nothing to index or drop.
pub(super) fn catch_up(db: &Arc<dyn Database>, hash: HashType, index: EventIndex, tip: u64) -> Result<u64, DbError> {
    let lowest = index.keep.lowest(tip);
    let state = Arc::new(State::new(db.clone()));
    let (mut from, mut blocks, mut txs) = (index.from, 0, 0);

    if from > lowest {
        from = from.min(tip + 1).max(lowest);

        while from > lowest && blocks < STEP_BLOCKS && txs < STEP_TXS {
            from -= 1;
            txs += add_block(&state, hash, from)?;
            blocks += 1;
        }
    } else {
        while from < lowest.min(tip + 1) && blocks < STEP_BLOCKS && txs < STEP_TXS {
            txs += drop_block(&state, hash, from)?;
            from += 1;
            blocks += 1;
        }

        if from > tip {
            from = lowest;
        }
    }

    state.put_value(Column::Meta, FROM_KEY.to_vec(), &IndexedFrom { height: from });
    db.commit(state.take_changes())?;

    Ok(from)
}

/// Indexes a main-chain block's events from its receipts. A block a reorg can still take off
/// forgets them with the rest of it: they join its undo record.
fn add_block(state: &Arc<State>, hash: HashType, height: u64) -> Result<usize, DbError> {
    let block = stored_block_at(&**state, height)?.ok_or_else(|| DbError::Corrupt(format!("no main-chain block at height {height}")))?;
    let id = block.id();
    let layer = State::new(state.clone());

    for (index, tx) in block.txs.iter().enumerate() {
        let tx_id = tx.id(hash);
        let receipt = stored_receipt(&**state, &tx_id)?.ok_or_else(|| DbError::Corrupt(format!("transaction {tx_id} has no receipt")))?;
        self::index(&layer, id, height, tx_id, index as u32, &receipt.events);
    }

    let changes = layer.take_changes();

    if let Some(mut undo) = read_value::<Undo>(&**state, Column::Undo, &id_key(&id))? {
        undo.entries.extend(changes.keys().map(|(column, key)| UndoEntry {
            column: *column,
            key: key.clone(),
            value: None,
        }));
        state.put_value(Column::Undo, id_key(&id), &undo);
    }

    state.merge(changes);

    Ok(block.txs.len())
}

/// Takes a main-chain block's events out of the index.
fn drop_block(state: &State, hash: HashType, height: u64) -> Result<usize, DbError> {
    let block = stored_block_at(state, height)?.ok_or_else(|| DbError::Corrupt(format!("no main-chain block at height {height}")))?;

    for (index, tx) in block.txs.iter().enumerate() {
        let tx_id = tx.id(hash);
        let receipt = stored_receipt(state, &tx_id)?.ok_or_else(|| DbError::Corrupt(format!("transaction {tx_id} has no receipt")))?;
        unindex(state, height, index as u32, &receipt.events);
    }

    Ok(block.txs.len())
}

#[document(
    summary = "What the event index holds and keeps, read straight from a database.",
    category = "Chain",
    returns = "The index, or None for a node that keeps none.",
    see_also = "stored_events",
)]
pub fn stored_event_index(db: &dyn Read) -> Result<Option<EventIndex>, DbError> {
    let Some(from) = read_value::<IndexedFrom>(db, Column::Meta, FROM_KEY)? else {
        return Ok(None);
    };
    // An index from before what it keeps was stored kept what it had: from where it was turned on.
    let keep = read_value(db, Column::Meta, KEEP_KEY)?.unwrap_or(EventsFrom::Height(from.height));

    Ok(Some(EventIndex { from: from.height, keep }))
}

#[document(
    summary = "A page of indexed events, newest first, read straight from a database.",
    category = "Chain",
    params = {
        db = "The database.",
        filter = "Which events.",
        cursor = "Where the last page stopped, as it returned; None for the newest.",
        limit = "The most events to return.",
    },
    returns = "The events, and the cursor for the next page, or None after the last.",
    see_also = "stored_event_index, IndexedEvent",
)]
pub fn stored_events(
    db: &dyn Database,
    filter: &EventFilter,
    cursor: Option<&[u8]>,
    limit: usize,
) -> Result<(Vec<IndexedEvent>, Option<Vec<u8>>), DbError> {
    let prefix = filter.prefix();
    let mut found: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    db.scan(Column::Events, &prefix, cursor.unwrap_or(&prefix), &mut |key, value| {
        found.push((key.to_vec(), value.to_vec()));

        found.len() <= limit
    })?;
    let next = (found.len() > limit).then(|| found.remove(limit).0);
    let mut events = Vec::with_capacity(found.len());

    for (_, value) in found {
        let record = match filter {
            EventFilter::Contract(_) => value,
            // The others point at the event, under its contract.
            EventFilter::Named(..) | EventFilter::Mentions(_) => db
                .read(Column::Events, &value)?
                .ok_or_else(|| DbError::Corrupt("an event index entry points at nothing".to_string()))?,
        };

        events.push(
            wincode::deserialize::<IndexedEvent>(&record)
                .map_err(|err| DbError::Corrupt(format!("an indexed event would not decode: {err}")))?,
        );
    }

    Ok((events, next))
}
