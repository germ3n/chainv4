//! Calls EVM contracts: bumps a Solidity counter and reads what it says back, through the EVM and
//! Solidity's ABI, both worked on the node.

#![no_std]

extern crate alloc;

use chainv4_sdk::{Address, Value, abi_decode, abi_encode, abort, evm_call, export};

/// Bumps an EVM contract's count with its bump(uint256), and returns the count it returns.
#[export]
fn bump(counter: Address, by: u64) -> Value {
    let input = abi_encode("bump(uint256)", &[Value::U64(by)]).unwrap_or_else(|why| abort(&why));
    let output = evm_call(&counter, [0; 32], &input).unwrap_or_else(|why| abort(&why));
    let mut values = abi_decode("(uint256)", &output).unwrap_or_else(|why| abort(&why));

    values.pop().unwrap_or_else(|| abort("the counter said nothing"))
}

/// Calls something that reverts, and returns why, as the EVM gave it.
#[export]
fn why(counter: Address, input: alloc::vec::Vec<u8>) -> alloc::string::String {
    match evm_call(&counter, [0; 32], &input) {
        Ok(_) => abort("it did not revert"),
        Err(why) => why,
    }
}
