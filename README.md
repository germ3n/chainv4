# chainv4

A blockchain node, and a command-line client for it, in one binary. Contracts run on Lua
(Luau) or WebAssembly; blocks come from proof of work or a development authority, and are
finalized by bonded validators, a set of signers, or depth; nodes talk over libp2p; a JSON
HTTP API serves queries and takes transactions. Every header commits to the chain's state and to
what its transactions did, as Merkle roots that values and receipts can be proven against.

This page covers the `chainv4` command and how contracts live on the chain.

- **How each part works:** the guides in [docs/](docs/README.md), starting with
  [Good to know](docs/good-to-know.md) for what to keep in mind.
- **The code reference:** the wiki that every build writes to `target/wiki/index.html`.
- **Contract storage:** how contracts keep state, and the layouts upgrades must keep, in
  [docs/contract-storage.md](docs/contract-storage.md).
- **Tokens:** they follow the [token standard](docs/token-standard.md).
- **The explorer:** [explorer/](explorer/), an egui app for the desktop, the browser and Android;
  see [docs/explorer.md](docs/explorer.md).
- **What is left to do:** [docs/todo.md](docs/todo.md).

## Building

```sh
cargo build --release
./target/release/chainv4 --help
```

You need stable Rust, a C/C++ toolchain and clang: RocksDB and Luau are built from source.
Everything is on by default; each piece sits behind a cargo feature (`lvm` and `wasm` for the
contract engines, `node`, `http` for the API, `client` for `chainv4 send`, one per hash, key
scheme and [kind of finality](#finality)) if you want a smaller build.

## Quick start

Run a node that mines and serves the API:

```sh
chainv4 --mine --http 127.0.0.1:8080
```

In another terminal, make a key and send it some coins from the development key, which holds a
million coins on every dev chain:

```sh
chainv4 keygen --out alice.key
# wrote alice.key
# address ed25519:0x5be0…

chainv4 send transfer --to ed25519:0x5be0… --amount 1000000000000000000
# submitted 0x9c1d…
# included in block 14 0x77ab… (1 confirmation)
# done
# gas used 3620, fee 2771562507240
```

Amounts are whole numbers in the smallest unit; coins have 18 decimals, so
`1000000000000000000` is one coin.

## Running a node

`chainv4` with no subcommand runs a node.

| Option | Default | What it does |
| --- | --- | --- |
| `--db PATH` | `chain.db` | Where the chain is stored. |
| `--listen ADDR` | `/ip4/0.0.0.0/udp/0/quic-v1` | A libp2p address to listen on; repeat for several. Port 0 picks one; the log shows which. |
| `--peer ADDR` | | A peer to dial at startup; repeat for several. Nodes on the same network also find each other through mDNS. |
| `--max-gossip-backlog N` | `10000` | The most gossiped transactions and votes, and submitted transactions, that may wait for the node. More are dropped; submissions get 503. |
| `--max-gossip-per-peer N` | `1000` | The most of those from any one peer. More are dropped, and a peer that goes over 10 times in a minute is disconnected. |
| `--max-block-txs N` | | The most transactions a block this node produces holds. Without it, blocks fill to the chain's `max_block_bytes` and gas limit. |
| `--consensus pow\|authority\|rotating\|staked` | `pow` | Which development chain to run. |
| `--genesis FILE` | | Run the chain a [genesis file](#your-own-chain) describes instead. |
| `--mine` | | Mine blocks (pow). With `--produce` too, the node does whichever the chain runs, as a chain with [governance](docs/governance.md) may change it. |
| `--reward-to ADDR` | dev key | Where mining rewards go. |
| `--mine-threads N` | `1` | How many threads to mine on. |
| `--produce` | | Produce blocks (authority). |
| `--producer-key PATH` | dev key | The authority's key file, for `--produce`. |
| `--validate` | | Vote on [finality](#finality), on chains whose finality takes votes. |
| `--validator-key PATH` | dev key | The voting key file a validator bonded with, or a signer's key, for `--validate`. |
| `--http ADDR` | | Serve the HTTP API, such as `127.0.0.1:8080`. |
| `--stream` | | Push what the node does to API clients as it happens, on `GET /stream`. |
| `--cors ORIGIN` | | Let web pages from this origin call the API, or `*` for any; repeat for several. |
| `--explorer DIR` | | Serve the [explorer](docs/explorer.md)'s web app, built into `explorer/dist`, from the API's `/`. |
| `--wallet DIR` | | Serve the [wallet](docs/wallet.md)'s web app, built into `wallet/dist`, from the API's `/wallet/`. |
| `--snapshot` | | On a fresh database, sync from a snapshot: check every header peers have, take the state of a block 16 below the best tip from them, checking every part against its state root, and run only the blocks after it. See [Syncing from a snapshot](docs/node.md#syncing-from-a-snapshot). |
| `--index-events[=FROM]` | | Index events by contract, by name and by the addresses in them, for `GET /events`. `FROM` is `genesis` (when left out), a height, or `latest-N` for a window of the latest N blocks. Older blocks are indexed in the background. Once on, it stays on until `--index-events=off` deletes it. |
| `--index-addresses[=FROM]` | | Index transactions by every address they touch, for `GET /accounts/{address}/txs`: what the [wallet](docs/wallet.md) lists a history from. `FROM` and `=off` as for `--index-events`. |

Logs go to stderr; set `RUST_LOG`, such as `RUST_LOG=debug`, for more.

### The dev chains

- **pow** (default): anyone may mine. Blocks aim for one every two seconds and difficulty
  follows. A block mints 50 coins for its miner, halving every 210,000 blocks. The heaviest
  chain wins, so forks are normal; a node reorganizes to a branch that holds more work, up to
  100 blocks back.
- **authority**: the development key produces a block every two seconds, with `--produce` on
  exactly one node. No rewards, no forks. Handy for tests and demos.
- **rotating**: authority finality's signers produce in turn, the development key being the one
  signer; **staked**: FFG's validators produce by stake, the development key being the one
  validator. Each with `--produce`, for no reward: the multi-producer schemes, run alone.

All give the development key a million coins and let it mint, and all but `rotating` make it
the one FFG validator, bonded with 10,000 coins: run one node with `--validate` and the chain
finalizes every 8 blocks. All charge [fees](#fees), starting at 1 gwei of gas, which `send`
works out on its own; an account needs coins to send anything. Its seed is in the source, so
never use a dev chain for anything of value. A dev chain also allows every contract engine the
node was built with, so nodes built with different features are on different chains.

### Several nodes

```sh
chainv4 --db a.db --mine --listen /ip4/127.0.0.1/udp/4001/quic-v1 --http 127.0.0.1:8080
chainv4 --db b.db --mine --listen /ip4/127.0.0.1/udp/4002/quic-v1 --peer /ip4/127.0.0.1/udp/4001/quic-v1
chainv4 --db c.db --peer /ip4/127.0.0.1/udp/4001/quic-v1 --http 127.0.0.1:8081
```

A node that starts late, or falls behind, catches up from its peers on its own. A new one can
skip running the history with `--snapshot`: it checks every header, then takes the state of a
recent block from its peers, checking each part against that block's state root, and runs only
the blocks after it.

```sh
chainv4 --db d.db --snapshot --peer /ip4/127.0.0.1/udp/4001/quic-v1
# [node] checked 46 headers; taking state at height 30
# [node] took 12 entries of state at height 30; fetching the block
# [node] synced from a snapshot to height 30 0x7d8e…
```

It keeps headers alone for the blocks before that one.

A database belongs to one chain: a node refuses to open a `chain.db` written for a different
genesis. Delete it, or point `--db` elsewhere, after switching `--consensus`, `--genesis` or
features.

### Your own chain

Every parameter of a chain lives in its genesis: the consensus and its timing, the hash, block
rewards, allocations, the minter, which engines run contracts, transaction limits, how deep a
reorg may go, which finality it runs and what transactions pay, with every setting of each.
`chainv4 genesis` writes the dev defaults as JSON to edit:

```sh
chainv4 genesis --consensus authority --finality authority --fees bid --out mychain.json
chainv4 --genesis mychain.json --produce --producer-key me.key --http 127.0.0.1:8080
```

`--finality` is `ffg` (the default), `authority`, `depth` or `none`. `--fees` is `base-fee`
(the default), `bid` or `free`, and `--scorer CONTRACT` has a contract score transactions.

Every node on the chain needs the same file. Each field must be there and unknown ones are
refused, so a misspelled parameter is an error, not a silent default; a node also refuses
settings that cannot work, such as a checkpoint interval of 0.

`hash` picks the chain's hash, used for block ids, transaction ids, roots and contract
addresses: `blake3`, `sha256`, `keccak256`, `blake2s` or `blake2b`, whichever the build has. A
pow chain picks its work hash separately, as `consensus.hash`. `xxh3` is fast but not secure,
so neither takes it.

### In Docker

`docker/` runs a node with every feature behind nginx, serving the API, the explorer and the
wallet over HTTPS:

```sh
cd docker && cp .env.example .env && docker compose up -d --build
# https://localhost/ and https://localhost/wallet/
```

`.env` picks the chain (`CHAIN_CONSENSUS=pow|authority`, `CHAIN_FINALITY=ffg|authority|depth|none`,
`CHAIN_FEES`) and the certificate (`TLS_MODE=self-signed|provided|letsencrypt`). See
[Docker](docs/docker.md).

### A light client

`chainv4 light` checks what a node says without running the chain. It trusts only the genesis
you give it, checks every header by consensus and every justification against voters it proved
from state, and answers only with values it proved:

```sh
chainv4 light --consensus authority account ed25519:0xa9d1…
# followed 45 headers to 45 0x7417…, each checked by authority
# justified epoch 5 0x23f4…, finalized epoch 4 0xb379…
# balance 1000003000000000000000000, nonce 0: proven at block 32 0xb379…, which is final
# balance 1000005000000000000000000, nonce 0: proven at block 45 0x7417…, not final yet
```

`status`, `account ADDRESS`, `entry COLUMN KEY`, `receipt TX` (who sent what, and what it did)
and `history ADDRESS`, from a node started with `--index-addresses`; `--genesis FILE` or
`--genesis-id 0x…` to trust another chain, and `--node URL` for the node, which it does not
trust. The explorer runs the same client and shows the balances it proved. See
[Light client](docs/light-client.md).

### The wallet

`wallet/` is a light wallet for the desktop, the browser and Android, in egui. Its accounts come
from a seed phrase, each with an Ed25519, an EdBlake and an Ethereum address; its keys are sealed
with a password on the device, and it shows only what it proved itself:

```sh
cd wallet && cargo run --release -- --node http://127.0.0.1:8080
cd wallet && trunk build --release    # then: chainv4 --http … --index-addresses --wallet wallet/dist
wallet/build-android.sh --install     # an Android APK, built with xbuild
```

Balances of the coin and every listed token at each address, sending with a reviewed fee from the
address picked, receiving with a QR code, several accounts and several wallets, and a history from
the node's address index. CI builds the APKs on every push, and attaches them to `v*` releases.
See [Wallet](docs/wallet.md).

### The explorer

`explorer/` is a window onto a node, in egui, for the desktop, the browser and Android: blocks on a
map, side branches included, transactions and the hops their contracts made, accounts, contracts
and tokens, the network, the mempool and finality, live. It lays out with a sidebar on a wide
screen and tabs on a phone:

```sh
cd explorer && cargo run --release -- --node http://127.0.0.1:8080
cd explorer && trunk build --release  # then: chainv4 --http … --stream --explorer explorer/dist
explorer/build-android.sh --install   # an Android APK, built with xbuild
```

It shares the `ui` crate with the wallet, and CI builds its APKs beside the wallet's. See
[Explorer](docs/explorer.md).

## Keys

```sh
chainv4 keygen [--scheme ed25519|edblake|secp256k1] [--out PATH]
chainv4 keygen --phrase [--words 12|24] [--account N] [--passphrase]
chainv4 keygen --from-phrase [--account N] [--passphrase] < words.txt
```

Writes a new key to `PATH` (default `chainv4.key`) and prints its address. The file holds one
line, `ed25519:0x…`, the scheme and the 32-byte secret in hex. It is created readable by you
only and is never overwritten, but it is not encrypted: guard it.

`secp256k1` is Ethereum's scheme: its addresses are Ethereum's 20 bytes, `secp256k1:0x…`, and a
key has the same `0x…` address here as there. `--phrase` makes new seed-phrase words, printed once,
and writes account `N`'s key from them; `--from-phrase` reads words from standard input instead.
The same words give the same keys as the wallet, and the same secp256k1 accounts as Ethereum's
wallets. See [Crypto](docs/crypto.md#seed-phrases).

## The database

```sh
chainv4 db format [--db chain.db]
chainv4 db migrate [--db chain.db] [--to N] [--yes] [--no-backup]
```

A database records the storage format it is kept in. A node moves an older one up to its own
format as it starts, after copying it aside as `chain.db.format-N`. `db migrate` does the same by
hand, and with `--to` takes a database back down to an older chainv4's format: run it with the
newer chainv4, which lists what it undoes and asks you to type `yes`. Migrations work in
crash-safe chunks, so a stopped one goes on from where it stopped. See
[Migrations](docs/storage.md#migrations).

## Sending transactions

```sh
chainv4 send [OPTIONS] <transfer|mint|burn|deploy|upgrade|call|bond|unbond|withdraw|slash|approve-signers|withdraw-approval> …
```

`send` looks up the signer's next nonce, signs, posts the transaction to a node, and waits until
it is in a block. It then prints where it landed, what it did and any events it emitted. A
transaction that landed but failed exits non-zero.

| Option | Default | What it does |
| --- | --- | --- |
| `--node URL` | `http://127.0.0.1:8080` | The node's HTTP API. Plain HTTP only. |
| `--key PATH` | dev key | The key file to sign with. Without one, the development key signs. |
| `--no-wait` | | Return as soon as the node takes the transaction. |
| `--timeout SECS` | `60` | Give up waiting after this long. The transaction may still land. |
| `--confirmations N` | `1` | Wait until N blocks hold it, its own included. On a pow chain, a few more guard against a fork taking it back off. |
| `--final` | | Wait until its block is finalized, which no fork can take back. |
| `--gas-limit N` | its cost | The most gas it may use. By default what the chain's gas schedule says it costs, plus the chain's fuel cap for calls, deploys and upgrades. |
| `--max-price N` | see [Fees](#fees) | The most it pays per gas, in the smallest unit: by default twice the base price plus the tip, or the lowest bid. |
| `--tip N` | `0` | Under a base fee, what it adds per gas for the producer. |
| `--anyway` | | Send it even though a dry run on the node says it would fail. |
| `--eip712` | | Sign it as EIP-712 typed data for this chain, as MetaMask does, rather than in chainv4's own form. A secp256k1 key only. See [Typed data](docs/crypto.md#typed-data-eip-712). |

### transfer, mint, burn

```sh
chainv4 send transfer --to ed25519:0x… --amount 250
chainv4 send mint --to ed25519:0x… --amount 1000   # only the chain's minter, the dev key here
chainv4 send --key alice.key burn --amount 5
```

### deploy

```sh
chainv4 send deploy --lua counter.lua
chainv4 send deploy --lua contracts/lua/names/names.lua   # with the files it --@includes put in
chainv4 send deploy --wasm contracts/target/wasm32-unknown-unknown/release/counter.wasm
# submitted 0x…
# contract contract:0x41f0…
# included in block 20 0x… (1 confirmation)
# deployed contract:0x41f0…
```

A Lua contract is a script that returns a table of functions:

```lua
local M = {}

function M.increment(by)
    local count = tonumber(storage.get("count") or "0") + by
    storage.set("count", tostring(count))
    events.emit("Incremented", count, ctx.sender)
    return count
end

return M
```

Events land in the transaction's receipt; `send` prints them.

Rust contracts use the SDK in [contracts/](contracts); build its examples with

```sh
cd contracts && cargo build --release
```

A deploy also runs the contract's `init`, if it has one; see [Contract lifecycle](#contract-lifecycle).

### upgrade

```sh
chainv4 send upgrade --contract contract:0x41f0… --lua counter_v2.lua
# included in block 30 0x… (1 confirmation)
# upgraded contract:0x41f0…
```

Replaces the contract's code; its address, storage and balance stay. The contract decides who
may: see below. `--wasm` works too, so a contract can even move between engines. New code must
keep the old code's [storage layout](#storage-layouts).

### call

```sh
chainv4 send call --to contract:0x41f0… --function increment i64:5
# included in block 21 0x… (1 confirmation)
# returned [{"type":"i64","value":"5"}]
# fuel used 4821
# event Incremented from contract:0x41f0… [{"type":"i64","value":"5"},{"type":"address","value":"ed25519:0x…"}]
```

`--amount N` sends coins along with the call; the contract sees them as `ctx.value`.

Arguments are `type:value`:

| Type | Example |
| --- | --- |
| `i64`, `u64`, `i128`, `u128`, `i256`, `u256` | `i64:-5`, `i256:-5`, `u256:1000` |
| `number` | `number:2`: Lua contracts take whole numbers only, and hand them back as `i64` |
| `amount` | `amount:1000000000000000000` |
| `bool` | `bool:true` |
| `string` | `string:hello` |
| `bytes` | `bytes:0x00ff` |
| `address` | `address:ed25519:0x…`, `address:secp256k1:0x…`, `address:contract:0x…` |
| `nil` | `nil`, alone: no value, Lua's `nil` |

## Contract lifecycle

Besides the functions a contract exports for everyone, the chain looks for four by name:

| Function | Who runs it | When |
| --- | --- | --- |
| `init()` | the chain only | Once, at deploy, with the deployer as `ctx.sender`. Record an owner, mint an initial supply. If it fails, so does the deploy. |
| `check_perms(permission, who)` | the chain, before a privileged action; anyone may call it | Before an upgrade, with `"upgrade"` and the sender. Only `true` lets the upgrade through. What it writes is thrown away. |
| `migrate()` | the chain only | Once, after an upgrade to code that exports it, with the upgrader as sender. Move storage to a new layout. If it fails, so does the upgrade. |
| `fallback(selector, ...)` | the chain only | Whenever a call names a function the contract does not export, with that function's selector, a `u64`, in front of the call's arguments. It answers in that function's place. |

Transactions and other contracts cannot call `init`, `migrate` or `fallback`. A contract without
`check_perms` can never be upgraded, and neither can one upgraded to code without it, so freezing
a contract is one last upgrade. `GET /contracts/{address}` shows `"upgradable"`.

An owned, upgradable contract:

```lua
local M = {}

function M.init()
    storage.set("owner", ctx.sender:hex())
end

function M.check_perms(permission, who)
    return permission == "upgrade" and who:hex() == storage.get("owner")
end

return M
```

`check_perms` can hold any rule: a list of admins, a vote counted in storage, a time lock. Rust
contracts export the same names with `#[export]`.

### Proxies: upgrades without the built-in

`contracts.delegate(address, function, ...)` (`delegate` in the SDK) runs another contract's
code as the caller's own, like the EVM's `delegatecall`: on the caller's storage and balance,
with `ctx.sender`, `ctx.contract` and `ctx.value` unchanged, and events in the caller's name.
Together with `fallback`, that is a proxy: callers always use the proxy's address, it keeps the
state, and its implementation can be swapped.

```lua
local M = {}

-- storage.own is the proxy's own space: implementations can read it, but never write it.
function M.init()
    storage.own:set("owner", ctx.sender:text())
end

function M.set_implementation(implementation)
    if ctx.sender:text() ~= storage.own:get("owner") then error("only the owner") end
    -- New code must keep the current implementation's storage layout, as an upgrade's would.
    local current = storage.own:get("implementation")
    if current then contracts.check_layout(address.parse(current), implementation) end
    storage.own:set("implementation", implementation:text())
end

-- ctx.args are the caller's arguments exactly as they came, and the results go back the same
-- way, so no value changes type passing through.
function M.fallback(selector)
    return contracts.delegate(address.parse(storage.own:get("implementation")), selector, ctx.args)
end

return M
```

```sh
chainv4 send deploy --lua contracts/lua/proxy.lua          # contract:0xP…
chainv4 send deploy --lua counter.lua                       # contract:0xC…
chainv4 send call --to contract:0xP… --function set_implementation address:contract:0xC…
chainv4 send call --to contract:0xP… --function increment i64:5   # runs the counter's code on the proxy
```

Full proxies are in [contracts/lua/proxy.lua](contracts/lua/proxy.lua) and
[contracts/examples/upgradable](contracts/examples/upgradable/src/lib.rs). Things to keep in mind:

- Delegated code has full access to the caller, but for its own space, which it can only read:
  only delegate to code you trust.
- The proxy and every implementation share its plain storage and its namespaces
  (`storage.namespace(name)`), so keep each new implementation able to read the old layout:
  declare it, and the proxy's `check_layout` holds new implementations to it (see below). The
  proxy's own keys go in `storage.own` (`storage::own()` in Rust), where no implementation can
  change them.
- Delegating only reaches functions the target exports, never its `fallback`, `init` or
  `migrate`. An implementation that needs setting up exports something like `initialize`, called
  through the proxy once it is pointed at it.
- Values pass through a Lua proxy as Lua values, so an `amount` comes back as a `u256` and bytes
  that read as text as a string; a Rust proxy forwards them exactly, and so does a Lua one
  passing `ctx.args` on.

### Storage layouts

Code can declare the namespaces it keeps its state in: maps, keys of one kind or any with values
of one kind, and records, only the keys they declare, each with its kind. A kind may name its
unit. New code, by `send upgrade` or a proxy repointing, must then keep every namespace and key
with the same kinds and units, or retire it on purpose:

```lua
--@namespace balances address-text -> decimal:wei   -- a map: text keys, decimal values in wei
--@namespace config                                 -- a record
--@key config owner address-hex
--@retired fees

local balances = storage.namespace("balances")
```

In Rust, `chainv4_sdk::namespace!(BALANCES, "balances", "address" => "u256:wei");`,
`namespace!(CONFIG, "config", { "owner": "address" });` and `retired!("fees");`.

Kinds are encodings the node checks on every write, and on every access for a map's keys:
`bytes`, `text`, `number`, `decimal`, `bool`, `i64`, `u64`, `i128`, `u128`, `i256`, `u256`, `address`,
`address-text` and `address-hex`, each with a unit after a colon or none. Lua writes the text
kinds and Rust the binary ones, and each engine reads and writes the other's, so a contract can
move between them and keep its state. Code that declares a layout reaches nothing past it: no
other namespace or key, no key or value of the wrong kind, and plain storage only to read and
clear it. Retired namespaces and keys can be read and
cleared but not written, so `migrate` can move their state into new ones. Check new code before
sending it:

```sh
chainv4 layout counter_v2.lua --from counter_v1.lua
chainv4 layout counter_v2.lua --from contract:0x41f0…    # what is deployed, over the API
# counter_v2.lua does not keep the layout of contract:0x41f0…: key owner of namespace config is dropped: keep it, or retire it
```

See [Storage layouts](docs/contracts.md#storage-layouts).

## Tokens

Tokens are contracts that follow the [token standard](docs/token-standard.md), much like ERC-20:
`transfer`, `approve`, `transfer_from`, `balance_of` and the rest, with `Transfer` and
`Approval` events, and optionally `name`, `symbol` and `decimals`. They never touch the native
coin, which moves only with `--amount` and `accounts.transfer`. The chain keeps a registry of
them: a contract that exports the functions genesis names in `token_functions`, ERC-20's six by
default, is listed at `GET /tokens` as it deploys. Deploy one of the examples and use it:

```sh
chainv4 send deploy --lua contracts/lua/token.lua
# contract contract:0x9a1c…
chainv4 send call --to contract:0x9a1c… --function transfer address:ed25519:0x5be0… u256:1000
# event Transfer from contract:0x9a1c… [{"type":"address",…},{"type":"address",…},{"type":"u256","value":"1000"}]
```

The deployer owns the example token: `init` gives them a million tokens, and only they may
`mint` more or upgrade it.

## Finality

Final blocks never come off the chain, whatever the work on another branch. Genesis picks how
blocks become final, as its `finality.type`; each kind is a cargo feature, all on by default:

| Type | Final when | Who | Feature |
| --- | --- | --- | --- |
| `ffg` | Two thirds by bond vote for it, as in Casper FFG. | Validators who bond on chain, and can be slashed. | `finality-ffg` |
| `authority` | A threshold of signers vote for it, the same way. | Keys genesis names, changed by their own approval. | `finality-authority` |
| `depth` | `depth` blocks are on top of it. | Nobody votes. | `finality-depth` |

Under the two that vote, every `checkpoint_interval` blocks starts an epoch, and the block there
is its checkpoint. A validator votes for a link from the newest justified checkpoint to the
newest checkpoint; a link with enough votes behind it justifies its target, and when the target
is the very next epoch, its source is finalized. A node refuses any branch that forks at or below
the finalized checkpoint, follows work only on branches that keep the justified checkpoint, and
moves to a branch with a newer justified checkpoint even when it holds less work. Votes travel by
gossip, and a node that falls behind fetches the justifications it lacks with the blocks.

### Validating

The rest of this section is about `ffg`; [authority](#authority-finality) and
[depth](#depth-finality) follow.

A validator is a voting key with a bond behind it. Make a key, bond to it from an account with
funds, and run a node that votes with it:

```sh
chainv4 keygen --out vote.key
# address ed25519:0x7c2e…
chainv4 send --key alice.key bond --voter ed25519:0x7c2e… --amount 100000000000000000000
chainv4 --peer … --validate --validator-key vote.key
```

The voting key only signs votes; the bond stays with the account that sent it, and only that
account may unbond or withdraw it. A validator bonded in epoch k votes from epoch
k + `activation_delay` + 1. A node votes `vote_depth` blocks after each checkpoint and remembers
every vote it signs, so it never contradicts itself, even across restarts. Run each voting key
on one node only: two nodes with the same key can sign conflicting votes, and lose the bond.

```sh
chainv4 send --key alice.key unbond --voter ed25519:0x7c2e…     # stops voting after activation_delay epochs
chainv4 send --key alice.key withdraw --voter ed25519:0x7c2e…   # withdrawal_delay epochs after that
```

Each vote in a justification earns its validator's account `vote_reward`, paid when a block
carries the justification; producers and miners add them on their own, and each epoch pays once.

### Slashing

Two votes by one key conflict when they target the same epoch, or when one surrounds the other.
Nodes keep any they see, log them with the command to report them, and list them at
`GET /evidence`. Anyone may report them:

```sh
chainv4 send slash --first 0x… --second 0x…
```

The validator's whole bond is gone at once, and it leaves the validator set the way an unbond
does, `activation_delay` epochs on. The reporter gets 1/`reporter_divisor` of the bond, and the
rest is burned.

### Authority finality

The same votes, from the keys in `finality.signers`, each counting once; `threshold` of them
voting for a link justify it. The threshold must be more than half the signers, so two
conflicting checkpoints can only both be finalized if a signer voted against itself. Each signer
runs a node with `--validate --validator-key` its key. There is no bonding or slashing: staking
transactions fail, and conflicting votes are only logged and listed at `GET /evidence`.
`vote_reward`, 0 in the dev settings, pays each signer's own address.

The signers change by their own approval. Each approving signer sends the whole new set, made
from the decided one, signed with its key:

```sh
chainv4 send --key signer.key approve-signers --add ed25519:0x7c2e… --threshold 2 --change-threshold 2
chainv4 send --key signer.key approve-signers --remove ed25519:0x41f0…
```

Once `change_threshold` of the decided signers approve the same set, it is decided; the
checkpoint after that fixes it for epochs `activation_delay` on, as FFG fixes its validator sets,
so a change decided in epoch k votes from epoch k + `activation_delay` + 1. Both thresholds must
be more than half the new set, and it may hold at most `max_signers`. An approval names the
version of the set it changes, which `send` fills in, so approvals for a set that has changed
since never count, and a signer that approves a different set moves its approval there.

An approval counts for `approval_epochs` epochs, 64 in the dev settings, so a change is only
decided by approvals that are recent; approving again starts the clock afresh. A signer can also
take its approval back:

```sh
chainv4 send --key signer.key withdraw-approval
```

`GET /signers` shows who votes now, the decided set, and the approvals that still count, with
the last epoch each counts in.

### Depth finality

No votes: a main-chain block is final once `depth` blocks are on top of it, and the node never
reorganizes past it. `depth` 0 makes every block final as soon as it is the tip. That is each
node's own view, so nodes kept apart for longer than `depth` blocks, by a partition or a private
fork, finalize different branches and never come back together. `--validate` is refused.

## Fees

On a chain with fees, every transaction pays for the gas it uses, failed ones too. It names a
gas limit and the most it pays per gas; that much is reserved from the sender before it runs,
and what it does not use comes back. A contract's fuel costs gas, so the gas limit also bounds
what the contract may burn, as well as `tx_limits.fuel`; one that runs out pays all of the fuel
it was allowed. A transaction whose gas limit does not cover its intrinsic cost, whose price is
too low, or whose sender cannot cover its gas limit at its price, cannot go in a block at all.

Genesis's `fees` sets every part of it, and `null` makes a chain free:

| Field | What it does |
| --- | --- |
| `block_gas_limit` | The most gas a block's transactions may use together. Producers fill blocks by priority until it is reached. |
| `burn_permille` | How much of the base part of each fee is burned, in thousandths; the producer gets the rest of it. |
| `schedule` | The gas schedule, or scorer: how much gas a transaction costs. |
| `pricing` | What each gas costs. |

Two gas schedules ship, and each is an implementation of the `GasSchedule` trait, so others can
be added the same way:

- **`weighted`**: `per_tx` gas for every transaction, `per_byte` for each byte of it, `per_code_byte`
  more for each byte of code a deploy or upgrade stores, and one gas for every `fuel_per_gas` fuel.
- **`contract`**: weighs transactions the same way, then calls `score` on the contract at
  `contract`, with `fuel` of its own, to say what the sender is charged. It is called as
  `score(sender, kind, target, bytes, fuel, gas, failed)` and returns the gas to charge: `kind`
  is the transaction's type as the API names it, `target` what it is sent to, and `gas` what the
  weights say. It can make transactions cheaper or dearer by anything it can read, up to their
  gas limit. Nothing it writes or emits is kept, and until a contract lives there, or when it
  fails, the weights alone count. Blocks always count the weighed gas, whatever it charges.

```lua
local M = {}

-- Transfers at half price; everything else as weighed.
function M.score(sender, kind, target, bytes, fuel, gas, failed)
    if kind == "transfer" then
        return gas / 2
    end
    return gas
end

return M
```

Two pricings ship, each an implementation of the `Pricing` trait:

- **`base_fee`**, as in EIP-1559: every block has a base price, set by how full its parent was
  against a target of `block_gas_limit / elasticity`, moving by at most `1 / change_denominator`
  a block and never below `min`. A transaction pays the base price plus its tip, as far as its max
  price allows.
- **`bid`**: each sender bids its price, at least `min_price`, and pays it whole. `min_price` is
  every block's base price.

Either way, the base price times the gas a transaction is charged for is the base part of its
fee, which `burn_permille` splits between burning and the producer; whatever it pays above the
base price goes to the producer whole. `GET /status` shows the fees and the base price a
transaction sent now must offer, and every block and receipt shows its gas and fee.

## HTTP API

Start a node with `--http ADDR`. Every answer is JSON; errors are `{"error": "…"}`.

| Route | Answers |
| --- | --- |
| `GET /status` | Genesis, tip, height, difficulty, total work, the chain's hash and coin, the epoch with the justified and finalized checkpoints, and the fees with the next block's base price. |
| `GET /blocks?before=H&limit=N` | Block summaries, newest first, with a `next` cursor. |
| `GET /blocks/{latest\|height\|id}` | A block, its gas used and base price, its seal, and each transaction with its fee offer and receipt. |
| `GET /txs/{id}` | Where a transaction landed, its confirmations, whether it is final, its gas used and fee, its result, events and trace of every call its contracts made; 404 until it is on the main chain. |
| `GET /accounts/{address}` | Nonce and balance. |
| `GET /accounts/{address}/txs` | The transactions that touched an address, newest first and paged. On nodes started with `--index-addresses`. |
| `GET /contracts/{address}` | Engine, deployer, exported functions, whether it can be upgraded, its storage layout, and what the token registry holds for it. |
| `GET /proofs/{column}/{key}` | A proof of one chain-state entry, or that there is none, against the tip's `state_root`, or a recent block's with `?block=`. |
| `GET /proofs/receipts/{id}` | A proof that a transaction's receipt is in its block's `receipts_root`, and the signed transaction in its `tx_root`. |
| `GET /light` | What a light client starts from: the chain's light parameters and genesis header, encoded. |
| `GET /headers?from=H&count=N` | Main-chain headers with their seals, encoded. |
| `GET /light/checkpoint/{block}` | What a light client starts from at a checkpoint rather than genesis; without `{block}`, the node's finalized one. |
| `GET /light/finality?from=E` | Justifications and light updates, 64 epochs at a time, for light clients to follow finality. |
| `GET /light/updates/{block}` | Proofs of the voters genesis or a checkpoint fixed, for light clients to follow finality. |
| `GET /justifications?from=E` | Justifications for main-chain checkpoints, encoded. |
| `GET /validators` | Every bonded validator, and who votes in the current epoch with what weight: the bonded set under `ffg`, the signers under `authority`; 404 under `depth`. |
| `GET /validators/{voter}` | One validator by voting key: its account, bond, the epochs it votes in, and whether it was slashed. |
| `GET /signers` | Under `authority`: who votes now, the set decided last and its version, and the approvals so far for changing it. |
| `GET /evidence` | Conflicting votes this node has seen, each encoded ready for `send slash`. |
| `POST /txs` | `{"tx": "0x…"}`, an encoded signed transaction; answers 202 with its id. |
| `POST /offer` | The same body, signed with any offer; answers the fee offer that covers it now. |
| `POST /call` | Runs a contract function read-only: `contract`, `function` and `args`; nothing it does is kept. |
| `GET /forks` | The side branches the node stores, back to where each meets the main chain. |
| `GET /node` | The node's peers, roles, sync and what it counted since it started. |
| `GET /mempool` | The transactions waiting for a block. |
| `GET /stream` | Server-sent events, from a node started with `--stream`: blocks, reorgs, finality, queued transactions and peers. |
| `GET /events` | Indexed events, newest first and paged: a contract's, a contract's of one name, or those that mention an address. On nodes started with `--index-events`. |
| `GET /tokens` | The token registry by symbol, paged, with a search by how a symbol starts. |

Amounts and large integers are decimal strings, hashes and bytes are `0x` hex, and addresses
are their kind and hex, such as `ed25519:0x…`, `secp256k1:0x…` or `contract:0x…`.
