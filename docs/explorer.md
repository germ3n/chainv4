# Explorer

The explorer (`explorer/`) is a window onto a node, built with egui. It shows:
- blocks on a map, side branches included;
- every transaction, and the hops its contracts made;
- accounts, contracts and tokens;
- the network, the mempool, finality and governance;
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

**The light client** asks for headers every 4 seconds without a stream. With one it asks as soon as an
event says the chain moved, and every 20 seconds besides, in case one went missing. It takes nothing
from the event: what it shows, it fetched over HTTP and proved.

**A stream that goes quiet:** a node says something at least every 15 seconds, so one silent for 40 is
taken for dead and opened again. A connection left behind on another network never ends by itself.

**Keeping the main chain whole:**
- **New blocks:** when the status names a tip the explorer has not loaded, it fetches the blocks
  since.
- **Deep reorgs:** when a page of blocks does not agree with what it loaded below, the chain
  reorganized deeper than the page. It keeps fetching older pages until they agree.
- **Missed events:** a stream client that falls too far behind gets a `lagged` event, and reads
  the chain again.

At most 20,000 main-chain blocks are kept; older ones are let go.

**When a request fails:** every request has a time limit, 30 seconds, or 90 through a proxy, so one
that never answers ends in a failure rather than a page that waits, through a proxy too. A page shows
what went wrong in a sentence with a **Retry** button, the same widget as the wallet's, and goes on
showing it until it is answered; while the node itself does not answer, every page says so at its top.
A page goes on asking by itself, a little less often each time, up to every 30 seconds. Retry goes past
that; what you sent is never sent again but by you.

**Reload:** on a phone, pull a page down from its very top. With a mouse, **Reload** is in the header,
or the top bar in a narrow window. Either asks again for what the page asked since it was first shown,
and for the node's status, spins while it works, and leaves what shows until the answers come. The
block map's drags pan it, so it has no pull: its header's button does it on a wide screen.

## The light client

The explorer does not have to take the node's word for balances. It runs the library's
[light client](light-client.md) against the node it talks to:
- **What it trusts:** the genesis it saw the first time it met the node, which it pins.
- **Where it starts:** from genesis; the first time it meets a node, from the node's own
  finalized checkpoint; and from a checkpoint pasted under Settings → Light client, each time. See
  [Starting at a checkpoint](light-client.md#starting-at-a-checkpoint). Settings → Light client →
  **Trust this node for fast resume**, per node and off by default, starts it at the node's finalized
  checkpoint each launch rather than walk from genesis, and the sidebar and Settings then say
  **Fast-resumed at block N, on this node's word**. See [In the explorer](light-client.md#in-the-explorer).
- **What it checks:** every header by consensus, and every justification against voters it proved
  from state. It follows the chain again every few seconds.
- **The sidebar** says whether it is starting, catching up, following, or stopped, with why on
  hover: a node now on another chain than the one pinned, or one that sent something that does
  not check. Settings tells the whole story on any screen, and can forget the pinned genesis.
- **An account's page** shows the balance it proved, from the node's proofs, at the finalized
  block and at the tip, each marked final or not yet. A proof that does not check is shown in red.
- **Governance's page** proves each decision it lists from its receipt and transaction, with
  `GET /proofs/receipts/{tx}` (see [Governance](#governance)): the receipt decided it by the
  chain's count of decisions, of the kind listed, and the transaction approved that kind's change
  from the kind's version before. For a decision in a block below the lowest header it holds, as
  after starting at a checkpoint, it first takes the headers down to it, which the hash chain
  proves.
- **The supply,** on the Rich list & supply page, and **any proof** on the Tools page's Proof tab,
  are checked against headers it checked itself, step by step. Without it following, each says it
  is the node's word alone (see [Tools](#tools)).

## Governance

The Governance page, under Network, shows a chain's [governance](governance.md) from
`GET /governance` and `GET /governance/history`. A chain without governance answers 404, and the
page says the chain has none.
- **Versions:** each kind of change (consensus, which also covers finality's tuning and a switch
  of its kind, bootnodes, name_prices, collections, federation, directory, fees, reward, limits,
  upgrade and name) has a ballot and a version of its own, how many of that kind were decided,
  which an approval names; deciding one kind leaves the others' approvals standing. The chain
  also counts every decision, of any kind, without gaps. The header shows that count, then the
  kinds decided at all, compactly: `By kind: upgrade 1 · name 2`.
- **Who approves:** the kind of governance, and for a council its members, each with its name
  where it has one. Under an electorate, finality's voters approve: validators by stake under
  FFG, signers one each under authority. Then what a change needs, how many epochs an approval
  counts for, the decisions of every kind and each kind's version, and a consensus or switch of
  finality decided for epochs ahead. **Weigh** looks up what an account's approval weighs now,
  and what it approved, one change a kind at most, each for its kind's next change, such as
  `Rename the chain to third, for the name's change 3`, with
  `GET /governance/weight/{address}`.
- **Open proposals:** a search box over each change's kind, words and JSON and who approved it, an
  order (newest approval, most support, or closing soonest, by when the last approval stops
  counting), and a filter by the kinds there are. Each says its kind, its version and the change it
  is approving, such as `version 2: approving the name's change 3`, and the change in words, such as
  `Rename the chain to chainv4 mainnet` or `Nodes older than 0.2.0 stop at block 150,000`, with its
  JSON to open, a bar of its approvals' weight against what it needs, and each approval: its
  account, weight, the epoch it was made in and the epoch it counts until.
- **In force:** every setting governance may change, summed up, with `genesis` while genesis's
  stands, or the change that last set it, a link to that decision.
- **Decisions:** a search box over each one's kind, words, JSON, sender, transaction and block, a
  filter by kind, and newest or oldest first; ten to a page, **Newer page** and **Older page** to
  move, and older pages of `GET /governance/history` read as a page comes to need them, so a search
  goes on through all of them and says how far it got. Each says what it changed, its transaction,
  block, sender and time, and whether it is final or how many confirmations it has. A link from In
  force reads older pages until the decision is listed, and shows the page it is on.

**Each decision checked:** the light client asks for the proof of its receipt and its
transaction against the block it landed in, and it is **proven** only when:
- the receipt says it decided the chain's decision of that number, and of the kind listed, the
  history's `finality` and `switch_finality` being the consensus's kind;
- the transaction is an approval, its own or a multisig's, of the change listed, of the
  receipt's kind, naming the kind's version before the receipt's;
- the block and sender are the ones the node listed.

Until then it says **checking**, or **waiting** for a block the light client has yet to reach. A
proof that disagrees says **does not check**, and why, in red. With the light client stopped,
each is **reported by the node** alone.

**Gaps:** the chain's count goes up by one a decision, of whatever kind, so a number missing
between the lowest read and the newest is a decision the node did not list. The page says so above the list. A node that
synced from a [snapshot](node.md#syncing-from-a-snapshot) keeps decisions from its snapshot on,
and the page says from which.

## Updates

The explorer learns of new releases from the node it talks to, which relays the
[release manifest](updates.md) at `GET /release`. It asks when it connects, then about every
hour. An older node answers 404, and the explorer shows nothing.
- **Checked here:** the node only relays the manifest. The explorer counts it once a threshold
  of the release keys built into it signed it. A build without keys, as `release/keys.json` is
  as checked in, shows no update notices at all.
- **Its own version:** what it compares is the explorer's version, from `explorer/Cargo.toml`,
  not the chainv4 library's.
- **A newer explorer:** a notice over every page says `Explorer 0.2.0 is out (this is 0.1.0)`,
  with the release's notes, until dismissed; a still newer one shows again.
- **One too old:** below the release's `least` version, a screen in place of every page says
  this explorer no longer works with the network, with **Update** and **Quit**. In the browser
  there is no Quit.
- **Update,** on the desktop, when the release has a binary for the target the explorer was
  built for: it downloads it through the proxy set, off the UI thread, and checks its size and
  SHA-256 against the signed release. It writes it beside the running binary, then renames it
  over. It then says `Restart the explorer to run 0.2.0`.
- **Elsewhere,** in the browser, on Android and on iOS, or without a binary for this target, it
  says `Update it from where you installed it.`

## Pages

| Page | Shows |
| --- | --- |
| Overview | The chain's name, coin, hash and genesis. Height, block time, throughput, peers, mempool, finality, base price and the chain's coin. Under proof of work, the network's hashrate as difficulty over block time, and the node's own when it mines, its workers' included. Charts of the last 120 blocks: block times, transactions, gas used against the limit, and the base price or the difficulty. The latest blocks, and the live feed. |
| Block map | Every loaded block by height. The main chain runs along the middle, with finalized blocks green. Side branches sit in lanes above and below, each block joined to its parent. The tip and the justified checkpoint are outlined. Brighter blocks hold more transactions. |
| Block | The header, its roots included, seal and finality proofs, and every transaction with its status and fee. For a mining pool's block: what went to the pool's escrow and its percent of the reward and fees, the escrow, what the producer kept as the pool's fee, and the root its miners claim against, over how many miners. |
| Transaction | Where it landed, how deep it is and whether it is final. Its offer, what it used and paid, and what it returned or why it failed. Its hops, what its contracts paid out, its events, its fields and its signature. A claim says whose mining pool earnings it claims and from which of the pool's blocks, its `earned` being the miner's lifetime total, not what it paid. A governance approval says what it changes, an upgrade as the nodes it stops: `an upgrade: nodes older than 0.2.0 stop at block 150,000`, and a rename as the new name: `Approves renaming the chain to chainv4 mainnet, for the name's change 5`, the change after the version of its kind it names. A withdrawal says the kind it takes its approval back from: `Withdraws its approval of the fees' change 3`. The approval that was the last one needed is `decided`, and names the chain's count and its kind's version, `change 5, the upgrade's version 2`, linking to Governance. |
| Account | The balance and nonce, and the balance the light client proved, at the finalized block and the tip. For a contract: its deployer, engine, code size and functions, the storage layout its code declares, a form to call any of them read-only, and its events, of one name or all. For a token: its name, symbol and decimals, from the token registry. For anyone: what they hold of the registered tokens, every transaction that touched them, with its type, parties, status and fee, narrowed by [filters](#an-accounts-transactions), and the events that mention them, token transfers and approvals in the token's units. Transactions need a node started with `--index-addresses`, events one started with `--index-events`. On a chain with names, the name the address goes by and the names it owns. |
| EVM contract | An [EVM](evm.md) contract's account page: its deployer and code size, and with its ABI pasted, kept on this device per chain, its read functions, run with `eth_call`, and its logs and calls read by name, here and on its transactions' pages. |
| Name | A [name](names.md), such as `alice.dev`: whether it is active, in its grace period or available, its owner, what it resolves to, when it expires and its grace ends, its text records, what a year of it costs, and for a subname, its name. |
| Network | The node's roles, uptime, listening addresses and sync state, its chainv4 version and how it stands against the newest release (`chainv4 0.2.0 is out`, or `0.2.0 installed, restarting` once it put one in place), any [upgrade](updates.md) that stops older nodes, decided by governance or announced by a release, with the blocks left to it from the tip, its gossip backlog, and what it counted since it started. The chain's name, or that it gave none, and its bootnodes. When it mines: its hashrate over the last ten minutes, its own GPUs or threads, and a table of its workers, each with its address, when it connected, its hashrate, its devices and the seals it found; a pool's worker also with its payout address and the shares it sent. When it runs a pool: its fee, window and miners, and a link to the Mining pool page. A graph of its peers, and a table of them with what each gossiped, how much of that was dropped, and how far its chain got. |
| Mempool | What waits for a block, by sender and nonce. |
| Tokens | The token registry by symbol, with a search by how a symbol starts: each token's symbol, decimals, contract and name, and what makes a token on this chain. |
| NFTs | The [collection registry](nft-standard.md#the-collection-registry): each collection's name, kind, NFT or multi-token, symbol and contract. A collection's page looks a token up, its owner, or how many exist, and its token URI, as text: the explorer fetches nothing from where it points. It lists what an address holds of it, and what moved, from its `Transfer` or `TransferSingle` events on a node with `--index-events`. An account's page lists the NFTs it holds, and a collection's contract links to its page. |
| DEX | The [DEXes](dex.md)' pools: what each holds, its price and share supply, and what traded through it over about a day and a week, in its first asset, with the fees it kept. A pool opens its price chart, from its `Sync` events, and its latest trades, adds and removals, each linked to its transaction. **Native pools** come from `dex.lua`, by read-only calls and its events; **EVM pools** from the router the node names, through `POST /rpc` and the pairs' logs. |
| Bridges | The [bridges](bridges.md): the federation in force and the chain's [directory](governance.md#directory). Transfers through the gateway, out and in, each with the board's count of signatures and who signed, and a page of its own; the wrapped assets it made, with what each stands for. The Bitcoin gateway: its relay's tip, wBTC minted against the bitcoin held, held outputs, deposits, withdrawals and who asked, and its releases, each with a page of its inputs' signatures and its outputs. |
| Swaps | Every [atomic swap](swaps.md) in the swap contract, newest first, from its events: what was locked, for whom, until when, and whether it was claimed or refunded, each lock with whether it is final yet. A swap is found by its hash, as the contract holds it, or by its code, `ur:chainv4-swap`, which lays out both legs, the other chain's too. |
| Mining pool | The pool the node runs, on one started with `--mining … --pool`: its fee, window, miners, escrow balance and all it credited; its producer and escrow; every miner with its part of the window's work, of the next block, and what it earned, claimed and may still claim; the blocks it sealed lately, each waiting to be final, credited, or dropped by a reorganization, with what it shared out among how many miners. A miner's payout address looks up its own books: its workers, and the blocks that paid it and how much. A node without a pool says how to run one. |
| Multisig | A [multisig](multisig.md) looked up by its `multisig:0x…` address, from `GET /multisig/{address}`: its members, each with the account its key controls and its Bitcoin xpub and EVM address where it names them, the threshold, its nonce and balance, and the proposals open for that nonce, each with its approvals against the threshold. A multisig's account page links here. See [Multisig](#multisig). |
| Names | The name directory from `GET /names/list`, sorted by name, 50 to a page with **More**: each name's status, owner, what it resolves to and when it expires. A search opens a name's page. See [Names](#names). |
| Rich list & supply | The richest accounts from `GET /rich`, each with its share of the supply, and the supply from `GET /supply`, its minted and burned proven by the light client. See [Rich list and supply](#rich-list-and-supply). |
| Tools | Decode, Convert, Hash & verify, and Proof, in tabs. See [Tools](#tools). |
| Finality | The kind, the epoch, and the justified and finalized checkpoints. Who votes and with what weight, the bonded validators, the signers under authority finality, and any conflicting votes the node caught. |
| Governance | Who approves changes and what one needs, the changes being approved with their approvals, every setting in force and the change that set it, and every decision, each proven by the light client. See [Governance](#governance). |
| Send | Signs and sends transfers, contract calls, deploys and burns. |
| Settings | The node, whether it answers and how updates arrive, the chain it is on, the proxy every request goes through, dark or light, and the genesis the light client pinned there, which it can forget. |
| More | On a phone, the pages the tabs have no room for. |

**Faucet,** under Tools, on a node that runs a [faucet](faucet.md): coins for an address, an
Ethereum `0x…` or a name, its proof of work worked out a slice a frame.

## Tools

The Tools page, under Tools in the sidebar and in More on a phone, has four tabs. All but
Proof's fetch work offline.

**Decode** reads what is pasted:
- **A raw signed transaction,** `0x…` as the chain encodes it, or the wallet's code for one,
  `ur:chainv4-signed/…`: its kind, id, with a link to its page, sender, nonce, offer, signer and
  signature, and whether the signature checks on this chain; an EVM call's calldata read by the
  ABIs; then every field as the tx page shows it, from the node's own JSON for it, written here.
- **A swap's code,** `ur:chainv4-swap/…`: the hash, offer or answer, and each leg's chain, what it
  locks, where, who pays, who claims and when it ends.
- **A multisig bundle,** `chainv4-multisig:…` or its code, `ur:chainv4-multisig/…`: the chain it is for, this one or another, the
  multisig, the transaction's id, nonce and offer, and each signature so far, checked over the
  bundle's chain. An EVM multisig's bundle, `chainv4-multisig-evm:…`, shows its chain, contract,
  call, digest and who signed.
- **EVM calldata and logs,** read by the ABIs built in: the standard ERC-20 and ERC-721, and every
  contract in `contracts/evm`. Or by an ABI pasted. A log takes its topics, one to a line, and its
  data. An ERC-20's and an ERC-721's `Transfer` share a topic: the one whose topics fit is read.

**Find events** lists what the node's event index holds: EVM logs by a signature such as
`Transfer(address,address,uint256)`, a built-in event's name or a topic, of one contract or any; or
everything naming an address, in an event's values or a log's topics. A node too old to find by
topic says so, and what it answered is not shown, as it would not be filtered.

**EVM calls and logs everywhere.** A transaction's page, the account page's events and Find events
read them the same way (`src/abi/known.rs`, shared with the wallet): names and typed values, not hex,
which stays a click away:
- **Which ABI:** the one kept for the contract goes first, and each says what read it: `read as
  ERC-20`, which is only a match, or `read by the ABI kept for this contract`. Bytes past what a call
  reads are said to be there.
- **Addresses** open their pages. An address read from EVM bytes may be a contract or a secp256k1
  account, so its page at `evm:0x…` links to the other where there is no contract.
- **Amounts** of the contract's own token go in its decimals where known: the registry's, set by
  hand, or what the contract says of itself through `eth_call`. Otherwise they are plain whole
  numbers. An amount elsewhere, such as a router's, stays plain.
- **Tokens moving:** an ERC-20's, ERC-721's or ERC-1155's transfer reads as who sent what to whom, as
  a native token's does, saying it is only what the log claims unless the registry lists the
  contract.
- **Nested calls:** a `bytes` argument that reads exactly as a call shows it, a few levels down, as
  a multisig's `execute` holds one. Bytes inside an array or a tuple are not looked into.

**Convert:**
- **Units:** the smallest unit to the coin and back, at the chain's decimals from `GET /status`.
- **Address and EVM alias:** a `secp256k1:0x…` account and its EVM `0x…` address are the same 20
  bytes, either way, shown with EIP-55's checksum too. Any other account's alias is a hash of its
  key, one way only.
- **Public key to address:** under each scheme the key fits. A 32-byte key under ed25519 and
  edblake, a 33-byte compressed one under secp256k1 and bitcoin, and a key with its scheme in
  front under that one.
- **Bech32 and hex:** a version 0 segwit address, `bc1q…`, `tb1q…` or `bcrt1q…`, to its witness
  program, P2WPKH or P2WSH, and a program back under the network picked.

**Hash & verify:**
- **Hashes** of text or hex: sha256, blake3, blake2s, blake2b, xxh3 and keccak256, then Bitcoin's
  sha256d and hash160.
- **A signature check:** a public key, `scheme:0x…`, a message as text or hex, and a signature of
  64 bytes. ed25519 and edblake sign the message itself, secp256k1 its keccak256, bitcoin its
  double SHA-256.
- **An EVM selector and topic** from a signature such as `transfer(address,uint256)`, hashed as
  Solidity does: `uint` is `uint256`, and spaces and names go.

**Proof** proves an account, any chain-state entry by its column and key, or a transaction's
receipt and the transaction. It fetches `GET /proofs/{column}/{key}` at the newest block the
light client checked, or `GET /proofs/receipts/{tx}`. A proof's JSON can be pasted instead. Each
is checked step by step:
- **header:** the block's, one the light client checked itself;
- **finality:** final, not yet, or a chain without finality;
- **root:** the root the node names, against the header's: the state root, or the receipts root
  and tx root;
- **path:** the hashes from the entry, or the receipt and the transaction, up to that root.

The verdict is **proven**, or proven and final; **does not check**, and why; **waiting**, for a
block above the light client's tip; or **not checked**. While the light client follows nothing,
or has stopped, a proof is read and said to be unchecked, never passed off as proven. An account's
value reads as its balance and nonce, and the supply's as what was minted and burned.

## Multisig

The Multisig page, under Chain, looks a [multisig](multisig.md) up by its `multisig:0x…` address
with `GET /multisig/{address}`. Anything else answers 404, and the page says it is no multisig.
- **Members:** each key, the account it controls, and its Bitcoin xpub and EVM address where it
  names them.
- **Threshold, nonce and balance.**
- **Open proposals,** all for the next nonce and void once any of the multisig's transactions
  runs: each one's kind, id, offer and approvals, counted once a member, against the threshold.
  What it does opens as JSON.
- **On other chains:** its Bitcoin multisig's first receiving address and descriptor, and its EVM
  contract and owners, where every member names keys there.

A multisig's account page links here. As the node says it: nothing on the page is proven.

## Names

The Names page, under Chain, lists every [name](names.md) from `GET /names/list?after=…`, sorted by
name, 50 to a page, **More** for the next. Each name has its status, owner, what it resolves to,
and when it expires, and opens its page. A search opens any name's page, by `GET /names/{name}`.
A chain without names, or an older node, answers 404, and the page says it keeps none. As the node
says it.

## Rich list and supply

The Rich list & supply page, under Chain:
- **Supply,** from `GET /supply`: what was minted and burned since genesis, the supply, minted less
  burned, what is staked and in the reward pool, at a block.
- **Proven:** minted and burned are one state entry, `chainv4::registry::supply::Supply`, under
  `coin:supply` in the validators column. Once the light client caught up, the page asks for its
  proof, `GET /proofs/validators/{key}`, at the newest block the light client checked, under the
  key the explorer knows rather than the one the node names. It says **proven**, or proven and
  final, with what the proof holds, when it checks. It says **does not check** for a proof that
  fails, or one at the node's own block that holds other numbers than the node reports. Until then,
  and without the light client, it says **reported by the node**.
- **Staked and the reward pool** are the node's word alone, and the page says so.
- **Richest accounts,** from `GET /rich`: up to 100, richest first, each with its balance and share
  of the supply, and how many accounts hold anything at all. The rich list is not proven, and the
  page says so.

An older node answers either route with 404, and the page says it is from before them.

## What a node keeps to itself

A node [keeps some of its report to itself](node.md#what-a-node-shares-of-itself): by default where
its peers, its workers and it itself are, and its workers' names. The Network page and the
Overview's tiles say "hidden by the node" for each, and Settings says what it shares with this
explorer. **Report token** in Settings, kept per node, shows you everything of a node that is
yours: it goes to `GET /node` alone, and only over https, an onion or this machine, and Settings
says when the node refuses it. An explorer on the node's own machine sees everything without one.

## Getting around

**On a wide screen,** a sidebar lists the pages in four groups: Chain (Overview, Block map,
Mempool, Tokens, NFTs, Multisig, Names, Rich list & supply), Markets (DEX, Swaps, Bridges),
Network (Network, Finality, Governance, Mining pool) and Tools (Tools, Send, Faucet, Settings). Under them sit the node's address, whether it answers and how, how old its tip is,
and the light client's state, which keep the sidebar's bottom while the pages above them scroll
in a window too short for them all (`chainv4_ui::layout::sidebar`, shared with the wallet).
Over the page are back and forward, the search box and the chain's height. Tapping a group's heading folds it, and the app remembers which are folded. A folded group still lists the page open now, so the sidebar always says where you are.

**The chain's name** heads the sidebar, or the bar on a phone, and titles the window or the
browser's tab, such as `chainv4 testnet explorer`. It comes from `GET /status`'s `name`, which
genesis sets and governance renames (see [Name](governance.md#name)), and is remembered per node,
so the next launch shows it before the node answers. A chain without one, or an older node that
sends none, shows as `chainv4`. The software's own name stays `chainv4`, as in the Network page's
version row and its release notices.

**On a phone,** below 640 points wide, the sidebar gives way to:
- **a bar on top:** back, once there is somewhere to go back to, the node's state as a dot, which a
  tap has say it and the light client's in words, and the search box. Android's back gesture goes back too, and on the first page sends the app to
  the background, as Android apps do. Back and forward return to a page where it was scrolled
  to, once what it shows has arrived; a page opened anew starts at its top;
- **tabs along the bottom:** Home, Blocks (the map), Mempool, Tokens and More, which lists the
  rest, NFTs, Multisig, Names, Rich list & supply, DEX, Swaps and Tools among them;
- **pages that stack:** tables become rows of a few lines each, facts sit under their names, and
  charts one above another, and a row with a value at its end keeps it there, however long what
  is beside it;
- **buttons and fields a finger tall,** 40 points, as on any touch screen, a phone on its side
  included, where the sidebar layout shows, whose foot folds to one line, the light client's and
  the node's dots and a word, on a screen under 560 points tall; the field being typed in scrolls
  into sight above the keyboard.

**Everywhere:**
- the search box takes a height, a block or transaction id, an address, or a name such as
  `alice.dev`;
- every block, transaction and address is a link. Right-clicking one, or holding a finger on it,
  opens a menu to copy it;
- every other id, such as a hash or a peer, copies when clicked or tapped, and says **Copied**
  for a moment. One shown cut short copies whole, as a peer's address on the Network page, whose
  peer id shows as `12D3Ko…n6Xb`;
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

## An account's transactions

The **Filters** button over an account's transactions opens a choice of kind, direction, token,
counterparty and when, says how many are on, and **Clear** turns them all off. They are the wallet's
controls and go by the same [filters on the node](api.md#get-accountsaddresstxs), so all the history
counts, not only what is loaded. **Look further back** asks for the next page, and goes on from where
the node stopped at its bound of 1,000 entries a request, which it says when it reaches. An older node
ignores filters, which its answer shows: then what passes the same test here shows, and the page says
only what was read was searched. Its transactions as its node lists them are the node's word, as on
any of its pages: their own pages prove them.

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

The node address, the proxy, dark or light, any token decimals set by hand, the nodes trusted for
fast resume, and the genesis the light client pinned and the chain's name last seen for each node. It keeps them where eframe keeps app data: the platform's app-data directory
on the desktop, the page's local storage in the browser, and `explorer.ron` in the app's data
folder on Android.

## Look

The wallet's, from the `ui` crate they share, set tighter for scanning a lot at once; its pages,
tables and charts are its own:
- **Colors:** the wallet's palette, dark or light, picked under Settings, its gold marking what is
  picked and what is a link. For the chain's own ideas: blue is the main chain, orange side branches,
  teal what is justified and green what is final, on every page, chart and the map, told apart under
  simulated color blindness in both themes.
- **Type:** Manrope for text, JetBrains Mono for ids, addresses and numbers to compare, and Newsreader
  for page titles and the chain's name. The fonts and their licences are in `ui/assets/fonts`.
- **Tables** stay tables, in the same cards, with a hairline under each row, as wide as the card.
- **Density:** tighter rows and margins than the wallet's, with the same corners: the `ui` crate's
  `COMPACT`, where the wallet is `COMFORTABLE` (see [The ui crate](development.md#the-ui-crate)).
- **Charts:** drawn by the explorer itself, a line or bars over block heights with a scale on the
  left. Hovering one says the block under the pointer and its value; they never take a drag.

## Building and testing

**Its own workspace:** the explorer is a workspace of its own, like `contracts/`. egui and the
browser crates never meet the node's lockfile, and the node's build never compiles egui.

**The chainv4 library:** it links only chainv4's parts that build for the browser too:
- transactions, keys and hashes;
- releases, which it checks against the release keys built in, and on the desktop installs;
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
- **Pasting:** hold a finger on a text field, or tap one already focused, for a bar with **Paste**,
  **Select all**, **Copy** and **Cut**, which goes as soon as typing starts: a key, an address or an
  id copied from anywhere goes straight into the send page or the search box.
- **Files:** keys, WebAssembly modules and Luau files come from Android's document picker, as on
  the desktop.
- **The bars and the keyboard:** the app keeps out from under the status bar, the navigation bar
  and the keyboard, and the keyboard comes up for a focused field.


## iOS

`explorer/build-ios.sh` builds the iOS app through `ui/build-ios.sh`, just as the wallet's is
built: the Simulator with `--sim`, an unsigned IPA with `--ipa`, and a free Apple ID's signing
with `--provision TEAMID` once and then `--install` (see [iOS](wallet.md#ios) in the wallet's
guide). It is `org.chainv4.explorer`, for iOS 16 and up, its icon from
`explorer/assets/icon.png`; iOS's swipe in from the left edge goes back a page.

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
