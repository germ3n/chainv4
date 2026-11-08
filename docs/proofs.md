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
| 2. State roots by proof | An SP1 proof that a range of blocks ran to its end's state root, in Groth16, PLONK or as SP1's STARK. The chain takes it as `Tx::Prove` and pays for it, and ChainV4Finality.sol takes the first two. | This page |
| 3. Bridges and light clients by proof | Gateway.sol releases withdrawals by a proof of the range they left in. The apps check the latest proof themselves. | This page, [Bridges](bridges.md#withdrawals-by-proof) |
| Finality by proof | A proof that two thirds of an epoch's voters signed a link, their own votes rather than their attesters', for ChainV4Finality.sol; and a proof of the header chain from genesis, for the apps. | [Proofs of finality](#proofs-of-finality) |

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
chainv4 prove run --node https://… --kind sp1-stark --out p.json   # or as the STARK, for the chain alone
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
  minted for each block a proof covers, and how Gateway.sol releases (`proof-or-federation`,
  `both` or `federation-only`, [Bridges](bridges.md#withdrawals-by-proof)):

  ```sh
  chainv4 send --key member.key vote-change --vote yes --change '{"proofs":{"kinds":["sp1-groth16"],"program":"0x…","reward_per_block":"1000000000000000","gateway":"proof-or-federation","finality_program":"0x…","headers_program":"0x…","reward_per_epoch":"0"}}'
  ```

  `finality_program`, `headers_program`, `reward_per_epoch` and `withdrawal_program` may be left
  out, as zeros: no finality, header-chain or withdrawal proofs, paying nothing.

  `kinds` is a set: the chain takes a proof of any kind listed (see [Proof kinds](#proof-kinds)).
  Until governance decides one, the chain takes no proofs. The settings are lockable like any other.
- **`Tx::Prove`** takes a proof of the range from the proven tip to a later checkpoint this chain
  recorded, of this genesis, by a kind governance named and its program. It moves the tip to the
  range's end and mints the sender `reward_per_block` for each block it covers. Each node checks it
  with the kind's verifier in `sp1-verifier`. For Groth16 and PLONK that's a few milliseconds of
  pairings; the STARK takes longer.
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
- **`proveLink(publicValues, proof)`:** a link by a finality proof, by the voters it holds, under
  the finality program; see [Proofs of finality](#proofs-of-finality).
- **Deploying:** `chainv4 bridge finality-deploy --sp1-verifier 0x… [--sp1-verifier 0x…] [--program 0x…]`
  starts proofs at the chain's proven tip, or at its trusted checkpoint while there is none. It
  takes a list of SP1's gateways, one for each wrapping (SP1 keeps Groth16's and PLONK's apart), and
  a proof stands if any of them takes it. Without one it takes no proofs. It never takes a STARK.

## Proofs of finality

A range proof says the blocks were right; which block is final still comes from the votes. Two
more proofs, each its own program, prove that from the votes themselves (`proofs::finality`):

| Proof | What it proves | Taken by | Program |
| --- | --- | --- | --- |
| Finality | Two thirds of an epoch's voters, by weight, signed the link from a source checkpoint to the target, the voters as the source's state fixed them; with the source's state root, the target's height and state root, and the next epoch's attesting set (`ProvenLink`, eleven words). | ChainV4Finality.sol's `proveLink`, beside the attesters' `submit` | `chainv4-finality-program`, governance's `finality_program` |
| Header chain | The chain finalized a checkpoint, every link from genesis to it signed by its epoch's voters as the checkpoint before fixed them (`ProvenHeaders`, eleven words). | The chain, as `Tx::ProveHeaders`; then the apps | `chainv4-headers-program`, governance's `headers_program` |

- **What a link checks:** the votes are for exactly that link, each signed by a voter of the target
  epoch, together at least the weight a link needs. The voters are read from a witness of the
  *source's* state, which fixed them two epochs ahead, checked against the source's state root,
  which the proof says. A witness of the target's state, checked against its header's root, which
  the votes vouch for as they name the target block, gives the next epoch's attesting set and
  governance's finality program (`check_link`).
- **The taker checks the source:** ChainV4Finality.sol takes a link only from a checkpoint it
  recorded with that very state root, by attestations or by proofs alike. Nothing is tracked from
  link to link, so the two paths take turns as they like, and with restaking moving the weights
  every epoch neither falls behind the other. With attestations off
  ([Turning Ethereum's side off](#turning-ethereums-side-off)) Ethereum still follows the chain, by
  proofs. A link may skip at most the epochs voters are fixed ahead.
- **The header chain from genesis:** each proof checks the previous one, the one the chain took
  last, with its kind's verifier inside the program, then goes on from its justified checkpoint
  and that checkpoint's state root link by link, at most `MAX_HEADER_LINKS` of them. The first
  starts from genesis and its state root. A proof takes only a previous one by its own program, so a new program starts again
  from genesis.
- **On chain:** `Tx::ProveHeaders`, from the `proofs` fork, takes a header-chain proof by
  governance's `headers_program` that finalizes a checkpoint on this chain later than the last one
  taken. The chain keeps it in state, where an app proves it, and mints `reward_per_epoch` for each
  epoch it adds. `GET /validity` gives it as `headers`.
- **Apps:** the wallet and explorer check the latest header-chain proof themselves, against the
  program governance names, and say how far the chain is finalized from genesis
  (`chainv4_ui::validity::headers_proven`).
- **Cost, measured:** a link runs in SP1 in about 7.8 million cycles, three links from genesis in
  12 million; signatures are most of it.

Proving them, by hand as ranges are:

```sh
chainv4 prove check --of finality --epoch 120 --node https://…     # the link to epoch 120, run off the node
chainv4 prove run --of finality --epoch 120 --out link.json --node https://…
chainv4 bridge … finality-prove 0x<ChainV4Finality.sol> --proof link.json   # proveLink
chainv4 prove run --of headers --out headers.json --node https://…  # on from the chain's last one
chainv4 send --key k.key prove --proof headers.json                 # Tx::ProveHeaders
chainv4 prove program --of finality                                 # its id, for governance
```

The node builds the inputs at `GET /validity/finality-input?epoch=` and
`GET /validity/headers-input?to=`, from history back to the links' sources.

The chain takes a header-chain proof only up to a checkpoint among the blocks it still records
(`RECORDED_BLOCKS`, from the `proofs` fork on). A chain older than one proof covers is caught up
off chain: each proof goes on from the last with `--previous headers.json`
(`GET /validity/headers-input?from=`), and only the one that reaches a recent checkpoint is sent.

## Withdrawals to Bitcoin

A range proof also commits to the Bitcoin gateway's withdrawals: a twelfth word, the keccak256
Merkle root of each `WithdrawRequested` the directory's Bitcoin gateway emitted in the range, in
order. Each leaf is the keccak256 of `chainv4 btc withdrawal`, the gateway, the withdrawal's id, its
script's length and script, and its satoshis (`btc_withdrawal_digest`). Anyone works a leaf out
from the event, whatever types Lua gave its fields (`btc_withdrawal_of`).

A **withdrawal proof** (`chainv4-withdrawal-program`, governance's `withdrawal_program`) proves
that a withdrawal was queued, with nothing taken on anyone's word:

- **a header-chain proof** finalizes a checkpoint from genesis;
- **a range proof** ends exactly there, so its blocks are that checkpoint's real ancestors;
- **the withdrawal's path** is in that range's Bitcoin withdrawals.

Both inner proofs are checked inside, by their kinds' verifiers under their programs. It's what a
BitVM operator shows for its reimbursement, and what watchers check (see
[Bridges](bridges.md#bitvm-beside-the-federation)).

```sh
chainv4 prove run --of withdrawal --headers-proof headers.json --range-proof range.json --withdrawal 7 --out w.json
```

`GET /validity/btc-withdrawals?from=&to=` lists a range's withdrawals with their leaves, which the
path is worked out from.

## Turning Ethereum's side off

Governance can stop Ethereum following the chain: `Change::Attestations` set to `false` ends the
validators' attestations from the next block, so ChainV4Finality.sol finalizes nothing new and
takes no more proofs. Set the gateway mode to `federation-only` first, by a proof that carries it:
governance refuses attestations off under `both`. The chain still takes proofs itself, STARKs
included. See [Finality](finality.md#turning-attestations-off).

## Light clients

The latest proof proves itself. An app asks any node for it (`GET /validity`) and checks it alone
(`TakenProof::check`), and the node's word adds nothing:

- the proof by the program governance names, which the app proves from state like any other value;
- its genesis against the app's own chain;
- its range's end against a header in the chain the app follows.

Then it knows the chain is proven up to there. Votes still carry it past that, to the newest
blocks.

## Proof kinds

A proof kind is how a proof is made and wrapped (`ProofKind`, with a pinned tag). Each one has its
own default-on feature, and `sp1` turns on all three:

| Kind | Feature | Proof | Trusted setup | Checked by |
| --- | --- | --- | --- | --- |
| `sp1-groth16` (0) | `sp1-groth16` | about 260 bytes | the circuit's own, from SP1's ceremony | the chain, the apps, Ethereum |
| `sp1-plonk` (1) | `sp1-plonk` | under a kilobyte | the universal one all PLONK circuits share | the chain, the apps, Ethereum |
| `sp1-stark` (2) | `sp1-stark` | about 1.27 MB, whatever the range | none | the chain and the apps only |

- **One program for all three.** The kind is only how SP1 wraps the proof, so the program id is the
  same for each. The STARK's verifier takes it as the eight field words it packs.
- **The chain takes a set.** `Change::Proofs` lists the kinds, and a proof of any of them is taken.
  To switch, vote the new kind in beside the old one, then the old one out. Then nobody is left
  holding a proof of a range that the chain has stopped taking.
- **Ethereum takes what SP1's gateways route.** Groth16 and PLONK each have a gateway contract of
  their own, and ChainV4Finality.sol asks each gateway in its list. A STARK can't go there: no EVM
  chain can afford to check one. `bridge finality-prove` refuses one, and a chain that proves only
  in STARKs leaves its Ethereum contracts on attestations.
- **Size:** `ProofKind::max_bytes` caps each kind in `Tx::Prove`: 4 KiB for the wrapped kinds and
  2 MiB for the STARK. While the chain takes STARK proofs and names no `max_tx_bytes`, the API takes
  bodies big enough for one hex-encoded. A chain that sets a `max_tx_bytes` must leave room for one.
- **Cost, measured:** a STARK proof of one block took 246 s and 26 GB of memory on an M-series Mac's
  CPU, and checking it took 42 ms.
- **Proving:** `chainv4 prove run --kind`. Groth16 and PLONK download SP1's circuit files the first
  time (6.2 GB for Groth16, 2.3 GB for PLONK, for SP1 6.10), and the STARK needs none.

## Choosing another zkVM

Nothing above is SP1's but the program crate, the SP1 features and the SP1 `ProofKind`s. A proof
system is a `ProofKind` and the `ProofSystem` that checks its proofs. A prover is a `Prover`. Another
zkVM is another kind and feature beside SP1's, and governance brings it in with `Change::Proofs`
beside the kinds in force.

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
  checked inside the zkVM by the same verifier. Checking a STARK in there costs far more cycles
  than checking a wrapped proof.
