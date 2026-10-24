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
- addresses are their kind and hex, such as `ed25519:0x…` or `contract:0x…`;
- call arguments and return values are `{"type": …, "value": …}`, the type being `address`,
  `amount`, `bytes`, `bool`, `string`, `number`, `i64`, `u64`, `i128`, `u128`, `u256` or `nil`,
  whose value is `null`.

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
| `coin` | The chain's coin as genesis names it: its `symbol` and `decimals`, for showing amounts. |
| `token_functions` | The functions that make a contract a token on this chain, as genesis names them; `[]` when it keeps no token registry. |
| `finality` | `null`, or `type`, `epoch`, `checkpoint_interval`, and the `justified` and `finalized` checkpoints, each with `epoch`, `block` and `height`. |
| `fees` | `null`, or the fee settings as genesis files write them, with `base_price`, what a transaction sent now must offer at least, and `tx_fuel`, the chain's fuel cap per transaction. |

### `GET /blocks?before=H&limit=N`

Block summaries, newest first, below height `before`, `limit` of them: 20 by default, at most
100. `next` is the cursor for the next page, or `null` at genesis.

Each summary has:
- `id`, `height`, `parent`, `timestamp`, `producer`;
- `difficulty`, `tx_root`, `receipts_root`, `state_root`, `gas_used`, `base_price`;
- `tx_count` and `proof_count`.

### `GET /blocks/{block}`

**Which block:** `block` is `latest`, a height, or a `0x` block id.

**What it returns:**
- the summary;
- whether it is on the main chain (`main`);
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
- `bond`, `unbond`, `withdraw`, `slash`, `approve_signers`, `withdraw_approval`.

### `GET /txs/{id}`

**Before it lands:** 404 until the transaction is on the main chain.

**Once it lands:**
- where it is: `block`, `height`, `index`, `confirmations`, and whether it is `final`;
- what it cost: `fuel_used`, `gas_used`, `fee`;
- its `events`;
- its `status`: `done`, `deployed` with `contract`, `upgraded` with `contract`, `returned` with
  `values`, or `failed` with `error`;
- its `trace`: where its contracts went (see below).

**The trace** is `hops`, depth first, and `dropped`, how many hops past the 256 a receipt keeps
were only counted. Each hop has:
- `depth`: 0 for what the transaction ran itself;
- `kind`: `call`, `delegate` or `transfer`;
- `from`, `to`, `function` and `value`;
- `fuel_used`, including the hops under it;
- `error`: why it failed, or `null`;
- `events`: where the events it emitted itself sit in the receipt's `events`, empty once it or
  anything above it failed.

A transaction that runs no contract has no hops. Receipts in `/blocks/{block}` carry the trace
too.

### `GET /accounts/{address}`

`address`, `nonce` and `balance`. An address never written to reads as nonce 0 and balance 0.

### `GET /contracts/{address}`

What a contract is:
- `address`, `deployer`, `engine` and `code_size`;
- its exported `functions`, and which are `reentrant`;
- whether it is `upgradable`, that is, whether it exports `check_perms`;
- its storage `layout`, empty for code that declares none. Each namespace its code declares has
  its `name` and `shape`:
  - `map`, with the `kind` of its values;
  - `record`, with its `keys`, each with its `key`, its `kind`, and whether it is `retired`,
    when `kind` is `null`;
  - `retired`;
- what the token registry holds for it in `token`: its `address`, `name`, `symbol` and
  `decimals`, each `null` when it exports none; `token` is `null` for a contract that is not a
  token.

### `GET /proofs/{column}/{key}`

A proof of one chain-state entry against the tip's state root (see
[Chain](chain.md#the-state-root)). `column` is one of `accounts`, `contracts`, `storage`,
`validators`, `epochs`, `tokens` and `namespaces`; `key` is the entry's key in that column, in hex,
as [Storage](storage.md#keys-and-encoding) describes. An account's key, for example, is its
address's tag and 32 bytes.

It answers:
- `block`, `height` and `state_root`: the tip it proves against;
- `column`, `key`, and the entry's `value` in hex, or `null` when it holds nothing;
- `siblings`: the hash beside the path at each branch, the root's first;
- `leaf`: where the path ends, its `path` and `value_hash`, or `null` where it ends empty.

`StateProof::verify` checks it. A light client that trusts the header needs nothing else.

**Consistency:** the node checks each proof against the root before it answers. A block committed
while the proof was read would mix two trees, so the proof is read again, three times at most,
and then it answers 503.

### `GET /proofs/receipts/{id}`

A proof that a transaction's receipt is in its block's receipts root: that the transaction is in
the chain, and what it did. It answers:
- `tx`, `block`, `height` and `receipts_root`;
- `index` and `count`: its place among the block's transactions, and how many there are;
- `receipt`: its `CommittedReceipt`, encoded, in hex;
- `path`: the Merkle path from its leaf, the deepest sibling first.

`crypto::merkle::verify(root, leaf(hash, receipt), index, count, path)` checks it. It answers 404 for a
transaction not on the main chain.

### `POST /txs`

**The body:** `{"tx": "0x…"}`, a `SignedTx` encoded as the chain encodes it. A body with bytes
past the transaction is refused.

**What it does:** the node's mempool admits it and gossips it. The API answers 202 with its
`id`, or 503 when the node's [backlog](node.md#the-gossip-backlog) has no room: try again
shortly.

`chainv4 send` is a client of this route.

### `POST /offer`

**The body:** `{"tx": "0x…"}`, the transaction signed with any offer, such as a zero one. Every
field of an offer has a fixed size, so it weighs what the real one will. `gas_limit`, and
`max_price` and `tip` as decimal strings, keep what the sender chose.

**What it does:** works out the offer `chainv4 send` would make now, for the rest:
- a gas limit of its intrinsic cost, plus the fuel cap's worth for a call, deploy or upgrade;
- a max price of twice the next base price plus the tip under a base fee, or the base price
  plus the tip when senders bid;
- a tip of 0.

It answers `gas_limit`, `max_price` and `tip`: all zero on a chain without fees. Clients that
cannot run the chain's fee code, such as the [explorer](explorer.md) in a browser, use it.

### `POST /call`

**The body:** `contract`, `function` by name, `args` as a list of `{"type": …, "value": …}` the
way `values` are written, and optionally the `sender` it runs as (the zero address otherwise).

**What it does:** runs the function as a transaction would call it, on a throwaway copy of the
committed state, under the chain's fuel cap. Whatever it writes, sends or emits is dropped.

**What it answers:**
- its `status`, `returned` with `values` or `failed` with `error`;
- `fuel_used`;
- the `events` it would have emitted;
- its `trace`.

404 for an address that holds no contract. For reading contracts, such as a token's
`decimals()` or someone's `balance_of`.

### `GET /forks?limit=N`

The side branches the node stores, every block off the main chain, newest first, `limit` of them
(256 by default, at most 1,000). Each is walked from a leaf back to where it meets the main chain.

**What it answers:**
- `blocks`: summaries, whose `parent` links each branch to the main chain;
- `tips`: every leaf, the tip among them.

### `GET /events`

Events from the node's event index, newest first. Only a node started with `--index-events`
keeps one; on any other, 404.

**Which events,** by query:
- `contract=…`: every event a contract emitted;
- `contract=…&name=…`: one kind of them, such as `Transfer`;
- `address=…`: every event, from any contract, with this address in its data, such as token
  transfers to or from an account.

**Pages:** `limit` of them (50 by default, at most 500). `next` is the `cursor` for the next,
older page, or `null` after the last.

**Each event** has its `contract`, `name` and `data`, the `tx` that emitted it, its `block` and
`height`, the transaction's place in the block (`index`) and the event's place among the
transaction's events (`position`).

**Coverage:**
- `indexed_from` is the lowest height the index holds: every main-chain block's events from there
  to the tip are in it.
- `catching_up_to` is the height the node is still moving that to, or `null` once it is there.
  It is below `indexed_from` while the node indexes older blocks, and briefly above it on a
  `latest-N` window, as the oldest block leaves.

### `GET /tokens`

The [token registry](token-standard.md#the-token-registry): every contract that exports what
makes a token on this chain.

**Order:** by symbol, alphabetically, ignoring the case of ASCII letters, then by address. Tokens
without a symbol come first.

**Searching:** `symbol=…` lists those whose symbol starts with it, ignoring ASCII case. An exact
match comes first.

**Pages:** `limit` of them (50 by default, at most 500). `next` is the `cursor` for the next page,
or `null` after the last.

**Each token** has its `address`, `name`, `symbol` and `decimals`, as it answered at its deploy
or last upgrade, each `null` when it exports none.

### `GET /node`

How the node behind the API is doing. 503 when no node runs behind it.

| Field | What it is |
| --- | --- |
| `started`, `now`, `uptime_ms` | When its loop first ran, the time of the report, and the time between. |
| `listening` | Where peers reach it, each ending in `/p2p/` and its peer id. |
| `producing`, `validating` | Whether it produces blocks and votes on finality. |
| `mempool` | `count` waiting and the `capacity`. |
| `syncing` | `null`, or the `peer` it fetches blocks from and the height `from`. |
| `counters` | What it heard and did since it started: blocks heard, imported, stored on side branches, rejected and produced; reorgs; transactions heard, submitted and refused; votes heard and cast; sync requests served, sent and failed; connections; gossip and submissions the backlog dropped (`gossip_dropped`), and peers disconnected for flooding (`flooders_disconnected`). |
| `backlog` | What waits for the node loop now (`waiting`), and the backlog's limits, `total` and `per_peer` (see [Node](node.md#the-gossip-backlog)). |
| `peers` | Each connected peer's `id`, `address` and `since`, what it gossiped (`heard`: `blocks`, `txs`, `votes`), how much of it the backlog `dropped`, and its `chain` from its last status (`height`, `work`, `justified`), or `null`. |

### `GET /mempool?limit=N`

What waits for a block, by sender and nonce: `count`, `capacity`, and the first `limit`
transactions (100 by default, at most 1,000), shown as blocks show them. 503 without a node.

### `GET /stream`

Server-sent events, for a node started with `--stream`; 404 otherwise. Each event has a name
and JSON data:

| Event | Data |
| --- | --- |
| `block` | A stored block's summary, with `main` true when it extended the main chain. |
| `reorganized` | The ids taken off (`undone`, newest first), and the summaries put on (`applied`, oldest first). |
| `justified`, `finalized` | The checkpoint: `epoch` and `block`. |
| `queued` | A transaction that joined the mempool. |
| `peer_up`, `peer_down` | The peer's `id`, and for `peer_up` its `address`. |
| `lagged` | The client fell too far behind and missed `missed` events: read the chain again. |

A comment line keeps the connection alive every 15 seconds.

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
- `approval_epochs`, how many epochs an approval counts for;
- the `proposals` with approvals that still count in the tip's epoch, each with its `signers`,
  `threshold`, `change_threshold` and `approvals`. Each approval has its `signer`, the `epoch`
  it was made in, and the last epoch it counts in (`counts_until`).

Sets show their `signers`, `threshold` and `change_threshold`.

404 under any other kind.

### `GET /evidence`

Conflicting votes this node has seen, newest last. Each has its `voter` and both votes. Each vote
is shown readable, and `encoded` as `chainv4 send slash` takes it.

## Serving web pages

**Other origins:** browsers only let a page call an API on its own origin. `--cors ORIGIN` lets
pages from another origin call the API, such as `http://localhost:8081`, or `*` for any. Repeat
it for several. Allowed pages get the CORS headers, and their preflight questions are answered.

**The explorer:** `--explorer DIR` serves the [explorer](explorer.md)'s web app, built with
`trunk build` into `explorer/dist`, from the API's own `/`. Its files never shadow a route, and
only plain paths below the directory are served.

## What to keep in mind

- **Plain HTTP only.** The CLI is built without TLS, so keep the API on a trusted network.
- **One request at a time.** A read-only call runs on the API's thread under the chain's fuel
  cap, and holds up every other request while it runs.
- **`/node`, `/mempool` and `/stream` ask the node.** The rest read the database; these go to
  the node loop, which answers between blocks.
- **Reads are not snapshot-consistent.** A request that reads several keys can see two tips.
  Proofs are the exception: each is checked against its root before it is sent.
- **Proofs are against the tip only.** The node keeps one state tree, so it cannot prove what
  state held at an older block.
