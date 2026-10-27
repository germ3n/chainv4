// SPDX-License-Identifier: MIT
// Atomic swaps on any EVM chain: Ethereum's side of a swap, as contracts/lua/swap.lua is
// chainv4's. Whoever pays locks the coin, an ERC-20 token, an ERC-721 NFT or an amount of an
// ERC-1155 id under the SHA-256 of a secret, for a
// recipient, until a deadline. Anyone may send the claim with the secret before the deadline,
// which pays the recipient and shows the secret to the other chain; from the deadline on, anyone
// may send the refund, which pays the locker back. Built with solc 0.8 for Paris, so it runs on
// every EVM chain, the ones without PUSH0 too:
//
//   solc --evm-version paris --optimize --bin --abi Swap.sol -o build
//
// Swap.bin beside it is what that writes for Swap: chainv4's apps deploy it, through the
// deterministic deployment proxy where a chain has one, so it sits at one address on every chain
// it was deployed to. docs/swaps.md walks through it.
pragma solidity ^0.8.24;

interface ERC20 {
    function balanceOf(address owner) external view returns (uint256);
}

interface ERC721 {
    function ownerOf(uint256 id) external view returns (address);
    function transferFrom(address from, address to, uint256 id) external;
}

interface ERC1155 {
    function balanceOf(address owner, uint256 id) external view returns (uint256);
    function safeTransferFrom(address from, address to, uint256 id, uint256 amount, bytes calldata data) external;
}

contract Swap {
    enum State {
        None,
        Locked,
        Claimed,
        Refunded
    }

    /// What a lock holds: the coin or an ERC-20, by its token; an ERC-721 token; or an amount of
    /// an ERC-1155 id.
    enum Kind {
        Fungible,
        Nft,
        Multi
    }

    struct Lock {
        State state;
        uint64 deadline;
        uint64 height;
        address locker;
        address recipient;
        address token;
        uint256 amount;
        bytes32 secret;
        Kind kind;
        uint256 id;
    }

    mapping(bytes32 => Lock) private locks;
    /// Set only while lockMulti moves tokens in, so ERC-1155 tokens sent any other way are refused.
    bool private receiving;

    event Locked(bytes32 indexed hash, address indexed locker, address indexed recipient, address token, uint256 amount, uint64 deadline);
    event LockedItem(bytes32 indexed hash, uint8 kind, uint256 id);
    event Claimed(bytes32 indexed hash, bytes32 secret);
    event Refunded(bytes32 indexed hash);

    /// Locks the coin this call sends, for recipient to claim with the secret before deadline, in
    /// Unix seconds.
    function lock(bytes32 hash, address recipient, uint64 deadline) external payable {
        open(hash, recipient, deadline, address(0), msg.value);
    }

    /// Locks amount of an ERC-20 token, moved with transferFrom: approve this contract for it
    /// first. A token that takes a fee as it moves is refused, as the lock would hold less than it
    /// says.
    function lockToken(bytes32 hash, address recipient, uint64 deadline, address token, uint256 amount) external {
        require(token.code.length > 0, "the token is not a contract");
        uint256 before = ERC20(token).balanceOf(address(this));
        open(hash, recipient, deadline, token, amount);
        move(token, abi.encodeWithSelector(0x23b872dd, msg.sender, address(this), amount));
        require(ERC20(token).balanceOf(address(this)) - before == amount, "the token moved less than the amount");
    }

    /// Locks an ERC-721 token, moved with transferFrom: approve this contract for it first. The
    /// token is the collection, and the amount 1.
    function lockNft(bytes32 hash, address recipient, uint64 deadline, address collection, uint256 id) external {
        require(collection.code.length > 0, "the collection is not a contract");
        open(hash, recipient, deadline, collection, 1);
        locks[hash].kind = Kind.Nft;
        locks[hash].id = id;
        ERC721(collection).transferFrom(msg.sender, address(this), id);
        require(ERC721(collection).ownerOf(id) == address(this), "the NFT did not move");
        emit LockedItem(hash, uint8(Kind.Nft), id);
    }

    /// Locks an amount of an ERC-1155 id, moved with safeTransferFrom: setApprovalForAll for this
    /// contract first.
    function lockMulti(bytes32 hash, address recipient, uint64 deadline, address collection, uint256 id, uint256 amount) external {
        require(collection.code.length > 0, "the collection is not a contract");
        uint256 before = ERC1155(collection).balanceOf(address(this), id);
        open(hash, recipient, deadline, collection, amount);
        locks[hash].kind = Kind.Multi;
        locks[hash].id = id;
        receiving = true;
        ERC1155(collection).safeTransferFrom(msg.sender, address(this), id, amount, "");
        receiving = false;
        require(ERC1155(collection).balanceOf(address(this), id) - before == amount, "the tokens moved less than the amount");
        emit LockedItem(hash, uint8(Kind.Multi), id);
    }

    /// Takes ERC-1155 tokens only as lockMulti moves them in.
    function onERC1155Received(address, address, uint256, uint256, bytes calldata) external view returns (bytes4) {
        require(receiving, "send tokens with lockMulti");
        return this.onERC1155Received.selector;
    }

    function onERC1155BatchReceived(address, address, uint256[] calldata, uint256[] calldata, bytes calldata) external pure returns (bytes4) {
        revert("send tokens with lockMulti");
    }

    /// ERC-165: an ERC-1155 receiver.
    function supportsInterface(bytes4 id) external pure returns (bool) {
        return id == 0x01ffc9a7 || id == 0x4e2312e0;
    }

    /// Pays the swap whose hash is the secret's SHA-256 to its recipient, before its deadline.
    function claim(bytes32 secret) external {
        bytes32 hash = sha256(abi.encodePacked(secret));
        Lock storage held = locks[hash];
        require(held.state == State.Locked, "no swap waits on that secret");
        require(block.timestamp < held.deadline, "too late to claim: its deadline passed");
        held.state = State.Claimed;
        held.secret = secret;
        emit Claimed(hash, secret);
        pay(held, held.recipient);
    }

    /// Pays a swap nobody claimed back to its locker, from its deadline on.
    function refund(bytes32 hash) external {
        Lock storage held = locks[hash];
        require(held.state == State.Locked, "no swap is locked under that hash");
        require(block.timestamp >= held.deadline, "too early to refund: its deadline is ahead");
        held.state = State.Refunded;
        emit Refunded(hash);
        pay(held, held.locker);
    }

    /// A swap's state, 0 none, 1 locked, 2 claimed, 3 refunded; who locked it and for whom; its
    /// token, the zero address for the coin; how much; its deadline; its secret once claimed; and
    /// the block it was locked in, for the other side to wait until that block is final.
    function get(bytes32 hash)
        external
        view
        returns (uint8 state, address locker, address recipient, address token, uint256 amount, uint64 deadline, bytes32 secret, uint64 height)
    {
        Lock storage held = locks[hash];
        return (uint8(held.state), held.locker, held.recipient, held.token, held.amount, held.deadline, held.secret, held.height);
    }

    /// What a swap holds: 0 the coin or an ERC-20, 1 an ERC-721 token, 2 an ERC-1155 amount; and
    /// the NFT's id, or 0. get's token is the collection, and its amount how many.
    function item(bytes32 hash) external view returns (uint8 kind, uint256 id) {
        Lock storage held = locks[hash];
        return (uint8(held.kind), held.id);
    }

    function open(bytes32 hash, address recipient, uint64 deadline, address token, uint256 amount) private {
        require(locks[hash].state == State.None, "a swap under that hash already exists");
        require(amount > 0, "lock something");
        require(recipient != address(0), "a swap pays someone");
        require(deadline > block.timestamp, "its deadline must be after this block's time");
        locks[hash] = Lock(State.Locked, deadline, uint64(block.number), msg.sender, recipient, token, amount, bytes32(0), Kind.Fungible, 0);
        emit Locked(hash, msg.sender, recipient, token, amount, deadline);
    }

    function pay(Lock storage held, address to) private {
        if (held.kind == Kind.Nft) {
            ERC721(held.token).transferFrom(address(this), to, held.id);
        } else if (held.kind == Kind.Multi) {
            ERC1155(held.token).safeTransferFrom(address(this), to, held.id, held.amount, "");
        } else if (held.token == address(0)) {
            (bool sent, ) = to.call{value: held.amount}("");
            require(sent, "the coin would not go");
        } else {
            move(held.token, abi.encodeWithSelector(0xa9059cbb, to, held.amount));
        }
    }

    /// Calls a token, taking no answer as yes, as tokens that predate ERC-20's return value give.
    function move(address token, bytes memory call) private {
        (bool done, bytes memory answer) = token.call(call);
        require(done && (answer.length == 0 || abi.decode(answer, (bool))), "the token would not move");
    }
}
