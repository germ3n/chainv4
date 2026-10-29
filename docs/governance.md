# Governance

Governance (`src/governance/`, `src/registry/changes.rs`) is how a running chain changes what
genesis set, without a hard fork. Genesis names a kind of governance, or none, and that kind says
who may approve a change and what the approvals must add up to. A change is proposed and approved
by transaction, decided once the approvals weigh enough, and fixed in state for an epoch ahead at
the next checkpoint, the way finality fixes its voters, so every branch and every light client
learns it the same way.

What can change today:
- **The consensus:** who produces blocks, and how. A chain can launch under proof of work and move
  to production by stake once enough validators have bonded, or move a rotating authority to a
  different slot length.
- **The bootnodes:** the addresses new nodes dial to find their first peers, which genesis fixes
  with the chain's id (see [Bootnodes](#bootnodes)).
- **What names cost:** the price of a year of a name by its length (see
  [Name prices](#name-prices)).
- **The bridges' federation:** who vouches and signs for the [bridges](bridges.md) (see
  [Federation](#federation)).
- **The directory:** the contracts the chain names for the apps and tools, the bridges'
  gateways, the swap contract, the DEX and the NFT factory (see [Directory](#directory)).
- **What makes a collection:** the functions NFT and multi-token collections export, which turns
  the collection registry on for a chain whose genesis kept none (see
  [Collections](#collections)).
- **Fees, the block reward and the limits:** the whole fee configuration, what a block mints and
  how it halves, and what a transaction and a block may use (see [Fees](#fees),
  [Reward](#reward) and [Limits](#limits)).
- **Finality's tunables:** how deep votes wait, what they pay, the staking rules and the
  inactivity leak (see [Finality](#finality)).
- **Finality's kind:** FFG to authority or back, from an epoch on (see
  [Switching finality](#switching-finality)).
- **An upgrade:** from a block on, nodes older than a version stop, for a release that changes the
  rules (see [Upgrade](#upgrade)).
- **The chain's name:** what the apps call the network (see [Name](#name)).

The parameters that define finality's epochs, `checkpoint_interval` and `activation_delay`, stay
what genesis says: every node's and light client's epoch arithmetic rests on them.

## The kinds

Each kind implements the `Governance` trait: who may approve, what each approval weighs, and what
the approvals for one change must add up to. Each sits behind a cargo feature, on by default, and
genesis names one as `governance.type`:

- **`electorate`** (`governance-electorate`): finality's own electorate approves. Under FFG, an
  account's approval weighs the stake of the validators it bonded, as the current epoch's set
  fixes them, and two thirds of the set decide, as a justification takes. Under authority
  finality, an account weighs the decided signers it speaks for, and `change_threshold` of them
  decide, as a signer change takes. Whoever finalizes the chain changes it.
- **`council`** (`governance-council`): accounts genesis names, each weighing one, `threshold`
  of them deciding. For a chain whose finality electorate should not govern it, such as one where
  a foundation decides what the validators run.

Both take `approval_epochs`: how many epochs an approval counts for, from the one it was made in.

Governance needs finality that votes, FFG or authority, because its decisions reach light clients
through finality's light updates; genesis refuses it with depth finality or none.

## A change

1. **Proposing and approving** are one transaction: `Tx::ApproveChange` names the `version` of
   the decision it changes, how many changes of any kind were decided before, and the `change`:
   `consensus` with a whole new consensus, or any of the others below, each whole. `chainv4 send
   --key account.key approve-change --consensus '{"type":"staked","slot_ms":2000,"schedule":"draw"}'`
   sends one, taking the version from the node. The consensus must fit the chain's finality as genesis's must: `rotating` needs
   authority finality, `staked` needs FFG; `pow` keeps the chain's window, if genesis set one.
2. **Counting.** The ballot holds at most one approval per account; an account that approves
   another change moves its approval there, and `Tx::WithdrawChange` (`send withdraw-change`)
   takes it back. An approval stops counting `approval_epochs` epochs after the one it was made
   in. What an approval weighs is counted afresh whenever the ballot is, in the state it is
   counted in: a validator that unbonded since weighs nothing.
3. **Deciding.** Once the approvals that still count for the same change weigh what the kind
   needs, it is decided: the version goes up by one and the ballot empties. Approvals for an
   older version never count.
4. **Taking effect.** The checkpoint after the decision, the block it landed in when that is one,
   fixes the decided consensus for the epoch `activation_delay` ahead, as it fixes the voters.
   Every epoch's consensus is fixed `activation_delay` epochs before it, so a branch that split
   off after the checkpoint that fixed an epoch's consensus checks that epoch's blocks by the
   same rules as the main chain.

`GET /governance` shows the kind, the consensus the tip's epoch runs and the one decided last,
and the approvals that still count, each change with what its approvals weigh and need; who may
approve; and every setting governance may change as it stands, each with the change that set it
last, or genesis. The explorer and the wallet show it on their Governance pages, and the wallet
approves, withdraws and proposes from there.

## Decisions

- **The approval that decides a change says so.** Its receipt's outcome is `Decided(version)`,
  where the others' is `Done`, and the receipts root commits to it, so a light client proves a
  decision with the receipt alone (`GET /proofs/receipts/{id}`): the transaction, the change it
  approved, and that it decided it, as which version.
- **Nodes index them.** As a block runs, each deciding approval's id goes in the `Decisions`
  column by the version it made: not chain state, written with the block's receipts and taken
  back with them in a reorg. `GET /governance/history` lists them, newest first.
- **None can be left out unseen.** Versions count up one a decision, and the newest is
  `change:decided` in state (column `validators`), so a gap in a node's list shows. A node that
  synced from a snapshot lacks the decisions before it, and says from which version it has them.
- **A change of the rules.** Nodes from before this disagree with newer ones on a deciding
  approval's receipt, so the receipts root of its block: a chain that had decided anything moves
  to it with a new genesis, or from a height an [upgrade](#upgrade) names.

## Bootnodes

Genesis's `bootnodes` are part of the chain's id, so they cannot change without a new chain; on a
chain with governance, a change replaces them:

```sh
chainv4 send --key member.key approve-change --bootnodes /ip4/203.0.113.7/udp/4001/quic-v1,/ip4/198.51.100.2/udp/4001/quic-v1
chainv4 send --key member.key approve-change --bootnodes none   # the chain has none from then on
```

- **The same ballot and version as consensus changes.** An account backs one change at a time,
  of either kind, and deciding one moves the version, so approvals of the other for the old
  version stop counting.
- **It takes effect at once.** Nothing checks blocks by the bootnodes, so no epoch need be fixed
  for them: from the block that decides them, `Chain::bootnodes` gives the new list, every node
  that imports the block tells its network so, and `GET /status` and `GET /governance` show it.
- **The list is checked as genesis's is:** at most 32 addresses, each named once, each a libp2p
  path such as `/ip4/…/udp/…/quic-v1`. A node skips, and logs, one libp2p cannot read.
- **A node's own `--bootnode`s stay beside the chain's,** whatever governance decides.

## Name prices

On a chain with [names](names.md), a change replaces the prices a year of a name costs:

```sh
chainv4 send --key member.key approve-change --name-prices 1000000000000000000000,200000000000000000000,10000000000000000000
```

- **The same ballot and version** as the other kinds.
- **It takes effect at once,** from the block that decides it, as bootnodes do: what a name costs
  is read when one is registered or renewed, and checks no block.
- **The list is checked as genesis's is:** 1 to 61 prices, in the smallest unit, the first for
  names of 3 characters and the last for every length from its own on. A chain without names
  refuses the change.
- **Names already held keep the years they paid for.** `GET /names` shows the prices in force
  and genesis's.

## Federation

A change names the [bridges' federation](bridges.md#the-federation), replacing genesis's or the
last decided:

```sh
chainv4 send --key member.key approve-change --federation '{"signers":["0x02…","0x03…","0x02…"],"threshold":2,"chain_depth":20,"evm_depth":12,"bitcoin_depth":6}'
```

- **The same ballot and version** as the other kinds.
- **It takes effect at once,** from the block that decides it: contracts read it as they run.
- **One state entry,** `change:federation`, holds the one in force, genesis's written there as
  the chain starts, so a light client proves it: the wallet computes Bitcoin deposit addresses
  from it.
- **Its own version counts up** with each one decided, after genesis's 0, so what was signed for
  one federation is told from what was signed for the next.
- **It is checked as genesis's is:** 1 to 20 secp256k1 keys, each once, a threshold of 1 to as
  many, and depths of at least 1.

## Directory

The directory names the contracts the apps and tools use, so none is taken from a name someone
else could register first:

| Field | What it names |
| --- | --- |
| `gateway` | The [bridges'](bridges.md) gateway here, `gateway.lua`. |
| `btc_gateway` | The [Bitcoin gateway](bridges.md#bitcoin), `btc_gateway.lua`, which names its relay and wBTC. |
| `evm_gateways` | `Gateway.sol` on each EVM chain: `{"chain_id": 1, "gateway": "0x…"}`, in the order of their ids, each chain once, 32 at most. |
| `swap` | The [swap contract](swaps.md#chainv4s-side-the-swap-contract) new offers use. |
| `dex`, `dex_router` | The native [DEX](dex.md), and the EVM DEX's router on this chain's EVM, `0x…`. |
| `nft_factory` | The [NFT factory](nft-standard.md) new collections are made with. |

Each is `null` while there is none. Genesis may name it (`directory`, see [Genesis](genesis.md)),
and a change replaces all of it:

```sh
chainv4 send --key member.key approve-change --directory '{"gateway":"contract:0x…","btc_gateway":"contract:0x…","evm_gateways":[{"chain_id":11155111,"gateway":"0x…"}],"swap":"contract:0x…","dex":"contract:0x…","dex_router":"0x…","nft_factory":"contract:0x…"}'
```

- **One state entry,** `change:directory` in the validators column, holds the one in force,
  genesis's from the first block or the last decided, so the wallet's light client proves it as
  it proves a balance. `GET /status` shows it as `directory`.
- **What clients take,** in order: what they are given (a flag, or Settings), the directory, then
  what the node's operator names (`--swap-contract`, `--dex-contract`, `--dex-router`).
- **It never moves the federation's version,** and nothing on chain reads it: it is for the apps,
  the CLI and the federation's signers, who refuse to vouch for a gateway peer the directory does
  not name.
- **It is checked as genesis's is:** contracts here are contract addresses, EVM ones not zero.

### Without governance: the directory's admin

A chain without governance changes its directory through an admin, if its genesis names one in
`directory_admin`. Each chain has one way at most:

| The chain | Who sets the directory |
| --- | --- |
| With governance | Governance, by `Change::Directory`; genesis may name no admin. |
| Without, with `directory_admin` | The admin, by `Tx::SetDirectory`. |
| Neither, or the admin renounced | Nobody: genesis's stands for good. |

```sh
chainv4 send --key admin.key set-directory --directory '{"gateway":"contract:0x…", …}'
chainv4 send --key admin.key hand-directory --to ed25519:0x…   # another account, or a multisig
chainv4 send --key admin.key hand-directory --renounce          # nobody, for good; asks for yes
```

- **The admin is any account that signs,** a native multisig's too (its members send it as the
  multisig), never a contract.
- **The directory takes effect at once,** checked as genesis's is, into the same state entry.
- **The admin is in state too,** `directory:admin` in the validators column, so a light client
  proves who it is, or that there is none; `GET /status` shows it as `directory_admin`.

## Collections

A change says what makes a contract a [collection](nft-standard.md#the-collection-registry),
replacing genesis's `collections` or the last decided: how a chain launched without the registry
turns it on.

```sh
chainv4 send --key member.key approve-change --collections standard
chainv4 send --key member.key approve-change --collections '{"nft":["owner_of",…],"multi":["balance_of",…]}'
```

- **The same ballot and version** as the other kinds.
- **It takes effect at once,** from the block that decides it: deploys, upgrades and refreshes
  register by it. Contracts deployed before join at their next upgrade or refresh.
- **The lists are checked as genesis's are:** each function named once in each list.
  `GET /status` shows what is in force, in `collections`.

## Fees

A change replaces the whole [fee configuration](fees.md), genesis's `fees` or the last decided,
in the shape genesis writes it (see [Genesis](genesis.md#fees)):

```sh
chainv4 send --key member.key approve-change --fees '{"block_gas_limit":50000000,"burn_permille":500,"schedule":{"type":"weighted","per_tx":1000,"per_byte":10,"per_code_byte":20,"fuel_per_gas":1},"pricing":{"type":"base_fee","initial":"1000000000","min":"10000000","elasticity":2,"change_denominator":8}}'
```

- **The same ballot and version** as the other kinds.
- **It takes effect from the next block:** the block after the one that decides it is checked,
  priced and charged by it. Under `base_fee` pricing, that block's base price follows from its
  parent's by the new rules; `initial` only starts a chain.
- **A chain without fees gets them** from a change; no change takes them away again.
- **It is checked as genesis's is.** `GET /status` shows what is in force, in `fees`.

## Reward

A change replaces what a block mints for its producer:

```sh
chainv4 send --key member.key approve-change --reward '{"initial":"25000000000000000000","halving_interval":210000}'
```

- **It takes effect from the next block,** and halves from there: the block after the decision
  mints `initial`, and `halving_interval` counts from it, not from genesis. `null` never halves.
- **Blocks before keep what they minted.** The schedule is genesis's until the deciding block,
  the decided one after; the API's `reward` on a block reads it at the block's height.
- **A halving interval of 0 is refused,** as in genesis.

## Limits

A change replaces genesis's `tx_limits` and `max_block_bytes` together:

```sh
chainv4 send --key member.key approve-change --limits '{"tx_limits":{"fuel":10000000,"memory":67108864},"max_block_bytes":4194304}'
```

- **It takes effect from the next block:** the transactions in it run under the new limits, and
  its size is checked by them. `max_block_bytes: null` bounds blocks by gas alone.
- **A change no contract could run under is refused:** fuel below 100,000, memory below 1 MiB, or
  `max_block_bytes` outside 1 MiB to 6 MiB, as genesis's.

## Finality

A change retunes finality, given as a whole genesis `finality` object (see
[Genesis](genesis.md#finality)):

```sh
chainv4 send --key member.key approve-change --finality "$(jq -c '.finality | .min_bond = "500000000000000000000"' genesis.json)"
```

- **Only what may change, changes.** The kind, `checkpoint_interval`, `activation_delay`, the
  genesis validators or signers and the thresholds must be genesis's; a change that moves any of
  them is refused. Under FFG that leaves `vote_depth`, the bond rules (`min_bond`,
  `withdrawal_delay`, `reporter_divisor`, `max_validators`), `vote_reward`,
  `max_proofs_per_block` and the `leak`; under authority, `vote_depth`, `max_signers`,
  `approval_epochs`, `vote_reward` and `max_proofs_per_block`.
- **It takes effect an epoch boundary ahead:** decided in epoch e, it runs from epoch
  `e + activation_delay + 1`, whose voters are already fixed by then, so every branch that split
  off after it counts votes and pays them the same. Each era is kept in `change:finality`, so a
  block of an older epoch is checked by the tuning of its own.
- **The leak and staking rules** a checkpoint applies are its epoch's; so are `vote_depth`, what
  a vote pays and how many proofs a block carries.
- **Light clients need nothing new:** the thresholds and validator sets they check stay as they
  were, and the sets reach them as before.
- **It retunes the finality decided last:** after a switch of kind, the switch's config is what
  a retune keeps the starting voters and thresholds of.

## Switching finality

A change switches finality to the other kind that votes, FFG to authority or authority to FFG,
from an epoch it names, given as a whole genesis `finality` object of the new kind:

```sh
chainv4 send --key member.key approve-change --switch-epoch 400 --switch-finality '{"type":"authority","checkpoint_interval":32,"vote_depth":2,"activation_delay":2,"max_signers":100,"signers":[…],"threshold":3,"change_threshold":3,"approval_epochs":64,"vote_reward":"0","max_proofs_per_block":4}'
```

- **The epochs stay.** The new config carries genesis's `checkpoint_interval` and
  `activation_delay`, so a block's epoch is the same sum before and after, and only who votes
  changes.
- **It runs from the epoch named,** or from `activation_delay + 1` after the one that decides it,
  whichever is later; approving one for an earlier epoch is refused. Each epoch's voters are
  fixed `activation_delay` ahead by the kind it runs, so the checkpoint that fixes the switch
  epoch's voters fixes the new kind's.
- **Into FFG:** the config names no `validators`. From the block that decides the switch,
  validators bond (`Tx::Bond`) under the new config's rules, and those bonded by the checkpoint
  that fixes the switch epoch make its set. Should nobody have, the switch **lapses**: the signers
  carry on as if it had never been decided, and bonds made too late are paid back. Name an epoch
  far enough ahead for validators to bond.
- **Into authority:** the config's `signers` become the decided set, voting from the switch
  epoch, its thresholds theirs; `Tx::ApproveSigners` changes them from then on. Every FFG
  validator is unbonding from the switch epoch: it votes until then, withdraws
  `withdrawal_delay` epochs later, and a double vote from before can still be reported and cost
  its bond. Nobody bonds under authority.
- **Finality carries on.** The justified and finalized checkpoints stay; the new electorate's
  first votes link from the old one's last justified checkpoint. Evidence of a double vote is
  judged by the kind its epoch ran.
- **One transition at a time.** A switch is refused while a change of finality or of the
  consensus decided earlier is still to take effect, and nothing else touching either is
  approved while a switch is.
- **The consensus must fit.** `staked` needs FFG and `rotating` needs authority, so a chain
  producing by stake moves to `pow` or `authority` first, and from `pow` or `authority` to
  `rotating` once the signers vote. A switch is checked against the consensus decided last.
- **Depth finality neither switches nor is switched to:** governance needs voting finality.
- **Light clients follow it.** A checkpoint's light update names the epochs among those it fixes
  whose voters are the other kind's (`LightUpdate::switched`), proved at that kind's keys; a state
  only ever fixes one kind's voters for an epoch, so a node can neither hide a switch nor make one
  up (see [Light client](light-client.md)).
- `GET /governance` shows the `finality` the tip's epoch runs and a `switch` on its way, with its
  `epoch` and `type`; `GET /status` shows the tip epoch's kind.

Fees, reward, limits and finality each keep their own state entry (`change:fees`,
`change:reward`, `change:limits`, `change:finality`), read for each block from its parent's state,
so a reorg past a decision undoes it with the rest. The mempool, the producer and the API read
what the tip's next block runs by.

## Upgrade

A change names the oldest release that runs past a block:

```sh
chainv4 send --key member.key approve-change --upgrade 0.2.0 --upgrade-from 150000
```

- **Older nodes stop before that block,** warning every ten minutes as it nears, and say to
  update; newer ones run on as before. It is advice to nodes, never a rule blocks are checked by.
- **`from` must be ahead** of the block deciding it, and the least version above 0.0.0.
- **A later upgrade replaces it.** It is kept in `change:upgrade` and shown as `upgrade` in
  `GET /status`.
- A chain without governance announces one in a release instead. See [Updates](updates.md).

## Name

A change renames the chain, replacing genesis's `name` or the last decided:

```sh
chainv4 send --key member.key approve-change --chain-name "chainv4 mainnet"
```

- **It takes effect at once,** from the block that decides it: only the apps read it, as
  `GET /status`'s `name`.
- **It is checked as genesis's is:** 1 to 40 characters, no control characters, no space at
  either end. A chain whose genesis named none gets one this way.
- It is kept in `change:name`, beside genesis's, so a light client proves either.

## What the chain does with it

- **Checking blocks.** `Chain::consensus_at` gives the rules the block at a height is checked by:
  genesis's on a chain without governance, or the consensus fixed for the height's epoch in the
  state at the tip. The rules for each consensus seen are built once and kept.
- **Producing.** A node started with `--mine`, `--produce` or both holds a `Producing` setup
  rather than one producer: it builds the producer for the consensus of each block it is about to
  build, and again when governance changes it, so a node on a chain that moves from proof of work
  to stake mines until the switch and signs from it, with no restart. A node with nothing for the
  consensus the chain runs follows only, and says so in its log.
- **Work.** A block's weight comes from its own epoch's rules: a proof-of-work block weighs its
  difficulty, a block in turn under stake weighs 2. Total work only ever compares branches of the
  same chain, so the units need not agree across the switch.
- **Light clients** learn each epoch's consensus from the light update of the checkpoint
  `activation_delay` epochs before it, as they learn its voters, trusted once that checkpoint is
  justified; `LightParams::governance` tells them to expect that. A header whose epoch's consensus
  they do not know yet waits, like one whose producers they have not proved. So under governance
  a light client follows `activation_delay` epochs behind finality on every chain, not only a
  rotating or staked one.
- **Snapshot sync** checks headers through a light client of the node's own chain, so a node
  syncing from a snapshot learns the eras the same way, and takes its pivot among the headers it
  could check.

## Things to keep in mind

- **A change is as slow as finality.** It takes effect `activation_delay` epochs after the
  checkpoint that follows the decision, and light clients see it only once that checkpoint is
  justified. A chain whose finality has stalled cannot change its consensus.
- **The new consensus needs its producers ready.** A move to `staked` needs validators bonded and
  in the sets fixed for the switch epoch, and their nodes running with `--produce`; a move to
  `pow` needs miners. Nobody checks that for you: a chain that switches to a scheme nobody serves
  stops until someone does, and governance cannot move it back without blocks to carry the
  approvals.
- **`checkpoint_interval` and `activation_delay` never change,** whatever kind runs.
- **A genesis file from before governance keeps its id:** `governance` encodes as nothing while
  unset, and so does the governance kind in the light parameters. `bootnodes`, after it, does the
  same while empty.
- **Light clients do not follow bootnodes.** They are advice to nodes, kept in state but carried
  in no light update.
