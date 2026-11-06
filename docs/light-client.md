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
and keeps the eras it learned in its saved state (`LightClient::consensus_at`). Should governance
switch finality's kind, the update that fixes an epoch of the other kind names it
(`LightUpdate::switched`), and the client proves that epoch's voters at the other kind's key and
weighs them as that kind does: validators by bond, two thirds deciding, or signers by threshold.
A state fixes only one kind's voters for an epoch, so a node that hides a switch, or claims one,
sends a proof that fails.

**Reorgs.** When the node's chain no longer links to the client's, the client steps back until it
finds where they split. It follows the new branch only if it holds more work, and never past a
justified checkpoint (`ConflictsWithFinality`). Under FFG or authority finality, nothing past the
justified checkpoint is settled, and the voters settle it, not work: so where the node's headers
stop linking above that checkpoint, the client goes back to it and follows the node from there,
every header checked, rather than stepping back a whole batch, which took it below the
checkpoint and read as leaving it.

**A branch the voters left.** Two miners can find blocks at one height, and the two branches may
weigh the same, so work alone keeps a client on the one it saw first. Under FFG or authority
finality the voters decide: a justification whose votes check (from a justified source, the
target epoch's voters, enough weight), but for a block other than the one the client holds at
that checkpoint's height, means the client followed a branch the chain left. It drops its headers
past its own justified checkpoint and follows the node again from there, checking every header
as ever, and takes the justification once its headers are in. Only votes that hold move it, so a
node cannot make it go back by saying so.

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

## Checking a range

[`GET /blocks/range`](api.md#get-blocksrangefromhcountnentrye) sends blocks with proofs of the state
entries each changed, so a client takes what it wants from blocks it checked, and the node does not
learn what it looked at. All of this is in the light build and in wasm:

- **`decode_block_range(bytes)`** reads a response, safely on any bytes, up to `MAX_RANGE_DECODE`.
- **`BlockRange::check_shape(from, entry)`** checks the answer is the one asked for: consecutive
  heights, no block left unfinished before another, and a cursor that moves on.
- **`check_range_item(hash, header, item)`** checks one item against a header already checked: that
  it is that header, that its body recomputes `tx_root`, `proof_root` and `pool_root`, that its
  receipts recompute `receipts_root`, each for the transaction at its place, and that each entry's
  proof holds against `state_root`. `LightClient::check_range_item(item)` does the same with the
  header the client followed at that height, and says how far to trust it.

It takes the item and gives back a `CheckedItem`, so only what checked is left to read; or a
`RangeError` that names what failed, and never quotes the bytes.

**What this does not tell you:**
- **Whether an entry was left out.** A header commits to the state, not to which entries a block
  changed, so a node can leave an entry out of its list, or claim `NoList`, and every check passes.
  Everything a client is given is true of the chain; that it was given everything is the node's word.
  A receipt cannot be left out so: `receipts_root` covers them all.
- **Blocks with no proofs** are said so per block, in `RangeChanges::None`: a client must treat what
  they changed as unknown, not unchanged.
- **Genesis** has no list, so what it allocated is not in a range.

**Reading older blocks than it holds.** A light client holds few headers below the finalized block it
saved, and fetching one for every block down to a height months back is more than a phone keeps. A
header's id is named by the one above, so `chainv4_ui::blocks` reads older blocks by stretches from the
top down: each item is checked by the header it came with, and nothing of a stretch is used until the
whole of it links up to a block already checked, whose parent the next stretch then ends at. Newer
blocks go up from there, each against the header the client followed at its height. The wallet's
[Read from checked blocks](wallet.md#read-from-checked-blocks) is built on it.

## Checking values

- **State:** `check_state` checks a chain-state entry, and its proof from
  `GET /proofs/{column}/{key}`, against a block it followed. `?block=` proves at a recent block
  rather than the tip, and the client asks at the finalized checkpoint: a value proven there is
  final. A node can prove at any block whose state it keeps, `--state-history` blocks back.
- **Receipts:** `check_receipt` checks a receipt, and its Merkle path from
  `GET /proofs/receipts/{id}`, against the block's receipts root: what a transaction did, and
  what its contracts paid, line by line. `chainv4 light history` shows those of the address.
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
Each run keeps what it checked in `.chainv4/light/<genesis id>.bin` in the home directory, or
the file `--state` names, written beside it and renamed over, and the next run carries on from
there, checked against the genesis it trusts and its headers linking up; one that does not restore
is replaced. `--fresh` starts again from genesis, or `--checkpoint`, as does `--checkpoint` itself.
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

**Fast resume** skips the walk on a node you say you trust for it. Settings has a switch per node,
**Trust this node for fast resume**, off by default. On, and with the saved client more than 8
epochs behind the node's finalized checkpoint (`GET /status`), the wallet takes up the chain there,
from `GET /light/checkpoint`, rather than walking the headers since:
- **What still holds:** the genesis pinned at the node, and `from_checkpoint`'s every check. From
  the checkpoint on, each header is checked by consensus and each justification by its voters'
  signatures, as for any client. Older receipts still prove down the hash chain.
- **What it gives up:** nothing ties that checkpoint to what the client checked before. A dishonest
  node could show a false history from that point. So it is said wherever the client is: the sidebar
  and Settings say **Fast-resumed at block N, on this node's word**.
- **When it does not:** with the switch off; a few epochs behind, where walking costs a request or
  two; under depth finality, where no voters' signatures check it from there on, and nothing signed
  says which block is final, and on a chain with no finality; on a node that names no checkpoint or
  has none to start from, an older one included, which walks on from what was saved; and when the
  checkpoint is behind what the client proved final, or at odds with a block it proved final.

## In the explorer

The explorer runs the same client against the node it talks to:
- **The sidebar,** or Settings on a phone, says whether it is starting, catching up, following,
  or stopped, and why it stopped.
- **An account's page** shows the balance it proved, at the finalized block and at the tip, and
  whether each is final. A proof that does not check is shown in red.
- **Fast resume** is there too: the same switch per node, off by default, the same decision
  (`chainv4::light::may_resume`) and the same words. As the explorer saves no client, on a trusted node
  it starts at the node's finalized checkpoint each launch, rather than walk from genesis: the genesis
  stays pinned, and headers and voters' signatures are checked from there. A checkpoint pasted comes
  first; under depth finality or none, or on a node with no checkpoint to give, it walks.

## Staying up to date

The wallet and the explorer learn of new blocks from the node's [`GET /stream`](api.md), where it has
one, and ask for headers when an event says the chain moved: a block, a reorganization, a justified or
finalized checkpoint, or `lagged`, for events they missed. They ask for more every 20 seconds besides,
in case an event went missing. Without a stream, or while it is lost, they ask every 3 seconds in the
wallet and 4 in the explorer, and try the stream again every 10.

- **The stream is trusted for nothing.** Only an event's name is used. Headers, blocks, transactions
  and proofs come over HTTP and are checked as ever, so a node that lies on its stream can make a
  client ask early, or find out late by up to those 20 seconds, and no more.
- **Walking does not change.** A client behind asks for what it wants next until it catches up, and
  events make no difference to that.
- **It tells the node nothing new:** that a client is there, which its requests say anyway. Through a
  proxy, the stream goes through the proxy too.
- **Balances keep their own pace.** A client that knows of a new block proves what it shows on its
  usual round, not at each event.
- **An offline signer never opens one.** It makes no requests at all.

## Batches

A [batch](chain.md#batches)'s receipt proves as any other's does: what each step did, and how many of
its events and payments are that step's, or which step stopped it. Why it failed is not committed to,
as no failure's message is. A client older than batches cannot read one: its proof of that transaction
fails, visibly, and it follows the chain's headers as before.

## What to keep in mind

- **It asks only for what it can check.** On a chain with governance it knows the rules and voters
  only about two epochs ahead of what it has proved, so it asks for that many headers and for finality
  only as far as those headers, rather than `MAX_LIGHT_HEADERS` and `MAX_FINALITY_STEPS` it would send
  back and ask for again. A round of two requests for every 16 blocks costs about 23 KB rather than
  1 MB.
- **It asks for it all at once when the node can.** Over a real network a walk's time is round trips:
  the node answers in about a millisecond and the client spends a few seconds of CPU on thousands of
  blocks. `GET /light/bundle` carries 32 epochs of headers and finality in one answer, which the client
  takes through its usual steps as `next` asks for them, checking each as ever. A node without the
  route answers 404 and the client goes back to a request at a time.
- **It fetches every header from genesis, the first time.** Headers are small, but there are as
  many as blocks. The CLI and the explorer follow the chain afresh each time, the explorer from a trusted node's
  checkpoint if its fast resume is on; the wallet saves the
  client and resumes from the finalized block, walking every header since, or on a node trusted for
  fast resume, from that node's own checkpoint.
- **Epochs can go unjustified for good,** as when too few voters are up: the next justification
  then comes from a source below them. A node's `GET /light/finality` says so with a step without a
  justification, and ends only where nothing justified follows, looking 4,096 epochs ahead. The
  voters and, under governance, the rules past the gap are in the skipped checkpoints' updates, which
  no justification covers, so the client takes them tentatively, proven against the headers it
  followed, follows headers under those rules, and takes the next justification only if the newest
  voters it already trusts carry what they need too. Then it asks for the same updates again as
  trusted ones. So crossing a gap rests on those same voters, and voters who changed by more than
  that across a gap leave a client unable to cross it: start it at a checkpoint past the gap. Until
  it crosses, what it knows only tentatively is neither justified nor saved. Clients from before stop
  at the gap, as they did.
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
