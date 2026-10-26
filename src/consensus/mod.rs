//! Consensus, in two halves. The rules every node checks blocks against come from genesis, so all
//! nodes agree on them: who may produce, when, with what difficulty, and how much each block
//! weighs when choosing between forks. Producing is separate, and only producing nodes do it:
//! an authority signs at its slot, a miner searches for a nonce on other threads.

use {
    crate::crypto::hashable::Hashable,
    crate::crypto::keypair::PublicKey,
    crate::registry::signers::Signer,
    crate::structs::address::Address,
    crate::structs::block::{BlockHeader, Seal},
    crate::structs::u256::u256,
    r#macro::document,
    std::fmt,
    wincode::{SchemaRead, SchemaWrite},
};

pub mod dev;
#[cfg(feature = "gpu")]
pub mod gpu;
pub mod pow;
pub mod rotating;

pub use dev::{AuthorityProducer, DevAuthority};
pub use pow::{Miner, PowConfig, ProofOfWork, meets, work_hash};
pub use rotating::{Rotating, RotatingProducer};
#[cfg(feature = "gpu")]
pub use gpu::GpuMiner;

/// Separates block seals from anything else a key might sign.
const SEAL_DOMAIN: &[u8] = b"chainv4 block";

/// What a producer signs: the seal domain and the header's id.
pub(crate) fn seal_bytes(header: &BlockHeader) -> Vec<u8> {
    let mut bytes = SEAL_DOMAIN.to_vec();
    bytes.extend_from_slice(&header.id().hashable_bytes_with_tag());

    bytes
}

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
        summary = "How many of a block's ancestors difficulty and check look at: 1 for rules that look at the parent alone, the window for proof of work that retargets over one. Whoever checks a block hands them that many, newest first, or fewer near genesis.",
    )]
    fn lookback(&self) -> usize {
        1
    }

    #[document(
        parent = "Consensus",
        summary = "Whether check needs the block's epoch's producers, the signers authority finality fixed for it: true for the rotating authority alone. Whoever checks a block hands them over: a node from its state, a light client from the voters it proved.",
    )]
    fn needs_producers(&self) -> bool {
        false
    }

    #[document(
        parent = "Consensus",
        summary = "The difficulty the block after the parent must declare, given its timestamp.",
        params = {
            ancestors = "The parent first, then its parent, and so on: lookback of them, or as many as the chain has above genesis, genesis itself included only when it is the parent.",
            timestamp = "The new block's timestamp.",
        },
    )]
    fn difficulty(&self, ancestors: &[BlockHeader], timestamp: u64) -> u128;

    #[document(
        parent = "Consensus",
        summary = "Checks a block's producer, timing, difficulty and seal against its ancestors.",
        params = {
            header = "The block's header.",
            seal = "The block's seal.",
            ancestors = "The parent first, then its parent, and so on, as difficulty takes them.",
            producers = "The signers who may produce in the block's epoch, when needs_producers says so; empty otherwise.",
            now = "This node's clock, in milliseconds since the Unix epoch.",
        },
    )]
    fn check(&self, header: &BlockHeader, seal: &Seal, ancestors: &[BlockHeader], producers: &[Signer], now: u64) -> Result<(), ConsensusError>;
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
        summary = "The earliest time to build the block after parent, in milliseconds since the Unix epoch; u64::MAX for never, as when this node is not among the epoch's producers.",
        params = {
            parent = "The block to build on.",
            producers = "The signers who may produce in the next block's epoch, under a consensus that needs them; empty otherwise.",
        },
    )]
    fn start_at(&self, parent: &BlockHeader, producers: &[Signer]) -> u64;

    #[document(
        parent = "Producer",
        summary = "The difficulty the block this producer builds declares, when the producer decides it rather than consensus: a rotating authority's offset in the schedule. None to declare what Consensus::difficulty says.",
    )]
    fn difficulty(&self, _parent: &BlockHeader, _producers: &[Signer]) -> Option<u128> {
        None
    }

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
    #[document(
        summary = "The signers of authority finality produce in turn, one per slot, the next in line stepping in when one misses. Needs finality of type authority, whose signers, as fixed for each epoch, are the producers.",
        params = { slot_ms = "Milliseconds between blocks, and from a missed slot to the next in line's." },
    )]
    #[wincode(tag = 2)]
    Rotating { slot_ms: u64 },
}

impl Hashable for ConsensusConfig {
    const TAG_LEN: usize = 1;
}

impl ConsensusConfig {
    #[document(
        parent = "ConsensusConfig",
        summary = "The rules this configuration describes.",
        params = { pow_window = "Under proof of work, the window difficulty retargets over and timestamps are bounded by, as genesis sets it beside the consensus (Genesis::pow_window); None for the parent-only rule. Ignored by the authority." },
    )]
    pub fn rules(&self, pow_window: Option<u64>) -> Box<dyn Consensus> {
        match self {
            ConsensusConfig::Authority { key, slot_ms } => Box::new(DevAuthority::new(*key, *slot_ms)),
            ConsensusConfig::ProofOfWork(config) => Box::new(ProofOfWork::new(config.clone(), pow_window)),
            ConsensusConfig::Rotating { slot_ms } => Box::new(Rotating::new(*slot_ms)),
        }
    }

    #[document(parent = "ConsensusConfig", summary = "The scheme's name: authority, pow or rotating.")]
    pub fn name(&self) -> &'static str {
        match self {
            ConsensusConfig::Authority { .. } => "authority",
            ConsensusConfig::ProofOfWork(_) => "pow",
            ConsensusConfig::Rotating { .. } => "rotating",
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
    #[document(summary = "The block came before its slot, not after its parent, or, under a proof-of-work window, not after the median time of the window.")]
    Early,
    #[document(summary = "The block's timestamp is further ahead of this node's clock than MAX_DRIFT_MS allows.")]
    FromTheFuture,
    #[document(summary = "The block declares a different difficulty than consensus requires.")]
    WrongDifficulty { expected: u128, got: u128 },
    #[document(summary = "The seal is missing, of the wrong kind, or does not match the header.")]
    BadSeal,
    #[document(summary = "The seal's work hash does not meet the block's difficulty.")]
    NotEnoughWork,
    #[document(summary = "The block was checked without its parent: the checker's fault, never the block's.")]
    NoAncestors,
    #[document(summary = "The block's epoch has no producers known: no signers were fixed for it, or the checker does not know them yet.")]
    NoProducers,
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
            ConsensusError::NoAncestors => write!(f, "checked without its parent"),
            ConsensusError::NoProducers => write!(f, "no producers known for its epoch"),
        }
    }
}

impl std::error::Error for ConsensusError {}
