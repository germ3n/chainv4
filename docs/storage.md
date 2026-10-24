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
| `Meta` | Bookkeeping: the tip (`tip`), every leaf block (`tips`), and the storage format (`format`). | Node's |
| `Work` | Total work of the chain ending at each block, which fork choice compares. | Node's |
| `Undo` | What each recent main-chain block replaced, for reorgs. | Node's |
| `Receipts` | Where each transaction landed and what it did, by id. | Node's |
| `Validators` | FFG validators by voting key; authority finality's decided signers and ballot. | Yes |
| `Epochs` | Each epoch's validators or signers, and which checkpoints' voters were paid. | Yes |
| `Finality` | What this node learned about finality: justifications, the justified and finalized checkpoints, the votes it counted or holds, its own votes, and evidence. | No |

**Chain state** is what every node computes the same from the blocks.

**The node's own columns** follow from the blocks the node holds.

**`Finality`** is outside the chain's state altogether: it is learned from the network, so two
nodes can differ in it, and a reorg never touches it.

## The storage format

`STORAGE_FORMAT` names the layout records are kept in, and genesis writes it to `Meta`.
- **On open:** a node refuses a database kept in another format (`OldFormat`), or in none, as
  databases from before formats were recorded are. It never misreads one.
- **When to bump it:** whenever a stored record changes shape.
- **Format 1:** receipts carry traces, and the chain keeps its leaf blocks in `Meta`.
- **Format 2:** authority finality's approvals of a signer change record the epoch they were made
  in, so they can expire.

There are no migrations: delete the database, or point `--db` elsewhere.

## Undo records

**What they are:** each main-chain block keeps an undo record, every value it replaced, so a
reorg can take it back by writing those values back. Undoing a block also deletes its undo
record.

**How long they last:** records are kept for the last `max_reorg_depth` blocks; older ones are
deleted, since nothing can roll back that far.

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
- **Formats are checked, not migrated.** A database in another storage format is refused, so it
  must be deleted after a format change. A genesis change refuses it as well (`WrongGenesis`).
- **Commits are not fsynced.** A power cut can lose the last few blocks, though never half of
  one.
- **No pruning.** Every block and all history are kept, and side branches are never pruned.
- **API reads are not snapshot-consistent.** The API reads the database while the node writes it,
  so a request that reads several keys can see two different tips.
