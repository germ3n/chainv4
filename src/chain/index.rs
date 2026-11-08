//! What the chain's indexes share. A node may keep two, each its own choice: the event index,
//! every event findable by its contract, its name, the addresses in its data and an EVM log's
//! topics; and the address index, every transaction findable by the addresses it touches. And
//! every node keeps a third, the versions index, each contract's code as deployed and upgraded.
//! None is part of the chain's state: each is what a node reads out of the blocks and receipts it keeps,
//! for its API to list.
//!
//! An index holds every main-chain block from its low end to the tip, and once on stays on until
//! dropped. New blocks' entries go into the same commit as their receipts, so a reorg takes them
//! off with the block.
//!
//! Which blocks it keeps is set apart from where its low end is. The node moves the low end a
//! step at a time: down by reading older blocks back, newest first, or up by dropping the oldest
//! blocks' entries, as a latest-N window moves with the tip. Each step commits the low end with
//! its entries, so an index never has gaps.

use {
    super::{Undo, UndoEntry, addresses, events, versions, id_key, read_value, stored_block_at, stored_receipt},
    crate::chain::receipt::TxReceipt,
    crate::crypto::hashable::Hashable,
    crate::db::{Column, Database, DbError, Read, State},
    crate::structs::hash::{Hash, HashType},
    crate::structs::tx::SignedTx,
    r#macro::document,
    std::fmt,
    std::str::FromStr,
    std::sync::Arc,
    wincode::{SchemaRead, SchemaWrite},
};

/// The most blocks, and transactions, one step of catching up reads before it commits.
const STEP_BLOCKS: u64 = 500;
const STEP_TXS: usize = 2_000;

/// What an event index's entries hold, in Meta: an index from before this was kept has no EVM log's
/// topics in it, and starts over the first time a node opens it.
const EVENTS_SCHEMA_KEY: &[u8] = b"events_schema";
const EVENTS_SCHEMA: u8 = 2;

#[derive(Clone, Copy, Debug, SchemaWrite, SchemaRead)]
struct EventsSchema {
    version: u8,
}

impl Hashable for EventsSchema {}

/// Marks an event index, in this change, as holding what this build's entries hold.
pub(super) fn mark_events_schema(state: &State) {
    state.put_value(Column::Meta, EVENTS_SCHEMA_KEY.to_vec(), &EventsSchema { version: EVENTS_SCHEMA });
}

/// Forgets what an event index's entries hold, in this change, as dropping it does.
pub(super) fn forget_events_schema(state: &State) {
    state.delete(Column::Meta, EVENTS_SCHEMA_KEY.to_vec());
}

/// The event index a database holds, brought up to what this build's entries hold. One from before
/// EVM logs' topics were indexed would never find its older logs by them, and the catching up that
/// fills an index only ever looks below its low end, so it starts over: emptied in one atomic write,
/// keeping the same blocks, and read back from their receipts, newest first, in the background, as
/// a new one is. Nothing here touches the chain's state.
pub(super) fn events_up_to_date(db: &Arc<dyn Database>, window: Option<IndexWindow>, tip: u64) -> Result<Option<IndexWindow>, DbError> {
    let Some(window) = window else {
        return Ok(None);
    };

    if read_value::<EventsSchema>(&**db, Column::Meta, EVENTS_SCHEMA_KEY)?.is_some_and(|schema| schema.version >= EVENTS_SCHEMA) {
        return Ok(Some(window));
    }

    let window = IndexWindow { from: tip + 1, ..window };
    let state = State::new(db.clone());
    state.put_value(Column::Meta, IndexKind::Events.low_end_key().to_vec(), &IndexedFrom { height: window.from });
    state.put_value(Column::Meta, IndexKind::Events.keep_key().to_vec(), &window.keep);
    mark_events_schema(&state);
    db.clear(Column::Events, state.take_changes())?;
    log::info!("[chain] the event index starts over, to hold EVM logs' topics: it reads {} back in the background", window.keep);

    Ok(Some(window))
}

#[document(
    summary = "Which of a node's indexes: of events, or of the transactions touching each address.",
    category = "Chain",
    see_also = "Chain::keep_index, IndexWindow",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IndexKind {
    #[document(summary = "The event index, which GET /events lists from: --index-events.")]
    Events,
    #[document(summary = "The address index, which GET /accounts/{address}/txs lists from: --index-addresses.")]
    Addresses,
    #[document(summary = "The versions index, which GET /contracts/{address}/versions lists from: every node keeps it, of every block.")]
    Versions,
}

impl IndexKind {
    #[document(parent = "IndexKind", summary = "Every one, in the order a block indexes them.")]
    pub const ALL: [IndexKind; 3] = [IndexKind::Events, IndexKind::Addresses, IndexKind::Versions];

    #[document(parent = "IndexKind", summary = "The column it is kept in, alone.")]
    pub fn column(self) -> Column {
        match self {
            IndexKind::Events => Column::Events,
            IndexKind::Addresses => Column::Addresses,
            IndexKind::Versions => Column::Versions,
        }
    }

    /// Where in Meta its low end is kept, and which blocks it keeps.
    pub(super) fn low_end_key(self) -> &'static [u8] {
        match self {
            IndexKind::Events => b"events_from",
            IndexKind::Addresses => b"addresses_from",
            IndexKind::Versions => b"versions_from",
        }
    }

    pub(super) fn keep_key(self) -> &'static [u8] {
        match self {
            IndexKind::Events => b"events_keep",
            IndexKind::Addresses => b"addresses_keep",
            IndexKind::Versions => b"versions_keep",
        }
    }

    /// Indexes one transaction, in the layer its block commits.
    pub(super) fn index(self, layer: &State, landed: Landed, signed: &SignedTx, receipt: &TxReceipt) {
        match self {
            IndexKind::Events => events::index(layer, landed.block, landed.height, landed.tx, landed.at, &receipt.events),
            IndexKind::Addresses => addresses::index(layer, landed, signed, receipt),
            IndexKind::Versions => versions::index(layer, landed, signed, receipt),
        }
    }

    /// Takes one transaction back out.
    fn unindex(self, layer: &State, height: u64, at: u32, signed: &SignedTx, receipt: &TxReceipt) {
        match self {
            IndexKind::Events => events::unindex(layer, height, at, &receipt.events),
            IndexKind::Addresses => addresses::unindex(layer, height, at, signed, receipt),
            IndexKind::Versions => versions::unindex(layer, height, at, signed, receipt),
        }
    }
}

impl fmt::Display for IndexKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IndexKind::Events => write!(f, "event index"),
            IndexKind::Addresses => write!(f, "address index"),
            IndexKind::Versions => write!(f, "versions index"),
        }
    }
}

/// Where a transaction landed, as an index keys it.
#[derive(Clone, Copy, Debug)]
pub(super) struct Landed {
    pub(super) block: Hash,
    pub(super) height: u64,
    /// Its place in the block.
    pub(super) at: u32,
    pub(super) tx: Hash,
}

/// An index's low end.
#[derive(Clone, Copy, Debug, SchemaWrite, SchemaRead)]
pub(super) struct IndexedFrom {
    pub(super) height: u64,
}

impl Hashable for IndexedFrom {}

#[document(
    summary = "Which blocks an index keeps the entries of, as --index-events and --index-addresses take it: genesis, a height, or latest-N.",
    category = "Chain",
    see_also = "IndexWindow, Chain::keep_index",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
#[wincode(tag_encoding = "u8")]
pub enum KeepFrom {
    #[document(summary = "Every block from this height on; 0 for every block, from genesis.")]
    #[wincode(tag = 0)]
    Height(u64),
    #[document(summary = "Only the latest this many blocks: as the chain grows, the oldest blocks' entries go.")]
    #[wincode(tag = 1)]
    Latest(u64),
}

impl Hashable for KeepFrom {}

impl KeepFrom {
    #[document(
        parent = "KeepFrom",
        summary = "The lowest height the index should hold, with the chain's tip at this height.",
    )]
    pub fn lowest(self, tip: u64) -> u64 {
        match self {
            KeepFrom::Height(height) => height,
            KeepFrom::Latest(blocks) => (tip + 1).saturating_sub(blocks),
        }
    }
}

impl FromStr for KeepFrom {
    type Err = String;

    fn from_str(text: &str) -> Result<KeepFrom, String> {
        let text = text.trim().to_ascii_lowercase();

        if text == "genesis" {
            return Ok(KeepFrom::Height(0));
        }

        if let Some(blocks) = text.strip_prefix("latest-") {
            return match blocks.parse() {
                Ok(0) => Err("latest-0 would keep nothing".to_string()),
                Ok(blocks) => Ok(KeepFrom::Latest(blocks)),
                Err(_) => Err(format!("{blocks} is not a number of blocks")),
            };
        }

        text.parse()
            .map(KeepFrom::Height)
            .map_err(|_| format!("{text} is not genesis, a height, latest-N such as latest-100000, or off"))
    }
}

impl fmt::Display for KeepFrom {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KeepFrom::Height(0) => write!(f, "every block"),
            KeepFrom::Height(height) => write!(f, "blocks from height {height}"),
            KeepFrom::Latest(blocks) => write!(f, "the latest {blocks} blocks"),
        }
    }
}

#[document(
    summary = "What --index-events or --index-addresses asks for: keep the index, of which blocks, or keep none.",
    category = "Chain",
    see_also = "KeepFrom, Chain::keep_index, Chain::drop_index",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Indexing {
    #[document(summary = "Keep it, of these blocks.")]
    Keep(KeepFrom),
    #[document(summary = "Keep none, dropping the index the database holds.")]
    Off,
}

impl FromStr for Indexing {
    type Err = String;

    fn from_str(text: &str) -> Result<Indexing, String> {
        match text.trim().eq_ignore_ascii_case("off") {
            true => Ok(Indexing::Off),
            false => text.parse().map(Indexing::Keep),
        }
    }
}

#[document(
    summary = "What one of a node's indexes holds, and what it keeps.",
    category = "Chain",
    see_also = "KeepFrom, stored_index",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IndexWindow {
    #[document(summary = "Its low end: it holds the entries of every main-chain block from this height to the tip.")]
    pub from: u64,
    #[document(summary = "Which blocks it keeps, which the node moves the low end toward.")]
    pub keep: KeepFrom,
}

impl IndexWindow {
    #[document(
        parent = "IndexWindow",
        summary = "Whether the low end is where it should be, with the chain's tip at this height.",
    )]
    pub fn caught_up(self, tip: u64) -> bool {
        self.from == self.keep.lowest(tip)
    }
}

/// Moves an index's low end a step toward where it keeps, in one commit: indexing the blocks below
/// it newest first, or dropping its oldest. Only blocks up to the tip are read; above it there is
/// nothing to index or drop.
pub(super) fn catch_up(db: &Arc<dyn Database>, hash: HashType, kind: IndexKind, window: IndexWindow, tip: u64, floor: u64) -> Result<u64, DbError> {
    let lowest = window.keep.lowest(tip).max(floor);
    let state = Arc::new(State::new(db.clone()));
    let (mut from, mut blocks, mut txs) = (window.from, 0, 0);

    if from > lowest {
        from = from.min(tip + 1).max(lowest);

        while from > lowest && blocks < STEP_BLOCKS && txs < STEP_TXS {
            from -= 1;
            txs += add_block(&state, hash, kind, from)?;
            blocks += 1;
        }
    } else {
        while from < lowest.min(tip + 1) && blocks < STEP_BLOCKS && txs < STEP_TXS {
            txs += drop_block(&state, hash, kind, from)?;
            from += 1;
            blocks += 1;
        }

        if from > tip {
            from = lowest;
        }
    }

    state.put_value(Column::Meta, kind.low_end_key().to_vec(), &IndexedFrom { height: from });
    db.commit(state.take_changes())?;

    Ok(from)
}

/// Each transaction of a main-chain block, with its receipt.
fn each_tx(state: &dyn Read, hash: HashType, height: u64, mut visit: impl FnMut(Hash, u32, Hash, &SignedTx, &TxReceipt)) -> Result<usize, DbError> {
    let block = stored_block_at(state, height)?.ok_or_else(|| DbError::Corrupt(format!("no main-chain block at height {height}")))?;
    let id = block.id();

    for (at, signed) in block.txs.iter().enumerate() {
        let tx = signed.id(hash);
        let receipt = stored_receipt(state, &tx)?.ok_or_else(|| DbError::Corrupt(format!("transaction {tx} has no receipt")))?;
        visit(id, at as u32, tx, signed, &receipt);
    }

    Ok(block.txs.len())
}

/// Indexes a main-chain block from its receipts. A block a reorg can still take off forgets its
/// entries with the rest of it: they join its undo record.
fn add_block(state: &Arc<State>, hash: HashType, kind: IndexKind, height: u64) -> Result<usize, DbError> {
    let layer = State::new(state.clone());
    let mut block = None;
    let count = each_tx(&**state, hash, height, |id, at, tx, signed, receipt| {
        block = Some(id);
        kind.index(&layer, Landed { block: id, height, at, tx }, signed, receipt);
    })?;
    let changes = layer.take_changes();

    if let Some(id) = block
        && let Some(mut undo) = read_value::<Undo>(&**state, Column::Undo, &id_key(&id))?
    {
        undo.entries.extend(changes.keys().map(|(column, key)| UndoEntry {
            column: *column,
            key: key.clone(),
            value: None,
        }));
        state.put_value(Column::Undo, id_key(&id), &undo);
    }

    state.merge(changes);

    Ok(count)
}

/// Takes a main-chain block's entries out of the index.
fn drop_block(state: &State, hash: HashType, kind: IndexKind, height: u64) -> Result<usize, DbError> {
    each_tx(state, hash, height, |_, at, _, signed, receipt| kind.unindex(state, height, at, signed, receipt))
}

#[document(
    summary = "What one of a node's indexes holds and keeps, read straight from a database.",
    category = "Chain",
    returns = "The index's window, or None for a node that keeps none of this kind.",
    see_also = "stored_events, stored_address_txs",
)]
pub fn stored_index(db: &dyn Read, kind: IndexKind) -> Result<Option<IndexWindow>, DbError> {
    let Some(from) = read_value::<IndexedFrom>(db, Column::Meta, kind.low_end_key())? else {
        return Ok(None);
    };
    // An index from before what it keeps was stored kept what it had: from where it was turned on.
    let keep = read_value(db, Column::Meta, kind.keep_key())?.unwrap_or(KeepFrom::Height(from.height));

    Ok(Some(IndexWindow { from: from.height, keep }))
}
