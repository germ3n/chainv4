use {
    super::{Changes, Column, Database, DbError, Read, scan_from},
    r#macro::document,
    rocksdb::{ColumnFamily, ColumnFamilyDescriptor, DB, Direction, IteratorMode, Options, WriteBatch},
    std::path::Path,
};

#[document(
    summary = "The RocksDB backend. Each Column is a column family, and a commit is one WriteBatch.",
    category = "Storage",
    see_also = "Database, MemoryDb",
)]
pub struct RocksDb {
    rocks: DB,
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

        Ok(RocksDb { rocks })
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

        self.rocks.write(batch).map_err(backend)
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

        self.rocks.write(batch).map_err(backend)
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
}
