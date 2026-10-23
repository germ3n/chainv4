# Notes and todo

## Things to know

- **Old `chain.db` files are refused.** Blocks now carry a difficulty and a seal enum, and genesis
  names its consensus, so every dev genesis id changed. Delete the database, or the node stops
  with `WrongGenesis`.
- **There are two dev chains, and pow is the default.** `--consensus pow` (mine with `--mine`)
  or `--consensus authority` (produce with `--produce`). Each is its own genesis and network.
- **The dev genesis depends on build features.** The dev chains allow every engine the node was
  built with, so nodes built with different engines get different genesis ids and land on
  separate networks.
- **Rewards have no coinbase transaction.** The account model needs none: after a block's
  transactions run, the chain credits `reward.at(height)` to the header's `producer`, and a
  reorg takes it back with the rest of the block. Rewards can be spent from the next block on;
  there is no maturity period.
- **Reorgs reach back at most 100 blocks.** `MAX_REORG_DEPTH` bounds how far the main chain
  rolls back; a heavier branch that splits off deeper is never followed, and undo records older
  than that are dropped.
- **The proof-of-work hash is named in genesis.** Only `PowHash::Blake3` exists; a memory-hard
  algorithm can join under a new tag without touching chains that use BLAKE3.
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
- **Lua numbers reach WASM contracts as float bits.** `Value::as_i64` and `Value::as_amount` in
  the SDK turn whole ones into integers using integer instructions only, since float
  instructions are refused at deploy.
- **Each call in a chain of calls gets its own memory limit.** The worst case is 8 nested calls
  × 32 MiB in one transaction.
- **The WASM `call` import takes a value pointer.** It is
  `call(address, value, name, name_len, args, args_len)`; rebuild contracts made against the
  earlier five-argument form. Contracts can also import `emit` and `delegate` now.
- **Contracts have four special functions.** `init` (run at deploy), `migrate` (run after an
  upgrade), `check_perms` (asked before an upgrade) and `fallback` (answers functions the
  contract does not export). Transactions and other contracts cannot call `init`, `migrate` or
  `fallback`, or delegate to them. See the README's Contract lifecycle.
- **Byte order.** Everything the chain encodes is little-endian. Three things are big-endian on
  purpose: the work hash is read as a big-endian number, so valid hashes start with zeros in hex;
  height keys, so RocksDB sorts them in order; and XXH3 digests as bytes, matching `xxhsum`.
- **The binary is also the client.** See the [README](../README.md). `chainv4 send` waits for
  inclusion by default (`--timeout`, `--confirmations`, `--no-wait`) and exits non-zero when the
  transaction landed but failed. Without `--key` it signs with the well-known development key.

## Todo

### Consensus

- [x] **Proof of work.** Genesis names the scheme. Under proof of work anyone mines: a nonce
  must make the BLAKE3 work hash of the block id meet the block's difficulty, which retargets
  every block toward a target block time. Miners run on threads of their own (`--mine`,
  `--mine-threads`).
- [x] **Fork choice and reorgs.** Side blocks are stored, and the branch with the most total work
  is the main chain. A heavier branch triggers a reorg: roll back to the fork with undo records,
  run the branch, one commit. A branch that fails to run is dropped, and its descendants refused.
- [ ] **No other schemes yet.** The authority is still one key whose seed is in the source. Next
  could be a rotating authority set, or stake with finality.
- [ ] **No finality.** Only the 100-block reorg limit stops deep rewrites.
- [ ] **Difficulty only looks at the parent.** A miner may set its timestamp up to 15 seconds
  ahead to lower the next difficulty a little. A median-time-past rule, or retargeting over a
  window of blocks, would make that harder.
- [ ] **Side branches are never pruned, and bad blocks are only remembered in memory.** With a
  low difficulty, a peer can fill the disk with side blocks cheaply; after a restart, a known-bad
  branch is run again before it is refused.
- [ ] **Block limits are not consensus rules.** `MAX_BLOCK_TXS` only limits the blocks this node
  builds. Import accepts any number of transactions, any size and any total fuel. Add maximum
  transactions, bytes and fuel per block, and check them on import.
- [x] **Block rewards.** Genesis sets an initial reward and an optional halving interval; each
  block's producer is credited after its transactions run.
- [ ] **No fees.** Transactions cost nothing, failed ones included, so spam is free. Fuel is
  capped per transaction but never paid for. Fees would go to the producer like the reward.
- [ ] **The reward is invisible in the API.** A block's JSON does not say what it minted; clients
  would have to know the genesis reward schedule.
- [ ] **Minting belongs to whoever genesis names.** The dev chains name the development key.
  Decide who may mint on a real chain, if anyone.

### Script layer

- [x] **Contracts can move funds.** `accounts.balance` and `accounts.transfer` in Lua, the
  `balance` and `transfer` host functions in WASM, `balance` and `transfer` in the SDK.
- [x] **Calls between contracts can send value.** `contracts.call_value` in Lua, the value
  argument of `call` in WASM, `call_with_value` in the SDK. The callee sees it in its context,
  and it goes back if the call fails.
- [x] **Events.** `events.emit(name, ...)` in Lua, the `emit` host function in WASM, `emit` in
  the SDK. They land in the transaction's receipt, in order, only if the call and every call
  above it succeed.
- [ ] **Events cannot be searched.** They are only reachable through their transaction's receipt;
  following a contract's events needs an index by contract and name.
- [ ] **Loading code is not charged.** What `init`, `check_perms` and `migrate` burn reaches the
  receipt, but loading the code does not: running a Lua script's top level burns fuel the
  receipt never shows, and compiling and validating either engine's code is unmetered, bounded
  only by `MAX_SOURCE_LEN` and `MAX_MODULE_LEN`. This applies to deploys and upgrades alike.
- [ ] **No read-only calls.** There is no way to run a contract function without a transaction,
  for example from the HTTP API.
- [x] **Contract upgrades.** `Tx::Upgrade` (`chainv4 send upgrade`) swaps a contract's code and
  keeps its address, storage and balance. The contract decides who may: the chain asks its
  `check_perms("upgrade", sender)`. A contract without it is immutable. New code's `migrate`
  runs once, and a failure undoes the whole upgrade.
- [x] **Constructors.** `init` runs once at deploy with the deployer as sender; its fuel and events
  land in the deploy's receipt. Only the chain may run `init` and `migrate`.
- [x] **A token standard.** ERC-20-like, by convention: [docs/token-standard.md](token-standard.md),
  with example tokens in Lua and Rust that one shared test holds to it.
- [x] **Delegate calls and fallback.** `contracts.delegate` / `delegate` run another contract's code
  on the caller's storage, like `delegatecall`; `fallback` answers functions a contract does not
  export. Together they make proxies, upgradable without the built-in upgrade, with examples in
  Lua and Rust.
- [ ] **No contract removal.** May be the right call; decide on purpose.
- [ ] **Lua proxies change value types in transit.** Arguments and return values pass through Lua,
  so an `i64` becomes a number. A pass-through that keeps the encoded values as they are would
  fix it.
- [ ] **No storage layout tools for proxies.** Keeping a proxy's and its implementations' keys
  apart is up to the contracts; a convention, or namespaced storage, would make it safer.
- [ ] **Only one permission exists.** `check_perms` is only ever asked about `"upgrade"`. Other
  privileged actions, such as removal, would ask with their own names.
- [ ] **Tokens are only a convention.** The chain cannot list tokens or tell a token from any other
  contract; a registry, or a check that a contract exports the standard functions, would help
  wallets.
- [ ] **WASM module cache.** Modules are parsed and compiled again on every call. Caching does not
  affect consensus, only speed.
- [ ] **Typed exports in the SDK.** `#[export]` functions take and return nothing and use
  `input()`/`output()`; decoding arguments into Rust parameters would be nicer. The token example
  shows how much argument handling that would save.
- [ ] **The Luau fork is pinned by branch, not commit.** `Cargo.lock` holds the commit, but
  `cargo update` would move it. Like wasmi it is consensus: pin a `rev` in `Cargo.toml`.

### Database

- [ ] **No state root.** Block headers do not commit to state. Needed for snapshot sync, light
  clients, and any later ZK work.
- [x] **Commits can be undone.** Each main-chain block keeps an undo record of every value it
  replaced, for the last 100 blocks.
- [ ] **No iteration.** `Read` only gets single keys. Snapshots, pruning, listing a contract's
  storage and most API listings need prefix or range scans.
- [ ] **No pruning.** Every block and all history are kept forever.
- [x] **Receipts and a transaction index.** Each transaction's block, index, fuel, result and
  events are stored by transaction id, and reorgs move them along with everything else.
- [ ] **No schema version.** Nothing in `Meta` records the storage format, so a format change
  cannot be detected or migrated.
- [ ] **Commits are not fsynced.** A power cut can lose the last few blocks, though never half of
  one. Decide whether to sync each block.
- [ ] **RocksDB runs on default options.** No tuning of caches, compression or compaction.
- [ ] **API reads are not snapshot-consistent.** A request that reads several keys can see state
  from two different tips.

### Network

- [x] **Sync.** A node asks each peer for its status when it connects, and again when it gossips a
  block that does not connect. While any peer's chain holds more work, it asks the heaviest one
  for the next 128 blocks and imports them in order, side branches and reorgs included. When a
  batch does not connect, the chains split earlier, so it steps a batch further back.
- [ ] **Sync uses one peer at a time.** Batches are fetched one after another from a single peer;
  downloading from several peers at once would be faster for long catch-ups.
- [ ] **Blocks gossiped ahead of the tip are dropped.** They only tell the node a peer is ahead;
  sync fetches them again. Keeping a few would save the round trip.
- [ ] **Gossip is forwarded before it is checked.** gossipsub relays blocks and transactions
  before the node validates them. Turn on `validate_messages` and report each result, so only
  valid messages spread.
- [ ] **No peer scoring or banning.** Peers that send invalid blocks, transactions or floods of
  sync requests pay no price. A peer whose sync batch does not fit is only forgotten.
- [ ] **The node's identity changes on every start.** The libp2p key is generated fresh each run;
  keep it in the data directory so the peer id is stable.
- [ ] **No bootnode list.** Peers come from `--peer` and mDNS only, and Kademlia warns at startup
  when it has nobody to bootstrap from.
- [ ] **No connection or request limits.**
- [ ] **No NAT traversal** (relay, hole punching, AutoNAT).

### Node, API and tooling

- [x] **Transactions can be submitted from outside.** `POST /txs` takes `{"tx": "0x…"}`, a
  `SignedTx` encoded as the chain encodes it, and hands it to the node's mempool, which gossips
  it. `chainv4 keygen` and `chainv4 send` make keys and sign and submit transactions.
- [x] **`chainv4 send` waits for inclusion.** It polls `GET /txs/{id}` until enough blocks hold
  the transaction or `--timeout` passes, then prints the result and events.
- [x] **A README for the CLI.** Building, running nodes, keys, sending transactions and the API.
- [ ] **The CLI speaks plain HTTP only.** ureq is built without TLS; add it before pointing
  `send` at a node across the internet.
- [ ] **Key files are not encrypted.** They are created readable by their owner only, but hold
  the seed in plain hex.
- [ ] **Only the dev chains exist.** There is no genesis file, so there is no way to start any
  other network.
- [ ] **No graceful shutdown.** Ctrl-C kills the process mid-loop. Commits are atomic, so data is
  safe, but the network and HTTP threads never close cleanly.
- [x] **The HTTP API has transaction lookup and receipts.** `GET /txs/{id}`, and each transaction
  in a block carries its receipt.
- [ ] **The HTTP API has no contract views and serves one request at a time.**
- [ ] **Windows builds in CI.** Left out because of RocksDB and Luau build setup.
