//! What a read-only call reads, and the same call run again over nothing but those reads, proven:
//! how an app runs a contract's read itself, on state the node proves, rather than take the node's
//! answer. The node runs the call over Recording, which notes every entry the call reads; it sends
//! each with a StateProof. The app checks the proofs against a header it checked, and runs the call
//! over ProvenEntries, which answers those entries alone: anything else the call reads, the node left out.

use {
    super::{Column, Committed, DbError, Read},
    r#macro::document,
    std::collections::{BTreeMap, BTreeSet},
    std::sync::{Arc, Mutex, PoisonError},
};

#[document(
    summary = "A database a read-only call runs over, which notes every entry the call reads and whether it scanned: what the node sends proofs of with the call's answer.",
    category = "Storage",
    see_also = "ProvenEntries",
)]
pub struct Recording {
    base: Arc<dyn Committed>,
    read: Mutex<BTreeSet<(Column, Vec<u8>)>>,
    scanned: Mutex<BTreeSet<Column>>,
}

impl Recording {
    #[document(parent = "Recording", summary = "Notes the reads of a call run over base.")]
    pub fn new(base: Arc<dyn Committed>) -> Recording {
        Recording { base, read: Mutex::default(), scanned: Mutex::default() }
    }

    #[document(parent = "Recording", summary = "Every entry read, by column and key, in order, each once.")]
    pub fn reads(&self) -> Vec<(Column, Vec<u8>)> {
        self.read.lock().unwrap_or_else(PoisonError::into_inner).iter().cloned().collect()
    }

    #[document(parent = "Recording", summary = "Whether the call scanned a range of keys, which no proof of single entries covers.")]
    pub fn scanned(&self) -> bool {
        !self.scanned.lock().unwrap_or_else(PoisonError::into_inner).is_empty()
    }

    #[document(parent = "Recording", summary = "Every column the call scanned a range of, each once.")]
    pub fn scanned_columns(&self) -> Vec<Column> {
        self.scanned.lock().unwrap_or_else(PoisonError::into_inner).iter().copied().collect()
    }
}

impl Read for Recording {
    fn read(&self, column: Column, key: &[u8]) -> Result<Option<Vec<u8>>, DbError> {
        self.read.lock().unwrap_or_else(PoisonError::into_inner).insert((column, key.to_vec()));

        self.base.read(column, key)
    }
}

impl Committed for Recording {
    fn scan(&self, column: Column, prefix: &[u8], start: &[u8], visit: &mut dyn FnMut(&[u8], &[u8]) -> bool) -> Result<(), DbError> {
        self.scanned.lock().unwrap_or_else(PoisonError::into_inner).insert(column);

        self.base.scan(column, prefix, start, visit)
    }
}

#[document(
    summary = "A database of proven entries alone, each a value or proven absent: what an app runs a call over to get its own answer. Reading anything else fails the call, and is noted, so the app can ask for it; so does scanning.",
    category = "Storage",
    see_also = "Recording",
)]
pub struct ProvenEntries {
    entries: BTreeMap<(Column, Vec<u8>), Option<Vec<u8>>>,
    missing: Mutex<Option<(Column, Vec<u8>)>>,
    scanned: Mutex<bool>,
}

impl ProvenEntries {
    #[document(parent = "ProvenEntries", summary = "ProvenEntries entries: each column and key with its value, None for one proven absent. Check the proofs first: this takes them as given.")]
    pub fn new(entries: impl IntoIterator<Item = (Column, Vec<u8>, Option<Vec<u8>>)>) -> ProvenEntries {
        ProvenEntries {
            entries: entries.into_iter().map(|(column, key, value)| ((column, key), value)).collect(),
            missing: Mutex::new(None),
            scanned: Mutex::new(false),
        }
    }

    #[document(parent = "ProvenEntries", summary = "The first entry the call read that was not among them: what to ask the node for next.")]
    pub fn missing(&self) -> Option<(Column, Vec<u8>)> {
        self.missing.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    #[document(parent = "ProvenEntries", summary = "Whether the call tried to scan a range of keys, which proven entries cannot answer.")]
    pub fn scanned(&self) -> bool {
        *self.scanned.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Read for ProvenEntries {
    fn read(&self, column: Column, key: &[u8]) -> Result<Option<Vec<u8>>, DbError> {
        match self.entries.get(&(column, key.to_vec())) {
            Some(value) => Ok(value.clone()),
            None => {
                self.missing.lock().unwrap_or_else(PoisonError::into_inner).get_or_insert_with(|| (column, key.to_vec()));

                Err(DbError::Backend(format!("{} {} was not proven", column.name(), crate::structs::hex::to_hex(key))))
            }
        }
    }
}

impl Committed for ProvenEntries {
    fn scan(&self, _column: Column, _prefix: &[u8], _start: &[u8], _visit: &mut dyn FnMut(&[u8], &[u8]) -> bool) -> Result<(), DbError> {
        *self.scanned.lock().unwrap_or_else(PoisonError::into_inner) = true;

        Err(DbError::Backend("a range of keys cannot be proven entry by entry".to_string()))
    }
}

#[cfg(test)]
mod tests {
    use {super::*, crate::db::{Changes, Database, MemoryDb}};

    #[test]
    fn a_recording_notes_each_read_and_scan_and_proven_entries_answer_alone() {
        let db = Arc::new(MemoryDb::default());
        db.commit(Changes::from([((Column::Storage, b"a".to_vec()), Some(b"1".to_vec()))])).unwrap();
        let recording = Recording::new(db.clone());

        assert_eq!(recording.read(Column::Storage, b"a").unwrap(), Some(b"1".to_vec()));
        assert_eq!(recording.read(Column::Storage, b"b").unwrap(), None);
        recording.read(Column::Storage, b"a").unwrap();
        assert_eq!(recording.reads(), [(Column::Storage, b"a".to_vec()), (Column::Storage, b"b".to_vec())], "each once, absent ones too");
        assert!(!recording.scanned());
        recording.scan(Column::Storage, b"", b"", &mut |_, _| true).unwrap();
        assert!(recording.scanned());

        let proven = ProvenEntries::new([(Column::Storage, b"a".to_vec(), Some(b"1".to_vec())), (Column::Storage, b"b".to_vec(), None)]);
        assert_eq!(proven.read(Column::Storage, b"a").unwrap(), Some(b"1".to_vec()));
        assert_eq!(proven.read(Column::Storage, b"b").unwrap(), None, "proven absent");
        assert!(proven.read(Column::Accounts, b"c").is_err());
        assert!(proven.read(Column::Accounts, b"d").is_err());
        assert_eq!(proven.missing(), Some((Column::Accounts, b"c".to_vec())), "the first one missing");
        assert!(proven.scan(Column::Storage, b"", b"", &mut |_, _| true).is_err() && proven.scanned());
    }
}
