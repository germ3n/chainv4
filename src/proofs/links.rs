//! What finality and header-chain proofs prove, as their public values lay it out, and a header-chain
//! proof as the chain keeps it: what the apps and contracts read, without the chain itself. How
//! they are proven is proofs::finality.

use {
    super::ProofKind,
    crate::structs::attestation::word,
    crate::structs::hash::{Hash, HashType},
    crate::structs::u256::u256,
    crate::structs::vote::Checkpoint,
    r#macro::document,
    wincode::{SchemaRead, SchemaWrite},
};

fn number(value: u64) -> [u8; 32] {
    u256::from(value).to_be_bytes()
}

/// The words of public values, or None for bytes of another length.
fn words(bytes: &[u8], count: usize) -> Option<Vec<[u8; 32]>> {
    (bytes.len() == 32 * count).then(|| bytes.as_chunks::<32>().0.to_vec())
}

fn small(word: &[u8; 32]) -> Option<u64> {
    u64::try_from(u256::from_be_bytes(*word)).ok()
}

#[document(
    summary = "What a finality proof proves: that two thirds of the target epoch's voters, by weight, signed the link from source to target on the chain of this genesis, the voters as the source's state fixed them; with the source's state root, which a contract checks against the one it recorded, the target's height and state root, and the hash of the next epoch's attesting set. Its public values are eleven words, as abi.encode lays them out.",
    category = "Proofs",
    see_also = "prove_finality",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct ProvenLink {
    #[document(summary = "The chain's EVM chain id.")]
    pub chain_id: u64,
    #[document(summary = "The genesis id.")]
    pub genesis: Hash,
    #[document(summary = "The link's source.")]
    pub source: Checkpoint,
    #[document(summary = "The source's state root, which fixed the target epoch's voters.")]
    pub source_root: Hash,
    #[document(summary = "Its target.")]
    pub target: Checkpoint,
    #[document(summary = "The target's height.")]
    pub height: u64,
    #[document(summary = "The target's state root.")]
    pub state_root: Hash,
    #[document(summary = "The hash of the next epoch's attesting set, so the attesters' path goes on from the target too.")]
    pub next_attesting_set: [u8; 32],
    #[document(summary = "The finality program governance names at the target, zeros for none: the one the contract takes the next proof by.")]
    pub finality_program: [u8; 32],
}

impl ProvenLink {
    #[document(parent = "ProvenLink", summary = "How many 32-byte words the public values are.")]
    pub const WORDS: usize = 11;

    #[document(
        parent = "ProvenLink",
        summary = "The public values: abi.encode(chainId, genesis, sourceEpoch, sourceBlock, sourceRoot, targetEpoch, targetBlock, height, stateRoot, nextAttestingSet, finalityProgram).",
    )]
    pub fn public_values(&self) -> Vec<u8> {
        [
            number(self.chain_id),
            word(&self.genesis),
            number(self.source.epoch),
            word(&self.source.block),
            word(&self.source_root),
            number(self.target.epoch),
            word(&self.target.block),
            number(self.height),
            word(&self.state_root),
            self.next_attesting_set,
            self.finality_program,
        ]
        .concat()
    }

    #[document(parent = "ProvenLink", summary = "A link from its public values, its hashes of the chain's kind; None for any that do not read.")]
    pub fn from_public_values(hash: HashType, bytes: &[u8]) -> Option<ProvenLink> {
        let words = words(bytes, ProvenLink::WORDS)?;
        let hash_at = |at: usize| Hash::from_bytes(hash, &words[at]);

        Some(ProvenLink {
            chain_id: small(&words[0])?,
            genesis: hash_at(1)?,
            source: Checkpoint { epoch: small(&words[2])?, block: hash_at(3)? },
            source_root: hash_at(4)?,
            target: Checkpoint { epoch: small(&words[5])?, block: hash_at(6)? },
            height: small(&words[7])?,
            state_root: hash_at(8)?,
            next_attesting_set: words[9],
            finality_program: words[10],
        })
    }
}

#[document(
    summary = "A checkpoint a header-chain proof names: its epoch and block, and the block's height and state root.",
    category = "Proofs",
    see_also = "ProvenHeaders",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct ChainPoint {
    #[document(summary = "The checkpoint.")]
    pub checkpoint: Checkpoint,
    #[document(summary = "Its block's height.")]
    pub height: u64,
    #[document(summary = "Its block's state root.")]
    pub state_root: Hash,
}

#[document(
    summary = "What a header-chain proof proves: that the chain of this genesis finalized the checkpoint named, every link from genesis to it signed by two thirds of its epoch's voters as the checkpoint before fixed them, and that its newest justified checkpoint is this one. Its public values are eleven words.",
    category = "Proofs",
    see_also = "prove_headers, TakenHeaders",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct ProvenHeaders {
    #[document(summary = "The chain's EVM chain id.")]
    pub chain_id: u64,
    #[document(summary = "The genesis id.")]
    pub genesis: Hash,
    #[document(summary = "The program that proved it, which each step's previous proof must be by too.")]
    pub program: [u8; 32],
    #[document(summary = "The newest finalized checkpoint.")]
    pub finalized: ChainPoint,
    #[document(summary = "The newest justified checkpoint, where the next proof's first link starts, its state fixing the voters who sign it.")]
    pub justified: ChainPoint,
}

impl ProvenHeaders {
    #[document(parent = "ProvenHeaders", summary = "How many 32-byte words the public values are.")]
    pub const WORDS: usize = 11;

    #[document(
        parent = "ProvenHeaders",
        summary = "The public values: abi.encode(chainId, genesis, program, finalizedEpoch, finalizedBlock, finalizedHeight, finalizedRoot, justifiedEpoch, justifiedBlock, justifiedHeight, justifiedRoot).",
    )]
    pub fn public_values(&self) -> Vec<u8> {
        let point = |point: &ChainPoint| [number(point.checkpoint.epoch), word(&point.checkpoint.block), number(point.height), word(&point.state_root)];

        [[number(self.chain_id), word(&self.genesis), self.program].as_slice(), &point(&self.finalized), &point(&self.justified)].concat().concat()
    }

    #[document(parent = "ProvenHeaders", summary = "A header chain from its public values, its hashes of the chain's kind; None for any that do not read.")]
    pub fn from_public_values(hash: HashType, bytes: &[u8]) -> Option<ProvenHeaders> {
        let words = words(bytes, ProvenHeaders::WORDS)?;
        let hash_at = |at: usize| Hash::from_bytes(hash, &words[at]);
        let point = |at: usize| -> Option<ChainPoint> { Some(ChainPoint { checkpoint: Checkpoint { epoch: small(&words[at])?, block: hash_at(at + 1)? }, height: small(&words[at + 2])?, state_root: hash_at(at + 3)? }) };

        Some(ProvenHeaders { chain_id: small(&words[0])?, genesis: hash_at(1)?, program: words[2], finalized: point(3)?, justified: point(7)? })
    }
}

#[document(
    summary = "A header-chain proof as the chain took it (Tx::ProveHeaders) and keeps the latest: its kind, its public values and the proof. What the next proof checks first, and what an app checks for itself.",
    category = "Proofs",
    see_also = "ProvenHeaders, Tx::ProveHeaders",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct TakenHeaders {
    #[document(summary = "The proof system it was made with.")]
    pub kind: ProofKind,
    #[document(summary = "Its public values, a ProvenHeaders's.")]
    pub values: Vec<u8>,
    #[document(summary = "The proof.")]
    pub proof: Vec<u8>,
}

impl crate::crypto::hashable::Hashable for TakenHeaders {}

impl TakenHeaders {
    #[document(
        parent = "TakenHeaders",
        summary = "Checks the proof for oneself, as an app does: of this chain, made by the header-chain program given, with this build's verifier for its kind.",
        params = { hash = "The chain's hash.", genesis = "The chain's genesis id.", program = "The header-chain program governance names." },
        returns = "What it proves, or why it does not hold.",
    )]
    pub fn check(&self, hash: HashType, genesis: &Hash, program: &[u8; 32]) -> Result<ProvenHeaders, String> {
        let chain = ProvenHeaders::from_public_values(hash, &self.values).ok_or("public values that are no header chain")?;

        if chain.genesis != *genesis {
            return Err("a proof of another chain".to_string());
        }

        if chain.program != *program {
            return Err("a proof by another program".to_string());
        }

        super::verifier(self.kind).ok_or_else(|| format!("this build checks no {} proofs", self.kind.name()))?.verify(program, &self.values, &self.proof)?;

        Ok(chain)
    }
}

