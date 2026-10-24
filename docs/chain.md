# Chain

`Chain` (`src/chain/mod.rs`) is the chain one node follows:
- every block it accepted, on the main chain or a side branch;
- which branch is the main chain;
- the rules blocks run under, all from genesis.

## Blocks

A block is a header, its transactions, the finality proofs it carries, and a seal.

| Header field | What it is |
| --- | --- |
| `parent` | The id of the block it extends. |
| `height` | Blocks before it; genesis is 0. |
| `timestamp` | When it was produced, in milliseconds since the Unix epoch. |
| `producer` | Who produced it. The reward and the producer's share of fees go here. |
| `difficulty` | The expected hashes to find its seal under proof of work; 0 under the authority. |
| `tx_root` | The chain hash of its transactions, in order. For genesis, the hash of the genesis configuration. |
| `proof_root` | The chain hash of its finality proofs, in order. |
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
- a signature over the first three and the key.

The sender is the signer's address. A transaction's id is its whole encoding, signature
included, hashed with the chain hash.

| Transaction | Does |
| --- | --- |
| `Transfer` | Moves an amount from the sender to an address. |
| `Mint` | Creates supply for an address. Only the genesis minter may. |
| `Burn` | Destroys supply the sender holds. |
| `ContractCall` | Calls a contract function by its XXH3-64 selector, with arguments and value. |
| `Deploy` | Deploys Luau source or a WebAssembly module, at an address from the sender and nonce. |
| `Upgrade` | Replaces a contract's code, if its `check_perms` says the sender may. |
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
1. that it is not known already, and does not build on a block that failed to run;
2. that its parent is known (`UnknownParent`, which makes the node ask for the gap);
3. its height, `tx_root` and `proof_root`;
4. its base price, which its parent's base price and gas used decide;
5. that it is not at or below the finalized checkpoint (`ConflictsWithFinality`);
6. its seal, producer, timing and difficulty, by consensus.

Then:
- **On the tip,** it runs and becomes the new tip, in one commit (`Imported::Extended`, with the
  receipts).
- **On a side branch,** it is stored without running (`Imported::Side`).
- **On a side branch that now holds more total work than the main chain,** the chain reorganizes
  to it (`Imported::Reorganized`, with the blocks taken off and put on).

A block that fails to run is remembered as bad, with everything built on it, so the same branch
is never tried twice. That memory lasts until the node restarts.

## Running a block

`apply` runs a block on a state layer over the parent's state. One block is one batch: either
all of it commits or none of it does.

1. Each transaction runs in order (see below), on every core (see
   [Running transactions side by side](#running-transactions-side-by-side)). Under fees, a
   transaction whose gas limit is more than the block has left makes the block invalid
   (`BlockFull`).
2. The gas they used must equal the header's `gas_used`.
3. The producer gets the block reward, `reward.at(height)`.
4. Finality checks the proofs the block carries and pays their voters, and at a checkpoint updates
   what it keeps in state, such as the next validator set.
5. Each transaction's receipt is stored by its id, the block becomes the tip at its height, and an
   undo record keeps every value the block replaced.

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

## Running transactions side by side

A block's transactions run on every core (`src/chain/parallel.rs`), with exactly the result
running them one by one gives.

1. **Waits:** each transaction waits for the last one before it in the block that names something
   it names. That is its sender, its recipient or contract, a deploy's address, the staking
   records, or the signer records.
2. **Rounds:** each round runs every transaction whose wait is over, side by side, each on a layer
   of its own over what the block holds so far. The layer records every key the transaction reads
   that it did not write itself. The first round also checks every signature in the block at once,
   most of what a simple transaction costs.
3. **In order:** results then go in in block order, for as long as they hold. A result holds if
   no transaction that went in after it ran wrote anything it read. What it read is then what it
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

A producer runs every block twice: once to build it, once to import it.

## Fork choice

The main chain is the branch with the most total work. Under the authority every block weighs
1; under proof of work, its difficulty. Finality narrows it:

- **Finalized:** nothing at or below the finalized checkpoint ever comes off. Blocks there that
  are not on the main chain are refused outright.
- **Justified:** work alone only moves the chain to a branch that holds the latest justified
  checkpoint.
- **Newer justified branch:** the chain moves to it even when it holds less work, as long as it
  keeps the finalized checkpoint.
- **Depth:** no reorg reaches back more than `max_reorg_depth` blocks, however much work the
  other branch holds.

## Reorganizing

To follow a heavier branch, the chain:
1. walks the branch back to where it meets the main chain;
2. takes the main chain's blocks off, newest first, by writing back the values each one replaced
   (its undo record);
3. runs the branch's blocks on top.

It all happens in one commit, so a crash never leaves state from a block the main chain does not
have. If a block of the branch fails to run, the reorg is dropped, and that block and everything
after it on the branch are remembered as bad.

Undo records are kept for the last `max_reorg_depth` blocks; older ones are deleted, since
nothing can roll back that far.

**Leaves:** the chain also keeps every leaf block it stores (`tips`), so a justification for a
side branch can move the chain to the best block on it. The leaves go into the database with
each block (`Meta`), so they outlast a restart, and the API's `/forks` finds side branches from
them.

## Receipts

Every transaction on the main chain has a receipt, stored by its id (`TxReceipt`). It records:
- the block it landed in, and its place in that block;
- the fuel its contracts burned;
- the gas it used, and the fee it paid;
- its status: what it did, or why it failed;
- the events its contracts emitted, in order;
- its trace: every call, delegate and transfer its contracts made (see
  [Contracts](contracts.md#traces)).

A failed transaction keeps only its error message and its trace, and emits nothing. A reorg moves
receipts with everything else.

A node started with `--index-events` also indexes every event by its contract, by its contract
and name, and by the addresses in its data, in the same commit (see
[Storage](storage.md#the-event-index)). The API lists them at `GET /events`.

## Rewards

There is no coinbase transaction. After a block's transactions run, the chain credits
`reward.at(height)` to the header's producer:
- the reward is `reward.initial`;
- with a `halving_interval`, it halves every that many blocks until it reaches zero.

A reorg takes the reward back with the rest of the block. A reward can be spent from the next
block on, with no maturity period.

## Limits

| Limit | Where | What it bounds |
| --- | --- | --- |
| `tx_limits.fuel` | genesis | The fuel one deploy, upgrade or call may burn, calls it makes included. |
| `tx_limits.memory` | genesis | The memory each contract in a chain of calls may use. |
| `block_gas_limit` | genesis fees | The gas one block may use. |
| `max_reorg_depth` | genesis | How far back the main chain may roll. |
| `MAX_DRIFT_MS` | 15 seconds | How far ahead of a node's clock a block's timestamp may be. |
| `MAX_BLOCK_TXS` | 1,000 | The transactions a node puts in a block it builds. Not checked on import. |
