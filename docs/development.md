# Development

## Building and testing

```sh
cargo build --release
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

You need stable Rust, a C/C++ toolchain and clang: RocksDB and Luau are built from source.

**Building:** the build also writes the wiki, `target/wiki/index.html`, from every
`#[document]` in `src` (`build.rs`, the `wiki` and `macro` crates).

**The contracts workspace,** `contracts/`, holds the Rust SDK and the example contracts:
- it builds for `wasm32-unknown-unknown`, through its own `.cargo/config.toml`;
- the end-to-end SDK test builds it from the main crate, so it needs
  `rustup target add wasm32-unknown-unknown`;
- the SDK's own tests run on the host: `cargo test -p chainv4-sdk --target <host>` inside
  `contracts/`.

## Features

Every piece sits behind a cargo feature, and everything is on by default.

| Features | What they are |
| --- | --- |
| `sha256`, `blake3`, `blake2s`, `blake2b`, `keccak256`, `xxh3` | Hash algorithms. |
| `ed25519`, `edblake` | Key schemes. |
| `lvm`, `wasm` | The contract engines; either pulls in `vm`. |
| `rocksdb` | The RocksDB backend. |
| `node` | The node, the chain and the CLI. |
| `http` | The HTTP API. |
| `client` | `chainv4 send`. |
| `finality-ffg`, `finality-authority`, `finality-depth` | Kinds of finality. |

**What a build must have:** contracts and nodes need at least one secure hash, or they fail to
build.

**Builds without some features:** a build without a kind of finality, or an engine, makes
different dev chains, which puts it on a different network.

**CI** (`.github/workflows/test.yml`):
- it runs clippy and the tests on the workspace;
- it runs clippy on the contracts and the SDK's tests;
- it runs clippy and the tests on a matrix of feature subsets.

The matrix:
- covers each hash and key scheme on its own;
- covers builds without BLAKE3;
- covers builds with each kind of finality alone, and with none.

A new feature belongs in the matrix too.

## Conventions

These are the rules the code follows. Keep to them when adding to it.

- **Owned state, never globals.** Long-lived machinery has an owner in the `Node` → `Chain` →
  `Host` chain and is passed down: engines, linkers, registries, finality, fees, the chain's
  rayon workers. Nothing lives in
  a `static` or `OnceLock`, so tests can build differently configured nodes in one process.
- **Every algorithm behind a feature, on by default.**
  - Each hash, key scheme and kind of finality is gated: the variant and every match arm.
  - Its enum uses `#[wincode(tag_encoding = "u8")]`, with an explicit `#[wincode(tag = N)]` on
    every variant.
  - A new variant takes the next unused tag.
  - It is tested in a build with that feature alone.
- **Nothing hardcoded that a chain might want to change.** Parameters live in genesis, and genesis
  files must name all of them. A new parameter is a new genesis field, with a dev default and
  JSON support in `src/cli/genesis.rs`.
- **Traits where kinds can differ.** Consensus, finality, gas schedules and pricings are all
  traits picked by genesis. A new kind is a new implementation, not a new branch in the chain.
- **Consensus is explicit.** Anything that changes what a block does or costs is consensus:
  encodings, tags, engine settings, fuel costs, hash and key schemes, limits. Changing it means a
  new chain.
- **Errors say who is at fault.**
  - A database failure is the node's fault: it stops the import, and never becomes a
    transaction's failure.
  - A transaction that cannot run makes its block invalid.
  - A transaction that runs and fails stays in its block.
- **Lua libraries** are one file per library under `src/vm/lvm/libs/`, each registering one
  global table, documented with `#[document]` pages of `kind = "library"`.
- **mlua and Luau are forks.** chainv4 uses the germ3n forks of mlua and Luau, which add fuel
  metering. Never swap them for upstream.

## Documenting code

Public items carry `#[document(...)]`, which the wiki is built from. The attribute is checked at
compile time, so a broken page fails the build.

**Common keys:**
- `summary`, `category` and `parent`;
- `params`, `returns` and `panics`;
- `note`, `example` and `see_also`;
- `kind` and `name` for Lua and WebAssembly pages.

**Fields and variants:** they can carry `#[document]` too; the macro strips them after reading.

**Prose:** write it in plain sentences that say what a thing does and why, the way the existing
pages do.

## Tests

- Each module keeps its tests next to it, as `tests.rs` or an inline `mod tests`, and tests read
  as sentences: `blocks_hold_no_more_than_their_gas_limit`.
- Chain tests build blocks with `Chain::build`, seal them with the dev authority, and import them.
- To test a block that breaks the rules, assemble it by hand, since building leaves invalid
  transactions out.
- Tests that are not about fees use chains with `fees: None`, so balances only move as the test
  says.
- Node tests drive `Node::step` with their own clock and a `FakeNet` instead of libp2p.
- CLI tests run a real node and API on a thread.
- Tests for one kind of finality, engine or scheme are gated on its feature, so every row of the
  feature matrix passes.
