//! Storage. The chain only talks to the Database trait, so the backend can change: RocksDb for
//! nodes, MemoryDb for tests and tools, anything else that implements it later. Nothing writes
//! to a database directly: changes collect in State layers, a call's on top of its
//! transaction's on top of its block's, and the block commits them in one atomic batch.

use {
    crate::crypto::hashable::Hashable,
    crate::structs::address::Address,
    r#macro::document,
    std::collections::{BTreeMap, BTreeSet},
    std::fmt,
    std::sync::{Arc, Mutex},
    wincode::{SchemaRead, SchemaWrite},
};

mod history;
mod memory;
#[cfg(feature = "rocksdb")]
mod rocksdb;
mod snapshot;
mod tree;

pub use self::history::{Historic, history_key, history_list_key, kept_in_history, replaced};
pub use self::memory::MemoryDb;
#[cfg(feature = "rocksdb")]
pub use self::rocksdb::{DEFAULT_CACHE_MB, RocksDb};
pub use self::snapshot::{SnapshotWalk, WalkError};
pub use self::tree::{PartContents, Path, STATE_COLUMNS, StateEntry, StatePart, StateProof, StateTree, root_of, state_path, stored_root};

#[document(
    summary = "Where a record lives. A backend keeps each column apart: RocksDb as column families, MemoryDb as part of the key.",
    category = "Storage",
    see_also = "Database, State",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, SchemaWrite, SchemaRead)]
#[wincode(tag_encoding = "u8")]
pub enum Column {
    #[document(summary = "Account by address.")]
    #[wincode(tag = 0)]
    Accounts,
    #[document(summary = "Deployed contract by address.")]
    #[wincode(tag = 1)]
    Contracts,
    #[document(summary = "Contract storage, keyed by the contract's address followed by the script's own key.")]
    #[wincode(tag = 2)]
    Storage,
    #[document(summary = "Block by id: every block this node accepted, on the main chain or a side branch.")]
    #[wincode(tag = 3)]
    Blocks,
    #[document(summary = "Main-chain block id by height, big-endian so heights sort in order.")]
    #[wincode(tag = 4)]
    Heights,
    #[document(summary = "Chain bookkeeping, such as which block is the tip.")]
    #[wincode(tag = 5)]
    Meta,
    #[document(summary = "Total work of the chain ending at a block, by block id. Fork choice compares these.")]
    #[wincode(tag = 6)]
    Work,
    #[document(summary = "What a main-chain block changed, as the values it replaced, by block id. Kept for recent blocks so a reorg can take them back.")]
    #[wincode(tag = 7)]
    Undo,
    #[document(summary = "Where a transaction landed and what it did, by transaction id.")]
    #[wincode(tag = 8)]
    Receipts,
    #[document(summary = "Finality's voters: FFG validators by voting key, who bonded, how much and when they vote; and authority finality's decided signers and the ballot to change them.")]
    #[wincode(tag = 9)]
    Validators,
    #[document(summary = "Per-epoch consensus records: each epoch's validator set or signers, fixed activation_delay epochs ahead, and which checkpoints' voters have been paid.")]
    #[wincode(tag = 10)]
    Epochs,
    #[document(summary = "What this node knows about finality and is not part of the chain's state: justifications it has seen, the latest justified and finalized checkpoints, the votes it counted or holds, its own votes, and evidence of cheating.")]
    #[wincode(tag = 11)]
    Finality,
    #[document(summary = "The event index, for a node that keeps one: each event by its contract, and pointers to it by contract and name and by every address in its data, newest first.")]
    #[wincode(tag = 12)]
    Events,
    #[document(summary = "The token and collection registries: every contract that exports the token standard, with its name, symbol and decimals, and every one that exports a collection standard, with its kind, name and symbol, each by address and by symbol.")]
    #[wincode(tag = 13)]
    Tokens,
    #[document(summary = "Contract storage in namespaces, the contract's own space among them: keyed by the contract's address, the namespace's name with its length in front, and the script's own key.")]
    #[wincode(tag = 14)]
    Namespaces,
    #[document(summary = "The state tree's nodes, by depth and the path bits above them: what the state root in each header is the root of. Kept in step with the chain-state columns, block by block.")]
    #[wincode(tag = 15)]
    StateTree,
    #[document(summary = "Every stored block's header and seal, by block id: a node that synced from a snapshot keeps these for the blocks before it, whose bodies it never had.")]
    #[wincode(tag = 16)]
    Headers,
    #[document(summary = "What light clients need to follow finality, by checkpoint block id: proofs, against the block's state root, of the voters it fixed for a later epoch.")]
    #[wincode(tag = 17)]
    Light,
    #[document(summary = "The address index, for a node that keeps one: each transaction under every address it touches, newest first.")]
    #[wincode(tag = 18)]
    Addresses,
    #[document(summary = "The name registry: each name's record by name, and each account's primary name by address.")]
    #[wincode(tag = 19)]
    Names,
    #[document(summary = "The Ethereum index, for a node with the ethereum feature: each Ethereum-signed transaction's chainv4 id, by the keccak256 of its signed bytes, which Ethereum's tools name it by. Not chain state: written as blocks run, and never wrong, as the same bytes are always the same transaction.")]
    #[wincode(tag = 20)]
    Ethereum,
    #[document(summary = "Peers this node met, by peer id: the addresses each said it listens on and when it was last seen, at most node::MAX_REMEMBERED of them, dialed at the next start. Not chain state.")]
    #[wincode(tag = 21)]
    Peers,
    #[document(summary = "Recent state as it was: for each key a main-chain block changed in the state columns or the state tree, the value it held just before, by key and height; and each height's list of the keys, for pruning, and for GET /blocks/range to prove what the block changed. Kept for the node's --state-history window, or every height with --archive. Not chain state: what a node can answer about past heights.")]
    #[wincode(tag = 22)]
    History,
    #[document(summary = "Mining pools' claims: the root each pool block committed to, by block id, and how much each miner has claimed from each pool, by pool and miner. Chain state.")]
    #[wincode(tag = 23)]
    Pools,
    #[document(summary = "Governance's decisions: the id of the approval that decided each change, by the version it made, big-endian so versions sort in order. Not chain state: written as blocks run, as each such approval's receipt says Decided, and taken back with its block.")]
    #[wincode(tag = 24)]
    Decisions,
    #[document(summary = "The member index, for a node that keeps one: each multisig under the address of every member's key, as the multisig registry names them now, for GET /accounts/{address}/multisigs. Not chain state: built from the registry, kept in step with it as blocks run, and taken back with its block.")]
    #[wincode(tag = 25)]
    Members,
}

impl Hashable for Column {
    const TAG_LEN: usize = 1;
}

impl Column {
    pub const ALL: [Column; 26] = [
        Column::Accounts,
        Column::Contracts,
        Column::Storage,
        Column::Blocks,
        Column::Heights,
        Column::Meta,
        Column::Work,
        Column::Undo,
        Column::Receipts,
        Column::Validators,
        Column::Epochs,
        Column::Finality,
        Column::Events,
        Column::Tokens,
        Column::Namespaces,
        Column::StateTree,
        Column::Headers,
        Column::Light,
        Column::Addresses,
        Column::Names,
        Column::Ethereum,
        Column::Peers,
        Column::History,
        Column::Pools,
        Column::Decisions,
        Column::Members,
    ];

    #[document(
        parent = "Column",
        summary = "A stable name for the column, for backends that need one.",
    )]
    pub fn name(self) -> &'static str {
        match self {
            Column::Accounts => "accounts",
            Column::Contracts => "contracts",
            Column::Storage => "storage",
            Column::Blocks => "blocks",
            Column::Heights => "heights",
            Column::Meta => "meta",
            Column::Work => "work",
            Column::Undo => "undo",
            Column::Receipts => "receipts",
            Column::Validators => "validators",
            Column::Epochs => "epochs",
            Column::Finality => "finality",
            Column::Events => "events",
            Column::Tokens => "tokens",
            Column::Namespaces => "namespaces",
            Column::StateTree => "state_tree",
            Column::Headers => "headers",
            Column::Light => "light",
            Column::Addresses => "addresses",
            Column::Names => "names",
            Column::Ethereum => "ethereum",
            Column::Peers => "peers",
            Column::History => "history",
            Column::Pools => "pools",
            Column::Decisions => "decisions",
            Column::Members => "members",
        }
    }
}

#[document(
    summary = "A storage failure. These come from the node, not from what a transaction did, so they should stop the node rather than fail the transaction.",
    category = "Storage",
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DbError {
    #[document(summary = "The backend reported an error.")]
    Backend(String),
    #[document(summary = "A stored record would not decode.")]
    Corrupt(String),
}

impl fmt::Display for DbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DbError::Backend(text) => write!(f, "database: {text}"),
            DbError::Corrupt(text) => write!(f, "corrupt record: {text}"),
        }
    }
}

impl std::error::Error for DbError {}

/// A key, with the column it is in.
pub type Key = (Column, Vec<u8>);

/// Changes waiting to be committed. None deletes the key.
pub type Changes = BTreeMap<Key, Option<Vec<u8>>>;

/// Keys, as a recording State lists what it read.
pub type Keys = BTreeSet<Key>;

/// Records by column, then key: what a State layer holds and MemoryDb keeps, so a read looks its
/// key up as it is given, without copying it into a (Column, Vec<u8>) first.
pub(crate) type ByColumn<V> = BTreeMap<Column, BTreeMap<Vec<u8>, V>>;

#[document(
    summary = "Anything state can be read from: a database, or a State layered on one.",
    category = "Storage",
)]
pub trait Read: Send + Sync {
    #[document(
        parent = "Read",
        summary = "Reads one value.",
        returns = "The value, or None when the key has none.",
    )]
    fn read(&self, column: Column, key: &[u8]) -> Result<Option<Vec<u8>>, DbError>;
}

#[document(
    summary = "Committed records to read and scan: a database, or a snapshot of one, which later commits never change.",
    category = "Storage",
    see_also = "Database::snapshot",
)]
pub trait Committed: Read {
    #[document(
        parent = "Committed",
        summary = "Visits a column's committed records whose keys start with prefix, in key order, from the first at or after start. Backends keep keys sorted, so this seeks once and reads only what it visits. It sees committed data only, never a State's changes.",
        params = {
            column = "The column to scan.",
            prefix = "What every visited key starts with.",
            start = "Where to begin: the prefix itself for all of them, or a longer key to skip those before it.",
            visit = "Called with each key and value; return false to stop. It must not write to the database.",
        },
    )]
    fn scan(&self, column: Column, prefix: &[u8], start: &[u8], visit: &mut dyn FnMut(&[u8], &[u8]) -> bool) -> Result<(), DbError>;
}

#[document(
    summary = "A storage backend. Reads come through Read and Committed; writes only ever arrive as a whole set of changes, which the backend must apply atomically: after a crash, either all of them are there or none are.",
    category = "Storage",
    see_also = "RocksDb, MemoryDb, State",
)]
pub trait Database: Committed {
    #[document(
        parent = "Database",
        summary = "Writes a set of changes atomically.",
        params = { changes = "Usually a block's State, from take_changes. None values delete." },
    )]
    fn commit(&self, changes: Changes) -> Result<(), DbError>;

    #[document(
        parent = "Database",
        summary = "The database as it is now, to read and scan while commits go on: everything read from it is from one commit, so a request that reads the tip and then the state never mixes two. RocksDB keeps the files it reads alive until it is dropped; MemoryDb copies its records.",
    )]
    fn snapshot(&self) -> Arc<dyn Committed>;

    #[document(
        parent = "Database",
        summary = "Deletes every record in a column and commits changes with it, atomically: after a crash, either the column is empty and the changes are there, or neither. Backends delete the column as a range, so this is quick however much it holds. Meant for the node's own columns, such as an index it stops keeping; never for the chain's state, which undo records would no longer match.",
        params = {
            column = "The column to empty.",
            changes = "What to commit with it, written after the column is emptied, so they may write to it too.",
        },
    )]
    fn clear(&self, column: Column, changes: Changes) -> Result<(), DbError>;
}

/// Where a scan begins: start, or the prefix when start comes before it.
fn scan_from<'a>(prefix: &'a [u8], start: &'a [u8]) -> &'a [u8] {
    prefix.max(start)
}

#[document(
    summary = "Uncommitted changes on top of something readable. Reads see this layer's changes first, then fall through. Merge a finished layer into the one below it, or drop it to throw its changes away.",
    category = "Storage",
    see_also = "Database::commit",
    example = "let block = Arc::new(State::new(db.clone()));\nlet tx = Arc::new(State::new(block.clone()));\n// ... run the transaction against tx ...\nblock.merge(tx.take_changes());\ndb.commit(block.take_changes())?;",
)]
pub struct State {
    base: Arc<dyn Committed>,
    changes: Mutex<ByColumn<Option<Vec<u8>>>>,
    /// What this layer read from below it, when it records reads.
    reads: Option<Mutex<Keys>>,
    /// The ranges this layer scanned from below, when it records reads.
    scans: Option<Mutex<Vec<Scanned>>>,
}

#[document(
    summary = "A range a recording State scanned: a column's keys with a prefix, from a key on, through the last one visited when the scan stopped early or to the prefix's end when it ran out. A transaction that wrote any key in it changed what the scan saw.",
    category = "Storage",
    see_also = "State::take_scans",
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scanned {
    #[document(summary = "The column scanned.")]
    pub column: Column,
    #[document(summary = "What every key it visited starts with.")]
    pub prefix: Vec<u8>,
    #[document(summary = "The first key it could visit.")]
    pub from: Vec<u8>,
    #[document(summary = "The last key it visited, when it stopped early; None when it ran to the prefix's end.")]
    pub to: Option<Vec<u8>>,
}

impl Scanned {
    #[document(parent = "Scanned", summary = "Whether a key falls in the range: one written there changes what the scan saw.")]
    pub fn covers(&self, column: Column, key: &[u8]) -> bool {
        column == self.column && key.starts_with(&self.prefix) && key >= self.from.as_slice() && self.to.as_deref().is_none_or(|to| key <= to)
    }
}

impl State {
    #[document(
        parent = "State",
        summary = "An empty layer over base.",
        params = { base = "The database, or the State below this one." },
    )]
    pub fn new(base: Arc<dyn Committed>) -> State {
        State {
            base,
            changes: Mutex::default(),
            reads: None,
            scans: None,
        }
    }

    #[document(
        parent = "State",
        summary = "An empty layer over base that remembers every key it reads from below: those it, or a layer on top of it, read without having written them first. What a transaction run ahead of its turn depended on.",
        params = { base = "The database, or the State below this one." },
        see_also = "State::take_reads",
    )]
    pub fn recording(base: Arc<dyn Committed>) -> State {
        State {
            reads: Some(Mutex::default()),
            scans: Some(Mutex::default()),
            ..State::new(base)
        }
    }

    #[document(
        parent = "State",
        summary = "Takes the keys a recording layer read from below, leaving it none. Empty for a layer that does not record.",
    )]
    pub fn take_reads(&self) -> Keys {
        match &self.reads {
            Some(reads) => std::mem::take(&mut *reads.lock().expect("a state layer is never poisoned")),
            None => Keys::new(),
        }
    }

    #[document(
        parent = "State",
        summary = "Takes the ranges a recording layer scanned from below, leaving it none: what a transaction run ahead of its turn depended on, besides the keys it read. Empty for a layer that does not record.",
        see_also = "Scanned",
    )]
    pub fn take_scans(&self) -> Vec<Scanned> {
        match &self.scans {
            Some(scans) => std::mem::take(&mut *scans.lock().expect("a state layer is never poisoned")),
            None => Vec::new(),
        }
    }

    #[document(
        parent = "State",
        summary = "Reads a value, seeing this layer's own changes first.",
    )]
    pub fn get(&self, column: Column, key: &[u8]) -> Result<Option<Vec<u8>>, DbError> {
        self.read(column, key)
    }

    #[document(
        parent = "State",
        summary = "Sets a value in this layer.",
    )]
    pub fn put(&self, column: Column, key: Vec<u8>, value: Vec<u8>) {
        self.lock().entry(column).or_default().insert(key, Some(value));
    }

    #[document(
        parent = "State",
        summary = "Deletes a value in this layer.",
    )]
    pub fn delete(&self, column: Column, key: Vec<u8>) {
        self.lock().entry(column).or_default().insert(key, None);
    }

    #[document(
        parent = "State",
        summary = "Reads and decodes a stored record.",
    )]
    pub fn get_value<T>(&self, column: Column, key: &[u8]) -> Result<Option<T>, DbError>
    where
        T: for<'de> wincode::SchemaRead<'de, wincode::config::DefaultConfig, Dst = T>,
    {
        let Some(bytes) = self.get(column, key)? else {
            return Ok(None);
        };

        wincode::deserialize(&bytes)
            .map(Some)
            .map_err(|err| DbError::Corrupt(err.to_string()))
    }

    #[document(
        parent = "State",
        summary = "Encodes and stores a record.",
    )]
    pub fn put_value<T: Hashable>(&self, column: Column, key: Vec<u8>, value: &T) {
        self.put(column, key, value.hashable_bytes_with_tag());
    }

    #[document(
        parent = "State",
        summary = "What this layer changed in these columns, without taking it: each key, and its new value or None for a delete.",
    )]
    pub fn changed(&self, columns: &[Column]) -> Vec<(Column, Vec<u8>, Option<Vec<u8>>)> {
        let changes = self.lock();

        columns
            .iter()
            .filter_map(|column| Some((*column, changes.get(column)?)))
            .flat_map(|(column, keys)| keys.iter().map(move |(key, value)| (column, key.clone(), value.clone())))
            .collect()
    }

    #[document(
        parent = "State",
        summary = "Takes this layer's changes, leaving it empty.",
        returns = "The changes, for merge on the layer below or Database::commit.",
    )]
    pub fn take_changes(&self) -> Changes {
        let changes = std::mem::take(&mut *self.lock());

        // In order already, column by column: the map builds from it in one pass.
        changes.into_iter().flat_map(|(column, keys)| keys.into_iter().map(move |(key, value)| ((column, key), value))).collect()
    }

    #[document(
        parent = "State",
        summary = "Applies a finished layer's changes on top of this one's.",
    )]
    pub fn merge(&self, changes: Changes) {
        let mut layer = self.lock();

        for ((column, key), value) in changes {
            layer.entry(column).or_default().insert(key, value);
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ByColumn<Option<Vec<u8>>>> {
        self.changes.lock().expect("a state layer is never poisoned")
    }
}

impl Read for State {
    fn read(&self, column: Column, key: &[u8]) -> Result<Option<Vec<u8>>, DbError> {
        if let Some(value) = self.lock().get(&column).and_then(|keys| keys.get(key)) {
            return Ok(value.clone());
        }

        if let Some(reads) = &self.reads {
            reads.lock().expect("a state layer is never poisoned").insert((column, key.to_vec()));
        }

        self.base.read(column, key)
    }
}

/// A scan as it goes: the visitor, the last key it was given, and whether it stopped there.
struct Visiting<'a> {
    visit: &'a mut dyn FnMut(&[u8], &[u8]) -> bool,
    last: Option<Vec<u8>>,
    stopped: bool,
}

impl Visiting<'_> {
    /// Gives the visitor a key; false once it stops.
    fn emit(&mut self, key: &[u8], value: &[u8]) -> bool {
        self.last = Some(key.to_vec());
        self.stopped = !(self.visit)(key, value);

        !self.stopped
    }
}

impl Committed for State {
    /// Visits the keys as the layers have them together: this layer's changes over what is below,
    /// a value set here in place of the one below, a key deleted here left out.
    fn scan(&self, column: Column, prefix: &[u8], start: &[u8], visit: &mut dyn FnMut(&[u8], &[u8]) -> bool) -> Result<(), DbError> {
        let from = scan_from(prefix, start);
        // Copied out first, so visit can read this layer without waiting on its lock.
        let own: Vec<(Vec<u8>, Option<Vec<u8>>)> = self
            .lock()
            .get(&column)
            .into_iter()
            .flat_map(|keys| keys.range::<[u8], _>((std::ops::Bound::Included(from), std::ops::Bound::Unbounded)))
            .take_while(|(key, _)| key.starts_with(prefix))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        let mut own = own.into_iter().peekable();
        let mut visiting = Visiting { visit, last: None, stopped: false };

        self.base.scan(column, prefix, start, &mut |key, value| {
            // This layer's keys before the one below, then this one, as set or deleted here.
            while let Some((own_key, own_value)) = own.next_if(|(own_key, _)| own_key.as_slice() < key) {
                if let Some(own_value) = own_value
                    && !visiting.emit(&own_key, &own_value)
                {
                    return false;
                }
            }

            match own.next_if(|(own_key, _)| own_key.as_slice() == key) {
                Some((_, Some(own_value))) => visiting.emit(key, &own_value),
                Some((_, None)) => true,
                None => visiting.emit(key, value),
            }
        })?;

        // And this layer's keys past everything below.
        if !visiting.stopped {
            for (key, value) in own {
                if let Some(value) = value
                    && !visiting.emit(&key, &value)
                {
                    break;
                }
            }
        }

        if let Some(scans) = &self.scans {
            let to = visiting.stopped.then_some(visiting.last).flatten();
            scans.lock().expect("a state layer is never poisoned").push(Scanned { column, prefix: prefix.to_vec(), from: from.to_vec(), to });
        }

        Ok(())
    }
}

/// The key of an address's record. The tag is part of it, so a key and a contract that share
/// their 32 bytes are still two records.
pub fn address_key(address: &Address) -> Vec<u8> {
    address.hashable_bytes_with_tag()
}

/// The key of one entry in a contract's storage.
pub fn storage_key(contract: &Address, key: &[u8]) -> Vec<u8> {
    let mut out = address_key(contract);
    out.extend_from_slice(key);

    out
}

/// The key of one entry in one of a contract's namespaces, in Column::Namespaces. The name goes
/// in with its length in front, so no two namespaces' keys ever meet; the contract's own space is
/// the one with no name. Names are at most 255 bytes.
pub fn namespace_key(contract: &Address, name: &[u8], key: &[u8]) -> Vec<u8> {
    let mut out = address_key(contract);
    out.push(u8::try_from(name.len()).expect("namespace names are at most 255 bytes"));
    out.extend_from_slice(name);
    out.extend_from_slice(key);

    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A temporary directory, removed when dropped.
    #[cfg(feature = "rocksdb")]
    pub(crate) struct TempDir(pub(crate) std::path::PathBuf);

    #[cfg(feature = "rocksdb")]
    impl TempDir {
        pub(crate) fn new(name: &str) -> TempDir {
            let path = std::env::temp_dir().join(format!("chainv4-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);

            TempDir(path)
        }
    }

    #[cfg(feature = "rocksdb")]
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A fresh backend for one test, with whatever keeps it alive until the test ends.
    struct Backend {
        db: Arc<dyn Database>,
        #[cfg(feature = "rocksdb")]
        _dir: Option<TempDir>,
    }

    impl Backend {
        fn base(&self) -> Arc<dyn Committed> {
            self.db.clone()
        }
    }

    /// Every backend this build has. A test that loops over these runs against each.
    #[cfg_attr(not(feature = "rocksdb"), allow(unused_variables))]
    fn backends(name: &str) -> Vec<Backend> {
        vec![
            Backend {
                db: Arc::new(MemoryDb::default()),
                #[cfg(feature = "rocksdb")]
                _dir: None,
            },
            #[cfg(feature = "rocksdb")]
            {
                let dir = TempDir::new(name);

                Backend {
                    db: Arc::new(RocksDb::open(&dir.0).unwrap()),
                    _dir: Some(dir),
                }
            },
        ]
    }

    #[test]
    fn scans_visit_a_prefix_in_order_from_a_start() {
        for backend in backends("scan") {
            let db = &backend.db;
            let state = State::new(backend.base());

            for key in [&b"b2"[..], b"a1", b"b1", b"b3", b"c1", b"b"] {
                state.put(Column::Finality, key.to_vec(), key.to_vec());
            }

            state.put(Column::Accounts, b"b0".to_vec(), b"other column".to_vec());
            db.commit(state.take_changes()).unwrap();
            let scan = |prefix: &[u8], start: &[u8], stop_after: usize| {
                let mut seen = Vec::new();
                db.scan(Column::Finality, prefix, start, &mut |key, value| {
                    assert_eq!(key, value);
                    seen.push(String::from_utf8(key.to_vec()).unwrap());

                    seen.len() < stop_after
                })
                .unwrap();

                seen
            };

            assert_eq!(scan(b"b", b"b", 10), ["b", "b1", "b2", "b3"]);
            assert_eq!(scan(b"b", b"b2", 10), ["b2", "b3"]);
            assert_eq!(scan(b"b", b"a", 10), ["b", "b1", "b2", "b3"], "a start before the prefix");
            assert_eq!(scan(b"b", b"b", 2), ["b", "b1"], "stopped early");
            assert!(scan(b"d", b"d", 10).is_empty());
        }
    }

    #[test]
    fn a_snapshot_reads_and_scans_one_commit_whatever_comes_after() {
        for backend in backends("snapshot") {
            let db = &backend.db;
            let state = State::new(backend.base());
            state.put(Column::Meta, b"tip".to_vec(), b"1".to_vec());
            state.put(Column::Accounts, b"a".to_vec(), b"at 1".to_vec());
            db.commit(state.take_changes()).unwrap();
            let snapshot = db.snapshot();

            // The next commit moves the tip, rewrites a, deletes nothing it held and adds b.
            let state = State::new(backend.base());
            state.put(Column::Meta, b"tip".to_vec(), b"2".to_vec());
            state.put(Column::Accounts, b"a".to_vec(), b"at 2".to_vec());
            state.put(Column::Accounts, b"b".to_vec(), b"at 2".to_vec());
            db.commit(state.take_changes()).unwrap();
            let state = State::new(backend.base());
            state.delete(Column::Accounts, b"a".to_vec());
            db.commit(state.take_changes()).unwrap();

            assert_eq!(snapshot.read(Column::Meta, b"tip").unwrap(), Some(b"1".to_vec()));
            assert_eq!(snapshot.read(Column::Accounts, b"a").unwrap(), Some(b"at 1".to_vec()));
            assert_eq!(snapshot.read(Column::Accounts, b"b").unwrap(), None);
            let mut seen = Vec::new();
            snapshot.scan(Column::Accounts, b"", b"", &mut |key, value| {
                seen.push((key.to_vec(), value.to_vec()));
                true
            })
            .unwrap();
            assert_eq!(seen, [(b"a".to_vec(), b"at 1".to_vec())]);

            // A State over it reads the snapshot too, while the database has moved on.
            assert_eq!(State::new(snapshot.clone()).get(Column::Meta, b"tip").unwrap(), Some(b"1".to_vec()));
            assert_eq!(db.read(Column::Meta, b"tip").unwrap(), Some(b"2".to_vec()));
            assert_eq!(db.read(Column::Accounts, b"a").unwrap(), None);
        }
    }

    #[test]
    fn a_layer_keeps_its_changes_by_column_and_hands_them_back_in_order() {
        let db: Arc<dyn Database> = Arc::new(MemoryDb::default());
        let state = State::new(db.clone());
        state.put(Column::Names, b"b".to_vec(), b"1".to_vec());
        state.put(Column::Accounts, b"z".to_vec(), b"2".to_vec());
        state.delete(Column::Accounts, b"a".to_vec());
        state.merge([((Column::Names, b"a".to_vec()), Some(b"3".to_vec())), ((Column::Accounts, b"z".to_vec()), Some(b"4".to_vec()))].into());

        assert_eq!(state.get(Column::Accounts, b"z").unwrap(), Some(b"4".to_vec()), "the merged layer's value wins");
        assert_eq!(state.get(Column::Names, b"z").unwrap(), None, "another column's key is another key");
        assert_eq!(state.changed(&[Column::Names]), [(Column::Names, b"a".to_vec(), Some(b"3".to_vec())), (Column::Names, b"b".to_vec(), Some(b"1".to_vec()))]);
        assert_eq!(
            state.take_changes().into_iter().collect::<Vec<_>>(),
            [
                ((Column::Accounts, b"a".to_vec()), None),
                ((Column::Accounts, b"z".to_vec()), Some(b"4".to_vec())),
                ((Column::Names, b"a".to_vec()), Some(b"3".to_vec())),
                ((Column::Names, b"b".to_vec()), Some(b"1".to_vec())),
            ]
        );
        assert!(state.take_changes().is_empty());
    }

    #[test]
    fn a_layer_scans_its_own_changes_over_what_is_below() {
        for backend in backends("layered-scan") {
            let state = State::new(backend.base());

            for key in [&b"k1"[..], b"k3", b"k5", b"k7"] {
                state.put(Column::Storage, key.to_vec(), b"below".to_vec());
            }

            state.put(Column::Storage, b"other".to_vec(), b"below".to_vec());
            backend.db.commit(state.take_changes()).unwrap();
            let block = Arc::new(State::new(backend.base()));
            block.put(Column::Storage, b"k3".to_vec(), b"block".to_vec());
            block.put(Column::Storage, b"k4".to_vec(), b"block".to_vec());
            let tx = State::new(block.clone());
            tx.delete(Column::Storage, b"k5".to_vec());
            tx.put(Column::Storage, b"k0".to_vec(), b"tx".to_vec());
            tx.put(Column::Storage, b"k9".to_vec(), b"tx".to_vec());
            let scan = |layer: &State, start: &[u8], stop_after: usize| {
                let mut seen = Vec::new();
                layer
                    .scan(Column::Storage, b"k", start, &mut |key, value| {
                        seen.push(format!("{}={}", String::from_utf8_lossy(key), String::from_utf8_lossy(value)));
                        seen.len() < stop_after
                    })
                    .unwrap();
                seen
            };

            // Set here wins, deleted here is gone, new here is in, all in key order.
            assert_eq!(scan(&tx, b"k", 99), ["k0=tx", "k1=below", "k3=block", "k4=block", "k7=below", "k9=tx"]);
            assert_eq!(scan(&tx, b"k4", 99), ["k4=block", "k7=below", "k9=tx"], "from a start");
            assert_eq!(scan(&tx, b"k", 3), ["k0=tx", "k1=below", "k3=block"], "stopped early");
            assert_eq!(scan(&block, b"k", 99), ["k1=below", "k3=block", "k4=block", "k5=below", "k7=below"], "the layer below sees none of the one above");
        }
    }

    #[test]
    fn a_recording_layer_notes_the_ranges_it_scanned() {
        let db: Arc<dyn Database> = Arc::new(MemoryDb::default());
        let state = State::new(db.clone());

        for key in [&b"a1"[..], b"a2", b"a3"] {
            state.put(Column::Storage, key.to_vec(), b"v".to_vec());
        }

        db.commit(state.take_changes()).unwrap();
        let recording = State::recording(db.clone());
        recording.scan(Column::Storage, b"a", b"a", &mut |_, _| true).unwrap();
        recording.scan(Column::Storage, b"a", b"a2", &mut |key, _| key != b"a2").unwrap();
        let scans = recording.take_scans();

        assert_eq!(scans, [
            Scanned { column: Column::Storage, prefix: b"a".to_vec(), from: b"a".to_vec(), to: None },
            Scanned { column: Column::Storage, prefix: b"a".to_vec(), from: b"a2".to_vec(), to: Some(b"a2".to_vec()) },
        ]);
        // A key written anywhere in the first, even one that was never there, spoils it.
        assert!(scans[0].covers(Column::Storage, b"a9"));
        assert!(!scans[0].covers(Column::Storage, b"b1") && !scans[0].covers(Column::Accounts, b"a1"));
        // The second saw only a2: a1 is before it, a3 past where it stopped.
        assert!(scans[1].covers(Column::Storage, b"a2"));
        assert!(!scans[1].covers(Column::Storage, b"a1") && !scans[1].covers(Column::Storage, b"a3"));
        assert!(recording.take_scans().is_empty());
        assert!(State::new(db).take_scans().is_empty(), "a layer that does not record notes none");
    }

    #[test]
    fn clearing_empties_one_column_and_commits_with_it() {
        for backend in backends("clear") {
            let db = &backend.db;
            let state = State::new(backend.base());

            for key in [&b"a"[..], b"m", b"z", b"\xff\xff"] {
                state.put(Column::Events, key.to_vec(), b"old".to_vec());
            }

            state.put(Column::Meta, b"keep".to_vec(), b"1".to_vec());
            state.put(Column::Meta, b"drop".to_vec(), b"1".to_vec());
            db.commit(state.take_changes()).unwrap();

            let state = State::new(backend.base());
            state.delete(Column::Meta, b"drop".to_vec());
            state.put(Column::Events, b"new".to_vec(), b"after".to_vec());
            db.clear(Column::Events, state.take_changes()).unwrap();
            let mut left = Vec::new();
            db.scan(Column::Events, b"", b"", &mut |key, value| {
                left.push((key.to_vec(), value.to_vec()));

                true
            })
            .unwrap();

            assert_eq!(left, [(b"new".to_vec(), b"after".to_vec())], "the last key goes too, and the changes land after");
            assert_eq!(db.read(Column::Meta, b"keep").unwrap(), Some(b"1".to_vec()), "other columns stay");
            assert_eq!(db.read(Column::Meta, b"drop").unwrap(), None);

            // An empty column clears too.
            db.clear(Column::Finality, Changes::new()).unwrap();
        }
    }

    #[test]
    fn a_recording_layer_lists_what_it_read_from_below() {
        let db = Arc::new(MemoryDb::default());
        let base = State::new(db.clone());
        base.put(Column::Accounts, b"below".to_vec(), b"1".to_vec());
        db.commit(base.take_changes()).unwrap();
        let recording = Arc::new(State::recording(db));
        recording.put(Column::Accounts, b"own".to_vec(), b"2".to_vec());
        // A layer on top reads through it, as a transaction's calls do.
        let top = State::new(recording.clone());
        top.put(Column::Storage, b"top".to_vec(), b"3".to_vec());

        for (column, key) in [(Column::Accounts, &b"below"[..]), (Column::Accounts, b"own"), (Column::Storage, b"top"), (Column::Storage, b"missing")] {
            top.get(column, key).unwrap();
        }

        let read = recording.take_reads();

        // Its own writes and the top layer's are not reads from below; a missing key is.
        assert_eq!(read, Keys::from([(Column::Accounts, b"below".to_vec()), (Column::Storage, b"missing".to_vec())]));
        assert!(recording.take_reads().is_empty());
        assert!(State::new(Arc::new(MemoryDb::default())).take_reads().is_empty());
    }

    #[test]
    fn commits_reach_the_backend() {
        for backend in backends("commit") {
            let db = &backend.db;
            let state = State::new(backend.base());
            state.put(Column::Accounts, b"a".to_vec(), b"1".to_vec());
            state.put(Column::Storage, b"a".to_vec(), b"2".to_vec());

            assert_eq!(db.read(Column::Accounts, b"a").unwrap(), None);

            db.commit(state.take_changes()).unwrap();

            assert_eq!(db.read(Column::Accounts, b"a").unwrap(), Some(b"1".to_vec()));
            assert_eq!(db.read(Column::Storage, b"a").unwrap(), Some(b"2".to_vec()));
            assert_eq!(db.read(Column::Contracts, b"a").unwrap(), None);
        }
    }

    #[test]
    fn layers_read_through_and_merge_down() {
        for backend in backends("layers") {
            let block = Arc::new(State::new(backend.base()));
            block.put(Column::Accounts, b"a".to_vec(), b"block".to_vec());
            let tx = State::new(block.clone());

            assert_eq!(tx.get(Column::Accounts, b"a").unwrap(), Some(b"block".to_vec()));

            tx.put(Column::Accounts, b"a".to_vec(), b"tx".to_vec());

            assert_eq!(block.get(Column::Accounts, b"a").unwrap(), Some(b"block".to_vec()));

            block.merge(tx.take_changes());

            assert_eq!(block.get(Column::Accounts, b"a").unwrap(), Some(b"tx".to_vec()));
        }
    }

    #[test]
    fn dropping_a_layer_discards_it() {
        for backend in backends("discard") {
            let db = &backend.db;
            let block = Arc::new(State::new(backend.base()));
            let tx = State::new(block.clone());
            tx.put(Column::Accounts, b"a".to_vec(), b"1".to_vec());
            drop(tx);
            db.commit(block.take_changes()).unwrap();

            assert_eq!(db.read(Column::Accounts, b"a").unwrap(), None);
        }
    }

    #[test]
    fn deletes_reach_the_backend() {
        for backend in backends("delete") {
            let db = &backend.db;
            let state = State::new(backend.base());
            state.put(Column::Accounts, b"a".to_vec(), b"1".to_vec());
            db.commit(state.take_changes()).unwrap();
            state.delete(Column::Accounts, b"a".to_vec());

            assert_eq!(state.get(Column::Accounts, b"a").unwrap(), None);

            db.commit(state.take_changes()).unwrap();

            assert_eq!(db.read(Column::Accounts, b"a").unwrap(), None);
        }
    }

    #[test]
    fn address_kinds_get_separate_keys() {
        assert_ne!(
            address_key(&Address::Ed25519([1; 32])),
            address_key(&Address::Contract([1; 32]))
        );
    }

    #[test]
    fn namespaces_never_share_keys() {
        let contract = Address::Contract([1; 32]);

        assert_ne!(namespace_key(&contract, b"ab", b"c"), namespace_key(&contract, b"a", b"bc"));
        assert_ne!(namespace_key(&contract, b"", b"\x01ak"), namespace_key(&contract, b"a", b"k"), "the own space is apart too");
        assert_ne!(namespace_key(&contract, b"a", b"k"), namespace_key(&Address::Contract([2; 32]), b"a", b"k"));
    }
}
