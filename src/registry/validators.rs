use {
    super::{Accounts, RegistryError},
    crate::crypto::hashable::Hashable,
    crate::crypto::keypair::PublicKey,
    crate::db::{Column, DbError, State},
    crate::structs::address::Address,
    crate::structs::u256::{U256Le, u256},
    r#macro::document,
    wincode::{SchemaRead, SchemaWrite},
};

const VOTERS: &[u8] = b"voters";

#[document(
    summary = "The staking rules a chain runs under, from its FinalityConfig.",
    category = "Finality",
    see_also = "Validators, FinalityConfig",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Staking {
    #[document(summary = "The smallest bond.")]
    pub min_bond: u256,
    #[document(summary = "Epochs before a bond or unbond changes who votes.")]
    pub activation_delay: u64,
    #[document(summary = "Epochs a validator's bond stays locked, and slashable, after it stops voting.")]
    pub withdrawal_delay: u64,
    #[document(summary = "A reporter of cheating gets the bond divided by this; the rest is burned.")]
    pub reporter_divisor: u64,
    #[document(summary = "The most validators registered at once, active or leaving.")]
    pub max_validators: u32,
}

#[document(
    summary = "A finality validator as stored, by voting key.",
    category = "Finality",
    see_also = "Validators",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct ValidatorRecord {
    #[document(summary = "The account that bonded: the only one that may unbond and withdraw, and where vote rewards go.")]
    pub account: Address,
    #[document(summary = "What it has bonded, and so how much its votes weigh. Zero once slashed.")]
    #[wincode(with = "U256Le")]
    pub bond: u256,
    #[document(summary = "The first epoch it votes in.")]
    pub active_from: u64,
    #[document(summary = "The first epoch it no longer votes in, once it unbonds or is slashed.")]
    pub active_until: Option<u64>,
    #[document(summary = "The first epoch its bond may be withdrawn in.")]
    pub withdrawable_from: Option<u64>,
    #[document(summary = "Whether it was caught signing conflicting votes.")]
    pub slashed: bool,
}

impl Hashable for ValidatorRecord {}

impl ValidatorRecord {
    #[document(parent = "ValidatorRecord", summary = "Whether it votes in an epoch.")]
    pub fn active_in(&self, epoch: u64) -> bool {
        !self.slashed
            && self.bond > u256::ZERO
            && self.active_from <= epoch
            && self.active_until.is_none_or(|until| epoch < until)
    }
}

#[document(
    summary = "One validator in an epoch's set.",
    category = "Finality",
    see_also = "ValidatorSet",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct Member {
    #[document(summary = "Its voting key.")]
    pub voter: PublicKey,
    #[document(summary = "The account that bonded it.")]
    pub account: Address,
    #[document(summary = "How much its votes weigh: its bond when the set was fixed.")]
    #[wincode(with = "U256Le")]
    pub weight: u256,
}

#[document(
    summary = "The validators whose votes count for one epoch's checkpoint, with their weights. Fixed two epochs ahead, at a checkpoint, so every branch that split off after that checkpoint agrees on it.",
    category = "Finality",
    see_also = "Validators::set",
)]
#[derive(Clone, Debug, Default, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct ValidatorSet {
    #[document(summary = "The members, sorted by voting key.")]
    pub members: Vec<Member>,
}

impl Hashable for ValidatorSet {}

impl ValidatorSet {
    #[document(parent = "ValidatorSet", summary = "The weight of every member together.")]
    pub fn total(&self) -> u256 {
        self.members.iter().fold(u256::ZERO, |total, member| total + member.weight)
    }

    #[document(parent = "ValidatorSet", summary = "The member with a voting key, if it is one.")]
    pub fn member(&self, voter: &PublicKey) -> Option<&Member> {
        self.members
            .binary_search_by(|member| member.voter.cmp(voter))
            .ok()
            .map(|index| &self.members[index])
    }

    #[document(parent = "ValidatorSet", summary = "Whether a weight is at least two thirds of the set's.")]
    pub fn supermajority(&self, weight: u256) -> bool {
        let total = self.total();

        total > u256::ZERO && weight.saturating_mul(u256::from(3u64)) >= total.saturating_mul(u256::from(2u64))
    }
}

#[document(
    summary = "Finality validators: bonding, unbonding, withdrawing and slashing, and the validator set of each epoch. Like Accounts, it reads and writes whichever state layer it is given.",
    category = "Finality",
    see_also = "ValidatorRecord, ValidatorSet, Staking",
)]
pub struct Validators<'a> {
    state: &'a State,
}

impl<'a> Validators<'a> {
    #[document(parent = "Validators", summary = "Reads and writes validators in the given state layer.")]
    pub fn new(state: &'a State) -> Validators<'a> {
        Validators { state }
    }

    #[document(parent = "Validators", summary = "A validator by voting key.")]
    pub fn get(&self, voter: &PublicKey) -> Result<Option<ValidatorRecord>, DbError> {
        self.state.get_value(Column::Validators, &voter_key(voter))
    }

    #[document(parent = "Validators", summary = "Every registered voting key, sorted.")]
    pub fn voters(&self) -> Result<Vec<PublicKey>, DbError> {
        Ok(self.state.get_value(Column::Epochs, VOTERS)?.unwrap_or_default())
    }

    #[document(
        parent = "Validators",
        summary = "Locks funds from an account as a new validator's bond. It votes from epoch + activation_delay + 1.",
        panics = "BondTooSmall, AlreadyBonded when the voting key is taken, TooManyValidators, InsufficientBalance.",
    )]
    pub fn bond(&self, account: &Address, voter: &PublicKey, amount: u256, epoch: u64, staking: &Staking) -> Result<(), RegistryError> {
        if amount < staking.min_bond {
            return Err(RegistryError::BondTooSmall(staking.min_bond));
        }

        if self.get(voter)?.is_some() {
            return Err(RegistryError::AlreadyBonded);
        }

        let mut voters = self.voters()?;

        if voters.len() >= staking.max_validators as usize {
            return Err(RegistryError::TooManyValidators);
        }

        Accounts::new(self.state).debit(account, amount)?;
        self.register(voter, ValidatorRecord {
            account: *account,
            bond: amount,
            active_from: epoch + staking.activation_delay + 1,
            active_until: None,
            withdrawable_from: None,
            slashed: false,
        }, &mut voters);

        Ok(())
    }

    /// Stores a new validator and adds it to the list of voters. Genesis uses it directly.
    pub(crate) fn register(&self, voter: &PublicKey, record: ValidatorRecord, voters: &mut Vec<PublicKey>) {
        if let Err(at) = voters.binary_search(voter) {
            voters.insert(at, *voter);
        }

        self.put_voters(voters);
        self.state.put_value(Column::Validators, voter_key(voter), &record);
    }

    #[document(
        parent = "Validators",
        summary = "Stops a validator voting from epoch + activation_delay + 1. Its bond can be withdrawn withdrawal_delay epochs after that.",
        panics = "NotBonded, NotYours when account did not bond it, AlreadyUnbonding.",
    )]
    pub fn unbond(&self, account: &Address, voter: &PublicKey, epoch: u64, staking: &Staking) -> Result<(), RegistryError> {
        let mut record = self.owned(account, voter)?;

        if record.active_until.is_some() {
            return Err(RegistryError::AlreadyUnbonding);
        }

        let until = epoch + staking.activation_delay + 1;
        record.active_until = Some(until);
        record.withdrawable_from = Some(until + staking.withdrawal_delay);
        self.state.put_value(Column::Validators, voter_key(voter), &record);

        Ok(())
    }

    #[document(
        parent = "Validators",
        summary = "Pays an unbonded validator's bond back to its account and forgets it.",
        returns = "What was paid: the bond, or nothing for a slashed validator.",
        panics = "NotBonded, NotYours, NotWithdrawable before withdrawable_from.",
    )]
    pub fn withdraw(&self, account: &Address, voter: &PublicKey, epoch: u64) -> Result<u256, RegistryError> {
        let record = self.owned(account, voter)?;

        if record.withdrawable_from.is_none_or(|from| epoch < from) {
            return Err(RegistryError::NotWithdrawable(record.withdrawable_from));
        }

        if record.bond > u256::ZERO {
            Accounts::new(self.state).credit(account, record.bond)?;
        }

        let mut voters = self.voters()?;
        voters.retain(|known| known != voter);
        self.put_voters(&voters);
        self.state.delete(Column::Validators, voter_key(voter));

        Ok(record.bond)
    }

    #[document(
        parent = "Validators",
        summary = "Takes a cheating validator's whole bond: the reporter gets bond / reporter_divisor, and the rest is burned. The validator stops voting as an unbond would, and can be withdrawn, for nothing, later.",
        returns = "What the reporter got.",
        panics = "NotBonded, AlreadySlashed.",
    )]
    pub fn slash(&self, reporter: &Address, voter: &PublicKey, epoch: u64, staking: &Staking) -> Result<u256, RegistryError> {
        let mut record = self.get(voter)?.ok_or(RegistryError::NotBonded)?;

        if record.slashed {
            return Err(RegistryError::AlreadySlashed);
        }

        let reward = record.bond / u256::from(staking.reporter_divisor.max(1));
        let until = epoch + staking.activation_delay + 1;
        record.bond = u256::ZERO;
        record.slashed = true;
        record.active_until = Some(record.active_until.map_or(until, |current| current.min(until)));
        record.withdrawable_from = Some(record.active_until.expect("set above") + staking.withdrawal_delay);
        self.state.put_value(Column::Validators, voter_key(voter), &record);

        if reward > u256::ZERO {
            Accounts::new(self.state).credit(reporter, reward)?;
        }

        Ok(reward)
    }

    #[document(
        parent = "Validators",
        summary = "Fixes the validator set for an epoch from the validators registered now: every one active in that epoch, weighted by bond.",
    )]
    pub fn fix_set(&self, epoch: u64) -> Result<ValidatorSet, DbError> {
        let mut members = Vec::new();

        for voter in self.voters()? {
            let Some(record) = self.get(&voter)? else {
                continue;
            };

            if record.active_in(epoch) {
                members.push(Member {
                    voter,
                    account: record.account,
                    weight: record.bond,
                });
            }
        }

        let set = ValidatorSet { members };
        self.state.put_value(Column::Epochs, epoch_key(b"set:", epoch), &set);

        Ok(set)
    }

    #[document(
        parent = "Validators",
        summary = "The validator set of an epoch, once fixed.",
    )]
    pub fn set(&self, epoch: u64) -> Result<Option<ValidatorSet>, DbError> {
        self.state.get_value(Column::Epochs, &epoch_key(b"set:", epoch))
    }

    #[document(parent = "Validators", summary = "Whether the voters of an epoch's checkpoint have been paid.")]
    pub fn paid(&self, epoch: u64) -> Result<bool, DbError> {
        Ok(self.state.get(Column::Epochs, &epoch_key(b"paid:", epoch))?.is_some())
    }

    #[document(parent = "Validators", summary = "Records that the voters of an epoch's checkpoint have been paid.")]
    pub fn mark_paid(&self, epoch: u64) {
        self.state.put(Column::Epochs, epoch_key(b"paid:", epoch), vec![1]);
    }

    fn put_voters(&self, voters: &Vec<PublicKey>) {
        let bytes = wincode::serialize(voters).expect("keys always encode");
        self.state.put(Column::Epochs, VOTERS.to_vec(), bytes);
    }

    fn owned(&self, account: &Address, voter: &PublicKey) -> Result<ValidatorRecord, RegistryError> {
        let record = self.get(voter)?.ok_or(RegistryError::NotBonded)?;

        if record.account != *account {
            return Err(RegistryError::NotYours);
        }

        Ok(record)
    }
}

fn voter_key(voter: &PublicKey) -> Vec<u8> {
    voter.hashable_bytes_with_tag()
}

fn epoch_key(prefix: &[u8], epoch: u64) -> Vec<u8> {
    let mut key = prefix.to_vec();
    key.extend_from_slice(&epoch.to_be_bytes());

    key
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::MemoryDb;
    use std::sync::Arc;

    const STAKING: Staking = Staking {
        min_bond: u256::new(100),
        activation_delay: 2,
        withdrawal_delay: 4,
        reporter_divisor: 4,
        max_validators: 2,
    };
    const ALICE: Address = Address::Ed25519([1; 32]);
    const BOB: Address = Address::Ed25519([2; 32]);

    /// A real voting key from a fixed seed, in whichever scheme this build has.
    fn voter(seed: u8) -> PublicKey {
        #[cfg(feature = "ed25519")]
        let private = crate::crypto::keypair::PrivateKey::Ed25519([seed; 32]);
        #[cfg(not(feature = "ed25519"))]
        let private = crate::crypto::keypair::PrivateKey::EdBlake([seed; 32]);

        crate::crypto::keypair::Keypair::from_private(private).public()
    }

    fn state() -> State {
        let state = State::new(Arc::new(MemoryDb::default()));
        Accounts::new(&state).credit(&ALICE, u256::new(1_000)).unwrap();

        state
    }

    #[test]
    fn bonds_start_voting_after_the_delay_and_stop_after_unbonding() {
        let state = state();
        let validators = Validators::new(&state);
        validators.bond(&ALICE, &voter(7), u256::new(300), 10, &STAKING).unwrap();

        assert_eq!(Accounts::new(&state).get(&ALICE).unwrap().balance, u256::new(700));
        assert!(validators.fix_set(12).unwrap().members.is_empty());
        assert_eq!(validators.fix_set(13).unwrap().total(), u256::new(300));

        assert_eq!(validators.unbond(&BOB, &voter(7), 20, &STAKING), Err(RegistryError::NotYours));
        validators.unbond(&ALICE, &voter(7), 20, &STAKING).unwrap();

        assert_eq!(validators.fix_set(22).unwrap().total(), u256::new(300));
        assert!(validators.fix_set(23).unwrap().members.is_empty());
        assert_eq!(validators.withdraw(&ALICE, &voter(7), 26), Err(RegistryError::NotWithdrawable(Some(27))));
        assert_eq!(validators.withdraw(&ALICE, &voter(7), 27), Ok(u256::new(300)));
        assert_eq!(Accounts::new(&state).get(&ALICE).unwrap().balance, u256::new(1_000));
        assert_eq!(validators.get(&voter(7)).unwrap(), None);
        assert!(validators.voters().unwrap().is_empty());
    }

    #[test]
    fn bonds_are_checked() {
        let state = state();
        let validators = Validators::new(&state);

        assert_eq!(validators.bond(&ALICE, &voter(1), u256::new(99), 0, &STAKING), Err(RegistryError::BondTooSmall(u256::new(100))));
        validators.bond(&ALICE, &voter(1), u256::new(100), 0, &STAKING).unwrap();
        assert_eq!(validators.bond(&ALICE, &voter(1), u256::new(100), 0, &STAKING), Err(RegistryError::AlreadyBonded));
        validators.bond(&ALICE, &voter(2), u256::new(100), 0, &STAKING).unwrap();
        assert_eq!(validators.bond(&ALICE, &voter(3), u256::new(100), 0, &STAKING), Err(RegistryError::TooManyValidators));
        assert_eq!(validators.bond(&BOB, &voter(3), u256::new(100), 0, &STAKING).unwrap_err(), RegistryError::TooManyValidators);
    }

    #[test]
    fn slashing_pays_the_reporter_and_burns_the_rest() {
        let state = state();
        let validators = Validators::new(&state);
        validators.bond(&ALICE, &voter(7), u256::new(400), 0, &STAKING).unwrap();

        assert_eq!(validators.slash(&BOB, &voter(7), 5, &STAKING), Ok(u256::new(100)));
        assert_eq!(validators.slash(&BOB, &voter(7), 5, &STAKING), Err(RegistryError::AlreadySlashed));
        assert_eq!(Accounts::new(&state).get(&BOB).unwrap().balance, u256::new(100));
        assert!(validators.fix_set(6).unwrap().members.is_empty());
        assert_eq!(validators.withdraw(&ALICE, &voter(7), 12), Ok(u256::ZERO));
    }

    #[test]
    fn two_thirds_is_a_supermajority() {
        let set = ValidatorSet {
            members: vec![
                Member { voter: voter(1), account: ALICE, weight: u256::new(2) },
                Member { voter: voter(2), account: BOB, weight: u256::new(1) },
            ],
        };

        assert!(set.supermajority(u256::new(2)));
        assert!(!set.supermajority(u256::new(1)));
        assert!(!ValidatorSet::default().supermajority(u256::ZERO));
        assert_eq!(set.member(&voter(2)).unwrap().account, BOB);
    }
}
