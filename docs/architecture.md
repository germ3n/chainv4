# Architecture

chainv4 is a library and one binary built on it. The binary runs a node, and with a subcommand
works as a client of one. The node is a set of parts with one owner each, talking through plain
function calls and two channels.

The library (`src/lib.rs`) holds every part; `src/main.rs` only reads the command line and starts
them. Other programs link the library too: the [explorer](explorer.md), a workspace of its own,
builds and signs transactions with the library's structs and keys, in the browser as well.

## The parts

| Part | Where | What it does |
| --- | --- | --- |
| Node | `src/node/` | Owns everything below and runs the loop: takes what arrived, produces blocks, votes. |
| Chain | `src/chain/` | The blocks, which branch is the main chain, and the rules blocks run under. |
| Consensus | `src/consensus/` | Who may produce, how blocks are sealed, and how much each weighs. |
| Finality | `src/chain/finality/` | When blocks can no longer come off. |
| Fees | `src/chain/fees/` | What transactions pay, and the block gas limit. |
| Registries | `src/registry/` | Accounts, contracts, validators and signers, read and written in state layers. |
| VM | `src/vm/` | Runs contracts: Luau scripts and WebAssembly modules. |
| Storage | `src/db/` | The database behind a trait: RocksDB for nodes, memory for tests; the state tree every header's state root is the root of; and the walk a node takes a snapshot of it by. |
| Light client | `src/light/` | Follows a chain from its genesis id alone, checking headers, finality and proofs; built without the node, so the explorer runs it too. |
| Mempool | `src/mempool/` | Transactions waiting for a block. |
| Network | `src/net/` | libp2p on a thread of its own, behind channels. |
| API | `src/api/` | JSON over HTTP, on a thread of its own, reading the committed chain. |
| CLI | `src/cli/` | Keys, genesis files, `send`, which talks to a node's API, `layout`, and `light`, the light client. |
| Explorer | `explorer/` | An egui app, for the desktop and the browser, that shows a node's chain, network and mempool and sends transactions, all through the API. |
| Structs and crypto | `src/structs/`, `src/crypto/` | Blocks, transactions, addresses, amounts, hashes, keys, and Merkle roots over lists. |

Each part has one owner, passed down rather than shared through globals:

- `Node` owns the `Chain`, the `Mempool`, the `Producer` and the network link.
- `Chain` owns the database handle, the `Vm`, the consensus rules, the finality and the fees.

Tests can build several differently configured nodes in one process because nothing lives in a
static.

## Threads

- **The node loop** runs on the main thread and owns all node state. Nothing else touches it.
- **libp2p** runs on a thread of its own. It sends the node `NetEvent`s, such as a gossiped block
  or a peer's request, and takes `NetCommand`s, such as gossiping a block or answering a request.
- **The HTTP API** runs on a thread of its own and reads the database. It reaches the node
  through a `NodeHandle`, so the node stays the one that changes anything:
  - a submitted transaction goes to the node as `NetEvent::Submitted`, and the node admits it;
  - `/node` and `/mempool` ask for a report with `NetEvent::Report`;
  - `/stream` subscribes to what a streaming node broadcasts.

  Read-only calls run on the API's own engines, on a throwaway copy of the committed state.
- **A miner** mines on threads of its own. A found seal comes back as `NetEvent::Sealed`.
- **The chain's workers,** a rayon pool the `Chain` owns, run a block's transactions side by side
  and check many signatures at once. The node thread hands them a block and gets the result back,
  exactly as if it had run the transactions one by one (see [Chain](chain.md)).
- **The Lua engine** builds fresh Lua states ahead on two threads of its own, and closes used ones
  on a third, so a contract call neither waits for one nor pays to close it. Each state is used
  for one call only (see [Contracts](contracts.md)).

A test can stand in for the whole network by holding the other ends of the two channels
(`NetLink::pair`). The node tests do this, driving the loop with `Node::step` and a clock of
their own.

## A transaction's way in

1. A client signs it (`chainv4 send`) and posts it to `POST /txs`.
2. The API decodes it and hands it to the node.
3. The node's mempool admits it. The signature, nonce and fee offer are checked against the tip's
   state. The node then gossips it.
4. Peers admit it into their own mempools the same way.
5. A producer picks it when it builds a block, best-paying first, and runs it to be sure it fits.

## A block's way through

1. A producer builds a block on the tip (`Chain::build`), seals it, imports it itself, and gossips
   it.
2. A peer imports it (`Chain::import`). It checks the header against its parent, the roots, the
   base price and the seal.
3. When the block extends the tip, the peer runs its transactions, credits the reward, checks and
   pays its finality proofs, and commits it all as one batch.
4. A block on a side branch is stored without running. If its branch comes to weigh more, the main
   chain is rolled back to where they meet and the branch is run on top, in one commit.
5. A block whose parent the peer does not have makes it ask its sender how far it got, and sync
   fetches what is missing.

## What is consensus and what is not

Every node on a chain must agree on everything that decides whether a block is valid and what
it does:

- the genesis and every parameter in it;
- the encodings and pinned tags;
- the engines and their settings;
- the hash and key schemes;
- the gas schedule and pricing.

A change to any of these splits a chain.

What a node keeps for itself is not consensus, and can differ between nodes:

- the votes it has heard;
- the justifications it learned;
- its mempool;
- its peers;
- the HTTP API.

The [Good to know](good-to-know.md) page lists the consensus-critical pieces one by one.
