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

- **Addresses:** a key's address is its scheme and its 32 bytes, such as `ed25519:0x…`. A
  contract's address is `contract:0x…`, derived from its deployer and nonce, so no key controls
  it.
- **Validity:** an address must be a valid point on its curve; an arbitrary 32 bytes usually is
  not.

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

`chainv4 keygen [--scheme ed25519|edblake] [--out PATH]` writes one line: the scheme, a colon,
and the 32-byte seed in hex.
- It is created readable by its owner only, and never overwritten.
- It is not encrypted, so guard it.
- `--key`, `--producer-key` and `--validator-key` read this format.

The development key's seed is in the source. Anyone can sign with it, so it must never hold
anything of value.

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
