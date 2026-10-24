# chainv4 docs

How chainv4 works, part by part, and what to keep in mind while running it or changing it.

- The [README](../README.md) covers the `chainv4` command: running nodes, keys, sending
  transactions and the HTTP API at a glance.
- These pages explain how each part works and why it is built that way.
- The wiki every build writes to `target/wiki/index.html` is the reference for every type and
  function, from the `#[document]` attributes in the code.

| Page | What it covers |
| --- | --- |
| [Good to know](good-to-know.md) | Gotchas, consensus-critical rules, and what surprises people. Read this first. |
| [Architecture](architecture.md) | The parts and how a transaction and a block travel through them. |
| [Chain](chain.md) | Blocks, transactions, importing, running a block, fork choice, reorgs, receipts and rewards. |
| [Genesis](genesis.md) | Every field of a genesis file, and the rules a node checks them against. |
| [Consensus](consensus.md) | Who may produce blocks: the development authority and proof of work. |
| [Finality](finality.md) | FFG, authority and depth finality, votes, staking, slashing and signer changes. |
| [Fees](fees.md) | Gas, scorers, base fees and bids, the block gas limit, and where fees go. |
| [Contracts](contracts.md) | The Lua and WebAssembly engines, fuel, the host API, lifecycle hooks, calls and proxies. |
| [Token standard](token-standard.md) | The interface every token contract follows, and the registry that lists them. |
| [Storage](storage.md) | The database, its columns, state layers, undo records and scans. |
| [Node and network](node.md) | The node loop, the mempool, peer-to-peer networking and sync. |
| [Crypto and encoding](crypto.md) | Hashes, keys, signatures, signing domains and how everything is encoded. |
| [HTTP API](api.md) | Every route and what it answers. |
| [Explorer](explorer.md) | The egui explorer for the desktop and the browser: pages, hops, tokens, sending and how it stays live. |
| [Development](development.md) | Building, features, tests, CI, the wiki, and the conventions the code follows. |
| [Todo](todo.md) | What is done, what is next, and known gaps. |
