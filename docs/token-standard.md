# Token standard

Tokens on chainv4 are contracts, as on Ethereum: a token keeps balances in its own storage, and
moving tokens never moves the chain's native coin. This page is the interface every token
follows, so wallets, exchanges and other contracts can work with any of them. It follows
ERC-20, adjusted to chainv4's types. The chain keeps a [registry](#the-token-registry) of every
contract that exports it.

There are two reference implementations, kept in step by one shared test:
[contracts/lua/token.lua](../contracts/lua/token.lua) and
[contracts/examples/token](../contracts/examples/token/src/lib.rs).

## Types

- **Amounts** are `u256`, in the token's smallest unit. Functions that take one also accept any
  non-negative whole number, such as an `i64` a Lua contract passes.
- **Addresses** are any address: a key's or a contract's.
- **The zero address**, `ed25519:0x00…00`, stands for "nobody": the source of a mint and the
  destination of a burn.

## Functions

**Required:** a token exports all of these. As in ERC-20, they are what makes a contract a
token.

| Function | Returns | Does |
| --- | --- | --- |
| `total_supply()` | `u256` | Every token in existence. |
| `balance_of(owner)` | `u256` | What `owner` holds. |
| `transfer(to, amount)` | `true` | Moves `amount` of the caller's tokens to `to`. |
| `approve(spender, amount)` | `true` | Lets `spender` move up to `amount` of the caller's tokens, replacing any earlier allowance. |
| `allowance(owner, spender)` | `u256` | What `spender` may still move for `owner`. |
| `transfer_from(from, to, amount)` | `true` | Moves `amount` of `from`'s tokens to `to`, spending the caller's allowance from `from`. |

**Optional:** a token may describe itself with these, as in ERC-20. Both examples export them
all.

| Function | Returns | Does |
| --- | --- | --- |
| `name()` | `string`, 1 to 64 bytes | The token's name, such as "Example Token". |
| `symbol()` | `string`, 1 to 16 bytes | Its ticker, such as "EXT". |
| `decimals()` | whole number, 0 to 77 | How many decimals a display divides by: 18 means 10¹⁸ units make one token. Lua returns a number, Rust a `u64`. |

Names and symbols hold no control characters.

A function that cannot do what was asked fails, so nothing it did is kept; it never returns
`false`. Failure messages are not part of the standard, but the examples say
`insufficient balance` and `insufficient allowance`.

"The caller" is `ctx.sender` in Lua and `sender()` in the SDK: the transaction's sender, or the
contract that called the token.

## Events

| Event | Data | When |
| --- | --- | --- |
| `Transfer` | `from`, `to`, `amount` | Tokens moved, including by `transfer_from`. A mint comes from the zero address, a burn goes to it. |
| `Approval` | `owner`, `spender`, `amount` | `approve` set an allowance. |

`from`, `to`, `owner` and `spender` are addresses; `amount` is a `u256`. A client can rebuild
every balance from `Transfer` events alone.

## Provable balances

A token keeps each holder's balance where any client can find it, and prove it against a header
alone: in its `balances` [namespace](contracts.md#storage-namespaces), under the holder's address
text, such as `ed25519:0x3b6a…`.
- **Lua** writes the balance as decimal text: `storage.namespace("balances")` and
  `balances:set(owner:text(), tostring(amount))`.
- **Rust** writes it as 32 bytes, little-endian: `storage::namespace(b"balances")`, keyed by
  `owner.text().as_bytes()`.

A light client reads the key from `light::token_balance_key(token, holder)`, proves the entry
with `GET /proofs/namespaces/{key}` and `check_state`, and reads either encoding with
`light::token_balance`. No entry is a balance of zero. This is how the [wallet](wallet.md) shows
token balances it proved, without trusting `balance_of` answers from a node.

It is a convention, not something the chain checks: a token that keeps balances elsewhere still
works, but no light client can prove what it holds. Both reference implementations follow it.

## The token registry

The chain keeps a registry of tokens (`registry::tokens`), so wallets can list them and tell a
token from any other contract.

**What makes a token:** exporting every function genesis names in `token_functions` (see
[Genesis](genesis.md)). The dev chains name the six required above, ERC-20's; a chain may name
others, or none to keep no registry.

**Getting in and out:** nothing to send. The deploy or upgrade itself keeps the registry:
- **In:** a deploy, or an upgrade, that leaves a contract exporting them all registers it.
- **Out:** an upgrade that leaves it short of one takes it off.

So a contract is in the registry exactly when it exports them.

**Name, symbol and decimals:** those of the optional three a token exports are asked once, after
`init` (or `migrate`).
- **A throwaway copy:** they run on a copy of state, so whatever they write or emit is gone.
- **Paid for:** what they burn is the deploy's or upgrade's fuel.
- **Answers must fit:** one that fails, or answers outside the limits above, fails the deploy or
  upgrade (`BadToken`), so what the registry holds can be trusted. One out of fuel fails it with
  `OutOfFuel`.

**Asking again:** a token whose name, symbol or decimals change, say by an owner's `rename`, asks
the registry to ask again: `tokens.refresh()` in Lua, `refresh_token()` in the SDK.
- **When:** once the transaction succeeds, the chain asks the same three questions a deploy
  does, out of the transaction's fuel, and keeps the new answers.
- **Only if it succeeds:** a request from a call that fails, or under one that fails, is dropped
  with it.
- **Answers must fit here too:** one that does not fails the whole transaction, so the rename is
  gone with it.
- **Who:** the contract running, or under `delegate` the one delegating. For a contract that is
  not a token it does nothing.

```lua
function M.rename(name)
    assert(ctx.sender == owner(), "only the owner renames")
    storage.set("name", name)
    tokens.refresh()
end
```

**Reading it:**
- `GET /tokens` lists tokens by symbol, ignoring ASCII case, with a search by how a symbol starts;
- `GET /contracts/{address}` says what the registry holds for a contract, in `token`;
- `GET /status` says what makes a token, in `token_functions` (see [API](api.md)).

## Beyond the standard

Anything else is up to the token. The examples add:

- `init`, run once by the chain at deploy, which makes the deployer the owner and mints the
  initial supply to them;
- `mint(to, amount)`, for the owner only;
- `check_perms(permission, who)`, which lets only the owner upgrade the token.

## Caveats

- **Changing an allowance can be raced.** As with ERC-20, a spender who sees `approve(spender,
  new)` coming may spend the old allowance first. Set it to zero before changing it, or have the
  spender use `transfer_from` only for amounts agreed off-chain.
- **The registry checks exports, not behaviour.** A contract that exports the required functions
  is listed, whatever they do. Anyone can deploy one called "Example Token" with the symbol EXT,
  as symbols are not unique: trust an address, not a symbol.
- **Name, symbol and decimals are kept, not read live.** The registry holds what they answered at
  the deploy, the last upgrade or the last refresh. A token whose answers change without asking
  for a refresh keeps showing the old ones.
