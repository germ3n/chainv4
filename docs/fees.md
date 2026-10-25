# Fees

On a chain with fees, every transaction pays for the gas it uses, failed ones too, so spam is
never free. Genesis's `fees` sets every part of it (`src/chain/fees/`), and `fees: null` keeps a
chain free, with blocks unbounded.

| Field | What it does |
| --- | --- |
| `block_gas_limit` | The most gas one block's transactions may use together. |
| `burn_permille` | How much of the base part of each fee is burned, in thousandths: 0 sends it all to the producer, 1000 burns it all. |
| `schedule` | The scorer: how much gas a transaction costs. |
| `pricing` | What each gas costs. |

## What a transaction pays

A transaction carries a signed fee offer:

| Field | What it is |
| --- | --- |
| `gas_limit` | The most gas it may use. |
| `max_price` | The most it pays per gas. |
| `tip` | Under a base fee, what it adds per gas for the producer. |

1. **Before it runs:**
   - its gas limit must cover its intrinsic cost, what the schedule says it costs whatever it
     does;
   - its price must be enough for the block;
   - its sender must hold the gas limit at that price, which is reserved.

   A transaction that fails any of these cannot go in a block: a block holding one is invalid,
   and builders leave it out.
2. **While it runs:** the fuel its contracts may burn is what its gas limit buys over the
   intrinsic cost, and at most `tx_limits.fuel`.
3. **After it runs:**
   - its gas used is the intrinsic cost plus the gas for the fuel it burned, at most its limit;
     one that ran out of fuel used all the fuel it was allowed;
   - the scorer says how much of it to charge;
   - the sender pays that at its price, and gets the rest of the reserve back.

Of what it paid:
- the block's base price times the gas charged is the **base part**: `burn_permille` of it is
  burned, and the producer gets the rest;
- whatever it paid **above the base price** goes to the producer whole.

## Scorers

A scorer is a `GasSchedule`:
- `intrinsic`: what a transaction costs before it runs;
- `for_fuel` and `fuel_for`: how gas and fuel convert;
- `charge`: what the sender is charged for once it ran, the gas it used by default.

Two kinds ship; a new one is a new implementation.

**`weighted`** weighs a transaction by what it is, how big it is and the fuel it burns.

| Field | Gas |
| --- | --- |
| `per_tx` | for every transaction |
| `per_byte` | for each byte of the signed transaction |
| `per_code_byte` | on top, for each byte of code a deploy or upgrade stores, which every node keeps. Compiling it is fuel on top, 1 per byte, which costs gas like any fuel |
| `fuel_per_gas` | how much fuel one gas buys; at least 1 |

**`contract`** weighs a transaction as `weighted` does, with the same fields, then calls
`score` on the contract at `contract`, with `fuel` of its own, to say what the sender is
charged.

```lua
local M = {}

-- Transfers to the treasury are free, calls cost double, everything else as weighed.
function M.score(sender, kind, target, bytes, fuel, gas, failed)
    if kind == "transfer" and target == address.parse("ed25519:0x…") then
        return 0
    end
    if kind == "call" then
        return gas * 2
    end
    return gas
end

return M
```

- **The call:** `score(sender, kind, target, bytes, fuel, gas, failed)` returns the gas to
  charge, as a whole number.
  - `kind` is the transaction's type as the API names it: `transfer`, `call`, `deploy` and so on.
  - `target` is what it is sent to, or the sender.
  - `gas` is what the weights say.
- **Limits:** what it returns is capped at the transaction's gas limit. The chain pays its fuel,
  bounded by `fuel`.
- **Read-only:** nothing it writes or emits is kept.
- **Fallback:** until a contract lives at `contract`, or when it fails or returns something other
  than a whole number, the weights alone count.
- **What counts against a block:** blocks always count the gas used, by the weights, whatever the
  scorer charges. A scorer can change what senders pay, but not how much a block holds.

Deploy the scorer once the chain runs. `chainv4 genesis --scorer ADDRESS` names it ahead of time.
The address a deploy lands at is fixed by the deployer and its nonce, so it can be worked out
before the deploy.

## Pricings

A pricing is a `Pricing`:
- `genesis_price` and `next_price`: each block's base price;
- `price`: what an offer pays at a base price, or none when it is too low;
- `floor`: the least base price any block can have.

Two kinds ship.

**`base_fee`**, as in EIP-1559:
- **The base price** is set by how full the parent block was against a target of
  `block_gas_limit / elasticity`:
  - fuller raises it and emptier lowers it, by at most `1 / change_denominator` a block;
  - it always rises by at least 1 over the target, and never falls below `min`.
- **What a transaction pays** is the base price plus its tip, as far as its `max_price` allows.
- **Too low:** it is not taken while its `max_price` is under the base price.

| Field | Dev value |
| --- | --- |
| `initial` | 1 gwei (10⁹), the genesis block's base price |
| `min` | 0.01 gwei; at least 1 |
| `elasticity` | 2: aims for half-full blocks |
| `change_denominator` | 8: at most an eighth a block |

**`bid`:** each sender bids its price, `max_price`, and pays it whole; the tip means nothing.
Bids under `min_price` are not taken. `min_price` is every block's base price, so that much of
each fee is the base part, and the rest of a bid goes to the producer.

## Blocks

- **Headers:** each header carries `gas_used` and `base_price`, both checked on import. The base
  price must be what the parent's makes it, and the gas used what the transactions used.
- **The gas limit:** a transaction is only taken while its gas limit fits in what the block has
  left. A block that breaks this is invalid.
- **Building:** producers build blocks by running the transactions, so the gas is known
  (`Chain::build`).

## The mempool

The mempool admits a transaction only when:
- its gas limit covers its intrinsic cost and fits a block;
- its max price is at least the pricing's floor;
- its sender can cover the reserve now.

Producers take the best-paying transactions first. The priority is what a transaction pays above
the base price, per gas: its tip under a base fee, its bid above the minimum under bids. Each
sender's transactions still go in nonce order.

## Sending with fees

`chainv4 send` asks the node for an offer at `POST /offer`, as the wallet and the explorer do.
The flags set any part of it.

| Flag | Default |
| --- | --- |
| `--gas-limit N` | For a call, deploy or upgrade, what a dry run of it on the node used, plus a quarter of what it ran past its intrinsic cost, at most the gas for `tx_limits.fuel`. For anything else, its intrinsic cost. |
| `--max-price N` | Twice the base price plus the tip under a base fee; the base price plus the tip under bids. |
| `--tip N` | 0. |

The receipt it prints shows the gas used and the fee. `GET /status` shows the fee settings and
the base price a transaction sent now must offer; blocks show their gas and base price, and
receipts their gas and fee.

## What to keep in mind

- **Accounts need coins to send anything,** the development key included.
- **Gas limits come from a dry run.** The node runs a call, deploy or upgrade on a throwaway
  copy of its state, as the sender at their next nonce, and offers what it used plus a quarter of
  what it ran, so blocks fill with what calls really use rather than their caps. A call whose
  work grows past that by the time it lands runs out and fails, still charged; one the dry run
  says would fail gets the whole fuel cap, since things may change, and `send` refuses it unless
  given `--anyway`. The wallet and the explorer show why and ask first.
- **A transaction that can no longer pay lingers in the mempool** until its nonce is used, and
  every block built tries it again.
- **A chain without fees is bounded by bytes alone,** when its genesis sets `max_block_bytes`.
- **Fee kinds are not behind cargo features.** Every node has both schedules and both pricings.
