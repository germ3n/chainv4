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
- else the public devnet, `https://chain.homepage.gs` (`chainv4_ui::PUBLIC_NODE`).

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

**Low data and Wait for answers:** Settings → Network use has the wallet's two settings (see
[Network use](wallet.md#network-use)). **Low data** has the explorer ask four times less often by
itself: the status every 4 seconds rather than 1 (12 rather than 3 with a stream), the node's report
every 8, forks every 20, a page's own every 10 rather than 2.5 and each page's refresh (tokens,
events, an account's history, the DEX, swaps, bridges, collections), the light client every 16, and
it wakes by itself every 2 seconds rather than half of one. What the stream tells, a retry and a
reload are not held back. **Wait for answers** gives every request 2, 4 or 10 times as long.

**When a request fails:** every request has a time limit, 30 seconds, or 90 through a proxy (times
Wait for answers), so one
that never answers ends in a failure rather than a page that waits, through a proxy too. A page shows
what went wrong in a sentence with a **Retry** button, the same widget as the wallet's, and goes on
showing it until it is answered; while the node itself does not answer, every page says so at its top.
A page goes on asking by itself, a little less often each time, up to every 30 seconds. Retry goes past
that; what you sent is never sent again but by you.

**Reload:** on a phone, pull a page down from its very top. With a mouse, **Reload** is in the header,
or the top bar in a narrow window. Either asks again for what the page asked since it was first shown,
and for the node's status, spins while it works, and leaves what shows until the answers come. The
block map's drags pan it, so it has no pull: its header's button does it on a wide screen.

## Read from checked blocks

Settings → **Read from checked blocks** has the explorer take what it shows from final blocks it
fetched and checked itself, rather than ask the node, so what it shows rests on those blocks and the
node learns less of what you look at. It is off by default, as reading blocks costs far more than
asking, and with it off nothing here happens at all. It is for the install rather than for a node:
changing the node never quietly starts asking it about everything. It works alike on the desktop, on
phones and in the browser. The [wallet's](wallet.md#read-from-checked-blocks) is the same setting, on the
same code.

It is strict: **what cannot be proven this way is turned off and says why**, rather than shown with a
label. The refusal is in the one way out to the node, `Api`, with every request named in `Ask::route`,
so a request from a path nobody thought of does not leave, and a new one does not build until someone
decides.

**What it does:**
- **It reads final blocks only,** from [`GET /blocks/range`](api.md#get-blocksrangefromhcountnentrye),
  each against what the light client checked, so nothing it read can be undone. So the newest few
  blocks do not show, nor side branches.
- **It starts at the final block,** and reads on from there, which keeps nothing older and pays for
  nothing older. Settings reads further back to a height you give, at any time: what it read stands, and
  it shows from what block it knows. Newest comes first, and older blocks are checked by their hash links
  up to one the light client checked.
- **Pages read what it read:** the Overview, the block map and pages of blocks, a block, a transaction,
  an address's transactions, events, and search, with the filters on an address's transactions. They ask
  as ever, and the index answers in the node's place, in the node's form. What was not read says so,
  with the way to read further back.
- **A block by itself.** A block's page for a height outside what was read can fetch that block alone,
  which says that the node sees which. It is checked against the light client's header, shown for now
  and not counted as read. With no header at that height, it says to read further back instead.
- **A direct proof, when pressed for.** An account's balance and nonce, a name's record, a token's record
  and the coin's supply each have a press that asks for a proof at the final block, saying the node sees
  what it is of, and each shows the block it was proven at. Nothing is asked by opening a page, and a
  search for an address goes nowhere near a node.
- **Sending** proves the sender's account at the final block for its nonce, and counts what it sent
  itself since; `POST /offer` is off, so the fee is suggested as the wallet suggests one, from what the same
  kind of transaction used lately in checked blocks, a quarter more, or a round guess that says it is one,
  and twice the newest checked header's base price, or what was seen paid if more. Both can be changed.
  Nothing tries it first, so it shows them before it goes, and a call that would fail still costs its fee.
  A deploy is not offered, as it needs a dry run. What was sent shows as landed once a final block read
  here holds it.
- **The stream only wakes it.** `/stream` says when to follow the chain; nothing it says of a block or a
  transaction is taken.

**What each page is, with it on:**

| Page | With it on |
| --- | --- |
| Overview, Block map | From what was read, up to the newest final block read, with where the reading stands. No peers, mempool or live feed |
| Block | From what was read, or that one block fetched by itself. No reward, which no header says |
| Transaction, Hops | From what was read: what it did, its events and payments. Not why it failed, nor its hops, which no block commits to |
| Account | Transactions and events from what was read; balance and nonce by a direct proof. No contract's code, holdings or names |
| Events, Decoded | From what was read, read by the ABIs that ship with the app, and any pasted; what a contract would say of itself through a call is off |
| Finality | What the light client checked. Voters, signers and evidence are off |
| Names, Name | One name's record by a direct proof, said as the record it is kept as: the chain's ending, which the node gives in `/status`, comes off, so `pay.alice` typed without it is found too. The directory, prices and what a name resolves to once expired are off |
| Tokens | One token's record by a direct proof. The registry's list is off |
| Rich list & supply | Minted and burned by a direct proof. The rich list and what is staked are off |
| Send | As above |
| Tools | Decode, Find events (from what was read), Convert, Hash & verify, and a pasted proof work. Fetching a proof is off |
| Update, Settings | As ever: a release is checked against the keys built in |
| Mempool, Network, NFTs, DEX, Swaps, Bridges, Multisig, Governance, Mining pool, Faucet | Off, each saying why |

**The index.** It keeps each block but for its state entries and their proofs, which are most of what a
block weighs and which nothing here reads: its header and seal, its transactions and receipts, with their
events and payments, and its finality proofs. An empty block costs about 2 KB to download and about 400
bytes to keep. So what it keeps is what the common pages need, and all that
recomputes a block's roots, so what is read back from a device is checked again just as a node's answer
is.

- **It is bounded:** 128 MB on a desktop, about 18 days of the devnet's empty blocks, and 32 MB on a phone or in
  a browser, about 4, unless **Keeps at most** in its card says 16 MB, 32 MB, 128 MB, 512 MB or 2 GB.
  The cap counts the index as encoded; held in memory it takes about 3.2 times that, measured on a
  block a transfer each (`what_an_index_takes_in_memory_beside_what_it_holds_encoded`), so 128 MB
  is some 410 MB of memory, and 32 MB some 100 MB.
  A smaller cap than it holds asks first (**Yes, let them go**), and then lets the oldest go at once. Transactions weigh what they weigh, a deploy's code among them. Past the cap the
  oldest blocks go, 1,024 heights at a time, and the height it knows from moves up to say so. It is all
  in memory while the app is open. With no room left, reading further back says so.
- **It is on the device, private to the app,** in `explorer-index` in the app's folder, written a bucket of
  1,024 heights at a time as those change, and in IndexedDB in a browser, as localStorage holds about 5 MB.
  **It is not sealed,** as the explorer has no vault: it holds no secrets, and the setting says so.
  It lists blocks, not what was looked at: searches, pressed proofs and blocks fetched by themselves
  are kept nowhere, though what height it reads from shows.
- **Losing it costs a rescan.** A part that does not read or check is set aside as
  `….unreadable-…`, what is older goes, and it knows from what is left; with nothing left it starts again.
  The app says so.

**What it costs:** about 2 KB an empty block, so 34 MB a day on the devnet at 17,280 blocks a day, only
while the app is open, plus catching up on what was missed. The light client walks every header each time
the app opens, as fast resume and a node's own checkpoint are off, unless a checkpoint was pasted. Settings
shows what was downloaded and kept, and how far it has read.

**What it cannot do, in plain words:**
- **A gap.** A block that comes without proofs counts as unknown, not unchanged. As nothing here follows
  the state from them, that leaves nothing out of date: what shows of the state is a direct proof,
  with its block. A direct proof of a block nodes have pruned fails, and only the final block is asked.
- **Something left out.** A node can leave a state entry out of a block's list without being caught, and
  nothing here reads those lists. Receipts cannot be left out: they recompute `receipts_root`.
- **A block without its receipts,** or with its header alone, as a snapshot node has, is shown as what it is,
  and counted in Settings.
- **The coin's name.** Nothing the light client checks says it. It shows as a node last said on this
  device, kept for each chain, or else amounts show as plain units, not guessed at.
- **Privacy.** Reading ranges hides interest in transactions and state changes. A direct proof does not,
  nor does a block fetched by itself, nor who is asking, for which the [proxy](#through-a-proxy) remains.
  Sending goes through a node by nature.
- **Node-served pages.** A page that came from a server can be replaced by that server: if the page comes
  from the node, the node could serve code faking these checks, and the setting says so there. The desktop
  and phone apps do not have this hole.
- **A chain with no finality** has no final blocks to read, and says so.

**A node without the route.** An older node answers 400 for `/blocks/range`. The explorer says so, stops
reading, and asks nothing else. An answer that does not check stops the reading the same way, keeps nothing
of itself, and says what was wrong. 429 and 503 are waited out, as long as `Retry-After` says.

## The light client

The explorer does not have to take the node's word for balances. It runs the library's
[light client](light-client.md) against the node it talks to:
- **What it trusts:** the genesis it saw the first time it met the node, which it pins; or one
  given under Settings → Light client before it meets the node, **Trust it**, as someone you trust
  shows the genesis id, after which a node on any other chain is refused from the start.
- **Contract reads:** on the desktop and Android, every read it makes, a balance, a pool, a bridge's
  transfer, is run by the explorer itself over the state the node proves, and its outcome is what
  the pages show; a node that answered otherwise shows in the live feed. See
  [Proven contract reads](light-client.md#proven-contract-reads).
- **What it keeps:** what the client checked, saved by genesis id with the settings at most every
  10 seconds while it moves, so the next launch carries on from there rather than walk every
  header again, Read from checked blocks on or off. A checkpoint pasted still comes first; a saved
  state that does not restore for the genesis trusted is set aside.
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
- **Governance's page** proves each decision and rejection it lists from its receipt and
  transaction, with `GET /proofs/receipts/{tx}` (see [Governance](#governance)): the receipt
  decided it by the chain's count of decisions, or rejected it by the count of rejections, of the
  kind listed, and the transaction voted on that kind's change at the kind's version. For a decision in a block below the lowest header it holds, as
  after starting at a checkpoint, it first takes the headers down to it, which the hash chain
  proves.
- **The supply,** on the Rich list & supply page, and **any proof** on the Tools page's Proof tab,
  are checked against headers it checked itself, step by step. Without it following, each says it
  is the node's word alone (see [Tools](#tools)).

## Governance

The Governance page, under Network, shows a chain's [governance](governance.md) from
`GET /governance`, `GET /governance/history` and `GET /governance/rejections`. A chain without
governance answers 404, and the page says the chain has none.
- **Versions:** each kind of change (consensus, which also covers finality's tuning and a switch
  of its kind, bootnodes, names, collections, token functions, federation, directory, fees, reward, limits,
  upgrade, name and governance) has a ballot and a version of its own, how many of that kind were
  decided, which a vote names; deciding one kind leaves the others' votes standing, but a change
  of governance itself clears every open vote. The chain also counts every decision, and every
  rejection, of any kind, without gaps. The header shows the decisions, the rejections, what Yes
  needs, the quorum, how long a vote counts and how many changes are open, then the kinds decided
  at all, compactly: `By kind: upgrade 1 · name 2`.
- **Who votes:** the kind of governance, and for a council its members, each with its name where
  it has one. Under an electorate, finality's voters vote: validators by stake under FFG, signers
  one each under authority. Then what a change needs, the quorum (a share of every voter's
  weight that must vote at all, Abstain included, before Yes decides), when a change is rejected
  (once what has not voted No can no longer reach what it needs), how many epochs a vote counts
  for, the decisions of every kind and each kind's version, and a consensus or switch of finality
  decided for epochs ahead. **Weigh** looks up what an account's vote weighs now, and how it
  voted, each vote for its kind's next change, such as
  `Yes on rename the chain to third, for the name's change 3`, with
  `GET /governance/weight/{address}`.
- **Turnout:** a tile each voter, as many across as fit: its name where it has one, a bar of the
  share of the changes put to a vote that it voted on, its last vote (`last: Yes on name, 3 h
  ago`), and on a chain whose votes weigh stake, its share of the stake as a bar of its own.
  Turnout is counted from each voter's newest page of `vote_change` transactions
  (`GET /accounts/{address}/txs?kind=vote_change`), against every change any voter's pages name,
  each change told apart by its kind's version and its JSON; votes a multisig cast count for the
  multisig. With **Read from checked blocks** on, those pages come from the blocks read.
- **Open proposals:** a search box over each change's kind, words and JSON and who voted which
  way, an order (newest vote, most support, by how far Yes got toward what it needs, or closing
  soonest, by when the oldest vote stops counting), and a filter by the kinds there are. Each is
  a card:
  - a **ring** toward what Yes needs, its share in the middle, No a red run from the ring's end,
    and inside it a thin ring toward the quorum when there is one;
  - its kind, its version and the change it is voting on, such as
    `version 2: voting on the name's change 3`, and the change in words;
  - where it stands: `1 more Yes needed`, `Yes is enough; … more votes of any kind make the
    quorum`, or in red `Can no longer pass: too much has voted No`; then what Yes, No and Abstain
    weigh, and when the oldest votes stop counting, in blocks and time, from the chain's
    `checkpoint_interval` and block time;
  - a **dot a voter** from `voters`: Yes a filled dot, No a cross, Abstain a half-filled ring,
    not voted an empty ring, each labelled by name where the explorer knows one, its weight said
    on a hover;
  - **What it moves:** the setting as it stands in force beside what the change would make it,
    field by field, each field that moves struck through and highlighted, `burn permille 500 →
    600`, with how many stay; **Every field** shows them all. It works on any setting's JSON, so a
    kind the explorer has no words for diffs all the same;
  - each vote, its account, way, weight, the epoch it was cast in and the epoch it counts until,
    and the change's JSON to open.
- **In force:** every setting governance may change, governance's own included, summed up, with
  `genesis` while genesis's stands, or the change that last set it, a link to that decision.
- **Along the chain:** the decisions and rejections read so far on a timeline by block height,
  decisions above the line as dots and rejections below it as crosses, each colored by its kind,
  with a legend; a hover says which, its kind's version, and the change in words. Under it, a line
  of the decisions so far by block.
- **Decisions:** a search box over each one's kind, words, JSON, sender, transaction and block, a
  filter by kind, and newest or oldest first; ten to a page, **Newer page** and **Older page** to
  move, and older pages of `GET /governance/history` read as a page comes to need them, so a search
  goes on through all of them and says how far it got. Each says what it changed, its transaction,
  block, sender and time, and whether it is final or how many confirmations it has. A link from In
  force reads older pages until the decision is listed, and shows the page it is on.
- **Rejections:** the same, of `GET /governance/rejections`: each the change that could no
  longer pass, the kind's version it was rejected at, its transaction, block, sender and time.

**Each decision and rejection checked:** the light client asks for the proof of its receipt and
its transaction against the block it landed in, and it is **proven** only when:
- the receipt says it decided the chain's decision of that number (`Decided`), or made its
  rejection of that number (`Rejected`), of the kind listed, the history's `finality` and
  `switch_finality` being the consensus's kind;
- the transaction is a vote, its own or a multisig's, on the change listed, of the receipt's
  kind, naming the kind's version before the one a decision made, or the very version a
  rejection was at;
- the block and sender are the ones the node listed.

Until then it says **checking**, or **waiting** for a block the light client has yet to reach. A
proof that disagrees says **does not check**, and why, in red. With the light client stopped,
each is **reported by the node** alone.

**Gaps:** the chain's counts go up by one a decision and one a rejection, of whatever kind, so a
number missing between the lowest read and the newest is one the node did not list. The page says
so above its list. A node that synced from a [snapshot](node.md#syncing-from-a-snapshot) keeps
decisions from its snapshot on, and the page says from which.

With **Read from checked blocks** on, governance's own pages, `GET /governance`, its history, its
rejections and an account's weight, are the node's to answer, and the page says they need asking.

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
  built for: it downloads it through the same client and proxy as every request to the node, off
  the UI thread, the one request that goes to another host, and only when pressed, so reading from
  checked blocks lets it go. It checks its size and
  SHA-256 against the signed release. It writes it beside the running binary, then renames it
  over. It then says `Restart the explorer to run 0.2.0`.
- **Elsewhere,** in the browser, on Android and on iOS, or without a binary for this target, it
  says `Update it from where you installed it.`

## Pages

| Page | Shows |
| --- | --- |
| Overview | The chain's name, coin, hash and genesis. Height, block time, throughput, peers, mempool, finality, base price and the chain's coin. The newest checkpoint anchored in Bitcoin, and in Ethereum once the chain takes those, each with its epoch and where that chain holds it (not when reading checked blocks, as the node says it unproven). Under proof of work, the network's hashrate as difficulty over block time, and the node's own when it mines, its workers' included. Charts of the last 120 blocks: block times, transactions, gas used against the limit, and the base price or the difficulty. The latest blocks, and the live feed. |
| Block map | Every loaded block by height. The main chain runs along the middle, with finalized blocks green. Side branches sit in lanes above and below, each block joined to its parent. The tip and the justified checkpoint are outlined. Brighter blocks hold more transactions. |
| Block | The header, its roots included, seal and finality proofs, and every transaction with its status and fee. For a mining pool's block: what went to the pool's escrow and its percent of the reward and fees, the escrow, what the producer kept as the pool's fee, and the root its miners claim against, over how many miners. |
| Transaction | Where it landed, how deep it is and whether it is final. Its offer, what it used and paid, and what it returned or why it failed. Its hops, what its contracts paid out, its events, its fields and its signature. A claim says whose mining pool earnings it claims and from which of the pool's blocks, its `earned` being the miner's lifetime total, not what it paid. A governance vote says which way and what it changes, an upgrade as the nodes it stops: `an upgrade: nodes older than 0.2.0 stop at block 150,000`, and a rename as the new name: `Votes Yes on renaming the chain to chainv4 mainnet, for the name's change 5`, the change after the version of its kind it names. A cleared vote says the change it clears its vote on: `Clears its vote on new fee settings, for the fees' change 3`. The vote that decided its change is `decided`, and names the chain's count and its kind's version, `change 5, the upgrade's version 2`; the one that left it unable to pass is `rejected`, with the chain's count of rejections, each linking to Governance. The bridge gateway's events read as what each did, `transfer #4 from Base voided, refused here for good: it is refunded where it left` or `transfer #1 paid back, to ed25519:0x…`, each linking to the transfer, here and among an account's events. An Ethereum wallet's type-4, [EIP-7702](evm.md#eip-7702-delegations), says how many accounts' delegations it sets, and lists each authorization: the account that signed it, the contract whose code it now runs or that it cleared it, and its nonce. A transaction signed as EIP-712 typed data shows the function names it carried. |
| Account | The balance and nonce, and the balance the light client proved, at the finalized block and the tip. For a contract: its deployer, engine, code size and functions, the storage layout its code declares, a form to call any of them read-only, and its events, of one name or all. For a token: its name, symbol and decimals, from the token registry. For anyone: what they hold of the registered tokens, every transaction that touched them, with its type, parties, status and fee, narrowed by [filters](#an-accounts-transactions), and the events that mention them, token transfers and approvals in the token's units. Transactions need a node started with `--index-addresses`, events one started with `--index-events`. On a chain with names, the name the address goes by and the names it owns, every page of them. The multisigs its key is a member of, each with how many must agree, on a node started with `--index-multisigs`. An Ethereum account EIP-7702 delegated says whose code it runs as its own. |
| EVM contract | An [EVM](evm.md) contract's account page: its deployer and code size, and with its ABI pasted, kept on this device per chain, its read functions, run with `eth_call`, and its logs and calls read by name, here and on its transactions' pages. |
| Name | A [name](names.md), such as `alice.dev`: whether it is active, in its grace period or available, its owner, what it resolves to, when it expires and its grace ends, its text records, what a year of it costs, and for a subname, its name. |
| Network | The node's roles, uptime, listening addresses and sync state, its chainv4 version and how it stands against the newest release (`chainv4 0.2.0 is out`, or `0.2.0 installed, restarting` once it put one in place), any [upgrade](updates.md) that stops older nodes, decided by governance or announced by a release, with the blocks left to it from the tip, each [fork](governance.md#forks) the chain scheduled, from which block or epoch, how far off or in force, and what it changes, or that the node's release does not know it, its gossip backlog, and what it counted since it started. The chain's name, or that it gave none, and its bootnodes. When it mines: its hashrate over the last ten minutes, its own GPUs or threads, and a table of its workers, each with its address, when it connected, its hashrate, its devices and the seals it found; a pool's worker also with its payout address and the shares it sent. When it runs a pool: its fee, window and miners, and a link to the Mining pool page. A graph of its peers, and a table of them with what each gossiped, how much of that was dropped, and how far its chain got. |
| Mempool | What waits for a block, by sender and nonce. |
| Tokens | The token registry by symbol, with a search by how a symbol starts: each token's symbol, decimals, contract and name, and what makes a token on this chain. |
| NFTs | The [collection registry](nft-standard.md#the-collection-registry): each collection's name, kind, NFT or multi-token, symbol and contract. A collection's page looks a token up, its owner, or how many exist, and its token URI, as text: the explorer fetches nothing from where it points. It lists what an address holds of it, and what moved, from its `Transfer` or `TransferSingle` events on a node with `--index-events`. An account's page lists the NFTs it holds, and a collection's contract links to its page. An [EVM collection](nft-standard.md#evm-collections), ERC-721 or ERC-1155 on the chain's EVM, is read through `eth_call`, the node's word: a token's `ownerOf` and its `tokenURI`, or `uri` with `{id}` filled in; what an address holds from its logs naming the address's EVM form, each id checked by `ownerOf` or `balanceOf`; what moved from its `Transfer` or `TransferSingle` logs. |
| DEX | The [DEXes](dex.md)' pools: what each holds, its price and share supply, and what traded through it over about a day and a week, in its first asset, with the fees it kept. A pool opens its price chart, from its `Sync` events, its time-weighted price over about the last hour, read at two blocks ([DEX](dex.md#in-the-explorer)), and its latest trades, adds and removals, each linked to its transaction. **Native pools** come from `dex.lua`, by read-only calls and its events; **EVM pools** from the router the node names, through `POST /rpc` and the pairs' logs. |
| Bridges | The [bridges](bridges.md): the federation in force and the chain's [directory](governance.md#directory). Transfers through the gateway, out and in, each with the board's count of signatures and who signed, and a page of its own; the wrapped assets it made, with what each stands for. Each transfer says where it truly stands, in the words and colors the wallet uses: `signing, 1 of 2`, `signed, to deliver`, `arrived`, `voiding, 1 of 2`, `void signed, to deliver`, `voided, refund to come`, `refunded`, or `stuck: its peer is gone`, from its `Sent`, `Attested`, `Received`, `Voided` and `Refunded` events in whatever order they come, and tiles count the refunds and the stuck. Signatures to void or refund a transfer are put with it, not listed as a transfer of their own: for one that left, the explorer reads it back with `sent(nonce)` and works out its void and refund digests, as the signers do; for one voided on its way here, it asks the gateway's `void_digest`. One that left is stuck when its digest is not the one worked out toward the Gateway.sol the directory names there now: it left for a peer since replaced, which no signer signs or voids. A transfer's page says when it was voided or paid back, in which transaction and to whom, and lists the signatures to void and to refund it apart from those to deliver it; a digest signed to void or refund one opens it too. Neither event says why a transfer was refused: an operator's `--why` stays with its own tool. A refund of one voided here is paid on the EVM chain it came from, which the explorer does not read, so it stays `voided, refund to come`; the signatures to refund it are signed over Gateway.sol's `leftBy`, so the explorer places them by the `attest_refund` call that posted each, which names the transfer, and lists one refused before it ever showed here from its `attest_void` calls. Bitcoin addresses are written for the network the relay follows: `bc1q…`, `tb1q…` or regtest's `bcrt1q…`. The Bitcoin gateway: its relay's tip, wBTC minted against the bitcoin held, held outputs, deposits, withdrawals and who asked, each with the most it may lose to fees or its share once in a release, and its releases, each with a page of its inputs' signatures and its outputs. |
| Swaps | Every [atomic swap](swaps.md) in the swap contract, newest first, from its events: what was locked, for whom, until when, and whether it was claimed or refunded, each lock with whether it is final yet. A swap is found by its id, as the contract holds it, by its hash, which every lock under it among the recent ones shares, or by its code, `ur:chainv4-swap`, which lays out both legs, the other chain's too. A claim whose payout was refused shows as owed. |
| Mining pool | The pool the node runs, on one started with `--mining … --pool`: its fee, window, miners, escrow balance and all it credited; its producer and escrow; every miner with its part of the window's work, of the next block, and what it earned, claimed and may still claim; the blocks it sealed lately, each waiting to be final, credited, or dropped by a reorganization, with what it shared out among how many miners. A miner's payout address looks up its own books: its workers, and the blocks that paid it and how much, and its newest claim (`GET /pool/claim/{address}`) proven by the light client: the root its path leads to and what it claimed, both proven at the tip, the figures the chain holds, and a warning when the node's say otherwise. A node without a pool says how to run one. |
| Multisig | A [multisig](multisig.md) looked up by its `multisig:0x…` address, from `GET /multisig/{address}`: its members, each with the account its key controls and its Bitcoin xpub and EVM address where it names them, the threshold, its nonce and balance, and the proposals open for that nonce, each with its approvals against the threshold. A multisig's account page links here. See [Multisig](#multisig). |
| Names | The name directory from `GET /names/list`, sorted by name, 50 to a page with **More**: each name's status, owner, what it resolves to and when it expires. A search opens a name's page. See [Names](#names). |
| Rich list & supply | The richest accounts from `GET /rich`, each with its share of the supply, and the supply from `GET /supply`, its minted and burned proven by the light client. See [Rich list and supply](#rich-list-and-supply). |
| Tools | Decode, Convert, Hash & verify, and Proof, in tabs. See [Tools](#tools). |
| Finality | The kind, the epoch, and the justified and finalized checkpoints. Who votes and with what weight, the bonded validators and where each one's rewards go, into its bond or its account, the signers under authority finality, and any conflicting votes the node caught. |
| Governance | Who votes on changes, what one needs and the quorum, each voter's turnout, the changes being voted on as cards with their votes and what each would move, every setting in force and the change that set it, the decisions and rejections along the chain, and every decision and rejection, each proven by the light client. See [Governance](#governance). |
| Send | Signs and sends transfers, contract calls, deploys and burns, for the chain the light client follows: until it says which, it says so and signs nothing. |
| Settings | The node, whether it answers and how updates arrive, the chain it is on, the proxy every request goes through, Low data and Wait for answers, dark or light, and the genesis the light client pinned there, which it can forget. |
| More | On a phone, the pages the tabs have no room for. |

**Faucet,** under Tools, on a node that runs a [faucet](faucet.md): coins for an address, an
Ethereum `0x…` or a name, its proof of work worked out a slice a frame.

A transaction that is a [batch](chain.md#batches) lists every step under What it asked, numbered, with
what came of it: what it returned or deployed and how many events and payments are its, or undone,
failed with why, or not run. Its hops are its trace, or a step each with no contract to trace.

## Tools

The Tools page, under Tools in the sidebar and in More on a phone, has four tabs. All but
Proof's fetch work offline.

**Decode** reads what is pasted:
- **A raw signed transaction,** `0x…` as the chain encodes it, or the wallet's code for one,
  `ur:chainv4-signed/…`: its kind, id, with a link to its page, sender, nonce, offer, signer and
  signature, and whether the signature checks on this chain, or that the chain is not known yet, as a
  signature names its chain; an EVM call's calldata read by the
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

A multisig's [batch](multisig.md#a-batch), proposed or sent, shows its steps as any batch does.

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
- **A sealed key file** (`keygen --seal`, see [Sealed key files](crypto.md#sealed-key-files)),
  pasted or picked: it waits in the key field for its passphrase, then **Use it**. The passphrase
  and the key's text are wiped once used.
- **What egui made of it:** the explorer's allocator zeroes every block of memory as it is freed,
  so the copies egui makes of a key as it is pasted and laid out are wiped as they go, and a crash
  writes no core file, as in the wallet (see [Wallet](wallet.md#keys)). The default `wipe-freed`
  feature; leaving it out makes a faster build.

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

The node address, the proxy, Low data, Wait for answers, the cap on what reading checked blocks keeps, dark or light, any token decimals set by hand, the nodes trusted for
fast resume, whether to read from checked blocks, the coin's name each chain's node last gave, and the
genesis the light client pinned and the chain's name last seen for each node. What it read from
checked blocks it keeps apart (see [Read from checked blocks](#read-from-checked-blocks)). It keeps them where eframe keeps app data: the platform's app-data directory
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
- `org.chainv4.ui.Picker`, the `ui` crate's Java activity for picking files, in the committed
  `ui/android/classes.dex` the build adds (see [Picking files](wallet.md#picking-files));
- `explorer/assets/icon.png` for its icon.

**CI:** `.github/workflows/android.yml` builds its APKs beside the wallet's, keeps them as
artifacts named `chainv4-explorer-android-ARCH`, and a run started on a `v*` tag attaches them to its
release.

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
