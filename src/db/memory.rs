use {
    super::{Changes, Column, Database, DbError, Read, scan_from},
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

    fn scan(&self, column: Column, prefix: &[u8], start: &[u8], visit: &mut dyn FnMut(&[u8], &[u8]) -> bool) -> Result<(), DbError> {
        // Copied out first, so visit can read the database without waiting on the lock.
        let found: Vec<(Vec<u8>, Vec<u8>)> = {
            let records = self.records.lock().expect("the memory database is never poisoned");

            records
                .range((column, scan_from(prefix, start).to_vec())..)
                .take_while(|((at, key), _)| *at == column && key.starts_with(prefix))
                .map(|((_, key), value)| (key.clone(), value.clone()))
                .collect()
        };

        for (key, value) in found {
            if !visit(&key, &value) {
                break;
            }
        }

        Ok(())
    }
}
