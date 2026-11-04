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
| 400 | A malformed request, or a transaction the mempool refuses for what it is: invalid, an offer that cannot pay, or a nonce too far past the account's. |
| 404 | Nothing there, such as a transaction not on the main chain yet. |
| 409 | A nonce already used, the same transaction already waiting, or one at its sender's nonce that does not raise both prices enough to replace the one waiting. |
| 413 | A body past room for the chain's largest transaction, hex-encoded: 2 MiB, or more on a chain whose `max_tx_bytes` takes more (`body_cap`). |
| 429 | A faucet asked again too soon, a client with too many requests in flight already or asking faster than `--http-rate`, with `Retry-After: 1`, or a sender with as many transactions waiting as the node keeps of one. |
| 500 | The database failed. |
| 503 | No node behind the API, a full mempool that this offer could not make room in, or a node that stopped. |

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
| `finality` | `null`, or `type`, the kind the tip's epoch runs, `epoch`, `checkpoint_interval`, and the `justified` and `finalized` checkpoints, each with `epoch`, `block` and `height`. |
| `fees` | `null`, or the fee settings in force, governance's or genesis's, as genesis files write them, with `base_price`, what a transaction sent now must offer at least, and `tx_fuel`, the chain's fuel cap per transaction. |
| `vote_rewards` | Where vote rewards come from: `source` `mint`, or `fees` with the `pool` address and its `balance`. |
| `max_block_bytes` | The most bytes the next block's transactions may take, encoded, as governance or genesis set it, or `null` for none. |
| `bootnodes` | The chain's bootnodes at the tip: the ones governance decided last, or genesis's. |
| `upgrade` | `null`, or the [upgrade](updates.md#upgrades) governance decided last: nodes older than `least` stop before block `from`. |
| `version` | The node's own chainv4 version, such as `0.1.0`. |
| `name` | The chain's name, genesis's or the one governance decided last, such as `chainv4 testnet`, or `null` for a chain that has none: what the apps call the network. |
| `dex_fee_bps` | The DEX's fee for the next block, in hundredths of a percent, which both DEXes charge. |
| `evm_spec` | The EVM's spec for the next block, such as `prague`. |
| `swap` | `null`, or `{"contract": …}`: the [swap contract](swaps.md) the `directory` names, else the one a node started with `--swap-contract` names, for the wallet and `chainv4 swap`. |
| `dex` | `null`, or `{"contract": …, "router": …}`, either `null`: the [DEX](dex.md) and the EVM DEX's router the `directory` names, each else the one a node started with `--dex-contract` and `--dex-router` names. |
| `operator` | `null`, or what the node's operator named with `--swap-contract`, `--dex-contract` and `--dex-router`, as `swap`, `dex` and `dex_router`, each only when given: a suggestion, shown apart, that fills only what the directory leaves unset. |

### `GET /blocks?before=H&limit=N`

Block summaries, newest first, below height `before`, `limit` of them: 20 by default, at most
100. `next` is the cursor for the next page, or `null` at genesis.

Each summary has:
- `id`, `height`, `parent`, `timestamp`, `producer`;
- `difficulty`, `tx_root`, `receipts_root`, `state_root`, `gas_used`, `base_price`;
- `reward`, what it minted for its producer by the genesis reward schedule, `"0"` for genesis
  and on a chain without a reward;
- `tx_count` and `proof_count`;
- `pool_root`, and `pool`: `null`, or a mining pool's commitment, its `parts` of a billion sent
  to the pool's escrow and their `percent`, and the `root` and `count` of its claims (see
  [Mining pools](consensus.md#mining-pools)).

### `GET /blocks/{block}`

**Which block:** `block` is `latest`, a height, or a `0x` block id.

**What it returns:**
- the summary;
- whether it is on the main chain (`main`);
- its `seal`;
- its `proofs`, each with its link and voters;
- for a pool's block, in `pool`: what it `shared` out, its reward and its producer's fees, what
  went to the `escrowed` account, and the `escrow`;
- every transaction.

**Each transaction** has:
- its `id`, `sender`, `nonce` and `signer`;
- its `fee` offer: `gas_limit`, `max_price`, `tip`;
- its `signature` and its `tx`;
- its `receipt` when the block is on the main chain.

**Each `tx`** has a `type` and that type's fields:
- `transfer`, `mint`, `burn`, `contract_call`, `evm_call`, `deploy`, `upgrade`;
- `bond`, `unbond`, `withdraw`, `slash`, `approve_signers`, `withdraw_approval`;
- `vote_change`, with its `version`, its `vote` (`yes`, `no` or `abstain`), the change's
  `kind`, and `change`, which holds the new `consensus` as a genesis file writes one, the new
  `bootnodes`, or any setting as [governance](governance.md#a-change) lists them; and
  `clear_vote`, with its `version`, `kind` and `change`;
- `multisig`, a multisig's own: its `account`, the `tx` it sends and the members' `signatures`,
  each with `key` and `signature`; `create_multisig` and `change_multisig` with `members`, each a
  `key`, its `bitcoin` xpub and its `evm` address or `null`, `threshold` and `testnet`;
  `approve_multisig` with `account`, `nonce`, `fee` and `tx`; `withdraw_multisig` with `account`
  and `proposal`;
- `ethereum`, with its signed bytes as `raw`, its Ethereum `hash`, and what they say: `kind`, 0, 1
  or 2, `chain_id`, `to`, `value` and `input`;
- `name`, with the `name`, without the suffix, and its `action`: a `type` of `register` or `renew`
  with `years`, `transfer` with `to`, `target` with `target`, `text` with `key` and `value`, or
  `subname` with `label` and `owner`; and `primary_name`, with the `name` or `null`;
- `claim`, a miner's claim on a mining pool: the `height` of a block of the pool's, the miner
  `to`, all it `earned`, decimal, and its leaf's `index` and Merkle `path`.

### `GET /blocks/range?from=H&count=N&entry=E`

Blocks from height `from` on, in binary, each with what a client needs to check it itself: what
recomputes every root in its header, its receipts, and a proof against its `state_root` of each
state entry it changed. A wallet scanning a range picks out its own accounts' entries, and the node
never learns which it wanted. `/blocks/{block}` cannot do this: its JSON has no encoded
transactions and shows proofs as voters, so `proof_root` cannot be recomputed from it.

**The body** is `application/octet-stream`, a `light::BlockRange` encoded with wincode. Errors are
JSON, as everywhere. `light::decode_block_range` reads it, safely on any bytes:

| `BlockRange` | |
| --- | --- |
| `tip` | The node's tip as it answered |
| `items` | The blocks, in height order; none when asked from past the tip |
| `next` | Where to go on from, as `{height, entry}`, which are the next `from` and `entry`; `None` once it reached the tip |

| `RangeItem` | |
| --- | --- |
| `sealed` | The header and seal |
| `body` | `Whole {txs, proofs, pool, receipts}`; or `HeaderOnly`, below where a snapshot node took state; or `Continued`, when going on through the block's entries |
| `changes` | `Proven {from, listed, entries}`, or `None(NoProofs)`, which says why not |

Each `ProvenEntry` is a `column`, a `key`, the `value` once the block had run, `None` for an entry
it deleted, and a `StateProof` of that value or of its absence. `receipts` is `None` where the node
has none: the block a snapshot node took state at, which it never ran.

**Why an item has no proofs,** always with the block all the same:

| `NoProofs` | |
| --- | --- |
| `NoList` | No list of what it changed: genesis, the block a snapshot node took state at, the lowest block the node's history reaches, or a list lost |
| `StatePruned {kept_from}` | State is kept only from that height: past `--state-history`, or below a snapshot node's pivot |

**Bounds:** an answer stops at the first of `count` and 100 blocks, 8 MiB, 4,096 proofs and a second
of work (`RangeLimits::SERVED`), and says where to go on from. That may be inside a block: then
`next.entry` is above 0, the next answer leaves the body out, as `Continued`, and carries on through
its entries, so the largest block's proofs arrive within the bounds too. The one thing that may pass
them is the first block's body and receipts, which cannot be split, with one of its entries.

**Busy:** proofs are worked out as they are asked for, so a client may have 2 of these under way
(429 past that), half the API's threads build them, and the rest wait their turn for up to 2 seconds
(503 past that). Both answers say `Retry-After: 1`. Each is built on the blocking pool, so it holds
up no other request. A client asks for one at a time.

**Refused:** 400 with no `from`, or an `entry` past the block's list, as after a reorg; 404 for a
height the node holds nothing of, not even a header: it started at a checkpoint above it.

**Checking it:** see [Checking a range](light-client.md#checking-a-range). The list of what a block
changed is the node's word, as nothing in a header commits to it: **every entry given is proven, but
a list that leaves one out cannot be told.** Nor can a node that says it holds no proofs be caught
out.

**What it costs,** per block, on RocksDB holding the state in cache, 60,000 accounts (M3 Pro):

| Block | Block alone | Item without proofs | With proofs | Entries | Building it | Checking it |
| --- | --- | --- | --- | --- | --- | --- |
| Empty, paying a reward | 386 B | 398 B | 2.0 KB | 2 | 34 µs | 53 µs |
| A deploy | 711 B | 854 B | 3.4 KB | 3 | 59 µs | 23 µs |
| 100 transfers to new accounts | 27 KB | 36 KB | 110 KB | 103 | 3.6 ms | 2.4 ms |
| 2,000 transfers | 524 KB | 720 KB | 2.2 MB | 2,002 | 63 ms | 20 ms |
| 10,000 transfers | 2.6 MB | 3.6 MB | 10.9 MB | 10,002 | 182 ms | 58 ms |
| 100 contract calls, each writing a key | 32 KB | 43 KB | 120 KB | 103 | 1.5 ms | 0.6 ms |
| 1,000 contract calls | 320 KB | 426 KB | 1.2 MB | 1,003 | 19 ms | 6 ms |

So a proof takes about 730 bytes, more by 33 each time the state doubles, 15 to 30 µs to build and
6 to 20 µs to check. A cold disk adds what some 34 reads take to each. The figures are one block in
one answer; as served, the block of 10,000 transfers takes 3 answers of at most 6.6 MB. To run them
again: `what_a_block_costs_with_its_proofs`, in `src/chain/tests/range.rs`.

### `GET /txs/{id}`

**Before it lands:** 404 until the transaction is on the main chain.

**Once it lands:**
- where it is: `block`, `height`, `index`, `confirmations`, and whether it is `final`;
- what it cost: `fuel_used`, `gas_used`, `fee`;
- its `events`;
- its `payments`: the coin its contracts paid, each `from`, `to` and `amount`, as the receipts
  root commits to them (see [Chain](chain.md#receipts));
- its `status`: `done`, `deployed` with `contract`, `upgraded` with `contract`, `returned` with
  `values`, `decided` for the [governance](governance.md#decisions) approval that decided a
  change, with `version`, the chain's count it made, its `kind` and `kind_version`, that kind's,
  or `failed` with `error`;
- its `trace`: where its contracts went (see below).

**The trace** is `hops`, depth first, and `dropped`, how many hops past the 256 a receipt keeps
were only counted. Each hop has:
- `depth`: 0 for what the transaction ran itself;
- `kind`: `call`, `delegate`, `transfer` or `clone`, a contract created by another;
- `from`, `to`, `function` and `value`;
- `fuel_used`, including the hops under it;
- `error`: why it failed, or `null`;
- `events`: where the events it emitted itself sit in the receipt's `events`, empty once it or
  anything above it failed.

A transaction that runs no contract has no hops. Receipts in `/blocks/{block}` carry the trace
too.

A [batch](chain.md#batches) writes its `tx` as `{"type":"batch","steps":[…]}`, each step as that kind
of transaction is written. Its receipt's `status` is `batch`, with `steps` in order, each with what
that kind's status says, and `events` and `payments`, how many of the receipt's lists are that step's:

```json
{"status":"batch","steps":[{"status":"deployed","contract":"contract:0x…","events":0,"payments":0},{"status":"returned","values":[…],"events":2,"payments":1},{"status":"done","events":0,"payments":0}]}
```

One that failed has `status` `failed`, as any failure, so what reads no further sees that, with
`failed_step`, counted from 0, and `error`, which says the step counted from 1 and why:
`step 2 failed, undoing every step: insufficient balance`.

### `GET /accounts/{address}`

`address`, `nonce` and `balance`, and the `height` read at. An address never written to reads as
nonce 0 and balance 0. `?block=` reads at an older main-chain block, by its height or id, while the
node keeps its state; 404 past that.
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
`height`, `index`, its place in the block, and `time`, when its block was made. It is the node's
word: a light client proves each with `GET /proofs/receipts/{id}`.

**Coverage:** `indexed_from` and `catching_up_to`, as for `GET /events`.

**Filters,** any together, all optional; one left empty is one not given. Nothing is stored for
them, and no format changes: the index is by height, so a height range seeks and stops, and the
rest are tested against the block and receipt each entry points to.

| Parameter | Keeps |
| --- | --- |
| `kind` | The transaction's `type`, such as `transfer`, `contract_call` or `deploy`. A multisig's counts as `multisig` and as what it sent. |
| `direction` | `out`: the address sent it. `in`: it took part and did not send it, or paid itself. |
| `counterparty` | An address the transaction touches, by the index's own rules: its parties, the contract called, addresses among a call's arguments, what its contracts paid. |
| `token` | `coin`: it asks to move the coin, or a contract paid some. A contract's address: it called that contract, or that contract emitted something while it ran. |
| `from`, `until` | When its block was made, in milliseconds since the Unix epoch, both ends included. |
| `from_height`, `until_height` | Its block's height, both ends included. |

**A page holds matches,** however many others lie between, and `next` goes on from where it stopped,
under the same filters. A request looks at no more than 1,000 index entries, so one query cannot tie
up the node. The answer says so:
- `scanned`: how many it looked at;
- `limited`: `true` when it stopped at that bound with its page not full, so more may match from
  `next` on, and an app asks again with that cursor. A page may then hold fewer than `limit`, or
  none;
- `applied`: the filters it went by, `[]` with none. A node from before these has no such field and
  ignores the parameters, so an answer without it was not filtered, whatever was asked.

A bad value is 400: a `direction` other than those two, an address that does not read, a time or
height that is not a whole number. A range that holds nothing, or a `kind` no transaction has, is an
empty list.

```sh
curl 'localhost:8080/accounts/ed25519:0x…/txs?kind=transfer&direction=in&from=1793318400000'
# {"txs":[…],"next":null,"indexed_from":0,"catching_up_to":null,"applied":["kind","direction","from"],"scanned":41,"limited":false}
```

A batch is listed under everyone any step names, and everyone contracts paid. `kind` passes it for
being a `batch` and for what any of its steps is; `token` and `direction` go by all its steps.

### `GET /accounts/{address}/multisigs`

The [multisigs](multisig.md) an address's key is a member of, from the node's member index: each
with its `address`, `threshold`, how many `members` it has, and whether its members name a Bitcoin
xpub (`bitcoin`) and an EVM address (`evm`). `member` is the address it listed them for. A member
is found by the address its key controls, so `address` may be an address, or a member's key as a
multisig names it, `secp256k1:0x…` or `bitcoin:0x…`, whose address it stands for. Each listed is
read back from the registry, so the listing is the members as they are now: one the address was
removed from is gone.

**Pages:** `limit` of them (50 by default, at most 200), in the order of the multisigs' addresses.
`next` is the `cursor` for the next page, the last multisig of this one, or `null`.

Only a node started with `--index-multisigs` keeps the index; on any other, 404 (see
[Storage](storage.md#the-member-index)).

```sh
curl 'http://127.0.0.1:8080/accounts/ed25519:0x3b6a…/multisigs'
# {"member":"ed25519:0x3b6a…","multisigs":[{"address":"multisig:0x…","threshold":2,"members":3,"bitcoin":false,"evm":true}],"next":null}
```

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

### `GET /light/bundle?headers=H&epoch=E`

What a walking light client wants next in one answer, so it spends its time checking rather than
waiting on round trips: `tip`, the node's height; `headers`, as `GET /headers` encodes them, up to
32 epochs' worth from height `headers`; and `steps`, as `GET /light/finality` encodes them, up to 32
epochs from `epoch`. It checks them itself, a stretch at a time (`LightClient::deliver_bundle`). 404
under finality that does not vote, and from a node from before the route: the client then asks for
each as it goes.

### `GET /light/finality?from=E&count=N`

What light clients follow finality with, an epoch at a time from epoch `from` on (0, genesis, by
default), at most `count`, at most 64: `{"steps": ["0x…", …]}`, each a `FinalityStep` encoded:
- **`justification`:** the justification of the epoch's main-chain checkpoint; none for epoch 0,
  or for an epoch nobody justified with a later one justified.
- **`update`:** the light update the checkpoint block keeps, as `GET /light/updates/{block}` serves
  it; none before a snapshot node's pivot.

It ends where nothing justified follows, looking 4,096 epochs ahead, and at the tip.
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

`?block=ID` proves at a main-chain block rather than the tip, one whose state the node still keeps
(`--state-history`, 10,000 blocks by default, or every one with `--archive`; see
[Storage](storage.md#history)): 404 for any other.

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
shortly. One signed for another chain is refused with 400: a signature names its chain by the
genesis id `GET /status` says.

**What the mempool's limits refuse,** each with an error a wallet can act on (see
[the mempool](node.md#the-mempool)):

| Status | Error | What to do |
| --- | --- | --- |
| 400 | `nonce too far ahead: the account's next is N, …` | Send the nonces before it first. |
| 409 | `a transaction with this nonce is already waiting: to replace it, raise both its max price and its tip by at least 10%` | Raise both, or wait for the one waiting. |
| 429 | `this sender has 64 transactions waiting already, …` | Wait for some to land. |
| 503 | `mempool full: offer a higher tip …` | Offer more, or try again after a block. |

`eth_sendRawTransaction` says the same in the words Ethereum's nodes use, which wallets know:
`replacement transaction underpriced`, `nonce too high`, `account limit exceeded` and
`txpool is full`.

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
ran past its intrinsic cost, at most the fuel cap's worth; a run that fails gets the cap. That
limit is tried with a second dry run, and raised until a run succeeds at it: a call inside
another gets 63/64 of the gas left, so nested calls, such as a DEX router's, need more than they
use.

It answers `gas_limit`, `max_price` and `tip`, all zero on a chain without fees, and `estimate`:
the dry run's `gas_used` and `fuel_used`, `status`, `ok` or `failed` with `error`, `returned`,
the values a call returned, and `deployed`, the address a deploy made, each `null` otherwise;
`null` for anything else. An EVM call returns one `bytes` value, its return data, so
`estimate.returned` is how `chainv4 send evm-call --read` reads, as `eth_call` does. Clients that cannot run the chain's fee code, such as the
[explorer](explorer.md) in a browser, use it, and warn before sending what would fail.

A batch is always dry-run, whatever it holds; its `estimate` says `steps` when it went through and
names the step in `error` when not. The gas limit it offers is its intrinsic cost, a transaction's for
each step, and if any step runs code, one allowance of fuel for all of them.

### `POST /call`

**The body:** `contract`, `function` by name, `args` as a list of `{"type": …, "value": …}` the
way `values` are written, and optionally the `sender` it runs as (the zero address otherwise),
and the `block` to run at, a height or a 0x id, rather than the tip.

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
  transfers to or from an account, and every EVM log with its EVM address in a topic, as an
  ERC-20's `Transfer` names its parties: a secp256k1 account's or an EVM contract's own 20 bytes,
  any other account's alias;
- `topic=0x…`: every EVM log, from any contract, with this topic in any place. The first topic is
  the keccak256 of its event's signature, so this finds an event by its signature: `Transfer` is
  `0xddf252ad…b3ef`. An address is a topic too, as 12 zero bytes and its 20;
- `contract=…&topic=0x…`: one contract's EVM logs with this topic.

Anything else is refused with 400, a `topic` with `name` or `address` among them, and a topic that
is not 32 bytes. Every EVM log has the name `log`, spelled so: names match by bytes. Addresses in a
log's data, rather than its topics, are not found.

**Pages:** `limit` of them (50 by default, at most 500), whichever way they are asked for. `next` is
the `cursor` for the next, older page, or `null` after the last.

**Each event** has its `contract`, `name` and `data`, the `tx` that emitted it, its `block` and
`height`, the transaction's place in the block (`index`) and the event's place among the
transaction's events (`position`).

**Coverage:**
- `indexed_from` is the lowest height the index holds: every main-chain block's events from there
  to the tip are in it.
- `catching_up_to` is the height the node is still moving that to, or `null` once it is there.
  It is below `indexed_from` while the node indexes older blocks, and briefly above it on a
  `latest-N` window, as the oldest block leaves.
- `topics` is `true`: this node's index holds EVM logs' topics. **A client asking for a topic must
  check it.** An older node leaves it out, and ignores what it does not know: it refuses `topic=`
  alone with 400, and answers `contract=…&topic=…` with all that contract's events, unfiltered.
  Its `address=…` leaves EVM logs out, with nothing to say so but this field's absence.

An index from before topics starts over once, the first time a node opens it: see
[Storage](storage.md#the-indexes).

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

### `GET /names/list?after=NAME&limit=N`

Every name with a record, top-level names and subnames, in name order: those after `after`, with
or without the suffix, at most `limit` (20 by default, at most 100). Each has its `name`,
`display`, `status`, `owner`, `target` and when it `expires`, as `GET /names/{name}` gives them;
`next` is the name to pass as `after` for the next page, `null` on the last. 404 on a chain
without names.

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

### `GET /collections`

The [collection registry](nft-standard.md#the-collection-registry): every contract that exports
what makes an NFT or a multi-token collection on this chain. Ordered, searched with `symbol=…`
and paged with `limit` and `cursor` as `GET /tokens` is.

**Each collection** has its `address`, its `kind`, `nft` or `multi`, and its `name` and `symbol`,
as it answered at its deploy or last upgrade, each `null` when it exports none.

`GET /status` also names the [bridges' federation](bridges.md#the-federation) in force, in
`federation`: its `signers`, `threshold`, depths and `version`, or `null` on a chain with none.
And the chain's [directory](governance.md#directory), in `directory`: its `gateway`,
`btc_gateway`, `evm_gateways`, `swap`, `dex`, `dex_router` and `nft_factory`, each `null` while
there is none, or `null` on a chain without one. Both are state entries,
`/proofs/validators/{key}` proves them: `change:federation` and `change:directory`, as hex. On a
chain without governance, `directory_admin` is who sets the directory, or `null` for nobody
(`directory:admin`).

`GET /contracts/{address}` says the same of one contract in `collection`, `null` when it is none;
`GET /status` says what makes one in `collections`, `{"nft": [...], "multi": [...]}`, as genesis
or governance decided last, or `null` on a chain without the registry.

### `GET /node`

How the node behind the API is doing. 503 when no node runs behind it.

| Field | What it is |
| --- | --- |
| `started`, `now`, `uptime_ms` | When its loop first ran, the time of the report, and the time between. |
| `listening` | Where peers reach it, each ending in `/p2p/` and its peer id. |
| `producing`, `validating` | Whether it produces blocks and votes on finality. |
| `mempool` | `count` waiting and the `capacity`. |
| `syncing` | `null`, or the `peer` it fetches blocks from and the height `from`. |
| `counters` | What it heard and did since it started: blocks heard, imported, stored on side branches, rejected and produced; reorgs; transactions heard, submitted and refused; votes heard and cast; sync requests served, sent and failed; connections; gossip and submissions the backlog dropped (`gossip_dropped`), peers disconnected for flooding (`flooders_disconnected`), and what it published that reached none of its connected peers (`gossip_unpublished`). |
| `backlog` | What waits for the node loop now (`waiting`), sync requests and blocks held for their parent included, and the backlog's limits, `total`, `per_peer` and `per_ip`, sync requests' rate, `requests` a second past `request_burst`, and its strike rules, `strikes` within `strike_window_ms` (see [Node](node.md#the-gossip-backlog)). |
| `limits` | What its operator tuned: every key of [Node's limits and tuning](node.md#limits-and-tuning), such as `ban.score`, `sync.headers` or `http.rate`, with its value, times in milliseconds and sizes in bytes, `null` for one unset such as `net.max-peers-in` without the flag. `{}` under `--report minimal` or `--hide limits`. |
| `peers` | Each connected peer's `id`, `address` and `since`, what it gossiped (`heard`: `blocks`, `txs`, `votes`), how much of it the backlog `dropped`, and its `chain` from its last status (`height`, `work`, `justified`), or `null`. |
| `mining` | `null` for a node that does not mine. Otherwise the hashes a second of everything mining for it (`rate`); its own `devices`, each a `name` and `rate`, its GPUs or CPU threads; where it takes workers (`listening`), or `null`; its `workers`, each with its `name`, `address`, `since`, `rate`, `devices`, the seals it `found` and the nonces `refused`, and under a pool its `payout` address, the `shares` taken and its `share_difficulty` (see [Mining with workers](consensus.md#mining-with-workers)); and `pool`, `null` or the pool's `fee`, `window`, whether it is `open`, and how many `miners` and `blocks` it counts. Rates are measured over the last few seconds. |

### `POST /node/unban`

Lifts bans on the node behind the API: in its database, so they stay lifted past a restart, and on
the network at once. The body names which, one of `{"all": true}`, `{"peer": "12D3…"}` or
`{"ip": "203.0.113.7"}`; lifting an IP leaves its peer banned by id. It answers `{"lifted": n}`,
the peer ids and IPs the database let go.

Only the operator may: a request from this machine with nothing forwarding it, or one bearing
`--report-token` as `Authorization: Bearer …`. Anyone else gets 403. 400 for a body naming none,
more than one, or what does not read; 503 when no node runs behind it. See
[Node](node.md#bans).

### `GET /claims/{pool}/{address}`

On every node, pool or not: what `address` has `claimed` from `pool`, the pool's `escrow` and its
`escrow_balance`, and the tip's `height`. With `?height=`, `root` is the root the pool's block at
that height on the main chain committed to, its `root` and `count`, or `null` for a block that is
not the pool's or carries none. What a miner's kept claim (see
[Mining pools](consensus.md#mining-pools)) is checked against before it is sent: one that holds
under that root pays.

### `GET /pool`, `GET /pool/{address}` and `GET /pool/claim/{address}`

On a node that runs a [mining pool](consensus.md#mining-pools); 404 otherwise.

`GET /pool` answers its `producer`, which keeps the fee; its `escrow` and the `escrow_balance`;
its `fee`, in parts of a billion, and `fee_percent`; its `window`, whether it is `open`, its
`min_share`, the `work` in the window and all its miners have `earned`. Then its `miners`, each
with its `work` and `shares` in the window, its `parts` and `percent` of the next block, all it
`earned`, what it `claimed` and what is `claimable`; and its `blocks`, newest first, each with its
`height`, `id`, `at`, whether it was `credited` (`null` while it may be undone, `false` for one
the chain dropped), what it `shared` out, and each miner's `payouts`, with the `amount` each came
to. Amounts are decimal strings.

`GET /pool/{address}` is the same for one miner, with its connected `workers`, each with its
`name`, `rate`, `shares` and `share_difficulty`.

`GET /pool/claim/{address}` is a miner's claim, ready for `Tx::Claim`: the `pool` and `escrow`,
the `height` and `block` of the newest of the pool's blocks on the main chain whose root holds it,
all it `earned`, what it `claimed` and what is `claimable`, and its leaf's `index`, the root's
`count`, the Merkle `path` and the chain's `hash` it is in. `settled` is the highest height the
pool's blocks can no longer be undone at, as its books know, or `null`: what a miner keeping its
claims prunes by. 404 for a miner the pool owes nothing yet.

### What a node keeps to itself

`GET /node`, `/stream`, `GET /pool/{address}` and `GET /mining` leave out what the node's
[`--report`, `--hide` and `--show`](node.md#what-a-node-shares-of-itself) say to keep, unless the
request comes from the node's own machine or bears its report token, `Authorization: Bearer
TOKEN`. Each such field stays, empty: `""`, `[]`, `{}`, `0`, `false` or `null`. `GET /node`'s
`hidden` lists the fields left out of this answer, and `token_refused` says it was sent a token it
does not take.

### `GET /mining` and `GET /mining/ws`

On a node that takes workers, `--mining`; 404 otherwise. `GET /mining` tells the apps where to mine
for it: the mining port's `port`, where it is reached from outside, `public` (`--mining-public`, or
`null` for the API's own host), where through Tor, `onion` (`--mining-onion`, or the onion in
`--onion-file` once tor has made it with `--mining-onion-port`; `null` otherwise), which an app that
reaches the node at its onion takes over `public`, the `protocol` it speaks, whether it runs a `pool` (with its `fee`,
in parts of a billion, and `window`), takes workers mining alone (`solo`, `--mining-solo`) and takes
anyone (`open`, `--mining-open`, whose workers prove the open token), the `ws` path below and how
many `workers` are connected, or `null` when the node keeps its workers to itself.

`GET /mining/ws` is the mining port over a WebSocket, for a browser, which opens no TCP
connections, and behind TLS for any worker on an open port, whose token is public
([why](consensus.md#mining-with-workers)): each text message is one line of the port's protocol,
relayed to the port on the node's machine as it is and back, so the handshake proves the token to
the port itself. A message holding a line end, or anything but text, ends it. The port sees the
relay as a connection from the node's own machine, so 32 are allowed from each asker, by
`X-Forwarded-For` from a `--trusted-proxy`.

### `GET /release`

The newest [release](updates.md) the node took, for the apps, workers and `chainv4 update` to check
against their own release keys: the node only relays it.

| Field | What it is |
| --- | --- |
| `version`, `target` | The node's chainv4 version and the target it was built for. |
| `updates` | Whether its build trusts any release keys; without, it takes no releases. |
| `manifest` | `null`, or the manifest as signed: `release` and `signatures`. |
| `standing` | Where the node stands against the newest chainv4: `current`, `behind`, `unsupported` (below its `least`), or `unknown` without a manifest that still counts. |
| `expired` | Whether the newest manifest it took is past its `expires`: it is still relayed, for what orders the next one, and nothing acts on it. |
| `newest` | The newest chainv4, when the node is behind. |
| `checked`, `error` | When the node last looked, in milliseconds, and why that failed, or `null`. |
| `installed` | The version `--auto-update` put in place, which runs once the node restarts, or `null`. |
| `upgrade` | `null`, or the upgrade that stops this node first, with `least`, `from` and its `source`, `governance` or `release`. |

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

### `GET /multisig/{address}`

A [multisig](multisig.md): its `members`, each a `key`, its `bitcoin` xpub with its origin as a
descriptor writes it and its `evm` address in hex, each or `null`, its `threshold`, `testnet`, and
`bitcoin`: `null`, or its `network`, its `receiving` and `change` descriptors with their checksums
and its `first_address`; `evm`: `null`, or its contract on EVM chains, its `contract` address,
the `start` owners and threshold its address commits to and those its members name `now`, each
`owners` in hex and a `threshold`; its next `nonce` and `balance`, and the `proposals` open on chain
for that nonce, each with its `id`, the `tx` as JSON and `encoded` in hex, its `fee` offer and the
member keys that `approvals` back it. 404 for an address that is not one.

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

### `GET /supply`

The coin's supply at the tip, in the smallest unit as decimal strings: `minted`, every coin made
since genesis (its allocations and bonds, block rewards, minted vote rewards and `Tx::Mint`),
`burned`, every coin destroyed (fees' burned share on a chain without a reward pool, `Tx::Burn`,
names' prices, slashing and the inactivity leak), and `supply`, the one less the other. `staked`,
what validators have bonded, and `reward_pool`, its balance, are the node's word.

`minted` and `burned` are one state entry, `Supply`, in the validators column under `key` (hex),
so a light client proves them with [`GET /proofs/validators/{key}`](#get-proofscolumnkey).
Supply is what every balance, the reward pool's and contracts' among them, and every bond add up
to.

### `GET /rich?limit=N`

The accounts holding the most coin at the tip, richest first, at most `limit` (20 by default, at
most 100), each its `address` and `balance`, and `holders`, how many hold any. A scan of every
account at the request: the node's word, which nothing proves as a list.

### `GET /governance`

On a chain with [governance](governance.md):
- its `kind`, `electorate` or `council`;
- `epoch`, the tip's;
- the `active` consensus, the one fixed for the tip's epoch, and the `decided` one and its
  `version`, each as a genesis file writes a consensus;
- `approval_epochs`, how many epochs a vote counts for, `quorum_permille`, the quorum, `needed`,
  what Yes on a change must weigh now, and `total`, what every voter weighs together;
- `voters`, every account that may vote, each with its `weight`, and `rejections`, how many
  changes were rejected;
- `bootnodes`, the chain's now;
- `finality`, the kind the tip's epoch runs, and `switch`, a switch of kind decided and still to
  take effect, with the `epoch` it runs from and its `type`, or `null`;
- the `proposals` with votes that still count in the tip's epoch, each its change as one field
  named for what it changes (`consensus`, `bootnodes`, `name`, …, as
  [governance](governance.md#a-change) lists them), its `kind` and that kind's `version`, its
  `tally` (`yes`, `no`, `abstain`, `voted`, `total`, `needed`, `quorum`, `more_needed`,
  `quorum_met` and `can_pass`, weights as decimal strings), `closes`, the last epoch its oldest
  vote counts in, and its `votes`. Each vote has its `account`, its `vote`, `yes`, `no` or
  `abstain`, what it weighs now, the `epoch` it was cast in, and the last epoch it counts in
  (`counts_until`);
- `version`, the chain's count of decisions of every kind, and `versions`, each kind's own: what
  a vote on a change of that kind names;
- `members`, a council's accounts, or `null` under the electorate, whose voters are finality's;
- `in_force`, every setting governance may change as it stands at the tip, by kind: `consensus`,
  `name`, `bootnodes`, `names`, `collections`, `token_functions`, `federation`, `directory`, `fees`,
  `reward`, `limits`, `finality`, `upgrade`, `governance`, `evm_spec` and `dex`, a kind left out
  when the chain has none of it.
  Each has its `value`, as a change writes it, and `decided`, the version of the change that set
  it last, or `null` while genesis's stands.

404 on a chain without governance.

### `GET /governance/history?before=V&limit=N`

Governance's decisions, newest first: those below version `before`, at most `limit` (20 by
default, at most 100). Each has the `version` it made, the `tx` that decided it, its `block`,
`height`, `timestamp`, `confirmations` and whether it is `final`, its `sender`, and the change, as
its `kind` and in `change`.

Beside them, `version`, the decisions so far, and `from`, the oldest this node can list: 1, unless
it synced from a snapshot, which lacks the decisions before it.

**A light client checks each** with [`GET /proofs/receipts/{tx}`](#get-proofsreceiptsid): the
receipt's outcome is `decided` with that version, and the transaction votes on the change listed,
or a multisig votes for it. Each decision also has `of`, its kind, and `kind_version`, the version
it made of that kind. Versions count up one a decision, so one left out shows as a gap; the
newest is `change:decided` in state, which a [state proof](#get-proofscolumnkey) shows.

### `GET /governance/rejections?before=R&limit=N`

Governance's rejections, newest first, as the history lists decisions: each has the `rejection`
count it made, the `tx` that rejected it, its block, `height`, `timestamp`, `confirmations`,
whether it is `final`, its `sender`, `of` and `kind_version`, the kind and the version it was
rejected at, and the change. Beside them, `rejection`, the count so far, and `from`, the oldest
this node lists. A light client checks each by its receipt, whose outcome is `rejected` with that
count; the count is `change:rejected` in state.

### `GET /governance/weight/{address}`

What an account's vote weighs in the tip's epoch (`weight`, `0` for one that may not vote), what a
change `needed`, what every voter weighs (`total`), and `voted`, its votes that still count, each
with its `kind`, that kind's `version`, its `vote`, `epoch`, `counts_until` and the `change`. What
the apps ask before offering to vote or clear a vote.

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
  is its IP, or, when the connection comes from a reverse proxy the node trusts, the last address
  `X-Forwarded-For` names. Loopback is always trusted, which covers a proxy on the same host such as
  Docker's nginx; any other address is trusted only when `--trusted-proxy` named it, so a second
  container or box on a shared private network cannot spoof a client's address past the limit. `GET
  /stream` is not counted.
- **A client may be held to so many requests a second** with `--http-rate`, none by default: a
  bucket per client holds a second's worth, so a page asking for several things at once goes
  through, and past it the answer is 429 with `Retry-After: 1`. Behind Docker's nginx, which limits
  each address itself, it is a second line for a node reached around nginx.
- **Every listing's page** is at most its own maximum, as each route says, unless `--api-max-page`
  sets one for them all, 10 to 10,000; its default is never above it. The apps follow `next` or
  `before` past a short page.
- **Bodies** are at most `body_cap`: 2 MiB, room for the largest deploy hex-encoded, or room for
  the chain's `max_tx_bytes` hex-encoded when that is more, as governance's limits stand when the
  request comes; past it, 413.
- **A JSON-RPC batch at `POST /rpc` holds at most 100 requests** (`--tune rpc.batch`). Each is
  answered in order on the one thread, and one may run a contract under the whole fuel cap, so a
  larger batch is refused with code -32600 rather than letting one body line up far more work than
  the rate limit allows. `eth_getLogs` scans at most 10,000 blocks (`rpc.log-blocks`), with at most
  1,000 addresses and 1,000 topics at a place (`rpc.log-addresses`, `rpc.log-topics`), and
  `eth_feeHistory` gives at most 1,024 blocks (`rpc.fee-history`), as geth's `--rpc.*` flags bound
  theirs.
- **`eth_gasPrice` is twice the next base price,** and an open offer's max price twice it plus the
  tip, unless `--suggest-multiplier` says how many times.
- **Mining WebSocket relays are capped per client,** 32 open at once from one address
  (`--tune http.relays-per-ip`). Each opens a
  connection into the mining port, which sees them all as loopback, so they are counted here by who
  asked; past the cap, 503.
- **`/node`, `/mempool` and `/stream` ask the node.** The rest read the database; these go to
  the node loop, which answers between blocks.
- **Reads are not snapshot-consistent.** A request that reads several keys can see two tips.
  Proofs are the exception: each is checked against its root before it is sent.
- **Past state reaches back `--state-history` blocks,** 10,000 by default, or every height on an
  archive node: reads, calls and proofs at an older block answer 404. `GET /blocks/range` gives the
  block and says the state is pruned.
- **A snapshot node's history is headers only** before its pivot: `GET /blocks` and
  `GET /blocks/{block}` show those blocks marked `"pruned": true`, without transactions.
