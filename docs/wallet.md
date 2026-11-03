# Wallet

The wallet (`wallet/`) is a light wallet, built with egui, for the desktop, the browser and
Android:
- **Keys:** wallets of accounts from a seed phrase, each sealed with a password on the device, or
  none, or with biometrics instead of it or as well as it (see [Biometrics](#biometrics)).
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
- else the public devnet, `https://chain.homepage.gs` (`chainv4_ui::PUBLIC_NODE`).

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

**A wallet** is a seed phrase, or several, the accounts made from them, and one password that seals
it all. The lock screen (`wallet/src/lock.rs`) makes the first wallet, and holds them all:
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

**Several seed phrases in one wallet.** Accounts, under **More words**, adds another, on an offline
signer and an online wallet alike; a watch-only wallet holds no words, so it has no such option:
- **Adding:** words you have, with their passphrase if they had one, or new ones, written down and
  three of them asked back as a new wallet's are. The same words twice are refused, by their
  fingerprint; the same words with another passphrase are another seed, and welcome. It comes with
  its account 0 and an optional name for each. It does not scan for accounts already used: **New
  account** brings the next one.
- **Counting:** each phrase counts its accounts and further keys on its own, so a phrase's first
  account is number 0 and has the addresses a restore of those words alone gives it. With several,
  **New account** asks which phrase, and each account says which it comes from. With one, the page
  is as it always was.
- **Settings** lists them, each with its name, its fingerprint and how many accounts it has. It
  shows one at a time, after the password. A phrase's name can be changed once there are several:
  a wallet of one keeps no name, to stay in the form older builds read.
- **Removing** an account leaves its words in the vault, even with none left. Removing a phrase is
  a step of its own in Settings: a preview of the accounts that go with it, a typed `yes`, and the
  password. The first phrase cannot go this way, only with the wallet.
- **Which words sign:** a MetaMask request by the fingerprint it names, or if it names none by the
  address at its path, and if it names neither it is refused rather than guessed at; with one
  phrase in the wallet, as it always was. A Bitcoin PSBT is signed by every phrase holding one of
  its inputs, and a chainv4 request by the account with its key. Pair's MetaMask and Bitcoin codes
  are those of the account in use's words, so pick an account from other words to pair those.
- **Backups:** write down every phrase. No phrase brings back another's accounts, and restoring a
  wallet from one set of words brings back that set alone.

## Keys

**The vault.** Each wallet's words and accounts are kept in app storage, sealed with its password,
at least 8 characters, or none (`wallet/src/vault.rs`):
- **Stretching:** Argon2id stretches the password into a key, with a random salt of the vault's
  own.
- **Sealing:** ChaCha20-Poly1305 seals the accounts with that key, under a fresh nonce each time
  they are saved.
- **What storage shows:** without the password, the salt, the nonce, and how long the sealed
  accounts are.

**Where it is kept:** a file of its own in `wallet-store`, in the app's storage directory on the
desktop and its private data folder on a phone, or the page origin's `localStorage` in the browser
(see [What it remembers](#what-it-remembers)). Clearing the site's data, or the app's, deletes the
keys, so keep the words written down.

**Showing an account's private keys** asks for what opens the wallet first, in every mode, as showing
its words does. Only a wallet that opens with a tap has nothing to ask for.

**Unlocking:** the password opens the vault. While unlocked, the wallet keeps the stretched key in
memory, so every change seals the accounts again without asking. Locking drops the keys and that
key alike.

**No password.** Leave both fields empty and the vault is sealed with an empty one: the lock screen
offers **Open** instead of a field, and says that anyone who can read the app's storage, a phone's
backup or malware, can open it too. Fine for small sums. Biometrics of the older kind, as an
alternative, change nothing about that: a copy of storage still opens it, as the vault is still
keyed from the empty password. Settings sets a password later, and also takes one away, or makes it
need biometrics (see [Biometrics](#biometrics)), which is what stops a copy of storage.

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
- **New account:** the words' next account, the lowest number the wallet does not hold of those
  words, with its four addresses. With several phrases it asks which. A wallet without words makes
  one random key instead, in the scheme picked.
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

**Going back to an older build.** A wallet of one phrase is stored byte for byte as it always was,
so an older build keeps reading it. Once it has a second, an older build says it is damaged and
writes nothing, rather than open half of it and save the rest away, and it stays that way until
this build removes the further phrases.

**An account this build cannot read.** An account whose key is damaged, or in a scheme this build was
made without, does not keep the wallet shut. Accounts lists it by name as one this build cannot read,
and it cannot send or sign. Every save writes it back as it came, so a build that reads it finds it
whole. Removing it takes a typed yes. An account from words opens with the keys this build has, and
any further key in a scheme it lacks stays in the account unused.

A vault with an account naming words it does not hold stays shut: nothing can be made from them, and
nothing is written. The vault is sealed as one piece under one key, so damage to its bytes does not
cost one account: it fails to open at all, and the file's copy is what helps then.

**Changing the password:** the current one, then the new one twice, or nothing to have none. The
accounts are sealed again under a new salt, and the new seal is written at once. Biometrics of the
older kind kept the key the old password stretched to, so they turn off: that key is retired, not
forgotten, until settings without it are on disk, so a kill in between strands nothing. A wallet that needs both a password and biometrics takes
the check as well and cannot have none: its key is made anew from the new password and the same
hardware secret, under a new salt, and opened again before it replaces the old vault. A wallet with
biometrics alone has no password to change.

## Biometrics

Settings' **How it opens** chooses, wallet by wallet, what opens it. Nothing changes until you
choose (`wallet/src/guard.rs`; the platforms are in `ui/src/biometric.rs`).

| Way of opening | What seals the vault | What a copy of this app's storage can do |
| --- | --- | --- |
| **No password** | A key stretched from the empty password. | Open it. |
| **Password only** | Argon2id's stretch of the password. | Guess at the password on its own computer: a long one is what stops it. |
| **Biometric only** | A random key, kept only wrapped by hardware or a passkey. No password exists. | Nothing: it opens nothing, and there is nothing in it to guess. |
| **Password and biometrics, both required** | HKDF-SHA-256 of the password's stretch and a random secret hardware keeps, with its own domain string and the vault's salt. | Nothing, even with the password. Hardware alone opens nothing without the password. |
| *Password or biometrics, either* (no longer offered) | The password's stretch, a copy of which biometrics keep. | As with the password alone, or, with no password, open it. |

**In every mode, while a wallet is unlocked its keys are in memory,** where malware on the unlocked
device can read them. These modes protect a wallet at rest, and nothing here stops malware that
waits for you to open it.

**Either opens it** stays for the wallets that have it, working as they always did, until they
switch: their biometrics can be turned off, and do not come back. On macOS, Linux and Windows no
mode that needs biometrics is on offer, and Settings says why.

**Every send** a wallet with biometrics signs, and every request an offline signer signs, waits for
one more check. The check of a wallet that needs biometrics is never skipped: when they read as
unavailable it fails, and nothing is signed. So do showing its words and keys, and removing its
words or itself, which ask for what opens it, biometrics in place of a password, or both.

**What holds the key:**

| Where | What wraps it | What a check is |
| --- | --- | --- |
| Android 9 and up | An AES key in the Android Keystore, inside secure hardware or none is made, usable only right after a strong biometric check, which a fingerprint or face enrolled later invalidates for good. | BiometricPrompt, through `ui/android/Biometric.java`. |
| iOS | A Keychain item of this device alone (`kSecAttrAccessibleWhenPasscodeSetThisDeviceOnly`), tied to the faces or fingers enrolled now (`kSecAccessControlBiometryCurrentSet`). The Secure Enclave enforces it as the item is read, and the system shows the prompt. The secret is not wrapped by a Secure Enclave key of its own. | Face ID or Touch ID, by the Keychain itself. |
| The browser | The PRF extension's output of a passkey made for the wallet, which only the passkey's user verification releases. A passkey may be synced by whoever provides it, and every device it reaches can release it: what is bound is the passkey, not this device. | WebAuthn: Touch ID, Windows Hello, a phone or a security key, whichever holds the passkey. |
| macOS | Nothing new. A wallet left with biometrics has Touch ID's check first, then the key read from the login Keychain. The app is unsigned, so it cannot tie an item to Touch ID: it is a check, not a lock. | LocalAuthentication. |

Whether a mode is offered is asked as Settings draws: a phone with no strong biometrics or none
enrolled, or a browser that says its passkeys have no PRF, does not offer them. A browser too old to
say finds out as the passkey is made, and the wallet stays as it was.

**Lockout means restoring from words.** Adding a fingerprint or face destroys the Android key that
opens a wallet needing biometrics; so do removing them all, turning off the screen lock, a factory
reset, a broken or lost phone, and restoring to another phone. On iOS, changing the faces or fingers
enrolled does it, and so on. Then nothing can open that wallet. So:

- **Before choosing such a mode,** Settings says exactly that, and you type yes to having your words
  written down, as many phrases as it has.
- **It refuses a wallet holding what words cannot bring back:** keys brought in, and swaps that keep
  their secret, each named. Move the funds and remove the keys, and finish and forget the swaps.
  Once it is on, such a wallet takes no key brought in and starts no swap, for the same reason;
  answering someone else's offer is fine.
- **Watch-only and MetaMask accounts** are allowed: they hold no keys here, and come back by pairing
  or connecting again, which it says. **A signer's history** is lost with the wallet, which it says.
- **When it happens,** the lock screen says the key is gone and why, and that Restore from words is
  the way back. Nothing is written: the wallet stays listed, and is never removed by itself.
- **A prompt turned down or failing is not a lockout:** try again. A browser cannot say a passkey is
  gone, so **It will not open** on the lock screen says what that looks like.
- **Removing it from the lock screen** takes a preview and a typed yes alone, as nothing else can be
  given by then. Anyone holding the unlocked device can do this: it costs you what your words bring
  back, and gives them nothing.

**Switching** proves what opens it now, makes the new key, checks that the new key gives back what
it was given, seals the vault anew under a new salt, opens that with the new factors in memory, and
only then replaces it. A failure anywhere leaves the old vault and its record as they were. Settings
are written at once, and the old key is forgotten only when settings saying so are read back as the
app next starts, so being killed at any step leaves a vault and the key it needs. A key made and
left behind by a kill is an orphan: nothing names it and it opens nothing. Changing the password of
a wallet that keeps biometrics as an alternative follows the same order: the new seal is written at
once, and the old biometric key is retired, not forgotten, until settings without it are on disk.

**An older build** on a phone or a desktop now finds no wallets at all, as they moved out of the file
it reads (see [What it remembers](#what-it-remembers)), and going back to this build finds them whole.
In the browser, where they did not move, it sees a wallet needing biometrics as asking for a password
that is always wrong, with biometrics it says were turned on elsewhere. It writes nothing, and keeps
the record as it saves its settings. The mode is carried by a tag at the start of the record it
already kept, `biometric-v1:` or `both-v1:`, rather than by a new field an older build's save would
drop.

**The lock screen** shows the prompt by itself when the app starts on Android, iOS and macOS, except
for a wallet needing both, which takes its password first. After **Lock** or the timeout it waits
for **Unlock with …**, as whoever locked it may be gone; in the browser, which shows a passkey's
prompt only right after a tap, it always waits.

**When the fingers or faces change** on a wallet left with biometrics as an alternative, they turn
off the next time they are used for it, the lock screen says so, and the password opens it. A check
that fails that way during a send locks the wallet.

**Things to know:**
- **A passkey stays with the browser or device** when a wallet stops using it: delete it there.
  Without the wrapped key the wallet kept, it opens nothing.
- **A browser or passkey without PRF** cannot keep the key; recent Chrome, Edge and Safari can,
  with the device's own passkeys. The page must be served over HTTPS, or from localhost. Making a
  passkey that turns out to have no PRF leaves that passkey behind.
- **Android builds without Java have no biometrics**, as they have no file picker: see below.
- **The iOS side has been compiled, and not yet run** on a phone or a Simulator.

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

**Fast resume:** opening the wallet walks every header since it last ran. Settings → Light client →
**Trust this node for fast resume**, per node and off by default, skips that once it is more than 8
epochs behind: it takes up the chain at the node's latest finalized checkpoint, and checks every
header and voter's signature from there on, against the genesis it pinned. What that gives up: a
dishonest node could show a false history from that point. Settings says so by the switch, and the
sidebar and Settings say **Fast-resumed at block N, on this node's word** when it did; on a phone, or
a phone on its side, Home says it in a line, and the dot on a tap. Under depth finality, or with no checkpoint to be had, it walks, as
with the switch off. See [In the wallet](light-client.md#in-the-wallet).

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

**New blocks:** where the node streams, the wallet holds `GET /stream` open and asks for headers as
soon as it says the chain moved, every 20 seconds besides; otherwise it asks every 3 seconds. It takes
nothing from the stream but that: see [Staying up to date](light-client.md#staying-up-to-date).

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

**EVM calls and logs in Activity.** A call's calldata and its logs read by name, by the ABI saved
for the contract on the Contracts page, or one built in (see [Signing screens](#contracts)), each
saying which read it; their bytes are kept with the proven transaction, and a click away. Only what
is short is kept, 16 KB of hex for a call and for a transaction's logs; past that a line says how
long it was. What was proven before bytes were kept shows as it did. An ERC-20's, ERC-721's or
ERC-1155's transfer counts as a token's does: the node's event index finds those that name the
account in their topics, which its address index cannot, and the light client proves them as any
other. An account's EVM address is its own 20 bytes for secp256k1 and an alias for anything else.
**A move from a contract the registry does not list is only shown as what its log claims,** as
"Says you received", with no plus sign. No move is ever added to a balance: those are what the
light client proves. With a filter on, the list is what the address index finds, so a token's
transfer naming the account only in a topic is not in it.

**What an unlisted token calls itself.** For such a claim's amount, the Activity page asks the node
for the contract's `symbol()` and `decimals()` by `eth_call`, and shows the answer labelled:
`1.5 "USDC" (unverified)`. A contract can say anything and nothing proves it, so this is for reading a
line and nothing else:

- **Only on the Activity page.** No signing screen, no balance, nothing sent or decided, and not the
  Overview's recent lines: what came back is private to the page. It adds no plus sign, and a move
  stays "Says you received" whatever it is called.
- **Hostile by default.** The symbol is cut to 16 characters and escaped, so it cannot break a line
  or run backwards, and it always sits in its quotes before `(unverified)`, on as many lines as it
  takes. One calling itself the chain's coin or a listed token looks like any other. Decimals above
  36, a symbol with nothing to see of it, or a failed or malformed answer leave the plain whole
  number. An NFT's and a multi-token's lines stay as they were, and a token the registry lists keeps
  its proven name.
- **Quiet and paced.** It asks only about lines on screen, the symbol first, one question every 400 ms,
  which leaves room under the 5 contract calls a second that nginx allows an address on `/rpc`. An
  answer is held per chain and contract until the wallet locks or closes, and never stored. What did
  not read is asked again after 5 minutes. A 429 waits 30 seconds and is no failure; a node that
  cannot answer, or has no `/rpc`, is left alone for 5 minutes. None of it shows on a page.
- **Never on a signer,** which has no such page and nothing to send a request by.

**Activity:** for each address of the account in use, the node's address index lists the
transactions that touched it (`GET /accounts/{address}/txs`), and the page merges them, newest
first, or oldest with the order turned round. Each line says which of the account's addresses it is
for, by its scheme: received to your Bitcoin, sent from your Ed25519, by your EdBlake. A transfer
from one of its addresses to another, of the coin, a token or an NFT, shows as **Between your
addresses**, from which to which, with what moved and no plus or minus, as the account holds as
much in all but the fee. **All**, **Waiting** (not proven yet) and **Failed**
narrow what shows, and the search finds a line by its id, a party, a contract, what it did or its
events. Only the chain the wallet follows now shows: each transaction kept
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

**Filters:** the **Filters** button opens a choice of kind, direction, token, counterparty and when,
says how many are on, and **Clear** turns them all off.
- **They run on the node** ([its filters](api.md#get-accountsaddresstxs)), so all the history counts,
  not only what is loaded. **Look further back** asks for the next page, and for what lay beyond the
  node's bound of 1,000 entries a request, which it says when it reaches.
- **Nothing shows as proven unproven.** What the node lists is where to look, as ever: each line
  stays **Checking** until its receipt and transaction check. The node picks what it lists, so a node
  could leave one out, and the page says so.
- **An older node ignores filters,** and its answer says nothing of them. Then what passes the same
  test here, among what the wallet has proven, shows, and the page says only what was read was
  searched. A node with no address index is the same, over what the wallet kept.
- **Counterparty** is what the node's index says a transaction touched, which a call's arguments
  count for; on an older node, here, what the wallet proved it moved and who sent it.

**Kept once proven.** What was proven is saved, so it shows again at once. What is not final yet
is proven again until it is.

**Older than what it kept.** The saved light client keeps headers only from the finalized block
on. A transaction in an older block it never proved, as one it meets for the first time after a
restart, is in no header it holds. So it fetches the headers between, a stretch of up to 1,000 at
a time, and proves each by its link to the one above (see
[Light client](light-client.md#saving-and-resuming)). Then the transaction checks, and is final.

## Read from checked blocks

Settings → **Read from checked blocks** has the wallet take what it shows from final blocks it
fetched and checked itself, rather than ask the node, so what it shows rests on those blocks and the
node learns less of what you look at. It is off by default, as reading blocks costs far more than
asking, and with it off nothing here happens at all. It is for the install rather than for a node or
a wallet: changing the node never quietly starts asking it about everything. It works alike on phones,
the desktop, in the browser and in the extension.

It is strict: **what cannot be proven this way is turned off and says why**, rather than shown with a
label. Every such page says the same thing: this needs asking the node, and Read from checked blocks
is on. The refusal is also in the wallet's one way out to the node, so a request from a path nobody
thought of still does not leave.

**What it does:**
- **It reads final blocks only,** from [`GET /blocks/range`](api.md#get-blocksrangefromhcountnentrye),
  each against what the light client checked, so nothing it read can be undone.
- **It starts at a height you pick.** A wallet made here starts where the chain stood when it was
  made. One restored or brought in asks. Read further back at any time: what it read stands, and it
  goes on from there. The Overview and Settings say from what block it knows, and Activity shows
  nothing older.
- **What is newest comes first.** A balance shows after its first proof, and history fills in going
  back. Older blocks are checked by their hash links up to a block the light client checked, a stretch
  at a time, and nothing of a stretch is kept until it has come whole and links up.
- **Balances and other state come from the proofs bundled with blocks.** One direct proof per account,
  at the final block it started at, sets where it starts from; what later blocks list updates it.
- **History comes from the blocks' receipts:** everything that names your addresses, a token's
  transfer in an EVM log's topics too. Filters and search on Activity work on what was read.
- **Your own sent transaction keeps one lookup,** its receipt proof, shown as *Included, not final*
  until the reading reaches it, when what the reading found takes over. It is checked, and the node
  already has that transaction.
- **It reads only while the wallet is unlocked and the app is open.** Its index is sealed with a key
  made from that wallet's vault key, so addresses and history in it are private. It is kept apart from
  the wallet's file, in the same private `wallet-store` folder on a device, written whole, and in
  IndexedDB in a browser, as localStorage holds about 5 MB. Losing it costs a rescan, never a wallet. One
  that will not open or read is set aside as `….unreadable-…` and read again, and the app says so. A
  password changed seals it anew with its next save.

**What it costs:** about 2 KB an empty block, so 34 MB a day on the devnet at 17,280 blocks a day.
Settings shows what has been downloaded and what it is up to.

**What it cannot do, in plain words:**
- **A gap.** A block that comes without proofs, because the node has none (`NoList`, `StatePruned`) or
  has only its header (`HeaderOnly`), counts as **unknown, not unchanged**. A value older than that
  block is not shown until one direct proof catches it up, which the wallet says, because the node sees
  that account; at most once in 5 minutes for each, so a node with no proofs at all is not told of it
  with every block. Nodes prune proofs after 10,000 blocks by default, about 14 hours on the devnet, and
  only `--archive` nodes prove older ones. Read far back on any other node and you get the old blocks
  and receipts, so history, but balances still rest on the proof at the top. A block without its
  receipts cannot speak for them either, and the count of those is shown.
- **Something left out.** A node can leave an entry out of a block's list, and nothing can tell: the
  balance stays proven but out of date. **Check now** proves each value directly, saying that the node
  sees each account, and if a proof shows the blocks left a change out, it says that too. Receipts
  cannot be left out this way.
- **Genesis.** What genesis allocated is in no block, so a start at height 0 takes its first proofs as
  any other does.
- **Tokens and names.** A token shows once a transaction moving it to or from this wallet has been
  read, so one held since before the height it reads from does not show. Its record, for its symbol and
  decimals, takes one direct proof, and so does its balance if it was first seen in a block older than
  the reading had reached. An ERC-20 shows in Activity as a log's claim, and as no balance: an EVM
  contract's storage is not one an entry can be looked up in. Names show once this wallet has been seen
  using them, as far as proven. Finding, pricing and registering one need asking, and so does paying one.
- **Fees.** `POST /offer` is off, so the review suggests a gas limit from what the same kind of
  transaction used lately in checked blocks, a quarter more, or a round guess that says it is one; and
  twice the newest checked header's base price, or what was seen paid if more. Both can be changed.
  Nothing tries it first, so a call that would fail still costs its fee. The coin and token transfers can
  be priced this way; what needs a dry run is turned off.
- **The coin's name.** Nothing the light client checks says it. It shows as a node last said on this
  device, kept for each chain, or else amounts show as plain units, not guessed at.
- **Privacy.** Reading ranges hides interest in transactions and state changes. The start and gap
  proofs do not, nor does who is asking, for which the [proxy and Tor](#through-a-proxy) settings
  remain. Sending goes through a node by nature. The settings keep what was sent until final and the
  account last used, unsealed, as they always did.
- **Node-served pages.** A page that came from a server can be replaced by that server: if the page
  comes from the node, the node could serve code faking these checks, and the setting says so there. The
  desktop and phone apps and the extension do not have this hole.

**A node without the route.** An older node answers 400 for `/blocks/range`. The wallet says so, stops
reading, and asks nothing else: it never falls back on asking. An answer that does not check stops the
reading the same way, keeps nothing of itself, and says what was wrong. 429 and 503 are waited out, as
long as `Retry-After` says, up to a minute, longer each time they go on: a node takes 2 ranges from one
client, and over an onion everyone counts as one.

**Fast resume is off** while this is on, as it rests on the node's word. A chain with no finality has no
final blocks to read, and the setting says so.

| Off, saying why | Works from blocks or proofs |
| --- | --- |
| Contracts, Batch, Swaps, Trade, Multisig, Mining and pool earnings, Governance, NFTs, Bridge, the faucet, what sites ask | Overview, Send (coin and tokens), Receive, Activity, Accounts, names seen |

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

**What the review reads a call as:** by what the light client proved the contract is, from the
chain's registries: the token registry's entry, or the collection registry's (`GET /collections`
says which collections to prove, `Column::Tokens` what each is). `transfer(to, n)` is a token's
amount only to a proven token. To a proven NFT collection, `transfer` or `safe_transfer(to, id)`
shows as **Send NFT #id**, with the collection, its id and the recipient, and no amount; to a
multi-token one, `transfer(to, id, n)` as **Send n of #id**, with how many. To a contract proven as
neither, the same call shows as the call it is, its numbers unnamed, and says the contract is
neither a token nor a collection the chain lists. Batch steps and the offline signer read calls the
same way; the offline signer knows only the token the sender names, so an NFT's transfer shows
there as a call to a contract it does not know.

**From:** each address holds its own coin, tokens and NFTs, so every page that sends has the same
From picker (`views/from.rs`): the account's addresses, each by its scheme, its address short and
the coin it holds. The pick holds across pages for the run: what was picked on Send is where Bridge,
Trade and Swaps start. A page that cannot use a scheme lists its addresses greyed, with why, and
says so under the picker. Batch starts from the pick too, but keeps its own once it has steps, as
changing its sender clears them.

**Dropped transactions:** one no block holds after 10 minutes shows as Not seen, and stops
holding its nonce, so the next transaction takes that nonce again.

### A batch

A signature holds on one chain alone, so Send signs nothing until the light client has said which
chain the node is on. Until then it says so: it never goes by a guess, or by the genesis pinned at the
node, which is what the node said last time.

The Batch page, in the sidebar and under More on a phone, builds a [batch](chain.md#batches): several
steps under one signature and one fee, all of which go through or none do, such as an approval and the
swap that spends it, or several people paid at once. Add any step a batch may hold, move each up or
down, take any out, up to 32; it shows the count, what they send in all, and where each deploy will
land, for the steps after it to call. **Review** goes to Send, as anything prepared does, so a
watch-only account goes on to the offline signer. Pick a multisig this wallet's accounts are in, or
one it follows, under Send as, and it goes to the Multisig page as a bundle for its members instead.

Every screen that signs one, Send's review, the offline signer, a multisig's bundle and its proposals,
puts the count and all they send in the coin first, then every step, numbered, in a frame of its own,
above the button. Nothing folds and nothing scrolls away apart from an EVM call's read, in its frame:
each read says what read it, and that a built-in ABI's is "A match, not proof". What a step says is
cut short and has anything that could lay out or hide text written as an escape, and fills the fields
of its own frame, so it cannot draw a step or a row of its own. Whatever contracts do past what the
steps say, a note says, and names' prices too: only the chain knows those.

A single transaction that is not a batch is laid out in the same frame: its kind, what it sends,
where to, and a call's data with the raw selector and size and the read below them, so a lone
contract or EVM call shows as much as a batch's step does, never a one-line summary that leaves its
value or recipient out. A multisig member reading a bundle or a proposal sees its inner transaction
the same way, with the fee the multisig pays and the chain the bundle is for.

**MetaMask cannot sign a batch.** It would show one opaque hash for all the steps, so the wallet says
so instead of asking, and the chain refuses one anyway: send it from an account this wallet or the
offline signer holds.

Activity lists a batch once, opened to each step and what came of it: done, or undone, failed and not
run. Which step failed is proven; why is the node's word, which the [explorer](explorer.md#pages) shows.

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

The **Multisig** page makes [multisigs](multisig.md#in-the-wallet) in plain steps, each member
needing only a key on this chain, of any scheme, Ed25519, EdBlake, Ethereum's or Bitcoin's, a
Bitcoin account key and an EVM address being optional, for one that reaches EVM chains, Bitcoin or
both. It lists every multisig any account in the wallet is a member of, from the node's member
index (`--index-multisigs`), beside those followed by address, each with its address whole, a Copy
button and its QR code; changes their members; and takes their transactions round the members: a
payment, or anything the Batch page makes, with its form, one step alone or several as a batch,
proposed and priced for its heaviest quorum, signed as whichever of the wallet's member keys is
picked under **Agree as**, one after another, or on the offline signer from a
`ur:chainv4-multisig` code, approved on chain through Send as the account picked, and submitted
once enough back it, under a key made for the moment, the multisig paying. The Batch page's Send
as lists those multisigs too. What the node says of a
multisig is shown as its word; nothing signed rests on it. The offline signer reads a bundle on
Sign and shows it back signed. For a multisig on Bitcoin it shows the descriptor and fresh
addresses, and with an Esplora API makes, combines and broadcasts its PSBTs; the offline signer
signs a multisig's Bitcoin input at m/48'/coin'/0'/2', and its Pair page shows each account's member
line, for whichever of its keys is picked. For one on EVM chains it shows its contract, and on a chain Settings reaches what it holds and
its owners; its calls are bundles signed with the account's EVM key or by the offline signer as
EIP-712 typed data, deployed and sent from the account. The Swaps page swaps as a followed
multisig too, its EVM locks going to the Multisig page as its contract's calls.

## Mining

The **Mining** page mines from the device, alone or in a [pool](consensus.md#mining-pools), and
claims what a pool owes the account's addresses. In a [store build](#app-store-builds), which has no
mining in it, it is called **Pool earnings**, and holds what is under
[Earnings and claims](#earnings-and-claims) alone.

### Mine

The **Mine** card at the top runs a worker of the node's mining port, as `chainv4 worker` does,
inside the wallet, paying the address picked.

- **Where:** the wallet asks `GET /mining` of the pool's node when **Pool's node** is set, else of
  its own. The port is at the address the node publishes (`public`), else at the node's own host
  with the port it names. **Other port** takes a `host:port` in its place, kept in settings as
  `mine_port`. A port open to anyone needs no token; a private one takes the token its operator
  hands its miners, typed into **Token** and kept in settings as `mine_token`: it is the port's
  shared secret, not a key of the wallet's, and proves only that this worker may mine there. A
  private port's worker connects over TCP, through the wallet's proxy when one is set. That proxy must
  be a `socks5h://` one: an HTTP proxy does not carry the port's protocol, and the wallet never goes
  around a proxy set, so with one it does not mine. A node that
  takes no miners answers 404, and the card says so.
- **A port open to anyone** goes over the node's `wss://`, through that proxy too, as its token is
  public and anyone on the way could otherwise change where the work pays, unseen. The certificate is
  checked against the host's name, against the roots built into the app, and nothing skips it. Plain
  TCP goes only where it cannot be read on the way: the node on this device, or at its onion through a
  proxy on this device, such as Orbot. A node at `http://` has no encrypted way to its open port, and
  **Start** says so rather than go anyway; a self-signed node's certificate is refused, with the reason
  under the card. **Other port** takes a `wss://…/mining/ws` too; without a token, a `host:port` is
  taken for an open one and refused the same.
- **Alone or in the pool:** alone, each block the device finds pays the address in full and
  nothing comes between blocks; in the pool, the pool pays the address its part of each block the
  pool finds, by the shares it sent, less the pool's fee, claimed below. Whichever the port does
  not take (`solo`, `pool` in `GET /mining`) cannot be picked; a port that takes neither mines
  for its node alone, and the card does not start on it. Kept as `mine_solo`.
- **On what:** the GPUs ticked, every one until one is unticked (`mine_gpu`, `mine_gpu_list`), or a
  number of CPU threads up to the device's (`mine_threads`). A GPU that OpenGL reaches too is listed
  twice, as `Mali-G715 (Vulkan)` and `Mali-G715 (Gl)`: ticking both mines on both, which together hash
  faster. With every one ticked, a GPU the machine gains later is mined on too; with none, Start says so.
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
  port's protocol unchanged, so the handshake and the MACs stay between the wallet and the port. A
  port open to anyone takes `ws://` only on this device or at an onion, and says why not otherwise.
  **Other port** does not show there. See [Mining in the browser](#mining-in-the-browser).

Whoever mines, the node checks every nonce before it seals a block with it; the wallet signs
nothing to mine. For a rig, **Mining from other machines** gives the worker's command with the
node and the address filled in, `chainv4 worker --node https://NODE --payout ADDRESS` for a port open
to anyone and `--node HOST:PORT` otherwise, with `--solo` to mine alone and `--token FILE` for a private
port.

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
  yet. A pool credits what it owes once its blocks are final, so earnings show a while after the
  blocks are found.
- **Kept claims:** each claim the pool's node gives is kept on the device, by chain, in settings as
  `pool_claims` (the newest few, the next to settle, and the newest settled: see
  [Mining pools](consensus.md#mining-pools)). When the pool's node does not answer, **Kept claims**
  checks them at the wallet's own node, `GET /claims/{pool}/{address}?height=`, newest first, and
  **Claim** sends the first whose root is still on chain: all the address earned up to it, less
  what it claimed.
- **History** shows a claim as "Claimed pool earnings for" the miner: the transaction names all the
  miner earned, not what the claim paid.

## Governance

The **Governance** page (`wallet/src/views/governance.rs`, with its form in `governance/form.rs`
and `governance/schema.rs`) covers the chain's settings under [governance](governance.md): who
decides, the changes open and their votes, voting Yes, No or Abstain or clearing a vote, proposing
a change, what is in force and every decision. It reads
[`GET /governance`](api.md#get-governance),
[`GET /governance/history`](api.md#get-governancehistorybeforevlimitn) and
[`GET /governance/weight/{address}`](api.md#get-governanceweightaddress), asking again every
15 seconds while the page is open. On a chain without governance (404) it says so and shows no
actions.

- **Who decides:** the council's members, or finality's voters for an electorate. The card also
  shows what Yes needs, the quorum (a share of every voter's weight that must vote at all,
  Abstain included, before Yes decides), when a change is rejected (once what has not voted No can
  no longer reach what it needs), how many epochs a vote counts for, the epoch, the number of
  decisions so far of every kind (what the history counts), and each kind's own version. Each kind
  of change has a ballot and a version of its own: consensus (which also covers finality's tuning
  and switching finality), bootnodes, names, collections, token functions, federation, directory, fees,
  reward, limits, upgrade, name and governance. A vote names the version of its change's kind,
  from `versions` in `GET /governance`. Under it, each of the wallet's addresses, and each
  multisig it follows, shows its weight: **may vote** with what it weighs, or **may not vote**. It
  also lists the address's votes that count, each with its way, the kind and the kind's version,
  and its own **Clear** for an address this page sends for. A council member weighs one vote;
  under an electorate, FFG weighs stake and authority finality weighs signers.
- **Open changes:** a search box over each change's kind, words and JSON and who voted on it, an
  order (newest vote, most support, by how far Yes got toward what it needs, or closing soonest,
  by when the oldest vote stops counting), and a filter by the kinds there are; a list of one has
  the search box alone. Each change is a card, the explorer's own (`chainv4_ui::votes::card`):
  - a **ring** toward what Yes needs, and inside it a thin one toward the quorum when there is one;
  - the change in words, its kind and that kind's version, where it stands (`1 more vote
    needed`, `Passes with the next vote counted`, or in red `Can no longer pass: too much has
    voted No`), what Yes, No and Abstain weigh, and when the oldest votes stop counting, in blocks
    and time;
  - a **dot a voter**, Yes filled, No a cross, Abstain half-filled, not voted empty, the wallet's
    own addresses named by their account, `Main (yours)`;
  - **What it moves:** the setting in force beside what the change would make it, each field that
    moves struck through and highlighted, with how many stay;
  - its JSON folded below.

  An address of the account in use, or a followed multisig, that may vote gets **Yes**, **No** and
  **Abstain** on each change, the way it voted marked `Voted Yes` in place of its button, and
  **Clear** once it has voted. A new vote replaces its old one on that change. An address backs
  one change of each kind with Yes at a time, so it can back a fee change and a rename at once, and
  vote No on any number: a Yes on another change of the same kind moves its Yes there, and leaves
  its other votes alone. The confirm screen says so, and that the vote lapses only once a change of
  its own kind is decided first and that kind's version moves on: a decision of another kind
  leaves it standing. To vote with another account's address, switch to that account.
- **Propose:** pick a kind, and each setting of it has a field of its own, with what stands beside
  it (`now 50%`), and **Put back** once it differs: numbers, amounts typed in the coin, sizes as
  `32 MiB`, addresses, on or off with its number (a reward's halving, a block's size, the leak), a
  choice for what picks other fields (the consensus's type, the gas schedule, the pricing), and
  lists with **Add** and **Remove** (bootnodes, name prices by length, collections' functions, the
  federation's signers, the directory's EVM gateways, a switch's signers). What the chain keeps
  through a change of finality shows as staying as it is: the kind, `checkpoint_interval`,
  `activation_delay`, the starting validators or signers and the thresholds.
  - **Only what changed is sent:** the fields that differ are laid over what stands with chainv4's
    `params::json::over`, as the CLI's `vote-change` does; a variant of another type, such as
    bids for a base price, replaces its object whole, as its old fields mean nothing to it.
  - **What is wrong shows under its field, in words,** before anything is sent: the form reads each
    field, runs the whole through chainv4's `change_from_json`, the same checks as `vote-change`,
    and puts each error under the field it is about. What a node checks against the chain and
    only it can read there is asked of the settings it lists: a consensus against the finality, a
    switch's epoch, kind and signers, a retune's most validators or signers, and that no switch of
    finality is on its way. The chain has the last word, as the vote's version names what it was
    laid over: should a change of that kind be decided first, the vote fails.
  - **Advanced**, folded under the form, is the kind's JSON, filled with what the form says, for
    what a form does not cover; while it is open it is what is proposed, and it still takes a part
    of fees, reward, limits, finality, federation or directory.
  - **Review** opens the confirm screen with each field that moves, `old → new`, the change in
    words and, folded, its whole JSON. A change is proposed by voting Yes on it. Voting on someone
    else's change shows the same list against what stands.
- **Sending:** from the confirm screen the transaction, `Tx::VoteChange { version, change, vote }`
  at the version of `change.kind()`, or `Tx::ClearVote { version, change }` at the version the
  vote named (from `voted` in `GET /governance/weight/{address}`), goes to Send like any other:
  priced, signed with the account's key, a hardware wallet or the offline signer, and sent. A
  followed multisig's goes to the Multisig page as a bundle for its members.
- **Your votes:** the account's votes and cleared votes whose receipts the light client proved,
  each Final or Proven. A vote that decided its change shows **Decided change N · kind vM**: its
  receipt's committed outcome is `Decided { decision: N, kind, version: M }`, N the chain's count
  of decisions and M the version of its kind it made. One that left its change unable to pass
  shows **Rejected a kind change**, from `Rejected { rejection, kind, version }`. History says the
  same, "Decided change N · Voted Yes on …, making kind vM" or "Rejected a kind change, the
  chain's rejection N · Voted No on …"; a cleared vote reads "Cleared a vote on a kind change from
  vM". Each vote the wallet keeps records its kind and way; one kept before votes named their kind
  reads as consensus, and an approval kept before votes could be No reads as a Yes.
- **In force:** every setting, governance's own included, in words, each marked "genesis" or
  "decided in change N".
- **Decisions:** every one, ten to a page with **Newer page** and **Older page**, each with its
  block and who sent it; a search box over its kind, words, JSON, sender, transaction and block, a
  filter by kind, and newest or oldest first. Older pages of `GET /governance/history` are read as
  a page comes to need them, so a search goes on through all of them, and says how far it got.
  Rejections, and the timeline and turnout, are the explorer's.
- **Trust:** the page shows what the node says. Nothing signed rests on it but the version, the
  change's kind's, which the chain checks: a vote naming a version that has moved on fails and
  changes nothing.

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

**Signing screens.** The offline signer, an EVM chain's review, a contract's review and a site's
request read their call by name too: by an ABI saved here or one built in, as the signer talks to no
node. What is read is a match, and says so; it is untrusted, so it is cut short and escaped, and
goes below To, the value and the data's selector and size, which stay as they were, in a frame that
takes no more of the screen than it may. What does not read leaves just those, with the raw data a
click away. Amounts go in a token's decimals only on this chain, for a token the registry lists.

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
   offer, the key to sign with, how the node names the coin and the token, and the chain it is for,
   by its genesis id, which the light client follows. A wallet that does not know the chain yet
   says so and signs nothing.
3. On the signer, **Sign** reads it and lays out what it signs: the chain, by the id **Your chain**
   takes and the genesis id, the amount in the asset's units and as the raw units signed, from, to,
   the token's contract, the most the fee comes to, the nonce, and everything the transaction holds.
   The signer follows no chain, so the chain is the sender's word, and a hostile sender may name
   another of the same keys: with **Your chain** set under Settings, it refuses any other. It
   refuses a key none of its accounts holds, and a transaction the chain would refuse. **Sign**
   shows the signed transaction as a code.
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

**What it signed stays in a history,** on the Sign page under **Signed here**, folded until asked for: what each thing did, its kind, who signed it and when, by this device's clock. It is sealed in the wallet's vault with its keys, so it opens with what opens them, and only with a password if the wallet has one. It keeps the last 200, and holds no key or signature, nor what a message or typed data said. Settings turns it off, which keeps what is there until cleared, and clears it, asking first. It records what this signer signed, not whether it landed: the sender is who proves that.

**Keep the signer offline:** airplane mode, Wi-Fi and Bluetooth off. The live camera keeps
nothing: each frame is searched in memory and dropped.

A batch's request carries the transaction as any other's does, so the signer shows every step as
Send's review does, but it knows no chain's tokens or hash: a token's amount shows in the whole units
signed, and it does not say where a deploy lands. A batch the chain would refuse says so as soon as it
is read. A signer older than batches says "a code that does not decode", and signs nothing.

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
as exactly what they say. With several phrases in the wallet, a request naming neither a
fingerprint nor an address is refused too, as nothing says which of them it means.

**Tested against MetaMask's own libraries:** `wallet/src/hardware/metamask/` makes requests with
Keystone's `@keystonehq/bc-ur-registry-eth` and ethereumjs, as MetaMask's QR keyring does, into
`metamask.json`, which the wallet's tests read and sign; and checks the wallet's pairing code and
signatures back with them: the accounts the extended key derives, and each signature rebuilt into
its transaction or recovered from its hash, under its request's id.

### Trade

The **Trade** page trades on the chain's [DEXes](dex.md), the native one and, when the node names
its router, the EVM's, switched with **Native** and **EVM** beside the page's two tabs:
- **Trade:** what you pay over what you get, with the balance of each and a button between them
  that swaps them; side by side on a phone on its side. Under them the rate, price impact, fee,
  minimum received and route of the best route, through one pool or two; slippage, offered or
  typed; and one button, which says what it waits for until it can trade.
- **Pools:** what each holds, one opening to its price, a native pool's price chart from its `Sync`
  events, and adding to it or taking your shares out; under them, folded, **Open a new pool**, at
  the price its first deposit sets.

Approving a token, trading, adding and taking out are calls the Send page reviews and signs,
from the address the From picker names: what it holds of each asset and each pool's shares is what
shows, asked again once another is picked.

### NFTs

The **NFTs** page shows what the account holds of the chain's [collections](nft-standard.md):
- **Where to look:** the node's collection registry, `GET /collections`, and each collection's
  `tokens_of` for each of the account's addresses, of every scheme, the first 100 of each. An
  address whose list did not come, as when the node turned away too many asks at once, is named in
  a warning, and what it listed before stands until it comes.
- **What it proves:** each NFT's owner, or each multi-token amount at every one of the account's
  addresses, against the tip, by the standards' [provable ownership](nft-standard.md#provable-ownership).
  An NFT shows at whichever of the account's addresses its proof names, compared as an address,
  whichever of them listed it: one sent from one of the account's addresses to another shows at the
  one it went to as soon as its proof says so, before that address's own list does. A proof answered
  late, from a block older than one already taken, is not taken.
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
- **From:** the address that sends, receives and has its deposit address shown, picked as on Send:
  any scheme. What comes from an EVM chain is still signed there with the account's Ethereum key,
  which the page names.
- **To an EVM chain:** the coin, a token, an NFT or a multi-token amount, to an address there:
  letting the gateway take it, then sending, calls the Send page signs. The account's transfers
  out, among the latest 20, show the board's count of signatures and whether Gateway.sol took
  them; one signed and not taken is delivered from here, with the account's Ethereum key, priced
  and signed as the Swaps page signs there. One the federation [voided](bridges.md#refunding-a-transfer-refused-for-good)
  says so, and refunded once it was; those are the federation's own steps, so it offers no delivery.
- **From an EVM chain:** letting Gateway.sol take it, then sending, signed there with the
  account's Ethereum key, or by the offline signer. It arrives as the federation signs it.
- **Bitcoin:** the picked address's deposit address, with its QR code, computed here from the
  federation, the chain's id and the gateway the directory names, so it holds on this chain alone.
  Each of the account's addresses has its own, and what is paid to it is minted to that address.
  The light client watches every address's deposit address, of every account, whichever is picked.
  A payment to it, found by the wallet's own [Bitcoin light client](#bitcoin-followed-by-the-wallet-itself)
  and deep enough, is minted as wBTC by its proof from the block it came in; without the light
  client, by Esplora's proof of a transaction typed in. wBTC is withdrawn to a `bc1q…` address or
  an output script nodes relay, from 10,000 satoshis: it says what you get, the amount up to a tenth
  less, as a release's fee comes off the withdrawals it pays. There is nothing more to do: the
  federation pays it out in a release of its own, usually within the hour, longer while fees run past
  what it agreed to lose. The account's withdrawals show queued, proposed or paid, and what each lost.

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

It signs the inputs of its own seeds' keys at m/84'/0'/0'/{0,1}/n or m/84'/1'/0'/{0,1}/n that
spend native segwit (P2WPKH) coins, or a [swap](swaps.md)'s HTLC whose script the PSBT gives
(claim or refund, named on the review), with BIP143's hash of all of the transaction, SIGHASH_ALL,
and gives back every pair the PSBT held. With several phrases in the wallet, each signs the inputs
it holds, and change back to any of them shows as change. It refuses:

- a PSBT with nothing of any of its seeds' to sign, as from a wallet paired with another;
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
- **History** shows a vote on an upgrade as "Voted Yes on an upgrade: nodes older than 0.2.0 stop
  at block 150000".

## The chain's name

The lock screen, the sidebar, the bar on top and the window's title (the browser's tab) name the
network as `GET /status` names it, such as "chainv4 testnet" and "chainv4 testnet wallet". The
wallet keeps the last name it saw for each genesis, so the lock screen shows it before the node
answers. With no name known, from no answer yet, a chain with none (`null`) or a node from before
names, it says "chainv4", and so does an offline signer. A name a chain could not have is taken
as none. The wallet's own name stays chainv4 wherever it means the software: commands such as
`chainv4 multisig`, releases and updates, and what it calls itself to MetaMask and sites.
**History** shows a vote on a governance rename as "Voted Yes on renaming the chain to chainv4
mainnet".

## When a request fails

Every request has a time limit: 30 seconds, or 90 through a proxy, as Tor builds a circuit first. A
node that never answers ends in a failure, not a page that waits, however it fails to answer: the
node itself, or a SOCKS or HTTP proxy on the way.

**Retry:** what failed or took too long shows at the top of every page, in a sentence, with a
**Retry** button, the same widget as the explorer's. It stays until what failed is answered, or
nothing asks for it any more. A page goes on asking by itself too, a little less often each time, up
to every 30 seconds. Retry goes past that. What you sent, such as a transaction, is never sent again
but by you.

**Reload:** on a phone, pull a page down from its very top: a circle comes down over it, and letting
go past the mark reloads. It never fights scrolling: a finger that starts lower down, goes up or
sideways first, or takes a slider, is not a pull. With a mouse, **Reload** is in the sidebar's foot, or
at the top of the page in a narrow window. Either asks the node again for what the page shows and for
its status, spins while it works, and leaves what shows until the answers come. Reloading the light
client's own is what it does anyway: what it follows and proves is asked again at once.

## What it remembers

On a phone or a desktop, each wallet has a file of its own and the rest has one more, in a private
`wallet-store` folder, saved every 30 seconds if anything changed, on exit, when Android sends the
app to the background, and at once when what opens a wallet changes. In the browser all of it is in
localStorage under one key, `chainv4-wallet`. Either way it holds:
- the node, the proxy, and the genesis pinned at each node;
- the nodes trusted for [fast resume](#trust);
- the mining pool's node, for a pool on another node;
- each wallet: its name, its vault, and the account in use in it;
- the light client's state, by genesis, to resume from the finalized block, not genesis;
- the chain's name its node last said, by genesis, for the titles before the node answers;
- what it sent that is not final yet, and what it proved, the newest 2,000;
- what the coin is called on each chain, as a node last said, for [reading checked blocks](#read-from-checked-blocks);
- the wallet last opened, the lock time, and the theme.

**A kill never loses a wallet.** Each file is written beside itself, flushed, read back, and only
then renamed into place, so a kill or a lost battery leaves it as it was or as it became, and only
what changed is written. Each has a copy, written just after and holding the same, for when the
first will not read: never what it held before, or a password changed would still open the copy. A
wallet whose files hold bytes that do not parse is set aside as `….unreadable-…`, never written
over, and the app says so across the top while every other wallet opens. A wallet whose files cannot
be read at all right now, a lock or a permission rather than damage, is left exactly in place and
read again next start, as its bytes may be good; the app says that too. So do settings that will not
read, which cost no wallet. A failed save, such as on a full disk, says so too, and is tried again,
and in the browser a save that storage would not take is reported rather than lost in this tab. A
wallet removed takes its files with it, even ones a kill had left this run never wrote, so it cannot
come back on a later start.

**From older builds.** An older build kept everything in eframe's one file. The first start moves it
all, and takes the wallets out of that file only once their own are written. An older build run
afterwards finds no wallets and leaves them alone; any wallet it makes is added to the list when the
newer build starts again.

Only the vaults are secret. What the wallet read from checked blocks is kept apart from all this, in
an index of its own sealed with its wallet's key: see [Read from checked blocks](#read-from-checked-blocks).
Removing a wallet removes its indexes.

## Look

It follows the look of the wiki every build writes (see [Documenting code](development.md#documenting-code)), quieter.
Its palette, fonts and widgets are the `ui` crate's, shared with the explorer; only its roomy spacing
is its own (see [The ui crate](development.md#the-ui-crate)).
- **Colors:** the wiki's palette, dark or light.
- **Type:** Manrope for text, JetBrains Mono for addresses and hashes, and Newsreader for titles
  and the big numbers. The fonts and their licences are in `ui/assets/fonts`.
- **The status dot,** on a phone's top bar and a short sidebar's foot, is the light client's state,
  green following, amber catching up, grey starting, and turns red and hollow, reading **Node not
  answering**, while the node's status goes unanswered, or **Stopped** when what the node sent does
  not check. A tap says it in words: why, what shows, and any fast resume.
- **Layout:** each page is a column of cards beside a sidebar with the account, the pages, and
  the light client's state. In a window too short for them all, the pages scroll, the light
  client's state, Lock and the theme staying at the sidebar's bottom
  (`chainv4_ui::layout::sidebar`), and in one too short even for those, as a phone on its side
  with the keyboard up, the whole sidebar scrolls; in one under 560 points tall, as a phone on its side, the
  sidebar's heading closes up and its foot folds to one line, the light client's dot and word beside
  the theme and Lock, to leave the pages more room. On a touch screen, buttons and fields
  are a finger tall, 40 points, and the field being typed in scrolls into sight above the
  keyboard. Below 640 points wide, as on a phone, the sidebar gives way to a bar
  on top with the account, the status dot and Lock, and five tabs along the bottom, evenly
  spaced and always in the same place: Home, Send, Receive, Activity and More. More lists every
  other page, each with a line on what it is for, and its tab stays marked while one of them shows.
  An offline signer, with five pages in all, has them as its tabs and no More. The sidebar, on a
  wide screen, lists all the pages. A card's row puts its label above what it says on a narrow
  screen, and text wraps everywhere, in rows too, so nothing is cut off at the screen's edge. A row
  with a button or a value at its end keeps that where it is, however long the text beside it
  (`chainv4_ui::widgets::end_row`). Cards stand one gap apart on every page, `layout::SECTION`,
  which the wallet's `theme::card` keeps below each. Android's back gesture closes a view the page
  opened, such as a swap, then returns through the pages left, each once, scrolled to where it
  was left, so from a page under More to More, then Home; on the first, the app goes to the
  background, as Android apps do. A page opened anew starts at its top.

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

### App store builds

Google Play and the App Store take no app that mines, so `mining`, on by default for direct APKs and
the web, is a feature of its own, and `store` is every other default feature:

```sh
wallet/build-android.sh --store            # wallet/target/android/chainv4-wallet-arm64-store.apk
wallet/build-ios.sh --store --ipa          # for an iPhone, or --sim for the Simulator

# What CI and a change to the wallet should check, from wallet/:
cargo clippy --all-targets --locked --no-default-features --features store -- -D warnings
cargo test --locked --no-default-features --features store
cargo tree --locked --no-default-features --features store --target aarch64-linux-android -e normal | grep -E 'wgpu|chainv4-gpu'   # nothing
```

- **Gone without it:** the Mine card and what drives it (`views/mine.rs`, `src/miner*`, the
  `miner-worker` bin), the `chainv4-gpu` and `wgpu` dependencies, `chainv4`'s `worker` and `gpu-mining`,
  and the command for mining from other machines.
- **Kept:** what a pool owes and claiming it, as claiming is not mining. The page is called **Pool
  earnings**, and its text and More's say nothing of mining; nor does it list workers or shares.
- **Settings:** the same in every build. The mining settings are kept in one that does not read them,
  so settings from either load in the other, and what was chosen comes back.
- **iOS:** a store build still links `wgpu`, as eframe draws through it there. It draws, and mines
  nothing; `chainv4-gpu` is gone as everywhere.
- **The web** the nodes serve stays with mining on: no store is involved. `index.html` builds the
  `miner-worker` bin, which a build without the feature does not have.
- **The browser extension** has a store build too, as the Chrome Web Store and Firefox Add-ons ban
  miners: see [Browser extension](#browser-extension).

### One app, three starts

The app is a library (`src/lib.rs`), as Android loads it, so the
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
wallet/build-extension.sh --store   # for the stores: chainv4-wallet-chrome-store.zip and -firefox-store.zip
```

**For the stores, use `--store`.** Both ban extensions with a miner, and a plain build has one. This
builds with the `store` feature alone and without the `miner-worker` page entry, and it fails if
anything of a miner's reaches the output. Each store signs what it publishes, so no key of your own is
needed. Mozilla also wants the source and build instructions for the wasm.

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
- **Its default node** is the public devnet's, `https://chain.homepage.gs`, as no node serves the extension's page; name
  another under Settings.
- **Storage** is the extension's own `localStorage`, apart from the web app's at any node: wallets
  made in one are not in the other, and removing the extension deletes its wallets. Keep the words.

**Biometrics** are a passkey's here too, made for the extension's own origin. Firefox refuses
WebAuthn to extension pages ("The operation is insecure"), so in its extension the wallet offers
no biometrics and the password stays; Chrome's may allow them. A browser that refuses them anywhere
else gets the same plain message rather than its own.

**CI:** `.github/workflows/extension.yml` runs the extension's script tests
(`node --test wallet/extension/test.mjs`), builds both, plain and for the stores, when started by hand, keeps the zips as
artifacts, and attaches them to a `v*` tag's release when started on that tag.

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
wallet/build-android.sh --store            # what an app store takes: see App store builds
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
explorer's, when started by hand (`workflow_dispatch`), and keeps them as artifacts. It
signs with the `ANDROID_SIGNING_PEM` secret, holding `signing.pem`'s text, when there is one, and
with the debug key otherwise. Started on a `v*` tag, it attaches all four to its release.

### Biometrics on Android

`ui/android/Biometric.java` is compiled in with the picker, and the manifest asks for
`USE_BIOMETRIC`, granted as the app is installed. It needs Android 9: BiometricPrompt is 28's. The
app asks it whether strong biometrics can be used as it starts and every few seconds after, so a
finger enrolled while it runs is noticed. The Keystore key is AES-256-GCM, made with
`setUserAuthenticationRequired` and `setInvalidatedByBiometricEnrollment`, and on Android 11 and up
`AUTH_BIOMETRIC_STRONG` for each use. It is checked to be inside secure hardware as it is made, and
none is kept if not, so an emulator with a software Keystore may refuse them where a phone does not.

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
- **The bars' icons:** dark on the light theme and light on the dark one, set by the app (`ui/android/Bars.java`), as the manifest's theme follows the phone's setting rather than the app's.
- **The bars:** the app draws behind the status bar, the navigation bar and the keyboard, and keeps
  its content in the part Android says can be seen.
- **Storage:** it saves when the app goes to the background, and every 30 seconds.
- **Copying** goes to Android's clipboard, through its ClipboardManager, which the `ui` crate calls
  over JNI: egui reaches no clipboard there. Android shows what was copied.
- **Pasting:** hold a finger on a text field, half a second, or tap one that already has focus,
  for a bar with **Paste** and **Select all**, and **Copy** and **Cut** once some text is selected;
  a drag across a field's text brings it too. It never comes on the tap that gives a field focus,
  and it goes as soon as typing starts. It goes just above the field, else just below it, kept within what can be seen, clear of the keyboard, the bottom tabs and the system's
  bars, and in the text's color so that it stands out on any page. The keyboard's own Paste button
  pastes too, and Ctrl+C, X, V and A work from a keyboard that has Ctrl.
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
wallet/build-ios.sh --store --ipa          # what an app store takes: see App store builds
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
- **Face ID or Touch ID** keep what a wallet needing them opens with in a Keychain item tied to the
  faces or fingers enrolled now, for this device alone, and say which the phone has (see
  [Biometrics](#biometrics)). What iOS kept before stays readable as it was: behind
  LocalAuthentication, as Touch ID does on a Mac.

## What to keep in mind

- **Keep the words.** They bring every account made from them back, on any device; with several
  phrases in a wallet, keep every one. Without them, a lost device or a forgotten password loses
  the wallet: nothing opens a vault without what seals it, and biometrics of the older kind keep
  only what the password opens, so they are no way back from a forgotten one. A wallet that needs
  biometrics has only its words to go back to when they are lost.
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
