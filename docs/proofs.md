# Validity proofs

Finality says which block is final: two thirds of the validators signed it. It doesn't say the
block was right. Two thirds of the validators could sign a state root that the blocks never came
to. A validity proof closes that gap. It is a zero-knowledge proof that every block of a range ran
by the chain's rules, from one state root to the next. A contract on Ethereum, the chain itself
and the apps each check it, so a wrong state root is refused there whatever the validators signed.

It comes in three stages:

| Stage | What it gives | Where |
| --- | --- | --- |
| 1. Finality on Ethereum | Validators' attesters sign each link with secp256k1. ChainV4Finality.sol follows justified and final checkpoints and their state roots. | [Finality](finality.md#attestations-finality-followed-on-ethereum) |
| 2. State roots by proof | An SP1 proof that a range of blocks ran to its end's state root. The chain takes it as `Tx::Prove` and pays for it, and ChainV4Finality.sol takes it too. | This page |
| 3. Bridges and light clients by proof | Gateway.sol releases withdrawals by a proof of the range they left in. The apps check the latest proof themselves. | This page, [Bridges](bridges.md#withdrawals-by-proof) |

Bitcoin can't check a zk proof, so the Bitcoin gateway stays with the federation. BitVM-style fraud
proofs are in the [todo](todo.md).

## What a proof proves

A proof covers a **range**: from a start block, whose state root is known (proven before, or
trusted), to a later block, a final checkpoint. Its public values are eleven 32-byte words, laid
out as `abi.encode` lays them out (`proofs::ProvenRange`):

```
chainId, genesis, startHeight, startBlock, startRoot, endHeight, endBlock, endRoot,
withdrawals, program, gatewayMode
```

- **genesis** names every rule the range ran by, so a proof of one chain is no proof of another.
- **withdrawals** is the keccak256 Merkle root of every transfer the directory's gateway sent in
  the range, in order. Each leaf is the keccak256 of a transfer's digest, each node the keccak256
  of its two children with the lower first, and an odd one out is carried up. It is zero when the
  range sent none (`proofs::withdrawals_root`, `withdrawal_proof`).
- **program** and **gatewayMode** are governance's settings at the end. The contracts follow
  them, so the chain's governance moves them without anyone holding a key there.

It says nothing about which block is final. That still comes from the validators' attestations
(stage 1). What the proof adds is that the final checkpoint's state root is the one the blocks
come to.

## How a range is run

The proof runs the very code a node runs a block by, `Chain::run_block`, inside the zkVM:

- **The witness:** the state the range starts from, checked against the start's state root before
  anything runs (`db::check_witness`). It holds every entry the range reads, and the state-tree
  nodes on the way down to them and to everything the range changes, each checked against its
  parent up to the root. A range that scans a column of state gets the whole state instead, since
  a proof of single entries can't show a scan is complete; its root is worked out from the entries
  (`root_of`).
- **The headers:** the start's header and the ancestors the range reads, the checkpoints its
  justifications name, each checked against the one after it by its parent's id.
- **Nothing else:** the replay serves only the state, the tree and those headers. A range that
  reads anything else fails, so nothing the proof didn't check goes into what it proves. The node
  builds the witness the same way, by running the range over its history and noting what it read
  (`RangeInput::build`). That's also how it finds such a read before anyone proves.
- **Each block** must build on the one before, hold its own transaction, proof and pool roots, and
  come to its header's gas, receipts root and state root, all as on import. Transactions'
  signatures are checked as they are on import. The consensus seal is not checked, as stage 1 says
  which block is final.

The replay is plain Rust in the library, `proofs::replay`, behind the `chain` feature: the chain's
state transition without the node. libp2p, RocksDB, tokio, rayon and the API are not in it. The
`consensus` feature lists everything that decides how a block runs, the engines among them. The
program builds chainv4 with that and nothing else.

## The program, and proving by hand

The program is `proofs/sp1/program`, SP1's guest: it reads a range's input, runs `replay`, and
commits to the public values. Build it with SP1's toolchain (`sp1up --c-toolchain`, which also
installs the RISC-V GCC that Luau's C++ needs):

```sh
cd proofs/sp1/program
export CC_riscv64im_succinct_zkvm_elf=riscv64-unknown-elf-gcc CXX_riscv64im_succinct_zkvm_elf=riscv64-unknown-elf-g++ \
       AR_riscv64im_succinct_zkvm_elf=riscv64-unknown-elf-ar CXXFLAGS_riscv64im_succinct_zkvm_elf="-include climits"
PATH="$PATH:$HOME/.sp1/bin:$HOME/.sp1/riscv/bin" cargo prove build --output-directory ../elf
```

- **Luau in the zkVM:** the program links SP1's newlib and libstdc++ (`build.rs`), and
  `src/libc.rs` gives Luau memory from SP1's allocator. Output is dropped, and time reads zero:
  contracts read the block's time, never the clock.
- **The program's id** is its verifying key hash: `chainv4 prove program`. Any change to how blocks
  run is another program, which governance names in turn.

Proving is by hand on whoever's machine, with the `sp1-prover` feature, which no node needs:

```sh
cargo build --release --features sp1-prover
chainv4 prove status --node https://…          # settings, the proven tip, the newest final checkpoint
chainv4 prove check  --node https://…          # the range from the proven tip to it, run off the node: a moment
chainv4 prove execute --node https://…         # in SP1, unproven: the cycles it takes
chainv4 prove run --node https://… --out p.json   # proven, in Groth16: slow, and heavy on memory
chainv4 send --key k.key prove --proof p.json  # to the chain
chainv4 bridge … finality-prove 0x<ChainV4Finality.sol> --proof p.json   # to Ethereum
```

`--from` and `--to` pick another range, and `chainv4 prove input --out f` keeps one to prove
later with `--input f`. The node builds the input at `GET /validity/input?from=&to=`, at most
`MAX_VALIDITY_RANGE` blocks at a time. That needs an archive, or a node whose `--state-history`
reaches the start. Tests run the replay natively and SP1's executor only, never a prover.

## On the chain: the `proofs` fork

From the `proofs` fork ([Forks](governance.md#forks)):

- **Each block records its parent's id**, the latest `RECORDED_BLOCKS` of them, for a proof to end
  at. The fork's first block starts the **proven tip** at its parent: the state the first proof
  runs from (`proofs::ProvenChain`).
- **Governance names the proof system and program** with `Change::Proofs`, along with what is
  minted for each block a proof covers, and how Gateway.sol releases:

  ```sh
  chainv4 send --key member.key vote-change --vote yes --change '{"proofs":{"kind":"sp1-groth16","program":"0x…","reward_per_block":"1000000000000000","gateway":"proof-or-federation"}}'
  ```

  Until it does, the chain takes no proofs. The settings are lockable like any other.
- **`Tx::Prove`** takes a proof of the range from the proven tip to a later checkpoint this chain
  recorded, of this genesis, by the kind and program governance named. It moves the tip to the
  range's end and mints the sender `reward_per_block` for each block it covers. Each node checks it
  with the kind's verifier: SP1's Groth16 by `sp1-verifier`, a few milliseconds of pairings.
- **Before the fork,** a block holding `Tx::Prove` or `Change::Proofs` is invalid, as it is to a
  release from before it.

`GET /validity`, and `validity` in `GET /status`, give the settings, the tip, and the latest proof
the chain took.

## On Ethereum

ChainV4Finality.sol takes proofs beside the attestations:

- **`prove(publicValues, proof, epoch)`:** a range from its proven tip to the checkpoint of an epoch
  it finalized, with that checkpoint's block, height and state root, checked by SP1's verifier
  gateway under its program. It moves the tip, keeps the range's withdrawals root by its end block,
  lists the range's end (`provenEnds`, `provenBlockAt`), and takes the program and gateway mode
  from the proof.
- **Deploying:** `chainv4 bridge finality-deploy --sp1-verifier 0x… [--program 0x…]` starts proofs at
  the chain's proven tip, or at its trusted checkpoint while there is none. Without a verifier it
  takes no proofs.

## Light clients

The latest proof proves itself. An app asks any node for it (`GET /validity`) and checks it alone
(`TakenProof::check`), and the node's word adds nothing:

- the proof by the program governance names, which the app proves from state like any other value;
- its genesis against the app's own chain;
- its range's end against a header in the chain the app follows.

Then it knows the chain is proven up to there. Votes still carry it past that, to the newest
blocks.

## Choosing another zkVM

Nothing above is SP1's but the program crate, the `sp1` and `sp1-prover` features, and one
`ProofKind`. A proof system is a `ProofKind`, with a pinned tag, and the `ProofSystem` that checks
its proofs. A prover is a `Prover`. Another zkVM is another kind and feature beside SP1's, and
governance switches the chain to it with `Change::Proofs`.

## What to keep in mind

- **A program per release.** The program is the replay as built. A release that changes how any
  block runs is another program, and the chain takes its proofs only once governance names it.
  Proofs by the old program still prove the ranges they cover.
- **The program and the node must agree.** Every feature that decides how a block runs goes in
  `consensus`. A build that runs blocks otherwise makes proofs fail, rather than prove something
  false.
- **Scans make big witnesses.** A range in which any contract scans its storage carries the whole
  state. Proving is slower then, though no less sound.
- **The proven tip is chain state.** A range that holds a `Tx::Prove` proves it too, the proof
  checked inside the zkVM by the same verifier.
