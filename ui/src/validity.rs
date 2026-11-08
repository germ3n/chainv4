//! How far a chain is proven by validity proofs, as the apps find it themselves: the latest proof a
//! node gives, in GET /status's validity, checked here by the proof system's own verifier against
//! the program governance names. Whoever served it adds nothing: a proof that holds proves itself.
//! Votes still carry the apps past it, to the newest blocks.
//!
//! Beside it, the latest header-chain proof (headers): the chain's finalized checkpoints proven from
//! genesis, checked the same way against the header-chain program.

use {
    chainv4::proofs::{ProofKind, ProvenRange, TakenProof},
    chainv4::structs::hash::{Hash, HashType},
    chainv4::structs::hex::from_hex,
    serde_json::Value,
};

/// What the apps found of a chain's validity proofs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Proven {
    /// Governance names no proof system: the chain takes no proofs.
    Off,
    /// It takes proofs, and none was taken yet.
    NoneYet,
    /// The latest proof holds, checked here: the chain is proven through this block.
    Checked { height: u64, block: String },
    /// A proof the node gave did not hold, or could not be checked here, and why.
    Unchecked { why: String },
}

impl Proven {
    /// A line to show beside the chain's finality.
    pub fn describe(&self) -> String {
        match self {
            Proven::Off => "the chain takes no validity proofs".to_string(),
            Proven::NoneYet => "no validity proof taken yet".to_string(),
            Proven::Checked { height, .. } => format!("proven through block {height}, the proof checked here"),
            Proven::Unchecked { why } => format!("a proof not checked: {why}"),
        }
    }
}

/// What the apps found of a chain's header-chain proofs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HeadersProven {
    /// Governance names no header-chain program.
    Off,
    /// It names one, and no proof was taken yet.
    NoneYet,
    /// The latest proof holds, checked here: finalized from genesis through this checkpoint.
    Checked { epoch: u64, height: u64, block: String },
    /// A proof the node gave did not hold, or could not be checked here, and why.
    Unchecked { why: String },
}

impl HeadersProven {
    /// A line to show beside the chain's finality.
    pub fn describe(&self) -> String {
        match self {
            HeadersProven::Off => "no header-chain proofs".to_string(),
            HeadersProven::NoneYet => "no header-chain proof taken yet".to_string(),
            HeadersProven::Checked { epoch, height, .. } => format!("finalized from genesis through epoch {epoch}, block {height}, the proof checked here"),
            HeadersProven::Unchecked { why } => format!("a header-chain proof not checked: {why}"),
        }
    }
}

/// Checks the latest header-chain proof in a validity object, as GET /status gives it, for the
/// chain with this genesis id and hash, against the header-chain program the object names unless
/// the caller proved its own.
pub fn headers_proven(validity: &Value, genesis: &str, hash: HashType, program: Option<[u8; 32]>) -> HeadersProven {
    let named = || validity["settings"]["headers_program"].as_str().and_then(from_hex).and_then(|bytes| <[u8; 32]>::try_from(bytes).ok());
    let Some(program) = program.or_else(named).filter(|program| *program != [0; 32]) else {
        return HeadersProven::Off;
    };
    let proof = &validity["headers"];

    if proof.is_null() {
        return HeadersProven::NoneYet;
    }

    let bytes = |value: &Value, what: &str| value.as_str().and_then(from_hex).ok_or_else(|| format!("no {what}"));
    let checked = (|| -> Result<chainv4::proofs::ProvenHeaders, String> {
        let kind = proof["kind"].as_str().and_then(ProofKind::parse).ok_or("a kind of proof this app does not know")?;
        let taken = chainv4::proofs::TakenHeaders { kind, values: bytes(&proof["public_values"], "public values")?, proof: bytes(&proof["proof"], "proof")? };
        let genesis = Hash::from_bytes(hash, &from_hex(genesis).ok_or("a genesis id that is not hex")?).ok_or("a genesis id of another length")?;

        taken.check(hash, &genesis, &program)
    })();

    match checked {
        Ok(headers) => HeadersProven::Checked { epoch: headers.finalized.checkpoint.epoch, height: headers.finalized.height, block: headers.finalized.checkpoint.block.to_string() },
        Err(why) => HeadersProven::Unchecked { why },
    }
}

/// Checks the latest proof in a validity object, as GET /status gives it, for the chain with this
/// genesis id and hash. The program is the one the object names, unless the caller proved its own.
pub fn proven(validity: &Value, genesis: &str, hash: HashType, program: Option<[u8; 32]>) -> Proven {
    if validity["settings"].is_null() {
        return Proven::Off;
    }

    let proof = &validity["proof"];

    if proof.is_null() {
        return Proven::NoneYet;
    }

    match check(validity, proof, genesis, hash, program) {
        Ok(range) => Proven::Checked { height: range.end.height, block: range.end.block.to_string() },
        Err(why) => Proven::Unchecked { why },
    }
}

fn check(validity: &Value, proof: &Value, genesis: &str, hash: HashType, program: Option<[u8; 32]>) -> Result<ProvenRange, String> {
    let bytes = |value: &Value, what: &str| value.as_str().and_then(from_hex).ok_or_else(|| format!("no {what}"));
    let kind = proof["kind"].as_str().and_then(ProofKind::parse).ok_or("a kind of proof this app does not know")?;
    let range = ProvenRange::from_public_values(hash, &bytes(&proof["public_values"], "public values")?).ok_or("public values that are no range")?;
    let program = match program {
        Some(program) => program,
        None => bytes(&validity["settings"]["program"], "program")?.try_into().map_err(|_| "a program of other than 32 bytes")?,
    };
    let genesis = Hash::from_bytes(hash, &from_hex(genesis).ok_or("a genesis id that is not hex")?).ok_or("a genesis id of another length")?;
    let taken = TakenProof { kind, range, proof: bytes(&proof["proof"], "proof")? };
    taken.check(&genesis, &program)?;

    Ok(range)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chain_without_settings_takes_none_and_a_proof_that_does_not_read_is_not_checked() {
        let hash = HashType::Blake3_256;
        let genesis = format!("0x{}", "11".repeat(32));

        assert_eq!(proven(&serde_json::json!({ "settings": null, "tip": null, "proof": null }), &genesis, hash, None), Proven::Off);
        let settings = serde_json::json!({ "kinds": ["sp1-groth16"], "program": format!("0x{}", "22".repeat(32)), "reward_per_block": "0", "gateway": "both" });
        assert_eq!(proven(&serde_json::json!({ "settings": settings, "tip": null, "proof": null }), &genesis, hash, None), Proven::NoneYet);

        let garbled = serde_json::json!({ "settings": settings, "proof": { "kind": "sp1-groth16", "public_values": "0x00", "proof": "0x00" } });
        assert!(matches!(proven(&garbled, &genesis, hash, None), Proven::Unchecked { .. }));
    }

    #[test]
    fn a_header_chain_proof_is_off_without_its_program_and_unchecked_when_it_does_not_read() {
        let hash = HashType::Blake3_256;
        let genesis = format!("0x{}", "11".repeat(32));
        let settings = serde_json::json!({ "kinds": ["sp1-groth16"], "program": format!("0x{}", "22".repeat(32)), "reward_per_block": "0", "gateway": "both" });

        assert_eq!(headers_proven(&serde_json::json!({ "settings": settings, "headers": null }), &genesis, hash, None), HeadersProven::Off, "no header-chain program named");
        let named = serde_json::json!({ "kinds": ["sp1-groth16"], "program": format!("0x{}", "22".repeat(32)), "reward_per_block": "0", "gateway": "both", "headers_program": format!("0x{}", "33".repeat(32)) });
        assert_eq!(headers_proven(&serde_json::json!({ "settings": named, "headers": null }), &genesis, hash, None), HeadersProven::NoneYet);
        let garbled = serde_json::json!({ "settings": named, "headers": { "kind": "sp1-groth16", "public_values": "0x00", "proof": "0x00" } });
        assert!(matches!(headers_proven(&garbled, &genesis, hash, None), HeadersProven::Unchecked { .. }));
    }
}
