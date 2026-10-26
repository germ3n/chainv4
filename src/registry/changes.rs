//! Governance's records, in chain state: the consensus decided last and the ballot for changing
//! it, in Column::Validators beside the signers' records, and the consensus fixed for each epoch,
//! in Column::Epochs beside each epoch's voters, which is what a light client asks a proof of.

use {
    super::RegistryError,
    crate::consensus::ConsensusConfig,
    crate::crypto::hashable::Hashable,
    crate::db::{Column, DbError, State},
    crate::governance::{Change, Governance},
    crate::structs::address::Address,
    crate::structs::u256::u256,
    r#macro::document,
    wincode::{SchemaRead, SchemaWrite},
};

const DECIDED: &[u8] = b"change:decided";
const BALLOT: &[u8] = b"change:ballot";

#[document(
    summary = "The consensus decided last, and how many changes came before it.",
    category = "Governance",
    see_also = "Changes",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct DecidedChange {
    #[document(summary = "How many changes have been decided since genesis. An approval names the version it changes, so approvals for an older decision never count.")]
    pub version: u64,
    #[document(summary = "The consensus itself, which the next checkpoints fix for the epochs ahead.")]
    pub consensus: ConsensusConfig,
}

impl Hashable for DecidedChange {}

#[document(
    summary = "One account's approval of a change.",
    category = "Governance",
    see_also = "ChangeBallot",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct ChangeApproval {
    #[document(summary = "The approving account.")]
    pub account: Address,
    #[document(summary = "The change it approves.")]
    pub change: Change,
    #[document(summary = "The epoch it was made in. It counts for approval_epochs epochs from there, and approving again starts it afresh.")]
    pub epoch: u64,
}

impl ChangeApproval {
    #[document(parent = "ChangeApproval", summary = "Whether it still counts in an epoch, when approvals count for approval_epochs epochs.")]
    pub fn counts(&self, epoch: u64, approval_epochs: u64) -> bool {
        epoch < self.epoch.saturating_add(approval_epochs)
    }
}

#[document(
    summary = "The approvals for changing what was decided last, at most one per account. An account that approves another change moves its approval there, and one that withdraws takes it back. An approval stops counting approval_epochs epochs after the one it was made in, and what it weighs is counted afresh whenever the ballot is: an account that lost its standing since weighs nothing.",
    category = "Governance",
    see_also = "Changes::approve",
)]
#[derive(Clone, Debug, Default, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct ChangeBallot {
    #[document(summary = "The version the approvals change.")]
    pub version: u64,
    #[document(summary = "The approvals, in the order they came.")]
    pub approvals: Vec<ChangeApproval>,
}

impl Hashable for ChangeBallot {}

impl ChangeBallot {
    #[document(
        parent = "ChangeBallot",
        summary = "The ballot as it stands in an epoch: only the approvals that still count.",
        params = {
            epoch = "The epoch to count in.",
            approval_epochs = "How many epochs an approval counts for.",
        },
    )]
    pub fn counting(mut self, epoch: u64, approval_epochs: u64) -> ChangeBallot {
        self.approvals.retain(|approval| approval.counts(epoch, approval_epochs));
        self
    }

    #[document(parent = "ChangeBallot", summary = "Who approved a change.")]
    pub fn approvers(&self, change: &Change) -> Vec<Address> {
        self.approvals.iter().filter(|approval| approval.change == *change).map(|approval| approval.account).collect()
    }

    #[document(parent = "ChangeBallot", summary = "The changes approved, each once, in the order their first approval came.")]
    pub fn changes(&self) -> Vec<Change> {
        let mut changes: Vec<Change> = Vec::new();

        for approval in &self.approvals {
            if !changes.contains(&approval.change) {
                changes.push(approval.change.clone());
            }
        }

        changes
    }
}

#[document(
    summary = "What an approval did.",
    category = "Governance",
    see_also = "Changes::approve",
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChangeApproved {
    #[document(summary = "It counted, and the change's approvals now weigh this much of what they need.")]
    Counted { weight: u256, needed: u256 },
    #[document(summary = "It was the last one needed: the change is decided, as this version.")]
    DecidedChange(u64),
}

#[document(
    summary = "Governance's records in chain state: the consensus fixed for each epoch, the one decided last, and the ballot for changing it. A decided change is fixed for epochs activation_delay on at the checkpoints that follow, like finality's voters, so every branch that split off after the checkpoint that fixed an epoch's consensus checks that epoch's blocks alike.",
    category = "Governance",
    see_also = "Governance, Change, Tx::ApproveChange",
)]
pub struct Changes<'a> {
    state: &'a State,
}

impl<'a> Changes<'a> {
    #[document(parent = "Changes", summary = "The records in a state layer.")]
    pub fn new(state: &'a State) -> Changes<'a> {
        Changes { state }
    }

    #[document(
        parent = "Changes",
        summary = "Writes genesis's consensus as the one decided, and fixes it for the epochs genesis already decides.",
    )]
    pub fn write_genesis(&self, consensus: &ConsensusConfig, activation_delay: u64) -> Result<(), DbError> {
        self.state.put_value(Column::Validators, DECIDED.to_vec(), &DecidedChange {
            version: 0,
            consensus: consensus.clone(),
        });

        for epoch in 0..=activation_delay {
            self.fix(epoch)?;
        }

        Ok(())
    }

    #[document(parent = "Changes", summary = "The consensus fixed for an epoch, if any is yet.")]
    pub fn consensus_for(&self, epoch: u64) -> Result<Option<ConsensusConfig>, DbError> {
        self.state.get_value(Column::Epochs, &Changes::consensus_key(epoch))
    }

    #[document(parent = "Changes", summary = "Where an epoch's consensus is kept, in Column::Epochs: what a light client asks a proof of.")]
    pub fn consensus_key(epoch: u64) -> Vec<u8> {
        let mut key = b"consensus:".to_vec();
        key.extend_from_slice(&epoch.to_be_bytes());

        key
    }

    #[document(parent = "Changes", summary = "Fixes the decided consensus for an epoch. Each checkpoint fixes the epoch activation_delay ahead.")]
    pub fn fix(&self, epoch: u64) -> Result<(), DbError> {
        let decided = self.decided()?;
        self.state.put_value(Column::Epochs, Changes::consensus_key(epoch), &decided.consensus);

        Ok(())
    }

    #[document(parent = "Changes", summary = "The consensus decided last, and its version.")]
    pub fn decided(&self) -> Result<DecidedChange, DbError> {
        self.state
            .get_value(Column::Validators, DECIDED)?
            .ok_or_else(|| DbError::Corrupt("no decided consensus".to_string()))
    }

    #[document(
        parent = "Changes",
        summary = "The approvals so far for changing what was decided last, expired ones included: ChangeBallot::counting leaves only those that count.",
    )]
    pub fn ballot(&self) -> Result<ChangeBallot, DbError> {
        let decided = self.decided()?;
        let ballot: ChangeBallot = self.state.get_value(Column::Validators, BALLOT)?.unwrap_or_default();

        // Approvals for an older version never count.
        Ok(match ballot.version == decided.version {
            true => ballot,
            false => ChangeBallot {
                version: decided.version,
                approvals: Vec::new(),
            },
        })
    }

    #[document(
        parent = "Changes",
        summary = "What a change's approvals weigh now: each approver's weight as governance counts it in this state, which an approver that lost its standing since no longer adds to.",
    )]
    pub fn weight_of(&self, ballot: &ChangeBallot, change: &Change, epoch: u64, governance: &dyn Governance) -> Result<u256, DbError> {
        let mut weight = u256::ZERO;

        for account in ballot.approvers(change) {
            weight = weight.saturating_add(governance.weight(self.state, epoch, &account)?.unwrap_or(u256::ZERO));
        }

        Ok(weight)
    }

    #[document(
        parent = "Changes",
        summary = "Counts an account's approval of a change. Once the approvals that still count for the same change weigh what governance needs, it is decided, and the checkpoints that follow fix it for later epochs.",
        params = {
            sender = "Who sent the approval: an account governance lets approve.",
            version = "The version it changes, which must be the decided one's.",
            change = "The change it approves, checked to fit the chain already.",
            epoch = "The epoch the approval is made in.",
            approval_epochs = "How many epochs an approval counts for.",
            governance = "Who may approve, and what it takes.",
        },
        returns = "Whether that decided it. NotGoverning for a sender that may not approve, StaleChange for an older version, SameChange for the consensus already decided.",
    )]
    pub fn approve(&self, sender: &Address, version: u64, change: Change, epoch: u64, approval_epochs: u64, governance: &dyn Governance) -> Result<ChangeApproved, RegistryError> {
        let decided = self.standing(sender, version, epoch, governance)?;
        let Change::Consensus(consensus) = &change;

        if *consensus == decided.consensus {
            return Err(RegistryError::SameChange);
        }

        let mut ballot = self.ballot()?.counting(epoch, approval_epochs);
        ballot.approvals.retain(|approval| approval.account != *sender);
        ballot.approvals.push(ChangeApproval {
            account: *sender,
            change: change.clone(),
            epoch,
        });
        let weight = self.weight_of(&ballot, &change, epoch, governance)?;
        let needed = governance.needed(self.state, epoch)?;

        if weight < needed {
            self.state.put_value(Column::Validators, BALLOT.to_vec(), &ballot);

            return Ok(ChangeApproved::Counted { weight, needed });
        }

        let version = decided.version + 1;
        let Change::Consensus(consensus) = change;
        self.state.put_value(Column::Validators, DECIDED.to_vec(), &DecidedChange { version, consensus });
        self.state.delete(Column::Validators, BALLOT.to_vec());

        Ok(ChangeApproved::DecidedChange(version))
    }

    #[document(
        parent = "Changes",
        summary = "Takes back an account's approval, while it still counts, so it backs no change.",
        params = {
            sender = "The account.",
            version = "The version its approval was for, which must be the decided one's.",
            epoch = "The epoch it is withdrawn in.",
            approval_epochs = "How many epochs an approval counts for.",
            governance = "Who may approve.",
        },
        returns = "NotGoverning for a sender that may not approve, StaleChange for an older version, NoApproval when it has no approval that still counts.",
    )]
    pub fn withdraw(&self, sender: &Address, version: u64, epoch: u64, approval_epochs: u64, governance: &dyn Governance) -> Result<(), RegistryError> {
        self.standing(sender, version, epoch, governance)?;
        let mut ballot = self.ballot()?.counting(epoch, approval_epochs);
        let before = ballot.approvals.len();
        ballot.approvals.retain(|approval| approval.account != *sender);

        if ballot.approvals.len() == before {
            return Err(RegistryError::NoApproval);
        }

        match ballot.approvals.is_empty() {
            true => self.state.delete(Column::Validators, BALLOT.to_vec()),
            false => self.state.put_value(Column::Validators, BALLOT.to_vec(), &ballot),
        }

        Ok(())
    }

    /// The decision a sender with standing changes, at a version that is the decided one's.
    fn standing(&self, sender: &Address, version: u64, epoch: u64, governance: &dyn Governance) -> Result<DecidedChange, RegistryError> {
        if governance.weight(self.state, epoch, sender)?.is_none() {
            return Err(RegistryError::NotGoverning);
        }

        let decided = self.decided()?;

        if version != decided.version {
            return Err(RegistryError::StaleChange(decided.version));
        }

        Ok(decided)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::MemoryDb;
    use crate::structs::address::Address;
    use std::sync::Arc;

    /// Three accounts, the first weighing two, needing three.
    struct Weighted;

    fn member(seed: u8) -> Address {
        Address::Ed25519([seed; 32])
    }

    impl Governance for Weighted {
        fn name(&self) -> &'static str {
            "weighted"
        }

        fn weight(&self, _state: &State, _epoch: u64, account: &Address) -> Result<Option<u256>, DbError> {
            Ok(match account {
                account if *account == member(1) => Some(u256::from(2u64)),
                account if *account == member(2) || *account == member(3) => Some(u256::ONE),
                _ => None,
            })
        }

        fn needed(&self, _state: &State, _epoch: u64) -> Result<u256, DbError> {
            Ok(u256::from(3u64))
        }
    }

    fn pow() -> ConsensusConfig {
        ConsensusConfig::ProofOfWork(crate::consensus::PowConfig {
            hash: crate::structs::hash::HashType::default_chain(),
            target_block_ms: 1_000,
            initial_difficulty: 1,
            min_difficulty: 1,
        })
    }

    fn staked() -> Change {
        Change::Consensus(ConsensusConfig::Staked {
            slot_ms: 2_000,
            schedule: crate::consensus::Schedule::Draw,
        })
    }

    #[test]
    fn approvals_weigh_up_to_a_decision_and_the_checkpoints_fix_it() {
        let state = State::new(Arc::new(MemoryDb::default()));
        let changes = Changes::new(&state);
        changes.write_genesis(&pow(), 2).unwrap();

        assert_eq!(changes.decided().unwrap(), DecidedChange { version: 0, consensus: pow() });
        assert_eq!(changes.consensus_for(2).unwrap(), Some(pow()));
        assert_eq!(changes.consensus_for(3).unwrap(), None);

        // A stranger may not; the same consensus is no change; a stale version is refused.
        assert_eq!(changes.approve(&member(9), 0, staked(), 1, 4, &Weighted), Err(RegistryError::NotGoverning));
        assert_eq!(changes.approve(&member(2), 0, Change::Consensus(pow()), 1, 4, &Weighted), Err(RegistryError::SameChange));
        assert_eq!(changes.approve(&member(2), 1, staked(), 1, 4, &Weighted), Err(RegistryError::StaleChange(0)));

        // One of weight one, then the one of weight two decide it: three of three.
        assert_eq!(
            changes.approve(&member(2), 0, staked(), 1, 4, &Weighted),
            Ok(ChangeApproved::Counted { weight: u256::ONE, needed: u256::from(3u64) })
        );
        assert_eq!(changes.ballot().unwrap().approvers(&staked()), [member(2)]);
        assert_eq!(changes.ballot().unwrap().changes(), [staked()]);
        assert_eq!(changes.approve(&member(1), 0, staked(), 2, 4, &Weighted), Ok(ChangeApproved::DecidedChange(1)));

        let Change::Consensus(consensus) = staked();

        assert_eq!(changes.decided().unwrap(), DecidedChange { version: 1, consensus: consensus.clone() });
        assert!(changes.ballot().unwrap().approvals.is_empty());

        // The next checkpoints fix it ahead; the epochs fixed before keep the old one.
        changes.fix(5).unwrap();

        assert_eq!(changes.consensus_for(5).unwrap(), Some(consensus));
        assert_eq!(changes.consensus_for(2).unwrap(), Some(pow()));
    }

    #[test]
    fn approvals_expire_move_and_withdraw() {
        let state = State::new(Arc::new(MemoryDb::default()));
        let changes = Changes::new(&state);
        changes.write_genesis(&pow(), 2).unwrap();
        let other = Change::Consensus(ConsensusConfig::Rotating { slot_ms: 1_000 });

        changes.approve(&member(2), 0, staked(), 1, 2, &Weighted).unwrap();
        changes.approve(&member(3), 0, staked(), 1, 2, &Weighted).unwrap();

        // Moving an approval takes it from the other change.
        changes.approve(&member(3), 0, other.clone(), 1, 2, &Weighted).unwrap();

        assert_eq!(changes.ballot().unwrap().approvers(&staked()), [member(2)]);
        assert_eq!(changes.ballot().unwrap().approvers(&other), [member(3)]);

        // Withdrawing needs an approval that still counts.
        assert_eq!(changes.withdraw(&member(3), 0, 1, 2, &Weighted), Ok(()));
        assert_eq!(changes.withdraw(&member(3), 0, 1, 2, &Weighted), Err(RegistryError::NoApproval));
        assert_eq!(changes.withdraw(&member(9), 0, 1, 2, &Weighted), Err(RegistryError::NotGoverning));

        // An approval from epoch 1 counts through epoch 2 and not in 3: the weight-two account
        // alone does not decide then.
        assert_eq!(changes.ballot().unwrap().counting(2, 2).approvers(&staked()), [member(2)]);
        assert!(changes.ballot().unwrap().counting(3, 2).approvers(&staked()).is_empty());
        assert_eq!(
            changes.approve(&member(1), 0, staked(), 3, 2, &Weighted),
            Ok(ChangeApproved::Counted { weight: u256::from(2u64), needed: u256::from(3u64) })
        );
    }
}
