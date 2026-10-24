# Genesis

A chain is its genesis. Every parameter it runs under is set there, and the genesis block's
`tx_root` is the hash of all of it, so nodes with different genesis are on different chains and
never connect.

`chainv4 genesis` writes the development settings as JSON to edit, and `chainv4 --genesis FILE`
runs a node on a file. A file must name every field, and unknown fields are refused, so a
misspelled parameter is an error rather than a silent default. Every node on a chain needs the
same file. Amounts, and integers that can pass 2^53, are decimal strings.

```sh
chainv4 genesis --consensus authority --finality authority --fees bid --out mychain.json
chainv4 genesis --scorer contract:0x41f0… --out scored.json   # a contract scores transactions
chainv4 genesis --finality none --fees free                     # neither finality nor fees
```

## Fields

| Field | What it sets |
| --- | --- |
| `timestamp` | The genesis block's timestamp, in milliseconds. |
| `hash` | The chain hash: block and transaction ids, roots, contract addresses, gossip ids. `blake3`, `sha256`, `keccak256`, `blake2s` or `blake2b`, whichever the build has; never `xxh3`. |
| `coin` | What the chain's coin is called and how it is shown: `symbol` and `decimals`. |
| `consensus` | Who may produce, by `type`: `authority` or `pow`. |
| `reward` | `initial`, what a block mints for its producer, and `halving_interval`, blocks between halvings or `null` for never. |
| `minter` | The one address allowed to send `Mint`, or `null`. |
| `allocations` | Who holds what from the start: a list of `address` and `amount`. |
| `engines` | Which engines contracts may be deployed for: `lua`, `wasm`. A node built without one refuses the chain. |
| `tx_limits` | `fuel` and `memory` one deploy, upgrade or call may use. |
| `max_reorg_depth` | How far back the main chain may be rolled. |
| `finality` | `null`, or the kind and its settings, by `type`: `ffg`, `authority` or `depth`. |
| `fees` | `null` for free transactions, or the fee settings. |

### coin

`{"symbol": "DEV", "decimals": 18}` on the dev chains.

| Field | What it sets |
| --- | --- |
| `symbol` | The coin's ticker: 1 to 12 ASCII letters and digits. |
| `decimals` | How many decimals a display divides by: 18 means 10¹⁸ units make one coin. At most 77, the digits a balance can have. |

**Only displays read it.** Balances, fees and rewards are always counted in whole units, and the
chain never divides by the decimals. They are in genesis so every client shows amounts alike:
`/status` passes them on, and the [explorer](explorer.md) shows amounts by them. Like every
field, they are part of the genesis id.

### consensus

```json
{ "type": "authority", "key": "ed25519:0x…", "slot_ms": 2000 }
{ "type": "pow", "hash": "blake3", "target_block_ms": 2000, "initial_difficulty": "100000", "min_difficulty": "1000" }
```

- `authority`: `key` is the only key that may produce, a block every `slot_ms` at most.
- `pow`: `hash` is the work hash, chosen apart from the chain hash and also never `xxh3`.
  Difficulty starts at `initial_difficulty`, follows `target_block_ms`, and never falls below
  `min_difficulty`. See [Consensus](consensus.md).

### finality

```json
{ "type": "ffg", "checkpoint_interval": 8, "vote_depth": 2, "min_bond": "100000000000000000000",
  "activation_delay": 2, "withdrawal_delay": 64, "reporter_divisor": 16, "max_validators": 1000,
  "vote_reward": "1000000000000000000", "max_proofs_per_block": 4,
  "validators": [{ "account": "ed25519:0x…", "voter": "ed25519:0x…", "bond": "10000000000000000000000" }] }
{ "type": "authority", "checkpoint_interval": 8, "vote_depth": 2, "activation_delay": 2, "max_signers": 100,
  "signers": ["ed25519:0x…"], "threshold": 1, "change_threshold": 1, "vote_reward": "0", "max_proofs_per_block": 4 }
{ "type": "depth", "depth": 6 }
```

What each field means is in [Finality](finality.md).

### fees

```json
{ "block_gas_limit": 50000000, "burn_permille": 500,
  "schedule": { "type": "weighted", "per_tx": 1000, "per_byte": 10, "per_code_byte": 20, "fuel_per_gas": 1 },
  "pricing": { "type": "base_fee", "initial": "1000000000", "min": "10000000", "elasticity": 2, "change_denominator": 8 } }
```

A `contract` schedule adds `contract` and `fuel` to the weighted fields, and a `bid` pricing
has only `min_price`. What each field means is in [Fees](fees.md).

## What a node refuses

`Chain::open` checks a genesis before writing anything, and refuses one no chain could run with:

- **Hashes:** a chain or work hash that is not secure: `xxh3`.
- **The coin:** a symbol that is empty, longer than 12, or not ASCII letters and digits, or more
  than 77 decimals.
- **FFG:** a `checkpoint_interval`, `activation_delay` or `reporter_divisor` of 0, or more genesis
  validators than `max_validators`.
- **Authority finality:**
  - a `checkpoint_interval` or `activation_delay` of 0;
  - no signers, a signer named twice, or more signers than `max_signers`;
  - a `threshold` or `change_threshold` that is not more than half the signers and at most all of
    them.
- **Fees:**
  - a `block_gas_limit` of 0, a `burn_permille` over 1000, or a `fuel_per_gas` of 0;
  - a base fee `min` of 0, an `initial` under `min`, or an `elasticity` or `change_denominator`
    of 0;
  - a scorer that is not a contract address, or that has no fuel.
- **The database:**
  - a database written for a different genesis (`WrongGenesis`);
  - a database kept in another storage format (`OldFormat`), such as one from before formats
    were recorded.

  Point `--db` elsewhere or delete it.

## The dev chains

Without `--genesis`, a node runs one of two development chains, picked with `--consensus`.

They share these settings:
- the development key holds a million coins and may mint;
- they hash with BLAKE3 when the build has it;
- they allow every engine the build has;
- they run FFG finality with the development key as the one validator;
- they charge the development fees.

They differ only in consensus:
- **pow:** mined, 50 coins a block, halving every 210,000 blocks.
- **authority:** the development key produces a block every two seconds, for no reward.

The dev genesis depends on the build: the engines it allows, the default hash, whether FFG is
built in. Nodes built with different features are on different dev chains. The development key's
seed is in the source, so never put anything of value on a dev chain.
