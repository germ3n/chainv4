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

- **Addresses:** an Edwards key's address is its scheme and its 32 bytes, such as `ed25519:0x…`.
  A secp256k1 key's is Ethereum's: the last 20 bytes of the keccak256 of its uncompressed point,
  as `secp256k1:0x` and 40 hex digits. The same key has the same `0x…` address here as on
  Ethereum. A contract's address is `contract:0x…`, derived from its deployer and nonce, so no key
  controls it.
- **Validity:** an Edwards address must be a valid point on its curve; an arbitrary 32 bytes
  usually is not. A secp256k1 address is a hash, so there is nothing to check until its key signs.
- **One signature per transaction.** secp256k1 signatures always have a low `s` and verifying
  refuses a high one, so no one can reshape a signature into a second valid one. Its public key
  travels compressed, in 33 bytes, and its signature in 64.
- **Not Ethereum's transactions.** A secp256k1 key signs chainv4's transactions under chainv4's
  domain, not Ethereum's RLP or EIP-191 messages. An Ethereum wallet can hold the key, but cannot
  sign for chainv4 itself.
- **In contracts,** every address takes 33 bytes: its kind's tag, then its bytes, a secp256k1
  address's 20 padded with zeros. Lua's `address(hex)` reads 40 hex digits as a secp256k1 address
  and 64 as an Ed25519 one.

### Signing domains

Everything signed or hashed for one purpose starts with a domain of its own, so a signature or
hash for one thing can never pass for another.

| Domain | What follows |
| --- | --- |
| `chainv4 transaction` | The transaction, nonce, fee offer and signer. |
| `chainv4 block` | A header's id, signed by the authority. |
| `chainv4 vote` | The genesis id and a finality vote, so a vote cannot be replayed on another chain. |
| `chainv4 work` | A block's id and nonce, for the work hash. |
| `chainv4 contract address` | The deployer and nonce, for a contract's address. |

## Key files

`chainv4 keygen [--scheme ed25519|edblake|secp256k1] [--out PATH]` writes one line: the scheme, a
colon, and the 32-byte secret in hex: an Edwards key's seed, or a secp256k1 key's scalar.
- It is created readable by its owner only, and never overwritten.
- It is not encrypted, so guard it.
- `--key`, `--producer-key` and `--validator-key` read this format.

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
  | Ed25519 | SLIP-0010 | `m/44'/7744'/n'/0'/0'` |
  | EdBlake | SLIP-0010 | `m/44'/7744'/n'/1'/0'` |
  | secp256k1 | BIP32 | `m/44'/60'/0'/0/n` |

- **Ethereum's accounts.** The secp256k1 path is Ethereum's, so the same words give the same
  `0x…` addresses here as in MetaMask and other Ethereum wallets, account for account.
- **The Edwards paths** are hardened at every step, as SLIP-0010 requires for Ed25519. 7744 is
  chainv4's coin type, not registered in SLIP-44. Ed25519 and EdBlake take a branch each, so one
  seed never makes the same secret for both.

`chainv4 keygen` makes keys from words:

```sh
chainv4 keygen --phrase                          # new words, printed once, and account 0's key
chainv4 keygen --phrase --words 24
chainv4 keygen --from-phrase --account 2 --scheme secp256k1 < words.txt
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
