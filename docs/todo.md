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

- **Sync.** Nodes answer `Status` and `Blocks` requests but never send them, so a node that
  starts late or misses a block stalls at "waiting for sync".
- **State root.** Block headers do not commit to state yet. Needed for snapshot sync, light
  clients, and any later ZK work.
- **Value in contract-to-contract calls.** Calls between contracts send nothing; `ctx.value` and
  the `value` host function read zero.
- **WASM module cache.** Modules are parsed and compiled again on every call. Caching does not
  affect consensus, only speed.
- **Typed exports in the SDK.** `#[export]` functions take and return nothing and use
  `input()`/`output()`; decoding arguments into Rust parameters would be nicer.
- **Transaction lookup in the HTTP API.** There is no index from transaction id to block.
- **Windows builds in CI.** Left out because of RocksDB and Luau build setup.
