//! Consensus, in two halves. The rules every node checks blocks against come from genesis, so all
//! nodes agree on them: who may produce, when, with what difficulty, and how much each block
//! weighs when choosing between forks. Producing is separate, and only producing nodes do it:
//! an authority signs at its slot, a miner searches for a nonce on other threads.

use {
    crate::crypto::hashable::Hashable,
    crate::crypto::keypair::PublicKey,
    crate::structs::address::Address,
    crate::structs::block::{BlockHeader, Seal},
    crate::structs::u256::u256,
    r#macro::document,
    std::fmt,
    wincode::{SchemaRead, SchemaWrite},
};

pub mod dev;
pub mod pow;

pub use dev::{AuthorityProducer, DevAuthority};
pub use pow::{Miner, PowConfig, ProofOfWork, meets, work_hash};

/// How far ahead of this node's clock a block's timestamp may be.
pub const MAX_DRIFT_MS: u64 = 15_000;

#[document(
    summary = "The rules blocks are checked against: who may produce, when, with what difficulty, and how much each block counts for when choosing between forks. Built from genesis by ConsensusConfig::rules.",
    category = "Consensus",
    see_also = "Producer, ConsensusConfig, Chain::import",
)]
pub trait Consensus: Send + Sync {
    #[document(
        parent = "Consensus",
        summary = "How much a block weighs. The main chain is the branch with the most total work.",
    )]
    fn work(&self, header: &BlockHeader) -> u256;

    #[document(
        parent = "Consensus",
        summary = "The difficulty the block after parent must declare, given its timestamp.",
    )]
    fn difficulty(&self, parent: &BlockHeader, timestamp: u64) -> u128;

    #[document(
        parent = "Consensus",
        summary = "Checks a block's producer, timing, difficulty and seal against its parent.",
        params = {
            header = "The block's header.",
            seal = "The block's seal.",
            parent = "The header it extends.",
            now = "This node's clock, in milliseconds since the Unix epoch.",
        },
    )]
    fn check(&self, header: &BlockHeader, seal: &Seal, parent: &BlockHeader, now: u64) -> Result<(), ConsensusError>;
}

#[document(
    summary = "What a producing node does: say where rewards go, say when to start the next block, and seal it. An authority seals at once; a miner hands the header to its threads and the seal arrives later.",
    category = "Consensus",
    see_also = "Consensus, AuthorityProducer, Miner",
)]
pub trait Producer: Send {
    #[document(parent = "Producer", summary = "Where this node's blocks, and their rewards, are credited.")]
    fn address(&self) -> Address;

    #[document(
        parent = "Producer",
        summary = "The earliest time to build the block after parent, in milliseconds since the Unix epoch.",
    )]
    fn start_at(&self, parent: &BlockHeader) -> u64;

    #[document(
        parent = "Producer",
        summary = "Starts sealing a header.",
        returns = "The seal when it is ready at once, or None when it will arrive later through the callback the producer was made with.",
    )]
    fn seal(&mut self, header: &BlockHeader) -> Option<Seal>;
}

#[document(
    summary = "Which consensus a chain runs, and its parameters. Part of genesis, so every node checks blocks the same way.",
    category = "Consensus",
    see_also = "Genesis, Consensus",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
#[wincode(tag_encoding = "u8")]
pub enum ConsensusConfig {
    #[document(
        summary = "One key produces every block, one per slot.",
        params = {
            key = "The only key that may produce.",
            slot_ms = "Milliseconds between blocks.",
        },
    )]
    #[wincode(tag = 0)]
    Authority { key: PublicKey, slot_ms: u64 },
    #[document(summary = "Anyone may produce a block by finding a nonce whose work hash meets the block's difficulty.")]
    #[wincode(tag = 1)]
    ProofOfWork(PowConfig),
}

impl Hashable for ConsensusConfig {
    const TAG_LEN: usize = 1;
}

impl ConsensusConfig {
    #[document(parent = "ConsensusConfig", summary = "The rules this configuration describes.")]
    pub fn rules(&self) -> Box<dyn Consensus> {
        match self {
            ConsensusConfig::Authority { key, slot_ms } => Box::new(DevAuthority::new(*key, *slot_ms)),
            ConsensusConfig::ProofOfWork(config) => Box::new(ProofOfWork::new(config.clone())),
        }
    }

    #[document(parent = "ConsensusConfig", summary = "The scheme's name: authority or pow.")]
    pub fn name(&self) -> &'static str {
        match self {
            ConsensusConfig::Authority { .. } => "authority",
            ConsensusConfig::ProofOfWork(_) => "pow",
        }
    }
}

#[document(
    summary = "Why consensus rejected a block.",
    category = "Consensus",
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConsensusError {
    #[document(summary = "The block was produced by an address that may not produce it.")]
    WrongProducer,
    #[document(summary = "The block came before its slot, or not after its parent.")]
    Early,
    #[document(summary = "The block's timestamp is further ahead of this node's clock than MAX_DRIFT_MS allows.")]
    FromTheFuture,
    #[document(summary = "The block declares a different difficulty than consensus requires.")]
    WrongDifficulty { expected: u128, got: u128 },
    #[document(summary = "The seal is missing, of the wrong kind, or does not match the header.")]
    BadSeal,
    #[document(summary = "The seal's work hash does not meet the block's difficulty.")]
    NotEnoughWork,
}

impl fmt::Display for ConsensusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConsensusError::WrongProducer => write!(f, "wrong producer"),
            ConsensusError::Early => write!(f, "produced too early"),
            ConsensusError::FromTheFuture => write!(f, "timestamp too far in the future"),
            ConsensusError::WrongDifficulty { expected, got } => {
                write!(f, "difficulty {got}, but consensus requires {expected}")
            }
            ConsensusError::BadSeal => write!(f, "bad seal"),
            ConsensusError::NotEnoughWork => write!(f, "not enough work"),
        }
    }
}

impl std::error::Error for ConsensusError {}
