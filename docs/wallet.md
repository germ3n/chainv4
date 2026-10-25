# Wallet

The wallet (`wallet/`) is a light wallet, built with egui, for the desktop and the browser:
- **Keys:** several accounts, sealed with a password on the device.
- **Balances:** the coin and every listed token, proven by the light client.
- **Sending:** the coin or a token, with the fee shown before you sign.
- **Receiving:** the address, with a QR code.
- **Activity:** every transaction an account sent or that touched it, proven.

It trusts a genesis id and nothing else. A node tells it where to look, and it shows only what it
proved, checking each answer against headers it checked itself. A node can withhold things from it,
but it cannot make the wallet show something false.

## Running it

**On the desktop:**

```sh
cd wallet
cargo run --release -- --node http://127.0.0.1:8080
```

Where it looks for the node:
- `--node`;
- else `CHAINV4_NODE`;
- else the node it talked to last;
- else `http://127.0.0.1:8080`.

The node can be changed under Settings.

**In the browser,** with [trunk](https://trunkrs.dev):

```sh
cd wallet && trunk build --release
chainv4 --http 127.0.0.1:8080 --index-addresses --wallet wallet/dist
```

Then open `http://127.0.0.1:8080/wallet/`. The node serves the app from its API port, so the page
talks to the node that served it. `?node=http://host:port` in the page's address names another,
which must allow the page's origin with `--cors`.

**The node it wants:** any node will do for balances and sending. The activity comes from the
node's address index, so start the node with `--index-addresses`. Without one, it falls back to
the event index (`--index-events`), which lists only transactions whose events name the account.

## Keys

**The vault.** On first start the wallet asks for a password, at least 8 characters. Its accounts
are kept in app storage, sealed with it (`wallet/src/vault.rs`):
- **Stretching:** Argon2id stretches the password into a key, with a random salt of the vault's
  own.
- **Sealing:** ChaCha20-Poly1305 seals the accounts with that key, under a fresh nonce each time
  they are saved.
- **What storage shows:** without the password, the salt, the nonce, and how long the sealed
  accounts are.

**Where it is kept:** the app's storage directory on the desktop, and the page origin's
`localStorage` in the browser. Clearing the site's data deletes the keys, so export each one
somewhere safe.

**Unlocking:** the password opens the vault. While unlocked, the wallet keeps the stretched key in
memory, so every change seals the accounts again without asking. Locking drops the keys and that
key alike.

**Locking:** the Lock button, or 15 minutes without input. Settings changes the time, or turns it
off.

**Accounts** (`wallet/src/views/accounts.rs`):
- **New account:** a fresh Ed25519 key from the system's randomness.
- **Open key file / paste a key:** a key file as `chainv4 keygen` writes it, `ed25519:0x…` or
  `edblake:0x…`.
- **Development key:** the dev chains' well-known key, which holds their genesis coin. Anyone can
  spend from it.
- **Use, rename, export and remove.** Export shows the key's text to copy. Removing an account
  with no copy of its key puts what it holds out of reach for good.

**Changing the password:** the current one, then the new one twice. The accounts are sealed again
under a new salt.

## Trust

**Pinned on first use.** The first time the wallet reaches a node, it trusts the genesis that node
shows, and remembers it for that node. From then on, a node on any other chain stops the light
client with an error that names both genesis ids.

**Trusting another:** Settings takes a genesis id, as someone you trust shows it, in place of the
pinned one. "Forget it" trusts whatever the node shows next, and starts over.

**What the node is trusted with:** nothing proven. It does say:
- which tokens exist, from the registry's first page (`GET /tokens`). Each listed token's entry,
  and each balance of it, is then proven;
- which transactions touched an account, from its address index. Each is then proven;
- the coin's symbol and decimals, from `GET /status`, which only change how amounts read;
- the fee offer, which the review shows before you sign.

## What it proves

The [light client](light-client.md) follows the node from the pinned genesis. Every header is
checked by consensus, and finality by the voters it proved (`wallet/src/chain.rs`).

**Balances:** every few seconds, for every account, at the tip and at the finalized block:
- **The coin:** the account's entry in `Column::Accounts`, with its nonce.
- **Tokens:** each listed token's registry entry in `Column::Tokens`, for its name, symbol and
  decimals. Then each account's balance of it, by the token standard's
  [provable balances](token-standard.md#provable-balances).

**How far each is trusted** shows beside it:
- **Final:** proven at a final block. Nothing can undo it.
- **Proven:** proven at the tip, which a reorg could still undo.

**Activity:** for the account in use, the node's address index lists the transactions that
touched it (`GET /accounts/{address}/txs`). The wallet proves each one with
`GET /proofs/receipts/{id}`:
- **Who sent what:** the signed transaction, against its block's `tx_root`, with `check_tx`.
- **What it did:** its receipt, against the `receipts_root`, with `check_receipt`.

From those it works out what moved for the account (`wallet/src/history.rs`):
- the coin, from a transfer, a mint, a burn or a call's value;
- tokens, from the standard's `Transfer` events.

Each line says Received, Sent, or what the transaction did. The line opens to show the block, the
sender, the fee, and every event it emitted.

**Kept once proven.** What was proven is saved, so it shows again at once. The saved light client
keeps headers only from the finalized block on, and could not prove older blocks again. What is
not final yet is proven again until it is.

## Sending

The Send page (`wallet/src/views/send.rs`):
1. **What, to whom, how much:** the coin, or a token the account holds. The amount is in the
   token's decimals, and checked against the proven balance.
2. **Review.** The wallet builds the transaction from what it proved:
   - **The nonce:** the proven one, past any it sent that is still waiting.
   - **The price:** it signs a probe with no fee offer and asks the node to price it, as
     `chainv4 send` does. An optional tip per gas, in the coin, goes along.
   - **What the review shows:** the most it can cost, gas limit × max price. It refuses to go on
     if the balance does not cover that and the amount.
3. **Sign and send.** It signs with the offer and submits it.
4. **Tracking:** the transaction goes into Activity as Pending, then Proven, then Final once the
   light client proves its block final.

A token transfer is the standard's `transfer(to, amount)`, sending no coin.

**Dropped transactions:** one no block holds after 10 minutes shows as Not seen, and stops
holding its nonce, so the next transaction takes that nonce again.

## Receiving

The Receive page shows the account's address, to copy, and as a QR code, dark on white in either
theme so any camera reads it. The coin and every token arrive at the same address.

## What it remembers

Everything is in app storage under one key, `chainv4-wallet`, saved by eframe every few seconds
and on exit:
- the node, and the genesis pinned at each node;
- the vault;
- the light client's state, by genesis, to resume from the finalized block, not genesis;
- what it sent that is not final yet, and what it proved, the newest 2,000;
- the account in use, the lock time, and the theme.

Only the vault is secret.

## Look

It follows the look of the wiki every build writes (see [Documenting code](development.md#documenting-code)), quieter:
- **Colors:** the wiki's palette, dark or light.
- **Type:** Manrope for text, JetBrains Mono for addresses and hashes, and Newsreader for titles
  and the big numbers. The fonts and their licences are in `wallet/assets/fonts`.
- **Layout:** each page is a column of cards beside a sidebar with the account, the pages, and
  the light client's state.

## Building and testing

The wallet is a workspace of its own, like the explorer: egui and the browser crates never meet
the node's lockfile, and it links only the chainv4 library's hash and key features, which build
for `wasm32-unknown-unknown`.

```sh
cd wallet
cargo test
cargo clippy --all-targets -- -D warnings
cargo clippy --target wasm32-unknown-unknown -- -D warnings
trunk build --release
```

## What to keep in mind

- **Lose the password, lose the keys.** Nothing can open the vault without it. Export each
  account's key and keep it safe.
- **Browser storage is per origin.** The wallet served from one node's `/wallet/` and the same
  wallet from another address keep separate vaults.
- **A node can withhold, not lie.** It can leave tokens off its list, or transactions out of its
  index. It cannot make a balance or a transaction show that does not check.
- **Coin a contract pays out shows in the balance, not the activity.** Only the trace records such
  payments, and the receipts root leaves the trace out, so the wallet cannot prove them line by
  line. The guestbook's `Tipped` event, for one, does show.
- **Only the registry's first page of tokens,** 100, is proven. A token past it does not show.
- **Unlocking takes a moment.** Argon2id is slow on purpose, and slower in the browser.
- **The API is plain HTTP.** What the wallet sends is signed and public anyway, but keep the node
  on a trusted network all the same.
