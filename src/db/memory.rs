use {
    super::{Changes, Column, Database, DbError, Read},
    r#macro::document,
    std::collections::BTreeMap,
    std::sync::Mutex,
};

#[document(
    summary = "A database kept in memory and lost when dropped. For tests, tools, and dry runs that must not touch disk.",
    category = "Storage",
    see_also = "Database, RocksDb",
)]
#[derive(Default)]
pub struct MemoryDb {
    records: Mutex<Records>,
}

type Records = BTreeMap<(Column, Vec<u8>), Vec<u8>>;

impl Read for MemoryDb {
    fn read(&self, column: Column, key: &[u8]) -> Result<Option<Vec<u8>>, DbError> {
        let records = self.records.lock().expect("the memory database is never poisoned");

        Ok(records.get(&(column, key.to_vec())).cloned())
    }
}

impl Database for MemoryDb {
    fn commit(&self, changes: Changes) -> Result<(), DbError> {
        // One lock for the whole set, so a reader never sees half of it.
        let mut records = self.records.lock().expect("the memory database is never poisoned");

        for (key, value) in changes {
            match value {
                Some(value) => records.insert(key, value),
                None => records.remove(&key),
            };
        }

        Ok(())
    }
}
