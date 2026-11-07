// SPDX-License-Identifier: MIT
// Makes Collectible and Items collections from a contract, as a factory does: what the chain's
// collection registry lists from any EVM creation, a transaction's own or a contract's CREATE. The
// collection tests use it. Built with solc 0.8 for Paris:
//
//   solc --evm-version paris --optimize --bin --abi CollectibleFactory.sol -o build
pragma solidity ^0.8.24;

import "./Collectibles.sol";

contract CollectibleFactory {
    event Made(address made);

    /// A new ERC-721 collection, which the factory mints.
    function make(string calldata name_, string calldata symbol_) external returns (address made) {
        made = address(new Collectible(name_, symbol_));
        emit Made(made);
    }

    /// A new ERC-1155 collection, by CREATE2 with the salt given.
    function makeItems(bytes32 salt) external returns (address made) {
        made = address(new Items{salt: salt}());
        emit Made(made);
    }
}
