// SPDX-License-Identifier: MIT
// A chainv4 native multisig's side on an EVM chain: a Safe-style contract its members' EVM keys
// control, M of N. It holds the coin, ERC-20 tokens and NFTs, and does what threshold of its owners
// sign: execute takes the call and their EIP-712 signatures over it and its nonce, and runs it as
// the contract. Its owners and threshold change only through itself, signed the same way.
//
// It sits at one address on every EVM chain: deployed through the deterministic deployment proxy,
// with no salt, its init code naming the chainv4 multisig and the owners and threshold it starts
// with, so anyone works out the address before it is deployed, coins can be sent to it first, and
// nobody but those owners can be the ones at it. Built with solc 0.8 for Paris, so it runs on every
// EVM chain, the ones without PUSH0 too:
//
//   solc --evm-version paris --optimize --bin --abi MultiSig.sol -o build
//
// MultiSig.bin beside it is what that writes. docs/multisig.md walks through it.
pragma solidity ^0.8.24;

contract MultiSig {
    /// The chainv4 multisig this is the side of: its address's 32 bytes.
    bytes32 public immutable multisig;
    /// The owners, sorted, each once.
    address[] private ownerList;
    mapping(address => bool) public isOwner;
    uint256 public threshold;
    /// How many calls it has run: what the next one's signatures are over.
    uint256 public nonce;

    bytes32 private constant DOMAIN_TYPE = keccak256("EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)");
    bytes32 private constant EXECUTE_TYPE = keccak256("Execute(address to,uint256 value,bytes data,uint256 nonce)");

    event Executed(uint256 indexed nonce, address indexed to, uint256 value);
    event OwnersChanged(address[] owners, uint256 threshold);

    constructor(bytes32 chainv4, address[] memory owners, uint256 startThreshold) {
        multisig = chainv4;
        setOwners(owners, startThreshold);
    }

    /// The owners, sorted.
    function getOwners() external view returns (address[] memory) {
        return ownerList;
    }

    /// The EIP-712 digest owners sign for a call: its target, value, data and the nonce it runs at,
    /// on this chain, for this contract.
    function digest(address to, uint256 value, bytes calldata data, uint256 atNonce) public view returns (bytes32) {
        bytes32 domain = keccak256(abi.encode(DOMAIN_TYPE, keccak256("chainv4 MultiSig"), keccak256("1"), block.chainid, address(this)));
        bytes32 call = keccak256(abi.encode(EXECUTE_TYPE, to, value, keccak256(data), atNonce));

        return keccak256(abi.encodePacked("\x19\x01", domain, call));
    }

    /// Runs a call as this contract, once threshold of its owners signed it at the current nonce:
    /// their 65-byte signatures, r, s and v, one after another, by owner address ascending, so
    /// none counts twice. Anyone may send it; a call that fails reverts all of it.
    function execute(address to, uint256 value, bytes calldata data, bytes calldata signatures) external returns (bytes memory) {
        bytes32 signed = digest(to, value, data, nonce);
        uint256 count = signatures.length / 65;
        require(signatures.length == count * 65 && count >= threshold, "fewer signatures than the threshold");
        address last = address(0);

        for (uint256 i = 0; i < count; i++) {
            address owner = recover(signed, signatures[i * 65:(i + 1) * 65]);
            require(owner > last && isOwner[owner], "a signature by no owner, or out of order");
            last = owner;
        }

        nonce += 1;
        emit Executed(nonce - 1, to, value);
        (bool ok, bytes memory returned) = to.call{value: value}(data);

        if (!ok) {
            assembly {
                revert(add(returned, 32), mload(returned))
            }
        }

        return returned;
    }

    /// Replaces the owners and threshold: only as one of its own calls, signed by the owners
    /// before.
    function changeOwners(address[] calldata owners, uint256 nextThreshold) external {
        require(msg.sender == address(this), "only through execute");
        setOwners(owners, nextThreshold);
    }

    receive() external payable {}

    /// ERC-721's and ERC-1155's checks before they send it a token: it takes them.
    function onERC721Received(address, address, uint256, bytes calldata) external pure returns (bytes4) {
        return this.onERC721Received.selector;
    }

    function onERC1155Received(address, address, uint256, uint256, bytes calldata) external pure returns (bytes4) {
        return this.onERC1155Received.selector;
    }

    function onERC1155BatchReceived(address, address, uint256[] calldata, uint256[] calldata, bytes calldata) external pure returns (bytes4) {
        return this.onERC1155BatchReceived.selector;
    }

    function supportsInterface(bytes4 id) external pure returns (bool) {
        // ERC-165 itself, and ERC-1155's receiver.
        return id == 0x01ffc9a7 || id == 0x4e2312e0;
    }

    function setOwners(address[] memory owners, uint256 nextThreshold) private {
        require(owners.length > 0 && owners.length <= 20, "1 to 20 owners");
        require(nextThreshold > 0 && nextThreshold <= owners.length, "a threshold of 1 to as many owners as there are");

        for (uint256 i = 0; i < ownerList.length; i++) {
            isOwner[ownerList[i]] = false;
        }

        for (uint256 i = 0; i < owners.length; i++) {
            require(owners[i] != address(0) && (i == 0 || owners[i] > owners[i - 1]), "owners sorted, each once, none zero");
            isOwner[owners[i]] = true;
        }

        ownerList = owners;
        threshold = nextThreshold;
        emit OwnersChanged(owners, nextThreshold);
    }

    function recover(bytes32 signed, bytes calldata signature) private pure returns (address) {
        bytes32 r = bytes32(signature[0:32]);
        bytes32 s = bytes32(signature[32:64]);
        uint8 v = uint8(signature[64]);

        // Low s only: one signature, one form.
        if (uint256(s) > 0x7fffffffffffffffffffffffffffffff5d576e7357a4501ddfe92f46681b20a0) {
            return address(0);
        }

        return ecrecover(signed, v < 27 ? v + 27 : v, r, s);
    }
}
