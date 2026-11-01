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
  token, NFT or multi-token amount moved in with `transfer_from` (approve the gateway first).
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
chainv4 send --key member.key approve-change --directory '{"gateway":"contract:0x…","evm_gateways":[{"chain_id":11155111,"gateway":"0x…"}],…}'
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
  paying the gas with `--evm-key`. Anyone may relay.
- **watch** does both every `--every` seconds, relaying whatever became of signing.

**Each transfer stands alone.** A transfer the gateway or Gateway.sol refuses, as one a recipient
will not take, or with more than a gateway here holds, is said with its nonce, chains and why, and
the pass goes on to the next. Under `watch` it sits out 1, 2, 4 and so on up to 64 passes between
tries, since a dry run or a failed delivery costs something every time, and each pass still lists
it, `still refused: …`, until it goes through. One EVM chain out of reach stops no other.

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
(`wrapped.lua`, cloned as Wrapped Bitcoin), the most fee a release may pay, and how long an old
federation's deposit addresses stay open.

**Depositing.** Every address here has its own deposit address on Bitcoin, under the federation in
force, for this chain and this gateway, a P2WSH any wallet pays:

```
<its 33 bytes> OP_DROP <chain id, 8 bytes little-endian><the gateway's 33 bytes> OP_DROP
<threshold> <signer keys…> <count> OP_CHECKMULTISIG
```

Once a payment to it is `bitcoin_depth` deep in the relay, anyone shows it, `deposit(block, tx,
index, path, coinbase, coinbase_path, vout, recipient)`, and its value is minted as wBTC to that address, once. The gateway
holds the output, by its transaction and place, for a later release.

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
it alone. The script is one Bitcoin's nodes relay: P2PKH, P2SH, or a witness program (P2WPKH,
P2WSH, P2TR or a later version), as a release with any other output would never be mined.

**Releasing** pays queued withdrawals:
1. **Proposed:** anyone proposes a release, an unsigned Bitcoin transaction, with the
   withdrawals it pays, at most 50, each once. The gateway checks its inputs are outputs it holds
   unspent, each once, it pays each withdrawal what it burned less its share of the fee, its one
   other output is change to the federation in force, 546 satoshis or more, and its fee is between
   the floor and the cap. It is
   version 2, with sequence `0xffffffff` and no lock time, so nothing delays it, and at most
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
one for one whatever the fee. It follows that a release spends just what they burned, or leaves
change of at least dust; the last satoshis under dust wait for a deposit. A sweep pays no one, so its
fee stays the federation's. Anyone may propose, so anyone may propose at the most a withdrawal agreed
to lose: that limit, and the cap, are the protection against it. `withdrawal(id)` says the limit, and
its share once it is in a release.

**The fee** of a release is held between two limits set once, at setup: `fee_cap`, the most it may
pay in satoshis, and `fee_floor`, the least per virtual byte, so a release cannot hold its inputs
for ever at next to nothing. Its size is reckoned as if signed, each signature at its largest:
`btc_release_vbytes` does the same for the tools. A cap under 710 virtual bytes at the floor, the
largest release one input can be, is refused at setup, as it could leave no release allowed; a
larger release past the cap is split. Read them with `fee_limits()`.

**Cancelling** frees what a release holds when it cannot be finished: one stuck on an old
federation no one can reach, or one signers refuse.
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

**A new federation** takes over the deposit addresses: new ones are made under it, and the old
ones stay open for the grace period. A **sweep** is a release with no withdrawals, paying what
older federations hold to the one in force.

**Reading it:** `wbtc()`, `deposit_script(address)` (the witness script, the output script and
the federation's version), `utxo(n)`, `utxo_of(outpoint)` and `utxo_count()`, `withdrawal(id)`,
`anchor(id)` and `withdrawal_count()`, `proposal(id)`, `voted(id, key)` and `proposal_count()`,
`change_script()` or `change_script(version)`, `fee_limits()`.

### The commands

`chainv4 bridge btc` drives both from an Esplora API (`--esplora`, such as
`https://mempool.space/api`), the gateway the one the [directory](governance.md#directory) names
as `btc_gateway`, or `--btc-gateway` before it does:

```sh
chainv4 send deploy --wasm btc_relay.wasm                      # the relay
chainv4 bridge btc relay-start 0x<relay> 840000 --esplora URL   # its checkpoint
chainv4 bridge btc relay-sync 0x<relay> --esplora URL           # the headers since, 25 a tx; run it often
chainv4 bridge btc deploy 0x<relay> --fee-cap 50000 --fee-floor 2 --grace-hours 168
chainv4 bridge btc address ed25519:0x… --btc-gateway 0x…       # where to pay, bc1q…
chainv4 bridge btc deposit <txid> 0 ed25519:0x… --btc-gateway 0x… --esplora URL
chainv4 bridge btc withdraw bc1q… 20000 --btc-gateway 0x…
chainv4 bridge btc propose --fee 1000 --btc-gateway 0x… --esplora URL
chainv4 bridge btc sign --btc-gateway 0x… --evm-key signer.key  # each signer
chainv4 bridge btc cancel 1 --btc-gateway 0x… --evm-key signer.key  # each signer's vote
chainv4 bridge btc finish 1 --btc-gateway 0x… --esplora URL     # assembled and broadcast
chainv4 bridge btc released 1 --btc-gateway 0x… --esplora URL
chainv4 bridge btc sweep --fee 1000 --btc-gateway 0x… --esplora URL
```

- **deploy** takes `--fee-floor` in satoshis a virtual byte, 2 unless told. Sync the relay first.
- **withdraw** takes a `bc1q…` address, P2WPKH or P2WSH, or another output script Bitcoin's nodes
  relay as `0x…` (a Taproot output as its `5120…` script). `--most-fee` is the most it may lose, a
  tenth of it unless told; **set-most-fee** changes that while it waits.
- **propose** pays the queued withdrawals, oldest first, up to 50, from what the gateway holds
  unspent, in the order it took it, starting with what a withdrawal's anchor requires; one whose
  anchor is in another release waits for it, and so does one that agreed to lose less than its
  share of `--fee`. A fee under the floor for that size is refused here, naming what it takes.
- **sweep** pays what older federations hold to the one in force, as much as the fee pays at the
  floor; sweep again for the rest.
- **sign** goes over every open release against the gateway's state before signing it: each
  withdrawal named once, still proposed, paid what it burned less its share of the fee, within what
  it agreed to lose, and with its anchor among the inputs; each
  input once, pending and held; only change besides, to the federation it was proposed under; the
  fee within its limits; the version, sequence and lock time fixed; and the transaction written as
  Bitcoin writes it, with the id the gateway keeps. One that fails is logged and left unsigned, for
  the next one, as is one this signer voted to cancel. The check is `check_btc_release`: a fault
  in the gateway alone should not be enough to take real bitcoin. It then signs each input it has
  not signed.
- **cancel** is a signer's vote, signed with `--evm-key`.

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
  no longer be cancelled: it is bumped by a child that pays for it, spending one of its outputs.
- **What was in flight at a reset stays there.** Whatever left Gateway.sol for a gateway before it
  was set up anew is signed by no one and arrives nowhere, and whatever left before `chainv4 bridge
  peer` named the new one the same: join them before anyone sends.
- **The gateway's owner sets its peers.** A signer vouches for what the peer gateway says left it,
  so an owner who repointed a peer could have fake deposits signed. `chainv4 bridge` refuses to
  sign or relay through a peer other than the Gateway.sol the directory names on that chain, or
  through any on a chain it leaves out, but for one its own operator names; the gateway taking its
  peers from the directory itself is in the todo.
- **Gateway.sol's signer set follows chainv4's** only when the federation in force signs the next
  one over (`setSigners`); the tooling for that is in the todo.
- **Names are the asset's own,** cut to what the registries take: a wrapped token says what it
  wraps, not that it is genuine. Check a wrapped asset by `origin_of` or `originOf`.
