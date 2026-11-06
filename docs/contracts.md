# Contracts

Contracts are Luau scripts or WebAssembly modules (`src/vm/`), each run by an engine behind
`ScriptEngine`. Genesis's `engines` says which kinds a chain allows, and every node on it must be
built with those engines. EVM contracts, Solidity's and Vyper's, run on revm and have
[a page of their own](evm.md).

The wallet's [Contracts page](wallet.md#contracts) reads, calls and deploys them, as it does EVM
contracts.

## Deploying

`Tx::Deploy` carries the code itself, and every node checks and loads it the same way:

- **Luau source,** at most 128 KiB, compiled by every node with the chain's fixed settings:
  optimization level 1, debug level 1, no type information. Contracts are deployed as source,
  never bytecode, because Luau does not check bytecode and loading someone else's can corrupt
  memory. Loading runs the script's top level once. It must return a table of functions, and
  those are its exports. A contract split across files is put together before it is sent (see
  [Splitting a contract across files](#splitting-a-contract-across-files)), so the chain holds
  one source.
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
- **Memory limits would depend on history.** In a reused state, the heap holds leftovers that
  depend on what that node ran before: garbage, interned strings, where the garbage collector is
  in its cycle. The collector frees that garbage while the next call runs, so a call near the
  limit could fail on one node and pass on another (see [Memory](#memory)).
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

[Contract storage](contract-storage.md) walks through this section and the next with examples.

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

## Iterating storage

Each part's keys can be gone through in byte order, each with its value, as the call's own writes
leave them: a key set earlier in the call is there, one deleted is not.

| | Lua | Rust SDK | WebAssembly |
| --- | --- | --- | --- |
| Plain storage | `for key, value in storage.iter(prefix, start) do` | `storage::iter(prefix)`, `storage::scan(prefix, start, limit)` | `storage_scan` |
| A namespace, or the own space | `ns:iter(prefix, start)` | `ns.iter(prefix)`, `ns.scan(prefix, start, limit)` | `namespace_scan` |

- **Pages:** the host gives at most 256 entries at a time (`MAX_SCAN`). The iterators ask for 64
  at a time, so a loop that stops early pays for, and depends on, only the pages it read. To go
  on later where one stopped, start at the last key with a 0 byte after it.
- **Cost:** 100 fuel a page, plus 40 an entry and 1 for every 16 bytes of keys and values given,
  and of the prefix and start.
- **Layouts:** code that declares a storage layout iterates only the namespaces it declares, and
  plain storage, which it may read.
- **Side by side:** a transaction that iterated is run again, when a block runs transactions side
  by side, if one before it wrote any key in the range it saw, even one that was not there (see
  [Chain](chain.md#running-transactions-side-by-side)).
- **Lists grow:** iterating a list anyone can add to costs more as it grows. Keep a count, or page
  through it with `scan`, rather than going through all of it in one call.

`contracts/examples/members` keeps a club's members, counts them by iterating, and lists them a
page at a time.

## Storage layouts

Code may declare its storage layout: the namespaces it keeps its state in, and what each holds.
New code then has to keep it, so an upgrade or a proxy's new implementation cannot silently
start from empty storage because it renamed a namespace or a key, or misread a value because it
changed how one is encoded.

A namespace is one of two shapes:
- **A map:** keys of one kind, or any key, and every value one kind. For state keyed by something,
  such as balances by address.
- **A record:** only the keys it declares, each with its own kind. For a contract's settings and
  counters.

```lua
--@namespace balances address-hex -> decimal:wei   -- a map: keys and values of a kind each
--@namespace notes text                            -- a map of any key
--@namespace config                                -- a record: only the keys below
--@key config owner address-hex
--@key config paused bool
--@retired config fee_rate                         -- a key no longer used
--@retired fees                                    -- a namespace no longer used

local balances = storage.namespace("balances")
balances:set(who:hex(), tostring(amount))
storage.namespace("config"):set("owner", ctx.sender:hex())
```

```rust
namespace!(BALANCES, "balances", "address-hex" => "decimal:wei");
namespace!(CONFIG, "config", { "owner": "address", "paused": "bool" });
retired!("config", "fee_rate");
retired!("fees");

BALANCES.set_u256(&who.encode(), amount);
CONFIG.set_address(b"owner", &sender());
```

Lua lines may sit anywhere in the script. Rust's macros write the module's `chainv4.layout`
custom section, check names, keys and kinds at compile time, and define a constant to use the
namespace by. Names are 1 to 64 bytes and keys 1 to 256, each one word.

**Kinds** are encodings, and the node checks every value written against its kind, and every key
a map with a key kind is read, written or cleared at. Both engines read and write every kind, the
SDK with a `get_` and `set_` on a namespace for each, Lua with strings and with namespace methods
for the binary ones:

| Kind | Encoding | Lua | Rust |
| --- | --- | --- | --- |
| `bytes` | Anything. | a string | `get`, `set` |
| `text` | UTF-8. | a string | `get_text`, `set_text` |
| `number` | A whole number within 2^53 either way, in digits: `5`, `-42`. | `ns:set_number`, `ns:get_number` | `get_number`, `set_number` |
| `decimal` | A whole number from 0 in decimal text, up to the largest u256. | `tostring(u256)`, `u256(text)` | `get_decimal`, `set_decimal` |
| `bool` | One byte, 0 or 1. | `ns:set_bool`, `ns:get_bool` | `get_bool`, `set_bool` |
| `i64`, `u64` | 8 bytes, little-endian. | `ns:set_i64`, `ns:get_u64`… | `set_i64`, `get_u64`… |
| `i128`, `u128` | 16 bytes, little-endian. | `ns:set_i128`, `ns:get_u128`… | `set_i128`, `get_u128`… |
| `i256`, `u256` | 32 bytes, little-endian; `i256` in two's complement. | `ns:set_i256`, `ns:get_u256`… | `set_i256`, `get_u256`… |
| `address` | 33 bytes: the kind's tag, then its 32, or a secp256k1 address's 20 and 12 zeros. | `ns:set_address`; `who:encode()`, `address.decode` | `set_address`, `Address::encode` |
| `address-text` | An address in text: `ed25519:0x…`, `secp256k1:0x…`. | `who:text()`, `address.parse` | `get_address_text`, `set_address_text` |
| `address-hex` | `0x` and 64 hex digits, or 40 for secp256k1. | `who:hex()`, `address(hex)` | `get_address_hex`, `set_address_hex` |

A kind that is not in the list is refused at deploy. A kind may name a **unit** after a colon,
`decimal:wei`: no write can check what a number counts, so the unit is the layout's word for it,
which later code must keep.

**What it enforces:**
- **Code that declares a layout reaches only what it declares.** Every one of these fails, naming
  what went wrong:
  - another namespace: "namespace elsewhere is not in this code's layout";
  - a key a record does not declare: "key onwer is not in namespace config's layout";
  - a map key that is not its key kind, read, written or cleared: "namespace balances takes keys
    of address-hex, and ed25519:0xa9d1… is not one";
  - a value that is not its kind: "namespace balances, key alice: the value is not decimal".
- **Plain storage is read and clear only** for such code, since declared state lives in
  namespaces. A contract that starts declaring a layout can still read its older plain keys and
  delete them, so it can move them into namespaces: in `migrate`, or behind a proxy in an
  exported function called once. Its own space stays free. Under `delegate`, it is the running
  code's layout that counts. Code that declares nothing may use anything.
- **New code keeps the old layout.** Every namespace the old code declared must still be
  declared, with the same shape: a map with the same kinds, a record with every key it had, each
  with the same kind. Anything may be retired instead. New namespaces and keys may come in. A map
  of any key may gain a key kind, and a kind without a unit may gain one; then they stay. Code
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
  [values lists](#values-lists)). Passing `...` instead converts them through Lua, so an
  `amount` comes back a `u256`. A Rust proxy forwards the encoded values as they are.

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
transfer under it, in the order they started (`structs::trace`). Calls into the EVM, and the EVM
transactions themselves, record every EVM frame among them, and the native calls those make
under them, so one tree reads across engines (see [EVM](evm.md#calls-with-lua-and-webassembly-contracts)).
Each hop says:
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
| Storage scan, a page | 100, plus 40 an entry and 1 per 16 bytes of keys, values, prefix and start |
| Hash | 20, plus 1 per 64 bytes hashed |
| Call another contract | 1,000, on top of what the callee burns, and 1,000 more when it sends value |
| Balance | 100 |
| Transfer | 1,000 |
| Event | 200, plus 1 per 16 bytes of name and data |
| Copying between a module's memory and the host | 1 per 64 bytes |
| Loading code at deploy or upgrade | 1 per byte of it, plus a Lua script's top level as it runs |
| Loading a WebAssembly module for a call | 1 per byte of it, every call, cached on the node or not |
| Asking a token's name, symbol and decimals at deploy or upgrade | What those functions burn |
| Asking for that again, with `tokens.refresh` or `refresh_token` | 100, and what the questions burn once the call succeeds |
| Resolving a name, or an address's primary name | 500, plus 1 per 64 bytes of name from WebAssembly |
| Checking a signature, of any scheme | 3,000, as the EVM's ecrecover, plus 1 per 64 bytes of message |
| Recovering a secp256k1 key | 3,000 |
| The address a key controls | 100 |
| RIPEMD-160, HASH160 or double SHA-256 | As a hash: 20, plus 1 per 64 bytes |
| Checking a state proof or a Merkle path | 200, plus 60 per 32 bytes of it |
| Checking an Ethereum BLS aggregate signature | 103,000 (`bls_verify`), plus 5,000 per public key (`bls_key`) and 1 per 64 bytes of message: in proportion to a signature's 3,000, as measured |
| Reading the federation, in force or of any version | 200 |
| Reading the chain's id | 20 |

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
  sees nothing of the call: `ctx`, `storage`, `contracts`, `accounts`, `events`, `tokens` and `collections` are
  not there yet. It is the same code doing the same work every time.
- **Every WebAssembly call pays for its module,** 1 fuel a byte at `load_byte`, whether the node
  had it compiled already or not. A node keeps compiled modules in a cache of its own, which no
  call may depend on: so a module evicted by many others, which the node must compile again,
  costs the call the same, and pays for that compiling.

**Lua's libraries pay for what they go over,** in the forked Luau, as instructions alone would let
one call do a great deal for nothing. Each is the same on every platform:
- **Patterns:** `string.find`, `match`, `gmatch` and `gsub` pay 1 fuel for every step of the
  matcher and 1 for every 8 characters a repetition or `%b` scans, so a pattern that backtracks
  runs out of fuel rather than for hours. A plain search pays 1 per 64 bytes searched.
- **Building strings:** `string.rep`, `sub`, `reverse`, `lower`, `upper`, `format`, `gsub`'s
  result, `table.concat` and `..` pay 1 per 64 bytes they make; `string.split` 1 per 64 bytes
  times the separator's length.
- **Tables:** `table.sort` pays one fuel for each comparison it is expected to make, n times
  log2 n, on top of a comparator's own instructions; `insert` and `remove` in the middle, `move`,
  `create`, `find`, `clear`, `clone` and `maxn` 1 per 8 elements they go over.
- **utf8:** `utf8.len` 1 per 64 bytes, `codepoint` 1 per 8 values.

**Numbers are whole.** A Lua number is a whole number within 2^53 − 1 either way. A double holds
every one of those exactly, so nothing a contract computes is ever rounded:
- a literal with a fraction, such as `1.5` or `1e-3`, fails to compile, and so does one past the
  bound, such as `9007199254740992`;
- `+`, `-`, `*` and `^` fail when the result would be past the bound;
- `/` gives the quotient when it divides evenly, so `6 / 3` is `2`, and fails otherwise, as `7 / 2`
  does. `//` floors and `%` takes the divisor's sign, as in Lua: `7 // 2` is `3`, `-7 // 2` is `-4`
  and `-7 % 3` is `2`;
- all three are worked out in integers, so `(2^53 - 1) / (2^53 - 2)` fails where a double would
  round it to `1`;
- dividing by zero fails, where a double would give an infinity or NaN;
- `tonumber` gives `nil` for text that reads as a fraction or as a number past the bound.

Past 2^53, use the [integer types](#integers), which are exact to 2^256. The compiler folds
constants by the same rules, and leaves whatever would fail for the VM to refuse. Nor does it fold
the math functions scripts lack, so `math.sqrt(4)` fails as it would when the script runs, and no
bytecode depends on how a platform's C library rounds.

**Powers take a whole exponent.** `^` and `math.pow` compute by repeated squaring, which every
platform rounds alike, where the C library's `pow()` need not. So `x ^ 2` works, and a negative
exponent such as `2 ^ -3` fails like any fraction would. The compiler folds a constant power the
same way.

A call that runs out of fuel fails with `OutOfFuel`. On a chain with fees, fuel costs gas, and the
gas limit bounds the fuel too (see [Fees](fees.md)).

Storage keys are at most 256 bytes and values at most 64 KiB.

## Memory

Each call may hold `tx_limits.memory` bytes, 32 MiB on the dev chains. A call past it fails with
`OutOfMemory`, and every node must fail it at the same allocation, with the same fuel used, or
their receipts differ.

**In Lua, the forked Luau counts it:**
- **From zero, at the call's start.** What the pristine state already holds, about 95 KiB of
  libraries and environment, is not counted.
- **By the sizes Luau asks for:** every string, table, function, userdata, buffer and stack, as it
  is made or grows. What the collector frees comes off.
- **Not by what the allocator holds.** Luau keeps small objects in pages, and which allocation
  needs a new page depends on where earlier frees left room, so a count of pages would move
  differently for the same script.
- **Caught or not.** A script can `pcall` running out and go on, with what it still holds counted.
- **Strings are interned.** Luau keeps one copy of each string, so making the same text twice
  counts once.

**The collector runs on bytes alone.** It starts a cycle once the heap is twice what the last one
left (Luau's goal of 200%) and does a fixed amount of work per kilobyte allocated, so it frees
garbage at the same allocations on every node.
- **Not on the clock.** Upstream Luau starts a cycle early by a guess of how much the program
  allocates while it marks, made from the clock. A loaded machine took longer, started earlier, and
  freed garbage at other points. Under load, a call that made garbage up to the limit failed a few
  strings sooner, for less fuel. The block that was built and the same block imported then gave
  different receipts (seen 2026-11-05). The fork starts every cycle at the goal.
- **Pristine states wait collected.** Each one is fully collected once built, so every call starts
  with the same heap and the collector paused, however long the state waited.
- **Weak tables** lose their entries at those same points, so what one still holds is the same on
  every node too.

**Across platforms:** the sizes are those of a 64-bit build, the same on x86-64 and arm64. A 32-bit
build would count smaller objects and run out later.

**In WebAssembly**, the limit is on the module's linear memory, in 64 KiB pages: a `memory.grow`
past it fails the call with `OutOfMemory`, and a module that starts with more cannot load.

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
- what `math` has for whole numbers: `abs`, `ceil`, `clamp`, `floor`, `max`, `min`, `round` and
  `sign`, and none of its float functions, such as `sqrt` or `pi`;
- the chain's libraries.

**Constants.** Luau's `const` binds a name once: `const LIMIT = 100`, or `const function
fee(amount) … end`. Assigning it again is refused when the contract compiles, so a deploy with one
fails, as any other syntax error does. It fixes the name, not what a table holds: `const T = {}`
can still have `T.x = 1`. It is not a function the compiler runs ahead of time: Luau folds
constant expressions such as `60 * 60` by itself, `const` or not.

Nothing reads the clock, the platform or the garbage collector, and nothing gives random
numbers. Nor does anything show a memory address, which differs from node to node: `tostring` of
a table, function or userdata without `__tostring` gives its type's name, `table`, and a failure
raised with such a value reads the same way. A table keyed by tables or functions iterates in the
order they were made, the same on every node, where plain Luau would follow their addresses; a
userdata, such as an Address, is refused as a key, since two equal ones would be two keys: key by
`who:text()`. `setfenv`, `getfenv`, `load`, `require`, `collectgarbage`, `coroutine`, `os`, `io`
and `debug` are not there at all.

| Library | What it has |
| --- | --- |
| `ctx` | `ctx.sender`, `ctx.contract`, `ctx.value`, `ctx.args`, the call's arguments as a values list, and `ctx.time` and `ctx.height`, the block's time in milliseconds since 1970 and its height: the header's, the same on every node, for deadlines. |
| `storage` | `storage.get(key)`, `storage.set(key, value)`; `nil` deletes. Keys and values are strings. `storage.namespace(name)` and `storage.own` have the same `get` and `set`, as methods, on keys of their own. |
| `accounts` | `accounts.balance(address)`, `accounts.transfer(to, amount)` from the contract's balance. |
| `contracts` | `contracts.call(address, name, ...)`, `contracts.call_value(address, value, name, ...)`, `contracts.delegate(address, name_or_selector, ...)`, `contracts.check_layout(from, to)`, `contracts.selector(name)`, `contracts.clone(template, salt, ...)`, `contracts.cloned_at(salt)`. |
| `events` | `events.emit(name, ...)`. |
| `tokens` | `tokens.refresh()`: asks the token registry to ask this token's name, symbol and decimals again once the transaction succeeds. |
| `collections` | `collections.refresh()`: the same request, for a [collection](nft-standard.md) renamed; one asks again for both registries. |
| `names` | `names.resolve(name)`, the address a name such as `alice.dev` resolves to, or `nil`; `names.name_of(address)`, the name an address goes by. At the block's time. See [Names](names.md#names-in-contracts). |
| `evm` | `evm.call(address, calldata, value)` calls an [EVM](evm.md) contract and returns what it returned, as bytes; `evm.encode("transfer(address,uint256)", to, 5)` writes calldata, `evm.encode("(uint256,string)", …)` arguments alone; `evm.decode("(uint256,bool)", bytes)` reads them back. Tables for arrays and tuples. See [EVM](evm.md#calls-with-lua-and-webassembly-contracts). |
| `address` | `address.parse("ed25519:0x…")`, `address.from_hex`, `address.from_bytes`; methods `hex`, `bytes`, `text`, `kind`, `is_contract`, `is_valid`. |
| `u64`, `u128`, `u256`, `i64`, `i128`, `i256` | The integer types: `u256(value)` or `u256.new`, and `zero`, `min`, `max`, for each. See [Integers](#integers). |
| `hash` | `hash.sha256`, `blake3`, `blake2s`, `blake2b`, `xxh3`, `keccak256`, whichever the node has; the result has `hex`, `bytes`, `algorithm`. |
| `crypto` | See [Crypto](#crypto). |
| `federation` | `federation.get()`: the bridges' federation in force, `{ version, threshold, signers }`, or `nil`. `federation.at(version)`: the one of any version, in force or past, as the chain keeps every one it decided, with `decided_height` and `decided_time`, the block that decided it, and once the next was decided, `replaced_height` and `replaced_time`, that block's; `nil` for a version never decided. Version 0 is genesis's. Each costs what reading the one in force does. See [Bridges](bridges.md#the-federation). |
| `chain` | `chain.id()`: the chain's id, what its EVM answers `CHAINID` with and the [bridges](bridges.md) name it by. Six bytes of its genesis id, so nothing an owner sets: bind to it whatever must hold on this chain alone. |

### Crypto

What a contract checks of another chain, a signer or a header, the same in Lua, WebAssembly and,
through the [crypto precompile](evm.md#crypto), the EVM (`vm::crypto`):

| Lua | SDK | Does |
| --- | --- | --- |
| `crypto.verify(scheme, key, message, signature)` | `verify` | Whether a signature over a message is the key's, by its scheme's rules, as chainv4's keys sign: strict Ed25519 and EdBlake, low-s ECDSA over the keccak256 for `secp256k1`, over the double SHA-256 for `bitcoin`. A scheme the chain lacks, or a key of the wrong length, is an error. |
| `crypto.recover(digest, signature)` | `recover` | The secp256k1 key that signed a 32-byte digest, compressed, as Ethereum's ecrecover finds it: `r`, `s` and `v`, 65 bytes, `v` 0, 1, 27 or 28. `nil` for none, high-s signatures included. |
| `crypto.address(scheme, key)` | `key_address` | The account a key controls. |
| `crypto.ripemd160`, `hash160`, `sha256d` | `ripemd160`, `hash160`, `sha256d` | Bitcoin's hashes, as bytes: what its addresses, blocks and transactions are made of. |
| `crypto.check_state(root, column, key, value, proof)` | `check_state` | Whether a [state proof](light-client.md), as `GET /proofs` encodes it, shows an entry holds a value, or with `nil`, nothing, under a state root. |
| `crypto.check_merkle(root, data, index, count, path)` | `check_merkle` | Whether data is the index-th of `count` leaves under a Merkle root, by its audit path: how a header holds its transactions and receipts. |
| `crypto.fast_aggregate_verify(pubkeys, message, signature)` | `fast_aggregate_verify` | Ethereum's BLS check, FastAggregateVerify with the ciphersuite `BLS_SIG_BLS12381G2_XMD:SHA-256_SSWU_RO_POP_`: whether a 96-byte signature on G2 is the aggregate of every 48-byte key's on G1 over one message, as a sync committee signs. Keys come as one string back to back or, in Lua, a table of them. No keys, wrong lengths, a key or signature that is no point of its group or is the point at infinity, and keys that sum to infinity all give false. A build without `bls12-381` fails the call. |

The SDK also has `sha256`, SHA-256 on the node as `hash` with `Algorithm::Sha256`, into 32 bytes
without allocating: what SSZ hashes with.

Proofs are checked by the chain's own hash, so they take this chain's headers, or another chainv4
chain's made with the same hash. What a header the contract trusts says is the contract's to
decide: a root it was given by its owner, by a federation, or proven some other way.

### Integers

A Lua number is a whole number within 2^53 either way (see [numbers are whole](#fuel)). Six
integer types hold the rest, each its own type with its own width, as in Rust: `U64`, `U128`,
`U256`, `I64`, `I128` and `I256`.

```lua
local supply = u256("21000000000000000000000000")   -- text past 2^53, which a number cannot hold
local fee = supply * 25 / 10000                       -- a number on either side takes the type
local debt = i256(-5000)

assert(supply > 1000 and 0 > debt)                    -- comparisons go by the true value
assert(u64(5) == u256(5) and u64(0) > i64(-1))         -- across types too

local total = u64.max:checked_add(1)                  -- nil: + would fail the call
local capped = u64.max:saturating_add(1)              -- u64.max
local wrapped = u64.max:wrapping_add(1)               -- 0
```

- **Making one:** `u256(x)`, short for `u256.new(x)`, takes a whole number within 2^53, decimal or
  `0x` hex text (after a `-` for the signed types), or any integer the type holds: `u64(u256(5))`.
  Each library also has `zero`, `min` and `max`.
- **Arriving:** arguments of the six types arrive as themselves, and amounts and `ctx.value` as a
  `U256`; `ns:get_u64` and the rest read storage back as their type.
- **Operators:** `+ - * / // % ^` and unary `-`. None wraps: overflow, underflow and division by
  zero fail the call, with `u64 overflow`, `u256 underflow` or `division by zero`.
- **Mixing:** a plain number takes the integer's type, so it must be whole, and not negative for
  an unsigned type. Two integers of one signedness make the wider: `u64(1) + u256(2)` is a
  `U256`. A signed and an unsigned one do not mix, since no type holds both `U256` and `I256`:
  convert one side, `i256(x)` or `u256(x)`.
- **Division:** `/` rounds toward zero and `%` takes the dividend's sign, as in Rust, so a Lua and
  a Rust version of a contract agree: `i64(-7) / 2` is `-3`, `i64(-7) % 2` is `-1`. `//` rounds
  down, as Lua's does: `i64(-7) // 2` is `-4`. Unsigned types round down either way.
- **Comparisons:** `== ~= < <= > >=` work with any integer type and any number, by the true value:
  `u256(2) < 2.5`, `u64(0) > -1`, `u256(2) ^ 60 == 2 ^ 60`. NaN equals nothing and orders
  against nothing; anything else, a string or an address, is never equal and fails to order.
- **Methods**, under the names Rust gives them:

| Method | Does |
| --- | --- |
| `checked_add`, `checked_sub`, `checked_mul`, `checked_div`, `checked_rem`, `checked_pow`, `checked_neg` | What the operator does, or `nil` where it would fail. |
| `saturating_add`, `saturating_sub`, `saturating_mul`, `saturating_pow`, `saturating_neg` | Stops at the type's `min` and `max`. `saturating_neg` is the signed types'. |
| `wrapping_add`, `wrapping_sub`, `wrapping_mul`, `wrapping_pow`, `wrapping_neg` | Wraps around modulo 2^bits. |
| `min`, `max` | The smaller or the larger, as the wider type when they differ. |
| `abs` | Without its sign; the signed types'. `i64.min:abs()` fails. |
| `tonumber` | A plain number, for a `for` loop or `string.rep`; fails past 2^53. |
| `hex` | `0xff`, or `-0x10` below zero. |

`tostring` and `..` write the value in decimal, and `typeof` names its type, `U256`. An integer is
no table key; key by `tostring(n)`.

### Splitting a contract across files

A line `--@include "path"` puts another file in where it stands, as C's `#include` does:

```lua
-- names.lua
local M = {}

--@include "lib/checks.lua"
--@include "lib/names.lua"

function M.claim(name)
    check_name(name)              -- a local of lib/checks.lua
    give(name, nil, ctx.sender)   -- and one of lib/names.lua
end

return M
```

```sh
chainv4 send deploy --lua contracts/lua/names/names.lua
# put together names.lua with lib/checks.lua, lib/names.lua: 2505 bytes
```

- **Put together before sending.** A contract goes on chain as one source, which every node
  compiles, so no node ever reads your files. `chainv4 send deploy --lua`, `send upgrade --lua`,
  `chainv4 layout` and the explorer's Send page put the files together first
  (`chainv4::structs::bundle`), and the chain holds the result, each included file between an
  `-- included lib/names.lua` line and an `-- end of lib/names.lua` line.
- **Paths** are relative to the file that includes, with `/` between folders, and `..` may leave
  the entry's folder.
- **One scope.** The included text shares the includer's scope: its locals are the includer's
  from that line on, so include a file above the code that uses it. Luau allows 200 locals in one
  function, and the top level of every file put together is one function.
- **Once each.** A second include of a file is left out, with a comment saying so. A file that
  includes itself, directly or through others, is refused.
- **Annotations work wherever they are.** A `--@namespace` in an included file declares the
  contract's layout, and `--@reentrant` sits above its function in whichever file defines it.
- **Mistakes name their file.** `send` and `layout` compile the bundle before sending, so a
  syntax error reads `lib/checks.lua:3: …`, and the explorer places a failed deploy's error the
  same way. An error a call raises later names the line of the source on chain, which the
  explorer shows; `send … call --source main.lua` puts the contract's files together again and
  names the file and line it came from instead, `lib/checks.lua:3: too much`.
- **The 128 KiB limit** is the bundle's.
- **Whole lines only.** The include is a line of its own, and any such line is replaced, even
  inside a long string or a `--[[ ]]` comment.
- **A node refuses** source that still holds a `--@include` line: the files must be put together
  before it is sent.

**How values cross into Lua:**
- integers arrive as their own types, `u64` as a `U64` up to `i256` as an `I256`, and amounts as
  a `U256`, so nothing is rounded;
- a `number` arrives as a number, and must be whole, within 2^53;
- addresses arrive as `address` values;
- `nil` arrives as `nil`.

**How values come back:**
- numbers come back as `i64`;
- each integer type comes back as itself: a `U64` as a `u64`, an `I256` as an `i256`;
- strings come back as strings, or bytes when they are not UTF-8;
- `nil` comes back as `nil`, so a function may return it, alone or among other values. A
  function that returns nothing returns no values at all.

### Values lists

**Numbers cross whole.** Luau has one number type, a float, and computes with it inside a script.
But every number a contract hands out, returns, arguments to other contracts, events and values
lists, goes out as an `i64`, and must be whole and within 2^53 either way: `return 1 / 2` fails
the call with "0.5: numbers leave a contract whole". A `number` argument coming in must be whole
too. So no float ever reaches a caller, another contract, a receipt or storage; the
[integer types](#integers) are for anything larger.

Lua has no bytes and no amounts of its own, so a few values converted into Lua and back change
type:

| Arrives as | In Lua | Goes back out as |
| --- | --- | --- |
| `number` | a number | `i64` |
| `amount` | a `U256` | `u256` |
| `bytes` that read as UTF-8 | a string | `string` |
| `u64` … `i256` | its own type | the same type |

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
| `block_time`, `block_height` | The block's time, in milliseconds since 1970, and its height, as `i64`s: the SDK's `block_time()` and `block_height()`. |
| `hash` | Hashes bytes with a chosen algorithm. |
| `balance`, `transfer` | Funds, from the contract's own balance. |
| `call`, `delegate` | Other contracts: `call(address, value, name, name_len, args, args_len)`. |
| `check_layout` | Whether one contract's code keeps another's storage layout. |
| `emit` | An event. |
| `refresh_token` | Asks the token registry to ask this token's name, symbol and decimals again once the transaction succeeds. |
| `resolve_name`, `name_of` | Names: `resolve_name(name, len, out)` writes the address a name resolves to, returning 1, or returns 0; `name_of(address)` puts an address's primary name in the result buffer and returns its length, or -1. |
| `evm_call` | Calls an [EVM](evm.md) contract: `evm_call(address, value, input, input_len)`, with what it returned in the result buffer and 0, or why it reverted and 1. |
| `abi_encode`, `abi_decode` | Solidity's ABI, on the node: `abi_encode(signature, len, values, values_len)` writes calldata or arguments from a list of values, `abi_decode(types, len, data, data_len)` reads them back into one. |
| `verify`, `recover`, `key_address`, `crypto_hash`, `check_state`, `check_merkle` | [Crypto](#crypto): a signature checked, 1 or 0; the key a secp256k1 signature recovers, 33 bytes at `out`, 1 or 0; a key's address at `out`; `ripemd160`, `hash160` or `sha256d` at `out`, returning its length; a state proof or a Merkle path checked, 1 or 0. |
| `fast_aggregate_verify` | [Crypto](#crypto): `fast_aggregate_verify(pubkeys, pubkeys_len, message, message_len, signature, signature_len)`, Ethereum's BLS aggregate check, 1 or 0. Traps in a build without `bls12-381`. |
| `federation` | The federation in force into the result buffer, its length, or -1 for none: its version (u64), threshold and signer count (u32s), then 33-byte keys. |
| `chain_id` | The chain's id, as an `i64`: what `chain.id()` gives in Lua, the SDK's `chain_id()`. |
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
- **Parameters** may be any `FromValue` type: `Address`, `U256`, `I256`, `bool`, `String`,
  `Vec<u8>`, `i64`, `u64`, `u32`, `u8`, `i128`, `u128`, a `Value` as it came, or an `Option` of
  any of them. Whole numbers arrive as any integer kind that holds them; Lua sends its numbers as
  `i64`.
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
  `refresh_token`, `resolve_name` and `name_of`, `evm_call`, `abi_encode` and `abi_decode`,
  `verify`, `recover`, `key_address`, `ripemd160`, `hash160`, `sha256d`, `sha256`,
  `check_state`, `check_merkle`, `fast_aggregate_verify`, `federation` and `chain_id`, and
  `storage`, with `storage::namespace(name)`
  and `storage::own()`;
- `namespace!` and `retired!`, which declare the storage layout, and typed methods on a
  namespace for every kind's encoding: `set_i64`, `set_i128`, `set_i256`, `set_u256`,
  `set_address`, `set_bool`, and the
  text kinds Lua writes, `set_decimal`, `set_text`, `set_number`, `set_address_text` and
  `set_address_hex`, each with its `get_` twin.

**Integers:** `u64`, `i64`, `u128` and `i128` are Rust's own. `U256` and `I256` behave the same:
- `+ - * / %`, `-` for `I256`, and their `+=` forms, with a `u64` or an `i64` on either side, so
  `amount * 3` and `debt - 5` need no conversion;
- comparisons with them either way round: `amount > 1000`, `0 > debt`;
- the operators abort the call on overflow, underflow and division by zero, with Lua's messages,
  `u256 overflow` and the like, and `/` and `%` round as Rust's do;
- `checked_`, `saturating_` and `wrapping_` forms of each, `pow`, `abs`, `MIN`, `MAX`, `ZERO`,
  `ONE`, decimal text both ways and `Display`;
- `From` the smaller types and `TryFrom` back, which fails with `OutOfRange`.

```rust
#[export]
fn settle(change: I256) -> I256 {
    let balance = LEDGER.get_i256(b"balance").unwrap_or(I256::ZERO) + change;
    LEDGER.set_i256(b"balance", balance);
    balance
}
```

**Numbers:** floats are refused in modules, and Lua sends its numbers as `i64`. A `number` value
from a transaction still comes as float bits, so `Value::as_i64` and `Value::as_amount` turn whole
ones into integers using integer instructions only.

**Building:** build from `contracts/`, whose `.cargo/config.toml` sets the wasm32 target. From
the repo root, pass `--target wasm32-unknown-unknown`.

## Examples

| Example | Shows |
| --- | --- |
| `contracts/examples/counter` | Storage, arguments and return values, and a storage layout. |
| `contracts/examples/vault` | Holding and sending funds. |
| `contracts/examples/members` | Iterating storage: a namespace's keys counted, and listed a page at a time. |
| `contracts/examples/names`, `contracts/evm/NamePayer.sol` | Resolving [names](names.md) and paying them, from WebAssembly and the EVM. |
| `contracts/examples/evm`, `contracts/evm/Bridge.sol` | Calls between engines: WebAssembly and Lua calling the EVM, and the EVM calling them through the native precompile. |
| `contracts/examples/proxy` | Calling other contracts for a caller, and a reentrant callback. |
| `contracts/examples/upgradable`, `contracts/lua/proxy.lua` | A proxy built on `fallback` and `delegate`, with its owner and implementation in its own space, that checks a new implementation's layout. |
| `contracts/examples/token`, `contracts/lua/token.lua` | The [token standard](token-standard.md), held to it by one shared test. |
| `contracts/examples/btc_relay`, `contracts/lua/btc_gateway.lua` | Bitcoin's headers as a contract, checked by its own rules, and the [bridge](bridges.md#bitcoin) that mints wBTC for deposits proven to it and checks BIP143 signatures over its releases. |
| `contracts/examples/eth_relay` | Ethereum's beacon chain as a contract, a light client of its sync committees on the shared `ethlight` crate: finalized headers kept from updates two thirds of the committee signed, their ancestors proven by block roots, and transactions proven in them. See [Ethereum's light client](#ethereums-light-client). |
| `contracts/lua/swap.lua` | An [atomic swap](swaps.md)'s chainv4 leg: the coin or a token locked under a SHA-256 hash for a recipient until a deadline by `ctx.time`, claimed with the secret or refunded after, with `transfer_from` for tokens. |
| `contracts/lua/dex.lua`, `contracts/lua/lp_token.lua` | The [DEX](dex.md): constant-product pools of the coin, tokens and ERC-20s through `evm.call`, each pool's share token a clone of `lp_token.lua` made with `contracts.clone`. |
| `contracts/lua/guestbook.lua` | Events with addresses in them, for the event index and the explorer; value passed on with `accounts.transfer`; and a storage layout of a map and a record. |
| `contracts/lua/names/` | A contract split across files with `--@include`, its storage layout declared in an included file. |
| `contracts/examples/integers`, `contracts/lua/integers.lua` | The integer types in both engines, with one layout: operators, the `checked_`, `saturating_` and `wrapping_` methods, comparisons with plain numbers, and the `i128`, `u128`, `i256` and `u256` kinds. One test runs both and upgrades each into the other's storage. |
| `contracts/lua/points/`, `contracts/examples/points` | Three versions of one contract: the second keeps the first's layout, adds to it, retires a key and moves its value in `migrate`; the third is the second in Rust, keeping the Lua version's text kinds and units. [Contract storage](contract-storage.md) walks through them. |

## Ethereum's light client

`contracts/examples/eth_relay` follows Ethereum's beacon chain as its light clients do, by the
consensus specs' sync protocol cut down to finality. Its rules are the `ethlight` crate's, at the
repository's root, `no_std`, which the node can run too: SSZ's roots and branches, the light
client's containers in each fork's layout, networks as whole definitions (slot timing and fork
schedule, SSZ-encoded, with mainnet, Sepolia and Hoodi built in), the store's rules, and
transactions proven in a payload and read from their RLP. SHA-256 and the BLS
check are passed in: the contract makes them host calls, the node runs them natively.

| Function | Who | Does |
| --- | --- | --- |
| `start(bootstrap, network_name, trusted_root)` | The deployer, once | Starts from a `LightClientBootstrap`, SSZ as `/eth/v1/beacon/light_client/bootstrap/{root}` serves it. Its network is the one the chain's anchoring names (below) when `network_name` is empty or names it, else `mainnet`, `sepolia` or `hoodi`. Its header must be the trusted block root, and hold its execution header and its committee. It keeps the network's definition and the root. |
| `submit(update)` | Anyone | Takes a `LightClientUpdate`, SSZ, signed by at least two thirds of the current committee, or across a period boundary of the next one it learned; its finalized header must be newer, or it must bring the next committee while that is unknown. Its finality, next committee and execution branches are checked, and the signature under the fork of the slot before its signature slot. A finality update counts written as an update with zeros for the next committee and its branch. Returns the finalized slot. |
| `prove_header(held_root, header, branch)` | Anyone | Keeps an ancestor of a header it holds, within the 8,192 slots before it, by the held state's `block_roots`. Returns its slot. |
| `finalized()` | | The finalized slot, its beacon root, and its execution block's number and hash. |
| `header_at(slot)`, `header(root)` | | A kept header's 288-byte record, or nil. |
| `includes(beacon_root, transaction, index, branch)` | | Whether a transaction's bytes are the index-th of a kept block's payload, by its 21-node SSZ branch to `transactions_root`. |
| `calldata(transaction)` | | What a transaction says: its recipient, nil for a creation, its calldata, and its chain id. Legacy, 0x01, 0x02, 0x03 and 0x04 transactions. |
| `network_name()`, `period()` | | Its network; the current committee's period and whether the next is known. |

**The network follows governance.** The host call `eth_network` (Wasm, the SDK's
`chainv4_sdk::eth_network()`, Lua's `chain.eth_network()`) gives the beacon network anchoring's
settings name in force, `anchoring.eth_start.network`, as `ethlight`'s `Network` SSZ: a preset's
built-in definition, or one governance carried whole. It costs what reading the federation does,
200 fuel. `start`, `submit` and `prove_header` follow it whenever its name is the one the relay
started on, and otherwise the definition the relay started with. So a vote adds a fork to a
schedule, or defines a network no build has, such as Gnosis, Chiado or a private beacon chain,
without redeploying the relay; a fork past the schedule in force stops updates until a vote names
it, as its signatures are under a version the schedule lacks.

**What it stores** is bounded and laid out for the node to read natively, as it reads the Bitcoin
relay's (the layout is in the contract's doc comment): its own `store`, `finalized`, `network`,
`network_def` (the definition it started with) and `start` (the trusted root), the
current and next committees in `committees` by period, and at most 8,192 headers in `headers` by
slot, with `roots` from root to slot and `order` saying which to drop next.

**What it costs,** measured in `vm::tests::eth_relay` with the default costs:

| `submit` | Fuel |
| --- | --- |
| 512 signers, bringing the next committee, which it stores | 2,992,025 |
| 512 signers, bringing the next committee it already knows | 3,055,147 |
| 342 signers, the fewest it takes, with no committee | 2,058,556 |

The BLS check is most of it: `bls_verify` and `bls_key` for each signer, 2,663,000 for 512. The
rest, about 300,000, is the module's load, hashing the committee's root through the host's SHA-256,
reading the committees and the network's definition, and asking the host for the governed one. Both fit a transaction's 10,000,000 by default. The contract and
`ethlight` are built at `opt-level = "s"` (`contracts/Cargo.toml`): at the workspace's `"z"` their
hashing and copying loops cost about twice the fuel, more than a larger module's load fee.

## What to keep in mind

- **Keep top levels light.** A Lua script's top level runs on every call, before the call is
  attached, and every call pays for it.
- **Read-only calls go through the API.** `POST /call` runs a function on a throwaway copy of the
  committed state (see [HTTP API](api.md)); from a contract, every call is part of a transaction.
- **A contract cannot be removed.**
