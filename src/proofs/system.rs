//! Proof systems: which zkVM a proof was made with, and what checks it. Each kind is a cargo feature
//! of its own, so a build carries the verifiers it needs and none of the provers it does not.

use {
    r#macro::document,
    wincode::{SchemaRead, SchemaWrite},
};

#[document(
    summary = "A proof system: the zkVM a proof was made with and the proof it wraps it in. Its tag is pinned, as transactions and governance name it.",
    category = "Proofs",
    see_also = "ProofSystem, ProofSettings",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
#[wincode(tag_encoding = "u8")]
pub enum ProofKind {
    #[document(summary = "SP1, Succinct's RISC-V zkVM, its proof wrapped in Groth16 over BN254: about 260 bytes, checked on an EVM chain by SP1's verifier contract and here by sp1-verifier. Under the sp1 feature.")]
    #[wincode(tag = 0)]
    Sp1Groth16,
    /// Only in tests: a proof that is the keccak256 of the program and the public values, so the
    /// chain's side of proofs is tested without a zkVM.
    #[cfg(test)]
    #[wincode(tag = 255)]
    Test,
}

impl ProofKind {
    #[document(parent = "ProofKind", summary = "Every kind there is.")]
    pub const ALL: [ProofKind; 1] = [ProofKind::Sp1Groth16];

    #[document(parent = "ProofKind", summary = "Its name as JSON and the CLI write it: sp1-groth16.")]
    pub fn name(self) -> &'static str {
        match self {
            ProofKind::Sp1Groth16 => "sp1-groth16",
            #[cfg(test)]
            ProofKind::Test => "test",
        }
    }

    #[document(parent = "ProofKind", summary = "The kind a name names.")]
    pub fn parse(name: &str) -> Option<ProofKind> {
        ProofKind::ALL.into_iter().find(|kind| kind.name() == name)
    }
}

#[document(
    summary = "What checks one kind's proofs: a node checking Tx::Prove, and a light client marking how far the chain is proven. Stateless, so one value serves every caller.",
    category = "Proofs",
    see_also = "ProofKind, verifier",
)]
pub trait ProofSystem: Send + Sync {
    #[document(parent = "ProofSystem", summary = "The kind it checks.")]
    fn kind(&self) -> ProofKind;

    #[document(
        parent = "ProofSystem",
        summary = "Checks a proof that the program, run, committed to these public values.",
        params = {
            program = "The program, as the system names it.",
            public_values = "What it committed to: a ProvenRange's public values.",
            proof = "The proof.",
        },
        returns = "Why it does not hold, if it does not.",
    )]
    fn verify(&self, program: &[u8; 32], public_values: &[u8], proof: &[u8]) -> Result<(), String>;
}

#[document(
    summary = "What makes proofs of ranges: a zkVM's prover with the replay built as its program. Off the node, on whoever proves, by hand: the sp1-prover feature's Sp1Prover is one.",
    category = "Proofs",
    see_also = "ProofSystem, RangeInput",
)]
pub trait Prover {
    #[document(parent = "Prover", summary = "The kind it makes.")]
    fn kind(&self) -> ProofKind;

    #[document(parent = "Prover", summary = "The program it proves, as ProofSettings::program names it.")]
    fn program(&self) -> [u8; 32];

    #[document(
        parent = "Prover",
        summary = "Runs the program on a range's input, as RangeInput::to_bytes writes it, without proving: a check, and a measure of what proving it costs.",
        returns = "The public values it commits to, which ProvenRange::from_public_values reads, and the cycles it ran.",
    )]
    fn execute(&self, input: &[u8]) -> Result<(Vec<u8>, u64), String>;

    #[document(
        parent = "Prover",
        summary = "Proves a range's input.",
        returns = "The public values it proved, and the proof, as ProofSystem::verify and the contracts take it.",
    )]
    fn prove(&self, input: &[u8]) -> Result<(Vec<u8>, Vec<u8>), String>;
}

#[document(
    summary = "What checks a kind's proofs in this build, or None for one it was built without.",
    category = "Proofs",
    see_also = "ProofSystem",
)]
pub fn verifier(kind: ProofKind) -> Option<&'static dyn ProofSystem> {
    match kind {
        #[cfg(feature = "sp1")]
        ProofKind::Sp1Groth16 => Some(&Sp1Groth16),
        #[cfg(not(feature = "sp1"))]
        ProofKind::Sp1Groth16 => None,
        #[cfg(test)]
        ProofKind::Test => Some(&TestProofs),
    }
}

/// The test kind's checker: a proof is the keccak256 of the program and the public values.
#[cfg(test)]
pub(crate) struct TestProofs;

#[cfg(test)]
impl TestProofs {
    pub(crate) fn proof(program: &[u8; 32], public_values: &[u8]) -> Vec<u8> {
        crate::crypto::secp256k1::keccak(&[program.as_slice(), public_values].concat()).to_vec()
    }
}

#[cfg(test)]
impl ProofSystem for TestProofs {
    fn kind(&self) -> ProofKind {
        ProofKind::Test
    }

    fn verify(&self, program: &[u8; 32], public_values: &[u8], proof: &[u8]) -> Result<(), String> {
        match TestProofs::proof(program, public_values) == proof {
            true => Ok(()),
            false => Err("the test proof does not hold".to_string()),
        }
    }
}

#[cfg(feature = "sp1")]
#[document(
    summary = "SP1's Groth16 proofs, checked by sp1-verifier against SP1's own Groth16 verifying key: the proof as SP1's SDK writes it for an EVM chain, its verifier's 4-byte selector first.",
    category = "Proofs",
    see_also = "ProofKind::Sp1Groth16",
)]
pub struct Sp1Groth16;

#[cfg(feature = "sp1")]
impl ProofSystem for Sp1Groth16 {
    fn kind(&self) -> ProofKind {
        ProofKind::Sp1Groth16
    }

    fn verify(&self, program: &[u8; 32], public_values: &[u8], proof: &[u8]) -> Result<(), String> {
        let program = format!("0x{}", program.iter().map(|byte| format!("{byte:02x}")).collect::<String>());

        sp1_verifier::Groth16Verifier::verify(proof, public_values, &program, *sp1_verifier::GROTH16_VK_BYTES).map_err(|err| format!("the SP1 proof does not hold: {err}"))
    }
}
