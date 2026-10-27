//! Chain state as it was at a recent height (docs/storage.md#history). For each key a main-chain
//! block changed in the state columns or the state tree, Column::History keeps the value it held
//! just before that block, under the key and then the block's height. So the value at height h is
//! the one the first change after h replaced, or, when nothing changed it since, the current one.
//! A read at a height seeks once, and the state tree's nodes are kept alike, so proofs at a kept
//! height cost what proofs at the tip do. Blocks write their entries in the commit that runs them,
//! a reorg's undo takes them back, and the node prunes those past its window as the tip moves.
//!
//! Keys go in escaped and terminated, a 0 byte as 0 then 255 and the end as two 0s, so they sort
//! as the keys do and none is a prefix of another: a scan of a prefix finds the history of every
//! key with it.

use {
    super::{Column, Committed, DbError, Read, STATE_COLUMNS, scan_from},
    crate::crypto::hashable::Hashable,
    r#macro::document,
    std::collections::BTreeSet,
    std::sync::Arc,
};

/// What a height's list of the keys its block changed is kept under, apart from every column's
/// entries, whose tags are all below it.
const LISTS: u8 = 0xff;

#[document(
    summary = "Whether a column's changes are kept in history: the state columns, and the state tree, whose nodes proofs at a height read.",
    category = "Storage",
    see_also = "Historic",
)]
pub fn kept_in_history(column: Column) -> bool {
    column == Column::StateTree || STATE_COLUMNS.contains(&column)
}

/// A key as history keeps it, without its height: the column's tag, then the key escaped and
/// terminated.
fn entry_prefix(column: Column, key: &[u8]) -> Vec<u8> {
    let mut out = escaped(column, key);
    out.extend_from_slice(&[0, 0]);

    out
}

/// The column's tag and the key escaped, unterminated: a prefix of every longer key's.
fn escaped(column: Column, key: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(key.len() + 11);
    out.push(column.hashable_bytes_with_tag()[0]);

    for byte in key {
        out.push(*byte);

        if *byte == 0 {
            out.push(0xff);
        }
    }

    out
}

#[document(
    summary = "Where history keeps what a key held just before the block at a height changed it.",
    category = "Storage",
    see_also = "Historic",
)]
pub fn history_key(column: Column, key: &[u8], height: u64) -> Vec<u8> {
    let mut out = entry_prefix(column, key);
    out.extend_from_slice(&height.to_be_bytes());

    out
}

#[document(summary = "A value as history keeps what a block replaced: None for a key it added.", category = "Storage", see_also = "history_key")]
pub fn replaced(value: &Option<Vec<u8>>) -> Vec<u8> {
    wincode::serialize(value).expect("bytes encode")
}

#[document(summary = "Where history lists the keys the block at a height changed, for pruning them together.", category = "Storage", see_also = "Historic")]
pub fn history_list_key(height: u64) -> Vec<u8> {
    [&[LISTS][..], &height.to_be_bytes()].concat()
}

/// A history key read back: its key, without the column's tag, and its height. None for a list's.
fn parse(entry: &[u8]) -> Option<(Vec<u8>, u64)> {
    let mut key = Vec::new();
    let mut at = 1;

    while at + 1 < entry.len() {
        match (entry[at], entry[at + 1]) {
            (0, 0) => {
                let height = u64::from_be_bytes(entry.get(at + 2..at + 10)?.try_into().ok()?);

                return Some((key, height));
            }
            (0, 0xff) => {
                key.push(0);
                at += 2;
            }
            (byte, _) => {
                key.push(byte);
                at += 1;
            }
        }
    }

    None
}

#[document(
    summary = "The database as it was at a height: the state columns and the state tree read through history, everything else as it is now. Readable from the lowest height history still keeps up to the tip; reading below that gives today's values, so callers check the height first, as Chain::state_at does.",
    category = "Storage",
    see_also = "history_key, kept_in_history",
)]
pub struct Historic {
    db: Arc<dyn Committed>,
    height: u64,
}

impl Historic {
    #[document(parent = "Historic", summary = "The database at a height.", params = { db = "The database, or a snapshot of it.", height = "The height whose state to read." })]
    pub fn new(db: Arc<dyn Committed>, height: u64) -> Historic {
        Historic { db, height }
    }
}

impl Read for Historic {
    fn read(&self, column: Column, key: &[u8]) -> Result<Option<Vec<u8>>, DbError> {
        if !kept_in_history(column) {
            return self.db.read(column, key);
        }

        // The first change after the height replaced the value it had then.
        let prefix = entry_prefix(column, key);
        let start = [&prefix[..], &(self.height + 1).to_be_bytes()].concat();
        let mut replaced = None;
        self.db.scan(Column::History, &prefix, &start, &mut |_, value| {
            replaced = Some(value.to_vec());
            false
        })?;

        match replaced {
            Some(bytes) => wincode::deserialize::<Option<Vec<u8>>>(&bytes).map_err(|err| DbError::Corrupt(format!("history: {err}"))),
            None => self.db.read(column, key),
        }
    }
}

impl Committed for Historic {
    /// Every key with the prefix that held a value at the height: those there now, and those any
    /// change since touched, each read as it was then.
    fn scan(&self, column: Column, prefix: &[u8], start: &[u8], visit: &mut dyn FnMut(&[u8], &[u8]) -> bool) -> Result<(), DbError> {
        if !kept_in_history(column) {
            return self.db.scan(column, prefix, start, visit);
        }

        let from = scan_from(prefix, start);
        let mut keys = BTreeSet::new();
        self.db.scan(column, prefix, from, &mut |key, _| {
            keys.insert(key.to_vec());
            true
        })?;
        self.db.scan(Column::History, &escaped(column, prefix), &escaped(column, from), &mut |entry, _| {
            if let Some((key, height)) = parse(entry)
                && height > self.height
                && key.as_slice() >= from
            {
                keys.insert(key);
            }

            true
        })?;

        for key in keys {
            if let Some(value) = self.read(column, &key)?
                && !visit(&key, &value)
            {
                break;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Database, MemoryDb, State};

    /// A change a test block makes: the column, the key, and the value, or None to delete.
    type Change<'a> = (Column, &'a [u8], Option<&'a [u8]>);

    /// Applies a block's changes at a height as the chain does: each value it replaced into
    /// history, then the changes.
    fn apply(db: &Arc<dyn Database>, height: u64, changes: &[Change<'_>]) {
        let state = State::new(db.clone());

        for (column, key, value) in changes {
            let before = db.read(*column, key).unwrap();
            state.put(Column::History, history_key(*column, key, height), replaced(&before));

            match value {
                Some(value) => state.put(*column, key.to_vec(), value.to_vec()),
                None => state.delete(*column, key.to_vec()),
            }
        }

        db.commit(state.take_changes()).unwrap();
    }

    #[test]
    fn keys_escape_so_they_sort_as_the_keys_do_and_read_back() {
        let keys: [&[u8]; 6] = [b"", b"\x00", b"\x00\x00", b"\x00\x01", b"a", b"a\x00b"];
        let encoded: Vec<Vec<u8>> = keys.iter().map(|key| history_key(Column::Storage, key, 7)).collect();
        let mut sorted = encoded.clone();
        sorted.sort();

        assert_eq!(sorted, encoded, "in the keys' own order");

        for (key, entry) in keys.iter().zip(&encoded) {
            assert_eq!(parse(entry), Some((key.to_vec(), 7)));
        }

        assert!(!history_key(Column::Storage, b"a\x00b", 1).starts_with(&entry_prefix(Column::Storage, b"a")), "no key's entries among another's");
        assert!(history_key(Column::Storage, b"a\x00b", 1).starts_with(&escaped(Column::Storage, b"a")), "but every longer key's under a scan of its prefix");
    }

    #[test]
    fn state_reads_and_scans_as_it_was_at_each_height() {
        let db: Arc<dyn Database> = Arc::new(MemoryDb::default());
        // Before history began: a, b.
        apply(&db, 0, &[(Column::Storage, b"a", Some(b"a0")), (Column::Storage, b"b", Some(b"b0"))]);
        // 5 changes a and adds c; 8 deletes b; 9 changes c.
        apply(&db, 5, &[(Column::Storage, b"a", Some(b"a5")), (Column::Storage, b"c", Some(b"c5"))]);
        apply(&db, 8, &[(Column::Storage, b"b", None)]);
        apply(&db, 9, &[(Column::Storage, b"c", Some(b"c9")), (Column::Accounts, b"a", Some(b"other column"))]);
        let at = |height: u64| Historic::new(db.clone(), height);
        let read = |height: u64, key: &[u8]| at(height).read(Column::Storage, key).unwrap();
        let scan = |height: u64, start: &[u8]| {
            let mut seen = Vec::new();
            at(height)
                .scan(Column::Storage, b"", start, &mut |key, value| {
                    seen.push(format!("{}={}", String::from_utf8_lossy(key), String::from_utf8_lossy(value)));
                    true
                })
                .unwrap();
            seen
        };

        assert_eq!((read(4, b"a"), read(5, b"a"), read(20, b"a")), (Some(b"a0".to_vec()), Some(b"a5".to_vec()), Some(b"a5".to_vec())));
        assert_eq!((read(7, b"b"), read(8, b"b")), (Some(b"b0".to_vec()), None), "deleted at 8");
        assert_eq!((read(4, b"c"), read(6, b"c"), read(9, b"c")), (None, Some(b"c5".to_vec()), Some(b"c9".to_vec())), "added at 5");
        assert_eq!(scan(4, b""), ["a=a0", "b=b0"]);
        assert_eq!(scan(6, b""), ["a=a5", "b=b0", "c=c5"]);
        assert_eq!(scan(8, b"b"), ["c=c5"], "from a start, b gone");
        assert_eq!(scan(9, b""), ["a=a5", "c=c9"]);
        // What history keeps nothing of reads as it is now.
        assert_eq!(at(1).read(Column::Meta, b"x").unwrap(), None);
    }
}
