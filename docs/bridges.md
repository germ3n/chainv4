# Bridges

Bridges carry value between chainv4 and other chains as wrapped assets: BTC as wBTC here, ETH and
ERC-20s and NFTs here as tokens and collections, and chainv4's own coin, tokens and NFTs there as
ERC-20s, ERC-721s and ERC-1155s. An [atomic swap](swaps.md) trades what exists on each chain; a
bridge mints a claim on what it holds locked on the other.

This page grows with the bridge batches: the federation, the contracts' crypto, and the gateways
between chainv4 and EVM chains, for coins, tokens and NFTs, are in; Bitcoin, through a light client
contract, comes next.

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
- **Genesis** may name one, `federation` (see [Genesis](genesis.md)): version 0.
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
chainv4 send name register gateway                        # found by the name gateway.<suffix>
chainv4 bridge deploy-evm --eth-rpc URL --evm-key k.key    # Gateway.sol under the federation in force
chainv4 bridge peer 0x<Gateway.sol> --eth-rpc URL --evm-key k.key
```

`deploy-evm` reads the federation from `GET /status`, so governance names one first. `peer` makes
each gateway the other's peer: signed here by the gateway's owner, there by Gateway.sol's.

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

## Caveats

- **The federation is trusted:** a threshold of its signers could mint wrapped assets for nothing.
  Bitcoin deposits will be proven by a light client contract instead.
- **The gateway's owner sets its peers.** A signer vouches for what the peer gateway says left it,
  so an owner who repointed a peer could have fake deposits signed. Signers should check each peer
  is the Gateway.sol they expect; moving peers to governance is in the todo.
- **Gateway.sol's signer set follows chainv4's** only when the federation in force signs the next
  one over (`setSigners`); the tooling for that is in the todo.
- **Names are the asset's own,** cut to what the registries take: a wrapped token says what it
  wraps, not that it is genuine. Check a wrapped asset by `origin_of` or `originOf`.
