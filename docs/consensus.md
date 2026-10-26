# Consensus

Consensus (`src/consensus/`) decides who may produce a block, when, and how much each block weighs
when choosing between forks. It comes in two halves:

- **The rules** (`Consensus`): what every node checks a block against. They are built from
  genesis, so all nodes agree on them:
  - `work`: how much a block weighs;
  - `difficulty`: what the next block must declare;
  - `check`: its producer, timing, difficulty and seal.
- **The producer** (`Producer`): what a producing node does:
  - where its rewards go;
  - when to build the next block;
  - how to seal it.

Only producing nodes have one. An authority seals at once; a miner hands the header to its
threads, and the seal arrives later.

## The development authority

One known key produces every block (`DevAuthority`, `AuthorityProducer`).

- **Who:** a block's producer must be the authority's address.
- **When:** its timestamp must be at least `slot_ms` after its parent's, and at most
  `MAX_DRIFT_MS`, 15 seconds, ahead of the checking node's clock.
- **Seal:** the authority's signature over `"chainv4 block"` and the header's id.
- **Difficulty** is 0, and every block weighs 1, so the longest chain is the heaviest. There is
  only one producer, so forks do not happen unless two nodes produce with the same key.

Run it with `--produce`, on exactly one node, with the development key unless `--producer-key`
names the authority's key file.

## Proof of work

Anyone may produce a block whose seal holds a nonce that meets the block's difficulty
(`ProofOfWork`, `Miner`).

- **The work hash** is the PoW config's `hash`, over `"chainv4 work"`, the block's id and the
  nonce. Read as a big-endian number, it must be at most `u256::MAX / difficulty`, so valid
  hashes start with zeros in hex. The work hash is chosen apart from the chain hash, and must be
  a secure one.
- **Difficulty** retargets every block, against `target_block_ms`, one of two ways. Genesis
  picks by setting `consensus.window` or not. Either way the first block uses
  `initial_difficulty`, and difficulty never falls below `min_difficulty`.
  - **With a window** (`window`, the dev chain's 11): the window is the block's `window` newest
    ancestors, genesis left out. Difficulty is the work those blocks took over the time they
    took, scaled to the target: the sum of their difficulties, times the target, over the sum of
    the intervals between them. Each interval counts as at least a quarter of the target and at
    most four times it, so one long gap, or one producer's clock set ahead, moves difficulty by a
    bounded share of the window, and the window carries the rest. A block's own timestamp never
    sets its own difficulty, only the next ones'. Until the window holds two blocks there is no
    interval to measure, so the first two blocks take the initial difficulty.
  - **Without one**, from the parent alone:
    - a block that comes in under the target time goes one step up;
    - one within one to two targets stays where it is;
    - each target past that is a step down, at most 32 in one block.

    A step is the parent's difficulty divided by 64.
- **Timestamps** must be at most 15 seconds ahead of the checking node's clock, and:
  - with a window, after the median of the window's timestamps (median time past, as in
    Bitcoin), so one producer's clock set ahead does not drag the chain's time along: the next
    block may be timestamped before it;
  - without one, after the parent's.
- **Ancestors:** the rules take the parent and as many ancestors as `Consensus::lookback` says,
  the window or 1. Whoever checks a header hands them over: the chain from its stored headers,
  the light client and snapshot sync from the headers they already checked.
- **Weight:** a block weighs its difficulty, and the main chain is the branch with the most total
  work.

## Rotating authority

The signers of authority finality produce the blocks in turn (`Rotating`, `RotatingProducer`).
Genesis names `consensus.type: rotating` with a `slot_ms`, and needs `finality.type: authority`:
the signers it fixes for each epoch (see [Finality](finality.md#authority)) are that epoch's
producers, so they change by the signers' own approval, and a signer thrown out for conflicting
votes stops producing with the set it leaves.

- **Who, and when:** the epoch's signers, sorted by key, take the heights round robin: at height
  h the signer at h mod n is in turn, and may produce from `slot_ms` after the parent. Each
  further slot that passes lets the next in line step in too, so a missing signer costs one slot
  and no more. A block names the signer's account as its producer, where its rewards go, and its
  seal is the signer's voting key's signature over the header's id, as its votes are signed.
- **Difficulty** is the producer's offset in the schedule: 0 in turn, 1 for the next in line,
  and so on. The producer declares it (`Producer::difficulty`), and the rules check it against
  the schedule and the time.
- **Weight:** an in-turn block weighs 2, a stepped-in one 1, so the branch that kept to the
  schedule is the heaviest, and a signer that produces out of turn gains nothing by it.
- **Where the signers come from:** the rules take them from whoever checks the block
  (`Consensus::needs_producers`): a node from the set fixed in its state for the block's epoch,
  a light client from the voters it proved from finality's updates. A block on a branch that
  split off before the checkpoint that fixed its epoch's set is checked against the main chain's
  set, as its votes would be.

Run it with `--produce` on each signer's node, with `--producer-key` naming the signer's voting
key file, the same key `--validate` votes with; the node works out the signer's account from
genesis, or takes the key's own address. A node whose key is not among the epoch's signers waits.

**Light clients** check a block against its epoch's signers, which they learn from the light
update of the checkpoint `activation_delay` epochs before, trusted once that checkpoint is
justified (see [Light client](light-client.md#following-the-chain)). So they follow headers
`activation_delay` epochs behind finality, and wait for it. **Snapshot sync** checks headers
through a light client of the node's own chain, fed over the peer protocol, so it learns the
signers the same way and takes its pivot among the headers it could check (see
[Node](node.md#syncing-from-a-snapshot)).

## Production by stake

The bonded validators of FFG finality produce the blocks, each with a share in proportion to its
bond (`Staked`, `StakedProducer`). Genesis names `consensus.type: staked` with a `slot_ms` and a
`schedule`, and needs `finality.type: ffg`: the validator set finality fixes for each epoch (see
[Finality](finality.md#ffg)) is that epoch's producers, so a validator produces from the epoch
its bond first counts in and stops with the epoch it leaves, and a slashed one goes with it.

- **Who, and when:** the schedule names, for each height, the validator in turn and the one each
  slot that passes hands the height to. The one in turn may produce from `slot_ms` after the
  parent, the next from one more slot on, and so on. A block names the account the validator's
  bond names as its producer, where its rewards go, and its seal is the validator's voting key's
  signature over the header's id, as its votes are signed.
- **The schedules**, picked in genesis:
  - **`draw`:** the chain's hash over the epoch's set, the height and the slot draws a validator
    with a chance in proportion to its bond, and every slot that passes draws again. Stateless,
    so checking a block takes the set alone. A validator may be drawn for several slots of a
    height in a row, and a missed slot costs the chain one slot's wait. A validator looks
    `MAX_DRAW_WAIT` (1,024) slots ahead for its turn and no further.
  - **`priority`:** Tendermint's proposer rotation. From the epoch's first height on, each
    validator's priority grows by its bond every height; the highest produces, ties going to the
    lower key, and its priority falls by the set's whole weight. Over an epoch each validator is
    in turn its bond's share of the heights, exactly when the weights add up to the epoch's
    length, and the order at each height, the next in line stepping in, is known in advance.
    Checking a block replays the epoch's heights before it: `checkpoint_interval` rounds over the
    set, which is cheap for the epochs chains run.
- **Difficulty** is how many slots the producer waited, 0 in turn, which the producer declares and
  the rules check against the schedule and the time. **Weight** is as under a rotating authority:
  an in-turn block weighs 2, a later one 1.
- **Where the validators come from:** as a rotating authority's signers, from whoever checks the
  block: a node from the set fixed in its state for the block's epoch, a light client from the
  voters it proved. An account that bonded more than one voting key produces with each, at each
  one's turn.

Run it with `--produce` on each validator's node, with `--producer-key` naming the validator's
voting key file, the same key `--validate` votes with; the node takes the account from the
epoch's set. A node whose key is not among the epoch's validators waits. Light clients and
snapshot sync follow a staked chain as they follow a rotating one.

## Changing consensus

A chain with [governance](governance.md) can change its consensus by transaction: start under
proof of work, say, and move to production by stake once enough validators have bonded. The
decided consensus is fixed in state for each epoch `activation_delay` epochs ahead, at
checkpoints, as finality fixes its voters, so a block is checked by the rules of its own epoch
(`Chain::consensus_at`), light clients learn each epoch's rules from a light update, and a node
started with `--mine`, `--produce` or both builds the producer for whatever consensus the chain
runs now. The finality kind stays what genesis names, and a change must fit it: `rotating` needs
authority finality, `staked` needs FFG.

**Mining** (`--mine`, `--mine-threads`, `--reward-to`):
1. The node builds a template block.
2. The `Miner` hands it to every thread, each trying its own share of the nonces.
3. Whichever thread finds one first reports the seal back to the node as an event.
4. The node rebuilds the template every two seconds, or when the tip moves, so it carries newer
   transactions and a newer timestamp.

**Mining on the GPU** (`--mine --mine-gpu`, in a build with the `gpu` feature): the
`chainv4-gpu` crate (`gpu/`) drives the GPU through wgpu, on Vulkan (Linux and Windows, NVIDIA
and AMD alike), Metal (macOS) and WebGPU, with one compute kernel in WGSL for every work hash a
genesis may name: SHA-256, BLAKE3, BLAKE2s, BLAKE2b and Keccak-256. The `GpuMiner` is a
`Producer` like the `Miner`, but with a thread for each GPU that hands it the template's work a
million nonces at a time (`Gpu::search`) and reports the lowest one that meets the target, which
is the one the threads would have found first. The GPU hashes exactly what the CPU does, the same
`"chainv4 work"`, block id and nonce, and the crate's tests hash against the CPU crates on
whatever GPU runs them. It logs its hashrate every 30 seconds; a GPU that fails stops mining,
and the log says so, while the others go on. CUDA has no kernel of its own: NVIDIA cards mine
through Vulkan.

**Every GPU, or those picked.** `--mine-gpu` mines on every GPU the machine has; `--gpus 0,2`
picks some, by the numbers `chainv4 gpus` lists. The node logs the same list as it starts. GPU
*i* of *n* takes every *n*-th batch of a million nonces from the *i*-th, so no two try the same.
A GPU listed through two backends, such as Vulkan and GL, is listed once, by its native one, and
software rasterizers are left out.

**The rig.** `Miner` and `GpuMiner` are both a `Rig`, its threads and devices, with an address
for the blocks; a worker mines on a `Rig` too, with no blocks of its own. `Rig::work` hands every
thread a block id, its difficulty and the nonce to start from; `Rig::devices` says each device's
hashrate, measured by a `Meter` over the last three seconds.

**Mining flags:**

| Flag | Default | What it does |
| --- | --- | --- |
| `--mine` | off | Mine, under proof of work. |
| `--reward-to ADDRESS` | the development key's | Where the block rewards go. |
| `--mine-threads N` | 1 | CPU threads to mine on; 0 for none, when workers mine for the node. |
| `--mine-gpu` | off | Mine on the GPUs instead, in a build with the `gpu` feature. |
| `--gpus 0,2` | every one | The GPUs `--mine-gpu` mines on. |
| `--mining ADDR` | none | Take workers on this TCP address, such as `0.0.0.0:4002`. |
| `--mining-token FILE` | none | The token workers must know, for `--mining`: at least 32 characters. |

## Mining with workers

A node with many GPUs across machines needs only one node: the others run `chainv4 worker`,
which mines for it over its **mining port** and keeps no chain at all.

```sh
openssl rand -hex 32 > mining.token        # once; give the workers a copy
chainv4 --mine --mine-threads 0 --mining 0.0.0.0:4002 --mining-token mining.token …
chainv4 worker --node 203.0.113.7:4002 --token mining.token --gpu          # every GPU
chainv4 worker --node 203.0.113.7:4002 --token mining.token --gpu --gpus 1 # one of them
chainv4 worker --node 203.0.113.7:4002 --token mining.token --threads 8    # CPU threads
```

**How it goes:**
1. The node's producer is a `Pool`: the node's own miner, if it has threads or GPUs, and the
   workers connected.
2. Each new template's work goes to every worker as a `work` message: the job's number, the block
   id, its difficulty, the work hash, and the start of the worker's **range** of nonces.
3. The ranges start at 2^60, far above anything the node's own devices reach, and are 2^48 apart,
   one for each connection, so no two workers try the same nonces. A worker splits its range
   among its own GPUs or threads the way a node does.
4. A worker sends `found` with a nonce. The node checks it meets the difficulty before it becomes
   the seal: a worker cannot make the node publish a bad block. A nonce that misses, or comes for
   a job already over, is refused and counted.
5. Every five seconds a worker sends its devices and their hashrates (`rate`), for the node's
   report and the explorer.
6. The node pings a worker it sent nothing to in 20 seconds; either side takes the other for
   gone after 60 seconds of silence, and the worker connects again every 5 seconds until it is
   back.

**The protocol** is lines of JSON over TCP (`src/mining/protocol.rs`):
- **The handshake proves the token without sending it.** The node sends a random challenge; the
  worker answers with its name, a nonce of its own, and an HMAC-SHA256 of both under the token;
  the node answers with its own HMAC, so the worker knows it found the node meant. Both say the
  protocol, `{name}-mining/1` for the `--protocol` name, and a worker of another name is turned
  away before anything else.
- **Every message after carries an HMAC** under a session key both derive from the token and
  both randoms, over its direction, its number in that direction and its text. A message changed,
  made up, replayed or dropped does not check, and the connection ends.
- **It is not encrypted.** Anyone on the path sees the work and the nonces, which only matter
  until the block is out, and cannot change them. Put the port behind a VPN or an SSH tunnel to
  hide even that.

**What it does not do:**
- **No shares.** Workers send only nonces that meet the block's difficulty, so a slow worker's
  part shows in its hashrate, not in what it found. The node pays nobody; the rewards go to its
  `--reward-to`, and splitting them among owners is up to them.
- **The node's limits:** at most 256 connections, handshakes included, each with 10 seconds to
  finish it.

`GET /node` reports it all under `mining`: the node's devices, `listening`, and each worker with
its address, when it connected, its devices, and what it found and had refused. The explorer's
Network page shows it with a hashrate chart, and the Overview a tile.

## Things to keep in mind

- **Pick a window long against the clock drift.** A producer may timestamp a block up to 15
  seconds ahead. With a window, that counts for at most four targets of one interval among the
  window's, so the longer the window's span against 15 seconds, the less it can move difficulty;
  the dev chain's 11 blocks of 2 seconds are short, as a dev chain can afford. Without a window,
  it lowers the next block's difficulty a little, up to 32 steps of a 64th.
- **The window changes the dev proof-of-work chain's id.** `Genesis::dev_pow` sets it, so a
  `--consensus pow` database from before starts over; a genesis file without `window` keeps its
  id and the parent-only rule, since the field encodes as nothing while unset.
- **Forks are normal under proof of work.** A node reorganizes to a heavier branch, up to
  `max_reorg_depth` blocks back. Finality stops it going further (see [Chain](chain.md)).
- **The authority is a development key.** Its seed is in the source. A chain with several
  producers runs a rotating authority or production by stake.
- **A rotating or staked chain lags for light clients.** Its blocks are checked against voters in
  state, so a light client follows `activation_delay` epochs behind what finality has justified,
  and so does a node syncing from a snapshot, which checks headers as a light client does.
- **Under the draw, a validator's turn is luck per slot.** A validator with a thousandth of the
  stake is in turn one height in a thousand, and, when the one in turn misses, drawn next with
  the same chance; the priority schedule gives exact shares and a known order instead, for the
  cost of replaying the epoch when checking.
- **Switching consensus needs its producers ready.** A move to stake needs validators in the
  switch epoch's set with nodes producing; a chain that switches to a scheme nobody serves stops
  (see [Governance](governance.md)).
