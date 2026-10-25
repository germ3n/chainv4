# Good to know

The things that surprise people, and the rules that keep every node on a chain in step. The
other pages explain each part in full; this one is the short list to keep in mind.

## Running a chain

- **Old `chain.db` files are migrated from format 8 on.** A node moves an older database up to its
  own format as it starts, after copying it aside as `chain.db.format-N`; going back is
  `chainv4 db migrate --to N`, which asks first (see [Migrations](storage.md#migrations)).
  Databases from before format 8 are still refused: delete them, or point `--db` elsewhere. A
  node stops with:
  - `WrongGenesis` on a database written for another genesis. Switching `--consensus`,
    `--genesis` or build features does this, and so did genesis gaining its coin and its
    `token_functions`;
  - `OldFormat` on a database kept in a storage format (`STORAGE_FORMAT`) no migration reaches,
    such as every database from before format 8, or one in a newer format;
  - `Migrating` on one a migration stopped part way through, which `chainv4 db migrate`, or
    starting the node, finishes.
- **There are two dev chains, and pow is the default.**
  - `--consensus pow`, mined with `--mine`;
  - `--consensus authority`, produced with `--produce` on exactly one node.

  Each is its own genesis and network.
- **The development key is public.** Its seed is in the source. It holds a million coins on both
  dev chains, may mint, produces the authority chain, is the dev chains' one FFG validator, and
  signs for `chainv4 send` without `--key`. Never put anything of value on a dev chain.
- **The dev genesis depends on the build.** The dev chains allow every engine the node was built
  with, hash with BLAKE3 when it is built in, and run FFG only when it is built in. Nodes built
  with different features are on different dev chains and never meet.
- **The dev chains charge fees.** A base fee starts at 1 gwei of gas, with half of the base part
  burned. An account needs coins to send anything, the development key included. `chainv4 send`
  works out the offer from `GET /status`. `chainv4 genesis --fees free` writes a chain without
  fees.
- **Nothing finalizes until someone validates.** Both dev chains run FFG with the development key
  as the one validator, epochs of 8 blocks. Without a node running `--validate`, the chain runs
  as if it had no finality.
- **Every chain parameter is in genesis.** That covers:
  - consensus and its timing;
  - hashes, rewards, allocations, the minter and engines;
  - transaction limits and the reorg depth;
  - finality and fees.

  `chainv4 genesis` writes the dev defaults, and `--genesis FILE` runs a file. Files must name
  every field and may not add any. See [Genesis](genesis.md).
- **A voting key belongs on one node.** What a node has voted is kept in its own database. Two
  nodes voting with one key can sign conflicting votes, and under FFG get it slashed.
- **The binary is also the client.** `chainv4 send` waits until its transaction is in a block
  (`--timeout`, `--confirmations`, `--final`, `--no-wait`). It exits non-zero when the
  transaction landed but failed.
- **The node's peer id changes on every start.** Its libp2p key is generated fresh each run.
- **The chain keeps a registry of tokens.** A contract that exports genesis's `token_functions`,
  ERC-20's six on the dev chains, is registered as it deploys, with its name, symbol and decimals
  if it exports them. `GET /tokens` lists them by symbol.
  - **Bad answers fail the deploy:** a token whose `name()`, `symbol()` or `decimals()` fails or
    gives something out of bounds does not deploy.
  - **Symbols are not unique:** trust an address, not a symbol.
  - **A token that changes its name asks for a refresh:** `tokens.refresh()` in Lua,
    `refresh_token()` in Rust. Without one, the registry keeps the old answers.
- **Rust exports take typed parameters.** `#[export] fn transfer(to: Address, amount: U256) ->
  bool` reads the call's arguments into its parameters and refuses a call whose arguments do not
  fit, naming the parameter. A function that takes and returns nothing still reads `input()`
  itself.
- **Events are searchable on nodes that index them.** `--index-events` indexes every event by its
  contract, its name and the addresses in it, for `GET /events`.
  - **Which blocks:** `--index-events` alone keeps every block's events. `=5000` keeps them from a
    height, and `=latest-100000` keeps a window that moves with the tip.
  - **Older blocks:** the node reads them back from their receipts in the background, newest
    first. `indexed_from` says how far it got.
  - **Staying on:** once on, the database keeps it on, keeping the same blocks until the flag
    says otherwise. `--index-events=off` deletes it.
- **Histories come from the address index.** `--index-addresses` keeps every transaction under
  each address it touches: its sender, a transfer's parties, the contract called and every
  address among the call's arguments, and a deploy's contract. It takes the same `FROM` and
  `=off` as `--index-events`, on its own: either can be on without the other. A wallet's
  activity, `chainv4 light history` and the explorer's account page list from it. Coin a contract
  pays out shows only in the trace, so not here.
- **Tokens keep balances where light clients can prove them.** The token standard puts each
  holder's balance in the `balances` namespace under the holder's address text. A token that
  keeps them elsewhere still works, but no wallet can prove what it holds.
- **Floods are turned away at the door.** Gossiped transactions and votes, and submissions, count
  against a backlog: 10,000 in all and 1,000 from any one peer by default
  (`--max-gossip-backlog`, `--max-gossip-per-peer`). Past it they are dropped, `POST /txs`
  answers 503, and a peer over its share 10 times in a minute is disconnected. It can come back
  at once: there are no bans yet (see [Node](node.md#the-gossip-backlog)).
- **A node only streams when asked.** `--stream` turns on `GET /stream`. Without it, clients
  such as the explorer poll.
- **Read-only calls hold up the API.** `POST /call` runs a contract under the chain's fuel cap on
  the API's one thread, and every other request waits for it.
- **Lua contracts can be split across files, put together when deploying.** A `--@include
  "lib.lua"` line is replaced by that file, sharing the includer's scope, before the transaction
  is signed: the chain holds one source, and the 128 KiB limit is the whole one's. `send` checks
  it compiles first and names any mistake by file and line. See
  [Contracts](contracts.md#splitting-a-contract-across-files).
- **The explorer signs where it runs.** Its key stays in the window, desktop, browser or phone, and only
  signed transactions go to the node. Serve the web app from the node itself with
  `--explorer explorer/dist`, or allow the page's origin with `--cors`. See [Explorer](explorer.md).
- **The wallet keeps its keys sealed on the device.** A password seals them, and nothing opens
  them without it. Its accounts come from 12 seed-phrase words, which bring them all back on any
  device: write them down. In the browser they live in the page origin's storage, which clearing
  the site's data deletes. It trusts the genesis a node first shows it, and proves everything
  else. `--wallet wallet/dist` serves it from the node's `/wallet/`. See [Wallet](wallet.md).
- **One account, three addresses.** A seed-phrase account has an Ed25519, an EdBlake and an
  Ethereum (secp256k1) address, each with its own balance and nonce. Coins at one are not at the
  others: the wallet adds them up on Home, but a transaction is sent from one address.
- **secp256k1 addresses are Ethereum's.** `secp256k1:0x` and 20 bytes, the same as Ethereum's for
  the same key, and the same words give MetaMask's accounts. But chainv4 signs its own
  transactions, so an Ethereum wallet can hold the key yet not sign for chainv4, and a
  secp256k1 address cannot be checked as a curve point: a mistyped one takes coins for good.
  See [Crypto](crypto.md#keys-and-signatures).
- **The Android apps are built with xbuild, not Gradle.** `wallet/build-android.sh` and
  `explorer/build-android.sh`, both through `ui/build-android.sh`, need clang, lld and llvm-ar, and
  no NDK. Without a key they sign with xbuild's debug key, the same for everyone: Android updates
  an app only with one signed by the same key, so pick your own key (`ANDROID_SIGNING_PEM` in CI)
  before anyone installs them. They need Android 7 or newer.
- **Picking files on Android takes Java.** Android hands a picked file only to Java code, so the
  build compiles a small activity, `ui/android/Picker.java`, into the APK with a JDK and Android's
  build-tools, and signs the APK again with the same key. Without them it builds an app that cannot
  pick files, and leaves its buttons out; pasting needs no Java and always works.
- **The wallet and the explorer share the `ui` crate, not a look.** `ui/` holds the palette,
  density and type machinery, the widgets and the Android glue; each app picks its own colors,
  fonts and density. A change to a shared widget shows in both. See
  [The ui crate](development.md#the-ui-crate).
- **Docker puts HTTPS in front, not around peer-to-peer.** nginx terminates TLS for the API,
  the explorer and the wallet; QUIC is TLS 1.3 already, so the p2p port goes straight to the node.
  A chain made from the presets signs with the development key until `docker/config` holds keys
  of your own. See [Docker](docker.md).
- **A restored light client takes old headers on demand.** It keeps headers from the finalized
  block on; to check something older, `deliver_ancestors` takes the headers between, proven by
  their hash chain to the lowest it holds.

## Consensus-critical: changing these makes a new chain

Every node on a chain must agree on all of these. Change one on purpose, never by accident.

- **Engines.** Every node must be built with the engines its genesis lists. A node missing one
  refuses to open the chain rather than fall out of step.
- **wasmi is pinned to `=2.0.0`.** Its version, enabled features and stack limits decide what a
  module does and costs. Upgrade on purpose, never with `cargo update`.
- **The Luau fork.** The germ3n forks of mlua and Luau meter fuel. They are pinned by branch, with
  the commit in `Cargo.lock`; `cargo update` would move them, so pinning a `rev` would be safer.
  Never swap them for upstream. Its comparisons are consensus too: a number against a userdata,
  and two userdata of different types, ask the userdata's metamethods, where plain Luau would
  fail or call them unequal (see below).
- **Luau compiler settings.** Optimization level 1, debug level 1, no type information. The
  bytecode, and so every call's fuel, depends on them.
- **Fuel costs** (`vm::cost`) and every limit: call depth, key and value sizes, event sizes,
  module and source sizes.
- **Encodings and tags.** Everything is encoded with wincode, every enum's tags are pinned, and a
  new variant takes the next unused tag. Never reuse or renumber one.
- **Byte order.** Everything is little-endian, with three exceptions on purpose:
  - the work hash is read as a big-endian number;
  - heights and epochs in database keys are big-endian, so they sort;
  - XXH3 digests are big-endian as bytes, matching `xxhsum`.
- **Hashes are chosen in genesis.**
  - `Genesis.hash` is the chain hash: block and transaction ids, roots, contract addresses and
    gossip ids.
  - The proof-of-work config names its own work hash.
  - Both must be secure: sha256, blake3, blake2s, blake2b or keccak256. XXH3 only names contract
    functions.
  - A header hashes itself with its parent's algorithm, so ids from chains with different hashes
    never connect.
- **Kinds of finality, scorers and pricings.** A node built without the kind of finality its
  genesis names refuses the genesis file.
- **The state root's tree, and what a receipt commits.** Paths, leaf and branch hashes, and which
  columns count (`STATE_COLUMNS`) decide every `state_root`; `CommittedReceipt`'s fields and the
  RFC 6962 split decide every `receipts_root`, and each `SignedTx`'s encoding and the same split
  every `tx_root`. A new chain-state column must join `STATE_COLUMNS` on purpose, and so becomes a
  new chain. The address index's column does not: it is a node's own.

## How the chain behaves

- **Failed transactions stay in their block.** They use up their nonce and pay their fee. Only a
  transaction that cannot run at all makes a block invalid: a wrong nonce, a bad signature, or an
  offer that cannot pay.
- **Producers run what they build.** `Chain::build` runs the transactions it is given and keeps
  those that can run and fit the block gas limit. A producer runs each block twice.
- **Blocks run on every core, with the result one by one gives.** A transaction waits for the
  earlier ones that name what it names, such as its sender or contract. Transactions then run side
  by side in rounds on what the block holds so far, and go in in order. One that read what an
  earlier one wrote all the same runs again. Execution must therefore stay deterministic and read
  state only through `State`: a read that went around it would escape the conflict check.
- **Receipts keep a trace.** Every call, delegate and transfer a transaction's contracts made,
  failed ones included, up to 256 hops. It is not consensus: the receipts root leaves it out, and
  a failure's message too.
- **Headers commit to state.** Each `state_root` is the root of a sparse Merkle tree over every
  chain-state entry, and each `receipts_root` a Merkle root over what the block's transactions
  did. A block whose roots are wrong is refused (`StateRoot`, `ReceiptsRoot`), after it runs.
  `GET /proofs/{column}/{key}` and `GET /proofs/receipts/{id}` serve proofs against the tip, and
  `StateProof::verify` and `crypto::merkle::verify` check them. See
  [Chain](chain.md#the-state-root).
- **Storage format 8.** `tx_root` became a Merkle root over the block's transactions, so every
  block with transactions has a new id, and the address index has a column of its own: delete old
  databases, or point `--db` elsewhere. Genesis ids stay as they were, since genesis's `tx_root`
  is the hash of its configuration.
- **A receipt proof proves the transaction too.** `GET /proofs/receipts/{id}` carries the signed
  transaction and its path in `tx_root`, so a light client knows who sent what (`check_tx`), not
  only what it did.
- **Genesis commits to the light client's parameters.** Its `proof_root` is the hash of
  `light_params()`: the chain hash, consensus and finality's timing. Changing any of them is a new
  chain, as changing genesis always is.
- **Snapshot nodes keep headers only before their pivot.** `--snapshot` checks every header and
  takes the state of a block 16 below the best tip, then runs the blocks after it. Before the
  pivot it has no bodies, receipts, events or light updates, and it never rolls back past it.
  Peers can serve state only at their last `max_reorg_depth` blocks, so a pivot that grows too
  old moves up, and the walk fetches only what changed. See
  [Node](node.md#syncing-from-a-snapshot).
- **Every dev chain has the same genesis id.** A wallet that followed one resumes against the next
  as if it were the same chain, and waits for blocks past where the old one got to: Settings →
  Light client → **Start over**. See [Light client](light-client.md#what-to-keep-in-mind).
- **Light clients trust only a genesis id.** `chainv4 light`, and the explorer, check every
  header by consensus, every justification against voters proven from state, and every value
  against a header they checked. Voters are trusted from a checkpoint's state once that
  checkpoint is justified. See [Light client](light-client.md).
- **Coin decimals are for display only.** Genesis names the coin's symbol and decimals, but the
  chain counts every balance, fee and reward in whole units.
- **Rewards have no coinbase transaction.** The chain credits the producer after a block's
  transactions run, and a reorg takes it back. A reward can be spent from the next block on.
- **Reorgs reach back at most `max_reorg_depth` blocks**, 100 on the dev chains. Finality bounds
  them further: nothing at or below the finalized checkpoint comes off, and work alone only moves
  the chain to branches that hold the justified checkpoint.
- **Genesis bonds are extra.** Genesis validators' bonds are minted on top of the allocations.
- **Accounts at the default are deleted.** An address with no balance and nonce 0 is not stored.
- **Running out of fuel costs all the fuel allowed.** On a chain with fees, that is the whole gas
  limit when gas bound it.
- **Calls reserve their whole fuel cap.** `send` sizes a call's gas limit from `tx_limits.fuel`,
  since nothing estimates gas. The sender must hold that reserve, though the unused part comes
  back.

## Contracts

- **Four functions are special:**
  - `init`, run at deploy;
  - `migrate`, run after an upgrade;
  - `check_perms`, asked before an upgrade;
  - `fallback`, which answers functions a contract does not export.

  Transactions and other contracts cannot call `init`, `migrate` or `fallback`, nor delegate to
  them. A contract without `check_perms` can never be upgraded. See [Contracts](contracts.md).
- **Reentrancy is refused by default.** A function only runs while its contract is already
  running further up the chain of calls if it is marked:
  - `--@reentrant` on the line right above `function M.name(` in Lua;
  - `#[export(reentrant)]` in Rust.
- **Lua annotations are read line by line.** A `--@reentrant`, `--@namespace` or `--@retired`
  line inside a multi-line string counts too.
- **Numbers leave a contract whole.** Luau computes with floats inside a script, but every number
  a contract returns, passes, emits or lists goes out as an `i64`, whole and within 2^53, or the
  call fails; a `number` argument must be whole to come in. `math` keeps only `abs`, `ceil`,
  `clamp`, `floor`, `max`, `min`, `round` and `sign`. Floats are refused in WebAssembly outright.
  This changed what Lua contracts return, `number` to `i64`, so chains from before it differ.
- **Scripts never see a memory address.** `tostring({})` is `table`, not `table: 0x…`, and
  `error({})` fails with `table`: an address differs from node to node, and a script that
  returned or stored one would split the chain.
- **Tables keyed by tables iterate the same everywhere; userdata is no key at all.** Plain Luau
  hashes such keys by address, so `pairs` would differ between nodes; the Luau fork hashes tables
  and functions by the order a script made them instead. An Address or u256 as a key fails, "table
  index is a userdata; use a string or a number", as each is a new value and two equal ones would
  be two keys: key by `who:text()` or `tostring(n)`.
- **The `number` kind holds whole numbers, spelled one way.** Digits, within 2^53: write it with
  `ns:set_number` in Lua, not `tostring`, which spells 10^15 as `1e+15`.
- **Each call in a chain of calls gets its own memory limit.** The worst case is 8 nested calls ×
  32 MiB in one transaction on the dev chains.
- **The WebAssembly `call` import takes a value pointer:**
  `call(address, value, name, name_len, args, args_len)`. Rebuild contracts made against the
  older five-argument form. Contracts can also import `emit` and `delegate`.
- **Every call gets a fresh engine state, on purpose.** Reusing one would let top-level locals
  survive between calls and make memory limits depend on what a node ran before, which could split
  the chain. Instead, the Lua engine keeps fresh states ready and the WebAssembly engine caches
  compiled modules. Both only change speed, never results.
- **Lua changes few value types; values lists none.** Integers keep their type through Lua, but an
  amount comes back a `u256`, a `number` an `i64`, and bytes that read as text a string. To pass
  values on as they came, forward `ctx.args`, as the Lua proxy does: calls given a values list
  return one, untouched.
- **Integer arguments arrive in Lua as their own types.** A `u64` is a `U64`, an `i64` an `I64`, up
  to `I256`, and `ns:get_u64` and the like read back the same way. They used to arrive as a `U256`,
  or a plain number for the signed ones, so a contract that did `for i = 1, n` with an `i64`
  argument now needs `n:tonumber()`, or a `number` argument; and chains from before differ.
- **Signed and unsigned integers do not mix.** `u128(5) + i128(1)` fails, "convert one side",
  as no type holds both `U256` and `I256`. One signedness widens, `u64 + u256` to a `U256`, and a
  plain number takes the integer's type. Comparisons mix freely, by the true value.
- **`/` and `%` round as Rust's do; `//` as Lua's.** For the signed types `i64(-7) / 2` is `-3`
  and `i64(-7) % 2` is `-1`, as in Rust and the SDK's `I256`, so the two engines agree; `//` rounds
  down, `-4`. Plain numbers keep Lua's arithmetic.
- **Numbers compare with integers, in the Luau fork.** `u256(5) == 5`, `x > 1000` and
  `table.sort` over a mix all ask the integer's metamethods, exactly: past 2^53, with fractions,
  and against NaN, which orders against nothing. Plain Luau never calls `__eq` across types and
  fails to order a number against a userdata. A table against a number still behaves as in plain
  Luau.
- **Delegate only to code you trust.** It runs with this contract's storage and balance, all but
  the contract's own space, which it can only read.
- **Storage has namespaces and an own space.** `storage.namespace(name)` and `storage.own`
  (`storage::namespace`, `storage::own()` in Rust) have keys of their own, apart from plain
  storage. Code a contract delegates to can read its own space but never write it, so the
  example proxies keep their owner and implementation there. See
  [Contracts](contracts.md#storage-namespaces).
- **Declared layouts are enforced.** Code with `--@namespace` and `--@key` lines, or the SDK's
  `namespace!`, reaches only what it declares: its namespaces, a record's keys, values of their
  kind, and plain storage only to read and clear. New code must keep every namespace and key with
  the same kind, or `--@retired` it: `Tx::Upgrade` refuses it otherwise, and the example proxies
  check with `check_layout` before repointing. Retired things stay retired, so rolling back to
  code from before a retirement is refused too. `chainv4 layout new.lua --from old.lua` checks
  before you send. See [Contracts](contracts.md#storage-layouts).
- **Kinds are encodings; units are their meaning.** A value written to a declared key must be its
  kind's encoding. Lua writes text (`decimal`, `address-text`) and Rust bytes (`u256`,
  `address`), but each engine has methods for the other's, so a Lua version and a Rust version
  share any namespace. A unit, `decimal:wei`, is the layout's word for what a number counts: the
  node keeps it across versions, but cannot check it.
- **A map's key kind is checked on reads too.** Code that forms a key another way than the map
  declares, `who:hex()` for `who:text()`, fails at once instead of reading an empty entry. Key
  kinds and units may be added to a deployed layout once, then never changed.
- **Loading code is charged.** A deploy or upgrade pays 1 fuel per byte of code to compile it,
  plus whatever a Lua script's top level burns, even when it fails. Every call pays for the top
  level again, since each runs it fresh, before the call is attached: a top level sees no `ctx`
  or `storage`.

## Building and testing

- **The end-to-end SDK test needs the wasm32 target.** It builds `contracts/` with cargo:
  `rustup target add wasm32-unknown-unknown`. CI installs it.
- **Contracts build from their own workspace.** `contracts/.cargo/config.toml` sets the wasm32
  target, but only when cargo runs inside `contracts/`. From the repo root, pass
  `--target wasm32-unknown-unknown`. The SDK's own tests run on the host target.
- **Clippy runs with `-D warnings`, on every feature subset CI tests.** A build that lacks a
  feature has to compile cleanly too: cfg-gate what only one kind uses. See
  [Development](development.md).
