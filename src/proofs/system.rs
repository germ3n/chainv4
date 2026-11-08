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
    #[document(summary = "SP1, Succinct's RISC-V zkVM, its proof wrapped in Groth16 over BN254: about 260 bytes, checked on an EVM chain by SP1's Groth16 verifier and here by sp1-verifier. Its trusted setup is the circuit's own, from SP1's ceremony. Under the sp1-groth16 feature.")]
    #[wincode(tag = 0)]
    Sp1Groth16,
    #[document(summary = "SP1's proof wrapped in PLONK over BN254: under a kilobyte, a little dearer to check on an EVM chain than Groth16, its trusted setup the universal one every PLONK circuit shares. Under the sp1-plonk feature.")]
    #[wincode(tag = 1)]
    Sp1Plonk,
    #[document(summary = "SP1's own proof, its STARK compressed by recursion and not wrapped: no trusted setup at all, but about 1.27 MB, too big for an EVM chain to check, so only the chain and apps take it. Under the sp1-stark feature.")]
    #[wincode(tag = 2)]
    Sp1Stark,
    /// Only in tests: a proof that is the keccak256 of the program and the public values, so the
    /// chain's side of proofs is tested without a zkVM.
    #[cfg(test)]
    #[wincode(tag = 255)]
    Test,
}

impl ProofKind {
    #[document(parent = "ProofKind", summary = "Every kind there is.")]
    pub const ALL: [ProofKind; 3] = [ProofKind::Sp1Groth16, ProofKind::Sp1Plonk, ProofKind::Sp1Stark];

    #[document(parent = "ProofKind", summary = "Its name as JSON and the CLI write it: sp1-groth16, sp1-plonk or sp1-stark.")]
    pub fn name(self) -> &'static str {
        match self {
            ProofKind::Sp1Groth16 => "sp1-groth16",
            ProofKind::Sp1Plonk => "sp1-plonk",
            ProofKind::Sp1Stark => "sp1-stark",
            #[cfg(test)]
            ProofKind::Test => "test",
        }
    }

    #[document(parent = "ProofKind", summary = "The kind a name names.")]
    pub fn parse(name: &str) -> Option<ProofKind> {
        ProofKind::ALL.into_iter().find(|kind| kind.name() == name)
    }

    #[document(parent = "ProofKind", summary = "The most bytes a proof of this kind may be, as Tx::Prove carries it: 4 KiB for the wrapped kinds, which an EVM chain checks, and 2 MiB for the STARK, whose compressed proof is about 1.27 MB whatever the range.")]
    pub fn max_bytes(self) -> usize {
        match self {
            ProofKind::Sp1Groth16 | ProofKind::Sp1Plonk => 4_096,
            ProofKind::Sp1Stark => 2 * 1024 * 1024,
            #[cfg(test)]
            ProofKind::Test => 4_096,
        }
    }

    #[document(parent = "ProofKind", summary = "Whether an EVM chain can check it, so ChainV4Finality.sol and Gateway.sol can take it: the wrapped kinds, not the STARK.")]
    pub fn on_evm(self) -> bool {
        match self {
            ProofKind::Sp1Groth16 | ProofKind::Sp1Plonk => true,
            ProofKind::Sp1Stark => false,
            #[cfg(test)]
            ProofKind::Test => false,
        }
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
        #[cfg(feature = "sp1-groth16")]
        ProofKind::Sp1Groth16 => Some(&Sp1Groth16),
        #[cfg(not(feature = "sp1-groth16"))]
        ProofKind::Sp1Groth16 => None,
        #[cfg(feature = "sp1-plonk")]
        ProofKind::Sp1Plonk => Some(&Sp1Plonk),
        #[cfg(not(feature = "sp1-plonk"))]
        ProofKind::Sp1Plonk => None,
        #[cfg(feature = "sp1-stark")]
        ProofKind::Sp1Stark => Some(&Sp1Stark),
        #[cfg(not(feature = "sp1-stark"))]
        ProofKind::Sp1Stark => None,
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

/// SP1's program id, its verifying key hash as an EVM chain takes it, as the hex string
/// sp1-verifier's wrapped verifiers take.
#[cfg(any(feature = "sp1-groth16", feature = "sp1-plonk"))]
fn sp1_program_hex(program: &[u8; 32]) -> String {
    format!("0x{}", program.iter().map(|byte| format!("{byte:02x}")).collect::<String>())
}

#[cfg(feature = "sp1-groth16")]
#[document(
    summary = "SP1's Groth16 proofs, checked by sp1-verifier against SP1's own Groth16 verifying key: the proof as SP1's SDK writes it for an EVM chain, its verifier's 4-byte selector first.",
    category = "Proofs",
    see_also = "ProofKind::Sp1Groth16",
)]
pub struct Sp1Groth16;

#[cfg(feature = "sp1-groth16")]
impl ProofSystem for Sp1Groth16 {
    fn kind(&self) -> ProofKind {
        ProofKind::Sp1Groth16
    }

    fn verify(&self, program: &[u8; 32], public_values: &[u8], proof: &[u8]) -> Result<(), String> {
        sp1_verifier::Groth16Verifier::verify(proof, public_values, &sp1_program_hex(program), *sp1_verifier::GROTH16_VK_BYTES).map_err(|err| format!("the SP1 Groth16 proof does not hold: {err}"))
    }
}

#[cfg(feature = "sp1-plonk")]
#[document(
    summary = "SP1's PLONK proofs, checked by sp1-verifier against SP1's own PLONK verifying key: the proof as SP1's SDK writes it for an EVM chain, its verifier's 4-byte selector first.",
    category = "Proofs",
    see_also = "ProofKind::Sp1Plonk",
)]
pub struct Sp1Plonk;

#[cfg(feature = "sp1-plonk")]
impl ProofSystem for Sp1Plonk {
    fn kind(&self) -> ProofKind {
        ProofKind::Sp1Plonk
    }

    fn verify(&self, program: &[u8; 32], public_values: &[u8], proof: &[u8]) -> Result<(), String> {
        sp1_verifier::PlonkVerifier::verify(proof, public_values, &sp1_program_hex(program), *sp1_verifier::PLONK_VK_BYTES).map_err(|err| format!("the SP1 PLONK proof does not hold: {err}"))
    }
}

#[cfg(feature = "sp1-stark")]
#[document(
    summary = "SP1's compressed STARK proofs, checked by sp1-verifier's recursion verifier: the proof as SP1's SDK makes it in compressed mode, bincode-encoded. The program is the same id the wrapped kinds take, its eight 31-bit words unpacked.",
    category = "Proofs",
    see_also = "ProofKind::Sp1Stark, sp1_stark_key",
)]
pub struct Sp1Stark;

#[cfg(feature = "sp1-stark")]
impl ProofSystem for Sp1Stark {
    fn kind(&self) -> ProofKind {
        ProofKind::Sp1Stark
    }

    fn verify(&self, program: &[u8; 32], public_values: &[u8], proof: &[u8]) -> Result<(), String> {
        let key = sp1_stark_key(program).ok_or("the program is no SP1 verifying key hash")?;

        sp1_verifier::compressed::SP1CompressedVerifierRaw::verify_with_public_values(proof, public_values, &key).map_err(|err| format!("the SP1 STARK proof does not hold: {err}"))
    }
}

#[document(
    summary = "SP1's verifying key hash as its STARK verifier takes it, from the 32-byte id the wrapped kinds take: that id packs eight 31-bit field words, the first highest, so they are unpacked and written as bincode writes eight u32s.",
    category = "Proofs",
    see_also = "Sp1Stark",
    returns = "The key, or None for an id that packs no eight words below the field's prime.",
)]
pub fn sp1_stark_key(program: &[u8; 32]) -> Option<Vec<u8>> {
    // KoalaBear's prime, SP1's field: 2^31 - 2^24 + 1.
    const PRIME: u32 = 0x7f00_0001;
    let mut packed = crate::structs::u256::u256::from_be_bytes(*program);

    let mut words = [0u32; 8];
    for word in words.iter_mut().rev() {
        *word = (packed & crate::structs::u256::u256::from(0x7fff_ffffu32)).as_u32();
        packed >>= 31;
    }

    if packed != 0 || words.iter().any(|word| *word >= PRIME) {
        return None;
    }

    Some(words.iter().flat_map(|word| word.to_le_bytes()).collect())
}
