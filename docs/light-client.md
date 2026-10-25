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

## Following the chain

**Headers.** It fetches the node's main-chain headers with their seals (`GET /headers`) from
genesis on, and checks each one with the chain's `Consensus::check`, as a node checks a block
before running it:
- that it follows its parent;
- its producer, timing and difficulty;
- its seal: the authority's signature, or the proof of work.

It adds up total work as it goes.

**Reorgs.** When the node's chain no longer links to the client's, the client steps back until it
finds where they split. It follows the new branch only if it holds more work, and never past a
justified checkpoint (`ConflictsWithFinality`).

**Finality,** under FFG or authority finality:
- **Justifications** come from the node (`GET /justifications`). The client checks each as a node
  does:
  - its target and source are checkpoints on the client's chain, and its source is justified;
  - the votes are sorted, each one for that link, and each signature checks;
  - they come from the target epoch's voters, and their weight adds up to enough: two thirds of
    the bonded weight under FFG, the threshold of signers under authority finality.
  - The source of a direct successor is finalized.
- **Voters** come from proofs, never the node's word. Each epoch's voters are kept in chain state,
  fixed `activation_delay` epochs ahead by the checkpoint `activation_delay` epochs before them, or
  by genesis for the first few.
  - **Light updates:** when a node runs genesis or a checkpoint, it records one (`LightUpdate`, in
    the `Light` column): proofs of the voters that block fixed, against its state root. It serves
    it at `GET /light/updates/{block}`.
  - **Taking one:** the client takes a block's update once it trusts the block's state, that is,
    once the block is at or below the latest checkpoint it saw justified. So trust runs forward
    from genesis:
    1. genesis's voters;
    2. then the first justifications, which make checkpoint 1 trusted;
    3. then its update, with the voters of epoch 1 + `activation_delay`;
    4. and so on.
- **Depth finality** has no votes: a block is final once `depth` blocks are on top of it.

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
- **What it says:** each check gives an `Assurance`: the block, its height, and whether it is
  final. A proof that does not check is `BadProof`, and a block the client never followed is
  `NotOnChain`.

## Saving and resuming

`save` gives a `LightState`: everything the client checked, to encode and keep, and `restore`
takes it back with the genesis id to trust, refusing a state from any other chain
(`WrongGenesis`). It keeps headers only from the finalized block on, at most 4,096 of them, so it
stays small:
- **Older blocks are behind it.** A restored client cannot prove at a block before what it kept;
  ask for the finalized block or the tip, and keep what was proven before, as the wallet does.
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

`history` lists from the node's address index, and proves each transaction and its receipt before
it prints them: the node only says where to look.

## In the wallet

The [wallet](wallet.md) is built on it: it follows the chain from the genesis it pinned, proves
every balance, token and transaction it shows, and saves the client between runs, so it resumes
where it was.

## In the explorer

The explorer runs the same client against the node it talks to:
- **The top bar** says whether it is starting, catching up, following, or stopped, and why it
  stopped.
- **An account's page** shows the balance it proved, at the finalized block and at the tip, and
  whether each is final. A proof that does not check is shown in red.

## What to keep in mind

- **It fetches every header from genesis, the first time.** Headers are small, but there are as
  many as blocks. The CLI and the explorer follow the chain afresh each time; the wallet saves the
  client and resumes from the finalized block.
- **Snapshot nodes cannot serve the finality before their pivot.** A node that synced from a
  snapshot never ran the checkpoints before it, so it has no light updates for them. A client
  following finality from genesis needs a node that ran them. Headers and proofs at the tip still
  check against such a node.
- **Proving at a block takes the node's undo records,** so only the last `max_reorg_depth` blocks
  can be proven at. Final values need the finalized checkpoint to be within that.
- **Trust on first use, in the explorer and the wallet.** They trust whatever genesis they first
  see at a node; the wallet remembers it, and takes a pasted one instead. To trust a chain from
  the start, give the CLI a genesis file or id.
