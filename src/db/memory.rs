use {
    super::{ByColumn, Changes, Column, Database, DbError, Read, Committed, scan_from},
    r#macro::document,
    std::sync::{Arc, Mutex},
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

type Records = ByColumn<Vec<u8>>;

/// Applies a set of changes to the records.
fn apply(records: &mut Records, changes: Changes) {
    for ((column, key), value) in changes {
        match value {
            Some(value) => {
                records.entry(column).or_default().insert(key, value);
            }
            None => {
                if let Some(keys) = records.get_mut(&column) {
                    keys.remove(&key);
                }
            }
        }
    }
}

impl Read for MemoryDb {
    fn read(&self, column: Column, key: &[u8]) -> Result<Option<Vec<u8>>, DbError> {
        let records = self.records.lock().expect("the memory database is never poisoned");

        Ok(records.get(&column).and_then(|keys| keys.get(key)).cloned())
    }
}

impl Database for MemoryDb {
    fn commit(&self, changes: Changes) -> Result<(), DbError> {
        // One lock for the whole set, so a reader never sees half of it.
        apply(&mut self.records.lock().expect("the memory database is never poisoned"), changes);

        Ok(())
    }

    fn clear(&self, column: Column, changes: Changes) -> Result<(), DbError> {
        let mut records = self.records.lock().expect("the memory database is never poisoned");
        records.remove(&column);
        apply(&mut records, changes);

        Ok(())
    }

    fn snapshot(&self) -> Arc<dyn Committed> {
        let records = self.records.lock().expect("the memory database is never poisoned").clone();

        Arc::new(MemoryDb { records: Mutex::new(records) })
    }
}

impl Committed for MemoryDb {
    fn scan(&self, column: Column, prefix: &[u8], start: &[u8], visit: &mut dyn FnMut(&[u8], &[u8]) -> bool) -> Result<(), DbError> {
        // Copied out first, so visit can read the database without waiting on the lock.
        let found: Vec<(Vec<u8>, Vec<u8>)> = {
            let records = self.records.lock().expect("the memory database is never poisoned");

            records
                .get(&column)
                .into_iter()
                .flat_map(|keys| keys.range::<[u8], _>((std::ops::Bound::Included(scan_from(prefix, start)), std::ops::Bound::Unbounded)))
                .take_while(|(key, _)| key.starts_with(prefix))
                .map(|(key, value)| (key.clone(), value.clone()))
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
