//! Who may make blocks, and when. The node only talks to the Consensus trait, so another
//! scheme, proof of work or proof of stake, can replace DevAuthority without touching the loop.

use {
    crate::crypto::keypair::{PublicKey, Signature},
    crate::structs::block::BlockHeader,
    r#macro::document,
    std::fmt,
};

pub mod dev;

pub use dev::DevAuthority;

#[document(
    summary = "A consensus scheme: when this node may produce, how it seals what it produces, and whether someone else's block was produced by the right key at the right time.",
    category = "Consensus",
    see_also = "DevAuthority, Node",
)]
pub trait Consensus: Send {
    #[document(
        parent = "Consensus",
        summary = "When this node should produce the block after parent, in milliseconds since the Unix epoch.",
        returns = "The time, or None when this node does not produce.",
    )]
    fn next_slot(&self, parent: &BlockHeader) -> Option<u64>;

    #[document(
        parent = "Consensus",
        summary = "The key this node produces with, which goes in its headers.",
    )]
    fn producer(&self) -> Option<PublicKey>;

    #[document(
        parent = "Consensus",
        summary = "Seals a header this node produced.",
        returns = "The seal, or None when this node does not produce.",
    )]
    fn seal(&self, header: &BlockHeader) -> Option<Signature>;

    #[document(
        parent = "Consensus",
        summary = "Checks a block's producer, timing and seal against its parent.",
        params = {
            header = "The block's header.",
            seal = "The block's seal.",
            parent = "The header it extends.",
            now = "This node's clock, in milliseconds since the Unix epoch.",
        },
    )]
    fn check(
        &self,
        header: &BlockHeader,
        seal: Option<&Signature>,
        parent: &BlockHeader,
        now: u64,
    ) -> Result<(), ConsensusError>;
}

#[document(
    summary = "Why consensus rejected a block.",
    category = "Consensus",
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConsensusError {
    #[document(summary = "The block was produced by a key that may not produce it.")]
    WrongProducer,
    #[document(summary = "The block came before its slot.")]
    Early,
    #[document(summary = "The block's timestamp is further ahead of this node's clock than drift allows.")]
    FromTheFuture,
    #[document(summary = "The seal is missing or does not match the header.")]
    BadSeal,
}

impl fmt::Display for ConsensusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConsensusError::WrongProducer => write!(f, "wrong producer"),
            ConsensusError::Early => write!(f, "produced before its slot"),
            ConsensusError::FromTheFuture => write!(f, "timestamp too far in the future"),
            ConsensusError::BadSeal => write!(f, "bad seal"),
        }
    }
}

impl std::error::Error for ConsensusError {}
