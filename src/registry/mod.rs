//! The chain's records, kept in a State: accounts by address, and deployed contracts. Each
//! registry borrows the State it reads and writes, so whichever layer it is given, a
//! transaction's or a block's, is the one its changes land in.

use {
    crate::db::DbError,
    crate::structs::address::Address,
    r#macro::document,
    std::fmt,
};

#[cfg(feature = "vm")]
use crate::vm::VmError;

pub mod accounts;
#[cfg(feature = "vm")]
pub mod contracts;

pub use accounts::Accounts;
#[cfg(feature = "vm")]
pub use contracts::Contracts;

#[document(
    summary = "Why a registry operation failed.",
    category = "Registries",
    see_also = "Accounts, Contracts",
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegistryError {
    #[document(summary = "The database failed. The node's fault, not the transaction's.")]
    Db(DbError),
    #[document(summary = "A debit or transfer asked for more than the account holds.")]
    InsufficientBalance,
    #[document(summary = "A credit would push a balance past the largest u256.")]
    BalanceOverflow,
    #[document(summary = "The account has sent as many transactions as a u64 can count.")]
    NonceOverflow,
    #[document(summary = "A contract already lives at the address a deploy derived.")]
    AlreadyDeployed(Address),
    #[document(summary = "No contract lives at the address.")]
    NoSuchContract(Address),
    #[document(summary = "The contract does not let this address do that: it has no check_perms, or check_perms did not return true.")]
    NotPermitted,
    #[document(summary = "The contract would not load, or this node lacks its engine.")]
    #[cfg(feature = "vm")]
    Vm(VmError),
}

impl fmt::Display for RegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RegistryError::Db(err) => write!(f, "{err}"),
            RegistryError::InsufficientBalance => write!(f, "insufficient balance"),
            RegistryError::BalanceOverflow => write!(f, "balance overflow"),
            RegistryError::NonceOverflow => write!(f, "nonce overflow"),
            RegistryError::AlreadyDeployed(address) => {
                write!(f, "a contract already lives at {address}")
            }
            RegistryError::NoSuchContract(address) => write!(f, "no contract at {address}"),
            RegistryError::NotPermitted => write!(f, "the contract does not permit it"),
            #[cfg(feature = "vm")]
            RegistryError::Vm(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for RegistryError {}

impl From<DbError> for RegistryError {
    fn from(err: DbError) -> RegistryError {
        RegistryError::Db(err)
    }
}

#[cfg(feature = "vm")]
impl From<VmError> for RegistryError {
    fn from(err: VmError) -> RegistryError {
        RegistryError::Vm(err)
    }
}
