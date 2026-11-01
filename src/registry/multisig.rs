//! Native multisig accounts: an Address::Multisig that M of its N members' keys control. Its
//! policy, the members and the threshold, is chain state, kept by the account, so the quorum can
//! change it and the address stays. Each member names its key here, any kind the chain takes, and
//! may name its keys on the chains a multisig reaches beyond this one: a Bitcoin key, for the
//! matching P2WSH M-of-N, and an EVM address, for the matching Safe-style contract.
//!
//! The multisig's own transactions are Tx::Multisig, carried by whoever submits one, with the
//! multisig's nonce and fee offer. They need threshold of the members: their signatures, collected
//! off chain into the transaction, and their approvals made on chain beforehand
//! (Tx::ApproveMultisig), count together. Approvals are for the multisig's next nonce: once any of
//! its transactions runs, every open proposal is void.

use {
    crate::crypto::hashable::Hashable,
    crate::crypto::keypair::PublicKey,
    crate::db::{Column, DbError, State},
    crate::registry::RegistryError,
    crate::structs::address::Address,
    crate::structs::hash::Hash,
    crate::structs::tx::{FeeOffer, Tx},
    r#macro::document,
    wincode::{SchemaRead, SchemaWrite},
};

#[document(summary = "The most members a multisig may have: CHECKMULTISIG's limit, which a Bitcoin P2WSH M-of-N is relayed up to, so every multisig can have its matching Bitcoin address.", category = "Multisig", see_also = "MultisigPolicy")]
pub const MAX_MEMBERS: usize = 20;

#[document(
    summary = "One member of a multisig: the key it signs with here, and its keys on the other chains the multisig reaches, each from the member's own words.",
    category = "Multisig",
    see_also = "MultisigPolicy",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct MultisigMember {
    #[document(summary = "Its key on this chain, any kind the chain takes: what it signs and approves the multisig's transactions with.")]
    pub key: PublicKey,
    #[document(summary = "Its multisig account xpub on Bitcoin, at m/48'/coin'/0'/2' on the policy's network, for the matching P2WSH M-of-N, or None. Every member names one, or none does.")]
    pub bitcoin: Option<BitcoinXpub>,
    #[document(summary = "Its address on EVM chains, for the matching Safe-style contract, or None. Every member names one, or none does.")]
    pub evm: Option<[u8; 20]>,
}

impl Hashable for MultisigMember {}

#[document(
    summary = "A member's multisig account on Bitcoin: its extended public key at m/48'/coin'/0'/2', BIP48's for native segwit multisig, and where it sits, so the multisig's addresses derive below it and Bitcoin's wallets name it as the member's own does.",
    category = "Multisig",
    see_also = "MultisigMember, MultisigPolicy::testnet",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct BitcoinXpub {
    #[document(summary = "The fingerprint of the member's seed, its master key: what a PSBT names the key's origin by.")]
    pub fingerprint: [u8; 4],
    #[document(summary = "The fingerprint of the key one step up, at m/48'/coin'/0'.")]
    pub parent: [u8; 4],
    #[document(summary = "The account's compressed key.")]
    pub public: [u8; 33],
    #[document(summary = "Its chain code.")]
    pub chain_code: [u8; 32],
}

impl Hashable for BitcoinXpub {}

#[document(
    summary = "Who controls a multisig: its members, and how many of them a transaction needs.",
    category = "Multisig",
    see_also = "Multisigs, Tx::CreateMultisig, Tx::ChangeMultisig",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct MultisigPolicy {
    #[document(summary = "The members, in the order given.")]
    pub members: Vec<MultisigMember>,
    #[document(summary = "How many of them each transaction needs: at least 1, at most all.")]
    pub threshold: u32,
    #[document(summary = "Which network the members' Bitcoin xpubs are for: false for Bitcoin, true for its test networks. A test multisig and a real one are separate multisigs.")]
    pub testnet: bool,
}

impl Hashable for MultisigPolicy {}

impl MultisigPolicy {
    #[document(
        parent = "MultisigPolicy",
        summary = "Why it cannot be a multisig's: no members or more than MAX_MEMBERS, a key, Bitcoin xpub or EVM address named twice, some members naming a Bitcoin xpub or EVM address and others not, an xpub whose key is not a compressed secp256k1 point, a zero EVM address, or a threshold of 0 or more than the members.",
    )]
    pub fn problem(&self) -> Option<&'static str> {
        let count = self.members.len();

        if count == 0 || count > MAX_MEMBERS {
            return Some("a multisig has 1 to 20 members");
        }

        if self.threshold == 0 || self.threshold as usize > count {
            return Some("a multisig's threshold is at least 1 and at most its members");
        }

        let distinct = |keys: Vec<Vec<u8>>| {
            let mut sorted = keys.clone();
            sorted.sort();
            sorted.dedup();

            sorted.len() == keys.len()
        };

        if !distinct(self.members.iter().map(|member| member.key.hashable_bytes_with_tag()).collect()) {
            return Some("a multisig names each member's key once");
        }

        let bitcoin: Vec<[u8; 33]> = self.members.iter().filter_map(|member| member.bitcoin.map(|xpub| xpub.public)).collect();
        let evm: Vec<[u8; 20]> = self.members.iter().filter_map(|member| member.evm).collect();

        if !(bitcoin.is_empty() || bitcoin.len() == count) || !(evm.is_empty() || evm.len() == count) {
            return Some("every member names a Bitcoin xpub and an EVM address, or none does");
        }

        if !distinct(bitcoin.iter().map(|key| key.to_vec()).collect()) || !distinct(evm.iter().map(|address| address.to_vec()).collect()) {
            return Some("a multisig names each Bitcoin xpub and EVM address once");
        }

        if bitcoin.iter().any(|key| !bitcoin_key(key)) {
            return Some("a member's Bitcoin xpub holds a compressed secp256k1 key");
        }

        if evm.contains(&[0; 20]) {
            return Some("a member's EVM address is not zero");
        }

        None
    }

    #[document(parent = "MultisigPolicy", summary = "The member whose key this is, if any.")]
    pub fn member(&self, key: &PublicKey) -> Option<&MultisigMember> {
        self.members.iter().find(|member| member.key == *key)
    }

    #[document(parent = "MultisigPolicy", summary = "The member whose key an account is, if any: whom a member's own transaction comes from.")]
    pub fn member_at(&self, account: &Address) -> Option<&MultisigMember> {
        self.members.iter().find(|member| member.key.address() == *account)
    }
}

/// Whether 33 bytes are a compressed secp256k1 key: on the curve, where the chain can tell.
fn bitcoin_key(key: &[u8; 33]) -> bool {
    #[cfg(feature = "secp256k1")]
    {
        k256::PublicKey::from_sec1_bytes(key).is_ok()
    }

    #[cfg(not(feature = "secp256k1"))]
    {
        matches!(key[0], 2 | 3)
    }
}

#[document(
    summary = "A transaction a multisig's members are asked to approve: what it does, and the nonce and fee offer it runs with. Its id, over the chain's genesis id too, is what members sign and approve.",
    category = "Multisig",
    see_also = "Tx::Multisig, Tx::ApproveMultisig",
)]
#[derive(Clone, Debug, PartialEq, SchemaWrite, SchemaRead)]
pub struct MultisigBody {
    #[document(summary = "The multisig it is from.")]
    pub account: Address,
    #[document(summary = "What it does, as the multisig.")]
    pub tx: Tx,
    #[document(summary = "The multisig's nonce it takes.")]
    pub nonce: u64,
    #[document(summary = "What the multisig offers to pay for it, from its own balance.")]
    pub fee: FeeOffer,
}

impl Hashable for MultisigBody {}

/// Keeps a member's signature apart from any other message its key signs.
const SIGNING_DOMAIN: &[u8] = b"chainv4 multisig transaction";

impl MultisigBody {
    #[document(
        parent = "MultisigBody",
        summary = "What a member signs: a domain tag, the chain's genesis id, so a signature holds on one chain only, and the body.",
        params = { genesis = "The chain's genesis id." },
    )]
    pub fn signing_bytes(&self, genesis: &Hash) -> Vec<u8> {
        [SIGNING_DOMAIN, &genesis.hashable_bytes_with_tag(), &self.hashable_bytes_with_tag()].concat()
    }

    #[document(
        parent = "MultisigBody",
        summary = "Its id: what an approval on chain names, the chain hash of what a member signs.",
        params = { genesis = "The chain's genesis id, whose hash it is hashed with." },
    )]
    pub fn id(&self, genesis: &Hash) -> Hash {
        crate::crypto::hasher::Hasher::digest(genesis.algorithm(), &self.signing_bytes(genesis))
    }
}

#[document(
    summary = "A transaction proposed on chain for a multisig's next nonce, and the members that approved it.",
    category = "Multisig",
    see_also = "Proposals",
)]
#[derive(Clone, Debug, PartialEq, SchemaWrite, SchemaRead)]
pub struct Proposal {
    #[document(summary = "Its id: MultisigBody::id.")]
    pub id: Hash,
    #[document(summary = "What it does.")]
    pub tx: Tx,
    #[document(summary = "Its fee offer.")]
    pub fee: FeeOffer,
    #[document(summary = "The members' keys that approve it, in the order they did.")]
    pub approvals: Vec<PublicKey>,
}

#[document(
    summary = "A multisig's open proposals, all for one nonce. Once the multisig's nonce moves past it, they are void and read as none.",
    category = "Multisig",
    see_also = "Multisigs::proposals",
)]
#[derive(Clone, Debug, Default, PartialEq, SchemaWrite, SchemaRead)]
pub struct Proposals {
    #[document(summary = "The nonce they are for.")]
    pub nonce: u64,
    #[document(summary = "Each proposal, in the order first approved. A member approves one at a time, so there are never more than the members.")]
    pub proposals: Vec<Proposal>,
}

impl Hashable for Proposals {}

#[document(
    summary = "The multisig registry: each multisig's policy and its open proposals, in Column::Accounts beside the account.",
    category = "Multisig",
    see_also = "MultisigPolicy, Proposals",
)]
pub struct Multisigs<'a> {
    state: &'a State,
}

#[document(
    summary = "The owners and threshold a multisig's EVM contract starts with: its members' EVM addresses, sorted, and its threshold, when they first named them. What the contract's address on every EVM chain commits to, so kept as they were whatever the multisig's members become.",
    category = "Multisig",
    see_also = "Multisigs::evm_start, multisig_contract",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct EvmStart {
    #[document(summary = "The members' EVM addresses as it was made, sorted.")]
    pub owners: Vec<[u8; 20]>,
    #[document(summary = "Its threshold as it was made.")]
    pub threshold: u32,
}

impl Hashable for EvmStart {}

impl MultisigPolicy {
    #[document(parent = "MultisigPolicy", summary = "Its members' EVM addresses, sorted, and its threshold: what its EVM contract's owners should be; None when its members name none.")]
    pub fn evm(&self) -> Option<EvmStart> {
        let mut owners = self.members.iter().map(|member| member.evm).collect::<Option<Vec<_>>>()?;
        owners.sort();

        Some(EvmStart { owners, threshold: self.threshold })
    }
}

fn evm_start_key(account: &Address) -> Vec<u8> {
    [b"multisig-evm:".as_slice(), &account.hashable_bytes_with_tag()].concat()
}

fn policy_key(account: &Address) -> Vec<u8> {
    [b"multisig:".as_slice(), &account.hashable_bytes_with_tag()].concat()
}

fn proposals_key(account: &Address) -> Vec<u8> {
    [b"proposals:".as_slice(), &account.hashable_bytes_with_tag()].concat()
}

impl<'a> Multisigs<'a> {
    #[document(parent = "Multisigs", summary = "Reads and writes multisigs in the given state layer.")]
    pub fn new(state: &'a State) -> Multisigs<'a> {
        Multisigs { state }
    }

    #[document(parent = "Multisigs", summary = "Where a multisig's policy is kept, in Column::Accounts: what a light client asks a proof of.")]
    pub fn policy_key(account: &Address) -> Vec<u8> {
        policy_key(account)
    }

    #[document(parent = "Multisigs", summary = "The owners and threshold a multisig's EVM contract starts with: its members' EVM addresses and threshold when they first named them, at its making or a change since; None while they name none.")]
    pub fn evm_start(&self, account: &Address) -> Result<Option<EvmStart>, DbError> {
        self.state.get_value(Column::Accounts, &evm_start_key(account))
    }

    #[document(parent = "Multisigs", summary = "A multisig's policy, or None for an address that is not one.")]
    pub fn get(&self, account: &Address) -> Result<Option<MultisigPolicy>, DbError> {
        self.state.get_value(Column::Accounts, &policy_key(account))
    }

    #[document(
        parent = "Multisigs",
        summary = "Makes a new multisig at an address with a policy.",
        returns = "BadMultisig for a policy no multisig may have, MultisigExists for an address already taken.",
    )]
    pub fn create(&self, account: &Address, policy: MultisigPolicy) -> Result<(), RegistryError> {
        if let Some(problem) = policy.problem() {
            return Err(RegistryError::BadMultisig(problem));
        }

        if self.get(account)?.is_some() {
            return Err(RegistryError::MultisigExists);
        }

        // Its EVM contract's address commits to these, so they are kept as they are now.
        if let Some(start) = policy.evm() {
            self.state.put_value(Column::Accounts, evm_start_key(account), &start);
        }

        self.state.put_value(Column::Accounts, policy_key(account), &policy);

        Ok(())
    }

    #[document(
        parent = "Multisigs",
        summary = "Replaces a multisig's policy, as its own transaction does: the address stays, and the open proposals go.",
        returns = "NotMultisig for an address that is not one, BadMultisig for a policy no multisig may have.",
    )]
    pub fn change(&self, account: &Address, policy: MultisigPolicy) -> Result<(), RegistryError> {
        if let Some(problem) = policy.problem() {
            return Err(RegistryError::BadMultisig(problem));
        }

        self.get(account)?.ok_or(RegistryError::NotMultisig)?;

        // A multisig whose members name EVM addresses for the first time starts its contract here.
        if let Some(start) = policy.evm()
            && self.evm_start(account)?.is_none()
        {
            self.state.put_value(Column::Accounts, evm_start_key(account), &start);
        }

        self.state.put_value(Column::Accounts, policy_key(account), &policy);
        self.clear(account);

        Ok(())
    }

    #[document(
        parent = "Multisigs",
        summary = "The open proposals for a multisig's next nonce: none once the nonce moved past those kept.",
        params = { nonce = "The multisig's next nonce." },
    )]
    pub fn proposals(&self, account: &Address, nonce: u64) -> Result<Proposals, DbError> {
        let kept: Option<Proposals> = self.state.get_value(Column::Accounts, &proposals_key(account))?;

        Ok(kept.filter(|kept| kept.nonce == nonce).unwrap_or(Proposals { nonce, proposals: Vec::new() }))
    }

    #[document(
        parent = "Multisigs",
        summary = "Counts a member's approval of a transaction for the multisig's next nonce, moving its approval from any other it made for that nonce.",
        params = {
            approver = "The account approving, a member's key's.",
            body = "What it approves, for the multisig's next nonce.",
            next = "The multisig's next nonce.",
            genesis = "The chain's genesis id, which the proposal's id is over.",
        },
        returns = "How many members approve it now. NotMultisig, NotMember for an account no member's key is, StaleMultisigNonce for a body for another nonce.",
    )]
    pub fn approve(&self, approver: &Address, body: &MultisigBody, next: u64, genesis: &Hash) -> Result<u32, RegistryError> {
        let policy = self.get(&body.account)?.ok_or(RegistryError::NotMultisig)?;
        let member = policy.member_at(approver).ok_or(RegistryError::NotMember)?.key;

        if body.nonce != next {
            return Err(RegistryError::StaleMultisigNonce(next));
        }

        let id = body.id(genesis);
        let mut open = self.proposals(&body.account, next)?;

        for proposal in &mut open.proposals {
            proposal.approvals.retain(|key| *key != member);
        }

        let approvals = match open.proposals.iter_mut().find(|proposal| proposal.id == id) {
            Some(proposal) => {
                proposal.approvals.push(member);

                proposal.approvals.len()
            }
            None => {
                open.proposals.push(Proposal { id, tx: body.tx.clone(), fee: body.fee, approvals: vec![member] });

                1
            }
        };
        open.proposals.retain(|proposal| !proposal.approvals.is_empty());
        self.state.put_value(Column::Accounts, proposals_key(&body.account), &open);

        Ok(approvals as u32)
    }

    #[document(
        parent = "Multisigs",
        summary = "Takes back a member's approval of an open proposal.",
        params = { next = "The multisig's next nonce." },
        returns = "NotMultisig, NotMember, NoApproval when the member backs no open proposal with that id.",
    )]
    pub fn withdraw(&self, approver: &Address, account: &Address, proposal: &Hash, next: u64) -> Result<(), RegistryError> {
        let policy = self.get(account)?.ok_or(RegistryError::NotMultisig)?;
        let member = policy.member_at(approver).ok_or(RegistryError::NotMember)?.key;
        let mut open = self.proposals(account, next)?;
        let backed = open.proposals.iter_mut().find(|open| open.id == *proposal && open.approvals.contains(&member)).ok_or(RegistryError::NoApproval)?;
        backed.approvals.retain(|key| *key != member);
        open.proposals.retain(|proposal| !proposal.approvals.is_empty());

        match open.proposals.is_empty() {
            true => self.clear(account),
            false => self.state.put_value(Column::Accounts, proposals_key(account), &open),
        }

        Ok(())
    }

    #[document(
        parent = "Multisigs",
        summary = "Who backs a body: the members whose signatures it carries, each checked against the chain's genesis id, and those that approved it on chain, each once.",
        params = {
            body = "The multisig's transaction.",
            signatures = "The members' signatures it carries.",
            genesis = "The chain's genesis id.",
        },
        returns = "The policy and how many members back it. NotMultisig, NotMember for a signature by a key no member has, BadMemberSignature for one that does not check out.",
    )]
    pub fn backing(&self, body: &MultisigBody, signatures: &[crate::structs::tx::MemberSignature], genesis: &Hash) -> Result<(MultisigPolicy, u32), RegistryError> {
        let policy = self.get(&body.account)?.ok_or(RegistryError::NotMultisig)?;
        let bytes = body.signing_bytes(genesis);
        let mut backers: Vec<PublicKey> = Vec::new();
        let mut verified: Vec<&crate::structs::tx::MemberSignature> = Vec::new();

        for signed in signatures {
            policy.member(&signed.key).ok_or(RegistryError::NotMember)?;

            // The same signature sent again is the same work and the same answer, so it is verified
            // once however many copies ride along: a transaction carrying twenty copies of one
            // member's signature costs one verify, not twenty. A different signature, even from the
            // same member, is still verified, so a bad one is still refused.
            if !verified.contains(&signed) {
                signed.key.verify_signature(&bytes, &signed.signature).map_err(|_| RegistryError::BadMemberSignature)?;
                verified.push(signed);
            }

            if !backers.contains(&signed.key) {
                backers.push(signed.key);
            }
        }

        let id = body.id(genesis);

        if let Some(proposal) = self.proposals(&body.account, body.nonce)?.proposals.into_iter().find(|proposal| proposal.id == id) {
            for key in proposal.approvals {
                if policy.member(&key).is_some() && !backers.contains(&key) {
                    backers.push(key);
                }
            }
        }

        Ok((policy, backers.len() as u32))
    }

    #[document(parent = "Multisigs", summary = "Forgets a multisig's open proposals: what running one of its transactions does.")]
    pub fn clear(&self, account: &Address) {
        self.state.delete(Column::Accounts, proposals_key(account));
    }
}

#[document(
    summary = "A multisig's transaction on its way round the members: the body, the chain it is for, and the signatures collected so far. Passed as text (MultisigBundle::to_text), as a file or a code, until it holds enough to submit.",
    category = "Multisig",
    see_also = "MultisigBody, Tx::Multisig",
)]
#[derive(Clone, Debug, PartialEq, SchemaWrite, SchemaRead)]
pub struct MultisigBundle {
    #[document(summary = "The chain's genesis id, which the signatures are over.")]
    pub genesis: Hash,
    #[document(summary = "The transaction.")]
    pub body: MultisigBody,
    #[document(summary = "The members' signatures so far, each key once.")]
    pub signatures: Vec<crate::structs::tx::MemberSignature>,
}

impl Hashable for MultisigBundle {}

/// What a bundle's text starts with, so a paste is told from any other.
const BUNDLE_PREFIX: &str = "chainv4-multisig:";

impl MultisigBundle {
    #[document(parent = "MultisigBundle", summary = "Adds a member's signature, replacing any earlier one by the same key.")]
    pub fn sign(&mut self, keypair: &crate::crypto::keypair::Keypair) {
        let signed = crate::structs::tx::MemberSignature::sign(&self.body, keypair, &self.genesis);
        self.signatures.retain(|held| held.key != signed.key);
        self.signatures.push(signed);
    }

    #[document(
        parent = "MultisigBundle",
        summary = "The multisig's transaction it makes, carried by a key: the envelope a node takes.",
        params = { carrier = "Whoever submits it; it pays nothing." },
    )]
    pub fn envelope(&self, carrier: &crate::crypto::keypair::Keypair) -> crate::structs::tx::SignedTx {
        let tx = Tx::Multisig { account: self.body.account, tx: Box::new(self.body.tx.clone()), signatures: self.signatures.clone() };

        crate::structs::tx::SignedTx::sign(tx, self.body.nonce, self.body.fee, carrier)
    }

    #[document(parent = "MultisigBundle", summary = "Its text: chainv4-multisig: and its encoding in hex, to save or paste.")]
    pub fn to_text(&self) -> String {
        format!("{BUNDLE_PREFIX}{}", crate::structs::hex::to_hex(&self.hashable_bytes_with_tag()))
    }

    #[document(parent = "MultisigBundle", summary = "Reads a bundle's text back, whitespace around it ignored.", returns = "None for text that is not one.")]
    pub fn from_text(text: &str) -> Option<MultisigBundle> {
        let bytes = crate::structs::hex::from_hex(text.trim().strip_prefix(BUNDLE_PREFIX)?)?;

        wincode::deserialize(&bytes).ok()
    }
}
