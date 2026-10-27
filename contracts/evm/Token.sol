// SPDX-License-Identifier: MIT
// An example ERC-20 token for chainv4's EVM: its whole supply to whoever deploys it, moved with
// transfer, or by someone approved with transferFrom. The swap tests lock it in Swap.sol. Built
// with solc 0.8 for Paris:
//
//   solc --evm-version paris --optimize --bin --abi Token.sol -o build
pragma solidity ^0.8.24;

contract Token {
    string public name;
    string public symbol;
    uint8 public constant decimals = 18;
    uint256 public totalSupply;
    mapping(address => uint256) public balanceOf;
    mapping(address => mapping(address => uint256)) public allowance;

    event Transfer(address indexed from, address indexed to, uint256 value);
    event Approval(address indexed owner, address indexed spender, uint256 value);

    constructor(string memory name_, string memory symbol_, uint256 supply) {
        (name, symbol, totalSupply) = (name_, symbol_, supply);
        balanceOf[msg.sender] = supply;
        emit Transfer(address(0), msg.sender, supply);
    }

    function transfer(address to, uint256 value) external returns (bool) {
        move(msg.sender, to, value);
        return true;
    }

    function approve(address spender, uint256 value) external returns (bool) {
        allowance[msg.sender][spender] = value;
        emit Approval(msg.sender, spender, value);
        return true;
    }

    function transferFrom(address from, address to, uint256 value) external returns (bool) {
        require(allowance[from][msg.sender] >= value, "not allowed that much");
        allowance[from][msg.sender] -= value;
        move(from, to, value);
        return true;
    }

    function move(address from, address to, uint256 value) private {
        require(balanceOf[from] >= value, "not that much to move");
        balanceOf[from] -= value;
        balanceOf[to] += value;
        emit Transfer(from, to, value);
    }
}
