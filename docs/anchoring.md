# Anchoring

Finality protects a node that already follows the chain: it never rolls back past its finalized
checkpoint. It does not protect a node that is new, or one that was offline for long, against a
**long-range fork**. That is a history built from an old checkpoint by validator keys that have
since left, been sold or been stolen. It can carry as many votes as the real one, and a new node
that meets it first has nothing to tell the two apart. `--checkpoint` fixes that by having the
operator trust a block id someone gave them.

Anchoring fixes it without trusting anyone, by borrowing Bitcoin's proof of work and Ethereum's
finality:
- Anyone writes the chain's finalized checkpoints into Bitcoin transactions, or Ethereum ones.
- The chain records each one once it is shown deep enough in its Bitcoin relay, or in a block its
  Ethereum relay holds as finalized. One list holds them all, whichever chain holds each.
- A new node picks, among the histories its peers show, the one anchored first.

A fork made later from old keys cannot have put its checkpoints into Bitcoin's past. Doing that
would mean outworking Bitcoin. Nor into Ethereum's finalized past, short of Ethereum's own sync
committees signing a fork of Ethereum.

Governance turns each part on and off, and a chain starts with all of them off. Builds choose too
(see [Development](development.md)):
- **`anchoring`** is what every anchor shares: the list and its root, `Tx::Anchor`'s dispatch and
  reward, the guard against branches leaving an enforced anchor, `GET /anchors`, serving the list
  to peers, and `--anchored`.
- **`anchoring-btc`** adds Bitcoin: its proofs, read against the relay, Bitcoin's headers for
  `--anchored`, and the anchorer's Bitcoin leg.
- **`anchoring-eth`** adds Ethereum, without Bitcoin, on `ethlight` and `bls12-381`: its proofs,
  read against the Ethereum relay, its sync committees followed for `--anchored` through
  `--beacon`, and the anchorer's Ethereum leg. `ethlight` itself is in every build, as anchoring's
  settings name a beacon network every node must read alike.

All three are on by default, and each of the chains builds without the other. A node built without
`anchoring` takes no anchors, and one without `anchoring-btc` no Bitcoin anchors, so neither can
follow a chain that has them on.

## What goes into Bitcoin and Ethereum

In Bitcoin, an anchor is one output of a transaction: `OP_RETURN OP_PUSHDATA1 80`, then 80 bytes,
the most Bitcoin's nodes relay by default (`anchor::Payload`). In Ethereum, it is the whole
calldata of a transaction, a 0-value one from the anchorer's key to itself: the same 80 bytes,
nothing before or after. The chain checks the calldata, not who sent it or to whom.

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

**`Tx::Anchor { proof }`** shows another chain holds an anchor. The proof is an `AnchorProof`, by
the chain it is in: `Bitcoin` (tag 0) or `Ethereum` (tag 1).

**A Bitcoin proof** (`BtcAnchorProof`) is:
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
`contracts/examples/btc_relay` lays them out: each header's record decodes as `btclight`'s `Kept`,
the code the relay writes it with. Nothing it runs can change what a proof shows.

**An Ethereum proof** (`EthAnchorProof`) is:
- the light client header of the block holding the transaction, as SSZ;
- the beacon root of the finalized header the relay holds that it rests on;
- the light client update that finalized that header, as SSZ, signed by its sync committee: a
  finality update, read as one bringing no next committee;
- the transaction, its place, and its SSZ branch to the payload's transactions root, 21 nodes;
- for a block that is not the finalized header itself but an ancestor of it, the branch from its
  root through the finalized header's state's `block_roots`.

It is recorded when all of these hold (`anchor::ethereum::check`), by `ethlight`'s rules, the code
the relay runs:
- **The settings take it:** `ethereum` is on, and `eth_relay` and `eth_start` are set.
- **The relay is the one the settings say:** it started at `eth_start`'s root, on its network.
- **The relay holds the header the proof rests on,** and the update finalizes exactly it: the same
  root, the same record.
- **The update is signed:** by two thirds of the sync committee the relay holds for its signature's
  period, its finality branch into the attested header's state, its slots in order and none after
  the block's time. A node syncing on its own checks the same signature (below).
- **The anchor's block is that header, or an ancestor of it** by `ethlight::client::prove_ancestor`,
  within the 8,192 slots before it.
- **The transaction is in that block's payload,** at its place (`ethlight::tx::verify_transaction`),
  and its calldata is the 80 bytes.
- **The anchor is this chain's,** of a checkpoint on this branch, with the list's root now, and the
  epoch has none yet, as for Bitcoin.

The entry records `AnchorSource::Ethereum` with the anchor's block's beacon root and slot, and the
reward is paid as for Bitcoin. The relay is read natively, from its records in `Column::Namespaces`,
as `contracts/examples/eth_relay` lays them out (`anchor::EthRelay`).

**What the chain keeps** (`anchor::Anchors`, chain state in `Column::Validators`, beside
governance's):
- **Each entry,** in the order they were recorded: the epoch, the block, where the anchor sits
  (`AnchorSource`: a Bitcoin block and its height, tag 0, or an Ethereum beacon root and its slot,
  tag 1), and whether `enforce` was on when it was recorded.
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
2. **It follows Ethereum's sync committees** from the `eth_start` a peer names, through the beacon
   APIs given by `--beacon` (`anchor::ethereum::EthCommittees`): the bootstrap at its root, then
   every period's best update to now, each checked as the relay checks them. Nothing a beacon API
   serves is trusted: what it gets wrong is passed over, and proves nothing. Without `--beacon`,
   Ethereum anchors decide nothing: each list is checked up to its newest anchor in a chain the node
   can check, and Bitcoin alone compares.
3. **It asks each peer's relay for Bitcoin's headers** (`SyncRequest::BtcHeaders`), from the
   start. The relay keeps each header's version and nonce, so any node can write a header whole
   again. The node checks them as Bitcoin's nodes do (`anchor::bitcoin::HeaderChain`), by the
   rules the relay itself runs, from the `btclight` crate: proof of work, the retarget on main, the
   median time past. The most work any peer showed is Bitcoin's
   main chain. A fork's relay can claim nothing of Bitcoin that Bitcoin's work does not back.
4. **It checks each list against those chains** (`anchor::sync::check_anchors`). A Bitcoin proof
   must lead to a main-chain block at least `anchors.depth` deep. An Ethereum proof's update must
   be signed by the committee it followed for that period, and its block and transaction must be
   where the entry says. The root the newest payload carries must be the root of every entry before
   it, so the whole list is that chain's own.
5. **Where two lists part, the one anchored first after that place wins** (`compare_histories`).
   Bitcoin decides where both histories have Bitcoin anchors after they part; Ethereum fills the
   gaps:
   - In each list, it takes the first Bitcoin entry at or after the place they part. If both have
     one, those decide: the lower in Bitcoin wins.
   - Else it does the same with Ethereum entries: the earlier slot wins.
   - Else no chain holds an anchor of both after they part, and nothing decides.

   Both entries deciding must be proven. They may sit at different places in the two lists, so the
   node asks each peer for its proof of its own (`NeedProofs`). Two lists of which one starts the
   other are the same history, and the longer wins. Two entries in the same block, or slot, are two
   histories anchored at once. No rule picks between them, nor where nothing decides, so the node
   says so and waits, and you start it with `--checkpoint` from someone you trust. A build that
   reads only one of the chains goes by that one alone.
6. **It starts at the winner's newest anchored checkpoint,** as `--checkpoint` would.

It hears from `anchors.peers` peers first, 2 unless tuned. One honest peer among them is enough. A
node that hears only from an attacker's peers is synced by what they show, as with any sync.

**Only Bitcoin itself makes an anchor worth more than votes.** On `test` (testnet4, signet) and
`regtest`, headers cost next to nothing to make, so a fork could make its own Bitcoin. Anchoring on
those is for trying it out.

## The anchorer

`chainv4 bridge anchor`, or `bridge btc anchor` as it was first named, runs a pass every
`--every` seconds, 60 unless given, or once with 0. It writes each anchor into each chain
anchoring takes, and shows it to the chain once that chain holds it, which earns the reward from
`--key`:

```sh
chainv4 bridge anchor --key payer.key \
  --esplora https://mempool.space/api --btc-key anchorer.key \
  --eth-rpc https://ethereum-sepolia-rpc.publicnode.com --evm-key anchorer.key \
  --beacon https://lodestar-sepolia.chainsafe.io
```

1. **When:** once the chain's newest finalized checkpoint is `every` epochs past that chain's own
   newest anchor, and past every anchor's epoch.
2. **Turns:** one list holds both chains' anchors, and each anchor carries the list's root, so one
   recorded in either chain wastes an anchor written in the other meanwhile. Neither is written
   while the other has one pending, and Bitcoin goes first when both are due.
3. **Into Bitcoin:** an OP_RETURN paid from `--btc-key`'s native segwit coins, `--evm-key`'s unless
   given, with change back to the key, at the rate Bitcoin's source asks for within a few blocks,
   or `--fee-rate`. Shown once the relay holds its block `depth` deep.
4. **Into Ethereum:** a 0-value transaction from `--evm-key` to itself, the anchor as its calldata,
   through `--eth-rpc`. It is sent 8 seconds before an epoch's first slot, to land in that slot's
   block, which is then the epoch's checkpoint. Once a finality update from `--beacon` finalizes that
   very block, the anchorer submits the update to the relay should the relay not hold the block,
   reads the block's transactions (`eth_getRawTransactionByBlockNumberAndIndex`, in batches) to
   prove its own, and sends `Tx::Anchor`. A transaction that landed in another slot, or whose
   finality update a pass missed, is written again: proving it would take an ancestry branch through
   a finalized state's `block_roots` and the block's own light client header, and no beacon API's
   light client routes serve either.
5. **If another anchor was recorded in between,** the pending one is past use, and a new one is
   written.

The relays must be kept synced (`bridge btc relay-sync`, `bridge eth relay-sync`) for anchors to
count. Pending anchors live in the anchorer's memory: one stopped before showing its anchor writes
another when it starts again.

`chainv4 send anchor --proof 0x…` sends a proof by hand. The proof is `AnchorProof` encoded, as
hex: its tag, 0 for Bitcoin or 1 for Ethereum, then the proof.

## The Ethereum relay

`contracts/examples/eth_relay` (see [Contracts](contracts.md#ethereums-light-client)) is fed from a
beacon API serving the light client routes, ChainSafe's Lodestar on Sepolia:

```sh
chainv4 send deploy --wasm eth_relay.wasm
chainv4 bridge eth relay-start 0x<relay> 0x<a finalized checkpoint's root> --network sepolia \
  --beacon https://lodestar-sepolia.chainsafe.io
chainv4 bridge eth relay-sync 0x<relay> --every 384 --beacon https://lodestar-sepolia.chainsafe.io
```

- **relay-start** fetches the bootstrap at the root (`/eth/v1/beacon/light_client/bootstrap/{root}`)
  and starts the relay with it. Without `--network`, the relay takes the network anchoring names.
- **relay-sync** submits each period's best update from the relay's own to now
  (`/eth/v1/beacon/light_client/updates?start_period&count`), then the newest finality update
  (`/finality_update`), each only when a dry run says the relay takes it, a transaction each. With
  `--every`, it does so for ever: the `chainv4-eth-relay` service ([Deploying](deploy.md)).
- **The network follows governance:** `anchoring.eth_start.network` reaches the relay through the
  `eth_network` host call, so a vote adds a fork to the schedule without a redeploy.

## Settings

Genesis's `anchoring`, or a `Change::Anchoring` decided by governance
(`params::anchoring::AnchorSettings`). Unset, everything is off.

| Field | Default | What |
| --- | --- | --- |
| `bitcoin` | false | Whether the chain takes Bitcoin anchors. |
| `ethereum` | false | Whether the chain takes Ethereum anchors. |
| `enforce` | false | Whether anchors recorded from now on bind nodes. |
| `relay` | none | The relay contract proofs are checked against. |
| `btc_start` | none | `{ "header": "0x…80 bytes", "height": N, "network": "main" \| "test" \| "regtest" }`, where the relay started. On main, a height divisible by 2016. |
| `eth_relay` | none | The Ethereum relay contract Ethereum proofs are checked against. |
| `eth_start` | none | `{ "root": "0x…32 bytes", "network": … }`, where the Ethereum relay started: the trusted beacon root, a finalized checkpoint's, and its network. |
| `depth` | 6 | Bitcoin blocks deep before an anchor counts, 1 to 144. |
| `every` | 90 | Epochs between rewarded anchors, at least 1. |
| `reward` | "0" | What the first proof of a rewarded anchor earns. |

**`eth_start.network`** is one of:
- a preset's name: `main` or `mainnet`, `sepolia`, `hoodi`, as this build defines them;
- a whole definition, written out: `{ "name", "genesis_validators_root", "genesis_time",
  "seconds_per_slot", "slots_per_epoch", "epochs_per_sync_committee_period", "forks": [{ "fork":
  "deneb", "version": "0x…4 bytes", "epoch": N }, …] }`;
- the same definition as `ethlight::Network`'s SSZ, `"0x…"`, which is how the chain carries it and
  GET /anchors writes it.

A definition named as a preset overrides it, to add a fork to its schedule; one of another name
defines a network no build has, such as Gnosis, Chiado or a private beacon chain. It must pass
`Network::problem`: a name of 1 to 64 bytes, Phase 0 first at epoch 0, forks in order each with a
version of its own.

`--change '{"anchoring":{"bitcoin":true,"enforce":true}}'` names only what it changes. It is laid
over what stands, as with every kind that takes a part. `GET /anchors` shows the settings in
force, the list's count, root and enforced height, the Bitcoin relay's tip, the Ethereum relay's
finalized header (`eth_finalized`: its slot, root, and execution block's number and hash), and a
page of entries (`?from`,
`?limit`). Each entry names its `chain`, `bitcoin` or `ethereum`, and where that chain holds it:
`btc_block` and `btc_height`, or `eth_root` and `eth_slot`. A `Tx::Anchor`'s JSON names its
`chain` too. The explorer's Finality page shows them, and the wallet proposes them.

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
- **Ethereum anchors are proven, not vouched for.** The relay checks every update's signature as
  Ethereum's light clients do, and the chain checks each proof against what the relay holds. The
  finality update rides with the proof so a syncing node checks the signature itself, against
  committees it followed from `eth_start`, rather than trusting the chain's own relay, which a fork
  could fill with anything.
- **The network is a vote.** Ethereum's forks come every year or so; a fork the schedule lacks stops
  the relay. A whole definition in the settings lets governance add it without a new build or a
  redeploy, and name networks no build has.
- **Bitcoin decides first.** Its proof of work costs a fork the same however old the keys it
  holds; Ethereum's finality rests on validators, whose old keys could in time sign a fork of
  their own. So Ethereum only decides where Bitcoin cannot.
- **The header rules are written once.** `btclight` holds them, `no_std`, and both the relay
  contract and the node's anchored sync run it, so the two can never disagree on what a valid
  Bitcoin header is.
