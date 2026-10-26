// SPDX-License-Identifier: MIT
// An example EVM contract for chainv4's names: it looks names up through the names precompile and
// pays them. Built with solc 0.8 for Prague:
//
//   solc --evm-version prague --optimize --bin --abi NamePayer.sol -o build
//
// NamePayer.bin beside it is what that writes for NamePayer, which chainv4's tests deploy;
// docs/names.md walks through it.
pragma solidity ^0.8.24;

/// chainv4's names, as the precompile at 0x…4e53 answers for them.
interface Names {
    /// The address a name resolves to now, such as alice.dev, or zero.
    function resolve(string calldata name) external view returns (address);

    /// The name an address goes by, such as alice.dev, or empty.
    function name(address account) external view returns (string memory);
}

contract NamePayer {
    Names constant NAMES = Names(0x0000000000000000000000000000000000004e53);

    event Paid(string name, address to, uint256 amount);

    /// Where a name points.
    function lookup(string calldata name) external view returns (address) {
        return NAMES.resolve(name);
    }

    /// What an address is called.
    function whois(address account) external view returns (string memory) {
        return NAMES.name(account);
    }

    /// Pays what was sent to whatever a name resolves to.
    function pay(string calldata name) external payable {
        address to = NAMES.resolve(name);
        require(to != address(0), "no such name");
        (bool paid, ) = payable(to).call{value: msg.value}("");
        require(paid, "the payment failed");
        emit Paid(name, to, msg.value);
    }
}
