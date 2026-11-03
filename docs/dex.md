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
| Found by | the chain's [directory](governance.md#directory), `dex`, else `--dex-contract` | the directory's `dex_router`, else `--dex-router` |
| Deadlines | Unix milliseconds | Unix seconds |

The `dex` cargo feature (default on) holds what the apps quote with, `src/dex.rs`. The wallet's
**Trade** page and `chainv4 dex` use both DEXes.

## Pools and prices

- **A pool's price** is what it holds of one asset per the other. A trade of `x` into a pool
  holding `X` and `Y` pays out `x·(10000−f)·Y / (X·10000 + x·(10000−f))` (`dex::amount_out`,
  Uniswap V2's `getAmountOut`), `f` the chain's fee, so the product of what it holds grows by it.
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

## The fee

One fee for both DEXes, the chain's: in hundredths of a percent, 30 (0.3%) unless genesis's `dex`
names another, and `Change::Dex` changes it from the block after its decision.

- **The native DEX** reads it as it trades, `chain.dex_fee()`, from the call's block.
- **The EVM DEX** reads it through the native precompile, `dexFeeBps()` at `0x…4e43`: the pair's
  swap checks the product by it, and the router quotes by it, so `getAmountOut` is `view`, no
  longer `pure`.
- **The apps** take it from `GET /status`'s `dex_fee_bps`, and quote, show and sum fees by it.
- **Pools already open** charge the new fee from the next block; nothing is redeployed.

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
  `dex.router` while the directory names none, and shows as `operator` either way;
- or, better, have governance name them in the chain's [directory](governance.md#directory),
  `dex` and `dex_router`, which the apps take first and the wallet proves, and which win over
  the flags in `GET /status` too.

## From the command line

Assets are `coin`, or a token's address: `contract:0x…`, `evm:0x…` or `0x…`. Amounts are in
smallest units.
- **Which DEX:** the native one the chain's [directory](governance.md#directory) names as `dex`,
  else the one the node was started with (`--dex-contract`), as `GET /status` says, or `--dex`.
  `--evm` uses the EVM DEX instead, through the directory's `dex_router`, else the node's
  `--dex-router`, or `--router`; `--dex` is not used with it.
- **Quotes** go by the chain's fee, `GET /status`'s `dex_fee_bps`, through `dex::amount_out`.

```sh
chainv4 dex pools
chainv4 dex quote coin contract:0x… 1000000             # the route, what comes out, price impact
chainv4 dex add coin contract:0x… 1000000 4000000       # at most these, at the pool's ratio; opens it if new
chainv4 dex swap coin contract:0x… 10000 --slippage 1   # at least the quote less 1%
chainv4 dex remove coin contract:0x… 500000             # shares out
chainv4 dex --evm swap coin 0x… 5000
```

**`add` takes at most the amounts given,** as Uniswap V2's router does:
- It reads what the pool holds and works out the largest pair at its ratio that fits both
  (`dex::add_amounts`), rounding each step down as `dex.lua`'s `add_liquidity` and the router's
  `_amounts` do, so the pair is exactly what the contract takes at those reserves.
- It says that pair before it sends, with how much of the amount given it leaves, and the least of
  each it takes should the price move first: the pair less `--slippage` (0.5% unless told, 0 to 50).
- It sends the amounts given as the most and that least. A pool whose price moved past the slippage
  refuses it; neither contract takes more than the most, and both send back the coin sent beyond
  what goes in.
- A new pool, or one holding nothing, opens at the amounts as given: their ratio is its price.

**Approvals go out only when short.** `swap`, `add` and, on the EVM, `remove` first read what the
DEX may already move of each token for you: a native token's `allowance(owner, spender)`, an ERC-20's
`allowance(address,address)` for the native DEX's EVM address or the router. They approve only when
that is less than the amount (for `add`, the most given), and say so either way. An allowance that
does not read is taken as none.

**Before this,** `add` asked for at least each amount given less the slippage, so amounts off the
pool's ratio by more than 0.5% were refused, `the pool's price moved: less b than the least`, after
its approval had gone out; and each run approved again, whatever was approved already. The test
chain's notes said to work the second amount out from `dex pools` by hand.

## In the wallet

The **Trade** page, on an online install, with **Native** and **EVM** to switch when the node names
both, and two tabs:
- **Trade:**
  - what you pay over what you get, each with its asset and balance, and a button between them
    that swaps them; on a screen wide enough, such as a phone on its side, they sit side by side;
  - once there is a quote, the rate, the price impact (amber past 1%, red past 5%), the fee, the
    minimum received at your slippage and the route, through one pool or two;
  - slippage, 0.1%, 0.5% by default, 1%, or typed;
  - one button, which says what it waits for until it can trade; **Let the DEX move it** comes
    above it for a token, which must be approved first.
- **Pools:**
  - each pool with what it holds and a mark on those you have shares in; **Manage** opens one;
  - its price, what your shares are worth, a native pool's price chart, from its `Sync` events, on
    a node with an event index;
  - **Add**, at the pool's ratio, and **Take out**, all or some of your shares (on the EVM, the
    router takes them, approved first).
- **Open a new pool**, folded under the pools: two assets and the first deposit of each, whose ratio
  is its opening price.

Every one of these is a call the Send page reviews and signs, on the offline signer too.

## In the explorer

The **DEX** page lists both DEXes' pools with what each holds, its price, and its volume and fees
over about a day and a week; a pool opens its price chart and its latest trades, adds and
removals. Volumes come from the last 500 trades at most, by the event index: a node without
`--index-events` shows pools and prices alone. See [Explorer](explorer.md).

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
  given; adds far off each pool's ratio, either way, at no slippage, taking the pair worked out;
  an approval only when the allowance is short, counted by the nonce.
- **Adding off the ratio, through whole blocks:** each DEX, both ways round and through
  `addLiquidityDEV`, takes the pair `dex::add_amounts` works out to the unit with that pair as its
  least, sends back the coin beyond it, and refuses the old least, the amounts given less 0.5%.
- **The routing and pricing:** `src/dex.rs`.
- **The Trade page's pieces:** prices from `Sync` events, the router's calldata, the EVM pairs
  read with WDEV as the coin.
