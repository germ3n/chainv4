# Finality

Finality is when a block can no longer come off the chain, whatever the work on another branch.
Genesis picks one of three kinds as `finality.type`, each behind a cargo feature of its own, all
on by default:

| Type | Final when | Who | Feature |
| --- | --- | --- | --- |
| `ffg` | Two thirds of the bonded weight vote for it, as in Casper FFG. | Validators who bond on chain, and can be slashed. | `finality-ffg` |
| `authority` | A threshold of signers vote for it, the same way. | Keys genesis names, changed by their own approval. | `finality-authority` |
| `depth` | `depth` blocks are on top of it. | Nobody votes. | `finality-depth` |

`finality: null` gives a chain no finality: only work and `max_reorg_depth` decide.

## How the chain uses it

The chain only talks to finality through the `Finality` trait (`src/chain/finality/mod.rs`).
`FinalityConfig::build` makes one from genesis.
- **Consensus:** `apply` runs on every block, checking the proofs it carries and updating state
  at checkpoints, and every node must agree on it.
- **Node-local:** the rest is what this node has learned, from votes or from its own main chain.
  - `finalized` and `justified` are the two anchors fork choice uses.
  - `tip_moved` runs after every import that moves the tip.
  - The vote methods take votes and justifications. Kinds without votes keep the defaults, which
    take none.

Whatever the kind:
- the chain never adds a block at or below the finalized checkpoint, nor reorganizes past it;
- work alone only moves the chain to a branch that holds the justified checkpoint.

A new kind is a new implementation of the trait.

## Checkpoints, votes and justification

The two kinds that vote share one engine (`src/chain/finality/votes.rs`). They differ only in
their electorate: who votes, with what weight, and how much it takes.

- **Epochs and checkpoints:** every `checkpoint_interval` blocks starts an epoch, and the block at
  its first height is its checkpoint. Genesis is epoch 0's checkpoint, justified and final from
  the start.
- **A vote** is a link from a source checkpoint to a later target checkpoint, signed by a voter
  over `"chainv4 vote"`, the genesis id and the vote, so it cannot be replayed on another chain.
- **Justification:** once the voters for one link add up to enough of the target epoch's
  weight, the target is justified. That is two thirds of the bonded weight under FFG, or
  `threshold` signers under authority finality.
- **Finalization:** justifying a checkpoint whose source is the epoch right before it finalizes
  the source.
- **What a justification is:** the link and the votes behind it, sorted by voter. It is stored,
  served to peers, and can be carried in blocks.

### When validators vote

A validating node votes `vote_depth` blocks after each checkpoint, so it rarely votes for a block
about to lose a fork. Its vote:
- has the latest justified checkpoint as its source;
- has the newest checkpoint that deep as its target;
- is only signed when this node's key is a voter in the target's epoch.

It remembers every vote it signs in its database before the vote goes out, so it never signs
one that conflicts with an earlier one, even across restarts. Its target epochs only ever rise,
and its source is never older than one it used before.

**Run each voting key on one node only.** Two nodes voting with the same key keep separate
records, can sign conflicting votes, and under FFG get the key slashed.

### Votes it cannot place yet

A vote whose checkpoints are not on this node's main chain yet, or whose source is not
justified here yet, is held back and tried again after every import. Up to 4,096 are held.

### What a node keeps

Every vote a node counts or holds back is stored in the Finality column, by target epoch and
voter. When the chain opens they are counted again, so a restart forgets nothing:
- the partial tallies;
- the held votes;
- the votes double-vote detection compares against.

Counted votes stay 64 epochs behind the finalized checkpoint, to catch one vote surrounding an
older one, and then they go.

### Voter sets agree across forks

Each epoch's voters are fixed in chain state at a checkpoint, `activation_delay` epochs ahead. A
branch that split off after the checkpoint that fixed a set agrees with the main chain on it.
So a vote for a checkpoint on a side branch only counts when that branch forked late enough.

### Cheating

Two votes by one key conflict when:
- they target the same epoch but differ;
- or one surrounds the other: an earlier source and a later target.

A node compares each vote against its voter's counted votes. Once a vote can be placed, which
shows its key really votes, the node also compares it against that voter's held votes.

A conflict is evidence. It is:
- logged, with the command to report it under FFG;
- kept in a log of the last 64;
- listed at `GET /evidence`.

Votes are only remembered once counted, so keys that do not vote cannot grow a node's memory or
push real evidence out of the log.

### Rewards

When `vote_reward` is above zero, a producer puts justifications whose voters have not been paid
yet into its block, at most `max_proofs_per_block`. Every voter in a proof gets `vote_reward`, and
each epoch pays once. A proof that does not check out makes the block invalid. With no reward,
blocks carry no proofs.

**Where rewards come from** is genesis's `vote_reward_source`:
- `mint`, the default: every reward is new supply.
- `fees`: the burned share of every fee (`burn_permille` of its base part) goes to the reward
  pool, `REWARD_POOL`, a contract address no contract lives at, instead of vanishing, and voters
  are paid from it. A proof pays its voters in order of key, each as much of `vote_reward` as the
  pool still holds, so a dry pool pays nothing and mints nothing. Supply then grows by block
  rewards alone. `GET /status` shows the source and the pool's balance under `vote_rewards`.

### Learning from peers

A node behind on justification asks a peer that is ahead for the justifications it lacks, once
no peer holds more work (see [Node and network](node.md)). It checks each one before it counts:
- its source is justified here;
- its source is on its target's branch;
- every vote is valid and by a voter;
- together they are enough.

A justified checkpoint on another branch moves the main chain to the best known block through
it, never past the finalized checkpoint.

## FFG

Validators bond on chain and vote with a key of their own; two thirds of the bonded weight
justify.

- **Bond** (`chainv4 send bond --voter KEY --amount N`): locks at least `min_bond` from the sender,
  for a voting key. Only the bonding account may unbond or withdraw it; the voting key only signs
  votes, so it can stay on the validating node while the account's key stays offline. A
  validator bonded in epoch k votes from epoch k + `activation_delay` + 1, weighted by its bond.
- **Unbond:** stops it voting after `activation_delay` epochs. Its bond stays locked, and
  slashable, for `withdrawal_delay` epochs more.
- **Withdraw:** pays the bond back to the account once it unlocks, and forgets the validator.
- **Slash** (`chainv4 send slash --first 0x… --second 0x…`): anyone may report two conflicting
  votes, as `GET /evidence` encodes them.
  - The validator's whole bond is gone at once.
  - The reporter gets 1/`reporter_divisor` of it, and the rest is burned.
  - The validator leaves the sets already fixed for the epochs from two after the slash on, at
    once. It keeps its weight in the slash's epoch and the next, whose sets stay as they were, so
    the next checkpoint's light update, which re-states the sets ahead, reaches light clients by
    the usual path (see [Light client](light-client.md#following-the-chain)). Sets fixed after
    the slash never hold it.
- **Limits:** at most `max_validators` are registered at once.
- **Genesis validators** are bonded from the start. Their bonds are minted on top of the
  allocations, not taken from them.
- **The inactivity leak** (`leak`, with `after_epochs` and `permille`; none without it): a
  chain whose voters drop under two thirds would never finalize again, so bonds that stop voting
  leak until those that still vote are two thirds of what is left.
  - **What state sees:** with a leak, blocks carry every justification whether or not it pays, so
    state knows the newest checkpoint proven justified; and once no checkpoint has been proven for
    `after_epochs` epochs, they may carry *bundles*, proofs whose votes fall short of a
    justification. A bundle pays nothing and proves nothing, but records that its voters voted
    for its target epoch. A producer puts in the votes it counted for checkpoints not yet proven
    whenever one of them is new to state.
  - **What leaks:** from one epoch later on, the stall, so the first leak already sees who voted,
    at each checkpoint every validator in the epoch's set whose
    vote for the epoch before has not been carried in a block loses `permille` thousandths of
    its bond, burned. The sets already fixed for the epochs after are reweighed, so the quiet
    weight falls out of the denominator at once. One that falls under `min_bond` leaves the way
    an unbond does.
  - **What it costs the honest:** nothing, as long as their votes reach a producer. A validator
    whose node is up but cut off from every producer leaks like a quiet one.

Staking transactions fail with "this chain has no staking" under any other kind.

## Authority

The same votes, from signers each counting once; `threshold` of them justify.

- **A signer** is a voting key and an account (`Signer`): the key signs its votes, and the
  account speaks for it on chain, sending its approvals of changes and taking its rewards. Named
  as a bare key, a signer's account is the key's own address; named as `{ "key", "account" }`, the
  account is another, so the voting key can stay on the validating node and sign nothing but
  votes, as an FFG voter's does. Two signers never share an account.
- **The signers** start as genesis's `signers`. Each epoch's set is fixed in state at the
  checkpoint `activation_delay` epochs before it. Under a rotating authority
  (`consensus.type: rotating`), the same set produces the epoch's blocks in turn (see
  [Consensus](consensus.md#rotating-authority)), as FFG's validator sets do under production
  by stake (`consensus.type: staked`, see [Consensus](consensus.md#production-by-stake)).
- **Both thresholds** must be more than half the signers. Then two conflicting checkpoints can
  only both be finalized if some signer voted against itself, and its votes show it.
- **Changing signers** (`chainv4 send --key account.key approve-signers --add KEY --remove KEY`,
  signed by a signer's account; `--add KEY@ACCOUNT` gives a new signer an account of its own):
  1. A signer's account approves a whole new set, each signer with its key and account, and both
     thresholds (`Tx::ApproveSigners`), naming the version of the decided set it changes.
  2. Once `change_threshold` of the decided signers approve the same set for the same version,
     with approvals that still count, it is decided, and the version goes up by one.
  3. The checkpoint after a decision fixes the new set for the epoch `activation_delay` ahead, so
     a change decided in epoch k votes from epoch k + `activation_delay` + 1.

  `GET /signers` shows the active set, the decided one, and the approvals that still count.
- **What makes an approval count:**
  - **One per signer:** a signer that approves another set moves its approval there.
  - **Withdrawing:** a signer can take its approval back (`Tx::WithdrawApproval`,
    `chainv4 send --key account.key withdraw-approval`), so it backs no change. It fails when the
    signer has no approval that still counts.
  - **Expiry:** an approval counts for `approval_epochs` epochs, the one it was made in included:
    made in epoch k, it counts through epoch k + `approval_epochs` - 1. A change is only decided
    by approvals that still count, so it reflects what the signers want now, not what they once
    did. Approving again starts the clock afresh. Expired approvals are simply not counted, and
    are let go the next time the ballot is written.
  - **Versions:** approvals for an older version never count. Deciding any change empties the
    ballot.
- **Rewards:** `vote_reward`, 0 in the dev settings, pays each signer's account.
- **Ejection:** there are no bonds to take, so `Tx::Slash` with a signer's two conflicting votes
  throws it out instead: it leaves the decided set, which keeps its thresholds and takes a new
  version, emptying the ballot, and the sets already fixed from two epochs on, as a slashed
  validator does. The evidence is refused when a set it would leave could no longer reach its
  thresholds, such as two of two signers: then the remedy is a change the other signers approve.

## Depth

No votes. A main-chain block is final once `depth` blocks are on top of it, and the chain never
reorganizes past it. Every block is a checkpoint, so the finalized checkpoint's epoch is its
height. `depth` 0 makes every block final as soon as it is the tip.

This finality is each node's own view, a rolling checkpoint as Bitcoin Cash keeps one ten blocks
deep, with the same trade: it stops deep reorganizations, at the cost of liveness across a split.
Bitcoin itself has no such rule, so its nodes always rejoin on the heaviest chain. Nodes kept
apart for longer than `depth` blocks, by a partition or a private fork, finalize different
branches and never come back together, and a node syncing from scratch keeps whichever branch it
syncs first. When that happens, an operator resyncs one side from an empty database. It is a
scheme for development and for chains one operator runs; a chain with many producers wants votes.
`--validate` is refused.

## Running a validator

```sh
chainv4 keygen --out vote.key
chainv4 send --key alice.key bond --voter ed25519:0x… --amount 100000000000000000000   # FFG
chainv4 --peer … --validate --validator-key vote.key
```

Without `--validator-key`, the node votes with the development key, which is the one validator,
or signer, of the dev settings.

## What to keep in mind

- **Set a leak on a chain of value.** Without one, a chain whose voters drop under two thirds
  never finalizes again until they return. Authority finality has no bonds to leak: its remedy
  is a change the remaining signers approve, which needs more than half of them.
- **A slashed or ejected voter keeps its weight for two epochs:** the slash's and the next,
  whose sets stay fixed. From two epochs on it is out of every set.
- **A light client learns a slash from the next justified checkpoint.** Its update re-states
  the sets ahead. Should every epoch between the slash and a justification be skipped, and that
  justification's margin be thinner than the slashed weight, a client that followed through the
  slash holds the old set and refuses it; starting it over at a checkpoint takes the sets as
  they are.
- **Start new nodes at a checkpoint on a chain of value.** `--snapshot --checkpoint 0x…` trusts
  a finalized checkpoint as a light client does, and fetches nothing below it, so no fork built
  by keys that have since unbonded can reach it (see [Node](node.md#syncing-from-a-snapshot)).
  A node that replays from genesis instead trusts every validator set since genesis.
- **Vote rewards are minted unless genesis says `fees`,** which pays them from the burned share
  of fees and no more.
