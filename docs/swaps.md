# Atomic swaps

Coins or a token on one chain for coins or a token on another, between two people, with no one
in between and no way for either to take both. The chains are chainv4, Bitcoin, and Ethereum or
any other EVM chain (Sepolia, Base, Arbitrum and so on), in any pair: DEV for bitcoin, ETH for
DEV, bitcoin for ETH, ETH on Ethereum for ETH on Base.

Each side locks what it pays under the same hash:
- on chainv4 in the swap contract;
- on Bitcoin in a hash time-locked contract (an HTLC);
- on an EVM chain in Swap.sol.

Whoever claims one lock shows the secret behind the hash on that chain, and the other side claims
the other lock with it. A lock nobody claims in time goes back to whoever made it.

All of it is behind the default-on `swap` cargo feature:

| Part | Where |
| --- | --- |
| The terms, the HTLC, Swap.sol's calls | `src/swap/` |
| chainv4's side | `contracts/lua/swap.lua` |
| An EVM chain's side | `contracts/evm/Swap.sol` |
| The CLI | `chainv4 swap` |
| The wallet | its Swaps page |

## How a swap goes

1. **The starter** makes a secret, 32 random bytes, and its SHA-256, the hash. They write an
   **offer**: the two legs (what they pay and what they get, each on its chain), the hash, the
   deadlines, and who they are on each leg.
2. **The other side** reads the offer, checks it, and shows back an **answer**: the same terms
   with their parts added.
3. **The starter locks first**, on the leg they pay, with the longer deadline (24 hours by
   default).
4. **The other side locks second**, with the shorter deadline (12 hours), once the first lock
   stands, confirmed, with the right amount, recipient and deadline.
5. **The starter claims the second lock** with the secret, before its deadline. That shows the
   secret on that chain.
6. **The other side claims the first lock** with the same secret, before its longer deadline.

Either the whole swap happens or neither side loses anything:
- if the other side never locks, the starter takes their lock back after its deadline;
- if the starter never claims, both take their locks back, the second side first;
- once the starter claims, the secret is public, and the other side has until the first deadline,
  at least four hours more, to claim in turn.

**The secret is the starter's alone until they claim.** Whoever knows it early can claim the
starter's lock without paying. The wallet keeps it sealed in the vault; the CLI prints it once.

## The terms

`swap::Terms`, written as a code, `ur:chainv4-swap`, in capitals, as a QR code or copyable text.

| Field | What it is |
| --- | --- |
| `hash` | The SHA-256 of the secret. |
| `first` | The starter's leg: what they pay, locked first. |
| `second` | The other's leg: what they pay, locked second. |
| `first_deadline`, `second_deadline` | When each leg's lock opens to a refund: Unix milliseconds. |

Each leg is a `swap::Leg` on one chain, with what is locked there, who locks it (its payer) and
who claims it (its receiver):

| Leg | What it holds |
| --- | --- |
| `Chain` | The swap contract; the token, or none for the coin; the NFT, if it locks one; the amount in smallest units; the payer's and receiver's chainv4 addresses. |
| `Bitcoin` | Mainnet or a test network; the satoshis; the HTLC's refund and claim keys, 33-byte compressed bitcoin keys. |
| `Evm` | The chain id (1 Ethereum, 11155111 Sepolia, 8453 Base…); its Swap.sol; the ERC-20 token, or none for the coin; the NFT, if it locks one; the amount (wei for the coin); the payer's and receiver's 20-byte addresses. |

**Filling in the parts:**
- An offer has the starter as the first leg's payer and the second's receiver. An answer adds
  the other side as the first leg's receiver and the second's payer.
- Who someone is on each kind of chain is a `swap::Parties`: a chainv4 address, a bitcoin key,
  an EVM address. A leg takes the one its chain needs.

**What `Terms::check` refuses:**
- both legs on one chain;
- less than 294 satoshis (Bitcoin's dust limit), or nothing on chainv4 or an EVM chain;
- deadlines past, or a second deadline less than four hours (`MIN_GAP_MS`) before the first;
- Bitcoin keys that are not keys, or the zero address on an EVM chain;
- the same party paying and receiving one leg.

**`Terms::answers`** holds an answer to every term of its offer. The wallet refuses an answer that
changes anything but the other side's parts.

**Codes made before legs still read.** Those were chainv4 against Bitcoin, and read as the same
swap in two legs. Terms are written as legs from then on, a 2 first.

**Terms that lock an NFT are written with a 3 first,** their legs holding it; terms that lock none
are still written with a 2, as before NFTs, so apps from before read every code they could.

## Deadlines

- **Times, on every chain:**
  - chainv4's lock ends at a block time, `ctx.time`;
  - an EVM chain's at `block.timestamp`, in whole seconds;
  - Bitcoin's lock time is the deadline in Unix seconds, which `OP_CHECKLOCKTIMEVERIFY` checks
    against the median time of the last eleven blocks.

  Nothing needs any chain's height.
- **Bitcoin's median time runs about an hour behind the clock,** so a Bitcoin refund confirms an
  hour or so after its deadline, not at once.
- **A block's time may be a little off the clock** on any chain. Leave hours of margin, never
  minutes: the defaults are 24 and 12 hours.
- **Claim early.** A claim sent just before a deadline may be mined after it, when the lock is
  refundable instead.

## chainv4's side: the swap contract

`contracts/lua/swap.lua`, or any contract with the same functions: nothing in the chain treats
one as special, and any contract can call one. **The terms name the contract a leg uses,** so
both sides lock and look in the same one, whatever their wallets would pick.

A new offer's leg on chainv4 takes, in this order:
1. the one given: the CLI's `--contract`, or the wallet's Settings, **Swaps**;
2. else the one the chain's [directory](governance.md#directory) names, which the wallet's
   light client proves. A name is never used: anyone could register `swap` first;
3. else the one the node advertises, started with `--swap-contract` (in Docker, `SWAP_CONTRACT`
   in `.env`), which `GET /status` shows as `swap.contract`.

| Function | What it does |
| --- | --- |
| `lock(hash, recipient, deadline)` | Locks the coin the call sends, for `recipient` to claim with the secret before `deadline`, Unix milliseconds. |
| `lock_token(hash, recipient, deadline, token, amount)` | Locks `amount` of a token, moved with `transfer_from`: approve the swap contract for it first. Sends no coin. |
| `claim(secret)` | Pays the swap whose hash is the secret's SHA-256 to its recipient, before its deadline. Anyone may send it. |
| `refund(hash)` | Pays a swap nobody claimed back to its locker, from its deadline on. Anyone may send it. |
| `lock_nft(hash, recipient, deadline, collection, id)` | Locks one [NFT](nft-standard.md), moved with `transfer_from`: approve the swap contract for it first. |
| `lock_multi(hash, recipient, deadline, collection, id, amount)` | Locks `amount` of a multi-token id, moved with `transfer_from`: make the swap contract an operator first. |
| `get(hash)` | Its state, `none`, `locked`, `claimed` or `refunded`; locker, recipient, token (the zero address for the coin, the collection for NFTs), amount, deadline, the secret once claimed, and the block it was locked in. |
| `item(hash)` | What it holds, `coin`, `token`, `nft` or `multi`, and the NFT's id, or 0; `none` for no swap. |

Hashes and secrets are `0x` and 64 hex digits. A secret must be 32 bytes, as Bitcoin's script
wants it, so a secret one chain takes the others take too. Events: `Locked(hash, locker,
recipient, token, amount, deadline)`, then `LockedItem(hash, kind, id)` for NFTs,
`Claimed(hash, secret)`, `Refunded(hash)`.

## An EVM chain's side: Swap.sol

`contracts/evm/Swap.sol`, the same functions for the EVM, built with solc 0.8 for Paris, so it
runs on every EVM chain, the ones without `PUSH0` too:

| Function | What it does |
| --- | --- |
| `lock(bytes32 hash, address recipient, uint64 deadline)` | Locks the ETH sent, until `deadline`, Unix **seconds**. |
| `lockToken(hash, recipient, deadline, address token, uint256 amount)` | Locks an ERC-20, moved with `transferFrom`: approve Swap.sol first. A token that takes a fee as it moves is refused. Tokens that return nothing, as USDT does, are taken. |
| `claim(bytes32 secret)` | Pays the recipient, before the deadline: SHA-256 through Ethereum's own precompile. Anyone may send it. |
| `refund(bytes32 hash)` | Pays the locker back, from the deadline on. Anyone may send it. |
| `lockNft(hash, recipient, deadline, address collection, uint256 id)` | Locks an ERC-721 token, moved with `transferFrom`: approve Swap.sol for it first. |
| `lockMulti(hash, recipient, deadline, address collection, uint256 id, uint256 amount)` | Locks an amount of an ERC-1155 id, moved with `safeTransferFrom`: `setApprovalForAll` for Swap.sol first. Swap.sol takes ERC-1155 tokens only as this moves them in. |
| `get(bytes32 hash)` | State (0 none, 1 locked, 2 claimed, 3 refunded), locker, recipient, token (zero for ETH, the collection for NFTs), amount, deadline, the secret once claimed, and the block it was locked in. |
| `item(bytes32 hash)` | What it holds: 0 the coin or an ERC-20, 1 an ERC-721 token, 2 an ERC-1155 amount; and the NFT's id. |

Events: `Locked`, then `LockedItem(hash, kind, id)` for NFTs, `Claimed(hash, secret)`,
`Refunded(hash)`. The secret is kept in the
contract's state as well, so `get` alone shows it, with no log search, which providers limit.

- **One address on every chain.** Swap.sol is deployed through the deterministic deployment
  proxy (`0x4e59b44847b379578588920ca78fbf26c0b4956c`, on most EVM chains) with no salt. CREATE2
  then puts it at the same address everywhere (`swap::evm::deployed_address()`), which new offers
  default to. A chain without the proxy gets a plain create, and its offers name that address.
- **Deploying it:** `chainv4 swap evm-deploy --eth-rpc URL --key FILE`, with a secp256k1 key
  holding that chain's coin for the gas. Run again, it says it is there already.
- **Reaching a chain:** by its JSON-RPC: `--eth-rpc` for the CLI, Settings, **Swaps** for the
  wallet. A provider's URL, your own node's, or chainv4's own `POST /rpc`, its EVM being an EVM
  chain too.

## Bitcoin's side: the HTLC

A P2WSH output (`bc1q…`, or `tb1q…` on testnet, longer than a key's) of this script:

```text
OP_IF
    OP_SIZE 32 OP_EQUALVERIFY OP_SHA256 <hash> OP_EQUALVERIFY <claim key>
OP_ELSE
    <lock time> OP_CHECKLOCKTIMEVERIFY OP_DROP <refund key>
OP_ENDIF
OP_CHECKSIG
```

- **Paying it** needs nothing special: send the exact amount to its address from any Bitcoin
  wallet, Sparrow included.
- **A claim** spends it with the claim key's signature, the secret and `1`; **a refund** with the
  refund key's signature and nothing, its transaction's lock time set to the deadline. Both pay
  one output, less a fee at the rate given, and are signed with BIP143's hash of the script.
- **Keys:** each side's bitcoin key is their account's, from the same words: `m/84'/0'/0'/0/n` on
  Bitcoin, `m/84'/1'/0'/0/n` on its test networks.

## From the command line

Legs are written as `--pay` and `--get`, amounts in smallest units:

| Leg | Written as |
| --- | --- |
| chainv4 | `chainv4:AMOUNT` or `chainv4:AMOUNT:contract:0x…` for a token |
| An NFT on chainv4 | `chainv4:nft:contract:0x…:ID`, or `chainv4:multi:contract:0x…:ID:AMOUNT` for a multi-token id |
| Bitcoin | `bitcoin:SATS`, `bitcoin-testnet:SATS` |
| An EVM chain | `evm:CHAIN_ID:AMOUNT[:0xTOKEN]`, or by name: `ethereum:`, `sepolia:`, `base:`, `arbitrum-one:`… |
| An NFT there | `evm:CHAIN_ID:nft:0xCOLLECTION:ID` for ERC-721, `evm:CHAIN_ID:multi:0xCOLLECTION:ID:AMOUNT` for ERC-1155, or by name: `sepolia:nft:…` |

Who you are on each: `--address` (chainv4), `--bitcoin-key` (a public key or key file), and
`--evm-address` (`0x…` or a secp256k1 key file).

```sh
# Alice pays bitcoin and starts; she gets 0.01 ETH on Sepolia:
chainv4 swap secret                                # keep the secret; share the hash
chainv4 swap offer --hash 0x… --pay bitcoin-testnet:100000 --get sepolia:10000000000000000 \
  --bitcoin-key alice-btc.key --evm-address 0x…    # prints the offer's code
# Bob:
chainv4 swap answer UR:CHAINV4-SWAP/… --bitcoin-key bob-btc.key --evm-address bob-eth.key
chainv4 swap show UR:CHAINV4-SWAP/…                # both legs, the HTLC's address and script
# Alice pays the HTLC from her Bitcoin wallet; Bob checks it confirmed, then locks his ETH:
chainv4 swap lock UR:CHAINV4-SWAP/… --key bob-eth.key --eth-rpc https://… --anyway
# Alice claims the ETH, showing the secret on Sepolia (anyone may send it):
chainv4 swap claim UR:CHAINV4-SWAP/… --secret 0x… --key alice-eth.key --eth-rpc https://…
chainv4 swap status UR:CHAINV4-SWAP/… --eth-rpc https://…   # claimed, with the secret
# Bob claims the bitcoin with it, and broadcasts what it prints:
chainv4 swap bitcoin-claim UR:CHAINV4-SWAP/… --secret 0x… --funding <txid>:<vout> --sats 100000 \
  --key bob-btc.key --to tb1q… --fee-rate 2
```

**Which leg a command acts on:**
- `lock` acts on the leg your `--key` pays.
- `claim` acts on the one it receives, or else the only one not on Bitcoin.
- `refund` acts on the one it pays.
- `--leg first|second` picks one outright.

**Locking second checks the first lock first:** it must stand as the terms say on chainv4 or the
EVM chain, or the lock is refused. A first lock on Bitcoin the CLI cannot see: check it yourself,
then lock with `--anyway`.

**Other commands:**
- `swap refund` and `swap bitcoin-refund` take a lock back after its deadline.
- `swap revealed --tx <hex>` reads the secret from a Bitcoin claim's transaction, for when the
  starter claims the bitcoin first.
- `swap offer --contract` and `--evm-contract` name contracts other than the defaults.

## In the wallet

The **Swaps** page, on an online install:
- **Start a swap** picks each leg's chain: this chain, Bitcoin, Bitcoin testnet, or an EVM chain
  from a list, or by id. Then what is locked there, the coin or a token, an NFT or an amount of a
  multi-token id, and for an EVM leg its Swap.sol, filled in as the one on every chain. It makes the offer and its secret, kept sealed
  with the rest of the vault.
- **Read an offer or an answer** reads the other side's code.

An open swap lays out each leg, where it stands, and what to do next:
- **Where each leg stands,** asked every ten seconds: this chain's from its contract, an EVM
  chain's from Swap.sol through its JSON-RPC. "Locked as the terms say", or "locked, but not as
  the terms say" and why. Once a leg is claimed, the secret, which it keeps.
- **The second lock waits for the first:** its button stays off until the first lock stands as
  the terms say, wherever the wallet can see it.
- **The starter claims only a lock that stands:** a second lock short of the terms is flagged,
  as claiming it would show the secret for less; so are an HTLC's coins below its amount.
- **This chain's locks, claims and refunds** are calls the Send page reviews and signs, on the
  offline signer too; for a token, **Let the contract move the token** first.
- **An EVM chain's** are priced through its JSON-RPC in one batch (nonce, gas price, tip, an
  estimate), shown with the most they may cost, signed with the account's Ethereum key after
  biometrics when they are on, sent, and waited for until mined. For an account watched from
  the offline signer, the wallet shows `ur:eth-sign-request` and reads back its
  `ur:eth-signature`, as MetaMask asks a hardware wallet.
- **Bitcoin's HTLC:** its address and amount, as a QR code to pay it from a Bitcoin wallet;
  claiming and refunding it, with the coins it holds (txid:vout and amount), where they go and
  the fee. It is signed with the account's key, or, for an account watched from the offline
  signer, as a PSBT the signer signs, showing that it claims or refunds a swap's coins.

**Settings, Swaps:**
- **the swap contract here,** when not the node's;
- **an Esplora API** (such as `https://mempool.space/api` or
  `https://mempool.space/testnet4/api`), to look the HTLC up, find a claim's secret and
  broadcast there, which tells that server your swap's addresses;
- **EVM chains' JSON-RPC,** one URL a line. Each is asked its chain id, and serves that chain's
  legs; with none, EVM legs cannot be followed.

**Following Bitcoin**, the wallet has three ways, best first:
- **its own light client,** on the desktop and Android apps, when Settings, **Bitcoin** follows the
  swap's network. It asks Bitcoin's peers for compact block filters, so no server learns the
  HTLC's address. It finds the coins paying the HTLC, with their confirmations, and fills them in
  to claim or refund. It reads the secret from a claim, and broadcasts the signed spend to its
  peers. With it, a first lock on Bitcoin counts as standing only once the HTLC is paid in full,
  in a block.
- **an Esplora API,** as above;
- **pasting,** a funding outpoint or a claim's transaction, and broadcasting the signed spend
  yourself. The browser has this and Esplora alone: it opens no raw connections.

Both sides must follow the same test network, testnet4 or signet, for a testnet swap.

## NFTs

Either leg on chainv4 or an EVM chain may lock an [NFT](nft-standard.md), or an amount of a
multi-token id (ERC-721 and ERC-1155 on the EVM), rather than coins or a fungible token: the leg's
`item` says which id, its token is the collection, and its amount how many, 1 for an NFT.
- **Locking:** the CLI and the wallet let the contract move it first, `approve` for an NFT or
  `set_approval_for_all` for a multi-token collection, then call `lock_nft` or `lock_multi`
  (`lockNft` and `lockMulti` on Swap.sol).
- **Checking a lock:** `Leg::stands` holds the lock to the terms' NFT, as `item` says it; the CLI
  and the apps ask `item` beside `get` for legs that lock one.
- **Claiming and refunding** are as for coins: the secret, or the deadline, sends the NFT on.
- **Older contracts:** a swap contract or Swap.sol from before NFTs has no `item`, so a leg locking
  an NFT never stands there: offers take the current default contracts.
- **Swap.sol's address changed with it:** CREATE2 puts the new code at a new address, which new
  offers default to; terms naming the old one still go to it.

## From a multisig

A [native multisig](multisig.md) swaps as a party on chainv4, Bitcoin and EVM chains, its quorum
controlling what it locks or receives throughout.
- **On Bitcoin** its branch of the HTLC, refund when it pays and claim when it receives, checks M of
  its members' keys with `OP_CHECKMULTISIG` instead of one key's `OP_CHECKSIG`, each branch then
  ending in its own check. An HTLC of two keys keeps its template byte for byte. The keys are the
  members' at one of the multisig's receiving addresses, the index `swap::multisig_index` takes
  from the swap's hash, so every member's wallet finds the same fresh one with nothing to pass
  round; the terms list them, sorted, with the threshold, so the other side checks the script.
- **Terms with a multisig** on Bitcoin are written with a 4 first; every other is written as
  before, a 3 or a 2, so older apps read all they could.
- **On chainv4** the multisig is the leg's address: its lock is its own transaction, proposed as a
  bundle for its members. A token's lock is two, the approval and then the lock. Claims and
  refunds anyone may send, so they need none of its members.
- **On an EVM chain** the multisig's party is its contract there, at the same address on every
  chain, so the terms are written as for any address. Its lock is the contract's calls, the
  approval of a token and the lock, bundles at its next nonces for its owners to sign
  (`chainv4 multisig evm … sign`, then `submit`). What it receives, Swap.sol pays to the
  contract; claims and refunds anyone sends.
- **Its Bitcoin claim or refund** is a PSBT its members sign, with chainv4 multisig, the offline
  signer or the wallet; the first threshold signatures in the script's key order finish it.

```sh
chainv4 swap offer --multisig multisig:0x… --pay chainv4:5000 --get bitcoin-testnet:100000 --hash 0x…
chainv4 swap lock --multisig multisig:0x… --key member.key --out lock.bundle 'UR:CHAINV4-SWAP/…'
chainv4 swap bitcoin-claim 'UR:…' --multisig multisig:0x… --psbt claim.psbt --funding txid:vout --sats 100000 --to tb1q… --secret 0x…
chainv4 multisig bitcoin multisig:0x… sign claim.psbt     # each member
chainv4 swap bitcoin-finish 'UR:…' --psbt claim.psbt --sats 100000 --secret 0x…
chainv4 swap lock --multisig multisig:0x… --key member-evm.key --eth-rpc … --out lock.evm 'UR:…'   # an EVM leg
chainv4 multisig --key other-evm.key evm multisig:0x… sign lock.evm
chainv4 multisig --key other-evm.key evm multisig:0x… submit lock.evm --eth-rpc …
```

In the wallet, **Swap as** picks a followed multisig when offering or answering. Its lock goes to
the Multisig page as a proposal, on an EVM leg as its contract's calls, each a bundle in turn; its Bitcoin claim or refund collects the members' signatures,
these words', the signer's from its code, or a member's pasted, and finishes once the branch
holds enough. The offline signer signs a multisig's branch as any multisig input, once the HTLC's
branch holds its key.

## In the explorer

The **Swaps** page lists the swap contract's swaps from its events, newest first, each lock with
whether it is final yet, and finds one by its hash, or by its code, which lays out both legs. See
[Explorer](explorer.md).

## Waiting for finality

A lock in a chain's state is not yet a lock for good: a reorganization can take its block away.
Two moments must not rest on one that can still go:
- **the second lock** goes in only once the first is final. Otherwise the starter's lock could
  vanish after the second side paid;
- **the starter's claim** waits until the second lock is final. The claim shows the secret, and
  if the second lock were then reorged away, the other side would hold the secret and the
  starter's coins both.

When a lock counts as final (`swap::Settled`):

| Chain | Final when |
| --- | --- |
| chainv4 | its block is finalized, on a chain with finality: as the wallet's light client proves it, or `GET /status` says to the CLI. On a chain without, 20 blocks deep (`CHAIN_DEPTH`). |
| An EVM chain | at or below the block its RPC tags `finalized`. Where it names none, as on chainv4's own EVM without finality, 12 blocks deep (`EVM_DEPTH`). |
| Bitcoin | paid in full with 3 confirmations (`BITCOIN_DEPTH`; Settings, Swaps, **Bitcoin depth**), as the wallet's light client counts them. Without it, depth is yours to check. |

The swap contracts say which block a swap was locked in, `get`'s last value, to reckon from.

**Holding, with an override:**
- `chainv4 swap lock` and `claim` refuse until the lock is final, saying what is left, and act
  anyway with `--anyway`.
- The Swaps page shows each leg's finality, keeps the buttons off until final, and offers
  **I checked it myself** after a warning.
- `chainv4 swap status` shows each lock's finality.
- A swap contract deployed before contracts said their lock's block (the first `swap.lua`) leaves
  finality unknown, and so held until overridden: deploy the current one, and point the name
  `swap` at it.

## What to keep in mind

- **Lock second only once the first lock is final,** with the amount, recipient and deadline the
  terms say. The CLI and the wallet check a contract's lock, and its finality, for you; on
  Bitcoin, the wallet's light client counts confirmations, or check the HTLC on an explorer.
- **The starter must claim before the second deadline,** and the other side before the first:
  a claim that misses its deadline cannot be mined, and the lock goes back.
- **A Bitcoin leg pays what it says, not more:** the HTLC is spent whole, to one output, less the
  fee. Pay it exactly, in one output.
- **An EVM leg needs that chain's coin for gas,** on both sides: the claim is a transaction there.
- **Testnet keys are other keys,** so a testnet Bitcoin leg needs an account of the wallet's
  words; accounts watched from the signer swap on mainnet, by their paired bitcoin key.
- **Not yet finality-aware:** a lock in the state may still be reorged away. See the todo.

## Tested

- **The HTLC runs on Bitcoin Core itself,** on a private regtest chain
  (`cargo test --lib -- --ignored bitcoin_core`, with `bitcoind` on the path):
  - a claim with the secret is mined;
  - a refund before its lock time is refused as not final;
  - a claim with the wrong secret fails the script;
  - a refund after its lock time is taken.
- **Swap.sol runs on chainv4's EVM,** through whole blocks, with `contracts/evm/Token.sol` for an
  ERC-20:
  - the token is locked once approved, and claimed with the secret;
  - the coin is refused a refund before its deadline and a claim at it, and is refunded after;
  - the wrong secret, a second lock under one hash, a deadline past and nothing locked are all
    refused.
- **The calls are Swap.sol's:** selectors as solc names them, and CREATE2's address checked
  against EIP-1014's example.
- **The CLI swaps end to end against a running node:**
  - chainv4 against Bitcoin;
  - chainv4 against the node's own EVM, through its JSON-RPC: Swap.sol deployed with
    `evm-deploy`, the second lock refused until the first stands, both claimed.
- **The wallet's pieces:**
  - the EVM pricing from a batch's answers;
  - the offline signer's signature checked against its request and address;
  - its signature over a Bitcoin swap spend matching the swap tools' own, byte for byte.
- **Terms written before legs** read as the same swap.
