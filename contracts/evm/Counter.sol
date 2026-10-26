// SPDX-License-Identifier: MIT
// An example EVM contract for chainv4: a counter that remembers, emits events, reverts with a
// reason, takes and pays out the coin, creates contracts of its own, and reads the block. Built
// with solc 0.8 for Prague, the EVM chainv4 runs:
//
//   solc --evm-version prague --optimize --bin --abi Counter.sol -o build
//
// Counter.bin beside it is what that writes for Counter, which chainv4's tests and the docs
// deploy; docs/evm.md walks through it.
pragma solidity ^0.8.24;

contract Counter {
    uint256 public count;

    event Bumped(address indexed by, uint256 count);

    constructor(uint256 start) {
        count = start;
    }

    /// Adds to the count; adding nothing is refused, with a reason.
    function bump(uint256 by) external {
        require(by > 0, "nothing to add");
        count += by;
        emit Bumped(msg.sender, count);
    }

    /// Takes the coin sent with it.
    function deposit() external payable {}

    /// Pays the coin it holds to an address.
    function pay(address payable to, uint256 amount) external {
        (bool paid, ) = to.call{value: amount}("");
        require(paid, "the payment failed");
    }

    /// Creates a contract of its own, at the address CREATE gives it.
    function spawn() external returns (address) {
        return address(new Child());
    }

    /// The block, as the EVM sees it: its number, its time in seconds and the chain id.
    function blockInfo() external view returns (uint256, uint256, uint256) {
        return (block.number, block.timestamp, block.chainid);
    }

    /// Runs until the gas runs out.
    function spin() external pure {
        while (true) {}
    }
}

contract Child {
    uint256 public made = 7;
}
