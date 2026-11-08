// SPDX-License-Identifier: MIT
// Only for tests: an SP1 verifier that takes a proof that is keccak256 of the program and the
// public values, as chainv4's test proof kind makes them, so ChainV4Finality.sol's proofs are
// tested without a zkVM. Never deployed anywhere but a test.
//
// Built with: solc --evm-version prague --optimize --bin MockSp1Verifier.sol
pragma solidity ^0.8.24;

contract MockSp1Verifier {
    function verifyProof(bytes32 programVKey, bytes calldata publicValues, bytes calldata proofBytes) external pure {
        require(proofBytes.length == 32 && keccak256(abi.encodePacked(programVKey, publicValues)) == bytes32(proofBytes), "the test proof does not hold");
    }
}
