# Names

chainv4 has a name registry of its own, like ENS (`src/registry/names.rs`): names such as
`alice.dev` that point at an address. People pay `alice.dev` instead of `ed25519:0x9a1f…`, and
an account can show a name instead of its address.

- **Registered for whole years**, at a price by the name's length, which is **burned**.
- **Points at an address**, and holds **text records**, such as `url`, `avatar` or `description`.
- **Subnames**, such as `pay.alice.dev`, are free for the name's owner to make, and last as long
  as the name does.
- **A primary name** is the one an account shows: reverse lookups give it for the account's
  address.
- **Expiry:** a name stops resolving when it expires, its owner alone may renew it for a grace
  period, and after that anyone may register it.
- **Prices change by governance:** on a chain with [governance](governance.md), a change of name
  prices is approved and decided like a change of consensus.

Names live in chain state, in their own column, `Names`. So every node agrees on them, the state
root commits to them, and `GET /proofs/names/{key}` proves one to a light client: the
[wallet](wallet.md#names) proves every name it shows or pays.

**The `names` cargo feature,** on by default, holds the registry, the name transactions' running,
`GET /names`, the CLI's `name` commands and the precompile. A build without it refuses to open a
chain whose genesis has names, so it never runs one differently. Contracts' `names` library and
imports are in every build alike, so a contract runs the same everywhere; on a chain without names
they fail.

## Turning it on

Genesis's `names` sets them up, or `null` leaves the chain without them:

```json
"names": {
  "suffix": "dev",
  "prices": ["640000000000000000000", "160000000000000000000", "5000000000000000000"],
  "grace_ms": 7776000000,
  "max_years": 100
}
```

| Field | What it is |
| --- | --- |
| `suffix` | What every name ends in: `dev` for `alice.dev`. One label: 1 to 63 of `a` to `z`, `0` to `9` and hyphens. |
| `prices` | What one year costs, in the coin's smallest units, by the name's length: the first for 3 characters, the next for 4, and the last for every length from its own on. 1 to 61 prices. |
| `grace_ms` | How long after a name expires its owner alone may renew it, in milliseconds. |
| `max_years` | The most years ahead a registration or renewal may hold a name. |

The dev chains have names under `dev`: a year costs 640 coins for 3 characters, 160 for 4 and 5
for 5 or more, as ENS prices them in dollars, with 90 days' grace and up to 100 years ahead.
`chainv4 genesis --names SUFFIX` writes a genesis with another suffix, and `--names none` one
without names.

A chain without names refuses name transactions: `this chain has no names`. Adding them later
makes a new chain, as any genesis field does.

## What a name is

A name is labels joined by dots, written without the suffix in transactions and records:
`alice`, `pay.alice`.

- **A label** is 1 to 63 characters, neither starting nor ending with a hyphen. Its ASCII is
  `a` to `z`, `0` to `9` and hyphens; past ASCII, letters of any script and emoji, as ENS allows
  them: `café`, `日本語`, `москва`, `🦀🦀🦀`.
- **A name you register** is one label, at least 3 characters long. Characters are counted, not
  bytes, for the length and for its price: `日本語` is three, priced as `abc` is.
- **The whole name,** suffix included, is at most 253 bytes.
- **Each name has one form, ENSIP-15's.** A name is taken only as ENS's normalization
  (ENSIP-15) leaves it: lower case, accents composed, full-width letters folded. A transaction
  naming another form is refused, and the refusal names the form it is kept in. Look-alikes are
  refused outright: a label mixing scripts that confuse, such as a Cyrillic `а` among Latin
  letters, invisible characters, and whatever else ENS disallows. The CLI, the API and the apps
  normalize what you type and take the suffix off, so `Café.dev` and `café` mean the same there.
  Normalizing is the `ens-normalize` crate, pinned, of Unicode 17, in the node and the apps alike;
  a new Unicode would be a consensus change, as it changes which names are taken.

## Registering and renewing

```sh
chainv4 send --key me.key name register alice.dev --years 2
# register alice.dev for 2 years: 10000000000000000000 burned
chainv4 send --key me.key name renew alice.dev --years 1
```

- **Registering** takes a name nobody holds, or one past its grace period. It makes the sender
  its owner and points it at the sender. Its price, a year's price for its length times the
  years, is burned.
- **Renewing** adds years from when the name expires, not from now, at the same price, burned.
  Only the owner may renew, before the name expires or in its grace period.
- **Neither** may hold a name more than `max_years` ahead of the block's time.

**Time** is block time: a name registered for a year expires 365 days after the block that
registered it, by that block's timestamp, and is judged by each later block's.

**A registration that fails burns nothing:** it pays its gas, as any failed transaction does.
`chainv4 send` dry-runs name transactions first, so it refuses, before sending, one that would
fail: a name that is taken, or yours already.

## Changing a name

Each of these needs the name to be active, and the sender to own it:

```sh
chainv4 send name target alice.dev --to ed25519:0x…   # point it elsewhere
chainv4 send name target alice.dev --none             # point it at nothing
chainv4 send name text alice.dev url https://alice.example
chainv4 send name text alice.dev url                  # clear it
chainv4 send name transfer alice.dev --to ed25519:0x… # give it away
chainv4 send name subname alice.dev pay --owner ed25519:0x…
```

- **Text records:** keys are 1 to 32 of `a` to `z`, `0` to `9`, dots, hyphens and underscores,
  values up to 512 bytes, at most 16 records a name. ENS's keys read the same here: `url`,
  `avatar`, `description`, `email`, `com.github`, `com.twitter`.
- **Transferring** gives the name to another account. What it points at and its records stay.
- **Subnames** are free. A new subname is owned by and points at its `--owner`, the sender
  without one. Its owner changes it as a name's owner does, and may make subnames of it. The
  name's owner takes a subname back by making it again.

## Expiry and grace

| Time | The name | Who may register or renew it |
| --- | --- | --- |
| Before it expires | Active: it resolves, and its owner may change it. | Its owner may renew it. |
| In its grace period | It resolves to nothing, nor do its subnames, and nothing about it changes. | Its owner alone may renew it, and everything comes back as it was. |
| After its grace period | Gone. | Anyone may register it. |

A name registered again after its grace period starts afresh: its old records, text records and
subnames are all gone. Each subname remembers the registration it was made under, the height of
its name's registration, so subnames made under an earlier owner never come back to life.

## Primary names

```sh
chainv4 send name primary alice.dev   # alice.dev is this account's name
chainv4 send name primary             # it has none
```

An account's primary name is the one it shows. The name must resolve to the account when it is
set, and stops being the account's primary name whenever it no longer does: when it points
elsewhere, expires, or changes hands. `GET /accounts/{address}` gives it as `name`.

## Paying a name

`chainv4 send transfer --to alice.dev --amount 1000` asks the node what `alice.dev` resolves to,
says so, and pays that address. The transaction carries the address, not the name: what the
sender saw is what happens, even if the name changes before the block.

## Reading names

`GET /names` gives the suffix, the prices in force, genesis's, the grace period and `max_years`.
`GET /accounts/{address}/names` lists the names and subnames an address owns that still stand.
`GET /names/{name}`, with or without the suffix, gives:

```json
{
  "name": "alice",
  "display": "alice.dev",
  "status": "active",
  "owner": "ed25519:0x…",
  "target": "ed25519:0x…",
  "resolves_to": "ed25519:0x…",
  "expires": 1798761600000,
  "grace_ends": 1806537600000,
  "texts": { "url": "https://alice.example" },
  "price_per_year": "5000000000000000000"
}
```

- `status` is `active`, `grace` or `available`.
- `resolves_to` is `target` while the name is active, and `null` otherwise.
- `price_per_year` is what a year of a name costs now, `null` for a subname.

## Names in contracts

Contracts resolve names at their block's time, so an expired name resolves to nothing in a
contract as anywhere. Each lookup costs 500 fuel, `cost::NAME`.

**Lua**, with the `names` library:

```lua
function M.pay(name)
    local to = names.resolve(name)          -- an Address, or nil
    assert(to, "no such name")
    accounts.transfer(to, ctx.value)
end

function M.whois(account)
    return names.name_of(account)           -- "alice.dev", or nil
end
```

**WebAssembly**, with the SDK's `resolve_name(name) -> Option<Address>` and
`name_of(&address) -> Option<String>`, over the `resolve_name` and `name_of` imports.
`contracts/examples/names` looks names up and pays them.

**The EVM**, with a precompile at `0x0000000000000000000000000000000000004e53`, NS in ASCII, on a
chain with names. Solidity calls it as a contract:

```solidity
interface Names {
    function resolve(string calldata name) external view returns (address);
    function name(address account) external view returns (string memory);
}

Names constant NAMES = Names(0x0000000000000000000000000000000000004e53);
```

- `resolve` gives the target's EVM address: an Ed25519 account's alias. Resolving one writes the
  alias down, so the EVM can pay the account at once (see [EVM](evm.md#addresses)). Zero for a
  name that resolves to nothing.
- `name` takes an EVM address, as the EVM knows the account, and gives its primary name, or an
  empty string.
- Each call costs 3,000 gas, and 3 for every 32 bytes of input. Bad input reverts.

`contracts/evm/NamePayer.sol` pays a name: `pay("alice.dev")` sends what came with the call to
whatever the name resolves to.

All three resolve subnames as the registry does: `pay.alice.dev` resolves while every name above
it stands, from the current registration of `alice.dev`.

## In the wallet and the explorer

- **The wallet** proves names: a name's records and every one above it, against the tip its light
  client checked. Its Send page takes a name, shows what it was proven to resolve to, and pays that
  address. Its Names page finds and registers names, lists those an address owns with what each
  points at and when it expires, and renews them, points them, sets text records, makes subnames,
  gives them away and picks the address's primary name, each sent through the Send page's review.
  Overview shows each address by its primary name. See [Wallet](wallet.md#names).
- **The explorer** finds a name from its search box, with a page for it: owner, what it resolves
  to, when it expires and its grace ends, its records, and what a year costs. An account's page
  shows the name it goes by and the names it owns. The explorer shows these as the node says them.

## Changing the prices

On a chain with governance, a member or validator proposes new prices, in the smallest unit, in
the same order as genesis's; `--change '{"names":{…}}'` changes the grace and the most years ahead
as well, all but the suffix:

```sh
chainv4 send --key member.key vote-change --vote yes --name-prices 1000000000000000000000,200000000000000000000,10000000000000000000
```

Votes count, move and expire as for any change (see [Governance](governance.md)). The new
settings take effect from the block after the one that decides them. Names already held keep the
years they paid for; a name in its grace period is judged by the grace in force when it is looked
at. `GET /names` shows the prices and grace in force and genesis's prices, and `GET /governance`
the change while its votes count.

## In the state

| Key, in `Names` | Value |
| --- | --- |
| `n` and the name, such as `nalice` or `npay.alice` | The name's record: `owner`, `target`, `expires` (0 for a subname), `root`, the height of its name's registration, and its text records. |
| `p` and an address's key | The name the account chose as its primary name. |
| `o`, an address key's length, the key, and a name | Empty: the name is listed under its owner. The registry moves it when the owner changes. A name expired past its grace stays listed until someone registers it, and `GET /accounts/{address}/names` leaves out what no longer stands. |

The prices governance decided last are kept beside its other decisions, in `Validators`.

## Not yet

- **Unicode names,** with ENSIP-15's normalization against look-alikes.
