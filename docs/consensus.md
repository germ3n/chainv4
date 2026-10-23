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
- **Difficulty** adjusts every block from its parent alone, against `target_block_ms`:
  - a block that comes in under the target time goes one step up;
  - one within one to two targets stays where it is;
  - each target past that is a step down, at most 32 in one block.

  A step is the parent's difficulty divided by 64. The first block uses `initial_difficulty`, and
  difficulty never falls below `min_difficulty`.
- **Timestamps** must be after the parent's, and at most 15 seconds ahead of the checking node's
  clock.
- **Weight:** a block weighs its difficulty, and the main chain is the branch with the most total
  work.

**Mining** (`--mine`, `--mine-threads`, `--reward-to`):
1. The node builds a template block.
2. The `Miner` hands it to every thread, each trying its own share of the nonces.
3. Whichever thread finds one first reports the seal back to the node as an event.
4. The node rebuilds the template every two seconds, or when the tip moves, so it carries newer
   transactions and a newer timestamp.

## Things to keep in mind

- **The difficulty rule looks only at the parent.** A miner may set its timestamp up to 15
  seconds ahead to lower the next difficulty a little.
- **Forks are normal under proof of work.** A node reorganizes to a heavier branch, up to
  `max_reorg_depth` blocks back. Finality stops it going further (see [Chain](chain.md)).
- **The authority is a development key.** Its seed is in the source. A rotating authority set,
  or producing by stake, would be new `Consensus` and `Producer` implementations.
