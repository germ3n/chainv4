# chainv4

A blockchain node, and a command-line client for it, in one binary. Contracts run on Lua
(Luau) or WebAssembly; blocks come from proof of work or a development authority; nodes talk
over libp2p; a JSON HTTP API serves queries and takes transactions.

This page covers the `chainv4` command and how contracts live on the chain. The code reference
is the wiki that every build writes to `target/wiki/index.html`, tokens follow the
[token standard](docs/token-standard.md), and open questions and caveats live in
[docs/todo.md](docs/todo.md).

## Building

```sh
cargo build --release
./target/release/chainv4 --help
```

You need stable Rust, a C/C++ toolchain and clang: RocksDB and Luau are built from source.
Everything is on by default; each piece sits behind a cargo feature (`lvm` and `wasm` for the
contract engines, `node`, `http` for the API, `client` for `chainv4 send`, one per hash and key
scheme) if you want a smaller build.

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
| `--consensus pow\|authority` | `pow` | Which development chain to run. |
| `--mine` | | Mine blocks (pow). |
| `--reward-to ADDR` | dev key | Where mining rewards go. |
| `--mine-threads N` | `1` | How many threads to mine on. |
| `--produce` | | Produce blocks with the development key (authority). |
| `--http ADDR` | | Serve the HTTP API, such as `127.0.0.1:8080`. |

Logs go to stderr; set `RUST_LOG`, such as `RUST_LOG=debug`, for more.

### The two dev chains

- **pow** (default): anyone may mine. Blocks aim for one every two seconds and difficulty
  follows. A block mints 50 coins for its miner, halving every 210,000 blocks. The heaviest
  chain wins, so forks are normal; a node reorganizes to a branch that holds more work, up to
  100 blocks back.
- **authority**: the development key produces a block every two seconds, with `--produce` on
  exactly one node. No rewards, no forks. Handy for tests and demos.

Both give the development key a million coins and let it mint. Its seed is in the source, so
never use either chain for anything of value. A dev chain also allows every contract engine the
node was built with, so nodes built with different features are on different chains.

### Several nodes

```sh
chainv4 --db a.db --mine --listen /ip4/127.0.0.1/udp/4001/quic-v1 --http 127.0.0.1:8080
chainv4 --db b.db --mine --listen /ip4/127.0.0.1/udp/4002/quic-v1 --peer /ip4/127.0.0.1/udp/4001/quic-v1
chainv4 --db c.db --peer /ip4/127.0.0.1/udp/4001/quic-v1 --http 127.0.0.1:8081
```

A node that starts late, or falls behind, catches up from its peers on its own.

A database belongs to one chain: a node refuses to open a `chain.db` written for a different
genesis. Delete it, or point `--db` elsewhere, after switching `--consensus` or features.

## Keys

```sh
chainv4 keygen [--scheme ed25519|edblake] [--out PATH]
```

Writes a new key to `PATH` (default `chainv4.key`) and prints its address. The file holds one
line, `ed25519:0x…`, the scheme and the 32-byte seed in hex. It is created readable by you
only and is never overwritten, but it is not encrypted: guard it.

## Sending transactions

```sh
chainv4 send [OPTIONS] <transfer|mint|burn|deploy|upgrade|call> …
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

### transfer, mint, burn

```sh
chainv4 send transfer --to ed25519:0x… --amount 250
chainv4 send mint --to ed25519:0x… --amount 1000   # only the chain's minter, the dev key here
chainv4 send --key alice.key burn --amount 5
```

### deploy

```sh
chainv4 send deploy --lua counter.lua
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
may: see below. `--wasm` works too, so a contract can even move between engines.

### call

```sh
chainv4 send call --to contract:0x41f0… --function increment i64:5
# included in block 21 0x… (1 confirmation)
# returned [{"type":"number","value":5.0}]
# fuel used 4821
# event Incremented from contract:0x41f0… [{"type":"number","value":5.0},{"type":"address","value":"ed25519:0x…"}]
```

`--amount N` sends coins along with the call; the contract sees them as `ctx.value`.

Arguments are `type:value`:

| Type | Example |
| --- | --- |
| `i64`, `u64`, `i128`, `u128`, `u256` | `i64:-5`, `u256:1000` |
| `number` | `number:1.5` |
| `amount` | `amount:1000000000000000000` |
| `bool` | `bool:true` |
| `string` | `string:hello` |
| `bytes` | `bytes:0x00ff` |
| `address` | `address:ed25519:0x…`, `address:contract:0x…` |

## Contract lifecycle

Besides the functions a contract exports for everyone, the chain looks for three by name:

| Function | Who runs it | When |
| --- | --- | --- |
| `init()` | the chain only | Once, at deploy, with the deployer as `ctx.sender`. Record an owner, mint an initial supply. If it fails, so does the deploy. |
| `check_perms(permission, who)` | the chain, before a privileged action; anyone may call it | Before an upgrade, with `"upgrade"` and the sender. Only `true` lets the upgrade through. What it writes is thrown away. |
| `migrate()` | the chain only | Once, after an upgrade to code that exports it, with the upgrader as sender. Move storage to a new layout. If it fails, so does the upgrade. |

Transactions and other contracts cannot call `init` or `migrate`. A contract without
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

## Tokens

Tokens are contracts that follow the [token standard](docs/token-standard.md), much like ERC-20:
`transfer`, `approve`, `transfer_from`, `balance_of` and the rest, with `Transfer` and
`Approval` events. They never touch the native coin, which moves only with `--amount` and
`accounts.transfer`. Deploy one of the examples and use it:

```sh
chainv4 send deploy --lua contracts/lua/token.lua
# contract contract:0x9a1c…
chainv4 send call --to contract:0x9a1c… --function transfer address:ed25519:0x5be0… u256:1000
# event Transfer from contract:0x9a1c… [{"type":"address",…},{"type":"address",…},{"type":"u256","value":"1000"}]
```

The deployer owns the example token: `init` gives them a million tokens, and only they may
`mint` more or upgrade it.

## HTTP API

Start a node with `--http ADDR`. Every answer is JSON; errors are `{"error": "…"}`.

| Route | Answers |
| --- | --- |
| `GET /status` | Genesis, tip, height, difficulty and total work. |
| `GET /blocks?before=H&limit=N` | Block summaries, newest first, with a `next` cursor. |
| `GET /blocks/{latest\|height\|id}` | A block, its seal, and each transaction with its receipt. |
| `GET /txs/{id}` | Where a transaction landed, its confirmations, result and events; 404 until it is on the main chain. |
| `GET /accounts/{address}` | Nonce and balance. |
| `GET /contracts/{address}` | Engine, deployer, exported functions, and whether it can be upgraded. |
| `POST /txs` | `{"tx": "0x…"}`, an encoded signed transaction; answers 202 with its id. |

Amounts and large integers are decimal strings, hashes and bytes are `0x` hex, and addresses
are their kind and hex, such as `ed25519:0x…` or `contract:0x…`.
