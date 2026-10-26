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
- [x] **A rotating authority.** `consensus.type: rotating` has the authority finality signers
  produce in turn, one per slot, the next in line stepping in a slot later, in-turn blocks
  weighing 2 and others 1; the producers are the signers fixed for each epoch, so they change by
  approval and leave with an ejection. Light clients learn them from finality's updates and so
  follow `activation_delay` epochs behind it, and snapshot sync is refused on such a chain.
- [ ] **No production by stake.** Stake only finalizes. Producing blocks by stake, or
  Tendermint-style instant finality, which replaces block production, would be a consensus
  scheme of its own.
- [ ] **Snapshot sync on a rotating chain.** It would have to learn the signers as a light client
  does, from finality's updates, before it could check headers; today it is refused there.
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
- [x] **A signer is a key and an account.** Each authority signer names the account that approves
  changes for it and takes its rewards (`Signer`), its own key's address unless genesis or the
  approved set says otherwise, so the voting key signs nothing but votes, as under FFG.
  `approve-signers --add KEY@ACCOUNT` names one.
- [x] **Conflicting authority votes eject the signer.** `Tx::Slash` works under authority
  finality too: the evidence throws the signer out of the decided set, thresholds kept, version
  bumped, and out of the sets already fixed from two epochs on; refused when a set would be left
  unable to reach its thresholds.
- [x] **Depth finality is each node's own, by design.** It is a rolling checkpoint, as Bitcoin
  Cash keeps one: nodes split for longer than `depth` blocks never rejoin on their own, and an
  operator resyncs one side. Kept for development and single-operator chains, and documented as
  such.
- [x] **A slashed validator leaves the sets already fixed.** From two epochs after the slash on
  it is out of every set at once; the slash's epoch and the next keep it, so the next
  checkpoint's light update, which now re-states the sets of every epoch ahead, reaches light
  clients by the usual trust path.
- [x] **An inactivity leak.** FFG genesis's `leak` makes blocks carry every justification, so
  state sees finality move, and bundles of votes that fall short while it has stalled for
  `after_epochs` epochs, so state sees who still votes; each stalled checkpoint then burns
  `permille` thousandths of every quiet validator's bond, reweighing the sets ahead, until the
  rest hold two thirds again.
- [x] **Vote tallies are stored.** Every vote a node counts or holds back is kept in the
  Finality column by target epoch and voter, and counted again when the chain opens, so tallies,
  held votes and double-vote detection survive a restart. Counted votes stay 64 epochs behind
  the finalized checkpoint, to catch one vote surrounding another, and then go.
- [x] **Only voters are remembered.** Double-vote detection compares a vote against its voter's
  counted votes, and, once the vote shows its key votes, against that voter's held ones. Votes
  are remembered only when counted, so throwaway keys can neither grow memory beyond the capped
  held votes nor push real evidence out of the evidence log.
- [x] **Vote rewards can come from fees.** Genesis's `vote_reward_source: fees` sends the burned
  share of every fee to a reward pool and pays voters from it, each as much as the pool holds;
  `mint`, the default, keeps minting them.
- [x] **Checkpoint sync.** `--snapshot --checkpoint 0x…` starts a new node at a finalized
  checkpoint it is told to trust: it takes the checkpoint's headers from a peer by their hashes,
  holds it as justified and finalized, fetches nothing below it, and syncs on from there, so a
  long-range fork under it never reaches the node.
- [x] **Difficulty can retarget over a window.** Genesis's `consensus.window` makes difficulty
  the window's work over the time it took, each interval counting between a quarter and four
  targets, and bounds a block's timestamp by the window's median time, as Bitcoin does. The
  consensus rules take the ancestors they need; the chain, the light client and snapshot sync
  hand them over, and a checkpoint start carries enough headers. Unset, the parent-only rule
  stays and the chain keeps its id; the dev pow chain sets 11.
- [x] **Mining on the GPU.** The `chainv4-gpu` crate mines through wgpu, on Vulkan, Metal and
  WebGPU, with a WGSL kernel for each of SHA-256, BLAKE3, BLAKE2s, BLAKE2b and Keccak-256, tested
  against the CPU hashes; `--mine --mine-gpu` picks it in a build with the `gpu` feature.
- [ ] **No CUDA kernels.** NVIDIA cards mine through Vulkan. Kernels in CUDA C, behind a `cuda`
  feature, would need a machine with nvcc and a card to write and test on.
- [x] **Side branches are pruned, and bad blocks are remembered across restarts.** When the tip
  moves, side branches that split off more than `max_reorg_depth` blocks back, or below the
  finalized checkpoint, are deleted, so the side blocks a node stores are bounded by the reorg
  window, each one sealed at the main chain's difficulty. The last 1,024 blocks that failed to
  run are kept in `Meta`, so a restart refuses their branches without running them again.
- [x] **A block gas limit.** On a chain with fees, a transaction is only taken while its gas
  limit fits what the block has left, and the header's gas used must match, both checked on
  import.
- [x] **Blocks are bounded by bytes on any chain.** Genesis's `max_block_bytes`, checked on
  import, bounds a block's transactions whether or not the chain has fees; the dev chains have
  4 MiB, and genesis files from before it keep their ids. The fixed 1,000-transaction cap is gone:
  producers fill to the bytes and gas limits, or to `--max-block-txs`.
- [x] **Admission off the node loop.** The verifier's gate checks and inserts transactions, so
  block building never starves the mempool.
- [x] **Gas estimation.** `POST /offer` dry-runs calls, deploys and upgrades and offers what they
  used plus a quarter; the CLI, wallet and explorer warn before sending what would fail.
- [x] **A producer can set a time budget.** `--max-build-ms` bounds how long a node runs
  transactions for a block it builds: what went in by then is sealed, and the rest wait for the
  next block. The bytes and gas limits are genesis's, alike on every machine; this and
  `--max-block-txs` are the brakes a producer sets for its own speed.
- [x] **Block rewards.** Genesis sets an initial reward and an optional halving interval; each
  block's producer is credited after its transactions run.
- [x] **Fees.** Every transaction pays for its gas, failed ones too, with what it does not use
  refunded. Genesis picks the gas schedule (`weighted`, or `contract`, where a contract's `score`
  says what is charged) and the pricing (`base_fee`, EIP-1559-style, or `bid`), with the block gas
  limit and the burned share of the base part; the rest goes to the producer. Producers take the
  best-paying transactions first. New schedules and pricings implement `GasSchedule` and
  `Pricing`.
- [x] **Transactions that cannot pay are dropped.** After each block the mempool drops what its
  sender can no longer pay for, and what has offered under the base price for 32 blocks in a
  row; selection stops a sender's run at an offer under the next base price, so the builder never
  runs what it would skip.
- [x] **A scorer runs for the kinds it names.** A `contract` schedule's `kinds` says which kinds
  of transaction call `score`; the rest are charged as weighed, with no call. It still only
  reads; a scorer that keeps counts would need its writes kept.
- [x] **The reward is visible in the API.** Every block summary, block page and stream event
  carries `reward`, what the block minted for its producer; the explorer shows it on the block
  page.
- [x] **Minting is for development chains.** Genesis's `minter` may be the development key or
  nobody, so `Mint` is a dev faucet and a chain of value gets coins from rewards and allocations
  alone.

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
- [x] **Layouts reach keys, kinds and plain storage.** A namespace is a map, any key with values
  of one kind, or a record, only the keys it declares (`--@key NAME KEY KIND`), each with its
  kind; keys retire like namespaces. Kinds are encodings the node checks on every write, and
  code that declares a layout may only read and clear plain storage.
- [x] **Map keys have kinds.** `--@namespace NAME KEYKIND -> KIND` checks every key read, written
  or cleared, so code that changes how it forms keys, `who:hex()` to `who:text()`, fails at once
  instead of starting from empty entries. A map of any key may gain a key kind once.
- [x] **Kinds name their units.** `decimal:wei`: no write can check what a number means, so the
  layout says it, and an upgrade must keep it, or retire and redeclare.
- [x] **Either engine keeps either's state.** Lua has get_ and set_ methods for the binary kinds
  Rust writes, and the SDK for the text kinds Lua writes, so a contract moves between engines by
  upgrade keeping every namespace: `contracts/examples/points` is version 3 of the Lua points.
- [x] **No floats leave a contract.** Lua numbers go out as `i64`, whole and within 2^53, or the
  call fails; `math` keeps only its whole-number functions; the `number` kind holds whole
  numbers in digits.
- [x] **No memory address reaches a result.** `tostring` of a table, function or userdata gives
  its type's name, and failure text has addresses cut, so a script cannot return, store or fail
  with something that differs between nodes.
- [x] **Tables keyed by tables iterate alike on every node.** The Luau fork hashes tables and
  functions by the order they were made, and refuses userdata keys, where an Address is a new
  value each time and two equal ones never meet.
- [ ] **Lua still computes with floats inside a script.** A plain number is a float, so `/` makes
  fractions, which only fail once they would leave. The integer types do exact math for whoever
  uses them, but a number type of the fork's own would close it for everyone.
- [x] **Integer types in Lua.** `U64`, `U128`, `U256`, `I64`, `I128` and `I256`, each overflowing
  at its own width, with Rust's `checked_`, `saturating_` and `wrapping_` methods; arguments and
  the integer kinds arrive as their type. The SDK's `U256` and a new `I256` have the same
  operators and methods, with plain integers on either side.
- [x] **Numbers compare with integers.** The Luau fork asks a userdata's metamethods for a number
  on the other side, and for another userdata type, so `u256(5) == 5` and `u64(1) < u256(2)`.
- [x] **Signed big integers everywhere.** The `i128` and `i256` kinds and an `i256` argument
  type, in the CLI, the JSON API and the explorer.
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

- [x] **A state root and a receipts root.** Each header's `state_root` is the root of a sparse
  Merkle tree over every chain-state entry, kept incrementally in the `StateTree` column, and its
  `receipts_root` a Merkle root over what its transactions did. Blocks with wrong roots are
  refused; `GET /proofs/{column}/{key}` and `GET /proofs/receipts/{id}` serve proofs that
  `StateProof::verify` and `crypto::merkle::verify` check.
- [x] **Snapshot sync.** `--snapshot` checks every header peers have, walks a recent pivot's state
  tree from them a part at a time, checking each part against the pivot's state root and skipping
  what it holds already, and makes the pivot its tip. A pivot that grows too old moves up, and
  the walk fetches only what changed.
- [x] **A light client.** `chainv4::light` follows a chain from its genesis id alone: headers by
  consensus, finality from justifications whose voters it proved from checkpoints' state, and
  values and receipts by their proofs. `chainv4 light` and the explorer run it.
- [ ] **Snapshot nodes cannot serve finality before their pivot.** They never ran the checkpoints
  before it, so they have no light updates for them. Fetching those from peers along with the
  state would let them serve light clients from genesis.
- [x] **Light clients resume.** `LightClient::save` keeps what it checked from the finalized block
  on, at most 4,096 headers, and `restore` takes it back for the same genesis. The wallet saves it
  as it goes.
- [x] **Restored light clients reach older blocks.** `deliver_ancestors` takes the headers below
  the lowest one a client holds, proven by their hash chain, so a receipt in a block older than
  it kept still checks.
- [x] **Light clients start at a checkpoint.** A finalized checkpoint's block id, given, or a
  node's own the first time an app meets it, starts a light client there in one request.
- [ ] **`chainv4 light` still keeps nothing between runs.** It follows from genesis, or from
  `--checkpoint`, each time; it could keep its state in a file the way the wallet does.
- [ ] **Snapshots come from one peer at a time,** and a pivot only stays servable for
  `max_reorg_depth` blocks. Asking several peers at once, and frozen snapshots kept by serving
  nodes, would suit large states.
- [x] **Signing offline.** The wallet installs as an online wallet or an offline signer that makes
  no requests; an online one holds watch-only wallets beside ones with keys. Pairing, requests and
  signed transactions travel as QR codes read off photos, or as text.
- [x] **Messages too long for one code go as parts,** each photographed in turn and put back
  together in any order. Animated codes, read by a live camera, would take them in one go.
- [ ] **The explorer pins genesis on first use.** It should also take a genesis id to trust from
  the person, before it meets a node.
- [ ] **Proofs are against the tip only.** The node keeps one state tree, so it cannot prove what
  an older block's state held; keeping nodes by version, as Jellyfish trees do, would.
- [x] **A Merkle `tx_root`.** It is the root over each signed transaction's leaf, as the receipts
  root is over receipts, and `GET /proofs/receipts/{id}` proves the transaction with its receipt:
  `check_tx` gives who sent what.
- [x] **An address index.** `--index-addresses` keeps every transaction under each address it
  touches, with its own window and column, beside the event index, which shares its catch-up.
  `GET /accounts/{address}/txs`, `chainv4 light history` and the explorer list from it.
- [x] **Token balances are provable.** By the token standard, each holder's balance is in the
  token's `balances` namespace under its address text, so a light client proves it with
  `token_balance_key` and `check_state`.
- [ ] **Contract payouts are not provable line by line.** Coin a contract pays out shows only in
  the trace, which the receipts root leaves out, and the address index does not follow it.
  Committing payments, as events of the chain's own, would let wallets list and prove them.
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
- [x] **Peers given are dialed again.** A `--peer` with no connection is dialed every 15 seconds,
  so a node behind NAT finds a restarted server again, and `--no-mdns` keeps a server off its
  network's multicast. `docker/pow.sh` runs a chain across a VPS and a machine at home.
- [ ] **Peer ids change every start.** A node makes a new libp2p identity each time, so `--peer`
  takes addresses alone and nothing can pin a peer by id. A key kept with the database would let
  a node keep its id.
- [ ] **No connection or request limits.**
- [ ] **No NAT traversal** (relay, hole punching, AutoNAT).

### Node, API and tooling

- [x] **Transactions can be submitted from outside.** `POST /txs` takes `{"tx": "0x…"}`, a
  `SignedTx` encoded as the chain encodes it, and hands it to the node's mempool, which gossips
  it. `chainv4 keygen` and `chainv4 send` make keys and sign and submit transactions.
- [x] **`chainv4 send` waits for inclusion.** It polls `GET /txs/{id}` until enough blocks hold
  the transaction or `--timeout` passes, then prints the result and events.
- [x] **A README for the CLI.** Building, running nodes, keys, sending transactions and the API.
- [x] **Docker.** `docker/compose.yaml` runs a node with every feature, its chain made from
  presets or a genesis of your own, behind nginx with HTTPS from your certificate, Let's Encrypt
  through certbot, or a self-signed one.
- [ ] **No published image.** CI does not build or push the Docker image; it is built where it
  runs.
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
- [x] **A light wallet.** `wallet/`, built with egui, for the desktop and the browser: accounts
  sealed with a password (Argon2id and ChaCha20-Poly1305), balances of the coin and every listed
  token proven at the tip and the final block, sending with a reviewed fee and tip, receiving with
  a QR code, and a history from the address index, each line proven. It pins the genesis on first
  use, saves the light client, and `--wallet` serves it from a node's `/wallet/`.
- [x] **The wallet runs on Android.** `wallet/build-android.sh` builds an APK with xbuild, a
  NativeActivity with no Java, for Android 7 and up. It keeps to the part of the screen Android
  says can be seen, and swaps the sidebar for tabs on narrow screens. CI builds it for arm64 and
  x86-64 and attaches both to `v*` releases, signed with the `ANDROID_SIGNING_PEM` secret.
- [x] **The explorer runs on Android, and on phones.** `explorer/build-android.sh` builds its APK
  the way the wallet's is built, through the shared `ui/build-android.sh`, and CI builds and
  releases it beside the wallet's. Below 640 points it swaps its sidebar for tabs, and its tables
  for stacked rows. Both apps share the `ui` crate: palettes, densities, widgets and Android glue.
- [x] **Copying on Android.** egui reaches no clipboard there, so the `ui` crate hands what is
  copied to Android's ClipboardManager over JNI. Addresses copy with a click or a tap in both apps.
- [x] **Picking files on Android.** A Java activity of the `ui` crate's, compiled into the APK
  after xbuild builds it, opens Android's document picker and hands back what it read: key files in
  both apps, and WebAssembly and Luau files in the explorer.
- [x] **Pasting on Android.** Holding a text field brings a bar with Paste, Select all, Copy and Cut,
  and a Paste button sits over the keyboard, reading Android's clipboard through JNI.
- [ ] **The APK's Java needs the Android SDK's build-tools.** xbuild cannot add a `classes.dex`, so
  the script repacks and signs again with `d8`, `zipalign` and `apksigner`. An xbuild that took a
  dex, or a pure Rust dexer and signer, would make the APK build need a JDK alone, or nothing.
- [ ] **xbuild is pinned to a commit.** It has had no release since 0.2.0; move to one when it
  does. It cannot add a network security config or other resources either.
- [x] **Seed phrases.** BIP39 words with an optional passphrase; each account derives an Ed25519
  and an EdBlake key by SLIP-0010 and a secp256k1 key on Ethereum's path, in the library, `chainv4
  keygen --phrase` and the wallet, which holds several wallets, each with its own words and password.
- [x] **secp256k1, Ethereum's scheme,** behind the `secp256k1` feature: low-s ECDSA over keccak256,
  and Ethereum's 20-byte addresses, in transactions, contracts (33-byte ABI slots) and the wallet.
- [x] **Lua contracts split across files:** `--@include "path"` lines, put together by `chainv4 send
  deploy`, `chainv4 layout` and the explorer before signing, with mistakes placed in their files.
- [x] **Database migrations:** a `Migration` per format from 8 on, up and down in crash-safe chunks;
  a node migrates up as it starts after a checkpoint backup, and `chainv4 db migrate --to N` goes
  back down once confirmed.
- [ ] **No migration exists yet.** The next format change is the first; formats before 8 stay
  unmigratable.
- [ ] **Errors a call raises name the bundle's lines.** Only deploys are placed back in their files;
  `chainv4 send call` could do the same when given the contract's files.
- [x] **Ethereum wallets sign for chainv4.** A transaction's EIP-712 typed form, salted with the
  genesis id, is a signature nodes take: MetaMask and the hardware wallets it drives sign for the
  browser wallet, and `chainv4 send --eip712` signs it with a key of its own.
- [ ] **Contract calls show as bytes in MetaMask.** Only transfers and token transfers have typed
  fields; a call's arguments have no clean EIP-712 form, as it has no unions.
- [ ] **Hardware wallets reach the wallet through MetaMask alone,** and the desktop and Android apps
  reach none. WalletConnect, or USB straight to the device, would.
- [ ] **The wallet keeps its words as plain strings while unlocked.** The vault seals them at rest,
  but in memory they are ordinary `String`s, not zeroized when the wallet locks.
- [ ] **The wallet proves only the registry's first page of tokens,** 100 by symbol, as the
  explorer's holdings do.
- [ ] **CI does not build the web bundles.** It lints the explorer's and the wallet's wasm32 builds
  but never runs `trunk build`, so a broken `index.html` or wasm-bindgen mismatch shows only when
  someone builds them.
- [ ] **Windows builds in CI.** Left out because of RocksDB and Luau build setup.

### Engine script layer

From a review of the Lua layer in `~/engine` (`game/base/src/script`), not chainv4 itself.

- [ ] **Server-sent Lua may run as bytecode.** `script::run_file` checks the network cache first
  and runs anything starting with `\x1bLJ` as bytecode, which LuaJIT does not verify, so a server
  could escape the lockdown. Not reachable yet, because the client runs autorun before any
  `LuaFile` arrives. Load cached files as text; keep binary for the engine's own paks.
- [ ] **`Vector3` and `Angle3` take any cdata or userdata.** Their `FromLua` reads any userdata as
  a 24-byte f64 `Vector3` and any cdata, even `1LL`, as three floats. Check with `ffi.istype`
  through a registry helper.
- [ ] **One failing hook stops the rest.** `hook.call` runs callbacks without `pcall`, in `pairs`
  order, which decides who wins first-non-nil hooks like `Move` at random, and adding a hook
  while its event runs is undefined. Keep insertion order, snapshot the list, `xpcall` each
  callback, and add `hook.remove`.
- [ ] **`run_hook` recovers from errors badly.** It compiles a `"return"` chunk on every error,
  and panics when the return type cannot be built from nil, as `(bool, String, i32)` cannot. Use
  `R::from_lua_multi(MultiValue::new(), lua)` and only allow types that can come back empty.
- [ ] **No runaway script guard.** `while true do end` in a hook freezes the frame, and LuaJIT's
  instruction hooks do not fire inside compiled traces. Needed before third-party addons or
  server-pushed code run.
- [ ] **The lua file cache is never cleared.** `cache::clear()` has no callers, so one server's
  files stay when joining another, and the cache is process-wide statics a listen server shares
  with its client.
- [ ] **Nine raw-pointer scopes.** `ScriptEngine` has nine `*_access` `AtomicPtr`s, each with its
  own scope guard, and `state.rs` builds nine raw pointers per call. One context struct behind one
  scope would also make it easier to check that no native holds `&mut EntityList` while calling
  back into Lua.
- [ ] **`ScriptEngine` repeats its wrappers.** About 15 methods repeat the same `has_ents` check,
  match and `log::error!`; one helper would do.
- [ ] **Stale `.luac` beats fresh `.lua`.** `pick_scripts` prefers compiled files at equal mount
  priority, so an old compiled file wins over source just edited.

add encryption to wallets/keypairs when stored to disk (if passphrase was set/selected)
add db migration system (upgrade/downgrade calls maybe?)
maybe have to check the event log not indexing properly in explorer? maybe settings werent properly changed
add fuzzing
make consensus switchable (pow -> pos, etc)
adjust fuel costs
if we have something like hashable bytes in block header, for light node, we mmight be able to do a macro to auto-error if field was omitted? make fields ignorable, rest has to be used in code

maybe make adjustable?/// come off in a reorg, yet recent enough for peers to serve: they keep undo records for the last
/// max_reorg_depth blocks.
pub(super) const PIVOT_DEPTH: u64 = 16;
/// A chain shorter than this is synced by running its blocks: a snapshot would save nothing.
pub(super) const MIN_SNAPSHOT_HEIGHT: u64 = 2 * PIVOT_DEPTH;

- A light wallet: build chainv4 wallet, combining keys and send with light-client checks of your balance and receipts.

❯ ohhh light client is wallet right

Close, but not quite. A wallet does two jobs, and the light client only does one of them:

1. Keys and sending: holding a key, signing transactions and submitting them. In chainv4 that's chainv4 keygen and chainv4 send, and the explorer's Send page. The light client holds no keys and sends nothing.
2. Knowing what's true: your balance, and whether a payment landed and is final. Most wallets just ask a node and trust the answer. The light client is a way to do this part without trusting the node: it checks headers, finality and proofs itself.

So a light client is what a trustless wallet uses for reading, like old SPV Bitcoin wallets or Helios under an Ethereum wallet. The explorer is now the closest thing to a light wallet here: it signs and sends on its Send page, and its account pages show balances the light client proved.

That also matters for what you asked earlier. There's no separate light process an explorer can point at, because each explorer runs its own light client against the node it's connected to.



these hardcoded?
   Ask::Receipt(id) => format!("/proofs/receipts/{id}"),
            Ask::Tokens => "/tokens?limit=100".to_string(),
            Ask::History(address) => format!("/accounts/{address}/txs?limit=50"),
            Ask::Events(address) => format!("/events?address={address}&limit=50")

maybe add ability to use u128 for amounts if not too mcuh effort
evm

tighten lua sandbox
make sure contract addresses start with contract:, then limit which keys can call into it potentailly
add a thing to storage (At least lua), where we dont have to save addresses as text but can just save it directly
make a native ens type thing, with fees/params that are from genesis or voted on i guess
make sure max supply is configurable
storage seems to save strings only, make it save raw types to save space, etc
add voting for proposals, like consensus change, etc, upgrades, what not
scan whole project in order -> scan docs -> go section by section and verify theres nothing that shouldnt be there/stale -> go through each subsytem nad find bugs, stuff that would be good to change -> go through each subsystems tests (all of them), and see if theres something that doesnt make sense/should be added or removed.
find more hardcoded values that maybe should be confiruable and like be able to be changed, voted on, yada yada