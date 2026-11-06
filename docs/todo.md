# Notes and todo

What chainv4 still lacks, and what got done. The things to keep in mind while running or changing
it are in [Good to know](good-to-know.md), and how each part works is in the
[docs](README.md).

## Todo

### Keys to rotate after the malware

The development Mac ran a stealer for about an hour on 2026-11-03 (removed the same day). No misuse
was seen, but whatever it could read is taken as copied. Postponed until the owner is at the machine
(2026-11-04); the sixth genesis went out on the same keys with the pentest's fixes. The rotation's genesis
will carry whatever is new by then. First planned: the new genesis carries them, as the Luau fork's fuel charges, powers and the Wasm call's load
fee change how blocks replay, and the fixed DEX is deployed on it. Push the fork's commit first
(`germ3n/luau-src-rs` 325972b) and point Cargo.lock at it, so the servers build the same engine.

- [ ] **Rotate the devnet's SSH key.** A new key on all three servers, checked to log in, then the
  old one removed from each, so access is never lost.
- [ ] **A new genesis with fresh chain keys.** New owner, EVM, relay, faucet, validator, voter and
  network keys, and new validator words; Gateway.sol on Sepolia deployed again under the new EVM
  key, with the Sepolia ETH moved over.

### Once every queued batch is done

- [ ] **A review pass, feature by feature.** Once every queued batch is in, before the fresh test
  chain, go through each feature and category in turn (consensus, finality, governance, storage,
  the node and network, the API, each engine, the registries, swaps, the DEX, NFTs, bridges, the
  wallet, the explorer, the CLI) and in each find:
  - bugs, and code or comments that make no sense or say what the code no longer does;
  - stale docs: guides, good-to-know, the todo and `#[document]` text that drifted from the code;
  - tests that test nothing, or the wrong thing, and places that want more;
  - then measure coverage, run the analysis tools (clippy pedantic, cargo-audit, fuzzing where the
    todo asks for it), and fix what turns up, a commit per feature.
- [x] **Pin the Rust and clang toolchains.** Done 2026-11-05: Rust 1.98.1 in `rust-toolchain.toml`,
  clang 19.1.7, LLVM's own release by its SHA-256 (`docker/llvm.sh`), in the image and the CI's
  node jobs as `CC`/`CXX`, checked alike by `tests/toolchains.rs`
  (see [Development](development.md#toolchains)). Was: Nothing pins either today: no `rust-toolchain.toml`,
  and the Luau fork's C++ (built by `luau0-src`) and RocksDB are compiled by whatever `cc`/clang the
  machine has. Fuel and memory metering go through that C++, so two nodes built with different
  compilers could, in principle, meter alike or not. That is the same worry as the cross-OS
  consistency check. Pin Rust with a `rust-toolchain.toml` at the root and in `wallet/`, `explorer/`
  and `contracts/`, with the components and targets the builds use (wasm32, Android, iOS). Pin clang
  for the C/C++ deps in the Docker image and the CI, and say which in `docs/development.md`. A
  release then names the toolchains it was built with.
- [ ] **An optimization pass (owner, 2026-11-06), after the consensus batches, before SurrealKV.**
  Go through the node, the engines, the database layer, the light client and the apps, hottest
  paths first, measured rather than guessed (criterion benches and flamegraphs of block import,
  sync, the mempool, state proofs and the apps' frames):
  - **containers:** the best fit at each place: `Vec` or `SmallVec` over maps for small sets,
    `HashMap` with a fast hasher (`ahash`/`foldhash`) where keys are not attacker-chosen and the
    default SipHash where they are, `BTreeMap` only where order is needed, `IndexMap` where
    insertion order matters, arenas and slabs over many small allocations;
  - **locks:** `parking_lot` over std where it measures faster, `RwLock` where reads dominate,
    narrower critical sections, no lock held across I/O or `.await`, fewer `Arc<Mutex<…>>`
    passed around;
  - **lock-free where it pays:** atomics for counters and flags, `ArcSwap` for read-mostly shared
    state (the tip, settings, rules in force), `crossbeam` channels and queues over mutex-guarded
    `Vec`s, sharded maps (`DashMap`, or per-shard locks) for the mempool and peer tables, epoch
    reclamation only where a benchmark shows contention;
  - **the fastest way to do things:** fewer clones and allocations on hot paths (`Bytes`, `Cow`,
    borrowing), batch database writes and reads, parallel verification with `rayon` where work
    splits, caches with bounds where the same thing is computed again, SIMD-friendly hashing, and
    `release` profile settings (LTO, codegen units, `panic`) measured.
  Each change kept only with a benchmark that shows it, consensus results unchanged (the
  determinism tests and a full resync agree), a commit per area.
- [ ] **Go through every test there is (owner, 2026-11-06), last of all.** Each test in the node, the
  CLI, the contracts, the wallet, the explorer and the ui crate, read for what it still checks:
  tests made stale by what changed since (to adjust or delete), ones that pass without checking what
  their names say, duplicates, and gaps, features and failure paths with no test at all. A list of
  what changed and why, with the gaps filled.
- [ ] **Run `cargo fmt` over everything.** The tree is not rustfmt-clean today (`cargo fmt --check`
  shows diffs in `build.rs`, `gpu/` and more), so batches have kept to the style around them by
  hand. Run it once across the node, `wallet/`, `explorer/`, `ui/`, `gpu/`, `macro/`, `wiki/` and
  `contracts/`, in a commit of its own with nothing else in it, so blame stays readable. Then
  add `cargo fmt --check` to CI.
- [x] **Review fixes: consensus and what strangers can send.** Done 2026-11-01, from the review in
  the queue. In the node and its API:
  - the ABI type parser caps a fixed array's length (`u32::MAX`), so measuring a static type's size
    cannot overflow a pointer, which wrapped on a release node and panicked on an overflow-checked
    one, and every node reads the same types whatever its width;
  - `POST /rpc` caps a batch at 100 requests, and `eth_getTransactionByHash`/`eth_getTransactionCount`
    ask the mempool a narrow question rather than cloning all of it on the node loop;
  - the mining port caps connections per address (loopback, the relay's, exempt), and the HTTP
    `/mining/ws` relay is capped per asker;
  - a transaction's nonce and balance are checked against the tip before its signature is verified,
    and a multisig verifies each distinct member signature once (`Mempool::affordable`/`backed`,
    `Gate::affordable`, dedup in `Multisigs::backing`);
  - gossiped blocks count against the per-peer backlog, released once judged;
  - `--trusted-proxy` names the reverse proxies whose `X-Forwarded-For` the per-IP limits believe;
    loopback alone otherwise, so a neighbour on a shared private network cannot spoof a client.
- [x] **Review fixes: the wallet, its signer and the extension.** Done 2026-11-01, from the same
  review. In the wallet:
  - every signing screen lays a single transaction out in the same step frame a batch's step uses,
    so a lone contract or EVM call shows its value, recipient and calldata rather than a one-line
    summary, on the Send review, the offline signer, and a multisig bundle and proposal; the offline
    multisig review now shows the fee a member signs for and the bundle's chain
    (`airgap::laid`, `steps::single`, `views::steps::of_one`);
  - changing the password writes the new seal at once and retires the old biometric key instead of
    forgetting it before the seal is on disk (`views/settings::change_password`);
  - the Argon2 password stretch is pinned, not left to the crate default, so a future bump cannot
    lock sealed wallets out (`vault::argon2`);
  - a wallet whose files cannot be read now (a lock or a permission, not corruption) is left in
    place, not set aside; a removed wallet's orphan files are cleared even when this run never wrote
    them; a new wallet that fails to seal is not opened; a browser save that does not take is
    reported (`store::read`, `store::job`, `lock::keep`, `app::save`).
  - The browser extension refuses a provider to an opaque origin, caps a site's waiting requests and
    will not reopen the approval window the instant it is dismissed, drops a page's requests when it
    goes, and will not relay a read to a loopback or private node for a site that has not connected.
- [x] **A fresh test chain on the VPS.** Done 2026-10-28, genesis `0x9a7fbd89…`, as
  [Deploying](deploy.md) records; external EVM gateways wait for a funded key there, and the
  review pass runs on it. Once all queued changes and batches are in, delete the
  VPS's chain and start a new one from a new genesis with everything enabled, so all of it can be
  tried end to end:
  - every engine, finality with governance, names, the collection registry, fees;
  - the DEXes deployed, native and EVM, with pools opened;
  - the swap contract and Swap.sol;
  - the NFT factory with sample collections of both kinds, minted;
  - the bridges' gateways, the relay started on Bitcoin's test network, and a federation;
  - each of these named in the genesis's directory (or by governance after);
  - sample tokens, the faucet, and the example contracts deployed and named;
  - the laptop rejoining with `--cpu`, the apps pointed at it, and new wallet and explorer
    builds.

- [ ] **A real testnet.** The VPS and this laptop are two nodes run by one person. Before
  calling anything ready, run it as a network: 4 to 6 nodes on separate machines, networks and
  operators, a couple of independent miners and validators, for weeks, with restarts, partitions,
  clock skew and nodes joining late from snapshots. Exercise it end to end there:
  - governance decisions, finality switches and an upgrade at a height, with nodes on both sides;
  - the apps against nodes they don't run, over Tor and I2P too, and light clients proving all of
    it;
  - swaps, the bridges and the relay with people who are not the developers;
  - pools with outside miners, and the faucet under real use.
  Write down what broke, and turn each into a scenario test.

- [ ] **A robust upgrade path, proven on the real testnet.** Releases, `--auto-update` and
  `Change::Upgrade` work in tests and on two nodes, with no release keys made yet. Before others
  run nodes:
  - **Release keys for real:** the M-of-N keys made and kept offline, `release/keys.json`
    filled, a signing ceremony written down, and how keys rotate (`updates`) rehearsed;
  - **Installing that can't brick a node:** a download that stops, a full disk, a binary that
    won't start, a wrong target or a downgrade each leave the old one running. Keep the previous
    binary and roll back to it when the new one fails its first start; the same on Windows,
    whose running binary moves to `.old`;
  - **The upgrade height:** nodes stop cleanly before `from`, restart on the new release and carry
    on from the same database, with no fork, whatever order they update in; a database the new
    release changes migrates on first start, and an older one refuses it rather than misreads;
  - **Staged rollouts:** a release reaching some nodes first, the manifest's `least` raised later,
    and the apps' own update path (wallet and explorer, desktop, Android, iOS) checked the same;
  - **On the real testnet:** a non-breaking release and a breaking one, each with nodes run by
    others, some updating by hand, some with `--auto-update`, some late, with the network watched
    across the height. Write down what broke, and make each a test.

### Multi-chain treasury

One quorum controlling assets on every chain it reaches: native multisigs already hold the
matching Bitcoin P2WSH and an EVM contract at one address everywhere, swap as one party, and sign
offline; these would make that hold up as a treasury.

- [ ] **Owners follow the members by themselves.** A change of members here is one transaction;
  each EVM chain's contract and each Bitcoin script then needs its own, signed separately
  (`multisig evm … sync`). Propose every chain's change with the native one, as one bundle the
  members sign once, and show which chains are still behind.
- [ ] **More kinds of keys and scripts elsewhere.** EVM owners are secp256k1 alone (see the item
  on other key kinds below); Bitcoin is P2WSH `sortedmulti` alone. Taproot with MuSig2 for a
  smaller, private spend, and miniscript policies for timelocked recovery and inheritance paths,
  such as a backup key that may spend after a year without movement.
- [ ] **Policies beyond M of N.** Spending limits by amount and period, a delay before large
  transfers run that members may cancel, and roles (who proposes, who approves, who may only
  watch), expressed once and enforced on each chain: natively here, in MultiSig.sol on EVM
  chains, and as far as script allows on Bitcoin.
- [ ] **One view of what it holds.** A multisig's page, in the wallet and the explorer, with its
  balances on every chain together: here proven by the light client, Bitcoin by the wallet's own
  Bitcoin light client, EVM chains through their RPCs; and its policy itself proven, not the
  node's word.
- [ ] **A chain worth trusting with it.** Many validators rather than one, an audit of the
  multisig, swap and bridge contracts and of the light client, and a public test network run for
  long enough to show it.

### Consensus

- [ ] **A fork schedule (owner, 2026-11-06).** Today a change to what blocks compute, such as the
  Lua whole-number rule, a fuel table or what a signature covers, takes a new genesis, as nothing
  tells a node which rules a past block ran under. Instead, a schedule of named forks, each with the
  height or epoch it takes effect from: in genesis, and added to by governance, as
  `Change::Upgrade` already stops nodes older than a version before a height. Blocks below a fork
  run the rules before it and blocks from it the rules after, so a node replays history under each
  block's own rules, and a live chain takes consensus changes without a regenesis. What it takes:
  - every consensus rule looked up by height where it applies: the engines and their versions (two
    Luau builds side by side, or the fork's rule behind a flag the VM is given per block), fuel
    tables, the transaction encoding and what is signed, the fee schedule, the limits;
  - each fork shipped in a release that knows it, with `Change::Upgrade` naming that release as the
    least version from the fork's height, so nodes that do not know it stop rather than split;
  - tests replaying a chain across a fork, and nodes on both sides of it;
  - `GET /status` and the explorer listing the forks passed and those scheduled, and docs listing
    each fork and what it changed.

  The Ethereum anchoring fork table is a small precedent. Batches 11 to 15, the consensus ones,
  could ship as the first scheduled fork rather than another regenesis.
- [x] **Mining pools.** `--pool` on the mining port: shares at a difficulty set for each worker's
  hashrate, PPLNS over the latest blocks' worth, each block sending its miners' share to the
  pool's escrow and committing to a Merkle root over every miner's earnings, which each claims
  with `Tx::Claim`: any number of miners in 44 bytes a block, credited once final, an optional
  fee, open or token pools, `--pool-push` claiming for them, `GET /pool`, the worker's log, the
  explorer's Pool page and the wallet's Mining page. See [Mining pools](consensus.md#mining-pools).
- [x] **Miners keep their own claims.** The pool sends each worker its claim, kept in `--claims`
  as `SavedClaims` keeps them, the wallet keeps those it is given, and `chainv4 claim --saved` and
  the wallet claim by them at any node, checked with `GET /claims/{pool}/{address}`: a pool whose
  node is gone still pays.
- [x] **Mining from the wallet, alone or in a pool.** `--mining-solo` has the mining port hand
  each worker mining alone a block of its own, paying its payout in full; `GET /mining` tells the
  apps the port and what it takes, `GET /mining/ws` carries it over a WebSocket; the wallet's
  Mining page mines on the GPU or CPU, natively over TCP and in the browser on WebGPU or Web
  Workers, phones only in the foreground and on the charger. See [Mining alone](consensus.md#mining-alone).
- [ ] **Mining pools: next.** Done 2026-11-06: the wallet's Mining page and the explorer's Pool
  page prove a claim's root and what was claimed through the light client (`PoolClaim::pays`),
  leaving only which claim is newest to the pool's word. A pool's node keeps four claim trees in memory, which a
  pool of millions of miners would want on disk. A share chain among the pool's own nodes, as
  P2Pool keeps, would take the split out of one node's hands altogether.
- [x] **A decoded transaction as the tx page shows one.** The node's JSON for a transaction is
  `structs::json`, apart from the node, so the explorer's Decode tool writes a decoded one as the
  node would and shows it with the tx page's own view.
- [x] **Fast resume on trusted nodes.** A per-node switch in the wallet and the explorer takes up the
  chain at the node's finalized checkpoint rather than walking the headers, off by default, saying what
  it gives up.
- [x] **The explorer in the wallet's look.** One palette, the same fonts, cards, headings, buttons,
  pills, sidebar, bars and tabs from the `ui` crate; the explorer keeps its own pages, tables and charts.
- [x] **Mining over Tor.** `--mining-onion`, or `--mining-onion-port` with `--onion-file`, names the
  onion's mining port in `GET /mining`, which a wallet on the onion dials rather than the public address.
- [x] **The wallet in the app stores.** A `mining` feature, on by default, and `--store` on the build
  scripts: no miner, no `wgpu` on Android, and the page called Pool earnings.
- [x] **Better transaction filters.** `GET /accounts/{address}/txs` takes kind, direction, counterparty,
  token and when, read through the index with a bound per request, and both apps offer them.
- [x] **Retry and pull to refresh.** Every request has a time limit, what failed says so with Retry, a page
  pulled from its top or Reload asks again, in both apps.
- [ ] **Polish the explorer and the wallet (owner, 2026-11-06: "a polish pass going thru every page").**
  Every page of both apps, run for real, on the desktop and in the Android emulator: what each page
  says and in what order, empty and loading states, what is hard to find or to read, controls that
  are easy to miss (the Bridge page's From box was one), spacing and fit on a phone, and the tools
  the explorer still lacks. Screenshots of each page before and after, and a list of what changed.
- [x] **The directory: governance, or an admin key.** A chain with governance sets it by
  `Change::Directory`; one without may name a `directory_admin` in genesis, which sets it with
  `Tx::SetDirectory` and hands the role on or renounces it with `Tx::HandDirectory`. Genesis
  keeps `directory` as where the chain starts, for contracts known ahead.
- [x] **keygen prints the public key.** `keygen` prints the scheme, the public key (compressed
  for secp256k1) and the address, with the EVM or testnet one, and `chainv4 key show` prints the
  same for key files already made, never the secret.
- [x] **relay-sync copes with a throttling Esplora.** It asks at an even pace, again with
  backoff on errors and web pages, 25 headers a transaction, keeping what a pass fetched.
- [ ] **Revisit the deployment proxy at genesis.** A chain running the EVM starts with the
  deterministic deployment proxy at `0x4e59…956c` (`Genesis::write_state`), since the chain
  refuses the proxy's own pre-EIP-155 deploy, so a multisig's MultiSig.sol and Swap.sol land at
  their everywhere-addresses here too. It is ownerless, holds nothing and is 69 bytes, as OP
  Stack's preinstalls are; but it is always on, so a chain made before it no longer opens with
  its old genesis (`WrongGenesis`). Maybe make it a genesis setting (`preinstalls`), off for old
  genesis files, or drop it for something else.
- [x] **Proof of work.** Genesis names the scheme. Under proof of work anyone mines: a nonce
  must make the work hash of the block id meet the block's difficulty, which retargets
  every block toward a target block time. Miners run on threads of their own (`--mine`,
  `--mine-threads`).
- [x] **Fork choice and reorgs.** Side blocks are stored, and the branch with the most total work
  is the main chain. A heavier branch triggers a reorg: roll back to the fork with undo records,
  run the branch, one commit. A branch that fails to run is dropped, and its descendants refused.
- [x] **A rotating authority.** `consensus.type: rotating` has the authority finality signers
  produce in turn, one per slot, the next in line stepping in a slot later, in-turn blocks
  weighing 2 and others 1; the producers are the signers fixed for each epoch, so they change by
  approval and leave with an ejection. Light clients learn them from finality's updates and so
  follow `activation_delay` epochs behind it, as does snapshot sync, which checks headers as a
  light client does.
- [x] **Production by stake.** `consensus.type: staked` has FFG's bonded validators, as fixed for
  each epoch, produce in slots with a share each in proportion to its bond, under a `schedule`
  genesis picks: `draw`, a weighted draw by the chain's hash over the set, height and slot, or
  `priority`, Tendermint's proposer rotation replayed from the epoch's start. Blocks declare the
  slots waited as difficulty, weigh 2 in turn and 1 otherwise, and are sealed by the voting key
  for the bond's account. Consensus takes its producers as weighted members now, so light
  clients and snapshot sync cover it as they cover a rotating authority.
- [ ] **No Tendermint-style instant finality.** It replaces both block production and finality:
  rounds of prevotes and precommits, locking, every block final once two thirds precommit, and
  no fork choice. A consensus and finality kind of its own, for a batch of its own.
- [x] **Snapshot sync on a rotating or staked chain.** Snapshot sync checks headers through a
  `LightClient` of the node's own chain, fed over the peer protocol, which learns each epoch's
  producers from the finality peers serve (`SyncRequest::Finality`) and checks on once it knows
  them; the pivot is taken among the headers it could check, and the checkpoint the node holds
  final is the trusted one or a later one the client saw finalized.
- [x] **Finality.** Casper FFG-style: validators bond on chain (`Tx::Bond`, `Unbond`,
  `Withdraw`) with a separate voting key, and vote at each epoch's checkpoint. Two thirds by
  bond justify a checkpoint, and a justified direct child finalizes its parent. Votes are
  gossiped, justifications stored and fetched by sync, and blocks carry them as proofs that pay
  each voter `vote_reward`. Conflicting votes are evidence; `Tx::Slash` takes the bond and pays
  the reporter a share. Fork choice never goes below the finalized checkpoint, follows work only
  from the justified one on, and follows a newer justified branch over more work.
- [x] **Switchable finality.** Genesis picks the kind as `finality.type`: `ffg`; `authority`, the
  same votes from signers genesis names, with a threshold above half and no staking; or `depth`, final
  under a number of blocks with no votes. `chainv4 genesis --finality` writes each.
- [x] **Authority signers change by approval.** `Tx::ApproveSigners` (`send approve-signers`)
  names a whole new set and both thresholds; once `change_threshold` of the decided signers
  approve the same one for the same version, it is decided and fixed for epochs
  `activation_delay` on. `GET /signers` shows the ballot.
- [x] **Signer approvals expire and can be withdrawn.** An approval counts for genesis's
  `approval_epochs` epochs from the one it was made in, and only approvals that still count
  decide a change. `Tx::WithdrawApproval` (`send withdraw-approval`) takes one back.
- [x] **A signer is a key and an account.** Each authority signer names the account that approves
  changes for it and takes its rewards (`Signer`), its own key's address unless genesis or the
  approved set says otherwise, so the voting key signs nothing but votes, as under FFG.
  `approve-signers --add KEY@ACCOUNT` names one.
- [x] **Conflicting authority votes eject the signer.** `Tx::Slash` works under authority
  finality too: the evidence throws the signer out of the decided set, thresholds kept, version
  bumped, and out of the sets already fixed from two epochs on; refused when a set would be left
  unable to reach its thresholds.
- [x] **Depth finality is each node's own, by design.** It is a rolling checkpoint, as Bitcoin
  Cash keeps one: nodes split for longer than `depth` blocks never rejoin on their own, and an
  operator resyncs one side. Kept for development and single-operator chains, and documented as
  such.
- [x] **A slashed validator leaves the sets already fixed.** From two epochs after the slash on
  it is out of every set at once; the slash's epoch and the next keep it, so the next
  checkpoint's light update, which now re-states the sets of every epoch ahead, reaches light
  clients by the usual trust path.
- [x] **An inactivity leak.** FFG genesis's `leak` makes blocks carry every justification, so
  state sees finality move, and bundles of votes that fall short while it has stalled for
  `after_epochs` epochs, so state sees who still votes; each stalled checkpoint then burns
  `permille` thousandths of every quiet validator's bond, reweighing the sets ahead, until the
  rest hold two thirds again.
- [x] **Vote tallies are stored.** Every vote a node counts or holds back is kept in the
  Finality column by target epoch and voter, and counted again when the chain opens, so tallies,
  held votes and double-vote detection survive a restart. Counted votes stay 64 epochs behind
  the finalized checkpoint, to catch one vote surrounding another, and then go.
- [x] **Only voters are remembered.** Double-vote detection compares a vote against its voter's
  counted votes, and, once the vote shows its key votes, against that voter's held ones. Votes
  are remembered only when counted, so throwaway keys can neither grow memory beyond the capped
  held votes nor push real evidence out of the evidence log.
- [x] **Vote rewards can come from fees.** Genesis's `vote_reward_source: fees` sends the burned
  share of every fee to a reward pool and pays voters from it, each as much as the pool holds;
  `mint`, the default, keeps minting them.
- [x] **Checkpoint sync.** `--snapshot --checkpoint 0x…` starts a new node at a finalized
  checkpoint it is told to trust: it takes the checkpoint's headers from a peer by their hashes,
  holds it as justified and finalized, fetches nothing below it, and syncs on from there, so a
  long-range fork under it never reaches the node.
- [x] **Anchoring into Bitcoin (owner's idea, 2026-11-04).** Anyone writes a finalized checkpoint
  into a Bitcoin OP_RETURN, with the anchor list's root, and proves it with `Tx::Anchor` against
  the relay, earning a governance-set reward every `every` epochs. Anchors recorded while `enforce`
  is on bind nodes: no block replaces one. `--snapshot --anchored` starts a new node at the newest
  checkpoint of the history Bitcoin saw first, checked against the Bitcoin headers peers' relays
  hold, by their work. `bridge anchor` (or its alias `bridge btc anchor`) is the anchorer. Each part switches on its own under
  `Change::Anchoring`, all off when unset. No Bitcoin height went into headers: the relay's tip is
  chain state already. See [Anchoring](anchoring.md).
- [ ] **Validity proofs: other chains check our blocks, not just our timestamps (owner, 2026-11-05).**
  Anchoring proves that a checkpoint existed by a certain time. It doesn't prove the checkpoint was
  right: two thirds of the current validators could still finalize an invalid state. Validity proofs
  close that. A zero-knowledge proof that each block (or a run of them) followed the rules is checked
  on Ethereum, and perhaps on Bitcoin through BitVM-style fraud proofs, so a bad block is refused
  there whatever the validators signed. In stages, each useful on its own:
  1. **Finality on Ethereum:** a contract follows our finality by checking the votes, if voters sign
     with secp256k1 (`ecrecover`, cheap), or a ZK proof of the Ed25519/BLAKE3 votes. That is what the
     trustless-bridge note under the bridges needs too.
  2. **State roots by ZK:** a zkVM (SP1, RISC Zero or the like) proves the state transition: the
     native transactions first, then Lua, Wasm and the EVM, whose metering must run the same inside
     the prover. It is checked by a verifier contract on Ethereum.
  3. **Bridges and light clients by proof:** the gateways and the light client take a proven state root
     in place of the federation's word or a vote count.
  Questions for when it starts: which zkVM; proving every block or batches; who pays for proving; and
  whether the engines can be proven at all at a sane cost, Luau's C++ VM above all. A large project,
  queued after the todo, with the second pentest.
- [ ] **Anchoring: next.**
  - [x] **Ethereum anchors that count** (2026-11-05). `ethlight` and `eth_relay` follow the beacon
    chain. `Tx::Anchor` proves an Ethereum anchor against the relay with the finality update that
    finalized it, and anchored sync checks it through `--beacon`, Ethereum deciding where Bitcoin
    can't. The fork table and the network are governed (`anchoring.eth_start.network`).
    `bridge eth relay-sync` feeds the relay, as the `chainv4-eth-relay` service.
  - **Ethereum anchoring is parked (the owner's call, 2026-11-05).** The code stays and builds, but
    no deploy turns it on, and the `chainv4-eth-relay` service is not set up. Picking it up again
    needs two things:
    1. **Gloas.** Sepolia forked to Gloas (ePBS, version `0x90000076`) at epoch 353,024, about
       2026-10-06. ethlight knows forks only up to Fulu, so it cannot follow Sepolia past that
       epoch: it reads the wrong fork version and header layout. Gloas's `LightClientHeader` is the
       beacon header, `execution_block_hash` and an 11-node `execution_branch` (gindex 2,856, through
       `signed_execution_payload_bid.message.parent_block_hash`), 496 bytes with no offsets. The
       finality branch is 9 nodes (gindex 735), and the sync committee branches 11 (2,945 and 2,946).
       Older headers are brought forward by `upgrade_lc_header_to_gloas`. Mainnet and Hoodi are
       still on Fulu, and mainnet has no Gloas date.
    2. **Transactions proven through the execution chain.** A Gloas beacon block holds only the
       payload bid, so no SSZ path reaches a block's transactions any more. The route that works in
       every fork: start from the finalized header's execution block hash, walk the RLP block
       headers back to the anchor's block by keccak and `parentHash`, then prove the transaction in
       that block's transaction trie (Merkle-Patricia). That is roughly 550 bytes a block between the
       anchor and finality, about 35 KB at most if the anchorer keeps up. It needs keccak and a trie
       check in ethlight, a keccak host call for `eth_relay` (whose `includes` and `prove_header`
       move to the same route), and the anchorer building the trie from the block's raw
       transactions. EIP-2935's history contract, proven by `eth_getProof` against the finalized
       state root, would make it about 10 KB whatever the distance; that could come later. Once this
       lands, the anchorer sends at once, with no epoch-first-slot timing and no writing again.
  - **Bitcoin and Ethereum anchors take turns:** each carries the list root, so one recorded wastes
    the other's pending one. The anchorer counts each chain's `every` from its own newest anchor,
    writes to neither while the other has one pending, and lets Bitcoin go first when both are due.
    Provisional; the owner to confirm.
  - **`multisig bitcoin` commands still use their own single-URL Esplora code,** not the shared
    endpoints with fallbacks, bitcoind, logins and `--proxy`.
  - **The anchorer keeps its pending anchor on disk,** so a restart does not write another.
  - **Anchored sync without `--snapshot`:** running every block from genesis, refusing any branch
    without the picked checkpoint.
  - **Running nodes check their peers' anchors now and then,** and say so loudly if a peer's history
    beats their own. They cannot roll back past their finalized checkpoint, but an operator can
    resync.
  - [x] **Ethereum anchoring on its own.** `anchoring-eth` builds alone (the owner's call): the anchor
    list, the guard and anchored sync's framework move into a base either feature turns on, so a
    build may anchor into Ethereum without Bitcoin, Bitcoin without Ethereum, or both.
  - **Deploy it:** the relay keeps each header's version and nonce now, so the devnet's relay is
    deployed again. That, the anchorer service on .94 and turning it on by a vote go with the key
    rotation's genesis.
- [x] **Difficulty can retarget over a window.** Genesis's `consensus.window` makes difficulty
  the window's work over the time it took, each interval counting between a quarter and four
  targets, and bounds a block's timestamp by the window's median time, as Bitcoin does. The
  consensus rules take the ancestors they need; the chain, the light client and snapshot sync
  hand them over, and a checkpoint start carries enough headers. Unset, the parent-only rule
  stays and the chain keeps its id; the dev pow chain sets 11.
- [x] **Mining on the GPU.** The `chainv4-gpu` crate mines through wgpu, on Vulkan, Metal and
  WebGPU, with a WGSL kernel for each of SHA-256, BLAKE3, BLAKE2s, BLAKE2b and Keccak-256, tested
  against the CPU hashes; `--mine --mine-gpu` picks it in a build with the `gpu` feature.
- [x] **Every GPU, and workers.** `--mine-gpu` mines on every GPU, or those `--gpus` picks, each
  on its own share of the nonces. A node with `--mining` takes workers, `chainv4 worker` on any
  machine, which mine for it over a TCP port authenticated with a token, each in a range of nonces
  of its own, with no chain of their own; the node checks every nonce, and reports each device's
  and worker's hashrate on `GET /node` and the explorer.
- [x] **Workers find no shares.** Under `--pool` they do: every nonce at the worker's share
  difficulty, which measures the work each did, and the pool pays by it.
- [ ] **The mining port is not encrypted.** Its messages are authenticated with the token; TLS
  on it would hide the work and nonces too, which matter only until the block is out.
- [ ] **No CUDA kernels.** NVIDIA cards mine through Vulkan. Kernels in CUDA C, behind a `cuda`
  feature, would need a machine with nvcc and a card to write and test on.
- [x] **Side branches are pruned, and bad blocks are remembered across restarts.** When the tip
  moves, side branches that split off more than `max_reorg_depth` blocks back, or below the
  finalized checkpoint, are deleted, so the side blocks a node stores are bounded by the reorg
  window, each one sealed at the main chain's difficulty. The last 1,024 blocks that failed to
  run are kept in `Meta`, so a restart refuses their branches without running them again.
- [x] **A block gas limit.** On a chain with fees, a transaction is only taken while its gas
  limit fits what the block has left, and the header's gas used must match, both checked on
  import.
- [x] **Blocks are bounded by bytes on any chain.** Genesis's `max_block_bytes`, checked on
  import, bounds a block's transactions whether or not the chain has fees; the dev chains have
  4 MiB, and genesis files from before it keep their ids. The fixed 1,000-transaction cap is gone:
  producers fill to the bytes and gas limits, or to `--max-block-txs`.
- [x] **Admission off the node loop.** The verifier's gate checks and inserts transactions, so
  block building never starves the mempool.
- [x] **Gas estimation.** `POST /offer` dry-runs calls, deploys and upgrades and offers what they
  used plus a quarter; the CLI, wallet and explorer warn before sending what would fail.
- [x] **A producer can set a time budget.** `--max-build-ms` bounds how long a node runs
  transactions for a block it builds: what went in by then is sealed, and the rest wait for the
  next block. The bytes and gas limits are genesis's, alike on every machine; this and
  `--max-block-txs` are the brakes a producer sets for its own speed.
- [x] **Block rewards.** Genesis sets an initial reward and an optional halving interval; each
  block's producer is credited after its transactions run.
- [x] **Fees.** Every transaction pays for its gas, failed ones too, with what it does not use
  refunded. Genesis picks the gas schedule (`weighted`, or `contract`, where a contract's `score`
  says what is charged) and the pricing (`base_fee`, EIP-1559-style, or `bid`), with the block gas
  limit and the burned share of the base part; the rest goes to the producer. Producers take the
  best-paying transactions first. New schedules and pricings implement `GasSchedule` and
  `Pricing`.
- [x] **Transactions that cannot pay are dropped.** After each block the mempool drops what its
  sender can no longer pay for, and what has offered under the base price for 32 blocks in a
  row; selection stops a sender's run at an offer under the next base price, so the builder never
  runs what it would skip.
- [x] **A scorer runs for the kinds it names.** A `contract` schedule's `kinds` says which kinds
  of transaction call `score`; the rest are charged as weighed, with no call. It still only
  reads; a scorer that keeps counts would need its writes kept.
- [x] **The reward is visible in the API.** Every block summary, block page and stream event
  carries `reward`, what the block minted for its producer; the explorer shows it on the block
  page.
- [x] **Minting is for development chains.** Genesis's `minter` may be the development key or
  nobody, so `Mint` is a dev faucet and a chain of value gets coins from rewards and allocations
  alone.

### Thought experiments

**Not planned work.** Nothing here is to be built until it is decided to; these are notes to think
with.

- [ ] **Post-quantum signatures, a thought experiment for now.** What it would take for the chain
  to survive a machine that breaks elliptic curves:
  - **A scheme beside the others.** Key schemes are already one feature and one pinned tag each,
    so ML-DSA (FIPS 204), SLH-DSA (FIPS 205) or FN-DSA (Falcon) could be another `KeyType` without
    touching the rest.
  - **Size is the cost.** An ML-DSA-44 key is 1,312 bytes and a signature 2,420, against
    ed25519's 32 and 64. Fees are charged per byte, so each transaction would cost tens of times
    more. That reaches block size, `max_tx_bytes`, multisig pricing (the heaviest quorum), the
    mempool's memory and the light client's proofs.
  - **Keys exposed in addresses.** An `ed25519:` address is its key, so a quantum attacker could
    sign for it from the address alone. Addresses that are a key's hash (secp256k1, bitcoin) hide
    the key until it first signs. Moving coins would need a rotation path, for example an account
    that commits to a post-quantum key it switches to later, and a deadline for accounts that never
    move.
  - **Votes and finality.** FFG votes and checkpoints are signatures too, and many of them per
    epoch. They would grow the most, and aggregation would matter.
  - **Hashes hold up.** Grover's algorithm only halves a hash's strength, so 256-bit hashes such
    as blake3 and sha256 stay sound. Merkle proofs, state roots and proof of work are unaffected.
  - **Bridges are not ours to fix.** The Bitcoin and EVM sides sign with secp256k1 and Schnorr
    until those chains change, so a bridge is only as safe as the weaker side.
  - **Hybrids first.** A transaction signed both classically and post-quantum, valid only with
    both, is the usual first step while the new schemes are young.

### Privacy

- [ ] **Hidden recipients: stealth addresses.** A recipient publishes a stealth meta-key; a sender
  derives a one-time address from it by ECDH and sends an ordinary transfer there, with the
  ephemeral key the recipient needs, in a field of the transfer or an event. The wallet scans for
  what is its own and derives each one-time key. Spending from a one-time address needs fees there,
  and moving funds on can link addresses again. ERC-5564 is the model. Moderate: little or nothing
  in consensus.
- [ ] **Hidden amounts: a confidential store for every key.** Beside its public balance, an
  account holds a confidential one: twisted ElGamal ciphertexts on Ristretto, under an encryption
  key derived from the account's own, so the words bring it back. Transfers between confidential
  balances carry range proofs (Bulletproofs) and equality proofs, which validators check; deposits
  and withdrawals move between the public and confidential balances. Incoming transfers land in a
  pending balance, applied to the available one by its owner, so a sender proving against its
  balance never races what arrives. Amounts are split into small chunks, so the owner decrypts
  them quickly. Made the first time a key uses it, so idle accounts cost no state. Fees stay
  public, and contracts cannot read confidential balances. Solana's confidential transfers are the
  model; `curve25519-dalek`, `bulletproofs` and `merlin` have the pieces. Hard: weeks.
- [ ] **Hidden senders: a shielded pool.** In an account model the sender shows by construction:
  its nonce, signature and fee. Hiding it takes notes instead of balances, a Merkle tree of note
  commitments, nullifiers against spending one twice, and a zero-knowledge proof (Halo2 or Plonk,
  with no trusted setup) that the spender owns an unspent note without saying which; and relayers,
  or fees paid from inside the pool, so paying gas leaks nothing. Proving must run in the wallet,
  phones and the browser included. Inside the pool, senders, recipients and amounts are all
  hidden. A bug can mint money no one sees, so it needs review of its own. Months.

### Names

- [x] **A name registry.** Names such as `alice.dev` (`names` in genesis): registered for years at
  a price by length, burned; a target, text records, free subnames and primary names; a grace
  period after expiry; prices changed by governance's `Change::NamePrices`. `chainv4 send name`,
  `transfer --to NAME`, `GET /names` and `GET /names/{name}`, and accounts show their primary
  name. See [Names](names.md).
- [x] **Names in the wallet and explorer.** The wallet proves names, pays them, and manages them
  on a Names page; the explorer searches them and shows them on pages of their own and accounts'.
- [x] **Listing an account's names.** An index by owner in the names column, moved with the
  owner, for `GET /accounts/{address}/names`.
- [x] **Names inside contracts.** `names.resolve` and `names.name_of` in Lua, `resolve_name` and
  `name_of` for WebAssembly and the SDK, and the EVM precompile at `0x…4e53`.
- [x] **Names, staking and governance as features.** `names`, `staking` and `governance`, each
  on by default.
- [ ] **Unicode names,** with ENSIP-15's normalization against look-alikes.

### Governance

- [x] **Governance locks** (2026-11-05). Genesis's `locked` names paths no vote may change, a whole
  kind such as `reward` or one field such as `anchoring.enforce`. `Change::Locks` adds more and
  nothing removes one; a lock on `locks` freezes the set. Every vote is checked against them, in the
  chain, in the CLI before it signs, and in the wallet's form, which shows locked fields read-only.
  `GET /governance` and the explorer list them. See [Governance](governance.md#locks).
- [x] **Prove the locks to the apps.** The wallet and the explorer read `locks` and `in_force` from
  the node, unproven. `change:locks` is one state entry, so a light-client proof of it could back
  them, as it backs balances. Done 2026-11-06 in the wallet: `change:locks` proven at the tip
  (`Changes::locks_key`); the explorer still reads them from the node.
- [x] **Tidy the warnings in a build without governance:** an unused `Changes` import in
  `execute.rs`, and four unused helpers in `chain/finality/switch.rs`. Done 2026-11-06: the import and the switch's
  helpers are built with governance alone.

- [x] **Governance over the consensus.** Genesis's `governance` names a kind, behind the
  `Governance` trait: `electorate`, finality's own validators by stake or signers by
  `change_threshold`, or `council`, accounts genesis names with a threshold. `Tx::ApproveChange`
  proposes and approves a new consensus, one approval per account, moved by approving another
  and taken back by `Tx::WithdrawChange`, expiring after `approval_epochs`; once the approvals
  weigh enough it is decided, and the next checkpoint fixes it `activation_delay` epochs ahead
  as finality fixes its voters. Blocks are checked by their own epoch's rules, light updates carry
  each epoch's consensus with a proof, light clients and snapshot sync wait for it as they wait
  for producers, and a node started with `--mine` and `--produce` builds the producer for
  whatever the chain runs. `GET /governance` shows the ballot.
- [x] **Finality's parameters by governance.** `Change::Finality` retunes the same kind:
  `vote_depth`, rewards, proofs per block, the bond rules and the leak under FFG, `max_signers`
  and `approval_epochs` under authority. The kind, `checkpoint_interval`, `activation_delay`,
  the genesis voters and the thresholds stay, since light clients and the fixed-ahead sets rest
  on them. Decided in epoch e, it runs from `e + activation_delay + 1`, each era kept in state.
- [x] **Switching the finality kind by governance.** `Change::SwitchFinality` moves FFG to
  authority or back from an epoch it names, the epochs kept: validators bond for a switch into
  FFG, which lapses should none have by the checkpoint that fixes its first set, and unbond at a
  switch out of it, still slashable; signers come from the switch. One electorate asks each
  epoch's kind, the checkpoints carry over, and light updates name the switched epochs. Depth
  is out: governance needs voting finality.
- [x] **Governance over fees, rewards and limits.** `Change::Fees`, `Change::Reward` (halving
  from its first block) and `Change::Limits` (`tx_limits` with `max_block_bytes`), on the same
  ballot, each from the block after the deciding one. Every block reads them from its parent's
  state, so reorgs undo them; the mempool, the producer and the API read the tip's.
- [x] **Governance pages in the apps.** The deciding approval's receipt says `Decided(version)`,
  so light clients prove decisions; nodes index them for `GET /governance/history`, and
  `GET /governance` adds every setting in force and who may approve. The explorer shows the
  ballot, what stands and the history; the wallet approves, withdraws and proposes any change.
- [x] **A version per kind of setting.** Each `ChangeKind` has its ballot and version (the
  consensus, finality's tuning and its switch sharing one), so a decision voids only the open
  changes of its own kind; an account backs one change of each kind; fees, the reward, limits,
  finality, the federation and the directory may be given in part, laid over what stands; the
  deciding receipt says `Decided { decision, kind, version }`, the chain's count kept gap-free for
  light clients.
- [ ] **`_` as a digit separator everywhere a number is read from text (owner, 2026-11-06).** In
  contracts, a consensus change for the next regenesis or fork: `u64()` to `i256()` and each
  `.new`, decimal and `0x` hex, and `tonumber` in the Luau fork, as Luau's own literals take
  `1_000_000`; with them the SDK's `U256::from_decimal` and `I256::from_decimal`, which mirror
  Lua's `u256(text)` and `i256(text)` and so change with them. Outside consensus, done at once: the
  CLI's amounts, coins, spans and typed arguments. Only between two digits: `_1`, `1_`, `1__0` and
  `0x_ff` are refused, stricter than Rust's and Luau's literals, so a slip is not read as some
  other number.
- [ ] **Fuel costs measured, not guessed.** `vm::cost` is a table of hand-picked constants
  (a storage write 500, a call 1,000, a hash 20…) compiled into the node. A Rust tool, run at
  build time or as `chainv4 calibrate`, would time each fuel-charged operation on a reference
  machine, Lua and WebAssembly instructions, every host call, signature checks and transfers, and
  the EVM's gas against them, and write the cost table and a fee schedule from it. Genesis would
  carry the table, so each chain fixes its own prices, and a governance change could replace it
  later, as the item above would let fees be.
  - **Lua too, down to the opcode.** The Luau fork meters fuel as a script's instructions run;
    the same tool would time each Luau opcode (arithmetic, table reads and writes, calls,
    closures, string building, the integer userdata's methods) and each library function, so
    Lua's prices come from what they cost, in a table genesis carries like the rest.
  - **Every engine's opcodes, one by one.** A benchmark per WebAssembly instruction and per EVM
    opcode as well, run under one harness, with the results printed beside the prices charged, so
    an operation priced far below what it costs, the way to stall every node with cheap work,
    shows up at once.
  - **Repeatable.** Runs pinned to one machine and build, each result with its spread, kept with
    the release, so a change to the engines that moves a cost is caught before it ships.
- [ ] **Changing a running chain's engines.** Which engines a chain runs, Lua, WebAssembly and the
  EVM, is fixed by genesis's `engines`: adding or dropping any of them today takes a new genesis.
  A `Change::Engines` by governance naming the whole new set, several engines at once, taking
  effect at a checkpoint like a consensus change, would let a live chain turn engines on and off,
  with the node building its `Vm` per era. Open: what becomes of contracts of an engine turned off
  (refused calls, or frozen but readable), and a node built without an engine the chain turns on
  stopping with a clear error at that height.

### Script layer

- [x] **Push the Luau and mlua forks, then drop the local `[patch]`.** Done 2026-11-05: both forks
  are pushed (`germ3n/luau-src-rs` 93df670, `germ3n/mlua` 3832a79), the `[patch]` is gone, and
  `Cargo.lock` builds from them on GitHub.
- [x] **A Lua block came out two ways on one machine (seen 2026-11-05).** Fixed 2026-11-05. Luau's
  collector timed its cycles by the clock (`getheaptrigger` read `lua_clock()`), so under load it
  freed garbage at other points, and a call near the memory limit failed at another allocation, for
  other fuel. Now every cycle starts at the heap goal, the limit is counted in Luau from zero by the
  sizes it asks for, and pristine states are collected once built. Proven by 60 calls under load,
  which came out four ways before and one way after.
  `chain::tests::pentest_lua::building_big_strings_stops_at_the_memory_limit` once failed with
  `Invalid(ReceiptsRoot)`, under load from a parallel build. The block that was built and the same
  block imported gave different receipts. It passed every time alone. The likely cause is Lua's
  memory limit: where an allocation crosses it, and so the fuel used before a transaction fails,
  can follow when the collector runs, which load changes. If that's it, it is a consensus split
  waiting to happen. Find out what differs between the two runs, and make running out of memory
  deterministic. One way is to count what the script asked to allocate, not what the allocator
  holds. Another is to collect at fixed points.
- [ ] **Check that memory limits and fuel metering come out the same on every OS and machine.**
  Lua's memory now counts 64-bit object sizes. These should match on x86-64 and arm64, but would
  differ on any 32-bit or wasm32 build of a node.
  Asked about 2026-11-04, during the pentest. A transaction that runs out of fuel or memory on one
  node and not another splits the chain, and the devnet already mixes x86-64 Linux servers with
  arm64 macOS laptops and Android phones. For each engine, run the same contracts and blocks on
  each kind of node and compare the fuel used, where each one fails, and the state root:
  - **Lua:** whether the Luau fork's fuel counts the same instructions everywhere, and whether
    its memory limit counts the same bytes: allocator sizes, string and table overhead, and GC
    timing differ by platform and pointer size, so a script near the limit may pass on one and
    fail on another.
  - **WebAssembly:** whether wasmi's fuel and its memory and stack limits give the same results
    across builds and targets.
  - **The EVM:** whether revm's gas and its memory limit (`memory_limit`) agree everywhere.
  - **The host:** anything charged by size in memory rather than by encoded bytes.

  Run it in CI on Linux x86-64, macOS arm64 and an Android or arm64 Linux target, and turn
  whatever disagrees into a deterministic rule. Related: the pentest's finding that Lua's `^`
  calls the platform's `pow()`.
- [ ] **Test the Lua fixes across OSes and CPU architectures (the owner's ask, 2026-11-05).** That
  means the memory fix (a1134bc, Luau 93df670, mlua 3832a79): the limit counted from zero by the
  sizes Luau asks for, and collection paced by the heap goal, not the clock. It also means the
  fuel fixes (Luau 325972b): the pattern matcher's steps, what the string, table and utf8 libraries
  and concatenation go over, and `^` by repeated squaring. Run them on Linux x86-64, Linux arm64,
  macOS arm64, macOS x86-64, Windows x86-64, and Android and iOS arm64, idle and under CPU load.
  Use the existing tests (`memory_heavy_calls_come_out_the_same_in_any_state_under_load`,
  `building_big_strings_stops_at_the_memory_limit`, and the fuel and `pow` tests), plus a fixture
  of calls that prints each call's fuel, its failure, the bytes counted and the receipts root. Then
  compare the printouts byte for byte across machines: they must be identical. The CI jobs from the
  item above can run it once they exist; until then, run it by hand on the servers, this Mac and
  a phone.

- [x] **The EVM.** revm runs Solidity and Vyper contracts beside Lua and WebAssembly (`evm`
  feature, `evm` in genesis's `engines`): `Code::Evm` deploys, `Tx::EvmCall` calls or pays,
  `Address::Evm` holds what the EVM creates, and every other account takes part at its 20 bytes,
  secp256k1 as itself and the rest under a keccak256 alias written down when it first sends a
  transaction. Balances and nonces are shared, logs become `log` events, EVM gas is chainv4
  gas. `chainv4 send deploy --evm` and `chainv4 send evm-call`, with `--read` for a dry run. See
  [EVM](evm.md).
- [x] **Ethereum-signed transactions and JSON-RPC.** `Tx::Ethereum` takes legacy EIP-155,
  EIP-2930 and EIP-1559 transactions as wallets sign them, and `POST /rpc` answers Ethereum's
  JSON-RPC from an index of them by their keccak hash. See [Ethereum's tools](ethereum.md).
- [x] **Pending transactions over JSON-RPC.** `eth_getTransactionByHash` finds one waiting in the
  mempool, with no block yet.
- [x] **The EVM in the wallet and explorer.** The wallet's Contracts page reads, calls and deploys
  contracts of every engine, EVM ones by their ABI; the explorer reads EVM contracts with
  `eth_call` and shows their logs and calls by name. MetaMask and Foundry are in
  [Ethereum's tools](ethereum.md). The ABI they share is `src/abi/`.
- [x] **Calls between the EVM and the other engines.** The native precompile at `0x…4e43`
  calls Lua and wasm contracts with ABI-encoded arguments mapped to native types, and Lua's
  `evm.call` and the SDK's `evm_call` call the EVM, with `evm.encode` and `evm.decode` for the ABI.
  Calls nest across engines to `MAX_CALL_DEPTH`, and a revert on either side unwinds both. See
  [EVM](evm.md#calls-with-lua-and-webassembly-contracts).
- [x] **An alias registered without an EVM transaction.** On a chain with the EVM, any transaction
  writes its sender's alias down, so coin the EVM pays it reaches the account. An account that has
  only received still has none.

- [x] **A nil value.** `TxCallArgumentType::Nil` (tag 11): Lua's `nil` both ways, so functions
  may return it, `null` in the API, `nil` on the command line, `Value::Nil` in the SDK.
- [x] **Contracts can move funds.** `accounts.balance` and `accounts.transfer` in Lua, the
  `balance` and `transfer` host functions in WASM, `balance` and `transfer` in the SDK.
- [x] **Calls between contracts can send value.** `contracts.call_value` in Lua, the value
  argument of `call` in WASM, `call_with_value` in the SDK. The callee sees it in its context,
  and it goes back if the call fails.
- [x] **Events.** `events.emit(name, ...)` in Lua, the `emit` host function in WASM, `emit` in
  the SDK. They land in the transaction's receipt, in order, only if the call and every call
  above it succeed.
- [x] **Events can be searched.** A node started with `--index-events` indexes every event by its
  contract, by its contract and name, and by every address in its data; `GET /events` lists them
  newest first, a page at a time, and the explorer shows them on contract and account pages.
- [x] **The event index catches up.** `--index-events` takes `genesis`, a height or `latest-N`.
  The node reads older blocks' receipts back in the background, a step at a time, and a
  `latest-N` window drops the oldest blocks' events as the tip moves.
- [x] **The event index can be turned off.** `--index-events=off` deletes it at startup with
  `Database::clear`, one atomic range delete however big it is.
- [x] **Loading code is charged.** A deploy or upgrade pays `cost::LOAD_BYTE` (1 fuel) a byte to
  compile its code, plus what a Lua script's top level burns, and a failing one pays for what it
  burned before it failed. A top level runs before a call is attached, so it is the same work at
  deploy and on every call, and every call pays for it.
- [x] **Read-only calls.** `POST /call` runs a contract function as a transaction would call it,
  on a throwaway copy of the committed state under the chain's fuel cap.
- [x] **Traces.** Every receipt keeps where its contracts went: each call, delegate and transfer,
  failed ones included, which events each emitted, up to 256 hops. Not consensus.
- [x] **Failed deploys and upgrades keep their trace and fuel.** When loading, `init`,
  `check_perms` or `migrate` fails, the receipt still shows where it went and what it burned.
- [x] **Contract upgrades.** `Tx::Upgrade` (`chainv4 send upgrade`) swaps a contract's code and
  keeps its address, storage and balance. The contract decides who may: the chain asks its
  `check_perms("upgrade", sender)`. A contract without it is immutable. New code's `migrate`
  runs once, and a failure undoes the whole upgrade.
- [x] **Constructors.** `init` runs once at deploy with the deployer as sender; its fuel and events
  land in the deploy's receipt. Only the chain may run `init` and `migrate`.
- [x] **A token standard.** ERC-20-like, by convention: [docs/token-standard.md](token-standard.md),
  with example tokens in Lua and Rust that one shared test holds to it.
- [x] **A DEX.** Constant-product pools, as Uniswap V2 keeps them: `contracts/lua/dex.lua` for the
  coin, tokens and ERC-20s on the EVM, each pool's shares a clone of `contracts/lua/lp_token.lua`,
  and `contracts/evm/Dex.sol`'s WDEV, factory, pairs and router; `contracts.clone` for contracts
  that make contracts; `chainv4 dex`; the wallet's Trade page. See [DEX](dex.md).
- [ ] **Decide what the DEXes' fees are for.** Today every trade's 0.3% stays in the pool, for its
  liquidity providers alone, on both DEXes, fixed in the code. Decide whether part of it goes
  elsewhere (a protocol share to a treasury, burned like the chain's fees, or to stakers), whether
  the rate is one for all pools or set per pool, and whether governance sets either, as it sets
  the chain's own fees; then the native DEX, Dex.sol and the apps' quotes follow.
- [x] **The DEX and swaps in the explorer.** A DEX page, both DEXes' pools with prices, volume and
  fees over a day and a week, a pool's price chart and latest activity; a Swaps page, the swap
  contract's locks, claims and refunds with each lock's finality, and a swap found by hash or by
  its code. See [Explorer](explorer.md).
- [ ] **The DEX: next.** Price oracles (time-weighted prices, as V2's cumulative ones), flash
  swaps, a fee switch for the protocol, concentrated liquidity (V3's ranges), and routing across
  both DEXes at once.
- [x] **Native calls in EVM traces.** Done 2026-11-04. The EVM's inspector records every frame as a
  hop (calls, delegate calls, creates as the new `create` kind), and the native precompile puts each
  native call's hops under the frame that made it, events placed where they land; `evm.call` from Lua
  or WebAssembly nests the EVM's frames under its hop. See [EVM](evm.md#calls-with-lua-and-webassembly-contracts).
- [x] **Token refreshes from native calls the EVM makes.** Done 2026-11-04. They travel out with the
  EVM's run, of the native calls that stand, and an EVM transaction refreshes them as a native one does.
- [x] **The wallet's Contracts page calling across engines.** Done 2026-11-04. Each native function has
  **EVM calldata**: the precompile and `callNative`'s calldata, from `chainv4::abi::call_native_input`.
- [x] **Proven contract reads.** Done 2026-11-06: the desktop and Android apps run every read
  themselves over the state the node proves (`POST /call` with `"prove"`, `vm::run_proven`). Left
  open: reads that scan storage, the rules besides state, and the browser. See
  [Proven contract reads](light-client.md#proven-contract-reads).
- [x] **Old state over JSON-RPC.** Balances, nonces, code, storage and calls at an older block,
  by number, tag or hash, from the node's history. See [Ethereum's tools](ethereum.md).
- [ ] **Blob and EIP-7702 transactions.**
- [x] **NFTs.** Two standards beside the fungible one: NFTs, ERC-721-like, and multi-token
  collections, ERC-1155-like, with safe transfers a receiving contract agrees to, owners and
  amounts kept provably, and `token_uri` metadata. Reference collections in Lua and Rust held to
  them by one shared test each, and a factory that clones them; the collection registry, set by
  genesis or turned on by governance; the wallet's NFTs page, proving what the account holds and
  showing pictures once Settings lets it fetch them; the explorer's NFTs pages; `chainv4 nft`; and
  NFTs in atomic swaps, on chainv4 and EVM chains. See [NFT standards](nft-standard.md).
- [ ] **NFTs: next.** A marketplace contract, which would pay royalties; EVM collections in the
  apps, for the consensus batches (owner, 2026-11-06): ERC-721 and ERC-1155 contracts on the
  chain's own EVM, listed by the collection registry as native ones are, an ERC-165 check at
  deploy deciding; the apps prove their ownership by running `ownerOf` and `balanceOf` on their
  own EVM over proven state, and the NFTs page sends them as an EVM `safeTransferFrom` or
  `safeBatchTransferFrom` call. Done 2026-11-06: batch transfers and royalties on the reference collections
  (docs/nft-standard.md), SVG and animated GIF pictures, royalties shown and batches sent in the
  wallet.
- [x] **Containers where they matter.** `eth_getLogs` matches its address and topic filters as
  sets, refuses ones past 1,000 addresses or topics at a place, and reads its blocks one at a
  time. The proofs a block carries are picked among same-epoch links in one order, and the chain's
  tips are kept and listed by `GET /forks` in one too. Held votes are found by voter, not by
  scanning them all. The explorer's block map lays its lanes out once a change, not every frame.
- [x] **State reads copy no key.** A layer, and MemoryDb, keep their records by column then key,
  so a read borrows its key. Imports a full block of transfers about 6% faster in memory.
- [x] **Native multisig, across chains.** On chainv4 itself, done: `Address::Multisig`, made by
  `Tx::CreateMultisig`, its transactions `Tx::Multisig` backed by members' signatures off chain
  and `Tx::ApproveMultisig` on chain together, the multisig paying, members changed by its own
  quorum, in the CLI, the wallet and the offline signer (see [Multisig](multisig.md)). On Bitcoin,
  done too: members name BIP48 xpubs, wsh(sortedmulti(…)) with fresh addresses and a descriptor
  other wallets import, PSBTs built, signed, combined and finished in the CLI, the wallet and the
  offline signer, checked against Bitcoin Core on regtest. Swaps from a multisig too, its quorum in
  the HTLC's branch, in the CLI, the wallet and the signer. On EVM chains, done: MultiSig.sol at
  one address on every chain, through the deployment proxy, its first owners kept on chain, calls
  signed as EIP-712 bundles by the CLI, the wallet and the offline signer, owners synced when the
  members change, and swaps from a multisig on EVM legs. The original note: Accounts that need M of N keys to move anything,
  built into the chain rather than a contract: the keys may be any kind the chain takes
  (Ed25519, secp256k1, Bitcoin), and the same signers hold matching multisig on the other
  chains swaps reach, a Bitcoin P2WSH M-of-N and an EVM Safe-style contract, so one quorum
  controls funds on all of them; the wallet and the offline signer collect signatures, and
  swaps can be made from a multisig.
- [ ] **Owners of other key kinds on a multisig's EVM contract.** MultiSig.sol checks secp256k1
  signatures alone, `ecrecover`'s, so every member names a secp256k1 EVM address. It could take
  P-256 owners, passkeys, where a chain has RIP-7212's precompile; contract owners through
  ERC-1271, another Safe or a smart-account wallet; or Ed25519 checked in Solidity, at hundreds of
  thousands of gas a signature. Each changes the contract, and so the address, for multisigs made
  after.
- [x] **Delegate calls and fallback.** `contracts.delegate` / `delegate` run another contract's code
  on the caller's storage, like `delegatecall`; `fallback` answers functions a contract does not
  export. Together they make proxies, upgradable without the built-in upgrade, with examples in
  Lua and Rust.
- [ ] **No contract removal.** May be the right call; decide on purpose.
- [x] **Lua contracts can pass values on untouched.** `ctx.args` is the call's arguments as a
  values list, which `contracts.call`, `call_value`, `delegate` and `events.emit` take in place
  of loose values; a call given one returns one, and a function returning one returns its values
  as they are. The Lua proxy forwards this way, so no value changes type passing through.
- [x] **Namespaced storage.** `storage.namespace(name)` and `storage.own` in Lua,
  `storage::namespace` and `storage::own()` in Rust, have keys of their own, apart from plain
  storage and each other. Code a contract delegates to may read its own space but never write
  it, so the example proxies keep their owner and implementation there, out of their
  implementations' reach.
- [x] **Storage layouts.** Code declares the namespaces it keeps its state in, and what each
  holds (`--@namespace NAME KIND`, `--@retired NAME`; `namespace!` and `retired!` in Rust), and
  may use no others. `Tx::Upgrade` refuses new code that drops one or changes its kind, proxies
  check with `check_layout` before repointing, and `chainv4 layout` compares two versions
  before anything is sent.
- [x] **Layouts reach keys, kinds and plain storage.** A namespace is a map, any key with values
  of one kind, or a record, only the keys it declares (`--@key NAME KEY KIND`), each with its
  kind; keys retire like namespaces. Kinds are encodings the node checks on every write, and
  code that declares a layout may only read and clear plain storage.
- [x] **Map keys have kinds.** `--@namespace NAME KEYKIND -> KIND` checks every key read, written
  or cleared, so code that changes how it forms keys, `who:hex()` to `who:text()`, fails at once
  instead of starting from empty entries. A map of any key may gain a key kind once.
- [x] **Kinds name their units.** `decimal:wei`: no write can check what a number means, so the
  layout says it, and an upgrade must keep it, or retire and redeclare.
- [x] **Either engine keeps either's state.** Lua has get_ and set_ methods for the binary kinds
  Rust writes, and the SDK for the text kinds Lua writes, so a contract moves between engines by
  upgrade keeping every namespace: `contracts/examples/points` is version 3 of the Lua points.
- [x] **No floats leave a contract.** Lua numbers go out as `i64`, whole and within 2^53, or the
  call fails; `math` keeps only its whole-number functions; the `number` kind holds whole
  numbers in digits.
- [x] **No memory address reaches a result.** `tostring` of a table, function or userdata gives
  its type's name, and failure text has addresses cut, so a script cannot return, store or fail
  with something that differs between nodes.
- [x] **Tables keyed by tables iterate alike on every node.** The Luau fork hashes tables and
  functions by the order they were made, and refuses userdata keys, where an Address is a new
  value each time and two equal ones never meet.
- [x] **Lua still computes with floats inside a script.** Done 2026-11-05 in the Luau fork (the
  owner's pick, whole numbers rather than int64 or an opt-in integer type): a number is whole
  within 2^53 − 1 either way, a fractional literal does not compile, `/` fails unless it divides
  evenly, `//` and `%` are exact in integers, and a result past the bound fails. The compiler folds
  by the same rules and folds none of the math functions scripts lack. Was: A plain number is a
  float, so `/` makes fractions, which only fail once they would leave.
- [ ] **Ship the forks' upstream merge.** On 2026-11-05 germ3n/luau-src-rs and germ3n/mlua were
  merged with upstream on their `upstream-merge` branches: Luau 0.653 to 0.745, mlua 0.10.2 to
  0.12.2. The whole-number rule above is on the Luau one. On 2026-11-06 both were pushed and
  fast-forwarded onto the forks' `main` (luau-src-rs 3fad1fb, mlua 46bc6e2), and chainv4's
  `lua-upstream` branch locks those commits with no `[patch]`. Left to ship it:
  - merge into master;
  - regenesis, as contract results and fuel change.

  Known on the forks: upstream's new `require`, `loadstring`, integer library, classes and
  coroutine `close` are left out or off. mlua's user values, `take` and `destroy` keep a table keyed
  by userdata, which the fork refuses. mlua's own tests that use fractions, coroutines or `require`
  fail by design.
- [x] **Integer types in Lua.** `U64`, `U128`, `U256`, `I64`, `I128` and `I256`, each overflowing
  at its own width, with Rust's `checked_`, `saturating_` and `wrapping_` methods; arguments and
  the integer kinds arrive as their type. The SDK's `U256` and a new `I256` have the same
  operators and methods, with plain integers on either side.
- [x] **Numbers compare with integers.** The Luau fork asks a userdata's metamethods for a number
  on the other side, and for another userdata type, so `u256(5) == 5` and `u64(1) < u256(2)`.
- [x] **Signed big integers everywhere.** The `i128` and `i256` kinds and an `i256` argument
  type, in the CLI, the JSON API and the explorer.
- [ ] **Only one permission exists.** `check_perms` is only ever asked about `"upgrade"`. Other
  privileged actions, such as removal, would ask with their own names.
- [x] **A token registry.** A contract that exports every function genesis names in
  `token_functions`, ERC-20's six by default, is registered as it deploys or upgrades, with what
  its optional `name()`, `symbol()` and `decimals()` answer; `GET /tokens` lists them by symbol.
- [x] **Tokens refresh their metadata.** `tokens.refresh()` in Lua, `refresh_token()` in the SDK,
  asks the registry to ask a token's name, symbol and decimals again once the transaction
  succeeds; an answer that does not fit fails it.
- [x] **Faster calls, same results.** Compiled WebAssembly modules are cached by their code, and
  the Lua engine builds fresh states ahead and closes used ones on threads of its own. Every call
  still runs in a fresh state or instance, so neither changes what a call does.
- [x] **Blocks run on every core.** Transactions run side by side on the block's starting state,
  recording what they read, then in order, keeping each result unless an earlier transaction wrote
  what it read and running those again. The result is what running them one by one gives, and
  signatures, in transactions and in finality proofs, check side by side.
- [x] **Transactions that depend on each other run once.** Each transaction waits for the last
  one before it that names its sender, recipient, contract or staking and signer records, and
  runs in rounds once its wait is over, with signatures checked alongside the first round. Only
  a contract reaching what nobody named, and reading what an earlier one wrote, still runs twice.
- [x] **Signatures are checked off the node loop.** A verifier thread checks arriving
  transactions' signatures in batches on the chain's workers, and hands every event on in order.
- [x] **Gossip waits in a bounded backlog.** Transactions and votes from the network, and
  submissions, count against `--max-gossip-backlog` in all and `--max-gossip-per-peer` per peer.
  Past them gossip is dropped and submissions get 503; a peer over its share 10 times in a minute
  is disconnected. See [Node](node.md#the-gossip-backlog) for what hardening is still to do.
- [x] **Typed exports in the SDK.** `#[export]` reads the call's arguments into the function's
  parameters and returns what it returns, refusing arguments that do not fit; the token and
  counter examples use it.
- [x] **The Luau fork is pinned by branch, not commit.** `Cargo.lock` holds the commit, but
  `cargo update` would move it. Like wasmi it is consensus: pin a `rev` in `Cargo.toml`. Done 2026-11-06: mlua by `rev` in
  Cargo.toml, and Luau, which mlua-sys takes from its fork's main, by the commit in
  `[package.metadata.consensus]`, which `tests/toolchains.rs` holds Cargo.lock to.

### Database

- [x] **A state root and a receipts root.** Each header's `state_root` is the root of a sparse
  Merkle tree over every chain-state entry, kept incrementally in the `StateTree` column, and its
  `receipts_root` a Merkle root over what its transactions did. Blocks with wrong roots are
  refused; `GET /proofs/{column}/{key}` and `GET /proofs/receipts/{id}` serve proofs that
  `StateProof::verify` and `crypto::merkle::verify` check.
- [x] **Snapshot sync.** `--snapshot` checks every header peers have, walks a recent pivot's state
  tree from them a part at a time, checking each part against the pivot's state root and skipping
  what it holds already, and makes the pivot its tip. A pivot that grows too old moves up, and
  the walk fetches only what changed.
- [x] **A light client.** `chainv4::light` follows a chain from its genesis id alone: headers by
  consensus, finality from justifications whose voters it proved from checkpoints' state, and
  values and receipts by their proofs. `chainv4 light` and the explorer run it.
- [x] **Snapshot nodes serve finality before their pivot.** The justifications and light updates
  their light client checked while checking headers are stored with the snapshot; an update must
  hold every epoch's voters its block fixes.
- [x] **Light clients resume.** `LightClient::save` keeps what it checked from the finalized block
  on, at most 4,096 headers, and `restore` takes it back for the same genesis. The wallet saves it
  as it goes.
- [x] **Restored light clients reach older blocks.** `deliver_ancestors` takes the headers below
  the lowest one a client holds, proven by their hash chain, so a receipt in a block older than
  it kept still checks.
- [x] **Light clients start at a checkpoint.** A finalized checkpoint's block id, given, or a
  node's own the first time an app meets it, starts a light client there in one request.
- [x] **`chainv4 light` still keeps nothing between runs.** Done 2026-11-06: `.chainv4/light/<genesis
  id>.bin`, or `--state FILE`, and `--fresh`. Was: it followed from genesis, or from
  `--checkpoint`, each time.
- [x] **Snapshots come from one peer at a time.** Done 2026-11-06: the state walk takes a position a
  peer from every peer serving the pivot, up to `--sync-peers`, and moves the pivot only once none
  serves it.
- [ ] **A pivot only stays servable for `max_reorg_depth` blocks.** Frozen snapshots kept by serving
  nodes would suit large states, whose walk outlasts the window.
- [x] **Signing offline.** The wallet installs as an online wallet or an offline signer that makes
  no requests; an online one holds watch-only wallets beside ones with keys. Pairing, requests and
  signed transactions travel as QR codes read off photos, or as text.
- [x] **Messages too long for one code go as parts,** put back together in any order. The screen
  cycles through them, about three a second, and the other device's live camera reads each as it
  comes round: getUserMedia in the browser, Camera2 on Android, nokhwa on the desktop.
- [x] **Fountain-coded parts.** The signer's codes are Uniform Resources, and a long one animates
  fountain-coded parts, of which a camera needs any enough.
- [x] **The explorer pins genesis on first use.** Done 2026-11-06: Settings → Light client takes a
  genesis id to trust before it meets a node.
- [x] **Proofs at older blocks.** History keeps what each block replaced, state and tree nodes,
  by key and height, so reads, calls and proofs at any kept height cost what they do at the tip:
  `--state-history N` blocks, or `--archive`. See [Storage](storage.md#history).
- [x] **A Merkle `tx_root`.** It is the root over each signed transaction's leaf, as the receipts
  root is over receipts, and `GET /proofs/receipts/{id}` proves the transaction with its receipt:
  `check_tx` gives who sent what.
- [x] **An address index.** `--index-addresses` keeps every transaction under each address it
  touches, with its own window and column, beside the event index, which shares its catch-up.
  `GET /accounts/{address}/txs`, `chainv4 light history` and the explorer list from it.
- [x] **Token balances are provable.** By the token standard, each holder's balance is in the
  token's `balances` namespace under its address text, so a light client proves it with
  `token_balance_key` and `check_state`.
- [x] **Contract payouts are provable line by line.** Each receipt commits to the coin its
  contracts paid, native and EVM, in the order they paid it; the address index lists the
  transaction under everyone paid, and the wallet, the explorer and `chainv4 light history` show
  and prove each line. See [Chain](chain.md#receipts).
- [x] **Commits can be undone.** Each main-chain block keeps an undo record of every value it
  replaced, for the last 100 blocks.
- [x] **Prefix scans.** `Database::scan` visits a column's keys with a prefix, in order, from a
  start key: one seek, then only what it visits, on both backends. Finality uses it to find
  stored votes.
- [x] **State layers scan, and contracts iterate their storage.** A `State` merges its changes
  into a scan; Lua's `storage.iter` and `Namespace:iter`, the SDK's `iter` and `scan`, and the
  `storage_scan` and `namespace_scan` host calls go through a contract's keys in order, a page at
  a time, and a transaction that scanned runs again when one before it wrote into the range. See
  [Contracts](contracts.md#iterating-storage).
- [x] **No pruning.** Done 2026-11-06: `--prune-blocks N` drops old bodies, receipts and index
  entries below the finalized checkpoint, keeping headers. See [Pruning](storage.md#pruning).
- [x] **Receipts and a transaction index.** Each transaction's block, index, fuel, result and
  events are stored by transaction id, and reorgs move them along with everything else.
- [x] **A storage format version.** `Meta` records `STORAGE_FORMAT`, and a node refuses a
  database kept in another (`OldFormat`) rather than misread it.
- [x] **No migrations.** Stale: `chain::migrate` and `chainv4 db migrate` move a database up and
  down from format 8 on. See [Migrations](storage.md#migrations).
- [x] **Leaves are stored.** The chain's leaf blocks go into `Meta` with each block, so side
  branches outlast a restart and the API can list them (`/forks`).
- [x] **Leaves are pruned.** A side branch, its blocks and its leaf go once the chain can never
  follow it: split off below the finalized checkpoint, or past `max_reorg_depth` without
  finality, after every move of the tip (`prune_side_branches`).
- [x] **Commits are fsynced.** Each commit waits for the disk, so a power cut loses no block the
  node stored; `--no-fsync` turns it off for benchmarks and tests.
- [x] **RocksDB is tuned.** One block cache for every column (`--db-cache`), Bloom filters,
  LZ4 for state and ZSTD for blocks, receipts, undo and events. See
  [Storage](storage.md#tuning).
- [x] **API reads are snapshot-consistent.** Each request reads one database snapshot, so the
  tip and every key it reads are from one commit. See [Storage](storage.md#snapshots-reading-one-commit).

### Network

- [x] **What a node shares of itself.** `--report full|public|minimal` with `--hide` and `--show`
  on top decide what `GET /node`, `/stream`, `GET /pool/{address}` and `GET /mining` say, addresses
  and workers' names hidden by default; this machine and `--report-token` see everything, and the
  explorer says what is hidden and sends the token from Settings. See
  [What a node shares of itself](node.md#what-a-node-shares-of-itself).
- [x] **A peer heard from one way only forks the chain for good.** Seen 2026-10-29: the laptop's
  node, restarted while its old connection to the VPS stood, kept hearing the VPS while the VPS
  heard none of its blocks, and the chains split past the reorg limit. gossipsub tells a peer its
  topics on the first connection alone, so the restarted node never learned the VPS subscribes.
  Now a connection that comes the way a standing one came drops the peer for a fresh one, every
  peer's status is asked once a minute, and what reaches no peer is counted and warned of. See
  [Networking](node.md#networking).

- [x] **An onion service.** With `TOR=1` the Docker stack runs Tor beside the node, its API,
  explorer and wallet at an `.onion`, and with `TOR_MINING=1` its mining port; the apps reach it
  through their SOCKS proxy, the worker through `--proxy`. See [Docker](docker.md#an-onion-service).
- [x] **Peer-to-peer over Tor.** libp2p over TCP, with Noise and Yamux, beside QUIC: `/onion3`
  peers dialed through `--tor-proxy`, the node reachable at its onion (`--onion-file`), and
  `--tor-only`, every dial through Tor and no QUIC. The Docker stack does it with `TOR=1`, and
  `docker/pow.sh join ADDRESS.onion` joins through Tor alone. See [Node](node.md#over-tor).
- [x] **Sync.** A node asks each peer for its status when it connects, and again when it gossips a
  block that does not connect. While any peer's chain holds more work, it asks the heaviest one
  for the next 128 blocks and imports them in order, side branches and reorgs included. When a
  batch does not connect, the chains split earlier, so it steps a batch further back.
- [x] **Sync uses one peer at a time.** Done 2026-11-06: a long catch-up fetches batches from up to
  `--sync-peers` peers at once, imported in order.
- [x] **Blocks gossiped ahead of the tip are held.** Done 2026-11-04. Up to 32 (`ahead.*`), 4 a peer,
  16 MiB, 2 minutes, each checked as far as it can be without its parent and counted in its peer's
  backlog share; they go in as soon as the parent lands. See [Sync](node.md#sync).
- [x] **Gossip is checked before it is passed on.** gossipsub's `validate_messages`, each message
  reported back as the node judged it: accepted, ignored, or rejected as its sender's fault. See
  [Node](node.md#networking).
- [x] **Faults are scored and peers banned.** Invalid blocks, bad signatures, undecodable gossip
  and floods add up per peer; past `BAN_SCORE` within `SCORE_WINDOW` the peer is banned for
  `--ban-for`, by id, and with `--ban-ips` by IP. See [Node](node.md#bans).
- [x] **Sync requests and bad sync batches pay a price, and bans outlive a restart.** Done 2026-11-04.
  A block breaking the rules in a sync batch counts against its peer as gossip would; sync requests
  count in the backlog until answered, at `--sync-rate` past `--sync-burst`, struck past it. Bans are
  kept in the database's `bans` column until they end; `--unban-all` and the operator's
  `POST /node/unban` lift them. See [Bans](node.md#bans).
- [x] **Backlog shares are per IP too.** Done 2026-11-04. The peers of one public IP share
  `--max-gossip-per-ip`, four peers' shares; loopback (Tor) and private addresses keep one each.
- [x] **Blocks and sync requests are counted in the backlog.** Gossiped blocks were already; sync
  requests are since 2026-11-04, until answered.
- [x] **The strike rules are fixed.** `--max-strikes` and `--strike-window` set them now, 10 within
  a minute unless given.
- [x] **The node keeps its identity.** Its libp2p key is `net.key`, beside the database or where
  `--net-key` says, so its peer id is the same each start and `--peer` can pin `/p2p/<id>`.
- [x] **Bootnodes, and discovery past the peers given.** Genesis's `bootnodes`, replaced on a live
  chain by `Change::Bootnodes`, and `--bootnode`, are dialed while a node has fewer than
  `--target-peers`; Kademlia bootstraps every 2 minutes, and the node dials a few of the peers it
  knows, so two nodes that dial a third meet.
- [ ] **Windows Hello in the wallet.** Biometrics cover Android, the browser's passkeys and
  macOS; a Windows desktop keeps the password.
- [ ] **Touch ID guards a check, not the key.** A signed macOS build could tie the Keychain item to
  biometry with an access control, as Android's Keystore does.
- [x] **Peers are remembered.** The 64 seen last, with the addresses identify gave, in the
  database's `peers` column, dialed at the next start as bootnodes are.
- [x] **Chains keep apart by protocol name and genesis.** Every protocol and topic is
  `/{protocol}/{network}/…`, `--protocol` naming the first; identify tells a node a peer of
  another chain, which it disconnects and forgets.
- [x] **Peers given are dialed again.** A `--peer` with no connection is dialed every 15 seconds,
  so a node behind NAT finds a restarted server again, and `--no-mdns` keeps a server off its
  network's multicast. `docker/pow.sh` runs a chain across a VPS and a machine at home.
- [x] **No connection or request limits.** `--max-peers-in` caps connections dialed in, and
  `--http-rate` requests a second per client beside `--http-per-ip`'s concurrency.
- [ ] **No NAT traversal** (relay, hole punching, AutoNAT).

### Node, API and tooling

- [x] **Every limit the node fixed is a flag or a `--tune` key, and the apps' are settings.** Done
  2026-11-04, from section 3 of `docs/hardcoded-values.md`. Node-local and app-only: no new
  genesis, no consensus change, no contract redeployed; every default is the old value.
  - Flags: `--max-peers-in` (libp2p's connection limits on connections dialed in), `--ban-score`,
    `--ban-window`, `--max-strikes`, `--strike-window`, `--tor-timeout`, `--http-rate` (a bucket
    per client a second, 429 with `Retry-After`), `--api-max-page`, `--suggest-multiplier`,
    `--update-every`, `--snapshot-depth`, `--underpriced-for`, `--lua-states`, `--lua-builders`,
    `--wasm-cache-mb` and `--wasm-cache-modules`, each bounded. `--state-history` and
    `--underpriced-for` take a time too, turned into blocks at the block time.
  - `--tune KEY=VALUE` for the rest, in one table (`node::tuning::KNOBS`): `sync.*` (down only),
    `gossip.*`, `dial.*`, `peers.remembered`, `node.event-budget`, `mining.*`, `http.*`, `range.*`,
    `rpc.*`, `filter.scan`, `finality.*` and `chain.bad-blocks`.
  - `MAX_BODY` follows the governed `max_tx_bytes`, never under 2 MiB; `MAX_HISTORY` follows FFG's
    `withdrawal_delay + vote_depth`, at most 4,096 epochs; `PIVOT_DEPTH` turned out the syncing
    node's alone.
  - `GET /node` shows every knob under `limits` (a new `limits` report field, hidden by
    `minimal`), and `backlog` its strike rules. Docker passes each as an optional variable,
    the mempool's and `--sync-batch-bytes` among them, and `TUNE`.
  - The apps: Low data (one switch stretching every refresh timer four times), Wait for answers
    (the HTTP timeouts 1, 2, 4 or 10 times), the wallet's trade deadline, and the reading caps; the
    wallet's Activity pages its address and event indexes with Older transactions, and the Trade
    page says when a DEX has more than the 40 pools it lists.
  - Left: the bridges' and swaps' rows (another batch), the faucet's budget, `SHARE_EVERY`,
    `MOST_SHARES` and `KEPT_BLOCKS`; a per-IP limit on peers.

- [x] **The explorer shows voided and refunded transfers.** Done 2026-11-03. Apps only: no new genesis, no
  contract redeployed.
  - Its Bridge page reads `Voided` and `Refunded` as well as `Sent`, `Attested` and `Received`, in any
    order, and says each transfer's standing as the wallet does, from `chainv4_ui::bridge::Standing`:
    signing, signed, arrived, voiding, voided with its refund to come, refunded, or stuck. Tiles count the
    refunds and the stuck; a transfer's page says where it was voided or paid back and to whom.
  - Signatures to void or refund a transfer go with it, not shown as a transfer arriving: worked out from
    `sent(nonce)` for one that left, asked of `void_digest` for one voided coming here.
  - Stuck is one that left for a Gateway.sol the directory no longer names there: its peer is gone, as in
    the item on a refund across a replaced gateway, which this only shows.
  - The transaction and account pages say the gateway's events in words, each linking to its transfer.
  - Left: a refund of one voided here is paid on the EVM chain, so the explorer keeps it at "voided, refund
    to come"; neither event says why it was refused. Its refund signatures are placed by the calls that
    posted them since 2026-11-04, below.
- [x] **A child's fee comes off the backing, and later releases repay it.** Done 2026-11-04. The gateway
  changed, with the fee bumping below, in the same regenesis. Each release proposed while `shortfall()` owes
  anything repays a budget of the least of what is left past what open releases repay and `btc.repay_pct` of
  its fee (20 percent), an even share from each withdrawal, at most `btc.repay_most` (500 satoshis), within
  what its most fee leaves: one at its limit repays nothing and is not held up. It goes to the federation's
  change, and the shortfall falls by it when a version of the release is shown; a release proposed counts as
  repaying until shown or cancelled, so none repays the same twice, and a replacement repays what its release
  does. The planner and the gateway work it out alike (`btc_repays`), and a signer refuses one past its bounds.
  `bridge btc withdraw` and the wallet's withdraw form say a little may come off toward it; the explorer shows
  what is left to repay.

- [x] **Fee bumping a stuck Bitcoin release, before real funds, and the signers taking turns.** Done
  2026-11-03. Needs a new genesis with the Bitcoin gateway redeployed: the contract changed.
  - Every release signals BIP125 (sequence `0xfffffffd`) and keeps change of at least 330 satoshis,
    Bitcoin's dust for P2WSH, the withdrawals topping it up when the gateway holds no more than they burned.
  - `replace(of, raw, signature)` proposes a replacement: the same inputs, scripts, withdrawals and change,
    each withdrawal less its even share of what it now loses within its limit, past every open version by a
    satoshi a virtual byte, under the cap. `child(parent, raw, signature)` proposes a child spending the
    version's change, held outputs only when needed, back to the federation, its fee charged to the
    withdrawals within their limits and recorded as `shortfall()`. One bump at a time; `released` takes
    whichever version was mined and closes the rest as `replaced`; a bump short of its signatures is
    cancelled alone; `tree(id)` and `proposal(id)` say each one's kind.
  - Signers' passes bump by policy, `btc.bump_after_ms` (an hour) and `btc.bump_below_pct` (80), governable,
    replace-by-fee first and a child once Esplora refuses the replacement or it goes unmined past the delay
    again; `bridge btc bump <release> [--rate] [--child]` by hand. The others sign only within their limits and
    while their Esplora says it is stuck (`check_btc_replacement`, `check_btc_child`, `ReleaseRefusal::Needless`).
  - The race on the devnet, all three signers sending `released` for one release in one block: what any one
    signer's call does for all waits its turn, its place times a block, at least a second, then checks the
    gateway and the node's mempool again (`btc_turn_delay`).
  - `set_most_fee` raises a limit while its release is open, for a bump. The explorer lists each bump and
    what children were paid from the federation's change.
  - Left: a release evicted from mempools is only replaced, and the wallet shows withdrawals, not bumps
    (below). The shortfall a child leaves is repaid by later releases (above).

- [x] **Four CLI bugs from the testnet run.** Done 2026-11-03. CLI only: no new genesis, no
  consensus change, no contract redeployed.
  - `dex add` takes at most the amounts given, as Uniswap V2's router does: it reads the reserves,
    works out the largest pair at the pool's ratio within them (`dex::add_amounts`, rounded as
    `dex.lua` and the router round), says it, and sends the amounts as the most and that pair less
    `--slippage` as the least. A new pool opens at the amounts as given. Off-ratio amounts were
    refused before, as the least was the amounts given less the slippage.
  - `dex swap`, `add` and `--evm remove` approve only when the allowance is short.
  - `bridge btc address` writes for the Bitcoin the relay follows (`network_name()`), else what
    Esplora's genesis block says, else `--network`, else main, saying which on stderr; `bcrt1…` on
    regtest; a contradicting `--network` is refused. `relay-start` takes its network from Esplora's
    genesis unless told, and refuses one that disagrees.
  - `dex --dex` and `--router` say where the DEX is found without them (the directory, then the
    node's `--dex-contract` / `--dex-router`, not a name), and the other `dex` arguments say what
    they take.
  - A multisig bundle's review lists a deploy by engine, size, SHA-256 and the address it lands
    at, not its source; `multisig sign --full` and `approve --full` show the code.
- [x] **Signers deliver to an EVM chain in turns and keep within a public node's limits.** Done 2026-11-04,
  found running the bridge to Sepolia. CLI only: no new genesis.
  - All three signers delivered transfer 1 within a block, and two of the three were mined failing, at
    their gas. Under `watch` they now take turns: the first in line for a nonce goes round the federation
    nonce by nonce, and each after it waits `--relay-turn` more, 90 seconds unless told, from when it
    found the transfer ready. `bridge relay` by hand goes at once.
  - A pass read the gateway for every transfer ever sent, every pass, past the public node's `/call`
    limit (429). A signer now remembers what it signed and what was delivered or voided, by gateway, and
    a read turned away with a 429 is asked again after 1, 2 and 4 seconds.
- [x] **What the CLI fixes left for the apps.** Done 2026-11-04. Apps and CLI only: no new genesis.
  - `chainv4::bridge::BtcNetwork` (main, test, regtest) writes and reads addresses for the Bitcoin the
    relay follows, for the CLI, the wallet and the explorer alike. The wallet's and the explorer's
    Bridge pages write `bcrt1q…` on regtest, where they wrote `tb1q…`.
  - `bridge btc withdraw`, and the wallet's withdraw form, refuse an address on another network than
    the relay's before anything burns, naming both; `withdraw --network` as `address` takes it.
  - The explorer places the signatures to refund a transfer voided coming here, signed over
    Gateway.sol's `leftBy`, by the `attest_refund` call that posted each, which names the transfer, and
    lists one refused before it ever showed here from its `attest_void` calls.
  - The wallet's Propose form sets fees' `vote_rewards`: as genesis set, minted, or the reward pool.

- [x] **Governance votes Yes, No or Abstain, rejects what can no longer pass, changes itself, and
  governs what the audit picked.** Done 2026-11-03, from section 1 of `docs/hardcoded-values.md`.
  A consensus change: **a new genesis.**
  - Votes: `Tx::VoteChange { version, change, vote }` and `Tx::ClearVote { version, change }`
    replace approve and withdraw; one vote an account on each change, one Yes a kind; a change is
    rejected once what has not voted No cannot reach what is needed, with a provable
    `Rejected { rejection, kind, version }` receipt, indexed and listed at
    `GET /governance/rejections`; a governable quorum (`quorum_permille`, Abstain counting); both
    electorates and the council alike.
  - `Change::Governance` (members, threshold, approval period, quorum; clears every open vote),
    `Change::TokenFunctions`, `Change::Names` (prices, grace, years), `Change::EvmSpec` from a
    height, `Change::Dex`; and inside existing kinds: `fees.vote_rewards`, `fees.costs` (host
    fuel and the precompiles' gas, read from each block's rules), `limits.contract`,
    `max_tx_bytes`, `multisig_members`, `finality.slash_permille`, the PoW window and steps, and
    the federation's `btc` policy the Bitcoin gateway and its signers read. Drift follows the
    block time.
  - Both DEXes charge the one governed fee: `chain.dex_fee()` in Lua, `dexFeeBps()` through the
    native precompile in `Dex.sol`.
  - The explorer and the wallet: proposal cards with a ring, quorum, a dot per voter, time left
    and what moves; Yes, No, Abstain and Clear in the wallet; a history timeline and turnout.
  - Left: WebAssembly's recursion and stack stay fixed (engine settings shared by the module
    cache); turnout is read from each voter's newest page of votes, not from a node index.

- [x] **One key can no longer fill the mempool, sync takes big blocks, and the directory wins
  over the operator's flags.** Done 2026-11-03, from the audit in `docs/hardcoded-values.md`
  (#2, #15 and section 2's first). Node only: a redeploy, no new genesis, no consensus change.
  - The mempool (`MempoolLimits`): `--mempool-size` (50,000), `--mempool-per-sender` (64),
    `--max-nonce-gap` (64, from the account's nonce at the tip, checked before any verify) and
    `--replace-bump-percent` (10: both the max price and the tip, and the max price at least 1).
    A replacement keeps its place in its sender's run. At its share, a sender's lower nonce takes
    the place of its highest. Full, a newcomer that can run pushes out the lowest of what waits
    behind a gap, then the least paid, a sender's highest nonce first, never a sender's next, and
    with the rest of the victim's run; it is refused if it pays no more or waits behind a gap
    itself. What waits behind a gap for `MAX_GAPPED_BLOCKS` (64) is pruned. Each refusal has its
    own error: 400, 409, 429 and 503 at `POST /txs`, Ethereum's words at
    `eth_sendRawTransaction`; gossip refused for them strikes no one. `chainv4 bench load` funds
    its senders 32 at a time to stay under the share.
  - Sync: `SyncRequest::Blocks` is answered with up to `--sync-batch-bytes` of blocks (8 MiB, 1 to
    32 MiB), past the first block, which always goes; the asker asks again from where the answer
    ended, as it did for a peer that ran out. `MAX_SYNC_RESPONSE` is public, and a compile-time
    check holds the cap and `MAX_BLOCK_BYTES` well inside it.
  - `GET /status`: `swap` and `dex` are the directory's, a flag filling only what it leaves
    unset, and `operator` shows the flags as given. The wallet, explorer and CLI already took the
    directory first, so the apps are unchanged; Docker's `SWAP_CONTRACT`, `DEX_CONTRACT` and
    `DEX_ROUTER` still pass the flags.
- [x] **The mempool, still open.** Done 2026-11-06: reserves summed per sender, `--mempool-bytes`,
  a sender's next pushed out when it pays least, `left` on `/stream`, and `--gapped-for`.
  - The fee reserve is checked per transaction, not summed over a sender's waiting ones: a key
    with enough for one can hold its share of 64, each needing all of its balance. Summing them,
    as the audit suggests, would refuse a 65th's worth sooner.
  - It is bounded by count, not bytes: 50,000 transactions of up to a block each could take far
    more memory than 50,000 transfers. A byte cap, or a per-transaction byte limit
    (`Change::Limits`'s `max_tx_bytes`), would bound it.
  - A sender's next transaction is never pushed out, so 50,000 senders with one each, offering
    under the base price, hold the mempool for `MAX_UNDERPRICED_BLOCKS` blocks.
  - Pushed-out and pruned transactions are not told to anyone: `GET /stream` shows what joined,
    not what left.
  - `MAX_GAPPED_BLOCKS` counts blocks, not time; `--underpriced-for` takes a time now.

- [x] **Multisigs in the wallet: every one you are in, anything to propose, any key to sign with.**
  Done 2026-11-03, from the user's reports and a testnet run. In the node: a member index,
  `--index-multisigs` (`Column::Members`, `src/chain/members.rs`), each multisig under the address
  of each member's key, built from the registry when turned on, on a chain already running too, and
  moved by every block that writes a policy, a removed member's going, and back on a rollback;
  `GET /accounts/{address}/multisigs`, by address or key, a page at a time. Proposals are priced
  for their heaviest quorum under the heaviest carrier (`MultisigBundle::probe`), in the CLI and the
  wallet, and sent with no more than threshold signatures (`trimmed`): a secp256k1 member signing
  an Ed25519 member's proposal was refused under its intrinsic cost. `send --multisig` prints its
  deploys' addresses, and `--member KEY,XPUB` makes one on Bitcoin alone. In the wallet: the
  Multisig page lists every multisig any account is in, merged with those followed, each with its
  address whole, Copy and its QR; proposes anything the Batch page makes, through its own step form,
  one step as it is; picks which of the wallet's member keys signs and approves, or an offline
  signer, starting at one that has not, and moving on after each; names who signed; takes members of
  every scheme, with its own keys one pick away and its own addresses turned into keys; reaches EVM
  chains, Bitcoin, or both, each row saying what it needs, filled from the wallet's own words in a
  tap. Pair shows a member line for any of the account's keys. The explorer's account page lists
  the multisigs an address is in. Listings the apps read a page at a time (the token and collection
  registries, an account's names, its multisigs) now follow each page's cursor to the end, where
  they stopped at 100 or 500.
- [x] **A multisig's listing is the node's word.** The member index and the policy it points to are
  not proven on the Multisig page; proving the policy (`Multisigs::policy_key`) as a balance is
  proven would make the page as sure as the rest. Done 2026-11-06 for the policy: the wallet proves
  the shown multisig's policy and compares it with the listing; the member index stays the node's word.
- [ ] **Per-account Bitcoin multisig xpubs.** Every account of one phrase names the same BIP48
  xpub, at account 0', so two of them cannot be members of one multisig on Bitcoin. Deriving at
  m/48'/coin'/n'/2' for account n would let them, once the offline signer signs there too.
  Found 2026-11-06: `BitcoinXpub` in a policy keeps no account, and descriptors and PSBT paths
  write `0'`, so naming account n's xpub needs an `account` field in it: a change to chain state,
  for the consensus batches. The library and the signers take any account already
  (`Seed::multisig_account(coin, account)`, the signer reading it from the path).
- [x] **Wallet pages send from any of the account's addresses, and read NFTs' transfers as NFTs.** Done
  2026-11-03. The review read every `transfer(to, n)` with no coin as a token's, so sending an NFT
  showed its id as "n units of the token". Steps now read a call by what the light client proved its
  contract is: the wallet proves the collection registry's entries as it proves tokens', and shows a
  collection's `transfer`/`safe_transfer` as the NFT, or the amount of an id, with its collection;
  one to a contract proven as neither says so and names no amount. Send, Bridge, Trade, Swaps and
  Batch share one From picker (`views/from.rs`) that holds across pages, where Bridge, Trade and
  Swaps took the account's first address alone, and the light client watches every address's
  Bitcoin deposit address. Activity says which address each line is for, and a transfer between the
  account's own addresses shows as **Between your addresses** with its amount: before, it showed a
  bare Transfer and only the fee. The NFTs page shows an NFT at whichever of the account's addresses
  its proof names, compared as an address, keeps it there before that address's own list says so,
  names an address whose list did not come, and takes no proof answered late from an older block,
  so one sent between the account's addresses no longer drops off the page. The node and both
  example collections write and prove every scheme's holders alike, a bitcoin `bc1q…` too (a test in
  each engine now says so). No node or contract change.
- [x] **The offline signer names the collection of an NFT it signs for.** It knows only the token the
  sender names in its request, so an NFT's transfer shows there as a call to a contract it does not
  know, honestly but bare. The request could carry the collection, the sender's word as its token
  is, which changes the airgap format; and the NFTs page could prove `tokens_of`'s ids rather than
  ask a node for them, which a node may leave short. Done 2026-11-06 for the
  name: the request carries the collection after its encoding, only when there is one. Left: the
  NFTs page proving `tokens_of`'s ids rather than ask a node for them.

- [x] **Stuck Bitcoin releases are voted down by the signers' own passes.** Done 2026-11-01. One signer could
  propose a release the others refuse, and only a threshold of votes cancels one, with nothing voting. In
  `bridge btc pass`, `watch` and `bridge watch --esplora`, a signer now votes at once on what it refuses for
  good (`check_btc_release` says `Wrong`, `Dead` or `Overpriced`, `btc_release_step` what to do), never on a
  read that failed or a limit with no fee rate behind it; votes on anything short of its signatures past
  `--cancel-after`, 6 hours by block time from `Proposed` events, even if it signed; and warns each pass of
  one with all its signatures unmined past that, with its rate against the network's, needing
  child-pays-for-parent. No flapping: a signer does not propose past its own limit, nor a withdrawal again
  near the rate a young cancelled release paid. No contract change.
- [x] **A transfer refused for good can be refunded.** Done 2026-11-01. It left its funds locked at the source
  for good. `chainv4 bridge refund`, each signer's operator's call, with a preview and a typed yes, reads both
  chains and signs in order: void at the destination, whose nonce is then taken however public its delivery
  signatures, then, once that is final, refund at the source, to who sent it, of what it recorded, once. Both
  directions, the coin, tokens, NFTs and multi-tokens, held or burned. `refund-to` lets a sender that cannot
  take it name another. New domains, `chainv4-gateway-void` and `chainv4-gateway-refund`, kept alike in Rust,
  Lua and Solidity. Needs a new gateway here and a new Gateway.sol; the Bitcoin gateway has no such path and
  needs none.
- [ ] **A release dropped from Bitcoin's mempools can only be replaced.** One paying under their minimum fee
  is evicted, and a child cannot bring it back through Esplora, which takes one transaction at a time; Bitcoin
  Core's `submitpackage` could take the two together.
- [ ] **The wallet shows a withdrawal's release, not its bumps.** It reads `withdrawal(id)`, whose fee says
  the newest version's share and any child's; the explorer lists each bump, with what it replaced or pays for.
- [ ] **A refund across a replaced gateway.** What left for a gateway since replaced can be neither delivered
  nor voided, so it stays where it left unless its peer is put back. A chain's reset leaves that for good.
- [x] **Only federation signers propose a Bitcoin release, and they do it by themselves.** Done 2026-11-01. Anyone could
  call `propose`, so a griefer could push a release's fee up to each withdrawer's limit, burning it to miners, or tie
  withdrawals up in releases no one signs. It now takes a signature of the federation in force over
  `btc_propose_digest` (chain, gateway, the number it will have, its transaction, its withdrawals), good once, so what
  proposed a release does not bring it back after a cancel. Nothing proposed by itself before: `bridge btc pass`,
  `bridge btc watch` and `bridge watch --esplora` now propose what is queued at Esplora's rate (`plan_btc_release`,
  worked out alike by every signer, and what cannot go says why without holding up the rest), sign, broadcast and show.
  Signers refuse what pays past `--max-fee-rate`, by default 3 times the fastest rate and at least 25 sat/vB, or a fixed
  200 with Esplora out of reach. Needs a new gateway: new deposit addresses.
- [x] **An open mining port goes over wss://.** Done 2026-11-01. Its token is public, so anyone on the path could rewrite a
  payout both ways, unseen. The native worker spoke plain TCP alone: it now speaks the node's `/mining/ws` over rustls
  (`mining::wss`), the certificate always checked, found from `GET /mining` with `--node https://…`, and `Worker::start`
  refuses plain TCP to an open port unless `--insecure-open-port`, or it cannot be read on the way: this machine, or an
  onion through a proxy here. The wallet's native miner follows the same rule, and the web wallet refuses `ws://` to an
  open port elsewhere, which it used to take. nginx's `/mining/ws` lost `X-Forwarded-For` to its own `proxy_set_header`,
  so the node counted every worker as nginx, 16 at most: it says it now, and both allow 32 an address.
- [x] **One Bitcoin withdrawal could be paid up to 50 times.** `propose` checked each withdrawal was queued but marked
  them only after the loop, so a release naming one id 50 times paid one burn 50 times. A repeated id is refused, and
  `bridge btc sign` goes over each release against the gateway's state before signing, rather than signing every open
  one.
- [x] **An invalid release could freeze BTC for good.** Duplicate inputs, a BIP68 delay, a 1-satoshi fee, change under
  dust, an output no node relays or a count written longer than it needs all made a release that holds its inputs and
  never confirms. All are refused, with a fee floor per virtual byte beside the cap. The federation in force can
  cancel a release with an input under its threshold; each withdrawal it paid stays anchored to one of its inputs, so
  it cannot be paid twice even if the cancelled one is finished with signatures that came too late.
- [x] **Bitcoin deposits replayed across chain resets.** A deposit script named only the recipient and federation, so
  the same proof minted again on a new chain. It commits to the chain's id and the gateway now, in Lua, Rust and the
  wallet, and a gateway refuses what was mined before it was set up. Contracts read the id with `chain.id()` and
  `chain_id`, a host function in both engines.
- [x] **Signers would re-mint old transfers on a chain restarted from an identical genesis.** The gateway here took its
  chain id from its owner, and the old chain's signatures are public. It reads the real id now, and is named in every
  transfer by its address and the time it was set up, so neither old signatures nor Gateway.sol's old logs mean
  anything to one set up anew.
- [x] **1 wei halted bridge signing.** A transfer refused at the threshold made that signer's pass stop there, every
  time, and `watch` then skipped relaying; a name that was not UTF-8 did the same, either way. Both gateways refuse a
  recipient that cannot arrive and drop a name that is not text, and each transfer now stands alone: one refused is
  logged with why, tried less and less often, and stays listed, and old logs after a reset pass quietly.
- [x] **A chain's own signature on a transaction is bound to its chain.** `signing_bytes` covered the transaction,
  nonce, fee offer and signer, while votes, EIP-155 and EIP-712 named the genesis. So a transaction signed on one
  chain was valid on another where its account had that nonce: with the keys reused, as across the devnet's
  genesises, anyone could send it again there. It covers the genesis id now, `SignedTx::verify_on` is the only check
  of a signature, the offline signer's request names its chain, and the apps sign nothing with none known. It
  needs every node and app at once, and a new genesis.
- [x] **One gateway-owner key could drain coins through a fake peer.** The owner sets the gateway's peers, and a signer
  refused only a peer the directory contradicted, so on a chain it left out, or with no directory at all, a signer signed
  whatever the owner's peer said left. Signers now refuse any chain the directory does not list, unless their own operator
  names that very gateway with `--evm-gateway CHAIN:0x…`; nothing on the chain can.
- [x] **The relay's proofs had the inner-node weakness, CVE-2017-12842.** `includes` took a path of any length, so the 64
  bytes of two ids could pass for a transaction a level up. A proof now brings the block's coinbase, itself and its path,
  which must be just as long, and the gateway refuses any 64-byte transaction. The CLI and the wallet, both its Esplora
  and light client proofs, bring it.
- [x] **A mining hello's `payout` and `solo` were unauthenticated.** On the way, one could be rewritten. The worker's and
  the node's proofs and the session key now cover every field, the node says what payout it took, and the worker stops if
  it is not its own; the native miner, the wallet's and the web one all go through it. An open port's token is public, so
  there only wss:// or a private network protects a payout from someone on the path, and the docs say so.
- [x] **`multisig bitcoin sign` signed a PSBT blind.** It showed nothing before using the seed. It now shows each output's
  address and amount and the fee, and takes a typed yes or `--yes`; what it cannot read is refused. `multisig sign`,
  `approve` and `multisig evm sign` show what they sign and ask the same.
- [x] **Bitcoin withdrawal fees came from the pool.** Nobody was charged, so a 1,000-satoshi withdrawal could spend the
  whole cap of what backs wBTC. A release's fee now comes off its withdrawals, evenly, each up to the most it agreed to
  lose, a tenth unless it says, and what the gateway holds falls by just what was burned. Withdrawals start at 10,000
  satoshis. Sweeps stay the federation's.
- [x] **Release manifests had no expiry, and only the node ordered them by `issued`.** A manifest now carries a signed
  `expires`, 30 days unless told, renewed with `chainv4 release renew`, and nothing acts on one past it. Every build has
  an `issued` floor, and the wallet, explorer, worker and CLI keep the newest they saw as the node does, so a host can
  hide a newer one for as long as the last one counts, and no longer.
- [x] **A chain's own signature on a transaction is not bound to its chain.** `signing_bytes` covers the transaction,
  nonce, fee offer and signer, while votes, EIP-155 and EIP-712 name the genesis. So a transaction signed on one
  chain is valid on another where its account has that nonce: with the keys reused, as across the devnet's
  genesises, anyone can send it again there. Binding it changes what every signer signs, so it needs every node
  and app at once, and a new genesis. Stale: done on the fourth
  genesis, as `signing_bytes` takes the genesis id.
- [x] **After a new chain, Activity showed the old one's transactions.** A row kept from the old chain stood in for
  a transaction with the same id on the new one, and being final was never proven again. With no chain followed,
  it listed every chain's. Each is kept with its genesis and goes by it now, the pinned one with none followed.
- [x] **In the Docker stack, every visitor would have counted as nginx.** `--trusted-proxy` took only an address, which
  a container's can change, and the stack passed none. It takes a host's name now, looked up again every few
  seconds, and the stack names `nginx`.
- [x] **The whole of CI on this machine.** `scripts/ci.sh` runs what `test.yml` does with no runner minutes and no
  GPU kernel. Its first run found node builds without `client` that no longer compiled, and tests assuming the
  default hash, scheme or finality.
- [x] **Reading from checked blocks: the explorer walks every header on each launch.** Done
  2026-11-06: the client is saved with the settings and carried on from. It saves no light client,
  so with the setting on each start goes from the beginning. Fine today, slow as the chain grows, and to fix
  before this gets real use. Save the client as the wallet does.
- [x] **Reading from checked blocks: the explorer holds its whole index in memory.** Measured
  2026-11-06: about 3.2 times its encoded bytes (docs/explorer.md). How much that takes has not
  been measured, and 128 MB encoded could be noticeably more.
- [x] **Reading from checked blocks: the explorer takes a name's ending to be its last label.** `alice.dev` is looked
  up as `alice`, as only a node says what the suffix is. A chain with a differently shaped suffix could wrongly
  be told nobody holds a name. Done
  2026-11-06: `/status` gives `names_suffix`, which alone comes off, and the page says which record
  it proved.
- [x] **Reading from checked blocks: the explorer's update download goes around its request gate.** It goes straight
  to the release host, only on the desktop and only when pressed. Done
  2026-11-06: it goes through the API's own client (`Api::download_client`).
- [x] **A setting for the wallet and the explorer to take everything from the blocks they fetched,** history
  among it, rather than look things up from the node: what they show then rests on blocks they checked, and
  the node learns less of what they look at. Off by default, as reading blocks costs far more than asking.
  What cannot be proven is turned off and says why. See [the wallet's](wallet.md#read-from-checked-blocks) and
  [the explorer's](explorer.md#read-from-checked-blocks) guides.
- [x] **Blocks a client can check whole, without asking for what it wants.** `GET /blocks/range` sends
  blocks in binary with what recomputes every root, their receipts, and a proof against `state_root`
  of each state entry they changed, deletions too, within bounds on blocks, bytes, proofs and time,
  saying where to go on from, even inside a block. `check_range_item`, in the light build and wasm,
  checks each. It reads the list history keeps, so nodes write nothing new and serve what they already
  ran. No list or pruned state is said per block. A list that leaves an entry out cannot be told.
- [x] **A wallet could run into the node's rate limits by itself.** Hundreds of proofs went out at once and
  every 4 seconds. Now 8 go at a time, none is asked again at a block it holds, rounds are spaced by their
  size, and a brief 429 waits rather than shows.
- [x] **The browser extension could not go to a store, as it held a miner.** `wallet/build-extension.sh --store`
  leaves it out, and fails if any reaches the output.
- [x] **A kill during a save could lose every wallet on a device.** eframe empties its file and then
  writes it. Off the web, each wallet and the settings now have a file and a copy of their own, each
  flushed, read back and renamed into place; what will not read is set aside, not written over.
- [x] **One unreadable account kept a wallet shut.** It is listed by name now, kept as it came in every
  save, never used to sign, and removed only by a typed yes.
- [x] **Showing private keys asked nothing of a wallet with a password.** It asks for what opens the wallet.
- [x] **Real unlock modes for the wallet's vault.** Password only, biometric only (a random key only
  hardware or a passkey keeps), or both required (HKDF of the password's stretch and a hardware
  secret), chosen per wallet in Settings on Android, iOS and the browser. Lockout means restoring
  from words: a hint and a typed yes first, refusal for what words cannot bring back, and the lock
  screen says so, writes nothing, and can remove the wallet. Older builds fail closed and keep it
  whole. Today's "either opens it" stays for wallets that have it, and is offered no more.
- [x] **`Tx::Batch`: several steps under one signature, nonce and fee, all or nothing.** Transfers,
  contract and EVM calls, deploys, upgrades, names, burns and claims, 1 to 32, sharing one fuel limit, the
  receipt saying each step's result or which failed. Multisigs may send one. The CLI's `chainv4 send batch`,
  the wallet's Batch page, every signing screen and the explorer list each step. It needs every node at once.
- [x] **Bytes nesting a transaction without end crashed whoever decoded them.** A multisig holding a
  multisig holding a multisig, a few megabytes of it, ran the decoder out of stack and aborted the node.
  Decoding stops two levels down now, the deepest a valid transaction goes.
- [x] **An offline signer forgot what it had signed.** It keeps a history now, sealed with its keys, that Settings turns off and clears.
- [x] **Pentest the chain with malicious contracts and calls,** in Lua, Wasm and the EVM: resource exhaustion
  (memory, fuel, stack, nesting, huge returns and logs), reentry across engines, escaping the sandbox, state left
  behind by reverts, and hostile calldata. On a chain of its own, never the public one. First run 2026-11-04; a
  second is queued for after this list is done (`docs/queue.md`). Its tests are `src/chain/tests/pentest_lua.rs`,
  `pentest_wasm.rs`, `pentest_contracts.rs`, `src/pentest_decoders.rs`, and two in `evm.rs`; an ignored test is a
  finding, failing until fixed. What it found, worst first:
  - [x] **A pattern that backtracks stalls every node.** Fixed 2026-11-04 in the Luau fork (325972b), with the
    two below; needs the new genesis, as old blocks would replay with other fuel. `string.find(s, ".-.-.-.-b")` on 160 bytes ran 24 s for 77
    fuel, about 16 times longer for each doubling: the C matcher charges nothing for its steps. Consensus: charge fuel
    per match step in the Luau fork, deterministic everywhere (`a_pattern_that_backtracks_is_paid_for_by_its_steps`).
  - [x] **Library calls that do work by size cost a flat few fuel.** `table.sort` of 200,000 numbers 50 times took
    1.9 s for 700 fuel; a plain `string.find` over 4 MB, `table.insert` at the front of a big table and their kin
    alike, and no memory limit stops a loop of them. Consensus: charge each by the bytes or elements it goes over
    (`sorting_and_scanning_are_paid_for_by_their_size`).
  - [x] **`^` computes with the platform's `pow()`.** Whole exponents by repeated squaring now; fractional refused. `2 ^ 0.5` runs and can be stored; Linux on x86-64 and macOS on
    arm64 need not round it alike, splitting the chain. Consensus: refuse a fractional exponent, or compute powers by
    a deterministic routine (`a_power_with_a_fractional_exponent_is_refused`).
  - [x] **A Wasm call that misses the module cache recompiles it unpaid.** Fixed 2026-11-04: every call pays
    `load_byte` a byte of its module, cached or not (consensus, with the new genesis). wasmi cannot save
    compiled modules, and its lazy translation charges fuel as it translates, which would make fuel depend on
    the cache. Deploying more large modules than the
    cache holds and calling them in turn makes every node compile on every call. Node-local: keep compiled modules on
    disk by code hash, or keep more (`calls_recompile_uncached_modules_as_unpaid_work`).
  - [x] **The DEX leans on the engine's reentry rule alone.** Fixed 2026-11-04 in `dex.lua`: reserves are
    written before paying out, and a native token's arrival is checked by its balance; deploy the new DEX. `remove_liquidity` writes the reserves after paying out,
    and adding a native token does not check what arrived, so a token that takes a fee or lies leaves its pool owing
    more than it holds. Not exploitable while no DEX function is reentrant; a `dex.lua` change and redeploy.
  - [x] **The EVM's trace copied each call's whole input, and kept every native call's hops.** A contract passing a
    megabyte to 280,000 cheap calls made every node copy about 280 GB; thousands of native calls each kept a trace.
    Fixed 2026-11-04: the selector is read in place, and frames and hops past what a trace holds are only counted.

  Found sound: Wasm deploy checks, memory, recursion and fill; every decoder (transactions, blocks, gossip, sync,
  ranges, the ABI, API bodies); the DEX, NFTs and multi-tokens under reentry; the gateways' and swap's ordering, by
  reading; the native precompile's gas, static calls and value; in Lua, building strings stopping at the memory limit,
  hashing paid by its bytes, every call starting fresh, the libraries unchangeable, and nesting without end failing
  cleanly. Not driven through a running federation yet: the gateways, `btc_gateway.lua` and the swap's claim and refund.
- [x] **Pentest the gateways and the swap contracts with hostile parties.** Done 2026-11-04 in
  local tests, on a federation of test signers, a relay fed regtest headers, and EVM contracts in
  process. Its tests are the last ones in `src/vm/tests/gateway.rs`, plus
  `src/chain/tests/pentest_gateway.rs` (Gateway.sol on this chain's EVM too),
  `pentest_btc_gateway.rs` and `pentest_swap.rs`. Each finding is an
  ignored test that fails until it is fixed. None is consensus: one is a client release, the rest
  a contract redeploy. No finding makes nodes disagree, stalls one, or does work it is not paid for.
  What it found, worst first:
  - [x] **Swap terms with a deadline near `u64::MAX` wrap the gap check** (`src/swap/mod.rs`,
    `Terms::check`). A starter's code passes in a release build, the other side locks on an EVM leg
    with a deadline that never comes, and the starter takes back their own lock and claims the
    other. Debug builds panic instead. Fix: `checked_add`, a first deadline in the future, and every
    leg's deadline capped. A client release (`terms_refuse_a_second_deadline_so_late_…`).
  - [x] **A claim whose payout is refused still shows the secret,** in `swap.lua` and `Swap.sol`. The
    counterparty, controlling the token (an upgradeable, pausable or blacklisting one, or a refusing
    ERC-1155 hook), makes the claim fail. They read the secret from the failed transaction, claim
    the other leg, and refund their own after the deadline. Fix: record the claim and the secret
    even when paying fails, and let anyone retry the payout by pull. A redeploy, which moves
    Swap.sol's CREATE2 address.
  - [x] **Swap hashes can be squatted,** in both contracts. Swaps are keyed by hash alone, so whoever
    saw the hash locks one unit under it first, and the real lock is refused for good. Fix: key by
    the hash and the locker, or by all the terms. A redeploy and a client change.
  - [x] **Dust ahead of the gateway's outputs stalls every Bitcoin release.** Deposits of 330 sats
    each mint, with no minimum. The planner takes outputs in the order they came, so a release drags
    in hundreds of them, past the fee cap: 300 of them made a 100,000-sat withdrawal 37,561 vB. Fix:
    pick inputs largest first and skip those not worth spending (planner only), and perhaps a
    minimum deposit (redeploy).
  - [x] **`gateway.lua` records what was asked for, not what arrived:** a token that takes a fee
    leaves the wrapped supply backed by less (300 recorded, 299 held), and a token or collection
    that moves nothing at all mints an unbacked wrapped one on the peer chain. Gateway.sol checks its
    balance and the NFT's owner. `swap.lua`'s `lock_token` has the same flaw, and makes its holders insolvent. Fix:
    compare balances before and after, as the Solidity contracts do. A redeploy. Fixed in
    `gateway.lua` and `swap.lua` 2026-11-04, the NFT's owner checked too.
  - [x] **`gateway.lua` keys what arrived by source chain and nonce alone:** after `set_peer` names a
    new Gateway.sol, which counts from 1 again, its transfers at nonces the old one used are never
    delivered, nor can they be voided. Gateway.sol keys by the gateway too. A redeploy.
  - [x] **`gateway.lua` works a transfer's ends out from the peers now:** one in flight when the
    owner changes a peer can no longer be signed or refunded. Fix: sign and refund by the ends it
    recorded as it left. A redeploy.
  - [x] **A federation the Bitcoin gateway never saw in force strands what was paid to it:** it learns
    a federation only when called while that one is in force, and the apps write deposit addresses
    from the node's status. Fix: a host call for a past federation, `federation.at(version)`, over a
    history the node keeps. That is consensus. Or the node writes each decided federation into the
    gateway.
  - [x] **A sweep's fee goes uncounted:** it comes out of what backs wBTC but is not added to
    `shortfall`, as a child's is: 99,000 held against 100,000 wBTC after one sweep. A redeploy.
  - [x] **A coinbase paying a deposit address mints before Bitcoin lets it be spent** (100 blocks), so
    a release spending it is stuck for hours and cannot be cancelled once signed. Fix: refuse
    coinbases, or wait 100 confirmations for one. A redeploy.
  - [x] **Lowering a withdrawal's most-fee undoes a release planned against it,** delaying the up to
    49 others with it, for two transactions a round. Fix: a lowered limit takes effect some blocks
    later. A redeploy.
  - [x] **Coin sent with `swap.lua`'s claim or refund is kept for good.** Fix: refuse it. A redeploy.
  - [x] **A Bitcoin HTLC claim never expires,** as Bitcoin's script cannot say "before", so Bitcoin as
    the second leg must be refunded the moment its deadline passes. `docs/swaps.md` says otherwise.
    Fix: the docs, and the apps refunding at once or warning. A passing test pins the property.

  - [ ] **A payout that burns all the fuel still fails a claim whole on chainv4.** `contracts.call`
    hands the callee all the fuel left, so a hostile token that spends it leaves `swap.lua`'s `pcall`
    nothing to record the claim with. Fix: a call with a fuel cap in the Lua host (and Wasm's), so
    a caller keeps enough to finish. That is likely consensus, so it goes on a new genesis.

  Found sound: a deposit on a branch that lost or shown twice, inner Merkle nodes posing as
  transactions, signatures for another input, release, digest, gateway or federation, high-s twins
  and duplicates, votes and showing raced, an output or withdrawal in two releases, the backing
  matching wBTC through releases, children, repayments and cancels, the swaps' deadline boundary,
  preimages, bad locks, hostile tokens on Swap.sol, re-entry into every claim, refund and payout, NFTs
  backing two locks, the HTLC's scripts and parsers under mutation, and Gateway.sol's ordering and
  signature counting, a re-entrant coin recipient on Gateway.sol paid once, and changing the coin
  after a send.
- [x] **The wallet's Activity names unlisted tokens** by what they say of themselves, labelled, like `1.5 "USDC" (unverified)`. Still only a claim: never on signing screens, and never in balances.
- [x] **Small things seen on the emulator:** an error from a failed import stayed on Accounts after a later success, and an offline signer's accounts said "not proven yet" for good.
- [x] **A small call could make every node allocate far more than it paid for.** `evm.decode` and its kin
  copied out one long value for every offset pointing at it, for a fee on the input alone. What decoding may
  make is limited by what it was given now. It needs every node at once.
- [x] **EVM calls and logs read as hex, or as `log 32 bytes 32 bytes`.** Both apps read them by name
  now, by every ABI shipped and the standard ERC-20's and ERC-721's in `src/abi/known.rs`, an ABI
  pasted or saved first, saying which read them. ERC-20 and NFT transfers count as token moves, as
  claims unless the registry lists the contract. The signing screens read their call too, under
  the facts they showed. `GET /events` finds logs by topic and by the addresses in their topics; an
  older index reads itself back once. The explorer's EVM pool activity and prices, which asked for
  `LOG` and got nothing, show.
- [x] **Several seed phrases in one wallet.** Accounts adds another, read or made new and written
  down, with its own account 0, on a signer or an online wallet, under the wallet's one password;
  each counts its accounts apart and matches a restore of it alone. Settings lists, names, shows
  and, after a preview, a typed yes and the password, removes them, all but the first. MetaMask
  requests pick their seed by fingerprint or address and refuse to guess, PSBTs sign with every
  seed they name. One phrase stays byte for byte as it was; two are a form older builds refuse.
  See [Wallet](wallet.md#wallets-and-words).
- [x] **A wallet could lose an account this build could not read,** left out as it opened and so
  gone with the next save. It refuses to open now, and says which, with nothing written.

- [x] **Nothing stopped one address flooding the API.** nginx limits each address now, contract calls harder, and the onion goes through it too.

- [x] **`gen_notices.py` stopped at hex_lit,** which names MITNFA and ships no text of it. Its standard
  text is in `notices/`, and the notices are generated again in full.
- [x] **A phone's status bar icons were pale on the light theme.** The app sets them from its theme now.
- [x] **The explorer's sidebar was one long list.** Its groups fold, and remember it.

- [x] **A Bitcoin reorganization stopped the relay for good:** `relay-sync` sent the headers from just
  past the relay's tip, which built on nothing it held. It goes back to where the two agree now.

- [x] **A node that justified a checkpoint on a branch that lost stayed there for good,** as it asked for
  justifications only with no peer heavier, and a heavier peer is what it could not follow. It asks
  once it holds all that peer's blocks. See [Finality](finality.md).

- [x] **Nodes restarted seconds apart could stop hearing each other,** as each came back the other way
  beside a connection not yet timed out. A peer that tells no topics for 30 seconds is dropped now, to
  come up afresh. See [Node](node.md).

- [x] **The phone's edit bar came up on pages with no field,** on a finger resting on a row whose id a
  field on another page once had, and stayed when its field went with a back gesture. A field now
  has to say it is under the finger, and the bar goes with its field.

- [x] **A light client stopped for good at epochs nobody justified.** On the testnet, epochs 2 and 3:
  it said it had caught up at height 31. Nodes now mark skipped epochs in `GET /light/finality`, and
  a client crosses on updates it holds tentatively until the voters it trusts agree with the next
  justification. See [Light client](light-client.md#what-to-keep-in-mind).
- [x] **A light client walking was slow:** about 1 MB and two round trips for every 16 blocks on a
  chain with governance. It now asks only for what it can check, and for it all at once through
  `GET /light/bundle`. See [Light client](light-client.md#what-to-keep-in-mind).
- [x] **The apps found out about new blocks only by asking** every few seconds. The wallet and the
  explorer's light client listen to `GET /stream` now, and ask when it says the chain moved. See
  [Light client](light-client.md#staying-up-to-date).
- [x] **The wallet's status dot stays green while the node is down;** it goes by `GET /status` now.
- [x] **A phone in portrait shows only a dot of the light client's state:** a tap says it in words, and
  Home carries a line while the client rests on a node's word.

- [x] **Updates: a warning, an updater, and a stop for upgrades.** A manifest a threshold of the
  release keys built in signs names the newest chainv4, wallet and explorer with a binary for each
  target; nodes fetch it with `--updates`, keep the newest in `release.json`, warn while behind,
  serve it at `GET /release` and tell workers; the apps check it themselves and offer Update on
  the desktop, and a screen past `least`. `chainv4 update`, `--auto-update` restarting in place,
  and `chainv4 release new/sign/verify` to publish. `Change::Upgrade`, or a release's `breaking`
  for a chain without governance, stops older nodes before a height. See [Updates](updates.md).
- [x] **The chain names itself.** Genesis's `name`, renamed by `Change::Name`, is what the apps
  call the network, from `GET /status`, in place of chainv4 hardcoded; the software keeps its own
  name in its commands, its releases and the EIP-712 signing domain.
- [ ] **Updates: next.** Release keys are the build's, so replacing a stolen one needs a new
  build; a rotation signed by the threshold, in the manifest, would let a running binary follow.
  The apps' Update replaces a desktop binary but not a macOS app bundle's signature, nor an APK or
  the iOS app, which update from where they were installed. Done 2026-11-06: the release
  workflow, run by hand on a tag, builds the binaries and an unsigned manifest as a draft release,
  signed by hand after (see [Updates](updates.md#publishing-a-release)).
- [x] **A faucet for dev chains.** `--faucet-key` serves `POST /faucet`: a drip per address and
  per IP each period, each request with a moment's proof of work; the wallet, the explorer and
  `chainv4 faucet` ask it, and `docker/pow.sh faucet` sets one up on the VPS. See
  [Faucet](faucet.md).
- [x] **The faucet's limits survive a restart.** They live in memory; a column of their own would
  keep them. Done 2026-11-06: the `faucet` column keeps each
  address and IP until its period ends.
- [x] **Engine pools sized on the node.** `--lua-states` (16), `--lua-builders` (2),
  `--wasm-cache-mb` (32) and `--wasm-cache-modules` (1,024), which the `Vm` takes at
  `Chain::open_with` (`EnginePools`), the API's own engines too. The EVM still builds its context
  fresh for every call: a pool of warm contexts is still open.

- [x] **The container's node stops on SIGTERM.** It stops its loop on SIGTERM or SIGINT and
  returns, so `docker/compose.sh stop` takes a second rather than ten and a kill.

- [x] **Transactions can be submitted from outside.** `POST /txs` takes `{"tx": "0x…"}`, a
  `SignedTx` encoded as the chain encodes it, and hands it to the node's mempool, which gossips
  it. `chainv4 keygen` and `chainv4 send` make keys and sign and submit transactions.
- [x] **`chainv4 send` waits for inclusion.** It polls `GET /txs/{id}` until enough blocks hold
  the transaction or `--timeout` passes, then prints the result and events.
- [x] **A README for the CLI.** Building, running nodes, keys, sending transactions and the API.
- [x] **Docker.** `docker/compose.yaml` runs a node with every feature, its chain made from
  presets or a genesis of your own, behind nginx with HTTPS from your certificate, Let's Encrypt
  through certbot, or a self-signed one.
- [x] **No published image.** Done 2026-11-06: the image workflow (by hand) pushes the node's and
  nginx's images to GHCR, private, by commit, `latest` and a version; `docker/pow.sh login` and
  `up --pull TAG` take them on a server. See [Docker](docker.md#pulling-what-ci-built).
- [ ] **A deploy compiles on every server.** Step 1 done 2026-11-05: `docker/pow.sh images` copies
  one server's images to the others and `up --no-build` starts them (see [Docker](docker.md#building-once-for-several-servers));
  the registry and CI building stay open. `docker/pow.sh up` builds the image on each VPS, so a
  redeploy of the devnet's three takes about 75 minutes, most of it the same build three times.
  Asked about 2026-11-04; queued after the pentest. In order of effort:
  1. **Build once, copy the image (first).** Build on the first server, as `staged.sh` already
     checks .57 first, then `podman save | ssh … podman load` the tested node and nginx images
     (about 170 MB and 70 MB compressed) to the others and restart them on it: no registry, no
     credentials, about 25 minutes in all. Keep building where it runs as the fallback, a flag on
     `pow.sh up`, for a server alone.
  2. **A registry,** GHCR, private as the repository is: one server or CI pushes, every node pulls.
     Worth it once there are more nodes, or other people's; each server then holds a pull token.
  3. **CI builds and pushes,** so no server compiles: spends runner minutes, and the workflows are
     manual only to save them.

  The images are x86_64, as the servers are; this Mac is arm64 and would build them under
  emulation, so a server stays the builder.
- [x] **The CLI speaks plain HTTP only.** ureq is built without TLS; add it before pointing
  `send` at a node across the internet. Stale: ureq is built with rustls, and the devnet's
  scripts send to https://chain.homepage.gs.
- [x] **Key files are not encrypted.** Done 2026-11-06: `keygen --seal`, `key encrypt` and `key
  decrypt`, Argon2id at 64 MiB and 3 passes with XChaCha20-Poly1305, opened by the node, the CLI,
  the wallet and the explorer, and the wallet's vault stretched the same way. See
  [Sealed key files](crypto.md#sealed-key-files).
- [x] **Genesis files.** `chainv4 genesis` writes the dev defaults as JSON; `--genesis FILE`
  runs any chain one describes. `--producer-key` names an authority other than the dev key.
- [x] **Finality in the API and CLI.** `/status` shows the kind and the justified and finalized
  checkpoints, receipts say whether they are final, `/validators` and `/evidence` list validators
  and conflicting votes, and `send` has `bond`, `unbond`, `withdraw`, `slash` and `--final`.
- [x] **Graceful shutdown.** Ctrl-C or SIGTERM stops the node loop between steps, within a
  second, and the database closes as the node is dropped. The network and HTTP threads end with
  the process rather than closing their connections first.
- [x] **The HTTP API has transaction lookup and receipts.** `GET /txs/{id}`, and each transaction
  in a block carries its receipt.
- [x] **The HTTP API answers side by side,** a thread a core (`--http-threads`), with each client
  held to 64 requests in flight (`--http-per-ip`), 429 past it.
- [x] **Transactions gossip in batches,** up to 256 or 50 ms' worth a message, each still judged
  alone and none sent twice. See [Node](node.md#throughput).
- [x] **`chainv4 bench`:** blocks built and imported in-process, and load against running nodes.
- [ ] **End-to-end tests.** Today's tests run a node in-process, or the CLI against one, and the
  regtest ones run Bitcoin Core; nothing runs the whole system as people use it. Done 2026-11-06,
  run by hand in the e2e workflow (see [Development](development.md#end-to-end)): several nodes
  as processes (gossip, sync, finality, a restart, a snapshot join, miners that mined apart);
  swaps across a node, anvil and Bitcoin Core (claims both ways, refunds past deadlines, a reorg
  under the Bitcoin lock); the wallet and the explorer driven by egui_kittest against a node.
  Left: the offline signer's codes, the Swaps and Trade pages and the DEXes through the apps,
  reorgs under a chainv4 or EVM lock, the extension with MetaMask, and Docker. Tests that start
  real processes and drive them through what users touch:
  - **several nodes** over real networking: producing, gossip, sync, finality, restarts, a node
    cut off and rejoining (as the laptop did, mining alone while the VPS rebuilt), snapshots,
    upgrades across a flag day;
  - **the wallet and the explorer,** driven through their interfaces (egui's kittest, or the web
    builds in a headless browser): accounts from words, sending, the offline signer's codes, the
    Swaps and Trade pages, the light client's proofs;
  - **swaps across real test chains:** chainv4 nodes, Bitcoin Core on regtest and an EVM node
    such as anvil, both parties' wallets, every pair of legs, claims and refunds past deadlines,
    and reorganizations under each lock;
  - **the DEXes** through the wallet: pools opened, trades, liquidity in and out, charts;
  - **the browser extension and MetaMask,** against a page that asks for accounts, sends and signs;
  - **Docker:** `pow.sh init`, `up`, `push` and `join` on throwaway machines.
- [ ] **Fuzzing,** with cargo-fuzz, run in CI for a while on every change and for longer
  overnight: everything a peer or a user can hand the node or the apps.
  - **Decoding:** blocks, transactions, votes and justifications, gossip and sync messages, light
    updates and proofs, from their wincode bytes; Ethereum's RLP transactions; JSON-RPC and the
    HTTP API's bodies.
  - **Running:** block import and reorganizations against a fuzzed sequence of blocks; the
    mempool; transactions with fuzzed arguments through each engine (Lua, WebAssembly, the EVM),
    held to their fuel and memory limits; the ABI encoder and decoder.
  - **The apps' inputs:** codes the scanner reads (ur: parts, PSBTs, swap terms, sign requests),
    BIP39 phrases, key files, the vault's sealed JSON.
  - **Differential:** the Lua and WebAssembly token examples against each other, and the EVM
    against revm's own test vectors.
- [ ] **Benchmarks across machines,** on the testnet: the same load spread over several nodes on
  separate links, and a long run for disk growth, which nothing prunes yet.
- [x] **An explorer.** `explorer/`, built with egui, for the desktop and, built to WebAssembly,
  the browser. It shows blocks on a map with side branches, transactions with their hops as a
  call tree and a graph, accounts, contracts and tokens, the network, the mempool and finality,
  with charts, and signs and sends transactions with chainv4's own code.
- [x] **The node tells how it is doing.** `GET /node` reports peers, roles, sync and counters,
  `GET /mempool` lists what waits, and `GET /stream` pushes blocks, reorgs, finality, queued
  transactions and peers as they happen, for nodes started with `--stream`.
- [x] **The API can serve web pages.** `--cors` lets pages from other origins call it, and
  `--explorer` serves the explorer's web app from the API's own `/`.
- [x] **The explorer lists tokens.** A Tokens page lists the registry by symbol, token pages read
  their name, symbol and decimals from it, and holdings cover its first 100 tokens.
- [x] **A light wallet.** `wallet/`, built with egui, for the desktop and the browser: accounts
  sealed with a password (Argon2id and ChaCha20-Poly1305), balances of the coin and every listed
  token proven at the tip and the final block, sending with a reviewed fee and tip, receiving with
  a QR code, and a history from the address index, each line proven. It pins the genesis on first
  use, saves the light client, and `--wallet` serves it from a node's `/wallet/`.
- [x] **The wallet runs on Android.** `wallet/build-android.sh` builds an APK with xbuild, a
  NativeActivity with no Java, for Android 7 and up. It keeps to the part of the screen Android
  says can be seen, and swaps the sidebar for tabs on narrow screens. CI builds it for arm64 and
  x86-64 and attaches both to `v*` releases, signed with the `ANDROID_SIGNING_PEM` secret.
- [x] **The explorer runs on Android, and on phones.** `explorer/build-android.sh` builds its APK
  the way the wallet's is built, through the shared `ui/build-android.sh`, and CI builds and
  releases it beside the wallet's. Below 640 points it swaps its sidebar for tabs, and its tables
  for stacked rows. Both apps share the `ui` crate: palettes, densities, widgets and Android glue.
- [x] **Copying on Android.** egui reaches no clipboard there, so the `ui` crate hands what is
  copied to Android's ClipboardManager over JNI. Addresses copy with a click or a tap in both apps.
- [x] **Picking files on Android.** A Java activity of the `ui` crate's, compiled into the APK
  after xbuild builds it, opens Android's document picker and hands back what it read: key files in
  both apps, and WebAssembly and Luau files in the explorer.
- [x] **Pasting on Android.** Holding a text field, or tapping one already focused, brings a bar with
  Paste, Select all, Copy and Cut, reading Android's clipboard through JNI.
- [x] **The APK's Java needs the Android SDK's build-tools.** Done 2026-11-06: `ui/android/classes.dex`
  committed, `ui/android/repack` adding it and signing with xbuild's library; apksigner verifies
  the result. Was: xbuild cannot add a `classes.dex`, so
  the script repacks and signs again with `d8`, `zipalign` and `apksigner`. An xbuild that took a
  dex, or a pure Rust dexer and signer, would make the APK build need a JDK alone, or nothing.
- [ ] **xbuild is pinned to a commit.** It has had no release since 0.2.0; move to one when it
  does. It cannot add a network security config or other resources either.
- [x] **Seed phrases.** BIP39 words with an optional passphrase; each account derives an Ed25519
  and an EdBlake key by SLIP-0010 and a secp256k1 key on Ethereum's path, in the library, `chainv4
  keygen --phrase` and the wallet, which holds several wallets, each with its own words and password.
- [x] **secp256k1, Ethereum's scheme,** behind the `secp256k1` feature: low-s ECDSA over keccak256,
  and Ethereum's 20-byte addresses, in transactions, contracts (33-byte ABI slots) and the wallet.
- [x] **Lua contracts split across files:** `--@include "path"` lines, put together by `chainv4 send
  deploy`, `chainv4 layout` and the explorer before signing, with mistakes placed in their files.
- [x] **Database migrations:** a `Migration` per format from 8 on, up and down in crash-safe chunks;
  a node migrates up as it starts after a checkpoint backup, and `chainv4 db migrate --to N` goes
  back down once confirmed.
- [x] **No migration exists yet.** Stale: format 9 came with the first; formats before 8 stay
  unmigratable.
- [x] **Errors a call raises name the bundle's lines.** Done 2026-11-06: `chainv4 send --source FILE`
  places a call's error back in the file it came from, as a deploy's are.
- [x] **Ethereum wallets sign for chainv4.** A transaction's EIP-712 typed form, salted with the
  genesis id, is a signature nodes take: MetaMask and the hardware wallets it drives sign for the
  browser wallet, and `chainv4 send --eip712` signs it with a key of its own.
- [ ] **Contract calls show as bytes in MetaMask.** Only transfers and token transfers have typed
  fields; a call's arguments have no clean EIP-712 form, as it has no unions. For the consensus
  batches (owner, 2026-11-06): a typed form for calls with each argument as its type's name and its
  value written out, which nodes take from the next regenesis.
- [x] **Sites reach the wallet through window.ethereum.** The browser extension gives dApps an
  EIP-1193 provider, announced by EIP-6963, once Settings let sites ask: reads go to the node,
  connecting, sending as an Ethereum transaction, `personal_sign` and `eth_signTypedData_v4` are
  approved in the wallet. See [Wallet](wallet.md#sites).
- [x] **Sites on the desktop and Android apps,** which have no extension. Done 2026-11-06:
  WalletConnect v2, spoken by the wallet itself (`walletconnect`): pairing by a scanned or pasted
  code, sessions sealed with the accounts, requests through the same approval as the extension's,
  the relay over wss:// through the proxy, off until Settings names a project. Not yet tried
  against Reown's relay with a project id: the protocol is tested against the reference client's
  formats, a simulated site and RFC 7748's vectors. See [Wallet](wallet.md#sites).
- [x] **wallet_watchAsset and several chains for sites.** Done 2026-11-06: a switch to a chain the
  wallet followed before connects to its node once you say so, any other is 4902; watched ERC-20s,
  ERC-721s and ERC-1155 ids on this chain's EVM show on Overview, read over proven state by the
  wallet's own EVM (`POST /call` with calldata, `vm::run_proven_evm`). See [Wallet](wallet.md#sites).
- [x] **The offline signer as MetaMask's QR hardware wallet,** ERC-4527: pairing by
  `ur:crypto-hdkey` at m/44'/60'/0', signing `ur:eth-sign-request`s read off MetaMask's animated,
  fountain-coded codes, for any chain, and showing `ur:eth-signature` back. See
  [Wallet](wallet.md#metamasks-qr-hardware-wallet).
- [x] **The offline signer as Bitcoin wallets' QR hardware wallet,** pairing by
  `ur:crypto-account` at m/84'/0'/0' or testnet's m/84'/1'/0', signing the native segwit inputs of
  `ur:crypto-psbt`s (BIP174 version 0, BIP143) and showing the PSBT back. See
  [Wallet](wallet.md#bitcoin-wallets).
- [ ] **The signer for other kinds of Bitcoin input and other chains.** Taproot (BIP341) and
  wrapped or legacy inputs, PSBT version 2, PSBTs as files, and Keystone's types for Solana,
  Cosmos and the rest, would each need their own paths and signing.
- [x] **Atomic swaps with Bitcoin.** The swap contract (`contracts/lua/swap.lua`) for the coin and
  tokens, Bitcoin's HTLC (`src/swap/htlc.rs`), offers and answers as `ur:chainv4-swap`,
  `chainv4 swap`, the wallet's Swaps page and the offline signer signing HTLC spends, deadlines by
  time on both chains, behind the `swap` feature. See [Atomic swaps](swaps.md).
- [x] **Swaps between any two chains, Ethereum's and ERC-20s included.** Terms as two legs, each on
  chainv4, Bitcoin or an EVM chain by its chain id, older codes still read; `contracts/evm/Swap.sol`
  for ETH and ERC-20 tokens on any EVM chain, at one address through the deterministic
  deployment proxy (`chainv4 swap evm-deploy`); `chainv4 swap` with `--pay`/`--get` legs and
  `--eth-rpc`; the wallet's Swaps page reading and sending EVM legs through JSON-RPC URLs in
  Settings, signed here or by the offline signer; the second lock held until the first stands.
- [x] **Swaps wait for finality.** The second lock waits until the first is final, and the
  starter's claim until the second is: chainv4's by its finality (or 20 blocks without), an EVM
  chain's by its finalized block (or 12), Bitcoin's by 3 confirmations, settable. The swap
  contracts say which block a swap was locked in. The CLI and the Swaps page hold, with an
  override. See [Atomic swaps](swaps.md#waiting-for-finality).
- [ ] **Before real value moves through the chain.** Not one of these is done:
  - ~~finality-aware swaps~~, done;
  - **independent validators,** run by different people on different machines, so no one key
    decides what is final, as the VPS's alone does now;
  - **security that is costly to attack:** real hashrate, or finality by stake, so a reorg is not
    cheap;
  - **frozen consensus:** no flag days without a migration and a release that says so;
  - **an outside audit** of the node, the swap contract, the HTLC flow and the wallet;
  - **a long run on Bitcoin's testnet** with swaps between people who are not the developers;
  - **legal advice** on exchanging assets where it would run.
- [x] **A Bitcoin light client in the wallet,** BIP157/158 compact block filters through Kyoto (the
  `bip157` crate), on the desktop and Android apps: swaps' HTLCs (funding, confirmations, the
  claim's secret, broadcasting to peers) and the accounts' Bitcoin balances, from a checkpoint
  built in, through the SOCKS5 proxy. See [Wallet](wallet.md).
- [x] **The Bitcoin light client: next.** Unconfirmed payments from the peers' mempools (Kyoto's
  gossip monitor), further keys' testnet addresses, checkpoints refreshed each release, and Send
  paying from the accounts' Bitcoin coins.
  Done 2026-11-06: checkpoints refreshed each release (`scripts/btc-checkpoints.sh`); further
  keys' testnet addresses; unconfirmed payments and spends, heard from gossip, shown pending on
  Overview and settled by a block (`light::hear`, `light::settle`; the regtest test hears one);
  Send paying from the account's confirmed coins, the fee suggested from Esplora or the tip
  block, the change to a new further key each time, replaceable (`bitcoin::pay`; Bitcoin Core
  mines one in the regtest test).
- [ ] **A Bitcoin SPV light client as a contract,** checking headers and merkle proofs with
  `sha256d` exposed to contracts, for proving Bitcoin deposits on chainv4.
- [ ] **Swaps with more chains.** The swap contracts know a hash and a deadline alone, so any chain
  that locks coins under a SHA-256 hash and a deadline can be a leg: a new `swap::Leg` with its
  own lock and spend. Litecoin takes Bitcoin's script as it is, with its own addresses and coin
  type; Dogecoin and Bitcoin Cash, without segwit, the same script in P2SH; Solana and Cosmos
  their own hash-lock programs. Monero, with no scripts, needs adaptor signatures instead: see
  below.
- [ ] **Swaps with Monero.** Monero has no scripts, so no HTLC: its side is a shared key, as in the
  COMIT protocol behind BTC↔XMR swaps (UnstoppableSwap) and Farcaster. The XMR goes to an address
  whose spend key is the sum of two halves, one from each side. chainv4's side is a contract that
  pays not for a SHA-256 secret but for a key half: Bob locks the coin, claimable with the scalar
  `s` where `s·G = S`, `S` Alice's half on Ed25519. Alice's claim shows `s`, which Bob adds to his
  own half to sweep the XMR. Monero cannot time out, so refunds live on chainv4's side: a cancel
  path after the deadline and a punish path, so neither side can stall the other.
  - **Simpler than BTC↔XMR:** Monero's curve is Ed25519, which chainv4 has natively, so the
    contract checks `s·G = S` itself and no cross-curve proof (secp256k1 against Ed25519, the
    hardest part of those protocols) is needed.
  - **Takes:** an Ed25519 scalar-to-point host function for Lua and WebAssembly (a consensus
    change); a swap contract for key halves, with cancel and punish; a `Monero` leg in the terms;
    and the wallet following the shared address by its view key and sweeping it, through a
    `monero-wallet-rpc` URL in Settings, as Esplora and EVM RPCs are, or a Monero library.
- [x] **Crypto for every engine.** Lua and WebAssembly contracts check signatures of every scheme
  the chain has, recover secp256k1 keys as `ecrecover` does, take a key's address, hash with
  RIPEMD-160, HASH160 and double SHA-256, and check state proofs and Merkle paths against a root;
  the EVM gets the same, and BLAKE3, BLAKE2 and XXH3, through the crypto precompile at `0x…4e56`.
  See [Contracts](contracts.md#crypto).
- [x] **The bridges' federation.** Its signers, threshold and depths, from genesis or
  `Change::Federation`, versioned, read by contracts and shown by `GET /status`. See
  [Bridges](bridges.md#the-federation).
- [x] **Bridges with EVM chains.** The gateway here, `gateway.lua`, and `Gateway.sol` on EVM
  chains carry the coin, ERC-20s and tokens, ERC-721s and NFTs, ERC-1155s and multi-token amounts
  both ways, held at their origin and wrapped elsewhere, on the federation's signatures gathered on
  the board here; `chainv4 bridge` deploys, joins, sends, signs and relays. See
  [Bridges](bridges.md).
- [x] **Bitcoin's bridge.** A Bitcoin light client contract, `contracts/examples/btc_relay`,
  checking headers as mainnet's nodes do from a checkpoint, with regtest's and a loose test mode;
  `btc_gateway.lua` minting wBTC for deposits proven to it, one deposit address per address, and
  paying withdrawals through releases it checks and the federation signs input by input (BIP143);
  sweeps to a new federation; `chainv4 bridge btc` from an Esplora API. See
  [Bridges](bridges.md#bitcoin).
- [x] **The chain's directory.** The contracts the apps and tools use, the bridges' gateways,
  the swap contract, the DEX and its router and the NFT factory, named by genesis or
  `Change::Directory` rather than by names anyone could register first; genesis's directory and
  federation written into the state, so the wallet proves both. See
  [Directory](governance.md#directory).
- [x] **The Bridge pages.** The wallet's: sending to and from EVM chains (its Ethereum key
  signing there, through the EVM link it shares with Swaps), the account's transfers out with the
  board's count and delivering one itself, the deposit address computed from the proven
  federation, deposits minted from the light client's proofs or Esplora's, withdrawals. The
  explorer's: the federation and directory, transfers with their signatures, wrapped assets, and
  the Bitcoin gateway's relay, holdings, deposits, withdrawals and releases. See
  [Wallet](wallet.md#bridge) and [Explorer](explorer.md#pages).
- [ ] **Bitcoin's bridge: next.** Replacing a release stuck at a low fee (its inputs freed, a new
  one proposed at a higher one); taproot deposit addresses, smaller and cheaper to spend; the
  relay pruning headers far below its tip; `chainv4 bridge btc watch`, syncing, signing and
  finishing on a timer.
- [ ] **Bridges: next.** The gateway's peers taken from the directory rather than set by its
  owner (signers already check them against it); carrying a federation change to each
  Gateway.sol, its signers signing `setSigners` through the board; a per-transfer and per-day
  cap; the wallet listing transfers from an EVM chain by Gateway.sol's logs, and asking a token
  there its decimals.
- [ ] **The federation as the validators.** Point the bridges' signer set at finality's validators,
  weighted by stake and slashable, once they sign with keys Bitcoin and the EVM check, and a
  threshold scheme lets a changing set hold one Bitcoin address.
- [ ] **Wrapped assets: bridges.** A swap trades what exists; a wrapped coin, such as BTC as a
  token here, needs a bridge that mints it one for one against coins locked on the other chain
  and releases them when it is burned. Minting can be trust-minimized with the Bitcoin SPV light
  client contract proving each deposit; releasing BTC cannot, as Bitcoin reads nothing of
  chainv4: a federation or a threshold of staked signers signs each release (as Liquid, WBTC or
  tBTC do) until BitVM-style proofs mature. With Ethereum, which runs contracts too, each chain
  can check the other's headers, and both directions can be trust-minimized. Swaps of the wrapped
  token against the real coin then keep its price honest.
  - **Fully trustless with Ethereum** takes a light client of each chain in a contract on the
    other, permissionless relayers, and no admin keys (immutable, or upgradable only behind a long
    timelock). Following Ethereum here needs BLS12-381 checks and Merkle-Patricia proofs as host
    functions; following chainv4 there needs its finality voted with secp256k1 keys, cheap
    through `ecrecover`, or a zero-knowledge proof of it, as the EVM has no Ed25519 or BLAKE3.
  - **With Bitcoin, only the way in can be trustless** today: releasing BTC needs a script that
    checks chainv4's state, which Bitcoin lacks. Short of a soft fork (covenants, or a ZK
    verifier), the best is 1-of-n honest operators (BitVM2) or signers slashed on chainv4 for
    stealing, safe while their stake exceeds what they could take.
  - **Either way** each chain's own consensus is trusted: a light client is only as good as the
    finality or depth it waits for.
- [x] **This wallet's own codes as URs.** `ur:chainv4-pairing`, `-sign-request` and `-signed`,
  carrying the chain's own encoding; CV4 and base45 are gone.
- [x] **Sites through the offline signer.** An account watched from the signer is offered to sites
  by the extension, and each send and signature goes to the signer as `ur:eth-sign-request`, its
  `ur:eth-signature` read back. See [Wallet](wallet.md#sites).
- [ ] **Hardware wallets reach the wallet through MetaMask alone,** and the desktop and Android apps
  reach none. WalletConnect, or USB straight to the device, would.
- [x] **Secrets are wiped from memory, and their copies.** The words, passphrase and keys as they
  drop, locking included; BIP39's own and `Phrase::parse`'s copies; typed fields with room made up
  front and no undo history; the vault's JSON written into room of its size; the explorer's pasted
  key and key file. See [Wallet](wallet.md#keys).
- [ ] **What egui lays out to show the words,** its text layout cache, and memory the system swaps
  out are not wiped; a locked page of shown words keeps the glyph runs until egui drops them.
- [x] **The wallet proves only the registry's first page of tokens,** 100 by symbol, as the
  explorer's holdings do. Stale: since cb57b89 the wallet follows
  every page of the registry to the end, and proves each token's entry and balances.
- [x] **CI does not build the web bundles.** It lints the explorer's and the wallet's wasm32 builds
  but never runs `trunk build`, so a broken `index.html` or wasm-bindgen mismatch shows only when
  someone builds them. Done 2026-11-06: the explorer and wallet jobs, and
  scripts/ci.sh, run `trunk build --release --locked`, with trunk pinned as the image pins it.
- [ ] **Windows builds in CI.** Left out because of RocksDB and Luau build setup. Done 2026-11-06,
  not yet run: the windows workflow (by hand) builds the node and runs its library tests, and
  builds the desktop wallet and explorer, LLVM's libclang for RocksDB's bindings and MSVC for the
  C++. Tick it once a run passes; what fails there is the next step.

### Engine script layer

From a review of the Lua layer in `~/engine` (`game/base/src/script`), not chainv4 itself.

- [ ] **Server-sent Lua may run as bytecode.** `script::run_file` checks the network cache first
  and runs anything starting with `\x1bLJ` as bytecode, which LuaJIT does not verify, so a server
  could escape the lockdown. Not reachable yet, because the client runs autorun before any
  `LuaFile` arrives. Load cached files as text; keep binary for the engine's own paks.
- [ ] **`Vector3` and `Angle3` take any cdata or userdata.** Their `FromLua` reads any userdata as
  a 24-byte f64 `Vector3` and any cdata, even `1LL`, as three floats. Check with `ffi.istype`
  through a registry helper.
- [ ] **One failing hook stops the rest.** `hook.call` runs callbacks without `pcall`, in `pairs`
  order, which decides who wins first-non-nil hooks like `Move` at random, and adding a hook
  while its event runs is undefined. Keep insertion order, snapshot the list, `xpcall` each
  callback, and add `hook.remove`.
- [ ] **`run_hook` recovers from errors badly.** It compiles a `"return"` chunk on every error,
  and panics when the return type cannot be built from nil, as `(bool, String, i32)` cannot. Use
  `R::from_lua_multi(MultiValue::new(), lua)` and only allow types that can come back empty.
- [ ] **No runaway script guard.** `while true do end` in a hook freezes the frame, and LuaJIT's
  instruction hooks do not fire inside compiled traces. Needed before third-party addons or
  server-pushed code run.
- [ ] **The lua file cache is never cleared.** `cache::clear()` has no callers, so one server's
  files stay when joining another, and the cache is process-wide statics a listen server shares
  with its client.
- [ ] **Nine raw-pointer scopes.** `ScriptEngine` has nine `*_access` `AtomicPtr`s, each with its
  own scope guard, and `state.rs` builds nine raw pointers per call. One context struct behind one
  scope would also make it easier to check that no native holds `&mut EntityList` while calling
  back into Lua.
- [ ] **`ScriptEngine` repeats its wrappers.** About 15 methods repeat the same `has_ents` check,
  match and `log::error!`; one helper would do.
- [ ] **Stale `.luac` beats fresh `.lua`.** `pick_scripts` prefers compiled files at equal mount
  priority, so an old compiled file wins over source just edited.

maybe have to check the event log not indexing properly in explorer? maybe settings werent properly changed
add fuzzing
make consensus switchable (pow -> pos, etc)
adjust fuel costs

- A light wallet: build chainv4 wallet, combining keys and send with light-client checks of your balance and receipts.

❯ ohhh light client is wallet right

Close, but not quite. A wallet does two jobs, and the light client only does one of them:

1. Keys and sending: holding a key, signing transactions and submitting them. In chainv4 that's chainv4 keygen and chainv4 send, and the explorer's Send page. The light client holds no keys and sends nothing.
2. Knowing what's true: your balance, and whether a payment landed and is final. Most wallets just ask a node and trust the answer. The light client is a way to do this part without trusting the node: it checks headers, finality and proofs itself.

So a light client is what a trustless wallet uses for reading, like old SPV Bitcoin wallets or Helios under an Ethereum wallet. The explorer is now the closest thing to a light wallet here: it signs and sends on its Send page, and its account pages show balances the light client proved.

That also matters for what you asked earlier. There's no separate light process an explorer can point at, because each explorer runs its own light client against the node it's connected to.

tighten lua sandbox
add a thing to storage (At least lua), where we dont have to save addresses as text but can just save it directly
make sure max supply is configurable
storage seems to save strings only, make it save raw types to save space, etc
scan whole project in order -> scan docs -> go section by section and verify theres nothing that shouldnt be there/stale -> go through each subsytem nad find bugs, stuff that would be good to change -> go through each subsystems tests (all of them), and see if theres something that doesnt make sense/should be added or removed.


On running the PoW consensus on a real network: the protocol side is complete and tested: a configurable secure work hash, Bitcoin-style windowed retargeting with median-time-past and clamped intervals, fork choice by work bounded by max_reorg_depth, optional FFG checkpoints on top, GPU mining, and a VPS-plus-home run from the earlier batch. What I would not call ready: it has had no outside audit, sync pulls from one peer at a time, there is no selfish-mining mitigation beyond what Bitcoin has, and without finality a partition longer than max_reorg_depth blocks never heals on its own. Usable for a test network or a small launch; treat value on it as at risk until audited.


What I'd prioritize next, over new features:
1. A real testnet: 4 to 6 nodes on separate machines and networks, a couple of miners, running for weeks, with restarts, partitions and clock skew. Most of what broke today would have shown up there.
2. Multi-node scenario tests in CI: in-process networks that fork, partition, rejoin and race miners. Today's two fixes now have tests; the class of bug needs more.
6. An outside security review before anything has real value: consensus, finality, the VM and the wallet's key handling.
