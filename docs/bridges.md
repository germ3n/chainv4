# Bridges

Bridges carry value between chainv4 and other chains as wrapped assets: BTC as wBTC here, ETH and
ERC-20s and NFTs here as tokens and collections, and chainv4's own coin, tokens and NFTs there as
ERC-20s, ERC-721s and ERC-1155s. An [atomic swap](swaps.md) trades what exists on each chain; a
bridge mints a claim on what it holds locked on the other.

The federation, the contracts' crypto, the gateways between chainv4 and EVM chains, for coins,
tokens and NFTs, and Bitcoin, through a light client contract, are in; so are the wallet's and the
explorer's Bridge pages ([Wallet](wallet.md#bridge), [Explorer](explorer.md#pages)).

## The federation

Who vouches for a deposit made on another chain, and signs what leaves for one, is a federation:
`registry::federation::Federation`.

| Field | What it is |
| --- | --- |
| `signers` | 1 to 20 secp256k1 keys, compressed, 33 bytes each: the same keys hold a Bitcoin multisig and are checked by an EVM chain's `ecrecover`. |
| `threshold` | How many must sign, 1 to as many as there are. |
| `chain_depth` | How deep a lock here must be before they vouch for it, on a chain without finality; one with finality waits for it to be final. |
| `evm_depth` | How deep a deposit on an EVM chain must be, where its RPC names no finalized block. |
| `bitcoin_depth` | How many confirmations a Bitcoin deposit needs. |

**Where it comes from:**
- **Genesis** may name one, `federation` (see [Genesis](genesis.md)): version 0, written into the
  state as the chain starts, so a light client proves it as it proves the one governance decides.
- **Governance** replaces it with `Change::Federation` (see [Governance](governance.md#federation)),
  each decided one a version more than the last.
- **Contracts read** the one in force, with its version: `federation.get()` in Lua,
  `federation()` in the SDK, the `federation` host call (see [Contracts](contracts.md)).
- **Every one is kept,** by its version, with the block that decided it: `federation.at(version)`
  in Lua reads one in force or past, with when it was decided and, once the next was, when it was
  replaced (`Host::federation_at`). The Bitcoin gateway reads an old federation's keys and grace by
  it, rather than by what it saw itself.
- **`GET /status`** shows it, in `federation`, with its `version`.

**Swappable:** the signer set is one value the bridge contracts read, so a later batch can point it
at the chain's validators instead (see the todo), once they sign with keys Bitcoin and the EVM can
check.

## Contracts' crypto

Every engine checks signatures of every scheme the chain has, recovers secp256k1 keys as
`ecrecover` does, hashes as Bitcoin does, and checks chainv4's own proofs against a root: the
[crypto library](contracts.md#crypto) for Lua and WebAssembly, and the
[crypto precompile](evm.md#crypto) for the EVM. The bridges check the federation's signatures
with them, and the Bitcoin relay checks blocks and transactions with them.

## The gateways

A transfer leaves through one chain's gateway and arrives at another's:
- **Here:** `contracts/lua/gateway.lua`, which is also the signature board.
- **On EVM chains:** `contracts/evm/Gateway.sol`, with its wrapped templates `WrappedToken`,
  `WrappedNft` and `WrappedMulti`, each wrapped asset a minimal proxy of one (EIP-1167) so the
  gateway stays under the EVM's size limit.

**What moves** is a kind: 0 the coin, 1 a fungible token, 2 an NFT, 3 an amount of a multi-token
id. An asset is native to one chain, its **origin**:
- **Leaving its origin,** it is held by the gateway it leaves through: the coin with the call, a
  token, NFT or multi-token amount moved in with `transfer_from` (approve the gateway first). Both
  gateways check what arrived, the balance before and after or the NFT's owner, and refuse less: a
  token that takes a fee, or moves nothing, would leave the wrapped one backed by less than it says.
- **Arriving elsewhere,** it is minted as its wrapped token or collection there, made the first
  time: wUSDT, Wrapped Tether, with the same decimals, ids and URIs.
- **Going back,** the wrapped one is burned where it is, and the held one released at its origin.

**A transfer** is its two gateways and chains, a nonce its source counts, the kind, origin and
asset, the id and amount, the recipient, and the asset's name, symbol, decimals and URI. Both
gateways hash it alike, and `chainv4::bridge::Transfer::digest` does too:

```
head   = keccak256(abi.encode(bytes from, bytes to, uint64 source, uint64 target, uint64 nonce, uint8 kind, uint64 origin))
body   = keccak256(abi.encode(bytes asset, uint256 id, uint256 amount, bytes recipient, string name, string symbol, uint8 decimals, string uri))
digest = keccak256(abi.encode("chainv4-gateway", head, body))
```

Chains are named by their EVM chain ids, this one by its own (`GET /status`, `chain_id`), which
the gateway here reads from the chain, `chain.id()`, and no owner sets. Addresses here are written
as their 33 bytes, an EVM chain's as their 20, and the coin as no bytes.

**The gateway here is named by its identity,** `identity()`: its 33 bytes and the time it was set
up. That is what `from` or `to` says in a transfer, and what Gateway.sol holds as its peer. A chain
restarted from the identical `genesis.json` has the same id, the same gateway address and nothing
marked done, while Gateway.sol's logs and the old chain's signature board stay readable: anyone
could post those signatures again. Set up at another time, the gateway computes another digest
for them, and they recover to no one. Gateway.sol marks what it delivered by the source gateway
too, so the new one's nonces can start from 1. The time is what the chain says when `setup` runs,
not anything its owner gives; someone who produces every block of a restart could give it the old
time, but such a chain is the old chain.

**Changing a peer strands nothing.** Both gateways keep what arrived by the source chain, the
gateway it came from and its nonce, so a new peer counting its nonces from 1 again has transfers of
its own. A transfer that left is signed, voided and refunded by the peer it was sent to, as the
gateway recorded it, whatever the peer is now. A transfer arriving is taken from the peer in force
alone, and `received` and `voided` read by it, or by the gateway named as a third argument.

**A recipient is checked as it is sent,** on both gateways: 20 bytes and not zero for an EVM chain;
for chainv4, 33 bytes of a known kind, a shorter kind followed by zeros, and not zero. Names, symbols
and URIs that are not UTF-8 cross as empty, as no signer's tools could hold them.

## The signature board

The federation's signers post their signatures over each transfer to the gateway here, a
transaction each, with `attest`:
- **Arriving here,** a transfer is done once the federation's threshold signed: minted or
  released, once, and `received(source, nonce)` says so.
- **Leaving here,** the signatures wait on the board, in the signers' order: `signatures(digest)`
  gives them and the threshold, and anyone relays them to Gateway.sol's `deliver`, which checks
  them with `ecrecover` against its own copy of the signer set.
- **Who signed** is what the signature recovers; a signature from anyone outside the federation in
  force is refused. Signatures are r, s and v, 65 bytes, low s only.

## Deploying

```sh
chainv4 bridge deploy                                     # the gateway here, with its templates
chainv4 bridge deploy-evm --gateway contract:0x… --eth-rpc URL --evm-key k.key    # Gateway.sol under the federation in force
chainv4 bridge peer 0x<Gateway.sol> --gateway contract:0x… --eth-rpc URL --evm-key k.key
chainv4 send --key member.key vote-change --vote yes --directory '{"gateway":"contract:0x…","evm_gateways":[{"chain_id":11155111,"gateway":"0x…"}],…}'
```

`deploy-evm` reads the federation from `GET /status`, so governance names one first. `peer` makes
each gateway the other's peer: signed here by the gateway's owner, there by Gateway.sol's. Then
governance names both in the chain's [directory](governance.md#directory): the tools, the apps
and the signers take the gateways from it, and a name never finds one, as anyone could register
it first. Until then, `--gateway` names it.

## Sending

```sh
chainv4 bridge send coin 0x<there> 1000 --eth-rpc URL                         # here to there
chainv4 bridge send token 0x<there> 500 --asset contract:0x… --eth-rpc URL
chainv4 bridge send nft 0x<there> 1 --asset contract:0x… --id 7 --eth-rpc URL
chainv4 bridge send token ed25519:0x… 400 --asset 0x<wrapped> --from-evm --eth-rpc URL --evm-key k.key
```

Each lets the gateway move what it sends first, then sends it. It arrives once the federation
signed it and, going to an EVM chain, someone relayed it.

## Signing and relaying

A signer runs, with its federation key as `--evm-key`:

```sh
chainv4 bridge watch --eth-rpc URL --evm-key signer.key --key payer.key
```

**A signer goes by the directory.** It signs only for an EVM chain the chain's
[directory](governance.md#directory) lists, with that gateway as the peer. What a peer says left it
is what the federation vouches for, and the gateway's owner sets its peers alone, so one key could
otherwise have anything minted here. On a chain with no directory, or for a chain it leaves out,
every chain is refused, and each pass says why, until governance lists it or the signer's own
operator vouches for that very gateway:

```sh
chainv4 bridge watch --evm-gateway 11155111:0x… --eth-rpc URL --evm-key signer.key --key payer.key
```

The flag is the operator's own word, never the chain's, and it must match the peer. `send` refuses a
chain no signer would sign for, as it would never arrive.

- **sign** reads what left here (`Sent` events, so the node keeps `--index-events`) and what left
  each EVM chain given (Gateway.sol's `Sent` logs), and once each is final, finalized here or
  `chain_depth` deep, at the RPC's finalized block or `evm_depth` deep there, posts its signature
  to the board. One it posted already, it skips, and it passes over quietly what is not for this
  chain's gateway from its peer, as Gateway.sol's old logs are after a reset.
- **relay** delivers what left here to each EVM chain once the board holds enough signatures,
  paying the gas with `--evm-key`. Anyone may relay. Run by hand, it delivers at once.
- **Under watch, signers take turns delivering,** as only the first delivery of a transfer goes
  through and every other one is mined failing, at its gas. The first turn goes round the
  federation nonce by nonce, so the signers share the gas, and a relayer that is no signer goes after
  them all. Each after the first waits `--relay-turn` seconds more, 90 unless told, from when it found
  the transfer ready, long enough for the one before it to be mined there; finding it delivered by
  then, it sends nothing. One signer down only delays a delivery by its turn.
- **watch** does both every `--every` seconds, relaying whatever became of signing.

**Each transfer stands alone.** A transfer the gateway or Gateway.sol refuses, as one a recipient
will not take, or with more than a gateway here holds, is said with its nonce, chains and why, and
the pass goes on to the next. Under `watch` it sits out 1, 2, 4 and so on up to 64 passes between
tries, since a dry run or a failed delivery costs something every time, and each pass still lists
it, `still refused: …`, until it goes through. One EVM chain out of reach stops no other.

## Refunding a transfer refused for good

A transfer its destination will never take, such as coins for a contract that takes none, would leave
what it moved locked or burned where it left, so the federation can take it back to who sent it. The
order is what keeps it from paying twice:

1. **Void at the destination.** The federation signs to void it, `Transfer::void_digest`. Its nonce is
   then taken for good at that gateway: Gateway.sol's `voidTransfer`, relayed from the board, or here
   once enough posted `attest_void`. Signatures over its delivery are public, and now fail at the
   contract, whoever relays them. One delivered cannot be voided, and whichever lands first wins.
2. **Refund at the source.** Only once that is final does the federation sign to refund it,
   `Transfer::refund_digest`. Its source gateway then pays who sent it, once: here once enough posted
   `attest_refund`, or Gateway.sol's `refund`, relayed from the board. What it held is released; what
   it burned, wrapped, is minted again, the same id and URI for an NFT. That goes for the coin, tokens,
   NFTs and multi-token amounts alike, each gateway having recorded all of it as it left.

```sh
chainv4 bridge refund 4 --why "the recipient contract takes no coin" --eth-rpc URL --evm-key signer.key --key payer.key   # left here for there
chainv4 bridge refund 9 --from-evm --eth-rpc URL --evm-key signer.key --key payer.key                                     # left there for here
chainv4 bridge refund-to 4 ed25519:0x…                                                                                    # its sender names where it goes
```

**Each signer's operator runs it,** when they decide to: nothing does this by itself. It reads both
chains, shows what the transfer moved, whom it pays and why, asks for a typed yes unless `--yes`, and
does the next step, so each operator runs it once for the void, and again for the refund. The one that
makes the threshold takes the signatures over to the EVM chain and pays its gas, as anyone may with a
key that is no signer's; a signature already on the board is not posted again.

**What a signer goes by is what it reads,** never what an operator says, as it does for a transfer:
- to void, that the transfer left its source in a block that is final there, the source's digest being
  the one worked out from the gateway the [directory](governance.md#directory) names, and that its
  destination says it was not delivered. A delivery its destination would take now is refused: deliver
  that instead. With too few signatures on the board to try one, it says so and goes on.
- to refund, that the destination says it was voided in a block final there: `voidedAt(key)` against
  the RPC's finalized block or `evm_depth`, or `voided(source, nonce)` against the finalized checkpoint
  or `chain_depth`. Before that, a reorganization could undo the void and let a delivery land.

**What it signs, and to whom.** Each message has its own domain and layout, in Rust, Lua and Solidity:

```
void   = keccak256(abi.encode("chainv4-gateway-void", bytes from, bytes to, uint64 source, uint64 target, uint64 nonce, bytes32 digest))
refund = keccak256(abi.encode("chainv4-gateway-refund", bytes from, bytes to, uint64 source, uint64 target, uint64 nonce, bytes32 digest, bytes sender))
```

`digest` is the transfer's own, and `sender` whom its source gateway recorded: `sent`'s 13th value here,
`leftBy(nonce)` there. A signature over a transfer or a signer set passes for neither. The amount, asset
and sender a refund pays are those the source gateway holds, so none comes from a caller: Gateway.sol's
`refund` takes only a nonce and signatures, and the gateway here refuses a digest or sender that is not
what it recorded.

**A sender that cannot take it.** The coin to a contract with no way to receive it, or an ERC-1155 to
one without its hook, fails, and takes the whole call with it: nothing is marked, it can be tried again,
and it cannot pay twice, as it is marked before it pays. So a sender may name another address, by its own
call alone, `set_refund_to` here and `setRefundTo` there, `chainv4 bridge refund-to`: what the federation
signs is still who sent it. A contract that can neither receive it nor make that call stays as it was
before this existed, its funds where they are.

**What this adds to what the federation can do.** A threshold could already mint anything here or there
by signing a transfer no one made, so a refund it signs without a void, or alongside a delivery, is no
new way to steal: and a refund pays only who sent it what they sent. What is new is censorship: a
threshold can void what has signatures and was not yet relayed, which before only a relayer's absence
could hold up. Nothing here checks the void at the source but those signatures.

**Rotations and resets.** A signer in force is all that counts on the board, and each EVM gateway counts
its own copy of the set, so a refund begun under one federation takes the threshold of the next, and one
on Gateway.sol waits for `setSigners` as a delivery would. Voids and refunds name both gateways. What left
for a gateway since replaced is refused by each signer rather than voided: put the peer back to do it, or
it stays where the caveats say. The gateway here marks what arrived and was voided by source chain and
nonce, as it always did, and Gateway.sol by the source gateway too.

**Not Bitcoin.** A deposit is a proof of a payment with no sender to pay back, and nothing a signer can
refuse. A withdrawal has its own path: it stays queued while it cannot go, each reason passing with fees
or more deposits, and its owner can raise what it may lose, so nothing locks it for good as a refused
transfer is. A release that cannot be finished is cancelled, as below.

## Bitcoin

BTC comes here as wBTC, a token of 8 decimals, one for one with satoshis: deposits are proven to a
Bitcoin light client contract rather than vouched for, and the federation signs what is paid out.

### The relay

`contracts/examples/btc_relay`, in Rust for WebAssembly, is Bitcoin's headers as a contract:
- **Started once,** by its deployer, at a checkpoint: a header, its height and a network,
  `start(header, height, network)`. On `main` the height is a difficulty period's first, divisible
  by 2016.
- **Anyone submits headers,** up to 2,000 a call, `submit(headers)`, each checked as Bitcoin's
  nodes check them: its proof of work, the difficulty every 2016 blocks (clamped to four times
  either way, never easier than the limit), a time after the median of the last 11 and at most two
  hours past the chain's block time, and a parent it holds.
- **The most work wins:** a branch with more total work becomes the main chain, as a reorg.
- **Contracts ask it** `tip()`, `block_at(height)`, `confirmations(block)` (0 off the main chain)
  and `includes(block, txid, index, path, coinbase, coinbase_path)`, a transaction's Merkle path to
  the block's root.

**It keeps each header whole:** its version and nonce beside the rest, so a node can write a header
again and serve it to a peer that checks anchors on its own ([Anchoring](anchoring.md#picking-a-history)).
A relay deployed before that keeps neither, and serves no headers.

**A proof comes with the block's coinbase.** A header does not say how tall its Merkle tree is, so
the 64 bytes of two ids, an inner node, could pass for a transaction a level up with a path one
short (CVE-2017-12842), and a real 64-byte transaction could be taken for an inner node with one too
long. So `includes` takes the coinbase, without its witness, and its path too: it must read as a
coinbase, one input spending nothing and not 64 bytes, lead to the root at place 0, and its path be
just as long as the transaction's. The gateway also refuses any transaction of 64 bytes outright, as
Bitcoin's nodes do not relay one.

| Network | Checks |
| --- | --- |
| `main` | Every rule above. |
| `regtest` | Every rule but the retarget: one difficulty throughout, as Bitcoin's regtest. |
| `test` | Only that each header meets the difficulty it names: testnet4's and signet's rules are looser than this checks, so it is for testing alone. |

### The gateway

`contracts/lua/btc_gateway.lua` is set up once by its owner, on a relay, with wBTC's template
(`wrapped.lua`, cloned as Wrapped Bitcoin), the most fee a release may pay, the least a virtual
byte, how long an old federation's deposit addresses stay open, and the least a deposit mints,
10,000 satoshis unless told: `setup(relay, template, fee_cap, fee_floor, grace_ms, least_deposit)`.
No federation policy changes the least deposit; `least_deposit()` reads it.

**The federation's Bitcoin policy.** Once the federation names one, its `btc`
(`fee_cap`, `fee_floor`, `grace_ms`, `cancel_after_ms`, `fee_part`, the share of a withdrawal
its fee may take unless it says, 10 for a tenth, `bump_after_ms` and `bump_below_pct`, when a
stuck release is [bumped](#bumping-a-stuck-release), and `repay_pct` and `repay_most`, how later
releases [repay](#repaying-what-a-child-cost) what a child cost the backing), the gateway goes by that in place of what it was
set up with, from the block after a change of federation decides it: `fee_limits`, the least
withdrawal, a release's cap and floor, and a retired federation's grace all read it as they run.
A spike in Bitcoin's fees is then a governance vote away, with no new gateway and no sweep. The
signers' `--cancel-after` and `--bump-after` give way to `cancel_after_ms` and `bump_after_ms` too, so
every signer waits alike rather
than the threshold-th most patient one deciding.

**Depositing.** Every address here has its own deposit address on Bitcoin, under the federation in
force, for this chain and this gateway, a P2WSH any wallet pays:

```
<its 33 bytes> OP_DROP <chain id, 8 bytes little-endian><the gateway's 33 bytes> OP_DROP
<threshold> <signer keys…> <count> OP_CHECKMULTISIG
```

Once a payment to it is `bitcoin_depth` deep in the relay, anyone shows it, `deposit(block, tx,
index, path, coinbase, coinbase_path, vout, recipient)`, and its value is minted as wBTC to that address, once. The gateway
holds the output, by its transaction and place, for a later release.
- **The least deposit:** a payment under it mints nothing, and stays on Bitcoin under the
  federation's keys, so say it where a deposit address is given out. Dust minted as wBTC was
  dragged into releases that cost more to sign than it was worth: 300 deposits of 330 satoshis
  once made a 100,000-satoshi withdrawal 37,561 virtual bytes, past the cap.
- **A coinbase,** a deposit at place 0 in its block, waits 100 confirmations rather than
  `bitcoin_depth`, then mints like any other: Bitcoin lets nothing spend a coinbase before, so a
  release spending one minted sooner would be stuck, and could not be cancelled once signed.

A proof of a payment outlives a chain, and a gateway on a new one holds nothing yet, under the same
address if the same keys deployed it. So the chain's id is in the script: an old payment pays
another address, and mints nothing. A chain restarted from the identical genesis has the same id;
there the gateway refuses what was mined at or under the relay's tip when it was set up, so **sync
the relay before you deploy the gateway**, or that tip protects nothing. The gateway's `setup`
refuses a relay that has not started.

**Withdrawing** burns wBTC for a Bitcoin output script and an amount, 10,000 satoshis or more, and
as much as a release of its own costs at the floor, `least_withdrawal()`, and queues it, with the
most it agrees to lose to fees: a tenth of it unless it says, and never so much that under 1,000
satoshis would arrive. It may change that while it waits, `set_most_fee(id, most)`, by who asked for
it alone. Raised, a limit holds at once. Lowered, it holds 30 blocks later, and a release proposed
before then is judged by the old one: a pass plans up to 50 withdrawals against the limits it read,
and one owner lowering theirs as the proposal landed once undid it for all of them. Until then
`withdrawal(id)` gives the lower limit and the height it holds from after the rest, and the planner
goes by it already, as what it proposes may land after. A lowering waiting in a release holds once
that is cancelled and the withdrawal queued again. The script is one Bitcoin's nodes relay: P2PKH, P2SH, or a witness program (P2WPKH,
P2WSH, P2TR or a later version), as a release with any other output would never be mined.

**Releasing** pays queued withdrawals, and the federation does it, with no one withdrawing having
anything more to do: its signers' [passes](#the-commands) go through these steps by themselves.
1. **Proposed:** a signer of the federation in force proposes a release, an unsigned Bitcoin
   transaction, with the withdrawals it pays, at most 50, each once, signing `btc_propose_digest`
   (double SHA-256 of a domain, the chain's id, the gateway, the number the release will have, its
   transaction's id, and the withdrawals it pays); anyone may post that, `propose(raw, signature,
   ids…)`. The gateway checks its inputs are outputs it holds
   unspent, each once, it pays each withdrawal what it burned less its share of the fee, its one
   other output is change to the federation in force, at least 330 satoshis, Bitcoin's dust for a
   P2WSH output, and its fee is between the floor and the cap. It is version 2, with no lock time
   and each input's sequence `0xfffffffd`, which signals BIP125, so a version paying more can
   [replace it](#bumping-a-stuck-release), and whose top bit leaves it no BIP68 delay; and at most
   100,000 virtual bytes, what Bitcoin's nodes relay. Its inputs are then pending.
2. **Signed:** each signer signs each input as BIP143 has it, SIGHASH_ALL, and posts the 64-byte
   signature, `sign(proposal, input, signature)`; the gateway checks it against that input's
   script and amount.
3. **Broadcast:** once each input holds the threshold, anyone assembles the witnesses,
   `input_signatures(proposal, input)` giving them in the keys' order, and broadcasts it.
4. **Shown:** once it is `bitcoin_depth` deep, anyone shows it, `released(proposal, block, tx,
   index, path, coinbase, coinbase_path)`: its inputs are spent, its change held, and its withdrawals paid.

**Withdrawers pay the fee.** A release's fee comes off the withdrawals it pays, split evenly, the
first taking what will not divide: each output costs about the same to carry, whatever it pays, and
that is exact enough for the contract to check. No share may pass the most that withdrawal agreed to
lose, so one that said little waits, or the fee is lower. Its change is all it spends past what those
withdrawals burned, so what the gateway holds falls by just what was burned, and wBTC stays backed
one for one whatever the fee. **Every release keeps change,** as a child paying for it spends that:
should what it spends past what was burned be under 330 satoshis, as when the gateway holds just
what they burned, the change is 330 all the same, and the withdrawals lose what tops it up with the
fee, split as the fee is. The planner takes another held output first, so this costs nothing unless
the gateway holds no more. A sweep pays no one, so its fee comes off what backs wBTC: once shown, it
is added to `shortfall()`, as a child's fee is, for later releases to
[repay](#repaying-what-a-child-cost). `withdrawal(id)`
says the limit, and its share once it is in a release, with any child's that paid for it.

**Only a signer proposes.** A release's fee is burned to Bitcoin's miners, off the withdrawals it pays,
so anyone able to propose could burn all each had agreed to lose, or tie them up in a release no one
signs. A signature says nothing but that one release: not on another chain or gateway, and it passes
for no vote to cancel, nor for a Bitcoin signature, whose preimages start otherwise. It is good for
one number, the next, so what proposed a release does not bring it back once the federation cancelled
it: proposing it again, which is allowed, takes a signer signing again. One signer alone may propose
at a fee the others refuse: their passes then vote it down, as below.

**The fee** of a release is held between two limits set once, at setup: `fee_cap`, the most it may
pay in satoshis, and `fee_floor`, the least per virtual byte, so a release cannot hold its inputs
for ever at next to nothing. Its size is reckoned as if signed, each signature at its largest:
`btc_release_vbytes` does the same for the tools. A cap under 710 virtual bytes at the floor, the
largest release one input can be, is refused at setup, as it could leave no release allowed; a
larger release past the cap is split. Read them with `fee_limits()`.

**Cancelling** frees what a release holds when it cannot be finished: one stuck on an old
federation no one can reach, or one signers refuse, such as one paying far past what the network asks.
- **Who:** the threshold of the federation in force, each signing `btc_cancel_digest` (double
  SHA-256 of a domain, the chain's id, the gateway, the release and its transaction's id), posted
  by anyone with `cancel(proposal, signature)`.
- **When:** only while an input is under its threshold of signatures. One with every input at its
  threshold can be finished by anyone, so a vote is refused. The exception is one Bitcoin can no
  longer mine, as another release spending one of its inputs was shown.
- **What:** its inputs go back to unspent and its withdrawals to queued, and signatures and votes
  for it count for nothing: `sign` is refused and `input_signatures` gives none.
- **Why it pays no one twice:** Bitcoin does not know of a cancel, and a signature in a `sign`
  that lands too late is public all the same, so a release under its threshold here may still be
  finished there. Each withdrawal it paid is therefore **anchored** to one of its inputs, `anchor(id)`,
  the newest federation's. Every release paying that withdrawal must spend that input as well, so
  they all conflict, and Bitcoin mines one. The anchor lapses once it is shown spent, as nothing
  holding it can be mined after that. A cancelled release that was finished anyway may still be
  shown with `released`: what it spent and paid is then spent and paid.

**The signers' passes vote by themselves,** rather than a release waiting on people, as one signer can
propose what the rest refuse, and nothing else ever cancels it:
- **What a signer refuses for good gets its vote at once:** what is wrong with it, or what pays past its
  limit. Any failure to read what the gateway says of a release gets none, as nothing is known, and nor
  does a release past only the fixed limit a signer goes by with no fee rate to read, as that may be the
  going rate. `check_btc_release` is told apart this way: it runs only on all of it read, and says
  `Wrong`, `Dead` or `Overpriced`; `btc_release_step` then says what to do.
- **A signer that signed it** gets no vote on a refusal, as a signature cannot be taken back, and all it
  usually means is that the rate moved. The gateway would count one, though, and past the age limit it
  votes all the same, or a release whose other signers went away could never be cancelled; each
  withdrawal's anchor is what keeps that signature, public, from paying twice.
- **The age limit** is `--cancel-after`, 6 hours unless told: a release short of its threshold past that
  gets a vote from each signer, rather than a signature, whether it stands or not. Its age is what the
  chain says, the tip's block time less that of the block with its `Proposed` event, as the gateway keeps
  no time of its own; so it takes a node with `--index-events`, and with no index a pass says so and votes
  on none for its age. No signer's clock has a say.
- **With every input at its threshold** no vote is sent, as the gateway would refuse it, but for one with
  an input shown spent, which Bitcoin can no longer mine.

**That does not go round.** Once it is cancelled, its withdrawals are queued, with their anchors, and a
pass would otherwise propose them again, for the same refusal, every minute, for as long as the network
asks what some signer refuses:
- A signer does not propose past what it would itself sign for, so it cannot vote down its own.
- A withdrawal from a cancelled release is not proposed again until that release is past `--cancel-after`,
  unless at a rate at least a quarter under what it paid, which is another question to put to the signers.
  One cancelled for its age has passed that limit already, so it holds nothing up. An age that goes unread
  errs on the side of waiting. `bridge btc propose --fee` is an operator's word, and goes by none of this.

**A release with all its signatures that is not mined** cannot be cancelled, as anyone could still
broadcast it. It is [bumped](#bumping-a-stuck-release) instead. Past `--cancel-after`, again by its
proposal's age, each pass also logs one line of it, its id, the version Bitcoin holds, what it pays
against what the network asks, and how it is bumped.

**A new federation** takes over the deposit addresses: new ones are made under it, and the old
ones stay open for the grace period, counted from the block that decided the next, as the chain
keeps it, whether or not the gateway ever ran under the old one: `deposit` takes the version an
address was made under, and reads it with `federation.at`. Before this, the gateway knew only a
federation it was called under while that one was in force, so one replaced before anything called
it left what was paid to its addresses unmintable. A **sweep** is a release with no withdrawals,
paying what older federations hold to the one in force, proposed by a signer as any release is.

**Reading it:** `wbtc()`, `deposit_script(address)` (the witness script, the output script and
the federation's version), `utxo(n)`, `utxo_of(outpoint)` and `utxo_count()`, `withdrawal(id)`,
`anchor(id)` and `withdrawal_count()`, `proposal(id)`, `tree(id)`, `voted(id, key)` and
`proposal_count()`, `change_script()` or `change_script(version)`, `fee_limits()`, `least_deposit()`,
`shortfall()`.

### Bumping a stuck release

A release with all its signatures that Bitcoin leaves unmined, as fees rose past what it pays, is
bumped, one of two ways, each a proposal of its own that the signers sign as they sign a release:
- **Replace-by-fee, by default:** `replace(of, raw, signature)` proposes a replacement of a version
  of the release, signed by a signer of the federation in force over `btc_bump_digest` under the
  domain `chainv4-btc-replace`. It spends just what that version spends, in the same order, so
  Bitcoin mines one of them at most, and its inputs signal BIP125; it pays the same withdrawals to
  the same scripts, each less its even share of what they now lose, within what each agreed to
  lose; its change is what it was, as the withdrawals pay the bump as they paid the fee; and its
  fee is past every open version of it by a satoshi a virtual byte, Bitcoin's incremental relay
  fee, and under the cap. A sweep's bump comes off its change, as its fee does, and whichever
  version is shown, its fee is owed to the backing.
- **A child, as the fallback:** `child(parent, raw, signature)`, under the domain
  `chainv4-btc-child`, proposes a child spending the change of a version, which every release
  keeps for this, then outputs the gateway holds unspent only should that change not pay its fee
  and leave dust; it pays the rest to the federation in force in one output, held like change once
  shown. Its fee is charged to that version's withdrawals, evenly, each within what it agreed to
  lose with its share of the release's own, the two fees together under the cap. A child is bumped
  by replacing it, by `replace`, as a second child of the same change would conflict with it.

Before a child is shown, the gateway knows the change it spends, unlisted, as `unshown`, so the
signers can sign it; once the release is shown, its change is held, pending under the child or spent
should the child have been shown first.

**Who pays.** The withdrawals, split evenly, each within its most fee, as for the fee itself: a
replacement's outputs to them are lower. A child's fee is charged to them too, within their limits,
and recorded, `withdrawal(id)` saying its share; but they have their outputs already, so the
federation pays it from its change, and the gateway then holds that much less than the wBTC it
backs, until later releases [repay it](#repaying-what-a-child-cost). With a sweep's fee, this is what
the backing bears, for a while, so a replacement comes first.

**A withdrawal whose limit is too low for its share.** A replacement pays exactly the same withdrawals
and splits evenly, so it goes as high as the tightest limit lets it, and the pass names whose limit held
it back. Its owner may raise its most fee with `set-most-fee` while the release is open, only higher, as
what it was proposed under stands. Dropping it, or making the others pay more than an even share, would
both be a release of another kind, which a replacement is not; with no room for even the increment, no
bump goes, and it waits, as a withdrawal past its limit waits today.

**When.**
- **By policy:** a signer's pass bumps a release once nothing of it waits for signatures, its newest
  version or child was proposed at least `bump_after_ms` ago by the chain's block time, an hour unless
  the federation says, and what the version Bitcoin holds pays, with any child of it, is under
  `bump_below_pct` percent of what Esplora says the network asks to mine it within 3 blocks, 80 unless
  it says (`btc_is_stuck`). It bumps to that rate. An hour is twice the three blocks its rate was asked
  for, so one mined in the usual time is never bumped, and well inside the six hours a release short of
  its signatures waits before it is voted down. 80 percent, as estimates wander by a tenth or so from
  one minute to the next, and a bump costs its withdrawals its whole fee again at the new rate.
- **Replace-by-fee first, then a child** (`btc_bump_kind`): a child once Esplora refused the
  replacement while an older version stands, as when a recipient pinned the release by spending their
  output in a large child at a low rate, which Bitcoin will not let a replacement evict for less than
  all it pays, and that at once; or once a replacement goes unmined past the delay again. A replacement
  refused is not sent again once a child pays for the version Bitcoin holds.
- **By hand,** sooner: `chainv4 bridge btc bump <release> [--rate N] [--child]`. The others check it as
  any bump.

**One at a time.** A bump is of what holds all its signatures, and while any bump of a release waits for
its signatures, another is refused: two signers proposing at once could only compete. One the signers
refuse is voted down by its own number, as below, and the next may go.

**Exactly one of them can be mined.** Every version of a release spends the same inputs, and a child
spends a version's change, so Bitcoin mines one version at most, and a child only with its own. `released`
takes whichever was mined: its inputs are spent, its withdrawals paid at its shares, its change held, and
every other version and every child of those closed as `replaced`, freeing what a child spent besides.
A child shown releases its own output and charges its fee. No output is held twice: another version's
change has another outpoint and can never be shown, as its inputs are spent, and a child's change is its
own output, held once. `proposal(id)` says each one's kind, `release`, `replacement` or `child`, what it
replaced or pays for, its fee and what its withdrawals lose by it; `tree(id)` the release and every
bump of it.

**Cancelling with bumps open.** A bump short of its signatures is cancelled alone by its own number:
the release, its other versions and what they hold stand, and a child's own inputs are freed once
none of its versions is open. The release's number cancels its whole tree, and only while no version of
it holds every signature, or Bitcoin can never mine it as an input was shown spent: then every version
and child goes with it, its inputs unspent, its withdrawals queued with their anchors. A bump needs no
anchor of its own: every version spends the same inputs, and a child pays no withdrawal, so neither can
pay anyone twice. As before, what is cancelled may still be shown if it was mined anyway.

**The threat model.** The federation's threshold could spend what the gateway holds anywhere, as ever;
what bumps add is what one signer, or one Esplora, can do.
- **Who proposes a bump:** any signer of the federation in force, as for a release; no one else's
  signature passes, nor one over a release's, a vote's or the other kind of bump's digest.
- **What each signer checks before it signs one** (`check_btc_replacement`, `check_btc_child`), from the
  gateway's state, not from who proposed it:
  - *the same outputs:* a replacement spends just what it replaces, in the same order, pays the same
    scripts and withdrawals, and keeps the change; a child spends that version's change first, other
    held outputs only when needed, and pays the federation in force alone;
  - *the cap and each limit:* the fee under the cap, a child's with its release's; each withdrawal's
    share within what it agreed to lose, a child's with the release's, and what it repays with it;
  - *what it repays:* within each withdrawal's even share of the budget and `repay_most`, the budget within
    `repay_pct` of the fee, the whole no more than `shortfall()` owes now, and all of it in the change;
    a replacement repaying just what it replaces. A wrong one gets no signature and a vote, as any
    release that is wrong. The signers bound it rather than work it out again, as what was owed past
    other open releases when it was proposed is the gateway's to say;
  - *the rate:* at the floor, past every open version by the increment, and no more a virtual byte than
    the signer signs for, a child's with what it pays for (`--max-fee-rate`, or 3 times Esplora's fastest);
  - *that it is stuck:* no version of the release is mined, and what it bumps pays under what its Esplora
    says the network asks now. A bump it finds needless gets no signature, and a vote only past the age
    limit, as another signer's Esplora may say otherwise.
- **A lone signer** can propose a bump the rest refuse; it holds up the next bump until it is voted down,
  and costs nothing, as only the threshold's signatures make anything mineable. Its share of the fee
  is bounded by the cap and each limit, whoever proposes.
- **An Esplora that lies** can get a needless bump signed, which costs the withdrawals up to their limits
  and the cap, or refuse a good one, which costs a delay; run your own for a check that does not rest on
  someone else's.
- **A recipient** can pin a release with a child of their own; the federation's child, spending its own
  change, pays for both.

**The signers take turns.** On the devnet, on 2026-11-03, all three signers sent `released` for release 1
in one block; two failed, each paying a fee. Now what any one signer's call does for all, `released`, a
vote to cancel, a proposal or a bump, waits for the signer's turn: its place among the federation's
signers times one block of the chain's, measured from its newest blocks, and at least a second
(`btc_turn_delay`). Each then checks again, by what the gateway says and what waits in the node's
mempool, and sends only what is still undone, so the first usually alone pays, and the others step in
only if it is down. A pass with no signer's key goes after them all. Signing takes no turn, as every
signer signs.

### Repaying what a child cost

Later releases repay what children and sweeps cost the backing, a little each, until it is back to
one for one, with no one stepping in. A sweep shown adds its fee, or its shown replacement's, to what
is owed, as nothing else pays it. `shortfall()` says what is left to repay, and how much of that open releases
repay once shown; the explorer shows the first as what is left to repay.
- **How much:** a release's budget is the least of what is owed past what open releases repay and
  `repay_pct` percent of its own fee, a fifth unless the federation says. Each withdrawal repays an even
  share of it, at most `repay_most`, 500 satoshis unless it says, and only what its most fee leaves past
  its share of the fee (`btc_repays`): one at its limit repays nothing, and is not held up for it. So it
  is never more than is owed, and falls on the next withdrawers thinly. A sweep repays nothing.
- **Where it goes:** to the federation's change, with the rest of what backs wBTC. The gateway works it
  out from its state when the release is proposed, and refuses one whose change and outputs do not carry
  just that; `proposal(id)` gives what each withdrawal repays and the budget, and `withdrawal(id)` counts
  it in what it lost.
- **Counted once:** a release proposed is counted as repaying, so another proposed meanwhile repays only
  what is left past it. Shown, the shortfall falls by what it repaid; cancelled, it is no longer counted,
  so the next release repays it instead. Every version of a release repays what the release does, from
  the same change, the bump coming off the withdrawals alone, so it is counted once whichever is mined.
  One cancelled and mined anyway is counted as it is shown, and nothing falls below zero.
- **Why the defaults:** a child's fee is about what pulls one release up to the network's rate, so at a
  fifth of each later release's fee, some five releases repay it; 500 satoshis is half of what the least
  withdrawal agrees to lose by default, so no one gives up most of their limit to it.

### The commands

`chainv4 bridge btc` drives both from an Esplora API (`--esplora`, such as
`https://mempool.space/api`), the gateway the one the [directory](governance.md#directory) names
as `btc_gateway`, or `--btc-gateway` before it does:

```sh
chainv4 send deploy --wasm btc_relay.wasm                      # the relay
chainv4 bridge btc relay-start 0x<relay> 840000 --esplora URL   # its checkpoint, on the network Esplora serves
chainv4 bridge btc relay-sync 0x<relay> --esplora URL           # the headers since, 25 a tx; run it often
chainv4 bridge btc deploy 0x<relay> --fee-cap 50000 --fee-floor 2 --grace-hours 168 --least-deposit 10000
chainv4 bridge btc address ed25519:0x… --btc-gateway 0x…       # where to pay: bc1q…, tb1q… or bcrt1q…, as the relay follows
chainv4 bridge btc deposit <txid> 0 ed25519:0x… --btc-gateway 0x… --esplora URL
chainv4 bridge btc withdraw bc1q… 20000 --btc-gateway 0x…
chainv4 bridge btc watch --esplora URL --evm-key signer.key --key payer.key  # each signer, left running
chainv4 bridge btc pass --esplora URL --evm-key signer.key --key payer.key   # one pass, as from cron
chainv4 bridge btc cancel 1 --btc-gateway 0x… --evm-key signer.key  # each signer's vote
chainv4 bridge btc bump 1 --rate 40 --evm-key signer.key --esplora URL  # sooner than a pass would
chainv4 bridge btc sweep --fee 1000 --evm-key signer.key --btc-gateway 0x… --esplora URL
chainv4 bridge btc anchor --esplora URL --btc-key anchorer.key --key payer.key  # the chain's checkpoints into Bitcoin: anchoring.md
```

By hand, each step of a pass, and with a fee told:

```sh
chainv4 bridge btc propose --fee 1000 --evm-key signer.key --btc-gateway 0x… --esplora URL
chainv4 bridge btc sign --evm-key signer.key --btc-gateway 0x… --esplora URL  # each signer
chainv4 bridge btc finish 1 --btc-gateway 0x… --esplora URL     # assembled and broadcast
chainv4 bridge btc released 1 --btc-gateway 0x… --esplora URL
```

**A signer's pass** is `btc pass`, or every `--every` seconds, 60 unless told, `btc watch`, or
`bridge watch` given `--esplora`. Each step stands alone, as with the other bridges: one going wrong
is said, and the next goes on.
1. **Proposes** what is queued, oldest first, at what Esplora says the network asks to mine it within
   3 blocks (`/fee-estimates`), rounded up, or the floor if that is more or Esplora has no estimates.
   It is worked out from the gateway's state and that rate alone, so signers at it together build the
   same release, and the first to land wins: the others see that before they send, by a dry run, or
   at worst one transaction later, and say so; none fails or pays again for it. What cannot go now is
   said with why, and stays queued, without holding up the rest:
   - one whose limit is under its share of the fee, until fees fall or its owner raises it;
   - one whose anchor is in another release;
   - one past what the gateway holds unspent for now;
   - one too large for the cap at this rate alone; a release of many past the cap pays fewer, for the
     next one to pay the rest.

   With Esplora out of reach, nothing is proposed, rather than guess a fee a release cannot then change.
2. **Signs** every open release that stands, as **sign** below, and [votes](#the-gateway) to cancel what
   it refuses for good or what is short of its signatures past `--cancel-after`.
3. **Broadcasts** the newest version of each release with all its signatures that Esplora has not heard
   of, and the newest of a child paying for the version it holds; **shows** any `bitcoin_depth` deep, a
   cancelled one that was mined anyway among them; and warns of any with all its signatures that Bitcoin
   has not mined past `--cancel-after`. This takes no signer's key: anyone may run a pass with no
   `--evm-key` to do just this, and `relay-sync` is still what keeps the relay able to prove it.
4. **Bumps,** with a signer's key, what is stuck, as [above](#bumping-a-stuck-release): what was broadcast
   this pass waits for the next.

Proposing, voting, showing and bumping each wait the signer's [turn](#bumping-a-stuck-release).

How long a withdrawal takes is then one pass of the federation, its threshold's signatures, and
Bitcoin's own time: usually under an hour to be mined, and a little more to show as `paid`. It waits
longer while fees run past what it agreed to lose, or the gateway holds too little.

- **relay-start** starts the relay on the network Esplora serves, known by its genesis block
  (`/block-height/0`): `main`, `test` for testnet4, testnet3 and signet, or `regtest`. `--network`
  names it where Esplora's genesis is none of those, and is refused where it is another.
- **deploy** takes `--fee-floor` in satoshis a virtual byte, 2 unless told, and `--least-deposit` in
  satoshis, 10,000 unless told, and says what it set up on stderr. Sync the relay first.
- **address** writes the deposit address for the Bitcoin the chain follows, and says on stderr which
  and what said so, the address alone going to stdout:
  1. the gateway's relay, by its `network_name()`, as it was started: `bc1q…` on `main`, `tb1q…` on
     `test`, `bcrt1q…` on `regtest`;
  2. else, for a relay not started, Esplora's genesis block, when `--esplora` is given;
  3. else `--network`, `main`, `test` or `regtest`;
  4. else mainnet, saying that nothing said which.

  It says the least deposit on stderr too, and that a coinbase mints once 100 deep.

  A `--network` the relay or Esplora contradicts is refused rather than written. The script is the
  same on every network, so a deposit to the address of another, such as a mainnet wallet paying the
  `bc1q…` one beside a testnet relay, is never seen here and mints nothing. **Before this,** it wrote
  mainnet's `bc1q…` unless given `--network test`, whatever the relay followed, as the test chain
  found. The wallet's and the explorer's Bridge pages read the relay's `network_name()` too, through
  the same `chainv4::bridge::BtcNetwork`: `bc1q…` for `main`, `tb1q…` for `test`, `bcrt1q…` for
  `regtest`. **Before this,** they wrote `tb1q…` on regtest, where Bitcoin Core wants `bcrt1q…`.
- **withdraw** takes a native segwit address on the Bitcoin the relay follows, P2WPKH or P2WSH, or
  another output script Bitcoin's nodes relay as `0x…` (a Taproot output as its `5120…` script). An
  address on another network is refused before anything is burned, naming both networks: a `bc1q…`
  beside a testnet relay would be paid to that program on testnet, where no one meant it. Which
  network goes as for addresses above, `--network` included. The wallet refuses one alike, under its
  field. `--most-fee` is the most it may lose, a tenth of it unless told; **set-most-fee** changes
  that while it waits.
- **propose** pays the queued withdrawals, oldest first, up to 50, from what the gateway holds
  unspent, starting with what a withdrawal's anchor requires, then the largest first, leaving out
  any output worth no more than it costs to spend at the rate (`btc_input_vbytes`), so dust held
  never drags a release past the cap; one whose
  anchor is in another release waits for it, and so does one that agreed to lose less than its
  share of `--fee`. A fee under the floor for that size is refused here, naming what it takes. It
  takes a signer's `--evm-key`; without `--fee` it goes by the network as a pass does.
- **sweep** pays what older federations hold to the one in force, as much as the fee pays at the
  floor; sweep again for the rest. Its fee is owed to the backing once it is shown. It takes a signer's `--evm-key`.
- **sign** goes over every open release against the gateway's state before signing it: each
  withdrawal named once, still proposed, paid what it burned less its share of the fee, within what
  it agreed to lose, and with its anchor among the inputs; each
  input once, pending and held; only change besides, to the federation it was proposed under; the
  fee within its limits; the version, sequence and lock time fixed; and the transaction written as
  Bitcoin writes it, with the id the gateway keeps. One that fails is logged and left unsigned, for
  the next one, as is one this signer voted to cancel: `sign` by hand votes on nothing, and a pass does.
  The check is `check_btc_release`: a fault in the gateway alone should not be enough to take real
  bitcoin. It then signs each input it has not signed, unless all hold their threshold already.
- **A signer refuses what pays too much.** A release past the most a virtual byte this signer signs
  for gets its pass's vote, each pass going on: `--max-fee-rate` if told, else 3 times the fastest rate
  Esplora says, at least 25, and never under the gateway's floor, so a quiet network refuses no normal
  fee. Esplora out of reach, or no `--esplora`, should not halt withdrawals, so it goes on signing up
  to a fixed 200, saying so, and past that leaves it, with no vote. A release it refuses, or that falls
  past the limit as fees calm, is under its threshold, so the federation cancels it, and fees rising
  past one proposed earlier refuse nothing; one with all its signatures is left to be mined or bumped,
  as above. Esplora is trusted for this, so one that lies could get a good release voted down, which
  costs a delay, and the cap still holds whatever it says: run your own for a limit that does not rest
  on someone else's.
- **`--cancel-after`** takes a time, `6h`, `90m` or `30s`, 6 hours unless told: see
  [the gateway](#the-gateway). A pass reading a release's age needs a node with `--index-events`.
- **cancel** is a signer's vote, signed with `--evm-key`.
- **bump** proposes a bump of a release, by any proposal of it, with a signer's `--evm-key`: a
  replacement of its newest version, or with `--child` a child paying for the version Bitcoin holds,
  or, should a child pay for it already, a replacement of that; at `--rate`, else what Esplora says the
  network asks within 3 blocks, and never past what this signer signs for. It is refused while any of
  the release waits for signatures. The others sign it only while their Esplora says it is stuck.
- **`--bump-after`** takes a time, 1 hour unless told, which `btc.bump_after_ms` overrides.

## Caveats

- **The federation is trusted:** a threshold of its signers could mint wrapped assets for nothing
  from EVM chains. Bitcoin deposits are proven to the relay instead, but what the gateway holds is
  still theirs to spend: a threshold could pay it anywhere on Bitcoin, as with Liquid or WBTC.
- **Bitcoin reorganizing does not stop `relay-sync`.** It goes back to the newest block the relay and
  Esplora agree on and submits from there, so a relay left on a branch that lost moves to the one
  Bitcoin took. Testnet4 does this often.
- **The relay follows the most work it has been shown.** Until someone submits the real chain's
  headers, a branch mined in secret with more work than what was submitted could prove a deposit
  that never was: keep it synced, and keep `bitcoin_depth` above what an attacker could mine.
- **A release that never confirms holds its inputs:** once proposed, they are pending until it is
  shown released or cancelled. One stuck with every signature, as fees rose past what it pays, can
  no longer be cancelled: the signers' passes bump it, by a replacement or a child. One its
  withdrawals' limits hold back waits for them to be raised, or for fees to fall.
- **A child's fee comes from the federation's change:** the withdrawals it is charged to were paid
  already, so the gateway then holds that much less than the wBTC it backs, `shortfall()`, until later
  releases repay it, a little from each withdrawal. Each child is bounded by the cap and its
  withdrawals' limits, and replace-by-fee, which costs the backing nothing, goes first.
- **What was in flight at a reset stays there.** Whatever left Gateway.sol for a gateway before it
  was set up anew is signed by no one and arrives nowhere, and whatever left before `chainv4 bridge
  peer` named the new one the same, and no refund takes it back, as no gateway it was for can void it:
  join them before anyone sends.
- **The gateway's owner sets its peers.** A signer vouches for what the peer gateway says left it,
  so an owner who repointed a peer could have fake deposits signed. `chainv4 bridge` refuses to
  sign or relay through a peer other than the Gateway.sol the directory names on that chain, or
  through any on a chain it leaves out, but for one its own operator names; the gateway taking its
  peers from the directory itself is in the todo.
- **Gateway.sol's signer set follows chainv4's** only when the federation in force signs the next
  one over (`setSigners`); the tooling for that is in the todo.
- **Names are the asset's own,** cut to what the registries take: a wrapped token says what it
  wraps, not that it is genuine. Check a wrapped asset by `origin_of` or `originOf`.
