// SPDX-License-Identifier: MIT
// Atomic swaps on any EVM chain: Ethereum's side of a swap, as contracts/lua/swap.lua is
// chainv4's. Whoever pays locks the coin, an ERC-20 token, an ERC-721 NFT or an amount of an
// ERC-1155 id under the SHA-256 of a secret, for a recipient, until a deadline. Anyone may send the
// claim with the secret before the deadline, which shows the secret to the other chain and pays
// the recipient; from the deadline on, anyone may send the refund, which pays the locker back.
//
// Each swap is kept by its id: keccak256(abi.encode(hash, locker, recipient, token, kind, nftId,
// amount, deadline)), all its terms, as idOf computes it, the same as contracts/lua/swap.lua and
// src/swap. Someone who saw the hash and locks under it first makes a swap of their own, and takes
// nothing from the real one.
//
// A claim records the swap as claimed, and keeps its secret, even when the payout is refused, as a
// token its issuer can pause, or a refusing ERC-1155 hook, would refuse it: the secret is out once
// the claim is sent, so the swap must be the recipient's from then on. The payout is then owed,
// and withdraw, which anyone may send, tries it again. A claimed swap is never refunded.
//
// Built with solc 0.8 for Paris, so it runs on every EVM chain, the ones without PUSH0 too:
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

    /// One swap, its fields in the order that packs them into the fewest slots.
    struct Lock {
        address locker;
        State state;
        Kind kind;
        /// Claimed, but its payout refused: withdraw pays it.
        bool owed;
        uint64 deadline;
        address recipient;
        uint64 height;
        address token;
        uint256 amount;
        bytes32 hash;
        bytes32 secret;
        uint256 nftId;
    }

    mapping(bytes32 => Lock) private locks;
    /// Set only while lockMulti moves tokens in, so ERC-1155 tokens sent any other way are refused.
    bool private receiving;

    event Locked(bytes32 indexed id, bytes32 indexed hash, address indexed locker, address recipient, address token, uint256 amount, uint64 deadline);
    event LockedItem(bytes32 indexed id, uint8 kind, uint256 nftId);
    event Claimed(bytes32 indexed id, bytes32 secret);
    event Owed(bytes32 indexed id);
    event Withdrawn(bytes32 indexed id);
    event Refunded(bytes32 indexed id);

    /// The id of a swap of these terms: kind 0 for the coin or an ERC-20, 1 an ERC-721 token, 2 an
    /// ERC-1155 amount; the token the zero address for the coin; the deadline in Unix seconds.
    function idOf(bytes32 hash, address locker, address recipient, address token, uint8 kind, uint256 nftId, uint256 amount, uint64 deadline)
        public
        pure
        returns (bytes32)
    {
        return keccak256(abi.encode(hash, locker, recipient, token, kind, nftId, amount, deadline));
    }

    /// Locks the coin this call sends, for recipient to claim with the secret before deadline, in
    /// Unix seconds. Returns the swap's id.
    function lock(bytes32 hash, address recipient, uint64 deadline) external payable returns (bytes32 id) {
        id = open(hash, recipient, deadline, address(0), Kind.Fungible, 0, msg.value);
    }

    /// Locks amount of an ERC-20 token, moved with transferFrom: approve this contract for it
    /// first. A token that takes a fee as it moves is refused, as the lock would hold less than it
    /// says.
    function lockToken(bytes32 hash, address recipient, uint64 deadline, address token, uint256 amount) external returns (bytes32 id) {
        require(token.code.length > 0, "the token is not a contract");
        uint256 before = ERC20(token).balanceOf(address(this));
        id = open(hash, recipient, deadline, token, Kind.Fungible, 0, amount);
        move(token, abi.encodeWithSelector(0x23b872dd, msg.sender, address(this), amount));
        require(ERC20(token).balanceOf(address(this)) - before == amount, "the token moved less than the amount");
    }

    /// Locks an ERC-721 token, moved with transferFrom: approve this contract for it first. The
    /// token is the collection, and the amount 1.
    function lockNft(bytes32 hash, address recipient, uint64 deadline, address collection, uint256 nftId) external returns (bytes32 id) {
        require(collection.code.length > 0, "the collection is not a contract");
        id = open(hash, recipient, deadline, collection, Kind.Nft, nftId, 1);
        ERC721(collection).transferFrom(msg.sender, address(this), nftId);
        require(ERC721(collection).ownerOf(nftId) == address(this), "the NFT did not move");
        emit LockedItem(id, uint8(Kind.Nft), nftId);
    }

    /// Locks an amount of an ERC-1155 id, moved with safeTransferFrom: setApprovalForAll for this
    /// contract first.
    function lockMulti(bytes32 hash, address recipient, uint64 deadline, address collection, uint256 nftId, uint256 amount) external returns (bytes32 id) {
        require(collection.code.length > 0, "the collection is not a contract");
        uint256 before = ERC1155(collection).balanceOf(address(this), nftId);
        id = open(hash, recipient, deadline, collection, Kind.Multi, nftId, amount);
        receiving = true;
        ERC1155(collection).safeTransferFrom(msg.sender, address(this), nftId, amount, "");
        receiving = false;
        require(ERC1155(collection).balanceOf(address(this), nftId) - before == amount, "the tokens moved less than the amount");
        emit LockedItem(id, uint8(Kind.Multi), nftId);
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

    /// Claims the swap with an id with its secret, whose SHA-256, through Ethereum's own
    /// precompile, is its hash, before its deadline. The swap is the recipient's from then on: its
    /// payout is tried at once, and when refused is owed, for withdraw to pay.
    function claim(bytes32 id, bytes32 secret) external {
        Lock storage held = locks[id];
        require(held.state == State.Locked, "no swap waits under that id");
        require(sha256(abi.encodePacked(secret)) == held.hash, "no swap waits on that secret: its hash is not the swap's");
        require(block.timestamp < held.deadline, "too late to claim: its deadline passed");
        held.state = State.Claimed;
        held.secret = secret;
        emit Claimed(id, secret);

        // The secret is in this call's calldata whether it runs or not, so the claim stands even
        // when the payout is refused. Nothing is owed while it is tried, so a recipient or token
        // called back during it finds nothing to withdraw.
        try this.payOut(id) {} catch {
            held.owed = true;
            emit Owed(id);
        }
    }

    /// Pays a claimed swap's recipient: for claim alone, through a call it can catch.
    function payOut(bytes32 id) external {
        require(msg.sender == address(this), "only claim pays out this way");
        Lock storage held = locks[id];
        pay(held, held.recipient);
    }

    /// Pays a claimed swap's recipient what its claim could not. Anyone may send it, as often as it
    /// takes.
    function withdraw(bytes32 id) external {
        Lock storage held = locks[id];
        require(held.state == State.Claimed && held.owed, "nothing is owed under that id");
        held.owed = false;
        emit Withdrawn(id);
        pay(held, held.recipient);
    }

    /// Pays a swap nobody claimed back to its locker, from its deadline on.
    function refund(bytes32 id) external {
        Lock storage held = locks[id];
        require(held.state == State.Locked, "no swap is locked under that id");
        require(block.timestamp >= held.deadline, "too early to refund: its deadline is ahead");
        held.state = State.Refunded;
        emit Refunded(id);
        pay(held, held.locker);
    }

    /// A swap's state, 0 none, 1 locked, 2 claimed, 3 refunded; who locked it and for whom; its
    /// token, the zero address for the coin; how much; its deadline; its secret once claimed; the
    /// block it was locked in, for the other side to wait until that block is final; and whether
    /// its payout is owed.
    function get(bytes32 id)
        external
        view
        returns (uint8 state, address locker, address recipient, address token, uint256 amount, uint64 deadline, bytes32 secret, uint64 height, bool owed)
    {
        Lock storage held = locks[id];
        return (uint8(held.state), held.locker, held.recipient, held.token, held.amount, held.deadline, held.secret, held.height, held.owed);
    }

    /// What a swap holds: 0 the coin or an ERC-20, 1 an ERC-721 token, 2 an ERC-1155 amount; and
    /// the NFT's id, or 0. get's token is the collection, and its amount how many.
    function item(bytes32 id) external view returns (uint8 kind, uint256 nftId) {
        Lock storage held = locks[id];
        return (uint8(held.kind), held.nftId);
    }

    function open(bytes32 hash, address recipient, uint64 deadline, address token, Kind kind, uint256 nftId, uint256 amount) private returns (bytes32 id) {
        require(amount > 0, "lock something");
        require(recipient != address(0), "a swap pays someone");
        require(deadline > block.timestamp, "its deadline must be after this block's time");
        id = idOf(hash, msg.sender, recipient, token, uint8(kind), nftId, amount, deadline);
        Lock storage held = locks[id];
        require(held.state == State.None, "a swap of these terms already exists");
        held.locker = msg.sender;
        held.state = State.Locked;
        held.kind = kind;
        held.deadline = deadline;
        held.recipient = recipient;
        held.height = uint64(block.number);
        held.token = token;
        held.amount = amount;
        held.hash = hash;
        held.nftId = nftId;
        emit Locked(id, hash, msg.sender, recipient, token, amount, deadline);
    }

    function pay(Lock storage held, address to) private {
        if (held.kind == Kind.Nft) {
            ERC721(held.token).transferFrom(address(this), to, held.nftId);
        } else if (held.kind == Kind.Multi) {
            ERC1155(held.token).safeTransferFrom(address(this), to, held.nftId, held.amount, "");
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
