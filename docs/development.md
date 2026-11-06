# Development

## Building and testing

```sh
cargo build --release
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

You need Rust 1.98.1, which `rust-toolchain.toml` pins and rustup installs on its own, and clang:
RocksDB and Luau are built from source. See [Toolchains](#toolchains).

## Toolchains

Luau's fuel and memory metering and RocksDB are C and C++, so the compiler that builds them is part
of what a node runs. Two nodes built by different compilers should still meter alike, but that is
something to check, not assume (the todo's cross-OS item). So both toolchains are pinned:

| What | Pinned to | Where |
| --- | --- | --- |
| Rust | 1.98.1, with clippy, rustfmt and `wasm32-unknown-unknown` | `rust-toolchain.toml` at the root, which rustup reads in every folder below it: the node, `wallet/`, `explorer/`, `ui/`, `contracts/`. Also `docker/Dockerfile`'s `RUST_VERSION` and every workflow's `toolchain` |
| clang, for the C and C++ | 19.1.7, LLVM's own release, by its SHA-256 for x86_64 and for arm64 | `docker/llvm.sh`, which downloads it, checks the hash, and unpacks clang, its headers and libclang to `/opt/llvm`. The Docker image runs it and sets `CC`, `CXX` and `LIBCLANG_PATH`, with `/opt/llvm/bin` first on `PATH`, where bindgen asks clang for its headers, and so do the CI's node jobs (test, features, build on Linux): the very same compiler in both |

- **`tests/toolchains.rs` checks that the places agree.** A version changed in one and not the
  others fails `cargo test`, as does an image or a workflow that compiles with another clang.
- **Why LLVM's release, not Debian's or Ubuntu's `clang-19`:** each distribution builds its own
  package of a different 19.1 patch, so the servers and the CI would compile with different
  compilers. LLVM's tarball is one file, the same everywhere. It is large, 1.65 GB for x86_64,
  downloaded once per image build (a cached layer after) and once per CI job.
- **How the hashes were checked (2026-11-05):** x86_64's by LLVM's signed build attestation
  (`gh attestation verify LLVM-19.1.7-Linux-X64.tar.xz --repo llvm/llvm-project --bundle ….jsonl`),
  arm64's by its GPG signature from an LLVM release signing key
  (https://releases.llvm.org/release-keys.asc). A new version is checked the same way before its
  hashes go into `docker/llvm.sh`.
- **The C++ standard library still comes from the system:** clang compiles against the GCC
  installation's libstdc++ headers, GCC 12's in the image and the runner's own in CI. The compiler
  is identical; the headers can differ between the two. Running the CI's node jobs in the image's
  own base (`rust:1.98.1-bookworm`) would make those the same too.
- **Changing a version** means changing it everywhere the table says, then rebuilding the image.
- **GCC is no longer used.** Debian's and Ubuntu's own `cc` and `c++` are GCC, which the servers'
  images used until 2026-11-05. Naming clang in `CC` and `CXX` is what makes it the compiler.
- **macOS** builds with Xcode's Apple clang, which can't be pinned the same way. The CI's macOS
  build prints its version.
- **Phones and the web:** the Android, iOS and other targets are added by the builds that need them
  (`rustup target add`, or the workflow's `targets`), on the same pinned Rust. The Android build
  links with the runner's clang and lld, which is not consensus code.

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
crate or file it expects is gone. A crate that names a license and ships no text of it gets one in
`notices/`, as `hex_lit` does, added in `extra_text`.

**The contracts workspace,** `contracts/`, holds the Rust SDK and the example contracts:
- it builds for `wasm32-unknown-unknown`, through its own `.cargo/config.toml`;
- the end-to-end SDK test builds it from the main crate, so it needs
  `rustup target add wasm32-unknown-unknown`;
- the SDK's own tests run on the host: `cargo test -p chainv4-sdk --target <host>` inside
  `contracts/`.

**The `ethlight` crate,** `ethlight/`, a member of the node's workspace, is Ethereum's light
client: `no_std`, without hashing or signatures of its own, so the node and
`contracts/examples/eth_relay` run the same rules from one source. Its `testing` feature makes
synthetic sync committees from BLS keys of its own, which sign real-format updates, for its tests
and the node's; `cargo test -p ethlight` runs its own.

**The `btclight` crate,** `btclight/`, a member of the node's workspace beside `ethlight` and built
the same way, is Bitcoin's header rules: `no_std`, without hashing of its own (a `Crypto` passes
SHA-256 twice in), so the node's anchored sync and `contracts/examples/btc_relay` run them from one
source. It holds compact difficulty, work and the retarget, the networks' limits and rules, the 80
bytes of a header, the record the relay keeps of each (`Kept`, byte for byte as the relay stores
it, version and nonce last) with `Kept::child`, which checks a header by every rule, the most-work
chain (`MainChain` and `follow`), and Merkle inclusion with the coinbase's path showing the tree's
height (CVE-2017-12842). Its 256-bit integers are `ethnum`'s, the node's own `u256`, on both sides.
The contracts workspace builds it at `opt-level = "s"`, as it does `ethlight`: the relay came out smaller, and cheaper in fuel, than at `"z"`. `cargo test -p btclight` runs its own tests; the relay's tests in the node check it as a contract.

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
  which keeps the section gap below it, `caps`, pills, buttons, `segmented`, `segmented_fit` (one
  row however narrow, the options sharing it evenly), `segmented_rows`, `group_heading`,
  `flagged` and `flagged_label` (monospace, so `--` never joins into a dash), `status_dot` and
  `copy_on_click`, which copies a label's text when it is clicked and says Copied for a moment; and
  the frame around them: `bar_frame`, `sidebar_name`, `bar_name`, `nav_entry`, `tabs` and
  `folded_foot`. `fast_resume_switch` and `fast_resumed` say the same of fast resume in both apps.
- **`wipe`:** `Wiping`, the apps' global allocator, which zeroes each block freed or moved;
  `let_go` and `letting_go`, which have egui free what it laid out for pages just dropped; and
  `no_dumps` (see [Wallet](wallet.md#keys)).
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
app's `Cargo.toml`, and writes `APP/target/android/PACKAGE-ARCH.apk`. It then adds the Java,
`ui/android/classes.dex`, committed, and signs the APK again, with `ui/android/repack`: a small
Rust tool on xbuild's own APK library (aligned entries, APK Signature Scheme v2), so no JDK and
nothing of Android's SDK is needed (see [Picking files](wallet.md#picking-files)); `--no-picker`
builds without it. After changing the Java, `ui/android/dex.sh` (a JDK and build-tools 34.0.0)
builds the dex again and `classes.dex.sources` the hash it was built from; the APK build refuses a
dex older than the Java, and the Android workflow's dex job checks the dex is what the Java
builds.

Inside `ui/`: `cargo test`, `cargo clippy --all-targets -- -D warnings`, and clippy for
`wasm32-unknown-unknown` and, with `--no-default-features`, `aarch64-linux-android`: without the
`tls` feature it needs no Android sysroot for ring's C. The apps' APKs, built with one, keep it.

## Features

Every piece sits behind a cargo feature, and everything is on by default.

| Features | What they are |
| --- | --- |
| `sha256`, `blake3`, `blake2s`, `blake2b`, `keccak256`, `xxh3` | Hash algorithms. |
| `ed25519`, `edblake`, `secp256k1` | Key schemes. |
| `bls12-381` | Ethereum's BLS signatures, on zkcrypto's `bls12_381` in pure Rust: FastAggregateVerify with the ciphersuite `BLS_SIG_BLS12381G2_XMD:SHA-256_SSWU_RO_POP_`, for contracts following Ethereum's sync committees (`fast_aggregate_verify`, see [Contracts](contracts.md#ethereums-light-client)). Without it the host call stays, so modules link alike, but fails the call. |
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
| `governance` | Changing what genesis set by transaction: running `VoteChange` and `ClearVote`, the change registry's ballot, `GET /governance`, and `vote-change` and `clear-vote`. |
| `abi` | Solidity's ABI (`src/abi/`): encoding calls and decoding results, logs and errors, every type, from a contract's JSON ABI; and EVM and CREATE addresses. What the CLI, the wallet and the explorer speak to EVM contracts with. Brings `keccak256`. `evm` brings it. |
| `ethereum` | [Ethereum's tools](ethereum.md): `POST /rpc`, and the index of Ethereum-signed transactions by their keccak256 it answers from. Brings `evm`, `secp256k1` and `http`. Checking and running those transactions is every build's with `secp256k1` and `evm`, so a node without this feature agrees on every block. |
| `names` | The [name registry](names.md): running name transactions, `GET /names`, `chainv4 send name` and names in `transfer --to`, and the EVM's names precompile. A build without it refuses a chain whose genesis has names. |
| `swap` | [Atomic swaps](swaps.md): the terms, Bitcoin's HTLC, Swap.sol's calls, `--swap-contract` and, with `client`, `chainv4 swap`. Brings `bitcoin`. |
| `anchoring` | [Anchoring](anchoring.md)'s base, whichever chains hold the anchors: the anchor list and its root, `Tx::Anchor`'s dispatch and reward, nodes refusing branches that leave an enforced anchor, `GET /anchors`, serving the list to peers, and `--anchored`, which compares the histories peers show. Without it the types stay, so nothing encodes differently, but the node takes, serves and enforces no anchors. |
| `anchoring-btc` | Anchors in Bitcoin: `Tx::Anchor`'s Bitcoin proofs, read against the relay's records, Bitcoin's headers checked by work for `--anchored` and served to peers, and, with `bridge`, the anchorer's Bitcoin leg (`chainv4 bridge anchor`). Brings `anchoring`, `bitcoin` and the `btclight` crate. Without it a Bitcoin proof fails, so the build cannot follow a chain that takes them. |
| `anchoring-eth` | Anchors in Ethereum, without Bitcoin: `Tx::Anchor`'s Ethereum proofs, read against the Ethereum relay's records with the sync committee's signature checked, `--anchored` checking them against committees followed through `--beacon` (with `client`), and, with `bridge`, the anchorer's Ethereum leg. Brings `anchoring` and `bls12-381`. The `ethlight` crate is in every build, as anchoring's settings name a beacon network every node reads alike. Without it an Ethereum proof fails, so the build cannot follow a chain that takes them. |
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

**On this machine:** `scripts/ci.sh` runs all of it without spending runner minutes, and adds the wallet's
store build and the iOS checks. `--list` names its steps, `--from N` picks up at one, and `CI_MATRIX_ONLY=1` and
`CI_NO_MATRIX=1` narrow it. It never runs a GPU kernel: `chainv4-gpu` stays out of the workspace's tests, and the
matrix's `gpu` row is linted and compiled but not run, as its tests skip only where there is no GPU.

**CI** (`.github/workflows/test.yml`), started by hand from the Actions tab or `gh workflow run`, as nothing runs by itself for now, to spare runner minutes:
- it runs clippy and the tests on the workspace;
- it runs clippy on the contracts and the SDK's tests;
- it runs clippy and the tests on the explorer and the wallet, and clippy on their web builds;
- it runs clippy and the tests on the `ui` crate, and clippy on its web and Android builds. The
  apps' own Android code is left to the APK builds: linting it would need an Android sysroot for
  ring's C;
- it runs clippy and the tests on a matrix of feature subsets.

**Builds** (`.github/workflows/build.yml`, `android.yml`, `extension.yml`), started by hand as well:
the node's release binaries with the wiki, the example contracts, the wallet's and the
explorer's APKs, and the wallet's Chrome and Firefox extensions, which a run started on a `v*` tag
also attaches to its release. Beside them, also by hand:
- **image** (`image.yml`): the node's and nginx's Docker images, pushed to GHCR (see
  [Docker](docker.md#pulling-what-ci-built));
- **release** (`release.yml`): a tag's binaries for Linux and macOS and its unsigned manifest, as a
  draft release (see [Updates](updates.md#publishing-a-release));
- **windows** (`windows.yml`): the node built and its library tests on Windows, and the desktop
  wallet and explorer built there.
- **e2e** (`e2e.yml`): the end-to-end tests below, with Bitcoin Core's release and Foundry's anvil.

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

### The fuel and memory fixture

`src/chain/tests/fixture.rs` runs Lua calls whose outcome rests on how the Luau fork counts fuel and
memory, each in its own block on one chain:
- powers;
- a backtracking pattern;
- a sort;
- strings and tables grown past the memory limit;
- garbage made and dropped;
- the integer types;
- utf8 and the string library;
- an endless loop;
- caught errors;
- storage.

It writes each call's fuel and how it ended, then the receipts and state roots, and compares that
byte for byte with `fixture.txt` beside it. A machine whose fuel, memory counting or roots differ
fails the test and prints the lines that differ: such a node would split the chain. Run it on each
kind of machine a chain mixes, `cargo test --lib --features lvm chain::tests::fixture`:
- Linux x86-64 and arm64;
- macOS arm64 and x86-64;
- Windows;
- under load as well as idle.

After a consensus change meant to move it, write it again with `CHAINV4_WRITE_FIXTURE=1` and commit
the new file with the change.

### Measuring fuel

`chainv4 calibrate` (built with `lvm`) times what fuel should cost on the machine it runs on. Each
probe is a loop in a deployed Lua contract, its body against a base that does all but the one
thing, at `--iterations` a call, the fastest of `--rounds` calls:
- **Opcodes:** one fuel is anchored to the opcode probes together, what they took over what they
  are charged, so overall prices stay where they are and only what each costs against the others
  moves (`--ns-per-fuel` sets it instead). The cheapest opcodes take hundredths of a nanosecond,
  too little to time alone.
- **Library rates:** the same call at two sizes, so only the work that grows with size is left.
- **Host operations:** storage reads and writes and hashing, past a cheap call into the host.

It prints each one's time, its cost now, what it was charged and what it suggests, and marks what
is charged under half of what it costs: the way to stall every node with cheap work. `--json FILE`
writes the measurements, and the suggestions as a fees object's `lua_costs` and `costs`, for a
genesis file or a `Change::Fees`. WebAssembly's instructions and the EVM's opcodes are priced by
wasmi and revm, and are not probed yet. Run it on the machines a chain's nodes run on, and price
for the slowest.

### End to end

The tests above run a node in-process. The end-to-end tests start real processes on this machine's
loopback, and drive them as people do. They take minutes and need outside programs, so they are
`#[ignore]`d; the **e2e** workflow runs them by hand in CI:
- **`tests/e2e_nodes.rs`** (`cargo test --release --test e2e_nodes -- --ignored --test-threads 1`):
  - three nodes of an authority chain over QUIC: blocks gossiped and synced, a transfer sent to
    one follower held by all, a checkpoint finalized;
  - a follower stopped and started again on its database, catching up;
  - a fourth node joining from a snapshot and holding the transfer it never ran;
  - two proof-of-work miners that mined apart meeting on one chain.
- **`tests/e2e_swaps.rs`** (needs `anvil` and `bitcoind` on the path):
  - chainv4 against anvil: Swap.sol deployed with `swap evm-deploy`, both legs locked and claimed
    by the CLI, and a second lock refunded once anvil's clock passes its deadline;
  - Bitcoin against chainv4: the HTLC's funding block taken back by `invalidateblock` and mined
    again on another branch, then the coin claimed with the secret and the bitcoin with what it
    showed; another HTLC refused a refund before its lock time and refunded once `setmocktime`
    passes it.
- **`wallet/tests/e2e_wallet.rs`** and **`explorer/tests/e2e_explorer.rs`** (`cargo test --test
  e2e_wallet -- --ignored` in their folders, after `cargo build` at the root): the whole desktop
  app, driven headlessly through what accesskit sees of it (egui_kittest), against a real node:
  - the wallet restored from words, a payment the CLI makes shown once proven, a send from its
    Send page that the node then holds, and what is left shown;
  - the explorer's latest blocks, an account found by searching it, and the block that paid it.

Each keeps its nodes' logs in a folder under the system's temp folder, left there when it fails
and named in the failure.

**Screenshots of every page,** for looking the apps over rather than testing them:
`SHOTS_DIR=/some/folder cargo test --test shots -- --ignored --nocapture` in `wallet/` or
`explorer/`, after `cargo build` at the root. Each starts a node, restores the wallet from the
public Hardhat words or opens the explorer, and writes `APP-WIDTHxHEIGHT-PAGE.png` for every page
at a desktop's 1280×800 and a phone's 390×844, through egui_kittest's wgpu renderer (its `snapshot`
feature). Pages are pressed in a tall window so the sidebar shows them all, and the explorer's run
opens a folded group to reach its page.
