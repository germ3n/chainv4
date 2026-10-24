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

mod memory;
#[cfg(feature = "rocksdb")]
mod rocksdb;

pub use self::memory::MemoryDb;
#[cfg(feature = "rocksdb")]
pub use self::rocksdb::RocksDb;

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
}

impl Hashable for Column {
    const TAG_LEN: usize = 1;
}

impl Column {
    pub const ALL: [Column; 13] = [
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
    summary = "A storage backend. Reads come through Read; writes only ever arrive as a whole set of changes, which the backend must apply atomically: after a crash, either all of them are there or none are.",
    category = "Storage",
    see_also = "RocksDb, MemoryDb, State",
)]
pub trait Database: Read {
    #[document(
        parent = "Database",
        summary = "Writes a set of changes atomically.",
        params = { changes = "Usually a block's State, from take_changes. None values delete." },
    )]
    fn commit(&self, changes: Changes) -> Result<(), DbError>;

    #[document(
        parent = "Database",
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
    base: Arc<dyn Read>,
    changes: Mutex<Changes>,
    /// What this layer read from below it, when it records reads.
    reads: Option<Mutex<Keys>>,
}

impl State {
    #[document(
        parent = "State",
        summary = "An empty layer over base.",
        params = { base = "The database, or the State below this one." },
    )]
    pub fn new(base: Arc<dyn Read>) -> State {
        State {
            base,
            changes: Mutex::new(Changes::new()),
            reads: None,
        }
    }

    #[document(
        parent = "State",
        summary = "An empty layer over base that remembers every key it reads from below: those it, or a layer on top of it, read without having written them first. What a transaction run ahead of its turn depended on.",
        params = { base = "The database, or the State below this one." },
        see_also = "State::take_reads",
    )]
    pub fn recording(base: Arc<dyn Read>) -> State {
        State {
            reads: Some(Mutex::default()),
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
        self.lock().insert((column, key), Some(value));
    }

    #[document(
        parent = "State",
        summary = "Deletes a value in this layer.",
    )]
    pub fn delete(&self, column: Column, key: Vec<u8>) {
        self.lock().insert((column, key), None);
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
        summary = "Takes this layer's changes, leaving it empty.",
        returns = "The changes, for merge on the layer below or Database::commit.",
    )]
    pub fn take_changes(&self) -> Changes {
        std::mem::take(&mut *self.lock())
    }

    #[document(
        parent = "State",
        summary = "Applies a finished layer's changes on top of this one's.",
    )]
    pub fn merge(&self, changes: Changes) {
        self.lock().extend(changes);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Changes> {
        self.changes.lock().expect("a state layer is never poisoned")
    }
}

impl Read for State {
    fn read(&self, column: Column, key: &[u8]) -> Result<Option<Vec<u8>>, DbError> {
        if let Some(value) = self.lock().get(&(column, key.to_vec())) {
            return Ok(value.clone());
        }

        if let Some(reads) = &self.reads {
            reads.lock().expect("a state layer is never poisoned").insert((column, key.to_vec()));
        }

        self.base.read(column, key)
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
        fn base(&self) -> Arc<dyn Read> {
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
}
