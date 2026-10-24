//! The event index: every event on the main chain, findable by its contract, by its contract and
//! name, and by any address in its data. Each event is stored once, under its contract; the other
//! two keys only point at it. Keys sort newest first, so a listing reads forward from where the
//! last page stopped.
//!
//! A node keeps the index only when started with --index-events, from the block it was turned on
//! at, and once on it stays on. Entries go into the same commit as the block's receipts, so a
//! reorg takes them off with the block.

use {
    super::read_value,
    crate::crypto::hashable::Hashable,
    crate::db::{Column, Database, DbError, Read, State, address_key},
    crate::structs::address::Address,
    crate::structs::event::Event,
    crate::structs::hash::Hash,
    crate::structs::tx::TxCallArgumentType,
    r#macro::document,
    std::collections::BTreeSet,
    wincode::{SchemaRead, SchemaWrite},
};

/// Where in Meta the height the index starts at is kept.
pub(super) const FROM_KEY: &[u8] = b"events_from";

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

/// The height the index starts at.
#[derive(Clone, Copy, Debug, SchemaWrite, SchemaRead)]
pub(super) struct IndexedFrom {
    pub(super) height: u64,
}

impl Hashable for IndexedFrom {}

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

/// Indexes one transaction's events, in the layer its block commits.
pub(super) fn index(layer: &State, block: Hash, height: u64, tx: Hash, index: u32, events: &[Event]) {
    for (position, event) in events.iter().enumerate() {
        let position = u32::try_from(position).unwrap_or(u32::MAX);
        let place = place(height, index, position);
        let primary = [&[BY_CONTRACT][..], &address_key(&event.contract), &place].concat();
        let mentioned: BTreeSet<Address> = event
            .data
            .iter()
            .filter_map(|value| match value {
                TxCallArgumentType::Address(address) => Some(*address),
                _ => None,
            })
            .collect();

        layer.put(Column::Events, [named_prefix(&event.contract, &event.name), place.clone()].concat(), primary.clone());

        for address in mentioned {
            let key = [&[BY_ADDRESS][..], &address_key(&address), &place].concat();
            layer.put(Column::Events, key, primary.clone());
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

#[document(
    summary = "The height the event index starts at, read straight from a database.",
    category = "Chain",
    returns = "The first height indexed, or None for a node that keeps no event index.",
    see_also = "stored_events",
)]
pub fn stored_events_from(db: &dyn Read) -> Result<Option<u64>, DbError> {
    Ok(read_value::<IndexedFrom>(db, Column::Meta, FROM_KEY)?.map(|from| from.height))
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
    see_also = "stored_events_from, IndexedEvent",
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
