# Genesis

A chain is its genesis. Every parameter it runs under is set there, and the genesis block's
`tx_root` is the hash of all of it, so nodes with different genesis are on different chains and
never connect.

The genesis block also commits to:
- **Light clients' parameters:** its `proof_root` is the hash of `light_params()`, the chain's
  hash, consensus and how its finality works. A [light client](light-client.md) holding only the
  genesis id can then check them.
- **The starting state:** its `state_root` is the root of the state genesis writes.

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
| `consensus` | Who may produce, by `type`: `authority`, `pow`, `rotating` or `staked`. |
| `reward` | `initial`, what a block mints for its producer, and `halving_interval`, blocks between halvings or `null` for never. |
| `minter` | The one address allowed to send `Mint`: the development key's, or `null`. `Mint` is a faucet for development chains; a chain of value names nobody, and new coins come from block rewards alone. |
| `allocations` | Who holds what from the start: a list of `address` and `amount`. |
| `dex` | Both [DEXes](dex.md#fee-tiers-and-the-burned-share)' settings: `{"tiers": [5, 30, 100], "protocol_permille": 0}`, the fee tiers a new pool may open at, in hundredths of a percent, and the thousandths of each fee burned. Left out, those. A file from before tiers, `{"fee_bps": 30}`, is read as that one tier. |
| `engines` | Which engines contracts may be deployed for: `lua`, `wasm`, `evm`. A node built without one refuses the chain. |
| `tx_limits` | `fuel` and `memory` one deploy, upgrade or call may use. |
| `token_functions` | The functions that make a contract a token: one that exports them all is in the [token registry](token-standard.md#the-token-registry). The dev chains name ERC-20's six: `allowance`, `approve`, `balance_of`, `total_supply`, `transfer` and `transfer_from`. `[]` keeps no registry. Each is named once. |
| `federation` | The bridges' federation from the start: `{"signers": ["0x02…", …], "threshold": 2, "chain_depth": 20, "evm_depth": 12, "bitcoin_depth": 6}`, compressed secp256k1 keys (see [Bridges](bridges.md#the-federation)). `null`, or left out, for none until governance decides one; left out, a chain from before it keeps its id. 1 to 20 signers, each once, a threshold of 1 to as many, and depths of at least 1. |
| `directory` | The contracts the chain names for the apps and tools: `{"gateway": …, "btc_gateway": …, "evm_gateways": [{"chain_id": 1, "gateway": "0x…"}], "swap": …, "dex": …, "dex_router": "0x…", "dex_split": …, "nft_factory": …}`, each `null` while there is none (see [Directory](governance.md#directory)). `null`, or left out, for none until governance decides one; left out, a chain from before it keeps its id. Written into the state from the first block, so light clients prove it. |
| `directory_admin` | Who sets the directory on a chain without governance: an account, a multisig's too, that may send `set-directory` and hand the role on or renounce it (see [Without governance](governance.md#without-governance-the-directorys-admin)). `null`, or left out, for none; refused beside `governance`, which sets the directory itself. Left out, a chain from before it keeps its id. |
| `name` | The chain's name, such as `"chainv4 testnet"`, which the apps show wherever they name the network: 1 to 40 characters, no control characters, no space at either end. `null`, or left out, for none, which the apps show as chainv4; left out, a chain from before it keeps its id. Written into the state from the first block, and renamed by governance with `Change::Name` (see [Name](governance.md#name)). |
| `collections` | What makes a contract a collection: `{"nft": [...], "multi": [...]}`, the functions an NFT collection exports and those a multi-token one does (see the [collection registry](nft-standard.md#the-collection-registry)). The dev chains name the standards' own. `null`, or left out, keeps no registry until governance decides one (`Change::Collections`); left out, a chain from before it keeps its id. Each list names each function once; an empty one keeps no collections of its kind. |
| `max_reorg_depth` | How far back the main chain may be rolled. |
| `finality` | `null`, or the kind and its settings, by `type`: `ffg`, `authority` or `depth`. |
| `fees` | `null` for free transactions, or the fee settings. |
| `max_block_bytes` | The most bytes a block's transactions may take, encoded, from 1 MiB (1048576), so a block holds the largest deploy, to 6 MiB (6291456), so a full block fits one gossip message; a block past it is refused. The dev chains have 4 MiB. Left out or `null`, blocks are bounded by gas alone, or by nothing without fees. A file written before it, without it, keeps its chain's genesis id. |
| `vote_reward_source` | Where finality's vote rewards come from: `mint`, new supply, or `fees`, the burned share of every fee, pooled and paid out. Needs `fees`. Left out or `null`, `mint`, and a file from before it keeps its id. `chainv4 genesis --vote-rewards fees` sets it. |
| `bootnodes` | Where nodes of the chain find their first peers: up to 32 libp2p addresses, such as `/ip4/203.0.113.7/udp/4001/quic-v1`, each named once. Every node dials them while it has fewer peers than `--target-peers`. Fixed with the chain's id; under governance, `approve-change --bootnodes` replaces them on the live chain. Left out or `[]`, none, and a file from before it keeps its id. See [Node](node.md#finding-peers). |
| `names` | `null` for a chain without names, or the [name registry](names.md#turning-it-on): `suffix`, `prices` by length, `grace_ms` and `max_years`. The dev chains have names under `dev`. Left out, `null`, and a file from before it keeps its id. `chainv4 genesis --names SUFFIX` sets the suffix, `--names none` leaves them out. |
| `governance` | `null`, or how the chain changes what genesis set by transaction: the kind and its settings, by `type`: `electorate` or `council`. Needs finality that votes. Left out or `null`, nothing changes, and a file from before it keeps its id. `chainv4 genesis --governance electorate` sets it. See [Governance](governance.md). |
| `locked` | Settings no vote may ever change, by path: a change's name, such as `"reward"`, alone or followed by its fields' names as `GET /governance` writes the change, such as `"anchoring.enforce"`, `"federation.threshold"` or `"limits.tx_limits.fuel"`; each named once. Governance may add more (`Change::Locks`) and nothing takes one away; `"locks"` itself freezes the set. Needs `governance`. Left out or `[]`, nothing is locked, and a file from before it keeps its id; written into the state from the first block while it names any. See [Locks](governance.md#locks). |

| `preinstalls` | Contracts the chain starts with, by name: `deployment_proxy`, the deterministic deployment proxy at `0x4e59…956c`, as other EVM chains have it, so what is deployed through it lands at the same address here, MultiSig.sol and Swap.sol among them (see [EVM](evm.md#deploying)). Needs `evm` in `engines`; each named once. `chainv4 genesis` names it for a chain with the EVM. Left out or `[]`, none, and a file from before it keeps its id, one written before the proxy too. |
| `forks` | The forks the chain runs from the start: `{"lead": null, "scheduled": [{"name": "…", "height": 0}]}`, each a fork its release knows, by `height` or by `epoch`, whose first block it starts at on a chain with finality; `lead`, how many blocks ahead governance must schedule another, `null` for a day of the chain's blocks. See [Forks](governance.md#forks). Left out or `null`, none, and a file from before it keeps its id. |

Governance may replace `fees`, `reward`, `tx_limits` with `max_block_bytes`, and finality's
tunables on a running chain (see [Governance](governance.md#fees)); the file, and the chain's id,
keep what genesis set.

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
{ "type": "pow", "hash": "blake3", "target_block_ms": 2000, "initial_difficulty": "100000", "min_difficulty": "1000", "window": 11 }
{ "type": "rotating", "slot_ms": 2000 }
{ "type": "staked", "slot_ms": 2000, "schedule": "draw" }
```

- `authority`: `key` is the only key that may produce, a block every `slot_ms` at most.
- `rotating`: the signers of authority finality produce in turn, one every `slot_ms`, the next in
  line stepping in a slot later when one misses. Needs `finality.type: authority`. See
  [Consensus](consensus.md#rotating-authority).
- `staked`: the bonded validators of FFG finality produce, one every `slot_ms`, each with a share
  in proportion to its bond, picked by `schedule`: `draw`, a weighted draw by the chain's hash, or
  `priority`, Tendermint's rotation. Needs `finality.type: ffg`. See
  [Consensus](consensus.md#production-by-stake).
- `pow`: `hash` is the work hash, chosen apart from the chain hash and also never `xxh3`.
  Difficulty starts at `initial_difficulty`, follows `target_block_ms`, and never falls below
  `min_difficulty`. `window`, at least 2, makes difficulty retarget over that many blocks and
  bounds each block's timestamp by their median; left out, difficulty follows the parent alone,
  as it did before the field existed, and the chain keeps the id it had. See
  [Consensus](consensus.md).

### governance

```json
{ "type": "electorate", "approval_epochs": 64 }
{ "type": "council", "members": ["ed25519:0x…", "ed25519:0x…"], "threshold": 2, "approval_epochs": 64 }
```

- `electorate`: finality's own validators or signers approve a change, by stake or by signer,
  two thirds of the stake or `change_threshold` of the signers deciding.
- `council`: the `members` approve, each weighing one, `threshold` of them deciding. Each is
  named once, and the threshold is at least 1 and at most the number of members.
- `approval_epochs`, at least 1, is how many epochs an approval counts for. See
  [Governance](governance.md).

### finality

```json
{ "type": "ffg", "checkpoint_interval": 8, "vote_depth": 2, "min_bond": "100000000000000000000",
  "activation_delay": 2, "withdrawal_delay": 64, "reporter_divisor": 16, "max_validators": 1000,
  "vote_reward": "1000000000000000000", "max_proofs_per_block": 4,
  "validators": [{ "account": "ed25519:0x…", "voter": "ed25519:0x…", "bond": "10000000000000000000000", "attester": "0x…" }],
  "leak": { "after_epochs": 4, "permille": 50 } }
{ "type": "authority", "checkpoint_interval": 8, "vote_depth": 2, "activation_delay": 2, "max_signers": 100,
  "signers": ["ed25519:0x…", { "key": "ed25519:0x…", "account": "ed25519:0x…" }],
  "threshold": 2, "change_threshold": 2, "approval_epochs": 64,
  "vote_reward": "0", "max_proofs_per_block": 4 }
{ "type": "depth", "depth": 6 }
```

What each field means is in [Finality](finality.md). An authority signer is a bare key, which is
its own account, or a `key` with the `account` that approves changes for it and takes its rewards.
FFG's `leak` is a later addition: left out, there is none, and a file from before it keeps its id.
A validator's `attester` is the Ethereum address it [attests for Ethereum](finality.md#attestations-finality-followed-on-ethereum)
with, or left out for none; a genesis with validators changed its id when the field came in.

### fees

```json
{ "block_gas_limit": 50000000, "burn_permille": 500,
  "schedule": { "type": "weighted", "per_tx": 1000, "per_byte": 10, "per_code_byte": 20, "fuel_per_gas": 1 },
  "pricing": { "type": "base_fee", "initial": "1000000000", "min": "10000000", "elasticity": 2, "change_denominator": 8 } }
```

A `contract` schedule adds `contract` and `fuel` to the weighted fields, and `kinds`, the kinds
of transaction the scorer is called for, such as `["call", "deploy"]`; left out, every kind. A
`bid` pricing has only `min_price`. What each field means is in [Fees](fees.md).

## What a node refuses

`Chain::open` checks a genesis before writing anything, and refuses one no chain could run with:

- **Hashes:** a chain or work hash that is not secure: `xxh3`.
- **The coin:** a symbol that is empty, longer than 12, or not ASCII letters and digits, or more
  than 77 decimals.
- **FFG:** a `checkpoint_interval`, `activation_delay` or `reporter_divisor` of 0, more genesis
  validators than `max_validators`, or a `leak` with `after_epochs` 0 or `permille` over 1000.
- **Authority finality:**
  - a `checkpoint_interval` or `activation_delay` of 0;
  - no signers, a signer named twice, two signers sharing an account, or more signers than
    `max_signers`;
  - an `approval_epochs` of 0;
  - a `threshold` or `change_threshold` that is not more than half the signers and at most all of
    them.
- **A `minter`** that is not the development key.
- **`vote_reward_source: fees`** without fees.
- **`consensus.type: rotating`** without `finality.type: authority`, or **`staked`** without
  `finality.type: ffg`.
- **`bootnodes`** with more than 32, one twice, or one that is not a libp2p path.
- **`names`** with a suffix that is not one label, no prices or more than 61, or `max_years` 0.
- **`governance`** without finality that votes, with an `approval_epochs` of 0, or a council with
  no members, a member named twice, or a `threshold` of 0 or more than the members.
- **`locked`** without `governance`, with a path that names nothing (`reward.initia`, `colour`,
  `switch_finality.epoch`), or with a path twice.
- **Fees:**
  - a `block_gas_limit` of 0, a `burn_permille` over 1000, or a `fuel_per_gas` of 0;
  - a scorer's `kinds` naming something that is no kind of transaction, or one twice;
  - a base fee `min` of 0, an `initial` under `min`, or an `elasticity` or `change_denominator`
    of 0;
  - a scorer that is not a contract address, or that has no fuel.
- **The database:**
  - a database written for a different genesis (`WrongGenesis`);
  - a database kept in another storage format (`OldFormat`), such as one from before formats
    were recorded.

  Point `--db` elsewhere or delete it.

## The dev chains

Without `--genesis`, a node runs one of four development chains, picked with `--consensus`.

They share these settings:
- the development key holds a million coins and may mint;
- they hash with BLAKE3 when the build has it;
- they allow every engine the build has;
- they run FFG finality with the development key as the one validator;
- they charge the development fees.

They differ only in consensus:
- **pow:** mined, 50 coins a block, halving every 210,000 blocks.
- **authority:** the development key produces a block every two seconds, for no reward.
- **rotating:** authority finality with the development key as its one signer, which produces
  every block in turn, every two seconds, for no reward. In a build with authority finality.
- **staked:** FFG finality with the development key as its one validator, bonded from genesis,
  which produces every block by stake, drawn, every two seconds, for no reward. In a build with
  FFG.

The dev genesis depends on the build: the engines it allows, the default hash, whether FFG is
built in. Nodes built with different features are on different dev chains. The development key's
seed is in the source, so never put anything of value on a dev chain.
