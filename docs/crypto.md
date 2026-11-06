# Crypto and encoding

## Hashes

Every hash algorithm sits behind a cargo feature of its own, all on by default:

| `HashType` | Feature | Secure | Used for |
| --- | --- | --- | --- |
| `Sha256` | `sha256` | yes | chain or work hash |
| `Blake3_256` | `blake3` | yes | chain or work hash; the dev chains' default |
| `Blake2s256` | `blake2s` | yes | chain or work hash |
| `Blake2b256` | `blake2b` | yes | chain or work hash |
| `Keccak256` | `keccak256` | yes | chain or work hash |
| `Xxh3_64` | `xxh3` | **no** | contract function selectors only |

- **The chain hash,** genesis's `hash`, is what block and transaction ids are hashed with. It also
  hashes the roots blocks commit to, contract addresses and gossip ids.
- **The work hash,** the proof-of-work config's `hash`, is chosen apart from it.
- **Both must be secure.** Genesis refuses XXH3 for either: it is fast but not cryptographic, so
  it only names contract functions.

Without a secure hash compiled, the `vm` and `node` features fail to build. `HashType::default_chain`
is BLAKE3 when built in, and otherwise the first secure one.

**A `Hash` carries its algorithm.** A block header hashes itself with its parent's algorithm,
which genesis fixes through its zero parent. So ids from chains with different hashes never
connect.

**`Hasher` is streaming.** Values hash as they encode (`Hashable`), so nothing large sits in one
buffer just to be hashed.

## Keys and signatures

Key schemes sit behind features too:

| `KeyType` | Feature | What it is |
| --- | --- | --- |
| `Ed25519` | `ed25519` | Standard Ed25519 (RFC 8032), hashing with SHA-512. |
| `EdBlake` | `edblake` | Ed25519 hashing with BLAKE3 read out to 64 bytes instead of SHA-512, matching chainv3's keys and signatures. |
| `Secp256k1` | `secp256k1` | Ethereum's: ECDSA on secp256k1 over the keccak256 of what is signed, deterministic (RFC 6979). |
| `Bitcoin` | `bitcoin` | Bitcoin's: ECDSA on secp256k1 over the double SHA-256 of what is signed, deterministic (RFC 6979). Needs `secp256k1`'s code, so it turns that feature on. |

- **Addresses:** an Edwards key's address is its scheme and its 32 bytes, such as `ed25519:0x…`.
  A secp256k1 key's is Ethereum's: the last 20 bytes of the keccak256 of its uncompressed point,
  as `secp256k1:0x` and 40 hex digits. The same key has the same `0x…` address here as on
  Ethereum. A bitcoin key's is Bitcoin's native segwit (P2WPKH) one: the HASH160 of its
  compressed key, RIPEMD-160 over SHA-256, written alone in bech32 as `bc1q…`, which carries its
  own kind and checksum. Every chain, dev or main, writes it with mainnet's `bc`, so the same key
  has the same `bc1q…` address here as in Bitcoin's wallets. A contract's address is `contract:0x…`, derived from its deployer and nonce, so no key
  controls it.
- **Validity:** an Edwards address must be a valid point on its curve; an arbitrary 32 bytes
  usually is not. A secp256k1 or bitcoin address is a hash, so there is nothing to check until its key signs;
  a bitcoin one's bech32 checksum does catch a mistyped letter.
- **One signature per transaction.** secp256k1 signatures, Ethereum's and Bitcoin's, always have a low `s` and verifying
  refuses a high one, so no one can reshape a signature into a second valid one. Its public key
  travels compressed, in 33 bytes, and its signature in 64.
- **Three ways to sign.** A secp256k1 key signs chainv4's transactions under chainv4's domain, or
  as EIP-712 typed data, as MetaMask and the hardware wallets it drives do (see
  [Typed data (EIP-712)](#typed-data-eip-712)), or signs Ethereum's own transactions, RLP and
  all, which the chain takes as `Tx::Ethereum` and runs in the EVM (see
  [Ethereum's tools](ethereum.md)).
- **In contracts,** every address takes 33 bytes: its kind's tag, then its bytes, a secp256k1 or
  bitcoin address's 20 padded with zeros. Lua's `address(hex)` reads 40 hex digits as a secp256k1
  address and 64 as an Ed25519 one; `address.parse("bc1q…")` reads a bitcoin one, and its
  `text()` writes it back so. The SDK's `Address::parse` and `text` do the same.
- **In the EVM,** a bitcoin account goes by an alias, as Ed25519 accounts do: its 20 bytes are a
  HASH160, not Ethereum's keccak256, so they are not its EVM address. See [EVM](evm.md).

### Signing domains

Everything signed or hashed for one purpose starts with a domain of its own, so a signature or
hash for one thing can never pass for another.

| Domain | What follows |
| --- | --- |
| `chainv4 transaction` | The genesis id, the transaction, nonce, fee offer and signer, so a signature holds on one chain alone. |
| EIP-155's chain id | An Ethereum transaction, signed as Ethereum signs it: bound to this chain by the chain id, the genesis id's first 6 bytes. |
| EIP-712's `chainv4` | A transaction's typed form, nonce and fee offer, under the genesis id as salt: see [Typed data (EIP-712)](#typed-data-eip-712). |
| `chainv4 block` | A header's id, signed by the authority. |
| `chainv4 vote` | The genesis id and a finality vote, so a vote cannot be replayed on another chain. |
| `chainv4 work` | A block's id and nonce, for the work hash. |
| `chainv4 contract address` | The deployer and nonce, for a contract's address. |

## Typed data (EIP-712)

An Ethereum wallet signs EIP-712 typed data: each field shown by name, then the keccak256 of
`0x19 0x01`, the domain's hash and the message's (`crypto::eip712`). So a secp256k1 key can sign
a chainv4 transaction that way too, with `Signature::Secp256k1Eip712`, and MetaMask, or a Ledger
or Trezor through it, can sign for chainv4 with the keys it holds.

**The domain:** `name` chainv4, `version` 1, and `salt` the genesis id, so a signature holds on
one chain alone. Nodes check it with `SignedTx::verify_on(genesis)`, at the gate, on the verifier
and in every block they run. Every signature on a transaction is checked this way, as each names
its chain: there is no check of one without a chain.

**The message,** one typed form per transaction:

| Form | Fields | For |
| --- | --- | --- |
| `Transfer` | `string from`, `string to`, `uint256 amount` | A coin transfer, its amount in its narrowest width. |
| `TokenTransfer` | `string token`, `string to`, `uint256 amount` | The token standard's `transfer` call. |
| `Transaction` | `string kind`, `bytes data` | Every other: its kind, and its encoding. |

Each ends with `uint64 nonce`, `uint64 gasLimit`, `uint256 maxPrice` and `uint256 tip`. Addresses
are their text, `ed25519:0x…`, since EIP-712's `address` holds Ethereum's alone. Each form maps
back to one transaction: a transfer whose amount could be narrower goes as `Transaction`, so no two
encodings share a signature.

**Checked against the standard and MetaMask:** EIP-712's own example hashes and signs as the
standard says, and `@metamask/eth-sig-util`, MetaMask's own library, hashes each form as chainv4
does; its signatures recover to the signer's key.

`chainv4 send --eip712` signs a transaction this way with a secp256k1 key of its own.

## Key files

`chainv4 keygen [--scheme ed25519|edblake|secp256k1|bitcoin] [--out PATH]` writes one line: the
scheme, a colon, and the 32-byte secret in hex: an Edwards key's seed, or a secp256k1 or bitcoin
key's scalar. It prints what others name the key by, never the secret, and
`chainv4 key show FILE…` prints the same for key files already made:

```
scheme secp256k1
public 0x02b337a4…660a      # compressed: what a federation or a multisig member lists
address secp256k1:0x0594…53ef
evm 0x0594…53ef             # the same 20 bytes, on every EVM chain
```

A bitcoin key adds `testnet`, its `tb1q…` address on Bitcoin's test networks; an Edwards key's
public key is its address.
- It is created readable by its owner only, and never overwritten.
- It is plain unless sealed, so guard it; `key show` says `sealed yes` for one that is.
- `--key`, `--producer-key`, `--validator-key`, `--faucet-key` and `--pool-push-key` read this
  format, sealed or not.

### Sealed key files

`chainv4 keygen --seal` writes the key sealed under a passphrase, asked twice and not shown as it
is typed. `chainv4 key encrypt FILE` seals a plain one in place, and `chainv4 key decrypt FILE`
writes a sealed one back plain; each writes beside the file first and renames it over, so a
failure leaves the old one whole. (`--passphrase` is BIP39's, other words for a phrase, and seals
nothing.) A sealed file is one line too:

```
chainv4-sealed-key:1:ed25519:<salt>:<nonce>:<sealed seed>
```

- **Stretching:** Argon2id at 64 MiB, 3 passes and 1 lane, pinned in `crypto::sealed::STRONG`,
  with a random 16-byte salt. Its working memory is wiped once it is done.
- **Sealing:** XChaCha20-Poly1305 seals the 32-byte seed under a random 24-byte nonce, with the
  version and the scheme as associated data, so neither can be changed without it failing.
- **What it shows without the passphrase:** the scheme, the salt and the nonce.

**Opening one.** Whatever reads a key file asks for a sealed one's passphrase on a terminal, not
shown, or reads it from a file:
- `--key-passphrase-file PATH` on the node, for its producer, validator, faucet and pool push keys;
- `CHAINV4_KEY_PASSPHRASE_FILE=PATH` for every command, and for the node without the flag. It names
  the file, never the passphrase, which would show among the process's environment. The file's first
  line is the passphrase; keep it readable by its owner only.

A node with neither, and no terminal, stops with what to give it. The wallet and the explorer take a
sealed key file pasted or picked, and ask for its passphrase. The passphrase, the stretched key, the
seed and its hex are wiped as they drop.

The development key's seed is in the source. Anyone can sign with it, so it must never hold
anything of value.

## Seed phrases

The `mnemonic` feature, on by default, makes keys from a seed phrase: BIP39's English words, 12
of them with 128 bits of randomness, or 24 with 256, the last word carrying a checksum.

- **The seed.** PBKDF2 stretches the words, with an optional passphrase, into a 64-byte seed. The
  same words with another passphrase are another seed, so a forgotten passphrase loses everything
  as surely as lost words.
- **One account, every scheme.** Account *n* of a seed has a key in each scheme:

  | Scheme | Standard | Path |
  | --- | --- | --- |
  | Ed25519 | SLIP-0010 | `m/44'/7744'/n'/0'/0'`, further keys `…/0'/k'` |
  | EdBlake | SLIP-0010 | `m/44'/7744'/n'/1'/0'`, further keys `…/1'/k'` |
  | secp256k1 | BIP32 | `m/44'/60'/0'/0/n` |
  | bitcoin | BIP32 (BIP84) | `m/84'/0'/0'/0/n` |
  | Bitcoin's test networks, for the offline signer's PSBTs | BIP32 (BIP84) | `m/84'/1'/0'/{0,1}/n` |

- **Ethereum's accounts.** The secp256k1 path is Ethereum's, so the same words give the same
  `0x…` addresses here as in MetaMask and other Ethereum wallets, account for account.
- **Bitcoin's addresses.** The bitcoin path is BIP84's, native segwit's, so the same words give
  the same `bc1q…` addresses here as Sparrow, BlueWallet and other Bitcoin wallets show: account
  *n* is the wallet's *n*th receiving address. What chainv4 sends to one stays on chainv4; bitcoin
  sent to it on Bitcoin is the same key's, but on Bitcoin. The offline signer signs Bitcoin's own
  transactions with these keys too, and their change at `m/84'/0'/0'/1/n`, for a Bitcoin wallet
  paired with it (see [Wallet](wallet.md#bitcoin-wallets)); on Bitcoin's test networks, with
  others, at coin type 1. `bech32::encode_witness` writes any witness version, Taproot's `bc1p…`
  in BIP350's bech32m, for showing what a PSBT pays.
- **The Edwards paths** are hardened at every step, as SLIP-0010 requires for Ed25519. 7744 is
  chainv4's coin type, not registered in SLIP-44. Ed25519 and EdBlake take a branch each, so one
  seed never makes the same secret for both.

`chainv4 keygen` makes keys from words:

```sh
chainv4 keygen --phrase                          # new words, printed once, and account 0's key
chainv4 keygen --phrase --words 24
chainv4 keygen --from-phrase --account 2 --scheme secp256k1 < words.txt
chainv4 keygen --from-phrase --scheme bitcoin < words.txt   # bc1q…, as Bitcoin's wallets show
chainv4 keygen --from-phrase --passphrase        # the words, then the passphrase, on stdin
```

The light wallet makes its accounts from words the same way: `keygen --from-phrase` with the same
words and account gives the same key the wallet holds.

## Encoding

Everything the chain stores, sends, hashes or signs is encoded with
[wincode](https://crates.io/crates/wincode).

- **Pinned tags.** Every enum is tagged with one byte, and each variant's tag is pinned with
  `#[wincode(tag = N)]`. Encodings stay the same across builds with different features, and a new
  variant takes the next unused tag. Never reuse or renumber a tag.
- **Little-endian.** Everything is, with three exceptions, on purpose:
  - the work hash is read as a big-endian number, so valid hashes start with zeros in hex;
  - heights and epochs in database keys are big-endian, so they sort in order;
  - XXH3 digests are big-endian as bytes, matching `xxhsum`.
- **One encoding per value.** The API refuses a submitted transaction with bytes past its end, so
  nothing can ride along after it.
- **Fixed-size fields.** Every field of a fee offer has a fixed size, which lets a client measure
  a transaction's size before it picks the gas limit.

Changing an encoding changes every id and hash built on it, and so every chain. Treat encodings
as consensus.
