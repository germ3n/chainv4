//! Governance by a council: accounts genesis names, each weighing one, a threshold of them
//! deciding. For a chain whose finality electorate should not govern it, such as one where a
//! foundation decides what the validators run.

use {
    super::Governance,
    crate::db::{DbError, State},
    crate::structs::address::Address,
    crate::structs::u256::u256,
};

/// The council kind: a fixed set of accounts.
pub(super) struct Council {
    members: Vec<Address>,
    threshold: u32,
}

impl Council {
    pub(super) fn new(members: Vec<Address>, threshold: u32) -> Council {
        Council { members, threshold }
    }
}

impl Governance for Council {
    fn name(&self) -> &'static str {
        "council"
    }

    fn weight(&self, _state: &State, _epoch: u64, account: &Address) -> Result<Option<u256>, DbError> {
        Ok(self.members.contains(account).then_some(u256::ONE))
    }

    fn needed(&self, _state: &State, _epoch: u64) -> Result<u256, DbError> {
        Ok(u256::from(self.threshold))
    }

    fn total(&self, _state: &State, _epoch: u64) -> Result<u256, DbError> {
        Ok(u256::from(self.members.len() as u64))
    }

    fn voters(&self, _state: &State, _epoch: u64) -> Result<Vec<(Address, u256)>, DbError> {
        Ok(self.members.iter().map(|member| (*member, u256::ONE)).collect())
    }

    fn members(&self) -> Option<&[Address]> {
        Some(&self.members)
    }
}
