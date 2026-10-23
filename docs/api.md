# HTTP API

Start a node with `--http ADDR`, such as `127.0.0.1:8080`, to serve the API (`src/api/`).

**What it does:**
- it runs on a thread of its own and answers from the committed chain;
- it never writes, except to hand submitted transactions to the node.

**Formats:**
- every answer is JSON;
- errors are `{"error": "…"}`;
- amounts, and integers that can pass 2^53, are decimal strings;
- hashes and bytes are `0x` hex;
- addresses are their kind and hex, such as `ed25519:0x…` or `contract:0x…`.

| Status | When |
| --- | --- |
| 400 | A malformed request, or a transaction the mempool refuses for what it is: invalid, or an offer that cannot pay. |
| 404 | Nothing there, such as a transaction not on the main chain yet. |
| 409 | A nonce already used, or the same sender and nonce already waiting. |
| 500 | The database failed. |
| 503 | No node behind the API, a full mempool, or a node that stopped. |

## Routes

### `GET /status`

Where the chain stands:

| Field | What it is |
| --- | --- |
| `genesis`, `tip`, `height`, `timestamp` | The chain and its tip. |
| `difficulty`, `work` | The tip's difficulty and the main chain's total work. |
| `hash` | The chain hash, such as `blake3`. Clients need it to predict contract addresses. |
| `finality` | `null`, or `type`, `epoch`, `checkpoint_interval`, and the `justified` and `finalized` checkpoints, each with `epoch`, `block` and `height`. |
| `fees` | `null`, or the fee settings as genesis files write them, with `base_price`, what a transaction sent now must offer at least, and `tx_fuel`, the chain's fuel cap per transaction. |

### `GET /blocks?before=H&limit=N`

Block summaries, newest first, below height `before`, `limit` of them: 20 by default, at most
100. `next` is the cursor for the next page, or `null` at genesis.

Each summary has:
- `id`, `height`, `parent`, `timestamp`, `producer`;
- `difficulty`, `tx_root`, `gas_used`, `base_price`;
- `tx_count` and `proof_count`.

### `GET /blocks/{block}`

**Which block:** `block` is `latest`, a height, or a `0x` block id.

**What it returns:**
- the summary;
- its `seal`;
- its `proofs`, each with its link and voters;
- every transaction.

**Each transaction** has:
- its `id`, `sender`, `nonce` and `signer`;
- its `fee` offer: `gas_limit`, `max_price`, `tip`;
- its `signature` and its `tx`;
- its `receipt` when the block is on the main chain.

**Each `tx`** has a `type` and that type's fields:
- `transfer`, `mint`, `burn`, `call`, `deploy`, `upgrade`;
- `bond`, `unbond`, `withdraw`, `slash`, `approve_signers`.

### `GET /txs/{id}`

**Before it lands:** 404 until the transaction is on the main chain.

**Once it lands:**
- where it is: `block`, `height`, `index`, `confirmations`, and whether it is `final`;
- what it cost: `fuel_used`, `gas_used`, `fee`;
- its `events`;
- its `status`: `done`, `deployed` with `contract`, `upgraded` with `contract`, `returned` with
  `values`, or `failed` with `error`.

### `GET /accounts/{address}`

`address`, `nonce` and `balance`. An address never written to reads as nonce 0 and balance 0.

### `GET /contracts/{address}`

What a contract is:
- `address`, `deployer`, `engine` and `code_size`;
- its exported `functions`, and which are `reentrant`;
- whether it is `upgradable`, that is, whether it exports `check_perms`.

### `POST /txs`

**The body:** `{"tx": "0x…"}`, a `SignedTx` encoded as the chain encodes it. A body with bytes
past the transaction is refused.

**What it does:** the node's mempool admits it and gossips it. The API answers 202 with its
`id`.

`chainv4 send` is a client of this route.

### `GET /validators`

Under `ffg` or `authority`:
- `epoch`;
- every bonded validator under `validators`, each with:
  - `voter`, `account` and `bond`;
  - `active_from`, `active_until` and `withdrawable_from`;
  - `slashed`;
- who votes in the current epoch under `active`, each with `voter`, `account` and `weight`;
- `total_weight`.

Under `authority`, `validators` is empty and `active` lists the signers, each weighing 1.

404 without finality or under `depth`.

### `GET /validators/{voter}`

One validator by voting key, as in `/validators`.

### `GET /signers`

Under `authority`:
- `epoch`;
- the `active` set;
- the `decided` set and its `version`;
- the `proposals` so far, each with its `signers`, `threshold`, `change_threshold` and
  `approvals`.

Sets show their `signers`, `threshold` and `change_threshold`.

404 under any other kind.

### `GET /evidence`

Conflicting votes this node has seen, newest last. Each has its `voter` and both votes. Each vote
is shown readable, and `encoded` as `chainv4 send slash` takes it.

## What to keep in mind

- **Plain HTTP only.** The CLI is built without TLS, so keep the API on a trusted network.
- **One request at a time,** with no contract views and no read-only calls.
- **Reads are not snapshot-consistent.** A request that reads several keys can see two tips.
