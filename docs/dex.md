# DEX

Two decentralized exchanges, the same idea twice: constant-product pools in the manner of Uniswap
V2, written afresh. A pool holds two assets and trades one for the other at whatever price keeps
the product of what it holds from falling, less a 0.3% fee that stays in the pool for whoever
provided it.

| | Native DEX | EVM DEX |
| --- | --- | --- |
| Code | `contracts/lua/dex.lua`, with `contracts/lua/lp_token.lua` | `contracts/evm/Dex.sol`: `WDEV`, `DexFactory`, `DexPair`, `DexRouter` |
| Its pools hold | the coin, token-standard tokens, and ERC-20s on chainv4's EVM, in any mix | ERC-20s, the coin wrapped as WDEV |
| A pool's shares | a token-standard token, cloned for each pool | the pair itself, an ERC-20 |
| Found by | the name `dex` under the chain's suffix, such as `dex.dev`, else `--dex-contract` | `--dex-router` |
| Deadlines | Unix milliseconds | Unix seconds |

The `dex` cargo feature (default on) holds what the apps quote with, `src/dex.rs`. The wallet's
**Trade** page and `chainv4 dex` use both DEXes.

## Pools and prices

- **A pool's price** is what it holds of one asset per the other. A trade of `x` into a pool
  holding `X` and `Y` pays out `x·997·Y / (X·1000 + x·997)` (`dex::amount_out`, Uniswap V2's
  `getAmountOut`), so the product of what it holds grows by the fee.
- **Adding** takes both assets at the pool's ratio, at most the amounts offered and at least the
  least asked, and mints shares:
  - for a new pool, the square root of the product, less 1,000 kept back forever so it is never
    empty;
  - after that, in proportion to what was added.
- **The first deposit opens a pool,** at the price its two amounts set.
- **Taking out** burns shares, and pays their part of each asset.
- **A route** is the pool that holds both assets, or a path through one middle pool, whichever
  pays out more (`dex::best_route`).
- **Price impact** is how far a trade's rate falls below the pools' own: the fee, and its own
  weight on the price.
- **Slippage** sets the least a trade takes (`dex::least_out`); a trade that would pay less fails
  instead.

## The native DEX: dex.lua

One contract holds every pool. A side of a pool is:
- **the coin,** named by the zero contract address, `contract:0x00…00`, and sent with the call.
  The zero key, `ed25519:0x00…00`, is no valid key, and transactions refuse to name it;
- **a token-standard token,** moved with `transfer_from`: approve the DEX first;
- **an ERC-20 on chainv4's EVM,** by its `evm:0x…` address, moved with `transferFrom` through
  `evm.call`: approve the DEX's EVM address, which the EVM sees it at. A token that moves less
  than asked, by a fee on transfer, is refused.

| Function | What it does |
| --- | --- |
| `setup(template)` | Once, by whoever deployed it: the share token every pool clones, a deployed `lp_token.lua`. |
| `create_pool(a, b)` | Opens an empty pool, and returns its share token. |
| `add_liquidity(a, b, desired_a, desired_b, least_a, least_b, deadline)` | Adds at the pool's price, opening it if it is new; the coin beyond what went in is sent back. Returns what went in and the shares minted. |
| `remove_liquidity(a, b, shares, least_a, least_b, deadline)` | Burns the caller's shares, and pays their part of each. |
| `swap(asset_in, asset_out, amount_in, least_out, deadline, middle)` | Trades through one pool, or two by way of `middle`. |
| `quote(asset_in, asset_out, amount_in, middle)` | What that would pay now. |
| `pool(a, b)` | What a pool holds of each, in the order asked, and its share token. |
| `pool_count()`, `pool_at(index)` | The pools, in the order they opened: each one's assets, what it holds of each, and its share token. |

**Events:** `PoolCreated(a, b, share)`, `Mint`, `Burn`, `Swap(asset_in, asset_out, sender,
amount_in, amount_out)`, and after every change `Sync(a, b, holds_a, holds_b)`, what the pool
holds after it. The wallet draws a pool's price from its `Sync` events.

### Share tokens, by cloning

Each pool's shares are their own token-standard token. The DEX makes it with `contracts.clone`:
a copy of `lp_token.lua`'s code at an address fixed by the DEX and the pool's two assets. It is
in the token registry like any token, and only the DEX mints and burns it. Holders move it as
any token.

**`contracts.clone(template, salt, ...)`** creates a contract running another's code, the way a
factory does:
- the copy is deployed by the calling contract, at `contracts.cloned_at(salt)`;
- its `init` runs with the arguments, the caller as its sender;
- nothing of the template's storage or balance comes along;
- a clone that is a token goes in the registry once the transaction succeeds;
- one salt makes one clone, and `contracts.cloned_at(salt)` finds it again later.

Templates are Lua or WebAssembly; WebAssembly contracts clone through the host functions `clone`
and `cloned_at`, `clone_contract` and `cloned_at` in the SDK.

It costs 5,000 fuel, 1 more for every 16 bytes of the template's compiled code, and what `init`
burns. Clones show in a transaction's trace as `clone` hops.

**This is a consensus change:** nodes without it refuse contracts that clone, so every node
updates together.

## The EVM DEX: Dex.sol

- **WDEV** wraps the coin one for one: `deposit` and `withdraw`.
- **DexFactory** opens one `DexPair` for any two tokens, with CREATE2 by the pair:
  `createPair`, `getPair`, `allPairs`, `allPairsLength`.
- **DexPair** holds the two tokens, is the shares' ERC-20 itself, and settles what was sent to it:
  `mint`, `burn`, `swap`, `sync`, `getReserves`. It has no flash swaps.
- **DexRouter** does it all in one call, wrapping and unwrapping the coin on the way:
  - `addLiquidity` and `addLiquidityDEV`;
  - `removeLiquidity` and `removeLiquidityDEV`, the shares approved to it;
  - `swapExactTokensForTokens`, `swapExactDEVForTokens` and `swapExactTokensForDEV`, along a
    path of pairs;
  - `getAmountsOut`, `getAmountOut`, `getReserves`.

They are built with solc 0.8 for Paris. `WDEV.bin`, `DexFactory.bin` and `DexRouter.bin` are
what the CLI deploys; DexPair's code is inside the factory's.

## Deploying

```sh
chainv4 dex deploy --node … --key owner.key          # lp_token.lua, dex.lua, setup: prints the DEX
chainv4 dex deploy --evm --node … --key owner.key    # WDEV, the factory, the router
```

Then tell clients:
- start the node with `--dex-contract contract:0x…` and `--dex-router 0x…` (in Docker,
  `DEX_CONTRACT` and `DEX_ROUTER` in `.env`), which `GET /status` names as `dex.contract` and
  `dex.router`;
- or register the name `dex` (`dex.dev`) for the native DEX, which the apps look up first.

## From the command line

Assets are `coin`, or a token's address: `contract:0x…`, `evm:0x…` or `0x…`. Amounts are in
smallest units. `--evm` uses the EVM DEX, through the router the node names, or `--router`.

```sh
chainv4 dex pools
chainv4 dex quote coin contract:0x… 1000000             # the route, what comes out, price impact
chainv4 dex add coin contract:0x… 1000000 4000000       # approves the token first; opens it if new
chainv4 dex swap coin contract:0x… 10000 --slippage 1   # at least the quote less 1%
chainv4 dex remove coin contract:0x… 500000             # shares out
chainv4 dex --evm swap coin 0x… 5000
```

## In the wallet

The **Trade** page, on an online install, with **Native pools** and **EVM pools** when the node
names both:
- **Trade:**
  - pick what you pay and what you get, and the amount;
  - the page shows the quote, the route, the price impact (amber past 1%, red past 5%) and the
    least you take at your slippage (0.5% by default);
  - **Let the DEX move it** approves a token first, then **Trade**.
- **Pools:**
  - each pool with what it holds, its price, and what your shares are worth;
  - a native pool's price chart, from its `Sync` events, on a node with an event index;
  - **Add**, at the pool's ratio, and **Take out**, all or some of your shares (on the EVM, the
    router takes them, approved first).
- **Open a pool:** two assets and the first deposit of each, whose ratio is its opening price.

Every one of these is a call the Send page reviews and signs, on the offline signer too.

## Gas estimates for nested calls

A router call runs calls inside calls, and each inner call gets only 63/64 of the gas left. So a
transaction can need more gas than it uses: what it used, plus a quarter, may still starve the
innermost call, which then reverts with no reason. The node checks its estimate:
- `POST /offer` tries the limit it would give with a second dry run, and raises it until a run
  succeeds;
- `eth_estimateGas` does the same.

MetaMask, the wallet and the CLI all get limits that work.

## Tested

- **The native DEX, through whole blocks:**
  - a coin–token pool opened, its shares a registered token;
  - trades both ways, and through two pools to an ERC-20;
  - half the shares taken out;
  - refusals: a moved price, a passed deadline, a missing pool, a pool of one asset, minting
    shares by hand, setting up twice.
- **Cloning,** for Lua and WebAssembly templates, and from a WebAssembly factory: the address,
  init's sender, one salt one clone, a failing init keeping nothing.
- **The EVM DEX, through whole blocks:** wrapping, adding, trading along a path back to the
  coin, shares out, and the router's refusals.
- **`chainv4 dex` against a running node:** both DEXes deployed and traded on, with no gas
  given.
- **The routing and pricing:** `src/dex.rs`.
- **The Trade page's pieces:** prices from `Sync` events, the router's calldata, the EVM pairs
  read with WDEV as the coin.
