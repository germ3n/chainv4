# Atomic swaps

Coins or a token on chainv4 for bitcoin on Bitcoin, between two people, with no one in between
and no way for either to take both. Each side locks what it pays under the same hash: on chainv4
in the swap contract, on Bitcoin in a hash time-locked contract (an HTLC). Whoever claims one
lock shows the secret behind the hash on that chain, and the other side claims the other lock
with it. A lock nobody claims in time goes back to whoever made it.

Behind the default-on `swap` cargo feature: `src/swap/` (the terms and the HTLC),
`contracts/lua/swap.lua` (chainv4's side), `chainv4 swap` (the CLI) and the wallet's Swaps page.

## How a swap goes

1. **The starter** makes a secret, 32 random bytes, and its SHA-256, the hash. They write an
   **offer**: what each side pays, the hash, the deadlines and their own keys.
2. **The other side** reads the offer, checks it, and shows back an **answer**: the same terms
   with their keys added.
3. **The starter locks first**, with the longer deadline (24 hours by default), on the chain
   they pay on.
4. **The other side locks second**, with the shorter deadline (12 hours), once the first lock
   stands, confirmed, with the right amount, recipient and deadline.
5. **The starter claims the second lock** with the secret, before its deadline. That shows the
   secret on that chain.
6. **The other side claims the first lock** with the same secret, before its longer deadline.

Either the whole swap happens or neither side loses anything:
- if the other side never locks, the starter takes their lock back after its deadline;
- if the starter never claims, both take their locks back, the second side first;
- once the starter claims, the secret is public, and the other side has until the first
  deadline, at least four hours more, to claim in turn.

**The secret is the starter's alone until they claim.** Whoever knows it early can claim the
starter's lock without paying. The wallet keeps it sealed in the vault; the CLI prints it once.

## The terms

`swap::Terms`, written as a code, `ur:chainv4-swap`, in capitals, as a QR code or copyable text:

| Field | What it is |
| --- | --- |
| `chain` | The other chain: `Bitcoin`, or `BitcoinTestnet` for testnet, signet and regtest. Another chain is another tag. |
| `contract` | The swap contract on chainv4 that the lock goes into, and both sides look in. |
| `hash` | The SHA-256 of the secret. |
| `sats` | The bitcoin locked, in satoshis. |
| `token`, `amount` | What is locked on chainv4: the coin, or a token's contract, and how much in its smallest units. |
| `starter` | What the starter pays: `Bitcoin` or `Chain`. |
| `bitcoin_payer`, `bitcoin_receiver` | The HTLC's refund and claim keys: 33-byte compressed bitcoin keys. |
| `chain_payer`, `chain_receiver` | Who locks on chainv4, and who claims there. |
| `first_deadline`, `second_deadline` | When the starter's lock and the other's open to a refund: Unix milliseconds. |

An offer leaves out the other side's two keys; an answer has all four. `Terms::check` refuses
terms with less than 294 satoshis (Bitcoin's dust limit) or nothing on chainv4, deadlines past,
a second deadline less than four hours (`MIN_GAP_MS`) before the first, keys that are not keys,
and the same key or address on both sides. An answer must keep every term of the offer: the
wallet refuses one that changes anything but the other side's keys.

## Deadlines

- **Times, on both chains:** chainv4's lock ends at a block time, `ctx.time`; Bitcoin's lock time
  is the same deadline in Unix seconds, which `OP_CHECKLOCKTIMEVERIFY` checks against the median
  time of the last eleven blocks. Nothing needs either chain's height.
- **Bitcoin's median time runs about an hour behind the clock,** so a Bitcoin refund confirms an
  hour or so after its deadline, not at once.
- **A block's time may be a little off the clock** on either chain. Leave hours of margin, never
  minutes: the defaults are 24 and 12 hours.
- **Claim early.** A claim sent just before a deadline may be mined after it, when the lock is
  refundable instead.

## chainv4's side: the swap contract

`contracts/lua/swap.lua`, or any contract with the same functions: nothing in the chain treats
one as special, and any contract can call one. **The terms name the contract a swap uses,** so
both sides lock and look in the same one, whatever their wallets would pick. A new offer takes:
1. the one given: the CLI's `--contract`, or the wallet's Settings, **Swaps**;
2. else the one the name `swap` under the chain's suffix resolves to, such as `swap.dev`, which
   whoever runs a swap contract registers, proven by the wallet's light client as any name;
3. else the one the node advertises, started with `--swap-contract` (in Docker, `SWAP_CONTRACT`
   in `.env`), which `GET /status` shows as `swap.contract`.

| Function | What it does |
| --- | --- |
| `lock(hash, recipient, deadline)` | Locks the coin the call sends, for `recipient` to claim with the secret before `deadline`, Unix milliseconds. |
| `lock_token(hash, recipient, deadline, token, amount)` | Locks `amount` of a token, moved with `transfer_from`: approve the swap contract for it first. Sends no coin. |
| `claim(secret)` | Pays the swap whose hash is the secret's SHA-256 to its recipient, before its deadline. Anyone may send it. |
| `refund(hash)` | Pays a swap nobody claimed back to its locker, from its deadline on. Anyone may send it. |
| `get(hash)` | Its state, `none`, `locked`, `claimed` or `refunded`; locker, recipient, token (the zero address for the coin), amount, deadline, and the secret once claimed. |

Hashes and secrets are `0x` and 64 hex digits. A secret must be 32 bytes, as Bitcoin's script
wants it, so a secret one chain takes the other takes too. Events: `Locked(hash, locker,
recipient, token, amount, deadline)`, `Claimed(hash, secret)`, `Refunded(hash)`.

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

```sh
# Alice, who pays bitcoin and starts:
chainv4 swap secret                                # keep the secret; share the hash
chainv4 swap offer --pays bitcoin --testnet --hash 0x… --sats 100000 --amount 5000000000000000000 \
  --bitcoin-key alice-btc.key --address ed25519:0x…   # prints the offer's code
# Bob:
chainv4 swap answer UR:CHAINV4-SWAP/… --bitcoin-key bob-btc.key --address ed25519:0x…
chainv4 swap show UR:CHAINV4-SWAP/…                 # the HTLC's address and script
# Alice pays the HTLC from her Bitcoin wallet; Bob checks it confirmed, then locks:
chainv4 swap lock UR:CHAINV4-SWAP/… --key bob.key --node https://…   # into the terms' contract
# Alice claims on chainv4, showing the secret:
chainv4 swap claim UR:CHAINV4-SWAP/… --secret 0x… --key alice.key --node https://…
chainv4 swap status UR:CHAINV4-SWAP/… --node https://…   # claimed, with the secret
# Bob claims the bitcoin with it, and broadcasts what it prints:
chainv4 swap bitcoin-claim UR:CHAINV4-SWAP/… --secret 0x… --funding <txid>:<vout> --sats 100000 \
  --key bob-btc.key --to tb1q… --fee-rate 2
```

`swap refund` and `swap bitcoin-refund` take a lock back after its deadline, and `swap revealed
--tx <hex>` reads the secret from a Bitcoin claim's transaction, for when the starter pays on
chainv4 and claims the bitcoin first. `swap offer --contract` names a swap contract other than the default.

## In the wallet

The **Swaps** page, on an online install: **Start a swap** makes the offer and its secret, kept
sealed with the rest of the vault; **Read an offer or an answer** reads the other side's code. An
open swap lays out each side and what to do next:
- the HTLC's address and amount, as a QR code to pay it from a Bitcoin wallet;
- **Lock**, and for a token **Let the contract move the token** first, as calls the Send page
  reviews and signs, on the offline signer too;
- where chainv4's side stands, asked of the contract every ten seconds; once it is claimed, the
  secret, which it keeps;
- claiming and refunding the HTLC: the coins it holds (txid:vout and amount), where they go and
  the fee; signed with the account's key, or for an account watched from the offline signer, as a
  PSBT the signer signs, showing that it claims or refunds a swap's coins.

The wallet does not follow Bitcoin: by default you paste what it needs, a funding outpoint or a
claim's transaction, and broadcast the signed spend yourself. With an **Esplora** API set in
Settings, **Swaps** (such as `https://mempool.space/api` or `https://mempool.space/testnet4/api`),
it looks the HTLC up, finds a claim's secret and broadcasts there, which tells that server your
swap's addresses. A Bitcoin light client, for the desktop and Android apps, is in the todo.

## What to keep in mind

- **Lock second only once the first lock is confirmed,** with the amount, recipient and deadline
  the terms say. On chainv4 the Swaps page reads it from the contract; on Bitcoin check the HTLC's
  address on an explorer, or with Esplora.
- **The starter must claim before the second deadline,** and the other side before the first:
  a claim that misses its deadline cannot be mined, and the lock goes back.
- **A swap pays what it says, not more:** the HTLC is spent whole, to one output, less the fee.
  Pay it exactly, in one output.
- **Testnet keys are other keys,** so a testnet swap needs an account of the wallet's words;
  accounts watched from the signer swap on mainnet, by their paired bitcoin key.

## Tested

The HTLC runs on Bitcoin Core itself, on a private regtest chain
(`cargo test --lib -- --ignored bitcoin_core`, with `bitcoind` on the path): a claim with the
secret is mined, a refund before its lock time is refused as not final, a claim with the wrong
secret fails the script, and a refund after its lock time is taken. The contract is tested with
the coin and a token, its deadlines and refusals; the CLI against a running node, the contract
found from `GET /status`; and the offline signer's signature over a swap's spend matches the
swap tools' own, byte for byte.
