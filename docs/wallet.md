# Wallet

The wallet (`wallet/`) is a light wallet, built with egui, for the desktop, the browser and
Android:
- **Keys:** wallets of accounts from a seed phrase, each sealed with a password on the device, or
  none, and opened with a fingerprint, a face, Touch ID or a passkey when biometrics are on.
  Every account has an Ed25519, an EdBlake and an Ethereum address, all from the same words.
- **Balances:** the coin and every listed token at each address, proven by the light client.
- **Sending:** the coin or a token, from the address picked, with the fee shown before you sign.
- **Receiving:** any of the account's addresses, with a QR code.
- **Activity:** every transaction an account sent or that touched one of its addresses, proven.

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

**On Android,** with [xbuild](https://github.com/rust-mobile/xbuild): see [Android](#android)
below.

```sh
wallet/build-android.sh --install         # an APK for arm64, put on the device adb sees
```

**The node it wants:** any node will do for balances and sending. The activity comes from the
node's address index, so start the node with `--index-addresses`. Without one, it falls back to
the event index (`--index-events`), which lists only transactions whose events name the account.

## Through a proxy

On the desktop and on Android, **Settings → Proxy** sends every request the wallet makes through
a proxy, and only through it:
- **SOCKS5:** `socks5h://host:port`. `socks5://` is read as `socks5h://`: the proxy looks up the
  node's name too, so no lookup leaves the device, and a `.onion` node is reached through Tor.
  `user:password@` may come before the host; messages show the host and port only.
- **HTTP:** `http://host:port`. Every request goes through a CONNECT tunnel, https or not.
- **Orbot** fills in `socks5h://127.0.0.1:9050`: Orbot's SOCKS port on a phone, and Tor's own on a
  desktop. Start Orbot, or Tor, first. Tor Browser's Tor listens on 9150 instead.
- **I2P** fills in `socks5h://127.0.0.1:4447`, i2pd's SOCKS port, which reaches a node at a `.i2p`
  address. The Java I2P router has an HTTP proxy instead: type `http://127.0.0.1:4444`. A node
  with an ordinary address is reached only through an I2P outproxy, slowly, if at all.
- **Off** goes straight to the node again.

**It fails closed.** While a proxy is set, nothing goes out any other way. With the proxy stopped,
every request fails with `through the proxy at 127.0.0.1:9050: …` rather than going direct. A kept
setting that no longer reads as a proxy sends nothing either.

Changing it starts afresh with the node, as **Connect** does; the light client resumes from what
it saved. The lock screen names the proxy beside the node: `Node http://… · through
127.0.0.1:9050`, and **Change the node or proxy** under it sets either before the wallet opens,
as Settings does, so a wallet whose node is gone, or that needs Tor to reach it, is never stuck
at the lock screen.

**In the browser** there is no setting: the browser makes every request, through the proxy set
in it. To reach a node over Tor, open the wallet in Tor Browser.

**A node over Tor:** give the node an onion service in its machine's `torrc`, pointing at the
API's port:

```
HiddenServiceDir /var/lib/tor/chainv4/
HiddenServicePort 80 127.0.0.1:8080
```

Tor writes the address to `/var/lib/tor/chainv4/hostname`. In the wallet, set the proxy to Orbot
and the node to `http://<that address>.onion`.

## Wallets and words

**A wallet** is a seed phrase, the accounts made from it, and a password that seals them. The lock
screen (`wallet/src/lock.rs`) makes the first, and holds them all:
- **New wallet:** a name, a password twice, or none, and under Advanced an optional BIP39
  passphrase. The
  wallet makes 12 words and shows them once, to write down, then asks for three of them back
  before it opens.
- **Restore from words:** 12 or 24 words written down before, from this wallet or any that
  follows BIP39, with their passphrase if they had one.
- **Several wallets:** "Add a wallet" makes another. With more than one, the lock screen asks
  which to unlock; each has its own password. The one last opened comes first.

**An account, four addresses.** Account *n* of the words has a key in every scheme, derived as
[Crypto](crypto.md#seed-phrases) says: Ed25519 and EdBlake by SLIP-0010, secp256k1 at
Ethereum's `m/44'/60'/0'/0/n`, and bitcoin at BIP84's `m/84'/0'/0'/0/n`. So its Ethereum
address is the one MetaMask shows for the same words and account, and its Bitcoin `bc1q…` the
*n*th receiving address Bitcoin's wallets show. `chainv4 keygen --from-phrase --account n` gives
the same keys. A wallet from before the bitcoin scheme gains the Bitcoin address on its next
unlock, from the same words; an account watched from an offline signer gains it when paired
again.

**More addresses per account.** Accounts, **Another address**, then a scheme, gives an account
from the words a further key in it (`Seed::key_at`):
- **Ed25519 and EdBlake:** below the account, its *k*th key at `m/44'/7744'/n'/0'/k'` and
  `m/44'/7744'/n'/1'/k'`, counting from 1.
- **Ethereum and Bitcoin:** the lowest index on the shared path, `m/44'/60'/0'/0/i` or
  `m/84'/0'/0'/0/i`, that no account's number and no further key takes, so MetaMask and Bitcoin's
  wallets, paired with the same words, list it too; a new account then skips it.
- **Bitcoin's habit of a new address for each payer:** on Receive, with a Bitcoin address shown,
  **New address** adds the next one and shows it.

The vault keeps which keys an account has, so the words bring all of them back; Send pays from any
of them, and Receive numbers a scheme's: Bitcoin, Bitcoin 2. Only accounts from the words have
them: a key brought in has no words to derive more from, and an account watched from the signer
gains them when paired again, though sites reach its first Ethereum key alone.

**Settings** shows the words, once the wallet's password is typed again, renames the wallet, and
removes it, also after the password. A wallet from before seed phrases has none: Settings makes
new words for it or takes words you have, and from then on new accounts come from them.

## Keys

**The vault.** Each wallet's words and accounts are kept in app storage, sealed with its password,
at least 8 characters, or none (`wallet/src/vault.rs`):
- **Stretching:** Argon2id stretches the password into a key, with a random salt of the vault's
  own.
- **Sealing:** ChaCha20-Poly1305 seals the accounts with that key, under a fresh nonce each time
  they are saved.
- **What storage shows:** without the password, the salt, the nonce, and how long the sealed
  accounts are.

**Where it is kept:** the app's storage directory on the desktop, the page origin's
`localStorage` in the browser, and a file in the app's private data folder on Android. Clearing
the site's data, or the app's, deletes the keys, so keep the words written down.

**Unlocking:** the password opens the vault. While unlocked, the wallet keeps the stretched key in
memory, so every change seals the accounts again without asking. Locking drops the keys and that
key alike.

**No password.** Leave both fields empty and the vault is sealed with an empty one: the lock screen
offers **Open** instead of a field, and says that anyone who can read the app's storage, a phone's
backup or malware, can open it too. Fine for small sums, or with biometrics on; Settings sets a
password later, and also takes one away.

**Wiped from memory.** The stretched key, the accounts as they open and seal, the words, and what is
typed into a password, words or key field are overwritten as they drop (`zeroize`), and so is every
copy of a key biometrics keep, Java's on Android included. So are the copies made along the way:
- **the phrase, as read:** BIP39's own (its `zeroize` feature) and the lowercase words
  `Phrase::parse` makes;
- **a field as it is typed into:** it has room for 1,024 characters from the start, so typing
  never moves it and leaves the old text behind, and egui's undo history of it is dropped each
  frame;
- **the vault as it seals:** its JSON is written into room made to its size, never grown;
- **a swap's secret,** kept in the vault, wiped as its record drops.

What egui lays out to draw words that are shown, what the browser or the OS hold of what was typed,
such as a keyboard's own buffer, and memory the system swaps to disk, are out of reach.

**Locking:** the Lock button, or 15 minutes without input. Settings changes the time, or turns it
off.

**Accounts** (`wallet/src/views/accounts.rs`, `wallet/src/keys.rs`):
- **New account:** the words' next account, the lowest number the wallet does not hold, with its
  four addresses. A wallet without words makes one random key instead, in the scheme picked.
- **Open key file / paste a key:** a key file as `chainv4 keygen` writes it, `ed25519:0x…`,
  `edblake:0x…`, `secp256k1:0x…` or `bitcoin:0x…`, picked or pasted. An account brought in has that one key, and
  one address, named after the file when no name is typed.
- **Development key:** the dev chains' well-known key, which holds their genesis coin. Anyone can
  spend from it.
- **Use, rename, export and remove.** Export asks first, then shows each key's text to copy;
  leaving the page hides them again. Removing an account from the words only takes it off the
  list: the next New account brings it back. Removing a key brought in, with no copy of it, puts
  what it holds out of reach for good.

**From before seed phrases:** a vault the wallet sealed before it had words opens as a wallet
named Wallet 1, its accounts keys brought in.

**Changing the password:** the current one, then the new one twice, or nothing to have none. The
accounts are sealed again under a new salt. Biometrics kept the key the old password stretched to,
so they turn off, and are turned on again under the new one.

## Biometrics

A wallet can be opened with a fingerprint or face on Android, Touch ID on macOS, or a passkey in
the browser, instead of its password; and once they are on, every send it signs, and every
request an offline signer signs, waits for one more check. Settings turns them on and off for the
wallet open (`wallet/src/biometrics.rs`; the platforms are in `ui/src/biometric.rs`).

**What they keep:** the key the password stretches to, the one that opens the vault, wrapped so
only a check gives it back:

| Where | What wraps it | What a check is |
| --- | --- | --- |
| Android 9 and up | An AES key in the Android Keystore, usable only right after a strong biometric check, which a fingerprint or face enrolled later invalidates for good. | BiometricPrompt, through `ui/android/Biometric.java`. |
| The browser | The PRF extension's output of a passkey made for the wallet, which only the passkey's user verification releases. | WebAuthn: Touch ID, Windows Hello, a phone or a security key, whichever holds the passkey. |
| macOS | Touch ID's check comes first, then the key is read from the login Keychain. The fingers enrolled are remembered, and once they change, the key is forgotten. | LocalAuthentication. |

Linux and Windows desktops keep the password; Windows Hello is not used yet.

**When the fingers or faces change,** biometrics turn off for a wallet the next time they are
used for it, the lock screen says so, and the password opens the wallet; turning them on again
keeps the key anew.
A check that fails that way during a send locks the wallet.

**The lock screen** shows the prompt by itself when the app starts on Android and macOS. After
**Lock** or the timeout it waits for **Unlock with …**, as whoever locked it may be gone; in the
browser, which shows a passkey's prompt only right after a tap, it always waits.

**Things to know:**
- **macOS is weaker than the others.** The Keychain item is guarded by the login, not by Touch ID,
  as an unsigned app cannot ask the Keychain to tie an item to biometry: Touch ID is a check the
  wallet makes, not a lock on the key.
- **A passkey stays with the browser or device** when biometrics are turned off: delete it there.
  Without the wrapped key the wallet kept, it opens nothing.
- **A browser or passkey without PRF** cannot keep the key; recent Chrome, Edge and Safari can,
  with the device's own passkeys. The page must be served over HTTPS, or from localhost.
- **Android builds without Java have no biometrics**, as they have no file picker: see below.

## Trust

**Pinned on first use.** The first time the wallet reaches a node, it trusts the genesis that node
shows, and remembers it for that node. From then on, a node on any other chain stops the light
client with an error that names both genesis ids.

**Trusting another:** Settings takes a genesis id, as someone you trust shows it, in place of the
pinned one. "Forget it" trusts whatever the node shows next, and starts over.

**Where the light client starts:**
- **Saved:** where it was, as always.
- **A checkpoint pasted** under Settings → Trusted chain → Start from a checkpoint: that finalized
  checkpoint, trusted as the genesis is, rather than genesis. Quick on a long chain. "Forget it"
  follows from genesis again.
- **A node met for the first time,** with no genesis given: its own finalized checkpoint, as the
  wallet trusts that node for the genesis it pins then anyway. Once a genesis was pasted, never.
- **Otherwise,** and after Start over: from genesis, checking every block.

See [Starting at a checkpoint](light-client.md#starting-at-a-checkpoint).

**What the node is trusted with:** nothing proven. It does say:
- which tokens exist, from the registry's first page (`GET /tokens`). Each listed token's entry,
  and each balance of it, is then proven;
- which transactions touched an account, from its address index. Each is then proven;
- the coin's symbol and decimals, from `GET /status`, which only change how amounts read;
- the chain's name, from `GET /status`, which only titles the wallet;
- the fee offer, which the review shows before you sign.

## What it proves

The [light client](light-client.md) follows the node from the pinned genesis. Every header is
checked by consensus, and finality by the voters it proved (`wallet/src/chain.rs`).

**Balances:** every few seconds, for every address of every account, at the tip and at the
finalized block:
- **The coin:** the address's entry in `Column::Accounts`, with its nonce.
- **Tokens:** each listed token's registry entry in `Column::Tokens`, for its name, symbol and
  decimals. Then each address's balance of it, by the token standard's
  [provable balances](token-standard.md#provable-balances).

Home shows the coin added up across the account's addresses, then each address with what it
holds of the coin and of each token.

**How far each is trusted** shows beside it:
- **Final:** proven at a final block. Nothing can undo it.
- **Proven:** proven at the tip, which a reorg could still undo.

**Activity:** for each address of the account in use, the node's address index lists the
transactions that touched it (`GET /accounts/{address}/txs`), and the page merges them, newest
first, or oldest with the order turned round. **All**, **Sent**, **Received**, **Waiting** (not
proven yet) and **Failed** filter it, and the search finds a line by its id, a party, a contract,
what it did or its events. Only the chain the wallet follows now shows: each transaction kept
records its chain's genesis, as a new chain of the same keys starts its heights again, so one
from an older chain no longer sits above the newest; those kept from before that are this
chain's once its node mentions them. The wallet proves each one with
`GET /proofs/receipts/{id}`:
- **Who sent what:** the signed transaction, against its block's `tx_root`, with `check_tx`.
- **What it did:** its receipt, against the `receipts_root`, with `check_receipt`.

From those it works out what moved for the account (`wallet/src/history.rs`):
- the coin, from a transfer, a mint, a burn or a call's value;
- the coin a contract paid, from the receipt's payments: a swap's claim, a bridge's release, a
  split, each line proven with the receipt;
- tokens, from the standard's `Transfer` events.

A move between two of the account's own addresses moved nothing for it, so its line says what the
transaction did instead of an amount.

Each line says Received, Sent, or what the transaction did. The line opens to show the block, the
sender, the fee, and every event it emitted.

**Kept once proven.** What was proven is saved, so it shows again at once. What is not final yet
is proven again until it is.

**Older than what it kept.** The saved light client keeps headers only from the finalized block
on. A transaction in an older block it never proved, as one it meets for the first time after a
restart, is in no header it holds. So it fetches the headers between, a stretch of up to 1,000 at
a time, and proves each by its link to the one above (see
[Light client](light-client.md#saving-and-resuming)). Then the transaction checks, and is final.

## Sending

The Send page (`wallet/src/views/send.rs`):
1. **From, what, to whom, how much:** the address it is sent from, one of the account's, then the
   coin or a token that address holds. Each address has its own balance and nonce. To takes any
   address, or an Ethereum one as Ethereum writes it, `0x` and 40 hex digits. The amount is in the
   token's decimals, and checked against the proven balance.
   To takes a [name](#names) too, such as `alice.dev`, which the wallet proves first.
2. **Review.** The wallet builds the transaction from what it proved:
   - **The nonce:** the proven one, past any it sent that is still waiting.
   - **The price:** it signs a probe with no fee offer and asks the node to price it, as
     `chainv4 send` does. An optional tip per gas, in the coin, goes along.
   - **What the review shows:** the most it can cost, gas limit × max price. It refuses to go on
     if the balance does not cover that and the amount.
   - **A token transfer that would fail:** the node dry-runs it to size its gas limit, and when
     the run fails, the review says why in red and the button reads **Send anyway**.
3. **Sign and send.** It signs with the offer and submits it.
4. **Tracking:** the transaction goes into Activity as Pending, then Proven, then Final once the
   light client proves its block final.

A token transfer is the standard's `transfer(to, amount)`, sending no coin.

**Dropped transactions:** one no block holds after 10 minutes shows as Not seen, and stops
holding its nonce, so the next transaction takes that nonce again.

## Names

On a chain with [names](names.md), the wallet proves every name it shows or pays
(`wallet/src/chain.rs`): the name's record and the record of every name above it, as
`pay.alice.dev` rests on `alice.dev`'s, each from `Column::Names` against the tip the light client
checked, and judged by that block's time. The node says the chain's suffix, prices and grace
period, and which names each address owns; what each name is, the wallet proves.

- **Paying a name:** Send's To takes a name. Under it shows what the name was proven to resolve
  to, with how far that is trusted, or that it resolves to nothing. The review shows the name and
  the address, and the transaction pays the address.
- **The Names page:** for the address picked, the name it goes by, a search that finds a name or
  registers it at its price, and the names it owns, each with what it points at and when it
  expires. Each opens to renew it, make it the address's primary name, point it, set a text
  record, make a subname or give it away.
- **Sending them:** every name transaction goes to the Send page, prepared, for the same review,
  biometric check, offline signer or MetaMask as a transfer. What a registration or renewal burns
  counts against the balance in the review.
- **Overview** shows each address by its primary name, proven still to resolve to it.
- **The offline signer** shows a name transaction for what it is: registering, renewing,
  pointing, and so on, with the name and what it sets. It cannot know what a registration burns;
  the chain charges that as it runs.

## Multisig

The **Multisig** page makes [multisigs](multisig.md#in-the-wallet), follows them, and takes their
transactions round the members: a payment proposed and priced, signed with the account's key or on
the offline signer from a `ur:chainv4-multisig` code, approved on chain through Send, and submitted
once enough back it, under a key made for the moment, the multisig paying. What the node says of a
multisig is shown as its word; nothing signed rests on it. The offline signer reads a bundle on
Sign and shows it back signed. For a multisig on Bitcoin it shows the descriptor and fresh
addresses, and with an Esplora API makes, combines and broadcasts its PSBTs; the offline signer
signs a multisig's Bitcoin input at m/48'/coin'/0'/2', and its Pair page shows each account's member
line. For one on EVM chains it shows its contract, and on a chain Settings reaches what it holds and
its owners; its calls are bundles signed with the account's EVM key or by the offline signer as
EIP-712 typed data, deployed and sent from the account. The Swaps page swaps as a followed
multisig too, its EVM locks going to the Multisig page as its contract's calls.

## Mining

The **Mining** page mines from the device, alone or in a [pool](consensus.md#mining-pools), and
claims what a pool owes the account's addresses.

### Mine

The **Mine** card at the top runs a worker of the node's mining port, as `chainv4 worker` does,
inside the wallet, paying the address picked.

- **Where:** the wallet asks `GET /mining` of the pool's node when **Pool's node** is set, else of
  its own. The port is at the address the node publishes (`public`), else at the node's own host
  with the port it names. **Other port** takes a `host:port` in its place, kept in settings as
  `mine_port`. A port open to anyone needs no token; a private one takes the token its operator
  hands its miners, typed into **Token** and kept in settings as `mine_token`: it is the port's
  shared secret, not a key of the wallet's, and proves only that this worker may mine there. The
  worker connects over TCP, through the wallet's proxy when one is set. That proxy must be a
  `socks5h://` one: an HTTP proxy does not carry the port's protocol, and the wallet never goes
  around a proxy set, so with one it does not mine. A node that
  takes no miners answers 404, and the card says so.
- **Alone or in the pool:** alone, each block the device finds pays the address in full and
  nothing comes between blocks; in the pool, the pool pays the address its part of each block the
  pool finds, by the shares it sent, less the pool's fee, claimed below. Whichever the port does
  not take (`solo`, `pool` in `GET /mining`) cannot be picked; a port that takes neither mines
  for its node alone, and the card does not start on it. Kept as `mine_solo`.
- **On what:** the GPU, one of those listed or every one (`mine_gpu`, `mine_gpus`), or a number of
  CPU threads up to the device's (`mine_threads`).
- **Start** and **Stop:** while mining, the card shows whether the worker is connected and, when
  not, why (it connects again by itself), what it mines for, the hashrate of each device and in
  all, the jobs it was handed, the blocks found and the nonces refused. In the pool it shows the
  shares the pool took and their difficulty, and the pool's word on the address's part of its next
  block and all it credited; alone, the block difficulty and how long a block takes on average at
  this hashrate (as many hashes as the difficulty).
- **Payout:** the address picked on the page, the account's first unless another is picked.
  Choosing another account, or another of its addresses, restarts the worker paying it.
- **Phones:** Android and iOS mine only while the wallet is in front. Going to the background
  stops mining, and coming back does not start it again: the person starts it. The worker stops
  itself once the wallet has drawn nothing for 5 seconds, as a phone stops drawing an app it sends
  to the background. Off the charger a phone pauses, and starts again by itself once it is on one,
  looked at every few seconds (Android's battery broadcast, iOS's battery state), unless **Mine on
  battery** is ticked (`mine_on_battery`). A desktop keeps mining while minimized, on battery or
  not.
- **Locking** the wallet, by hand or after `lock_after` minutes without input, stops mining on
  every platform. The wallet never starts mining by itself as it opens.
- **In the browser** the wallet reaches no TCP port: it mines over the node's WebSocket,
  `GET /mining/ws` (`ws://` for a node at `http://`, `wss://` for `https://`), which relays the
  port's protocol unchanged, so the handshake and the MACs stay between the wallet and the port.
  **Other port** does not show there. See [Mining in the browser](#mining-in-the-browser).

Whoever mines, the node checks every nonce before it seals a block with it; the wallet signs
nothing to mine. For a rig, **Mining from other machines** gives the worker's command with the
port and the address filled in, `chainv4 worker --node HOST:PORT --payout ADDRESS`, with `--solo`
to mine alone and `--token FILE` for a private port.

### Mining in the browser

The web wallet and the extension mine on the browser's GPU through WebGPU when the browser has it,
which needs a secure page (https, or localhost). Otherwise, or when CPU is picked, they mine on Web
Workers, one a thread; GPU picked but missing, or failing, mines on every thread but the page's own
and says why. Each job's range of nonces is handed out from one cursor, a GPU batch or a worker's
chunk at a time, so nothing is tried twice; a chunk is sized for about a tenth of a second. A
nonce the GPU finds is checked on the CPU before it is sent. Mining goes on only while the page,
or the extension's panel, is open; the wallet sends its hashrates every five seconds and connects
again five seconds after the connection goes.

trunk builds the worker beside the app, `miner-worker.js` and `miner-worker_bg.wasm`
(`src/bin/miner-worker.rs`), started by `miner-worker-boot.js`, and the extension build takes them
too. The GPU's side is `chainv4-gpu`'s non-blocking path: `Gpu::open_async(&[WorkHash])` opens the
GPU WebGPU gives and compiles the kernels for those hashes alone, and `Gpu::search_async` searches
a batch and awaits the GPU, so the page runs on between batches. The blocking `open`, `open_all`,
`search` and `list` are native only and wait on the same futures, so both search alike. On the
web WebGPU is the only backend: WebGL has no compute shaders.

### Earnings and claims

For the address picked, the page asks the pool's node what the pool owes it
(`GET /pool/claim/{address}`): all the pool credited it, what it claimed, and what a claim pays
now. **Claim** prepares `Tx::Claim` from that answer, with its Merkle path, and hands it to Send,
where it is reviewed, signed and sent like any other transaction, the address paying the fee.
Under it, from `GET /pool/{address}`, the address's part of the pool's next block, its shares in
the window, earnings credited but not yet in a root on chain, the pool's fee, and each worker
connected for it with its hashrate.

- **Which node:** the wallet's own, unless **Pool's node** names another's HTTP API, kept in
  settings as `pool_node`. Only the pool is asked there; the claim goes through the wallet's node,
  so the pool must be on the same chain.
- **Trust:** the pool's numbers are shown as its node says them. A claim rests on nothing it says
  but the fee: the chain checks the path against the root the pool's block committed to, and a
  path that does not hold fails and moves nothing.
- **Nothing yet:** a node with no pool says so, and so does a pool that owes the address nothing
  yet. A pool credits its miners once its blocks are final, so earnings show a while after the
  blocks are found.
- **Kept claims:** each claim the pool's node gives is kept on the device, by chain, in settings as
  `pool_claims` (the newest few, the next to settle, and the newest settled: see
  [Mining pools](consensus.md#mining-pools)). When the pool's node does not answer, **Kept claims**
  checks them at the wallet's own node, `GET /claims/{pool}/{address}?height=`, newest first, and
  **Claim** sends the first whose root is still on chain: all the address earned up to it, less
  what it claimed.
- **History** shows a claim as "Claimed mining pool earnings for" the miner: the transaction
  names all the miner earned, not what the claim paid.

## Governance

The **Governance** page (`wallet/src/views/governance.rs`) covers the chain's settings under
[governance](governance.md): who decides, the changes open and their approvals, approving or
withdrawing, proposing a change, what is in force and what was decided last. It reads
[`GET /governance`](api.md#get-governance),
[`GET /governance/history`](api.md#get-governancehistorybeforevlimitn) and
[`GET /governance/weight/{address}`](api.md#get-governanceweightaddress), asking again every
15 seconds while the page is open. On a chain without governance (404) it says so and shows no
actions.

- **Who decides:** the council's members, or finality's voters for an electorate. The card also
  shows what a change needs, how many epochs an approval counts for, the epoch, the number of
  decisions so far of every kind (what the history counts), and each kind's own version. Each kind
  of change has a ballot and a version of its own: consensus (which also covers finality's tuning
  and switching finality), bootnodes, name_prices, collections, federation, directory, fees,
  reward, limits, upgrade and name. An approval names the version of its change's kind, from
  `versions` in `GET /governance`. Under it, each of the wallet's addresses, and each multisig it
  follows, shows its weight: **may vote** with what it weighs, or **may not vote**. It also lists
  the address's approvals that count, one per kind with the kind's version, each with its own
  **Withdraw** for an address this page sends for. A council member weighs one vote; under an
  electorate, FFG weighs stake and authority finality weighs signers.
- **Open changes:** each change in words, with its kind and that kind's version under it and its
  JSON folded below. Each has a bar for its approvals' weight against what it needs, and its
  approvals, each with the epoch it counts until. An address of the account in use, or a
  followed multisig, that may vote gets **Approve** on each change, or **Withdraw** on the one its
  approval of that kind counts for. An address backs one change of each kind at a time, so it can
  back a fee change and a rename at once. Approving another change of the same kind moves its
  approval there, and leaves its approvals of other kinds alone. The confirm screen says so, and
  that the approval lapses only once a change of its own kind is decided first and that kind's
  version moves on: a decision of another kind leaves it standing. To vote with another account's
  address, switch to that account.
- **Propose:** pick a kind. The chain's name, bootnodes (one a line, none for none), an upgrade
  (least version and the block it starts from, which must be ahead of the light client's tip) and
  name prices (one a line in the coin, by length from 3 characters, the last for any longer) have
  fields of their own. Every other kind (consensus, fees, reward, limits, finality,
  switch_finality, collections, federation, directory) is a JSON editor, filled in with the value
  in force for that kind, so editing starts from what stands. For fees, reward, limits, finality,
  federation and directory the editor also takes a partial object, such as
  `{ "burn_permille": 300 }` for fees: the wallet lays it over the value in force, field by field
  (an object's fields one by one, anything else whole), with chainv4's `params::json::over`, as
  the CLI's `approve-change` does. Fields left out keep what stands, and the form says so. The
  kind's version the approval names guarantees nothing changed in between: should a change of
  that kind be decided first, the approval fails. Whatever the form says goes through chainv4's
  `change_from_json`, the same checks as the CLI's `approve-change`. Its error shows under the
  form, and the change in words shows once it reads, with the kind's version it approves at.
  **Review** opens the confirm screen with the change in words and its whole JSON. A change is
  proposed by approving it.
- **Sending:** from the confirm screen the transaction, `Tx::ApproveChange { version, change }`
  at the version of `change.kind()`, or `Tx::WithdrawChange { kind, version }` at the version
  the approval named (from `approved` in `GET /governance/weight/{address}`), goes to Send like
  any other: priced, signed with the account's key, a hardware wallet or the offline signer, and
  sent. A followed multisig's goes to the Multisig page as a bundle for its members.
- **Your votes:** the account's approvals and withdrawals whose receipts the light client proved,
  each Final or Proven. An approval that was the last one needed shows **Decided change N · kind
  vM**: its receipt's committed outcome is `Decided { decision: N, kind, version: M }`, N the
  chain's count of decisions and M the version of its kind it made. History says the same,
  "Decided change N · Approved …, making kind vM"; a withdrawal reads "Withdrew approval of a
  kind change from vM". Each vote the wallet keeps records its kind; one kept before votes named
  their kind reads as consensus.
- **In force:** every setting, in words, each marked "genesis" or "decided in change N".
- **Recent decisions:** the latest few, newest first, each with its block and who sent it.
- **Trust:** the page shows what the node says. Nothing signed rests on it but the version, the
  change's kind's, which the chain checks: an approval naming a version that has moved on fails
  and changes nothing.

## Faucet

On a node that runs a [faucet](faucet.md), as a dev or test chain's may, the Overview has a
**Faucet** card: pick one of the account's addresses and **Get**. The wallet works out the
faucet's proof of work a slice a frame, so the page keeps moving, then asks.

## Contracts

The Contracts page (`wallet/src/views/contracts.rs`) reads, calls and deploys contracts of every
engine, from the address picked.

- **Kept contracts:** add one by its address, `contract:0x…` or `evm:0x…`, with a name, and for an
  EVM one its ABI's JSON, as `solc --abi` or Foundry writes it. They are kept on this device, and
  a deploy from here keeps the contract it makes.
- **A native contract,** Lua or WebAssembly, lists its functions as its record on the node says.
  Each takes its arguments as `type:value`, one a line, such as `i64:5`, `string:hi` or
  `address:ed25519:0x…`, as `chainv4 send call` does, and coin to send with it.
- **An EVM contract** lists its ABI's functions, reads first, each with a field for every
  parameter, typed: numbers, addresses (any chainv4 address goes as its EVM address), hex for
  bytes, text for strings, and JSON for arrays and tuples, such as `[1, 2]`. A payable one takes
  coin too.
- **Read** runs a function on the node, sending nothing: `POST /call` for a native one, and
  `POST /offer`'s dry run of the call for an EVM one, whose result the ABI reads, as a revert's
  reason or custom error too. A write function's **Try it** runs it the same way, to see what it
  would do.
- **Send** prepares the call for the Send page, which reviews, signs and sends it as any
  transaction, with biometrics, the offline signer or MetaMask.
- **Deploy** takes Lua source, pasted or picked, a WebAssembly module, picked, or EVM init code in
  hex with its ABI, whose constructor's parameters get fields. It keeps the contract under the
  address it will land at, worked out from the sender's next nonce, and prepares the deploy for
  the Send page.

What a read answers comes from the node, as a dry run does: it is not proven.

## MetaMask and hardware wallets

In the browser, MetaMask, and a Ledger or Trezor connected to it, can hold an account and sign
for it. The wallet proves its balance and builds what it sends as for any account; MetaMask signs.
- **Connecting:** Accounts → **Connect MetaMask**, shown when the page reaches it. MetaMask names
  an account by its Ethereum address alone, so it is asked to sign one fixed message, which moves
  nothing: the key that signature recovers to, checked against the address, is the account's.
  The account sits beside the others, as `metamask` in the vault, with its public key alone.
- **Sending:** Review reads **Sign with MetaMask**. MetaMask shows the transaction as EIP-712
  typed data, each field by name: a transfer's from, to, amount, nonce and fee, a token
  transfer's token too. Its signature is recovered, checked to be the account's and to hold on
  this chain, and sent.
- **Bound to the chain:** the typed data's domain is salted with the genesis id, so what MetaMask
  signs for this chain holds on no other. See [Typed data (EIP-712)](crypto.md#typed-data-eip-712).
- **The desktop and Android apps** reach no MetaMask: its accounts are the browser's.

## Signing offline

The keys can live on a device that never goes online. On its first launch, the app asks whether
this install is an offline signer, and it stays that way: changing it takes removing its
wallets, after which the lock screen asks again.

| Install | What it does |
| --- | --- |
| **Online wallet** | Talks to a node, as above. Installs from before roles are this. Its wallets are any mix of two kinds, side by side: ones that hold their keys, from words, and **watch-only** ones, which hold an offline signer's public keys alone, prove their balances, price what to send, and send what the signer signed. |
| **Offline signer** | Holds its keys and makes no network requests at all: it has no client to make them with, so none can go out, whatever its settings say. It follows no chain, so its pages are Sign, Pair, Receive, Accounts and Settings. |

**Pairing:** on the signer, **Pair** shows the account in use as a QR code holding its public
keys, one per scheme, and, for an account its words make, its number and the seed's fingerprint,
which a site's request through the browser extension names its Ethereum key by. On an online install, the lock screen's **Add a wallet** → **Pair with a
signer** makes a watch-only wallet from it, beside any others; in that wallet, Accounts → **Add
from the signer** watches more of the signer's accounts. Read the code, then name and seal the
wallet with a password, as any other.

**Sending:**
1. The watch-only wallet builds the transaction on Send as usual, and the node prices it from a probe whose
   signature signs nothing: the node checks none for a price.
2. **Sign on the signer** shows the request as a QR code: the transaction, its nonce and fee
   offer, the key to sign with, and how the node names the coin and the token.
3. On the signer, **Sign** reads it and lays out what it signs: the amount in the asset's units
   and as the raw units signed, from, to, the token's contract, the most the fee comes to, the
   nonce, and everything the transaction holds. It refuses a key none of its accounts holds, and a
   transaction the chain would refuse. **Sign** shows the signed transaction as a code.
4. The watch-only wallet reads it back, and sends it only if it is the very transaction it asked for, signed
   by that key, and the signature holds.

**Reading a code:** **Scan with the camera** opens the camera live and shows what it sees, reading
each code as soon as it is in view, and a message in parts as the other screen cycles through
them, with how many are in. It is the back camera in a phone's browser and on Android, and the
first webcam on the desktop:

| Where | The camera |
| --- | --- |
| The browser | `getUserMedia`, which wants https or localhost and asks for leave the first time. A browser extension's side panel cannot ask: open the wallet in a tab once, and allow it there. |
| Android | Camera2, through `ui/android/Camera.java`, which the APK carries when it was built with a JDK, as the picker is; the app asks for the CAMERA permission the first time. |
| The desktop | [nokhwa](https://github.com/l1npengtul/nokhwa): AVFoundation on macOS, which asks for leave the first time, V4L2 on Linux, Media Foundation on Windows. |

Or read a still: **Open a photo** (**A photo** in the browser, where a phone offers its camera)
reads a photo or screenshot of the code, held close enough that the code fills most of it, and
**Open a file** an image or a text file. Every code's text can be copied and pasted instead:
**Copy as text** under each.

**What the codes hold:** a Uniform Resource, as MetaMask's and Keystone's are:
`ur:chainv4-pairing`, `ur:chainv4-sign-request` or `ur:chainv4-signed`, then the message's
encoding, the chain's own, in bytewords, with a checksum, in capitals, which a QR code packs
densest. An account's pairing is about 265 characters, a transfer's request about 450, a token
transfer's about 580, and a signed transfer about 550. Bytes that are not exactly one message, or
have anything past its encoding, are refused.

**Too long for one code:** a message over 300 bytes animates: fountain-coded parts, each a code of
about 650 characters, `ur:chainv4-sign-request/7-4/…`, one after another, about three a second,
with the part showing counted; **Pause** holds one, and **Next part** steps by hand. Past the first
round, each part mixes several of the message's, so the other device's camera needs any enough of
them, in any order, and never waits a full round for one it missed; it says how many are in.
**Copy as text** always copies the whole, as one UR.

**From before:** the codes used to be `CV4:1:` and base45. A wallet from before reads none of these,
nor this one any of those: update the signer and the sender together.

**Keep the signer offline:** airplane mode, Wi-Fi and Bluetooth off. The live camera keeps
nothing: each frame is searched in memory and dropped.

### MetaMask's QR hardware wallet

The offline signer is also a QR hardware wallet for MetaMask, as a Keystone is (ERC-4527): MetaMask
holds the public keys and builds what to send; the signer reads each request off MetaMask's
screen, shows what it signs, and shows the signature back.

1. **Pair:** on the signer, **Pair**, **Show MetaMask's code**. In MetaMask, **Add account or
   hardware wallet**, **Add hardware wallet**, **QR-based**, and read it. The code is the extended
   public key at m/44'/60'/0' (`ur:crypto-hdkey`), so MetaMask lists every Ethereum account the
   words make, account n at m/44'/60'/0'/0/n, as this wallet derives them; tick the ones to add.
   It holds every one of those addresses, but no key. A wallet brought in as keys alone, with no
   words, has none to show.
2. **Sign:** MetaMask shows its request as an animated code. On the signer, **Sign**, **Scan with
   the camera**: the parts are fountain-coded, so any enough of them read in any order will do.
3. **Check:** the review names the chain large, by name for the common ones and as **your chain**
   for the id set under Settings, **Your chain**, warning of any other; the account and its path;
   and what it signs: a transaction's recipient, amount at 18 decimals, data, most fee and nonce,
   typed data's domain and message, or a message's text.
4. **Back:** **Sign** shows the signature as a code; in MetaMask, **Get signature**, and read it.

| What MetaMask asks | What the signer signs, and v |
| --- | --- |
| An EIP-1559 or EIP-2930 transaction | The keccak256 of its unsigned bytes; v is the parity, 0 or 1. |
| A legacy transaction | The same, with EIP-155's chain id; v is EIP-155's, `chain_id * 2 + 35 + parity`. One without a chain id is refused. |
| Typed data, eth_signTypedData_v4 | EIP-712's hash, as MetaMask's own; v is 27 or 28. |
| A message, personal_sign | EIP-191's hash; v is 27 or 28. |

It signs for any chain MetaMask is set to, and refuses a request for another seed's keys (its
fingerprint), a path below another account level than m/44'/60'/0', an address that is not the
key's, a transaction for another chain than the request names, and bytes that do not read back
as exactly what they say.

**Tested against MetaMask's own libraries:** `wallet/src/hardware/metamask/` makes requests with
Keystone's `@keystonehq/bc-ur-registry-eth` and ethereumjs, as MetaMask's QR keyring does, into
`metamask.json`, which the wallet's tests read and sign; and checks the wallet's pairing code and
signatures back with them: the accounts the extended key derives, and each signature rebuilt into
its transaction or recovered from its hash, under its request's id.

### Trade

The **Trade** page trades on the chain's [DEXes](dex.md), the native one and, when the node names
its router, the EVM's:
- **trades:** the best route, through one pool or two, its quote, price impact and the least it
  takes at the slippage allowed;
- **pools:** what each holds and its price, a native pool's price chart from its `Sync` events,
  and adding to it or taking your shares out;
- **new pools,** opened by their first deposit, at the price its two amounts set.

Approving a token, trading, adding and taking out are calls the Send page reviews and signs.

### NFTs

The **NFTs** page shows what the account holds of the chain's [collections](nft-standard.md):
- **Where to look:** the node's collection registry, `GET /collections`, and each collection's
  `tokens_of` for each of the account's addresses, the first 100 of each.
- **What it proves:** each NFT's owner, or each multi-token amount, against the tip, by the
  standards' [provable ownership](nft-standard.md#provable-ownership). An NFT shows only once its
  proof says it is the account's, with how far that is trusted.
- **Names and pictures:** a token's metadata and picture, where its `token_uri` points. `data:`
  URIs, on the chain, show always. `https://` and `ipfs://` ones only once Settings, **NFTs**,
  lets the wallet fetch them: each fetch tells whoever serves it, the IPFS gateway set there
  included, which tokens you look at, and from where. PNG, JPEG and WebP pictures show; SVG and GIF
  ones do not yet. At most 8 MB is fetched.
- **Sending:** an NFT, or an amount of a multi-token id, to an address or a name, as a call the
  Send page reviews and signs: `safe_transfer` to a contract, which must say it takes it,
  `transfer` to a key.

### Bridge

The **Bridge** page moves value through the chain's [bridges](bridges.md), the ones its
[directory](governance.md#directory) names, under the federation in force, both proven by the
light client:
- **To an EVM chain:** the coin, a token, an NFT or a multi-token amount, to an address there:
  letting the gateway take it, then sending, calls the Send page signs. The account's transfers
  out, among the latest 20, show the board's count of signatures and whether Gateway.sol took
  them; one signed and not taken is delivered from here, with the account's Ethereum key, priced
  and signed as the Swaps page signs there.
- **From an EVM chain:** letting Gateway.sol take it, then sending, signed there with the
  account's Ethereum key, or by the offline signer. It arrives as the federation signs it.
- **Bitcoin:** the account's deposit address, with its QR code, computed here from the federation.
  A payment to it, found by the wallet's own [Bitcoin light client](#bitcoin-followed-by-the-wallet-itself)
  and deep enough, is minted as wBTC by its proof from the block it came in; without the light
  client, by Esplora's proof of a transaction typed in. wBTC is withdrawn to a `bc1q…` address or
  any output script, and the account's withdrawals show queued or paid.

Settings gives the EVM chains' JSON-RPC URLs and the Esplora API, as for Swaps.

### Swaps

The **Swaps** page trades coins, a token or an NFT between any two of this chain, Bitcoin and EVM chains
such as Ethereum, Sepolia or Base, with no one in between. Each swap is two legs:
- **an offer and its answer** travel as codes, and the swap's secret is sealed in the vault;
- **where each leg stands** is shown, and the second lock waits until the first stands as the
  terms say;
- **this chain's locks and claims** are calls the Send page signs;
- **an EVM chain's** go through its JSON-RPC: priced, signed with the account's Ethereum key, or
  by the offline signer through `ur:eth-sign-request`, and waited for until mined;
- **the HTLC on Bitcoin** is paid from any Bitcoin wallet, and claimed or refunded here, by the
  account's key or, for one watched from the offline signer, as a PSBT the signer signs.

The page has three sections: **Your swaps**, **Make an offer** and **Answer an offer**, which reads
an offer to answer or the answer to one of yours. A swap opened takes the page, with **← All
swaps** back. It says what you pay and get, marks its six steps (offered, answered, each side's
lock, each side's claim, in the order they happen) and says in a sentence whose turn it is; the
button for that step follows. The terms, the hash, the contracts, each leg's state and the HTLC
fold under **Details**, and forgetting the swap under **More**.

Settings, **Swaps** names the swap contract when the chain's directory and the node do not, an optional Esplora API to
look Bitcoin up at, and EVM chains' JSON-RPC URLs, one a line. See [Atomic swaps](swaps.md).

### Bitcoin, followed by the wallet itself

On the desktop and Android apps, Settings, **Bitcoin** turns on a light client for Bitcoin, its
test network (testnet4 or signet), or both. It runs Kyoto (the `bip157` crate):
- **what it asks:** each block's compact filter (BIP157/158), from Bitcoin's own peers;
- **what it fetches:** only the blocks whose filters match the scripts it watches, so no server
  learns which addresses are yours;
- **what it watches:**
  - each account's own Bitcoin addresses, its further ones included; on the test networks, its
    words' first testnet key;
  - every swap's HTLC on that network.

What it finds shows on **Overview**, a Bitcoin card with the account's balance, confirmed in
blocks, and its latest coins. On **Swaps**, an HTLC's funding is found with its confirmations, a
claim's secret is read, and spends go out to Bitcoin's peers.

**Syncing:**
- a first sync starts from a checkpoint built into the app, a block a week or so before its
  build, and takes some minutes and a few tens of megabytes;
- later syncs pick up from where the last one got to, which the wallet keeps between runs, apart
  from the settings;
- **Scan again** forgets what it found and starts from the checkpoint, for an account added with
  history since then.

**Connections:**
- Bitcoin's DNS seeds give its peers, or name your own node under **Peer** to try first;
- with a proxy set, every connection and seed lookup goes through it. It must be SOCKS5, as Tor's
  is: an HTTP proxy is refused rather than gone around.

It runs while the wallet is open, and not on an offline signer, nor in the browser, which opens no
raw connections.

### Bitcoin wallets

The offline signer is also a QR hardware wallet for Bitcoin wallets, as a Keystone or a Passport
is: Sparrow, BlueWallet, Nunchuk or another that pairs with an airgapped signer by its account.
The Bitcoin wallet watches the addresses and builds each transaction as a PSBT; the signer reads
it off the screen, lays out what leaves, signs, and shows the PSBT back for the wallet to send.

1. **Pair:** on the signer, **Pair**, the **Bitcoin** card: pick **Mainnet** or **Testnet**, then
   **Show the Bitcoin code**. In the Bitcoin wallet, add a QR or airgapped hardware wallet (in
   Sparrow, a new wallet's **Airgapped Hardware Wallet**; in BlueWallet, **Import wallet**, **Scan
   or import a file**) and read it. The code is `ur:crypto-account`: the seed's fingerprint and
   `wpkh()` of the extended public key at BIP84's m/84'/0'/0', or m/84'/1'/0' on the test
   networks, so the wallet is native segwit, receiving at 0/n and changing at 1/n. On mainnet its
   receiving addresses are the `bc1q…` ones this wallet's accounts have, account n the nth. The
   card names the first address: the Bitcoin wallet's should read the same.
2. **Sign:** the Bitcoin wallet shows the PSBT as an animated `ur:crypto-psbt`. On the signer,
   **Sign**, **Scan with the camera**.
3. **Check:** the review names the network large, **Bitcoin** or **Bitcoin testnet**; each output,
   its amount and address (`bc1q…`, `bc1p…`, `1…`, `3…`, or data), marked **Change** with its path
   when it pays one of this seed's keys back; the fee, with its rate when the signer signs every
   input; what it spends; and a lock time, if any.
4. **Back:** **Sign** shows the PSBT with the signatures added, animated; read it in the Bitcoin
   wallet, which finishes the transaction and sends it.

It signs the inputs of its own seed's keys at m/84'/0'/0'/{0,1}/n or m/84'/1'/0'/{0,1}/n that
spend native segwit (P2WPKH) coins, or a [swap](swaps.md)'s HTLC whose script the PSBT gives
(claim or refund, named on the review), with BIP143's hash of all of the transaction, SIGHASH_ALL,
and gives back every pair the PSBT held. It refuses:

- a PSBT with nothing of this seed's to sign, as from a wallet paired with another;
- an input of this seed's at another path, of another kind (legacy, wrapped or Taproot), whose
  key is not the one at its path, or asking for another sighash than ALL;
- an input whose coins it is not told, as the fee could not be known;
- a previous transaction that is not the one spent from, or coins told two ways that disagree;
- inputs or change on Bitcoin and its testnet both;
- outputs worth more than the inputs, and PSBTs that are not BIP174's version 0 or do not read
  whole.

When the wallet leaves out the transactions an input's coins came in, their amounts are its word:
the review says so, and a fee more than a tenth of what is spent is shown in red.

**Tested against Bitcoin wallets' libraries:** `wallet/src/bitcoin/wallets/` makes PSBTs with
bitcoinjs-lib, as BlueWallet does, and Keystone's `@keystonehq/bc-ur-registry`, into
`wallets.json`, which the wallet's tests read, review and sign, matching bitcoinjs-lib's own
signatures byte for byte; and checks the wallet's pairing code and signed PSBTs back with them:
the addresses the account derives, and each PSBT finished and its transaction extracted.

## Receiving

The Receive page shows one of the account's addresses, picked by its scheme, to copy and as a QR
code, dark on white in either theme so any camera reads it, and lists them all below. The coin and
every token arrive at any of them; each keeps its own balance.

**Copying an address:** click or tap it, on any page, and it is copied, with **Copied** beside it
for a moment. Hovering over a shortened one shows it whole. Receive's **Copy address** button
copies the one shown.

## Updates

The wallet asks its node for the newest [release](updates.md) (`GET /release`) on connecting and
then hourly, and checks it itself against the release keys built in: the node only relays it. A
build without release keys, as one from the checked-in `release/keys.json`, shows nothing, and so
does a node from before releases, which answers 404.

- **Behind:** a card above every page says "Wallet 0.2.0 is out (this is 0.1.0)", with the
  release's notes. **Not now** puts it away until the wallet starts again.
- **Update, on the desktop:** with a binary in the release for the target the wallet was built
  for, **Update** downloads it through the proxy set, off the UI thread, checks its size and
  SHA-256 against what the keys signed, and puts it in place of the wallet's own binary. It says
  "Restart the wallet to run 0.2.0" once done; the running wallet carries on as it was.
- **Elsewhere:** in the browser, on Android and on iOS, and on a desktop with no binary for its
  target, it says to update it from where you installed it.
- **Unsupported:** a wallet older than the release's oldest that still works shows only a screen
  saying it no longer works with the network, locked or open, with **Update** on the desktop and
  **Quit**.
- **The node's own upgrade:** when governance's upgrade (`GET /status`) or the release's
  (`GET /release`) stops the node's chainv4 at a height, Settings → Node says "Your node stops at
  block H unless updated to chainv4 L". Past it the node imports no blocks, so the wallet follows
  no further until it is updated or you connect to another.
- **History** shows an approved upgrade as "Approved an upgrade: nodes older than 0.2.0 stop at
  block 150000".

## The chain's name

The lock screen, the sidebar, the bar on top and the window's title (the browser's tab) name the
network as `GET /status` names it, such as "chainv4 testnet" and "chainv4 testnet wallet". The
wallet keeps the last name it saw for each genesis, so the lock screen shows it before the node
answers. With no name known, from no answer yet, a chain with none (`null`) or a node from before
names, it says "chainv4", and so does an offline signer. A name a chain could not have is taken
as none. The wallet's own name stays chainv4 wherever it means the software: commands such as
`chainv4 multisig`, releases and updates, and what it calls itself to MetaMask and sites.
**History** shows a governance rename as "Approved renaming the chain to chainv4 mainnet".

## What it remembers

Everything is in app storage under one key, `chainv4-wallet`, saved by eframe every 30 seconds,
on exit, and when Android sends the app to the background:
- the node, the proxy, and the genesis pinned at each node;
- the mining pool's node, for a pool on another node;
- each wallet: its name, its vault, and the account in use in it;
- the light client's state, by genesis, to resume from the finalized block, not genesis;
- the chain's name its node last said, by genesis, for the titles before the node answers;
- what it sent that is not final yet, and what it proved, the newest 2,000;
- the wallet last opened, the lock time, and the theme.

Only the vaults are secret.

## Look

It follows the look of the wiki every build writes (see [Documenting code](development.md#documenting-code)), quieter.
Its palette, fonts and roomy spacing are its own; the widgets they dress come from the `ui` crate
it shares with the explorer (see [The ui crate](development.md#the-ui-crate)).
- **Colors:** the wiki's palette, dark or light.
- **Type:** Manrope for text, JetBrains Mono for addresses and hashes, and Newsreader for titles
  and the big numbers. The fonts and their licences are in `wallet/assets/fonts`.
- **Layout:** each page is a column of cards beside a sidebar with the account, the pages, and
  the light client's state. In a window too short for them all, the pages scroll, the light
  client's state, Lock and the theme staying at the sidebar's bottom
  (`chainv4_ui::layout::sidebar`). Below 640 points wide, as on a phone, the sidebar gives way to a bar
  on top with the account, the light client's dot and Lock, and tabs along the bottom: sharing
  the width when they fit, otherwise each wide enough for its name in a row that scrolls
  sideways, faded at an edge where more wait, and bringing the page opened into view. A card's
  row puts its label above what it says on a narrow screen, and text wraps everywhere, in rows
  too, so nothing is cut off at the screen's edge. Android's back gesture closes a view the page
  opened, such as a swap, then returns through the pages left, each once, scrolled to where it
  was left; on the first, the app goes to the background, as Android apps do. A page opened anew
  starts at its top.

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
node --test extension/test.mjs   # the extension's scripts, in a browser of mocks
```

**One app, three starts.** The app is a library (`src/lib.rs`), as Android loads it, so the
package is `chainv4-wallet`. Its binary, `wallet` (`src/main.rs`), starts it on the desktop and in
the browser. Android starts the library at `android_main`, which hands it to the `ui` crate's
`android::run`.

## Browser extension

`wallet/build-extension.sh` builds the wallet as an extension for Chrome and for Firefox, into
`wallet/target/extension`: a folder for each, to load unpacked, and a zip of each, for the stores.
It needs trunk, python3 and zip; `--browser chrome` or `--browser firefox` builds one.

```sh
wallet/build-extension.sh
# Chrome: chrome://extensions, Developer mode, Load unpacked, wallet/target/extension/chrome
# Firefox: about:debugging, This Firefox, Load Temporary Add-on, wallet/target/extension/firefox/manifest.json
```

**What it is:** trunk's web build, as a node serves from `/wallet/`, with the extension's own files
from `wallet/extension/`:
- **Manifest V3** for each browser: `manifest.chrome.json` and `manifest.firefox.json`, the version
  taken from `wallet/Cargo.toml`. Their CSP lets the page run WebAssembly
  (`'wasm-unsafe-eval'`) and nothing inline, so the script trunk writes into `index.html`, which
  starts the wasm, moves to `boot.js`.
- **Beside the page, not a popup:** the toolbar button opens Chrome's side panel or Firefox's
  sidebar (`background.js`). It stays open as you browse, so a biometric prompt, a camera scan or
  MetaMask does not close it, as they would a popup; and at a side panel's width, the wallet
  takes its phone layout.
- **Nodes, as they are needed:** the extension asks for no site at install. Its manifest lists
  every `http` and `https` host as optional; the wallet says which node it talks to
  (`window.chainv4Node`), and `access.js` shows a bar above it, **Let the wallet reach its node**,
  whose **Allow** asks the browser for that one host. Until then the node's answers are blocked.
- **Its default node** is `http://127.0.0.1:8080`, as no node serves the extension's page; name
  another under Settings.
- **Storage** is the extension's own `localStorage`, apart from the web app's at any node: wallets
  made in one are not in the other, and removing the extension deletes its wallets. Keep the words.

**Biometrics** are a passkey's here too, made for the extension's own origin. Firefox refuses
WebAuthn to extension pages ("The operation is insecure"), so in its extension the wallet offers
no biometrics and the password stays; Chrome's may allow them. A browser that refuses them anywhere
else gets the same plain message rather than its own.

**CI:** `.github/workflows/extension.yml` runs the extension's script tests
(`node --test wallet/extension/test.mjs`), builds both on every push to master, keeps the zips as
artifacts, and attaches them to a `v*` tag's release.

### Sites

The extension gives sites `window.ethereum`, so a dApp that speaks to MetaMask speaks to the
wallet: it connects, sends Ethereum transactions, and signs messages and typed data, from the
wallet's Ethereum accounts: the secp256k1 ones it holds keys for, and those watched from the
offline signer.

- **Off until you turn it on.** Settings, **Sites**, **Let sites ask to connect** shows a bar above
  the wallet whose **Allow** asks the browser for every site, once; then `inpage.js` and
  `content.js` run on every page. Turned off, they go, and so does that leave.
- **Beside MetaMask.** It announces itself by EIP-6963, which dApps list wallets by, as
  **chainv4 wallet**, and takes `window.ethereum` only when no other wallet did first.
- **Reads** a site makes, `eth_call`, `eth_getBalance`, blocks, receipts and logs, go to the wallet's
  node, at `POST /rpc`, from the extension's background, whether the wallet is open or not. They
  are the node's word, as any dApp's reads are.
- **One chain:** the wallet's node's. `eth_chainId` says it, and `wallet_switchEthereumChain` to
  any other fails with 4902: switch the node under Settings instead.
- **Approving:** a site's connect, send or signature waits for the wallet. An unlocked wallet that
  is open, in the side panel or a tab, shows it over its page; otherwise a small window of the
  wallet opens, unlocks, shows it, and closes once nothing is left. Closing it rejects what waits.
  - **Connect** ticks which Ethereum accounts the site sees; it sees only those, and asks to sign
    with only those.
  - **Send** prices what the site left out on the node, its nonce, gas and gas price, and shows the
    recipient, the amount, a call by name when the contract is kept with its ABI on the Contracts
    page, and the most it can cost. It goes as an EIP-1559 Ethereum transaction.
  - **Sign a message** is `personal_sign`, shown as text, or hex when it is not; **sign typed
    data** is `eth_signTypedData_v4`, its domain and message laid out, with a warning when it
    names another chain.
  - With biometrics on, each send and signature waits for them too.
  - **An account watched from the offline signer** signs there: **Ask the signer** shows the
    request as `ur:eth-sign-request`, as MetaMask's are, naming the key at m/44'/60'/0'/0/n under
    the signer's fingerprint, the site and, for a send, the EIP-1559 transaction, priced. On the
    signer, **Sign** reads it and lays it out as it does MetaMask's, headed **A site asks, through
    the wallet**; its `ur:eth-signature` is read back here with the camera, and the send goes to
    the node, or the signature to the site. A signature for another request, or not the account's
    key's over what was asked, is refused. Only accounts paired by a code that says where the
    signer derives them are offered: an account paired before pairs again. The camera needs the
    extension's leave: give it once by scanning in a tab, as the side panel cannot ask for it.
    Pasting the signer's code as text, or opening a photo of it, works too.
- **Connected sites** are listed under Settings, **Sites**, each with **Disconnect**. A site may
  disconnect itself with `wallet_revokePermissions`.
- **Never:** `eth_sign`, `eth_signTransaction` and the older typed data versions, which sign what
  cannot be read, are refused with 4200.

## Android

`wallet/build-android.sh` builds the Android app with [xbuild](https://github.com/rust-mobile/xbuild),
pinned to a commit, which it installs if it is missing. It hands over to `ui/build-android.sh`,
which builds the explorer's app the same way (see [Android](explorer.md#android)):

```sh
wallet/build-android.sh                    # wallet/target/android/chainv4-wallet-arm64.apk
wallet/build-android.sh --arch x64         # for x86-64 emulators
wallet/build-android.sh --debug --install  # a debug build, put on the device adb sees
```

**What it needs:**
- Rust's Android targets, which it adds;
- clang, lld and llvm-ar: `brew install llvm lld`, or `apt install clang lld llvm`. Homebrew keeps
  LLVM off the PATH, and the script looks there;
- for picking files, a JDK and Android's build-tools, found through `ANDROID_HOME` (or Android
  Studio's own SDK folder). Without them, or with `--no-picker`, the app is built without
  **Open key file**, and says so as it builds.

It needs no NDK and no Android SDK: xbuild downloads an Android sysroot and `android.jar` itself,
and builds the APK without Gradle. `adb` is only for `--install`.

**The app** (`wallet/manifest.yaml`):
- `org.chainv4.wallet`, for Android 7 (API 24) and up, targeting API 33;
- a NativeActivity, and the INTERNET permission;
- `org.chainv4.ui.Picker`, a small Java activity of the `ui` crate's for picking files (see
  [Picking files](#picking-files));
- `wallet/assets/icon.png` for its icon.

Plain-HTTP nodes need nothing more: Android's cleartext rule binds Java's networking, and the
wallet talks to nodes from Rust over sockets of its own.

**Signing:**
- **Without a key:** xbuild's debug key, the same for everyone. Fine to sideload, but Android
  only updates an app with one signed by the same key: moving to your own means uninstalling
  first, and the vault goes with it.
- **With your own key:** `--pem FILE`, or `X_PEM` set to the PEM's text: a certificate and its
  RSA key in PKCS#8. Make one once, and keep it:

  ```sh
  openssl req -newkey rsa:2048 -new -nodes -x509 -days 10000 -subj "/CN=chainv4 wallet" \
      -keyout key.pem -out cert.pem
  cat cert.pem key.pem > signing.pem
  ```

**CI:** `.github/workflows/android.yml` builds both of the wallet's APKs, and both of the
explorer's, on every push to master and on `workflow_dispatch`, and keeps them as artifacts. It
signs with the `ANDROID_SIGNING_PEM` secret, holding `signing.pem`'s text, when there is one, and
with the debug key otherwise. A `v*` tag attaches all four to its release.

### Biometrics on Android

`ui/android/Biometric.java` is compiled in with the picker, and the manifest asks for
`USE_BIOMETRIC`, granted as the app is installed. It needs Android 9: BiometricPrompt is 28's. The
app asks it whether strong biometrics can be used as it starts and every few seconds after, so a
finger enrolled while it runs is noticed. The Keystore key is AES-256-GCM, made with
`setUserAuthenticationRequired` and `setInvalidatedByBiometricEnrollment`, and on Android 11 and up
`AUTH_BIOMETRIC_STRONG` for each use.

**On an emulator:** set a PIN (`adb shell locksettings set-pin 1234`), enroll a fingerprint under
Settings › Security, touching the sensor with `adb emu finger touch 1`, and touch it the same way
when the wallet's prompt shows. Enrolling another with `adb emu finger touch 2` invalidates the
key, as a new finger on a phone would.

### Picking files

Android hands a picked file back to the activity that asked for it, through Java, and a
NativeActivity drops it. So the APK carries `ui/android/Picker.java`, which the build script
compiles with `javac` and Android's `d8` after xbuild has built the APK, adds as `classes.dex`,
and then signs the APK again, with xbuild's own debug key or your release key, so it updates an
app built without it. The app starts the picker through JNI, the picker opens Android's document
picker, reads each file picked whole, at most 32 MiB in all, and the app takes what it read once it
is back. The manifest declares the picker on the lines marked `# picker`, which the script leaves
out when it builds without one, as Android refuses an app that declares code it does not hold.

**On the phone:**
- **The node:** `http://10.0.2.2:8080` at first, which is the machine running the emulator, as the
  emulator sees it. On a phone, name the node under Settings, such as `http://192.168.1.20:8080`.
- **The keyboard** comes up when a field is focused, and its keys arrive as key events.
- **The bars:** the app draws behind the status bar, the navigation bar and the keyboard, and keeps
  its content in the part Android says can be seen.
- **Storage:** it saves when the app goes to the background, and every 30 seconds.
- **Copying** goes to Android's clipboard, through its ClipboardManager, which the `ui` crate calls
  over JNI: egui reaches no clipboard there. Android shows what was copied.
- **Pasting:** hold a finger on a text field for a bar with **Paste** and **Select all**, and
  **Copy** and **Cut** once some text is selected; a drag across a field's text brings it too.
  While the keyboard is up, a **Paste** button sits above it whenever the clipboard holds text.
- **Picking files:** **Open key file** opens Android's own document picker, which shows the
  phone's files, Downloads and any cloud drives.
- **Scrolling:** a swipe scrolls the page wherever it starts, text included, as text is not
  selectable on a touch screen.


## iOS

`wallet/build-ios.sh` builds the iOS app, through `ui/build-ios.sh`, which builds the explorer's
the same way (see [iOS](explorer.md#ios)). It draws through Metal, wgpu's, as Apple has deprecated
OpenGL on iOS; everywhere else the apps keep glow.

```sh
wallet/build-ios.sh --sim                  # in the iOS Simulator, installed and started
wallet/build-ios.sh --ipa                  # wallet/target/ios/wallet.ipa, unsigned
wallet/build-ios.sh --provision TEAMID     # once, with the iPhone connected (see below)
wallet/build-ios.sh --install              # signed and installed on the iPhone connected
```

**What it needs:** Xcode, and Rust's iOS targets (`rustup target add aarch64-apple-ios
aarch64-apple-ios-sim`); for the Simulator, an iOS runtime (`xcodebuild -downloadPlatform iOS`);
for `--provision`, XcodeGen (`brew install xcodegen`).

**Getting it on an iPhone,** three ways:
- **Your own Apple ID, free:** sign in to Xcode (Settings › Accounts), note the personal team's id
  there, connect the iPhone, trust the Mac and turn on Developer Mode (Settings › Privacy &
  Security). `--provision TEAMID` has Xcode build a stub app of the same bundle id, which makes
  the Apple Development certificate and the provisioning profile; `--install` then signs the
  wallet with them and installs it through `devicectl`. A free team's profile lasts 7 days:
  provision and install again after.
- **Sideloading:** `--ipa` packs an unsigned IPA that AltStore or Sideloadly sign with your Apple
  ID as they install it, and renew.
- **The Simulator,** for the layout and everything but the camera. Since Xcode 27 its window is
  DeviceHub (`open -a /Applications/Xcode.app/Contents/Applications/DeviceHub.app`), which
  `--sim` opens.

**The app:** `org.chainv4.wallet`, for iOS 16 and up, its icon sized from
`wallet/assets/icon.png`, and an Info.plist that says why it wants the camera, Face ID and the
local network. App storage is `wallet.ron` under its Application Support folder.

**What `ui/src/ios.rs` and the `ui` crate do on iOS,** as `ui/src/android.rs` does on Android:
- **the safe area:** winit's view is moved inside the notch and the home indicator, and the window
  around it painted the app's color; the keyboard is winit's own, rising while a text field wants
  it;
- **iOS 27's scene lifecycle:** a scene delegate the Info.plist names, and winit's window put in
  its scene, without which iOS 27 stops the app at launch;
- **the clipboard** is UIPasteboard's: what the app copies goes there, and a held finger over a
  field brings up the same bar Android has, Paste among it;
- **back** is iOS's own gesture: a swipe in from the left edge comes to the app as the back key,
  which closes an open view or returns to the page before;
- **files** are picked with UIKit's document picker, a copy of each read; a photo is taken with
  the system's camera and comes back as a JPEG;
- **the camera** for reading codes is the back camera through AVFoundation, each frame's luma
  turned upright;
- **Face ID or Touch ID** keep the vault's key in the Keychain, behind LocalAuthentication, as
  Touch ID does on a Mac, and say which the phone has.

## What to keep in mind

- **Keep the words.** They bring every account back, on any device. Without them, a lost device
  or a forgotten password loses the wallet: nothing opens a vault without its password, and
  biometrics keep only what the password opens, so they are no way back from a forgotten one.
- **A passphrase is part of the wallet.** The same words with another passphrase, or none, make
  other accounts, all empty.
- **The Ethereum address is not on Ethereum.** It is the address Ethereum's wallets show for the
  same words, and what is sent to it on this chain stays on this chain. Ether sent to it on
  Ethereum is on Ethereum, out of this wallet's sight.
- **Browser storage is per origin.** The wallet served from one node's `/wallet/` and the same
  wallet from another address keep separate vaults.
- **A node can withhold, not lie.** It can leave tokens off its list, or transactions out of its
  index. It cannot make a balance or a transaction show that does not check.
- **Coin a contract pays out shows in the balance, not the activity.** Only the trace records such
  payments, and the receipts root leaves the trace out, so the wallet cannot prove them line by
  line. The guestbook's `Tipped` event, for one, does show.
- **Only the registry's first page of tokens,** 100, is proven. A token past it does not show.
- **Unlocking takes a moment.** Argon2id is slow on purpose, and slower in the browser.
- **A new dev chain looks like the old one.** Every dev chain has the same genesis id, so after
  starting a new one the wallet resumes what it followed on the old, and keeps proving: Settings
  → Light client → **Start over** follows the new one. Its keys and wallets stay.
- **The API is plain HTTP.** What the wallet sends is signed and public anyway, but keep the node
  on a trusted network all the same.
- **A signer trusts the sender for names alone.** What the signer shows of the coin's and token's
  symbol and decimals is the sender's word, from its node: check the raw units and addresses,
  which are what is signed. The sender trusts the signer for nothing: what comes back is checked
  against what it asked.
- **A proxy carries plain HTTP as it is.** It can read the wallet's requests to an `http://` node,
  and so can a Tor exit relay. The light client still checks every answer. A `.onion` node is
  reached without leaving Tor, end to end encrypted, and the node never learns where the wallet
  is.
