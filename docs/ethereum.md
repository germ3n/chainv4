# Ethereum's tools

A chain that runs the [EVM](evm.md) takes Ethereum's own transactions, signed by MetaMask,
Foundry, ethers or any Ethereum wallet, and answers Ethereum's JSON-RPC at `POST /rpc`. A dApp
points its provider at the node and works as it would on Ethereum: it deploys and calls contracts,
reads them, pays addresses and follows logs.

## Ethereum-signed transactions

`Tx::Ethereum` carries a transaction's signed bytes as Ethereum's wallets make them
(`src/crypto/ethereum.rs`):

| Type | What it is |
| --- | --- |
| Legacy | A gas price, with the chain id in `v` as EIP-155 puts it. Ones without a chain id are refused: they could be replayed on any chain. |
| 1, EIP-2930 | A gas price and an access list. |
| 2, EIP-1559 | A most per gas, a priority fee and an access list. What MetaMask and Foundry send by default. |

Blob transactions, type 3, and EIP-7702's, type 4, are refused.

**How every node checks one,** in `SignedTx::verify_on`:
- its bytes read as one of the three types, in RLP's one canonical encoding;
- its chain id is this chain's;
- its signature has a low `s`, as Ethereum requires, and recovers a secp256k1 key;
- the `SignedTx` around it says exactly what the bytes say: that key as its signer, their nonce,
  and their fee offer. `SignedTx::from_ethereum` builds it from the bytes.

**How it runs:** through the EVM, as Ethereum runs it: a call to its `to`, or without one, a
create, sending its value either way, with its access list. A plain payment runs through the EVM
too, so a contract's `receive()` runs.

**The sender** is the secp256k1 account of the key the signature recovers, the same account a
chainv4 key of that scheme sends from: one balance, one nonce. The nonce must be the account's
next, as for any transaction.

**The fee:** gas is chainv4's, as for every transaction: the intrinsic cost of the transaction's
bytes, and the EVM's gas as fuel (see [EVM](evm.md#gas)). An EIP-1559 transaction's max fee per
gas is its offer's `max_price` and its priority fee its `tip`; a legacy or type 1 one's gas price
is both. A wallet sizes all of this by asking the node, so it needs nothing special.

**Its hash** is Ethereum's, the keccak256 of its bytes. Its chainv4 id is the chain's own hash of
the whole `SignedTx`, as for any transaction. A node with the `ethereum` feature indexes one by
the other as blocks run (`Column::Ethereum`), so JSON-RPC finds a transaction by its Ethereum
hash.

**The chain id** is the first 6 bytes of the genesis id as a number, so every chain has its own,
within the 53 bits MetaMask holds. `GET /status` gives it as `chain_id`, and `eth_chainId` in hex.

Checking these transactions is in every build with `secp256k1`, and running them in every build
with `evm`, so every node agrees on them. The `ethereum` feature adds what is a node's own: the
hash index and `POST /rpc`.

## JSON-RPC

`POST /rpc` takes one JSON-RPC 2.0 request, or a batch, and answers each with status 200, as
JSON-RPC does: an error is in the answer.

| Method | What it answers |
| --- | --- |
| `eth_chainId`, `net_version` | The chain id, in hex and in decimal. |
| `eth_blockNumber` | The tip's height. |
| `eth_getBalance`, `eth_getTransactionCount` | An account's balance and nonce at its EVM address: a secp256k1 account's own, an EVM contract's, or an alias. `pending` counts what the sender has waiting in the mempool. |
| `eth_getCode`, `eth_getStorageAt` | An EVM contract's code, and a slot of its storage. |
| `eth_call` | What a call returns, run on a copy of the state at the block asked, the latest unless asked, as the block after it would run it. A revert answers code 3, `execution reverted: ` and its reason, with the bytes it returned as `data`, which ethers decodes custom errors from. |
| `eth_estimateGas` | The gas a transaction of the call needs on this chain: its bytes' intrinsic cost and the EVM's gas, with a quarter more of what ran. The gas is tried, and raised until a run succeeds, as nested calls need more than they use. |
| `eth_gasPrice`, `eth_maxPriorityFeePerGas`, `eth_feeHistory` | Twice the next block's base price; a tip of 0; and each block's base price and how full it was. |
| `eth_sendRawTransaction` | Takes a signed transaction to the node's mempool, and answers its hash. |
| `eth_getTransactionByHash`, `eth_getTransactionReceipt` | A transaction on the main chain, by its hash, and what it did: status, gas used, the price paid, a created contract's address, and its logs. `eth_getTransactionByHash` finds one waiting in the mempool too, its block fields null; its receipt is null until a block holds it. |
| `eth_getBlockByNumber`, `eth_getBlockByHash`, `eth_getBlockTransactionCountBy…` | A main-chain block: its header's fields and its Ethereum-signed transactions, as hashes or in full. `finalized` names the finalized checkpoint's block and `safe` the justified one's; on a chain without finality there is none, `null`, as on proof of work. |
| `eth_getLogs` | EVM logs by block range or block hash, address and topics: up to 10,000 blocks at once, 1,000 addresses, and 1,000 topics at each of a log's 4 places. A filter past those is refused, as public providers refuse them. |
| `eth_accounts`, `eth_syncing`, `net_listening`, `web3_clientVersion` | None, false, true, and `chainv4/` and the version. |

**What differs from an Ethereum node:**
- **Recent state, not all of it.** A balance, nonce, code, storage or call at an older block, by
  number, tag or `{"blockHash": …}` (EIP-1898), answers while the node keeps that block's state,
  `--state-history` blocks back; past that it is refused. An archive node (`--archive`) keeps
  every height from when it started keeping them.
- **Blocks list Ethereum-signed transactions alone.** chainv4's own, a Lua call or a bond, change
  balances and nonces but are not listed, so a tool never meets one it cannot read. Logs from EVM
  calls sent the chainv4 way are in `eth_getLogs`, under their chainv4 id.
- **Blocks' Ethereum fields** that chainv4 has none of are zero: `nonce`, `mixHash`, `logsBloom`
  and the uncles. A block hash is the chainv4 block id. Gas is chainv4's.
- **A transaction waiting in the mempool** is found by hash, with no block, by looking through the
  whole mempool, which is slow next to the index once a node holds thousands.

## MetaMask

Add the network by hand, in MetaMask's settings:

| Field | Value |
| --- | --- |
| Network name | Anything, such as chainv4 |
| RPC URL | The node's API and `/rpc`, such as `https://66-78-40-94.sslip.io/rpc` |
| Chain ID | `chain_id` from `GET /status` |
| Currency symbol | The chain's coin, such as DEV |

An account MetaMask holds is a secp256k1 account here: its address on the chain is
`secp256k1:` and the address MetaMask shows. Pay it from chainv4 with
`chainv4 send transfer --to secp256k1:0x…`, and it sends, deploys and calls from MetaMask.

## dApps

A dApp in the browser reaches the chain through the wallet's browser extension, as it would
MetaMask: the extension gives it `window.ethereum`, announced by EIP-6963, once the wallet's
Settings let sites ask. Reads go to the wallet's node; connecting, sending and signing are
approved in the wallet. See [Wallet](wallet.md#sites).

## Foundry and ethers

```sh
forge create Counter --rpc-url https://node.example/rpc --private-key 0x… --constructor-args 5
cast call 0xContract "count()(uint256)" --rpc-url https://node.example/rpc
cast send 0xContract "bump(uint256)" 3 --rpc-url https://node.example/rpc --private-key 0x…
```

```js
const provider = new ethers.JsonRpcProvider("https://node.example/rpc");
const wallet = new ethers.Wallet(key, provider);
const counter = await new ethers.ContractFactory(abi, bytecode, wallet).deploy(5);
await (await counter.bump(3)).wait();
console.log(await counter.count());
```

Each key is a secp256k1 account here, which needs coin to pay its fees, as on Ethereum.

## What is tested

- `src/crypto/ethereum/tests.rs` reads EIP-155's own example transaction, hashes it to the hash
  the EIP gives and recovers its sender; signs, reads back and recovers every type; and refuses a
  high `s`, a missing chain id, blob transactions and non-canonical RLP.
- `src/chain/tests/evm.rs` deploys, calls and pays with every type through blocks, and refuses
  one for another chain, or whose nonce, fee or signer is not what its bytes say.
- `src/api/tests.rs` asks `POST /rpc` what tools ask: receipts and logs by hash, blocks, calls and
  a revert's reason, estimates, sending, and batches.
- An ethers.js script against a running node deployed the example Counter, called it, decoded its
  log and a revert, paid an address with type 2 and legacy transactions, and read logs, as a dApp
  would.
