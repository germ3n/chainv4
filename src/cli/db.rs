//! chainv4 db: which format a database is kept in, and moving it to another. A node moves an older
//! database up to its own format by itself as it starts; going back down is only ever asked for
//! here, and asks to be confirmed.

use {
    super::CliResult,
    crate::chain::STORAGE_FORMAT,
    crate::chain::migrate::{self, CHUNK, MIGRATIONS, Migration},
    crate::db::RocksDb,
    std::io::{BufRead, Write},
    std::path::{Path, PathBuf},
};

#[derive(clap::Args)]
pub struct DbArgs {
    #[command(subcommand)]
    command: DbCommand,
}

#[derive(clap::Subcommand)]
enum DbCommand {
    /// Says which format a database is kept in, which this chainv4 reads, and which migrations it
    /// knows.
    Format {
        /// The database.
        #[arg(long, default_value = "chain.db")]
        db: PathBuf,
    },
    /// Moves a database to another format: up to this chainv4's own, or with --to back down to an
    /// older one, which asks first. It backs the database up before it starts.
    Migrate(MigrateArgs),
}

#[derive(clap::Args)]
pub(super) struct MigrateArgs {
    /// The database. No node may have it open.
    #[arg(long, default_value = "chain.db")]
    pub(super) db: PathBuf,
    /// The format to move it to; this chainv4's own unless given.
    #[arg(long)]
    pub(super) to: Option<u32>,
    /// Go back down without asking.
    #[arg(long)]
    pub(super) yes: bool,
    /// Do not copy the database aside before moving it.
    #[arg(long)]
    pub(super) no_backup: bool,
}

pub fn db(args: DbArgs) -> CliResult<()> {
    match args.command {
        DbCommand::Format { db } => format(&db, MIGRATIONS),
        DbCommand::Migrate(args) => migrate_reading(args, MIGRATIONS, &mut std::io::stdin().lock()),
    }
}

/// What a node does with its database before it opens the chain: moves an older one up to its own
/// format, after copying it aside, and finishes a migration a run before left part way.
pub fn on_start(db: &RocksDb, path: &Path) -> CliResult<()> {
    Ok(migrate::on_start(db, MIGRATIONS, |format| backup(db, path, format).map(|copy| copy.display().to_string()))?)
}

/// Says which format a database is in, and what this chainv4 can do with it.
pub(super) fn format(path: &Path, migrations: &[&dyn Migration]) -> CliResult<()> {
    let db = open(path)?;

    match migrate::format_of(&db)? {
        Some(format) => println!("{} is kept in format {format}", path.display()),
        None => println!("{} records no format: it is from before formats were recorded", path.display()),
    }

    println!("this chainv4 reads format {STORAGE_FORMAT}");

    if let Some(step) = migrate::under_way(&db)? {
        println!("a migration from format {} to {} stopped part way: `chainv4 db migrate` finishes it", step.from, step.to);
    }

    match migrations {
        [] => println!("it knows no migrations yet, so it reads format {STORAGE_FORMAT} alone"),
        _ => {
            println!("it migrates between formats:");

            for migration in migrations {
                let back = migration.irreversible().map_or_else(String::new, |why| format!(" (one way: {why})"));
                println!("  {} and {}: {}{back}", migration.from(), migration.from() + 1, migration.describe());
            }
        }
    }

    Ok(())
}

/// Moves a database to another format, asking on input before it goes back down.
pub(super) fn migrate_reading(args: MigrateArgs, migrations: &[&dyn Migration], input: &mut dyn BufRead) -> CliResult<()> {
    let db = open(&args.db)?;
    let to = args.to.unwrap_or(STORAGE_FORMAT);
    let plan = migrate::plan(&db, migrations, to)?;
    let shown = args.db.display();

    if plan.steps.is_empty() {
        println!("{shown} is in format {to} already");

        return Ok(());
    }

    println!("{shown} is in format {}; to reach {to}:", plan.from);

    for (index, step) in plan.steps.iter().enumerate() {
        let describe = migrate::migration(migrations, *step).map_or("", |migration| migration.describe());
        let undo = if step.is_down() { "undo: " } else { "" };
        let resumed = if plan.resumes && index == 0 { ", going on from where it stopped" } else { "" };
        println!("  {} to {}: {undo}{describe}{resumed}", step.from, step.to);
    }

    if plan.goes_down() && !args.yes {
        print!("This takes {shown} back to format {to}, which only a chainv4 that reads format {to} opens. Type yes to go on: ");
        std::io::stdout().flush()?;
        let mut answer = String::new();
        input.read_line(&mut answer)?;

        if answer.trim() != "yes" {
            return Err(format!("{shown} is left as it was").into());
        }
    }

    if !args.no_backup && !plan.resumes {
        println!("backed up to {}", backup(&db, &args.db, plan.from)?.display());
    }

    migrate::migrate(&db, migrations, &plan, CHUNK)?;
    println!("{shown} is in format {to}");

    Ok(())
}

/// Opens a database that is there already: never a new, empty one by mistake.
fn open(path: &Path) -> CliResult<RocksDb> {
    if !path.is_dir() {
        return Err(format!("there is no database at {}", path.display()).into());
    }

    RocksDb::open(path).map_err(|err| format!("cannot open {}: {err}; is a node using it?", path.display()).into())
}

/// Copies a database aside, beside it, named for the format it is in: chain.db.format-8, or
/// chain.db.format-8.1 and on when that is taken.
fn backup(db: &RocksDb, path: &Path, format: u32) -> Result<PathBuf, String> {
    let name = path.file_name().map_or_else(|| "chain.db".into(), |name| name.to_string_lossy().into_owned());
    let named = |suffix: String| path.with_file_name(format!("{name}.format-{format}{suffix}"));
    let copy = (0..)
        .map(|taken| named(if taken == 0 { String::new() } else { format!(".{taken}") }))
        .find(|copy| !copy.exists())
        .expect("some name is free");
    db.checkpoint(&copy).map_err(|err| err.to_string())?;

    Ok(copy)
}
