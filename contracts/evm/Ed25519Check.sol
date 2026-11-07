// SPDX-License-Identifier: MIT
// For the tests of Ed25519.sol's: its signature check and its SHA-512, called straight, as
// MultiSig.sol calls them on chains without chainv4's crypto precompile. Built as MultiSig.sol is:
//
//   solc --evm-version paris --optimize --bin --abi Ed25519Check.sol -o build
pragma solidity ^0.8.24;

import "./Ed25519.sol";

contract Ed25519Check {
    function verify(bytes32 key, bytes32 r, bytes32 s, bytes calldata message) external pure returns (bool) {
        return Ed25519.verify(key, r, s, message);
    }

    function sha512(bytes calldata message) external pure returns (bytes memory) {
        return Ed25519.sha512(message);
    }
}
