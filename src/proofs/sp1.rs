//! Proving ranges with SP1 on this machine: its SDK's CPU prover, the program built from
//! proofs/sp1/program with cargo prove build, and the kind of proof asked for: Groth16 or PLONK,
//! which the contracts and the chain check, or the STARK unwrapped, which the chain does. Heavy, slow and by hand: the sp1-prover feature, which no node needs.

use {
    super::{ProofKind, Prover},
    r#macro::document,
    sp1_sdk::blocking::{CpuProver, ProveRequest as _, Prover as _},
    sp1_sdk::{HashableKey as _, ProvingKey as _, SP1Stdin},
};

#[document(
    summary = "SP1's prover with the validity proofs' program: set up once from the program's ELF, it runs ranges and proves them as one of SP1's kinds, Groth16, PLONK or the STARK.",
    category = "Proofs",
    see_also = "Prover, ProofKind::Sp1Groth16",
    example = "let prover = Sp1Prover::new(std::fs::read(\"proofs/sp1/elf/chainv4-proof-program\")?, ProofKind::Sp1Groth16)?;\nlet (values, proof) = prover.prove(&input.to_bytes())?;",
)]
pub struct Sp1Prover {
    prover: CpuProver,
    kind: ProofKind,
    elf: sp1_sdk::Elf,
    /// Set up the first time a proof or the program's key is asked for, as running needs none.
    key: std::sync::OnceLock<<CpuProver as sp1_sdk::blocking::Prover>::ProvingKey>,
}

impl Sp1Prover {
    #[document(
        parent = "Sp1Prover",
        summary = "SP1's prover with the program's ELF, what cargo prove build wrote, making proofs of one kind. Its keys are set up the first time a proof or the program's id is asked for.",
        params = { elf = "The program's ELF.", kind = "The kind of proof it makes: one of SP1's." },
        returns = "The prover, or why not: a kind that is not SP1's.",
    )]
    pub fn new(elf: Vec<u8>, kind: ProofKind) -> Result<Sp1Prover, String> {
        match kind {
            ProofKind::Sp1Groth16 | ProofKind::Sp1Plonk | ProofKind::Sp1Stark => {}
            #[cfg(test)]
            ProofKind::Test => return Err("SP1 makes no test proofs".to_string()),
        }

        Ok(Sp1Prover { prover: CpuProver::new(), kind, elf: elf.into(), key: std::sync::OnceLock::new() })
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
        self.kind
    }

    fn program(&self) -> [u8; 32] {
        self.key().map_or([0; 32], |key| key.verifying_key().bytes32_raw())
    }

    fn execute(&self, input: &[u8]) -> Result<(Vec<u8>, u64), String> {
        let (values, report) = self.prover.execute(self.elf.clone(), Sp1Prover::stdin(input)).run().map_err(|err| format!("the program did not run: {err}"))?;

        Ok((values.to_vec(), report.total_instruction_count()))
    }

    fn prove(&self, input: &[u8]) -> Result<(Vec<u8>, Vec<u8>), String> {
        let request = self.prover.prove(self.key()?, Sp1Prover::stdin(input));
        let request = match self.kind {
            ProofKind::Sp1Groth16 => request.groth16(),
            ProofKind::Sp1Plonk => request.plonk(),
            _ => request.compressed(),
        };
        let proof = request.run().map_err(|err| format!("SP1 did not prove it: {err}"))?;

        let bytes = match self.kind {
            // As sp1-verifier's STARK verifier reads it: the proof itself, bincode-encoded.
            ProofKind::Sp1Stark => bincode::serialize(&proof.proof).map_err(|err| format!("cannot write the STARK proof: {err}"))?,
            _ => proof.bytes(),
        };

        Ok((proof.public_values.to_vec(), bytes))
    }
}
