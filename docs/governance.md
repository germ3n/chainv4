# Governance

Governance (`src/governance/`, `src/registry/changes.rs`) is how a running chain changes what
genesis set, without a hard fork. Genesis names a kind of governance, or none, and that kind says
who may vote on a change and what its votes must add up to. A change is proposed and voted on by
transaction, Yes, No or Abstain, decided once Yes weighs enough and a quorum has voted, or
rejected once it can no longer pass; a consensus change is then fixed in state for an epoch ahead
at the next checkpoint, the way finality fixes its voters, so every branch and every light client
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
- **Governance itself:** a council's members, threshold, how long votes count, and the quorum
  (see [Governance itself](#governance-itself)).
- **What makes a token, the EVM's spec and the DEX's fee** (see [Tokens](#tokens),
  [EVM spec](#evm-spec) and [DEX fees](#dex-fees)).
- **Host costs, contract limits, the slash, the PoW steps and the Bitcoin policy,** as parts of
  fees, limits, finality, the consensus and the federation (see
  [What else each kind carries](#what-else-each-kind-carries)).
- **The locks:** settings, whole or a field of one, that no vote changes from then on, and which
  nothing unlocks (see [Locks](#locks)).

The parameters that define finality's epochs, `checkpoint_interval` and `activation_delay`, stay
what genesis says: every node's and light client's epoch arithmetic rests on them.

## The kinds

Each kind implements the `Governance` trait: who may vote, what each vote weighs, what Yes on one
change must add up to, and what every voter weighs together. Each sits behind a cargo feature, on by default, and
genesis names one as `governance.type`:

- **`electorate`** (`governance-electorate`): finality's own electorate approves. Under FFG, an
  account's vote weighs the stake of the validators it bonded, as the current epoch's set
  fixes them, and two thirds of the set decide, as a justification takes. Under authority
  finality, an account weighs the decided signers it speaks for, and `change_threshold` of them
  decide, as a signer change takes. Whoever finalizes the chain changes it.
- **`council`** (`governance-council`): accounts genesis names, each weighing one, `threshold`
  of them deciding. For a chain whose finality electorate should not govern it, such as one where
  a foundation decides what the validators run.

Both take `approval_epochs`, how many epochs a vote counts for from the one it was cast in, and
`quorum_permille`, the share of every voter's weight that must vote on a change before Yes decides
it, 0 for none and for genesis files from before it.

Governance needs finality that votes, FFG or authority, because its decisions reach light clients
through finality's light updates; genesis refuses it with depth finality or none.

## A change

Each kind of setting has a ballot and a version of its own, its `ChangeKind`: the consensus
(with finality's tuning and a switch of finality's kind, which are checked against each other
and so share one), bootnodes, names, collections, token functions, the federation, the
directory, fees, the reward, limits, an upgrade, the name, governance itself, the EVM spec, the
DEX's fee, anchoring and the locks. Deciding a change of one kind leaves every other kind's votes standing, but for
governance itself, which clears them all.

1. **Proposing and voting** are one transaction: `Tx::VoteChange` names the `version` of the
   change's kind it changes, how many changes of that kind were decided before, the `change`,
   and the `vote`: `yes`, `no` or `abstain`. The first vote on a change proposes it.
   `chainv4 send --key account.key vote-change --vote yes --consensus '{"type":"staked","slot_ms":2000,"schedule":"draw"}'`
   sends one, taking the kind's version from the node; `--change '{"dex":{"tiers":[5,30,100],"protocol_permille":100}}'`
   takes any change as `GET /governance` writes one. The consensus must fit the chain's finality
   as genesis's must: `rotating` needs authority finality, `staked` needs FFG.
2. **Part of a setting.** Fees, the reward, limits, finality, the federation, the directory,
   names, governance and the DEX may be given in part, such as `--fees '{"burn_permille":600}'`
   or `--change '{"governance":{"threshold":2}}'`: what is left out keeps what stands, laid over
   it field by field (`params::json::over`) before the whole is checked and sent. The kind's
   version makes it safe: had a change of that kind been decided in between, the vote would name
   an older version, and count for nothing.
3. **One vote an account on each change.** A new vote replaces the old one until the change is
   decided or rejected, and `Tx::ClearVote` (`send clear-vote --kind fees`) takes it back. An
   account backs one change of each kind at once: Yes on another change of the kind moves its Yes
   there, while No and Abstain stand on as many changes as it likes. A vote stops counting
   `approval_epochs` epochs after the one it was cast in. What a vote weighs is counted afresh
   whenever the ballot is, in the state it is counted in: a validator that unbonded since weighs
   nothing.
4. **Deciding.** Once Yes weighs what the kind needs (two thirds of the stake, the signers'
   `change_threshold`, or the council's `threshold`), and Yes, No and Abstain together weigh the
   quorum, the change is decided: the kind's version goes up by one and its ballot empties, and
   the chain's count of decisions, of every kind, goes up by one too.
5. **Rejecting.** A change is rejected as soon as it can no longer pass: once what has not voted
   No, Yes and Abstain and everyone yet to vote, weighs less than what is needed. On a council of
   3 needing 2, two No votes reject it; under FFG, No from more than a third of the stake does.
   It is judged at each vote on it: a No, or any vote after the electorate shrank. Its votes go,
   the chain's count of rejections goes up by one, and the same change is refused at the kind's
   version from then on, so it does not come straight back; one that differs in any field may be
   proposed.
6. **The quorum** (`quorum_permille`, 0 unless set) is a share of every voter's weight that must
   have voted on a change at all, Abstain included, before Yes can decide it. It rounds up, so any
   quorum asks for at least one vote's weight. A No counts toward it too: it may be the vote that
   lets Yes decide.
7. **Taking effect.** A consensus change is fixed at the checkpoint after the decision for the
   epoch `activation_delay` ahead, as it fixes the voters. Every other kind holds from the block
   after the one that decides it, read from that block's parent's state, but for an EVM spec,
   which runs from the height it names, and finality's tuning, which waits for an epoch.

**Why a rejection does not move the kind's version.** A version says what stands, and a rejection
changes nothing that stands: every other change of the kind keeps its votes, and every vote in
flight for that version still counts. The rejected change is kept in the ballot's `rejected` list
instead, until a change of its kind is decided.

`GET /governance` shows the kind, the consensus the tip's epoch runs and the one decided last,
who may vote (`voters`, each with its weight) and what they weigh together (`total`), the quorum,
how many changes were rejected, and each open change with every vote that still counts and its
`tally`: what Yes, No and Abstain weigh, what Yes needs, the quorum's weight, how much more Yes
needs and whether it can still pass; `closes` is the last epoch its first vote counts in. The
explorer and the wallet show it on their Governance pages, and the wallet votes Yes, No or
Abstain and clears a vote from there (see [Wallet](wallet.md#governance) and
[Explorer](explorer.md)).

## Decisions

- **The vote that decides a change says so.** Its receipt's outcome is
  `Decided { decision, kind, version }`, where the others' is `Done`: the chain's decision-th, of
  every kind, and its kind's version-th. The receipts root commits to it, so a light client proves
  a decision with the receipt alone (`GET /proofs/receipts/{id}`): the transaction, the change it
  voted on, and that it decided it, as which.
- **So does the vote that rejects one:** `Rejected { rejection, kind, version }`, the chain's
  rejection-th, at the kind's version, which stays. Rejections count up one at a time of their
  own, `change:rejected` in state, and nodes index them beside the decisions; `GET
  /governance/rejections` lists them, newest first, each provable the same way.
- **Nodes index them.** As a block runs, each deciding vote's id goes in the `Decisions`
  column by the chain's count it made: not chain state, written with the block's receipts and
  taken back with them in a reorg. `GET /governance/history` lists them, newest first.
- **None can be left out unseen.** The chain's count goes up one a decision, whatever its kind,
  and the newest is `change:decided` in state (column `validators`), so a gap in a node's list
  shows. A node that synced from a snapshot lacks the decisions before it, and says from which it
  has them.
- **A change of the rules.** Nodes from before this disagree with newer ones on a deciding
  vote's receipt, so the receipts root of its block: a chain that had decided anything moves
  to it with a new genesis, or from a height an [upgrade](#upgrade) names.

## Bootnodes

Genesis's `bootnodes` are part of the chain's id, so they cannot change without a new chain; on a
chain with governance, a change replaces them:

```sh
chainv4 send --key member.key vote-change --vote yes --bootnodes /ip4/203.0.113.7/udp/4001/quic-v1,/ip4/198.51.100.2/udp/4001/quic-v1
chainv4 send --key member.key vote-change --vote yes --bootnodes none   # the chain has none from then on
```

- **A ballot and version of their own.** An account may back new bootnodes and a consensus
  change at once, and deciding one leaves the other's votes standing.
- **It takes effect at once.** Nothing checks blocks by the bootnodes, so no epoch need be fixed
  for them: from the block that decides them, `Chain::bootnodes` gives the new list, every node
  that imports the block tells its network so, and `GET /status` and `GET /governance` show it.
- **The list is checked as genesis's is:** at most 32 addresses, each named once, each a libp2p
  path such as `/ip4/…/udp/…/quic-v1`. A node skips, and logs, one libp2p cannot read.
- **A node's own `--bootnode`s stay beside the chain's,** whatever governance decides.

## Name prices

On a chain with [names](names.md), a change of names replaces the prices a year of a name costs,
the grace after one expires and the most years ahead one is held, all but the suffix
(`--change '{"names":{"grace_ms":2592000000}}'` names only the grace). `--name-prices` changes the
prices alone:

```sh
chainv4 send --key member.key vote-change --vote yes --name-prices 1000000000000000000000,200000000000000000000,10000000000000000000
```

- **A ballot and version of its own,** as each kind has (see [A change](#a-change)).
- **It takes effect from the next block:** each block reads the names' settings from its parent's
  state. A name in its grace period is judged by the grace in force when it is looked at.
- **The list is checked as genesis's is:** 1 to 61 prices, in the smallest unit, the first for
  names of 3 characters and the last for every length from its own on. A chain without names
  refuses the change.
- **Names already held keep the years they paid for.** `GET /names` shows the prices in force
  and genesis's.

## Federation

A change names the [bridges' federation](bridges.md#the-federation), replacing genesis's or the
last decided:

```sh
chainv4 send --key member.key vote-change --vote yes --federation '{"signers":["0x02…","0x03…","0x02…"],"threshold":2,"chain_depth":20,"evm_depth":12,"bitcoin_depth":6}'
```

- **A ballot and version of its own,** as each kind has (see [A change](#a-change)).
- **It takes effect at once,** from the block that decides it: contracts read it as they run.
- **One state entry,** `change:federation`, holds the one in force, genesis's written there as
  the chain starts, so a light client proves it: the wallet computes Bitcoin deposit addresses
  from it.
- **Every one is kept for good,** under `change:past-federation:` and its version, with the height
  and time of the block that decided it, genesis's as decided at height 0: contracts read one by
  its version with `federation.at`, and the next one's block is when it was replaced. The Bitcoin
  gateway counts an old federation's grace from that, so one replaced before anything called the
  gateway still mints what was paid to its addresses. A chain from before these records has none
  of them.
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
| `dex_split` | The split router, [router.lua](dex.md#across-both-dexes-routerlua), which divides one trade between both DEXes. Left out of a genesis or vote written before it, none. |
| `nft_factory` | The [NFT factory](nft-standard.md) new collections are made with. |

Each is `null` while there is none. Genesis may name it (`directory`, see [Genesis](genesis.md)),
and a change replaces all of it:

```sh
chainv4 send --key member.key vote-change --vote yes --directory '{"gateway":"contract:0x…","btc_gateway":"contract:0x…","evm_gateways":[{"chain_id":11155111,"gateway":"0x…"}],"swap":"contract:0x…","dex":"contract:0x…","dex_router":"0x…","dex_split":"contract:0x…","nft_factory":"contract:0x…"}'
```

- **One state entry,** `change:directory` in the validators column, holds the one in force,
  genesis's from the first block or the last decided, so the wallet's light client proves it as
  it proves a balance. `GET /status` shows it as `directory`.
- **What clients take,** in order: what they are given (a flag, or Settings), the directory, then
  what the node's operator names (`--swap-contract`, `--dex-contract`, `--dex-router`). The node
  keeps to the same order: `GET /status`'s `swap` and `dex` are the directory's, a flag filling
  only what it leaves unset, and the flags show apart as `operator`, so no operator can point the
  apps somewhere other than what governance announced.
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
chainv4 send --key member.key vote-change --vote yes --collections standard
chainv4 send --key member.key vote-change --vote yes --collections '{"nft":["owner_of",…],"multi":["balance_of",…]}'
```

- **A ballot and version of its own,** as each kind has (see [A change](#a-change)).
- **It takes effect at once,** from the block that decides it: deploys, upgrades and refreshes
  register by it. Contracts deployed before join at their next upgrade or refresh.
- **The lists are checked as genesis's are:** each function named once in each list.
  `GET /status` shows what is in force, in `collections`.

## Fees

A change replaces the whole [fee configuration](fees.md), genesis's `fees` or the last decided,
in the shape genesis writes it (see [Genesis](genesis.md#fees)):

```sh
chainv4 send --key member.key vote-change --vote yes --fees '{"block_gas_limit":50000000,"burn_permille":500,"schedule":{"type":"weighted","per_tx":1000,"per_byte":10,"per_code_byte":20,"fuel_per_gas":1},"pricing":{"type":"base_fee","initial":"1000000000","min":"10000000","elasticity":2,"change_denominator":8}}'
```

- **A ballot and version of its own,** as each kind has (see [A change](#a-change)).
- **It takes effect from the next block:** the block after the one that decides it is checked,
  priced and charged by it. Under `base_fee` pricing, that block's base price follows from its
  parent's by the new rules; `initial` only starts a chain.
- **A chain without fees gets them** from a change; no change takes them away again.
- **It is checked as genesis's is.** `GET /status` shows what is in force, in `fees`.

## Reward

A change replaces what a block mints for its producer:

```sh
chainv4 send --key member.key vote-change --vote yes --reward '{"initial":"25000000000000000000","halving_interval":210000}'
```

- **It takes effect from the next block,** and halves from there: the block after the decision
  mints `initial`, and `halving_interval` counts from it, not from genesis. `null` never halves.
- **Blocks before keep what they minted.** The schedule is genesis's until the deciding block,
  the decided one after; the API's `reward` on a block reads it at the block's height.
- **A halving interval of 0 is refused,** as in genesis.

## Limits

A change replaces genesis's `tx_limits` and `max_block_bytes` together:

```sh
chainv4 send --key member.key vote-change --vote yes --limits '{"tx_limits":{"fuel":10000000,"memory":67108864},"max_block_bytes":4194304}'
```

- **It takes effect from the next block:** the transactions in it run under the new limits, and
  its size is checked by them. `max_block_bytes: null` bounds blocks by gas alone.
- **A change no contract could run under is refused:** fuel below 100,000, memory below 1 MiB, or
  `max_block_bytes` outside 1 MiB to 6 MiB, as genesis's.

## Finality

A change retunes finality, given as a whole genesis `finality` object (see
[Genesis](genesis.md#finality)):

```sh
chainv4 send --key member.key vote-change --vote yes --finality "$(jq -c '.finality | .min_bond = "500000000000000000000"' genesis.json)"
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
chainv4 send --key member.key vote-change --vote yes --switch-epoch 400 --switch-finality '{"type":"authority","checkpoint_interval":32,"vote_depth":2,"activation_delay":2,"max_signers":100,"signers":[…],"threshold":3,"change_threshold":3,"approval_epochs":64,"vote_reward":"0","max_proofs_per_block":4}'
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
chainv4 send --key member.key vote-change --vote yes --upgrade 0.2.0 --upgrade-from 150000
```

- **Older nodes stop before that block,** warning every ten minutes as it nears, and say to
  update; newer ones run on as before. It is advice to nodes, never a rule blocks are checked by.
- **`from` must be ahead** of the block deciding it, and the least version above 0.0.0.
- **A later upgrade replaces it.** It is kept in `change:upgrade` and shown as `upgrade` in
  `GET /status`.
- A chain without governance announces one in a release instead. See [Updates](updates.md).

## Forks

A fork changes what blocks compute from a height on, so a live chain takes a consensus change
without a new genesis: blocks below it run by the rules before it, blocks from it by its own, and
a node replaying history runs each block by its own height's (`params::forks`).

- **A fork is a name its release knows** (`FORKS`), with the code it changes asking whether it is
  in force (`Forks::has`, through `VmRules::forks`, set for every block from its parent's state).
  There's no most: `Forks` keeps a bit for each, in as many words as the list needs, and it's
  worked out per block rather than stored, so a release with more forks changes nothing a chain
  holds. This release knows three:
  - `restaking` (see [Finality](finality.md#restaking)): validators' rewards restaked into their
    bonds when their accounts turn it on, bonds topped up by hand, and the attestations of the
    justifications blocks carry checked as gossiped ones are.
  - `proofs` (see [Validity proofs](proofs.md#on-the-chain-the-proofs-fork)): each block records
    its parent for a proof to end at, the proven tip starts, and the chain takes `Tx::Prove` and
    `Change::Proofs`.
  - `memory` (see [Contracts](contracts.md#memory)): what a contract holds counted against its
    call's memory limit: a Lua storage iterator's pages and a wasm module's table entries.

  Tests also run a fork that exists in them alone.
- **Genesis may list forks** (`forks`, see [Genesis](genesis.md)), each its release knows, at a
  height or a finality epoch's first block.
- **Governance schedules one** at least the chain's lead ahead, a day of its blocks unless it set
  another, by height or by epoch, naming the least release that knows it. That also stops nodes
  older than that release there, as an [upgrade](#upgrade) does, unless an upgrade already
  stops them sooner. It moves or cancels one until it is within the lead; from then, and once
  passed, it is fixed.
- **A node whose release does not know a scheduled fork** stops before its height, warning as it
  nears as for an upgrade, and runs no block from it: a release that knows it is needed.
- **Locks hold it too:** `fork` locked freezes the schedule.

```sh
chainv4 send --key member.key vote-change --vote yes --fork-lead 2000
chainv4 send --key member.key vote-change --vote yes --fork faster-lua --fork-height 180000 --fork-least 0.3.0
chainv4 send --key member.key vote-change --vote yes --fork faster-lua --fork-epoch 22500 --fork-least 0.3.0   # moves it
chainv4 send --key member.key vote-change --vote yes --cancel-fork faster-lua
chainv4 send --key member.key vote-change --vote yes --change '{"fork":{"type":"schedule","name":"faster-lua","height":180000,"epoch":null,"least":"0.3.0"}}'
```

## Proofs

`Change::Proofs`, from the `proofs` fork on, names which validity proofs the chain takes: the proof
systems, one or more of `sp1-groth16`, `sp1-plonk` and `sp1-stark`, any of which it takes; the
program (its verifying key hash, from `chainv4 prove program`, the same for all three); what is
minted for each block a proof covers, and how Gateway.sol releases (`proof-or-federation`, `both`
or `federation-only`); and the finality and header-chain programs, with what is minted for each
epoch a header-chain proof adds ([Proofs of finality](proofs.md#proofs-of-finality)). Each proof carries the program and the mode at its range's end, so the contracts on
Ethereum follow them. Until it is decided, the chain takes no proofs. See
[Validity proofs](proofs.md).

```sh
chainv4 send --key member.key vote-change --vote yes --change '{"proofs":{"kinds":["sp1-groth16"],"program":"0x…","reward_per_block":"0","gateway":"proof-or-federation"}}'
```

To switch proof systems, vote the new one in beside the old (`["sp1-groth16","sp1-stark"]`), then
the old one out, so a prover halfway through a range isn't left with a proof the chain refuses.

## Bitcoin peg-ins

`Change::BtcPath`, from the `proofs` fork on, says which way new Bitcoin peg-ins go: `federation`,
the federation's deposit addresses as before; `bitvm`, BitVM's presigned graph alone; or `both`.
The gateway reads it as `federation.get().btc_path`, nil until decided. Peg-ins already made keep
their way. See [Bridges](bridges.md#bitvm-beside-the-federation).

```sh
chainv4 send --key member.key vote-change --vote yes --change '{"btc_path":"both"}'
```

## Attestations

`Change::Attestations`, from the `proofs` fork on, says whether validators attest links for
Ethereum: `true` as genesis has it, or `false`. Off, from the next block, no validator attests and
blocks carry no attestations, so ChainV4Finality.sol follows the chain no further. It's refused
while the proofs' gateway mode is `both`. See [Finality](finality.md#turning-attestations-off).

```sh
chainv4 send --key member.key vote-change --vote yes --change '{"attestations":false}'
```

The wallet's Governance page proposes the same under **Fork schedule**: schedule or move a fork by
block or by epoch, cancel one, or set the lead. The apps' own runs of contract reads go by the
forks in force at each read's height, from the schedule `GET /status` gives.

The schedule is kept whole in `change:forks`, genesis's written there as the chain starts when it
lists any, so a light client proves it as any setting. `GET /status` lists it as `forks`, and the
explorer's network page each fork, when it runs from and what it changes.

**What a fork may change:** anything a block computes, each looked up by the block's height where
it applies: the VM's rules, fuel and host costs, the fee schedule, the limits, the checks a
transaction must pass. A new transaction kind or encoding is a new tag that only blocks from its
fork take. A change of an engine itself, such as another Luau build, would run beside the old one
for the blocks before it.

## Name

A change renames the chain, replacing genesis's `name` or the last decided:

```sh
chainv4 send --key member.key vote-change --vote yes --chain-name "chainv4 mainnet"
```

- **It takes effect at once,** from the block that decides it: only the apps read it, as
  `GET /status`'s `name`.
- **It is checked as genesis's is:** 1 to 40 characters, no control characters, no space at
  either end. A chain whose genesis named none gets one this way.
- It is kept in `change:name`, beside genesis's, so a light client proves either.

## Governance itself

A change of governance's own settings, decided by those who vote now:

```sh
chainv4 send --key member.key vote-change --vote yes --governance '{"members":["ed25519:0x…","ed25519:0x…"],"threshold":2}'
chainv4 send --key member.key vote-change --vote yes --governance '{"quorum_permille":500}'
```

- **The kind stays.** A council stays a council and the electorate the electorate; a council's
  change names its members, threshold, `approval_epochs` and `quorum_permille`, the electorate's
  only the last two.
- **It counts votes from the next block,** read from that block's parent's state like the other
  settings. The rest of the deciding block refuses votes, which would be cast under one set of
  rules and counted under another.
- **Every open vote is cleared,** of every kind: each was cast for, and weighed by, settings that
  are gone, and a smaller council or a lower threshold must not decide on them. Versions stay, as
  nothing they name changed.
- It is kept in `change:governance`, with the block it counts from.

## Tokens

`--change '{"token_functions":["balance_of","transfer",…]}'`, or `"standard"`, replaces genesis's
`token_functions`, as `Change::TokenFunctions`. A kind of its own rather than part of collections:
the two registries serve different contracts and apps, and neither should restate the other.
Deploys, upgrades and refreshes register by it from the next block.

## EVM spec

Genesis's `evm_spec` (`shanghai`, `cancun`, `prague` or `osaka`; `prague` when unset) sets the
EVM's hard fork, and a change moves to another from a height:

```sh
chainv4 send --key member.key vote-change --vote yes --change '{"evm_spec":{"spec":"osaka","from":250000}}'
```

`from` must be ahead of the deciding block, and the spec no older than the one in force or
decided last, so no deployed contract loses an opcode. Each spec decided is kept with its height
in `change:evm-spec`, so a block below it replays as it ran.

## DEX fees

Genesis's `dex` and `Change::Dex` set both DEXes' fee tiers and the share of each fee burned:
`{"tiers": [5, 30, 100], "protocol_permille": 0}` when unset.
- **`tiers`:** one to four fees a new pool may open at, in hundredths of a percent, 1 to 1,000
  each, smallest first. A pool keeps its tier when a vote takes it off the list.
- **`protocol_permille`:** the thousandths of each trade's fee burned, 0 to 1,000.

The wallet's form takes the tiers as a list and the share as thousandths; each may be locked,
`dex.tiers` and `dex.protocol_permille`. See [DEX](dex.md#fee-tiers-and-the-burned-share).

## Engines

Genesis's `engines` and `Change::Engines` set which engines contracts deploy and run on: any of
`lua`, `wasm` and `evm`, each once, several at once, or none. It replaces the set whole, from the
block after the one that decides it:
- **An engine turned off freezes its contracts.** Every call to one is refused, from a
  transaction or another contract of any engine, with "the chain has turned the lua engine off:
  its contracts are frozen until it is on again"; nothing deploys or upgrades on that engine. Their
  storage, code and balances stay, readable through the API and provable, and nothing is lost.
- **Turning it on again** brings every one back as it was.
- **A node built without an engine the chain turns on stops** once the change is decided, before
  the next block, saying to build with the engine's feature: it could not run that block as every
  other node does. A node refuses a genesis naming an engine it lacks, as ever. A node runs every
  engine it was built with, and the block's rules say which are on, so turning one on needs no
  restart where the build has it.
- **Lockable** as `engines`.

`chainv4 send vote-change --change '{"engines": ["lua", "evm"]}'` votes on one from the CLI, as
the wallet's Governance page does.

## Anchoring

Genesis's `anchoring` and `Change::Anchoring` set which anchors of the chain's checkpoints in
Bitcoin the chain takes, whether they bind nodes, the relay and start they are checked against, how
deep, and the reward, each part on its own and all of it off when unset. In force from the block
after the decision. Anchors recorded while `enforce` was on stay enforced. See
[Anchoring](anchoring.md#settings).

## Locks

A lock holds a setting against every vote, for good. Genesis names the first (`locked`, see
[Genesis](genesis.md#fields)); a change of locks adds more:

```sh
chainv4 send --key member.key vote-change --vote yes --lock anchoring.enforce,federation.threshold
chainv4 send --key member.key vote-change --vote yes --change '{"locks":["reward"]}'
```

- **A lock is a path:** a change's name alone, which holds all of it, such as `reward` or
  `upgrade`, or followed by the dotted names of its fields as `GET /governance` writes the change,
  which holds that field and anything under it: `anchoring.enforce`, `fees.burn_permille`,
  `limits.tx_limits.fuel`, `fees.costs`. A vote may change everything else in the kind.
- **What is compared:** the change's value at each locked path of its kind, against the value in
  force there, as `GET /governance`'s `in_force` shows it: the consensus fixed for the epoch, the
  finality it runs, and of every other setting the last decided or genesis's. JSON against JSON,
  so every kind is held alike; a field left out is the same as `null`. A change whose value
  differs at any locked path is refused, with "`anchoring.enforce` is locked".
- **When:** at every vote on a change, Yes, No or Abstain, and so again at the vote that would
  decide it, against what stands then. A lock decided after a change's first votes holds that
  change too: its deciding Yes is refused, and its votes expire. Clearing a vote is never refused.
- **Variants.** A path under a kind whose `type` picks its fields names that field in whichever
  variant has it, and a change of variant is refused as soon as what it holds there differs, a
  field the new variant lacks or adds included. With `consensus.slot_ms` locked under an
  authority producing every 2 s, the chain may move to `staked` or `rotating` at 2 s, never to
  `pow`, which has no slot; with `consensus.target_block_ms` locked, it never moves to proof of
  work at all, since nothing stands there. The same goes for `fees.schedule`, `fees.pricing` and
  finality's kinds.
- **The variant kinds whole.** `consensus` refuses any change of the consensus, `finality` any
  retune and any switch, `upgrade` any upgrade. A switch of finality is locked whole, as
  `switch_finality`: no switch stands to compare one with, so `switch_finality.epoch` names
  nothing. What a switch sets is held by the locks under `finality`, against the finality that
  runs: `finality.type` refuses every switch, `finality.vote_reward` one that pays otherwise.
- **Locks only grow.** A change of locks names the paths to add, in order, each once, none locked
  already by itself or a lock over it (`dex.tiers` under `dex` is refused, `dex` over
  `dex.tiers` is not). Deciding it adds them to what stands, from the next vote on; nothing
  takes one away, governance changing itself included. A lock on `locks` freezes the set: no more
  may be added.
- **Any kind may be locked,** consensus and upgrade included. A chain that locks `upgrade` can
  never announce an upgrade, and one that locks `consensus` never changes how blocks are made:
  any fix to either needs a new genesis, which such a chain accepts.
- **A path that names nothing is refused,** in genesis and in a change of locks: an unknown kind
  (`colour`), a field no variant of the kind has (`reward.initia`), or a field of a list or string
  (`bootnodes.0`).
- **One state entry,** `change:locks` in the validators column, holds them all, genesis's written
  there as the chain starts while it names any, so a genesis without locks keeps its id and its
  state, and a light client proves them. `GET /governance` lists them as `locks`.
- **The tools refuse first.** `send vote-change` refuses a change that breaks a lock, in the
  chain's words, before it signs; the wallet's propose form shows a locked field read-only and
  marked locked, and a kind locked whole as locked in its list; the explorer's Governance page
  lists the locks and marks each setting locked whole.

## What else each kind carries

| Kind | Fields | What they do |
| --- | --- | --- |
| `fees` | `vote_rewards` | `mint` or `fees`: where vote rewards come from, in place of genesis's `vote_reward_source`. |
| `fees` | `costs` | Every host operation's fuel and the EVM native and names precompiles' gas, as `HostCosts`; each field left out takes the default. |
| `limits` | `contract` | `call_depth`, `key_len`, `value_len`, `scan`, `event_data`, `events`, `module_len`, `source_len`. |
| `limits` | `max_tx_bytes`, `multisig_members` | The most bytes a transaction takes (Ethereum-signed ones by their raw bytes, 128 KiB while unset), and the most members of a new multisig, at most 20. |
| `finality` | `slash_permille` | How much of a cheater's FFG bond a slash takes, 1000 by default. |
| `finality` | `justify_permille` | How much of the bonded weight justifies a link under FFG, 667 to 1000, two thirds unset. From the `proofs` fork on. |
| `consensus` | `window`, `adjustment`, `max_steps_down`, `max_swing` | Proof of work's retarget window and steps; unset, genesis's window and 64, 32 and 4. |
| `federation` | `btc` | The Bitcoin policy: `fee_cap`, `fee_floor`, `grace_ms`, `cancel_after_ms`, `fee_part`, `bump_after_ms` and `bump_below_pct`, when a stuck release is bumped, an hour and 80 percent of what the network asks when left out, and `repay_pct` and `repay_most`, how much of its fee a release repays at most of what children cost the backing and the most one withdrawal repays, 20 percent and 500 satoshis when left out, as a policy written before them is read (see [Bridges](bridges.md#bumping-a-stuck-release)). |

Genesis carries each too, unset meaning what every chain ran before (`contract_limits`,
`max_tx_bytes`, `multisig_members`, `evm_spec` and `dex` at its top level, the rest inside fees,
finality, the consensus and the federation). Each holds from the block after its decision.

- **Lowering a limit can break deployed contracts.** A contract that stores 64 KiB values, or
  nests calls 8 deep, would fail where it once worked; so a change never lowers `call_depth`,
  `key_len`, `value_len`, `scan`, `event_data` or `events` below what stands. `module_len` and
  `source_len` bound only new deploys, and may go either way. WebAssembly's recursion and stack
  stay fixed: they are the engine's own settings, shared by its module cache.
- **Raising `multisig_members` past 20 is refused,** as CHECKMULTISIG's most: every multisig keeps
  its Bitcoin address. A multisig made under a higher limit keeps its members.
- **How far a block may run ahead of the clock** follows the consensus's block time: three blocks,
  never under 15 s, so 15 s on 5 s blocks and three minutes on 60 s ones. It is checked against
  the local clock alone, so a change never makes the past invalid.

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
