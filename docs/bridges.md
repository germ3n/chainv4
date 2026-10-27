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

Chains are named by their EVM chain ids, this one by its own (`GET /status`, `chain_id`).
Addresses here are written as their 33 bytes, an EVM chain's as their 20, and the coin as no
bytes.

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

- **sign** reads what left here (`Sent` events, so the node keeps `--index-events`) and what left
  each EVM chain given (Gateway.sol's `Sent` logs), and once each is final, finalized here or
  `chain_depth` deep, at the RPC's finalized block or `evm_depth` deep there, posts its signature
  to the board. One it posted already, it skips.
- **relay** delivers what left here to each EVM chain once the board holds enough signatures,
  paying the gas with `--evm-key`. Anyone may relay.
- **watch** does both every `--every` seconds.

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
  and `includes(block, txid, index, path)`, a transaction's Merkle path to the block's root.

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
force, a P2WSH any wallet pays:

```
<its 33 bytes> OP_DROP <threshold> <signer keys…> <count> OP_CHECKMULTISIG
```

Once a payment to it is `bitcoin_depth` deep in the relay, anyone shows it, `deposit(block, tx,
index, path, vout, recipient)`, and its value is minted as wBTC to that address, once. The gateway
holds the output, by its transaction and place, for a later release.

**Withdrawing** burns wBTC for a Bitcoin output script (P2WPKH, P2WSH, P2TR or any other, 80 bytes
at most) and an amount, 1,000 satoshis or more, and queues it.

**Releasing** pays queued withdrawals:
1. **Proposed:** anyone proposes a release, an unsigned Bitcoin transaction, with the
   withdrawals it pays, at most 50. The gateway checks its inputs are outputs it holds unspent, it
   pays each withdrawal exactly, its one other output is change to the federation in force, and
   its fee is above nothing and under the cap. Its inputs are then pending.
2. **Signed:** each signer signs each input as BIP143 has it, SIGHASH_ALL, and posts the 64-byte
   signature, `sign(proposal, input, signature)`; the gateway checks it against that input's
   script and amount.
3. **Broadcast:** once each input holds the threshold, anyone assembles the witnesses,
   `input_signatures(proposal, input)` giving them in the keys' order, and broadcasts it.
4. **Shown:** once it is `bitcoin_depth` deep, anyone shows it, `released(proposal, block, tx,
   index, path)`: its inputs are spent, its change held, and its withdrawals paid.

**A new federation** takes over the deposit addresses: new ones are made under it, and the old
ones stay open for the grace period. A **sweep** is a release with no withdrawals, paying what
older federations hold to the one in force.

**Reading it:** `wbtc()`, `deposit_script(address)` (the witness script, the output script and
the federation's version), `utxo(n)` and `utxo_count()`, `withdrawal(id)` and
`withdrawal_count()`, `proposal(id)` and `proposal_count()`, `change_script()`.

### The commands

`chainv4 bridge btc` drives both from an Esplora API (`--esplora`, such as
`https://mempool.space/api`), the gateway the one the [directory](governance.md#directory) names
as `btc_gateway`, or `--btc-gateway` before it does:

```sh
chainv4 send deploy --wasm btc_relay.wasm                      # the relay
chainv4 bridge btc relay-start 0x<relay> 840000 --esplora URL   # its checkpoint
chainv4 bridge btc relay-sync 0x<relay> --esplora URL           # the headers since; run it often
chainv4 bridge btc deploy 0x<relay> --fee-cap 50000 --grace-hours 168
chainv4 bridge btc address ed25519:0x… --btc-gateway 0x…       # where to pay, bc1q…
chainv4 bridge btc deposit <txid> 0 ed25519:0x… --btc-gateway 0x… --esplora URL
chainv4 bridge btc withdraw bc1q… 20000 --btc-gateway 0x…
chainv4 bridge btc propose --fee 1000 --btc-gateway 0x… --esplora URL
chainv4 bridge btc sign --btc-gateway 0x… --evm-key signer.key  # each signer
chainv4 bridge btc finish 1 --btc-gateway 0x… --esplora URL     # assembled and broadcast
chainv4 bridge btc released 1 --btc-gateway 0x… --esplora URL
chainv4 bridge btc sweep --fee 1000 --btc-gateway 0x… --esplora URL
```

- **withdraw** takes a `bc1q…` address, P2WPKH or P2WSH, or any other output script as `0x…`
  (a Taproot output as its `5120…` script).
- **propose** pays the queued withdrawals, oldest first, up to 50, from what the gateway holds
  unspent, in the order it took it; change under dust (546 satoshis) goes to the fee.
- **sweep** pays everything older federations hold to the one in force.
- **sign** signs every input of every open release with the signer's key, skipping what it signed.

## Caveats

- **The federation is trusted:** a threshold of its signers could mint wrapped assets for nothing
  from EVM chains. Bitcoin deposits are proven to the relay instead, but what the gateway holds is
  still theirs to spend: a threshold could pay it anywhere on Bitcoin, as with Liquid or WBTC.
- **The relay follows the most work it has been shown.** Until someone submits the real chain's
  headers, a branch mined in secret with more work than what was submitted could prove a deposit
  that never was: keep it synced, and keep `bitcoin_depth` above what an attacker could mine.
- **A release that never confirms holds its inputs:** once proposed, they are pending until it is
  shown released, so one stuck at too low a fee is bumped by a child that pays for it, spending
  one of its outputs; replacing a release is in the todo.
- **The gateway's owner sets its peers.** A signer vouches for what the peer gateway says left it,
  so an owner who repointed a peer could have fake deposits signed. `chainv4 bridge` refuses to
  sign or relay through a peer other than the Gateway.sol the directory names on that chain;
  the gateway taking its peers from the directory itself is in the todo.
- **Gateway.sol's signer set follows chainv4's** only when the federation in force signs the next
  one over (`setSigners`); the tooling for that is in the todo.
- **Names are the asset's own,** cut to what the registries take: a wrapped token says what it
  wraps, not that it is genuine. Check a wrapped asset by `origin_of` or `originOf`.
