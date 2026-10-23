# Contracts

Contracts are Luau scripts or WebAssembly modules (`src/vm/`), each run by an engine behind
`ScriptEngine`. Genesis's `engines` says which kinds a chain allows, and every node on it must be
built with those engines.

## Deploying

`Tx::Deploy` carries the code itself, and every node checks and loads it the same way:

- **Luau source,** at most 128 KiB, compiled by every node with the chain's fixed settings:
  optimization level 1, debug level 1, no type information. Contracts are deployed as source,
  never bytecode, because Luau does not check bytecode and loading someone else's can corrupt
  memory. Loading runs the script's top level once. It must return a table of functions, and
  those are its exports.
- **A WebAssembly module,** at most 512 KiB:
  - it may only import from `chain`, the host functions below, and must export its memory as
    `memory`;
  - each export that takes and returns nothing is a callable function;
  - floating-point instructions, SIMD and start functions are refused, since floats are the one
    part of WebAssembly machines may disagree on.

A contract lands at an address derived from the deployer and its nonce, hashed with the chain
hash under the domain `"chainv4 contract address"`. The same deployer and nonce always give the
same address on a chain, and no key controls it. `chainv4 send deploy` prints the address before
the deploy lands.

## Calling

A `Tx::ContractCall` names a function by its selector: the XXH3-64 of its name. Two exports with
the same selector are refused at deploy. A call sends value from the sender to the contract
first; if the call fails, the value goes back with everything else.

**What persists:** every call runs in a fresh engine state: a new Lua state, or a new WebAssembly
instance. Nothing outside storage survives a call, so a node that restarts computes exactly
what one that never stopped does.

**Calls between contracts** (`contracts.call` in Lua, `call` in WebAssembly) reach any contract,
whichever engine runs it.
- **The callee's context:** it sees the caller as its sender, and the value it was sent, which
  comes from the caller's own balance.
- **On failure:** its writes and its value are kept only if it succeeds. A failed callee leaves
  nothing behind, and the caller sees the error.
- **Limits:** calls nest at most 8 deep. A callee burns from the fuel its caller has left, and
  each call in a chain of calls gets its own memory limit, `tx_limits.memory`. The worst case is
  8 × 32 MiB in one transaction on the dev chains.

## Reentrancy

A function can only run while its contract is already running further up the chain of calls if
it is marked:
- in Lua, a `--@reentrant` line directly above `function M.name(`;
- in Rust, `#[export(reentrant)]`, which writes the module's `chainv4.reentrant` section.

Anything else refuses with `Reentrant`. Lua annotations are read line by line, so one inside a
multi-line string counts too.

## Lifecycle hooks

Four function names mean something to the chain.

| Function | When it runs |
| --- | --- |
| `init` | Once at deploy, with the deployer as sender and no arguments. Record an owner or mint a supply here. If it fails, the deploy fails. |
| `check_perms(permission, who)` | Asked before a privileged action, so far only `"upgrade"`. Only a single `true` lets it through, and what it writes is thrown away. A contract without it can never be upgraded. |
| `migrate` | Once after an upgrade to code that exports it, with the upgrader as sender. Move storage to a new layout here. If it fails, the upgrade fails. |
| `fallback(selector, ...)` | When a call names a function the contract does not export. It gets the selector as a `u64` in front of the call's arguments, and answers in its place. |

Transactions and other contracts cannot call `init`, `migrate` or `fallback`, nor delegate to
them (`Reserved`).

**Upgrades** (`Tx::Upgrade`, `chainv4 send upgrade`) replace a contract's code but keep its
address, storage and balance. They:
1. ask its `check_perms("upgrade", sender)`;
2. load the new code;
3. run the new code's `migrate`.

If anything fails, nothing changes.

## Delegate calls and proxies

`contracts.delegate` in Lua, and `delegate` in WebAssembly, run another contract's code as this
contract's own:
- on this contract's storage and balance;
- under this call's sender and value;
- with events in this contract's name.

It is the EVM's `delegatecall`. The target must export the function itself; neither its fallback
nor a hook is ever run this way.

A proxy forwards everything to an implementation with `fallback` and `delegate`. It is then
upgradable without the built-in upgrade, by changing which implementation it points at. The
README's "Proxies" section has a worked example. Keep in mind:
- **Delegate only to code you trust.** It has full access to the contract.
- **Keep storage layouts apart:** the proxy's keys and the implementation's live in the same
  storage.
- **Lua proxies pass values as Lua values,** so an `i64` comes back as a number. A Rust proxy
  forwards values exactly.

## Events

A contract emits named events with typed data (`events.emit` in Lua, `emit` in WebAssembly).
An event lands in the transaction's receipt only if its call, and every call above it, succeeds.

| Limit | Value |
| --- | --- |
| Name | 1 to 64 bytes |
| Data, encoded | at most 16 KiB |
| Events per call | 256, not counting calls it makes |

## Fuel

Every engine meters fuel: Luau per instruction, in the forked Luau, and wasmi per instruction.
Host operations cost the same in both, so moving a contract to the other engine never changes
what its storage or calls cost (`vm::cost`).

| Operation | Fuel |
| --- | --- |
| Storage read | 100, plus 1 per 16 bytes of key and value |
| Storage write or delete | 500, plus 1 per 16 bytes of key and value |
| Hash | 20, plus 1 per 64 bytes hashed |
| Call another contract | 1,000, on top of what the callee burns, and 1,000 more when it sends value |
| Balance | 100 |
| Transfer | 1,000 |
| Event | 200, plus 1 per 16 bytes of name and data |
| Copying between a module's memory and the host | 1 per 64 bytes |

A call that runs out of fuel fails with `OutOfFuel`. On a chain with fees, fuel costs gas, and the
gas limit bounds the fuel too (see [Fees](fees.md)).

Storage keys are at most 256 bytes and values at most 64 KiB.

## Lua contracts

```lua
local M = {}

function M.init()
    storage.set("owner", ctx.sender:hex())
end

function M.increment(by)
    local count = tonumber(storage.get("count") or "0") + by
    storage.set("count", tostring(count))
    events.emit("Incremented", count, ctx.sender)
    return count
end

return M
```

**The sandbox** has only:
- the globals that behave the same on every machine: `assert`, `error`, `pairs`, `pcall`,
  `select`, `tostring`, `type`, `string`, `table`, `utf8`, `bit32` and the like;
- the deterministic part of `math`: `abs`, `floor`, `ceil`, `max`, `min`, `sqrt` and so on;
- the chain's libraries.

Nothing reads the clock, the platform or the garbage collector, and nothing gives random
numbers.

| Library | What it has |
| --- | --- |
| `ctx` | `ctx.sender`, `ctx.contract`, `ctx.value`. |
| `storage` | `storage.get(key)`, `storage.set(key, value)`; `nil` deletes. Keys and values are strings. |
| `accounts` | `accounts.balance(address)`, `accounts.transfer(to, amount)` from the contract's balance. |
| `contracts` | `contracts.call(address, name, ...)`, `contracts.call_value(address, value, name, ...)`, `contracts.delegate(address, name_or_selector, ...)`, `contracts.selector(name)`. |
| `events` | `events.emit(name, ...)`. |
| `address` | `address.parse("ed25519:0x…")`, `address.from_hex`, `address.from_bytes`; methods `hex`, `bytes`, `text`, `kind`, `is_contract`, `is_valid`. |
| `u256` | `u256(value)` or `u256.new`, `u256.zero`, `u256.max`; arithmetic, comparisons and `hex`. |
| `hash` | `hash.sha256`, `blake3`, `blake2s`, `blake2b`, `xxh3`, `keccak256`, whichever the node has; the result has `hex`, `bytes`, `algorithm`. |

**How values cross into Lua:**
- unsigned integers and amounts arrive as `u256`, so nothing is rounded;
- signed integers arrive as numbers and must fit one exactly, within 2^53;
- addresses arrive as `address` values.

**How values come back:**
- numbers come back as numbers;
- `u256` values come back as `u256`;
- strings come back as strings, or bytes when they are not UTF-8.

The full API is in the wiki under Lua.

## WebAssembly contracts

**The host functions:** a module imports them from `chain`, with pointers and lengths as `i32`s
into its exported memory.

| Function | Does |
| --- | --- |
| `input_len`, `input_read` | The call's arguments, encoded. |
| `output` | Sets what the call returns; the last one wins. |
| `result_len`, `result_read` | What the last `storage_get` or `call` left in the result buffer. |
| `storage_get`, `storage_set`, `storage_delete` | The contract's own storage. |
| `sender`, `contract`, `value` | The call's context. |
| `hash` | Hashes bytes with a chosen algorithm. |
| `balance`, `transfer` | Funds, from the contract's own balance. |
| `call`, `delegate` | Other contracts: `call(address, value, name, name_len, args, args_len)`. |
| `emit` | An event. |
| `abort` | Stops the call with an error. Only the first 1,024 bytes of the message are kept. |

**The encoding:** arguments and return values travel as the chain's encoding of a list of
`TxCallArgumentType`.

**Limits:** modules recurse at most 1,000 deep and hold at most a million values on the stack.
wasmi is pinned to `=2.0.0`: its version, features and limits are consensus.

### The Rust SDK

`contracts/sdk` writes WebAssembly contracts in Rust, as `cdylib`s built for
`wasm32-unknown-unknown`.

```rust
use chainv4_sdk::{Value, export, input, output, storage};

#[export]
fn increment() {
    let by = input().first().and_then(Value::as_i64).unwrap_or(1);
    let count = storage::get_i64(b"count").unwrap_or(0) + by;
    storage::set(b"count", &count.to_le_bytes());
    output(&[Value::I64(count)]);
}
```

**What it offers:**
- `#[export]` makes a function callable by name; `#[export(reentrant)]` also marks it reentrant;
- `input` and `output`, `sender`, `this`, `value`, `balance`, `transfer`;
- `call` and `call_with_value`, `delegate`, `emit`, `hash`, `abort`, and `storage`.

**Numbers:** Lua numbers reach a WebAssembly contract as float bits, and floats are refused in
modules. So `Value::as_i64` and `Value::as_amount` turn whole numbers into integers using integer
instructions only.

**Building:** build from `contracts/`, whose `.cargo/config.toml` sets the wasm32 target. From
the repo root, pass `--target wasm32-unknown-unknown`.

## Examples

| Example | Shows |
| --- | --- |
| `contracts/examples/counter` | Storage, arguments and return values. |
| `contracts/examples/vault` | Holding and sending funds. |
| `contracts/examples/upgradable` | `check_perms` and `migrate`. |
| `contracts/examples/proxy`, `contracts/lua/proxy.lua` | A proxy built on `fallback` and `delegate`. |
| `contracts/examples/token`, `contracts/lua/token.lua` | The [token standard](token-standard.md), held to it by one shared test. |

## What to keep in mind

- **Loading code is not charged.** Compiling and validating are bounded only by the size limits,
  and a Lua script's top level burns fuel no receipt shows.
- **There are no read-only calls.** Every call is a transaction.
- **Modules are not cached.** A WebAssembly module is parsed and compiled again on every call.
- **A contract cannot be removed.**
