# chainv4 docs

How chainv4 works, part by part, and what to keep in mind while running it or changing it.

- The [README](../README.md) covers the `chainv4` command: running nodes, keys, sending
  transactions and the HTTP API at a glance.
- These pages explain how each part works and why it is built that way.
- The wiki every build writes to `target/wiki/index.html`, or `cargo run -p wiki` on its own, is
  one page that holds these guides and the reference for every type and function, from the
  `#[document]` attributes in the code. Open it in a browser; it needs nothing else.

| Page | What it covers |
| --- | --- |
| [Good to know](good-to-know.md) | Gotchas, consensus-critical rules, and what surprises people. Read this first. |
| [Architecture](architecture.md) | The parts and how a transaction and a block travel through them. |
| [Chain](chain.md) | Blocks, transactions, importing, running a block, fork choice, reorgs, receipts and rewards. |
| [Genesis](genesis.md) | Every field of a genesis file, and the rules a node checks them against. |
| [Consensus](consensus.md) | Who may produce blocks: the development authority, proof of work, a rotating authority and production by stake. |
| [Finality](finality.md) | FFG, authority and depth finality, votes, staking, slashing and signer changes. |
| [Governance](governance.md) | Changing the consensus on a running chain: the electorate and council kinds, approvals, and how a change takes effect. |
| [Fees](fees.md) | Gas, scorers, base fees and bids, the block gas limit, and where fees go. |
| [Contracts](contracts.md) | The Lua and WebAssembly engines, with the EVM on its own page, fuel, the host API, lifecycle hooks, calls and proxies. |
| [EVM](evm.md) | Solidity contracts on revm: addresses and aliases, deploying, calling and reading, gas, logs and reverts, and the example counter. |
| [Contract storage](contract-storage.md) | A guide to how contracts keep state: plain storage, namespaces, the own space, layouts with key kinds and units that upgrades must keep, and moving a contract between engines, with a worked example. |
| [Token standard](token-standard.md) | The interface every token contract follows, and the registry that lists them. |
| [Storage](storage.md) | The database, its columns, state layers, undo records and scans. |
| [Node and network](node.md) | The node loop, the mempool, peer-to-peer networking, sync, and syncing from a snapshot. |
| [Light client](light-client.md) | Following a chain from its genesis id alone: headers by consensus, finality by proven voters, and values by proofs. |
| [Crypto and encoding](crypto.md) | Hashes, keys, signatures, signing domains and how everything is encoded. |
| [HTTP API](api.md) | Every route and what it answers. |
| [Explorer](explorer.md) | The egui explorer for the desktop, the browser and Android: pages, hops, tokens, sending and how it stays live. |
| [Wallet](wallet.md) | The egui light wallet: sealed keys, proven balances and history, sending and receiving. |
| [Docker](docker.md) | A node in a container behind nginx, with HTTPS from your certificate, Let's Encrypt, or a self-signed one. |
| [Development](development.md) | Building, features, tests, CI, the ui crate the apps share, the wiki, and the conventions the code follows. |
| [Todo](todo.md) | What is done, what is next, and known gaps. |
