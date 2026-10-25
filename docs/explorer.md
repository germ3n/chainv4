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

## The light client

The explorer does not have to take the node's word for balances. It runs the library's
[light client](light-client.md) against the node it talks to:
- **What it trusts:** the genesis it saw the first time it met the node, which it pins.
- **What it checks:** every header by consensus, and every justification against voters it proved
  from state. It follows the chain again every few seconds.
- **The top bar** says whether it is starting, catching up, following, or stopped, with why on
  hover: a node now on another chain than the one pinned, or one that sent something that does
  not check.
- **An account's page** shows the balance it proved, from the node's proofs, at the finalized
  block and at the tip, each marked final or not yet. A proof that does not check is shown in red.

## Pages

| Page | Shows |
| --- | --- |
| Overview | Height, block time, throughput, peers, mempool, finality, base price and the chain's coin. Charts of the last 120 blocks: block times, transactions, gas used against the limit, and the base price or the difficulty. The latest blocks, and the live feed. |
| Block map | Every loaded block by height. The main chain runs along the middle, with finalized blocks green. Side branches sit in lanes above and below, each block joined to its parent. The tip and the justified checkpoint are outlined. Brighter blocks hold more transactions. |
| Block | The header, its roots included, seal and finality proofs, and every transaction with its status and fee. |
| Transaction | Where it landed, how deep it is and whether it is final. Its offer, what it used and paid, and what it returned or why it failed. Its hops, its events, its fields and its signature. |
| Account | The balance and nonce, and the balance the light client proved, at the finalized block and the tip. For a contract: its deployer, engine, code size and functions, the storage layout its code declares, a form to call any of them read-only, and its events, of one name or all. For a token: its name, symbol and decimals, from the token registry. For anyone: what they hold of the registered tokens, every transaction that touched them, with its type, parties, status and fee, and the events that mention them, token transfers and approvals in the token's units. Transactions need a node started with `--index-addresses`, events one started with `--index-events`. |
| Network | The node's roles, uptime, listening addresses and sync state, its gossip backlog, and what it counted since it started. A graph of its peers, and a table of them with what each gossiped, how much of that was dropped, and how far its chain got. |
| Mempool | What waits for a block, by sender and nonce. |
| Tokens | The token registry by symbol, with a search by how a symbol starts: each token's symbol, decimals, contract and name, and what makes a token on this chain. |
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

**Tokens:** the chain's [token registry](token-standard.md#the-token-registry) says which
contracts are tokens.
- **What it says of itself:** the token's name, symbol and decimals, as the registry holds them,
  from `GET /tokens` or `GET /contracts/{address}`. Nothing is called to learn them.
- **Its amounts:** shown in its own units, such as a `Transfer` event's amount.
- **Holdings:** an account's page asks `balance_of` of the registry's first 100 tokens by symbol,
  and of any other token met, and lists those it holds some of.
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

**A Lua contract in several files:** "Open Luau files…" takes them all at once. The file that
includes the others goes in the editor, and its `--@include` lines take the rest by path, or by
file name, as a browser gives no folders. Below the editor it says what goes in, or which include
it cannot find; a deploy that lands and fails says where, in the files it came from. See
[Splitting a contract across files](contracts.md#splitting-a-contract-across-files).

## What it remembers

The node address, any token decimals set by hand, and the genesis the light client pinned for
each node. It keeps them where eframe keeps app
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
- **Holdings check at most the registry's first 100 tokens,** by symbol, besides those met
  elsewhere, with a read-only call each on the node.
- **Events and an account's transactions need the node's indexes.** A node started with
  `--index-events` lists events, and one started with `--index-addresses` an account's
  transactions, from the lowest height its index holds, and says when it is still indexing older
  blocks. On any other, the explorer says so instead. A list read to its end starts over once the
  node has indexed further back.
