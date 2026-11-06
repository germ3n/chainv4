// SPDX-License-Identifier: MIT
// A flash swap's borrower, for the tests of Dex.sol's: borrows from a pair, and in its callback
// pays back what it took and the fee on it, or, when told to, a unit less, which the pair refuses.
// Built as Dex.sol is:
//
//   solc --evm-version paris --optimize --bin --abi FlashBorrower.sol -o build
pragma solidity ^0.8.24;

interface IERC20 {
    function transfer(address to, uint256 value) external returns (bool);
}

interface IDexPair {
    function token0() external view returns (address);
    function token1() external view returns (address);
    function fee() external view returns (uint256);
    function swap(uint256 amount0Out, uint256 amount1Out, address to, bytes calldata data) external;
}

contract FlashBorrower {
    /// Borrows amount0 of the pair's first token, repaying it with the fee, short by `short`.
    function borrow(IDexPair pair, uint256 amount0, uint256 short) external {
        pair.swap(amount0, 0, address(this), abi.encode(short));
    }

    function dexCall(address, uint256 amount0, uint256, bytes calldata data) external {
        IDexPair pair = IDexPair(msg.sender);
        uint256 short = abi.decode(data, (uint256));
        // What keeps the product: the amount, and the fee on what comes back, rounded up.
        uint256 owed = amount0 * 10000 / (10000 - pair.fee()) + 1;
        require(IERC20(pair.token0()).transfer(msg.sender, owed - short), "the repayment would not move");
    }
}
