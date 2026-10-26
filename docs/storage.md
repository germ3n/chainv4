# Storage

The chain only talks to the database through two traits (`src/db/`), so the backend can change.
- `RocksDb` is for nodes: each column is a column family, and a commit is one write batch.
- `MemoryDb` is for tests, tools and dry runs: a sorted map, lost when dropped.

| Trait | What it does |
| --- | --- |
| `Read` | Reads one value by column and key. Databases and state layers both implement it. |
| `Database` | Commits a whole set of changes atomically, and scans committed keys by prefix. |

## Writing: state layers and commits

Nothing writes to a database directly. Changes collect in `State` layers:
- a contract call's layer sits on its transaction's;
- the transaction's sits on its block's;
- the block's sits on the database.

**Reads and merges:** a read sees its own layer's changes first, then falls through to the one
below. When a layer is done, it merges into the one below it, or is dropped to throw its changes
away. That is how a failed transaction or call leaves nothing behind.

**Commits:** a block commits its layer's changes in one atomic batch, so after a crash either all
of a block is there or none of it is. A reorg's whole rollback and replay is one batch too.

**Recorded state:** the registries read and write whichever layer they are given:
- `Accounts`: balances and nonces;
- `Contracts`: code and records;
- `Validators`: FFG bonds and sets;
- `Signers`: authority finality's signers.

An account that returns to the default, no balance and nonce 0, is deleted, so stored state only
holds addresses that matter.

## Columns

| Column | What it holds | Chain state? |
| --- | --- | --- |
| `Accounts` | Account by address: nonce and balance. | Yes |
| `Contracts` | Deployed contract by address. | Yes |
| `Storage` | Contract storage, by the contract's address followed by the contract's own key. | Yes |
| `Blocks` | Every block this node accepted, by id, on the main chain or a side branch. | Node's |
| `Heights` | Main-chain block id by height, big-endian so heights sort in order. | Node's |
| `Meta` | Bookkeeping: the tip (`tip`), every leaf block (`tips`), the last blocks that failed to run (`bad`), the storage format (`format`), and the block a node that synced from a snapshot took state at (`snapshot`). | Node's |
| `Work` | Total work of the chain ending at each block, which fork choice compares. | Node's |
| `Undo` | What each recent main-chain block replaced, for reorgs. | Node's |
| `Receipts` | Where each transaction landed and what it did, by id. | Node's |
| `Validators` | FFG validators by voting key; authority finality's decided signers and ballot. | Yes |
| `Epochs` | Each epoch's validators or signers, and which checkpoints' voters were paid. | Yes |
| `Finality` | What this node learned about finality: justifications, the justified and finalized checkpoints, the votes it counted or holds, its own votes, and evidence. | No |
| `Events` | The event index, on a node started with `--index-events`: each event under its contract, and pointers to it by contract and name and by every address in its data, newest first. | Node's |
| `Tokens` | The token registry: each token by address, with its name, symbol and decimals, and a listing key by symbol, in ASCII lower case, then a zero byte, then its address. | Yes |
| `Headers` | Every stored block's header and seal, by id: a node that synced from a snapshot keeps these alone for the blocks before its pivot. | Node's |
| `Light` | For genesis and each checkpoint, the light update: proofs of the voters it fixed, against its state root (see [Light client](light-client.md)). | Node's |
| `StateTree` | The state tree's nodes, by depth and the path bits above them, kept in step with the chain-state columns block by block (see [Chain](chain.md#the-state-root)). Its root is each header's `state_root`. | Node's |
| `Addresses` | The address index, on a node started with `--index-addresses`: each transaction under every address it touches, newest first. | Node's |
| `Names` | The [name registry](names.md#in-the-state): each name's record by `n` and the name, and each account's primary name by `p` and its address's key. | Yes |
| `Namespaces` | Contract storage in [namespaces](contracts.md#storage-namespaces) and each contract's own space: by the contract's address, the namespace's name with its length in front (none for the own space), then the contract's own key. Which namespaces a contract's code may use is its [layout](contracts.md#storage-layouts), kept in its `Contracts` record. | Yes |

**Chain state** is what every node computes the same from the blocks.

**The node's own columns** follow from the blocks the node holds.

**`Finality`** is outside the chain's state altogether: it is learned from the network, so two
nodes can differ in it, and a reorg never touches it.

## The indexes

A node may keep two indexes, each its own choice: neither is chain state, and two nodes can keep
different ones. What they share is in `src/chain/index.rs`.

**The event index** (`--index-events`, `src/chain/events.rs`) finds every event on the main chain:
- **Once:** each event is stored once, under its contract, with its transaction, block, height and
  place.
- **Pointers:** keys by contract and name, and by every address in its data, point at it.

**The address index** (`--index-addresses`, `src/chain/addresses.rs`) finds every transaction
that touched an address: what a wallet lists a history from, plain coin transfers to it included,
which no event tells of. A transaction touches (`touched`):
- its sender;
- a transfer's, mint's or burn's parties;
- the contract it calls or upgrades, and every address among the call's arguments, as
  `transfer(to, amount)` names its recipient;
- the contract it deployed.

Each address keeps its own small record (`IndexedTx`: the id, block, height and place), so a
listing is one scan. It only says where to look: a light client proves the transaction against
its block's `tx_root` and what it did against the `receipts_root`. Coin a contract pays out, which
only the trace shows, is not in it.

Both work the same way:
- **Newest first:** heights and places are stored inverted, so a listing reads forward from where
  the last page stopped.
- **Reorgs:** a new block's entries go into the same commit as its receipts, so the block's undo
  record takes them off with it.

**Which blocks an index holds.** `Meta` keeps two records for each: its low end (`events_from`,
`addresses_from`) and which blocks it keeps (`events_keep`, `addresses_keep`), as the flag set
them. An index always holds every main-chain block from its low end to the tip, and the node moves
the low end toward what it keeps:
- **Down:** it reads older blocks and their receipts back, newest first, and indexes them. A block
  a reorg could still take off gets those entries added to its undo record, so a reorg takes them
  off too.
- **Up:** a `latest-N` window drops the oldest block's entries once the tip moves past it.
  Asking for fewer blocks than are held drops the rest the same way.

**A step at a time:** each step reads at most 500 blocks or 2,000 transactions, and commits the
new low end together with the entries it wrote or removed. A node stopped mid-way picks up where
the last commit left off, and an index never has gaps.

**Starting:** a new index starts empty at the block after the tip, and steps down from there.
Once on, the database keeps it on, even without the flag, keeping the same blocks.

**Turning one off:** `--index-events=off` or `--index-addresses=off` deletes that index's whole
column and its two records in `Meta` in one atomic write, `Database::clear`: a range delete in
RocksDB, so it is quick however big the index is. The other index stays as it was. A new index
clears its column the same way first, in case anything was left behind.

## The storage format

`STORAGE_FORMAT` names the layout records are kept in, and genesis writes it to `Meta`.
- **On open:** a node moves an older database up to its format first, where migrations reach
  (see [Migrations](#migrations)). It refuses one kept in a format they do not reach
  (`OldFormat`), or in none, as databases from before formats were recorded are. It never
  misreads one.
- **When to bump it:** whenever a stored record changes shape, with a migration to the new
  format.
- **Format 1:** receipts carry traces, and the chain keeps its leaf blocks in `Meta`.
- **Format 2:** authority finality's approvals of a signer change record the epoch they were made
  in, so they can expire.
- **Format 3:** the token registry, which an older database would be missing its tokens from.
- **Format 4:** contract records keep their code's [storage layout](contracts.md#storage-layouts).
- **Format 5:** layouts declare records and their keys, and kinds the node checks.
- **Format 6:** headers commit to a state root and a receipts root, and the state tree is kept.
- **Format 7:** every header is kept in `Headers`, state tree leaves name their entry's column and
  key, and checkpoints keep light updates.
- **Format 8:** `tx_root` is a Merkle root, so stored headers changed meaning, and the address
  index has a column of its own.

Formats before 8 have no migrations: delete such a database, or point `--db` elsewhere.

## Migrations

From format 8 on, each new format comes with a migration that moves a database to it from the
one before, and back (`chain::migrate`). `MIGRATIONS` lists them, oldest first.

**What a node does as it starts:**
1. finishes a migration a run before left part way;
2. finds a database in an older format, copies it aside with a RocksDB checkpoint, beside it as
   `chain.db.format-8`, and moves it up one format at a time, logging each step;
3. opens the chain.

A database in a newer format is refused: going back down is only ever asked for, never automatic.

**By hand,** with the node stopped:

```sh
chainv4 db format --db chain.db              # the format it is in, and what this chainv4 knows
chainv4 db migrate --db chain.db             # up to this chainv4's format
chainv4 db migrate --db chain.db --to 8      # back down: shows what it undoes, asks for yes
chainv4 db migrate --db chain.db --to 8 --yes --no-backup
```

- **Going back down** takes the chainv4 that wrote the newer format: only it knows how to undo
  its formats. It lists every step it undoes, and goes on only once you type `yes`, or pass
  `--yes`.
- **The backup** is a checkpoint: RocksDB hard-links the files it can, so it is quick, and takes
  room only as the database moves on. A name already taken gets `.1`, `.2` and so on. Delete it
  once the node runs well; `--no-backup` skips it.
- **One-way migrations:** a migration that cannot be undone says why, and a downgrade past it is
  refused before anything changes.

**Crash-safe, a chunk at a time.** A migration works through the data in chunks of about
`CHUNK` records, 10,000. Each chunk's changes are committed atomically with where it got to, under
`Meta`'s `migrating` key, and the format changes only with a step's last chunk. A crash or a stop
leaves the database wholly in one format with a step under way:
- the next `db migrate`, or the next start, finishes that step from where it stopped, with no
  second backup of a half-migrated database;
- a node refuses to open the chain while a step is under way (`Migrating`).

**Writing one:** bump `STORAGE_FORMAT`, note what changed in its documentation, and add a
`Migration` to `MIGRATIONS`. `src/chain/migrate/example.rs` is a complete one, tested on a real
chain's database though not in `MIGRATIONS`: an imagined format 9 whose work records keep their
block's height beside the total work.

```rust
/// Work records as format 8 keeps them, and as format 9 does: the migration's own copies of each
/// shape, so it still reads format 8 once the chain's WorkRecord has moved on.
struct WorkV8 { #[wincode(with = "U256Le")] total: u256 }
struct WorkV9 { #[wincode(with = "U256Le")] total: u256, height: u64 }

pub struct WorkHeights;

impl Migration for WorkHeights {
    fn from(&self) -> u32 {
        8
    }

    fn describe(&self) -> &'static str {
        "work records keep their block's height"
    }

    /// Gives each work record in the chunk its block's height, from the block's stored header.
    fn up(&self, at: &At) -> Result<Chunk, DbError> {
        let records = next_records(at)?;   // up to at.limit of them, from at.cursor
        let mut chunk = Chunk::default();

        for (key, value) in &records {
            let old: WorkV8 = decode(value)?;
            let header: SealedHeader = decode(&at.db.read(Column::Headers, key)?.ok_or(…)?)?;
            let new = WorkV9 { total: old.total, height: header.header.height };
            chunk.changes.insert((Column::Work, key.clone()), Some(new.hashable_bytes_with_tag()));
        }

        chunk.next = after(&records, at.limit);   // past the last key, unless this was the last chunk

        Ok(chunk)
    }

    /// Takes the heights back out: format 8 has nowhere to keep them.
    fn down(&self, at: &At) -> Result<Chunk, DbError> {
        // The same scan, writing WorkV8 { total } for each WorkV9.
    }
}
```

- **A chunk is all or nothing,** and runs again from its cursor after a crash, so it must read
  where it starts from the cursor alone.
- **Never write `Meta`'s `format` or `migrating`:** the run writes those.
- **A migration changes how things are kept, never what the chain says.** State roots, ids and
  hashes stay what they were; a change to those is a new chain, not a new format.
- **Test both ways,** up and down, on a database big enough for several chunks, and stopped part
  way: `chain::migrate`'s tests show how.

## Undo records

**What they are:** each main-chain block keeps an undo record, every value it replaced, so a
reorg can take it back by writing those values back. Undoing a block also deletes its undo
record.

**How long they last:** records are kept for the last `max_reorg_depth` blocks; older ones are
deleted, since nothing can roll back that far.

**What they hold:** a block's state tree nodes too, so undoing a block puts its tree back. That
adds about ten nodes to the record for every entry the block changed.

## Scans

`Database::scan(column, prefix, start, visit)` visits a column's committed records whose keys
start with a prefix, in key order, from the first at or after `start`, until `visit` returns
false.

**Cost:** both backends keep keys sorted, so a scan seeks once (O(log n)) and then reads only
what it visits: an iterator on RocksDB, a range on MemoryDb.

**Limits:**
- scans see committed data only, never a `State`'s uncommitted changes;
- `visit` must not write to the database.

**Use so far:** finality uses scans to find its stored votes. Listing a contract's storage from
inside a block would need `State` to merge its changes into a scan.

## Keys and encoding

Records are stored in the chain's encoding (see [Crypto and encoding](crypto.md)). Keys are:
- an address's or id's encoding with its tag;
- heights and epochs in big-endian, so they sort in order;
- prefixes for kinds of record within a column, such as `j:` for justifications or `v:` for
  counted votes in `Finality`.

## What to keep in mind

- **A database belongs to one chain.** Opening one written for another genesis fails with
  `WrongGenesis`.
- **Formats are migrated from 8 on.** A node moves an older database up as it starts, after a
  backup; one in a format no migration reaches is refused, and must be deleted. A genesis change
  refuses it as well (`WrongGenesis`).
- **Commits are not fsynced.** A power cut can lose the last few blocks, though never half of
  one.
- **The main chain is never pruned.** Every main-chain block and all history are kept. Side
  branches go once the chain can no longer follow them: when they split off more than
  `max_reorg_depth` blocks below the tip, or below the finalized checkpoint (see
  [Chain](chain.md#reorganizing)).
- **API reads are not snapshot-consistent.** The API reads the database while the node writes it,
  so a request that reads several keys can see two different tips.
