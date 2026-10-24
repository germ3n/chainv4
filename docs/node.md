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
Then it hands them on, in the order they came, with what each check found. The loop never
spends its time on signatures, and a flood of gossiped transactions is checked on every core.
`NetLink::pair` links, in tests, check on the loop's own thread instead, so a test's events are
there the moment it sends them.

Its loop, `Node::run`, repeats two steps. **`step`** does the work:
1. It handles what arrived, at most 256 events a pass so a flood cannot starve block production:
   - gossiped blocks, transactions and votes;
   - peers' requests and answers;
   - peers connecting and leaving;
   - a miner's seal;
   - transactions submitted through the API, and the API asking for a report.
2. It produces a block if its slot has come. An authority seals at once; a miner gets a template.
3. It votes, if it validates and a checkpoint is deep enough.
4. It catches the event index up a step, on a node started with `--index-events` (see
   [Storage](storage.md#the-event-index)).
5. It logs where it stands every 30 seconds.

**`wait`** then sleeps until the next slot, the next event, or a second at most. While the event
index is still catching up, it does not sleep, so a long catch-up runs a step at a time in
between everything else.

Tests drive `step` with a clock of their own, so they never sleep.

| Constant | Value | What it bounds |
| --- | --- | --- |
| `EVENT_BUDGET` | 256 | Events handled per pass. |
| `MAX_BLOCK_TXS` | 1,000 | Transactions this node puts in one block. |
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
its current nonce, up to the first gap. Across senders it takes whichever sender's next
transaction pays the producer most, ties going to the lower address. The block builder then runs
them and leaves out any that cannot run.

**Pruning** after each block drops transactions whose nonce is used.

## Networking

**The thread:** libp2p runs on a thread of its own (`src/net/`), behind two channels:
- `NetEvent`s come in;
- `NetCommand`s go out.

The node never sees libp2p itself.

| Piece | What it does |
| --- | --- |
| QUIC | The transport. `--listen` picks addresses, `/ip4/0.0.0.0/udp/0/quic-v1` by default; port 0 picks one, and the log shows which. |
| gossipsub | Blocks, transactions and votes, on topics `/chainv4/{network}/blocks`, `/txs` and `/votes`. |
| Kademlia | Finding peers, as `/chainv4/kad/1`. |
| mDNS | Finding peers on the local network. |
| identify | Learning peers' addresses, as `/chainv4/1`. |
| request-response | Sync, as `/chainv4/sync/1`. |

**Topics:** `{network}` is the first eight bytes of the genesis id in hex, so nodes on different
chains never share topics.

**Message ids:** a gossip message's id is the chain hash of its bytes, so the same block or
transaction is one message however many peers send it.

**Size limits:** a gossip message is at most 4 MiB, a sync request 1 KiB, a sync response
64 MiB.

**Peers** come from `--peer`, mDNS and Kademlia. There is no bootnode list.

## The gossip backlog

Gossiped transactions and votes, and transactions clients submit, wait between the network and
the node loop: in the verifier's queue, then the loop's. The backlog (`src/net/backlog.rs`)
counts them, so a flood is turned away at the door instead of queueing without end.
- **Counted in:** the network thread counts a transaction or vote in as it arrives, before it is
  even decoded. A `NodeHandle` counts a submission in.
- **Counted out:** the node loop counts it out once it handled it, taken or refused.

**The limits:**

| Flag | Default | What it bounds |
| --- | --- | --- |
| `--max-gossip-backlog N` | 10,000 | Everything waiting, submissions included. |
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

**Hardening still to do.** The backlog bounds memory and the loop's work, but a determined peer
still has ways around it:
- **No bans.** A disconnected flooder can connect again at once, through its own dial, mDNS or
  Kademlia, and starts with a clean slate. A ban list, with an expiry, would keep it out.
- **Shares are per peer id.** Ids are free, so one host with many ids gets many shares. Limits per
  IP address, or on connections, would close that.
- **Invalid gossip costs nothing.** A peer whose transactions fail their signature check, or whose
  blocks are invalid, is not struck; only volume is. Striking, or scoring, on what the verifier
  and the chain find would.
- **Gossip is forwarded before it is checked.** gossipsub relays messages on before the node has
  looked at them, so a flood passes through this node to its peers. Its `validate_messages` mode,
  reporting each verdict back, would stop that, and gossipsub's own peer scoring could build on it.
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
2. **Blocks.** While some peer holds more work, the node asks the heaviest for the next blocks:
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

Kinds of finality that take no votes never ask for justifications.

## What to keep in mind

- **The node's identity changes on every start.** The libp2p key is generated fresh each run.
- **Gossip is forwarded before it is checked,** and peers pay no price for invalid blocks or
  floods of requests. Floods of transactions and votes are bounded by the
  [backlog](#the-gossip-backlog), which disconnects a peer that keeps going over its share, but
  there is no peer scoring or banning yet.
- **Sync uses one peer at a time,** and blocks gossiped ahead of the tip are dropped and fetched
  again by sync.
- **No graceful shutdown.** Ctrl-C stops the process mid-loop; commits are atomic, so data is
  safe.
- **No NAT traversal,** and no limits on connections or requests.
- **The event index is opt in.** `--index-events` keeps it, from genesis or the height or
  `latest-N` window given, and once on, the database keeps it on. `--index-events=off` deletes
  it at startup, in one atomic write however big it is. `GET /events` answers 404 on nodes
  without one.
- **Catching up on a long chain takes a while.** Every older block and its receipts are read
  back. The node keeps syncing and producing meanwhile, and logs how far it got every 30
  seconds.
- **Streaming is opt in, twice:** at build time with the `stream` feature, in the default set,
  and at run time with `--stream`. A node that does not stream answers `/stream` with 404, and
  clients such as the explorer poll instead.
