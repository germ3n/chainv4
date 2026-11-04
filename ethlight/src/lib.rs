//! A light client of Ethereum's beacon chain, as the consensus specs write it, cut down to
//! finalized headers: what lets a chain that runs no Ethereum node prove that an Ethereum
//! transaction is in a block Ethereum finalized.
//!
//! - [`ssz`]: hash_tree_root, Merkle branches and generalized indices.
//! - [`types`]: the beacon block header, sync committees, the light client's headers, updates and
//!   bootstrap, decoded from and encoded to SSZ as beacon nodes serve them.
//! - [`forks`]: mainnet's, Sepolia's and Hoodi's fork schedules, genesis validators roots and
//!   signing domains, picked by name.
//! - [`client`]: `validate_light_client_update` and `apply_light_client_update`, for finalized
//!   updates: a [`Store`] started from a bootstrap at a trusted block root, moved on by updates
//!   that two thirds of the sync committee signed.
//! - [`tx`]: a transaction proven in a finalized block's execution payload, and its calldata read
//!   from its RLP.
//!
//! No hashing or signature checking is built in: a [`Crypto`] passes them in, so the eth_relay
//! contract (contracts/examples/eth_relay) runs SHA-256 and BLS as host calls, and the node runs
//! them natively, by the same rules from this one source.

#![no_std]

extern crate alloc;

pub mod client;
pub mod forks;
pub mod ssz;
pub mod tx;
pub mod types;

#[cfg(any(test, feature = "testing"))]
pub mod testing;

#[cfg(test)]
mod tests;

pub use {
    client::{Applied, Store, prove_ancestor},
    forks::{Fork, ForkAt, HOODI, MAINNET, NETWORKS, Network, SEPOLIA},
    types::{BeaconBlockHeader, ExecutionPayloadHeader, Finalized, LightClientBootstrap, LightClientHeader, LightClientUpdate, SyncAggregate, SyncCommittee},
};

/// A node of a Merkle tree, or any 32-byte root.
pub type Root = [u8; 32];

/// A BLS public key, compressed: a point on G1.
pub type PublicKey = [u8; 48];

/// A BLS signature, compressed: a point on G2.
pub type Signature = [u8; 96];

/// What the light client needs done that it does not do itself.
pub trait Crypto {
    /// SHA-256, what SSZ's hash_tree_root is made of.
    fn sha256(&self, data: &[u8]) -> [u8; 32];

    /// FastAggregateVerify, with the ciphersuite Ethereum signs with,
    /// BLS_SIG_BLS12381G2_XMD:SHA-256_SSWU_RO_POP_: whether the signature is the aggregate of
    /// every key's over the message. False for no keys, for a key or signature that is no point
    /// of its group or is the point at infinity.
    fn fast_aggregate_verify(&self, pubkeys: &[PublicKey], message: &[u8; 32], signature: &Signature) -> bool;
}

/// Why the light client refused something.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// Bytes that are not the SSZ they should be, naming what.
    Malformed(&'static str),
    /// A network that is none of mainnet, sepolia and hoodi.
    UnknownNetwork,
    /// A header from before Capella, which holds no execution payload header.
    BeforeCapella,
    /// A header whose execution payload header its body does not hold.
    BadExecutionBranch,
    /// A bootstrap whose header is not the trusted block root.
    NotTrusted,
    /// A bootstrap whose committee its header's state does not hold.
    BadCommitteeBranch,
    /// Fewer than two thirds of the sync committee signed.
    LowParticipation,
    /// Slots out of order: a signature at or before the attested header, or a finalized header
    /// after it.
    BadSlots,
    /// A signature slot after the current slot.
    FromTheFuture,
    /// A signature from a period whose committee the store does not know.
    UnknownPeriod,
    /// An update with neither a newer finalized header nor a next committee the store lacks:
    /// replayed, or older.
    Stale,
    /// An update without a finalized header, or one finalizing genesis.
    NotFinality,
    /// A finalized header the attested header's state does not hold.
    BadFinalityBranch,
    /// A next sync committee the attested header's state does not hold.
    BadNextCommitteeBranch,
    /// A next sync committee other than the one the store knows for that period.
    NextCommitteeMismatch,
    /// A sync aggregate whose signature does not hold for the committee, the fork and the header.
    BadSignature,
    /// An update finalizing a period the store cannot reach from its own.
    SkipsPeriod,
    /// An ancestor not within the last SLOTS_PER_HISTORICAL_ROOT slots before the header proving it.
    NotAncestor,
    /// An ancestor whose root the header's state does not hold.
    BadAncestorBranch,
    /// A transaction whose RLP does not read, naming why.
    BadTransaction(&'static str),
}

impl Error {
    /// What went wrong, in a sentence a contract can fail with.
    pub fn message(self) -> &'static str {
        match self {
            Error::Malformed(what) => what,
            Error::UnknownNetwork => "the network is mainnet, sepolia or hoodi",
            Error::BeforeCapella => "a header from before Capella holds no execution payload",
            Error::BadExecutionBranch => "a header's execution payload is not in its body",
            Error::NotTrusted => "the bootstrap's header is not the trusted block root",
            Error::BadCommitteeBranch => "the bootstrap's committee is not in its header's state",
            Error::LowParticipation => "fewer than two thirds of the sync committee signed",
            Error::BadSlots => "the update's slots are out of order",
            Error::FromTheFuture => "the update is signed after the current slot",
            Error::UnknownPeriod => "the update is signed in a period whose committee is not known",
            Error::Stale => "the update brings neither a newer finalized header nor the next committee",
            Error::NotFinality => "the update finalizes no header",
            Error::BadFinalityBranch => "the finalized header is not in the attested header's state",
            Error::BadNextCommitteeBranch => "the next sync committee is not in the attested header's state",
            Error::NextCommitteeMismatch => "the next sync committee is not the one known",
            Error::BadSignature => "the sync committee's signature does not hold",
            Error::SkipsPeriod => "the update skips a sync committee period",
            Error::NotAncestor => "the ancestor is not within 8192 slots before the header",
            Error::BadAncestorBranch => "the ancestor is not in the header's state",
            Error::BadTransaction(why) => why,
        }
    }
}

impl core::fmt::Display for Error {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(self.message())
    }
}
