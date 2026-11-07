// SPDX-License-Identifier: MIT
// A chainv4 native multisig's side on an EVM chain: a Safe-style contract its members control, M of
// N. It holds the coin, ERC-20 tokens and NFTs, and does what threshold of its owners sign: execute
// takes the call and their signatures over its EIP-712 digest and nonce, and runs it as the
// contract. Its owners and threshold change only through itself, signed the same way.
//
// An owner is one of three kinds:
//   an EVM key, checked by ecrecover, 65 bytes r, s, v;
//   a contract, asked by ERC-1271's isValidSignature, through a static call so it cannot change
//   anything while it answers: a Safe, a smart-account wallet, or another MultiSig;
//   an Ed25519 key, a chainv4 member's own, its 64-byte signature checked by chainv4's crypto
//   precompile on the chain it is from and by Ed25519.sol everywhere else. Its address here is its
//   chainv4 account's EVM address: the last 20 bytes of keccak256(0x00 || key).
// It answers ERC-1271 itself, threshold of its owners signing a hash wrapped in its own domain, so
// one MultiSig may own another and its execute signatures never stand in for anything else.
//
// It sits at one address on every EVM chain: deployed through the deterministic deployment proxy,
// with no salt, its init code naming the chainv4 multisig, chainv4's chain id and the owners and
// threshold it starts with, so anyone works out the address before it is deployed, coins can be sent
// to it first, and nobody but those owners can be the ones at it. Built with solc 0.8 for Paris, so
// it runs on every EVM chain, the ones without PUSH0 too:
//
//   solc --evm-version paris --optimize --bin --abi MultiSig.sol -o build
//
// MultiSig.bin beside it is what that writes. docs/multisig.md walks through it.
pragma solidity ^0.8.24;

import "./Ed25519.sol";

interface IERC1271 {
    function isValidSignature(bytes32 hash, bytes calldata signature) external view returns (bytes4);
}

contract MultiSig {
    uint8 public constant KEY = 0;
    uint8 public constant CONTRACT = 1;
    uint8 public constant ED25519 = 2;
    /// What isValidSignature answers for a signature it holds, ERC-1271's magic value.
    bytes4 private constant VALID = 0x1626ba7e;
    /// chainv4's crypto precompile, which checks Ed25519 there for a few thousand gas.
    address private constant CRYPTO = address(0x4e56);

    /// The chainv4 multisig this is the side of: its address's 32 bytes.
    bytes32 public immutable multisig;
    /// chainv4's own chain id, where the crypto precompile is trusted to check Ed25519.
    uint256 public immutable home;
    /// The owners, sorted by address, each once.
    address[] private ownerList;
    mapping(address => bool) public isOwner;
    /// Each owner's kind, and an Ed25519 owner's key.
    mapping(address => uint8) public kindOf;
    mapping(address => bytes32) public keyOf;
    uint256 public threshold;
    /// How many calls it has run: what the next one's signatures are over.
    uint256 public nonce;

    bytes32 private constant DOMAIN_TYPE = keccak256("EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)");
    bytes32 private constant EXECUTE_TYPE = keccak256("Execute(address to,uint256 value,bytes data,uint256 nonce)");
    bytes32 private constant MESSAGE_TYPE = keccak256("Message(bytes32 hash)");

    event Executed(uint256 indexed nonce, address indexed to, uint256 value);
    event OwnersChanged(address[] owners, uint256 threshold);

    constructor(bytes32 chainv4, uint256 chainv4Id, uint8[] memory kinds, bytes32[] memory keys, uint256 startThreshold) {
        multisig = chainv4;
        home = chainv4Id;
        setOwners(kinds, keys, startThreshold);
    }

    /// The owners, sorted.
    function getOwners() external view returns (address[] memory) {
        return ownerList;
    }

    /// The owners as setOwners takes them, in their order: each one's kind, and its key, an
    /// address in its last 20 bytes but for an Ed25519 owner.
    function getOwnerKeys() external view returns (uint8[] memory kinds, bytes32[] memory keys) {
        kinds = new uint8[](ownerList.length);
        keys = new bytes32[](ownerList.length);

        for (uint256 i = 0; i < ownerList.length; i++) {
            address owner = ownerList[i];
            kinds[i] = kindOf[owner];
            keys[i] = kindOf[owner] == ED25519 ? keyOf[owner] : bytes32(uint256(uint160(owner)));
        }
    }

    /// An owner's address from its kind and key: an Ed25519 key's chainv4 account's EVM address,
    /// or the address the key holds.
    function ownerOf(uint8 kind, bytes32 key) public pure returns (address) {
        if (kind == ED25519) {
            return address(uint160(uint256(keccak256(abi.encodePacked(uint8(0), key)))));
        }

        require(kind == KEY || kind == CONTRACT, "an owner is a key, a contract or an Ed25519 key");
        require(uint256(key) >> 160 == 0, "an address in the key's last 20 bytes");

        return address(uint160(uint256(key)));
    }

    function domain() private view returns (bytes32) {
        return keccak256(abi.encode(DOMAIN_TYPE, keccak256("chainv4 MultiSig"), keccak256("1"), block.chainid, address(this)));
    }

    /// The EIP-712 digest owners sign for a call: its target, value, data and the nonce it runs at,
    /// on this chain, for this contract.
    function digest(address to, uint256 value, bytes calldata data, uint256 atNonce) public view returns (bytes32) {
        bytes32 call = keccak256(abi.encode(EXECUTE_TYPE, to, value, keccak256(data), atNonce));

        return keccak256(abi.encodePacked("\x19\x01", domain(), call));
    }

    /// The EIP-712 digest owners sign for isValidSignature to hold a hash: the hash wrapped in this
    /// contract's domain, so no execute's signatures are ever one.
    function messageDigest(bytes32 hash) public view returns (bytes32) {
        return keccak256(abi.encodePacked("\x19\x01", domain(), keccak256(abi.encode(MESSAGE_TYPE, hash))));
    }

    /// Runs a call as this contract, once threshold of its owners signed it at the current nonce:
    /// the signers by address ascending, so none counts twice, each with its signature as its kind
    /// takes it. Anyone may send it; a call that fails reverts all of it.
    function execute(address to, uint256 value, bytes calldata data, address[] calldata signers, bytes[] calldata signatures) external returns (bytes memory) {
        require(signedBy(digest(to, value, data, nonce), signers, signatures), "fewer signatures than the threshold, or one by no owner, or out of order");
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

    /// ERC-1271: whether threshold of its owners signed messageDigest(hash), the signature being
    /// abi.encode(address[] signers, bytes[] signatures) as execute takes them.
    function isValidSignature(bytes32 hash, bytes calldata signature) external view returns (bytes4) {
        (address[] memory signers, bytes[] memory signatures) = abi.decode(signature, (address[], bytes[]));

        return signedBy(messageDigest(hash), signers, signatures) ? VALID : bytes4(0xffffffff);
    }

    /// Replaces the owners and threshold: only as one of its own calls, signed by the owners
    /// before.
    function changeOwners(uint8[] calldata kinds, bytes32[] calldata keys, uint256 nextThreshold) external {
        require(msg.sender == address(this), "only through execute");
        setOwners(kinds, keys, nextThreshold);
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
        // ERC-165 itself, ERC-1155's receiver and ERC-1271.
        return id == 0x01ffc9a7 || id == 0x4e2312e0 || id == 0x1626ba7e;
    }

    function setOwners(uint8[] memory kinds, bytes32[] memory keys, uint256 nextThreshold) private {
        require(kinds.length == keys.length, "a kind for each key");
        require(keys.length > 0 && keys.length <= 20, "1 to 20 owners");
        require(nextThreshold > 0 && nextThreshold <= keys.length, "a threshold of 1 to as many owners as there are");

        for (uint256 i = 0; i < ownerList.length; i++) {
            isOwner[ownerList[i]] = false;
            delete kindOf[ownerList[i]];
            delete keyOf[ownerList[i]];
        }

        address[] memory owners = new address[](keys.length);

        for (uint256 i = 0; i < keys.length; i++) {
            address owner = ownerOf(kinds[i], keys[i]);
            require(owner != address(0) && (i == 0 || owner > owners[i - 1]), "owners sorted by address, each once, none zero");
            owners[i] = owner;
            isOwner[owner] = true;
            kindOf[owner] = kinds[i];

            if (kinds[i] == ED25519) {
                keyOf[owner] = keys[i];
            }
        }

        ownerList = owners;
        threshold = nextThreshold;
        emit OwnersChanged(owners, nextThreshold);
    }

    /// Whether threshold of the owners, ascending, each signed signed.
    function signedBy(bytes32 signed, address[] memory signers, bytes[] memory signatures) private view returns (bool) {
        if (signers.length != signatures.length || signers.length < threshold) {
            return false;
        }

        address last = address(0);

        for (uint256 i = 0; i < signers.length; i++) {
            address owner = signers[i];

            if (owner <= last || !isOwner[owner] || !holds(owner, signed, signatures[i])) {
                return false;
            }

            last = owner;
        }

        return true;
    }

    /// Whether one owner's signature, as its kind takes it, is over signed.
    function holds(address owner, bytes32 signed, bytes memory signature) private view returns (bool) {
        uint8 kind = kindOf[owner];

        if (kind == KEY) {
            return signature.length == 65 && recover(signed, signature) == owner;
        }

        if (kind == CONTRACT) {
            // A static call: whatever the owner runs to answer, it changes nothing here or there.
            (bool ok, bytes memory answer) = owner.staticcall(abi.encodeWithSelector(IERC1271.isValidSignature.selector, signed, signature));

            return ok && answer.length >= 32 && abi.decode(answer, (bytes4)) == VALID;
        }

        if (signature.length != 64) {
            return false;
        }

        bytes32 key = keyOf[owner];
        bytes32 r;
        bytes32 s;

        assembly {
            r := mload(add(signature, 32))
            s := mload(add(signature, 64))
        }

        // chainv4's own precompile at home; anywhere else an address with no code there, or
        // something else, so Solidity checks it.
        if (block.chainid == home) {
            (bool ok, bytes memory answer) = CRYPTO.staticcall(abi.encodeWithSignature("verify(string,bytes,bytes,bytes)", "ed25519", abi.encodePacked(key), abi.encodePacked(signed), signature));

            return ok && answer.length == 32 && abi.decode(answer, (bool));
        }

        return Ed25519.verify(key, r, s, abi.encodePacked(signed));
    }

    function recover(bytes32 signed, bytes memory signature) private pure returns (address) {
        bytes32 r;
        bytes32 s;
        uint8 v = uint8(signature[64]);

        assembly {
            r := mload(add(signature, 32))
            s := mload(add(signature, 64))
        }

        // Low s only: one signature, one form.
        if (uint256(s) > 0x7fffffffffffffffffffffffffffffff5d576e7357a4501ddfe92f46681b20a0) {
            return address(0);
        }

        return ecrecover(signed, v < 27 ? v + 27 : v, r, s);
    }
}
