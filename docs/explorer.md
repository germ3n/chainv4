# Explorer

The explorer (`explorer/`) is a window onto a node, built with egui. It shows:
- blocks on a map, side branches included;
- every transaction, and the hops its contracts made;
- accounts, contracts and tokens;
- the network, the mempool and finality;
- charts of the last blocks.

It can also sign and send transactions.

**One code base, three ways to run it:**
- a desktop app;
- a web app in the browser, built to WebAssembly;
- an Android app (see [Android](#android)).

Each talks to a node's [HTTP API](api.md) and nothing else, so any of them works against any node,
local or remote. Each lays itself out for the room it has: a sidebar on a wide screen, and tabs
along the bottom on a phone (see [Getting around](#getting-around)).

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

The node can be changed under Settings at any time, and on the desktop and on Android so can the
proxy every request goes through, such as Orbot's. See [Through a proxy](#through-a-proxy).

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

**On Android,** with [xbuild](https://github.com/rust-mobile/xbuild): see [Android](#android)
below.

```sh
explorer/build-android.sh --install       # an APK for arm64, put on the device adb sees
```

## Through a proxy

**Settings → Proxy** works as the wallet's (see [Through a proxy](wallet.md#through-a-proxy)):
`socks5h://host:port`, with `socks5://` read as `socks5h://` so the proxy looks up names too, or
`http://host:port`. **Orbot** fills in `socks5h://127.0.0.1:9050`, **I2P** i2pd's
`socks5h://127.0.0.1:4447` (the Java I2P router's HTTP proxy is `http://127.0.0.1:4444`), and
**Off** goes direct.
- **Everything goes through it:** every request and the event stream. With the proxy down,
  nothing is sent: requests fail with `through the proxy at …`, the stream reports itself closed,
  and the explorer polls in vain until the proxy is back.
- **Changing it** connects to the node afresh, as **Connect** does.
- **In the browser** there is no setting: the browser makes the requests, through its own proxy.
  Tor Browser reaches a `.onion` node.

## How it stays live

**With a stream:** a node started with `--stream` pushes what it does over `GET /stream`, as it
happens:
- blocks, side blocks and reorgs;
- justified and finalized checkpoints;
- transactions joining the mempool;
- peers coming and going.

The node's state, under the sidebar or as the dot in a phone's top bar, says **live**, and the
overview's live feed lists each event.

**Polling:** without a stream the explorer polls, and says **polling**; Settings and the network
page say why. It also polls besides
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
- **Where it starts:** from genesis; the first time it meets a node, from the node's own
  finalized checkpoint; and from a checkpoint pasted under Settings → Light client, each time. See
  [Starting at a checkpoint](light-client.md#starting-at-a-checkpoint).
- **What it checks:** every header by consensus, and every justification against voters it proved
  from state. It follows the chain again every few seconds.
- **The sidebar** says whether it is starting, catching up, following, or stopped, with why on
  hover: a node now on another chain than the one pinned, or one that sent something that does
  not check. Settings tells the whole story on any screen, and can forget the pinned genesis.
- **An account's page** shows the balance it proved, from the node's proofs, at the finalized
  block and at the tip, each marked final or not yet. A proof that does not check is shown in red.

## Pages

| Page | Shows |
| --- | --- |
| Overview | Height, block time, throughput, peers, mempool, finality, base price and the chain's coin. Under proof of work, the network's hashrate as difficulty over block time, and the node's own when it mines, its workers' included. Charts of the last 120 blocks: block times, transactions, gas used against the limit, and the base price or the difficulty. The latest blocks, and the live feed. |
| Block map | Every loaded block by height. The main chain runs along the middle, with finalized blocks green. Side branches sit in lanes above and below, each block joined to its parent. The tip and the justified checkpoint are outlined. Brighter blocks hold more transactions. |
| Block | The header, its roots included, seal and finality proofs, and every transaction with its status and fee. |
| Transaction | Where it landed, how deep it is and whether it is final. Its offer, what it used and paid, and what it returned or why it failed. Its hops, what its contracts paid out, its events, its fields and its signature. |
| Account | The balance and nonce, and the balance the light client proved, at the finalized block and the tip. For a contract: its deployer, engine, code size and functions, the storage layout its code declares, a form to call any of them read-only, and its events, of one name or all. For a token: its name, symbol and decimals, from the token registry. For anyone: what they hold of the registered tokens, every transaction that touched them, with its type, parties, status and fee, and the events that mention them, token transfers and approvals in the token's units. Transactions need a node started with `--index-addresses`, events one started with `--index-events`. On a chain with names, the name the address goes by and the names it owns. |
| EVM contract | An [EVM](evm.md) contract's account page: its deployer and code size, and with its ABI pasted, kept on this device per chain, its read functions, run with `eth_call`, and its logs and calls read by name, here and on its transactions' pages. |
| Name | A [name](names.md), such as `alice.dev`: whether it is active, in its grace period or available, its owner, what it resolves to, when it expires and its grace ends, its text records, what a year of it costs, and for a subname, its name. |
| Network | The node's roles, uptime, listening addresses and sync state, its gossip backlog, and what it counted since it started. The chain's bootnodes. When it mines: its hashrate over the last ten minutes, its own GPUs or threads, and a table of its workers, each with its address, when it connected, its hashrate, its devices and the seals it found. A graph of its peers, and a table of them with what each gossiped, how much of that was dropped, and how far its chain got. |
| Mempool | What waits for a block, by sender and nonce. |
| Tokens | The token registry by symbol, with a search by how a symbol starts: each token's symbol, decimals, contract and name, and what makes a token on this chain. |
| NFTs | The [collection registry](nft-standard.md#the-collection-registry): each collection's name, kind, NFT or multi-token, symbol and contract. A collection's page looks a token up, its owner, or how many exist, and its token URI, as text: the explorer fetches nothing from where it points. It lists what an address holds of it, and what moved, from its `Transfer` or `TransferSingle` events on a node with `--index-events`. An account's page lists the NFTs it holds, and a collection's contract links to its page. |
| DEX | The [DEXes](dex.md)' pools: what each holds, its price and share supply, and what traded through it over about a day and a week, in its first asset, with the fees it kept. A pool opens its price chart, from its `Sync` events, and its latest trades, adds and removals, each linked to its transaction. **Native pools** come from `dex.lua`, by read-only calls and its events; **EVM pools** from the router the node names, through `POST /rpc` and the pairs' logs. |
| Bridges | The [bridges](bridges.md): the federation in force and the chain's [directory](governance.md#directory). Transfers through the gateway, out and in, each with the board's count of signatures and who signed, and a page of its own; the wrapped assets it made, with what each stands for. The Bitcoin gateway: its relay's tip, wBTC minted against the bitcoin held, held outputs, deposits, withdrawals and who asked, and its releases, each with a page of its inputs' signatures and its outputs. |
| Swaps | Every [atomic swap](swaps.md) in the swap contract, newest first, from its events: what was locked, for whom, until when, and whether it was claimed or refunded, each lock with whether it is final yet. A swap is found by its hash, as the contract holds it, or by its code, `ur:chainv4-swap`, which lays out both legs, the other chain's too. |
| Finality | The kind, the epoch, and the justified and finalized checkpoints. Who votes and with what weight, the bonded validators, the signers under authority finality, and any conflicting votes the node caught. |
| Send | Signs and sends transfers, contract calls, deploys and burns. |
| Settings | The node, whether it answers and how updates arrive, the chain it is on, the proxy every request goes through, dark or light, and the genesis the light client pinned there, which it can forget. |
| More | On a phone, the pages the tabs have no room for. |

**Faucet,** under Tools, on a node that runs a [faucet](faucet.md): coins for an address, an
Ethereum `0x…` or a name, its proof of work worked out a slice a frame.

## Getting around

**On a wide screen,** a sidebar lists the pages in four groups: Chain (Overview, Block map,
Mempool, Tokens, NFTs), Markets (DEX, Swaps), Network (Network, Finality) and Tools (Send,
Settings). Under them sit the node's address, whether it answers and how, how old its tip is,
and the light client's state.
Over the page are back and forward, the search box and the chain's height.

**On a phone,** below 640 points wide, the sidebar gives way to:
- **a bar on top:** back, once there is somewhere to go back to, the node's state as a dot, and
  the search box. Android's back gesture goes back too, and on the first page sends the app to
  the background, as Android apps do;
- **tabs along the bottom:** Home, Blocks (the map), Mempool, Tokens and More, which lists the
  rest, NFTs, DEX and Swaps among them;
- **pages that stack:** tables become rows of a few lines each, facts sit under their names, and
  charts one above another.

**Everywhere:**
- the search box takes a height, a block or transaction id, an address, or a name such as
  `alice.dev`;
- every block, transaction and address is a link. Right-clicking one, or holding a finger on it,
  opens a menu to copy it;
- every other id, such as a hash or a peer, copies when clicked or tapped, and says **Copied**
  for a moment;
- a page opened anew starts at its top;
- a swipe scrolls the page wherever it starts: on a touch screen text is not selectable, and
  nothing inside a page takes a drag for itself but the map.

**The map:** drag to pan, scroll or pinch to zoom, click a block to see it and double-click
to open it. Panning stops it following the tip until **Reset view**. It opens at real size, so a
phone shows the newest blocks large enough to tap; a block picked there shows in a panel at the
bottom.

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
such as a transfer, shows its one step the same way. On a phone the graph only shows: a swipe
over it scrolls the page.

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

The send page needs a key, as `chainv4 keygen` writes it, or the development key on a dev chain.

**Where the key comes from:**
- **Typed or pasted:** its text, such as `ed25519:0x…`, into the key field, then **Use it**.
- **A key file:** the file picked: from the desktop's dialog, the browser's, or on Android,
  Android's document picker, when the APK carries it (see [Android](#android)).

**Where the key stays:**
- **The key itself:** it stays in the window, and is never saved.
- **Signing:** transactions are built and signed in the explorer with chainv4's own code, the
  same library the node is built from. Only the signed bytes go to the node.

**Steps:**
1. It asks for the sender's nonce.
2. It signs a probe of the transaction and asks the node, with `POST /offer`, for a fee offer
   that covers it now; for a call, deploy or upgrade, its gas limit comes from a dry run. The gas
   limit, max price and tip can each be set by hand instead.
3. It signs with that offer and submits. When the dry run failed, it first shows why, and sends
   only on **Send anyway**.
4. It waits for a block, then links the transaction.

**Amounts:** a transfer's amount and a call's value are typed in the coin's units, such as
`1.5`. Call arguments take the API's argument types, and amounts among them are whole units.

**A Lua contract in several files:** "Open Luau files…" takes them all at once. The file that
includes the others goes in the editor, and its `--@include` lines take the rest by path, or by
file name, as a browser gives no folders. Below the editor it says what goes in, or which include
it cannot find; a deploy that lands and fails says where, in the files it came from. See
[Splitting a contract across files](contracts.md#splitting-a-contract-across-files).

## What it remembers

The node address, the proxy, dark or light, any token decimals set by hand, and the genesis the
light client pinned for each node. It keeps them where eframe keeps app data: the platform's app-data directory
on the desktop, the page's local storage in the browser, and `explorer.ron` in the app's data
folder on Android.

## Look

Its own, for scanning a lot at once, unlike the wallet's quieter one:
- **Colors:** cool slate, dark or light, picked under Settings. Blue is the main chain, amber side
  branches, teal what is justified and green what is final, on every page, chart and the map.
- **Type:** IBM Plex Sans for text, and IBM Plex Mono for ids, addresses and numbers to compare.
  The fonts and their licence are in `explorer/assets/fonts`.
- **Density:** tight rows, small corners and narrow margins: the `ui` crate's `COMPACT`, where
  the wallet is `COMFORTABLE` (see [The ui crate](development.md#the-ui-crate)).
- **Charts:** drawn by the explorer itself, a line or bars over block heights with a scale on the
  left. Hovering one says the block under the pointer and its value; they never take a drag.

## Building and testing

**Its own workspace:** the explorer is a workspace of its own, like `contracts/`. egui and the
browser crates never meet the node's lockfile, and the node's build never compiles egui.

**The chainv4 library:** it links only chainv4's parts that build for the browser too:
- transactions, keys and hashes;
- every hash and key feature, and none of the node's.

**One app, three starts.** The app is a library (`src/lib.rs`), as Android loads it, so the
package is `chainv4-explorer`. Its binary, `explorer` (`src/main.rs`), starts it on the desktop and
in the browser. Android starts the library at `android_main`, which hands it to the `ui` crate's
`android::run`.

**Commands:**

```sh
cd explorer
cargo test
cargo clippy --all-targets -- -D warnings
cargo clippy --target wasm32-unknown-unknown -- -D warnings
CHAINV4_NODE=http://127.0.0.1:8080 cargo test -- --ignored   # sends a transfer through a dev node
```

## Android

`explorer/build-android.sh` builds the Android app with xbuild, through `ui/build-android.sh`, just
as the wallet's is built: the same flags, the same tools, and the same signing (see
[Android](wallet.md#android) in the wallet's guide).

```sh
explorer/build-android.sh                    # explorer/target/android/chainv4-explorer-arm64.apk
explorer/build-android.sh --arch x64         # for x86-64 emulators
explorer/build-android.sh --debug --install  # a debug build, put on the device adb sees
```

**The app** (`explorer/manifest.yaml`):
- `org.chainv4.explorer`, for Android 7 (API 24) and up, targeting API 33, so it sits beside the
  wallet;
- a NativeActivity, and the INTERNET permission;
- `org.chainv4.ui.Picker`, the `ui` crate's Java activity for picking files, compiled in when the
  build finds a JDK and Android's build-tools (see [Picking files](wallet.md#picking-files));
- `explorer/assets/icon.png` for its icon.

**CI:** `.github/workflows/android.yml` builds its APKs beside the wallet's, keeps them as
artifacts named `chainv4-explorer-android-ARCH`, and a `v*` tag attaches them to its release.

**On the phone:**
- **The node:** `http://10.0.2.2:8080` at first, which is the machine running the emulator, as the
  emulator sees it. On a phone, name the node under Settings, such as `http://192.168.1.20:8080`;
  a node on another machine listens on `--http 0.0.0.0:8080`.
- **Copying** goes to Android's clipboard, through the `ui` crate, and Android shows what was
  copied.
- **Pasting:** hold a finger on a text field for a bar with **Paste**, **Select all**, **Copy**
  and **Cut**, or tap the **Paste** button over the keyboard: a key, an address or an id copied
  from anywhere goes straight into the send page or the search box.
- **Files:** keys, WebAssembly modules and Luau files come from Android's document picker, as on
  the desktop.
- **The bars and the keyboard:** the app keeps out from under the status bar, the navigation bar
  and the keyboard, and the keyboard comes up for a focused field.

## What to keep in mind

- **The API is plain HTTP.** Keep a node's API, and so the explorer's traffic, on a trusted
  network. A phone talking to a node over Wi-Fi is on that network too. A proxy, or a Tor exit
  relay, can read plain HTTP as well; a `.onion` node never leaves Tor.
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
