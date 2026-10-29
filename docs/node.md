# Node and network

## The node loop

`Node` (`src/node/`) owns all node state:
- the chain and the mempool;
- the producer, if it produces;
- the validator key, if it votes;
- the link to the network.

**The verifier:** events reach the loop through a thread of their own, `verify`
(`src/node/verifier.rs`). It takes them from the network and from local clients, up to 256 at a
time, and checks every transaction's signature in the batch side by side on the chain's workers.

**The gate** (`src/node/gate.rs`) then admits the batch's transactions on that same thread:
- it checks each against the tip's state with no lock held, then locks the mempool only to
  insert it;
- it answers the client that submitted it, gossips it, and tells subscribers.

Everything else goes on to the loop in the order it came, and the loop hears only a tally of
what came of each batch's transactions, for its counters. So a loop busy building or importing a
block never keeps clients waiting, and the mempool keeps filling while it builds: the loop locks
the mempool only to pick a block's transactions and to drop what landed. `NetLink::pair` links, in
tests, check and admit on the loop's own thread instead, so a test's events are there the moment
it sends them.

Its loop, `Node::run`, repeats two steps. **`step`** does the work:
0. It stops, with `NodeError::Upgrade`, once the next block is one an [upgrade](updates.md#upgrades)
   stops this version at, and says so every ten minutes until then. Blocks at or past that
   height are never imported.
1. It handles what arrived, at most 256 events a pass so a flood cannot starve block production:
   - gossiped blocks and votes;
   - the gate's tallies of what came of transactions;
   - peers' requests and answers;
   - peers connecting and leaving;
   - a miner's seal;
   - the API asking for a report.
2. It produces a block if its slot has come. An authority seals at once; a miner gets a template.
3. It votes, if it validates and a checkpoint is deep enough.
4. It catches each index up a step, on a node started with `--index-events` or
   `--index-addresses` (see [Storage](storage.md#the-indexes)).
5. It logs where it stands every 30 seconds.

**`wait`** then sleeps until the next slot, the next event, or a second at most. While an index
is still catching up, it does not sleep, so a long catch-up runs a step at a time in
between everything else.

Tests drive `step` with a clock of their own, so they never sleep.

| Constant | Value | What it bounds |
| --- | --- | --- |
| `EVENT_BUDGET` | 256 | Events handled per pass. |
| `UNBOUNDED_CHAIN_TXS` | 1,000 | Transactions in a block on a chain bounded by neither bytes nor gas. |
| `MEMPOOL_CAPACITY` | 50,000 | Transactions waiting at once. |
| `TEMPLATE_REFRESH` | 2 s | How long a miner works on one template before it is rebuilt. |

**After an import:**
- the node brings the mempool in line: transactions the main chain now holds leave it, and those
  a reorg took off come back;
- it retries the finality votes it held back.

**A block it produced** is imported first, then gossiped.

## Other threads' way in

A `NodeHandle` (`Node::handle`) is how another thread, such as the HTTP API's, reaches the
running node. It is cheap to clone, and does three things.

**Submit:** it hands the node a transaction and waits for the mempool's verdict.

**Report:** it asks how the node is doing. The loop answers between events with a `NodeReport`,
so it is always current. A report holds:
- its peers: each one's address, when it connected, what it gossiped, and its chain from its
  last status;
- where it listens, and whether it produces or validates;
- the sync it has under way;
- the mempool's count and its first transactions;
- what it counted since it started (`Counters`): blocks heard, imported, stored on side
  branches, rejected and produced; reorgs; transactions heard, submitted and refused; votes
  heard and cast; sync requests served, sent and failed; connections.

**Subscribe:** it follows what the node does as it happens (`NodeEvent`), when the node streams.
`Node::stream` turns streaming on: `--stream` on the command line, in a build with the `stream`
feature. Then the node broadcasts:
- every block it stores, on the main chain or a side branch;
- every reorg, justified and finalized checkpoint and transaction it queues;
- every peer that comes or goes.

Events are only made while someone listens. A subscriber that falls more than 1,024 events
behind misses some and is told so. The API serves this as `GET /stream` (see
[HTTP API](api.md#get-stream)).

## The mempool

**How it is kept:** the mempool (`src/mempool/`) holds transactions by sender and nonce, so each
sender's go in order.

**Admission** checks:
- the signature, which the verifier checked already for anything gossiped or submitted
  (`Mempool::add_checked`), after the [backlog](#the-gossip-backlog) had room for it;
- that the nonce is not used yet;
- that the same sender and nonce are not waiting already;
- on a chain with fees, that the gas limit could fit a block, the price could be taken and the
  sender can pay (see [Fees](fees.md)).

**Selection** for a block takes, for each sender, the waiting transactions in nonce order from
its current nonce, up to the first gap, or the first one the next block cannot take because its
offer is under the base price (`Chain::worth`). Across senders it takes whichever sender's next
transaction pays the producer most, ties going to the lower address. The block builder then runs
them and leaves out any that cannot run.

**How many** a block takes is a budget, not a count (`Budget`):
- the bytes genesis's `max_block_bytes` allows, which every node checks;
- gas limits adding up to twice the block gas limit: running then fills the block however little
  of their limits the transactions use, without running far more than fits;
- `--max-block-txs N`, when this node caps its own blocks.

A transaction past the budget is left out with the rest of its sender's, and the others still
fill the block. A chain that bounds blocks by neither bytes nor gas takes 1,000 a block.

**How long** building may take is `--max-build-ms N`, when this node bounds it. The block builder
runs the selected transactions in order, and once the time is up, those that went in by then are
the block: the rest are left out unrun and wait for the next block. It is a brake for a slow
machine, so it seals something before its slot is gone, rather than a limit every node checks:
the bytes and gas limits still apply. Without it, blocks fill to the chain's limits whatever
they take to run.

**Pruning** after each block drops:
- transactions whose nonce is used;
- on a chain with fees, those whose sender can no longer cover their reserve, as its funds went
  since they were admitted;
- those that have offered less than the base price for `MAX_UNDERPRICED_BLOCKS` (32) blocks in a
  row. An offer under the base price is kept until then, since the base price moves a step a
  block and may come back down; a block it can go in resets the count.

## Networking

**The thread:** libp2p runs on a thread of its own (`src/net/`), behind two channels:
- `NetEvent`s come in;
- `NetCommand`s go out.

The node never sees libp2p itself.

| Piece | What it does |
| --- | --- |
| QUIC | The transport. `--listen` picks addresses, `/ip4/0.0.0.0/udp/0/quic-v1` by default; port 0 picks one, and the log shows which. |
| TCP, Noise, Yamux | For addresses `--listen` gives as TCP, `/ip4/0.0.0.0/tcp/4001`, and for peers over Tor (see [Over Tor](#over-tor)). |
| gossipsub | Blocks, transactions and votes, on topics `/{protocol}/{network}/blocks`, `/txs-batch` and `/votes`. |
| Kademlia | Finding peers, as `/{protocol}/{network}/kad/1`. |
| mDNS | Finding peers on the local network; `--no-mdns` turns it off, as on a server. |
| identify | Learning peers' addresses, and which chain they are on, as `/{protocol}/{network}/1`. |
| request-response | Sync, as `/{protocol}/{network}/sync/1`. |

**Names:** `{network}` is the first eight bytes of the genesis id in hex, and `{protocol}` is
`chainv4` unless `--protocol` names another: 1 to 32 lowercase letters, digits, dashes, dots and
underscores. With both in every name, nodes of two chains share no topic, no DHT and no sync, even
when they find each other.
- **A peer of another chain is disconnected.** Once identify says a peer runs another protocol
  name or another genesis, the node drops it and forgets it in Kademlia, so mDNS on a shared
  network, or a `--peer` pointed at the wrong node, pulls in nothing. The log says which it ran.
- **Give a chain of your own a name of its own** when it is built from this code: its genesis
  already keeps it apart, and the name keeps it apart from any other chainv4 chain that happens to
  share the genesis, such as two development chains. Workers (`chainv4 worker --protocol`) and
  every node of the chain give the same.
- **Changing it is a split.** Nodes under the old name and the new one no longer talk; change it
  on every node at once.

**Message ids:** a gossip message's id is the chain hash of its bytes, so the same block or
transaction is one message however many peers send it.

**Transactions go in batches:** a node gathers what it gossips for up to `TX_BATCH_WAIT`, 50 ms,
or `TX_BATCH`, 256 transactions, and sends them as one message on `/txs-batch`, so a busy network
sends a message every 50 ms rather than one a transaction, which overflowed libp2p's queues past a
few thousand a second. A batch is never passed on whole: each transaction in it is counted into
the backlog, checked, admitted and judged on its own, a bad signature counting against the peer,
and those the node takes go out again in its own next batch. Each node remembers, for two minutes,
the transactions it sent and heard of, by id: one is sent once, and one a peer passes back is
dropped before it costs a slot of the backlog. A node from before batches gossips on `/txs`,
which no node reads now: every node updates together.

**Checked before it is passed on:** gossipsub runs with `validate_messages`, so a block,
transaction or vote goes on to this node's peers only once the node has checked it and said so,
with `NetCommand::Judge` and its `Verdict`:

| What came | Verdict |
| --- | --- |
| A block that ran, a transaction the mempool took, a vote counted | **Accept**: passed on. |
| A block this node has, or whose parent it lacks, from the future, past finality, or it cannot check yet; a transaction it cannot take now (nonce, fee, full, syncing); a vote stale or not a validator's; anything while syncing from a snapshot | **Ignore**: dropped, no one's fault. |
| A block that breaks the rules (work, seal, transactions, roots); a transaction or vote whose signature does not hold; bytes that do not decode; a flood past the backlog | **Reject**: dropped, and counted against the peer that sent it. |

So a flood, or an invalid block, stops at the first honest node rather than passing through it.

**The node's identity** is its network key, kept in `net.key` beside the database's directory
(`--net-key FILE` for another), made at the first start, readable by its owner alone: its peer id
is the same each start, so a peer can be named by it in `--peer` as `/p2p/<id>`, and is checked
when it is. A file that does not read stops the node rather than giving it a new id; move it aside
for a new one. It signs gossip and connections, nothing on the chain.

**A peer that comes back over a dead connection starts afresh.** gossipsub tells a peer the
topics this node takes on their first connection alone. A peer that restarts, or moves behind its
NAT, dials again before this node has timed its old connection out, so the new one is a second,
and the peer never hears what this node subscribes to: nothing it publishes reaches this node,
while this node's own gossip still reaches it. So a connection that comes the way a standing one
came, both dialed by the peer or both by this node, drops the peer whole, and its next dial comes
up alone, telling both sides' topics. Two nodes dialing each other at once make one each way,
which both stand.

**Status, once a minute.** A node asks every peer how far its chain got as it connects, when its
gossip does not connect, and every `STATUS_EVERY`, 60 seconds. A peer whose gossip stopped arriving
while it built on a branch of its own is seen for what it is within a minute, and synced from when
its branch is heavier, rather than taken for where it was when its gossip stopped.

**Published to nobody:** a block, batch or vote no connected peer takes, as no peer said it
subscribes, counts as `gossip_unpublished` in `GET /node` and logs a warning at most once a
minute; without peers, it is the usual best effort.

## Over Tor

Tor carries TCP alone, and QUIC is UDP, so a node meets peers over Tor on libp2p's TCP transport,
with Noise and Yamux, through a Tor SOCKS5 proxy (`src/net/tor.rs`):

| Flag | What it does |
| --- | --- |
| `--tor-proxy socks5h://host:port` | `/onion3` peers and bootnodes are dialed through it; everything else directly, as before. |
| `--tor-only` | Every dial through the proxy and no QUIC: the node never shows its IP. It reaches onion peers, and TCP ones through Tor's exits, not QUIC ones. It must listen on TCP alone, `--listen /ip4/127.0.0.1/tcp/4001`. |
| `--onion-file PATH` | The file tor writes the node's onion into, `/onion3/NAME:PORT`, read every redial until it is there and then told to peers as the node's address. |

**Reachable as an onion:** listen on TCP too, have tor publish that port in a hidden service
(`HiddenServicePort 4001 127.0.0.1:4001`), and give `--onion-file` the address, so identify tells
peers. The Docker stack does all three with `TOR=1` (see [Docker](docker.md#an-onion-service)).

```sh
chainv4 … --listen /ip4/0.0.0.0/udp/4001/quic-v1 --listen /ip4/0.0.0.0/tcp/4001 \
  --tor-proxy socks5h://127.0.0.1:9050 --onion-file /var/lib/tor/chainv4/p2p
chainv4 … --listen /ip4/127.0.0.1/tcp/4001 --tor-proxy socks5h://127.0.0.1:9050 --tor-only --no-mdns \
  --peer /onion3/NAME:4001                       # a node behind Tor alone, joining an onion
```

- **What a Tor-only node gives away:** nothing of its IP. It still runs gossip, Kademlia and sync
  as any node, and its peers know its peer id; mDNS, which speaks on the local network, should be
  off (`--no-mdns`).
- **Kademlia tells it QUIC addresses too,** which it cannot dial; those dials fail at once and it
  moves on. A Tor-only node needs onion peers or TCP ones to stay connected.

## Bans

Faults add up per peer within `SCORE_WINDOW`, 10 minutes, by weight: an invalid block or
undecodable gossip 50, a flood 25, a bad signature on a transaction or vote 10. At `BAN_SCORE`,
100 (two invalid blocks, or ten bad signatures), the peer is banned:
- **Cut off:** disconnected, forgotten in Kademlia and in the remembered peers, and never dialed.
- **Refused:** a connection from it, whoever dialed, is closed as soon as it stands.
- **For `--ban-for`,** an hour unless given (`30m`, `2d`); bans live in memory, so a restart lifts
  them.
- **By peer id,** and with `--ban-ips` by its IP too, which also refuses a new id from the same
  machine, but also any honest node behind the same NAT or VPN.

`GET /node`'s counters show `gossip_rejected` and `peers_banned`.

**Size limits:** a gossip message is at most 8 MiB, which a full block at genesis's largest
`max_block_bytes`, 6 MiB, fits with its header and proofs; a sync request 1 KiB, a sync response
64 MiB.

## Finding peers

A node finds peers five ways:
- **`--peer`:** dialed at startup, and again every 15 seconds while no connection to it stands.
- **Bootnodes:** the chain's, from genesis's `bootnodes` or the ones governance decided last,
  and the node's own, from `--bootnode`. Dialed while the node has fewer peers than
  `--target-peers` (15 by default), each at most once a minute.
- **Kademlia:** every peer it connects to tells it, through identify, where it listens, and
  Kademlia keeps those addresses, leaving out a remote peer's loopback ones, which reach it only
  from its own machine; it looks for peers near the node when the first one connects
  and every 2 minutes, asking the peers it knows for theirs. While the node is short of peers it
  dials 3 of those it knows every 15 seconds, each at most once in 5 minutes. So a node given one
  bootnode meets the peers the bootnode knows, and theirs.
- **mDNS** on the local network, unless `--no-mdns`.
- **Peers that dial in,** however many it has: `--target-peers` bounds only what it looks for.

Only nodes of the same chain take part: Kademlia speaks `/{protocol}/{network}/kad/1`, so the DHT
holds the chain's nodes alone, and a peer of another chain is dropped once identify says so.

| Flag | Default | What it does |
| --- | --- | --- |
| `--peer ADDR` | none | A peer to keep a connection to; repeat for several. |
| `--bootnode ADDR` | none | A bootnode of this node's own, beside the chain's; repeat for several. |
| `--target-peers N` | 15 | How many peers to look for. |
| `--no-mdns` | off | Do not look on the local network, as on a server. |

**Peers remembered:** each peer's listen addresses, as identify gives them, are kept in the
database's `peers` column by peer id with when it was last seen, the 64 most recent
(`MAX_REMEMBERED`). A restarted node dials them as it does bootnodes, while short of peers, each
at most once a minute, so it finds its old peers even when its `--peer`s are gone. A banned peer
is forgotten.

**About addresses:**
- **`--peer` takes an address,** such as `/ip4/203.0.113.7/udp/4001/quic-v1`, with or without
  `/p2p/<id>`: peers keep their ids now, so one given is checked, and without it QUIC learns the id
  in its handshake.
- **It is dialed again** every 15 seconds while no connection to it stands: after the peer
  restarts, after this machine's network drops or it wakes from sleep, and before the peer is up
  at all. The log says each time it could not be reached.
- **Names are not looked up.** The node dials `/ip4/` and `/ip6/` addresses, not `/dns4/`: look a
  name up first, as `docker/pow.sh join` does.

## The gossip backlog

Gossiped transactions and votes, and transactions clients submit, wait between the network and
the node loop: in the verifier's queue, then the loop's. The backlog (`src/net/backlog.rs`)
counts them, so a flood is turned away at the door instead of queueing without end.
- **Counted in:** the network thread counts a transaction or vote in as it arrives, before it is
  even decoded. A `NodeHandle` counts a submission in.
- **Counted out:** the gate counts it out once it handled it, taken or refused, and the loop a
  vote.

**The limits:**

| Flag | Default | What it bounds |
| --- | --- | --- |
| `--max-gossip-backlog N` | 10,000 | Everything waiting, submissions included. |
| `--max-block-txs N` | none | The transactions in each block this node produces; without it, blocks fill to the chain's bytes and gas limits. |
| `--max-build-ms N` | none | How long this node runs transactions for each block it produces; what ran by then is sealed, the rest wait. |
| `--max-gossip-per-peer N` | 1,000 | What any one peer's may be. |

**Over a limit:**
- **Gossip is dropped.** It is best effort: honest peers gossip it again, and the transaction is
  still out there.
- **A peer over its own share is struck.** `MAX_STRIKES` (10) strikes within `STRIKE_WINDOW` (a
  minute) and the network disconnects it. A full backlog strikes nobody, since no one peer
  filled it.
- **Submissions are refused:** `POST /txs` answers 503, "the node is busy; try again shortly".

`GET /node` shows what waits (`backlog`), what was dropped in all and from each peer, and how many
peers were disconnected for it. The explorer's Network page shows them too.

**Hardening still to do.** The backlog bounds memory and the loop's work, and bans keep out a
peer that floods or sends what breaks the rules, but a determined peer still has ways around it:
- **Shares are per peer id.** Ids are free, so one host with many ids gets many shares. Limits per
  IP address, or on connections, would close that.
- **gossipsub's own peer scoring is off.** The node's verdicts feed the bans above; gossipsub's
  mesh scoring, which could build on the same verdicts, is not configured.
- **Blocks and sync requests are not counted.** Only transactions and votes are; gossiped blocks
  and requests from syncing peers still queue without bound.
- **The strike rules are fixed.** `MAX_STRIKES` and `STRIKE_WINDOW` are constants; a share small
  enough to stop a flood can also cut off an honest peer relaying a burst.
- **The API has no per-client limit.** Submissions share the backlog's total, so one client can
  still keep others out, if not the node.

## Sync

A node catches up from its peers on its own (`src/node/sync.rs`).

1. **Status.** It asks a peer for its status when the peer connects, and again whenever the peer
   gossips a block that does not connect. A status says:
   - the peer's genesis;
   - its tip and height;
   - its main chain's total work;
   - its latest justified epoch.

   A peer on another chain is never asked for anything again.
2. **Blocks.** While some peer holds more work, the node asks the heaviest for the next blocks.
   With nobody heavier, it asks a peer ahead of it on justification for its blocks, until it has
   them all: a node that mined a heavier branch on its own while cut off from the validators
   fetches the branch they justified, as side blocks, for the justifications to move it to.
   - it asks for at most 128, from just past what both chains share;
   - it imports them in order, side blocks and reorgs included;
   - when a batch does not connect, the chains split earlier, so it asks again a batch further
     back;
   - one request is in flight at a time.
3. **Justifications.** Once nobody holds more work, the node asks a peer ahead of it on
   justification for the justifications it lacks, at most 64 at a time. It checks each before it
   counts, and they may finalize checkpoints here or move the chain to a justified branch.

**Answering peers:**

| Request | Answer |
| --- | --- |
| `Status` | Its genesis, tip, height, work and justified epoch. |
| `Blocks { from, count }` | Up to 128 consecutive main-chain blocks. |
| `Justifications { from, count }` | Up to 64 justifications for main-chain checkpoints, oldest first. |
| `Headers { from, count }` | Up to 1,024 consecutive main-chain headers with their seals. |
| `State { block, depth, prefix }` | One position of the state tree at a recent main-chain block: its entries, at most 512, or its two halves' hashes. `None` for a block it cannot serve. |
| `Checkpoint { block }` | What a node starting at that checkpoint needs: the `CheckpointStart` a light client would get at `GET /light/checkpoint`, and the checkpoint's total work. `None` for a block that is no checkpoint on its main chain, or whose headers it lacks. |

Kinds of finality that take no votes never ask for justifications.

## Syncing from a snapshot

`--snapshot` makes a node with a fresh database take the chain's state from its peers rather
than run every block from genesis (`src/node/snapshot.rs`). It checks every header, takes the
state of one recent block, the pivot, checking it against that block's state root, and makes it
the tip. Then it syncs the rest as usual.

1. **Headers.** It asks the heaviest peer for its headers, 1,024 at a time, and checks each by
   consensus: that it follows its parent, its producer, timing, difficulty and seal. The checking
   is a light client's (`LightClient`) of the node's own chain, fed over the peer protocol: it
   adds up total work as it goes, and when a batch does not link, the peer's chain split off
   earlier, so it steps a batch back. Under a rotating authority, production by stake or
   governance, the client needs each epoch's producers or rules, which finality's light updates
   name: it asks the peer for finality (`Finality`, as `GET /light/finality` serves it), an epoch
   at a time, and checks on once it knows them, so it follows `activation_delay` epochs behind
   what finality has justified, as any light client does.
2. **The pivot.** Once a peer has no more, the pivot is the block `PIVOT_DEPTH` (16) below the
   last header, or half `max_reorg_depth` below when that is less. Deep enough not to come off in
   a reorg, and recent enough for peers to serve: they read state at any height their history
   keeps, `--state-history` blocks back. A chain shorter than `MIN_SNAPSHOT_HEIGHT` (32) is
   synced by running its blocks instead.
3. **State.** It walks the pivot's state tree down from the root (`SnapshotWalk`), a position at a
   time:
   - **What it gets:** a peer answers with the position's entries, when there are at most 512, or
     else with its two halves' hashes, to ask for one at a time.
   - **Checked first:** every part is checked against the pivot's state root before it is kept.
     Its entries must sit under the position and hash, with the siblings, up to the root. A peer
     that sends one that does not is never asked again.
   - **Skipping:** any subtree whose hash matches what the node holds already is skipped.
4. **Moving the pivot.** If a peer can no longer serve the pivot, because the chain moved past
   the state it keeps, the node fetches the headers since, picks a newer pivot and walks
   again. The walk skips every subtree that did not change, so it only fetches what did. After 8
   moves it gives up on the snapshot and syncs by running blocks.
5. **Installing.** Once the tree is the pivot's, it fetches the pivot block itself, checks its
   body against the header, and makes it the tip (`Chain::install_snapshot`):
   - it stores every header below it, with the main chain's heights and the pivot's total work;
   - it records the pivot in `Meta`;
   - it asks every peer for its status again, since what they said may be from before the pivot
     moved.
6. **The rest.** From there it syncs blocks from the pivot on as usual, and justifications from
   the checkpoint it holds final on, which it checks against the headers and the voters in its
   state. The checkpoint it holds final is the one it was told to trust, or a later one the light
   client saw finalized among the headers it installed.

**Starting at a checkpoint** (`--snapshot --checkpoint 0x…`) skips step 1 below the checkpoint.
Given a finalized checkpoint's block id from someone the operator trusts, as a light client takes
one, the node:
1. asks a peer for the checkpoint start (`Checkpoint`): the checkpoint's header and those below
   it that consensus and finality look at, back to the checkpoint `activation_delay` epochs
   before, and the checkpoint's total work. The headers link up by their hashes to the id it
   trusts, so it trusts every one of them without checking them by consensus; the work it takes
   on the peer's word, and a peer that lies about it only loses the node's requests, since
   gossip still brings every block and work is compared from there on;
2. checks headers on from the checkpoint as usual, picks the pivot no lower than the checkpoint,
   and takes state as before, never falling back to running blocks from genesis;
3. installs the pivot with the checkpoint as its justified and finalized checkpoint, so it never
   asks for, imports or rolls back to anything below it, and syncs justifications from the
   checkpoint's epoch on.

A long-range fork, one built below the checkpoint by keys that have since left, cannot reach it:
the node holds no headers below the checkpoint to compare against, and refuses any block at or
below it. `GET /status` of a node you trust shows a checkpoint under `finality.finalized.block`.

**While it takes state,** its state is half the pivot's. So it imports, produces, votes on and
admits nothing: gossip is ignored, submitted transactions answer `Syncing` (503), and it serves
no state of its own.

**Afterwards,** it keeps headers alone for the blocks before the pivot:
- the API shows them marked `pruned`, and the node cannot serve their bodies to peers;
- it never rolls back past the pivot;
- its indexes reach back to the block after it, since it has no receipts before;
- it serves the finality before it all the same: the justifications and light updates its light
  client checked as it checked the headers, from genesis or from the checkpoint start, are stored
  with the snapshot, so light clients follow finality from genesis through it;
- one that started at a checkpoint holds no headers below the checkpoint start either, so it
  cannot serve a checkpoint start, or a light client's older headers, from there down.

## Throughput

`chainv4 bench` measures it:
- **`bench chain`** fills blocks with transfers on an in-process chain, from `--senders` funded
  keys, builds each as its producer and imports it as any other node does, and times signing,
  building and importing: what one machine can execute, without a network. `--db DIR` keeps it in
  RocksDB, synced as a node keeps it.
- **`bench load --node URL…`** funds `--senders` new keys from `--key` (the development key
  unless given), then submits transfers among them at each of `--rates` per second for `--step`
  seconds, from `--threads` connections it keeps open, spread over every node given. Each step
  reports what the nodes took, refused (an HTTP status) and failed, what landed in blocks per
  second, and the median and 99th-percentile time of a submission; it stops at the first rate the
  nodes take less than nine in ten of. `--json FILE` writes the steps.

Measured on an Apple M3 Pro, 12 cores, on the development chain (2-second blocks, 50 million gas,
an Ed25519 transfer 3,620 gas, so 13,812 to a full block and about 6,900 a second at most):

| What | Result |
| --- | --- |
| A full block, built | about 230 ms |
| A full block, imported, in memory | about 265 ms: some 50,000 transfers a second |
| The same, RocksDB, synced | about 380 ms: some 37,000 a second, a fifth of its slot |
| One node, submitted to directly | 6,550 a second land at 9,600 offered, the most blocks hold; past it the excess is refused, never lost |
| Two nodes, submitted to the one that does not produce | 5,940 a second land at 9,600 offered, every transaction gossiped; before batches, 3,660, with a tenth of them lost on the way |

So the protocol's own limit, not the node, bounds a well-connected network; a machine with fewer
cores runs blocks slower, and links slower than a laptop's loopback carry fewer batches.

## What to keep in mind

- **The node's identity changes on every start.** The libp2p key is generated fresh each run.
- **Gossip is forwarded before it is checked,** and peers pay no price for invalid blocks or
  floods of requests. Floods of transactions and votes are bounded by the
  [backlog](#the-gossip-backlog), which disconnects a peer that keeps going over its share, but
  there is no peer scoring or banning yet.
- **Sync uses one peer at a time,** snapshots included, and blocks gossiped ahead of the tip are
  dropped and fetched again by sync.
- **A snapshot node keeps no history before its pivot:** headers, justifications and light
  updates only, no bodies, receipts or events (see [Syncing from a snapshot](#syncing-from-a-snapshot)).
- **Stopping is between steps.** Ctrl-C or SIGTERM sets a flag the loop checks after each step,
  within a second, and the node returns and closes its database. Peers and API clients see their
  connections drop with the process, not a goodbye.
- **No NAT traversal,** and no limits on connections or requests.
- **The indexes are opt in, each on its own.** `--index-events` and `--index-addresses` keep
  theirs, from genesis or the height or `latest-N` window given, and once on, the database keeps
  it on. `=off` deletes one at startup, in one atomic write however big it is, and leaves the
  other. `GET /events` and `GET /accounts/{address}/txs` answer 404 on nodes without the index
  they need.
- **Catching up on a long chain takes a while.** Every older block and its receipts are read
  back. The node keeps syncing and producing meanwhile, and logs how far it got every 30
  seconds.
- **Streaming is opt in, twice:** at build time with the `stream` feature, in the default set,
  and at run time with `--stream`. A node that does not stream answers `/stream` with 404, and
  clients such as the explorer poll instead.
