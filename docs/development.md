# Development

## Building and testing

```sh
cargo build --release
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

You need stable Rust, a C/C++ toolchain and clang: RocksDB and Luau are built from source.

**Building:** the build also writes the wiki, `target/wiki/index.html`, from every
`#[document]` in `src` and the guides in `docs/` and the README (`build.rs`, the `wiki` and
`macro` crates); `cargo run -p wiki` writes it alone. It is one self-contained page: the guides,
then the Rust, Lua and WebAssembly references, with search. A guide's link to another guide or a
heading in one becomes a wiki address, and one that leads nowhere fails the build. A link to
another file of the repository shows as its path, since the repository is private; add a guide to
`SECTIONS` in `wiki/src/guides.rs` to place it in the sidebar, or it goes under More.

**Third-party notices:** `python3 gen_notices.py` writes `THIRD_PARTY_NOTICES.txt` from the
Cargo.lock files of the node, the explorer and the wallet: chainv4's own MIT license, the license
texts and copyright lines of every crate, and the C/C++ and fonts they bundle (Luau, RocksDB and
its compression libraries, Android GameActivity, the apps' and the wiki's fonts). Run it after
changing dependencies, and ship the file with any binary. It stops, naming what to update, when a
crate or file it expects is gone.

**The contracts workspace,** `contracts/`, holds the Rust SDK and the example contracts:
- it builds for `wasm32-unknown-unknown`, through its own `.cargo/config.toml`;
- the end-to-end SDK test builds it from the main crate, so it needs
  `rustup target add wasm32-unknown-unknown`;
- the SDK's own tests run on the host: `cargo test -p chainv4-sdk --target <host>` inside
  `contracts/`.

**Tests that open sockets** are ignored by default: `cargo test -- --ignored net::` floods one
real libp2p node from another on 127.0.0.1 and checks the flooder is turned away, then
disconnected.

**The explorer workspace,** `explorer/`, is the egui explorer (see [Explorer](explorer.md)):
- it has a lockfile of its own, so egui and the browser crates never meet the node's, whose
  libp2p pins older web crates than egui needs;
- it links the chainv4 library with only the hash and key features, which build for
  `wasm32-unknown-unknown` too;
- inside `explorer/`: `cargo test`, `cargo clippy --all-targets -- -D warnings`, and
  `cargo clippy --target wasm32-unknown-unknown -- -D warnings` for the web build;
- `trunk build --release` builds the web app into `explorer/dist`.

**The wallet workspace,** `wallet/`, is the light wallet (see [Wallet](wallet.md)), built the same
way: a lockfile of its own, the chainv4 library with only the hash and key features, and inside
`wallet/` the same three commands. `trunk build --release` builds its web app into `wallet/dist`,
which a node serves from `/wallet/` with `--wallet`, and `wallet/build-android.sh` its Android app
(see [Wallet](wallet.md#android)). The explorer builds for Android the same way, with
`explorer/build-android.sh` (see [Explorer](explorer.md#android)).

## The ui crate

`ui/` is what the wallet and the explorer share, a workspace of its own like theirs, which both
take by path. They share one look, with the crate holding all of it, so a change here shows in both:
- **`theme`:** the `DARK` and `LIGHT` palettes, the fonts in `ui/assets/fonts` with their licences
  (`install_fonts`), and a `Density` for spacing, card margins and text sizes, which is all an app
  picks: the wallet is `COMFORTABLE`, the explorer `COMPACT`, tighter but with the same corners.
  `apply` sets egui's style from them.
- **`widgets`:** the pieces pages are built from: `title` and `title_with`, `card` and `page_card`,
  which keeps the section gap below it, `caps`, pills, buttons, `flagged_label`, `status_dot` and
  `copy_on_click`, which copies a label's text when it is clicked and says Copied for a moment; and
  the frame around them: `bar_frame`, `sidebar_name`, `bar_name`, `nav_entry`, `tabs` and
  `folded_foot`. `fast_resume_switch` and `fast_resumed` say the same of fast resume in both apps.
- **`layout`:** `NARROW`, the 640 points below which an app lays out for a phone, `SHORT`, the 560
  points below which a sidebar's foot folds to a line, the sidebar's and the page's margins, and
  `suit_touch`, which turns off selecting text on a touch screen so a swipe that starts on text
  scrolls.
- **`net`:** `Client`, how both apps talk to their node. On the desktop and Android it is ureq,
  built with the `Proxy` set, socks5h or http, as its only way out, so a request with the proxy
  down fails rather than going direct; in the browser it is the browser's fetch, through ehttp.
  `fetch` answers a GET or POST, and `stream` hands over an event stream piece by piece.
  `widgets::proxy` is the field both apps' Settings set it with. https comes from the default
  `tls` feature, rustls, whose ring is C.
- **`files`:** `pick` and `can_pick`, which pick files alike everywhere: rfd's dialog on the
  desktop and in the browser, and on Android the APK's own Java activity,
  `ui/android/Picker.java`, which opens Android's document picker. An APK built without it cannot
  pick files, and `can_pick` says so, so the apps leave their buttons out.
- **`android`:** `run` starts an app in Android's NativeActivity, with logging, storage in the
  app's data folder, and `OnAndroid` around it, which keeps the content out of the system bars
  and the keyboard, shows the keyboard for a focused field, copies to and pastes from Android's
  clipboard through JNI, and starts the picker and takes what it read.
- **`edit`:** the bar a finger held on a text field brings up, with Paste, Select all, Copy and
  Cut, and the Paste button over the keyboard. They send the field the events a keyboard would,
  and keep its selection across the tap that takes its focus. Android only, but its tests run on
  the host, against a clipboard of their own.

**`ui/build-android.sh APP`** builds either app's APK; `wallet/build-android.sh` and
`explorer/build-android.sh` call it with their own folder. It reads the package's name from the
app's `Cargo.toml`, and writes `APP/target/android/PACKAGE-ARCH.apk`. With a JDK and Android's
build-tools it then compiles the picker into the APK and signs it again (see
[Picking files](wallet.md#picking-files)); `--no-picker` builds without it. CI's Android runs fail
if an APK comes out without it.

Inside `ui/`: `cargo test`, `cargo clippy --all-targets -- -D warnings`, and clippy for
`wasm32-unknown-unknown` and, with `--no-default-features`, `aarch64-linux-android`: without the
`tls` feature it needs no Android sysroot for ring's C. The apps' APKs, built with one, keep it.

## Features

Every piece sits behind a cargo feature, and everything is on by default.

| Features | What they are |
| --- | --- |
| `sha256`, `blake3`, `blake2s`, `blake2b`, `keccak256`, `xxh3` | Hash algorithms. |
| `ed25519`, `edblake`, `secp256k1` | Key schemes. |
| `mnemonic` | Seed phrases: BIP39 words, and keys derived from them for every scheme built in. |
| `lvm`, `wasm`, `evm` | The contract engines; any pulls in `vm`. `evm` is revm, and pulls in `keccak256`. |
| `rocksdb` | The RocksDB backend. |
| `node` | The node, the chain and the CLI, which stops cleanly on SIGTERM and SIGINT through `signal-hook`. |
| `http` | The HTTP API. |
| `stream` | `GET /stream`, and the node's `--stream`. Needs `http`. |
| `faucet` | A [faucet](faucet.md) for dev and test chains: `--faucet-key` and `POST /faucet`. Brings `http` and `sha256`; `chainv4 faucet` comes with `client`. |
| `client` | `chainv4 send`, `light` and `layout --from`: HTTP, and HTTPS through rustls, with `--ca-cert` and `--insecure`. |
| `gpu` | Off by default: mining on the GPU, through the `chainv4-gpu` crate and wgpu, for `--mine-gpu`, `chainv4 worker --gpu` and `chainv4 gpus`. |
| `finality-ffg`, `finality-authority`, `finality-depth` | Kinds of finality. `finality-ffg` brings `staking`. |
| `governance-electorate`, `governance-council` | Kinds of governance; either brings `governance`. |
| `staking` | Validators bonding funds: running `Bond`, `Unbond`, `Withdraw` and FFG's `Slash`, and the registry's bonding. Without it they fail, `this chain has no staking`. |
| `governance` | Changing what genesis set by transaction: running `ApproveChange` and `WithdrawChange`, the change registry's ballot, `GET /governance`, and `approve-change` and `withdraw-change`. |
| `abi` | Solidity's ABI (`src/abi/`): encoding calls and decoding results, logs and errors, every type, from a contract's JSON ABI; and EVM and CREATE addresses. What the CLI, the wallet and the explorer speak to EVM contracts with. Brings `keccak256`. `evm` brings it. |
| `ethereum` | [Ethereum's tools](ethereum.md): `POST /rpc`, and the index of Ethereum-signed transactions by their keccak256 it answers from. Brings `evm`, `secp256k1` and `http`. Checking and running those transactions is every build's with `secp256k1` and `evm`, so a node without this feature agrees on every block. |
| `names` | The [name registry](names.md): running name transactions, `GET /names`, `chainv4 send name` and names in `transfer --to`, and the EVM's names precompile. A build without it refuses a chain whose genesis has names. |
| `swap` | [Atomic swaps](swaps.md): the terms, Bitcoin's HTLC, Swap.sol's calls, `--swap-contract` and, with `client`, `chainv4 swap`. Brings `bitcoin`. |
| `dex` | The [DEX](dex.md)'s arithmetic, `--dex-contract` and `--dex-router`, and, with `client` and `evm`, `chainv4 dex`. |
| `bridge` | [Bridges](bridges.md) for the apps and the federation's tools: transfers as the gateways hash and the signers sign them, and, with `client` and `swap`, `chainv4 bridge`, `chainv4 bridge btc` among it. The gateways and the federation are contracts and consensus, in every build. |
| `nft` | [NFTs](nft-standard.md) for the apps: where a token URI leads and the metadata it names, and, with `client`, `chainv4 nft`. The collection registry itself is every build's, as it is consensus. |
| `pool` | A [mining pool](consensus.md#mining-pools) on the mining port: `--pool`, shares, PPLNS, `GET /pool` and pushed claims. Blocks that split their reward, and claims, run in every build. |
| `release` | [Releases](updates.md) for the apps and tools: the signed manifest, checked against the release keys built in, and putting a downloaded binary in place. |
| `updates` | A node fetching releases (`--updates`), serving them at `GET /release`, telling workers, stopping at an upgrade a release announces, and `--auto-update`; `chainv4 update` and `chainv4 release`. Brings `release` and `client`. An upgrade governance decides stops a node in every build. |

**Release keys:** `build.rs` builds in `release/keys.json`, or the file `CHAINV4_RELEASE_KEYS`
names, as the keys a binary trusts releases from (see [Updates](updates.md#release-keys)). The
checked-in file names none, so a build from it takes no releases; a release pipeline sets the
variable, or the file, to its own.

**What a build must have:** at least one key scheme, and for contracts and nodes, at least one
secure hash, or they fail to build.

**Builds without some features:** a build without a kind of finality, or an engine, makes
different dev chains, which puts it on a different network.

**CI** (`.github/workflows/test.yml`):
- it runs clippy and the tests on the workspace;
- it runs clippy on the contracts and the SDK's tests;
- it runs clippy and the tests on the explorer and the wallet, and clippy on their web builds;
- it runs clippy and the tests on the `ui` crate, and clippy on its web and Android builds. The
  apps' own Android code is left to the APK builds: linting it would need an Android sysroot for
  ring's C;
- it runs clippy and the tests on a matrix of feature subsets.

**Builds** (`.github/workflows/build.yml`, `android.yml`, `extension.yml`), on every push to master and on tags:
the node's release binaries with the wiki, the example contracts, the wallet's and the
explorer's APKs, and the wallet's Chrome and Firefox extensions, which a `v*` tag also attaches to
its release.

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
- **Stored shapes change with a migration.** A record kept in a new shape bumps
  `STORAGE_FORMAT` and adds a `Migration`, up and back, tested both ways and stopped part way (see
  [Migrations](storage.md#migrations)). What the chain says, its ids, hashes and state roots, never
  changes this way: that is consensus.
- **Errors say who is at fault.**
  - A database failure is the node's fault: it stops the import, and never becomes a
    transaction's failure.
  - A transaction that cannot run makes its block invalid.
  - A transaction that runs and fails stays in its block.
- **Lua libraries** are one file per library under `src/vm/lvm/libs/`, each registering one
  global table, documented with `#[document]` pages of `kind = "library"`.
- **mlua and Luau are forks.** chainv4 uses the germ3n forks of mlua and Luau, which add fuel
  metering, and keep a script's results the same on every node: Luau's fork hashes tables and
  functions as keys by the order they were made, not their address, and refuses userdata keys.
  It also lets a number and a userdata compare through the userdata's `__eq`, `__lt` and `__le`
  (`luaV_equalmixed`, `luaV_lessthan`, `luaV_lessequal` and the `JUMPIFEQ`, `JUMPIFNOTEQ` and
  `JUMPXEQKN` opcodes), and two userdata of different types through the first one's metamethod,
  or the second's, which is how `u64(5) < u256(6)` works. Never swap them for upstream.
- **Trying a change to the Luau fork.** Point chainv4 at the clone with a temporary
  `[patch."https://github.com/germ3n/luau-src-rs"] luau0-src = { path = "../luau-src-rs" }` in
  `Cargo.toml`. Cargo only notices changes to files rustc reads, never to the C++ sources, so
  `touch ../luau-src-rs/src/lib.rs` after each edit, or the old Luau stays linked. Once the fork
  is pushed, drop the patch and `cargo update -p luau0-src`.

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
