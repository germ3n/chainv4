# Notes and todo

## Things to know

- **Old `chain.db` files are refused.** Genesis now lists the engines a chain allows, so the dev
  genesis id changed. Delete the database, or the node stops with `WrongGenesis`.
- **The dev genesis depends on build features.** `Genesis::dev` allows every engine the node was
  built with, so nodes built with different engines get different genesis ids and land on
  separate networks.
- **The end-to-end SDK test needs the wasm32 target.** It builds `contracts/` with cargo:
  `rustup target add wasm32-unknown-unknown`. CI installs it.
- **Contracts build from their own workspace.** `contracts/.cargo/config.toml` sets the wasm32
  target, but only when cargo runs inside `contracts/`. From the repo root, pass
  `--target wasm32-unknown-unknown`. The SDK's own tests run on the host target.
- **wasmi is pinned to `=2.0.0`.** Its version, enabled features and stack limits are consensus:
  an upgrade can change what a module does or costs. Upgrade on purpose, never with
  `cargo update`.
- **Engines are consensus.** Every node on a chain must be built with the engines its genesis
  lists. A node missing one refuses to open the chain rather than fall out of step.
- **Reentrancy is guarded by default.** A function can only run while its contract is already
  running further up the call chain if it is marked: `--@reentrant` on the line directly above
  `function M.name(` in Lua, `#[export(reentrant)]` in Rust.
- **Lua annotations are read line by line.** A `--@reentrant` line inside a multi-line string
  would be read as an annotation too.
- **Lua numbers reach WASM contracts as float bits.** `Value::as_i64` in the SDK turns whole ones
  into integers using integer instructions only, since float instructions are refused at deploy.
- **Each call in a chain of calls gets its own memory limit.** The worst case is 8 nested calls
  × 32 MiB in one transaction.

## Not done yet

### Consensus

- **Only a development scheme exists.** `DevAuthority` lets one key produce every block, and its
  seed is in the source. A real network needs a real scheme: a rotating authority set first,
  then stake with finality.
- **No fork choice or reorgs.** Import only extends the tip; a competing block at the same height
  is dropped as `NotOnTip`. Any scheme with more than one producer needs a fork-choice rule, and
  the database needs a way to roll state back (see Database).
- **No finality.**
- **Block limits are not consensus rules.** `MAX_BLOCK_TXS` only limits the blocks this node
  builds. Import accepts any number of transactions, any size and any total fuel. Add maximum
  transactions, bytes and fuel per block, and check them on import.
- **No fees or rewards.** Transactions cost nothing, failed ones included, so spam is free. Fuel
  is capped per transaction but never paid for.
- **Minting belongs to the dev authority key.** Decide who may mint on a real chain, if anyone.

### Script layer

- **Contracts cannot move funds.** A contract call can send value to a contract, but neither
  engine can read a balance or transfer out of one, so funds sent to a contract are stuck. Add
  `balance` and `transfer` to both engines' host interfaces.
- **Calls between contracts send no value.** `ctx.value` and the `value` host function read zero
  in nested calls.
- **No events or logs.** Contracts have no way to emit anything for clients to follow.
- **Deploys are not charged.** Running a Lua contract's top level at deploy burns fuel that never
  reaches the receipt (`fuel_used` is 0). Compiling and validating are unmetered, bounded only by
  `MAX_SOURCE_LEN` and `MAX_MODULE_LEN`.
- **No read-only calls.** There is no way to run a contract function without a transaction, for
  example from the HTTP API.
- **No contract upgrades or removal.** May be the right call; decide on purpose.
- **WASM module cache.** Modules are parsed and compiled again on every call. Caching does not
  affect consensus, only speed.
- **Typed exports in the SDK.** `#[export]` functions take and return nothing and use
  `input()`/`output()`; decoding arguments into Rust parameters would be nicer.
- **The Luau fork is pinned by branch, not commit.** `Cargo.lock` holds the commit, but
  `cargo update` would move it. Like wasmi it is consensus: pin a `rev` in `Cargo.toml`.

### Database

- **No state root.** Block headers do not commit to state. Needed for snapshot sync, light
  clients, and any later ZK work.
- **Commits cannot be undone.** Reorgs need either undo records per block or versioned state.
- **No iteration.** `Read` only gets single keys. Snapshots, pruning, listing a contract's
  storage and most API listings need prefix or range scans.
- **No pruning.** Every block and all history are kept forever.
- **Receipts are not stored, and there is no transaction index.** A transaction's result is lost
  once its block is imported, and nothing maps a transaction id to its block.
- **No schema version.** Nothing in `Meta` records the storage format, so a format change cannot
  be detected or migrated.
- **Commits are not fsynced.** A power cut can lose the last few blocks, though never half of one.
  Decide whether to sync each block.
- **RocksDB runs on default options.** No tuning of caches, compression or compaction.
- **API reads are not snapshot-consistent.** A request that reads several keys can see state from
  two different tips.

### Network

- **Sync.** Nodes answer `Status` and `Blocks` requests but never send them, so a node that starts
  late or misses a block stalls at "waiting for sync". Responses and failed requests are only
  logged.
- **Gossip is forwarded before it is checked.** gossipsub relays blocks and transactions before
  the node validates them. Turn on `validate_messages` and report each result, so only valid
  messages spread.
- **No peer scoring or banning.** Peers that send invalid blocks, transactions or floods of sync
  requests pay no price.
- **The node's identity changes on every start.** The libp2p key is generated fresh each run; keep
  it in the data directory so the peer id is stable.
- **No bootnode list.** Peers come from `--peer` and mDNS only, and Kademlia warns at startup when
  it has nobody to bootstrap from.
- **No connection or request limits.**
- **No NAT traversal** (relay, hole punching, AutoNAT).

### Node, API and tooling

- **Transactions cannot be submitted from outside.** The HTTP API is read-only and there is no
  CLI command, so only code can call `Node::submit`. Add `POST /txs` or a client tool.
- **Only the dev genesis exists.** There is no genesis file, so there is no way to start any other
  network.
- **No key management.** Only the dev key exists; no key generation or keystore.
- **No graceful shutdown.** Ctrl-C kills the process mid-loop. Commits are atomic, so data is safe,
  but the network and HTTP threads never close cleanly.
- **The HTTP API has no transaction lookup, receipts or contract views.** It also serves one
  request at a time.
- **Windows builds in CI.** Left out because of RocksDB and Luau build setup.
