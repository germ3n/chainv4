use {
    super::RegistryError,
    crate::crypto::hashable::Hashable,
    crate::crypto::keypair::PublicKey,
    crate::db::{Column, DbError, State},
    crate::registry::validators::{Member, ValidatorSet},
    crate::structs::address::Address,
    crate::structs::u256::u256,
    r#macro::document,
    wincode::{SchemaRead, SchemaWrite},
};

/// The set the signers decided on last, which the next checkpoints fix for later epochs.
const DECIDED: &[u8] = b"signers:decided";
/// Approvals for a change from the decided set.
const BALLOT: &[u8] = b"signers:ballot";

#[document(
    summary = "One signer under authority finality: the key that signs its votes, and the account that speaks for it on chain, which sends its approvals of changes and gets its rewards. The account is the key's own address unless genesis or the approved set names another, so the voting key can stay on the validating node and sign nothing but votes, as an FFG voter's does.",
    category = "Finality",
    see_also = "SignerSet, GenesisValidator",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct Signer {
    #[document(summary = "The key that signs its votes.")]
    pub key: PublicKey,
    #[document(summary = "The account that approves changes for it and is paid its rewards.")]
    pub account: Address,
}

impl Hashable for Signer {}

impl Signer {
    #[document(parent = "Signer", summary = "A signer whose account is its own key's address.")]
    pub fn own(key: PublicKey) -> Signer {
        Signer { key, account: key.address() }
    }
}

#[document(
    summary = "Who signs under authority finality, and what it takes: how many of them justify a checkpoint, and how many change the set.",
    category = "Finality",
    see_also = "Signers, Signer, AuthorityConfig, Tx::ApproveSigners",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct SignerSet {
    #[document(summary = "The signers, sorted by key, each counting once.")]
    pub signers: Vec<Signer>,
    #[document(summary = "How many of them voting for one link justify it.")]
    pub threshold: u32,
    #[document(summary = "How many of them must approve a change to the set.")]
    pub change_threshold: u32,
}

impl Hashable for SignerSet {}

impl SignerSet {
    #[document(
        parent = "SignerSet",
        summary = "A set with its signers sorted by key, which is how sets are compared and stored.",
    )]
    pub fn new(mut signers: Vec<Signer>, threshold: u32, change_threshold: u32) -> SignerSet {
        signers.sort_by_key(|signer| signer.key);

        SignerSet {
            signers,
            threshold,
            change_threshold,
        }
    }

    #[document(parent = "SignerSet", summary = "The signers' keys, in the set's order.")]
    pub fn keys(&self) -> Vec<PublicKey> {
        self.signers.iter().map(|signer| signer.key).collect()
    }

    #[document(
        parent = "SignerSet",
        summary = "Why no chain could use this set, if it is unusable: no signers, a signer named twice, two sharing an account, more than max_signers, or a threshold that is not more than half the signers.",
    )]
    pub fn problem(&self, max_signers: u32) -> Option<&'static str> {
        if self.signers.is_empty() {
            return Some("authority finality needs at least one signer");
        }

        if self.signers.windows(2).any(|pair| pair[0].key == pair[1].key) {
            return Some("a signer is named twice");
        }

        let mut accounts: Vec<Address> = self.signers.iter().map(|signer| signer.account).collect();
        accounts.sort();

        if accounts.windows(2).any(|pair| pair[0] == pair[1]) {
            return Some("two signers share an account");
        }

        if self.signers.len() > max_signers as usize {
            return Some("more signers than max_signers");
        }

        // More than half, so two rival links, or two rival changes, cannot both have enough.
        let count = self.signers.len() as u64;
        let fits = |threshold: u32| u64::from(threshold) * 2 > count && u64::from(threshold) <= count;

        if !fits(self.threshold) {
            return Some("threshold must be more than half the signers, and at most all of them");
        }

        if !fits(self.change_threshold) {
            return Some("change_threshold must be more than half the signers, and at most all of them");
        }

        None
    }

    #[document(parent = "SignerSet", summary = "The key of the signer whose account this address is, if one's is.")]
    pub fn signer_at(&self, address: &Address) -> Option<PublicKey> {
        self.signers.iter().find(|signer| signer.account == *address).map(|signer| signer.key)
    }

    #[document(
        parent = "SignerSet",
        summary = "The signers as a validator set: each weighs one, votes with its key and is paid at its account.",
    )]
    pub fn validators(&self) -> ValidatorSet {
        ValidatorSet {
            members: self
                .signers
                .iter()
                .map(|signer| Member {
                    voter: signer.key,
                    account: signer.account,
                    weight: u256::ONE,
                    // Authority's signers attest nothing: Ethereum follows FFG's validators.
                    attester: None,
                })
                .collect(),
        }
    }
}

#[document(
    summary = "The set the signers decided on last, and how many changes came before it.",
    category = "Finality",
    see_also = "Signers",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct Decided {
    #[document(summary = "How many changes have been decided since genesis. An approval names the version it changes, so approvals for an older set never count.")]
    pub version: u64,
    #[document(summary = "The set itself.")]
    pub set: SignerSet,
}

impl Hashable for Decided {}

#[document(
    summary = "One signer's approval of a change.",
    category = "Finality",
    see_also = "Ballot",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct Approval {
    #[document(summary = "The approving signer.")]
    pub signer: PublicKey,
    #[document(summary = "The set it approves.")]
    pub proposal: SignerSet,
    #[document(summary = "The epoch it was made in. It counts for approval_epochs epochs from there, and approving again starts it afresh.")]
    pub epoch: u64,
}

impl Approval {
    #[document(parent = "Approval", summary = "Whether it still counts in an epoch, when approvals count for approval_epochs epochs.")]
    pub fn counts(&self, epoch: u64, approval_epochs: u64) -> bool {
        epoch < self.epoch.saturating_add(approval_epochs)
    }
}

#[document(
    summary = "The approvals for changing the decided set, at most one per signer. A signer that approves another change moves its approval there, and one that withdraws takes it back. An approval stops counting approval_epochs epochs after the one it was made in.",
    category = "Finality",
    see_also = "Signers::approve",
)]
#[derive(Clone, Debug, Default, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct Ballot {
    #[document(summary = "The version the approvals change.")]
    pub version: u64,
    #[document(summary = "The approvals, in the order they came.")]
    pub approvals: Vec<Approval>,
}

impl Hashable for Ballot {}

impl Ballot {
    #[document(
        parent = "Ballot",
        summary = "The ballot as it stands in an epoch: only the approvals that still count.",
        params = {
            epoch = "The epoch to count in.",
            approval_epochs = "How many epochs an approval counts for.",
        },
    )]
    pub fn counting(mut self, epoch: u64, approval_epochs: u64) -> Ballot {
        self.approvals.retain(|approval| approval.counts(epoch, approval_epochs));
        self
    }

    #[document(parent = "Ballot", summary = "Who approved a proposal.")]
    pub fn approvers(&self, proposal: &SignerSet) -> Vec<PublicKey> {
        self.approvals
            .iter()
            .filter(|approval| approval.proposal == *proposal)
            .map(|approval| approval.signer)
            .collect()
    }
}

#[document(
    summary = "What an approval did.",
    category = "Finality",
    see_also = "Signers::approve",
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Approved {
    #[document(summary = "It counted, and the proposal now has this many of the approvals it needs.")]
    Counted { approvals: u32, needed: u32 },
    #[document(summary = "It was the last one needed: the proposal is decided, as this version.")]
    Decided(u64),
}

#[document(
    summary = "Authority finality's signers, in chain state: the set fixed for each epoch, the set decided last, and the ballot for changing it. A change the current signers approve is fixed for epochs activation_delay on, like FFG's validator sets, so every branch that split off after the checkpoint that fixed a set agrees on it.",
    category = "Finality",
    see_also = "SignerSet, Tx::ApproveSigners",
)]
pub struct Signers<'a> {
    state: &'a State,
}

impl<'a> Signers<'a> {
    #[document(parent = "Signers", summary = "The signers in a state layer.")]
    pub fn new(state: &'a State) -> Signers<'a> {
        Signers { state }
    }

    #[document(
        parent = "Signers",
        summary = "Writes genesis's signers as the decided set, and fixes them for the epochs genesis already decides.",
    )]
    pub fn write_genesis(&self, set: &SignerSet, activation_delay: u64) -> Result<(), DbError> {
        self.state.put_value(Column::Validators, DECIDED.to_vec(), &Decided {
            version: 0,
            set: set.clone(),
        });

        for epoch in 0..=activation_delay {
            self.fix_set(epoch)?;
        }

        Ok(())
    }

    #[document(
        parent = "Signers",
        summary = "Makes a set the decided one as finality switches to authority: one version after any decided before, the ballot emptied, so approvals made under an earlier authority era never count.",
    )]
    pub fn start(&self, set: &SignerSet) -> Result<(), DbError> {
        let version = self.state.get_value::<Decided>(Column::Validators, DECIDED)?.map_or(0, |decided| decided.version + 1);
        self.state.put_value(Column::Validators, DECIDED.to_vec(), &Decided { version, set: set.clone() });
        self.state.delete(Column::Validators, BALLOT.to_vec());

        Ok(())
    }

    #[document(
        parent = "Signers",
        summary = "The account a signer of the set decided last speaks through, or None for a key it does not name, or a chain that never ran authority finality.",
    )]
    pub fn account_of(&self, key: &PublicKey) -> Result<Option<Address>, DbError> {
        let decided: Option<Decided> = self.state.get_value(Column::Validators, DECIDED)?;

        Ok(decided.and_then(|decided| decided.set.signers.iter().find(|signer| signer.key == *key).map(|signer| signer.account)))
    }

    #[document(parent = "Signers", summary = "The signers fixed for an epoch, if any are yet.")]
    pub fn set(&self, epoch: u64) -> Result<Option<SignerSet>, DbError> {
        self.state.get_value(Column::Epochs, &Signers::set_key(epoch))
    }

    #[document(parent = "Signers", summary = "Where an epoch's signers are kept, in Column::Epochs: what a light client asks a proof of.")]
    pub fn set_key(epoch: u64) -> Vec<u8> {
        epoch_key(epoch)
    }

    #[document(parent = "Signers", summary = "Fixes the decided set for an epoch. Each checkpoint fixes the epoch activation_delay ahead.")]
    pub fn fix_set(&self, epoch: u64) -> Result<(), DbError> {
        let decided = self.decided()?;
        self.state.put_value(Column::Epochs, epoch_key(epoch), &decided.set);

        Ok(())
    }

    #[document(parent = "Signers", summary = "The set decided last, and its version.")]
    pub fn decided(&self) -> Result<Decided, DbError> {
        self.state
            .get_value(Column::Validators, DECIDED)?
            .ok_or_else(|| DbError::Corrupt("no decided signer set".to_string()))
    }

    #[document(
        parent = "Signers",
        summary = "The approvals so far for changing the decided set, expired ones included: Ballot::counting leaves only those that count.",
    )]
    pub fn ballot(&self) -> Result<Ballot, DbError> {
        let decided = self.decided()?;
        let ballot: Ballot = self.state.get_value(Column::Validators, BALLOT)?.unwrap_or_default();

        // Approvals for an older version never count.
        Ok(match ballot.version == decided.version {
            true => ballot,
            false => Ballot {
                version: decided.version,
                approvals: Vec::new(),
            },
        })
    }

    #[document(
        parent = "Signers",
        summary = "Counts a signer's approval of a new set. Once change_threshold of the decided set's signers approve the same one with approvals that still count, it becomes the decided set, and the checkpoints that follow fix it for later epochs.",
        params = {
            sender = "Who sent the approval: the account of one of the decided set's signers.",
            version = "The version it changes, which must be the decided one's.",
            proposal = "The set it approves.",
            max_signers = "The most signers a set may have.",
            epoch = "The epoch the approval is made in.",
            approval_epochs = "How many epochs an approval counts for.",
        },
        returns = "Whether that decided it. Fails for a sender that is not a signer, an older version, an unusable set, or the set already decided.",
    )]
    pub fn approve(
        &self,
        sender: &Address,
        version: u64,
        proposal: SignerSet,
        max_signers: u32,
        epoch: u64,
        approval_epochs: u64,
    ) -> Result<Approved, RegistryError> {
        let (decided, signer) = self.signer(sender, version)?;
        let proposal = SignerSet::new(proposal.signers, proposal.threshold, proposal.change_threshold);

        if let Some(problem) = proposal.problem(max_signers) {
            return Err(RegistryError::BadSigners(problem));
        }

        if proposal == decided.set {
            return Err(RegistryError::SameSigners);
        }

        let mut ballot = self.ballot()?.counting(epoch, approval_epochs);
        ballot.approvals.retain(|approval| approval.signer != signer);
        ballot.approvals.push(Approval {
            signer,
            proposal: proposal.clone(),
            epoch,
        });
        let approvals = ballot.approvers(&proposal).len() as u32;
        let needed = decided.set.change_threshold;

        if approvals < needed {
            self.state.put_value(Column::Validators, BALLOT.to_vec(), &ballot);

            return Ok(Approved::Counted { approvals, needed });
        }

        let version = decided.version + 1;
        self.state.put_value(Column::Validators, DECIDED.to_vec(), &Decided { version, set: proposal });
        self.state.delete(Column::Validators, BALLOT.to_vec());

        Ok(Approved::Decided(version))
    }

    #[document(
        parent = "Signers",
        summary = "Takes back a signer's approval, while it still counts, so it backs no change.",
        params = {
            sender = "The signer's account.",
            version = "The version its approval was for, which must be the decided one's.",
            epoch = "The epoch it is withdrawn in.",
            approval_epochs = "How many epochs an approval counts for.",
        },
        panics = "NotSigner for a sender that is not a signer, StaleSigners for an older version, NoApproval when it has no approval that still counts.",
    )]
    pub fn withdraw(&self, sender: &Address, version: u64, epoch: u64, approval_epochs: u64) -> Result<(), RegistryError> {
        let (_, signer) = self.signer(sender, version)?;
        let mut ballot = self.ballot()?.counting(epoch, approval_epochs);
        let before = ballot.approvals.len();
        ballot.approvals.retain(|approval| approval.signer != signer);

        if ballot.approvals.len() == before {
            return Err(RegistryError::NoApproval);
        }

        match ballot.approvals.is_empty() {
            true => self.state.delete(Column::Validators, BALLOT.to_vec()),
            false => self.state.put_value(Column::Validators, BALLOT.to_vec(), &ballot),
        }

        Ok(())
    }

    #[document(
        parent = "Signers",
        summary = "Throws a signer that signed conflicting votes out: it leaves the decided set, which keeps its thresholds and takes a new version, emptying the ballot, and the sets already fixed for the epochs from two after this one on, as a slashed FFG validator does. Refused when a set it leaves could no longer reach its thresholds.",
        params = {
            voter = "The cheating signer's voting key.",
            epoch = "The epoch the evidence runs in.",
            activation_delay = "How many epochs ahead sets are fixed.",
            max_signers = "The most signers a set may have.",
        },
        returns = "The new version. UnknownSigner for a key not in the decided set, BadSigners when a set would be left unusable.",
    )]
    pub fn eject(&self, voter: &PublicKey, epoch: u64, activation_delay: u64, max_signers: u32) -> Result<u64, RegistryError> {
        let decided = self.decided()?;

        if !decided.set.signers.iter().any(|signer| signer.key == *voter) {
            return Err(RegistryError::UnknownSigner);
        }

        let without = |set: &SignerSet| -> Result<SignerSet, RegistryError> {
            let signers = set.signers.iter().copied().filter(|signer| signer.key != *voter).collect();
            let set = SignerSet::new(signers, set.threshold, set.change_threshold);

            match set.problem(max_signers) {
                Some(problem) => Err(RegistryError::BadSigners(problem)),
                None => Ok(set),
            }
        };
        let set = without(&decided.set)?;
        let mut fixed = Vec::new();

        for epoch in epoch + 2..=epoch + activation_delay {
            if let Some(current) = self.set(epoch)?
                && current.signers.iter().any(|signer| signer.key == *voter)
            {
                fixed.push((epoch, without(&current)?));
            }
        }

        let version = decided.version + 1;
        self.state.put_value(Column::Validators, DECIDED.to_vec(), &Decided { version, set });
        self.state.delete(Column::Validators, BALLOT.to_vec());

        for (epoch, set) in fixed {
            self.state.put_value(Column::Epochs, epoch_key(epoch), &set);
        }

        Ok(version)
    }

    /// The decided set and the key of the signer whose account sent, for a ballot on the decided
    /// version.
    fn signer(&self, sender: &Address, version: u64) -> Result<(Decided, PublicKey), RegistryError> {
        let decided = self.decided()?;

        if version != decided.version {
            return Err(RegistryError::StaleSigners(decided.version));
        }

        let signer = decided.set.signer_at(sender).ok_or(RegistryError::NotSigner)?;

        Ok((decided, signer))
    }
}

fn epoch_key(epoch: u64) -> Vec<u8> {
    let mut key = b"signers:".to_vec();
    key.extend_from_slice(&epoch.to_be_bytes());

    key
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::MemoryDb;
    use std::sync::Arc;

    /// A real key from a fixed seed, in whichever scheme this build has.
    fn key(seed: u8) -> PublicKey {
        crate::crypto::keypair::Keypair::from_seed([seed; 32]).public()
    }

    /// Signers whose accounts are their own keys' addresses.
    fn own(keys: impl IntoIterator<Item = PublicKey>) -> Vec<Signer> {
        keys.into_iter().map(Signer::own).collect()
    }

    /// Three signers, two of whom justify and two of whom change the set.
    fn genesis() -> (State, SignerSet) {
        let state = State::new(Arc::new(MemoryDb::default()));
        let set = SignerSet::new(own([key(1), key(2), key(3)]), 2, 2);
        Signers::new(&state).write_genesis(&set, 2).unwrap();

        (state, set)
    }

    fn with_fourth() -> SignerSet {
        SignerSet::new(own([key(1), key(2), key(3), key(4)]), 3, 3)
    }

    /// How many epochs approvals count for in these tests.
    const WINDOW: u64 = 4;

    #[test]
    fn genesis_signers_are_fixed_for_the_first_epochs() {
        let (state, set) = genesis();
        let signers = Signers::new(&state);

        for epoch in 0..=2 {
            assert_eq!(signers.set(epoch).unwrap(), Some(set.clone()));
        }

        assert_eq!(signers.set(3).unwrap(), None);
        assert_eq!(signers.decided().unwrap(), Decided { version: 0, set });
        assert_eq!(signers.ballot().unwrap(), Ballot::default());
    }

    #[test]
    fn enough_approvals_decide_a_change_that_later_checkpoints_fix() {
        let (state, set) = genesis();
        let signers = Signers::new(&state);
        let first = signers.approve(&key(1).address(), 0, with_fourth(), 10, 0, WINDOW).unwrap();

        assert_eq!(first, Approved::Counted { approvals: 1, needed: 2 });
        assert_eq!(signers.ballot().unwrap().approvers(&with_fourth()), [key(1)]);
        assert_eq!(signers.approve(&key(3).address(), 0, with_fourth(), 10, 0, WINDOW).unwrap(), Approved::Decided(1));
        assert_eq!(signers.decided().unwrap(), Decided { version: 1, set: with_fourth() });
        assert!(signers.ballot().unwrap().approvals.is_empty());

        // Already fixed epochs keep the old set; the next checkpoint fixes the new one.
        assert_eq!(signers.set(2).unwrap(), Some(set));
        signers.fix_set(3).unwrap();

        assert_eq!(signers.set(3).unwrap(), Some(with_fourth()));
        assert_eq!(signers.set(3).unwrap().unwrap().validators().total(), u256::new(4));
    }

    #[test]
    fn a_signer_has_one_approval_at_a_time() {
        let (state, _) = genesis();
        let signers = Signers::new(&state);
        let without_third = SignerSet::new(own([key(1), key(2)]), 2, 2);
        signers.approve(&key(1).address(), 0, with_fourth(), 10, 0, WINDOW).unwrap();
        signers.approve(&key(1).address(), 0, without_third.clone(), 10, 0, WINDOW).unwrap();
        let ballot = signers.ballot().unwrap();

        assert!(ballot.approvers(&with_fourth()).is_empty());
        assert_eq!(ballot.approvers(&without_third), [key(1)]);

        // Its earlier approval no longer counts toward the first proposal.
        assert_eq!(
            signers.approve(&key(2).address(), 0, with_fourth(), 10, 0, WINDOW).unwrap(),
            Approved::Counted { approvals: 1, needed: 2 }
        );
    }

    #[test]
    fn bad_approvals_are_refused() {
        let (state, set) = genesis();
        let signers = Signers::new(&state);
        let approve = |sender: PublicKey, version: u64, proposal: SignerSet| {
            signers.approve(&sender.address(), version, proposal, 4, 0, WINDOW)
        };

        assert_eq!(approve(key(9), 0, with_fourth()), Err(RegistryError::NotSigner));
        assert_eq!(approve(key(1), 1, with_fourth()), Err(RegistryError::StaleSigners(0)));
        assert_eq!(approve(key(1), 0, set.clone()), Err(RegistryError::SameSigners));

        let cases = [
            (SignerSet::new(Vec::new(), 1, 1), "at least one signer"),
            (SignerSet::new(own([key(1), key(1)]), 2, 2), "named twice"),
            (SignerSet::new(vec![Signer::own(key(1)), Signer { key: key(2), account: key(1).address() }], 2, 2), "share an account"),
            (SignerSet::new(own((1..=5).map(key)), 3, 3), "max_signers"),
            (SignerSet::new(own([key(1), key(2), key(3), key(4)]), 2, 3), "threshold must"),
            (SignerSet::new(own([key(1), key(2), key(3), key(4)]), 3, 5), "change_threshold must"),
        ];

        for (proposal, expected) in cases {
            match approve(key(1), 0, proposal) {
                Err(RegistryError::BadSigners(why)) => assert!(why.contains(expected), "{why}"),
                other => panic!("expected {expected}, got {other:?}"),
            }
        }

        // Once a change is decided, approvals for the version before it are stale.
        approve(key(1), 0, with_fourth()).unwrap();
        approve(key(2), 0, with_fourth()).unwrap();

        assert_eq!(approve(key(3), 0, with_fourth()), Err(RegistryError::StaleSigners(1)));
        assert_eq!(
            approve(key(4), 1, set).unwrap(),
            Approved::Counted { approvals: 1, needed: 3 },
            "the new signer approves, against the new change_threshold"
        );
    }

    #[test]
    fn approvals_stop_counting_once_they_expire() {
        let (state, _) = genesis();
        let signers = Signers::new(&state);
        signers.approve(&key(1).address(), 0, with_fourth(), 10, 3, WINDOW).unwrap();

        // Made in epoch 3, it counts in epochs 3 to 6.
        assert_eq!(signers.ballot().unwrap().counting(6, WINDOW).approvers(&with_fourth()), [key(1)]);
        assert!(signers.ballot().unwrap().counting(7, WINDOW).approvals.is_empty());

        // A second approval in epoch 7 finds the first expired, so it decides nothing.
        assert_eq!(
            signers.approve(&key(2).address(), 0, with_fourth(), 10, 7, WINDOW).unwrap(),
            Approved::Counted { approvals: 1, needed: 2 }
        );
        assert_eq!(signers.ballot().unwrap().approvals.len(), 1, "the expired approval is let go");

        // Approving again starts the clock afresh, and now two count.
        assert_eq!(
            signers.approve(&key(1).address(), 0, with_fourth(), 10, 8, WINDOW).unwrap(),
            Approved::Decided(1)
        );
    }

    #[test]
    fn a_signer_can_take_its_approval_back() {
        let (state, _) = genesis();
        let signers = Signers::new(&state);
        signers.approve(&key(1).address(), 0, with_fourth(), 10, 0, WINDOW).unwrap();
        signers.approve(&key(2).address(), 0, SignerSet::new(own([key(1), key(2)]), 2, 2), 10, 0, WINDOW).unwrap();

        signers.withdraw(&key(1).address(), 0, 1, WINDOW).unwrap();

        assert!(signers.ballot().unwrap().approvers(&with_fourth()).is_empty());
        assert_eq!(signers.ballot().unwrap().approvals.len(), 1, "another signer's approval stays");
        assert_eq!(signers.withdraw(&key(1).address(), 0, 1, WINDOW), Err(RegistryError::NoApproval));
        assert_eq!(signers.withdraw(&key(9).address(), 0, 1, WINDOW), Err(RegistryError::NotSigner));
        assert_eq!(signers.withdraw(&key(2).address(), 1, 1, WINDOW), Err(RegistryError::StaleSigners(0)));

        // An approval that expired has nothing left to withdraw.
        assert_eq!(signers.withdraw(&key(2).address(), 0, WINDOW, WINDOW), Err(RegistryError::NoApproval));

        signers.withdraw(&key(2).address(), 0, 1, WINDOW).unwrap();

        assert_eq!(signers.ballot().unwrap(), Ballot::default());
    }

    #[test]
    fn a_signer_with_an_account_of_its_own_speaks_through_it() {
        let state = State::new(Arc::new(MemoryDb::default()));
        let owner = key(7).address();
        let set = SignerSet::new(vec![Signer { key: key(1), account: owner }, Signer::own(key(2)), Signer::own(key(3))], 2, 2);
        Signers::new(&state).write_genesis(&set, 2).unwrap();
        let signers = Signers::new(&state);

        // Its key's address is nobody here; its account approves, withdraws and is paid.
        assert_eq!(set.signer_at(&owner), Some(key(1)));
        assert_eq!(set.signer_at(&key(1).address()), None);
        assert_eq!(set.validators().member(&key(1)).unwrap().account, owner);
        let (mut keys, mut expected) = (set.keys(), vec![key(1), key(2), key(3)]);
        keys.sort();
        expected.sort();
        assert_eq!(keys, expected);
        assert_eq!(signers.approve(&key(1).address(), 0, with_fourth(), 10, 0, WINDOW), Err(RegistryError::NotSigner));
        assert_eq!(signers.approve(&owner, 0, with_fourth(), 10, 0, WINDOW).unwrap(), Approved::Counted { approvals: 1, needed: 2 });
        assert_eq!(signers.ballot().unwrap().approvers(&with_fourth()), [key(1)]);
        signers.withdraw(&owner, 0, 0, WINDOW).unwrap();
        assert_eq!(signers.ballot().unwrap(), Ballot::default());
    }

    #[test]
    fn a_cheating_signer_is_ejected_unless_the_set_would_break() {
        let (state, set) = genesis();
        let signers = Signers::new(&state);
        signers.approve(&key(1).address(), 0, with_fourth(), 10, 0, WINDOW).unwrap();

        for epoch in 3..=4 {
            signers.fix_set(epoch).unwrap();
        }

        // Three signers, two justifying: without key 3, two of two still do.
        assert_eq!(signers.eject(&key(3), 2, 2, 10).unwrap(), 1);
        let decided = signers.decided().unwrap();

        assert_eq!(decided.version, 1);
        assert_eq!(decided.set, SignerSet::new(own([key(1), key(2)]), 2, 2));
        assert!(signers.ballot().unwrap().approvals.is_empty(), "a new version empties the ballot");
        assert_eq!(signers.set(2).unwrap(), Some(set.clone()), "this epoch keeps it");
        assert_eq!(signers.set(3).unwrap(), Some(set), "and so does the next");
        assert_eq!(signers.set(4).unwrap(), Some(decided.set), "from two epochs on it is gone");
        assert_eq!(signers.eject(&key(3), 2, 2, 10), Err(RegistryError::UnknownSigner));

        // Two of two: neither can go without leaving the threshold out of reach.
        assert!(matches!(signers.eject(&key(1), 2, 2, 10), Err(RegistryError::BadSigners(why)) if why.contains("threshold")));
        assert_eq!(signers.decided().unwrap().version, 1, "nothing changed");
    }
}
