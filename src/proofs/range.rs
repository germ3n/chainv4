//! What a proof says: the range it ran, as words a contract reads, and the root of what left
//! through the bridge in it.

#[cfg(feature = "secp256k1")]
use crate::crypto::secp256k1::keccak;
use {
    crate::structs::hash::{Hash, HashType},
    crate::structs::u256::u256,
    r#macro::document,
    wincode::{SchemaRead, SchemaWrite},
};

#[document(
    summary = "One end of a proven range: a block's height, id and state root.",
    category = "Proofs",
    see_also = "ProvenRange",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct RangeEnd {
    #[document(summary = "The block's height.")]
    pub height: u64,
    #[document(summary = "The block's id.")]
    pub block: Hash,
    #[document(summary = "The state root its header commits to.")]
    pub state_root: Hash,
}

#[document(
    summary = "How Gateway.sol on an EVM chain releases what left here, as governance sets it: by a proof or the federation's signatures, either; only by both; or by the federation's alone, proofs refused.",
    category = "Proofs",
    see_also = "ProofSettings",
)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, SchemaWrite, SchemaRead)]
#[wincode(tag_encoding = "u8")]
pub enum GatewayMode {
    #[document(summary = "A proof of the range it left in, or the federation's signatures: proven first, the federation for what is not proven yet.")]
    #[wincode(tag = 0)]
    #[default]
    ProofOrFederation,
    #[document(summary = "A proof and the federation's signatures both.")]
    #[wincode(tag = 1)]
    Both,
    #[document(summary = "The federation's signatures alone, as before proofs: Gateway.sol refuses a release by proof. The last proof Ethereum takes carries it there, and from then on it follows proofs no more.")]
    #[wincode(tag = 2)]
    FederationOnly,
}

impl GatewayMode {
    #[document(parent = "GatewayMode", summary = "Every mode, by its number.")]
    pub const ALL: [GatewayMode; 3] = [GatewayMode::ProofOrFederation, GatewayMode::Both, GatewayMode::FederationOnly];

    #[document(parent = "GatewayMode", summary = "Its name as JSON and the CLI write it: proof-or-federation, both or federation-only.")]
    pub fn name(self) -> &'static str {
        match self {
            GatewayMode::ProofOrFederation => "proof-or-federation",
            GatewayMode::Both => "both",
            GatewayMode::FederationOnly => "federation-only",
        }
    }

    #[document(parent = "GatewayMode", summary = "The mode a name names.")]
    pub fn parse(name: &str) -> Option<GatewayMode> {
        GatewayMode::ALL.into_iter().find(|mode| mode.name() == name)
    }

    /// Its number, as the public values and Gateway.sol hold it.
    fn number(self) -> u8 {
        match self {
            GatewayMode::ProofOrFederation => 0,
            GatewayMode::Both => 1,
            GatewayMode::FederationOnly => 2,
        }
    }
}

#[document(
    summary = "What governance sets for validity proofs (Change::Proofs): the proof systems the chain takes, the programs of each kind of proof (ranges of blocks, links of finality, the header chain), what it mints for each block a range covers and each epoch a header chain adds, and how Gateway.sol releases. Unset, the chain takes no proofs.",
    category = "Proofs",
    see_also = "ProofKind, GatewayMode, Tx::Prove",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct ProofSettings {
    #[document(summary = "The proof systems the chain takes proofs made with, any of them, in the order ProofKind::ALL lists them: a switch from one to another votes the new one in beside the old, then the old one out, so no prover is left with a range half proven.")]
    pub kinds: Vec<crate::proofs::ProofKind>,
    #[document(summary = "The program proven, as the system names it: SP1's verifying key hash. A release whose replay runs otherwise is another program, which governance names in turn.")]
    pub program: [u8; 32],
    #[document(summary = "What is minted to whoever proves, for each block a proof covers: 0 pays nothing.")]
    #[wincode(with = "crate::structs::u256::U256Le")]
    pub reward_per_block: u256,
    #[document(summary = "How Gateway.sol releases what left here.")]
    pub gateway: GatewayMode,
    #[document(summary = "The finality program: what proves a link of finality, two thirds of an epoch's voters signing it, for ChainV4Finality.sol beside the attesters' signatures. Zeros for none.")]
    pub finality_program: [u8; 32],
    #[document(summary = "The header-chain program: what proves the chain's finalized checkpoints from genesis, each proof checking the one before, for the apps to sync by (Tx::ProveHeaders). Zeros for none.")]
    pub headers_program: [u8; 32],
    #[document(summary = "What is minted to whoever proves the header chain, for each epoch its proof moves the proven finalized checkpoint on: 0 pays nothing.")]
    #[wincode(with = "crate::structs::u256::U256Le")]
    pub reward_per_epoch: u256,
}

impl crate::crypto::hashable::Hashable for ProofSettings {}

impl ProofSettings {
    #[document(parent = "ProofSettings", summary = "Why no chain takes these settings, if none does: no kind, kinds out of order or twice, or every program zeros, naming none.")]
    pub fn problem(&self) -> Option<&'static str> {
        let rank = |kind: &crate::proofs::ProofKind| crate::proofs::ProofKind::ALL.iter().position(|known| known == kind).unwrap_or(usize::MAX);

        if self.kinds.is_empty() {
            return Some("no proof system named");
        }

        if !self.kinds.windows(2).all(|pair| rank(&pair[0]) < rank(&pair[1])) {
            return Some("proof systems named twice or out of order");
        }

        (self.program == [0; 32] && self.finality_program == [0; 32] && self.headers_program == [0; 32]).then_some("a program of zeros names none")
    }

    #[document(parent = "ProofSettings", summary = "Whether the chain takes proofs of this kind.")]
    pub fn takes(&self, kind: crate::proofs::ProofKind) -> bool {
        self.kinds.contains(&kind)
    }
}

#[document(
    summary = "What a validity proof proves: that every block from start, exclusive, to end ran by the rules of the chain with this genesis, from start's state root to end's, and what left through the bridge's gateway on the way. Its public values are eleven words, as abi.encode lays them out, which the zkVM commits to and a contract decodes.",
    category = "Proofs",
    see_also = "RangeEnd, ProofSystem, replay",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct ProvenRange {
    #[document(summary = "The chain's EVM chain id, from its genesis id.")]
    pub chain_id: u64,
    #[document(summary = "The genesis id, which names every rule the range ran by.")]
    pub genesis: Hash,
    #[document(summary = "Where it starts: its state is what the range ran from.")]
    pub start: RangeEnd,
    #[document(summary = "Where it ends.")]
    pub end: RangeEnd,
    #[document(summary = "The keccak256 Merkle root of the digests of every transfer the directory's gateway sent in the range, in order (withdrawals_root): what Gateway.sol releases by.")]
    pub withdrawals: [u8; 32],
    #[document(summary = "The program governance names at the end, zero for none: the one the next range is proven by, as the contracts follow it.")]
    pub program: [u8; 32],
    #[document(summary = "How Gateway.sol releases, as governance has it at the end.")]
    pub gateway: GatewayMode,
}

impl ProvenRange {
    #[document(parent = "ProvenRange", summary = "How many 32-byte words the public values are.")]
    pub const WORDS: usize = 11;

    #[document(
        parent = "ProvenRange",
        summary = "The public values: abi.encode(chainId, genesis, startHeight, startBlock, startRoot, endHeight, endBlock, endRoot, withdrawals, program, gatewayMode), every hash a 32-byte word.",
    )]
    pub fn public_values(&self) -> Vec<u8> {
        let number = |value: u64| u256::from(value).to_be_bytes();
        let words: [[u8; 32]; ProvenRange::WORDS] = [
            number(self.chain_id),
            word(&self.genesis),
            number(self.start.height),
            word(&self.start.block),
            word(&self.start.state_root),
            number(self.end.height),
            word(&self.end.block),
            word(&self.end.state_root),
            self.withdrawals,
            self.program,
            number(self.gateway.number().into()),
        ];

        words.concat()
    }

    #[document(
        parent = "ProvenRange",
        summary = "A range from its public values, hashes read as the chain's.",
        returns = "None for bytes that are not eleven words of a range.",
    )]
    pub fn from_public_values(hash: HashType, bytes: &[u8]) -> Option<ProvenRange> {
        if bytes.len() != ProvenRange::WORDS * 32 {
            return None;
        }

        let words: Vec<[u8; 32]> = bytes.as_chunks::<32>().0.to_vec();
        let number = |at: usize| -> Option<u64> { u64::try_from(u256::from_be_bytes(words[at])).ok() };
        let hash_at = |at: usize| Hash::from_bytes(hash, &words[at]);
        let gateway = match number(10)? {
            0 => GatewayMode::ProofOrFederation,
            1 => GatewayMode::Both,
            2 => GatewayMode::FederationOnly,
            _ => return None,
        };

        Some(ProvenRange {
            chain_id: number(0)?,
            genesis: hash_at(1)?,
            start: RangeEnd { height: number(2)?, block: hash_at(3)?, state_root: hash_at(4)? },
            end: RangeEnd { height: number(5)?, block: hash_at(6)?, state_root: hash_at(7)? },
            withdrawals: words[8],
            program: words[9],
            gateway,
        })
    }
}

/// A hash as a word: the chain's hash is a 256-bit one, so its bytes as they are.
fn word(hash: &Hash) -> [u8; 32] {
    let mut word = [0; 32];
    let bytes = hash.to_bytes();
    let at = bytes.len().min(32);
    word[..at].copy_from_slice(&bytes[..at]);

    word
}

#[cfg(feature = "secp256k1")]
/// A leaf of the withdrawals tree: keccak256 of the transfer's digest, so no leaf is a node.
fn leaf(digest: &[u8; 32]) -> [u8; 32] {
    keccak(digest)
}

#[cfg(feature = "secp256k1")]
/// Two nodes' parent, the lower first, so a proof needs no left or right.
fn parent(a: &[u8; 32], b: &[u8; 32]) -> [u8; 32] {
    let (low, high) = if a <= b { (a, b) } else { (b, a) };

    keccak(&[low.as_slice(), high.as_slice()].concat())
}

#[cfg(feature = "secp256k1")]
/// Every level of the tree over some leaves, the leaves first and the root last.
fn levels(digests: &[[u8; 32]]) -> Vec<Vec<[u8; 32]>> {
    let mut levels = vec![digests.iter().map(leaf).collect::<Vec<_>>()];

    while levels.last().is_some_and(|level| level.len() > 1) {
        let below = levels.last().expect("a level");
        // An odd one out goes up as it is.
        let above = below.chunks(2).map(|pair| if let [a, b] = pair { parent(a, b) } else { pair[0] }).collect();
        levels.push(above);
    }

    levels
}

#[cfg(feature = "secp256k1")]
#[document(
    summary = "The root of the withdrawals tree over transfers' digests, in the order they left: each leaf the keccak256 of a digest, each node the keccak256 of its two children, the lower first, an odd one out carried up; zero for none. What Gateway.sol checks a withdrawal's proof against.",
    category = "Proofs",
    see_also = "withdrawal_proof, verify_withdrawal",
)]
pub fn withdrawals_root(digests: &[[u8; 32]]) -> [u8; 32] {
    match digests.is_empty() {
        true => [0; 32],
        false => levels(digests).last().expect("a level")[0],
    }
}

#[cfg(feature = "secp256k1")]
#[document(
    summary = "The proof that one transfer is in a withdrawals tree: the sibling on each level up, where it has one.",
    params = {
        digests = "Every digest of the range, in order.",
        index = "Which of them.",
    },
    returns = "None for an index past the end.",
    category = "Proofs",
    see_also = "withdrawals_root, verify_withdrawal",
)]
pub fn withdrawal_proof(digests: &[[u8; 32]], index: usize) -> Option<Vec<[u8; 32]>> {
    if index >= digests.len() {
        return None;
    }

    let mut proof = Vec::new();
    let mut at = index;

    for level in levels(digests).iter().take_while(|level| level.len() > 1) {
        if let Some(sibling) = level.get(at ^ 1) {
            proof.push(*sibling);
        }

        at /= 2;
    }

    Some(proof)
}

#[cfg(feature = "secp256k1")]
#[document(
    summary = "Whether a transfer's digest is in the withdrawals tree with this root, by its proof: what Gateway.sol's deliverProven checks.",
    category = "Proofs",
    see_also = "withdrawals_root",
)]
pub fn verify_withdrawal(root: &[u8; 32], digest: &[u8; 32], proof: &[[u8; 32]]) -> bool {
    *root != [0; 32] && proof.iter().fold(leaf(digest), |node, sibling| parent(&node, sibling)) == *root
}

#[document(
    summary = "A validity proof the chain took, as a node keeps the latest beside the chain: what a light client checks for itself, by the program governance names, to mark how far the chain is proven.",
    category = "Proofs",
    see_also = "Tx::Prove, ProofSystem",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct TakenProof {
    #[document(summary = "The proof system it was made with.")]
    pub kind: crate::proofs::ProofKind,
    #[document(summary = "What it proves.")]
    pub range: ProvenRange,
    #[document(summary = "The proof.")]
    pub proof: Vec<u8>,
}

impl crate::crypto::hashable::Hashable for TakenProof {}

impl TakenProof {
    #[document(
        parent = "TakenProof",
        summary = "Checks the proof for oneself: of this chain, by the program given, which a light client proves from state (Change::Proofs), with this build's verifier for its kind.",
        params = {
            genesis = "The chain's genesis id.",
            program = "The program governance names.",
        },
        returns = "Why it does not hold, if it does not: another chain, no verifier in this build, or a proof that fails.",
    )]
    pub fn check(&self, genesis: &Hash, program: &[u8; 32]) -> Result<(), String> {
        if self.range.genesis != *genesis {
            return Err("a proof of another chain".to_string());
        }

        let verifier = crate::proofs::verifier(self.kind).ok_or_else(|| format!("this build checks no {} proofs", self.kind.name()))?;

        verifier.verify(program, &self.range.public_values(), &self.proof)
    }
}
