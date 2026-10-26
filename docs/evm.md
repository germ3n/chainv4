# EVM

chainv4 runs Ethereum's virtual machine beside its Lua and WebAssembly engines, on
[revm](https://github.com/bluealloy/revm) (`src/vm/evm/`). Solidity and Vyper contracts compile
for it unchanged: they deploy as their init code, are called with ABI calldata, keep storage by
slot, emit logs, create contracts of their own and revert with reasons, as on Ethereum. The EVM
runs Ethereum's Prague rules.

The EVM shares the chain's state:

- **One balance and one nonce per account** across every engine. Coin an Ed25519 account holds
  is what it spends in the EVM, and coin an EVM contract pays out lands in the same balance.
- **EVM code** is a contract record like any other, of engine `evm`, in the contract registry.
- **EVM storage** lives in the storage column, keyed by the contract and the 32-byte slot.
- **Calls** go both ways: EVM contracts call Lua and WebAssembly ones through a precompile, and
  those call EVM contracts with `evm.call`. See
  [Calls with Lua and WebAssembly contracts](#calls-with-lua-and-webassembly-contracts).

Ethereum's own wallets and tools work with it too: MetaMask, Foundry and ethers sign
transactions the chain takes as they are, and the node answers Ethereum's JSON-RPC at `POST /rpc`.
See [Ethereum's tools](ethereum.md).

## Turning it on

Genesis's `engines` must name `evm`, and every node on the chain must be built with the `evm`
cargo feature, which is on by default:

```json
"engines": ["lua", "wasm", "evm"]
```

The dev chains include it. A chain whose genesis leaves it out refuses EVM deploys and EVM calls,
`this chain does not run evm contracts`, and adding it later makes a new chain, as any engine
does.

## Addresses

The EVM knows 20-byte addresses, and chainv4 has an address kind for them, `Address::Evm`,
written `evm:0x…`. Each account takes part in the EVM at one 20-byte address:

| Account | Its EVM address |
| --- | --- |
| An EVM contract, `evm:0x…` | Its own 20 bytes. |
| A secp256k1 account, `secp256k1:0x…` | Its own 20 bytes, which are Ethereum's address for the key. |
| Anything else: Ed25519, EdBlake, a Lua or wasm contract | Its **alias**: the last 20 bytes of the keccak256 of its key. |

`msg.sender`, `tx.origin`, `address(this)`, `block.coinbase` and every address the EVM is given
or returns are these 20 bytes.

**An alias is written down the first time its account sends a transaction** of any kind on a
chain that runs the EVM: a transfer, a Lua call, a bond, an EVM call, whether it succeeds or fails.
A name resolved through the [names precompile](#names) writes it down too. From then on, coin the
EVM sends to the alias reaches the account. Until then, the EVM has no way to know whose alias it
is, and coin sent to those 20 bytes goes to the secp256k1 account with them, which no one holds
the key to. So an account that has only ever received should send something before it gives its
alias to a contract to pay. secp256k1 accounts and EVM contracts need nothing written down.

A contract the EVM creates lives at Ethereum's address for it: the keccak256 of the RLP of the
creator's EVM address and nonce, as CREATE gives it, or CREATE2's. A deploy from an Ed25519
account goes to the address Ethereum would give its alias, at the account's nonce.
`chainv4 send deploy --evm` prints it before the deploy lands.

## Deploying

`Tx::Deploy` carries `Code::Evm`, the init code as `solc --bin` or `forge build` writes it, with
the constructor's ABI-encoded arguments after it. The chain runs the init code, and what the
constructor returns becomes the contract's code. Two of Ethereum's limits apply: 48 KiB of init
code, and 24 KiB of deployed code. A deploy sends no coin, so a payable constructor receives
none: send it with a call afterwards.

```sh
chainv4 send --key me.key deploy --evm contracts/evm/Counter.bin --constructor "(uint256)" --arg 5
```

- **The file** is the init code in hex, as solc writes it, with or without `0x`, or its raw bytes.
- **`--constructor`** lists the constructor's parameter types, and each **`--arg`** gives one
  value, in order, encoded as Solidity's ABI says (`src/abi/`): every type, `string`, `bytes`,
  arrays and tuples too, those last two written as JSON, such as `[1, 2]` or `["0x…", 5]`.

An EVM contract cannot be replaced by `Tx::Upgrade`. EVM code is fixed once deployed, as on
Ethereum: upgradeable contracts use a proxy, which the EVM runs as usual, with DELEGATECALL.

## Calling

`Tx::EvmCall { to, value, input }` calls an EVM contract with raw calldata, sending `value` with
it. Sent to anything that holds no EVM code, it simply pays the address `value`, as an Ethereum
transfer does. `to` may be any address: an EVM contract, a secp256k1 account, or an Ed25519 one,
which the EVM reaches at its alias.

A Lua or wasm contract is called with `Tx::ContractCall`, and an EVM contract only with
`Tx::EvmCall`: a contract call to an `evm:` address is refused.

```sh
# Sends a transaction that bumps the counter by 3.
chainv4 send --key me.key evm-call --to 0xc578dcea6338eae42aad8bc6a3411c4df684a4e9 --function "bump(uint256)" 3

# Reads the count without sending anything, as eth_call does.
chainv4 send --key me.key evm-call --to 0xc578…a4e9 --function "count()" --read
# 0x0000000000000000000000000000000000000000000000000000000000000008 (8)
# gas …

# Pays 1000 into the contract with deposit().
chainv4 send --key me.key evm-call --to 0xc578…a4e9 --function "deposit()" --value 1000

# Dynamic types too, arrays as JSON; or calldata made elsewhere, with --data.
chainv4 send --key me.key evm-call --to 0xc578…a4e9 --function "setNames(string[])" '["alice", "bob"]'
chainv4 send --key me.key evm-call --to 0xc578…a4e9 --data "$(cast calldata 'setName(string)' hello)"
```

- **`--to`** takes a bare `0x` and 40 hex digits as an EVM contract, or any chainv4 address.
- **`--function`** takes the signature, and the values after it are its arguments, encoded as
  `--constructor`'s are. An address argument may be any chainv4 address: an Ed25519 one is
  passed as its alias.
- **`--read`** runs the call on the node, through `POST /offer`'s dry run, and prints what it
  returned. A single word also shows as a number. Nothing is signed for the chain or sent. A
  revert is an error, with the reason.

Every call is dry run before it is sent, as contract calls are: one that would revert is refused
with its reason unless `--anyway` is given, and its gas limit is what the dry run used and a
quarter more.

## Gas

EVM gas is chainv4 gas, one for one. revm counts what a call uses, the EVM's own 21,000 base and
calldata cost included. The chain charges that as fuel, at the fee schedule's `fuel_per_gas`, on
top of the transaction's intrinsic cost, as for any contract:

- the EVM is given the gas the transaction's gas limit leaves after its intrinsic cost, capped by
  the chain's fuel cap per transaction;
- a call that runs out of gas fails with `out of fuel`, keeps nothing it did, and is charged
  everything;
- the EVM's memory, every call frame's together, is capped at the chain's `tx_limits.memory`.

revm charges no fee itself and checks no nonce: the chain has done both already. `gasprice` and
`block.basefee` read as 0 inside the EVM.

## What a contract sees

| Solidity | What it is |
| --- | --- |
| `block.number` | The block's height. |
| `block.timestamp` | The block's time, in seconds. |
| `block.chainid` | The chain id, below. |
| `block.coinbase` | The producer's EVM address. |
| `block.prevrandao`, `block.difficulty` | 0: chainv4 has no randomness beacon. |
| `block.basefee`, `tx.gasprice` | 0. |
| `blockhash(n)` | The id of the block at height `n` on this branch, when the chain hash is 32 bytes; otherwise 0. |
| `msg.sender`, `tx.origin` | EVM addresses, as above. |

**The chain id** is the first 6 bytes of the genesis id, as a number, so every chain has its own
and it fits JavaScript's numbers. `GET /status` shows the genesis id.

## In the apps

- **The wallet's Contracts page** keeps EVM contracts with their ABIs, reads them, sends their
  functions through the Send page's review, and deploys init code with typed constructor
  arguments. It does the same for Lua and WebAssembly contracts. See
  [Wallet](wallet.md#contracts).
- **The explorer** shows an EVM contract's page, and with its ABI pasted, reads its functions with
  `eth_call` and shows its logs and calls by name. See [Explorer](explorer.md).
- **The ABI** they share is `src/abi/`, behind the `abi` feature: Solidity's encoding of every type,
  dynamic ones included, a contract's JSON ABI, selectors, logs, custom errors, and EVM and CREATE
  addresses worked out without an EVM. `chainv4 send evm-call --function` uses it too.

## Names

On a chain with [names](names.md), a precompile at `0x0000000000000000000000000000000000004e53`
resolves them: `resolve(string) returns (address)` and `name(address) returns (string)`.
`contracts/evm/NamePayer.sol` pays a name with it. See [Names in contracts](names.md#names-in-contracts).

## Calls with Lua and WebAssembly contracts

EVM contracts and native ones, Lua and WebAssembly, call each other within one transaction, as
deep as `MAX_CALL_DEPTH`, 8, across the engines, and a revert on either side unwinds both.

**From the EVM,** a precompile at `0x0000000000000000000000000000000000004e43` calls native
contracts, on every chain that runs the EVM:

```solidity
interface Native {
    function callNative(bytes32 target, string calldata name, string calldata types, bytes calldata args)
        external payable returns (bytes memory);
}

bytes memory out = Native(address(0x4e43)).callNative{value: msg.value}(counter, "add", "u64", abi.encode(by));
uint64 total = abi.decode(out, (uint64));
```

- `target` is the native contract's 32 bytes, as `contract:0x…` writes them.
- `types` lists its arguments' native types, comma separated, and `args` holds them ABI-encoded
  as these:

  | Native | ABI |
  | --- | --- |
  | `u64`, `u128`, `u256`, `i64`, `i128`, `i256` | `uint64` … `int256` |
  | `amount` | `uint256` |
  | `number` | `int64` |
  | `bool`, `string`, `bytes` | themselves |
  | `address` | `address`, read as the account behind those 20 bytes |
  | `contract` | `bytes32`, a native contract |

- What the function returns comes back ABI-encoded, one value after another, each by the same
  table: a native contract as `bytes32`, any other address as its 20 bytes inside the EVM.
- The value sent goes to the native contract. It sees the calling EVM contract as `ctx.sender`.
- A native call that fails reverts with `Error(string)`, so Solidity passes the reason up. One
  that runs out of fuel runs the call out of gas. In a static call, as a `view` makes, a native
  function that writes anything reverts.
- It costs 2,600 gas and 3 a word of input, then the fuel the native call burns, at the chain's
  rate of fuel to gas.

**From Lua,** the `evm` library calls EVM contracts and speaks the ABI:

```lua
function M.bump(counter, by)
    local output = evm.call(counter, evm.encode("bump(uint256)", u256(by)))
    return evm.decode("(uint256)", output)
end
```

`evm.call(address, calldata, value)` returns what the EVM returned, as bytes, and raises an error,
with the revert's reason, when it reverts. `evm.encode` takes a signature with a name for
calldata, or types alone in parentheses for arguments, and `evm.decode` reads either back:
integers as their own types, addresses as the account behind them, arrays and tuples as tables.
The EVM sees the Lua contract at its alias.

**From WebAssembly,** the SDK's `evm_call(&address, value, &input)`, `abi_encode(signature,
&values)` and `abi_decode(types, &data)` do the same; arrays and tuples have no value there.
`contracts/examples/evm` bumps `Bridge`.

**How unwinding works.** revm keeps the EVM's changes in its journal and reverts a frame's with
the frame. A native call runs on a layer with the journal written into it, so it sees everything
the EVM did so far. What it changed that the EVM sees goes back into the journal, and reverts with
it: balances, nonces, EVM storage and EVM code, which a nested EVM call may have written. Anything
else is kept aside with a marker slot in the journal. A frame that reverts takes the marker with
it, and when the transaction ends, only what still has its marker is kept. So if an inner frame
reverts and the outer one catches it, the native call that frame made is undone, and the rest
stays.

**Events** from native calls land in the receipt where the calls ran, among the EVM's logs.

**Reentry** follows the native rule: a native contract already running further up, even across the
EVM, is entered again only through a function marked reentrant.

`contracts/evm/Bridge.sol` calls a native counter, reverts after it, catches a revert, reads in a
static call, and is called back. `src/chain/tests/evm.rs` drives it through Lua, then the EVM,
then Lua, and through the EVM, then Lua, then the EVM again.

## Logs and reverts

**Logs** become events named `log` in the transaction's receipt, emitted by the contract: their
values are each topic, as 32 bytes, then the log's data, all as `bytes`. `event Bumped(address
indexed by, uint256 count)` gives three values: the event's signature hash, the indexed address,
and the encoded count.

**A revert** fails the transaction as any contract failure does: nothing it did is kept, but its
nonce and fee are spent. Its error is `reverted: ` and the reason, from `require`'s message or
`revert("…")`, or the returned bytes in hex for a custom error or an empty revert.

## The example

`contracts/evm/Counter.sol` is a contract the tests deploy, and a starting point. `Counter.bin`
and `Counter.abi` beside it are what solc writes:

```sh
solc --evm-version prague --optimize --bin --abi contracts/evm/Counter.sol -o build
```

It has:

- a count with a constructor and a `bump` that emits `Bumped`, refusing nothing with
  `"nothing to add"`;
- `deposit` and `pay`, for coin in and out of a contract;
- `spawn`, which creates a `Child` with CREATE;
- `blockInfo`, which returns the block number, time and chain id;
- `spin`, which runs out of gas.

`src/chain/tests/evm.rs` drives every one of them through blocks, and `src/cli/tests.rs` deploys
and calls it with `chainv4 send`.

## Not yet

- **Native calls in traces.** An EVM transaction's receipt has no trace, so the native calls it
  makes are not in one either.
- **Token refreshes** a native contract asks for while the EVM calls it are not done.
- **Blob transactions and the KZG point evaluation precompile,** which needs blobs.
- **Selfdestruct** only clears what it can in the same transaction, as Ethereum's Cancun rules
  say: elsewhere it only sends the contract's coin.
