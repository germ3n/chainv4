# Good to know

The things that surprise people, and the rules that keep every node on a chain in step. The
other pages explain each part in full; this one is the short list to keep in mind.

## Batches

- **A failed batch undoes every step and still pays.** It uses its nonce and pays for the fuel its
  steps burned, the ones undone too. Which step failed is proven with its receipt; why is not. See
  [Batches](chain.md#batches).
- **A batch's steps share one fuel limit.** Step 2 gets what step 1 left, across engines. A contract
  that is expensive only at the end fails all of it, at a whole gas limit's cost when it runs out.
- **Its deploys have their own nonces.** Only the first lands where a lone deploy at that nonce would;
  work out the rest with `batch_deploy_nonce`, or read them from the wallet or the CLI before signing.
- **Each EVM step is its own transaction to the EVM.** Transient storage does not survive from step to
  step, nothing stays warm, and a contract made in step 1 is not new to `SELFDESTRUCT` in step 2.
- **MetaMask cannot sign one.** EIP-712 would show an opaque hash, so the chain refuses that signature.
- **A batch costs a transaction's `per_tx` for each step.** Batching saves signatures and nonces, not
  gas.
- **Old nodes fork on it.** A node older than batches cannot read a block holding one, so every node of
  a chain upgrades together before anyone sends one. Old apps show it as "Does not check", or a signer
  refuses the code; none of them misreads it.
- **Bytes can nest a transaction only so far.** A transaction decodes at most two levels below the
  outermost, a multisig's batch's steps. Past that, decoding stops: it used to run out of stack.

## Running a chain

- **A sender's waiting transactions share its balance.** The mempool adds up the fee reserves, gas
  limit times max price, of all a sender has waiting: one more past its balance is `cannot pay`,
  however many it sent before were each payable alone. And a full mempool pushes out the least paid
  whoever's next it is, with that sender's later ones.
- **A pruning node cannot sync others from genesis.** With `--prune-blocks`, blocks below
  `bodies_from` keep their headers alone: peers sync their bodies from another node, or start from
  a snapshot, and their transactions are not found there. Pruning never comes undone.
- **A sealed key file needs its passphrase wherever it is read.** On a server, give the node
  `--key-passphrase-file PATH`, or set `CHAINV4_KEY_PASSPHRASE_FILE` for any command: a node with
  neither, and no terminal to ask on, stops. Lose the passphrase and the key is gone; nothing gets it
  back but the words it came from, if it came from words. See
  [Sealed key files](crypto.md#sealed-key-files).
- **A wallet vault moves to the stronger stretch as its password opens it.** Once it has, an older
  wallet build says the vault is damaged rather than open it, and leaves it as it was; a reading
  index sealed under the old key is rebuilt once.
- **Old `chain.db` files are migrated from format 8 on.** A node moves an older database up to its
  own format as it starts, after copying it aside as `chain.db.format-N`; going back is
  `chainv4 db migrate --to N`, which asks first (see [Migrations](storage.md#migrations)).
  Databases from before format 8 are still refused: delete them, or point `--db` elsewhere. A
  node stops with:
  - `WrongGenesis` on a database written for another genesis. Switching `--consensus`,
    `--genesis` or build features does this, and so did genesis gaining its coin and its
    `token_functions`;
  - `OldFormat` on a database kept in a storage format (`STORAGE_FORMAT`) no migration reaches,
    such as every database from before format 8, or one in a newer format;
  - `Migrating` on one a migration stopped part way through, which `chainv4 db migrate`, or
    starting the node, finishes.
- **There are two dev chains, and pow is the default.**
  - `--consensus pow`, mined with `--mine`;
  - `--consensus authority`, produced with `--produce` on exactly one node;
  - `--consensus rotating` and `--consensus staked`, produced with `--produce` on each signer's
    or validator's node, the development key being the only one on the dev chains.

  Each is its own genesis and network.
- **The development key is public.** Its seed is in the source. It holds a million coins on both
  dev chains, may mint, produces the authority chain, is the dev chains' one FFG validator, and
  signs for `chainv4 send` without `--key`. Never put anything of value on a dev chain.
- **The dev genesis depends on the build.** The dev chains allow every engine the node was built
  with, hash with BLAKE3 when it is built in, and run FFG only when it is built in. Nodes built
  with different features are on different dev chains and never meet.
- **The dev chains charge fees.** A base fee starts at 1 gwei of gas, with half of the base part
  burned. An account needs coins to send anything, the development key included. `chainv4 send`
  works out the offer from `GET /status`. `chainv4 genesis --fees free` writes a chain without
  fees.
- **Nothing finalizes until someone validates.** Both dev chains run FFG with the development key
  as the one validator, epochs of 8 blocks. Without a node running `--validate`, the chain runs
  as if it had no finality.
- **Every chain parameter is in genesis.** That covers:
  - consensus and its timing;
  - hashes, rewards, allocations, the minter and engines;
  - transaction limits and the reorg depth;
  - finality and fees.

  `chainv4 genesis` writes the dev defaults, and `--genesis FILE` runs a file. Files must name
  every field and may not add any. See [Genesis](genesis.md).
- **A voting key belongs on one node.** What a node has voted is kept in its own database. Two
  nodes voting with one key can sign conflicting votes, and under FFG get it slashed.
- **The binary is also the client.** `chainv4 send` waits until its transaction is in a block
  (`--timeout`, `--confirmations`, `--final`, `--no-wait`). It exits non-zero when the
  transaction landed but failed.
- **The node's peer id changes on every start.** Its libp2p key is generated fresh each run.
- **The chain keeps a registry of tokens.** A contract that exports genesis's `token_functions`,
  ERC-20's six on the dev chains, is registered as it deploys, with its name, symbol and decimals
  if it exports them. `GET /tokens` lists them by symbol.
  - **Bad answers fail the deploy:** a token whose `name()`, `symbol()` or `decimals()` fails or
    gives something out of bounds does not deploy.
  - **Symbols are not unique:** trust an address, not a symbol.
  - **A token that changes its name asks for a refresh:** `tokens.refresh()` in Lua,
    `refresh_token()` in Rust. Without one, the registry keeps the old answers.
- **A decoded call is a match, not proof.** The ABIs built in (`src/abi/known.rs`) only say the
  bytes fit a signature: any contract can have a `transfer(address,uint256)`, or emit a `Transfer`.
  So:
  - **Say what read it:** `read as ERC-20`, or `read by the ABI kept for this contract`, which always
    goes first. `approve` and `transferFrom` fit both standards, and say both.
  - **Never instead of the facts:** a signing screen keeps To, the value and the data's selector and
    size as they were, and shows what was read below them, in a frame of its own.
  - **It is untrusted text:** go through `ReadArg::pieces` and `clean_text`, which cut it short and
    escape line breaks, direction overrides and invisible characters. Names from a pasted ABI come
    out as letters and digits alone.
  - **A log's token move is a claim:** unless the registry lists the contract it never says it
    happened, and nothing adds moves to a balance.
  - **What an unlisted contract says of itself stays on the Activity page:** `views/activity/unlisted.rs`
    is private to it, which is what keeps it from signing screens, balances and the Overview, so do
    not hand it to `Ctx`. It is not stored either, as settings that fail to decode lose everything.
    Go through `Said::amount`, which always writes the quotes and `(unverified)`. The amount's column
    wraps that text, because `theme::fitted` truncates past its smallest size, and what it would cut
    is the label.
  - **Paced by the page, not by `Node`:** `Ask::Rpc` has a body, so `Node::ask` sends it whenever it
    is asked, and frames come often. `Unlisted::next` sends one question at most every 400 ms, and a
    429 from nginx only delays them. Anything new that polls `/rpc` needs its own pace too: the limit
    is 5 a second per address, shared with the Trade page and sites.
  - **Decode for people with a budget:** `decode_values` lets dynamic values share bytes, so a small
    `bytes[]` of offsets at one long value decodes to a thousand times its size. `known` spends a
    budget; contracts, whose path is consensus, decode as they always have.
- **Rust exports take typed parameters.** `#[export] fn transfer(to: Address, amount: U256) ->
  bool` reads the call's arguments into its parameters and refuses a call whose arguments do not
  fit, naming the parameter. A function that takes and returns nothing still reads `input()`
  itself.
- **Events are searchable on nodes that index them.** `--index-events` indexes every event by its
  contract, its name, the addresses in it and an EVM log's topics, for `GET /events`.
  - **An EVM log is called `log`,** in those letters, and holds its topics and data as bytes: the
    index compares names by bytes, so asking for `LOG` finds nothing and says nothing. An address
    in a log is found by its topic alone, and an unindexed one in its data not at all.
  - **Topics came later:** a node opening an older index empties it once and reads it back in the
    background, so older events are missing until `catching_up_to` clears. It is no consensus
    change. An older node ignores `topic=` beside `contract=` and lists everything: check that the
    answer has `topics: true`.
  - **Which blocks:** `--index-events` alone keeps every block's events. `=5000` keeps them from a
    height, and `=latest-100000` keeps a window that moves with the tip.
  - **Older blocks:** the node reads them back from their receipts in the background, newest
    first. `indexed_from` says how far it got.
  - **Staying on:** once on, the database keeps it on, keeping the same blocks until the flag
    says otherwise. `--index-events=off` deletes it.
- **Histories come from the address index.** `--index-addresses` keeps every transaction under
  each address it touches: its sender, a transfer's parties, the contract called and every
  address among the call's arguments, and a deploy's contract. It takes the same `FROM` and
  `=off` as `--index-events`, on its own: either can be on without the other. A wallet's
  activity, `chainv4 light history` and the explorer's account page list from it. Coin a contract
  pays out shows only in the trace, so not here.
- **Tokens keep balances where light clients can prove them.** The token standard puts each
  holder's balance in the `balances` namespace under the holder's address text. A token that
  keeps them elsewhere still works, but no wallet can prove what it holds.
- **Floods are turned away at the door.** Gossiped blocks, transactions and votes, and submissions,
  count against a backlog: 10,000 in all and 1,000 from any one peer by default
  (`--max-gossip-backlog`, `--max-gossip-per-peer`). Past it they are dropped, `POST /txs`
  answers 503, and a peer over its share 10 times in a minute is disconnected. It can come back
  at once: there are no bans yet (see [Node](node.md#the-gossip-backlog)). A block counts for its
  sender's share too, so a peer streaming blocks whose parent the node lacks cannot make it decode
  and queue them without bound; the slot is let go once the node has judged the block.
- **A cheap request is not verified before it is priced.** A submitted or gossiped transaction has
  its nonce and, on a fee chain, its balance checked against the tip before its signature is
  verified, and a multisig's members' signatures only after that: a flood from a fresh, unfunded
  key costs no signature checks, and the same member's signature sent many times over costs one.
- **A node only streams when asked.** `--stream` turns on `GET /stream`. Without it, clients
  such as the explorer and the wallet poll.
- **The apps' stream only says when to ask.** A wallet and the explorer's light client take from it
  that the chain moved, never what is on it, and ask over HTTP for a header, a block or a
  transaction as ever, proving what comes. A node that lies on its stream can make them ask early, or
  late by up to 20 seconds, the slowest they ask with a stream open. Each open app holds one connection to
  the node, and a phone in the background keeps it until the system ends it. A stream that says
  nothing for 40 seconds is taken for dead, as the node says something every 15.
- **ABI decoding is limited by consensus.** `decode_values` makes at most twice what it is given and a
  little, as fuel pays only for the input and dynamic values may share their bytes: 328 KB of offsets at
  one long value came to 537 MB, outside any VM's memory limit. It is a rule every node must apply alike,
  so it goes out to all nodes at once.
  - **A fixed array's length is capped where a type is parsed,** at `u32::MAX`, so measuring a static
    type's size cannot overflow a pointer, which once wrapped on a release node and panicked on an
    overflow-checked one, and so every node reads the same types whatever its pointer width. Far above
    any real ABI; a longer one is refused, not decoded.
- **Read-only calls hold up the API.** `POST /call` runs a contract under the chain's fuel cap on
  the API's one thread, and every other request waits for it. Behind nginx, an address gets 5 a second; see
  [Docker](docker.md#what-is-exposed).
- **Lua contracts can be split across files, put together when deploying.** A `--@include
  "lib.lua"` line is replaced by that file, sharing the includer's scope, before the transaction
  is signed: the chain holds one source, and the 128 KiB limit is the whole one's. `send` checks
  it compiles first and names any mistake by file and line. See
  [Contracts](contracts.md#splitting-a-contract-across-files).
- **The explorer signs where it runs.** Its key stays in the window, desktop, browser or phone, and only
  signed transactions go to the node. Serve the web app from the node itself with
  `--explorer explorer/dist`, or allow the page's origin with `--cors`. See [Explorer](explorer.md).
- **The wallet keeps its keys sealed on the device.** A password seals them, and nothing opens
  them without it. Its accounts come from 12 seed-phrase words, which bring them all back on any
  device: write them down. In the browser they live in the page origin's storage, which clearing
  the site's data deletes. It trusts the genesis a node first shows it, and proves everything
  else. `--wallet wallet/dist` serves it from the node's `/wallet/`. See [Wallet](wallet.md).
- **One account, four addresses.** A seed-phrase account has an Ed25519, an EdBlake, an
  Ethereum (secp256k1) and a Bitcoin address, each with its own balance and nonce. Coins at one are not at the
  others: the wallet adds them up on Home, but a transaction is sent from one address.
- **secp256k1 addresses are Ethereum's.** `secp256k1:0x` and 20 bytes, the same as Ethereum's for
  the same key, and the same words give MetaMask's accounts. But chainv4 signs its own
  transactions, so an Ethereum wallet can hold the key yet not sign for chainv4, and a
  secp256k1 address cannot be checked as a curve point: a mistyped one takes coins for good.
  See [Crypto](crypto.md#keys-and-signatures).
- **A wallet page sends from the address picked, not the first.** Send, Bridge, Trade, Swaps and
  Batch share one From picker, and Activity lists every address's together, each line saying which
  address it is for. A transfer between two of the account's own addresses shows as **Between your
  addresses**, with what moved and its fee; before, it showed as a bare Transfer, as if nothing had
  moved. An NFT sent from one of its addresses to another shows at the one it went to, by its proof.
  See [Wallet](wallet.md#sending).
- **The review reads a contract by what the registries prove it is.** `transfer(to, n)` is an
  amount only to a proven token: to a proven NFT collection it sends NFT #n, to a multi-token one
  `transfer(to, id, n)` sends n of #id, and to a contract proven as neither it is a call whose
  numbers the wallet does not name. The wallet proves each listed collection's entry, as it proves
  tokens'. See [Wallet](wallet.md#sending).
- **bitcoin addresses are Bitcoin's.** `bc1q…`, alone, with no `kind:` before it: the HASH160 of a
  key at BIP84's `m/84'/0'/0'/0/n`, the address Bitcoin's wallets show for the same words. Every
  chain writes it with mainnet's `bc`. The key signs chainv4's transactions over double SHA-256,
  so it is a scheme of its own, `bitcoin`, a consensus change: nodes without it refuse those
  transactions. Text that tells addresses from names now parses, since a `bc1q…` has no colon.
  See [Crypto](crypto.md#keys-and-signatures).
- **The Android apps are built with xbuild, not Gradle.** `wallet/build-android.sh` and
  `explorer/build-android.sh`, both through `ui/build-android.sh`, need clang, lld and llvm-ar, and
  no NDK. Without a key they sign with xbuild's debug key, the same for everyone: Android updates
  an app only with one signed by the same key, so pick your own key (`ANDROID_SIGNING_PEM` in CI)
  before anyone installs them. They need Android 7 or newer.
- **A keyboard's Paste comes as Ctrl+V, and winit drops the Ctrl.** A NativeActivity offers the
  keyboard no InputConnection, so Gboard's Paste can only send key events, and it sends Ctrl+V. winit
  on Android tracks no modifiers: it looks the key's character up with the Ctrl held, finds none,
  falls back on its table of letters, and tells egui of a plain `v` typed, text and all. Nothing
  egui hears tells that from a real `v`. `ui/src/android.rs` therefore defines
  `AKeyEvent_getMetaState` itself: the linker takes it over libandroid.so's for winit, it calls the
  real one through `dlsym`, and notes each letter that went down with Ctrl held, and
  `KEYCODE_PASTE`, which egui-winit answers from a clipboard of its own, empty on Android. The
  frame's `raw_input_hook` then swaps the letter for what it meant (`edit::ctrl_key`). If winit ever
  stops asking for the meta state on each key, Paste goes back to typing `v`: check it after
  upgrading winit, on a phone or with `adb shell input keycombination KEYCODE_CTRL_LEFT KEYCODE_V`.
  Desktop and web are untouched: their clipboard shortcuts go through egui-winit and the browser.
- **The phone's edit bar keeps its own hold.** egui's long touch takes 0.8 seconds and ends if the
  finger moves 6 points, which a finger does just by rolling, so the bar often did not come. `edit`
  watches the press itself: half a second, 20 points of slack. A hold under egui's 0.8 seconds counts
  as a click to egui when it lifts, so the press that brought the bar up is kept from taking it down.
  The bar goes by the page's own scroll area for where the page ends, as noted by
  `layout::keep_focus_in_view`, as the screen's bottom would be under the tabs.
- **Picking files on Android takes Java.** Android hands a picked file only to Java code, so the
  build compiles a small activity, `ui/android/Picker.java`, into the APK with a JDK and Android's
  build-tools, and signs the APK again with the same key. Without them it builds an app that cannot
  pick files, and leaves its buttons out; pasting needs no Java and always works.
- **The wallet and the explorer share one look, in the `ui` crate.** `ui/` holds the palette, the
  fonts, the widgets, the sidebar, bars and tabs, and the Android glue; each app picks only its
  density, and keeps its own pages. A change to a shared widget shows in both. The explorer's chart
  colors (`Roles`) are its own, as the palette's gold accent and amber warning look alike: a test
  holds them apart under simulated color blindness, in both themes. See
  [The ui crate](development.md#the-ui-crate).
- **A button lays its text out the way the row around it runs.** egui aligns a button's text by the
  surrounding layout, so a button wider than its text keeps it at the left in a left-aligned column.
  `widgets::primary` sizes itself from outside, as `add_sized` centers. The sidebar's entries count
  on the same behavior to stay left.
- **A grid's cell keeps the width text first wrapped to,** next to nothing, which is why the
  explorer's In force table gives what a setting is its width outright rather than wrap in the cell.
- **The Paste bar comes only when asked for:** a hold, or a tap on a field already focused, never the
  tap that gives a field focus, and it goes at the first key. The clipboard is read only while the
  bar is up.
- **A status dot's words come on a tap,** as a tooltip does not stay on a touch screen, and the
  wallet's dot goes by `GET /status`, not by whichever page's request failed: the banner speaks for
  those. It turns hollow as well as red when something is wrong, to read without color.
- **Docker or Podman, by `CONTAINER_ENGINE`.** `docker/compose.sh` and `docker/pow.sh up` run
  either; rootless Podman needs the unprivileged port bound lowered to 80, or `HTTP_PORT` and
  `HTTPS_PORT` above 1024. See [Podman](docker.md#podman).
- **`--peer` is an address, dialed until it answers.** Nodes take a new peer id every start, so
  give `/ip4/…/udp/4001/quic-v1` with no `/p2p/`, and a name looked up first: the node dials no
  `/dns4/`. A peer with no connection is dialed again every 15 seconds. See
  [Docker](docker.md#a-proof-of-work-chain-across-machines) for a chain across a VPS and home.
- **A light client trusts a checkpoint as it trusts a genesis.** One pasted, or `--checkpoint`,
  starts it there without checking what came before; the apps take a node's own only the first
  time they meet it, with no genesis given. Nodes still sync from genesis or `--snapshot`. See
  [Starting at a checkpoint](light-client.md#starting-at-a-checkpoint).
- **A secp256k1 key signs chainv4's own form or EIP-712's.** Nodes check both, the second bound to
  the chain by its genesis id (`SignedTx::verify_on`), so MetaMask and hardware wallets sign for
  the browser wallet, as chainv4's own is bound by signing the genesis id. See
  [Typed data (EIP-712)](crypto.md#typed-data-eip-712).
- **An offline signer has no network client at all.** The wallet's role is picked once per
  install. A signer's requests fail before they start, and it polls nothing, nor does a new install
  before it picks its role. Transactions travel as QR codes: Uniform Resources, `ur:chainv4-…`, animated when long. See
  [Signing offline](wallet.md#signing-offline).
- **The apps' proxy fails closed.** A proxy set under Settings in the wallet or the explorer
  carries that app's every request, on the desktop and Android, and while it is down nothing goes
  out. `HTTP_PROXY` and `ALL_PROXY` apply only while none is set, and `NO_PROXY` never exempts a
  host from one that is. SOCKS5 always resolves names at the proxy, so `.onion` nodes work through
  Orbot. In the browser, the browser's own proxy applies. See
  [Through a proxy](wallet.md#through-a-proxy).
- **Docker puts HTTPS in front, not around peer-to-peer.** nginx terminates TLS for the API,
  the explorer and the wallet; QUIC is TLS 1.3 already, so the p2p port goes straight to the node.
  A chain made from the presets signs with the development key until `docker/config` holds keys
  of your own. See [Docker](docker.md).
- **A restored light client takes old headers on demand.** It keeps headers from the finalized
  block on; to check something older, `deliver_ancestors` takes the headers between, proven by
  their hash chain to the lowest it holds.

## Consensus-critical: changing these makes a new chain

Every node on a chain must agree on all of these. Change one on purpose, never by accident.

- **Engines.** Every node must be built with the engines its genesis lists. A node missing one
  refuses to open the chain rather than fall out of step.
- **wasmi is pinned to `=2.0.0`.** Its version, enabled features and stack limits decide what a
  module does and costs. Upgrade on purpose, never with `cargo update`.
- **The Luau fork.** The germ3n forks of mlua and Luau meter fuel. mlua is pinned by `rev`, and
  Luau, which mlua-sys takes from its fork's main, by the commit in Cargo.toml's
  `[package.metadata.consensus]`: `tests/toolchains.rs` fails when `cargo update` moves either.
  Never swap them for upstream. Its comparisons are consensus too: a number against a userdata,
  and two userdata of different types, ask the userdata's metamethods, where plain Luau would
  fail or call them unequal (see below). So are its memory limit and its collector's pacing,
  below.
- **Luau compiler settings.** Optimization level 1, debug level 1, no type information. The
  bytecode, and so every call's fuel, depends on them.
- **Fuel costs** (`vm::cost`) and every limit: call depth, key and value sizes, event sizes,
  module and source sizes.
- **Encodings and tags.** Everything is encoded with wincode, every enum's tags are pinned, and a
  new variant takes the next unused tag. Never reuse or renumber one.
- **Byte order.** Everything is little-endian, with three exceptions on purpose:
  - the work hash is read as a big-endian number;
  - heights and epochs in database keys are big-endian, so they sort;
  - XXH3 digests are big-endian as bytes, matching `xxhsum`.
- **Hashes are chosen in genesis.**
  - `Genesis.hash` is the chain hash: block and transaction ids, roots, contract addresses and
    gossip ids.
  - The proof-of-work config names its own work hash.
  - Both must be secure: sha256, blake3, blake2s, blake2b or keccak256. XXH3 only names contract
    functions.
  - A header hashes itself with its parent's algorithm, so ids from chains with different hashes
    never connect.
- **Kinds of finality, scorers and pricings.** A node built without the kind of finality its
  genesis names refuses the genesis file.
- **The state root's tree, and what a receipt commits.** Paths, leaf and branch hashes, and which
  columns count (`STATE_COLUMNS`) decide every `state_root`; `CommittedReceipt`'s fields and the
  RFC 6962 split decide every `receipts_root`, and each `SignedTx`'s encoding and the same split
  every `tx_root`. A new chain-state column must join `STATE_COLUMNS` on purpose, and so becomes a
  new chain. The address index's column does not: it is a node's own.

## The node on the network

- **Single-transaction gossip is gone.** Transactions travel in batches on `/txs-batch`; a node
  from before speaks `/txs`, which no node reads now. Update every node together.
- **A batch is never forwarded whole,** so a transaction a node refuses goes no further than it,
  and one heard twice costs nothing the second time.
- **The API's limits are per client, behind a proxy too:** 64 requests in flight, then 429. Raise
  `--http-per-ip` for a load generator on one machine.
- **Bench from another machine for real numbers:** a laptop's loopback is far faster than the
  internet, and `bench load` itself takes cores the node would have.

- **A node keeps its peer id.** Its network key is `net.key`, beside the database's directory
  (`/data/net.key` in Docker, in the chain's volume), made at the first start. Delete it and the
  node takes a new id, which peers that pinned `/p2p/<id>` no longer reach. A file that does not
  read stops the node instead.
- **Gossip waits for the node's verdict.** Nothing is passed on before it is checked, so an
  invalid block stops at the first honest node. A node that is still syncing from a snapshot
  ignores gossip, and so passes none on until it is done.
- **Bans are by peer id unless `--ban-ips`.** Ids are free to make, so a determined peer comes back
  under another one; `--ban-ips` stops that, and also refuses honest nodes behind the same NAT or
  VPN. Bans end after `--ban-for` and with a restart.
- **Being honest costs nothing.** A block whose parent is missing, one already had, one a little
  from the future, a nonce used or a fee too low: dropped, never scored.
- **Remembered peers are dialed like bootnodes,** while the node is short of peers, at most once a
  minute each; a peer that changed its address is found again through the others.
- **Every commit is synced.** It costs a few milliseconds a block; `--no-fsync` for benchmarks.
- **Receipts commit to payments now:** the receipts root of every block changed shape, a new
  chain's consensus; receipts a node stored before read back with none.
- **A payment is a contract's, never the sender's:** a transfer or a call's value is the
  transaction's own and shows in it; payments are what contracts sent on.
- **Iterating storage is paid a page at a time:** 100 fuel a page of up to 64 from the iterators,
  plus 40 an entry; a loop over a list anyone can grow gets dearer as it grows.
- **A transaction that iterates may run twice** in a block whose transactions run side by side:
  when one before it writes anywhere in the range it went through.
- **The API answers from a snapshot per request,** so a reply never mixes two tips; a slow request
  keeps the files it reads on disk until it ends.
- **`--db-cache` sizes the database's block cache,** 256 MiB by default, shared by every column:
  give it more on a node whose state outgrows memory.
- **One key keeps at most 64 transactions waiting** (`--mempool-per-sender`), each at most 64
  nonces past its account's (`--max-nonce-gap`). A faucet, pool or relayer key of the node's own
  that sends more a block needs them raised; `chainv4 bench load` funds its senders 32 at a time,
  each lot waited on, for the same reason. A 65th is refused with 429, `too far ahead` with 400.
- **A transaction at a nonce already waiting replaces it** when it raises both its max price and
  its tip by 10% (`--replace-bump-percent`); less is 409, which wallets read as Ethereum's
  `replacement transaction underpriced`. A chain without fees replaces nothing.
- **A full mempool makes room for a better offer:** it pushes out the least paid transaction that
  is not its sender's next, with the rest of that sender's after it, what waits behind a gap
  first. Its sender sees nothing: the transaction is simply gone, and is sent again or not.
- **A nonce nobody sends drops what waits behind it** after 64 blocks (`MAX_GAPPED_BLOCKS`), five
  minutes of the devnet's 5-second blocks.
- **Sync answers stop at 8 MiB of blocks** (`--sync-batch-bytes`), past their first block, and the
  asker asks again from where one ended. Before, a run of big blocks could pass the 64 MiB a
  response may be, and a node asking for them never got them. Nodes old and new sync from each
  other: an older asker takes a shorter answer as it always did. Only an older node still answers
  with 128 however big, so big blocks asked of one can still fail.
- **Every limit the node fixed is a flag or a `--tune` key now,** each defaulting to what every
  node did before, so a node given none behaves exactly as it did: `--max-peers-in`, `--ban-score`,
  `--ban-window`, `--max-strikes`, `--strike-window`, `--tor-timeout`, `--http-rate`,
  `--api-max-page`, `--suggest-multiplier`, `--update-every`, `--snapshot-depth`,
  `--underpriced-for`, `--lua-states`, `--lua-builders`, `--wasm-cache-mb` and
  `--wasm-cache-modules`, and `--tune KEY=VALUE` for some thirty more, `sync.headers=512` and the
  like. None is consensus, so nodes need not agree. A value past its bounds stops the node at
  start; `GET /node` shows every one under `limits`, unless `--report minimal`.
- **`--tune`'s sync caps and `gossip.batch` only go down.** Askers ask for no more than the
  defaults, and peers refuse a batch of more than 256 transactions as garbage, which would get the
  node banned.
- **`--max-peers-in` counts connections, not peers,** and only those dialed in: a peer connected
  both ways holds one of each, and `--peer`s and bootnodes this node dials always get through.
- **`--http-rate` is per client as `--http-per-ip` is:** behind a proxy on the node's machine with
  no `X-Forwarded-For`, every visitor is one client. Docker's nginx forwards it, and limits each
  address already (`RATE_API`).
- **`--underpriced-for` and `--state-history` take a time too,** `3m` or `14h`, turned into blocks
  at the chain's block time as the node starts: a plain number is blocks, as before.
- **A snapshot's depth is the syncing node's own** (`--snapshot-depth`): peers serve the state of
  whatever block they still hold, so they need not agree on it. It is held to half the chain's
  `max_reorg_depth`, and a chain shorter than twice it is run instead.
- **Surround votes are caught as far back as a bond can be slashed:** counted votes stay
  `max(64, withdrawal_delay + vote_depth)` epochs behind the finalized one, at most 4,096, so a
  `Change::Finality` that raises `withdrawal_delay` past 64 no longer leaves surrounds signed in
  the gap unpunished. Node-local; the slash itself is checked on its two votes.
- **The largest request body follows the chain's `max_tx_bytes`:** 2 MiB unless its hex takes more,
  read as governance's limits stand for each POST, 413 past it.

## How the chain behaves

- **`GET /blocks/range` proves what it gives, not that it gave everything.** Each block recomputes
  its header's roots and each state entry it lists is proven against `state_root`, but nothing commits
  to which entries a block changed: that list is the node's word. A node can leave one out, or claim
  it has none, and every check passes. Receipts cannot be left out so. A client that rests on a range
  must say so to its user, and treat a block with no proofs as unknown, not unchanged.
- **What a block changed is history's list, not a column of its own.** The list `GET /blocks/range`
  proves from is the one history prunes by, so it writes nothing new, lives as long as the state it
  is proven against, follows reorgs, and a node has it for all it ran before upgrading. The lowest
  height the window reaches can be read but has no list. Genesis has none either, so its allocations
  are not in a range.
- **A list may hold entries that are not there.** A block that writes a key and takes it out again,
  or leaves an account at nothing, lists it, and proves it absent. It is not a sign the key ever held
  anything.
- **A range's cost is bounded in time as well as work.** 4,096 proofs take about 70 ms with state in
  cache, but each is some 34 reads, so a cold disk would pass that many times over: the second's limit
  is what bounds those, and where an answer stops then depends on the machine. Go by the cursor.

- **Failed transactions stay in their block.** They use up their nonce and pay their fee. Only a
  transaction that cannot run at all makes a block invalid: a wrong nonce, a bad signature, or an
  offer that cannot pay.
- **Producers run what they build.** `Chain::build` runs the transactions it is given and keeps
  those that can run and fit the block gas limit. A producer runs each block twice.
- **Blocks run on every core, with the result one by one gives.** A transaction waits for the
  earlier ones that name what it names, such as its sender or contract. Transactions then run side
  by side in rounds on what the block holds so far, and go in in order. One that read what an
  earlier one wrote all the same runs again. Execution must therefore stay deterministic and read
  state only through `State`: a read that went around it would escape the conflict check.
- **Receipts keep a trace.** Every call, delegate and transfer a transaction's contracts made,
  failed ones included, up to 256 hops. It is not consensus: the receipts root leaves it out, and
  a failure's message too.
- **Headers commit to state.** Each `state_root` is the root of a sparse Merkle tree over every
  chain-state entry, and each `receipts_root` a Merkle root over what the block's transactions
  did. A block whose roots are wrong is refused (`StateRoot`, `ReceiptsRoot`), after it runs.
  `GET /proofs/{column}/{key}` and `GET /proofs/receipts/{id}` serve proofs against the tip, and
  `StateProof::verify` and `crypto::merkle::verify` check them. See
  [Chain](chain.md#the-state-root).
- **Storage format 8.** `tx_root` became a Merkle root over the block's transactions, so every
  block with transactions has a new id, and the address index has a column of its own: delete old
  databases, or point `--db` elsewhere. Genesis ids stay as they were, since genesis's `tx_root`
  is the hash of its configuration.
- **A receipt proof proves the transaction too.** `GET /proofs/receipts/{id}` carries the signed
  transaction and its path in `tx_root`, so a light client knows who sent what (`check_tx`), not
  only what it did.
- **Genesis commits to the light client's parameters.** Its `proof_root` is the hash of
  `light_params()`: the chain hash, consensus and finality's timing. Changing any of them is a new
  chain, as changing genesis always is.
- **Snapshot nodes keep headers only before their pivot.** `--snapshot` checks every header and
  takes the state of a block 16 below the best tip, then runs the blocks after it. Before the
  pivot it has no bodies, receipts, events or light updates, and it never rolls back past it.
  Peers can serve state only at their last `max_reorg_depth` blocks, so a pivot that grows too
  old moves up, and the walk fetches only what changed. With `--checkpoint 0x…` it fetches
  nothing below a finalized checkpoint it is told to trust, and holds it as final: the answer to
  long-range forks on a staked chain. See [Node](node.md#syncing-from-a-snapshot).
- **Every dev chain has the same genesis id.** A wallet that followed one resumes against the next
  as if it were the same chain, and waits for blocks past where the old one got to: Settings →
  Light client → **Start over**. See [Light client](light-client.md#what-to-keep-in-mind).
- **`max_block_bytes` gave the dev chains new ids, and left every other chain's alone.** Genesis
  is hashed for its id, and the field encodes as nothing while unset (`Unset`), so a genesis file
  without it keeps its chain. The dev chains set 4 MiB, so their databases from before start
  over. A field added to genesis later must do the same, and come last.
- **`consensus.window` gave the dev proof-of-work chain a new id, and left every other chain's
  alone.** Like `max_block_bytes`, it is `Genesis::pow_window`, a last field that encodes as
  nothing while unset, so a genesis file without it keeps its id and the parent-only difficulty
  rule. The light parameters hash the same way (`LightParams`), so the `proof_root` genesis
  commits to holds too; on the wire they are a plain encoding. A saved light client state from
  before has no `ancestors` and does not decode, so the wallet and explorer start over.
- **`vote_reward_source`, FFG's `leak` and a scorer's `kinds` are later genesis additions too,** encoding as
  nothing while unset, so files from before keep their ids. Under `fees`, the burned share of
  fees sits at `REWARD_POOL`, `contract:0x00…`, a balance nothing but proofs can move.
- **GPU mining is a feature and a crate of its own.** `cargo build --features gpu` pulls in
  `chainv4-gpu` and wgpu; `--mine --mine-gpu` then mines on Vulkan, Metal or WebGPU, on every GPU
  unless `--gpus` picks. wgpu is held at 24 there, since the node's libp2p pins the wasm crates
  newer versions want past. The crate's tests skip where no GPU is found, as on CI.
- **Workers mine for a node, not for a chain.** `chainv4 worker` keeps no blocks and no genesis:
  the node tells it the work hash with each job. Rewards go to the node's `--reward-to`, and the
  node checks every nonce a worker sends, so a worker with the token can waste its own power and
  nothing else. Worker nonces start at 2^60 and the node's own at 0, so a seal says which found
  it.
- **The protocol names changed for everyone.** Sync, Kademlia and identify were `/chainv4/sync/1`,
  `/chainv4/kad/1` and `/chainv4/1`; they now carry the genesis id, as `/chainv4/{network}/sync/1`
  and so on, as the gossip topics did already. Nodes from before this change and after it do not talk: upgrade
  every node of a chain together. `--protocol` replaces `chainv4` in them all.
- **`--ca-cert` wants a certificate that is not a CA.** rustls refuses a server certificate marked
  as a CA, which is what `openssl req -x509` makes unless told otherwise; `docker/nginx/tls.sh`
  now makes its self-signed one with `CA:FALSE`, and makes again one an older version made.
- **Genesis bootnodes are as permanent as the chain.** They are hashed into its id. On a chain
  with governance, `Change::Bootnodes` replaces them on the live chain, at once, on a ballot of
  their own; without governance, `--bootnode` and `BOOTNODES` add a node's own, which change with
  no vote.
- **Discovery dials only while short.** A node with `--target-peers` (15) peers or more stops
  dialing bootnodes and Kademlia's peers, but still takes peers that dial in. Kademlia bootstraps
  every 2 minutes either way.
- **A pow.sh chain's voting key lives on the VPS alone.** `init --finality ffg` bonds a voting key
  of its own for your account; only the VPS votes with it, since two nodes voting with one key on
  different forks sign a slashable pair. Your own key never leaves your machine.
- **pow.sh chains made before this have a minter, which nodes now refuse.** `init` used to name
  your account as `minter`; genesis allows only the development key there since minting became a
  development-chain faucet, so such a chain no longer opens. `init` now writes none: make the
  chain again.
- **An offline signer keeps a history of what it signed, sealed with its keys.** On by default, the last 200, in `Book.signed`, written only when there is any so a wallet without one stays as it was. An older build ignores it and loses it on its next save, and nothing else. It is a record of what was signed, not that it landed.
- **A wallet's guard is read from the record it always kept.** `WalletEntry::guard` says what opens
  it: a `biometric` record tagged `biometric-v1:` or `both-v1:` needs hardware, any other is the older
  alternative to a password, and none is a password or, if `unprotected`, nothing. There is no new
  field, as an older build's save drops what it does not know, and that field would be the only way
  to tell how the vault is keyed. An older build reads a tagged record as from elsewhere and writes
  nothing, and every password is wrong to it. `Vault::both` joins the password's stretch and the
  hardware secret with HKDF-SHA-256 under `BOTH_INFO`, whose output a test pins: changing it shuts
  every such wallet. Never `Vault::open` a wallet that needs hardware: it says wrong password.
- **`turn_off` does nothing for a wallet that needs hardware, and pages do not call it.** Every page
  goes through `biometrics::invalidated`, which turns off today's kind as it always did and writes
  nothing for one that needs them; `biometrics::enrolled` returns those even when they read as
  unavailable, so a check fails rather than is skipped. A test scans the pages for this.
- **A record is retired, not forgotten, when a vault is replaced.** `guard::adopt` puts it in
  `Settings::retired`, and asks for settings to be written at once. Its key is forgotten as the app
  next starts, once settings saying so were read back. Forgetting it first would strand a wallet
  whose new settings never reached disk. eframe writes its file in place, not atomically, so a kill
  during any save can still lose settings: it always could, and words are the way back.
- **Older wallet biometrics keep the vault's key, not the password.** Each platform wraps the key the
  password stretched to: the Android Keystore, a passkey's PRF output, or the macOS Keychain after
  Touch ID. Changing the password turns them off, and so does a change in the fingers or faces
  enrolled. On macOS Touch ID is a check, not a lock on the key, as an unsigned app cannot tie a
  Keychain item to biometry. That is why macOS offers nothing new, and why iOS, which is signed, has a
  module of its own in `ui/src/biometric.rs` rather than sharing macOS's.
- **Reorg depth counts from the tip, not along the branch.** A heavier branch is followed when it
  splits off at most `max_reorg_depth` blocks below this node's tip, however long the branch is:
  a node that mined a few blocks of its own switches to a peer's chain that grew from below them.
  It used to stop walking the branch after `max_reorg_depth` of its blocks, so a long one was
  refused even when it split off near the tip.
- **A miner waits for its peers.** Under proof of work a node mines only while no peer reports
  more work, and, just started with peers connected, not before one reports or 10 seconds pass.
- **The browser extension is the web build in a side panel.** `wallet/build-extension.sh` moves
  trunk's inline boot script to `boot.js`, as MV3 runs none, and adds manifests for Chrome and
  Firefox. It asks for each node's host when the wallet first talks to it, from a click, as
  browsers require. Chrome stable no longer loads unpacked extensions from `--load-extension`;
  Chromium, or Developer mode's Load unpacked, does.
- **A wallet may have no password.** It is sealed with an empty one and says so on its lock
  screen; `WalletEntry::unprotected` marks it. Settings adds or removes one, or makes it need
  biometrics. A wallet with biometrics alone is not this: its key is random, and no empty password
  opens it.
- **Wallet secrets are zeroized.** `Vault`'s key, the opened vault's JSON, `Words`, typed
  passwords, words and keys (`chainv4_ui::secret::Typed`), and biometric secrets
  (`chainv4_ui::biometric::Secret`) are wiped as they drop; the Android glue overwrites Java's copy
  of a secret once it has read it.
- **And the copies along the way.** BIP39 is built with its `zeroize` feature and
  `Phrase::parse` wipes its lowercase copies; a `Typed` field starts with room for 1,024
  characters, so a `String` growing never leaves the old text in freed memory, and
  `chainv4_ui::secret::secret` drops egui's undo history of the field each frame; the vault's JSON
  is counted first and written into room of exactly its size. Use both for any new secret field,
  in the wallet or the explorer, whose pasted key and key file are wiped the same way.
- **A vault's `phrase` is null, an object or a list, and the list is a fence.** One phrase is
  written as it always was, its `name` left out, so older builds read it byte for byte. Two or more
  are a list, always, even with no account from the second: an older build's `Option<Words>` reads
  a list by position, wants a string first, finds an object, and says the vault is damaged and
  writes nothing. It would otherwise ignore the new data, derive an account of the second phrase
  from the first, and save the rest away. `vault::tests::older` holds a copy of the old structs to
  prove it: keep it as it is.
- **An account of a further phrase names it by fingerprint, `Source::Phrase::from`.** Absent means
  the first phrase, which is why the first can never be removed by itself, and why `from` is never
  written for it. `Book::check` runs on every open and seal: an account whose phrase is not there
  fails both.
- **An account this build cannot read has no holder and still gets saved.** `keys::open` leaves out a
  key damaged or in a scheme it was built without, and `keys::stored`, which every save rebuilds the
  book's accounts with, puts `Book::unread` back after the holders. Build the accounts any other way
  and those go for good. Add a kind of account, and add it to `StoredAccount::readable`.
- **A wallet's proofs are held to what nginx allows.** Accounts times addresses times listed tokens
  comes to hundreds of proofs. It has 8 out at a time (`PROOFS_AT_ONCE`), asks nothing again at a block
  it holds a proof at, as a block's state never changes, and spaces its rounds at 15 a second over the
  round (`PROOFS_A_SECOND`). A 429 is only shown once it comes 4 times running: it waits and asks again.
- **A transaction names its chain by signing it, and rows from before did not.** A signature covers the genesis id,
  so a transaction signed for one chain fails on any other, and the same keys sending the same thing on a new chain
  make another id. Older builds' rows may hold ids a new chain could share, as signatures then named no chain, so the
  wallet still keeps what it proved by id with its genesis, and goes by both (`Checked::on`).
- **Binding a transaction's signature to its chain needs a new genesis, every node and app at once.** It changes what
  every signer signs, and there is no compatibility path: a chain's old transactions fail their signatures, so a node
  on the new rules cannot replay an old chain from its genesis, and an old app's transactions are refused. Only the
  signature's content changed: no tag or field encoding moved. The new genesis must have a new id, or it has not parted
  from the old chain at all: a file run again as it was, same timestamp and all, makes the same chain, whose transactions
  hold on the new one. Change its `timestamp` or anything else it commits to.
- **A signature is for the chain whoever gives it the genesis id says.** The wallet and the explorer take it from the
  light client, which holds it against the genesis pinned at the node, and sign nothing with none known: never a pinned
  one not yet checked, nor a zero. `chainv4 send` takes it from `GET /status`, so `--node` chooses the chain, as it
  does the nonce and fee; `--genesis` refuses any other before signing. A wrong or lying node gets a signature for
  the chain it names, of a transaction you asked for, and without `--genesis` nothing here can tell.
- **The offline signer holds a request's chain against Your chain by its id alone.** That is the genesis id's first
  6 bytes, as for EIP-155, so a sender could make another genesis id that passes, by about 2^48 tries: a signature for
  a chain of that id, and for no other. The signer shows the whole genesis id to check. With Your chain unset it has
  nothing to hold it against and says so. It does not name a native request's chain as MetaMask's are named, such as
  Ethereum: those first bytes are the sender's to choose.
- **A request's code grew by a genesis id, 33 bytes.** A token transfer's request is now about 310 bytes, so one code
  holds up to 340 (`ONE_CODE`), about 700 characters, while a long message's animated parts stay at 300 (`FRAGMENT`).
  If a phone struggles with that one code, setting `ONE_CODE` back to 300 makes those requests animate instead.
- **Behind a proxy on another host or container, the API's per-IP limits need `--trusted-proxy`.** Without
  it every visitor is one client, and shares one faucet wait and one limit on requests. The Docker stack
  sets it to `nginx`, by name, as a container's address can change.
- **An extension for a store is built with `--store`.** A plain `wallet/build-extension.sh` has the
  miner, as the page the nodes serve does, and the Chrome Web Store and Firefox Add-ons refuse that.
- **Off the web, wallets are not in eframe's storage.** eframe empties its file and then writes it, so
  a kill between loses all of it, and a torn file reads as empty. `wallet/src/store.rs` keeps a file
  and a copy per wallet, and for settings, each flushed, read back and renamed into place. The copy
  holds the same as the first, never an older version, or a changed password would still open it.
  Wallet files go before settings naming them, and a removed wallet's go before settings stop naming
  it, so a kill never brings one back after its hardware key is forgotten. Only a wallet's own text
  changing writes its file.
- **A `Book`'s `Debug` holds no secret.** `Words`, `Source::Key` and a swap's secret print as
  hidden, and `Stored` has no `Debug`. Keep it so for any new secret field: `assert_eq!` on a book
  prints it when a test fails.
- **`Holder::number` alone says nothing across phrases.** Every phrase has its own 0, and its own
  shared paths, so count with `keys::unused` and `keys::next_index` by `Holder::from`, and derive
  with `keys::seed_of`. `Words` keeps its seed once stretched, wiped with it: `words.seed()` costs
  nothing per frame, and its fields are private so that seed cannot go stale.
- **`GET /node` has `mining`,** null for a node that does not mine: the total `rate`, the node's
  `devices`, `listening` for workers, and `workers`, each with its devices, `found` and `refused`.
- **A rotating authority's producers are its finality signers, and a staked chain's its FFG
  validators.** `consensus.type: rotating` and `staked` check each block against the voters
  fixed for its epoch, so header checks need state: light clients follow `activation_delay`
  epochs behind finality, and so does a node syncing from a snapshot, which checks headers as a
  light client does and learns the voters from the finality its peers serve. The dev chains
  `--consensus rotating` and `--consensus staked` run them with the development key alone.
- **Under `staked`, pick the schedule for the chain.** `draw` is stateless and cheap to check, but
  a validator's turn is luck per slot; `priority` gives each validator its exact share and a known
  order, and checking a block replays its epoch's heights, `checkpoint_interval` rounds over the
  set. Both weigh an in-turn block 2 and a stepped-in one 1.
- **Governance changes the consensus as finality fixes voters.** With `governance` in genesis,
  `Tx::ApproveChange` from the electorate or the council decides a new consensus, fixed
  `activation_delay` epochs ahead at the next checkpoint; the finality kind and its epochs stay.
  A node started with `--mine` and `--produce` together serves whichever the chain runs. Light
  clients then wait for each epoch's consensus from a light update, on any chain with governance,
  so they follow `activation_delay` epochs behind finality. A chain that switches to a scheme
  nobody produces for stops; have the producers ready first.
- **A deciding approval's receipt says Decided.** Its outcome is `Decided(version)`, committed in
  the receipts root, so light clients prove each decision from its receipt; nodes index them by
  version for `GET /governance/history`, and versions count up one at a time, so a gap shows. A
  change of the rules: a chain that decided anything before moves to it with a new genesis.
  `GET /governance` also shows every setting in force, with the change that set it, and who may
  approve; the apps' Governance pages read it, and the wallet approves and proposes there.
- **The supply is counted in state.** Minted and burned, under `coin:supply`, proven as a balance
  is; `GET /supply` serves them with what is bonded and pooled, `GET /rich` the richest accounts
  and `GET /names/list` every name, both the node's word. Counting it moved every genesis state
  root, so chains from before need a new genesis.
- **The wallet's governance form reads the node's own checks.** A field's bounds are in
  `wallet/src/views/governance/schema.rs` and each error under a field comes from `change_from_json`,
  matched to the field by its words (`form::blame`); a test holds the bounds to what the node
  refuses and another that every leaf of every change has a field, so a new setting fails a test
  until it has one. What only a node can check against the chain, such as a consensus against its
  finality (`consensus_problem`, `retune_problem`, `switch_problem`, in `chain/`, behind `node`), is
  mirrored from what `GET /governance` lists: a form can pass and the chain still refuse, the
  transaction failing as it would from the CLI.
- **`over` only adds to an object.** It cannot take a field away, so a change of variant, bids for
  a base price or one consensus for another, would keep the old variant's fields and be refused as
  unknown ones. The form drops such an object from what stands before laying over it
  (`schema::variants`); Advanced's JSON gives it whole.
- **Searching a history is reading it.** `GET /governance/history` has no search, so the apps filter
  what they read, and a search or a page that comes up short reads older pages until it is full or
  none are left (`ballots::wants_older`), saying how much of the history it has been through. A
  page that fails stops that reading until the next asking.
- **The text fonts have no arrow.** The bundled faces are Latin subsets, without `→`, `‹` or `›`, and
  of egui's own only its mono face, Hack, draws `→`. So `install_fonts` adds it to every family's
  fallbacks, and an arrow never comes out as nothing; `widgets::arrowed` sets one in the mono face
  anyway, where it should match the numbers around it.
- **`--mining-onion-port` reads the onion when asked.** In Docker the node starts before tor, and tor
  waits for the node to be healthy, so nothing at the node's start can know the onion. `GET /mining`
  reads `--onion-file` on each request, and says no `onion` until it is there.
- **One way to change the directory per chain.** Governance's `Change::Directory`, or without
  governance the `directory_admin` genesis names, by `send set-directory`; the admin hands the
  role on, or renounces it for good (`hand-directory --renounce`, which asks for yes). Genesis
  refuses an admin beside governance; with neither, genesis's directory stands.
- **Keys show what others name them by.** `keygen` and `chainv4 key show FILE…` print the scheme,
  the public key (compressed for secp256k1, as a federation lists it), the address, and the EVM
  or testnet one, never the secret.
- **Each kind of change has its own ballot.** Approvals name their kind's version, so deciding
  fees leaves a rename's approvals standing; an account backs one change of each kind; withdraw
  names the kind. Part of fees, the reward, limits, finality, the federation or the directory may
  be given, laid over what stands. The chain still counts every decision once, gap-free, which
  the history and light clients follow.
- **One JSON for changes.** `params::json` (the `json` feature, no node needed) reads and writes
  every change, `change_json` and `change_from_json`, and each part of genesis one may replace,
  with the same checks; `chain::json` adds genesis files. The CLI, the API and the apps share it.
- **Fees, the reward and the limits change from the next block; finality's tunables an epoch
  boundary ahead.** Each block reads them from its parent's state, so a reorg past the decision
  takes it back, and a block is never checked by rules decided after it. A finality change runs
  from `e + activation_delay + 1`, the first epoch whose voters were fixed after the decision.
  It keeps the kind, the epochs, the genesis voters and the thresholds: light clients check
  nothing new. A new reward halves from its own first block, not from genesis.
- **Settings live in `src/params/`, their behaviour with what uses them.** `FeeConfig`,
  `Reward`, `BlockLimits`, `Limits` and `FinalityConfig` with each kind's config are plain data
  there, in every build, so a light build decodes every `Change`; `chain` and `vm` re-export them
  and keep the methods. In a node each finality kind is there only with its feature; a build
  without `node`, such as the wallet's, has them all.
- **Finality's kind is per epoch, its epochs are not.** After a `Change::SwitchFinality`,
  `Changes::finality_in` says which kind an epoch runs, and every reader asks it: the vote engine
  (one `Elected` electorate for both kinds), the staking and signer rules, slashing evidence by
  its own epoch, governance's electorate, the producers and the API. Each checkpoint fixes the
  voters `activation_delay` ahead by that epoch's kind only, so an epoch never has both kinds'
  sets, which is what makes `LightUpdate::switched` provable. A switch into FFG lapses at the
  checkpoint that would fix its first set if nobody bonded; bonds made in that checkpoint's own
  block vote only an epoch later, so they are paid back.
- **A multisig's transaction carries the multisig's nonce and fee.** `SignedTx::sender` is the
  multisig for a `Tx::Multisig`, so the nonce, fee reserve and refund, mempool and receipts all
  work as for a key's account; the carrier's signature only wraps it. The quorum is checked before
  the nonce (`execute::authorize`), and again at mempool admission, which now takes the genesis id,
  so an under-signed one never costs the multisig anything or takes room. A dry run skips the check,
  as it skips signatures, so a proposal is priced before its members sign.
- **A transaction inside a transaction needs `Nested`.** wincode's derive works out a type's
  static size from its fields, and `Tx` holding a `Box<Tx>` asks itself: the cycle only shows once
  something evaluates it, as tests do. `Nested` encodes the inner one as itself, with no size known
  ahead.
- **PSBTs live in the library now** (`src/crypto/bitcoin/psbt.rs`): the maps, reading and writing,
  the HTLC spends, and a multisig's, built, signed, combined and finished. The wallet keeps its
  signer's review and signing as the `Signs` trait over it, so the CLI and the wallet make the same
  PSBTs.
- **A multisig's Bitcoin side is checked against Bitcoin Core.** An ignored test
  (`bitcoin_core_derives_the_descriptor_s_addresses_and_takes_a_two_of_three_spend`, run with
  `--ignored` where `bitcoind` is installed) has Bitcoin Core derive our descriptor's address,
  decode our PSBT and take the 2-of-3 spend. BIP32's public derivation, BIP380's checksum and the
  test seed's BIP48 xpub as Sparrow exports it have vectors of their own.
- **An HTLC has two templates now.** Two keys keep the old one byte for byte, one shared
  `OP_CHECKSIG` after the branches, so swaps made before still read; with a multisig's branch each
  branch ends in its own check. `Htlc::parse` takes either and refuses any other writing.
- **A multisig's swap keys sit at an index from the swap's hash** (`swap::multisig_index`): no one
  passes or stores it, and every member finds the same.
- **The regtest tests each take their own port and directory,** so they run side by side; two
  `bitcoind`s on one port hang the second.
- **Multisig policies live in `Column::Accounts`** beside the accounts, under `multisig:`,
  `proposals:` and `multisig-evm:` prefixes, which no account key starts with. Only the member
  index scans the `multisig:` prefix, once, when a node starts keeping it.
- **The member index follows state, not blocks.** `--index-multisigs` keeps `Column::Members`, each
  multisig under the address of each member's key, built whole from the registry when turned on and
  moved by each block that writes a policy, from the members before to those after. A rollback
  moves it back by the same rule (`revert` follows the registry), so it is right after a reorg even
  for a block run before the node kept it. It needs no new genesis: it is not chain state.
- **A member is found by its key's address.** An Ethereum or Bitcoin account's address is a hash
  of its key, so `GET /accounts/{address}/multisigs` takes the address, or the key, and the policy
  names the key; nobody can turn an address back into a key to make a member of it.
- **A multisig proposal is priced for its heaviest quorum.** Keys are 32 or 33 bytes and gas counts
  bytes, so a probe of the proposer's key alone under-priced a quorum holding a secp256k1 or
  bitcoin member, carried by one: "gas limit under its intrinsic cost". `MultisigBundle::probe`
  carries the threshold heaviest members' signatures under the heaviest carrier; the CLI and the
  wallet both price with it, and send no more than threshold signatures (`trimmed`).
- **A bundle's review shows a deploy's hash, not its code.** `multisig sign` and `approve` list a
  deploy or upgrade by engine, size, `code_hash` (SHA-256 of the code's bytes: the Lua file, the
  module, or the init code decoded from its `.bin` hex) and the address it lands at; `--full` shows
  the code. The API's JSON, `structs::json::tx`, still carries Lua's `source`: the CLI drops it for
  the review alone. A build without `sha256` names the chain's own hash in `code_hash` instead.
- **A member's xpub is its words', not its account's.** `Seed::multisig_account` is at
  m/48'/coin'/0'/2' for every account of a phrase, so two accounts of one phrase cannot both be
  members of one multisig on Bitcoin, which names each xpub once. A `bitcoin:` member key, at
  m/84'/…, gives an xpub from the same words all the same.
- **A multisig's EVM contract address commits to its first EVM owners.** `Multisigs::evm_start`
  keeps the owners and threshold from when the members first named EVM addresses, written once at
  creation or the first change that names them, never after, since `multisig_contract` hashes them
  into the init code. Owners there move only by `changeOwners` through the contract itself.
- **MultiSig.bin is built for Paris** (`solc --evm-version paris --optimize`), so it runs on
  chains without PUSH0; rebuilding it with another solc changes every multisig's address, as its
  init code is in the hash.
- **chainv4's EVM has the deployment proxy from genesis.** Its runtime code is written at
  `0x4e59…956c` when genesis names the `evm` engine, which changed the genesis id of every such
  chain: an old data directory meets `WrongGenesis`. The todo has revisiting it.
- **A contract made at a funded address takes the coin over** (`vm::evm::apply`). Before, coin
  sent through the EVM to a counterfactual address was copied into the contract and left with the
  secp256k1 account of those bytes too, and coin sent natively to `evm:0x…` was overwritten with
  the EVM's view at creation; now the contract gets both, and that account keeps none.
- **An EVM bundle is EIP-712 typed data**, so the offline signer signs it through the MetaMask-style
  `eth-sign-request` it already reads, with no code of its own, and any other EIP-712 signer can
  too. `EvmBundle::add` takes a signature from elsewhere, checked to recover with a low s.
- **A light client on a left branch used to stop.** A justification for a block it did not hold
  at the checkpoint failed as `BadJustification`, and the wallet stops its light client on any
  error, so a phone that saw the losing side of a fork, common with two machines mining, sat on a
  block the node had dropped, and every proof waited. `LightClient` now rewinds to its justified
  checkpoint when such a justification's votes hold (`take_justification`). Its headers had the
same trouble: on a fork above the justified checkpoint it stepped back a whole batch, took the
batch's start, below the checkpoint, for the fork, and failed as `ConflictsWithFinality`; under
voting finality it now goes back to the checkpoint and follows the node from there.
- **iOS renders with wgpu, everything else with glow.** Each app's manifest asks eframe for glow
  off iOS and wgpu on it, as does `ui`'s: a renderer feature for both would build glutin for iOS,
  which has no OpenGL to give it. iOS also leaves out rfd and nokhwa, which have no iOS side;
  `ui/src/files.rs` and `ui/src/camera.rs` have their own iOS modules on UIKit and AVFoundation,
  and what Face ID keeps for a wallet needing it is `ui/src/biometric.rs`'s `ios`, a Keychain item
  tied to the faces enrolled, which is compile-checked only so far. The older kind it reads shares
  the macOS module, `mac`, the Keychain and LocalAuthentication being both systems'.
- **iOS 27 runs apps through the scene lifecycle alone,** and stops one that does not adopt it at
  launch, which winit predates. The Info.plist names `Chainv4SceneDelegate`, a class
  `ui/src/ios.rs` registers before UIKit starts, which keeps the scene; winit's window, made
  outside any scene, draws nothing until it is the scene's, so it is caught from eframe's creation
  context and joined to the scene by whichever of the two comes second.
- **The device's own wgpu limits.** eframe asks for WebGPU's default limits, a little more than the
  Simulator's GPU gives (`max_inter_stage_shader_variables`), and quits if refused, saying so only
  through `log`; on iOS the app asks for the adapter's own limits, and logs to standard error.
- **iOS's safe area is the app's whole screen.** winit's inner size on iOS is the safe area's, which
  eframe sizes the Metal surface by, while egui-winit lays the app out over the whole screen: the
  layout ran past the surface and lost its bottom, the tab bar with it. So winit's view is moved
  into the safe area, egui is told that is the screen, touches, which come in the window's points,
  move by its corner, and the window behind is painted the app's color.
- **iOS's back is a swipe.** iOS has no back button, so `ui/src/ios.rs` turns a touch that starts
  within 24 points of the left edge and moves 80 points right into `Key::BrowserBack`, the key
  Android's gesture sends; the apps handle both the same.
- **A free Apple ID signs through a stub app.** Xcode makes a provisioning profile only for an
  app it builds, so `ui/build-ios.sh --provision` has XcodeGen write a stub of the same bundle id,
  which Xcode signs automatically; the real app is then signed with that profile's entitlements
  and installed with `devicectl`.
- **The onion goes to the node, not through nginx.** nginx's port 80 redirects to HTTPS, which an
  onion needs no part of, and its certificate would name the domain, not the onion; tor's
  `HiddenServicePort 80 node:8080` reaches the node on compose's network directly, its port still
  published nowhere. With `TOR_MINING=1` the mining port is offered on `MINING_PORT` and forwarded
  to the 4002 the node takes workers on inside its container.
- **The node builds its own transport stack** (`net::tor::transport`), handed to libp2p's builder
  as one custom transport: QUIC, then direct TCP, then Tor for `/onion3`; or under `--tor-only`, a
  TCP that only listens (`ListenOnly`, whose dials all refuse) before a Tor that dials every TCP
  address, and no QUIC. A Tor that dialed only onions while TCP dialed the rest would leak the IP.
- **The node learns its onion late.** tor makes the address after the node starts, so the stack's
  tor writes `/onion3/NAME:PORT` into the `onion` volume and the node reads `--onion-file` on each
  redial until it is there, then adds it as an external address, which identify tells peers.
- **The worker speaks SOCKS5 itself** (`mining::worker::through`): a greeting with no
  authentication, then CONNECT with the node's name, never an address, so a `.onion` resolves
  inside Tor; the reply's bound address is read and dropped. It waits up to 90 seconds, as Tor
  builds a circuit to an onion first.
- **The anvil test is ignored** (`a_multisig_s_contract_sits_at_one_address_on_anvil_and_chainv4_and_pays_on_anvil`):
  run it with `--ignored` where Foundry's `anvil` is installed; it takes port 18965.
- **Light updates gained a field again.** `LightUpdate::switched` is a plain field, not an
  Unset one, since updates travel inside lists; stored updates from before read back through
  `ErasLightUpdate` and `LegacyLightUpdate`, but a wallet or explorer built before it cannot read a
  newer node's light finality, so the apps and nodes update together.
- **A decided finality change is a framed encoding.** `FfgConfig::leak` decodes only at the end of
  a message, as a chain from before it encodes nothing there; inside `Change::Finality` and the
  kept eras it goes length-prefixed (`Framed`), so what follows still reads.
- **Mint is for development chains.** Genesis's `minter` may name the development key or
  nobody; a genesis file naming any other address is refused as `BadGenesis`. Coins on a chain
  of value come from block rewards and allocations alone.
- **Depth finality splits for good.** Nodes apart for longer than `depth` blocks never rejoin on
  their own, as under Bitcoin Cash's rolling checkpoints; resync one side.
- **Authority signers have accounts, which gave authority-finality chains new ids.** A signer
  set holds each signer's key and account (`Signer`), in genesis, in state and in
  `Tx::ApproveSigners`, so the genesis state root of every chain with `finality.type` `authority`
  changed, and with it the genesis id: such databases start over, and an `approve_signers`
  transaction from before does not decode. The dev chains use FFG, so theirs stood. A genesis
  file naming signers as bare keys still reads, each key its own account.
- **Blocks have no fixed transaction count.** A producer fills a block to genesis's
  `max_block_bytes`, checked on import, and the gas limit; `MAX_BLOCK_TXS`, 1,000 a block and
  never checked, is gone. A slow node can cap its own blocks with `--max-block-txs`, or bound
  the time it spends building one with `--max-build-ms`, which seals what ran by then. With 2 s
  slots, plain transfers now fill a block to its gas limit, about 13,800.
- **wincode refuses lists past 4 MiB in memory, encoding too.** A `SignedTx` takes 352 bytes in
  memory, so a block of 12,000 transfers would not encode, store or gossip. A block's `txs` and
  its undo record take up to `MAX_LIST_MEMORY`, 64 MiB, with the same encoding
  (`BincodeLen<…>`); any other list a full block makes needs the same.
- **Transactions are admitted off the node loop.** The verifier thread's gate checks and inserts
  them while the loop builds or imports blocks; the loop locks the mempool only to select and
  prune. Lock it once per statement: `std::sync::Mutex` is not reentrant, and two guards in one
  expression deadlock.
- **Gas limits come from dry runs.** `POST /offer` runs a call, deploy or upgrade on a throwaway
  copy of committed state and offers what it used plus a quarter of what it ran. Read-only calls
  and dry runs run on blocking threads, since the API answers everything on one. A dry run's
  state is the committed one: a sender's own pending transactions are not in it.
- **Light clients trust only a genesis id.** `chainv4 light`, and the explorer, check every
  header by consensus, every justification against voters proven from state, and every value
  against a header they checked. Voters are trusted from a checkpoint's state once that
  checkpoint is justified. See [Light client](light-client.md).
- **Coin decimals are for display only.** Genesis names the coin's symbol and decimals, but the
  chain counts every balance, fee and reward in whole units.
- **Rewards have no coinbase transaction.** The chain credits the producer after a block's
  transactions run, and a reorg takes it back. A reward can be spent from the next block on. The
  API says what each block minted as its `reward`.
- **Reorgs reach back at most `max_reorg_depth` blocks**, 100 on the dev chains. Finality bounds
  them further: nothing at or below the finalized checkpoint comes off, and work alone only moves
  the chain to branches that hold the justified checkpoint.
- **Genesis bonds are extra.** Genesis validators' bonds are minted on top of the allocations.
- **Accounts at the default are deleted.** An address with no balance and nonce 0 is not stored.
- **Running out of fuel costs all the fuel allowed.** On a chain with fees, that is the whole gas
  limit when gas bound it.
- **Calls reserve their whole fuel cap.** `send` sizes a call's gas limit from `tx_limits.fuel`,
  since nothing estimates gas. The sender must hold that reserve, though the unused part comes
  back.

## Contracts

- **Four functions are special:**
  - `init`, run at deploy;
  - `migrate`, run after an upgrade;
  - `check_perms`, asked before an upgrade;
  - `fallback`, which answers functions a contract does not export.

  Transactions and other contracts cannot call `init`, `migrate` or `fallback`, nor delegate to
  them. A contract without `check_perms` can never be upgraded. See [Contracts](contracts.md).
- **Reentrancy is refused by default.** A function only runs while its contract is already
  running further up the chain of calls if it is marked:
  - `--@reentrant` on the line right above `function M.name(` in Lua;
  - `#[export(reentrant)]` in Rust.
- **Lua annotations are read line by line.** A `--@reentrant`, `--@namespace` or `--@retired`
  line inside a multi-line string counts too.
- **Numbers leave a contract whole.** Luau computes with floats inside a script, but every number
  a contract returns, passes, emits or lists goes out as an `i64`, whole and within 2^53, or the
  call fails; a `number` argument must be whole to come in. `math` keeps only `abs`, `ceil`,
  `clamp`, `floor`, `max`, `min`, `round` and `sign`. Floats are refused in WebAssembly outright.
  This changed what Lua contracts return, `number` to `i64`, so chains from before it differ.
- **Scripts never see a memory address.** `tostring({})` is `table`, not `table: 0x…`, and
  `error({})` fails with `table`: an address differs from node to node, and a script that
  returned or stored one would split the chain.
- **Tables keyed by tables iterate the same everywhere; userdata is no key at all.** Plain Luau
  hashes such keys by address, so `pairs` would differ between nodes; the Luau fork hashes tables
  and functions by the order a script made them instead. An Address or u256 as a key fails, "table
  index is a userdata; use a string or a number", as each is a new value and two equal ones would
  be two keys: key by `who:text()` or `tostring(n)`.
- **The `number` kind holds whole numbers, spelled one way.** Digits, within 2^53: write it with
  `ns:set_number` in Lua, not `tostring`, which spells 10^15 as `1e+15`.
- **Each call in a chain of calls gets its own memory limit.** The worst case is 8 nested calls ×
  32 MiB in one transaction on the dev chains.
- **Lua's memory limit counts the call's own bytes, from zero.** The Luau fork counts what each
  object asks for as it is made, less what the collector frees, from the call's start. The
  allocator's pages are not counted: which allocation needs a new page depends on where earlier
  frees left room. mlua's own limit is off for Luau, and `Lua::set_memory_limit` sets the fork's.
  The pristine state's 95 KiB or so is not counted, where the whole heap of pages, about 464 KiB,
  used to be. See [Memory](contracts.md#memory).
- **Luau's collector never reads the clock.** Upstream starts a cycle early by a guess made from
  the clock: how fast the program allocates, and how long marking takes. On a loaded machine that
  freed garbage at other points, and a call near the memory limit failed at another allocation,
  for other fuel. A block built and the same block imported then gave different receipts (seen
  2026-11-05, in `building_big_strings_stops_at_the_memory_limit`). The fork starts every cycle
  once the heap reaches the goal. Pristine states are fully collected once built, so each call
  starts with the collector paused and the same heap. The same goes for weak tables, which lose
  entries when the collector runs.
- **The WebAssembly `call` import takes a value pointer:**
  `call(address, value, name, name_len, args, args_len)`. Rebuild contracts made against the
  older five-argument form. Contracts can also import `emit` and `delegate`.
- **Every call gets a fresh engine state, on purpose.** Reusing one would let top-level locals
  survive between calls and make memory limits depend on what a node ran before, which could split
  the chain. Instead, the Lua engine keeps fresh states ready and the WebAssembly engine caches
  compiled modules. Both only change speed, never results.
- **Lua changes few value types; values lists none.** Integers keep their type through Lua, but an
  amount comes back a `u256`, a `number` an `i64`, and bytes that read as text a string. To pass
  values on as they came, forward `ctx.args`, as the Lua proxy does: calls given a values list
  return one, untouched.
- **Integer arguments arrive in Lua as their own types.** A `u64` is a `U64`, an `i64` an `I64`, up
  to `I256`, and `ns:get_u64` and the like read back the same way. They used to arrive as a `U256`,
  or a plain number for the signed ones, so a contract that did `for i = 1, n` with an `i64`
  argument now needs `n:tonumber()`, or a `number` argument; and chains from before differ.
- **Signed and unsigned integers do not mix.** `u128(5) + i128(1)` fails, "convert one side",
  as no type holds both `U256` and `I256`. One signedness widens, `u64 + u256` to a `U256`, and a
  plain number takes the integer's type. Comparisons mix freely, by the true value.
- **`/` and `%` round as Rust's do; `//` as Lua's.** For the signed types `i64(-7) / 2` is `-3`
  and `i64(-7) % 2` is `-1`, as in Rust and the SDK's `I256`, so the two engines agree; `//` rounds
  down, `-4`. Plain numbers keep Lua's arithmetic, but whole: `/` on them fails unless it divides
  evenly.
- **Lua numbers are whole, within 2^53 − 1, in the Luau fork.** A fractional literal does not
  compile, and `7 / 2`, `1 / 0`, `2 ^ -1` or a sum past the bound fail where they happen. `//` and
  `%` are exact, worked out in integers. The bound is 2^53 − 1, not 2^53, because a sum whose true
  value is 2^53 + 1 rounds to 2^53, and allowing that would let a wrong result through. Fractions
  still reach a script from the host, such as an argument a Rust caller passes, and compare
  exactly; arithmetic on them is checked like any other (see [Contracts](contracts.md)).
- **`math.sqrt(4)` fails even though it would fold to 2.** The compiler is told the math
  functions scripts lack are not builtins, so it folds none of them through the platform's C
  library, and a call compiles to a lookup that finds nothing, as at run time.
- **Numbers compare with integers, in the Luau fork.** `u256(5) == 5`, `x > 1000` and
  `table.sort` over a mix all ask the integer's metamethods, exactly: past 2^53, with fractions,
  and against NaN, which orders against nothing. Plain Luau never calls `__eq` across types and
  fails to order a number against a userdata. A table against a number still behaves as in plain
  Luau.
- **Delegate only to code you trust.** It runs with this contract's storage and balance, all but
  the contract's own space, which it can only read.
- **Storage has namespaces and an own space.** `storage.namespace(name)` and `storage.own`
  (`storage::namespace`, `storage::own()` in Rust) have keys of their own, apart from plain
  storage. Code a contract delegates to can read its own space but never write it, so the
  example proxies keep their owner and implementation there. See
  [Contracts](contracts.md#storage-namespaces).
- **Declared layouts are enforced.** Code with `--@namespace` and `--@key` lines, or the SDK's
  `namespace!`, reaches only what it declares: its namespaces, a record's keys, values of their
  kind, and plain storage only to read and clear. New code must keep every namespace and key with
  the same kind, or `--@retired` it: `Tx::Upgrade` refuses it otherwise, and the example proxies
  check with `check_layout` before repointing. Retired things stay retired, so rolling back to
  code from before a retirement is refused too. `chainv4 layout new.lua --from old.lua` checks
  before you send. See [Contracts](contracts.md#storage-layouts).
- **Kinds are encodings; units are their meaning.** A value written to a declared key must be its
  kind's encoding. Lua writes text (`decimal`, `address-text`) and Rust bytes (`u256`,
  `address`), but each engine has methods for the other's, so a Lua version and a Rust version
  share any namespace. A unit, `decimal:wei`, is the layout's word for what a number counts: the
  node keeps it across versions, but cannot check it.
- **A map's key kind is checked on reads too.** Code that forms a key another way than the map
  declares, `who:hex()` for `who:text()`, fails at once instead of reading an empty entry. Key
  kinds and units may be added to a deployed layout once, then never changed.
- **Loading code is charged.** A deploy or upgrade pays 1 fuel per byte of code to compile it,
  plus whatever a Lua script's top level burns, even when it fails. Every call pays for the top
  level again, since each runs it fresh, before the call is attached: a top level sees no `ctx`
  or `storage`.

## The faucet

- **Its limits survive a restart.** Who had a drip when is kept in the database's `faucet` column
  until each period ends, IPs among it.
- **Behind a proxy, the IP is the last X-Forwarded-For names,** and only when the connection comes
  from this machine or a private network: the Docker stack's nginx appends the real one. A node
  reached straight from the internet goes by the connection.
- **The faucet's key is hot:** it sits on the node. Fund it with what you would give away, and top
  it up as it drains; `GET /faucet` says what it holds.
- **Work is bound to the window it names,** ten minutes, and the faucet takes the last one too: a
  clock far off fails the work.

## The signer's codes

- **The pairing says where the account is.** `ur:chainv4-pairing` carries the account's number and
  the seed's fingerprint beside its public keys, for an account words make; one brought in as a
  key says nothing. Older pairing codes no longer read: pair again, as both devices update
  together.
- **The codes are URs now, and only URs.** `CV4:1:` and its parts are gone: a signer or sender
  from before cannot read today's codes, nor today's theirs. Update both devices together.
- **A third longer, but never a full round to wait.** UR's bytewords take two characters a byte,
  against base45's one and a half, so one code holds 300 bytes where it held about 400. Long ones
  animate fountain-coded parts, of which any enough will do.
- **Inside, the chain's own encoding.** Only the transport changed: what is read is checked as
  strictly as before, and anything past a message's encoding is refused.

## MetaMask's QR hardware wallet

- **The pairing code shows every Ethereum address the words make.** It is the extended public key
  at m/44'/60'/0', so whoever sees it can list every account below it, as MetaMask does. No key.
- **The signer signs for any chain.** It is offline and cannot tell which chain is real: set
  **Your chain** in its Settings, and any other is named and warned of on the review. A native
  request or a multisig's bundle for any other is refused outright: those come from your own
  sender, for your own chain, while MetaMask may rightly ask for many.
- **v is what MetaMask's keyring expects for each kind:** a typed transaction's parity alone, a
  legacy one's EIP-155 v, as many bytes as it takes for a large chain id, and 27 or 28 for messages
  and typed data.
- **Only words pair.** MetaMask derives accounts from the extended key, which a key brought in
  alone has none of.
- **The codes are written in capitals.** A UR's bytewords are letters alone, which QR's
  alphanumeric mode packs densest in capitals; readers take either case.

## Bitcoin wallets

- **The pairing code shows every Bitcoin address the words make on its network.** It is the
  extended public key at m/84'/0'/0' (or m/84'/1'/0'), so whoever sees it can list every
  receiving and change address below it, as the Bitcoin wallet does. No key.
- **Testnet keys are other keys.** Testnet, signet and regtest share coin type 1, so their coins
  live at m/84'/1'/0' and never share a key with real ones; their addresses show as `tb1…`, regtest
  included. A PSBT on both networks at once is refused.
- **Only native segwit, only SIGHASH_ALL.** Inputs of this seed's that are legacy, wrapped segwit
  or Taproot, or that ask to leave some of the transaction open to change, are refused, not
  skipped. Inputs of other keys are left alone, and counted on the review.
- **Change is checked, not taken on the wallet's word.** An output is marked as coming back only
  when the key at its path is this seed's and its script pays that key's hash.
- **Amounts are proven by the previous transaction when it is there.** Its id is what the input
  spends from, so its amounts cannot lie; without it, a wallet that lies about the amounts across
  two signatures of the same coins could make the fee larger than shown.
- **The answer goes back in the name the request came in:** `ur:crypto-psbt`, or `ur:psbt`, the
  registry's newer name, which some wallets use.
- **Signatures are RFC 6979's,** with no extra entropy, so the same PSBT signs the same twice, as
  bitcoinjs-lib signs it; not ground for a low R, so some are a byte longer than Bitcoin Core's.

## More addresses per account

- **Ethereum's and Bitcoin's further keys share one path with every account's first,** so an
  account's second Ethereum address is the next index no account and no further key takes: what
  MetaMask lists as another account, and Sparrow as another receiving address. A new account
  skips the indices further keys hold, or two accounts would hold one key.
- **The Edwards schemes' sit below their account,** at `…/s'/k'`, which no other wallet uses.
- **The vault keeps them,** by scheme and index, so restoring words in a new install brings back
  the accounts but not their further keys: add them again, and they come back the same.
- **Sites reach a watched account's first Ethereum key alone:** the request names the key by the
  account's place under the signer's words, which a further key is not at.

## Atomic swaps

- **Whoever starts holds the secret, and locks first.** Their lock has the longer deadline, so
  once they claim the second lock, showing the secret, the other side still has hours to claim
  the first. The secret leaking early lets the other side take the starter's lock unpaid: the
  wallet keeps it sealed in the vault.
- **Lock second only once the first is confirmed** with the amount, recipient and deadline the
  terms say: the Swaps page reads chainv4's from the contract; Bitcoin's you check, or look up
  with Esplora.
- **Deadlines are times on both chains,** chainv4's by block time and Bitcoin's by its median time
  past, an hour or so behind the clock: a Bitcoin refund confirms about an hour after its
  deadline. Leave hours of margin, and claim early.
- **A secret is exactly 32 bytes** on both sides, so neither chain takes one the other refuses.
- **Anyone may send a claim or a refund** on chainv4: the coins only go to the recipient or the
  locker.
- **Swaps are between any two chains now:** the terms are two legs, the starter's and the
  other's, each on chainv4, Bitcoin or an EVM chain by its chain id. Codes made before, chainv4
  against Bitcoin, still read; `chainv4 swap offer` takes `--pay` and `--get` legs in place of
  `--pays`, `--sats`, `--amount`, `--token` and `--testnet`.
- **Swap.sol sits at one address on every EVM chain** it was deployed to through the
  deterministic deployment proxy. The terms still name it, so a chain without the proxy works
  with a plain create's address. `chainv4 swap evm-deploy` deploys it.
- **Locking second checks the first lock,** where a contract holds it: the CLI refuses, and the
  wallet's button waits, until it stands as the terms say. A first lock on Bitcoin is yours to
  check (`--anyway` in the CLI).
- **The starter is warned off a short second lock:** claiming it would show the secret, and with
  it the whole first lock, for less.
- **An EVM leg costs gas on that chain** to lock and to claim: both sides need some of its coin.
  The wallet signs EVM legs with the account's first Ethereum key.
- **An EVM leg is only as true as its JSON-RPC:** whether the first lock stands is what that URL
  says. Use your own node, or a provider you trust; a secret it reports is kept only if its
  SHA-256 is the swap's hash.
- **Contracts can read the block's time and height** now, `ctx.time` and `ctx.height` in Lua,
  `block_time()` and `block_height()` in the SDK: the header's, the same on every node. A node
  without them refuses such contracts, so every node updates together.
- **Swaps are kept by an id of all their terms now,** in swap.lua and Swap.sol alike: the
  keccak256 of the hash, locker, recipient, token, kind, NFT id, amount and deadline in Solidity's
  ABI. A stranger who saw the hash and locked under it first used to refuse the real lock for
  good; now theirs is a swap of its own. Claim, withdraw, refund, get and item take the id, which
  the apps work out from the terms (`Terms::swap_id`) and `chainv4 swap show` prints. Both
  contracts redeploy with it, Swap.sol at a new CREATE2 address, and codes made before still
  read.
- **A claim whose payout is refused still stands,** with its secret, its payout owed; anyone may
  send `withdraw(id)` to pay it once the token lets it, and a claimed swap is never refunded.
  Before, a token the other side controls, paused, upgradeable or refusing in a hook, failed the
  claim whole, yet the secret was mined in it, and they took both sides.
- **swap.lua checks what arrived:** a token's and a multi-token's balance before and after, and an
  NFT's owner, as Swap.sol does. It refuses coin sent with a claim, withdraw or refund, and a lock
  for the zero address.
- **A Bitcoin HTLC's claim never expires:** Bitcoin's script cannot say "before", so a claim is
  mined after the lock time until a refund is. With Bitcoin as the second leg the starter could
  claim it late and take back the first lock. Whoever pays a second lock on Bitcoin must refund it
  the moment Bitcoin takes the refund: the wallet does so by itself at a high fee
  (`htlc::URGENT_FEE_RATE`), and `chainv4 swap bitcoin-refund --esplora … --wait` from the CLI.
  Both warn when Bitcoin is offered as the second leg.
- **`Terms::check` refuses deadlines that wrap or run on too long:** the gap is added checked,
  the first deadline must be ahead, and none may be more than a year on (`MAX_SPAN_MS`) nor past
  what its leg's chain can write.

## The DEX

- **Contracts can clone contracts now:** `contracts.clone(template, salt, ...)` in Lua, `clone`
  in WebAssembly, a copy of another contract's code at an address fixed by the cloner and the
  salt, its init run with the arguments. A consensus change: every node updates together.
- **The native DEX names the coin by the zero contract address,** `contract:0x00…00`: the zero
  key, which swap.lua uses inside, is no valid key, and transactions refuse it as an argument.
- **ERC-20s in native pools** are approved to the DEX's EVM address, as the EVM sees it, not its
  `contract:` one.
- **The two DEXes are separate:** their pools do not share liquidity, and a trade goes through
  one or the other. The EVM DEX trades the coin as WDEV, and Lua tokens not at all.
- **Gas estimates are checked now:** `POST /offer` and `eth_estimateGas` try the limit they give
  and raise it until a run succeeds. A DEX router's nested calls need more gas than they use, and
  failed with no reason before.
- **A pool's chart** comes from the node's event index: a node without `--index-events` shows
  none.
- **Deploying the DEXes** is `chainv4 dex deploy` (and `--evm`), then naming them to clients:
  the directory's `dex` and `dex_router`, or `--dex-contract` and `--dex-router`.
- **`dex add` works its pair out with the contracts' own rounding.** Neither contract refunds a
  token beyond the pool's ratio: each takes the largest pair within the most given, and refuses one
  under the least. So the CLI's least, the pair less the slippage, is exact only because
  `dex::add_amounts` rounds each step down in the same order as `dex.lua` and `Dex.sol`'s
  `_amounts`; change one and change the other. The amounts given go as the most, which is also what
  it approves, since a price moving within the slippage can take up to that.
- **The CLI approves only when the allowance is short,** reading it first; an allowance it cannot
  read counts as none, so it approves rather than fail.

## Bridges

- **`received` and `voided` read by the peer in force** unless a third argument names another
  gateway: what arrived is kept by the gateway it came from, so after `set_peer` the old peer's
  transfers are asked about by its address.
- **A native token, NFT or multi-token is sent only if it really moves in:** the gateway checks its
  balance or the NFT's owner, so a test token needs a working `balance_of`, and `owner_of` for an NFT.
- **The gateway's setup takes no chain id:** it reads `chain.id()`, the id the chain's EVM answers
  `CHAINID` with, so no owner can give it another. `setup` takes the three templates alone.
- **`Transfer::from` or `to` for a gateway here is its `identity()`,** not its address: its 33 bytes
  and the time it was set up. Read it with `identity()`; Gateway.sol's peer is that, 40-odd bytes,
  and `chainv4 bridge peer` sends it. A gateway set up anew needs `peer` again.
- **Gateway.sol's `delivered` key takes the source gateway too:** `Transfer::delivered_key`. A chain
  restarted from the identical genesis counts from nonce 1 again, and would otherwise find every
  nonce taken.
- **A pass of `sign`, `relay` or `watch` gives no error for a refused transfer.** It logs it, and
  under `watch` lists it each pass, `still refused: …`: watch stderr, not the exit code.
- **A signer refuses a chain the directory does not list,** including every chain on a chain with
  no directory. `--evm-gateway CHAIN:0x…` is its operator's own way round that, naming that very
  gateway. The sign and relay passes say why, and `send` refuses such a chain up front.
- **A signer relaying pays gas with `--evm-key`:** under `watch` too, so fund it on each EVM chain,
  or every delivery stays listed.
- **A ban outlives a restart:** the node keeps each in its database until it ends. `--unban-all` at
  start, or the operator's `POST /node/unban`, lifts them; deleting the database is no longer the
  only way.
- **Peers of one public IP share a backlog cap, four peers' shares by default:** a host running many
  nodes behind one public address may want `--max-gossip-per-ip` higher. Tor and private addresses
  are not capped together.
- **A peer that asks for sync faster than `--sync-rate` past `--sync-burst`** gets no answer, and
  kept up, is disconnected as a flood. An honest node asks one batch at a time.
- **A block gossiped before its parent is held, up to 32, for 2 minutes,** and goes in as soon as
  the parent does; `GET /node`'s backlog counts the held ones as waiting.
- **A Bitcoin withdrawal takes only an address on the Bitcoin the relay follows:** `tb1q…` beside a
  testnet relay, `bcrt1q…` on regtest. The CLI and the wallet refuse another network's before
  anything burns, as its output script would be paid there, where no one meant it.
- **Under `watch`, a delivery to an EVM chain can take up to two turns,** 90 s each by default,
  while the first signer in line is down or slow: signers take turns so only one pays.
  `--relay-turn` sets the turn. A `bridge relay` run by hand goes at once.
- **A signer under `watch` reads only what is still to do:** a transfer it signed, or one delivered
  or voided, is never read again while it runs, so a pass stays small however many came before.
  Restarted, it reads them all once more. A read the node turns away with a 429 is asked again
  after 1, 2 and 4 seconds, so a public node's `/call` limit (5 a second, `RATE_CALLS`) slows a
  pass rather than failing its steps.
- **`chainv4 bridge refund` takes two runs by each signer's operator:** one signs the void, and once that
  is voided and final, one signs the refund, each run reading both chains and doing the next step. Never
  the other way round: the signatures are all a source gateway has to go by, so it is the signers' reading
  of the void that keeps it from paying twice.
- **`delivered(key)` stays false for a voided transfer:** it says what arrived, for the apps, and
  `voidedAt(key)` says the block it was voided in; `deliver` and `voidTransfer` each refuse both. Here,
  `received` and `voided` are the same two. A signer's and a relayer's passes skip what is voided.
- **`sent` here has a 14th value now,** the height it left at, and Gateway.sol has `leftBy(nonce)`, who sent
  it and the block, as a signer may call a transfer final only by those.
- **The gateway here keys what arrived by source chain and nonce,** not by the source gateway as Gateway.sol
  does: a new Gateway.sol's nonces, from 1, would find old ones taken. Both are redeployed together.
- **Signers need the event index:** `chainv4 bridge sign` reads what left here from `Sent` events.
- **Wrapped coins go back as the coin:** a wrapped ETH token sent back says kind 0, so its gateway
  releases ETH.
- **Deploy Gateway.sol plainly, not through a deployment proxy:** whoever creates it owns it, and
  only the owner sets its peers.
- **An EVM chain must have its own chain id:** two gateways on chains with the same id cannot tell
  a transfer's ends apart.

## Bitcoin's bridge

- **Hashes are written two ways:** explorers and Esplora write a txid or block hash reversed;
  the relay and the gateway take and give them in Bitcoin's own byte order, and the gateway's
  `utxo` keys them so. `chainv4 bridge btc` turns one into the other.
- **The relay is only as current as what was submitted:** a deposit shows once its block is in the
  relay, `bitcoin_depth` deep; run `relay-sync` before `deposit` and `released`.
- **Start main at a retarget:** the relay needs the period's first block to check the next
  difficulty, so `relay-start` on `main` takes a height divisible by 2016.
- **The relay is what says which Bitcoin a chain follows:** nothing in the federation or the
  directory does. `relay-start` takes it from Esplora's genesis block unless `--network` says, and
  `btc address` writes for the relay's `network_name()`, then Esplora's genesis, then `--network`,
  then mainnet, saying which on stderr. Testnet4, testnet3 and signet are all `test` to the relay,
  sharing `tb1…` addresses; regtest's are `bcrt1…`. The same script on another network is another
  coin, which the relay never sees.
- **A deposit address is per address, federation, chain and gateway:** `btc_deposit_script` takes
  all of them, in Lua and Rust alike. A new chain or a new gateway makes new ones, so one given out
  before a reset pays what nobody will mint: tell people. Sync the relay before `btc deploy`: a
  gateway mints only what was mined past the relay's tip when it was set up.
- **A new federation makes new deposit addresses:** pay
  an old one only within the grace period, after which its deposits do not mint. The grace counts
  from the block that decided the next federation, which the chain keeps with every federation it
  decided (`federation.at`), not from when the gateway next ran: one the gateway never ran under
  mints all the same.
- **Deposits start at 10,000 satoshis** unless the gateway was set up with another least
  (`least_deposit()`, `btc deploy --least-deposit`): a payment under it mints nothing and stays on
  Bitcoin. `btc address` says it on stderr.
- **A coinbase paying a deposit address mints once 100 deep,** whatever `bitcoin_depth` says, as
  Bitcoin lets nothing spend it before.
- **The planner takes the largest outputs first,** and leaves out any worth no more than spending
  it costs at the rate, so dust the gateway holds, as a gateway set up with a low least deposit may,
  is never dragged into a release.
- **Withdrawals start at 10,000 satoshis,** more if the floor is high, and a release pays at most 50.
- **A withdrawal pays its own fee.** It arrives as what it burned less its share of the release's
  fee, split evenly, up to the most it agreed to lose, a tenth unless it says. So `--fee` to `btc
  propose` comes off them, not the pool, and change is exactly what was spent past what they burned.
- **Every release keeps change of at least 330,** Bitcoin's dust for a P2WSH output, so a child can pay
  for it: should the gateway hold no more than they burned, the withdrawals top it up to that with the fee,
  and `withdrawal(id)` counts it in their share. Dust was 546 before, a P2PKH output's.
- **Releases signal BIP125:** each input's sequence is `0xfffffffd`, so Bitcoin takes a replacement paying
  more, and its top bit leaves no BIP68 delay. A gateway from before took `0xffffffff` alone, so its
  releases can only be paid for by a child: the gateway is redeployed with a new genesis.
- **A bump is a proposal of its own:** `replace` and `child` take a number as `propose` does, signed over
  `btc_bump_digest`, and are signed input by input like any release. `proposal(id)` gives four more values
  after the state, its kind, what it replaced or pays for, its fee and what its withdrawals lose by it, and a
  proposal another version of which was shown is `replaced`. `tree(id)` lists a release and its bumps.
- **One bump at a time:** while any bump of a release waits for signatures, another is refused, and a
  child of a change a child spends already is too: replace that child instead.
- **A change a child spends before it is shown is `unshown`** in `utxo_of`, and unlisted: `utxo_count`
  counts it once the release it is of is shown, still pending under the child or spent.
- **A child's fee is the backing's one cost, for a while:** the withdrawals it is charged to were paid, so
  the federation pays it from its change, and `shortfall()` says how far what the gateway holds is short of
  wBTC, then how much of that open releases repay. Later releases repay it, a fifth of their fee at most and
  500 satoshis from any one withdrawal, within its most fee: so a withdrawal's quote, from `bridge btc
  withdraw` or the wallet, says a little may come off toward it while anything is owed. The gateway works
  out what each repays; a release that does not carry just that, or a replacement that changes it, is refused.
- **`set_most_fee` raises a limit while its release is open,** for a bump its limit held back; it never
  lowers one in a release, as what the release took stands.
- **A lowered most fee holds 30 blocks later,** while its withdrawal is queued; a raised one at once. A
  release proposed before then is judged by the old limit, so lowering one cannot undo a release planned
  against it. `withdrawal(id)` gives the lower one and its height after the rest, and the planner goes by
  it from the moment it is asked.
- **A sweep's fee is owed to the backing:** it pays no withdrawal, so once shown it is added to
  `shortfall()`, as a child's is, and later releases repay it.
- **Signers take turns:** a pass waits its place among the federation's signers times a block of the chain,
  at least a second, before it shows, votes, proposes or bumps, then checks the gateway and the node's
  mempool again. A large federation's last signer may wait a while each time; the first never does. A pass
  with no `--evm-key` goes last.
- **A bump by policy needs fee estimates and ages:** no `/fee-estimates`, as on regtest, or no
  `--index-events`, and nothing is stuck by policy; `bridge btc bump --rate` still goes by hand.
- **A proof needs the block's coinbase:** `deposit` and `released` take it and its path, without its
  witness. Esplora gives it as `/block/HASH/txid/0`, then as any transaction. A wallet's coin kept
  before that is shown through Esplora instead.
- **`setup` takes a fee floor:** `setup(relay, template, fee_cap, fee_floor, grace_ms, least_deposit)`,
  the floor in satoshis a virtual byte, at least 1, and the cap at least 710 virtual bytes of it, or it
  is refused; the least deposit is 10,000 satoshis when left out. A release paying less for its size than the floor is refused too: `--fee` is checked
  against it before anything is sent.
- **A withdrawal's script is one nodes relay:** P2PKH, P2SH or a witness program, not any 80 bytes.
- **Only a signer proposes a release,** `propose(raw, signature, ids…)` over `btc_propose_digest`, as a
  release's fee comes off the withdrawals and anyone could otherwise burn what they agreed to lose.
  Anyone may post it. The digest takes the number the release will have, so a signature is good until
  any release takes that number: a cancelled release's own does not bring it back, and two signers
  building the same release race for the one number, the loser's transaction failing at worst once.
- **Something has to run a pass.** `bridge btc watch`, or `bridge watch --esplora`, or nothing is ever
  proposed, signed, broadcast or shown. A withdrawal says why it waits in the pass's log, not on chain.
- **`/fee-estimates` is Esplora's,** and mempool.space's API has it too, a rate by how many blocks for.
  A pass reads it once, asking twice at most, as a read that goes unanswered must not hold it up. No
  estimates, as on regtest, proposes at the floor; out of reach proposes nothing and signs up to 200
  satoshis a virtual byte.
- **A signer's limit is against what it reads now,** not when a release was proposed, so a release from
  a rush can fall past 3 times the fastest rate as the network calms. It is then refused until cancelled
  or `--max-fee-rate` says otherwise: nothing stays held past a cancel.
- **`Esplora /tx/ID/status` says a transaction it never heard of is unconfirmed,** so a pass asks
  `/tx/ID/hex` too before it broadcasts.
- **A cancelled release can still be mined.** Its signatures are public, the board only stops
  counting them. So the withdrawals it paid must spend one of its inputs, `anchor(id)`, in their next
  release, until that input is shown spent; `bridge btc propose` does this itself, and one whose
  anchor is in another release waits for it. Show a cancelled release that was mined with
  `released` all the same, so the gateway knows.
- **A signer that voted to cancel stops signing that release,** as its signature would take it to
  the threshold, where no vote stands.
- **`bridge btc pass`, `watch` and `bridge watch --esplora` vote to cancel by themselves;** `bridge btc sign`
  by hand never does. A vote cannot be taken back, so a limit lower than the rest of the federation's costs
  them a release at the next proposal's fee. A failed read, or a limit with no fee rate behind it, gets none.
- **A release's age is its proposal's block time against the tip's,** from `Proposed` events: the gateway
  keeps no time for a release, and was not changed for this. No `--index-events`, or an event pruned, and
  it has no age: no vote for it, no warning for it, and what it paid holds back a new proposal at about
  that rate for good, short of `propose --fee` by hand.
- **A signer does not propose what it would refuse,** so with `--max-fee-rate` under what the network asks
  it proposes nothing, and says so. Nor does anyone propose a withdrawal again near the rate a young
  cancelled release paid it at: it waits, says why, and goes at a quarter less or past `--cancel-after`.
- **A stuck release's warning goes by when it was proposed,** not when it got its last signature, so one
  signed late can warn soon after it is broadcast.
- **Releases are given without their witnesses:** the gateway reads and checks the base
  transaction, as its txid hashes it; the witnesses are assembled after, from the board.

## The directory

- **Names no longer find contracts:** `swap`, `dex`, `nfts` and `gateway` under the suffix were
  anyone's to register first, and a wallet would have paid whoever did. The chain's directory
  names them now, decided by governance; a name registered for one finds nothing.
- **Genesis writes its directory and federation into the state,** so their state roots, and so
  their genesis ids, differ from a chain with none: one entry each, proven like a balance.
- **The node's own word comes last:** `--swap-contract`, `--dex-contract` and `--dex-router`
  still answer when the directory names none, and only for what it leaves unset. `GET /status`'s
  `swap` and `dex` were the flags' until 2026-11-03, whatever the directory said; now they are the
  directory's, and the flags show apart as `operator`. The apps and the CLI took the directory
  first already, so none of them changed.
- **A signer checks the directory first:** `chainv4 bridge sign` and `relay` refuse a gateway
  peer on an EVM chain other than the one the directory names there.

## Bridges in the apps

- **On the desktop and Android, contract reads are the app's own.** It runs each over the state
  the node proves; one that lists storage, or anything in the browser, is the node's word, and the
  Contracts page says which. A node answering otherwise than its own proofs shows up as `differed`,
  and in the explorer's feed.
- **A Bitcoin deposit can be for any address.** The deposit address is made for one recipient, of
  any scheme, and anyone may mint it: the wallet's **Deposit to** picks another of the account's
  addresses or takes one typed, and the CLI's `bridge btc address RECIPIENT` and `deposit … RECIPIENT`
  always did.
- **The mining handshake covers the hello.** A worker's name, payout and whether it mines alone are
  under the MACs, so a worker and node of different `mining/` versions do not talk. An open port's
  token is public, so someone on the path can still make those MACs: serve it over wss://, or keep
  it on a private network, if a miner's payout matters to it.
- **Deposit addresses are computed, not asked:** the wallet builds the account's deposit address
  from the proven federation and directory, and the chain's id from its genesis, so a node cannot hand it one of its own; the gateway's
  `deposit_script` says the same, for the CLI.
- **The light client keeps a deposit's proof as it finds it:** its block, the transaction
  without its witness, its place and Merkle path, for Mint wBTC. Coins found before an update
  have none: show them through Esplora.
- **The wallet lists your transfers out and withdrawals among the latest 20** each, by who sent
  them, which `gateway.lua` and `btc_gateway.lua` record now (`sent`'s 13th value,
  `withdrawal`'s 4th).
- **From an EVM chain, a token's amount is in its smallest units,** as the wallet does not ask
  the token there its decimals.
- **Both apps say where a transfer stands alike:** `chainv4_ui::bridge::Standing`, the furthest along
  first, refunded, voided, arrived, stuck, so the same whatever order its events came in. A transfer
  read back from `sent(nonce)`, `chainv4_ui::bridge::left`, stands only if its fields hash to the
  digest the gateway keeps.
- **An `Attested` event names only the digest signed:** a signature to void or refund a transfer
  looks like one more transfer arriving. The explorer works the void and refund digests out for
  what left, and asks `void_digest` for what was voided coming here. A refund of that is signed
  over who Gateway.sol says sent it, `leftBy`, which only that chain knows, so for a signature
  nothing else places the explorer reads the `attest_void` or `attest_refund` call that posted it,
  which names the transfer, its chain and its nonce: up to 50 of them, a transaction and its
  block each. A transfer refused before it ever showed here is listed from those calls, as one
  coming from its chain.
- **Stuck goes by the directory, as the signers do:** a transfer whose recorded digest is not the
  one worked out toward the Gateway.sol the directory names there now left for a peer since
  replaced. The wallet leaves such a one out of its list rather than say so.
- **Neither `Voided` nor `Refunded` says why:** `chainv4 bridge refund --why` is shown to its own
  operator alone, so no app can show the reason.

## Crypto and the federation

- **New host calls and a precompile are consensus:** a contract calling `crypto` or `federation`,
  or an EVM call to `0x…4e56`, runs differently on a node without them. Nodes update together.
- **Each scheme signs its own way:** `secp256k1` verifies over the message's keccak256, as
  Ethereum keys sign, `bitcoin` over its double SHA-256; `recover` takes a 32-byte digest, as
  `ecrecover` does. Hash the message the way it was signed.
- **Proofs are checked by this chain's hash:** another chainv4 chain's state or receipts check
  only if it hashes the same.
- **The federation is versioned:** genesis's is 0, and each decided by governance one more, so a
  signature can name the federation it was made for.

## Maps and sets in the code

- **Consensus never iterates a hash map.** Execution, state, receipts and roots keep `BTreeMap`s
  where order could show; a `HashMap` there is only looked up. Keep it so: a node iterating one
  could build, or reject, what another does not.
- **Grindable keys keep SipHash.** Transaction and block ids, peer ids and anything a peer or a
  request sends are keys an attacker can pick; std's randomly keyed hasher stands against
  flooding, where FNV, Fx or an identity hasher would not.
- **Order shows where nodes or users see it:** the proofs a block carries and `GET /forks` come
  from ordered maps, so the same node answers the same way.

## NFTs

- **Two standards, one registry:** NFT collections (one owner a token) and multi-token ones
  (amounts of each id), listed by `GET /collections`. A contract exporting both lists is an NFT
  collection: the NFT list holds the multi-token one. See [NFT standards](nft-standard.md).
- **The running chain needs governance to list them:** a genesis without `collections` keeps no
  registry until a council or electorate decides `Change::Collections`; contracts deployed before
  join at their next upgrade or refresh. New dev chains have it from genesis, which changes their
  genesis id.
- **Safe transfers call the receiver;** plain ones do not. Send to a contract with `safe_transfer`,
  and pull a token into a contract with `transfer_from`, as the swap contract does.
- **The wallet proves ownership, not completeness:** each NFT shows once its owner is proven, but
  which ids to look at is `tokens_of`'s word.
- **Pictures leak what you look at:** the wallet fetches metadata and pictures only once Settings
  lets it, through the proxy, and the gateway sees each one.
- **NFT swaps need the current contracts:** swap.lua and Swap.sol from before NFTs have no `item`,
  and Swap.sol's CREATE2 address changed with its code: deploy both again and name the new swap.lua
  in the directory.
- **Terms that lock an NFT encode with a 3,** which apps from before cannot read; terms that lock
  none keep the 2 they had.

## The explorer's DEX and Swaps pages

- **Under Markets in the sidebar,** and under More on a phone. They find the DEX and the swap
  contract as the wallet does: by the names `dex` and `swap`, else the node's `/status`.
- **Volumes and charts need the event index:** without `--index-events` the DEX page shows pools
  and prices, and the Swaps page finds a swap by its hash alone.
- **Volumes are estimates of a day and a week:** from the last 500 trades at most, by block
  heights at the block time the explorer measures from what it loaded.
- **A swap's code lays out both legs,** the other chain's too, but the explorer reads only this
  chain: an EVM or Bitcoin leg's lock is for its own explorer.

## A node cut off rejoins the validators' chain

- **A node that mined on its own while cut off,** as the laptop did while the VPS rebuilt, now
  moves back to the branch the validators justified however far it got, down to the finalized
  checkpoint. Before, a branch splitting off more than `max_reorg_depth` blocks below its tip was
  never followed, and the node stayed on its own fork until its database was moved aside.
- **Undo records and side branches are kept down to the finalized checkpoint** on a chain with
  finality, beyond `max_reorg_depth` while finality lags: a stalled finality grows them, and they
  go together once it moves on.
- **A node syncs from a peer ahead of it on justification** even when it holds more work itself,
  until it has that peer's blocks.
- **Nodes choose forks differently now,** so the VPS and every node that follows it update
  together.

## Swaps wait for finality

- **The second lock and the starter's claim are held** until the lock they rest on is final:
  chainv4's by its finality (20 blocks deep on a chain without), an EVM chain's by its finalized
  block (12 deep where it names none), Bitcoin's by 3 confirmations (Settings, Swaps). `--anyway`
  and **I checked it myself** act sooner.
- **The swap contracts now say the block a swap was locked in.** One deployed before (the VPS's
  first `swap.lua`) leaves finality unknown, so everything through it is held until overridden:
  deploy the current `swap.lua` and name it in the directory. `Swap.sol`'s `get` returns one more
  value, and its address through the deployment proxy changed with its code.
- **chainv4's JSON-RPC tags mean what Ethereum's do now:** `finalized` is the finalized
  checkpoint's block and `safe` the justified one's. On a chain without finality there is no such
  block, as on proof of work: tools count depth. They named the tip before.

## The Bitcoin light client

- **Off until switched on:** Settings, **Bitcoin**, for Bitcoin, its test network, or both. A
  first sync downloads a few tens of megabytes of filters from the checkpoint built in.
- **It sees blocks, not the mempool:** a payment shows once it is mined, with its confirmations.
- **Coins paid before the checkpoint are not seen,** nor those to an address only added after the
  client passed their block: **Scan again** starts over from the checkpoint.
- **Testnet swaps need both sides on one test network:** testnet4 and signet are different chains
  with the same tb1q addresses.
- **A proxy must be SOCKS5;** an HTTP proxy stops the light client rather than letting it connect
  directly.
- **On the test networks it watches each account's first address alone,** its words' testnet key;
  on Bitcoin, all of an account's Bitcoin addresses.
- **What it found is kept apart from the settings** (the `chainv4-bitcoin` entry), so a new
  install starts from the checkpoint again.

## Anchoring

- **Off until governance turns it on,** each part on its own: taking Bitcoin anchors, taking
  Ethereum anchors, and enforcing them. See [Anchoring](anchoring.md).
- **An enforced anchor binds even a checkpoint that is not final:** no block at or under it is
  imported, and no branch splitting off below it is followed, whatever it weighs or justified.
  Turning `enforce` off later frees nothing recorded while it was on.
- **An anchor's root must be the list's root when it is shown:** an anchor written while another
  was recorded in between is refused, and the anchorer writes a new one. Two anchorers racing
  waste Bitcoin fees; only the first earns the reward.
- **Only Bitcoin itself protects anything.** On testnet4, signet or regtest, headers cost next to
  nothing to make, so a fork could make its own Bitcoin.
- **The relay must keep each header's version and nonce** for a node to serve Bitcoin's headers to
  one syncing with `--anchored`. Relays from before anchoring keep neither, so they need deploying
  again.
- **`--anchored` hears from `anchors.peers` peers, 2 unless tuned,** before it picks a history:
  with fewer it waits. A node that hears only an attacker's peers follows what they show.
- **The anchorer keeps its pending anchors in memory,** each chain's apart: one restarted before
  showing its anchor writes another, at another fee.
- **The two chains take turns:** an anchor carries the list's root, so one recorded in either chain
  wastes an anchor written in the other meanwhile. The anchorer writes into neither while the other
  has one pending, Bitcoin first when both are due, each chain's cadence (`every`) counted from its
  own newest anchor.
- **An Ethereum anchor is calldata to self:** the 80 bytes as a 0-value transaction's whole
  calldata, from the anchorer's key to itself. The chain checks the calldata, not who sent it.
- **An Ethereum anchor's block must be a finalized header the relay holds, or its ancestor:** its
  proof carries the finality update that finalized the header it rests on, and the chain checks it
  matches what the relay holds and that the committee the relay holds for its period signed it. Once
  the relay moves two periods on, it no longer holds that committee, and the proof needs a newer
  update.
- **The anchorer lands Ethereum anchors in an epoch's first slot:** it sends the transaction 8
  seconds before one, so its block is the epoch's checkpoint, which a finality update then finalizes
  itself. One landing elsewhere is written again, as proving it would take an ancestry branch
  through a state's `block_roots`, which no beacon API's light client routes serve, and the anchor
  block's light client header, which they serve only for checkpoints. The chain takes ancestry
  proofs; the anchorer has no source for them yet.
- **The anchorer must see the finality update naming its block,** about 13 minutes after it landed,
  before the next finalization replaces it: its passes, every 60 seconds unless told, do. One missed
  is written again.
- **Without `--beacon`, Ethereum anchors decide nothing** in anchored sync: a list is checked up to
  its newest Bitcoin anchor, and only Bitcoin compares histories. With it, the node follows the sync
  committees from `eth_start` itself, a request per 128 periods, and checks each kept update's
  signature.
- **One list holds anchors from both chains,** in the order they were recorded. Its root covers each
  entry's chain and where that chain holds it, so it changed with them: a chain from before it
  needs a new genesis.
- **Where two histories part, Bitcoin decides if it can:** each history's first Bitcoin anchor at or
  after the parting place, the lower winning, even if Ethereum says otherwise. Only where one of
  them has none does each one's first Ethereum anchor decide, the earlier slot winning. Where no
  chain holds an anchor of both after they part, nothing decides, and the node stops as for a tie.
  A build that reads only one of the chains goes by that one alone.
- **The places compared may differ:** each history's first anchor in the deciding chain can sit at
  another place in its list, so the node may ask each peer for the proof of a different entry.
- **Three features:** `anchoring` is the list, the guard, the reward, `GET /anchors` and
  `--anchored`; `anchoring-btc` and `anchoring-eth` add each chain, and either builds without the
  other. A build without `anchoring-btc` refuses Bitcoin anchors, so it cannot follow a chain that
  takes them.
- **Bitcoin's header rules live once, in `btclight`:** the relay contract and the node's anchored
  sync run the same code, so a change to them changes both, and the relay's code with it.

## Ethereum's light client

- **The presets' fork schedules are built in,** through Fulu on mainnet, Sepolia and Hoodi
  (`ethlight::forks`). A fork after the last signs under a version they lack, so the relay refuses
  every update from it until governance names a schedule that has it, in `anchoring.eth_start`:
  the relay follows that one, through the `eth_network` host call, without a redeploy. Blob-parameter-only
  forks change no version, and nothing here.
- **A network is a whole definition,** not only a name: its genesis validators root and time,
  seconds per slot, slots per epoch, epochs per sync committee period, and its forks with their
  versions and epochs (`ethlight::Network`). Mainnet, Sepolia and Hoodi are presets, by
  `Network::by_name`; another, such as Gnosis with 5-second slots and 16-slot epochs, is made by
  hand, or decoded from its SSZ (`Network::encode` and `decode`, which refuses one `problem`
  refuses). Every network must run the mainnet preset's sync committees of 512 and 8,192 block
  roots. Governance carries one in `anchoring.eth_start.network`, which the relay follows.
- **Electra moved the state's generalized indices:** the finalized root to 169, the current and
  next committees to 86 and 87, one level deeper, as the state grew past 32 fields. An update's
  layout and gindices go by its attested header's fork, a bootstrap's by its header's.
- **Only finality counts:** no optimistic header, no best-update tracking and no forced updates.
  An update needs two thirds of the committee, at least 342 of 512, and must finalize a header
  after genesis's.
- **A finality update has no next committee:** send it as a `LightClientUpdate` with zeros for the
  committee and its branch, as the specs allow.
- **A transaction proven must be in a header the relay keeps:** a finalized one, or an ancestor of
  one within 8,192 slots proven with `prove_header`. One older than that, before any kept header's
  window, cannot be proven any more.
- **At most 8,192 headers are kept,** the oldest kept dropped first, ancestors included; the
  finalized one always stays. Proving many ancestors drops older headers, at a transaction each.
- **The BLS check is strict:** a key or signature that is no point of its prime-order group, or is
  at infinity, or keys summing to infinity, fail it, the same on every node. It trusts that each
  key proved possession, as the beacon chain checked when its validator joined: never feed it keys
  anyone may choose.
- **`fast_aggregate_verify`'s fuel is consensus,** `bls_verify` and `bls_key` in `HostCosts`. They
  are new fields of `HostCosts`, so a chain whose genesis or a decided change named `fees.costs`
  encodes differently from before: a new chain.

## Endpoints

- **Every outside URL repeats:** `--esplora`, `--bitcoind`, `--eth-rpc` and `--beacon`. The first
  is asked; an error, a timeout, a 429, a 5xx or a refused login moves on to the next; the one that
  answered stays in use, and the first is tried again five minutes later. An answer that is a
  failure, a refused transaction say, is no URL failing, and the next is not asked.
- **Several `--eth-rpc` of one chain stand in for each other:** each is asked its chain id once, and
  those of a chain are grouped.
- **Logins are never a flag's own text:** `--…-auth` takes `cookie:PATH` (bitcoind's alone),
  `file:PATH`, refused while its group or others may read it, or `env:NAME`. `user:password` is
  basic auth, anything without a colon a bearer token. A cookie is read at each request, so a
  restarted bitcoind's new one works at once.
- **Only scheme, host and port are shown** of a URL, in logs and errors: no userinfo, path or query,
  where API keys sit.
- **bitcoind needs no wallet,** as coins come from `scantxoutset`, which scans the whole UTXO set and
  takes a minute on mainnet; but finding a transaction already in a block by its id
  (`getrawtransaction`, `gettxoutproof` without its block) takes `-txindex`.
- **`--proxy` is SOCKS5,** `socks5://` or `socks5h://`, which resolves names at the proxy, as Tor
  wants; it carries every outside connection but none to the node's own API.

## The camera

- **The live camera is the wallet's alone.** `chainv4-ui`'s `camera` feature brings it, and nokhwa
  with it on the desktop; the explorer reads no codes and leaves it off.
- **Android asks for CAMERA now.** The wallet declares the permission, and asks the first time the
  scanner opens. A photo from the system's camera app would want it too, so on Android the live
  scanner takes the place of **Take a photo**; **Open a file** still reads a photo taken elsewhere.
- **The extension's side panel cannot ask for the camera.** Chrome shows no permission prompt
  there: open the wallet's page in a tab once and allow it, and the panel has it from then on.
- **Frames are searched the newest at a time.** A search slower than the camera drops the frames
  between, so a code read is never stale; in the browser, where the page has one thread, a frame
  is searched about five times a second.

## The extension's window.ethereum

- **Sites see Ethereum accounts alone.** Only secp256k1 accounts are offered: Ethereum's tools know
  no Ed25519 address. Those the wallet holds keys for sign here; those watched from the offline
  signer sign there, by `ur:eth-sign-request` and `ur:eth-signature`, and only when their pairing
  said where the signer derives them, which pairings from before this did not: pair again.
  MetaMask's accounts are MetaMask's to offer.
- **The signer's answer is checked before it is used.** Its id must be the request's, and it must
  recover to the account over the very hash asked; a send's transaction is then the one priced,
  with that signature, as signing in the wallet would have made it, byte for byte.
- **The provider is off by default.** It needs the browser's leave for every site, asked from a
  click in the wallet's Settings; until then no page gets `window.ethereum`.
- **A site's reads are not proven.** They are the node's word at `POST /rpc`, as any dApp's are;
  only what the wallet itself shows as proven was checked by its light client.
- **What sites may see is kept by the extension,** in its storage, by origin, apart from the
  wallets: removing a wallet does not disconnect sites, and a connected site loses an account the
  wallet no longer has.
- **Typed data is hashed as MetaMask hashes it.** `chainv4::crypto::eip712::typed` follows
  eth-sig-util's version 4, checked against it byte for byte: arrays of anything, a struct left out
  as zero, and the domain's own fields when its types leave EIP712Domain out.

## The EVM

- **Calls between engines go through a precompile and `evm.call`.** The native precompile at
  `0x…4e43` takes a native contract as `bytes32`, its function by name, the native types of the
  arguments as text, and the arguments ABI-encoded; what comes back is ABI-encoded inside one
  `bytes`, as its interface says. See [EVM](evm.md#calls-with-lua-and-webassembly-contracts).
- **A native call sees what the EVM did so far, and is undone with the frame that made it.** It
  runs on a copy of revm's journal. Its writes are kept only while a marker it sets in the
  journal stands, so a `try` that catches a revert undoes the native calls inside it too.
- **Contracts cross as `bytes32`, other addresses as their 20 bytes.** A Lua or wasm contract
  only gets an alias once it sends something into the EVM, so callNative names its target by its
  32 bytes, and a contract a native function returns comes back as `bytes32`. Any address
  encoded for the EVM, by `evm.encode` or callNative's return, has its alias written down.
- **Lua numbers cross as `int64`.** A whole number a Lua function returns to the EVM is an
  `int64`; return a `u64(…)` or `u256(…)` for an unsigned type.
- **A view cannot write through the precompile.** In a static call, a native function that
  changes anything, or emits an event, makes callNative revert.
- **`CallContext.time` is now `CallContext.block`.** A call's context carries the block's height,
  time, producer and the fuel one EVM gas burns, as `BlockAt`, which `Contracts::deploy`,
  `upgrade` and `refresh` take in place of the time; `BlockAt::at(time)` gives one with the rest
  zero, for tests.

- **An Ed25519 account's EVM address is an alias, known only once it sends.** Inside the EVM,
  every account but secp256k1 ones and EVM contracts goes by the last 20 bytes of the keccak256 of
  its key. The chain learns whose alias it is when the account first sends a transaction, of any
  kind. Coin the EVM pays to an alias before then goes to the secp256k1 account with those bytes,
  which no one can spend. An account that has only received should send something before handing
  its alias to a contract. See [EVM](evm.md#addresses).
- **Every transaction writes its sender's alias down, on a chain with the EVM.** It used to take an
  EVM transaction. This changes state, so nodes from before it and after it disagree on the first
  block where an Ed25519 account sends anything else: update every node of a chain at once. Chains
  without `evm` in their engines write nothing.
- **EVM contracts are called with `evm-call`, never `call`.** `Tx::ContractCall` to an `evm:`
  address is refused, and `Tx::Upgrade` cannot replace EVM code: upgradeable EVM contracts use a
  proxy, as on Ethereum.
- **EVM gas includes Ethereum's 21,000 base.** revm counts it on top of chainv4's intrinsic cost,
  so an EVM payment costs more than a plain `transfer`.
- **No randomness, no gas price, no base fee inside the EVM.** `block.prevrandao`, `tx.gasprice`
  and `block.basefee` read as 0, and `block.timestamp` is in seconds.
- **The chain id is the genesis id's first 6 bytes.** A new genesis is a new chain id.
- **The EVM made new dev chains.** The dev genesis lists `evm` among its engines now, so dev
  chains from before have other ids, and a build without the `evm` feature is on another dev
  network.
- **Ethereum's wallets sign transactions the chain takes as they are.** `Tx::Ethereum` holds the
  signed bytes; the sender is the secp256k1 account their signature recovers, at its own nonce.
  Legacy ones must carry EIP-155's chain id, and blob and EIP-7702 transactions are refused. See
  [Ethereum's tools](ethereum.md).
- **Old state is kept 10,000 blocks back by default.** Balances, calls and proofs at older blocks
  answer over the API and JSON-RPC within `--state-history`; `--archive` keeps every height, from
  when the node first ran a version that keeps history. Blocks list only Ethereum-signed transactions, and a transaction is found by hash only
  once a block holds it.
- **An Ethereum transaction's gas is chainv4's.** Its limit covers its bytes' intrinsic cost and
  the EVM's gas as fuel, so it differs from what the same transaction takes on Ethereum; wallets
  ask `eth_estimateGas`, which knows.
- **ABIs are kept per device.** The wallet keeps a contract's ABI with it on its Contracts page,
  and the explorer per chain and contract in its settings. Neither is on chain: a new device needs
  the ABI pasted again.
- **The apps' reads are not proven.** The wallet's and explorer's contract reads are dry runs on
  the node, as `POST /call`, `/offer` and `eth_call` are, unlike the balances the wallet proves.
- **revm brings older crypto crates.** It pins k256 0.13 and sha3 0.11 beside the newer ones
  chainv4 uses, so both are built. It has no KZG precompile, since that needs C.

## Governance votes

- **Yes, No and Abstain.** A change is decided once Yes weighs what governance needs and the
  quorum has voted, Abstain and No included; rejected once what has not voted No could no longer
  reach what is needed. A rejection leaves the kind's version where it was, and the same change is
  refused until a change of its kind is decided.
- **One vote an account on each change, one Yes a kind.** A second Yes in a kind moves the first;
  No and Abstain stand beside it. `Tx::ClearVote` takes one back. Votes expire after
  `approval_epochs`, No as much as Yes.
- **Governance changing itself clears every open vote,** of every kind, and the rest of its block
  refuses votes; the new settings count from the next block.
- **Settings hold from the block after their decision,** read from each block's parent's state:
  host costs, contract limits, names, the DEX's fee and governance's own settings alike. An EVM
  spec runs from the height it names; finality's tuning from an epoch ahead.
- **A limit deployed contracts may rely on never falls,** and `multisig_members` never rises past
  20. Lowering the module or source size bounds only new deploys.
- **The Bitcoin gateway goes by the federation's `btc` policy once it names one,** and by its own
  setup until then; signers go by its `cancel_after_ms` over their `--cancel-after`.
- **Every new setting changes genesis's encoding,** so the devnet moves to them with a new genesis
  (see [Governance](governance.md#what-else-each-kind-carries)).

## Governance locks

- **A lock is a path, compared as JSON.** `reward` holds the whole reward, `anchoring.enforce` one
  field, `limits.tx_limits.fuel` one field two levels down. A vote on a change whose value at a
  locked path differs from the one in force (as `GET /governance`'s `in_force` shows it) fails
  with "`PATH` is locked"; the rest of the kind still changes. A field left out is the same as
  `null`, so a change of variant breaks a lock as soon as what it holds there differs: with
  `consensus.slot_ms` locked, authority may move to staked at the same slot, never to proof of
  work, which has none.
- **Every vote is checked, the one that would decide included,** against what stands then, so a
  lock decided after a change's first Yes still holds it. No and Abstain on a locked change are
  refused too; clearing a vote never is.
- **Locks only grow.** Genesis's `locked` sets the first, `Change::Locks` adds more (in order, each
  once, none locked already, by itself or a lock over it), and nothing removes one; governance
  changing itself leaves them. A lock on `locks` freezes the set.
- **A switch of finality is locked whole** (`switch_finality` alone; `switch_finality.epoch` is
  refused). What a switch sets is held by the locks under `finality`: with `finality.type` locked,
  no switch passes.
- **Locking `upgrade` or `consensus` is for good.** A chain that does accepts that any fix to them
  needs a new genesis.
- **One state entry, `change:locks`,** written at genesis only while it names any, so a genesis
  without locks keeps its id and its state root. Consensus: it ships on the next genesis.
- **The tools refuse first.** `send vote-change` checks the change against `GET /governance`'s
  `locks` and `in_force` before it signs; the wallet's form shows a locked field read-only, marked
  locked, and a kind locked whole as locked in its list; the explorer lists the locks. The chain
  has the last word.

## Names

- **A name's price is burned, by length, per year.** Its fee is paid on top. A registration that
  fails burns nothing, and `chainv4 send` dry-runs name transactions, refusing one that would
  fail before it is sent. See [Names](names.md).
- **Names expire by block time.** A name expires 365 days a year after the block that registered
  it, by block timestamps. It stops resolving then, its subnames too, and only its owner may
  renew it for the grace period. After that anyone may register it, and everything it held is
  gone.
- **Transactions carry names in lower case, without the suffix.** Upper case is refused, not
  folded, so every node reads one name one way; the CLI and the API lower-case and strip what you
  type.
- **Paying a name pays the address it resolved to when you sent.** `send transfer --to alice.dev`
  resolves on the node and sends the address, so a name that changes before the block does not
  redirect the payment.
- **A primary name lapses by itself.** It counts only while the name resolves to the account:
  pointed elsewhere, expired or given away, the account has none.
- **Contracts resolve names at their block's time.** `names.resolve` in Lua, `resolve_name` in
  the SDK and the EVM's precompile at `0x…4e53` give what a name resolves to as the block runs.
  The Lua library and the WebAssembly imports exist in every build and on every chain, so a script
  behaves alike everywhere; on a chain without names they fail. A Lua contract that used a global
  of its own called `names` now finds the library there.
- **The EVM's precompile writes the alias down.** Resolving a name to an Ed25519 account through
  it registers the account's alias, so `payable(NAMES.resolve("alice.dev"))` reaches her at once.
- **`staking`, `governance` and `names` are features.** Each is on by default. `finality-ffg`
  brings `staking`, either governance kind brings `governance`. A build without `names` refuses a
  chain whose genesis has names.
- **Names made new dev chains.** The dev genesis has names under `dev` now, so dev chains from
  before have other ids.

## Mining pools

- **Format 9 is a new chain.** Headers commit to a `pool_root` and blocks carry an optional
  `PoolCommitment`, which changes every block's id, so a database in format 8 is refused and no
  migration reaches it: start a new chain, with a new genesis. The dev chains' ids moved with it.
- **Any number of miners, 44 bytes a block.** A pool's block carries the parts it sends to the
  escrow and a Merkle root over every miner's earnings, never a list of them. Miners claim with
  `Tx::Claim` and a path from `GET /pool/claim/{address}`, at most 64 hashes long.
- **Claims name a height, not a block id.** A block's id hangs on the state it leaves, and the
  root it commits is part of that state, so the pools column keeps each root by its block's
  height: a branch holds one block at each height.
- **Earnings are cumulative.** A leaf says all a miner earned from the pool; a claim pays that
  less what it claimed before. One claim covers any number of blocks, and claiming against an
  older root after a newer one pays nothing more.
- **Credited once final.** The pool credits a block's miners only once it is under the finalized
  checkpoint or deeper than `max_reorg_depth`, so a root never promises what a reorg took back. On
  a chain whose `max_reorg_depth` is large, that wait is long.
- **A pool's node leaves its own transactions out of its blocks.** The fees it sends to the
  escrow must still be in the producer's account as the block ends; one of its own that spent
  them would make the block invalid.
- **The escrow is a Contract address with no code.** No key and no contract can spend it; only
  `Tx::Claim` does. Anyone may send it more, which pays the pool's miners.
- **The `pool` feature is the node's side alone.** Without it a node takes workers as before and
  pays only its `--reward-to`; every build checks pool commitments and runs claims.
- **Open pools take a public token.** `--mining-open` uses `OPEN_TOKEN`, which every worker without
  `--token` proves, so its messages are still numbered and checked, but anyone may join. It opens
  mining alone too, and needs `--pool` or `--mining-solo`.
- **Mining alone costs a block a payout.** A node with `--mining-solo` builds a template for each
  payout mining alone every time it builds its own, at most 16 (`MOST_SOLO`); the 17th is turned
  away. Any seal ends every template of the round. A solo worker sends no shares, so its hashrate
  shows only in what it says, and it hears of nothing until it finds a block.
- **An open port goes over wss://.** Its token is public, so its MACs stop no one on the path, who can
  rewrite a payout both ways, unseen. `Worker::start` refuses plain TCP to one, and everything that
  mines comes through it, unless plain TCP cannot be read on the way (`plain_cannot_be_read`: this
  machine with no proxy, or this machine or an onion through a proxy on this machine; a proxy elsewhere
  reads what it is sent, and an onion with no proxy goes by the system's resolver) or `--insecure-open-port`
  says so. An open port is told by its token, not by what the node says, so writing the public token out
  as a file changes nothing. Private tokens stay on TCP: the token protects them.
- **A worker does TLS and the WebSocket on a thread of its own** (`mining::wss::pump`), as a TLS session
  cannot be read and written from two threads at once, unlike the TCP connection it cloned before. It
  waits 25 ms at most to read before what is waiting goes out, so a nonce waits no longer than that.
- **The certificate is always checked.** rustls on ring with the Mozilla roots built in, the same on a
  desktop, Android and iOS, and no flag skips it. `--ca-cert` gives what to trust in their place, its
  host's name checked all the same. rustls refuses a certificate with `CA:TRUE` as a server's own,
  which is what plain `openssl req -x509` makes; docker/nginx/tls.sh makes one without.
- **nginx drops what a location does not say again.** `proxy_set_header` in a location replaces all the
  server's, so `/mining/ws` setting `Upgrade` lost `X-Forwarded-For`, and the node counted every
  worker as nginx, 16 at most on the whole Internet. It says them all now, and allows 32 from an
  address, as the mining port does.
- **A worker behind a relay has no address of its own to the port,** which sees loopback. Its limits
  are nginx's and the relay's, per asker, and the report's address for it is the node's own.
- **The WebSocket only relays.** `GET /mining/ws` passes lines to the mining port on the node's own
  machine, so the port's limit of 256 connections counts them, and its report shows each as a
  connection from the node itself. Behind nginx, the location needs the WebSocket upgrade headers.
- **`GET /mining` is not in `GET /status`.** It asks the node's loop, as `GET /node` does, while
  `/status`, which the apps ask most, reads only the database.
- **A node's report hides by default.** `Sharing::default()` is the `public` preset, so a
  `GET /node` from elsewhere no longer lists peers' or workers' addresses, its own, or workers'
  names; `--report full` is what it was. Apps read empty values as they did, and `hidden` says why.
  The rule for who sees everything is stricter than the faucet's `asker`: only a loopback TCP peer
  counts, never a private one, so nothing on the LAN or in Docker's network gets in by writing
  `X-Forwarded-For`. Its token compares in time that does not say how much of it matched, and is
  read with `read_token`, as the mining token is.
- **A chain can have epochs nobody justified,** as this testnet does at 2 and 3, where the validators
  came up at different times. A client walking from genesis used to stop for good just before such a
  gap and say it had caught up, when its voters and rules ran out; see
  [Light client](light-client.md#what-to-keep-in-mind). Bringing voters up together avoids making one.
- **A dial can fail one way for good under containers.** See
  [Docker](docker.md): a stale conntrack entry, kept alive by the dialer's own retries, blocks one
  direction while the other works. Seen as `could not reach … Handshake with the remote timed out`
  every 15 seconds between nodes that are connected. `same_node` stops those retries once the peer
  connected first, by the IP it came from and its listening port.
- **`Duplicate` from gossipsub is no failure.** Two nodes passing on one transaction each publish
  a batch of it alone, the same bytes and so the same message id, and the later one is refused:
  every peer has it. `gossip_unpublished` leaves it out, and libp2p's warning for it is filtered.
- **A peer's height on the Network page** comes from its status, once a minute, and from each block
  it passes on, so it trails by a block or two at most while its gossip flows.
- **Restart a network's nodes together and a peer can come back the other way.** Each dials on
  starting, beside a connection the other has not timed out, which looks like two nodes dialing each
  other at once, so both stand and topics go untold. A peer connected for 30 seconds with no topics
  told is dropped and comes up afresh. Before that, the testnet's public node reached nobody for 20
  minutes after a redeploy (`gossip_unpublished` 575), forking the chain and stalling finality.
- **gossipsub tells its topics on a peer's first connection alone.** A peer back from a restart
  before its old connection timed out arrives as a second connection and would never hear them,
  publishing into nothing (libp2p-gossipsub 0.50's `on_connection_established` returns early for
  `other_established > 0`). The net thread keeps each peer's connections and their direction
  (`links`), and a second the same way as one standing drops the peer whole; it costs the peer one
  redial. `gossip_unpublished` counts what still reaches nobody, and statuses asked once a minute
  heal a fork while it is shallower than `max_reorg_depth`.
- **A GPU can be mined on through two contexts.** wgpu lists a card under each backend that reaches it.
  Vulkan, Metal and DX12 count as one, the same ids sorting side by side, but OpenGL is listed as well:
  on a phone, the Mali's two contexts together hash faster than either. By default both run, each
  taking its own share of the nonces, and any can be left unticked. A phone runs hotter for it. Two
  cards alike look identical to wgpu, so all a backend lists stay, and only a second native backend
  showing the same ones goes.
- **The wallet's GPU pick has a field of its own, `mine_gpu_list`.** Settings in app storage do not load
  at all if a field changes shape, losing every setting, so `mine_gpus`, the one from before, is still
  read until a tick replaces it.
- **The worker builds without the node.** The `worker` feature is the mining port's protocol, a
  worker's devices and its claims, for the wallet; `gpu-mining` adds GPUs, and the node's `gpu`
  feature brings it. `WorkerStatus` (`chainv4::mining::status`) is what a worker shows of itself,
  and the protocol's handshake and MACs (`hello`, `Joining`, `Sealer`, `Opener`) carry over any
  transport.
- **Miners keep their own claims; the pool's node is not needed to claim.** Only the pool holds
  the leaves under its roots, so the pool sends each worker its claim, kept in `--claims`, and the
  wallet keeps those it is given. `chainv4 claim --saved` and the wallet check them against any
  node, `GET /claims/{pool}/{address}?height=`. A worker run without a payout keeps nothing; one
  started before this kept nothing either, so its first claims arrive with its next standing.
- **The newest claim is the likeliest to be undone.** It names the pool's newest block, at the
  tip. Kept claims are the newest 32 not yet settled, counting the oldest of them, which settles
  next, and the newest two settled: one of those always holds. A pool that seals no block for a
  while leaves its settled height behind, which only keeps more claims around.
- **A kept claim pays up to its root.** What the pool credited after it lives only in the pool's
  books; claims carry the miner's whole earnings to then, so nothing older is ever needed.
- **The pool's books grow with its miners.** `pool.json` keeps each miner's earnings, and the node
  keeps the last four claim trees in memory for paths to the roots blocks committed already.

## Reading checked blocks

- **A block with no proofs is unknown, not unchanged.** The wallet does not show a value older than
  the newest such block until one direct proof catches it up. Nodes keep proofs for 10,000 blocks by
  default, so on anything but an `--archive` node old blocks come with receipts and no state.
- **A proven balance can still be stale.** Nothing commits to which entries a block changed, so a node
  can leave one out. Only a direct proof finds that, which is Check now, and the node sees the account.
- **It reads downward for old blocks.** A light client holds few headers below the finalized block it
  saved, and a phone has no room for one per block for months. So older blocks are proven by their hash
  links up to a block it did check, a stretch of up to 100 at a time, and nothing is kept until the
  stretch links up. So balances are right from the first proof, and history fills in going back.
- **Answers are checked before anything is taken.** All of an answer, or a whole stretch going down;
  an answer that fails leaves the index and its place as they were, and the reading stops until told.
- **A token seen while reading down takes a proof.** The blocks above it were passed over before it was
  being looked for, so what its older block says is no use for now.
- **`check_shape` will not let a cursor stand still.** A block whose list claims more, given with no
  entries and a cursor where it was asked from, is refused there. The scanner's own refusal stays.
- **The explorer's pages do not know whether a node or the index answered.** `Ask::route` sends what was
  read to `Reading::answer`, which writes the node's JSON, so pages keep one path, and a page's `Ask` that
  was forgotten is refused at `Api`, not in the page. What differs is the 451 of what is off and the 404
  words of what was not read.
- **The explorer keeps no state from blocks.** It has no accounts to follow, so a gap or a left-out entry
  cannot leave it stale, and it drops entries and their proofs: most of a block's 2 KB. A balance is a
  press, a proof at the final block, with its block shown.
- **What the explorer keeps is rechecked on opening.** It is kept in `GET /blocks/range`'s own encoding, minus
  entries, so it reads with `decode_block_range` and `check_range_item`. It is not sealed, so this is what
  stands against a damaged or changed file. The top must also match the light client's header, if held.
- **Its head is written last.** A kill between leaves a head saying less than the buckets hold, so blocks above
  it are ignored and read again. One saying more finds a block missing, sets that bucket aside, and knows from
  above it.
- **The index goes a bucket at a time.** Going up past the cap, the oldest 1,024 heights go, so the file goes
  whole and nothing is rewritten. Going down past it, the same drops what was just read once, stops, and from then
  on `further_back` says Full: anything older would only go the same way.
- **The explorer walks every header on each launch with it on.** It saves no client, and a node's checkpoint is
  the node's word. Pasting a checkpoint is the way round. Saving the client as the wallet does would help.
- **The Api's refusals go through `Asks` too.** A page polling something off is told once in 3 seconds, as the
  wallet's is, and what the index answers waits in `for_index` until it is open.
- **`Ask::needs_asking` has no catch-all.** A new request does not build until someone decides whether
  it may leave with this on. The refusal is in `Node`, not the pages.
- **The index's key is not the vault's.** It is derived from it, so sealing either never reuses a nonce
  across both, and a password changed seals the index anew at its next save.
- **What the Activity page shows from blocks is not in the settings file.** `Checked::came` says how each
  came to be known, and `kept_only` writes only the old kind. Pages read one map; only the file differs.

## Fast resume

- **In the explorer, it is each launch.** The explorer saves no client, so with the switch on it starts
  at the node's checkpoint every time it opens, rather than walk from genesis. There is nothing to be
  a few epochs behind of, so the wallet's 8 epochs do not come into it. The decision is
  `chainv4::light::may_resume`, and the words `ui`'s, in both.
- **It is a trade, and off.** The wallet's switch, per node, swaps walking every header since it last
  ran for one request: the node's finalized checkpoint. Headers and voters' signatures are checked
  from there, against the genesis pinned, but nothing ties that checkpoint to what the wallet checked
  before, so a dishonest node could show a false history from it. The sidebar says when it happened.
- **Only under voting finality, and only far behind.** More than 8 epochs behind. Under depth finality
  nothing signed says what is final, and there are no voters to check from there on, so it walks.
- **Below it, older receipts still prove,** down the hash chain, a stretch at a time, as for a client
  that started at a pasted checkpoint.
- **The saved state moves on.** Once it resumed at a checkpoint, what it saves is from there. Start over
  walks from genesis, and with the switch on takes the checkpoint again.

## Filtering an address's transactions

- **Filters are read-time.** The address index stays as it was: nothing new is stored, and the existing
  chain works as it is. Height is the index's own order, so a range seeks and stops, and times come to
  heights by halving with some slack, as a chain's timestamps need only pass a median. Everything else
  reads the block and receipt of each entry, which is why a request looks at no more than 1,000.
- **A rare match takes more requests,** not a slower one: an answer may hold nothing, with `limited`
  true and a cursor to go on from.
- **Tell an older node by `applied`.** It ignores the parameters and answers with everything, without
  that field. Both apps then test what they have here, and say that only what was read was searched.
- **The filter is the node's word.** In the wallet each line is proven before it says so, but a node
  could leave a match out, as it could from any listing.
- **`token` looks at calls and events,** not balances: the contract called, or one that emitted anything
  while the transaction ran. `counterparty` is what the index calls touching: a call's arguments count,
  a token's `Transfer` to someone a contract chose does not.

## Retrying, reloading and time limits

- **Every request ends.** 30 seconds in all and 12 to connect, or 90 and 60 through a proxy; a browser's
  fetch has none of its own, so the apps race it with a timer. A stream may last, but its answer must
  start within a minute. So nothing stays in the apps' list of what is out.
- **Failures stand until answered.** They used to clear after three seconds so a page would ask again,
  which showed a spinner most of the time and the reason for a moment. Now a page goes on asking, held
  back 3, 6, 12 … up to 30 seconds by `ui::asks`, and the reason stays, with Retry past the hold.
- **A 404 is an answer.** It holds 3 seconds and no longer, as a transaction not on the chain yet will
  be, and it is not called trouble.
- **Reload re-asks what the page asked,** noted per page as it went out, whether or not the page would
  ask again, so a block cached for good is asked again too. It clears nothing first: what shows stays.
  Nothing with a body is ever sent again by Retry or Reload.
- **The pull is only a pull from the very top.** A finger that lands while the page is scrolled scrolls,
  even if it reaches the top on the way. Drags something on the page took, such as a slider, are out.

- **Low data is one switch for every timer.** Settings → Network use → Low data has the wallet and
  explorer ask their node four times less often by themselves (`ui/src/pace.rs`) and wake by
  themselves less often. What you ask for, retries, reloads and what `/stream` pushes are never
  held back, so a node with `--stream` still shows blocks as they come.
- **Wait for answers** multiplies every request's limits (30 s, or 90 s through a proxy; 12 s or 60 s
  to connect) by 2, 4 or 10 for slow Tor or I2P. The browser has no connect limit and uses the
  whole one. Changing it reconnects, as a new proxy does.
- **Reading caps are settings now:** the explorer keeps 16 MB to 2 GB of blocks (128 MB desktop and
  32 MB phone or browser as usual), the wallet 5,000 to 500,000 transactions (20,000 as usual).
  Lowering below what is held asks first, then lets the oldest go at once; reading them again costs
  their download.
- **The wallet's Activity pages now.** Address-index and event-index lists come 50 at a time;
  "Older transactions" follows each one's `cursor`. With Read from checked blocks on there is no
  such button: Settings → Read further back is what reaches older history.
- **The wallet's DEX trade deadline is a setting,** 20 minutes unless changed, 1 minute to a day,
  beside slippage.

## App store builds

- **`--store` builds `--no-default-features --features store`.** Add a default feature to `store` too,
  or the store's build goes without it.
- **Android's store build has no `wgpu`;** iOS's does, as eframe draws through it there, but no
  `chainv4-gpu`. `cargo tree` says which: see [Building and testing](wallet.md#building-and-testing).
- **The browser mines through its own WebGPU,** so `chainv4`'s `gpu-mining` builds nothing in wasm.
- **Settings don't change with features.** The mining settings stay in a store build's settings, unread,
  so a person moving between builds keeps them.

## The chain's name

- **The chain's name is not the software's.** Genesis's `name`, or governance's `Change::Name`,
  is what the apps call the network; the binary, its commands (`chainv4 worker`, `chainv4
  update`), the releases and the EIP-712 domain stay chainv4, as changing that domain would change
  what every wallet signs. A chain with no name shows as chainv4.

## Updates

- **No release keys, no updates.** `release/keys.json` names none, so a build from the repo takes
  no releases: nodes fetch nothing, the apps show nothing, `chainv4 update` refuses. A release
  build names the keys in that file or in the one `CHAINV4_RELEASE_KEYS` points to.
- **The signatures are what count, never the host.** A threshold of the keys built in signs each
  manifest, covering every binary's URL, size and SHA-256, so a node relaying it at
  `GET /release`, a mirror or a CDN can withhold a release but never change one. Workers, the apps
  and `chainv4 update` check it again themselves.
- **A manifest lapses.** `expires` is signed, 30 days after `issued` unless `--valid-days` says, 366
  at most. Run `chainv4 release renew` and sign again before it passes, or nothing acts on it: no
  node installs from it and no app shows it, though a node still stops at an upgrade it already took.
- **`floor` in `release/keys.json`** is the oldest `issued` a build takes. Raise it at each release
  build, to that release's `issued`: a fresh install then goes no further back than that.
- **Everything keeps the newest manifest it saw:** the node in `release.json`, the CLI and a worker
  in `~/.chainv4/release.json`, the apps in their settings. Lose it and the floor is all there is
  to go by, until the next one comes.
- **An older manifest never replaces a newer.** Releases order by `issued`, and a node keeps the
  newest in `release.json` beside its database: `chainv4 release new` issues each one after the
  last.
- **An upgrade stops nodes, not blocks.** `Change::Upgrade`, or a release's `breaking`, makes a
  node older than its version stop before its height; nothing about the blocks is checked by it,
  so a node new enough never notices. Decide it after the release with the new binary is out.
- **`--auto-update` restarts the node in place.** On Unix it starts the new binary in the same
  process with the same flags, so a supervisor sees no exit. A container's binary is the image's:
  leave it off there and update the image.
- **A replaced binary runs only once started again.** `chainv4 update` and the apps' Update button
  rename the new binary over the old one; whatever runs keeps the old one in memory until it
  restarts.

## Kyoto's gossip monitor

The wallet hears pending Bitcoin payments through Kyoto's `subscribe_to_gossip`, which opens a
connection of its own to the next peer. Kyoto pops each peer named with `add_peer` once, so on a
private regtest chain of one peer, the light client names it twice, or the monitor never
connects. Later subscriptions only add scripts and outpoints, and there is no taking one off:
`light::run` keeps what it asked for and asks again for all of it when the connection closes.
Gossip is filtered again on the wallet's side (`light::hear`), and only a block settles it.

## Digit separators

The CLI reads `1_000_000` wherever it takes a number from text: amounts, coins such as
`1_000.5`, spans such as `1_000s`, and typed arguments such as `u256:1_000_000` (strings and bytes
keep their `_`). A `_` must stand between two digits, hex digits after `0x`, so `_1`, `1_`, `1__0`
and `0x_ff` are refused: stricter than Rust's and Luau's literals, which take some of those, so a
slip is not quietly read as another number. One rule for all of it, `structs::digits::ungrouped`.
The apps' amount fields drop `_` and `,` as they always did. Contracts do not take them yet:
Lua's `u256("1_000")` and `tonumber("1_000")`, and the SDK's `from_decimal`, change what a
contract computes, so they wait for the next regenesis (see the todo).

## Building and testing

- **The end-to-end SDK test needs the wasm32 target.** It builds `contracts/` with cargo:
  `rustup target add wasm32-unknown-unknown`. CI installs it.
- **Contracts build from their own workspace.** `contracts/.cargo/config.toml` sets the wasm32
  target, but only when cargo runs inside `contracts/`. From the repo root, pass
  `--target wasm32-unknown-unknown`. The SDK's own tests run on the host target.
- **Clippy runs with `-D warnings`, on every feature subset CI tests.** A build that lacks a
  feature has to compile cleanly too: cfg-gate what only one kind uses. See
  [Development](development.md).
