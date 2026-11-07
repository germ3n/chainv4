# Multisig

A native multisig (`src/registry/multisig.rs`) is an account that M of its N members' keys
control, built into the chain rather than a contract. Its address, `multisig:0x…`, holds coin,
tokens, names and contracts as any account does, and sends every kind of transaction a key's
account sends; each one needs `threshold` of its members.

A member also names its keys on the chains a multisig reaches beyond this one, so the same quorum
holds the matching Bitcoin P2WSH M-of-N (see [On Bitcoin](#on-bitcoin)) and a Safe-style contract
at one address on every EVM chain (see [On EVM chains](#on-evm-chains)).

**Nothing is signed blind.** Each of the commands that signs says what it agrees to and asks for a
typed `yes`, or takes `--yes` in a script, which still shows it, as a signature that has left the
file cannot be taken back:
- **`multisig bitcoin sign`** shows each output's address and amount, what the coins hold and the
  fee, before it even asks for the seed. It refuses a spend it cannot say that of: an input with no
  amount, inputs naming no key or both networks, an output with no address, or one paying more than
  it spends. It reads no node, so it cannot tell change from a payment: look at every line.
- **`multisig sign`** and **`approve`** show the multisig, its nonce, the transaction, every step
  of a batch, and what it may pay in gas; a deploy by its engine, size, SHA-256 and address, and its
  code with `--full` ([a batch](#a-batch)).
- **`multisig evm sign`** shows each call's chain, contract, whom it calls, value, nonce and data.

## Making one

**All it takes is a key on this chain from each member, and how many of them must agree.** No
Bitcoin address is needed, nor anything else from another chain: what a member names for one is
optional, for a multisig that is to hold coins there too.

```sh
chainv4 send --key alice.key create-multisig --threshold 2 \
  --member ed25519:0x…alice --member ed25519:0x…bob --member secp256k1:0x…carol
```

`Tx::CreateMultisig` makes it at `multisig_address` of the sender and the nonce it is sent at, as a
deploy's address is made, and `send` prints it. The creator pays and has no say after, unless it is
a member.

- **Members:** 1 to 20, CHECKMULTISIG's limit, which a Bitcoin P2WSH multisig relays up to. Each is
  a key here, of any scheme the chain has, named once: `ed25519:0x…`, `edblake:0x…`, or an Ethereum
  or Bitcoin account's public key, `secp256k1:0x…` or `bitcoin:0x…` and 66 hex digits. Not its
  address: an Ethereum or Bitcoin address is a hash of the key, and the key cannot be read back from
  it. A multisig may mix them, one member of each.
- **Their other chains' keys:** a member may name its Bitcoin multisig account xpub, with its
  origin, and its EVM address, `--member KEY,[73c5da0a/48'/0'/0'/2']xpub…,0x…`, each from its own
  words; its EVM address alone, `--member KEY,,0x…`; or its xpub alone, `--member
  KEY,[73c5da0a/48'/0'/0'/2']xpub…`. So a multisig reaches EVM chains, Bitcoin, or both. Every
  member names an xpub, or none does, and an EVM address, or none does.
- **A `bitcoin:` member names an xpub too,** from the same words: its key here derives at BIP84's
  m/84'/0'/0'/0/n, its xpub at BIP48's m/48'/coin'/n'/2', apart, n its account. Each account of
  the words names its own xpub, so two of them may both be members of one multisig on Bitcoin.
- **Its Bitcoin network:** the members' xpubs say it, xpub for Bitcoin or tpub for its test
  networks, all the same; a test multisig and a real one are separate multisigs.
- **Threshold:** at least 1, at most the members.
- **The policy lives in state,** in `Column::Accounts` under `Multisigs::policy_key`, so a light
  client proves it as it proves a balance. `GET /multisig/{address}` shows it.

## On Bitcoin

A multisig whose members name xpubs holds coins on Bitcoin the same quorum spends: a P2WSH M-of-N,
as Bitcoin's own multisig wallets make one, so Sparrow, Nunchuk, Electrum, Bitcoin Core or a
hardware wallet watch and co-sign it (`src/crypto/bitcoin/multisig.rs`):

```text
wsh(sortedmulti(M,[fingerprint/48'/0'/0'/2']xpub…/0/*,…))#checksum
```

- **Each member's xpub** is its account at BIP48's path for native segwit multisig, m/48'/0'/n'/2'
  on Bitcoin and m/48'/1'/n'/2' on its test networks, n the account of its words, with its seed's
  fingerprint. The policy keeps n with it, and the descriptor writes it in the origin.
- **Fresh addresses:** receiving address n takes every member's key at 0/n, change at 1/n, sorted
  (BIP67), so the same members make the same addresses in any order. The descriptor, with its
  BIP380 checksum, imports into other wallets unchanged; `GET /multisig/{address}` shows both and
  the first address.
- **Spends are PSBTs** (`src/crypto/bitcoin/psbt.rs`): each input with its coins, its witness script
  and every member's key and origin, the change said to be the multisig's own. Each member signs
  its copy, here, on the offline signer, or in its own Bitcoin wallet; the copies combine, and the
  first threshold signatures, in the script's key order, finish it.

```sh
chainv4 multisig bitcoin multisig:0x… descriptor
chainv4 multisig bitcoin multisig:0x… address --index 3
chainv4 multisig bitcoin multisig:0x… balance --esplora https://mempool.space/testnet4/api
chainv4 multisig bitcoin multisig:0x… send --esplora … --to tb1q… --amount 50000 --fee-rate 2 --out pay.psbt
chainv4 multisig bitcoin multisig:0x… sign pay.psbt          # shows it, asks for yes, reads the member's seed phrase
chainv4 multisig bitcoin multisig:0x… combine pay.psbt theirs.psbt
chainv4 multisig bitcoin multisig:0x… finish pay.psbt --esplora …
```

`balance` and `send` look at every address on each chain until 20 in a row were never used; `send`
takes the coins largest first and sends change to the next unused change address, unless it would
be dust. PSBT files are base64, as other wallets pass them.

## On EVM chains

A multisig whose members name EVM owners has a contract on EVM chains they control, M of N:
`contracts/evm/MultiSig.sol`, Safe-style, built with solc for Paris so it runs on chains without
PUSH0 (`src/abi/multisig.rs` speaks to it).

- **Its owners are of three kinds** (`EvmKind`), each member naming one:

  | Kind | `--member` | Signs with | Checked by |
  | --- | --- | --- | --- |
  | An EVM key | `KEY,,0x…` | 65 bytes, r, s, v | `ecrecover`, low s |
  | A contract | `KEY,,contract:0x…` | whatever its `isValidSignature` reads | ERC-1271, through a static call |
  | An Ed25519 key | `KEY,,ed25519` (the member's own) or `KEY,,ed25519:0x…` | 64 bytes, over the digest | chainv4's crypto precompile at home, `contracts/evm/Ed25519.sol` elsewhere |

  An Ed25519 owner's address there is its chainv4 account's EVM address, the last 20 bytes of
  `keccak256(0x00 || key)`, which the contract works out from the key.
- **Ed25519 costs where the precompile is not.** On chainv4's own EVM, the chain whose id the
  contract's init code names (its *home*), the precompile at `0x…4e56` checks a signature for a few
  thousand gas. Everywhere else `Ed25519.sol` does, SHA-512 and the curve in Solidity and Yul,
  about 400,000 gas a signature, held to RFC 8032's vectors and to Rust's signatures. The contract
  trusts the precompile only at home, so no other chain's code at that address can stand in for it.
- **A contract owner** is a Safe, a smart-account wallet, or another multisig's MultiSig.sol. It is
  asked through a static call, so it changes nothing, here or anywhere, while it answers.
- **It answers ERC-1271 itself:** `isValidSignature(hash, signature)` holds when threshold of its
  owners signed `messageDigest(hash)`, EIP-712's digest of `Message(bytes32 hash)` under its own
  domain, the signature being `abi.encode(address[] signers, bytes[] signatures)`. So one multisig
  owns another, and its owners' signatures of its own calls never count as approvals of anything
  else.
- **One address everywhere.** It is deployed through the deterministic deployment proxy,
  `0x4e59…956c`, with no salt, its init code naming the chainv4 multisig, chainv4's chain id, and
  the owners, each with its kind, and threshold it starts with. So its address, `multisig_contract`,
  is the same on every chain with the proxy, known before it is deployed, and coin sent there first
  waits for it. Nobody but those owners can be the ones at it.
- **Its first owners are kept.** The address commits to the members' EVM addresses and threshold
  when they first named them, at its making or a change since, so the chain keeps those as
  they were (`Multisigs::evm_start`), whatever the members become. `GET /multisig/{address}`
  shows the contract, those, and the ones named now.
- **Anyone deploys it.** The first call sent deploys it when it is not there yet; `deploy` does it
  alone. Whoever sends pays the gas, and need not be an owner.
- **A call is signed by its owners.** Each signs EIP-712's digest of `Execute(to, value, data,
  nonce)` under the domain `chainv4 MultiSig`, version 1, of the contract on its chain, so a
  signature holds for one call, on one chain, once. `execute(to, value, data, signers,
  signatures)` takes threshold of them, the signers by address ascending, each with its signature
  as its kind takes it, runs the call as the contract, and moves its nonce; a call that fails
  reverts it all. A hardware wallet, MetaMask or the offline signer signs the same call as typed
  data (`EvmBundle::typed_data`); an Ed25519 owner signs the digest itself.
- **Bundles** carry a call round the owners, `chainv4-multisig-evm:0x…` as text, one a line in a
  file, until enough signed. A bundle also gathers the approvals of each owner that is another
  multisig's contract, its own owners signing `nested_digest`, and packs them as that contract's
  signature once they are there (`EvmBundle::nested`).
- **Its owners change through itself.** `changeOwners` runs only as one of its own calls, signed
  by the owners before. When the members change on chainv4, nothing moves on the EVM chains by
  itself: `status` says the owners there are not the members' now, and `sync` proposes the
  change, which the owners before sign.
- **It takes NFTs**, ERC-721's and ERC-1155's checks answering yes, and ERC-20 tokens as any
  address does.
- **On chainv4's own EVM** it lands at the same address, the chain having the proxy from genesis;
  the native multisig there already sends EVM calls itself, so it is not needed.

```sh
chainv4 multisig evm multisig:0x… address
chainv4 multisig evm multisig:0x… status --eth-rpc https://ethereum-sepolia-rpc.publicnode.com
chainv4 multisig --key alice-evm.key evm multisig:0x… propose --eth-rpc … --to 0x… --value 1000000000000000 --out pay.evm
chainv4 multisig --key bob-evm.key evm multisig:0x… sign pay.evm             # shows each call, asks for yes; needs no node or chain
chainv4 multisig --key anyone-evm.key evm multisig:0x… submit pay.evm --eth-rpc …
chainv4 multisig --key alice-evm.key evm multisig:0x… sync --eth-rpc … --out owners.evm
chainv4 multisig --key carol.key evm multisig:0x… sign pay.evm               # an Ed25519 owner signs with its own key
chainv4 multisig --key dave-evm.key evm multisig:0xINNER… approve pay.evm   # as an owner of a multisig whose contract owns this one
chainv4 multisig --key anyone.key evm multisig:0x… add-signature pay.evm --owner 0x… --signature 0x…   # a Safe's bytes
```

`--key` signs as an owner: a secp256k1 key file, the member's EVM key, or an Ed25519 key file for
an Ed25519 owner. `submit` and `deploy` pay gas, so they take an EVM key, owner or not; a proposal
an Ed25519 key signed is written for another to send. `propose --nonce` lines calls up past the
next one. `approve` runs on the multisig whose contract is the owner: its members each sign, and
the bundle carries their approvals, passed back to the other multisig's members. `submit` asks
each contract owner's `isValidSignature` before it counts it.

## Swaps

A multisig swaps as a party on chainv4, Bitcoin and EVM chains, its quorum in the Bitcoin HTLC's
branch and its contract the EVM party (see [Swaps](swaps.md#from-a-multisig)).

## Its transactions

A multisig's own transaction is `Tx::Multisig`: the transaction it sends, the members' signatures
collected for it, and the multisig as its sender. Its `SignedTx` carries the **multisig's** nonce
and fee offer, so the multisig takes the nonce and pays the fee from its own balance; whoever
submits it, the carrier, signs the envelope only and pays nothing.

What members sign is the body, `MultisigBody`: the multisig, the transaction, the nonce and the
fee offer, after a domain tag and the chain's genesis id, so a signature holds on one chain only.

Two ways to back it, which count together:
- **Signatures off chain.** One member proposes it and signs; the others sign the same bundle where
  their keys are, offline too; anyone submits it once it holds enough. One transaction, one fee.
- **Approvals on chain.** A member sends `Tx::ApproveMultisig` from its own account, at its own
  cost, naming the multisig's next nonce, the fee offer and the transaction; `Tx::WithdrawMultisig`
  takes it back. A `Tx::Multisig` then needs only the signatures the approvals leave short, or
  none.

A member backs one proposal at a time: approving another for the same nonce moves its approval.
Proposals are for the multisig's next nonce, and void once any of its transactions runs, so state
only ever holds those still to come.

**Refused before it runs.** A `Tx::Multisig` its members do not back enough, carrying a signature by
a key that is no member's or that does not check out, or for an address that is no multisig, is
refused before its nonce or fee: the mempool does not take it and a block holding it is invalid.
Nobody but the quorum moves a multisig's nonce or spends its fee. Once backed, it runs as any
transaction runs: one that fails still takes its nonce and pays.

**What it may send:** anything a key's account sends: transfers, calls, deploys, EVM calls, names,
bonds, governance approvals, swaps, which are calls, and its own `Tx::ChangeMultisig`. Not another
multisig's transaction, an approval of one, or an Ethereum-signed transaction, which one key signs.
A transfer or burn it sends moves its own coin only.

## A batch

A multisig may send a [batch](chain.md#batches): `Tx::Multisig { tx: Batch }`. Members approve one
bundle, on chain or off, and all the steps run as the multisig, at its one nonce, its fee from its
balance, all or nothing. Every step must take from the multisig, or the bundle is invalid, sent or
approved. Never the reverse: a batch holds no multisig's transaction, and no multisig's administration
(`CreateMultisig`, `ChangeMultisig`, `ApproveMultisig`, `WithdrawMultisig`).

Build one with the wallet's Batch page by picking the multisig under Send as, or
`chainv4 send batch --multisig multisig:0x…`. Everything that signs one lists every step first: the
wallet's bundle, its proposals, the offline signer, and `chainv4 multisig sign` and `approve`, which
print each before the member's key goes on it.

**Members in MetaMask.** A member whose key is an Ethereum one signs a multisig's transaction, a
batch or a lone one, as EIP-712 typed data, its steps written out and its calls by name (see
[Typed data](crypto.md#typed-data-eip-712)): off chain into the bundle, approving it on chain, or
carrying it. The bundle carries the names of its calls as its proposer knew them, which the member
signs with; each is checked against its call's hash. `--eip712` on `chainv4 send --multisig`,
`multisig sign` and `multisig approve` does the same with a secp256k1 key file.

**A deploy is listed by what identifies it, not its code.** `multisig sign` and `approve` print the
transaction, or each step of a batch, as the API writes it, but a deploy or upgrade as its engine,
its size, its code's SHA-256 and, for a deploy, the address it lands at. `send --multisig` lists a
batch's steps the same way, its deploys' addresses on lines of their own after them, as before:

```
a batch of 2 steps, all or nothing:
  step 1: {"type":"deploy","engine":"lua","code_size":14230,"code_hash":"sha256:0x5f…","address":"contract:0x9a…"}
  step 2: {"type":"contract_call","to":"contract:0x9a…","func":…}
```

Check the hash against the file you were sent, `shasum -a 256 dex.lua`, or for EVM init code, which
`.bin` files hold as hex, `xxd -r -p Token.bin | shasum -a 256` (with any constructor arguments
after it). `sign --full` and `approve --full` print the code whole instead: Lua's source, or a
WebAssembly module's or init code's hex. The address comes from the multisig's nonce the bundle
fixes and the chain's hash, which its genesis id is in, so signing still asks no node. **Before
this,** a batch deploying a contract printed its whole Lua source in the review, once in the
transaction and again in its step, burying the steps around it.

## Changing members

`Tx::ChangeMultisig` replaces the members and threshold, checked as at creation. Only a multisig
sends it, through its own quorum, so the old members decide the new ones; the address stays, and
the open proposals go.

```sh
chainv4 send --key alice.key --multisig multisig:0x… --out change.bundle change-multisig --threshold 2 --member … --member …
```

## From the command line

`send --multisig ADDRESS` sends any transaction as the multisig: the transaction is built as the
multisig's, priced by a dry run, and signed by `--key` as a member. If that is enough it is
submitted at once; otherwise the bundle is written to `--out`, or printed, as
`chainv4-multisig:0x…` text. A deploy's address, or each of a batch's deploys', is printed first:
the bundle fixes the multisig's nonce, so it is known before anyone signs.

**Priced for its heaviest quorum.** The probe it prices carries the signatures of the threshold
members whose keys weigh most, and is carried by the heaviest key the build has, so whichever
members sign and whoever carries it, the fee covers it (`MultisigBundle::probe`), each secp256k1
member's signature with the calls' names beside it, as one signed in MetaMask carries. A secp256k1 or
bitcoin key is a byte longer than an Ed25519 one; priced for the proposer's key alone, a quorum
with one of them was refused as under its intrinsic cost. A bundle goes out with no more
signatures than the threshold, which is all it was priced for (`MultisigBundle::trimmed`).

**Before this, those signatures were all the proposer's,** so the dry run's size was right only when the
members who signed were of its key's kind. A transaction priced at its intrinsic cost, as a transfer is,
then ran short when a larger signature joined it, such as a secp256k1 member's beside an Ed25519
proposer's: the node refused it, `gas limit under its intrinsic cost`, whoever submitted it. On the test
chain a transfer proposed at 6,000 used 6,020 with such a member. A bundle proposed by an older CLI or
wallet still has that limit: propose it again, or with a little more `--gas-limit`.

```sh
chainv4 send --key alice.key --multisig multisig:0x… --out pay.bundle transfer --to ed25519:0x… --amount 1000
chainv4 multisig --key bob.key sign pay.bundle --submit     # shows it, asks for yes; needs no node to sign; --submit sends it once backed; --full shows deploys' code
chainv4 multisig --key carol.key approve pay.bundle         # on chain, from carol's account; submits once backed
chainv4 multisig --key anyone.key submit pay.bundle
chainv4 multisig --key carol.key withdraw multisig:0x… --proposal 0x…
chainv4 multisig show multisig:0x…
```

## In the wallet

The **Multisig** page lists every multisig any account in the wallet is a member of, as the node's
member index says (`GET /accounts/{address}/multisigs`, asked for each of the wallet's addresses,
every page to the end), beside those followed by pasting their address, which is for multisigs
one only watches, or a node that keeps no member index. Each says which of the wallet's accounts
are its members. The one picked shows its address whole, to copy, and as a QR code; what the node
says of it, its members, named by account where they are the wallet's, its balance and next
transaction, and the proposals open on chain with who approves each, to approve, take back an
approval from or send. With Read from checked blocks on, the node is asked none of it.

- **Make a multisig** goes through four steps, with Back and Cancel, and the page says up front
  what it needs:
  1. **Who are the members?** A key for each, required, of any scheme. **Add one of this wallet's
     keys** adds any account's, in any scheme: Ed25519, EdBlake, Ethereum's or Bitcoin's. A line
     Pair or `chainv4 multisig` wrote, pasted in, goes to its fields; one of the wallet's own
     addresses typed in becomes its key; another Ethereum or Bitcoin address is told it is an
     address, not a key, and where the member finds its key.
  2. **Should it reach other chains?** Optional: only this chain, EVM chains too, Bitcoin too, on
     its network or its test networks, or both. Each member's row then asks plainly what each
     needs: an EVM address, and a Bitcoin account key, which is not a Bitcoin address, with where a
     member finds theirs. **Fill in from** fills a row that is one of the wallet's accounts from
     its own words, in one tap.
  3. **How many must agree?** So many of so many, with what that means: one stolen key, or one lost.
  4. **Review:** who, where, the address it will be at and who pays, then Send to sign.
- **Change its members** takes the same steps from what the node lists, adding, removing or
  rotating keys or changing how many must agree; it is the multisig's own `Tx::ChangeMultisig`, so
  it goes round its members as a bundle. On the offline signer, Pair shows a member's line for an
  account whose keys are there.
- **Waiting for approval** says of each proposal how many agree, what it needs and who from, and
  once it has enough, that anyone can send it.
- **Propose:** a payment, the quick default, or **anything else** the Batch page makes, with the
  same form: token transfers, contract and EVM calls, deploys, names, burns and claims. One step
  goes out as it is; several as a batch. Nothing goes out yet: the node prices it for its heaviest
  quorum, and the bundle shows what it does, its number, the fee at most, who signed and approved,
  named by account where the wallet knows them, and what comes next.
- **Agree as** picks which key signs and approves: each of the wallet's accounts that is a member,
  any of its schemes, and an offline signer. It starts at one that has not signed or approved yet,
  a key held here before one on a signer, and moves on to the next once it signs, so one wallet
  holding several members' keys signs with each in turn. **Sign as** signs with that key; **Sign
  with MetaMask as**, for a member MetaMask holds, in the browser, has MetaMask sign the bundle's
  typed form, every step written out, and checks its signature is the member's before adding it.
  A call whose function the bundle names no name for would show as its hash, so the wallet says so
  instead. **Ask the signer**, for a watch-only account or an offline signer, shows the bundle as a
  `ur:chainv4-multisig` code, animated, and reads the signer's code back. **Approve on chain as**
  goes to Send as that account, so a hardware or offline account approves too; an offline signer
  the wallet holds no account of approves once paired under Accounts. **Copy as text** gives what
  chainv4 multisig reads. **Send it** once enough back it, under a key made for the moment: the
  multisig pays.
- **Another member sent you something to sign?** Folded until wanted: paste its text, or read its
  code.

For a multisig on EVM chains, the page shows its contract's address, and on a chain whose JSON-RPC
URL Settings names, whether it is deployed, what it holds, its owners and next nonce. **Make the
bundle** proposes a call, a payment or call data; **Sign** signs it with the account's key, its
EVM key or, for an Ed25519 owner, its Ed25519 key; **Ask the signer** shows it to the offline
signer, as EIP-712 typed data for an EVM key or as the bundle for an Ed25519 key, and reads its
answer back; and a member's pasted bundle adds what it signed, approvals for contract owners
too. **Deploy it here first** and **Send it** go out as the account's own EVM transactions, from
its EVM key, priced and signed as on the Swaps page. When its members changed, it offers the
change of owners for the owners before to sign. **Approve another multisig's call**, on the page
of a multisig whose contract is an owner of another's, signs that one's pasted bundle for it, here
or on the offline signer, and copies it back.

For a multisig on Bitcoin, the page shows its descriptor, a fresh receiving address, and, with an
Esplora API in Settings, what its addresses hold. **Pay from it on Bitcoin** makes the PSBT; **Sign
with these words** signs it here, **Ask the signer** shows it as `ur:crypto-psbt` and reads the
signed one back, a member's base64 PSBT pasted is combined in, and **Finish and broadcast** sends it
through Esplora once every input holds its threshold. The offline signer signs a multisig's input
as any of its own, at m/48'/coin'/n'/2', every account's, once its witness script holds the key and is the coins',
and shows change back to the multisig as its own.

The **offline signer** reads a bundle on Sign, shows what the multisig's transaction does, from
which multisig, at which nonce, and who signed so far, signs it with the key picked, and shows the
bundle back. It cannot know whether the key is a member's: the chain checks that.

## Things to keep in mind

- **What the node says of a multisig is not proven** on the page yet: the members, nonce and
  proposals are its word, and so is which multisigs an account is in. Nothing signed rests on it,
  since a bundle says what it does, and the chain checks the quorum as it runs.
- **Anyone can name your key in a multisig.** Making one needs no member's consent, so the
  listing may hold multisigs a stranger made naming you; they can do nothing without your
  signature. The wallet lists every page of them.
- **A bundle is for one nonce.** Once any other transaction of the multisig runs, its signatures
  and approvals are for a nonce gone; propose it again.
- **The fee is the multisig's,** at the offer the members signed. A multisig with no coin cannot
  act until someone sends it some.
- **The Bitcoin side is the members' xpubs as the node says them**, unproven as the rest: a PSBT
  says what it spends and pays, and each signer checks its own key is in the script the coins are
  locked to before it signs.
- **The EVM contract's owners are the members' EVM addresses as the node says them**, unproven:
  its address commits to the first ones, so a node lying about them names an address nobody
  controls, not one someone else does; and the owners there are what the contract says.
- **A member is one owner on each EVM chain:** an EVM key, its own Ed25519 key here, or a
  contract. An Ed25519 owner costs about 400,000 gas a signature on chains other than chainv4,
  and pays no gas itself: an EVM key sends what it signed.
- **The carrier's key does not matter** to the chain: it signs the envelope so a node takes it,
  and pays nothing.
