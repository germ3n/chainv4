//! The event index: every event on the main chain, findable by its contract, by its contract and
//! name, by any address in its data, and for an EVM log, by any of its topics, alone or with its
//! contract. Each event is stored once, under its contract; the other keys only point at it. Keys
//! sort newest first, so a listing reads forward from where the last page stopped.
//!
//! An EVM log holds everything as bytes, so the addresses it names are never Address values: those
//! a contract indexes are topics, as an ERC-20's Transfer carries its parties. An address is found
//! in them by its EVM address, padded as a topic is, so what mentions an address takes in both.
//!
//! A node keeps it only when started with --index-events, and once on it stays on until
//! --index-events=off drops it. How it holds blocks, and catches up, is what every index shares,
//! in the index module.

use {
    crate::crypto::hashable::Hashable,
    crate::db::{Column, Committed, DbError, State, address_key},
    crate::structs::address::Address,
    crate::structs::event::Event,
    crate::structs::hash::Hash,
    crate::structs::tx::TxCallArgumentType,
    r#macro::document,
    std::collections::BTreeSet,
    wincode::{SchemaRead, SchemaWrite},
};

/// Key kinds: the event itself by contract, and pointers to it by name, by address, and by topic,
/// on its own or under the contract.
const BY_CONTRACT: u8 = b'c';
const BY_NAME: u8 = b'n';
const BY_ADDRESS: u8 = b'a';
const BY_TOPIC: u8 = b't';
const BY_CONTRACT_TOPIC: u8 = b'k';

/// How long where an event sits is, which ends every key.
const PLACE: usize = 16;

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
    #[document(summary = "Every event with this address in its data, from any contract, and every EVM log with its EVM address in a topic.")]
    Mentions(Address),
    #[document(summary = "Every EVM log with this topic, in any place, from any contract: the first is its event's signature.")]
    Topic([u8; 32]),
    #[document(summary = "A contract's EVM logs with this topic, in any place.")]
    ContractTopic(Address, [u8; 32]),
}

impl EventFilter {
    /// Where its keys are: one place, but for an address, which may be in an event's values or a
    /// log's topics.
    fn prefixes(&self) -> Vec<Vec<u8>> {
        match self {
            EventFilter::Contract(contract) => vec![[&[BY_CONTRACT][..], &address_key(contract)].concat()],
            EventFilter::Named(contract, name) => vec![named_prefix(contract, name)],
            EventFilter::Mentions(address) => {
                #[cfg_attr(not(feature = "abi"), allow(unused_mut))]
                let mut prefixes = vec![[&[BY_ADDRESS][..], &address_key(address)].concat()];

                #[cfg(feature = "abi")]
                {
                    let mut topic = [0; 32];
                    topic[12..].copy_from_slice(&crate::abi::evm_address(address));
                    prefixes.push(topic_prefix(&topic));
                }

                prefixes
            }
            EventFilter::Topic(topic) => vec![topic_prefix(topic)],
            EventFilter::ContractTopic(contract, topic) => vec![contract_topic_prefix(contract, topic)],
        }
    }
}

fn topic_prefix(topic: &[u8; 32]) -> Vec<u8> {
    [&[BY_TOPIC][..], topic].concat()
}

fn contract_topic_prefix(contract: &Address, topic: &[u8; 32]) -> Vec<u8> {
    [&[BY_CONTRACT_TOPIC][..], &address_key(contract), topic].concat()
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

    if let Some((topics, _)) = event.evm_log() {
        for topic in topics.iter().collect::<BTreeSet<_>>() {
            pointers.push([topic_prefix(topic), place.to_vec()].concat());
            pointers.push([contract_topic_prefix(&event.contract, topic), place.to_vec()].concat());
        }
    }

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
pub(super) fn unindex(layer: &State, height: u64, index: u32, events: &[Event]) {
    for (position, event) in events.iter().enumerate() {
        let position = u32::try_from(position).unwrap_or(u32::MAX);
        let (primary, pointers) = keys(event, &place(height, index, position));

        for key in pointers.into_iter().chain([primary]) {
            layer.delete(Column::Events, key);
        }
    }
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
    see_also = "stored_index, IndexedEvent",
)]
pub fn stored_events(
    db: &dyn Committed,
    filter: &EventFilter,
    cursor: Option<&[u8]>,
    limit: usize,
) -> Result<(Vec<IndexedEvent>, Option<Vec<u8>>), DbError> {
    // Where the last page stopped, which every prefix shares: a cursor is where an event sits, and
    // one from before cursors were only that is a whole key, which ends with it.
    let from = cursor.map(|cursor| &cursor[cursor.len().saturating_sub(PLACE)..]);
    let mut found: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();

    for prefix in filter.prefixes() {
        let start = [prefix.as_slice(), from.unwrap_or_default()].concat();
        let mut read = 0;

        db.scan(Column::Events, &prefix, &start, &mut |key, value| {
            found.push((key[key.len().saturating_sub(PLACE)..].to_vec(), value.to_vec()));
            read += 1;

            read <= limit
        })?;
    }

    // Newest first across the prefixes, each event once.
    found.sort_by(|first, second| first.0.cmp(&second.0));
    found.dedup_by(|later, earlier| later.0 == earlier.0);

    let next = (found.len() > limit).then(|| found.swap_remove(limit).0);
    found.truncate(limit);

    let mut events = Vec::with_capacity(found.len());

    for (_, value) in found {
        let record = match filter {
            EventFilter::Contract(_) => value,
            // The others point at the event, under its contract.
            EventFilter::Named(..) | EventFilter::Mentions(_) | EventFilter::Topic(_) | EventFilter::ContractTopic(..) => db
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
