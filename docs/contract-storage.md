# Contract storage

How a contract keeps its state, from a counter's one key to a layout that survives upgrades. This
guide walks through it with examples; the reference tables are in
[Contracts](contracts.md#storage-namespaces), and the database underneath is in
[Storage](storage.md).

**The short version:**
- Every contract has storage of its own: keys to values, both bytes.
- It comes in three spaces: plain storage, namespaces by name, and the contract's own space.
- `--@namespace`, `--@key` and `--@retired` lines declare a **layout**: which namespaces the code
  uses, what each holds, how a map's keys are formed, and the units its numbers count. The node
  then holds the code to it, and holds every later version to it too, so an upgrade cannot lose or
  misread state by accident.
- **Either engine keeps either's state:** Lua writes the text kinds and Rust the binary ones, and
  each has methods for the other's, so a contract can move from Lua to Rust, or back, keeping
  every entry.
- Everything a contract stores is part of the chain's state, so anyone can prove any entry
  against a block's state root.

## Where storage lives

A contract's storage is a set of entries, each a key and a value:
- **Keys** are 1 to 256 bytes, **values** up to 64 KiB. In Lua both are strings, and setting a
  key to `nil` deletes it.
- **Every entry is chain state.** It sits under the block's state root, like balances do, so a
  node proves any entry with `GET /proofs/{column}/{key}`, and a light client checks it against a
  header alone (see [Reading storage from outside](#reading-storage-from-outside)).
- **Nothing lists keys.** A contract reads a key it knows; it cannot ask which keys exist. Keep
  what you will need to find again under keys you can work out, such as an address's text, or
  keep a list of them yourself.

It comes in three spaces, each with keys of its own: the same key in two of them is two entries.

| Space | Lua | Who may write it | For |
| --- | --- | --- | --- |
| Plain storage | `storage.get(key)`, `storage.set(key, value)` | Any code running on the contract | A small contract's state, before it declares a layout. |
| A namespace | `storage.namespace("name"):get(key)`, `:set(key, value)` | Any code running on the contract | State grouped by what it is, which a layout can declare. |
| The own space | `storage.own:get(key)`, `:set(key, value)` | Only the contract's own code | What code it delegates to must never change, such as a proxy's owner. |

**In the database,** each entry's key starts with the contract's address, so no two contracts'
entries ever meet:

```
Storage column      [contract]                          [key]      plain storage
Namespaces column   [contract] [len] [name]             [key]      a namespace
Namespaces column   [contract] [0]                      [key]      the own space
```

`[contract]` is the address's tag and bytes, and `[len]` the name's length in one byte. The length
is what keeps namespaces apart: namespace `ab` with key `c`, and namespace `a` with key `bc`, are
two entries, not one.

## Plain storage

The simplest contract keeps everything in plain storage:

```lua
local M = {}

function M.increment()
    local count = tonumber(storage.get("count") or "0") + 1
    storage.set("count", tostring(count))

    return count
end

return M
```

Values are strings, so numbers go through `tostring` and `tonumber`, and amounts through
`tostring` and `u256`, which keeps every digit. A missing key reads as `nil`.

This is fine for a contract that stays small. It stops being fine when several pieces of code
share one contract, as a proxy's implementations do, or one version follows another: every piece
of code reaches every key, and nothing says which code meant what by them.

## Namespaces

A namespace is a part of storage by name, 1 to 64 bytes. Code that names the same namespace
shares it, and keys in different namespaces never meet:

```lua
local balances = storage.namespace("balances")
local config = storage.namespace("config")

balances:set(who:text(), tostring(amount))   -- balances["ed25519:0x…"] = "250"
config:set("owner", ctx.sender:text())       -- config["owner"] = "ed25519:0x…"
```

- **Group state by what it is,** as you would tables in a database: balances in one namespace,
  settings in another, a module's state in its own.
- **They cost a little more:** what plain storage costs, plus 1 fuel for every 16 bytes of the
  name.
- **A layout can declare them,** which plain storage cannot be: see
  [Declaring a layout](#declaring-a-layout).

Taking a namespace's handle at the top level of the script, as the examples do, reads nothing;
reading and writing happen inside functions.

## The own space

`storage.own` is a namespace with no name that only the contract's own code can write. Code the
contract runs with `contracts.delegate` may read it, but writing it fails, whatever key it picks:
"a contract's own storage is written only by its own code, not by code it delegates to".

That is what a proxy needs. It keeps its owner and the implementation it points at in its own
space, so no implementation, buggy or hostile, can change either:

```lua
-- In the proxy, never in its implementations:
storage.own:set("implementation", implementation:text())
storage.own:set("owner", ctx.sender:text())
```

A layout never covers the own space: code may use it freely, layout or not. See
`contracts/lua/proxy.lua`.

## Declaring a layout

A layout says which namespaces the code keeps its state in, and what each holds. In Lua it is
comment lines starting `--@`, anywhere in the script or in a file it
[includes](contracts.md#splitting-a-contract-across-files). Luau reads them as comments; every
node reads them when the code is deployed.

```lua
--@namespace points address-text -> decimal:points   -- a map: keys of one kind, values of another
--@namespace notes text                              -- a map of any key
--@namespace config                                  -- a record: only the keys declared below
--@key config owner address-text                     -- one of config's keys, and its kind
--@key config name text
```

There are six kinds of line:

| Line | Declares |
| --- | --- |
| `--@namespace NAME KEYKIND -> KIND` | A **map** whose keys are one kind and values another. For state keyed by something, such as points by address. |
| `--@namespace NAME KIND` | A **map** of any key, every value of one kind. |
| `--@namespace NAME` | A **record**: only the keys its `--@key` lines declare. For settings and counters. |
| `--@key NAME KEY KIND` | One of a record's keys, and the kind of its value. |
| `--@retired NAME` | A namespace earlier code used, kept only to be read and cleared. |
| `--@retired NAME KEY` | One of a record's keys, retired the same way. |

Names and keys are one word each: names 1 to 64 bytes, keys 1 to 256. A Lua comment may follow
any line, as above.

### Kinds

A kind is how a value is written. The node checks every value written against its kind, so a
map of `decimal` can never hold `"lots"`. Each engine reads and writes every kind:

| Kind | What a value is | In Lua | In Rust |
| --- | --- | --- | --- |
| `text` | Any UTF-8 text. | A string. | `get_text`, `set_text` |
| `decimal` | A whole number from 0, in decimal digits, up to the largest u256. | `tostring(u256)`, read back with `u256(text)` | `get_decimal`, `set_decimal` |
| `number` | A whole number within 2^53 either way, in digits: `5`, `-42`. | `ns:set_number(key, n)`, `ns:get_number(key)` | `get_number`, `set_number` |
| `address-text` | An address's text form: `ed25519:0x…`, `secp256k1:0x…`. | `who:text()`, read back with `address.parse` | `get_address_text`, `set_address_text` |
| `address-hex` | `0x` and 64 hex digits, or 40 for secp256k1. | `who:hex()`, read back with `address(hex)` | `get_address_hex`, `set_address_hex` |
| `bytes` | Anything. | Any string. | `get`, `set` |
| `bool` | One byte, 0 or 1. | `ns:set_bool`, `ns:get_bool` | `get_bool`, `set_bool` |
| `i64`, `u64` | 8 bytes, little-endian. | `ns:set_i64`, `ns:get_u64`…, as an `I64` or a `U64` | `get_i64`, `set_u64`… |
| `i128`, `u128` | 16 bytes, little-endian. | `ns:set_i128`, `ns:get_u128`…, as an `I128` or a `U128` | `get_i128`, `set_u128`… |
| `i256`, `u256` | 32 bytes, little-endian, `i256` in two's complement. | `ns:set_i256`, `ns:get_u256`…, as an `I256` or a `U256` | `get_i256`, `set_u256`… |
| `address` | 33 bytes: the address's tag, then its bytes. | `ns:set_address`, `ns:get_address`; `who:encode()` and `address.decode` for keys | `get_address`, `set_address`; `Address::encode` |

**Which to pick:** `decimal` for amounts, `address-text` for addresses, `text` for words, and
`number` or `i64` for small counts, and `i256` for a balance that can go below zero. The integer
kinds take any of Lua's [integer types](contracts.md#integers), or a whole number, that they hold:
`ns:set_u64(key, u256(5))` writes 5, and `ns:set_u64(key, -1)` fails. The text kinds read well
anywhere, in proofs and the explorer
included; the binary ones are smaller. Lua writes `number` with `set_number`, not `tostring`,
which spells 10^15 as `1e+15`: the kind takes one spelling for each number.

### Key kinds

A map declared `KEYKIND -> KIND` checks its keys too, whatever the code does with them: reading,
writing and clearing a key that is not its kind all fail. Keys are bytes, so `who:text()` and
`who:hex()` are two different entries for one address; without a key kind, code that switched
from one to the other would read every entry as empty and never know. With one, it fails at the
first read: "namespace points takes keys of address-text, and 0x0a0a… is not one".

### Units

A kind may name a unit after a colon: `decimal:wei`, `u256:points`, `u64:basis-points`. The node
cannot check what a number means, only how it is written, so a unit is the layout's word for it,
which every later version must keep. A version whose `decimal:wei` became `decimal:coins` is
refused like a changed kind, so code that changes what its numbers count says so, retiring the old
namespace and declaring a new one. Units are 1 to 32 letters, digits, `-`, `_` or `.`.

### What the node holds the code to

Once code declares any namespace, it may reach only what it declares. Each of these fails the call,
and with it the transaction, naming what went wrong:

| The code… | Fails with |
| --- | --- |
| uses a namespace it does not declare | `namespace elsewhere is not in this code's layout` |
| uses a key a record does not declare | `key onwer is not in namespace config's layout` |
| reads, writes or clears a map key that is not its key kind | `namespace points takes keys of address-text, and 0x0a0a… is not one` |
| writes a value that is not its kind | `namespace points, key ed25519:0x…: the value is not decimal` |
| writes something retired | `key name of namespace config is retired: it can be read and cleared, not written` |
| writes plain storage | `code that declares a layout keeps its state in namespaces: plain storage can be read and cleared, not written` |

And these stay allowed:
- **reading and clearing** anything declared, retired included, and plain storage, so code can
  move older state into namespaces;
- **the own space,** freely;
- **anything at all,** for code that declares no layout.

Under `contracts.delegate`, the layout that counts is the running code's.

## Changing a layout: an example

`contracts/lua/points/` holds a contract in two versions, and `contracts/examples/points` a third,
in Rust. Version 1, `points_v1.lua`, keeps who holds how many points in a map, keyed by their
address's text, and its owner and name in a record:

```lua
--@namespace points address-text -> decimal:points
--@namespace config
--@key config owner address-text
--@key config name text

local M = {}

local points = storage.namespace("points")
local config = storage.namespace("config")

local function owner()
    return address.parse(config:get("owner"))
end

function M.init()
    config:set("owner", ctx.sender:text())
    config:set("name", "Points")
end

function M.award(who, amount)
    if ctx.sender ~= owner() then
        error("only the owner awards points")
    end

    amount = u256(amount) -- whatever integer type, or whole number, the caller sent
    local now = u256(points:get(who:text()) or "0") + amount
    points:set(who:text(), tostring(now))

    return now
end

function M.check_perms(permission, who)
    return permission == "upgrade" and who == owner()
end

return M
```

```sh
chainv4 send deploy --lua contracts/lua/points/points_v1.lua
# deployed contract:0x8e27…
chainv4 send call --to contract:0x8e27… --function award address:ed25519:0xa9d1… u256:100
# returned [{"type":"u256","value":"100"}]
```

Version 2, `points_v2.lua`, wants three things: badges, a cap on one award, and to call the name
a title. Its layout:

```lua
--@namespace points address-text -> decimal:points   -- kept as it was
--@namespace badges address-text -> text            -- new: a namespace may come in
--@namespace config
--@key config owner address-text                     -- kept as it was
--@retired config name                               -- no longer used: retired, not dropped
--@key config title text                             -- new: replaces name
--@key config max_award decimal:points               -- new
```

**The rules a new version keeps:**
- **Every namespace and key the old code declared is still declared,** with the same shape and the
  same kind.
- **Or it is retired,** on purpose. It can still be read and cleared, never written, and every
  later version must keep it retired.
- **New namespaces and keys may come in,** and start empty.
- **Kinds never change, and maps never become records,** or the other way round: retire the old
  one and declare another.
- **Key kinds and units come in once, then stay.** A map of any key may declare a key kind, and a
  kind without a unit may name one, as a version learns what it keeps; after that, changing or
  dropping either is refused like changing a kind.

`chainv4 layout` checks before anything is sent, loading the code as a node would:

```sh
chainv4 layout contracts/lua/points/points_v2.lua --from contracts/lua/points/points_v1.lua
# contracts/lua/points/points_v2.lua declares
#   namespace badges address-text -> text
#   namespace config
#   key config max_award decimal:points
#   retired config name
#   key config owner address-text
#   key config title text
#   namespace points address-text -> decimal:points
# contracts/lua/points/points_v1.lua declares
#   namespace config
#   key config name text
#   key config owner address-text
#   namespace points address-text -> decimal:points
# contracts/lua/points/points_v2.lua keeps the layout of contracts/lua/points/points_v1.lua
```

`--from contract:0x…` compares against what is deployed, over the API. Had version 2 left out the
`--@retired config name` line, both `chainv4 layout` and the upgrade itself would refuse it:

```
the transaction failed: the new code does not keep the storage layout: key name of namespace config is dropped: keep it, or retire it
```

And had it declared `points` as `decimal:coins`, or its keys as `address-hex`: "namespace points
held decimal:points, and now holds decimal:coins: retire it and declare another", and "namespace
points took keys of address-text, and now takes address-hex".

**Moving state with `migrate`.** The upgrade runs the new code's `migrate` once, right after it
loads. Version 2 moves the name into its new key there, and sets the new cap:

```lua
function M.migrate()
    local name = config:get("name")      -- a retired key can still be read…

    if name ~= nil then
        config:set("title", name)
        config:set("name", nil)          -- …and cleared, never written
    end

    config:set("max_award", "1000")
end
```

```sh
chainv4 send upgrade --contract contract:0x8e27… --lua contracts/lua/points/points_v2.lua
# upgraded contract:0x8e27…
```

The points map kept every entry, `title()` answers `Points`, and `name` is gone from storage. If
`migrate` fails, the whole upgrade fails and nothing changes: had it written `name` instead of
clearing it, the upgrade would have failed with "key name of namespace config is retired".

### Recipes

- **Add a key or a namespace:** declare it. It starts empty; set a value in `migrate` if it needs
  one, as `max_award` does.
- **Rename a key:** retire the old one, declare the new one, and move the value in `migrate`, as
  `name` became `title`.
- **Change a kind or a unit:** the same, under a new key or namespace: retire `count number`,
  declare `total decimal:points`, and convert in `migrate`.
- **Give a map a key kind, or a kind a unit:** declare it, once, in place: `points decimal` may
  become `points address-text -> decimal:points`. Every later version keeps it.
- **Move plain storage into namespaces:** declare the layout, then in `migrate` read each old
  plain key, write it into its namespace, and clear it. Declared code may still read and clear
  plain storage, for exactly this.
- **Change a map's keys or kind:** `migrate` cannot list a map's keys, so it cannot move them all
  at once. Move each when it is next used instead: read the new namespace, and when an entry is
  not there, read the retired one, write the entry anew, and clear the old one.

**A layout is for good.** Code that declares nothing keeps nothing, so it cannot replace code that
declares a layout.

### Moving to the other engine

An upgrade may swap Lua code for WebAssembly, or back, like any other version: the layout is the
same, and so is the storage. What the new code needs is to read and write what the old wrote,
which each engine does for every kind. Version 3, `contracts/examples/points`, is version 2 in
Rust, and keeps its layout line for line:

```rust
namespace!(POINTS, "points", "address-text" => "decimal:points");
namespace!(BADGES, "badges", "address-text" => "text");
namespace!(CONFIG, "config", { "owner": "address-text", "title": "text", "max_award": "decimal:points" });
retired!("config", "name");

#[export]
fn award(who: Address, amount: U256) -> U256 {
    // …only the owner, at most max_award…
    let key = who.text();                                  // the key Lua formed with who:text()
    let held = POINTS.get_decimal(key.as_bytes()).unwrap_or(U256::ZERO);
    let now = held.checked_add(amount).unwrap_or_else(|| abort("too many points"));
    POINTS.set_decimal(key.as_bytes(), now);               // "150", as Lua's tostring wrote it
    now
}
```

```sh
chainv4 layout contracts/target/wasm32-unknown-unknown/release/points.wasm --from contracts/lua/points/points_v2.lua
# …keeps the layout of contracts/lua/points/points_v2.lua
chainv4 send upgrade --contract contract:0x8e27… --wasm contracts/target/wasm32-unknown-unknown/release/points.wasm
```

Every point and badge stays where it was, nothing is retired, and nothing moves. Since version 3
writes the same text Lua did, version 2 could take over again just the same. Going the other way, a
Lua version keeps a Rust one's binary kinds with `ns:get_u256`, `ns:set_address`, `who:encode()`
and the rest.

## Proxies

A proxy upgrades without `Tx::Upgrade`, by pointing at a new implementation, so the chain does not
check the layout for it. The proxy checks itself before it repoints, with
`contracts.check_layout(current, new)`, which applies the same rules:

```lua
function M.set_implementation(implementation)
    if ctx.sender:text() ~= storage.own:get("owner") then
        error("only the owner")
    end

    local current = storage.own:get("implementation")

    if current ~= nil then
        -- Fails, and with it the call, unless the new code keeps the current one's layout.
        contracts.check_layout(address.parse(current), implementation)
    end

    storage.own:set("implementation", implementation:text())
end
```

The implementations share the proxy's namespaces, so each must keep the one before's layout,
and the proxy's own keys sit in its own space, out of their reach. See `contracts/lua/proxy.lua`
and [Delegate calls and proxies](contracts.md#delegate-calls-and-proxies).

## Reading storage from outside

- **Through the contract:** export a function that reads it, and call it read-only with
  `POST /call`, as `points_of` and `title` are. Nothing is written, and no transaction is sent.
- **The layout:** `GET /contracts/{address}` shows what the code declares, each namespace with its
  shape, kind and keys, and which are retired.
- **Proven, without trusting the node:** `GET /proofs/{column}/{key}` proves one entry against a
  block's state root. The key is the one in [the database](#where-storage-lives): for alice's
  points, the contract's tag `01` and 32 bytes, then `06`, `points`, then her address's text:

  ```sh
  curl localhost:8080/proofs/namespaces/0x018e2779…06706f696e7473656432353531393a3078613964…
  # {"column": "namespaces", "value": "0x313030", …}          0x313030 is "100"
  ```

  A light client checks the proof with `check_state` against a header it trusts. Tokens keep
  balances exactly so, in a `balances` namespace under the holder's address text, which is how the
  [wallet](wallet.md) proves every token balance it shows (see
  [Provable balances](token-standard.md#provable-balances)).

## Costs and limits

| What | Cost or limit |
| --- | --- |
| A read | 100 fuel, plus 1 per 16 bytes of key and value |
| A write or delete | 500 fuel, plus 1 per 16 bytes of key and value |
| A namespace | plus 1 fuel per 16 bytes of its name, on every read and write |
| Checking a layout from a contract | 2,000 fuel |
| Keys | 1 to 256 bytes |
| Values | up to 64 KiB |
| Namespace names | 1 to 64 bytes |

## In Rust

The SDK declares the same layouts with macros, which write them into the module's
`chainv4.layout` section and check names, keys and kinds when it compiles. Version 2's layout:

```rust
use chainv4_sdk::{namespace, retired, sender};

namespace!(POINTS, "points", "address-text" => "decimal:points");
namespace!(BADGES, "badges", "address-text" => "text");
namespace!(CONFIG, "config", { "owner": "address-text", "title": "text", "max_award": "decimal:points" });
retired!("config", "name");

CONFIG.set_address_text(b"owner", &sender());
let title = CONFIG.get_text(b"title");
```

Each constant is the namespace's handle, with `get`, `set` and `delete`, and a `get_` and `set_`
for every kind: `set_u256`, `get_address`, `set_decimal`, `get_address_text` and so on (see
[Kinds](#kinds)). `storage::get`, `set` and `delete` are plain storage, and `storage::own()` the
own space. See `contracts/examples/counter` and `contracts/examples/points`.

## What to keep in mind

- **Declare a layout from the start.** Until code declares one, nothing checks an upgrade against
  it, and whatever it kept in plain storage has to be moved into namespaces once it does.
- **Retiring is for good.** A retired name or key can never be used again, so pick new names
  for what replaces it.
- **Nothing lists keys.** Plan how you will find every entry again before you need to migrate it.
- **`decimal` for amounts.** `number` holds whole numbers only to 2^53, as far as a Lua number is
  exact.
- **Declare how a map's keys are formed.** Keys are bytes, so `who:text()` and `who:hex()` are two
  different keys for the same address. A key kind makes that a loud error instead of an empty
  entry; the token standard keys by the text.
- **A unit is a promise, not a check.** The node keeps it across versions; it cannot see whether
  the numbers still count what it says.
