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
  `amount`, `bytes`, `bool`, `string`, `number`, `i64`, `u64`, `i128`, `u128`, `i256`, `u256` or
  `nil`,
  whose value is `null`.

| Status | When |
| --- | --- |
| 400 | A malformed request, or a transaction the mempool refuses for what it is: invalid, or an offer that cannot pay. |
| 404 | Nothing there, such as a transaction not on the main chain yet. |
| 409 | A nonce already used, or the same sender and nonce already waiting. |
| 429 | A faucet asked again too soon, or a client with too many requests in flight already. |
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
| `vote_rewards` | Where vote rewards come from: `source` `mint`, or `fees` with the `pool` address and its `balance`. |
| `max_block_bytes` | The most bytes a block's transactions may take, encoded, or `null` when genesis sets none. |
| `bootnodes` | The chain's bootnodes at the tip: the ones governance decided last, or genesis's. |
| `swap` | `null`, or `{"contract": …}`: the [swap contract](swaps.md) a node started with `--swap-contract` names, for the wallet and `chainv4 swap`. |

### `GET /blocks?before=H&limit=N`

Block summaries, newest first, below height `before`, `limit` of them: 20 by default, at most
100. `next` is the cursor for the next page, or `null` at genesis.

Each summary has:
- `id`, `height`, `parent`, `timestamp`, `producer`;
- `difficulty`, `tx_root`, `receipts_root`, `state_root`, `gas_used`, `base_price`;
- `reward`, what it minted for its producer by the genesis reward schedule, `"0"` for genesis
  and on a chain without a reward;
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
- `transfer`, `mint`, `burn`, `contract_call`, `evm_call`, `deploy`, `upgrade`;
- `bond`, `unbond`, `withdraw`, `slash`, `approve_signers`, `withdraw_approval`;
- `approve_change`, with its `version` and `change`, which holds the new `consensus` as a genesis
  file writes one, the new `bootnodes`, or the new `name_prices` as decimal strings, and
  `withdraw_change`;
- `ethereum`, with its signed bytes as `raw`, its Ethereum `hash`, and what they say: `kind`, 0, 1
  or 2, `chain_id`, `to`, `value` and `input`;
- `name`, with the `name`, without the suffix, and its `action`: a `type` of `register` or `renew`
  with `years`, `transfer` with `to`, `target` with `target`, `text` with `key` and `value`, or
  `subname` with `label` and `owner`; and `primary_name`, with the `name` or `null`.

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
On a chain with [names](names.md), `name` too: the account's primary name with its suffix, such as
`alice.dev`, while that name resolves to it, or `null`.

### `GET /accounts/{address}/txs`

The transactions that touched an address, newest first, from the node's address index: those it
sent, those that paid or minted to it, and every call that named it among its arguments (see
[Storage](storage.md#the-indexes)). Only a node started with `--index-addresses` keeps one; on any
other, 404.

**Pages:** `limit` of them (50 by default, at most 200). `next` is the `cursor` for the next,
older page, or `null` after the last.

**Each transaction** is as `GET /blocks/{block}` lists it, with its `receipt`, plus its `block`,
`height` and `index`, its place in the block. It is the node's word: a light client proves each
with `GET /proofs/receipts/{id}`.

**Coverage:** `indexed_from` and `catching_up_to`, as for `GET /events`.

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

### `GET /light`

What a [light client](light-client.md) starts from: the chain's `genesis` id, its light
parameters (`params`) and its genesis header with seal (`header`), each encoded, in hex. A client
checks that the header hashes to the genesis id it trusts, and the parameters to the header's
`proof_root`. The parameters hash as genesis commits to them, the proof-of-work window encoding
as nothing while unset, so chains from before the window keep their `proof_root`.

### `GET /headers?from=H&count=N`

Main-chain headers with their seals, from height `from` (1 by default), at most `count`, at most
1,000: `{"headers": ["0x…", …]}`, each a `SealedHeader` encoded. Below the pivot of a node that
synced from a snapshot, these are all it has.

### `GET /light/finality?from=E&count=N`

What light clients follow finality with, an epoch at a time from epoch `from` on (0, genesis, by
default), at most `count`, at most 64: `{"steps": ["0x…", …]}`, each a `FinalityStep` encoded:
- **`justification`:** the justification of the epoch's main-chain checkpoint; none for epoch 0.
- **`update`:** the light update the checkpoint block keeps, as `GET /light/updates/{block}` serves
  it; none before a snapshot node's pivot.

It stops before the first checkpoint past genesis it holds no justification for, and at the tip.
One answer carries a client across 64 epochs: each update a justification makes trusted names the
voters of the justifications after it, so the client checks them in order, without asking again.
404 on a chain whose finality does not vote.

### `GET /light/checkpoint/{block}` and `GET /light/checkpoint`

What a light client starts from at a checkpoint rather than genesis: the one named, or this
node's finalized checkpoint. `{"checkpoint": {"epoch", "height", "block"}, "start": "0x…"}`, a
`CheckpointStart` encoded:
- **`headers`:** the checkpoint's header and those below it, back to the checkpoint
  `activation_delay` epochs before, or genesis, and further back when consensus looks at more
  ancestors than that leaves (the proof-of-work window), consecutive, with their seals.
- **`updates`:** each of those checkpoints' light updates, oldest first, null where none is kept.
  Empty under depth finality or none, where any block is a checkpoint and its header is all.

404 for a block not on the main chain, or before anything past genesis is final; 400 for one that
opens no epoch under voting finality. See
[Starting at a checkpoint](light-client.md#starting-at-a-checkpoint).

### `GET /light/updates/{block}`

The light update genesis or a checkpoint keeps: `{"update": "0x…"}`, a `LightUpdate` encoded, with
proofs of the voters of the epochs ahead of it, the one it fixed and those re-stated, against its
state root. 404 for any other block, and for those
before a snapshot node's pivot.

### `GET /justifications?from=E&count=N`

The justifications this node holds for main-chain checkpoints from epoch `from` on, in order, at
most `count`, at most 64: `{"justifications": ["0x…", …]}`. It stops at the first checkpoint it
holds none for. 404 on a chain whose finality does not vote.

### `GET /proofs/{column}/{key}`

A proof of one chain-state entry against the tip's state root (see
[Chain](chain.md#the-state-root)). `column` is one of `accounts`, `contracts`, `storage`,
`validators`, `epochs`, `tokens`, `namespaces` and `names`; `key` is the entry's key in that column, in hex,
as [Storage](storage.md#keys-and-encoding) describes. An account's key, for example, is its
address's tag and bytes: 32, or 20 for a secp256k1 address.

`?block=ID` proves at a recent main-chain block rather than the tip, one within the last
`max_reorg_depth` blocks, whose undo records the node keeps: 404 for any other.

It answers:
- `block`, `height` and `state_root`: the block it proves against;
- `proof`: the `StateProof`, encoded, in hex;
- `column`, `key`, and the entry's `value` in hex, or `null` when it holds nothing;
- `siblings`: the hash beside the path at each branch, the root's first;
- `leaf`: where the path ends, its `path` and `value_hash`, or `null` where it ends empty.

`StateProof::verify` checks it. A light client that trusts the header needs nothing else.

**Consistency:** the node checks each proof against the root before it answers. A block committed
while the proof was read would mix two trees, so the proof is read again, three times at most,
and then it answers 503.

### `GET /proofs/receipts/{id}`

A proof that a transaction is in its block, who sent what, and what it did: its receipt in the
block's receipts root, and the signed transaction at the same place in its `tx_root`. It answers:
- `tx`, `block`, `height`, `receipts_root` and `tx_root`;
- `index` and `count`: its place among the block's transactions, and how many there are;
- `receipt`: its `CommittedReceipt`, encoded, in hex, and `path`, the Merkle path from its leaf,
  the deepest sibling first;
- `signed`: the `SignedTx`, encoded, in hex, and `tx_path`, its Merkle path in `tx_root`.

`crypto::merkle::verify(root, leaf(hash, bytes), index, count, path)` checks each, and a light
client's `check_receipt` and `check_tx` check them against a header it followed. It answers 404
for a transaction not on the main chain.

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
- a gas limit of its intrinsic cost, plus the fuel cap's worth for a call, EVM call, deploy or
  upgrade;
- a max price of twice the next base price plus the tip under a base fee, or the base price
  plus the tip when senders bid;
- a tip of 0.

**A dry run** of a call, EVM call, deploy, upgrade or name transaction comes first: it runs on a throwaway copy of the
committed state, as its sender at their next nonce, on a thread apart from the rest of the API.
Unless the sender chose a gas limit, the offer's is what the run used plus a quarter of what it
ran past its intrinsic cost, at most the fuel cap's worth; a run that fails gets the cap.

It answers `gas_limit`, `max_price` and `tip`, all zero on a chain without fees, and `estimate`:
the dry run's `gas_used` and `fuel_used`, `status`, `ok` or `failed` with `error`, `returned`,
the values a call returned, and `deployed`, the address a deploy made, each `null` otherwise;
`null` for anything else. An EVM call returns one `bytes` value, its return data, so
`estimate.returned` is how `chainv4 send evm-call --read` reads, as `eth_call` does. Clients that cannot run the chain's fee code, such as the
[explorer](explorer.md) in a browser, use it, and warn before sending what would fail.

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

### `POST /rpc`

Ethereum's JSON-RPC, on a node with the `ethereum` feature: one request or a batch, each answered
with status 200 and its result or error inside, as JSON-RPC does. `eth_chainId`, balances, nonces,
code and storage, `eth_call` and `eth_estimateGas`, fees, `eth_sendRawTransaction`, transactions
and receipts by their keccak hash, blocks, and `eth_getLogs`. Only the latest state is kept. See
[Ethereum's tools](ethereum.md#json-rpc).

### `GET /faucet` and `POST /faucet`

On a node that runs a [faucet](faucet.md). `GET /faucet` answers its settings, as `GET /status`
shows them under `faucet`, and how much it holds:

```json
{ "address": "ed25519:0x…", "amount": "10000000000000000000", "every_ms": 86400000, "work_bits": 20, "window_ms": 600000, "balance": "…" }
```

`POST /faucet` with `{"to": "ed25519:0x…", "window": W, "nonce": N}`, the proof of work for that
address and window, sends a drip and answers `{"id", "to", "amount"}`. It answers 400 for work
that falls short or is for another window, or an address the chain takes no transfer to; 429 with
how long to wait when the address or the connection had a drip this period; 503 when the faucet is
dry; and 404 on a node with none.

### `GET /names`

The chain's [names](names.md): their `suffix`, the `prices` a year costs now by length, as decimal
strings, the `genesis_prices` governance may have replaced, `grace_ms` and `max_years`. 404 on a
chain without names.

### `GET /names/{name}`

A name at the tip, with or without the suffix, in any case: `name` without the suffix and
`display` with it; `status`, `active`, `grace` or `available`; its `owner`, `target`, and
`resolves_to`, the target while it is active; when it `expires` and its `grace_ends`, in
milliseconds since the Unix epoch, a subname's being its name's; its `texts`, as an object; and
`price_per_year`, what a year of it costs now, `null` for a subname. An available name has `null`
for everything it would hold. 400 for what cannot be a name, 404 on a chain without names.

### `GET /accounts/{address}/names`

The names and subnames an address owns that still stand, in name order: each with its `name`,
`display`, `status`, `target` and when it `expires`. A page at a time: `limit` of them (50 by
default, at most 200), and `next`, the `cursor` for the next page, or `null`. 404 on a chain
without names.

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
| `mining` | `null` for a node that does not mine. Otherwise the hashes a second of everything mining for it (`rate`); its own `devices`, each a `name` and `rate`, its GPUs or CPU threads; where it takes workers (`listening`), or `null`; and its `workers`, each with its `name`, `address`, `since`, `rate`, `devices`, the seals it `found` and the nonces `refused` (see [Mining with workers](consensus.md#mining-with-workers)). Rates are measured over the last few seconds. |

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

Sets show their `signers`, each with its voting `key` and the `account` that speaks for it, and
their `threshold` and `change_threshold`. An `approve_signers` transaction shows the same.

404 under any other kind.

### `GET /governance`

On a chain with [governance](governance.md):
- its `kind`, `electorate` or `council`;
- `epoch`, the tip's;
- the `active` consensus, the one fixed for the tip's epoch, and the `decided` one and its
  `version`, each as a genesis file writes a consensus;
- `approval_epochs`, how many epochs an approval counts for, and `needed`, what a change's
  approvals must weigh now;
- `bootnodes`, the chain's now;
- the `proposals` with approvals that still count in the tip's epoch, each with its `consensus`,
  its `bootnodes` or its `name_prices`, what its approvals `weight` and what they `needed`, and its `approvals`. Each approval has its
  `account`, what it weighs now, the `epoch` it was made in, and the last epoch it counts in
  (`counts_until`).

404 on a chain without governance.

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

**The wallet:** `--wallet DIR` serves the [wallet](wallet.md)'s web app, built with `trunk build`
into `wallet/dist`, from `/wallet/`, so it talks to the node that served it. `/wallet` redirects
there, since its files load relative to the page. Nothing under `/wallet/` ever falls through to
the explorer.

## What to keep in mind

- **Plain HTTP only.** The CLI is built without TLS, so keep the API on a trusted network.
- **Requests are answered side by side,** on a thread a core (`--http-threads` for another
  count), dry runs and contract calls on the blocking pool, so a slow one holds up no other.
- **Each client may have 64 requests in flight at once** (`--http-per-ip`); more get 429. A client
  is its IP, or behind a proxy on the node's own machine or network, such as Docker's nginx, the
  last address `X-Forwarded-For` names. `GET /stream` is not counted.
- **`/node`, `/mempool` and `/stream` ask the node.** The rest read the database; these go to
  the node loop, which answers between blocks.
- **Reads are not snapshot-consistent.** A request that reads several keys can see two tips.
  Proofs are the exception: each is checked against its root before it is sent.
- **Proofs reach back `max_reorg_depth` blocks.** The node keeps one state tree, and rolls it back
  by undo records to prove at an older block, so it can only go as far back as it keeps those.
- **A snapshot node's history is headers only** before its pivot: `GET /blocks` and
  `GET /blocks/{block}` show those blocks marked `"pruned": true`, without transactions.
