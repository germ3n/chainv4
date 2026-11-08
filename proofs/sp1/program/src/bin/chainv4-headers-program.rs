//! The header-chain proofs' program for SP1: reads its input, checks the previous header-chain
//! proof by its kind's verifier, or starts from genesis, then each link after it
//! (chainv4::proofs::prove_headers), and commits to what it proves, a ProvenHeaders's twelve words,
//! which Tx::ProveHeaders takes and the apps check. Its verifying key is the header-chain program
//! governance names; each proof takes only a previous one by the same program.

#![no_main]
sp1_zkvm::entrypoint!(main);

#[path = "../libc.rs"]
mod libc;

pub fn main() {
    libc::start_cxx();
    let bytes = sp1_zkvm::io::read_vec();
    let input = chainv4::proofs::HeadersInput::from_bytes(&bytes).unwrap_or_else(|err| panic!("{err}"));
    let headers = chainv4::proofs::prove_headers(&input).unwrap_or_else(|err| panic!("{err}"));

    sp1_zkvm::io::commit_slice(&headers.public_values());
}
