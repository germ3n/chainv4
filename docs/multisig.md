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
- **`multisig sign`** and **`approve`** show the multisig, its nonce, the transaction in full, every
  step of a batch, and what it may pay in gas.
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
  a key here, any kind the chain takes, named once.
- **Their other chains' keys:** a member may name its Bitcoin multisig account xpub, with its
  origin, and its EVM address, `--member KEY,[73c5da0a/48'/0'/0'/2']xpub…,0x…`, each from its own
  words; or its EVM address alone, `--member KEY,,0x…`. Every member names each, or none does.
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

- **Each member's xpub** is its account at BIP48's path for native segwit multisig, m/48'/0'/0'/2'
  on Bitcoin and m/48'/1'/0'/2' on its test networks, with its seed's fingerprint.
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

A multisig whose members name EVM addresses has a contract on EVM chains its members' EVM keys
control, M of N: `contracts/evm/MultiSig.sol`, Safe-style, built with solc for Paris so it runs
on chains without PUSH0 (`src/abi/multisig.rs` speaks to it).

- **One address everywhere.** It is deployed through the deterministic deployment proxy,
  `0x4e59…956c`, with no salt, its init code naming the chainv4 multisig and the owners and
  threshold it starts with. So its address, `multisig_contract`, is the same on every chain with
  the proxy, known before it is deployed, and coin sent there first waits for it. Nobody but those
  owners can be the ones at it.
- **Its first owners are kept.** The address commits to the members' EVM addresses and threshold
  when they first named them, at its making or a change since, so the chain keeps those as
  they were (`Multisigs::evm_start`), whatever the members become. `GET /multisig/{address}`
  shows the contract, those, and the ones named now.
- **Anyone deploys it.** The first call sent deploys it when it is not there yet; `deploy` does it
  alone. Whoever sends pays the gas, and need not be an owner.
- **A call is signed by its owners.** Each signs EIP-712's digest of `Execute(to, value, data,
  nonce)` under the domain `chainv4 MultiSig`, version 1, of the contract on its chain, so a
  signature holds for one call, on one chain, once. `execute` takes threshold of them, by owner
  address ascending, low s, runs the call as the contract, and moves its nonce; a call that fails
  reverts it all. A hardware wallet, MetaMask or the offline signer signs the same call as typed
  data (`EvmBundle::typed_data`).
- **Bundles** carry a call round the owners, `chainv4-multisig-evm:0x…` as text, one a line in a
  file, until enough signed.
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
```

`--key` is a secp256k1 key file, the member's EVM key, which signs; `submit` and `deploy` pay gas
with it. `propose --nonce` lines calls up past the next one.

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

## Changing members

`Tx::ChangeMultisig` replaces the members and threshold, checked as at creation. Only a multisig
sends it, through its own quorum, so the old members decide the new ones; the address stays, and
the open proposals go.

```sh
chainv4 send --key alice.key --multisig multisig:0x… --out change.bundle change-multisig --threshold 2 --member … --member …
```

## From the command line

`send --multisig ADDRESS` sends any transaction as the multisig: the transaction is built as the
multisig's, priced by a dry run that carries as many signatures as the quorum will, and signed by
`--key` as a member. If that is enough it is submitted at once; otherwise the bundle is written to
`--out`, or printed, as `chainv4-multisig:0x…` text.

```sh
chainv4 send --key alice.key --multisig multisig:0x… --out pay.bundle transfer --to ed25519:0x… --amount 1000
chainv4 multisig --key bob.key sign pay.bundle --submit     # shows it, asks for yes; needs no node to sign; --submit sends it once backed
chainv4 multisig --key carol.key approve pay.bundle         # on chain, from carol's account; submits once backed
chainv4 multisig --key anyone.key submit pay.bundle
chainv4 multisig --key carol.key withdraw multisig:0x… --proposal 0x…
chainv4 multisig show multisig:0x…
```

## In the wallet

The **Multisig** page follows multisigs by address: those made there, those whose bundles it read,
and those added. For the one picked it shows what the node says of it, its members, the account
among them, its balance and next transaction, and the proposals open on chain with who approves
each, to approve, take back an approval from or send.

- **Make a multisig** goes through four steps, with Back and Cancel, and the page says up front
  what it needs:
  1. **Who are the members?** A key for each, required. **Add this account** adds the account's;
     a line Pair or `chainv4 multisig` wrote, pasted in, goes to its fields.
  2. **Should it reach other chains?** Optional: only this chain, unless it is to hold coins on EVM
     chains, which asks each member's EVM address, or on Bitcoin too, on its network or its test
     networks, which asks each member's multisig account key as well. This account's fill in by
     themselves, for an account its words make.
  3. **How many must agree?** So many of so many, with what that means: one stolen key, or one lost.
  4. **Review:** who, where, the address it will be at and who pays, then Send to sign.
- **Change its members** takes the same steps from what the node lists, adding, removing or
  rotating keys or changing how many must agree; it is the multisig's own `Tx::ChangeMultisig`, so
  it goes round its members as a bundle. On the offline signer, Pair shows a member's line for an
  account whose keys are there.
- **Waiting for approval** says of each proposal how many agree, what it needs and who from, and
  once it has enough, that anyone can send it.
- **Propose a payment:** nothing goes out yet: the node prices it, and the bundle shows what it does,
  its number, the fee at most, who signed, and what comes next.
- **Sign** with the account's key, or **Ask the signer** for a watch-only account: the bundle as a
  `ur:chainv4-multisig` code, animated, and the signer's code read back. **Approve on chain** goes
  to Send, so a hardware or offline account approves too. **Copy as text** gives what chainv4
  multisig reads. **Send it** once enough back it, under a key made for the moment: the multisig
  pays.
- **Another member sent you something to sign?** Folded until wanted: paste its text, or read its
  code.

For a multisig on EVM chains, the page shows its contract's address, and on a chain whose JSON-RPC
URL Settings names, whether it is deployed, what it holds, its owners and next nonce. **Make the
bundle** proposes a call, a payment or call data; **Sign** signs it with the account's EVM key,
**Ask the signer** shows it to the offline signer as EIP-712 typed data and reads its signature
back, and a member's pasted bundle adds what it signed. **Deploy it here first** and **Send it**
go out as the account's own EVM transactions, priced and signed as on the Swaps page. When its
members changed, it offers the change of owners for the owners before to sign.

For a multisig on Bitcoin, the page shows its descriptor, a fresh receiving address, and, with an
Esplora API in Settings, what its addresses hold. **Pay from it on Bitcoin** makes the PSBT; **Sign
with these words** signs it here, **Ask the signer** shows it as `ur:crypto-psbt` and reads the
signed one back, a member's base64 PSBT pasted is combined in, and **Finish and broadcast** sends it
through Esplora once every input holds its threshold. The offline signer signs a multisig's input
as any of its own, at m/48'/coin'/0'/2', once its witness script holds the key and is the coins',
and shows change back to the multisig as its own.

The **offline signer** reads a bundle on Sign, shows what the multisig's transaction does, from
which multisig, at which nonce, and who signed so far, signs it with the key picked, and shows the
bundle back. It cannot know whether the key is a member's: the chain checks that.

## Things to keep in mind

- **What the node says of a multisig is not proven** on the page yet: the members, nonce and
  proposals are its word. Nothing signed rests on it, since a bundle says what it does, and the
  chain checks the quorum as it runs.
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
- **A member is one key on each EVM chain:** `ecrecover` checks secp256k1 alone, so a member whose
  key here is Ed25519 names its EVM address from its words.
- **The carrier's key does not matter** to the chain: it signs the envelope so a node takes it,
  and pays nothing.
