# DEX

Two decentralized exchanges, the same idea twice: constant-product pools in the manner of Uniswap
V2, written afresh. A pool holds two assets and trades one for the other at whatever price keeps
the product of what it holds from falling, less its fee. A pool's fee is its tier, picked when it
opens from those the chain offers: of each fee, the chain's protocol share is burned and the rest
stays in the pool for whoever provided it.

| | Native DEX | EVM DEX |
| --- | --- | --- |
| Code | `contracts/lua/dex.lua`, with `contracts/lua/lp_token.lua` | `contracts/evm/Dex.sol`: `WDEV`, `DexFactory`, `DexPair`, `DexRouter` |
| Its pools hold | the coin, token-standard tokens, and ERC-20s on chainv4's EVM, in any mix | ERC-20s, the coin wrapped as WDEV |
| A pool's shares | a token-standard token, cloned for each pool | the pair itself, an ERC-20 |
| Found by | the chain's [directory](governance.md#directory), `dex`, else `--dex-contract` | the directory's `dex_router`, else `--dex-router` |
| Deadlines | Unix milliseconds | Unix seconds |

The `dex` cargo feature (default on) holds what the apps quote with, `src/dex.rs`. The wallet's
**Trade** page and `chainv4 dex` use both DEXes, and `contracts/lua/router.lua` trades across both
at once.

## Pools and prices

- **A pool's price** is what it holds of one asset per the other. A trade of `x` into a pool
  holding `X` and `Y` pays out `x·(10000−f)·Y / (X·10000 + x·(10000−f))` (`dex::amount_out`,
  Uniswap V2's `getAmountOut`), `f` the pool's tier, so the product of what it holds grows by it.
- **Adding** takes both assets at the pool's ratio, at most the amounts offered and at least the
  least asked, and mints shares:
  - for a new pool, the square root of the product, less 1,000 kept back forever so it is never
    empty;
  - after that, in proportion to what was added.
- **The first deposit opens a pool,** at the price its two amounts set.
- **Taking out** burns shares, and pays their part of each asset.
- **A route** is the pool that holds both assets, or a path through one middle pool, at whichever
  tiers pay out the most (`dex::best_route`): every pool of a pair, at every tier, is weighed.
- **A split** divides a trade between the two DEXes where that pays more than either alone
  (`dex::best_split`): it tries each twentieth to the native DEX, then the hundredths around the
  best, each part along its own DEX's best route.
- **Price impact** is how far a trade's rate falls below the pools' own: the fee, and its own
  weight on the price.
- **Slippage** sets the least a trade takes (`dex::least_out`); a trade that would pay less fails
  instead.

## Fee tiers and the burned share

The chain's `dex` settings, from genesis and `Change::Dex`, both DEXes alike:

```json
"dex": { "tiers": [5, 30, 100], "protocol_permille": 0 }
```

- **`tiers`** are the fees a new pool may open at, in hundredths of a percent: one to four of
  them, 1 to 1,000 each, smallest first. Unset, 0.05%, 0.3% and 1%.
  - A pair has at most one pool at each tier, so a pair may have several.
  - A pool's tier is fixed when it opens. It keeps trading at it when a vote takes that tier off
    the list; only new pools are held to the list.
  - The tier is the pool's whole fee, its providers' and the protocol's share together.
- **`protocol_permille`** is the share of each trade's fee that is burned, in thousandths: 0 unless
  set, 1,000 at most. Of a fee of `x·f/10000`, `protocol_permille/1000` of it, rounded down, leaves
  the pool, so what it holds grows by the rest.
  - **The native DEX** burns the coin out of the supply with `accounts.burn`, a token-standard
    token by sending it to the zero address, `ed25519:0x00…00`, and an ERC-20 to `0x…dEaD`.
  - **The EVM DEX** sends either token's share to `0x…dEaD`.
- **Both change from the block after the decision.** No pool is redeployed.
- **Contracts read them** the same way: Lua's `chain.dex()`, `{ tiers = {…}, protocol_permille }`;
  the EVM's native precompile at `0x…4e43`, `dexTier(uint256 fee)` (1 if a new pool may pick it,
  else 0) and `dexProtocolPermille()`.
- **The apps** take them from `GET /status`'s `dex_tiers` and `dex_protocol_permille`, and each
  pool's own tier from the pool: the native DEX's `pool_at`, an EVM pair's `fee()`.
- **Before tiers,** one fee, `{"fee_bps": 30}`, held for every pool. Genesis files and changes that
  still say it are read as that one tier.

## The native DEX: dex.lua

One contract holds every pool. A side of a pool is:
- **the coin,** named by the zero contract address, `contract:0x00…00`, and sent with the call.
  The zero key, `ed25519:0x00…00`, is no valid key, and transactions refuse to name it;
- **a token-standard token,** moved with `transfer_from`: approve the DEX first;
- **an ERC-20 on chainv4's EVM,** by its `evm:0x…` address, moved with `transferFrom` through
  `evm.call`: approve the DEX's EVM address, which the EVM sees it at.

Either kind of token that moves less than asked, by a fee on transfer or a balance that lies, is
refused: the DEX reads its own balance before and after, and a pool's reserves are what arrived.
`remove_liquidity` writes the pool's new reserves before it pays anything out, so whatever a
payment calls finds the pool as it stands after (since 2026-11-04, a DEX deployed from then).

| Function | What it does |
| --- | --- |
| `setup(template)` | Once, by whoever deployed it: the share token every pool clones, a deployed `lp_token.lua`. |
A pool is a pair and a tier: every function names both, the tier (`fee`) in hundredths of a
percent, as a number or any of the integer types.

| Function | What it does |
| --- | --- |
| `create_pool(a, b, fee)` | Opens an empty pool at a tier the chain offers, and returns its share token. |
| `add_liquidity(a, b, fee, desired_a, desired_b, least_a, least_b, deadline)` | Adds at the pool's price, opening it if it is new; the coin beyond what went in is sent back. Returns what went in and the shares minted. |
| `remove_liquidity(a, b, fee, shares, least_a, least_b, deadline)` | Burns the caller's shares, and pays their part of each. |
| `swap(asset_in, asset_out, fee, amount_in, least_out, deadline, middle, middle_fee)` | Trades through one pool, or two by way of `middle`, the second at `middle_fee`. |
| `quote(asset_in, asset_out, fee, amount_in, middle, middle_fee)` | What that would pay now. |
| `flash(a, b, fee, amount_a, amount_b, to, data)` | Lends what a pool holds within the call: see [Flash swaps](#flash-swaps). |
| `repay()` | The coin a flash loan pays back, sent with the call while the loan is out. |
| `burn(asset, amount)` | Burns what the DEX itself holds beyond its pools: how the protocol's share goes. Only the DEX calls it. |
| `pool(a, b, fee)` | What a pool holds of each, in the order asked, its share token, and its two cumulative prices as of now. |
| `pool_count()`, `pool_at(index)` | The pools, in the order they opened: each one's assets, tier, what it holds of each, and its share token. |

**Events:** `PoolCreated(a, b, fee, share)`, `Mint(a, b, sender, amount_a, amount_b, shares,
fee)`, `Burn(…, fee)`, `Swap(asset_in, asset_out, sender, amount_in, amount_out, fee)`,
`Flash(a, b, to, amount_a, amount_b, paid_a, paid_b)`, `Burned(asset, amount)`, and after every
change `Sync(a, b, holds_a, holds_b, fee)`, what the pool holds after it. The tier comes last, so a
pair's pools are told apart; events from before tiers lack it. The wallet and the explorer draw a
pool's price from its `Sync` events.

### Prices over time

Each pool sums its prices as Uniswap V2's pairs do, for a time-weighted average that one block's
trades cannot move far:
- **Native:** each side's price in the other, in 10^18 fixed point, times the milliseconds it held,
  summed since the pool opened and wrapping past 2^256. `pool(a, b, fee)`'s last two values are the
  sums as of the call. Read them twice; the difference over the milliseconds between is the
  average.
- **EVM:** `price0CumulativeLast` and `price1CumulativeLast`, UQ112x112 times seconds as V2 keeps
  them, with `getReserves()`'s timestamp to bring them up to now.

### Flash swaps

Both DEXes lend a pool's holdings within one call, as Uniswap V2 does: the borrower gets the
assets first and must, before its callback returns, have paid back enough that the pool's product,
less the fee on what came back, is no less than before. It may repay in either asset, or both.
The fee, and the burned share of it, are a trade's.

- **Native:** `flash(a, b, fee, amount_a, amount_b, to, data)` sends the amounts to `to`, a contract,
  and calls `to.on_flash(sender, a, b, amount_a, amount_b, data)`, which returns `paid_a, paid_b`,
  what it pays back of each. The coin's part comes from what it sent the DEX's `repay`, the one
  function open while the loan is out, and anything sent past it is returned; a token's the DEX
  takes from it with `transfer_from`, so approve the DEX first. A pool lends less than it holds, one loan at a time, and every other function of the DEX
  is refused until it is repaid.
- **EVM:** `DexPair.swap(amount0Out, amount1Out, to, data)` with `data` not empty calls
  `IDexCallee(to).dexCall(sender, amount0Out, amount1Out, data)` after sending, and settles what
  came back as any swap does. `contracts/evm/FlashBorrower.sol` is the tests' borrower.

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
- **DexFactory** opens one `DexPair` for any two tokens at each tier, with CREATE2 by the pair and
  tier: `createPair(a, b, fee)`, refused for a tier the chain does not offer, `getPair[a][b][fee]`,
  `allPairs`, `allPairsLength`.
- **DexPair** holds the two tokens, is the shares' ERC-20 itself, and settles what was sent to it:
  `mint`, `burn`, `swap(amount0Out, amount1Out, to, data)`, `sync`, `getReserves`; `fee()`, its
  tier, fixed at creation; `price0CumulativeLast` and `price1CumulativeLast`.
- **DexRouter** does it all in one call, wrapping and unwrapping the coin on the way:
  - `addLiquidity(a, b, fee, …)` and `addLiquidityDEV(token, fee, …)`;
  - `removeLiquidity(a, b, fee, …)` and `removeLiquidityDEV(token, fee, …)`, the shares approved
    to it;
  - `swapExactTokensForTokens`, `swapExactDEVForTokens` and `swapExactTokensForDEV`, along a
    path of pairs with a tier for each step, `uint256[] fees` after the path;
  - `getAmountsOut(amountIn, path, fees)`, `getAmountOut(amountIn, reserveIn, reserveOut, fee)`,
    `pure` again, and `getReserves`.

They are built with solc 0.8 for Paris. `WDEV.bin`, `DexFactory.bin` and `DexRouter.bin` are
what the CLI deploys; DexPair's code is inside the factory's.

## Across both DEXes: router.lua

`contracts/lua/router.lua` trades part of an amount through the native DEX and the rest through
the EVM's router, in one transaction, all or nothing, the least out checked on what both paid
together. The apps work the split out (`dex::best_split`) and send it.

| Function | What it does |
| --- | --- |
| `setup(dex, evm_router, wdev)` | Once, by whoever deployed it: the two DEXes. |
| `route(asset_in, asset_out, amount_in, least_out, deadline, native_amount, fee, middle, middle_fee, evm_amount, evm_route)` | Trades `native_amount` on the native DEX, at `fee` and by way of `middle` if given, and `evm_amount` along `evm_route`, the ABI encoding of `(address[] path, uint256[] fees)` with WDEV for the coin. The two add up to `amount_in`. |

Approve the router for a token first; the coin is sent with the call. It emits `Routed(asset_in,
asset_out, sender, amount_in, native_amount, evm_amount, out)`.

## Deploying

```sh
chainv4 dex deploy --node … --key owner.key          # lp_token.lua, dex.lua, setup: prints the DEX
chainv4 dex deploy --evm --node … --key owner.key    # WDEV, the factory, the router
chainv4 dex deploy-router --node … --key owner.key   # router.lua, set up with both: prints it
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
- **Quotes** go by each pool's own tier, through `dex::amount_out`, over every tier's pools.
- **`--fee`** names a pool's tier for `add` and `remove`, in hundredths of a percent: 30 unless
  told. A new pool must take one the chain offers.
- **`swap --split`** divides the trade between both DEXes through router.lua, named with
  `--split-router contract:0x…`, when that pays more.

```sh
chainv4 dex pools
chainv4 dex quote coin contract:0x… 1000000             # the route, what comes out, price impact
chainv4 dex add coin contract:0x… 1000000 4000000       # at most these, at the pool's ratio; opens it if new
chainv4 dex add coin contract:0x… 1000 4000 --fee 100   # the pair's 1% pool
chainv4 dex swap coin contract:0x… 10000 --slippage 1   # at least the quote less 1%
chainv4 dex swap coin evm:0x… 10000 --split --split-router contract:0x…
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
    minimum received at your slippage and the route, through one pool or two, at whichever tiers
    pay most, on the DEX switched to;
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
  is its opening price. It opens at 0.3% where the chain offers it, else at its smallest tier.

Picking a tier when opening a pool, a split route through router.lua, and the time-weighted
price come to the wallet later (see the [todo](todo.md)).

Every one of these is a call the Send page reviews and signs, on the offline signer too.

## In the explorer

The **DEX** page lists both DEXes' pools, each named by its pair and tier, with what it holds,
its price, and its volume and fees, at its tier, over about a day and a week; a pool opens its price chart and its latest trades, adds and
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

- **Tiers, through whole blocks, on both DEXes:** one pool per pair at each tier, side by side,
  each trading at its own; a tier the chain does not offer refused for a new pool and kept for one
  already open; `chain.dex()` and the precompile's answers.
- **The burned share:** on the native DEX the coin's supply falls by it and a token's goes to the
  zero address, on the EVM DEX an ERC-20's goes to `0x…dEaD`, and the pool keeps the rest.
- **Prices over time:** the native sums against what the reserves held for how long.
- **Flash swaps on both DEXes:** repaid with the fee, natively in a token and in the coin through
  `repay`, what was sent past it returned; a unit short refused and undone; `repay` refused to
  anyone but the borrower.
- **router.lua:** a trade split across both DEXes in one transaction, its least out on the sum.

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
- **The Trade page's pieces:** prices from `Sync` events, the router's calldata with its tiers,
  a native trade's tier after its assets, the EVM pairs read with WDEV as the coin and listed only
  once their tier is read.
- **The explorer's DEX page:** a pair's pools at two tiers told apart by their events' tier, fees
  summed at each pool's own, events from before tiers matching either.
- **The governance form:** tiers as a list and the burned share, each a path a lock may name.
