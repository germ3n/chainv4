//! An example migration, written as the next format's would be: not in MIGRATIONS, since format 9
//! needs none, but tested as a real one is, on a real chain's database.
//!
//! The format 10 it imagines keeps each block's height in its work record, beside the total work, so
//! fork choice could weigh two tips without loading their blocks. Every work record changes shape;
//! nothing else does.

use {
    super::{At, Chunk, Migration},
    crate::crypto::hashable::Hashable,
    crate::db::{Column, DbError},
    crate::structs::block::SealedHeader,
    crate::structs::u256::{U256Le, u256},
    wincode::{SchemaRead, SchemaWrite, config::DefaultConfig},
};

/// A work record as format 9 keeps it: the total work of the chain up to its block. A migration
/// keeps its own copy of each shape it reads or writes, so it still reads format 9 once the chain's
/// own WorkRecord has moved on to format 10's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
struct WorkV9 {
    #[wincode(with = "U256Le")]
    total: u256,
}

impl Hashable for WorkV9 {}

/// A work record as format 10 keeps it: the same, and its block's height.
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
struct WorkV10 {
    #[wincode(with = "U256Le")]
    total: u256,
    height: u64,
}

impl Hashable for WorkV10 {}

/// Records as a scan finds them: each key and its value.
type Records = Vec<(Vec<u8>, Vec<u8>)>;

/// Format 9 to 10: work records keep their block's height.
pub struct WorkHeights;

impl Migration for WorkHeights {
    fn from(&self) -> u32 {
        9
    }

    fn describe(&self) -> &'static str {
        "work records keep their block's height"
    }

    /// Gives each work record in the chunk its block's height, from the block's stored header,
    /// which Headers keeps under the same key: the block's id.
    fn up(&self, at: &At) -> Result<Chunk, DbError> {
        let records = next_records(at)?;
        let mut chunk = Chunk::default();

        for (key, value) in &records {
            let old: WorkV9 = decode(value)?;
            let header: SealedHeader = match at.db.read(Column::Headers, key)? {
                Some(bytes) => decode(&bytes)?,
                None => return Err(DbError::Corrupt("a work record whose block has no header".to_string())),
            };
            let new = WorkV10 {
                total: old.total,
                height: header.header.height,
            };
            chunk.changes.insert((Column::Work, key.clone()), Some(new.hashable_bytes_with_tag()));
        }

        chunk.next = after(&records, at.limit);

        Ok(chunk)
    }

    /// Takes the heights back out: format 9 has nowhere to keep them, and reads them from the
    /// blocks when it needs them.
    fn down(&self, at: &At) -> Result<Chunk, DbError> {
        let records = next_records(at)?;
        let mut chunk = Chunk::default();

        for (key, value) in &records {
            let new: WorkV10 = decode(value)?;
            let old = WorkV9 { total: new.total };
            chunk.changes.insert((Column::Work, key.clone()), Some(old.hashable_bytes_with_tag()));
        }

        chunk.next = after(&records, at.limit);

        Ok(chunk)
    }
}

/// The work records this chunk changes: up to the limit of them, from where the chunk before
/// stopped. They keep their keys, so a scan from the cursor finds the ones still to do.
fn next_records(at: &At) -> Result<Records, DbError> {
    let mut records = Vec::new();
    at.db.scan(Column::Work, &[], at.cursor.unwrap_or(&[]), &mut |key, value| {
        records.push((key.to_vec(), value.to_vec()));

        records.len() < at.limit
    })?;

    Ok(records)
}

/// Where the next chunk starts: just past the last record of a full chunk. One that came up short
/// was the last.
fn after(records: &Records, limit: usize) -> Option<Vec<u8>> {
    match records.len() == limit {
        true => records.last().map(|(key, _)| [key.as_slice(), &[0]].concat()),
        false => None,
    }
}

fn decode<T: for<'de> SchemaRead<'de, DefaultConfig, Dst = T>>(bytes: &[u8]) -> Result<T, DbError> {
    wincode::deserialize(bytes).map_err(|err| DbError::Corrupt(err.to_string()))
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::chain::migrate::{format_of, migrate, plan},
        crate::chain::{Chain, Genesis},
        crate::consensus::{AuthorityProducer, DevAuthority, Producer},
        crate::db::{Committed, MemoryDb, Read},
        std::sync::Arc,
    };

    fn work_records(db: &MemoryDb) -> Records {
        let mut records = Vec::new();
        db.scan(Column::Work, &[], &[], &mut |key, value| {
            records.push((key.to_vec(), value.to_vec()));

            true
        })
        .unwrap();

        records
    }

    #[test]
    fn work_records_gain_their_heights_and_give_them_back() {
        // A real chain's database: genesis and 25 blocks, so 26 work records, three chunks of ten.
        let db = Arc::new(MemoryDb::default());
        let genesis = Genesis {
            fees: None,
            ..Genesis::dev_authority(DevAuthority::dev_keypair().public())
        };
        let mut chain = Chain::open(db.clone(), &genesis).unwrap();
        let mut producer = AuthorityProducer::new(DevAuthority::dev_keypair(), 2_000);

        for _ in 0..25 {
            let now = chain.tip().timestamp + 2_000;
            let mut block = chain.build(Vec::new(), Vec::new(), producer.address(&[]), now).unwrap();
            block.seal = producer.seal(&block.header).unwrap();
            chain.import(&block, now).unwrap();
        }

        drop(chain);
        let before = work_records(&db);
        let migrations: &[&dyn Migration] = &[&WorkHeights];

        assert_eq!(before.len(), 26);

        migrate(&*db, migrations, &plan(&*db, migrations, 10).unwrap(), 10).unwrap();

        assert_eq!(format_of(&*db).unwrap(), Some(10));

        let mut heights = Vec::new();

        for ((key, value), (_, old)) in work_records(&db).iter().zip(&before) {
            let record: WorkV10 = decode(value).unwrap();
            let header: SealedHeader = decode(&db.read(Column::Headers, key).unwrap().unwrap()).unwrap();

            assert_eq!(record.total, decode::<WorkV9>(old).unwrap().total, "the total work kept");
            assert_eq!(record.height, header.header.height);
            heights.push(record.height);
        }

        heights.sort_unstable();

        assert_eq!(heights, (0..=25).collect::<Vec<u64>>(), "every block's, once");

        // And back: byte for byte what format 9 kept, which this build's chain opens again.
        let down = plan(&*db, migrations, 9).unwrap();

        assert!(down.goes_down());

        migrate(&*db, migrations, &down, 10).unwrap();

        assert_eq!(work_records(&db), before);
        assert_eq!(Chain::open(db.clone(), &genesis).unwrap().tip().height, 25);
    }
}
