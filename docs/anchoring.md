# Anchoring

Finality protects a node that already follows the chain: it never rolls back past its finalized
checkpoint. It does not protect a node that is new, or one that was offline for long, against a
**long-range fork**. That is a history built from an old checkpoint by validator keys that have
since left, been sold or been stolen. It can carry as many votes as the real one, and a new node
that meets it first has nothing to tell the two apart. `--checkpoint` fixes that by having the
operator trust a block id someone gave them.

Anchoring fixes it without trusting anyone, by borrowing Bitcoin's proof of work:
- Anyone writes the chain's finalized checkpoints into Bitcoin transactions.
- The chain records each one once it is shown deep enough in its Bitcoin relay.
- A new node picks, among the histories its peers show, the one Bitcoin saw first.

A fork made later from old keys cannot have put its checkpoints into Bitcoin's past. Doing that
would mean outworking Bitcoin.

Governance turns each part on and off, and a chain starts with all of them off.

## What goes into Bitcoin

An anchor is one output of a Bitcoin transaction: `OP_RETURN OP_PUSHDATA1 80`, then 80 bytes, the
most Bitcoin's nodes relay by default (`anchor::Payload`).

| Bytes | What |
| --- | --- |
| 4 | `cv4a` |
| 4 | the first 4 bytes of the chain's genesis id |
| 8 | the checkpoint's epoch, big-endian |
| 32 | the checkpoint's block id |
| 32 | the root of the chain's anchor list before this anchor, all zeros while it is empty |

**Why the list's root is in there.** A new node checks the list a peer shows against the root in
Bitcoin. Without the root, a peer could copy the real chain's list and claim its fork parted
later than it did. See [Picking a history](#picking-a-history).

## On chain

**`Tx::Anchor { proof }`** shows Bitcoin holds an anchor. The proof is:
- the Bitcoin transaction, without its witnesses;
- its block, keyed as the relay keys it (the header's SHA-256 twice, unreversed);
- its Merkle path;
- the block's coinbase, with its own path.

Anyone may send it, whoever wrote the Bitcoin transaction. It is recorded when all of these hold
(`anchor::prove`):
- **The settings take it:** `bitcoin` is on, and `relay` and `btc_start` are set.
- **The relay is the one the settings say:** it started at `btc_start`, on its network.
- **The anchor is this chain's:** the chain tag is this chain's, and the block at the epoch's
  checkpoint height on this branch is the one named. A checkpoint that is not final yet may be
  anchored. If the checkpoint ever leaves the branch, the block recording its anchor leaves with
  it.
- **Its root is the list's root now:** so the list stays whole. An anchor written while another
  was recorded in between is written again.
- **The relay holds the block deep enough:** at least `depth` deep, and on its main chain.
- **The proof leads to the block's Merkle root:** both paths do, with the coinbase showing how
  tall the tree is, as the relay's own `includes` checks (CVE-2017-12842).
- **The epoch has no anchor yet.**

The relay is read natively, from its records in `Column::Namespaces`, as
`contracts/examples/btc_relay` lays them out. Nothing it runs can change what a proof shows.

**What the chain keeps** (`anchor::Anchors`, chain state in `Column::Validators`, beside
governance's):
- **Each entry,** in the order they were recorded: the epoch, the block, the Bitcoin block and its
  height, and whether `enforce` was on when it was recorded.
- **The running root after each entry:** SHA-256 of the root before it and the entry's encoding.
- **The proof that showed it,** so any node can show it to a peer.
- **The highest checkpoint an enforced entry holds.**

**The reward.** The first proof of an anchor whose epoch is at least `every` epochs past the last
rewarded one earns `reward`, credited to the sender. It is minted, or on a chain whose vote
rewards come from fees it is paid from the reward pool, at most what the pool holds. Anchors in
between are recorded and enforced, unpaid. This keeps the anchorer that writes every `every`
epochs paid, and anyone anchoring more often pays their own Bitcoin fees.

## Enforcement

**Running nodes.** No block at or under the highest enforced anchored checkpoint is imported
(`BlockError::ConflictsWithAnchor`). No branch that splits off below it is followed, however much
work it holds or whatever it justified. On a node that follows the real chain this adds little
beyond finality: it binds a checkpoint once anchored, final or not.

**Anchors recorded while `enforce` was on stay enforced.** A vote that turns `enforce` off stops new
anchors from binding. It never undoes what anchors already hold.

## Picking a history

`chainv4 --snapshot --anchored` starts a node that holds only genesis at the checkpoint the anchors
pick, rather than at one it is told to trust (`node::snapshot::anchored`):
1. **It asks each peer for its anchor list** (`SyncRequest::Anchors`). The answer holds every
   entry, the newest one's proof, and where the peer's relay starts and how far it reaches.
2. **It asks each peer's relay for Bitcoin's headers** (`SyncRequest::BtcHeaders`), from the
   start. The relay keeps each header's version and nonce, so any node can write a header whole
   again. The node checks them as Bitcoin's nodes do (`anchor::bitcoin::HeaderChain`): proof of
   work, the retarget on main, the median time past. The most work any peer showed is Bitcoin's
   main chain. A fork's relay can claim nothing of Bitcoin that Bitcoin's work does not back.
3. **It checks each list against those headers** (`anchor::sync::check_anchors`). The newest
   entry's proof must lead to a main-chain block at least `anchors.depth` deep. The root its
   payload carries must be the root of every entry before it, so the whole list is that chain's
   own.
4. **Where two lists part, the one whose entry there sits lower in Bitcoin wins**
   (`compare_histories`). Both entries at that place must be proven, and the node asks each peer
   for its proof of that place. Two lists of which one starts the other are the same history, and
   the longer wins. Two entries in the same Bitcoin block are two histories anchored at once. No
   rule picks between them, so the node says so and waits, and you start it with `--checkpoint`
   from someone you trust.
5. **It starts at the winner's newest anchored checkpoint,** as `--checkpoint` would.

It hears from `anchors.peers` peers first, 2 unless tuned. One honest peer among them is enough. A
node that hears only from an attacker's peers is synced by what they show, as with any sync.

**Only Bitcoin itself makes an anchor worth more than votes.** On `test` (testnet4, signet) and
`regtest`, headers cost next to nothing to make, so a fork could make its own Bitcoin. Anchoring on
those is for trying it out.

## The anchorer

`chainv4 bridge btc anchor --esplora URL --btc-key key.json` runs a pass every `--every` seconds,
60 unless given, or once with 0:
1. Once the chain's newest finalized checkpoint is `every` epochs past the newest anchor, it writes
   the checkpoint into an OP_RETURN, paid from the key's native segwit coins, with change back to
   the key. It pays the rate Esplora asks for within a few blocks, or `--fee-rate`.
2. Once Esplora has the transaction in a block, and the relay holds that block `depth` deep, it
   sends `Tx::Anchor` with the proof, from `--key`, which earns the reward.
3. If another anchor was recorded in between, it writes a new one.

With `ethereum` on, `--eth-rpc` and `--evm-key`, it writes the same 80 bytes to that EVM chain
too, as a transaction from the key to itself. The chain does not count those yet: that waits for
a light client of Ethereum's finality (see [Todo](todo.md)).

The relay must be kept synced (`bridge btc relay-sync`) for anchors to get deep enough. A pending
anchor lives in the anchorer's memory: one stopped before showing its anchor writes another when
it starts again.

`chainv4 send anchor --proof 0x…` sends a proof by hand. The proof is `AnchorProof` encoded, as
hex.

## Settings

Genesis's `anchoring`, or a `Change::Anchoring` decided by governance
(`params::anchoring::AnchorSettings`). Unset, everything is off.

| Field | Default | What |
| --- | --- | --- |
| `bitcoin` | false | Whether the chain takes Bitcoin anchors. |
| `ethereum` | false | Whether anchorers write to Ethereum too. Not counted yet. |
| `enforce` | false | Whether anchors recorded from now on bind nodes. |
| `relay` | none | The relay contract proofs are checked against. |
| `btc_start` | none | `{ "header": "0x…80 bytes", "height": N, "network": "main" \| "test" \| "regtest" }`, where the relay started. On main, a height divisible by 2016. |
| `depth` | 6 | Bitcoin blocks deep before an anchor counts, 1 to 144. |
| `every` | 90 | Epochs between rewarded anchors, at least 1. |
| `reward` | "0" | What the first proof of a rewarded anchor earns. |

`--change '{"anchoring":{"bitcoin":true,"enforce":true}}'` names only what it changes. It is laid
over what stands, as with every kind that takes a part. `GET /anchors` shows the settings in
force, the list's count, root and enforced height, the relay's tip, and a page of entries (`?from`,
`?limit`). The explorer's Finality page shows them, and the wallet proposes them.

## Why it is built this way

- **Anchors are proven on chain, through the relay, rather than vouched for.** Nothing but Bitcoin's
  headers and a Merkle path stands behind a recorded anchor, as for a Bitcoin deposit.
- **No node runs Bitcoin.** Anchored sync checks Bitcoin's headers by their work, from what peers'
  relays hold. Bitcoin's headers are cheap to check and costly to make, so it needs no source it
  trusts.
- **The start lives in the settings, not in genesis,** so a chain can turn anchoring on later. A
  fork cannot move it without a vote from before the fork.
- **No Bitcoin height in block headers.** The relay's tip is chain state already, which every node
  agrees on, so a header field would add nothing a proof against the state root cannot show. On its
  own it would prove only that a block was made after some Bitcoin block.
- **Ethereum waits for a light client.** Without one, an Ethereum anchor counts only as far as
  whoever vouches for it, which a long-range attacker holding old signer keys could do as well.
