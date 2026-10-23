# Notes and todo

## Things to know

- **Old `chain.db` files are refused.** Headers now commit to finality proofs, and genesis names
  its hash, limits and finality settings, so every dev genesis id changed again. Delete the
  database, or the node stops with `WrongGenesis`.
- **There are two dev chains, and pow is the default.** `--consensus pow` (mine with `--mine`)
  or `--consensus authority` (produce with `--produce`). Each is its own genesis and network.
- **The dev genesis depends on build features.** The dev chains allow every engine the node was
  built with, so nodes built with different engines get different genesis ids and land on
  separate networks.
- **Rewards have no coinbase transaction.** The account model needs none: after a block's
  transactions run, the chain credits `reward.at(height)` to the header's `producer`, and a
  reorg takes it back with the rest of the block. Rewards can be spent from the next block on;
  there is no maturity period.
- **Reorgs reach back at most `max_reorg_depth` blocks**, 100 on the dev chains. It bounds how
  far the main chain rolls back; a heavier branch that splits off deeper is never followed, and
  undo records older than that are dropped. Finality bounds it further: nothing at or below the
  finalized checkpoint comes off.
- **Every chain parameter is in genesis.** Consensus and timing, hashes, rewards, allocations,
  the minter, engines, transaction limits, the reorg depth and every finality setting.
  `chainv4 genesis` writes the dev defaults as a JSON file and `--genesis FILE` runs it; files
  must name every field and may not add any.
- **Both dev chains run FFG finality.** The dev key is the one validator, bonded with 10,000
  coins, with epochs of 8 blocks. Without a node running `--validate`, nothing finalizes, and the
  chain runs as before. A build without `finality-ffg` gives the dev chains no finality, so like
  engines, it puts them on a different network.
- **Finality kinds are consensus and features.** `finality-ffg`, `finality-authority` and
  `finality-depth`, all default. A node built without the kind its genesis names refuses the
  genesis file. The chain only talks to finality through the `Finality` trait
  (`src/chain/finality/`); the two voting kinds share one vote engine and differ in their
  `Electorate`.
- **Genesis bonds are extra.** Genesis validators' bonds are locked on top of the allocations,
  not taken from them.
- **A voting key belongs on one node.** What a node has voted is kept in its own database; two
  nodes voting with one key can sign conflicting votes and get it slashed.
- **Hashes are chosen in genesis.** `Genesis.hash` is the chain hash: block and transaction
  ids, the roots blocks commit to, contract addresses and gossip ids. The proof-of-work config
  names its own work hash. Either can be any secure 256-bit algorithm the build has: sha256,
  blake3, blake2s, blake2b or keccak256. XXH3 is refused for both; it only names contract
  functions. A header hashes itself with its parent's algorithm, which genesis fixes through its
  zero parent, so ids from another chain's hash never connect. The dev chains use BLAKE3 when it
  is built in. Contracts and nodes need at least one secure hash feature; nothing requires
  BLAKE3 any more.
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
  inclusion by default (`--timeout`, `--confirmations`, `--final`, `--no-wait`) and exits
  non-zero when the transaction landed but failed. Without `--key` it signs with the well-known development key.

## Todo

### Consensus

- [x] **Proof of work.** Genesis names the scheme. Under proof of work anyone mines: a nonce
  must make the work hash of the block id meet the block's difficulty, which retargets
  every block toward a target block time. Miners run on threads of their own (`--mine`,
  `--mine-threads`).
- [x] **Fork choice and reorgs.** Side blocks are stored, and the branch with the most total work
  is the main chain. A heavier branch triggers a reorg: roll back to the fork with undo records,
  run the branch, one commit. A branch that fails to run is dropped, and its descendants refused.
- [ ] **No other schemes yet.** The authority is still one key whose seed is in the source. Stake
  now only finalizes; producing blocks by stake, or a rotating authority set, could come next.
  Tendermint-style instant finality replaces block production, so it would be a consensus scheme,
  not a finality kind.
- [x] **Finality.** Casper FFG-style: validators bond on chain (`Tx::Bond`, `Unbond`,
  `Withdraw`) with a separate voting key, and vote at each epoch's checkpoint. Two thirds by
  bond justify a checkpoint, and a justified direct child finalizes its parent. Votes are
  gossiped, justifications stored and fetched by sync, and blocks carry them as proofs that pay
  each voter `vote_reward`. Conflicting votes are evidence; `Tx::Slash` takes the bond and pays
  the reporter a share. Fork choice never goes below the finalized checkpoint, follows work only
  from the justified one on, and follows a newer justified branch over more work.
- [x] **Switchable finality.** Genesis picks the kind as `finality.type`: `ffg`; `authority`, the
  same votes from fixed signers with a threshold above half and no staking; or `depth`, final
  under a number of blocks with no votes. `chainv4 genesis --finality` writes each.
- [ ] **Authority signers are fixed.** Changing them takes a new chain. A signer-set change voted
  by the current signers, taking effect a few epochs on as FFG's sets do, would fix it.
- [ ] **Conflicting authority votes go unpunished.** They are kept as evidence and logged, but
  with no bonds there is nothing to take; the remedy is social.
- [ ] **Depth finality is each node's own.** Nodes split for longer than `depth` blocks finalize
  different branches and never rejoin, and a node syncing from scratch keeps whichever branch it
  syncs first.
- [ ] **A slashed validator keeps its weight in sets already fixed.** Its bond is zero at once,
  but sets are fixed `activation_delay` epochs ahead, so it counts until it leaves them. Dropping
  slashed validators from every set would close that window.
- [ ] **Finality stalls if a third goes quiet.** There is no inactivity leak, and no penalty for
  not voting, so a chain whose validators drop below two thirds never finalizes again until
  they return.
- [ ] **Vote tallies live in memory.** A restart forgets votes for links not yet justified, the
  held-back votes, and the votes double-vote detection compares against. Justifications and
  finality are stored; storing tallies would let a node resume counting.
- [ ] **Vote rewards are minted.** Nothing pays for them but new supply. Fees, once there are
  any, could fund them instead.
- [ ] **No checkpoint sync.** A new node replays every block from genesis. There is no trusted
  recent checkpoint to start from, which a long-lived staked chain needs against long-range
  attacks.
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
- [x] **Genesis files.** `chainv4 genesis` writes the dev defaults as JSON; `--genesis FILE`
  runs any chain one describes. `--producer-key` names an authority other than the dev key.
- [x] **Finality in the API and CLI.** `/status` shows the kind and the justified and finalized
  checkpoints, receipts say whether they are final, `/validators` and `/evidence` list validators
  and conflicting votes, and `send` has `bond`, `unbond`, `withdraw`, `slash` and `--final`.
- [ ] **No graceful shutdown.** Ctrl-C kills the process mid-loop. Commits are atomic, so data is
  safe, but the network and HTTP threads never close cleanly.
- [x] **The HTTP API has transaction lookup and receipts.** `GET /txs/{id}`, and each transaction
  in a block carries its receipt.
- [ ] **The HTTP API has no contract views and serves one request at a time.**
- [ ] **Windows builds in CI.** Left out because of RocksDB and Luau build setup.
