//! The withdrawal proofs' program for SP1: reads a withdrawal's input, checks the header-chain proof
//! and the range proof inside by their kinds' verifiers, that the range ends where the header chain
//! finalized, and that the withdrawal is in the range's Bitcoin withdrawals
//! (chainv4::proofs::prove_withdrawal); and commits to what it proves, a ProvenWithdrawal's nine
//! words, which BitVM's watchers check, and in phase 2 Bitcoin's script. Its verifying key is the
//! withdrawal program governance names.

#![no_main]
sp1_zkvm::entrypoint!(main);

#[path = "../libc.rs"]
mod libc;

pub fn main() {
    libc::start_cxx();
    let bytes = sp1_zkvm::io::read_vec();
    let input = chainv4::proofs::WithdrawalInput::from_bytes(&bytes).unwrap_or_else(|err| panic!("{err}"));
    let withdrawal = chainv4::proofs::prove_withdrawal(&input).unwrap_or_else(|err| panic!("{err}"));

    sp1_zkvm::io::commit_slice(&withdrawal.public_values());
}
