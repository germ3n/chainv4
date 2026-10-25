# Wallet

The wallet (`wallet/`) is a light wallet, built with egui, for the desktop, the browser and
Android:
- **Keys:** wallets of accounts from a seed phrase, each sealed with a password on the device.
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

## Wallets and words

**A wallet** is a seed phrase, the accounts made from it, and a password that seals them. The lock
screen (`wallet/src/lock.rs`) makes the first, and holds them all:
- **New wallet:** a name, a password twice, and under Advanced an optional BIP39 passphrase. The
  wallet makes 12 words and shows them once, to write down, then asks for three of them back
  before it opens.
- **Restore from words:** 12 or 24 words written down before, from this wallet or any that
  follows BIP39, with their passphrase if they had one.
- **Several wallets:** "Add a wallet" makes another. With more than one, the lock screen asks
  which to unlock; each has its own password. The one last opened comes first.

**An account, three addresses.** Account *n* of the words has a key in every scheme, derived as
[Crypto](crypto.md#seed-phrases) says: Ed25519 and EdBlake by SLIP-0010, and secp256k1 at
Ethereum's `m/44'/60'/0'/0/n`. So its Ethereum address is the one MetaMask shows for the same
words and account. `chainv4 keygen --from-phrase --account n` gives the same keys.

**Settings** shows the words, once the wallet's password is typed again, renames the wallet, and
removes it, also after the password. A wallet from before seed phrases has none: Settings makes
new words for it or takes words you have, and from then on new accounts come from them.

## Keys

**The vault.** Each wallet's words and accounts are kept in app storage, sealed with its password,
at least 8 characters (`wallet/src/vault.rs`):
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

**Locking:** the Lock button, or 15 minutes without input. Settings changes the time, or turns it
off.

**Accounts** (`wallet/src/views/accounts.rs`, `wallet/src/keys.rs`):
- **New account:** the words' next account, the lowest number the wallet does not hold, with its
  three addresses. A wallet without words makes one random key instead, in the scheme picked.
- **Open key file / paste a key:** a key file as `chainv4 keygen` writes it, `ed25519:0x…`,
  `edblake:0x…` or `secp256k1:0x…`, picked or pasted. An account brought in has that one key, and
  one address, named after the file when no name is typed.
- **Development key:** the dev chains' well-known key, which holds their genesis coin. Anyone can
  spend from it.
- **Use, rename, export and remove.** Export asks first, then shows each key's text to copy;
  leaving the page hides them again. Removing an account from the words only takes it off the
  list: the next New account brings it back. Removing a key brought in, with no copy of it, puts
  what it holds out of reach for good.

**From before seed phrases:** a vault the wallet sealed before it had words opens as a wallet
named Wallet 1, its accounts keys brought in.

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
first. The wallet proves each one with
`GET /proofs/receipts/{id}`:
- **Who sent what:** the signed transaction, against its block's `tx_root`, with `check_tx`.
- **What it did:** its receipt, against the `receipts_root`, with `check_receipt`.

From those it works out what moved for the account (`wallet/src/history.rs`):
- the coin, from a transfer, a mint, a burn or a call's value;
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

The Receive page shows one of the account's addresses, picked by its scheme, to copy and as a QR
code, dark on white in either theme so any camera reads it, and lists them all below. The coin and
every token arrive at any of them; each keeps its own balance.

**Copying an address:** click or tap it, on any page, and it is copied, with **Copied** beside it
for a moment. Hovering over a shortened one shows it whole. Receive's **Copy address** button
copies the one shown.

## What it remembers

Everything is in app storage under one key, `chainv4-wallet`, saved by eframe every 30 seconds,
on exit, and when Android sends the app to the background:
- the node, and the genesis pinned at each node;
- each wallet: its name, its vault, and the account in use in it;
- the light client's state, by genesis, to resume from the finalized block, not genesis;
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
  the light client's state. Below 640 points wide, as on a phone, the sidebar gives way to a bar
  on top with the account, the light client's dot and Lock, and tabs along the bottom.

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

**One app, three starts.** The app is a library (`src/lib.rs`), as Android loads it, so the
package is `chainv4-wallet`. Its binary, `wallet` (`src/main.rs`), starts it on the desktop and in
the browser. Android starts the library at `android_main`, which hands it to the `ui` crate's
`android::run`.

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

## What to keep in mind

- **Keep the words.** They bring every account back, on any device. Without them, a lost device
  or a forgotten password loses the wallet: nothing opens a vault without its password.
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
