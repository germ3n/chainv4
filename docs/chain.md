# Chain

`Chain` (`src/chain/mod.rs`) is the chain one node follows:
- every block it accepted, on the main chain or a side branch;
- which branch is the main chain;
- the rules blocks run under, all from genesis.

## Blocks

A block is a header, its transactions, the finality proofs it carries, a mining pool's commitment
when its producer runs a pool, and a seal.

| Header field | What it is |
| --- | --- |
| `parent` | The id of the block it extends. |
| `height` | Blocks before it; genesis is 0. |
| `timestamp` | When it was produced, in milliseconds since the Unix epoch. |
| `producer` | Who produced it. The reward and the producer's share of fees go here. |
| `difficulty` | The expected hashes to find its seal under proof of work; 0 under the authority. |
| `tx_root` | The Merkle root of its transactions, in order, so one is proven without the others (see [Receipts](#receipts)). The zero hash for a block without transactions. For genesis, the hash of the genesis configuration. |
| `proof_root` | The chain hash of its finality proofs, in order. |
| `pool_root` | The chain hash of its mining pool's commitment, or the zero hash for a block that carries none (see [Mining pools](consensus.md#mining-pools)). |
| `receipts_root` | The Merkle root of what its transactions did, in order (see [Receipts](#receipts)). The zero hash for a block without transactions. |
| `state_root` | The root of the state tree once it has run: every chain-state entry (see [The state root](#the-state-root)). |
| `gas_used` | The gas its transactions used; 0 without fees. |
| `base_price` | The base price of gas in it; 0 without fees. |

- **The id:** a block's id is the hash of its header. The header hashes itself with its parent's
  algorithm. Genesis's parent is the chain hash's zero hash, so every id on a chain uses the chain
  hash, and blocks from a chain with another hash never connect.
- **The seal** is consensus's proof that the producer was allowed to produce. It is a signature
  under the authority, or a nonce under proof of work (see [Consensus](consensus.md)). It is not
  part of the id.

## Transactions

A `SignedTx` is:
- a `Tx`;
- the sender's nonce;
- a fee offer;
- the signer's public key;
- a signature over the chain's genesis id, the first three and the key.

The sender is the signer's address. A transaction's id is its whole encoding, signature
included, hashed with the chain hash. The genesis id is signed, not sent: a transaction signed for
one chain fails its signature on any other, so it cannot be sent again on a new chain of the same
keys, and its id is that chain's alone. `SignedTx::verify_on(genesis)` is the only check of a
signature, and takes the chain.

| Transaction | Does |
| --- | --- |
| `Transfer` | Moves an amount from the sender to an address. |
| `Mint` | Creates supply for an address. Only the genesis minter may, and only the development key can be one: a faucet for development chains. |
| `Burn` | Destroys supply the sender holds. |
| `ContractCall` | Calls a contract function by its XXH3-64 selector, with arguments and value. |
| `Deploy` | Deploys Luau source or a WebAssembly module, at an address from the sender and nonce. |
| `Upgrade` | Replaces a contract's code, if its `check_perms` says the sender may. |
| `Remove` | Removes a Lua or WebAssembly contract for good, its balance to a beneficiary, if its `check_perms` says the sender may (see [Contracts](contracts.md#lifecycle-hooks)). |
| `Bond`, `Unbond`, `Withdraw` | FFG staking (see [Finality](finality.md)). |
| `Slash` | Reports two conflicting votes, under FFG. |
| `ApproveSigners` | Approves a new set of authority finality signers. |
| `WithdrawApproval` | Takes back the sender's approval of a new set of signers. |

**Validity checks:**
- **Always:** a transfer or burn may only take from the sender, and amounts must not be zero.
- **Against chain state, when it runs:** whether its nonce is the next one, and whether its fee
  offer can pay.

## Importing a block

`Chain::import` checks a block against its parent before it stores anything:
1. that it is not known already;
2. that it is not at or below the finalized checkpoint (`ConflictsWithFinality`), before anything
   is fetched for it, so a node never asks for a branch under its finality;
3. that its parent is known (`UnknownParent`, which makes the node ask for the gap), and did not
   fail to run;
4. its height, `tx_root`, `proof_root` and `pool_root`, and that its pool commitment sends at most
   the whole reward (its `receipts_root` and `state_root` are checked once it runs);
5. its base price, which its parent's base price and gas used decide;
6. its seal, producer, timing and difficulty, by consensus.

Then:
- **On the tip,** it runs and becomes the new tip, in one commit (`Imported::Extended`, with the
  receipts).
- **On a side branch,** it is stored without running (`Imported::Side`).
- **On a side branch that now holds more total work than the main chain,** the chain reorganizes
  to it (`Imported::Reorganized`, with the blocks taken off and put on).

A block that fails to run is remembered as bad, with everything built on it, so the same branch
is never tried twice. The last `MAX_BAD_BLOCKS` (1,024) of them are kept in `Meta`, so a restart
refuses them too, without running them again.

## Running a block

`apply` runs a block on a state layer over the parent's state. One block is one batch: either
all of it commits or none of it does.

1. Each transaction runs in order (see below), on every core (see
   [Running transactions side by side](#running-transactions-side-by-side)). Under fees, a
   transaction whose gas limit is more than the block has left makes the block invalid
   (`BlockFull`).
2. The gas they used must equal the header's `gas_used`.
3. The producer gets the block reward, `reward.at(height)`. A pool's block then sends its
   commitment's parts of the reward and of the fees its producer earned to the pool's escrow, and
   keeps the commitment's root under its height in `Pools` (`Pool` when the producer no longer
   holds them).
4. Finality checks the proofs the block carries and pays their voters, and at a checkpoint updates
   what it keeps in state, such as the next validator set.
5. The root of what its transactions did must equal the header's `receipts_root`
   (`ReceiptsRoot`).
6. The state tree follows everything the block changed, and its root must equal the header's
   `state_root` (`StateRoot`).
7. Each transaction's receipt is stored by its id, the block becomes the tip at its height, and an
   undo record keeps every value the block replaced, the state tree's nodes included.

## The state root

Every header commits to the state the chain is in once the block has run. That is what lets a
snapshot be checked, a light client trust a value with a proof, and later ZK work prove
transitions.

**What it covers:** every column of chain state: `Accounts`, `Contracts`, `Storage`,
`Namespaces`, `Validators`, `Epochs`, `Tokens` and `Names` (`STATE_COLUMNS`). Blocks, receipts, the event
index and what finality learned from the network are not chain state, and are left out.

**The tree:** a sparse Merkle tree (`src/db/tree.rs`), binary over 256-bit paths.
- **Paths:** an entry's path is the chain hash of `"chainv4 state path"`, its column's tag and its
  key, so entries scatter evenly whatever their keys.
- **Hashes:** a leaf is the hash of `0`, its path and its value's hash; a branch the hash of `1`
  and its two halves'. An empty half is the zero hash.
- **Canonical:** an empty subtree has no node, a subtree of one entry is that entry's leaf, as
  high up as it can sit, and anything more is a branch. So the root depends on the entries alone,
  not the order they came in, and `root_of` works it out from a list of entries.
- **Kept incrementally:** the nodes are stored by depth and path in the `StateTree` column. Each
  block updates only the nodes on the paths it changed, about log₂ n per entry, in the same layer
  as everything else it did. So a block that is thrown away throws its nodes away too, and a
  reorg's undo records put them back.

**Proofs:** `StateTree::prove` gives the hashes beside an entry's path, and the leaf where the path
ends. `StateProof::verify` checks it against a root: that the entry holds a value, or that it
holds nothing, when the path ends empty or at another entry's leaf. The API serves them at
`GET /proofs/{column}/{key}` (see [HTTP API](api.md#get-proofscolumnkey)).

**Checking a database:** `stored_root` reads every chain-state entry back from a database and works
out the root from them alone, without the tree. Tests check it against every tip, reorgs
included.

**Leaves name their entries:** each leaf keeps its entry's column and key besides its path, so the
tree can say which entries any subtree holds. That is what a node serving a snapshot sends, a
subtree at a time (see [Node](node.md#syncing-from-a-snapshot)).

**State at an older block:** `stored_state_at` rolls the main chain back by its undo records, on a
throwaway layer, to a block within the last `max_reorg_depth`, for proofs and snapshots against
that block's root.

## Running a transaction

`execute` (`src/chain/execute.rs`) runs one transaction on its block's layer.
1. It checks the signature, then the nonce. A wrong nonce makes the whole block invalid.
2. Under fees, it checks that the offer can pay (see [Fees](fees.md)). Then it bumps the nonce
   and reserves the fee.
3. It runs the transaction on a layer of its own:
   - **On success,** the layer merges into the block.
   - **On failure,** the layer is dropped. The transaction still counts: it stays in its block, its
     nonce is used up, and it pays its fee.
4. Under fees, it settles the fee: it refunds what was not used. The block loop then pays the
   producer its share, right after the transaction, and the rest is burned.

The outcome is an `Effect` on success or a `Failure`.

| Effect | Failure |
| --- | --- |
| `Done`: a transfer, mint, burn or staking transaction | `Registry`: for example a lack of balance |
| `Deployed(address)` | `Contract`: the contract failed, ran out of fuel or memory |
| `Upgraded(address)` | `NotMinter`, `EngineNotAllowed` |
| `Returned(values)`: a call's return values | `NoStaking`, `NoSigners`, `BadEvidence` |

**Database failures:** a database failure in the middle of a transaction is the node's fault,
not the transaction's. It stops the import instead of being recorded as a failure, even when a
contract caught the error.

## Batches

`Tx::Batch` is several steps under one signature, one nonce and one fee offer, all or nothing: any
step failing undoes every step, and the fee is charged as for any failed transaction. The nonce is
used and the fee paid either way.

**What it may hold:** a transfer, contract call, EVM call, deploy, upgrade, removal, name, primary name, burn
or claim. Anything else makes the whole transaction invalid before it runs, in the mempool and in a
block alike: validators' and governance's transactions, a multisig's administration, a mint, an
Ethereum-signed transaction, which has a signature and a nonce of its own, a multisig's own
transaction, and another batch. So does a batch with no steps or more than 32, and a transfer or burn
taking from anyone but the batch's sender. A multisig may send a batch through `Tx::Multisig`, never
the reverse (see [Multisig](multisig.md#a-batch)).

**How it runs.** All the steps run in order on one layer, kept only if all of them go through, so
each sees everything those before it did, across engines: an approval, then the swap that spends it.

- **Fuel is shared.** Each step is given what those before it left of the transaction's limit, never
  an allowance of its own, and EVM gas is drawn from the same fuel. The limit is `tx_limits.fuel`,
  and the gas limit's room above the intrinsic cost as for any transaction. A step that runs out has
  used all the batch was allowed.
- **Memory is the same limit at every moment.** What a step's engine held is gone before the next
  starts, so no moment holds more than `tx_limits.memory`.
- **Events and hops are shared too.** The steps emit at most 256 events together, as one call may, and
  a trace keeps its 256 hops. Past those the step that went over fails the batch.
- **Each step is its own transaction to the EVM.** Transient storage, what is warm, gas refunds and
  what `SELFDESTRUCT` counts as new all start afresh with each step: a contract deployed in one step
  and destroyed in the next stays (see [EVM](evm.md#in-a-batch)).

**Fees.** Its intrinsic gas is `per_tx` for each step, `per_byte` for its whole encoding, and
`per_code_byte` for each deploy's and upgrade's code, as those steps would cost sent apart or more,
so a batch never saves what a block is limited by. Against a block's gas and byte limits it counts
whole. A contract scorer sees it as the kind `batch`, whenever it names `batch` or the kind of any
step, so naming `transfer` still weighs a batch holding one.

**Deploys.** A deploy's address comes from its deployer and a nonce, and a batch has one nonce, so
each deploy in it is made at its own: `batch_deploy_nonce(nonce, before)`, by how many deploys of
either engine came before it in the batch. The first's is the batch's own nonce, so one deploy lands
where it would alone. Each later one's has the top bit set, then the batch's nonce, then the count, a
number no account's nonce reaches, so it never meets a later transaction's. The address follows as
from any nonce, `contract_address` or `create_address`, so anyone can work it out before signing, and
the steps after a deploy can call it. `chainv4 send batch` prints each, and the wallet shows each on
its step. Under `--multisig`, the CLI prints none in the bundle or as members sign it, only each step's
contract once it ran. A contract's own `CREATE` goes on from its own nonce, step to step.

**What the receipt says.** A batch that went through has status `batch`, with what each step did and
how many of the receipt's events and payments are its; those lists stay flat and in order, so the
event index and everything reading them work as they do for anything else. One that failed says which
step stopped it, counted from 0, as the API's `failed_step`, and why; the CLI's messages count steps from
1, so its `step 8 failed` is `failed_step` 7. Which step is committed to the receipts root
(`CommittedOutcome::Batch`, `BatchFailed`), so a light client proves it; why is not, as no failure's
message is. As with any failure, it keeps no events or payments, and keeps its trace.

**From the command line,** `chainv4 send batch` takes `--file` with a JSON list of steps, then any
number of `--step`, each an object with a `type` and that send command's fields, under the usual
`--key`, `--gas-limit` and so on, and `--multisig`:

```bash
chainv4 send batch \
  --step '{"type":"transfer","to":"ed25519:0x…","amount":"5"}' \
  --step '{"type":"call","to":"contract:0x…","function":"approve","args":["address:ed25519:0x…","u256:9"]}' \
  --step '{"type":"evm_call","to":"0x…","function":"swap(uint256)","args":["9"],"value":"0"}' \
  --step '{"type":"deploy","lua":"token.lua"}' \
  --step '{"type":"upgrade","contract":"contract:0x…","wasm":"new.wasm"}' \
  --step '{"type":"burn","amount":"1"}' \
  --step '{"type":"name","action":"register","name":"alice","years":1}' \
  --step '{"type":"primary_name","name":"alice"}' \
  --step '{"type":"claim","height":9,"to":"ed25519:0x…","earned":"100","index":2,"path":["0x…"]}'
```

A `deploy` takes `lua`, `wasm`, or `evm` with `constructor` and `args`. An `evm_call` takes `function`
and `args`, or `data`. A name's `action` is `register`, `renew`, `transfer`, `target`, `text` or
`subname`, with that command's fields. A field nothing asks for is an error, not a default.

**EIP-712.** A batch signed as typed data shows its signer every step: its `Batch` form writes each
transfer, call and EVM call out, a call by its function's name, which travels beside the signature
(see [Typed data](crypto.md#typed-data-eip-712)), and anything else as its kind and bytes;
`--eip712` signs one. A multisig's batch, carried or approved, has no such form, and
`SignedTx::verify_on` refuses one under `Signature::Secp256k1Eip712`; the same key signs it the
chain's own way.

## Running transactions side by side

A block's transactions run on every core (`src/chain/parallel.rs`), with exactly the result
running them one by one gives.

1. **Waits:** each transaction waits for the last one before it in the block that names something
   it names. That is its sender, its recipient or contract, a deploy's address, the staking
   records, or the signer records.
2. **Rounds:** each round runs every transaction whose wait is over, side by side, each on a layer
   of its own over what the block holds so far. The layer records every key the transaction reads
   that it did not write itself, and every range of keys it scanned, from where to where it
   stopped. The first round also checks every signature in the block at once,
   most of what a simple transaction costs.
3. **In order:** results then go in in block order, for as long as they hold. A result holds if
   no transaction that went in after it ran wrote anything it read, nor any key in a range it
   scanned, one that was not there before included. What it read is then what it
   would have read in its turn, so its result is the one it would have had. Each one that goes in
   ends the wait of those waiting for it, and they run in the next round.
4. **Again:** what a transaction names is only a guess at what it touches, since a contract can
   reach accounts and contracts nobody named. A transaction whose result no longer holds runs
   again in the next round, on what the block holds by then. These are the only ones that run
   twice (`Ran.again`).

**What it means for blocks:**
- **Nothing to conflict:** transactions that name nothing in common, such as transfers between
  different accounts, all run in the first round.
- **Dependents:** a sender's transactions, or calls to one contract, run one at a time, as they
  must, and each runs once. Everything beside them runs alongside.
- **Contracts reaching further:** a call that reads an account or contract no transaction named,
  after an earlier transaction wrote it, runs twice.
- **Fees:** every transaction pays the producer, so the block loop credits the producer's share
  right after each transaction, rather than the transaction doing it. The order and the result
  are the same, but the producer's balance is no longer something every transaction reads. A
  transaction that names the producer, such as a transfer to it, waits for the one right before
  it.

**What it means for consensus:** none of this is consensus; it only changes how fast a block
runs. On 12 cores, a block of 1,000 transfers runs about five times faster, and none of them
runs twice:

| Block | One by one | Side by side |
| --- | --- | --- |
| Different senders and recipients | 43 ms | 9 ms |
| All from one sender | 41 ms | 8 ms |
| Half from one sender | 42 ms | 8 ms |

One sender's transfers run one at a time, but their signatures still check side by side in the
first round, and that is most of what they cost.

The threads are a rayon pool the `Chain` owns (`Workers`), started the first time a block has more
than one transaction. Finality uses them to check the signatures in a block's proofs and in a
peer's justification all at once.

## Building a block

`Chain::build` makes the next block on the tip for a producer to seal.
1. It runs the transactions it is given on a layer that is then thrown away.
2. It keeps those that can run and fit the block gas limit, and leaves out the rest. For example,
   a nonce already used, or an offer that cannot pay.
3. It records the gas they used and the base price.
4. It credits the reward and settles the finality proofs too, and records the receipts root and
   the state root they reach.

A producer runs every block twice: once to build it, once to import it.

`Chain::build_within` builds the same way within a time budget: once its deadline passes, the
transactions that went in by then are the block, and the rest are left out unrun. The block is
still the one running those transactions in order gives, since results only ever go in in block
order. A node sets the budget with `--max-build-ms`.

`Chain::build_on` builds on any stored block instead, the tip or one on a side branch, in the
state at the end of that block. It works that state out on a throwaway layer the way a reorg
would: it rolls the main chain back to where the branch splits off, then runs the branch.

**Headers** of every block are also stored on their own (`Headers`), which is all a node that
synced from a snapshot has for the blocks before its pivot. Finality reads headers alone, so it
checks old checkpoints on such a node too.

## Fork choice

The main chain is the branch with the most total work. Under the authority every block weighs
1; under proof of work, its difficulty. Finality narrows it:

- **Finalized:** nothing at or below the finalized checkpoint ever comes off. Blocks there that
  are not on the main chain are refused outright.
- **Justified:** work alone only moves the chain to a branch that holds the latest justified
  checkpoint.
- **Newer justified branch:** the chain moves to it even when it holds less work, as long as it
  keeps the finalized checkpoint.
- **Depth:** no reorg by work reaches back more than `max_reorg_depth` blocks, however much work
  the other branch holds. One to a justified branch reaches down to the finalized checkpoint,
  however deep: a node that mined on its own while cut off from the validators follows the branch
  they justified back, as far as it went.

## Reorganizing

To follow a heavier branch, the chain:
1. walks the branch back to where it meets the main chain;
2. takes the main chain's blocks off, newest first, by writing back the values each one replaced
   (its undo record);
3. runs the branch's blocks on top.

It all happens in one commit, so a crash never leaves state from a block the main chain does not
have. If a block of the branch fails to run, the reorg is dropped, and that block and everything
after it on the branch are remembered as bad.

Undo records are kept for the last `max_reorg_depth` blocks, and on a chain with finality for
every block above the finalized checkpoint too: a justified branch may roll back that far. Older
ones are deleted; while finality stalls they are kept, and go together once it moves on.

**Leaves:** the chain also keeps every leaf block it stores (`tips`), so a justification for a
side branch can move the chain to the best block on it. The leaves go into the database with
each block (`Meta`), so they outlast a restart, and the API's `/forks` finds side branches from
them.

**Pruning side branches:** whenever the tip moves, the chain deletes every side branch it can
never follow: one that splits off below the finalized checkpoint or the snapshot the node took
state at, and, on a chain without finality, more than `max_reorg_depth` blocks below the tip. Its blocks, headers and work
go, from the leaf down to the fork, and the leaf leaves `tips`. So what side blocks a node stores
is bounded by the reorg window, and each of them cost its producer a seal at the main chain's
difficulty: filling a disk with side blocks costs as much as mining the main chain.

## Receipts

Every transaction on the main chain has a receipt, stored by its id (`TxReceipt`). It records:
- the block it landed in, and its place in that block;
- the fuel its contracts burned;
- the gas it used, and the fee it paid;
- its status: what it did, or why it failed;
- the events its contracts emitted, in order;
- its payments: the coin its contracts paid, in the order they paid it (see below);
- its trace: every call, delegate and transfer its contracts made (see
  [Contracts](contracts.md#traces)).

A failed transaction keeps only its error message and its trace, and emits nothing. A reorg moves
receipts with everything else.

**The receipts root:** each header commits to what its transactions did as a Merkle root, built the
way RFC 6962 (Certificate Transparency) builds one (`crypto::merkle`):
- **Leaves:** one per transaction, in order: the hash of `0` and its `CommittedReceipt`, which is
  its id, fuel burned, gas used, fee, outcome, events and payments.
- **Not committed:** a failure's message and the trace. Neither is consensus, so their wording and
  depth can change without splitting the chain. The outcome only says that it failed.
- **The tree:** a node is the hash of `1` and its two halves', and a list splits at the largest
  power of two below its length, so no leaf is repeated to fill a level.

**The transactions** are a Merkle root the same way: `tx_root` is the root over each signed
transaction's leaf (`SignedTx::leaf`, its whole encoding, signature included), in order. So a
receipt says what a transaction did, and its leaf at the same place in `tx_root` says who sent
what, which the receipt does not.

**Payments:** every coin movement a contract made while the transaction ran, that stuck, each
from, to and amount, in the order they ran:
- **Native:** what a contract sent with `accounts.transfer` or the SDK's `transfer`, and the value
  it sent with a call, `evm.call` included, before what the callee paid on.
- **EVM:** every frame below the top that sends value (`CALL`), creates a contract with value, or
  pays its balance on with `SELFDESTRUCT`, each address told as the account it is: an alias as its
  native account. What an EVM contract sends a native one through the native precompile is
  recorded where the native call ran.
- **Not payments:** the transaction's own value or transfer, which the transaction itself says,
  and anything a failed call or a reverted frame sent, which it took back.

So a wallet proves what a contract paid it as it proves a receipt, line by line, and the address
index lists the transaction under every contract that paid and everyone paid.

`GET /proofs/receipts/{id}` serves both paths: that the transaction is in its block, who sent
what, and what it did. A light client checks them with `check_tx` and `check_receipt`.

A node started with `--index-events` also indexes every event by its contract, by its contract
and name, by the addresses in its data, and for an EVM log by its topics, in the same commit; one started with
`--index-addresses` indexes every transaction under each address it touches. Blocks from before
an index was turned on are read back from the stored receipts (see
[Storage](storage.md#the-indexes)). The API lists them at `GET /events` and
`GET /accounts/{address}/txs`.

## Rewards

There is no coinbase transaction. After a block's transactions run, the chain credits
`reward.at(height)` to the header's producer:
- the reward is `reward.initial`;
- with a `halving_interval`, it halves every that many blocks until it reaches zero.

A reorg takes the reward back with the rest of the block. A reward can be spent from the next
block on, with no maturity period.

## Supply

Every coin made and every coin destroyed is counted in state, as `Supply` under `coin:supply` in
the validators column, so a light client proves the supply as it proves a balance:
- **Made:** genesis's allocations and its validators' bonds, each block's reward, vote rewards a
  chain mints, and `Tx::Mint`.
- **Destroyed:** fees' burned share, where no reward pool takes it, `Tx::Burn`, what names cost,
  slashing past the reporter's share, and the inactivity leak.

Minted less burned is what every balance and every bond add up to: tests check it after fees,
rewards, names, minting, slashing, the leak, mining pools, both DEXes and a reorg. The counters
are written with the block, and nothing is written for nothing made or destroyed. They moved
every chain's genesis state root, so chains from before start again from a new genesis.
`GET /supply` serves them; see [HTTP API](api.md#get-supply).

## Limits

| Limit | Where | What it bounds |
| --- | --- | --- |
| `tx_limits.fuel` | genesis | The fuel one deploy, upgrade or call may burn, calls it makes included. |
| `tx_limits.memory` | genesis | The memory each contract in a chain of calls may use. |
| `block_gas_limit` | genesis fees | The gas one block may use. |
| `max_reorg_depth` | genesis | How far back the main chain may roll. |
| `MAX_DRIFT_MS` | 15 seconds | How far ahead of a node's clock a block's timestamp may be. |
| `max_block_bytes` | genesis | The bytes one block's transactions may take, encoded; checked on import. |
| `--max-block-txs` | node flag | The transactions in a block this node builds, if it caps them. Not checked on import. |
