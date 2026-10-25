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

**Tokens:** a contract that exports every function genesis names in `token_functions`, ERC-20's
six on the dev chains, goes in the token registry. Its name, symbol and decimals are asked once,
after `init`, and an answer that does not fit fails the deploy (see
[Token standard](token-standard.md#the-token-registry)).

## Calling

A `Tx::ContractCall` names a function by its selector: the XXH3-64 of its name. Two exports with
the same selector are refused at deploy. A call sends value from the sender to the contract
first; if the call fails, the value goes back with everything else.

**What persists:** every call runs in a fresh engine state: a new Lua state, or a new WebAssembly
instance. Nothing outside storage survives a call, so a node that restarts computes exactly
what one that never stopped does.

**Why fresh, and not one state per engine:** a single long-lived state would break consensus in
three ways:
- **Top-level locals would survive.** In Lua they live on as upvalues, so
  `local calls = 0; function M.count() calls += 1 end` would count across calls and
  transactions, differently on each node.
- **Memory limits would depend on history.** The memory limit counts the whole heap. In a reused
  state, the heap holds leftovers that depend on what that node ran before: garbage, interned
  strings, the garbage collector's pacing. A call near the limit could then fail on one node and
  pass on another.
- **Nested calls would share limits.** Each call in a chain of calls has its own fuel and memory
  limit; in one shared state they would share them.

**What makes fresh states cheap:**
- **Lua pool.** Building a state is most of what a Lua call costs. `LuaEngine` keeps a pool of
  pristine states:
  - they are built ahead on two threads of its own, started on its first call;
  - each is used for one call and then closed on a third thread;
  - every pristine state is built the same way, so a call gives the same result in any of them.

  If the pool runs dry, the call builds its own.
- **WebAssembly cache.** Compiling is nearly all of what a WebAssembly call costs. `WasmEngine`
  caches compiled modules by their code:
  - the same bytes always compile to the same module, so the cache never changes a result, only
    how fast it comes;
  - identical code at many addresses shares one entry;
  - an upgrade is new code, which compiles once and takes its own entry;
  - the module used longest ago leaves first, once 32 MiB or 1,024 modules are cached.

  Each call still gets a fresh instance, with fresh memory.

| Engine | A call without | With |
| --- | --- | --- |
| Lua | about 70 µs, mostly building and closing the state | about 20 µs |
| WebAssembly | about 300 µs, mostly compiling | about 35 µs |

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
3. check that the new code keeps the old code's [storage layout](#storage-layouts);
4. run the new code's `migrate`;
5. bring the token registry in line with what the new code exports, asking a token's name, symbol
   and decimals again.

If anything fails, nothing changes.

## Storage namespaces

A contract's storage has three parts, each with keys of its own: the same key in two of them is
two entries.

| Part | Lua | Rust SDK | Who may write it |
| --- | --- | --- | --- |
| Plain storage | `storage.get(key)`, `storage.set(key, value)` | `storage::get`, `set`, `delete` | Any code running on the contract. |
| A namespace | `storage.namespace(name):get(key)`, `:set(key, value)` | `storage::namespace(name).get(key)`, `.set`, `.delete` | Any code running on the contract, by its name. |
| Its own space | `storage.own:get(key)`, `:set(key, value)` | `storage::own().get(key)`, `.set`, `.delete` | Only the contract's own code. Code it delegates to may read it, never write it. |

- **Namespaces** keep pieces of code that share a contract apart, such as a module, a library of
  functions, or one version's layout. Names are 1 to 64 bytes, and code naming the same
  namespace shares it. This is ERC-7201's namespaced storage, except the chain keeps namespaces
  apart, rather than hashing.
- **The own space** holds what code a contract delegates to must never change: a proxy's owner and
  implementation. Delegated code that writes it fails, whatever key it picks: "a contract's own
  storage is written only by its own code, not by code it delegates to". The built-in upgrade
  keeps it, since the code changes but the contract does not.
- **Cost:** what plain storage costs, plus 1 fuel for every 16 bytes of the name.
- **In WebAssembly:** `namespace_get`, `namespace_set` and `namespace_delete` take the name
  first, and an empty name is the own space.

They are kept in the `Namespaces` column (see [Storage](storage.md#columns)).

## Storage layouts

Code may declare its storage layout: the namespaces it keeps its state in, and what each holds.
New code then has to keep it, so an upgrade or a proxy's new implementation cannot silently
start from empty storage because it renamed a namespace or a key, or misread a value because it
changed how one is encoded.

A namespace is one of two shapes:
- **A map:** any key, every value one kind. For state keyed by something, such as balances by
  address.
- **A record:** only the keys it declares, each with its own kind. For a contract's settings and
  counters.

```lua
--@namespace balances decimal        -- a map: any key, every value decimal text
--@namespace config                  -- a record: only the keys below
--@key config owner address-hex
--@key config paused bool
--@retired config fee_rate           -- a key no longer used
--@retired fees                      -- a namespace no longer used

local balances = storage.namespace("balances")
balances:set(who:hex(), tostring(amount))
storage.namespace("config"):set("owner", ctx.sender:hex())
```

```rust
namespace!(BALANCES, "balances", "u256");
namespace!(CONFIG, "config", { "owner": "address", "paused": "bool" });
retired!("config", "fee_rate");
retired!("fees");

BALANCES.set_u256(&who.encode(), amount);
CONFIG.set_address(b"owner", &sender());
```

Lua lines may sit anywhere in the script. Rust's macros write the module's `chainv4.layout`
custom section, check names, keys and kinds at compile time, and define a constant to use the
namespace by. Names are 1 to 64 bytes and keys 1 to 256, each one word.

**Kinds** are encodings, and the node checks every value written against its kind:

| Kind | Encoding | Written by |
| --- | --- | --- |
| `bytes` | Anything. | |
| `text` | UTF-8. | |
| `number` | A number in text: `5`, `-2.5`, `1e+20`, `inf`, `nan`. | Lua's `tostring(n)` |
| `decimal` | A whole number from 0 in decimal text, up to the largest u256. | Lua's `tostring(u256)` |
| `bool` | One byte, 0 or 1. | `set_bool` |
| `i64`, `u64` | 8 bytes, little-endian. | `set_i64`, `set_u64` |
| `u128` | 16 bytes, little-endian. | |
| `u256` | 32 bytes, little-endian. | `set_u256` |
| `address` | 33 bytes: the kind's tag, then its 32. | `set_address`, `Address::encode` |
| `address-text` | An address in text: `ed25519:0x…`. | Lua's `address:text()` |
| `address-hex` | `0x` and 64 hex digits. | Lua's `address:hex()` |

A kind that is not in the list is refused at deploy.

**What it enforces:**
- **Code that declares a layout reaches only what it declares.** Every one of these fails, naming
  what went wrong:
  - another namespace: "namespace elsewhere is not in this code's layout";
  - a key a record does not declare: "key onwer is not in namespace config's layout";
  - a value that is not its kind: "namespace balances, key alice: the value is not decimal".
- **Plain storage is read and clear only** for such code, since declared state lives in
  namespaces. A contract that starts declaring a layout can still read its older plain keys and
  delete them, so it can move them into namespaces: in `migrate`, or behind a proxy in an
  exported function called once. Its own space stays free. Under `delegate`, it is the running
  code's layout that counts. Code that declares nothing may use anything.
- **New code keeps the old layout.** Every namespace the old code declared must still be
  declared, with the same shape: a map with the same kind, a record with every key it had, each
  with the same kind. Anything may be retired instead. New namespaces and keys may come in. Code
  that declares nothing keeps nothing, so it cannot replace code that declares a layout.
- **Retiring is on purpose, and for good.** A retired namespace or key can be read and cleared,
  but not written, so `migrate` can move its state somewhere new. Later code must keep it
  retired. To change a kind, or turn a map into a record, retire it and declare another.

**Where it is checked:**
- **`Tx::Upgrade`** refuses new code that does not keep the layout: "the new code does not keep
  the storage layout: key owner of namespace config is dropped: keep it, or retire it".
- **Proxies** check before repointing, with `contracts.check_layout(current, new)` in Lua,
  `check_layout(&current, &new)` in the SDK and `check_layout` in WebAssembly. The example
  proxies do.
- **Before you send anything,** `chainv4 layout new.lua --from old.lua` compares two files, and
  `--from contract:0x…` compares against a deployed contract, such as a proxy's current
  implementation, over the API (`--node`). It loads the code as a node would, prints both
  layouts, and fails with the first namespace or key that breaks.

Contract records keep their layout, and `GET /contracts/{address}` shows it.

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
- **Delegate only to code you trust.** It has full access to the contract, except that it can
  only read the contract's own space.
- **Keep the proxy's keys in its own space,** as the examples do. Implementations share the
  proxy's plain storage and namespaces, but cannot change its own space, so no key they pick
  can change its owner or implementation (see [Storage namespaces](#storage-namespaces)).
- **Keep layouts in agreement:** each new implementation must still read the keys and namespaces
  of the one before. Declare a [layout](#storage-layouts) and have the proxy call
  `check_layout` before it repoints, as the examples do, and the namespaces are checked for you.
- **Forward with `ctx.args`.** A Lua proxy's fallback passes `ctx.args` on, and returns what
  `delegate` gives back, so every value keeps its type both ways (see
  [values lists](#values-lists)). Passing `...` instead converts them through Lua, so an `i64`
  comes back a number. A Rust proxy forwards the encoded values as they are.

## Events

A contract emits named events with typed data (`events.emit` in Lua, `emit` in WebAssembly).
An event lands in the transaction's receipt only if its call, and every call above it, succeeds.

| Limit | Value |
| --- | --- |
| Name | 1 to 64 bytes |
| Data, encoded | at most 16 KiB |
| Events per call | 256, not counting calls it makes |

## Traces

Every run a transaction or the chain starts records where it went, in the receipt's `trace`:
- a call;
- a deploy's `init`;
- an upgrade's `check_perms` and `migrate`.

**What it records:** a hop for the run itself at depth 0, and one for every call, delegate and
transfer under it, in the order they started (`structs::trace`). Each hop says:
- who made it and who it reached;
- the function, by name: a selector that names a function is shown as the function's name;
- the value it sent, and the fuel it burned with everything under it;
- why it failed, if it did;
- which of the receipt's events it emitted itself.

**Failures:**
- **Failed hops are kept.** A call that failed shows where it failed, even when its caller
  caught the error and went on.
- **Their events are not.** A failed hop's events were dropped with it, so it and every hop
  under it point at no events. A whole transaction that failed keeps its trace, with no events.

**What it is not:**
- **Not consensus:** nothing hashes the trace, and recording it costs no fuel.
- **Kept small:** a run keeps at most 256 hops (`MAX_HOPS`). Past that, hops are only counted in
  `dropped`, so a contract looping over transfers cannot make its receipt huge.
- **Kept for failed hooks too:** a deploy or upgrade whose `init`, `check_perms` or `migrate`
  fails keeps the trace up to where it failed.

The [explorer](explorer.md) draws traces as a call tree and as a graph.

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
| Loading code at deploy or upgrade | 1 per byte of it, plus a Lua script's top level as it runs |
| Asking a token's name, symbol and decimals at deploy or upgrade | What those functions burn |
| Asking for that again, with `tokens.refresh` or `refresh_token` | 100, and what the questions burn once the call succeeds |

**Loading code** is paid for by the deploy or upgrade, and shows in its receipt's `fuel_used`:
- **Compiling:** every byte costs 1 fuel (`cost::LOAD_BYTE`), as compiling and checking the code
  is real work on every node: up to about 131,000 fuel for a full Lua script, 524,000 for a full
  module.
- **The top level:** a Lua script's top level runs once while it loads, to find its exports, and
  burns fuel like any code. WebAssembly modules run nothing while they load: start functions are
  refused.
- **Failing still pays:** a deploy or upgrade that fails pays for what it burned before it did,
  loading included. One that runs out of fuel, while loading or in a hook, pays for all of it.
- **Every call pays for the top level too.** Each call runs the script's top level again in its
  fresh state, so the same work is charged again. It runs before the call is attached, so it
  sees nothing of the call: `ctx`, `storage`, `contracts`, `accounts`, `events` and `tokens` are
  not there yet. It is the same code doing the same work every time.

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
| `ctx` | `ctx.sender`, `ctx.contract`, `ctx.value`, and `ctx.args`, the call's arguments as a values list. |
| `storage` | `storage.get(key)`, `storage.set(key, value)`; `nil` deletes. Keys and values are strings. `storage.namespace(name)` and `storage.own` have the same `get` and `set`, as methods, on keys of their own. |
| `accounts` | `accounts.balance(address)`, `accounts.transfer(to, amount)` from the contract's balance. |
| `contracts` | `contracts.call(address, name, ...)`, `contracts.call_value(address, value, name, ...)`, `contracts.delegate(address, name_or_selector, ...)`, `contracts.check_layout(from, to)`, `contracts.selector(name)`. |
| `events` | `events.emit(name, ...)`. |
| `tokens` | `tokens.refresh()`: asks the token registry to ask this token's name, symbol and decimals again once the transaction succeeds. |
| `address` | `address.parse("ed25519:0x…")`, `address.from_hex`, `address.from_bytes`; methods `hex`, `bytes`, `text`, `kind`, `is_contract`, `is_valid`. |
| `u256` | `u256(value)` or `u256.new`, `u256.zero`, `u256.max`; arithmetic, comparisons and `hex`. |
| `hash` | `hash.sha256`, `blake3`, `blake2s`, `blake2b`, `xxh3`, `keccak256`, whichever the node has; the result has `hex`, `bytes`, `algorithm`. |

**How values cross into Lua:**
- unsigned integers and amounts arrive as `u256`, so nothing is rounded;
- signed integers arrive as numbers and must fit one exactly, within 2^53;
- addresses arrive as `address` values;
- `nil` arrives as `nil`.

**How values come back:**
- numbers come back as numbers;
- `u256` values come back as `u256`;
- strings come back as strings, or bytes when they are not UTF-8;
- `nil` comes back as `nil`, so a function may return it, alone or among other values. A
  function that returns nothing returns no values at all.

### Values lists

Lua has no `i64`, no `u64` and no bytes of its own, so values converted into Lua and back can
change type:

| Arrives as | In Lua | Goes back out as |
| --- | --- | --- |
| `i64`, `i128` | a number | `number` |
| `u64`, `u128`, `amount` | a `u256` | `u256` |
| `bytes` that read as UTF-8 | a string | `string` |

**Keeping types:** a values list keeps values exactly as they came. `ctx.args` is the call's
arguments as one, without the selector a fallback was given.
- **Passing one on:** give it to `contracts.call`, `contracts.call_value`, `contracts.delegate`
  or `events.emit` in place of loose values.
- **Results:** a call or delegate given a values list returns its results as one too, untouched.
- **Returning one:** a function that returns one returns its values as they are.

A list goes alone, never among other values.

**Looking inside:** `#list` counts its values, `list:type(i)` names the i-th one's type, and
`list:unpack()` converts them to Lua values the usual way.

**Memory:** a list keeps its values encoded in a Lua string, so it counts against the call's
memory like any other value.

```lua
-- A router: every call it does not export goes on to a target, as it came, both ways.
function M.fallback(selector)
    return contracts.call(target(), "handle", ctx.args)
end
```

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
| `namespace_get`, `namespace_set`, `namespace_delete` | A namespace, by its name first, or for an empty name the contract's own space. |
| `sender`, `contract`, `value` | The call's context. |
| `hash` | Hashes bytes with a chosen algorithm. |
| `balance`, `transfer` | Funds, from the contract's own balance. |
| `call`, `delegate` | Other contracts: `call(address, value, name, name_len, args, args_len)`. |
| `check_layout` | Whether one contract's code keeps another's storage layout. |
| `emit` | An event. |
| `refresh_token` | Asks the token registry to ask this token's name, symbol and decimals again once the transaction succeeds. |
| `abort` | Stops the call with an error. Only the first 1,024 bytes of the message are kept. |

**The encoding:** arguments and return values travel as the chain's encoding of a list of
`TxCallArgumentType`.

**Limits:** modules recurse at most 1,000 deep and hold at most a million values on the stack.
wasmi is pinned to `=2.0.0`: its version, features and limits are consensus.

### The Rust SDK

`contracts/sdk` writes WebAssembly contracts in Rust, as `cdylib`s built for
`wasm32-unknown-unknown`.

```rust
use chainv4_sdk::{Address, U256, export, sender};

#[export]
fn transfer(to: Address, amount: U256) -> Result<bool, &'static str> {
    let from = sender();
    // move the balance in storage, failing with "insufficient balance", then:
    Ok(true)
}
```

**Typed exports:** an exported function's parameters are read from the call's arguments, in
order, and what it returns becomes the call's return values (`chainv4_sdk::args`).
- **Parameters** may be any `FromValue` type: `Address`, `U256`, `bool`, `String`, `Vec<u8>`,
  `i64`, `u64`, `u32`, `u8`, `i128`, `u128`, a `Value` as it came, or an `Option` of any of
  them. Whole numbers arrive as any integer kind, and as Lua's floats when whole.
- **Refused arguments:** a call with too many arguments, one left out, or one of the wrong kind
  aborts, naming the parameter: `amount: expected an amount`, `amount: missing`,
  `transfer takes 2 arguments, not 3`. An `Option` parameter may be left out or passed nil.
- **Results** may be nothing, any `IntoValue` (the same types; `None` returns nil), a tuple of up
  to four, a `Vec<Value>`, or a `Result` whose error aborts the call with its message.
- **By hand:** a function that takes and returns nothing is exported as it is, and may read its
  arguments with `input()` and set its return values with `output()`, as a proxy's `fallback`
  does.

**What else it offers:**
- `#[export(reentrant)]` also marks a function reentrant;
- `sender`, `this`, `value`, `balance`, `transfer`;
- `call` and `call_with_value`, `delegate`, `check_layout`, `emit`, `hash`, `abort`,
  `refresh_token`, and `storage`, with `storage::namespace(name)` and `storage::own()`;
- `namespace!` and `retired!`, which declare the storage layout, and typed methods on a
  namespace that write each kind's encoding: `set_i64`, `set_u64`, `set_u256`, `set_address`,
  `set_bool`, and their `get_` twins.

**Numbers:** Lua numbers reach a WebAssembly contract as float bits, and floats are refused in
modules. So `Value::as_i64` and `Value::as_amount` turn whole numbers into integers using integer
instructions only.

**Building:** build from `contracts/`, whose `.cargo/config.toml` sets the wasm32 target. From
the repo root, pass `--target wasm32-unknown-unknown`.

## Examples

| Example | Shows |
| --- | --- |
| `contracts/examples/counter` | Storage, arguments and return values, and a storage layout. |
| `contracts/examples/vault` | Holding and sending funds. |
| `contracts/examples/proxy` | Calling other contracts for a caller, and a reentrant callback. |
| `contracts/examples/upgradable`, `contracts/lua/proxy.lua` | A proxy built on `fallback` and `delegate`, with its owner and implementation in its own space, that checks a new implementation's layout. |
| `contracts/examples/token`, `contracts/lua/token.lua` | The [token standard](token-standard.md), held to it by one shared test. |
| `contracts/lua/guestbook.lua` | Events with addresses in them, for the event index and the explorer; value passed on with `accounts.transfer`; and a storage layout of a map and a record. |

## What to keep in mind

- **Keep top levels light.** A Lua script's top level runs on every call, before the call is
  attached, and every call pays for it.
- **Read-only calls go through the API.** `POST /call` runs a function on a throwaway copy of the
  committed state (see [HTTP API](api.md)); from a contract, every call is part of a transaction.
- **A contract cannot be removed.**
