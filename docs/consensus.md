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
before it has any state to take signers from, so it is refused on a rotating chain: such a chain
syncs by running its blocks.

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
`Producer` like the `Miner`, but with one thread that hands the GPU the template's work a million
nonces at a time (`Gpu::search`) and reports the lowest one that meets the target, which is the
one the threads would have found first. The GPU hashes exactly what the CPU does, the same
`"chainv4 work"`, block id and nonce, and the crate's tests hash against the CPU crates on
whatever GPU runs them. It logs its hashrate every 30 seconds; a GPU that fails stops mining,
and the log says so. CUDA has no kernel of its own: NVIDIA cards mine through Vulkan.

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
  producers runs a rotating authority; producing by stake would be another `Consensus` and
  `Producer`.
- **A rotating chain lags for light clients, and takes no snapshot.** Its blocks are checked
  against signers in state, so a light client follows `activation_delay` epochs behind what
  finality has justified, and a new node syncs by running every block.
