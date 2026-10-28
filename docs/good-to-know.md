# Good to know

The things that surprise people, and the rules that keep every node on a chain in step. The
other pages explain each part in full; this one is the short list to keep in mind.

## Running a chain

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
- **Rust exports take typed parameters.** `#[export] fn transfer(to: Address, amount: U256) ->
  bool` reads the call's arguments into its parameters and refuses a call whose arguments do not
  fit, naming the parameter. A function that takes and returns nothing still reads `input()`
  itself.
- **Events are searchable on nodes that index them.** `--index-events` indexes every event by its
  contract, its name and the addresses in it, for `GET /events`.
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
- **Floods are turned away at the door.** Gossiped transactions and votes, and submissions, count
  against a backlog: 10,000 in all and 1,000 from any one peer by default
  (`--max-gossip-backlog`, `--max-gossip-per-peer`). Past it they are dropped, `POST /txs`
  answers 503, and a peer over its share 10 times in a minute is disconnected. It can come back
  at once: there are no bans yet (see [Node](node.md#the-gossip-backlog)).
- **A node only streams when asked.** `--stream` turns on `GET /stream`. Without it, clients
  such as the explorer poll.
- **Read-only calls hold up the API.** `POST /call` runs a contract under the chain's fuel cap on
  the API's one thread, and every other request waits for it.
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
- **Picking files on Android takes Java.** Android hands a picked file only to Java code, so the
  build compiles a small activity, `ui/android/Picker.java`, into the APK with a JDK and Android's
  build-tools, and signs the APK again with the same key. Without them it builds an app that cannot
  pick files, and leaves its buttons out; pasting needs no Java and always works.
- **The wallet and the explorer share the `ui` crate, not a look.** `ui/` holds the palette,
  density and type machinery, the widgets and the Android glue; each app picks its own colors,
  fonts and density. A change to a shared widget shows in both. See
  [The ui crate](development.md#the-ui-crate).
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
  the browser wallet. Plain `verify` refuses an EIP-712 signature: it knows no chain. See
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
- **The Luau fork.** The germ3n forks of mlua and Luau meter fuel. They are pinned by branch, with
  the commit in `Cargo.lock`; `cargo update` would move them, so pinning a `rev` would be safer.
  Never swap them for upstream. Its comparisons are consensus too: a number against a userdata,
  and two userdata of different types, ask the userdata's metamethods, where plain Luau would
  fail or call them unequal (see below).
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

## How the chain behaves

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
  with governance, `Change::Bootnodes` replaces them on the live chain, at once, on the same ballot
  and version as consensus changes, so deciding one moves the version for the other too; without
  governance, `--bootnode` and `BOOTNODES` add a node's own, which change with no vote.
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
- **Wallet biometrics keep the vault's key, not the password.** Each platform wraps the key the
  password stretched to: the Android Keystore, a passkey's PRF output, or the macOS Keychain after
  Touch ID. Changing the password turns them off, and so does a change in the fingers or faces
  enrolled. On macOS Touch ID is a check, not a lock on the key, as an unsigned app cannot tie a
  Keychain item to biometry.
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
  screen; `WalletEntry::unprotected` marks it. Settings adds or removes one.
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
  `proposals:` and `multisig-evm:` prefixes, which no account key starts with; nothing scans the
  column.
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
  and Face ID shares the macOS module, `mac`, the Keychain and LocalAuthentication being both
  systems'.
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
  down, `-4`. Plain numbers keep Lua's arithmetic.
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

- **Its limits forget on restart.** Who had a drip when lives in the node's memory alone.
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
  **Your chain** in its Settings, and any other is named and warned of on the review.
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

## Bridges

- **The gateway's setup takes the chain id:** `setup(chain_id, …)`, the id the chain's EVM answers
  CHAINID with, as `GET /status` says it. A wrong one makes every digest a stranger's.
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
- **A deposit address is per address and per federation:** a new federation makes new ones; pay
  an old one only within the grace period, after which its deposits do not mint.
- **Withdrawals start at 1,000 satoshis,** and a release pays at most 50.
- **Releases are given without their witnesses:** the gateway reads and checks the base
  transaction, as its txid hashes it; the witnesses are assembled after, from the board.

## The directory

- **Names no longer find contracts:** `swap`, `dex`, `nfts` and `gateway` under the suffix were
  anyone's to register first, and a wallet would have paid whoever did. The chain's directory
  names them now, decided by governance; a name registered for one finds nothing.
- **Genesis writes its directory and federation into the state,** so their state roots, and so
  their genesis ids, differ from a chain with none: one entry each, proven like a balance.
- **The node's own word comes last:** `--swap-contract`, `--dex-contract` and `--dex-router`
  still answer when the directory names none.
- **A signer checks the directory first:** `chainv4 bridge sign` and `relay` refuse a gateway
  peer on an EVM chain other than the one the directory names there.

## Bridges in the apps

- **Deposit addresses are computed, not asked:** the wallet builds the account's deposit address
  from the proven federation, so a node cannot hand it one of its own; the gateway's
  `deposit_script` says the same, for the CLI.
- **The light client keeps a deposit's proof as it finds it:** its block, the transaction
  without its witness, its place and Merkle path, for Mint wBTC. Coins found before an update
  have none: show them through Esplora.
- **The wallet lists your transfers out and withdrawals among the latest 20** each, by who sent
  them, which `gateway.lua` and `btc_gateway.lua` record now (`sent`'s 13th value,
  `withdrawal`'s 4th).
- **From an EVM chain, a token's amount is in its smallest units,** as the wallet does not ask
  the token there its decimals.

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

## Building and testing

- **The end-to-end SDK test needs the wasm32 target.** It builds `contracts/` with cargo:
  `rustup target add wasm32-unknown-unknown`. CI installs it.
- **Contracts build from their own workspace.** `contracts/.cargo/config.toml` sets the wasm32
  target, but only when cargo runs inside `contracts/`. From the repo root, pass
  `--target wasm32-unknown-unknown`. The SDK's own tests run on the host target.
- **Clippy runs with `-D warnings`, on every feature subset CI tests.** A build that lacks a
  feature has to compile cleanly too: cfg-gate what only one kind uses. See
  [Development](development.md).
