// SPDX-License-Identifier: MIT
// An example EVM contract for chainv4's calls between engines: it calls Lua and WebAssembly
// contracts through the native precompile, and they call it back. Built with solc 0.8 for Prague:
//
//   solc --evm-version prague --optimize --bin --abi Bridge.sol -o build
//
// Bridge.bin beside it is what that writes for Bridge, which chainv4's tests deploy; docs/evm.md
// walks through it.
pragma solidity ^0.8.24;

/// Lua and WebAssembly contracts, as the precompile at 0x…4e43 calls them. types lists the native
/// types of the arguments, such as "u64,string"; args are ABI-encoded as those; what the function
/// returns comes back ABI-encoded, one value after another.
interface Native {
    function callNative(bytes32 target, string calldata name, string calldata types, bytes calldata args)
        external
        payable
        returns (bytes memory);
}

contract Bridge {
    Native constant NATIVE = Native(address(0x4e43));

    uint256 public count;

    event Crossed(uint64 total);

    /// Adds to a native counter's total, sending it what this call carries, and returns the total.
    function add(bytes32 target, uint64 by) public payable returns (uint64 total) {
        bytes memory out = NATIVE.callNative{value: msg.value}(target, "add", "u64", abi.encode(by));
        total = abi.decode(out, (uint64));
        count += 1;
        emit Crossed(total);
    }

    /// Adds, then changes its mind: the native counter's addition is undone with the rest.
    function addThenRevert(bytes32 target, uint64 by) external {
        add(target, by);
        revert("changed my mind");
    }

    /// Runs addThenRevert as a call of its own, which reverts, and carries on: only what that call
    /// did is undone, the native addition with it.
    function tryAdd(bytes32 target, uint64 by) external returns (bool ok) {
        try this.addThenRevert(target, by) {
            ok = true;
        } catch {
            ok = false;
        }

        count += 10;
    }

    /// Reads a native function in a static call: it may change nothing, or the call reverts.
    function peek(bytes32 target, string calldata name) external view returns (uint64) {
        (bool ok, bytes memory out) =
            address(NATIVE).staticcall(abi.encodeCall(Native.callNative, (target, name, "", "")));

        if (!ok) {
            assembly {
                revert(add(out, 32), mload(out))
            }
        }

        return abi.decode(abi.decode(out, (bytes)), (uint64));
    }

    /// What native contracts call: adds to the count and returns it.
    function bump(uint256 by) external payable returns (uint256) {
        count += by;

        return count;
    }

    /// What a native contract calls to come back to another: adds to a native counter from here.
    function relay(bytes32 target, uint64 by) external returns (uint64) {
        return add(target, by);
    }

    receive() external payable {}
}
