# Notes and todo

What chainv4 still lacks, and what got done. The things to keep in mind while running or changing
it are in [Good to know](good-to-know.md), and how each part works is in the
[docs](README.md).

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
  same votes from signers genesis names, with a threshold above half and no staking; or `depth`, final
  under a number of blocks with no votes. `chainv4 genesis --finality` writes each.
- [x] **Authority signers change by approval.** `Tx::ApproveSigners` (`send approve-signers`)
  names a whole new set and both thresholds; once `change_threshold` of the decided signers
  approve the same one for the same version, it is decided and fixed for epochs
  `activation_delay` on. `GET /signers` shows the ballot.
- [x] **Signer approvals expire and can be withdrawn.** An approval counts for genesis's
  `approval_epochs` epochs from the one it was made in, and only approvals that still count
  decide a change. `Tx::WithdrawApproval` (`send withdraw-approval`) takes one back.
- [ ] **A signer's key is its account.** Approvals are sent from the address of the signer's
  voting key, so that key has to sign transactions too, unlike FFG, where the voting key stays
  apart from the bond's account.
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
- [x] **Vote tallies are stored.** Every vote a node counts or holds back is kept in the
  Finality column by target epoch and voter, and counted again when the chain opens, so tallies,
  held votes and double-vote detection survive a restart. Counted votes stay 64 epochs behind
  the finalized checkpoint, to catch one vote surrounding another, and then go.
- [x] **Only voters are remembered.** Double-vote detection compares a vote against its voter's
  counted votes, and, once the vote shows its key votes, against that voter's held ones. Votes
  are remembered only when counted, so throwaway keys can neither grow memory beyond the capped
  held votes nor push real evidence out of the evidence log.
- [ ] **Vote rewards are minted.** Nothing pays for them but new supply. Fees could fund them
  instead, from what producers get or what is burned.
- [ ] **No checkpoint sync.** A new node replays every block from genesis. There is no trusted
  recent checkpoint to start from, which a long-lived staked chain needs against long-range
  attacks.
- [ ] **Difficulty only looks at the parent.** A miner may set its timestamp up to 15 seconds
  ahead to lower the next difficulty a little. A median-time-past rule, or retargeting over a
  window of blocks, would make that harder.
- [ ] **Side branches are never pruned, and bad blocks are only remembered in memory.** With a
  low difficulty, a peer can fill the disk with side blocks cheaply; after a restart, a known-bad
  branch is run again before it is refused.
- [x] **A block gas limit.** On a chain with fees, a transaction is only taken while its gas
  limit fits what the block has left, and the header's gas used must match, both checked on
  import.
- [ ] **Other block limits are not consensus rules.** A chain without fees still takes any number
  of transactions of any size, and `MAX_BLOCK_TXS` only limits the blocks this node builds.
  Bytes per block would bound blocks on any chain.
- [x] **Block rewards.** Genesis sets an initial reward and an optional halving interval; each
  block's producer is credited after its transactions run.
- [x] **Fees.** Every transaction pays for its gas, failed ones too, with what it does not use
  refunded. Genesis picks the gas schedule (`weighted`, or `contract`, where a contract's `score`
  says what is charged) and the pricing (`base_fee`, EIP-1559-style, or `bid`), with the block gas
  limit and the burned share of the base part; the rest goes to the producer. Producers take the
  best-paying transactions first. New schedules and pricings implement `GasSchedule` and
  `Pricing`.
- [ ] **No gas estimation.** `send` sizes gas limits from the schedule and the chain's fuel cap,
  not by running the transaction, so calls reserve far more than they use; the reserve comes
  back, but the sender must hold it. Read-only calls would allow estimates.
- [ ] **Transactions that cannot pay linger.** The mempool checks a transaction can pay when it
  arrives, but one whose sender's funds have gone since, or whose price fell under the base price,
  waits until its nonce is used, and every block built tries it again.
- [ ] **A scorer runs for every transaction.** The chain pays its fuel, bounded by its `fuel`
  setting, so a costly scorer slows every block. It may only read; a scorer that keeps counts
  would need its writes kept.
- [ ] **The reward is invisible in the API.** A block's JSON does not say what it minted; clients
  would have to know the genesis reward schedule.
- [ ] **Minting belongs to whoever genesis names.** The dev chains name the development key.
  Decide who may mint on a real chain, if anyone.

### Script layer

- [x] **A nil value.** `TxCallArgumentType::Nil` (tag 11): Lua's `nil` both ways, so functions
  may return it, `null` in the API, `nil` on the command line, `Value::Nil` in the SDK.
- [x] **Contracts can move funds.** `accounts.balance` and `accounts.transfer` in Lua, the
  `balance` and `transfer` host functions in WASM, `balance` and `transfer` in the SDK.
- [x] **Calls between contracts can send value.** `contracts.call_value` in Lua, the value
  argument of `call` in WASM, `call_with_value` in the SDK. The callee sees it in its context,
  and it goes back if the call fails.
- [x] **Events.** `events.emit(name, ...)` in Lua, the `emit` host function in WASM, `emit` in
  the SDK. They land in the transaction's receipt, in order, only if the call and every call
  above it succeed.
- [x] **Events can be searched.** A node started with `--index-events` indexes every event by its
  contract, by its contract and name, and by every address in its data; `GET /events` lists them
  newest first, a page at a time, and the explorer shows them on contract and account pages.
- [x] **The event index catches up.** `--index-events` takes `genesis`, a height or `latest-N`.
  The node reads older blocks' receipts back in the background, a step at a time, and a
  `latest-N` window drops the oldest blocks' events as the tip moves.
- [x] **The event index can be turned off.** `--index-events=off` deletes it at startup with
  `Database::clear`, one atomic range delete however big it is.
- [x] **Loading code is charged.** A deploy or upgrade pays `cost::LOAD_BYTE` (1 fuel) a byte to
  compile its code, plus what a Lua script's top level burns, and a failing one pays for what it
  burned before it failed. A top level runs before a call is attached, so it is the same work at
  deploy and on every call, and every call pays for it.
- [x] **Read-only calls.** `POST /call` runs a contract function as a transaction would call it,
  on a throwaway copy of the committed state under the chain's fuel cap.
- [x] **Traces.** Every receipt keeps where its contracts went: each call, delegate and transfer,
  failed ones included, which events each emitted, up to 256 hops. Not consensus.
- [x] **Failed deploys and upgrades keep their trace and fuel.** When loading, `init`,
  `check_perms` or `migrate` fails, the receipt still shows where it went and what it burned.
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
- [x] **Lua contracts can pass values on untouched.** `ctx.args` is the call's arguments as a
  values list, which `contracts.call`, `call_value`, `delegate` and `events.emit` take in place
  of loose values; a call given one returns one, and a function returning one returns its values
  as they are. The Lua proxy forwards this way, so no value changes type passing through.
- [x] **Namespaced storage.** `storage.namespace(name)` and `storage.own` in Lua,
  `storage::namespace` and `storage::own()` in Rust, have keys of their own, apart from plain
  storage and each other. Code a contract delegates to may read its own space but never write
  it, so the example proxies keep their owner and implementation there, out of their
  implementations' reach.
- [x] **Storage layouts.** Code declares the namespaces it keeps its state in, and what each
  holds (`--@namespace NAME KIND`, `--@retired NAME`; `namespace!` and `retired!` in Rust), and
  may use no others. `Tx::Upgrade` refuses new code that drops one or changes its kind, proxies
  check with `check_layout` before repointing, and `chainv4 layout` compares two versions
  before anything is sent.
- [ ] **Layouts stop at namespaces.** Keys inside a namespace, plain storage, and whether values
  match their kind are not checked: a kind is the code's own label, and renaming a key inside a
  namespace still starts it empty.
- [ ] **Only one permission exists.** `check_perms` is only ever asked about `"upgrade"`. Other
  privileged actions, such as removal, would ask with their own names.
- [x] **A token registry.** A contract that exports every function genesis names in
  `token_functions`, ERC-20's six by default, is registered as it deploys or upgrades, with what
  its optional `name()`, `symbol()` and `decimals()` answer; `GET /tokens` lists them by symbol.
- [x] **Tokens refresh their metadata.** `tokens.refresh()` in Lua, `refresh_token()` in the SDK,
  asks the registry to ask a token's name, symbol and decimals again once the transaction
  succeeds; an answer that does not fit fails it.
- [x] **Faster calls, same results.** Compiled WebAssembly modules are cached by their code, and
  the Lua engine builds fresh states ahead and closes used ones on threads of its own. Every call
  still runs in a fresh state or instance, so neither changes what a call does.
- [x] **Blocks run on every core.** Transactions run side by side on the block's starting state,
  recording what they read, then in order, keeping each result unless an earlier transaction wrote
  what it read and running those again. The result is what running them one by one gives, and
  signatures, in transactions and in finality proofs, check side by side.
- [x] **Transactions that depend on each other run once.** Each transaction waits for the last
  one before it that names its sender, recipient, contract or staking and signer records, and
  runs in rounds once its wait is over, with signatures checked alongside the first round. Only
  a contract reaching what nobody named, and reading what an earlier one wrote, still runs twice.
- [x] **Signatures are checked off the node loop.** A verifier thread checks arriving
  transactions' signatures in batches on the chain's workers, and hands every event on in order.
- [x] **Gossip waits in a bounded backlog.** Transactions and votes from the network, and
  submissions, count against `--max-gossip-backlog` in all and `--max-gossip-per-peer` per peer.
  Past them gossip is dropped and submissions get 503; a peer over its share 10 times in a minute
  is disconnected. See [Node](node.md#the-gossip-backlog) for what hardening is still to do.
- [x] **Typed exports in the SDK.** `#[export]` reads the call's arguments into the function's
  parameters and returns what it returns, refusing arguments that do not fit; the token and
  counter examples use it.
- [ ] **The Luau fork is pinned by branch, not commit.** `Cargo.lock` holds the commit, but
  `cargo update` would move it. Like wasmi it is consensus: pin a `rev` in `Cargo.toml`.

### Database

- [ ] **No state root.** Block headers do not commit to state. Needed for snapshot sync, light
  clients, and any later ZK work.
- [x] **Commits can be undone.** Each main-chain block keeps an undo record of every value it
  replaced, for the last 100 blocks.
- [x] **Prefix scans.** `Database::scan` visits a column's keys with a prefix, in order, from a
  start key: one seek, then only what it visits, on both backends. Finality uses it to find
  stored votes.
- [ ] **State layers cannot scan.** Scans see committed data only, so code running inside a
  block still reads single keys. Snapshots, pruning, listing a contract's storage and most API
  listings could build on `Database::scan`; contract storage iteration would need State to merge
  its changes into a scan.
- [ ] **No pruning.** Every block and all history are kept forever.
- [x] **Receipts and a transaction index.** Each transaction's block, index, fuel, result and
  events are stored by transaction id, and reorgs move them along with everything else.
- [x] **A storage format version.** `Meta` records `STORAGE_FORMAT`, and a node refuses a
  database kept in another (`OldFormat`) rather than misread it.
- [ ] **No migrations.** A database in an old format has to be deleted; nothing upgrades it.
- [x] **Leaves are stored.** The chain's leaf blocks go into `Meta` with each block, so side
  branches outlast a restart and the API can list them (`/forks`).
- [ ] **Leaves are never pruned.** Every side branch's leaf stays, even far below the finalized
  checkpoint, where no branch can matter again.
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
  sync requests pay no price. A peer whose sync batch does not fit is only forgotten, and one the
  backlog disconnects for flooding can connect again at once.
- [ ] **Backlog shares are per peer id.** One host with many ids gets many shares; limits per IP
  address would close that.
- [ ] **Invalid gossip is not struck.** The backlog strikes a peer for volume only, not for
  transactions that fail their signature check or blocks that do not import.
- [ ] **Blocks and sync requests are not counted in the backlog.** Gossiped blocks and requests
  from syncing peers still queue without bound.
- [ ] **The strike rules are fixed.** `MAX_STRIKES` and `STRIKE_WINDOW` are constants, and a small
  share can cut off an honest peer relaying a burst.
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
- [ ] **The HTTP API serves one request at a time.** A slow read-only call holds up every other
  request; a multi-threaded runtime, or calls on a pool of their own, would keep the API answering.
- [x] **An explorer.** `explorer/`, built with egui, for the desktop and, built to WebAssembly,
  the browser. It shows blocks on a map with side branches, transactions with their hops as a
  call tree and a graph, accounts, contracts and tokens, the network, the mempool and finality,
  with charts, and signs and sends transactions with chainv4's own code.
- [x] **The node tells how it is doing.** `GET /node` reports peers, roles, sync and counters,
  `GET /mempool` lists what waits, and `GET /stream` pushes blocks, reorgs, finality, queued
  transactions and peers as they happen, for nodes started with `--stream`.
- [x] **The API can serve web pages.** `--cors` lets pages from other origins call it, and
  `--explorer` serves the explorer's web app from the API's own `/`.
- [x] **The explorer lists tokens.** A Tokens page lists the registry by symbol, token pages read
  their name, symbol and decimals from it, and holdings cover its first 100 tokens.
- [ ] **CI does not build the explorer's web bundle.** It lints the wasm32 build but never runs
  `trunk build`, so a broken `index.html` or wasm-bindgen mismatch shows only when someone builds
  it.
- [ ] **Windows builds in CI.** Left out because of RocksDB and Luau build setup.

add encryption to wallets/keypairs when stored to disk (if passphrase was set/selected)
add db migration system (upgrade/downgrade calls maybe?)
maybe have to check the event log not indexing properly in explorer? maybe settings werent properly changed