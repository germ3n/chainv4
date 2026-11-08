//! The validity proofs' program for SP1: reads a range's input, runs it as a node runs those blocks
//! (chainv4::proofs::replay), and commits to what it proves, a ProvenRange's eleven words. A range
//! that does not hold panics, and SP1 proves nothing of it.

#![no_main]
sp1_zkvm::entrypoint!(main);

mod libc;

pub fn main() {
    libc::start_cxx();
    let bytes = sp1_zkvm::io::read_vec();
    let input = chainv4::proofs::RangeInput::from_bytes(&bytes).unwrap_or_else(|err| panic!("{err}"));
    let proven = chainv4::proofs::replay(&input).unwrap_or_else(|err| panic!("{err}"));

    sp1_zkvm::io::commit_slice(&proven.public_values());
}
