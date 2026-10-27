# Bridges

Bridges carry value between chainv4 and other chains as wrapped assets: BTC as wBTC here, ETH and
ERC-20s and NFTs here as tokens and collections, and chainv4's own coin, tokens and NFTs there as
ERC-20s, ERC-721s and ERC-1155s. An [atomic swap](swaps.md) trades what exists on each chain; a
bridge mints a claim on what it holds locked on the other.

This page grows with the bridge batches: the federation and the contracts' crypto came first.

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
