//! Validity proofs: a zero-knowledge proof that a run of blocks followed the chain's rules, so what
//! the validators sign is checked by what the blocks did, not taken on their word (docs/proofs.md).
//!
//! A proof covers a range, from a block whose state root is known, proven before or trusted, to a
//! later one: it runs every block of the range by the very code a node runs them by
//! (`Chain::run_block`), over a witness of what the range reads, and says what it ran to, the end's
//! block and state root, with a keccak256 root of what left through the bridge's gateway in the
//! range. Which block is final still comes from the validators' attestations; the proof makes
//! their state root a checked one.
//!
//! Nothing here is bound to one zkVM: a range's public values are laid out as Solidity's
//! `abi.encode` lays them out, the replay is plain Rust, and a proof system is a ProofKind and the
//! ProofSystem that checks its proofs: SP1's in Groth16, PLONK and as its STARK, each under a
//! feature of its own (sp1-groth16, sp1-plonk, sp1-stark), governance naming which the chain takes.
//! Another zkVM is another kind beside them.
//!
//! Beside ranges, two proofs of finality (proofs::finality): of one link, two thirds of an epoch's
//! voters signing it, for Ethereum; and of the header chain from genesis, each proof checking the
//! one before, for the apps. Each is its own program.

#[cfg(feature = "finality-proofs")]
mod finality;
#[cfg(feature = "secp256k1")]
mod links;
mod range;
#[cfg(all(feature = "chain", feature = "secp256k1"))]
mod replay;
#[cfg(feature = "sp1-prover")]
mod sp1;
mod state;
mod system;

pub use self::range::{GatewayMode, ProofSettings, ProvenRange, RangeEnd, TakenProof};
#[cfg(feature = "finality-proofs")]
pub use self::finality::{FinalityInput, LinkInput, check_link, prove_finality};
#[cfg(feature = "header-proofs")]
pub use self::finality::{HeadersInput, MAX_HEADER_LINKS, prove_headers};
#[cfg(feature = "secp256k1")]
pub use self::links::{ChainPoint, ProvenHeaders, ProvenLink, TakenHeaders};
#[cfg(feature = "secp256k1")]
pub use self::range::{verify_withdrawal, withdrawal_proof, withdrawals_root};
#[cfg(all(feature = "chain", feature = "secp256k1"))]
pub use self::replay::{RangeInput, Ran, ReplayError, Witness, WitnessEntry, WitnessNode, replay, run_range};
pub use self::state::{ProvenChain, RECORDED_BLOCKS};
pub use self::system::{ProofKind, ProofSystem, Prover, sp1_stark_key, verifier};
#[cfg(feature = "sp1-groth16")]
pub use self::system::Sp1Groth16;
#[cfg(feature = "sp1-plonk")]
pub use self::system::Sp1Plonk;
#[cfg(feature = "sp1-stark")]
pub use self::system::Sp1Stark;
#[cfg(feature = "sp1-prover")]
pub use self::sp1::Sp1Prover;
#[cfg(test)]
pub(crate) use self::system::TestProofs;

#[cfg(test)]
mod tests;
