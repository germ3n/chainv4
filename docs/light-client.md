# Light client

A light client checks what a node tells it without running the chain. It trusts one thing, the
genesis id, and checks everything else it is sent against it:
- every header, by the chain's consensus;
- finality, from justifications whose voters it proved itself;
- any value or receipt, with its proof, against a header it already checked.

A node that lies is caught: what it sends does not check.

The code is `chainv4::light` (`src/light/`). It builds without the node, so the explorer runs it
in the browser. It is sans-IO: `LightClient::next` says what to fetch, and the `deliver_*` and
`take_*` methods check what came back. The CLI drives it over HTTP with one library and the
explorer with another.

## What it trusts, and how it starts

**The genesis id.** You give it: a genesis file (`--genesis`), a development chain
(`--consensus`), or an id you got from someone you trust (`--genesis-id`). The explorer pins the
genesis it sees the first time it meets a node, and refuses that node from then on if it changes
(trust on first use).

**The parameters.** To check headers it needs the chain's hash, its consensus and how its
finality works: `LightParams`, which any node sends at `GET /light`. Genesis commits to them, since
the genesis block's `proof_root` is the hash of its `light_params()`. So a client holding the
genesis id checks that the genesis header hashes to it and that the parameters hash to its
`proof_root`, then trusts both. A node cannot give it other parameters for the same id.

## Starting at a checkpoint

A client need not follow from genesis. Given a finalized checkpoint's block id it trusts, as it
trusts the genesis id, it starts there, one request away (`LightClient::from_checkpoint`), then
follows on and checks everything from there as before:
- **What the node sends** (`GET /light/checkpoint/{block}`): the checkpoint's header and those
  below it back to the checkpoint `activation_delay` epochs before, genesis for the first few,
  and further back when consensus looks at more ancestors than that leaves, with each of those
  checkpoints' light updates. A `CheckpointStart`.
- **What the client checks:** that the headers link up by their hashes to the block id it trusts,
  so it trusts every one of them, and each update against its header's state root, as
  `take_update` does. The updates name the voters of the epochs after the checkpoint, so the next
  justifications check as they would for a client that followed from genesis.
- **What it refuses:** headers that end at another block, have a gap, or start too late to prove
  the voters (`WrongCheckpoint`), a forged update (`BadProof`), and under voting finality a block
  that opens no epoch. Under depth finality, or none, any block is a checkpoint, and it needs that
  header alone.
- **What it never does:** go below the checkpoint. It is justified and finalized for the client,
  as genesis is for one that starts there. It still takes older headers on demand, proven by their
  hash chain, for receipts below it.

`checkpoint_start(finality, height)` says which epoch a start begins at, and
`checkpoint_first_header` which header it begins with once consensus's ancestors are counted, for
nodes and clients alike. The headers below the epochs' start are kept as the client's ancestors:
trusted by their hashes, looked at by consensus, never followed from.

**Where the id comes from:** someone you trust, or the explorer's Finality page and `GET /status`
of a node you trust (`finality.finalized.block`). The wallet and the explorer also take a node's
own finalized checkpoint (`GET /light/checkpoint`) the first time they meet it, when they pin its
genesis on first use too, as their trust in that node is the same then; never once a genesis was
given by hand.

On the demo chain at 7,900 blocks, `chainv4 light --checkpoint` takes 6 requests where following
from genesis takes 26.

## Following the chain

**Headers.** It fetches the node's main-chain headers with their seals (`GET /headers`) from
genesis on, and checks each one with the chain's `Consensus::check`, as a node checks a block
before running it:
- that it follows its parent;
- its producer, timing and difficulty, against as many ancestors as consensus looks at
  (`LightParams::lookback`: the proof-of-work window, or the parent alone);
- its seal: the authority's signature, or the proof of work.

It adds up total work as it goes. A saved client keeps the ancestors just below its first header
(`LightState::ancestors`), so it checks the next header as a node would.

**Under a rotating authority,** a header's producer must be among its epoch's signers, which the
client only knows once it proved them: from genesis's update for the first epochs, then from the
update of the checkpoint `activation_delay` epochs before, trusted once that checkpoint is
justified. A header it cannot check yet stops the batch (`Extended::Waiting`, or fewer added);
the client keeps what it checked, asks for finality, and asks for those headers again once the
signers are known. So it follows the chain `activation_delay` epochs behind finality. Production
by stake works the same way, with the epoch's validators and their weights.

**Under governance** (`LightParams::governance`), the chain's consensus may change, so each
epoch's consensus comes from a light update too (`EraProof`), fixed and trusted as the epoch's
voters are, and a header whose epoch's consensus the client does not know yet waits the same way.
The client builds the rules for each consensus it learns, checks every header by its own epoch's,
and keeps the eras it learned in its saved state (`LightClient::consensus_at`).

**Reorgs.** When the node's chain no longer links to the client's, the client steps back until it
finds where they split. It follows the new branch only if it holds more work, and never past a
justified checkpoint (`ConflictsWithFinality`).

**Finality,** under FFG or authority finality:
- **Justifications** come from the node, with the light updates, many epochs to a request
  (`GET /light/finality`, see [Asking in batches](#asking-in-batches)). The client checks each
  as a node does:
  - its target and source are checkpoints on the client's chain, and its source is justified;
  - the votes are sorted, each one for that link, and each signature checks;
  - they come from the target epoch's voters, and their weight adds up to enough: two thirds of
    the bonded weight under FFG, the threshold of signers under authority finality.
  - The source of a direct successor is finalized.
  - **One past the headers it followed waits.** A node can justify a checkpoint after sending
    the client its headers, and send that justification with the rest. The client stops there,
    with no error, and the next refresh brings the headers and the justification together.
- **Voters** come from proofs, never the node's word. Each epoch's voters are kept in chain state,
  fixed `activation_delay` epochs ahead by the checkpoint `activation_delay` epochs before them, or
  by genesis for the first few.
  - **Light updates:** when a node runs genesis or a checkpoint, it records one (`LightUpdate`, in
    the `Light` column): proofs of the voters of the epochs ahead, against its state root. Genesis
    carries epochs 1 through `activation_delay`; a checkpoint of epoch k carries epochs k + 1
    through k + `activation_delay`, the last of which it fixed, the rest re-stated, since a slash
    or an ejection takes a voter out of sets fixed earlier. A client that took them before
    replaces them. It serves it with the justifications at `GET /light/finality`, and alone at
    `GET /light/updates/{block}`.
  - **Taking one:** the client takes a block's update once it trusts the block's state, that is,
    once the block is at or below the latest checkpoint it saw justified. So trust runs forward
    from genesis:
    1. genesis's voters;
    2. then the first justifications, which make checkpoint 1 trusted;
    3. then its update, with the voters of epochs 2 through 1 + `activation_delay` (genesis's
       carries epochs 0 through `activation_delay`);
    4. and so on.
- **Depth finality** has no votes: a block is final once `depth` blocks are on top of it.

### Asking in batches

Trust runs forward an epoch at a time: a justification makes its checkpoint's update trusted, and
the update names the voters of the justifications `activation_delay` epochs on. Asked one by one,
that was a request per update and one per stretch of justifications: 1,121 requests to follow
the demo chain's 6,400 blocks, slow from a phone, where each one takes a round trip.

So `LightClient::next` asks for `LightRequest::Finality { from }`, and the node sends 64 epochs
of both, in order (`FinalityStep`). `deliver_finality` checks each epoch's justification, then its
update, exactly as `deliver_justifications` and `take_update` do one at a time, and stops without
an error where the node justified no further, at one past the headers, or at voters it cannot
know yet. The same chain takes 23 requests: 8 for headers and 14 for finality. A forged update or
justification is refused as before.

## Checking values

- **State:** `check_state` checks a chain-state entry, and its proof from
  `GET /proofs/{column}/{key}`, against a block it followed. `?block=` proves at a recent block
  rather than the tip, and the client asks at the finalized checkpoint: a value proven there is
  final. A node can prove at blocks within its last `max_reorg_depth`, whose undo records it keeps.
- **Receipts:** `check_receipt` checks a receipt, and its Merkle path from
  `GET /proofs/receipts/{id}`, against the block's receipts root: what a transaction did.
- **Transactions:** `check_tx` checks the signed transaction the same answer carries, and its path,
  against the block's `tx_root`: who sent what, which the receipt does not say. So a plain coin
  transfer to an account is proven down to its sender and amount.
- **Token balances:** by the [token standard](token-standard.md#provable-balances), each holder's
  balance is in the token's `balances` namespace under the holder's address text.
  `token_balance_key(token, holder)` is its key in `Column::Namespaces`, for `check_state`, and
  `token_balance(value)` reads what it holds, whichever engine wrote it.
- **NFTs:** by the [NFT standards](nft-standard.md#provable-ownership), an NFT's owner is in its
  collection's `owners` namespace, `nft_owner_key(collection, id)`, read with `nft_owner`; a
  multi-token amount in its `balances` namespace, `multi_balance_key(collection, id, holder)`,
  read with `token_balance`.
- **What it says:** each check gives an `Assurance`: the block, its height, and whether it is
  final. A proof that does not check is `BadProof`, and a block the client never followed is
  `NotOnChain`.

## Saving and resuming

`save` gives a `LightState`: everything the client checked, to encode and keep, and `restore`
takes it back with the genesis id to trust, refusing a state from any other chain
(`WrongGenesis`). It keeps headers only from the finalized block on, at most 4,096 of them, so it
stays small:
- **Older blocks are behind it,** until it takes their headers back. `deliver_ancestors` takes the
  headers just below the lowest it holds, from `GET /headers`: each must hash to what the one above
  names as its parent, the top one to the parent of the lowest it holds. The hash chain proves
  them, since every block above was checked already. Then `check_receipt`, `check_tx` and
  `check_state` work at those blocks too, and `lowest` says how far down it holds. Ancestors are
  not saved. The wallet takes them when a receipt lands below what it holds.
- **Finality goes on from where it was:** the voters and justified checkpoints it needs come with
  it, so it does not fetch from genesis again.

## On the command line

`chainv4 light` follows a node from the genesis you trust and answers only with what it proved:

```sh
chainv4 light --consensus authority status
# followed 45 headers to 45 0x7417…, each checked by authority
# justified epoch 5 0x23f4…, finalized epoch 4 0xb379…

chainv4 light --consensus authority account ed25519:0xa9d1…
# balance 1000003000000000000000000, nonce 0: proven at block 32 0xb379…, which is final
# balance 1000005000000000000000000, nonce 0: proven at block 45 0x7417…, not final yet

chainv4 light --genesis-id 0x5f0d… receipt 0x4b6f…
# 0x4b6f… from ed25519:0xa9d1…: transfer 5 from ed25519:0xa9d1… to ed25519:0x3b6a…
# done, 0 events, gas 3620, fee 36200000000: proven at block 51 0xdf6d…, not final yet

chainv4 light history ed25519:0x3b6a… --limit 5   # needs a node started with --index-addresses
#       51 0x4b6f… transfer 5 from ed25519:0xa9d1… to ed25519:0x3b6a…: done [not final]

chainv4 light entry tokens 0x…            # any chain-state entry, by column and key in hex
```

`--node` names the node, which it does not trust. Without `--genesis` or `--genesis-id`, it
trusts the development chain `--consensus` names, `pow` by default, as a node does.
`--checkpoint 0x…` starts at a finalized checkpoint you trust rather than at genesis:

```sh
chainv4 light --genesis genesis.json --checkpoint 0xee85… status
# followed 16 headers from the checkpoint at 7904 to 7920 0x2de3…, each checked by authority
```

`history` lists from the node's address index, and proves each transaction and its receipt before
it prints them: the node only says where to look.

## In the wallet

The [wallet](wallet.md) is built on it: it follows the chain from the genesis it pinned, proves
every balance, token and transaction it shows, and saves the client between runs, so it resumes
where it was. It starts at a checkpoint pasted in Settings, and the first time it meets a node
with no genesis given, at the node's own finalized checkpoint.

## In the explorer

The explorer runs the same client against the node it talks to:
- **The sidebar,** or Settings on a phone, says whether it is starting, catching up, following,
  or stopped, and why it stopped.
- **An account's page** shows the balance it proved, at the finalized block and at the tip, and
  whether each is final. A proof that does not check is shown in red.

## What to keep in mind

- **It fetches every header from genesis, the first time.** Headers are small, but there are as
  many as blocks. The CLI and the explorer follow the chain afresh each time; the wallet saves the
  client and resumes from the finalized block.
- **Snapshot nodes serve the finality before their pivot** as their own light client checked it:
  each update proven against its block's state root and holding every epoch's voters it should.
  A node that synced from one that lacked them lacks them too.
- **An update must hold every epoch's voters** its block fixes, each once: one with voters left
  out is refused, though each proof in it holds, so a node cannot stall a client by holding some
  back.
- **Proving at a block takes the node's undo records,** so only the last `max_reorg_depth` blocks
  can be proven at. Final values need the finalized checkpoint to be within that.
- **Dev chains share a genesis id.** Every chain started from the built-in dev genesis has the
  same one, so a wallet that followed one dev chain resumes its saved client against the next, and
  waits for blocks past where the old chain got to. Settings → Light client → **Start over** in the
  wallet follows the node's chain afresh. A chain of your own, from its own genesis, has its own id.
- **Trust on first use, in the explorer and the wallet.** They trust whatever genesis they first
  see at a node; the wallet remembers it, and takes a pasted one instead. To trust a chain from
  the start, give the CLI a genesis file or id.
