//! Proving ranges with SP1 on this machine: its SDK's CPU prover, the program built from
//! proofs/sp1/program with cargo prove build, and its Groth16 wrapping, which the contracts and
//! the chain check. Heavy, slow and by hand: the sp1-prover feature, which no node needs.

use {
    super::{ProofKind, Prover},
    r#macro::document,
    sp1_sdk::blocking::{CpuProver, ProveRequest as _, Prover as _},
    sp1_sdk::{HashableKey as _, ProvingKey as _, SP1Stdin},
};

#[document(
    summary = "SP1's prover with the validity proofs' program: set up once from the program's ELF, it runs ranges and proves them in Groth16.",
    category = "Proofs",
    see_also = "Prover, ProofKind::Sp1Groth16",
    example = "let prover = Sp1Prover::new(std::fs::read(\"proofs/sp1/elf/chainv4-proof-program\")?)?;\nlet (values, proof) = prover.prove(&input.to_bytes())?;",
)]
pub struct Sp1Prover {
    prover: CpuProver,
    elf: sp1_sdk::Elf,
    /// Set up the first time a proof or the program's key is asked for, as running needs none.
    key: std::sync::OnceLock<<CpuProver as sp1_sdk::blocking::Prover>::ProvingKey>,
}

impl Sp1Prover {
    #[document(parent = "Sp1Prover", summary = "SP1's prover with the program's ELF, what cargo prove build wrote. Its keys are set up the first time a proof or the program's id is asked for.")]
    pub fn new(elf: Vec<u8>) -> Result<Sp1Prover, String> {
        Ok(Sp1Prover { prover: CpuProver::new(), elf: elf.into(), key: std::sync::OnceLock::new() })
    }

    fn key(&self) -> Result<&<CpuProver as sp1_sdk::blocking::Prover>::ProvingKey, String> {
        if let Some(key) = self.key.get() {
            return Ok(key);
        }

        let key = self.prover.setup(self.elf.clone()).map_err(|err| format!("SP1's setup: {err}"))?;

        Ok(self.key.get_or_init(|| key))
    }

    fn stdin(input: &[u8]) -> SP1Stdin {
        let mut stdin = SP1Stdin::new();
        stdin.write_vec(input.to_vec());

        stdin
    }
}

impl Prover for Sp1Prover {
    fn kind(&self) -> ProofKind {
        ProofKind::Sp1Groth16
    }

    fn program(&self) -> [u8; 32] {
        self.key().map_or([0; 32], |key| key.verifying_key().bytes32_raw())
    }

    fn execute(&self, input: &[u8]) -> Result<(Vec<u8>, u64), String> {
        let (values, report) = self.prover.execute(self.elf.clone(), Sp1Prover::stdin(input)).run().map_err(|err| format!("the program did not run: {err}"))?;

        Ok((values.to_vec(), report.total_instruction_count()))
    }

    fn prove(&self, input: &[u8]) -> Result<(Vec<u8>, Vec<u8>), String> {
        let proof = self.prover.prove(self.key()?, Sp1Prover::stdin(input)).groth16().run().map_err(|err| format!("SP1 did not prove it: {err}"))?;

        Ok((proof.public_values.to_vec(), proof.bytes()))
    }
}
