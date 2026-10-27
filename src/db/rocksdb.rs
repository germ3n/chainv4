use {
    super::{Changes, Column, Database, DbError, Read, scan_from},
    r#macro::document,
    rocksdb::{ColumnFamily, ColumnFamilyDescriptor, DB, Direction, IteratorMode, Options, WriteBatch},
    std::path::Path,
};

#[document(
    summary = "The RocksDB backend. Each Column is a column family, and a commit is one WriteBatch, synced to disk before it returns unless opened unsynced: a power cut loses no commit the node was told stood.",
    category = "Storage",
    see_also = "Database, MemoryDb",
)]
pub struct RocksDb {
    rocks: DB,
    /// Whether each commit waits for the disk.
    sync: bool,
}

impl RocksDb {
    #[document(
        parent = "RocksDb",
        summary = "Opens the database, creating it and its column families when missing.",
        params = { path = "The database directory." },
    )]
    pub fn open(path: impl AsRef<Path>) -> Result<RocksDb, DbError> {
        let mut options = Options::default();
        options.create_if_missing(true);
        options.create_missing_column_families(true);
        let columns = Column::ALL
            .iter()
            .map(|column| ColumnFamilyDescriptor::new(column.name(), Options::default()));
        let rocks = DB::open_cf_descriptors(&options, path, columns).map_err(backend)?;

        Ok(RocksDb { rocks, sync: true })
    }

    #[document(
        parent = "RocksDb",
        summary = "The same database, its commits no longer waiting for the disk: faster, but a power cut can lose the last few. What --no-fsync gives, for benchmarks and tests.",
    )]
    pub fn unsynced(mut self) -> RocksDb {
        self.sync = false;

        self
    }

    /// How a commit is written: synced, unless opened unsynced.
    fn options(&self) -> rocksdb::WriteOptions {
        let mut options = rocksdb::WriteOptions::default();
        options.set_sync(self.sync);

        options
    }

    #[document(
        parent = "RocksDb",
        summary = "Copies the database as it is now into a new directory, cheaply: RocksDB hard-links the files it can, so the copy takes room only as the two drift apart. The copy opens as a database of its own.",
        params = { path = "Where the copy goes: a directory that does not exist yet." },
    )]
    pub fn checkpoint(&self, path: impl AsRef<Path>) -> Result<(), DbError> {
        rocksdb::checkpoint::Checkpoint::new(&self.rocks)
            .and_then(|checkpoint| checkpoint.create_checkpoint(path))
            .map_err(backend)
    }

    fn column(&self, column: Column) -> &ColumnFamily {
        self.rocks
            .cf_handle(column.name())
            .expect("every column family is created on open")
    }
}

fn backend(err: rocksdb::Error) -> DbError {
    DbError::Backend(err.into_string())
}

impl Read for RocksDb {
    fn read(&self, column: Column, key: &[u8]) -> Result<Option<Vec<u8>>, DbError> {
        self.rocks.get_cf(self.column(column), key).map_err(backend)
    }
}

impl RocksDb {
    /// Adds a set of changes to a batch.
    fn add(&self, batch: &mut WriteBatch, changes: Changes) {
        for ((column, key), value) in changes {
            match value {
                Some(value) => batch.put_cf(self.column(column), key, value),
                None => batch.delete_cf(self.column(column), key),
            }
        }
    }
}

impl Database for RocksDb {
    fn commit(&self, changes: Changes) -> Result<(), DbError> {
        let mut batch = WriteBatch::default();
        self.add(&mut batch, changes);

        self.rocks.write_opt(batch, &self.options()).map_err(backend)
    }

    fn clear(&self, column: Column, changes: Changes) -> Result<(), DbError> {
        let handle = self.column(column);
        let first = self.rocks.iterator_cf(handle, IteratorMode::Start).next().transpose().map_err(backend)?;
        let last = self.rocks.iterator_cf(handle, IteratorMode::End).next().transpose().map_err(backend)?;
        let mut batch = WriteBatch::default();

        if let (Some((first, _)), Some((last, _))) = (first, last) {
            // A range delete leaves its end out, so the last key goes on its own.
            batch.delete_range_cf(handle, &first, &last);
            batch.delete_cf(handle, &last);
        }

        self.add(&mut batch, changes);

        self.rocks.write_opt(batch, &self.options()).map_err(backend)
    }

    fn scan(&self, column: Column, prefix: &[u8], start: &[u8], visit: &mut dyn FnMut(&[u8], &[u8]) -> bool) -> Result<(), DbError> {
        let mode = IteratorMode::From(scan_from(prefix, start), Direction::Forward);

        for record in self.rocks.iterator_cf(self.column(column), mode) {
            let (key, value) = record.map_err(backend)?;

            if !key.starts_with(prefix) || !visit(&key, &value) {
                break;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::State;
    use crate::db::tests::TempDir;
    use std::sync::Arc;

    #[test]
    fn committed_changes_survive_reopening() {
        let dir = TempDir::new("reopen");
        let db = Arc::new(RocksDb::open(&dir.0).unwrap());
        let state = State::new(db.clone());
        state.put(Column::Accounts, b"a".to_vec(), b"1".to_vec());
        db.commit(state.take_changes()).unwrap();
        drop(state);
        drop(db);
        let db = RocksDb::open(&dir.0).unwrap();

        assert_eq!(db.read(Column::Accounts, b"a").unwrap(), Some(b"1".to_vec()));
        assert_eq!(db.read(Column::Storage, b"a").unwrap(), None);
    }

    #[test]
    fn a_checkpoint_is_a_database_of_its_own() {
        let dir = TempDir::new("checkpoint");
        let db = RocksDb::open(dir.0.join("db")).unwrap();
        db.commit([((Column::Accounts, b"a".to_vec()), Some(b"1".to_vec()))].into()).unwrap();
        db.checkpoint(dir.0.join("copy")).unwrap();
        db.commit([((Column::Accounts, b"a".to_vec()), Some(b"2".to_vec()))].into()).unwrap();
        let copy = RocksDb::open(dir.0.join("copy")).unwrap();

        assert_eq!(copy.read(Column::Accounts, b"a").unwrap(), Some(b"1".to_vec()), "as it was when copied");
        assert!(db.checkpoint(dir.0.join("copy")).is_err(), "never over another");
    }
}
