# NFT standards

NFTs on chainv4 are contracts, as on Ethereum: a collection keeps who holds what in its own
storage. There are two standards, as Ethereum has ERC-721 and ERC-1155:
- **NFT:** each token has one owner, as in ERC-721.
- **Multi-token:** holders hold amounts of each id, as in ERC-1155, so one collection keeps
  one-of-a-kind items and interchangeable ones side by side.

This page is the interface collections follow, so wallets, explorers, swaps and other contracts
can work with any of them. The chain keeps a [registry](#the-collection-registry) of every
contract that exports one.

There are four reference collections, held to the standards by one shared test each, and a
factory:
- [contracts/lua/nft.lua](../contracts/lua/nft.lua) and
  [contracts/examples/nft](../contracts/examples/nft/src/lib.rs), the NFT standard;
- [contracts/lua/multi.lua](../contracts/lua/multi.lua) and
  [contracts/examples/multi](../contracts/examples/multi/src/lib.rs), the multi-token standard;
- [contracts/lua/nft_factory.lua](../contracts/lua/nft_factory.lua), which clones either.

## Types

- **Ids** are `u256`. Functions that take one also accept any non-negative whole number, such as
  an `i64` a Lua contract passes.
- **Amounts** are `u256`, how many of an id.
- **Addresses** are any address: a key's or a contract's.
- **The zero address**, `ed25519:0x00…00`, stands for "nobody": the source of a mint and the
  destination of a burn.

## The NFT standard

**Required:** an NFT collection exports all of these.

| Function | Returns | Does |
| --- | --- | --- |
| `balance_of(owner)` | `u256` | How many tokens `owner` holds. |
| `owner_of(id)` | address | Who holds `id`. Fails for a token that does not exist. |
| `transfer(to, id)` | `true` | Moves the caller's token `id` to `to`. |
| `transfer_from(from, to, id)` | `true` | Moves `id` from `from`, when the caller is `from`, approved for `id`, or an operator of `from`. |
| `safe_transfer(to, id)` | `true` | As `transfer`, then asks a receiving contract first: see [Safe transfers](#safe-transfers). |
| `safe_transfer_from(from, to, id)` | `true` | As `transfer_from`, then asks a receiving contract first. |
| `approve(approved, id)` | `true` | Lets `approved` move `id` once, replacing any earlier approval; the zero address clears it. The owner or an operator may. |
| `get_approved(id)` | address or nil | Who may move `id`. |
| `set_approval_for_all(operator, approved)` | `true` | Lets `operator` move everything the caller holds, now and later, or stops it. |
| `is_approved_for_all(owner, operator)` | `bool` | Whether `operator` is one of `owner`'s. |
| `token_uri(id)` | `string` | Where `id`'s metadata is: see [Metadata](#metadata). |
| `tokens_of(owner, start, count)` | ids | Up to `count` ids `owner` holds, from the `start`-th on, counting from 0; both may be left out. |

**Events:**

| Event | Data | When |
| --- | --- | --- |
| `Transfer` | `from`, `to`, `id` | A token moved. A mint comes from the zero address, a burn goes to it. |
| `Approval` | `owner`, `approved`, `id` | `approve` set who may move a token. A transfer clears it without an event. |
| `ApprovalForAll` | `owner`, `operator`, `approved` | `set_approval_for_all`. |

## The multi-token standard

**Required:** a multi-token collection exports all of these.

| Function | Returns | Does |
| --- | --- | --- |
| `balance_of(owner, id)` | `u256` | How much of `id` `owner` holds. |
| `transfer(to, id, amount)` | `true` | Moves `amount` of the caller's `id` to `to`. |
| `transfer_from(from, to, id, amount)` | `true` | Moves `amount` of `from`'s `id`, when the caller is `from` or an operator of `from`. |
| `safe_transfer(to, id, amount)` | `true` | As `transfer`, then asks a receiving contract first. |
| `safe_transfer_from(from, to, id, amount)` | `true` | As `transfer_from`, then asks a receiving contract first. |
| `set_approval_for_all(operator, approved)` | `true` | As the NFT standard's. There is no per-id approval. |
| `is_approved_for_all(owner, operator)` | `bool` | Whether `operator` is one of `owner`'s. |
| `token_uri(id)` | `string` | Where `id`'s metadata is. |
| `tokens_of(owner, start, count)` | ids | Up to `count` ids `owner` holds any of, as the NFT standard's. |

**Events:**

| Event | Data | When |
| --- | --- | --- |
| `TransferSingle` | `operator`, `from`, `to`, `id`, `amount` | Tokens moved: `operator` is the caller. A mint comes from the zero address, a burn goes to it. |
| `ApprovalForAll` | `owner`, `operator`, `approved` | `set_approval_for_all`. |

## Both standards

**Optional:** a collection may describe itself with `name()` and `symbol()`, as tokens do: a
name of 1 to 64 bytes, a symbol of 1 to 16, no control characters. A collection may also export
`total_supply()`, every token in an NFT collection, or `total_supply(id)`, how much of an id
exists, as the examples do.

**Failing:** a function that cannot do what was asked fails, so nothing it did is kept; it never
returns `false`. The examples say `no such token`, `insufficient balance` and `not the owner,
approved or an operator`.

**The caller** is `ctx.sender` in Lua and `sender()` in the SDK: the transaction's sender, or the
contract that called the collection.

**Listing what someone holds:** `tokens_of` pages through it, as there is no storage iteration
for a wallet to do it by. Ask with a `count` and go on from where the page ended until a page
comes back short. The examples answer at most 100 ids at once, and keep a holder's ids in a list
they swap the last into when one leaves, so the order changes as tokens move.

## Safe transfers

A plain `transfer` or `transfer_from` only moves a token. A safe one moves it, then, when `to` is
a contract's address, calls `on_nft_received(operator, from, id, amount)` on it (`amount` is 1
for an NFT) and fails unless it returns `true`. A contract that cannot handle a token, or exports
no such function, refuses it, so nothing is stuck there.
- **Which to use:** safe ones to send to a contract, as wallets do; plain ones for a contract
  pulling a token in itself, as the swap contract's `lock_nft` does with `transfer_from`, and to
  keys.
- **Reentrancy:** chainv4 contracts are not reentrant unless they say so, so the receiver cannot
  call back into the collection while it runs.
- **EVM contracts** are never asked: the receiver must be a native contract, `contract:0x…`.

## Beyond the standard: batches and royalties

The reference collections, Lua and Rust, export two more kinds of function. Neither is in the
standard's lists (`STANDARD_NFT_FUNCTIONS`, `STANDARD_MULTI_FUNCTIONS`), so a collection is
registered with or without them, and the chain's rules are unchanged.

**Batch transfers,** multi-token collections, as ERC-1155's `safeBatchTransferFrom`:
- `batch_transfer_from(from, to, packed)` and `safe_batch_transfer_from(from, to, packed)`;
- `packed` is bytes, 64 a pair, the id then its amount, 32 bytes each, big-endian, at most 100
  pairs, as calls take no lists;
- each pair moves as `transfer_from` moves one, with its own `TransferSingle`; one that cannot
  move fails the whole batch, and nothing moves;
- the safe kind asks a receiving contract for each pair, once all have moved.

**Royalties,** both kinds, as ERC-2981's:
- `royalty_info(id, price)` answers who a sale owes and how much: the receiver and
  `price × bps / 10,000`, rounded down; the zero address and 0 while none is set;
- `set_royalty(receiver, bps, id?)`, the owner's alone, sets it for every id, or for one, which
  then stands in for the collection's there; at most 10,000 basis points, and `RoyaltySet` says it;
- nothing makes a sale pay it: a marketplace reads it and pays it, as on Ethereum.

## Provable ownership

A collection keeps owners and amounts where any client can prove them against a header alone, as
tokens keep balances:
- **NFT:** in its `owners` [namespace](contracts.md#storage-namespaces), under the id in decimal,
  the owner's address text, such as `ed25519:0x3b6a…`. No entry is no owner.
- **Multi-token:** in its `balances` namespace, under the id in decimal, a colon and the
  holder's address text, the amount as a token balance is written: decimal text in Lua, 32 bytes
  little-endian in Rust. No entry is none.

A light client reads the key from `light::nft_owner_key(collection, id)` or
`light::multi_balance_key(collection, id, holder)`, proves the entry with
`GET /proofs/namespaces/{key}` and `check_state`, and reads it with `light::nft_owner` or
`light::token_balance`. This is how the [wallet](wallet.md) shows NFTs it proved are yours.

It is a convention, not something the chain checks: both standards' reference collections follow
it, and so should yours.

## Metadata

`token_uri(id)` names where `id`'s metadata is, JSON as ERC-721 lays it out:

```json
{
  "name": "Ape #1",
  "description": "An ape.",
  "image": "ipfs://bafy…/1.png",
  "attributes": [{ "trait_type": "Fur", "value": "Gold" }]
}
```

- **Where:** `https://`, `ipfs://CID/path`, which apps fetch through a gateway, or `data:`, the
  JSON or picture in the URI itself, so it lives on the chain.
- **Pictures:** `image` is such a URI too; `image_url` and `image_data`, an inline SVG, stand in
  for it.
- **Reading it:** `chainv4::nft::locate` says where a URI leads and `Metadata::read` reads the
  JSON, for apps to share.
- **The examples:** each token's own URI given at its mint, else a base URI the owner sets
  (`set_base_uri`) followed by the id, as many ERC-721s do.

## The collection registry

The chain keeps a registry of collections (`registry::collections`), so wallets and explorers can
list them, beside the [token registry](token-standard.md#the-token-registry) and in the same
column.

**What makes a collection:** exporting every function the chain names for one standard, in
`collections` (see [Genesis](genesis.md)): `{"nft": [...], "multi": [...]}`, usually the lists
above. One that exports the NFT list is an NFT collection, else one that exports the multi-token
list is a multi-token one. The NFT list holds the multi-token one, so a contract exporting both
is an NFT collection. A chain without `collections` keeps no registry.

**Turning it on for a running chain:** a chain whose genesis kept none turns it on by governance,
with `Change::Collections`. From the block that decides it on, deploys, upgrades and refreshes
register; contracts deployed before join at their next upgrade or refresh.

```sh
chainv4 send vote-change --vote yes --collections standard
```

**Getting in and out:** nothing to send, as for tokens. A deploy, upgrade or clone that leaves a
contract exporting a standard's functions registers it; one that leaves it short takes it off.

**Name and symbol:** those it exports are asked once, after `init` (or `migrate`), on a throwaway
copy of state, out of the deploy's fuel. One that fails, or answers outside the limits, fails the
deploy (`BadCollection`). A contract that is both a token and a collection is asked once, for
both.

**Asking again:** a collection renamed asks with `collections.refresh()` in Lua or
`refresh_collection()` in the SDK: `tokens.refresh()` and `refresh_token()` by another name, as
one request asks again for both registries.

### EVM collections

ERC-721 and ERC-1155 contracts on the chain's own [EVM](evm.md) are listed too, beside native ones.
Every contract the EVM creates, by a transaction's own deploy or by another contract's `CREATE`
or `CREATE2`, as a factory makes them, is asked what it is once the transaction stands, with
ERC-165's `supportsInterface`: one that says it is ERC-165 (`0x01ffc9a7` yes, `0xffffffff` no) and
ERC-721 (`0x80ac58cd`) is an NFT collection, one that says ERC-1155 (`0xd9b67a26`) a multi-token
one. Its `name()` and `symbol()`, which ERC-721's metadata extension has and many ERC-1155s add,
are kept when each is one string within the registry's limits, else left out
(`Contracts::refresh`).

Unlike a native collection's, these questions never fail the transaction, which runs as it would
on Ethereum: each runs on a throwaway copy of state with at most `EVM_QUESTION_GAS`, 60,000 gas,
ERC-165's 30,000 for `supportsInterface` and what the call costs, paid out of what the transaction
has left; when too little is left, or a question fails, the contract is not a collection. A
contract the EVM destroys leaves the registry. A chain without `collections` lists none.

What an account holds of one is proven as the EVM proves anything: its `ownerOf` or `balanceOf`,
run over proven state ([In the wallet](wallet.md#nfts)). Where to look comes from its logs.

**Reading it:**
- `GET /collections` lists them by symbol, ignoring ASCII case, with a search by how a symbol
  starts;
- `GET /contracts/{address}` says what the registry holds for a contract, in `collection`;
- `GET /status` says what makes a collection, in `collections` (see [API](api.md)).

## The factory

`contracts/lua/nft_factory.lua` makes a collection with one call, for anyone:
- **Set up once:** its deployer deploys `nft.lua` and `multi.lua` as templates and calls
  `setup(nft, multi)`.
- **Create:** `create(kind, name, symbol, base_uri)`, `kind` `nft` or `multi`, clones the
  template with [contracts.clone](contracts.md), naming the caller its owner, and returns where.
  The clone goes in the registry once the transaction succeeds.
- **List:** `count()` and `at(n)` give what it made; `Created(collection, kind, creator)` says so
  as it happens.
- **Found by the directory:** the CLI takes the factory the chain's
  [directory](governance.md#directory) names, `nft_factory`, unless given `--factory`.

## From the command line

```sh
chainv4 nft factory                                  # deploy the templates and the factory
chainv4 nft create nft "My Art" ART --base-uri ipfs://bafy…/
chainv4 nft collections --symbol ar                  # the registry
chainv4 nft mint contract:0x… ed25519:0x…            # the next id of an NFT collection
chainv4 nft mint contract:0x… ed25519:0x… --id 7 --amount 10
chainv4 nft show contract:0x… 7                      # its owner, or supply, and URI
chainv4 nft held contract:0x… ed25519:0x…            # what an address holds
chainv4 nft send contract:0x… ed25519:0x… 7 [--amount 3] [--safe]
chainv4 nft operator contract:0x… contract:0x… [--revoke]
```

Each takes `--node`, `--key` and the TLS flags as `chainv4 send` does.

## In the apps and in swaps

- **The wallet's NFTs page** lists what the account holds, each proven, with pictures once
  Settings lets it fetch them, and sends them: see [Wallet](wallet.md#nfts).
- **The explorer's NFTs pages** list the registry, and per collection a token's owner and URI,
  what an address holds and what moved: see [Explorer](explorer.md).
- **Atomic swaps** lock an NFT, or an amount of a multi-token id, on chainv4 or an EVM chain
  (ERC-721 and ERC-1155): see [Atomic swaps](swaps.md#nfts).

## Caveats

- **The registry checks exports, not behaviour.** Anyone can deploy something called "Apes"
  exporting the functions: trust a collection's address, not its name or symbol.
- **Names and symbols are kept, not read live**, as for tokens: refresh after changing them.
- **Metadata is not on the chain** unless it is a `data:` URI. A server or IPFS gateway can
  change or lose it, and every fetch tells them which tokens you look at.
- **`tokens_of` is the collection's own word.** The wallet proves each NFT's owner before showing
  it; it cannot prove the list is complete.
