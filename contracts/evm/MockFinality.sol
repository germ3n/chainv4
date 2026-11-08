// SPDX-License-Identifier: MIT
// Only for tests: a ChainV4Finality.sol as Gateway.sol reads it, whose proven withdrawals and
// gateway mode a test sets, so Gateway.sol's proven deliveries are tested alone.
//
// Built with: solc --evm-version prague --optimize --bin MockFinality.sol
pragma solidity ^0.8.24;

contract MockFinality {
    mapping(bytes32 => bool) public proven;
    uint8 public gatewayMode;

    function set(bytes32 transfer, uint8 mode) external {
        proven[transfer] = true;
        gatewayMode = mode;
    }

    function withdrew(bytes32, bytes32 transfer, bytes32[] calldata) external view returns (bool) {
        return proven[transfer];
    }
}
