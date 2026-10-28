# Multisig

A native multisig (`src/registry/multisig.rs`) is an account that M of its N members' keys
control, built into the chain rather than a contract. Its address, `multisig:0x…`, holds coin,
tokens, names and contracts as any account does, and sends every kind of transaction a key's
account sends; each one needs `threshold` of its members.

This part is the multisig on chainv4 itself. A member also names its keys on the chains a multisig
reaches beyond this one, so the same quorum can hold the matching Bitcoin P2WSH M-of-N and an EVM
Safe-style contract; those come in their own batches (see the [todo](todo.md)).

## Making one

```sh
chainv4 send --key alice.key create-multisig --threshold 2 \
  --member ed25519:0x…alice --member ed25519:0x…bob --member secp256k1:0x…carol
```

`Tx::CreateMultisig` makes it at `multisig_address` of the sender and the nonce it is sent at, as a
deploy's address is made, and `send` prints it. The creator pays and has no say after, unless it is
a member.

- **Members:** 1 to 20, CHECKMULTISIG's limit, which a Bitcoin P2WSH multisig relays up to. Each is
  a key here, any kind the chain takes, named once.
- **Their other chains' keys:** a member may name its compressed Bitcoin key and its EVM address,
  `--member KEY/BITCOIN_KEY/EVM_ADDRESS`, each from its own words. Every member names them, or none
  does.
- **Threshold:** at least 1, at most the members.
- **The policy lives in state,** in `Column::Accounts` under `Multisigs::policy_key`, so a light
  client proves it as it proves a balance. `GET /multisig/{address}` shows it.

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
chainv4 multisig --key bob.key sign pay.bundle --submit     # needs no node to sign; --submit sends it once backed
chainv4 multisig --key carol.key approve pay.bundle         # on chain, from carol's account; submits once backed
chainv4 multisig --key anyone.key submit pay.bundle
chainv4 multisig --key carol.key withdraw multisig:0x… --proposal 0x…
chainv4 multisig show multisig:0x…
```

## In the wallet

The **Multisig** page follows multisigs by address: those made there, those whose bundles it read,
and those added. For the one picked it shows what the node says of it, its members, the account
among them, its balance and next nonce, and the proposals open on chain with who approves each, to
approve, withdraw from or submit.

- **Make a multisig:** members one a line, **Add this account** adding the account's key, with its
  Bitcoin key and EVM address when it has both, and the threshold. It goes to Send to sign.
- **Propose a payment:** the node prices it, and the bundle shows: what it does, the nonce, the fee
  at most, and who signed.
- **Sign** with the account's key, or **Ask the signer** for a watch-only account: the bundle as a
  `ur:chainv4-multisig` code, animated, and the signer's code read back. **Approve on chain** goes
  to Send, so a hardware or offline account approves too. **Copy as text** gives what chainv4
  multisig reads. **Submit** once enough back it, under a key made for the moment: the multisig
  pays.
- **A bundle from another member:** paste its text, or read its code.

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
- **The carrier's key does not matter** to the chain: it signs the envelope so a node takes it,
  and pays nothing.
