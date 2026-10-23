# Token standard

Tokens on chainv4 are contracts, as on Ethereum: a token keeps balances in its own storage, and
moving tokens never moves the chain's native coin. This page is the interface every token
follows, so wallets, exchanges and other contracts can work with any of them. It follows
ERC-20, adjusted to chainv4's types.

There are two reference implementations, kept in step by one shared test:
[contracts/lua/token.lua](../contracts/lua/token.lua) and
[contracts/examples/token](../contracts/examples/token/src/lib.rs).

## Types

- **Amounts** are `u256`, in the token's smallest unit. Functions that take one also accept any
  non-negative whole number, since Lua passes its numbers as floats.
- **Addresses** are any address: a key's or a contract's.
- **The zero address**, `ed25519:0x00…00`, stands for "nobody": the source of a mint and the
  destination of a burn.

## Functions

A token exports all of these.

| Function | Returns | Does |
| --- | --- | --- |
| `name()` | `string` | The token's name, such as "Example Token". |
| `symbol()` | `string` | Its ticker, such as "EXT". |
| `decimals()` | whole number | How many decimals a display divides by: 18 means 10¹⁸ units make one token. Lua returns a number, Rust a `u64`. |
| `total_supply()` | `u256` | Every token in existence. |
| `balance_of(owner)` | `u256` | What `owner` holds. |
| `transfer(to, amount)` | `true` | Moves `amount` of the caller's tokens to `to`. |
| `approve(spender, amount)` | `true` | Lets `spender` move up to `amount` of the caller's tokens, replacing any earlier allowance. |
| `allowance(owner, spender)` | `u256` | What `spender` may still move for `owner`. |
| `transfer_from(from, to, amount)` | `true` | Moves `amount` of `from`'s tokens to `to`, spending the caller's allowance from `from`. |

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
- **Tokens are only a convention.** The chain does not know which contracts are tokens; a client
  has to be told an address, or check that it exports these functions.
