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
| [Validity proofs](proofs.md) | Ranges of blocks proven in SP1 to come to their state roots: the replay over a witness, the program, proving by hand, Tx::Prove and the proofs fork, ChainV4Finality.sol's proofs, withdrawals by proof and light clients. |
| [Anchoring](anchoring.md) | Finalized checkpoints written into Bitcoin, proven through the relay and paid for, nodes refusing to replace them, and new nodes picking the history Bitcoin saw first. |
| [Governance](governance.md) | Changing the consensus on a running chain: the electorate and council kinds, approvals, and how a change takes effect. |
| [Fees](fees.md) | Gas, scorers, base fees and bids, the block gas limit, and where fees go. |
| [Contracts](contracts.md) | The Lua and WebAssembly engines, with the EVM on its own page, fuel, the host API, lifecycle hooks, calls and proxies. |
| [Ethereum's tools](ethereum.md) | Ethereum-signed transactions and JSON-RPC at POST /rpc: MetaMask, Foundry and ethers against chainv4, and what differs from an Ethereum node. |
| [EVM](evm.md) | Solidity contracts on revm: addresses and aliases, deploying, calling and reading, gas, logs and reverts, and the example counter. |
| [Contract storage](contract-storage.md) | A guide to how contracts keep state: plain storage, namespaces, the own space, layouts with key kinds and units that upgrades must keep, and moving a contract between engines, with a worked example. |
| [Multisig](multisig.md) | Native multisig accounts: M of N members' keys, any kind, signatures collected off chain or approvals on chain, the multisig paying its own fee, members changed by its quorum, in the CLI, the wallet and the offline signer. |
| [Names](names.md) | Names such as alice.dev: registering for years at a price by length, burned, records, subnames, primary names, expiry and grace, and prices changed by governance. |
| [Token standard](token-standard.md) | The interface every token contract follows, and the registry that lists them. |
| [NFT standards](nft-standard.md) | NFTs, one owner a token, and multi-token collections, amounts of each id: their interfaces, safe transfers, provable owners, metadata, the collection registry, the factory and the CLI. |
| [Atomic swaps](swaps.md) | Coins, tokens or NFTs between any two of chainv4, Bitcoin and EVM chains, with no one in between: the two-leg terms, the swap contract, Swap.sol, Bitcoin's HTLC, the CLI and the wallet's Swaps page. |
| [Bridges](bridges.md) | Wrapped assets between chainv4, Bitcoin and EVM chains: the federation that vouches and signs, the gateways and their signature board, wrapped tokens and collections, the CLI's signing and relaying, and the crypto contracts check it all with. |
| [DEX](dex.md) | Constant-product pools, as Uniswap V2 keeps them: the native DEX for the coin, tokens and ERC-20s, the EVM's factory, pairs and router, contract cloning for share tokens, the CLI and the wallet's Trade page. |
| [Storage](storage.md) | The database, its columns, state layers, undo records and scans. |
| [Node and network](node.md) | The node loop, the mempool, peer-to-peer networking, sync, and syncing from a snapshot. |
| [Light client](light-client.md) | Following a chain from its genesis id alone: headers by consensus, finality by proven voters, and values by proofs. |
| [Crypto and encoding](crypto.md) | Hashes, keys, signatures, signing domains and how everything is encoded. |
| [HTTP API](api.md) | Every route and what it answers. |
| [Explorer](explorer.md) | The egui explorer for the desktop, the browser, Android and iOS: pages, hops, tokens, sending and how it stays live. |
| [Wallet](wallet.md) | The egui light wallet: sealed keys, proven balances and history, sending and receiving. |
| [Updates](updates.md) | Releases: the signed manifest, release keys and their threshold, publishing, nodes and apps warning while behind, chainv4 update and --auto-update, and upgrades that stop older nodes at a height. |
| [Deploying](deploy.md) | Starting a fresh test chain on the VPS with everything enabled: the runbook, and what bit. |
| [Docker](docker.md) | A node in a container behind nginx, with HTTPS from your certificate, Let's Encrypt, or a self-signed one. |
| [Development](development.md) | Building, features, tests, CI, the ui crate the apps share, the wiki, and the conventions the code follows. |
| [Todo](todo.md) | What is done, what is next, and known gaps. |
| [Queue](queue.md) | Briefs for the review and CI, the testnet exercises and the pentest. |
