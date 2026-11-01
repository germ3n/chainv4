// SPDX-License-Identifier: MIT
// A bridge's gateway on an EVM chain (docs/bridges.md), contracts/lua/gateway.lua's twin: ETH,
// ERC-20s, ERC-721s and ERC-1155s leave for chainv4 through it, held here until they come back,
// and what arrives from chainv4 is minted here as wrapped ERC-20s, ERC-721s and ERC-1155s,
// which go back burned. A transfer arrives with the federation's signatures over its digest,
// which anyone relays from chainv4's gateway; the signer set changes only when the one in force
// signs the next. The wrapped contracts are deployed once, as templates, and each wrapped asset is
// a minimal proxy of one (EIP-1167), so the gateway stays under the EVM's size limit. Built with
// solc 0.8 for Paris, through the IR pipeline:
//
//   solc --evm-version paris --optimize --via-ir --bin --abi Gateway.sol -o build
pragma solidity ^0.8.24;

interface IERC20 {
    function transfer(address to, uint256 amount) external returns (bool);
    function transferFrom(address from, address to, uint256 amount) external returns (bool);
    function balanceOf(address owner) external view returns (uint256);
    function name() external view returns (string memory);
    function symbol() external view returns (string memory);
    function decimals() external view returns (uint8);
}

interface IERC721 {
    function transferFrom(address from, address to, uint256 id) external;
    function ownerOf(uint256 id) external view returns (address);
    function tokenURI(uint256 id) external view returns (string memory);
    function name() external view returns (string memory);
    function symbol() external view returns (string memory);
}

interface IERC1155 {
    function safeTransferFrom(address from, address to, uint256 id, uint256 amount, bytes calldata data) external;
    function balanceOf(address owner, uint256 id) external view returns (uint256);
    function uri(uint256 id) external view returns (string memory);
}

interface Receiver1155 {
    function onERC1155Received(address operator, address from, uint256 id, uint256 amount, bytes calldata data) external returns (bytes4);
}

/// Set once, by the gateway that cloned it: a template's own storage is never set, so it serves
/// no one.
abstract contract Once {
    address public gateway;

    function claim() internal {
        require(gateway == address(0), "set up already");
        gateway = msg.sender;
    }
}

/// A wrapped coin or token: an ERC-20 only its gateway mints and burns.
contract WrappedToken is Once {
    string public name;
    string public symbol;
    uint8 public decimals;
    uint256 public totalSupply;
    mapping(address => uint256) public balanceOf;
    mapping(address => mapping(address => uint256)) public allowance;

    event Transfer(address indexed from, address indexed to, uint256 value);
    event Approval(address indexed owner, address indexed spender, uint256 value);

    function initialize(string calldata name_, string calldata symbol_, uint8 decimals_) external {
        claim();
        (name, symbol, decimals) = (name_, symbol_, decimals_);
    }

    function transfer(address to, uint256 amount) external returns (bool) {
        move(msg.sender, to, amount);
        return true;
    }

    function approve(address spender, uint256 amount) external returns (bool) {
        allowance[msg.sender][spender] = amount;
        emit Approval(msg.sender, spender, amount);
        return true;
    }

    function transferFrom(address from, address to, uint256 amount) external returns (bool) {
        allowance[from][msg.sender] -= amount;
        move(from, to, amount);
        return true;
    }

    function mint(address to, uint256 amount) external {
        require(msg.sender == gateway, "only the gateway mints");
        totalSupply += amount;
        balanceOf[to] += amount;
        emit Transfer(address(0), to, amount);
    }

    function burn(address from, uint256 amount) external {
        require(msg.sender == gateway, "only the gateway burns");
        balanceOf[from] -= amount;
        totalSupply -= amount;
        emit Transfer(from, address(0), amount);
    }

    function move(address from, address to, uint256 amount) private {
        balanceOf[from] -= amount;
        balanceOf[to] += amount;
        emit Transfer(from, to, amount);
    }
}

/// A wrapped NFT collection: an ERC-721 only its gateway mints and burns, each token with the
/// URI it had on chainv4.
contract WrappedNft is Once {
    string public name;
    string public symbol;
    mapping(uint256 => address) public ownerOf;
    mapping(address => uint256) public balanceOf;
    mapping(uint256 => address) public getApproved;
    mapping(address => mapping(address => bool)) public isApprovedForAll;
    mapping(uint256 => string) public tokenURI;

    event Transfer(address indexed from, address indexed to, uint256 indexed id);
    event Approval(address indexed owner, address indexed approved, uint256 indexed id);
    event ApprovalForAll(address indexed owner, address indexed operator, bool approved);

    function initialize(string calldata name_, string calldata symbol_) external {
        claim();
        (name, symbol) = (name_, symbol_);
    }

    function approve(address approved, uint256 id) external {
        address owner = ownerOf[id];
        require(msg.sender == owner || isApprovedForAll[owner][msg.sender], "not the owner or an operator");
        getApproved[id] = approved;
        emit Approval(owner, approved, id);
    }

    function setApprovalForAll(address operator, bool approved) external {
        isApprovedForAll[msg.sender][operator] = approved;
        emit ApprovalForAll(msg.sender, operator, approved);
    }

    function transferFrom(address from, address to, uint256 id) public {
        require(ownerOf[id] == from && to != address(0), "not from's token, or to nobody");
        require(msg.sender == from || getApproved[id] == msg.sender || isApprovedForAll[from][msg.sender], "not the owner, approved or an operator");
        delete getApproved[id];
        (ownerOf[id], balanceOf[from], balanceOf[to]) = (to, balanceOf[from] - 1, balanceOf[to] + 1);
        emit Transfer(from, to, id);
    }

    function safeTransferFrom(address from, address to, uint256 id) external {
        transferFrom(from, to, id);
    }

    function mint(address to, uint256 id, string calldata uri) external {
        require(msg.sender == gateway, "only the gateway mints");
        require(ownerOf[id] == address(0) && to != address(0), "minted already, or to nobody");
        (ownerOf[id], balanceOf[to], tokenURI[id]) = (to, balanceOf[to] + 1, uri);
        emit Transfer(address(0), to, id);
    }

    function burn(address from, uint256 id) external {
        require(msg.sender == gateway, "only the gateway burns");
        require(ownerOf[id] == from, "not from's token");
        delete ownerOf[id];
        delete getApproved[id];
        delete tokenURI[id];
        balanceOf[from] -= 1;
        emit Transfer(from, address(0), id);
    }

    function supportsInterface(bytes4 id) external pure returns (bool) {
        return id == 0x01ffc9a7 || id == 0x80ac58cd || id == 0x5b5e139f;
    }
}

/// A wrapped multi-token collection: an ERC-1155 only its gateway mints and burns.
contract WrappedMulti is Once {
    string public name;
    string public symbol;
    mapping(uint256 => mapping(address => uint256)) private balances;
    mapping(address => mapping(address => bool)) public isApprovedForAll;
    mapping(uint256 => string) private uris;

    event TransferSingle(address indexed operator, address indexed from, address indexed to, uint256 id, uint256 value);
    event ApprovalForAll(address indexed owner, address indexed operator, bool approved);

    function initialize(string calldata name_, string calldata symbol_) external {
        claim();
        (name, symbol) = (name_, symbol_);
    }

    function balanceOf(address owner, uint256 id) external view returns (uint256) {
        return balances[id][owner];
    }

    function uri(uint256 id) external view returns (string memory) {
        return uris[id];
    }

    function setApprovalForAll(address operator, bool approved) external {
        isApprovedForAll[msg.sender][operator] = approved;
        emit ApprovalForAll(msg.sender, operator, approved);
    }

    function safeTransferFrom(address from, address to, uint256 id, uint256 amount, bytes calldata) external {
        require(msg.sender == from || isApprovedForAll[from][msg.sender], "not the holder or an operator");
        require(to != address(0), "to nobody");
        balances[id][from] -= amount;
        balances[id][to] += amount;
        emit TransferSingle(msg.sender, from, to, id, amount);
        require(to.code.length == 0 || Receiver1155(to).onERC1155Received(msg.sender, from, id, amount, "") == Receiver1155.onERC1155Received.selector, "the receiver did not take the tokens");
    }

    function mint(address to, uint256 id, uint256 amount, string calldata uri_) external {
        require(msg.sender == gateway, "only the gateway mints");
        balances[id][to] += amount;

        if (bytes(uris[id]).length == 0) {
            uris[id] = uri_;
        }

        emit TransferSingle(msg.sender, address(0), to, id, amount);
    }

    function burn(address from, uint256 id, uint256 amount) external {
        require(msg.sender == gateway, "only the gateway burns");
        balances[id][from] -= amount;
        emit TransferSingle(msg.sender, from, address(0), id, amount);
    }

    function supportsInterface(bytes4 id) external pure returns (bool) {
        return id == 0x01ffc9a7 || id == 0xd9b67a26 || id == 0x0e89341c;
    }
}

contract Gateway {
    uint8 constant COIN = 0;
    uint8 constant TOKEN = 1;
    uint8 constant NFT = 2;
    uint8 constant MULTI = 3;

    /// A transfer as both gateways write it: contracts/lua/gateway.lua says what each field is.
    struct Transfer {
        bytes from;
        bytes to;
        uint64 source;
        uint64 target;
        uint64 nonce;
        uint8 kind;
        uint64 origin;
        bytes asset;
        uint256 id;
        uint256 amount;
        bytes recipient;
        string name;
        string symbol;
        uint8 decimals;
        string uri;
    }

    /// What a wrapped asset here stands for.
    struct Origin {
        uint64 chain;
        uint8 kind;
        bytes asset;
        string name;
        string symbol;
        uint8 decimals;
    }

    address public immutable owner;
    /// The templates wrapped assets are cloned from: a WrappedToken, a WrappedNft, a WrappedMulti.
    address public immutable tokenTemplate;
    address public immutable nftTemplate;
    address public immutable multiTemplate;
    string public coinName;
    string public coinSymbol;
    address[] public signers;
    mapping(address => bool) public isSigner;
    uint256 public threshold;
    uint64 public version;
    uint64 public nonce;
    mapping(uint64 => bytes) public peers;
    mapping(uint64 => Transfer) private sentTransfers;
    mapping(bytes32 => bool) public delivered;
    mapping(bytes32 => address) public wrappedOf;
    mapping(address => Origin) private origins;
    /// Set only while send moves ERC-1155 tokens in, so those sent any other way are refused.
    bool private receiving;

    event Sent(uint64 indexed nonce, uint64 indexed target, bytes32 digest);
    event Delivered(uint64 indexed source, uint64 indexed nonce, bytes32 digest);
    event Wrapped(uint64 origin, uint8 kind, bytes asset, address here);
    event SignersChanged(uint64 version, uint256 threshold);

    constructor(
        address[] memory signers_,
        uint256 threshold_,
        uint64 version_,
        string memory coinName_,
        string memory coinSymbol_,
        address[3] memory templates
    ) {
        owner = msg.sender;
        (tokenTemplate, nftTemplate, multiTemplate) = (templates[0], templates[1], templates[2]);
        (coinName, coinSymbol) = (coinName_, coinSymbol_);
        setSet(signers_, threshold_, version_);
    }

    /// The gateway on another chain, by its chain id: chainv4's as its identity(), its 33 bytes and
    /// when it was set up, so what left for one set up before it is not for this one.
    function setPeer(uint64 chain, bytes calldata gateway) external {
        require(msg.sender == owner, "only the owner");
        peers[chain] = gateway;
    }

    /// The next federation, signed over by the one in force: what chainv4's governance decided.
    function setSigners(address[] calldata next, uint256 nextThreshold, uint64 nextVersion, bytes[] calldata signatures) external {
        require(nextVersion > version, "an older federation");
        bytes32 digest = keccak256(abi.encode("chainv4-gateway-signers", address(this), block.chainid, nextVersion, next, nextThreshold));
        check(digest, signatures);
        setSet(next, nextThreshold, nextVersion);
    }

    function signerCount() external view returns (uint256) {
        return signers.length;
    }

    /// A transfer's digest, as the federation signs it and chainv4's gateway makes it.
    function digestOf(Transfer memory t) public pure returns (bytes32) {
        bytes32 head = keccak256(abi.encode(t.from, t.to, t.source, t.target, t.nonce, t.kind, t.origin));
        bytes32 body = keccak256(abi.encode(t.asset, t.id, t.amount, t.recipient, t.name, t.symbol, t.decimals, t.uri));

        return keccak256(abi.encode("chainv4-gateway", head, body));
    }

    /// Sends something to recipient on another chain: ETH with the call, or an ERC-20, ERC-721 or
    /// ERC-1155 amount, which this gateway moves in, so approve it, or make it an operator, first.
    /// A wrapped one goes back, burned here; a native one is held here until it comes back.
    function send(uint64 target, bytes calldata recipient, uint8 kind, address asset, uint256 id, uint256 amount) external payable returns (uint64) {
        require(peers[target].length > 0, "no gateway on that chain is this one's peer");
        require(reachable(recipient, peers[target].length), "a recipient that is no address on that chain");
        Transfer memory t;
        (t.from, t.to, t.source, t.target, t.recipient) = (abi.encodePacked(address(this)), peers[target], uint64(block.chainid), target, recipient);
        Origin storage wrapped = origins[asset];

        if (kind == COIN) {
            require(msg.value > 0 && msg.value == amount && asset == address(0), "send the coin with the call, as much as amount says");
            (t.kind, t.origin, t.amount, t.name, t.symbol, t.decimals) = (COIN, uint64(block.chainid), amount, coinName, coinSymbol, 18);
        } else if (wrapped.chain != 0) {
            require(msg.value == 0, "send no coin with a token");
            (t.kind, t.origin, t.asset, t.name, t.symbol, t.decimals) = (wrapped.kind, wrapped.chain, wrapped.asset, wrapped.name, wrapped.symbol, wrapped.decimals);
            (t.id, t.amount) = (id, wrapped.kind == NFT ? 1 : amount);
            burnBack(t, asset);
        } else {
            require(msg.value == 0, "send no coin with a token");
            (t.kind, t.origin, t.asset, t.id) = (kind, uint64(block.chainid), abi.encodePacked(asset), id);
            t.amount = kind == NFT ? 1 : amount;
            hold(t, asset);
        }

        require(t.amount > 0, "send something");
        t.nonce = ++nonce;
        sentTransfers[t.nonce] = t;
        bytes32 digest = digestOf(t);
        emit Sent(t.nonce, target, digest);

        return t.nonce;
    }

    /// A transfer that left, for the federation's signers to sign.
    function sent(uint64 which) external view returns (Transfer memory) {
        return sentTransfers[which];
    }

    /// Delivers a transfer from another chain's gateway with the federation's signatures over its
    /// digest: what anyone relays from chainv4's signature board.
    function deliver(Transfer calldata t, bytes[] calldata signatures) external {
        require(t.target == block.chainid && keccak256(t.to) == keccak256(abi.encodePacked(address(this))), "not for this gateway");
        require(peers[t.source].length > 0 && keccak256(t.from) == keccak256(peers[t.source]), "not from this gateway's peer");
        // By the gateway it came from too: a chain restarted from the identical genesis counts its
        // nonces from 1 again, under a gateway with the same address but set up anew.
        bytes32 key = keccak256(abi.encode(t.source, keccak256(t.from), t.nonce));
        require(!delivered[key], "delivered already");
        bytes32 digest = digestOf(t);
        check(digest, signatures);
        delivered[key] = true;
        require(t.recipient.length == 20, "the recipient is an address here");
        address to = address(bytes20(t.recipient));

        if (t.origin == block.chainid) {
            release(t, to);
        } else {
            mintWrapped(t, to);
        }

        emit Delivered(t.source, t.nonce, digest);
    }

    /// Takes ERC-1155 tokens only as send moves them in.
    function onERC1155Received(address, address, uint256, uint256, bytes calldata) external view returns (bytes4) {
        require(receiving, "send tokens with send");
        return this.onERC1155Received.selector;
    }

    function onERC721Received(address, address, uint256, bytes calldata) external pure returns (bytes4) {
        return this.onERC721Received.selector;
    }

    function supportsInterface(bytes4 id) external pure returns (bool) {
        return id == 0x01ffc9a7 || id == 0x4e2312e0;
    }

    /// What a wrapped asset here stands for: its chain, kind and address there.
    function originOf(address asset) external view returns (uint64, uint8, bytes memory) {
        Origin storage found = origins[asset];
        return (found.chain, found.kind, found.asset);
    }

    function hold(Transfer memory t, address asset) private {
        if (t.kind == TOKEN) {
            (t.name, t.symbol, t.decimals) = (nameOf(asset), symbolOf(asset), decimalsOf(asset));
            uint256 before = IERC20(asset).balanceOf(address(this));
            (bool done, bytes memory answer) = asset.call(abi.encodeWithSelector(IERC20.transferFrom.selector, msg.sender, address(this), t.amount));
            require(done && (answer.length == 0 || abi.decode(answer, (bool))), "the token would not move");
            require(IERC20(asset).balanceOf(address(this)) - before == t.amount, "the token moved less than the amount");
        } else if (t.kind == NFT) {
            (t.name, t.symbol, t.uri) = (nameOf(asset), symbolOf(asset), tokenUriOf(asset, t.id));
            IERC721(asset).transferFrom(msg.sender, address(this), t.id);
            require(IERC721(asset).ownerOf(t.id) == address(this), "the NFT did not move");
        } else if (t.kind == MULTI) {
            t.uri = multiUriOf(asset, t.id);
            receiving = true;
            IERC1155(asset).safeTransferFrom(msg.sender, address(this), t.id, t.amount, "");
            receiving = false;
        } else {
            revert("kind is 0 the coin, 1 a token, 2 an NFT or 3 a multi-token amount");
        }
    }

    function burnBack(Transfer memory t, address asset) private {
        if (t.kind <= TOKEN) {
            WrappedToken(asset).burn(msg.sender, t.amount);
        } else if (t.kind == NFT) {
            t.uri = WrappedNft(asset).tokenURI(t.id);
            WrappedNft(asset).burn(msg.sender, t.id);
        } else {
            t.uri = WrappedMulti(asset).uri(t.id);
            WrappedMulti(asset).burn(msg.sender, t.id, t.amount);
        }
    }

    function release(Transfer calldata t, address to) private {
        if (t.kind == COIN) {
            (bool sentCoin, ) = to.call{value: t.amount}("");
            require(sentCoin, "the coin would not go");
            return;
        }

        require(t.asset.length == 20, "an asset here is an address");
        address asset = address(bytes20(t.asset));

        if (t.kind == TOKEN) {
            (bool done, bytes memory answer) = asset.call(abi.encodeWithSelector(IERC20.transfer.selector, to, t.amount));
            require(done && (answer.length == 0 || abi.decode(answer, (bool))), "the token would not move");
        } else if (t.kind == NFT) {
            IERC721(asset).transferFrom(address(this), to, t.id);
        } else {
            IERC1155(asset).safeTransferFrom(address(this), to, t.id, t.amount, "");
        }
    }

    function mintWrapped(Transfer calldata t, address to) private {
        bytes32 salt = keccak256(abi.encode(t.origin, t.kind <= TOKEN ? TOKEN : t.kind, t.asset));
        address made = wrappedOf[salt];

        if (made == address(0)) {
            string memory name = string.concat("Wrapped ", t.name);
            string memory symbol = string.concat("w", t.symbol);

            if (t.kind <= TOKEN) {
                made = clone(tokenTemplate, salt);
                WrappedToken(made).initialize(name, symbol, t.decimals);
            } else if (t.kind == NFT) {
                made = clone(nftTemplate, salt);
                WrappedNft(made).initialize(name, symbol);
            } else {
                made = clone(multiTemplate, salt);
                WrappedMulti(made).initialize(name, symbol);
            }

            wrappedOf[salt] = made;
            origins[made] = Origin(t.origin, t.kind, t.asset, t.name, t.symbol, t.decimals);
            emit Wrapped(t.origin, t.kind, t.asset, made);
        }

        if (t.kind <= TOKEN) {
            WrappedToken(made).mint(to, t.amount);
        } else if (t.kind == NFT) {
            WrappedNft(made).mint(to, t.id, t.uri);
        } else {
            WrappedMulti(made).mint(to, t.id, t.amount, t.uri);
        }
    }

    /// Whether a recipient is an address on the chain of a peer gateway: on another EVM chain, 20
    /// bytes and not nobody; on chainv4, its 33, its kind's tag first, a shorter kind followed by
    /// zeros, as its gateway reads them, and not nobody. Anything else can never arrive, and once
    /// signed could only be refused there.
    function reachable(bytes calldata recipient, uint256 peerLength) private pure returns (bool) {
        if (peerLength == 20) {
            return recipient.length == 20 && bytes20(recipient) != bytes20(0);
        }

        if (recipient.length != 33 || uint8(recipient[0]) > 5 || bytes32(recipient[1:33]) == bytes32(0)) {
            return false;
        }

        return uint8(recipient[0]) < 3 || bytes12(recipient[21:33]) == bytes12(0);
    }

    /// A minimal proxy of a template (EIP-1167), at an address fixed by the salt.
    function clone(address template, bytes32 salt) private returns (address made) {
        bytes memory code = abi.encodePacked(hex"3d602d80600a3d3981f3363d3d373d3d3d363d73", template, hex"5af43d82803e903d91602b57fd5bf3");

        assembly {
            made := create2(0, add(code, 0x20), mload(code), salt)
        }

        require(made != address(0), "the wrapped asset was not made");
    }

    /// At least threshold signatures by distinct signers in force over a digest.
    function check(bytes32 digest, bytes[] calldata signatures) private view {
        uint256 counted = 0;
        address[] memory seen = new address[](signatures.length);

        for (uint256 i = 0; i < signatures.length; i++) {
            address signer = recover(digest, signatures[i]);

            if (signer == address(0) || !isSigner[signer]) {
                continue;
            }

            bool again = false;

            for (uint256 j = 0; j < counted; j++) {
                again = again || seen[j] == signer;
            }

            if (!again) {
                seen[counted++] = signer;
            }
        }

        require(counted >= threshold, "not enough of the federation signed");
    }

    function recover(bytes32 digest, bytes calldata signature) private pure returns (address) {
        if (signature.length != 65) {
            return address(0);
        }

        bytes32 r = bytes32(signature[0:32]);
        bytes32 s = bytes32(signature[32:64]);
        uint8 v = uint8(signature[64]);

        // Low s only, as chainv4's gateway recovers: one signature, one form.
        if (uint256(s) > 0x7fffffffffffffffffffffffffffffff5d576e7357a4501ddfe92f46681b20a0) {
            return address(0);
        }

        return ecrecover(digest, v < 27 ? v + 27 : v, r, s);
    }

    function setSet(address[] memory next, uint256 nextThreshold, uint64 nextVersion) private {
        require(next.length > 0 && nextThreshold > 0 && nextThreshold <= next.length, "a threshold of 1 to as many signers as there are");

        for (uint256 i = 0; i < signers.length; i++) {
            isSigner[signers[i]] = false;
        }

        delete signers;

        for (uint256 i = 0; i < next.length; i++) {
            require(next[i] != address(0) && !isSigner[next[i]], "each signer once");
            isSigner[next[i]] = true;
            signers.push(next[i]);
        }

        (threshold, version) = (nextThreshold, nextVersion);
        emit SignersChanged(nextVersion, nextThreshold);
    }

    /// Text as a transfer may carry it: chainv4's gateway and tools hold names and URIs as UTF-8,
    /// and one that is not could never be signed, so a token with such a name crosses without it.
    function readable(string memory text) private pure returns (string memory) {
        bytes memory found = bytes(text);
        uint256 index = 0;

        while (index < found.length) {
            uint8 first = uint8(found[index]);
            uint256 size = first < 0x80 ? 1 : first < 0xc2 ? 0 : first < 0xe0 ? 2 : first < 0xf0 ? 3 : first < 0xf5 ? 4 : 0;

            if (size == 0 || index + size > found.length) {
                return "";
            }

            for (uint256 next = 1; next < size; next++) {
                uint8 byte_ = uint8(found[index + next]);
                // The second byte has narrower bounds after these: none overlong, none a
                // surrogate, none past U+10FFFF.
                uint8 least = next == 1 && first == 0xe0 ? 0xa0 : next == 1 && first == 0xf0 ? 0x90 : 0x80;
                uint8 most = next == 1 && first == 0xed ? 0x9f : next == 1 && first == 0xf4 ? 0x8f : 0xbf;

                if (byte_ < least || byte_ > most) {
                    return "";
                }
            }

            index += size;
        }

        return text;
    }

    function nameOf(address asset) private view returns (string memory) {
        try IERC20(asset).name() returns (string memory found) {
            return readable(found);
        } catch {
            return "";
        }
    }

    function symbolOf(address asset) private view returns (string memory) {
        try IERC20(asset).symbol() returns (string memory found) {
            return readable(found);
        } catch {
            return "";
        }
    }

    function decimalsOf(address asset) private view returns (uint8) {
        try IERC20(asset).decimals() returns (uint8 found) {
            return found;
        } catch {
            return 0;
        }
    }

    function tokenUriOf(address asset, uint256 id) private view returns (string memory) {
        try IERC721(asset).tokenURI(id) returns (string memory found) {
            return readable(found);
        } catch {
            return "";
        }
    }

    function multiUriOf(address asset, uint256 id) private view returns (string memory) {
        try IERC1155(asset).uri(id) returns (string memory found) {
            return readable(found);
        } catch {
            return "";
        }
    }
}
