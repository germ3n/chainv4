//! Database migrations. A database records the STORAGE_FORMAT it is kept in, and every format
//! from 9 on comes with a Migration that moves a database to it from the one before, and back.
//! A node migrates an older database up to its own format as it starts. Going back is
//! `chainv4 db migrate --to N`, run by the newer chainv4, which alone knows how to undo its
//! formats; an older one would not know what it was reading.
//!
//! A migration works through the data a chunk at a time. Each chunk's changes are committed with
//! where it got to, in one atomic commit, and the format moves only with the last chunk. So a crash
//! or a stop leaves a database that is wholly in one format with a step under way, which the next
//! run finishes from where it stopped: never half of one format and half of the next.

use {
    super::{FORMAT_KEY, FormatRecord, STORAGE_FORMAT, read_value},
    crate::crypto::hashable::Hashable,
    crate::db::{Changes, Column, Database, DbError},
    r#macro::document,
    std::fmt,
    wincode::{SchemaRead, SchemaWrite},
};

#[cfg(test)]
mod example;

/// Where a step under way records how far it got.
pub(super) const MIGRATING_KEY: &[u8] = b"migrating";

/// About how many records one chunk changes: what bounds the memory a migration takes.
pub const CHUNK: usize = 10_000;

#[document(
    summary = "Every migration this build knows, oldest first: each moves a database from its format to the next. Formats before the first one's cannot be migrated.",
    category = "Chain",
    see_also = "Migration, STORAGE_FORMAT",
)]
pub const MIGRATIONS: &[&dyn Migration] = &[];

#[document(
    summary = "Moves a database from one STORAGE_FORMAT to the next, and back. It works a chunk at a time: up and down are called again and again, each time with where the chunk before stopped, until a chunk says it is the last. Each chunk's changes are committed atomically with that place, so a chunk must leave the database whole if it is the last thing that runs.",
    category = "Chain",
    see_also = "MIGRATIONS, migrate, At, Chunk",
    example = "struct SplitHeaders;\n\nimpl Migration for SplitHeaders {\n    fn from(&self) -> u32 { 8 }\n    fn describe(&self) -> &'static str { \"keeps headers apart from blocks\" }\n    fn up(&self, at: &At) -> Result<Chunk, DbError> { /* scan from at.cursor, at most at.limit records */ }\n    fn down(&self, at: &At) -> Result<Chunk, DbError> { /* the same, the other way */ }\n}",
)]
pub trait Migration: Send + Sync {
    #[document(parent = "Migration", summary = "The format it moves a database from. It moves it to the next one, and down moves it back.")]
    fn from(&self) -> u32;

    #[document(parent = "Migration", summary = "What it changes, in a few words: the log says it, and a downgrade asks to confirm undoing it.")]
    fn describe(&self) -> &'static str;

    #[document(
        parent = "Migration",
        summary = "One chunk of moving a database up: what to change, and where the next chunk starts, if there is one.",
        params = { at = "The database, as the chunks before left it, and where they stopped." },
        returns = "The chunk, or a database error, which stops the migration where the last committed chunk left it.",
    )]
    fn up(&self, at: &At) -> Result<Chunk, DbError>;

    #[document(
        parent = "Migration",
        summary = "One chunk of moving a database back down, from the next format to this one's. Never called when irreversible says why it cannot be.",
        params = { at = "The database, and where the chunks before stopped." },
    )]
    fn down(&self, at: &At) -> Result<Chunk, DbError>;

    #[document(
        parent = "Migration",
        summary = "Why it cannot be undone, when it cannot: then a downgrade past it is refused before it changes anything. None, the default, for one that can.",
    )]
    fn irreversible(&self) -> Option<&'static str> {
        None
    }
}

#[document(summary = "What a migration's chunk works on.", category = "Chain", see_also = "Migration")]
pub struct At<'a> {
    #[document(summary = "The database, with every chunk before committed.")]
    pub db: &'a dyn Database,
    #[document(summary = "Where the chunk before said the next one starts; None for the first chunk.")]
    pub cursor: Option<&'a [u8]>,
    #[document(summary = "About how many records a chunk should change at most.")]
    pub limit: usize,
}

#[document(summary = "One chunk of a migration's work.", category = "Chain", see_also = "Migration")]
#[derive(Debug, Default)]
pub struct Chunk {
    #[document(summary = "What to change. Never the format, nor where a migration got to: the migration's run writes those.")]
    pub changes: Changes,
    #[document(summary = "Where the next chunk starts, opaque to all but the migration; None when this chunk is the last.")]
    pub next: Option<Vec<u8>>,
}

#[document(summary = "One step of a plan: from a format to the one above it, or the one below.", category = "Chain", see_also = "Plan")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step {
    pub from: u32,
    pub to: u32,
}

impl Step {
    #[document(parent = "Step", summary = "Whether it goes back down.")]
    pub fn is_down(self) -> bool {
        self.to < self.from
    }

    /// The format of the migration that makes it: the lower of the two.
    fn migration(self) -> u32 {
        self.from.min(self.to)
    }
}

#[document(
    summary = "The steps that take a database from its format to another: first any step a run before left under way, then one migration at a time.",
    category = "Chain",
    see_also = "plan, migrate",
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    #[document(summary = "The format the database is in.")]
    pub from: u32,
    #[document(summary = "The format it is to be in.")]
    pub to: u32,
    #[document(summary = "Each step, in order. Empty when it is in that format already.")]
    pub steps: Vec<Step>,
    #[document(summary = "Whether the first step is one a run before left under way, which goes on from where it stopped.")]
    pub resumes: bool,
}

impl Plan {
    #[document(parent = "Plan", summary = "Whether any step goes back down, which asks to be confirmed.")]
    pub fn goes_down(&self) -> bool {
        self.steps.iter().any(|step| step.is_down())
    }
}

#[document(summary = "Why a database could not be migrated.", category = "Chain")]
#[derive(Debug, PartialEq, Eq)]
pub enum MigrationError {
    #[document(summary = "The database failed, or a migration found a record it could not read: during this step, which stays under way to be resumed.")]
    Db { step: Option<Step>, error: DbError },
    #[document(summary = "The database records no format: it is from before formats were recorded, and nothing migrates it.")]
    Unknown,
    #[document(summary = "No migration this build knows goes from this format toward the one asked for.")]
    NoPath { from: u32, to: u32 },
    #[document(summary = "A downgrade would pass a migration that cannot be undone, and why not. Nothing was changed.")]
    Irreversible { step: Step, why: &'static str },
    #[document(summary = "A chunk said the next one starts where it started, and changed nothing: the migration would never end.")]
    Stuck(Step),
}

impl fmt::Display for MigrationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MigrationError::Db { step: Some(step), error } => write!(f, "migrating from format {} to {}: {error}; run it again to go on from there", step.from, step.to),
            MigrationError::Db { step: None, error } => write!(f, "{error}"),
            MigrationError::Unknown => write!(f, "the database records no format: it is from before formats were recorded, and nothing migrates it"),
            MigrationError::NoPath { from, to } => write!(f, "no migration goes from format {from} toward {to}"),
            MigrationError::Irreversible { step, why } => write!(f, "format {} cannot go back to {}: {why}", step.from, step.to),
            MigrationError::Stuck(step) => write!(f, "migrating from format {} to {} made no progress", step.from, step.to),
        }
    }
}

impl std::error::Error for MigrationError {}

impl From<DbError> for MigrationError {
    fn from(error: DbError) -> MigrationError {
        MigrationError::Db { step: None, error }
    }
}

/// A step under way: which, and where its next chunk starts.
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub(super) struct MigratingRecord {
    pub(super) from: u32,
    pub(super) to: u32,
    pub(super) cursor: Option<Vec<u8>>,
}

impl Hashable for MigratingRecord {}

#[document(
    summary = "The format a database is kept in, as it records it.",
    category = "Chain",
    returns = "The format, or None for a database that records none: a new one, or one from before formats were recorded.",
)]
pub fn format_of(db: &dyn Database) -> Result<Option<u32>, DbError> {
    Ok(read_value::<FormatRecord>(db, Column::Meta, FORMAT_KEY)?.map(|record| record.format))
}

#[document(
    summary = "The step a run before left under way, if one did.",
    category = "Chain",
    returns = "The step, which the next migration of this database finishes first.",
)]
pub fn under_way(db: &dyn Database) -> Result<Option<Step>, DbError> {
    Ok(read_value::<MigratingRecord>(db, Column::Meta, MIGRATING_KEY)?.map(|record| Step { from: record.from, to: record.to }))
}

#[document(
    summary = "Works out the steps from a database's format to another: first finishing any step a run before left under way, then one migration at a time. Nothing is changed.",
    category = "Chain",
    params = {
        db = "The database.",
        migrations = "The migrations to use, oldest first: MIGRATIONS.",
        to = "The format it is to be in.",
    },
    returns = "The plan, or why there is none: no format recorded, a format no migration starts from, or a downgrade past one that cannot be undone.",
)]
pub fn plan(db: &dyn Database, migrations: &[&dyn Migration], to: u32) -> Result<Plan, MigrationError> {
    let from = format_of(db)?.ok_or(MigrationError::Unknown)?;
    let under_way = under_way(db)?;
    let mut steps: Vec<Step> = under_way.into_iter().collect();
    let mut at = under_way.map_or(from, |step| step.to);

    while at != to {
        let step = match at < to {
            true => Step { from: at, to: at + 1 },
            false => Step { from: at, to: at - 1 },
        };
        let migration = migration(migrations, step).ok_or(MigrationError::NoPath { from: at, to })?;

        if step.is_down()
            && let Some(why) = migration.irreversible()
        {
            return Err(MigrationError::Irreversible { step, why });
        }

        steps.push(step);
        at = step.to;
    }

    Ok(Plan {
        from,
        to,
        steps,
        resumes: under_way.is_some(),
    })
}

#[document(
    summary = "Carries out a plan, a chunk at a time: each chunk is committed with where it got to, and each step's last chunk with the new format. Stopped anywhere, the database is left in one format, with any step under way for the next run to finish.",
    category = "Chain",
    params = {
        db = "The database, open to nothing else.",
        migrations = "The migrations the plan was made with.",
        plan = "What plan worked out.",
        limit = "About how many records a chunk changes: CHUNK.",
    },
    returns = "Nothing once the database is in the plan's format; otherwise why it stopped, the database in a format with its step under way.",
)]
pub fn migrate(db: &dyn Database, migrations: &[&dyn Migration], plan: &Plan, limit: usize) -> Result<(), MigrationError> {
    for (index, step) in plan.steps.iter().copied().enumerate() {
        let migration = migration(migrations, step).ok_or(MigrationError::NoPath { from: step.from, to: plan.to })?;
        let mut cursor = match plan.resumes && index == 0 {
            true => read_value::<MigratingRecord>(db, Column::Meta, MIGRATING_KEY)?.and_then(|record| record.cursor),
            false => None,
        };
        let mut chunks = 0u64;
        log::info!(
            "[db] {} from format {} to {}: {}{}",
            if step.is_down() { "migrating back" } else { "migrating" },
            step.from,
            step.to,
            migration.describe(),
            if cursor.is_some() { ", from where it stopped" } else { "" }
        );

        loop {
            let at = At {
                db,
                cursor: cursor.as_deref(),
                limit,
            };
            let failed = |error| MigrationError::Db { step: Some(step), error };
            let chunk = match step.is_down() {
                true => migration.down(&at),
                false => migration.up(&at),
            }
            .map_err(failed)?;
            let Chunk { mut changes, next } = chunk;

            if next.is_some() && next == cursor && changes.is_empty() {
                return Err(MigrationError::Stuck(step));
            }

            let last = next.is_none();

            match &next {
                Some(_) => {
                    let record = MigratingRecord {
                        from: step.from,
                        to: step.to,
                        cursor: next.clone(),
                    };
                    changes.insert((Column::Meta, MIGRATING_KEY.to_vec()), Some(record.hashable_bytes_with_tag()));
                }
                None => {
                    changes.insert((Column::Meta, MIGRATING_KEY.to_vec()), None);
                    changes.insert((Column::Meta, FORMAT_KEY.to_vec()), Some(FormatRecord { format: step.to }.hashable_bytes_with_tag()));
                }
            }

            db.commit(changes).map_err(failed)?;
            chunks += 1;

            if chunks.is_multiple_of(100) {
                log::info!("[db] format {} to {}: {chunks} chunks done", step.from, step.to);
            }

            if last {
                break;
            }

            cursor = next;
        }

        log::info!("[db] the database is in format {}", step.to);
    }

    Ok(())
}

#[document(
    summary = "What a node does with its database as it starts: migrates one in an older format up to STORAGE_FORMAT, after taking a backup, and finishes any step a run before left under way. A new database, one already in its format, and one in a newer format are left as they are, for Chain::open to judge.",
    category = "Chain",
    params = {
        db = "The database, open to nothing else.",
        migrations = "MIGRATIONS.",
        backup = "Takes a backup before a migration starts afresh, and says where it went. Not called when a step under way is resumed: the database is half migrated by then, and the backup taken when it started is the one to keep.",
    },
    returns = "Nothing once the database is ready to open; otherwise why not.",
)]
pub fn on_start(db: &dyn Database, migrations: &[&dyn Migration], backup: impl FnOnce(u32) -> Result<String, String>) -> Result<(), MigrationError> {
    let Some(format) = format_of(db)? else {
        return Ok(());
    };

    if format > STORAGE_FORMAT && under_way(db)?.is_none() {
        return Ok(());
    }

    let plan = plan(db, migrations, STORAGE_FORMAT)?;

    if plan.steps.is_empty() {
        return Ok(());
    }

    if !plan.resumes {
        match backup(format) {
            Ok(path) => log::info!("[db] backed up format {format} to {path} before migrating"),
            Err(why) => return Err(MigrationError::Db { step: None, error: DbError::Backend(format!("could not back up the database: {why}")) }),
        }
    }

    migrate(db, migrations, &plan, CHUNK)
}

#[document(
    summary = "The migration that makes a step, up or down: the one from the lower of its two formats.",
    category = "Chain",
)]
pub fn migration<'a>(migrations: &[&'a dyn Migration], step: Step) -> Option<&'a dyn Migration> {
    migrations.iter().copied().find(|migration| migration.from() == step.migration())
}

/// Records a format, as a database kept in it would, for tests elsewhere.
#[cfg(test)]
pub(crate) fn set_format(db: &dyn Database, format: u32) {
    db.commit([((Column::Meta, FORMAT_KEY.to_vec()), Some(FormatRecord { format }.hashable_bytes_with_tag()))].into()).unwrap();
}

#[cfg(test)]
mod tests {
    use {super::*, crate::db::{Committed, MemoryDb}, std::sync::atomic::{AtomicUsize, Ordering}};

    /// Moves every Storage record whose key starts with `old` to the same key starting with `new`,
    /// at most limit at a time: its format keeps them under `a`, the next one under `b`.
    struct Rename(u32);

    impl Rename {
        fn shift(at: &At, old: u8, new: u8) -> Result<Chunk, DbError> {
            let mut records = Vec::new();
            at.db.scan(Column::Storage, &[old], at.cursor.unwrap_or(&[old]), &mut |key, value| {
                records.push((key.to_vec(), value.to_vec()));

                records.len() < at.limit
            })?;
            let mut chunk = Chunk::default();

            for (key, value) in &records {
                let mut moved = key.clone();
                moved[0] = new;
                chunk.changes.insert((Column::Storage, key.clone()), None);
                chunk.changes.insert((Column::Storage, moved), Some(value.clone()));
            }

            // Moved records leave the prefix scanned, so the next chunk starts at its beginning again;
            // the cursor only shows it got somewhere.
            chunk.next = (records.len() == at.limit).then(|| records.last().map(|(key, _)| key.clone())).flatten();

            Ok(chunk)
        }
    }

    impl Migration for Rename {
        fn from(&self) -> u32 {
            self.0
        }

        fn describe(&self) -> &'static str {
            "keeps the test's records under b"
        }

        fn up(&self, at: &At) -> Result<Chunk, DbError> {
            Rename::shift(at, b'a', b'b')
        }

        fn down(&self, at: &At) -> Result<Chunk, DbError> {
            Rename::shift(at, b'b', b'a')
        }
    }

    /// From 101 to 102, adds a record; it cannot be undone.
    struct OneWay;

    impl Migration for OneWay {
        fn from(&self) -> u32 {
            101
        }

        fn describe(&self) -> &'static str {
            "adds a record"
        }

        fn up(&self, _: &At) -> Result<Chunk, DbError> {
            let mut chunk = Chunk::default();
            chunk.changes.insert((Column::Storage, b"c".to_vec()), Some(b"added".to_vec()));

            Ok(chunk)
        }

        fn down(&self, _: &At) -> Result<Chunk, DbError> {
            unreachable!("irreversible")
        }

        fn irreversible(&self) -> Option<&'static str> {
            Some("what it adds was never kept before")
        }
    }

    /// Rename, failing its second chunk the first time it gets there, as a crash would stop it.
    struct Crashing(AtomicUsize);

    impl Migration for Crashing {
        fn from(&self) -> u32 {
            100
        }

        fn describe(&self) -> &'static str {
            Rename(100).describe()
        }

        fn up(&self, at: &At) -> Result<Chunk, DbError> {
            match self.0.fetch_add(1, Ordering::SeqCst) {
                1 => Err(DbError::Backend("the power went".to_string())),
                _ => Rename(100).up(at),
            }
        }

        fn down(&self, at: &At) -> Result<Chunk, DbError> {
            Rename(100).down(at)
        }
    }

    /// A database in this format, with five records under `a`.
    fn database(format: u32) -> MemoryDb {
        let db = MemoryDb::default();
        set_format(&db, format);
        let mut changes = Changes::new();

        for index in 0..5u8 {
            changes.insert((Column::Storage, vec![b'a', index]), Some(vec![index]));
        }

        db.commit(changes).unwrap();

        db
    }

    fn records(db: &MemoryDb) -> Vec<(Vec<u8>, Vec<u8>)> {
        let mut found = Vec::new();
        db.scan(Column::Storage, &[], &[], &mut |key, value| {
            found.push((key.to_vec(), value.to_vec()));

            true
        })
        .unwrap();

        found
    }

    fn under(prefix: u8) -> Vec<(Vec<u8>, Vec<u8>)> {
        (0..5u8).map(|index| (vec![prefix, index], vec![index])).collect()
    }

    #[test]
    fn a_database_goes_up_and_back_a_chunk_at_a_time() {
        let db = database(100);
        let migrations: &[&dyn Migration] = &[&Rename(100)];
        let up = plan(&db, migrations, 101).unwrap();

        assert_eq!(up.steps, [Step { from: 100, to: 101 }]);
        assert!(!up.goes_down() && !up.resumes);

        migrate(&db, migrations, &up, 2).unwrap();

        assert_eq!((format_of(&db).unwrap(), under_way(&db).unwrap()), (Some(101), None));
        assert_eq!(records(&db), under(b'b'));

        let down = plan(&db, migrations, 100).unwrap();

        assert!(down.goes_down());

        migrate(&db, migrations, &down, 2).unwrap();

        assert_eq!(format_of(&db).unwrap(), Some(100));
        assert_eq!(records(&db), under(b'a'));
        assert!(plan(&db, migrations, 100).unwrap().steps.is_empty(), "nothing left to do");
    }

    #[test]
    fn a_stopped_migration_goes_on_from_where_it_stopped() {
        let db = database(100);
        let crashing = Crashing(AtomicUsize::new(0));
        let migrations: &[&dyn Migration] = &[&crashing];
        let first = migrate(&db, migrations, &plan(&db, migrations, 101).unwrap(), 2).unwrap_err();

        assert!(matches!(first, MigrationError::Db { step: Some(Step { from: 100, to: 101 }), .. }), "{first}");
        assert_eq!(format_of(&db).unwrap(), Some(100), "still the old format, with a step under way");
        assert_eq!(under_way(&db).unwrap(), Some(Step { from: 100, to: 101 }));
        assert_eq!(records(&db).iter().filter(|(key, _)| key[0] == b'b').count(), 2, "the first chunk was kept");

        let again = plan(&db, migrations, 101).unwrap();

        assert!(again.resumes);

        migrate(&db, migrations, &again, 2).unwrap();

        assert_eq!((format_of(&db).unwrap(), under_way(&db).unwrap()), (Some(101), None));
        assert_eq!(records(&db), under(b'b'), "every record once");
    }

    #[test]
    fn a_step_under_way_is_finished_before_turning_back() {
        let db = database(101);
        let migrations: &[&dyn Migration] = &[&Rename(100)];
        let mut changes = Changes::new();
        changes.insert((Column::Meta, MIGRATING_KEY.to_vec()), Some(MigratingRecord { from: 101, to: 100, cursor: None }.hashable_bytes_with_tag()));
        db.commit(changes).unwrap();

        // Interrupted going down; asked to be in 101 again, it goes down to 100 first, then up.
        let plan = plan(&db, migrations, 101).unwrap();

        assert_eq!(plan.steps, [Step { from: 101, to: 100 }, Step { from: 100, to: 101 }]);
        assert!(plan.resumes);
    }

    #[test]
    fn downgrades_past_a_one_way_migration_are_refused_before_anything_changes() {
        let db = database(100);
        let migrations: &[&dyn Migration] = &[&Rename(100), &OneWay];
        migrate(&db, migrations, &plan(&db, migrations, 102).unwrap(), CHUNK).unwrap();

        assert_eq!(format_of(&db).unwrap(), Some(102));

        let refused = plan(&db, migrations, 100).unwrap_err();

        assert_eq!(refused, MigrationError::Irreversible { step: Step { from: 102, to: 101 }, why: "what it adds was never kept before" });
        assert_eq!(refused.to_string(), "format 102 cannot go back to 101: what it adds was never kept before");
        assert_eq!(format_of(&db).unwrap(), Some(102));
    }

    #[test]
    fn formats_no_migration_reaches_are_refused() {
        let migrations: &[&dyn Migration] = &[&Rename(100)];

        assert_eq!(plan(&database(99), migrations, 101), Err(MigrationError::NoPath { from: 99, to: 101 }));
        assert_eq!(plan(&database(100), migrations, 102), Err(MigrationError::NoPath { from: 101, to: 102 }));
        assert_eq!(plan(&MemoryDb::default(), migrations, 101), Err(MigrationError::Unknown));
    }

    #[test]
    fn a_chunk_that_gets_nowhere_stops_the_migration() {
        struct Nowhere;

        impl Migration for Nowhere {
            fn from(&self) -> u32 {
                100
            }

            fn describe(&self) -> &'static str {
                "goes nowhere"
            }

            fn up(&self, at: &At) -> Result<Chunk, DbError> {
                Ok(Chunk {
                    changes: Changes::new(),
                    next: Some(at.cursor.unwrap_or(b"start").to_vec()),
                })
            }

            fn down(&self, at: &At) -> Result<Chunk, DbError> {
                self.up(at)
            }
        }

        let db = database(100);
        let migrations: &[&dyn Migration] = &[&Nowhere];

        assert_eq!(migrate(&db, migrations, &plan(&db, migrations, 101).unwrap(), 2), Err(MigrationError::Stuck(Step { from: 100, to: 101 })));
    }

    #[test]
    fn a_node_starting_migrates_up_after_a_backup_and_leaves_the_rest_alone() {
        // A migration up to this build's own format, as the newest of MIGRATIONS would be.
        let migrations: &[&dyn Migration] = &[&Rename(STORAGE_FORMAT - 1)];

        // Already in its format, new, or newer, for Chain::open to refuse: nothing to do.
        for db in [database(STORAGE_FORMAT), MemoryDb::default(), database(STORAGE_FORMAT + 1)] {
            on_start(&db, migrations, |_| panic!("nothing to back up")).unwrap();

            assert_eq!(records(&db).first().map(|(key, _)| key[0]), (format_of(&db).unwrap().is_some()).then_some(b'a'));
        }

        let old = database(STORAGE_FORMAT - 1);
        let mut backed_up = None;
        on_start(&old, migrations, |format| {
            backed_up = Some(format);

            Ok("chain.db.format-7".to_string())
        })
        .unwrap();

        assert_eq!(backed_up, Some(STORAGE_FORMAT - 1), "backed up before it started");
        assert_eq!((format_of(&old).unwrap(), under_way(&old).unwrap()), (Some(STORAGE_FORMAT), None));
        assert_eq!(records(&old), under(b'b'));

        // A step a run before left under way is finished, with no second backup of a half-migrated
        // database.
        let stopped = database(STORAGE_FORMAT - 1);
        let mut changes = Changes::new();
        let record = MigratingRecord {
            from: STORAGE_FORMAT - 1,
            to: STORAGE_FORMAT,
            cursor: None,
        };
        changes.insert((Column::Meta, MIGRATING_KEY.to_vec()), Some(record.hashable_bytes_with_tag()));
        stopped.commit(changes).unwrap();
        on_start(&stopped, migrations, |_| panic!("no backup when resuming")).unwrap();

        assert_eq!(format_of(&stopped).unwrap(), Some(STORAGE_FORMAT));

        // With no backup, nothing is migrated.
        let unsafe_to_start = database(STORAGE_FORMAT - 1);

        assert!(on_start(&unsafe_to_start, migrations, |_| Err("no room".to_string())).unwrap_err().to_string().contains("could not back up the database: no room"));
        assert_eq!(format_of(&unsafe_to_start).unwrap(), Some(STORAGE_FORMAT - 1));
    }
}
