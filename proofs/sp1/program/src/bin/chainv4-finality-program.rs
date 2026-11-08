//! The finality proofs' program for SP1: reads a link's input, checks two thirds of the target
//! epoch's voters signed it over a witness of the target's state (chainv4::proofs::prove_finality),
//! and commits to what it proves, a ProvenLink's twelve words, which ChainV4Finality.sol's
//! proveLink takes. A link that does not hold panics, and SP1 proves nothing of it. Its verifying
//! key is the finality program governance names.

#![no_main]
sp1_zkvm::entrypoint!(main);

#[path = "../libc.rs"]
mod libc;

pub fn main() {
    libc::start_cxx();
    let bytes = sp1_zkvm::io::read_vec();
    let input = chainv4::proofs::FinalityInput::from_bytes(&bytes).unwrap_or_else(|err| panic!("{err}"));
    let link = chainv4::proofs::prove_finality(&input).unwrap_or_else(|err| panic!("{err}"));

    sp1_zkvm::io::commit_slice(&link.public_values());
}
