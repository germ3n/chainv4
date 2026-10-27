// SPDX-License-Identifier: MIT
// Example NFTs for chainv4's EVM: Collectible, an ERC-721 collection, and Items, an ERC-1155 one,
// each minting only to whoever deployed it. The swap tests lock them in Swap.sol. Built with
// solc 0.8 for Paris:
//
//   solc --evm-version paris --optimize --bin --abi Collectibles.sol -o build
pragma solidity ^0.8.24;

interface ERC721Receiver {
    function onERC721Received(address operator, address from, uint256 id, bytes calldata data) external returns (bytes4);
}

interface ERC1155Receiver {
    function onERC1155Received(address operator, address from, uint256 id, uint256 amount, bytes calldata data) external returns (bytes4);
    function onERC1155BatchReceived(address operator, address from, uint256[] calldata ids, uint256[] calldata amounts, bytes calldata data) external returns (bytes4);
}

contract Collectible {
    string public name;
    string public symbol;
    address public immutable minter;
    uint256 public totalSupply;
    mapping(uint256 => address) public ownerOf;
    mapping(address => uint256) public balanceOf;
    mapping(uint256 => address) public getApproved;
    mapping(address => mapping(address => bool)) public isApprovedForAll;

    event Transfer(address indexed from, address indexed to, uint256 indexed id);
    event Approval(address indexed owner, address indexed approved, uint256 indexed id);
    event ApprovalForAll(address indexed owner, address indexed operator, bool approved);

    constructor(string memory name_, string memory symbol_) {
        name = name_;
        symbol = symbol_;
        minter = msg.sender;
    }

    /// Mints the next id, from 1, to whoever the deployer likes.
    function mint(address to) external returns (uint256 id) {
        require(msg.sender == minter, "only the deployer mints");
        require(to != address(0), "mint to the zero address");
        id = ++totalSupply;
        ownerOf[id] = to;
        balanceOf[to] += 1;
        emit Transfer(address(0), to, id);
    }

    function tokenURI(uint256 id) external view returns (string memory) {
        require(ownerOf[id] != address(0), "no such token");
        return string.concat("https://example.com/", toString(id));
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
        require(ownerOf[id] == from, "from does not own the token");
        require(to != address(0), "transfer to the zero address");
        require(msg.sender == from || getApproved[id] == msg.sender || isApprovedForAll[from][msg.sender], "not the owner, approved or an operator");
        delete getApproved[id];
        ownerOf[id] = to;
        balanceOf[from] -= 1;
        balanceOf[to] += 1;
        emit Transfer(from, to, id);
    }

    function safeTransferFrom(address from, address to, uint256 id) external {
        safeTransferFrom(from, to, id, "");
    }

    function safeTransferFrom(address from, address to, uint256 id, bytes memory data) public {
        transferFrom(from, to, id);
        require(
            to.code.length == 0 || ERC721Receiver(to).onERC721Received(msg.sender, from, id, data) == ERC721Receiver.onERC721Received.selector,
            "the receiver did not take the token"
        );
    }

    function supportsInterface(bytes4 id) external pure returns (bool) {
        return id == 0x01ffc9a7 || id == 0x80ac58cd || id == 0x5b5e139f;
    }
}

contract Items {
    address public immutable minter;
    mapping(uint256 => mapping(address => uint256)) private balances;
    mapping(address => mapping(address => bool)) public isApprovedForAll;

    event TransferSingle(address indexed operator, address indexed from, address indexed to, uint256 id, uint256 value);
    event TransferBatch(address indexed operator, address indexed from, address indexed to, uint256[] ids, uint256[] values);
    event ApprovalForAll(address indexed owner, address indexed operator, bool approved);

    constructor() {
        minter = msg.sender;
    }

    function balanceOf(address owner, uint256 id) external view returns (uint256) {
        return balances[id][owner];
    }

    function uri(uint256) external pure returns (string memory) {
        return "https://example.com/{id}.json";
    }

    /// Mints an amount of an id to whoever the deployer likes.
    function mint(address to, uint256 id, uint256 amount) external {
        require(msg.sender == minter, "only the deployer mints");
        balances[id][to] += amount;
        emit TransferSingle(msg.sender, address(0), to, id, amount);
        received(address(0), to, id, amount);
    }

    function setApprovalForAll(address operator, bool approved) external {
        isApprovedForAll[msg.sender][operator] = approved;
        emit ApprovalForAll(msg.sender, operator, approved);
    }

    function safeTransferFrom(address from, address to, uint256 id, uint256 amount, bytes calldata) external {
        require(msg.sender == from || isApprovedForAll[from][msg.sender], "not the holder or an operator");
        require(to != address(0), "transfer to the zero address");
        require(balances[id][from] >= amount, "insufficient balance");
        balances[id][from] -= amount;
        balances[id][to] += amount;
        emit TransferSingle(msg.sender, from, to, id, amount);
        received(from, to, id, amount);
    }

    function safeBatchTransferFrom(address, address, uint256[] calldata, uint256[] calldata, bytes calldata) external pure {
        revert("this example moves one id at a time");
    }

    function supportsInterface(bytes4 id) external pure returns (bool) {
        return id == 0x01ffc9a7 || id == 0xd9b67a26 || id == 0x0e89341c;
    }

    function received(address from, address to, uint256 id, uint256 amount) private {
        require(
            to.code.length == 0 || ERC1155Receiver(to).onERC1155Received(msg.sender, from, id, amount, "") == ERC1155Receiver.onERC1155Received.selector,
            "the receiver did not take the tokens"
        );
    }
}

function toString(uint256 value) pure returns (string memory text) {
    if (value == 0) {
        return "0";
    }

    while (value > 0) {
        text = string.concat(string(abi.encodePacked(bytes1(uint8(48 + value % 10)))), text);
        value /= 10;
    }
}
