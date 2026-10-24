# Good to know

The things that surprise people, and the rules that keep every node on a chain in step. The
other pages explain each part in full; this one is the short list to keep in mind.

## Running a chain

- **Old `chain.db` files are refused.** Formats change as features land, and nothing migrates
  them. Delete the database, or point `--db` elsewhere. A node stops with:
  - `WrongGenesis` on a database written for another genesis. Switching `--consensus`,
    `--genesis` or build features does this, and so did genesis gaining its coin;
  - `OldFormat` on a database kept in another storage format (`STORAGE_FORMAT`), such as every
    database from before receipts carried traces.
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
- **A node only streams when asked.** `--stream` turns on `GET /stream`. Without it, clients
  such as the explorer poll.
- **Read-only calls hold up the API.** `POST /call` runs a contract under the chain's fuel cap on
  the API's one thread, and every other request waits for it.
- **The explorer signs where it runs.** Its key stays in the window, desktop or browser, and only
  signed transactions go to the node. Serve the web app from the node itself with
  `--explorer explorer/dist`, or allow the page's origin with `--cors`. See [Explorer](explorer.md).

## Consensus-critical: changing these makes a new chain

Every node on a chain must agree on all of these. Change one on purpose, never by accident.

- **Engines.** Every node must be built with the engines its genesis lists. A node missing one
  refuses to open the chain rather than fall out of step.
- **wasmi is pinned to `=2.0.0`.** Its version, enabled features and stack limits decide what a
  module does and costs. Upgrade on purpose, never with `cargo update`.
- **The Luau fork.** The germ3n forks of mlua and Luau meter fuel. They are pinned by branch, with
  the commit in `Cargo.lock`; `cargo update` would move them, so pinning a `rev` would be safer.
  Never swap them for upstream.
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
  failed ones included, up to 256 hops. It is not consensus: nothing hashes it.
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
- **Lua annotations are read line by line.** A `--@reentrant` line inside a multi-line string
  counts too.
- **Lua numbers reach WebAssembly contracts as float bits.** Floats are refused in modules, so the
  SDK's `Value::as_i64` and `Value::as_amount` turn whole numbers into integers using integer
  instructions only.
- **Each call in a chain of calls gets its own memory limit.** The worst case is 8 nested calls ×
  32 MiB in one transaction on the dev chains.
- **The WebAssembly `call` import takes a value pointer:**
  `call(address, value, name, name_len, args, args_len)`. Rebuild contracts made against the
  older five-argument form. Contracts can also import `emit` and `delegate`.
- **Every call gets a fresh engine state, on purpose.** Reusing one would let top-level locals
  survive between calls and make memory limits depend on what a node ran before, which could split
  the chain. Instead, the Lua engine keeps fresh states ready and the WebAssembly engine caches
  compiled modules. Both only change speed, never results.
- **Delegate only to code you trust.** It runs with this contract's storage and balance.
- **Loading code is not charged.** Compiling and validating are bounded only by the 128 KiB source
  and 512 KiB module limits.

## Building and testing

- **The end-to-end SDK test needs the wasm32 target.** It builds `contracts/` with cargo:
  `rustup target add wasm32-unknown-unknown`. CI installs it.
- **Contracts build from their own workspace.** `contracts/.cargo/config.toml` sets the wasm32
  target, but only when cargo runs inside `contracts/`. From the repo root, pass
  `--target wasm32-unknown-unknown`. The SDK's own tests run on the host target.
- **Clippy runs with `-D warnings`, on every feature subset CI tests.** A build that lacks a
  feature has to compile cleanly too: cfg-gate what only one kind uses. See
  [Development](development.md).
