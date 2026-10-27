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
pub mod changes;
pub mod collections;
pub mod names;
pub mod signers;
pub mod tokens;
pub mod validators;
#[cfg(feature = "vm")]
pub mod contracts;

pub use accounts::Accounts;
#[cfg(feature = "names")]
pub use names::Names;
#[cfg(feature = "vm")]
pub use contracts::Contracts;
pub use tokens::Tokens;
pub use collections::Collections;

#[document(
    summary = "What makes a contract a token or a collection on a chain: genesis's token functions, and the collection standards genesis names or governance decided last.",
    category = "Registries",
    see_also = "Tokens, Collections, Contracts::refresh",
)]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Standards {
    #[document(summary = "The functions that make a token; empty keeps no token registry.")]
    pub tokens: std::sync::Arc<[String]>,
    #[document(summary = "The functions that make a collection, or None for a chain without the collection registry.")]
    pub collections: Option<collections::CollectionFunctions>,
}

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
    #[document(summary = "A bond under the chain's minimum, which it carries.")]
    BondTooSmall(crate::structs::u256::u256),
    #[document(summary = "The voting key is already bonded.")]
    AlreadyBonded,
    #[document(summary = "No validator has this voting key.")]
    NotBonded,
    #[document(summary = "Only the account that bonded a validator may unbond or withdraw it.")]
    NotYours,
    #[document(summary = "The validator is already leaving.")]
    AlreadyUnbonding,
    #[document(summary = "The bond is still locked; it carries the epoch it unlocks in, if unbonding has started.")]
    NotWithdrawable(Option<u64>),
    #[document(summary = "The validator was slashed already.")]
    AlreadySlashed,
    #[document(summary = "The chain has as many validators as it allows.")]
    TooManyValidators,
    #[document(summary = "Only one of the decided signers may approve a change to them.")]
    NotSigner,
    #[document(summary = "The approval is for an older set of signers; it carries the decided version.")]
    StaleSigners(u64),
    #[document(summary = "The proposed signers could not finalize safely, and why.")]
    BadSigners(&'static str),
    #[document(summary = "The proposed signers are the decided ones already.")]
    SameSigners,
    #[document(summary = "The signer has no approval that still counts to withdraw.")]
    NoApproval,
    #[document(summary = "The voting key is not one of the decided signers.")]
    UnknownSigner,
    #[document(summary = "A change approved or withdrawn by an account governance does not let approve.")]
    NotGoverning,
    #[document(summary = "A change approved or withdrawn for an older version than the consensus decided last; the decided version.")]
    StaleChange(u64),
    #[document(summary = "A change that does not fit the chain, and why: a consensus that needs another finality, or settings no chain could run.")]
    BadChange(&'static str),
    #[document(summary = "A change to what was decided already: the same consensus, bootnodes, name prices or collection standards.")]
    SameChange,
    #[document(summary = "The contract exports what makes a token on this chain, but its name(), symbol() or decimals() did not answer as the registry needs, and why.")]
    BadToken(String),
    #[document(summary = "The contract exports what makes a collection on this chain, but its name() or symbol() did not answer as the registry needs, and why.")]
    BadCollection(String),
    #[document(summary = "The contract would not load, or this node lacks its engine.")]
    #[cfg(feature = "vm")]
    Vm(VmError),
    #[document(summary = "An upgrade's new code does not keep the old code's storage layout, and why.")]
    Layout(String),
    #[document(summary = "A name transaction that cannot go through, and why: a name taken, not the sender's, expired, or not one at all.")]
    Name(String),
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
            RegistryError::BondTooSmall(min) => write!(f, "a bond must be at least {min}"),
            RegistryError::AlreadyBonded => write!(f, "that voting key is already bonded"),
            RegistryError::NotBonded => write!(f, "no validator has that voting key"),
            RegistryError::NotYours => write!(f, "only the account that bonded it may do that"),
            RegistryError::AlreadyUnbonding => write!(f, "the validator is already leaving"),
            RegistryError::NotWithdrawable(Some(epoch)) => write!(f, "the bond unlocks in epoch {epoch}"),
            RegistryError::NotWithdrawable(None) => write!(f, "the validator has not unbonded"),
            RegistryError::AlreadySlashed => write!(f, "the validator was slashed already"),
            RegistryError::TooManyValidators => write!(f, "the chain has as many validators as it allows"),
            RegistryError::NotSigner => write!(f, "only a signer may approve or withdraw a change to the signers"),
            RegistryError::StaleSigners(version) => write!(f, "the signers changed since; the decided version is {version}"),
            RegistryError::BadSigners(why) => write!(f, "{why}"),
            RegistryError::SameSigners => write!(f, "those are the signers already"),
            RegistryError::NoApproval => write!(f, "no approval of this signer's counts to withdraw"),
            RegistryError::UnknownSigner => write!(f, "not one of the decided signers"),
            RegistryError::NotGoverning => write!(f, "only an account governance lets approve may approve or withdraw a change"),
            RegistryError::StaleChange(version) => write!(f, "the consensus changed since; the decided version is {version}"),
            RegistryError::BadChange(why) => write!(f, "{why}"),
            RegistryError::SameChange => write!(f, "that change was decided already"),
            RegistryError::BadToken(why) => write!(f, "it is a token, but {why}"),
            RegistryError::BadCollection(why) => write!(f, "it is a collection, but {why}"),
            #[cfg(feature = "vm")]
            RegistryError::Vm(err) => write!(f, "{err}"),
            RegistryError::Layout(why) => write!(f, "the new code does not keep the storage layout: {why}"),
            RegistryError::Name(why) => write!(f, "{why}"),
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
