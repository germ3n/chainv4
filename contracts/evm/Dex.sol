// SPDX-License-Identifier: MIT
// A decentralized exchange for chainv4's EVM, in the manner of Uniswap V2, written afresh:
// constant-product pairs of ERC-20s, each pair its own share token, a fee to whoever provides, the
// chain's own as governance sets it (0.3% by default), read through the native precompile; a
// factory that opens one pair for any two tokens, and a router that adds, removes and
// trades through a path of pairs, the chain's coin wrapped as WDEV. contracts/lua/dex.lua is the
// same for native tokens. Built with solc 0.8 for Paris:
//
//   solc --evm-version paris --optimize --bin --abi Dex.sol -o build
//
// DexFactory.bin, DexRouter.bin and WDEV.bin beside it are what that writes; DexPair is created
// by the factory, from its own code. docs/dex.md walks through them.
pragma solidity ^0.8.24;

/// The chain's native precompile, which answers the DEX's fee as governance set it, in hundredths
/// of a percent: 30 is 0.3%.
interface ChainSettings {
    function dexFeeBps() external view returns (uint256);
}

address constant NATIVE_PRECOMPILE = 0x0000000000000000000000000000000000004e43;

/// The fee every trade of this chain's DEXes pays, as the chain says it now.
function dexFee() view returns (uint256) {
    return ChainSettings(NATIVE_PRECOMPILE).dexFeeBps();
}

interface IERC20 {
    function balanceOf(address owner) external view returns (uint256);
    function transfer(address to, uint256 value) external returns (bool);
    function transferFrom(address from, address to, uint256 value) external returns (bool);
}

/// The ERC-20 the shares and WDEV are.
abstract contract ERC20 {
    string public name;
    string public symbol;
    uint8 public constant decimals = 18;
    uint256 public totalSupply;
    mapping(address => uint256) public balanceOf;
    mapping(address => mapping(address => uint256)) public allowance;

    event Transfer(address indexed from, address indexed to, uint256 value);
    event Approval(address indexed owner, address indexed spender, uint256 value);

    function transfer(address to, uint256 value) external returns (bool) {
        _move(msg.sender, to, value);
        return true;
    }

    function approve(address spender, uint256 value) external returns (bool) {
        allowance[msg.sender][spender] = value;
        emit Approval(msg.sender, spender, value);
        return true;
    }

    function transferFrom(address from, address to, uint256 value) external returns (bool) {
        uint256 allowed = allowance[from][msg.sender];
        if (allowed != type(uint256).max) {
            require(allowed >= value, "not allowed that much");
            allowance[from][msg.sender] = allowed - value;
        }
        _move(from, to, value);
        return true;
    }

    function _move(address from, address to, uint256 value) internal {
        require(balanceOf[from] >= value, "not that much to move");
        balanceOf[from] -= value;
        balanceOf[to] += value;
        emit Transfer(from, to, value);
    }

    function _mint(address to, uint256 value) internal {
        totalSupply += value;
        balanceOf[to] += value;
        emit Transfer(address(0), to, value);
    }

    function _burn(address from, uint256 value) internal {
        require(balanceOf[from] >= value, "not that much to burn");
        balanceOf[from] -= value;
        totalSupply -= value;
        emit Transfer(from, address(0), value);
    }
}

/// The chain's coin as an ERC-20, one for one: deposit it to wrap, withdraw to unwrap.
contract WDEV is ERC20 {
    event Deposit(address indexed to, uint256 value);
    event Withdrawal(address indexed from, uint256 value);

    constructor() {
        (name, symbol) = ("Wrapped DEV", "WDEV");
    }

    receive() external payable {
        deposit();
    }

    function deposit() public payable {
        _mint(msg.sender, msg.value);
        emit Deposit(msg.sender, msg.value);
    }

    function withdraw(uint256 value) external {
        _burn(msg.sender, value);
        (bool sent, ) = msg.sender.call{value: value}("");
        require(sent, "the coin would not go");
        emit Withdrawal(msg.sender, value);
    }
}

/// Two tokens, held in a constant product. Low level: tokens are sent to it first, then mint,
/// burn or swap settles what arrived. The router does both in one call.
contract DexPair is ERC20 {
    uint256 public constant MINIMUM_LIQUIDITY = 1000;

    address public immutable factory;
    address public immutable token0;
    address public immutable token1;

    uint112 private reserve0;
    uint112 private reserve1;
    uint32 private blockTimestampLast;
    bool private locked;

    event Mint(address indexed sender, uint256 amount0, uint256 amount1);
    event Burn(address indexed sender, uint256 amount0, uint256 amount1, address indexed to);
    event Swap(address indexed sender, uint256 amount0In, uint256 amount1In, uint256 amount0Out, uint256 amount1Out, address indexed to);
    event Sync(uint112 reserve0, uint112 reserve1);

    modifier lock() {
        require(!locked, "the pair is busy");
        locked = true;
        _;
        locked = false;
    }

    constructor(address a, address b) {
        (factory, token0, token1) = (msg.sender, a, b);
        (name, symbol) = ("chainv4 DEX share", "DEX-LP");
    }

    function getReserves() public view returns (uint112, uint112, uint32) {
        return (reserve0, reserve1, blockTimestampLast);
    }

    function _send(address token, address to, uint256 value) private {
        (bool done, bytes memory answer) = token.call(abi.encodeWithSelector(IERC20.transfer.selector, to, value));
        require(done && (answer.length == 0 || abi.decode(answer, (bool))), "the token would not move");
    }

    function _update(uint256 balance0, uint256 balance1) private {
        require(balance0 <= type(uint112).max && balance1 <= type(uint112).max, "more than a pair holds");
        (reserve0, reserve1, blockTimestampLast) = (uint112(balance0), uint112(balance1), uint32(block.timestamp));
        emit Sync(reserve0, reserve1);
    }

    /// Mints shares for what was sent in beyond the reserves.
    function mint(address to) external lock returns (uint256 liquidity) {
        (uint112 held0, uint112 held1, ) = getReserves();
        uint256 balance0 = IERC20(token0).balanceOf(address(this));
        uint256 balance1 = IERC20(token1).balanceOf(address(this));
        uint256 amount0 = balance0 - held0;
        uint256 amount1 = balance1 - held1;

        if (totalSupply == 0) {
            liquidity = sqrt(amount0 * amount1) - MINIMUM_LIQUIDITY;
            _mint(address(0xdead), MINIMUM_LIQUIDITY);
        } else {
            liquidity = min(amount0 * totalSupply / held0, amount1 * totalSupply / held1);
        }

        require(liquidity > 0, "too little to mint a share");
        _mint(to, liquidity);
        _update(balance0, balance1);
        emit Mint(msg.sender, amount0, amount1);
    }

    /// Burns the shares sent in, and pays their part of both tokens.
    function burn(address to) external lock returns (uint256 amount0, uint256 amount1) {
        uint256 balance0 = IERC20(token0).balanceOf(address(this));
        uint256 balance1 = IERC20(token1).balanceOf(address(this));
        uint256 liquidity = balanceOf[address(this)];
        amount0 = liquidity * balance0 / totalSupply;
        amount1 = liquidity * balance1 / totalSupply;
        require(amount0 > 0 && amount1 > 0, "too few shares to take anything out");
        _burn(address(this), liquidity);
        _send(token0, to, amount0);
        _send(token1, to, amount1);
        _update(IERC20(token0).balanceOf(address(this)), IERC20(token1).balanceOf(address(this)));
        emit Burn(msg.sender, amount0, amount1, to);
    }

    /// Pays out what is asked, if what was sent in keeps the product, less the 0.3% fee.
    function swap(uint256 amount0Out, uint256 amount1Out, address to) external lock {
        require(amount0Out > 0 || amount1Out > 0, "trade more than nothing");
        (uint112 held0, uint112 held1, ) = getReserves();
        require(amount0Out < held0 && amount1Out < held1, "the pair holds too little");
        require(to != token0 && to != token1, "not to the tokens themselves");

        if (amount0Out > 0) _send(token0, to, amount0Out);
        if (amount1Out > 0) _send(token1, to, amount1Out);
        uint256 balance0 = IERC20(token0).balanceOf(address(this));
        uint256 balance1 = IERC20(token1).balanceOf(address(this));
        uint256 amount0In = balance0 > held0 - amount0Out ? balance0 - (held0 - amount0Out) : 0;
        uint256 amount1In = balance1 > held1 - amount1Out ? balance1 - (held1 - amount1Out) : 0;
        require(amount0In > 0 || amount1In > 0, "nothing came in");
        {
            // The fee in hundredths of a percent, kept in the pair: in a block of its own, as the
            // stack holds no more.
            uint256 fee = dexFee();
            require((balance0 * 10000 - amount0In * fee) * (balance1 * 10000 - amount1In * fee) >= uint256(held0) * held1 * 10000 ** 2, "that trade takes more than it gives");
        }

        _update(balance0, balance1);
        emit Swap(msg.sender, amount0In, amount1In, amount0Out, amount1Out, to);
    }

    /// Sets the reserves to what the pair holds.
    function sync() external lock {
        _update(IERC20(token0).balanceOf(address(this)), IERC20(token1).balanceOf(address(this)));
    }

    function sqrt(uint256 y) private pure returns (uint256 z) {
        if (y > 3) {
            z = y;
            uint256 x = y / 2 + 1;
            while (x < z) {
                z = x;
                x = (y / x + x) / 2;
            }
        } else if (y != 0) {
            z = 1;
        }
    }

    function min(uint256 x, uint256 y) private pure returns (uint256) {
        return x < y ? x : y;
    }
}

/// Opens one pair for any two tokens, at an address CREATE2 fixes by them.
contract DexFactory {
    mapping(address => mapping(address => address)) public getPair;
    address[] public allPairs;

    event PairCreated(address indexed token0, address indexed token1, address pair, uint256 count);

    function allPairsLength() external view returns (uint256) {
        return allPairs.length;
    }

    function createPair(address a, address b) external returns (address pair) {
        require(a != b, "a pair trades two different tokens");
        (address token0, address token1) = a < b ? (a, b) : (b, a);
        require(token0 != address(0), "the zero address is no token");
        require(getPair[token0][token1] == address(0), "that pair exists");
        pair = address(new DexPair{salt: keccak256(abi.encodePacked(token0, token1))}(token0, token1));
        getPair[token0][token1] = pair;
        getPair[token1][token0] = pair;
        allPairs.push(pair);
        emit PairCreated(token0, token1, pair, allPairs.length);
    }
}

/// Adds, removes and trades through the factory's pairs in one call each, the coin wrapped as
/// WDEV on the way in and unwrapped on the way out. Deadlines are Unix seconds.
contract DexRouter {
    DexFactory public immutable factory;
    WDEV public immutable wdev;

    modifier before(uint256 deadline) {
        require(block.timestamp < deadline, "its deadline passed");
        _;
    }

    constructor(DexFactory factory_, WDEV wdev_) {
        (factory, wdev) = (factory_, wdev_);
    }

    receive() external payable {
        require(msg.sender == address(wdev), "send the coin through a function");
    }

    // What a trade of amountIn pays out of a pair.
    function getAmountOut(uint256 amountIn, uint256 reserveIn, uint256 reserveOut) public view returns (uint256) {
        require(amountIn > 0, "trade more than nothing");
        require(reserveIn > 0 && reserveOut > 0, "the pair holds nothing to trade");
        uint256 withFee = amountIn * (10000 - dexFee());
        return withFee * reserveOut / (reserveIn * 10000 + withFee);
    }

    function getReserves(address a, address b) public view returns (uint256 reserveA, uint256 reserveB) {
        address pair = factory.getPair(a, b);
        require(pair != address(0), "no such pair");
        (uint112 held0, uint112 held1, ) = DexPair(pair).getReserves();
        (reserveA, reserveB) = a < b ? (held0, held1) : (held1, held0);
    }

    /// What each step of a trade along path pays out, the first being amountIn.
    function getAmountsOut(uint256 amountIn, address[] memory path) public view returns (uint256[] memory amounts) {
        require(path.length >= 2, "a path has two tokens or more");
        amounts = new uint256[](path.length);
        amounts[0] = amountIn;
        for (uint256 i; i < path.length - 1; i++) {
            (uint256 reserveIn, uint256 reserveOut) = getReserves(path[i], path[i + 1]);
            amounts[i + 1] = getAmountOut(amounts[i], reserveIn, reserveOut);
        }
    }

    function _pull(address token, address from, address to, uint256 value) private {
        (bool done, bytes memory answer) = token.call(abi.encodeWithSelector(IERC20.transferFrom.selector, from, to, value));
        require(done && (answer.length == 0 || abi.decode(answer, (bool))), "the token would not move");
    }

    function _amounts(address a, address b, uint256 desiredA, uint256 desiredB, uint256 leastA, uint256 leastB) private returns (uint256, uint256, address) {
        address pair = factory.getPair(a, b);
        if (pair == address(0)) pair = factory.createPair(a, b);
        (uint112 held0, uint112 held1, ) = DexPair(pair).getReserves();
        (uint256 reserveA, uint256 reserveB) = a < b ? (uint256(held0), uint256(held1)) : (uint256(held1), uint256(held0));
        if (reserveA == 0 && reserveB == 0) return (desiredA, desiredB, pair);
        uint256 optimalB = desiredA * reserveB / reserveA;
        if (optimalB <= desiredB) {
            require(optimalB >= leastB, "the pair's price moved: less b than the least");
            return (desiredA, optimalB, pair);
        }
        uint256 optimalA = desiredB * reserveA / reserveB;
        require(optimalA <= desiredA && optimalA >= leastA, "the pair's price moved: less a than the least");
        return (optimalA, desiredB, pair);
    }

    /// Adds two tokens to their pair at its price, opening it at the price given if it is new.
    function addLiquidity(address a, address b, uint256 desiredA, uint256 desiredB, uint256 leastA, uint256 leastB, address to, uint256 deadline)
        external
        before(deadline)
        returns (uint256 amountA, uint256 amountB, uint256 liquidity)
    {
        address pair;
        (amountA, amountB, pair) = _amounts(a, b, desiredA, desiredB, leastA, leastB);
        _pull(a, msg.sender, pair, amountA);
        _pull(b, msg.sender, pair, amountB);
        liquidity = DexPair(pair).mint(to);
    }

    /// Adds a token and the coin sent, wrapped; what is sent beyond the pair's price comes back.
    function addLiquidityDEV(address token, uint256 desiredToken, uint256 leastToken, uint256 leastDEV, address to, uint256 deadline)
        external
        payable
        before(deadline)
        returns (uint256 amountToken, uint256 amountDEV, uint256 liquidity)
    {
        address pair;
        (amountToken, amountDEV, pair) = _amounts(token, address(wdev), desiredToken, msg.value, leastToken, leastDEV);
        _pull(token, msg.sender, pair, amountToken);
        wdev.deposit{value: amountDEV}();
        require(wdev.transfer(pair, amountDEV), "WDEV would not move");
        liquidity = DexPair(pair).mint(to);
        if (msg.value > amountDEV) {
            (bool sent, ) = msg.sender.call{value: msg.value - amountDEV}("");
            require(sent, "the coin would not go back");
        }
    }

    /// Takes shares out of a pair: they are pulled from the caller, so approve the router for them.
    function removeLiquidity(address a, address b, uint256 liquidity, uint256 leastA, uint256 leastB, address to, uint256 deadline)
        public
        before(deadline)
        returns (uint256 amountA, uint256 amountB)
    {
        address pair = factory.getPair(a, b);
        require(pair != address(0), "no such pair");
        _pull(pair, msg.sender, pair, liquidity);
        (uint256 amount0, uint256 amount1) = DexPair(pair).burn(to);
        (amountA, amountB) = a < b ? (amount0, amount1) : (amount1, amount0);
        require(amountA >= leastA && amountB >= leastB, "the pair's price moved: less than the least");
    }

    /// Takes shares out of a token's pair with the coin, the coin unwrapped.
    function removeLiquidityDEV(address token, uint256 liquidity, uint256 leastToken, uint256 leastDEV, address to, uint256 deadline)
        external
        before(deadline)
        returns (uint256 amountToken, uint256 amountDEV)
    {
        (amountToken, amountDEV) = removeLiquidity(token, address(wdev), liquidity, leastToken, leastDEV, address(this), deadline);
        require(IERC20(token).transfer(to, amountToken), "the token would not move");
        wdev.withdraw(amountDEV);
        (bool sent, ) = to.call{value: amountDEV}("");
        require(sent, "the coin would not go");
    }

    // Trades what the first pair of path already holds beyond its reserves along path, to `to`.
    function _swap(uint256[] memory amounts, address[] memory path, address to) private {
        for (uint256 i; i < path.length - 1; i++) {
            (address input, address output) = (path[i], path[i + 1]);
            uint256 out = amounts[i + 1];
            (uint256 out0, uint256 out1) = input < output ? (uint256(0), out) : (out, uint256(0));
            address next = i < path.length - 2 ? factory.getPair(output, path[i + 2]) : to;
            DexPair(factory.getPair(input, output)).swap(out0, out1, next);
        }
    }

    /// Trades exactly amountIn of path's first token for at least leastOut of its last.
    function swapExactTokensForTokens(uint256 amountIn, uint256 leastOut, address[] calldata path, address to, uint256 deadline)
        external
        before(deadline)
        returns (uint256[] memory amounts)
    {
        amounts = getAmountsOut(amountIn, path);
        require(amounts[amounts.length - 1] >= leastOut, "the price moved: less out than the least");
        _pull(path[0], msg.sender, factory.getPair(path[0], path[1]), amountIn);
        _swap(amounts, path, to);
    }

    /// Trades the coin sent, wrapped, along a path starting at WDEV.
    function swapExactDEVForTokens(uint256 leastOut, address[] calldata path, address to, uint256 deadline)
        external
        payable
        before(deadline)
        returns (uint256[] memory amounts)
    {
        require(path[0] == address(wdev), "a path for the coin starts at WDEV");
        amounts = getAmountsOut(msg.value, path);
        require(amounts[amounts.length - 1] >= leastOut, "the price moved: less out than the least");
        wdev.deposit{value: msg.value}();
        require(wdev.transfer(factory.getPair(path[0], path[1]), msg.value), "WDEV would not move");
        _swap(amounts, path, to);
    }

    /// Trades exactly amountIn of a token along a path ending at WDEV, for the coin, unwrapped.
    function swapExactTokensForDEV(uint256 amountIn, uint256 leastOut, address[] calldata path, address to, uint256 deadline)
        external
        before(deadline)
        returns (uint256[] memory amounts)
    {
        require(path[path.length - 1] == address(wdev), "a path to the coin ends at WDEV");
        amounts = getAmountsOut(amountIn, path);
        uint256 out = amounts[amounts.length - 1];
        require(out >= leastOut, "the price moved: less out than the least");
        _pull(path[0], msg.sender, factory.getPair(path[0], path[1]), amountIn);
        _swap(amounts, path, address(this));
        wdev.withdraw(out);
        (bool sent, ) = to.call{value: out}("");
        require(sent, "the coin would not go");
    }
}
