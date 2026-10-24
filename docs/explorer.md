# Explorer

The explorer (`explorer/`) is a window onto a node, built with egui. It shows:
- blocks on a map, side branches included;
- every transaction, and the hops its contracts made;
- accounts, contracts and tokens;
- the network, the mempool and finality;
- charts of the last blocks.

It can also sign and send transactions.

**One code base, two ways to run it:**
- a desktop app;
- a web app in the browser, built to WebAssembly.

Both talk to a node's [HTTP API](api.md) and nothing else, so either works against any node,
local or remote.

## Running it

**On the desktop:**

```sh
cd explorer
cargo run --release -- --node http://127.0.0.1:8080
```

Where it looks for the node:
- `--node`;
- else `CHAINV4_NODE`;
- else the node it talked to last;
- else `http://127.0.0.1:8080`.

The address can be changed in the top bar at any time.

**In the browser,** with [trunk](https://trunkrs.dev):

```sh
rustup target add wasm32-unknown-unknown
cargo install trunk --locked
cd explorer && trunk build --release
chainv4 --http 127.0.0.1:8080 --stream --explorer explorer/dist
```

Then open `http://127.0.0.1:8080/`. The node serves the app from its API port, so the page talks
to the node that served it, with nothing to configure.

**Other ways to point the page at a node:**
- **A hosted page:** a page hosted anywhere else talks to a node that allows its origin with
  `--cors`. `?node=http://host:port` in the page's address names the node.
- **While developing:** run `trunk serve --port 8081`, and start the node with
  `--cors http://127.0.0.1:8081`.

## How it stays live

**With a stream:** a node started with `--stream` pushes what it does over `GET /stream`, as it
happens:
- blocks, side blocks and reorgs;
- justified and finalized checkpoints;
- transactions joining the mempool;
- peers coming and going.

The top bar says **live**, and the overview's live feed lists each event.

**Polling:** without a stream the explorer polls, and says **polling**. It also polls besides
the stream:
- `/status` every second, every three while streaming;
- `/node` every two seconds;
- whatever the page in view shows.

**Keeping the main chain whole:**
- **New blocks:** when the status names a tip the explorer has not loaded, it fetches the blocks
  since.
- **Deep reorgs:** when a page of blocks does not agree with what it loaded below, the chain
  reorganized deeper than the page. It keeps fetching older pages until they agree.
- **Missed events:** a stream client that falls too far behind gets a `lagged` event, and reads
  the chain again.

At most 20,000 main-chain blocks are kept; older ones are let go.

## Pages

| Page | Shows |
| --- | --- |
| Overview | Height, block time, throughput, peers, mempool, finality, base price and the chain's coin. Charts of the last 120 blocks: block times, transactions, gas used against the limit, and the base price or the difficulty. The latest blocks, and the live feed. |
| Block map | Every loaded block by height. The main chain runs along the middle, with finalized blocks green. Side branches sit in lanes above and below, each block joined to its parent. The tip and the justified checkpoint are outlined. Brighter blocks hold more transactions. |
| Block | The header, seal and finality proofs, and every transaction with its status and fee. |
| Transaction | Where it landed, how deep it is and whether it is final. Its offer, what it used and paid, and what it returned or why it failed. Its hops, its events, its fields and its signature. |
| Account | The balance and nonce. For a contract: its deployer, engine, code size and functions, a form to call any of them read-only, and its events, of one name or all. For a token: its name, symbol and decimals. For anyone: what they hold of every token the explorer has met, and the events that mention them, token transfers and approvals in the token's units. Events need a node started with `--index-events`. |
| Network | The node's roles, uptime, listening addresses and sync state, and what it counted since it started. A graph of its peers, and a table of them with what each gossiped and how far its chain got. |
| Mempool | What waits for a block, by sender and nonce. |
| Finality | The kind, the epoch, and the justified and finalized checkpoints. Who votes and with what weight, the bonded validators, the signers under authority finality, and any conflicting votes the node caught. |
| Send | Signs and sends transfers, contract calls, deploys and burns. |

**Getting around:**
- the search box takes a height, a block or transaction id, or an address;
- every id and address is a link, and right-clicking one copies it;
- the arrows go back and forward;
- on the map, drag to pan, scroll or pinch to zoom, click a block to see it and double-click
  to open it. Panning stops it following the tip until **Reset view**.

## Hops

A transaction's hops come from its receipt's trace (see [Contracts](contracts.md#traces)):
- every call, delegate and transfer its contracts made;
- in the order they started, failed ones included.

They are shown two ways at once.

**The call tree:**
- indented by depth, like a debugger's stack;
- each hop's function, value and fuel, and why it failed;
- the events each hop emitted itself, under it.

**The graph of who it touched:**
- an arrow numbered by hop for each call, transfer and, dashed, delegate;
- hops between the same two fanned apart;
- calls blue, transfers green, delegates purple and failures red.

Selecting a hop in either view selects it in the other. A transaction that runs no contract,
such as a transfer, shows its one step the same way.

## Coins and tokens

**The chain's coin:** amounts of it show in the coin's own units. Genesis names the coin's
symbol and decimals (see [Genesis](genesis.md#coin)), and `/status` passes them on, so every
client shows balances alike.

**Tokens:** a contract that exports the [token standard](token-standard.md)'s functions is a
token.
- **What it says of itself:** the explorer asks it for `name()`, `symbol()` and `decimals()`
  through the API's read-only calls.
- **Its amounts:** shown in its own units, such as a `Transfer` event's amount.
- **Decimals by hand:** a token's decimals can be set by hand on its page, for one that reports
  them wrongly or not at all. What is set is remembered per chain, by genesis id.

## Sending

The send page needs a key: a key file written by `chainv4 keygen`, or the development key on a
dev chain.

**Where the key stays:**
- **The key itself:** it stays in the window. On the desktop it is read from the file picked; in
  the browser, from the file chosen in the page.
- **Signing:** transactions are built and signed in the explorer with chainv4's own code, the
  same library the node is built from. Only the signed bytes go to the node.

**Steps:**
1. It asks for the sender's nonce.
2. It signs a probe of the transaction and asks the node, with `POST /offer`, for a fee offer
   that covers it now. The gas limit, max price and tip can each be set by hand instead.
3. It signs with that offer and submits.
4. It waits for a block, then links the transaction.

**Amounts:** a transfer's amount and a call's value are typed in the coin's units, such as
`1.5`. Call arguments take the API's argument types, and amounts among them are whole units.

## What it remembers

The node address and any token decimals set by hand. It keeps them where eframe keeps app
data: the platform's app-data directory on the desktop, the page's local storage in the browser.

## Building and testing

**Its own workspace:** the explorer is a workspace of its own, like `contracts/`. egui and the
browser crates never meet the node's lockfile, and the node's build never compiles egui.

**The chainv4 library:** it links only chainv4's parts that build for the browser too:
- transactions, keys and hashes;
- every hash and key feature, and none of the node's.

**Commands:**

```sh
cd explorer
cargo test
cargo clippy --all-targets -- -D warnings
cargo clippy --target wasm32-unknown-unknown -- -D warnings
CHAINV4_NODE=http://127.0.0.1:8080 cargo test -- --ignored   # sends a transfer through a dev node
```

## What to keep in mind

- **The API is plain HTTP.** Keep a node's API, and so the explorer's traffic, on a trusted
  network.
- **Read-only calls run on the node.** Each one runs under the chain's fuel cap on the API's
  thread, holding up the API's other requests while it runs.
- **Side branches show only what the node kept.** `/forks` lists the side blocks the node
  stores, from each leaf back to the main chain. Restarting a node keeps its leaves.
- **Token holdings are only for tokens met so far.** Nothing lists every token on the chain.
- **Events need the node's index.** A node started with `--index-events` lists them, from the
  lowest height its index holds, and says when it is still indexing older blocks. On any other,
  the explorer says so instead. A list read to its end starts over once the node has indexed
  further back.
