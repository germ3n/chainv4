//! The DEX, contracts/lua/dex.lua, through the whole chain: pools of the coin, a token-standard
//! token and an ERC-20 on the EVM, each pool's share token a clone of contracts/lua/lp_token.lua,
//! trades through one pool and two, and shares added and taken out.

use {
    super::*,
    crate::abi::{AbiType, AbiValue, encode_values},
    crate::crypto::secp256k1::keccak,
    crate::vm::evm,
};

const COIN: Address = Address::Contract([0; 32]);

fn deploy(chain: &mut Chain, code: Code) -> Address {
    let receipt = advance(chain, vec![SignedTx::sign(Tx::Deploy { code }, nonce(chain, &authority()), FeeOffer::NONE, &authority())]).remove(0);

    match receipt.result {
        Ok(Effect::Deployed(address)) => address,
        other => panic!("expected a deploy, got {other:?}"),
    }
}

/// Calls a native contract as the authority, sending coin, and returns its receipt.
fn call(chain: &mut Chain, to: Address, function: &str, coin: u128, data: Vec<TxCallArgumentType>) -> Receipt {
    let tx = Tx::ContractCall { to, func: Hash::Xxh3_64(crate::vm::selector(function)), amount: Amount::U128(coin), data };

    advance(chain, vec![SignedTx::sign(tx, nonce(chain, &authority()), FeeOffer::NONE, &authority())]).remove(0)
}

fn returned(receipt: &Receipt) -> Vec<TxCallArgumentType> {
    match &receipt.result {
        Ok(Effect::Returned(values)) => values.clone(),
        other => panic!("expected a return, got {other:?}"),
    }
}

fn refused(receipt: &Receipt, why: &str) {
    assert!(format!("{:?}", receipt.result).contains(why), "{:?}", receipt.result);
}

fn evm_call(chain: &mut Chain, to: Address, signature: &str, args: &[AbiValue], types: &[AbiType]) -> Vec<u8> {
    let input = [keccak(signature.as_bytes())[..4].to_vec(), encode_values(types, args).unwrap()].concat();
    let tx = Tx::EvmCall { to, value: Amount::U128(0), input };
    let receipt = advance(chain, vec![SignedTx::sign(tx, nonce(chain, &authority()), FeeOffer::NONE, &authority())]).remove(0);

    match receipt.result {
        Ok(Effect::Returned(values)) => match values.as_slice() {
            [TxCallArgumentType::Bytes(bytes)] => bytes.clone(),
            other => panic!("{other:?}"),
        },
        other => panic!("expected a return, got {other:?}"),
    }
}

fn erc20_balance(chain: &mut Chain, token: Address, owner: &Address) -> u256 {
    let output = evm_call(chain, token, "balanceOf(address)", &[AbiValue::Address(evm::evm_alias(owner))], &[AbiType::Address]);

    u256::from_be_bytes(output.try_into().unwrap())
}

fn wide(value: u128) -> TxCallArgumentType {
    TxCallArgumentType::U256(u256::new(value))
}

fn at(address: Address) -> TxCallArgumentType {
    TxCallArgumentType::Address(address)
}

/// What a trade pays out of a pool, as Uniswap V2 reckons it.
fn out(amount_in: u128, reserve_in: u128, reserve_out: u128) -> u128 {
    amount_in * 997 * reserve_out / (reserve_in * 1000 + amount_in * 997)
}

#[test]
fn pools_of_the_coin_a_token_and_an_erc20_trade_and_pay_their_shares_back() {
    let db = Arc::new(MemoryDb::default());
    let mut chain = open(&db);
    let me = authority().public().address();
    let token = deploy(&mut chain, Code::Lua(include_str!("../../../contracts/lua/token.lua").to_string()));
    let template = deploy(&mut chain, Code::Lua(include_str!("../../../contracts/lua/lp_token.lua").to_string()));
    let dex = deploy(&mut chain, Code::Lua(include_str!("../../../contracts/lua/dex.lua").to_string()));
    let supply = encode_values(&[AbiType::String, AbiType::String, AbiType::Uint(256)], &[AbiValue::String("Test".into()), AbiValue::String("TST".into()), AbiValue::Uint(u256::new(10u128.pow(24)))]).unwrap();
    let erc20 = deploy(&mut chain, Code::Evm([crate::structs::hex::from_hex(include_str!("../../../contracts/evm/Token.bin").trim()).unwrap(), supply].concat()));

    // Set up once, by its deployer; nothing opens before.
    refused(&call(&mut chain, dex, "create_pool", 0, vec![at(COIN), at(token)]), "not set up");
    assert!(call(&mut chain, dex, "setup", 0, vec![at(template)]).result.is_ok());
    refused(&call(&mut chain, dex, "setup", 0, vec![at(template)]), "set up already");

    // Approve the DEX: the token by its address, the ERC-20 at the DEX's EVM address.
    assert!(call(&mut chain, token, "approve", 0, vec![at(dex), wide(u128::MAX)]).result.is_ok());
    evm_call(&mut chain, erc20, "approve(address,uint256)", &[AbiValue::Address(evm::evm_alias(&dex)), AbiValue::Uint(u256::MAX)], &[AbiType::Address, AbiType::Uint(256)]);

    // A pool of the coin and the token, opened by its first deposit at the price it sets; what
    // the call sent beyond what went in comes back.
    let before = balance(&chain, &me);
    let added = call(&mut chain, dex, "add_liquidity", 12_000_000, vec![at(COIN), at(token), wide(10_000_000), wide(20_000_000), wide(0), wide(0)]);
    let minted = 14_142_135 - 1_000;
    assert_eq!(returned(&added), [wide(10_000_000), wide(20_000_000), wide(minted)]);
    assert_eq!(before - balance(&chain, &me), u256::new(10_000_000), "the extra 2,000,000 sent back");
    let [_, _, TxCallArgumentType::Address(lp)] = <[_; 3]>::try_from(returned(&call(&mut chain, dex, "pool", 0, vec![at(token), at(COIN)]))).unwrap() else { panic!() };
    assert_eq!(returned(&call(&mut chain, lp, "balance_of", 0, vec![at(me)])), [wide(minted)]);
    assert_eq!(returned(&call(&mut chain, lp, "balance_of", 0, vec![at(dex)])), [wide(1_000)], "the first thousand never come out");
    assert_eq!(crate::registry::Tokens::new(&chain.state()).get(&lp).unwrap().and_then(|token| token.symbol), Some("DEX-LP".to_string()), "the share is a token in the registry");
    refused(&call(&mut chain, lp, "mint", 0, vec![at(me), wide(1)]), "only the pool");
    assert!(added.events.iter().any(|event| event.name == "Sync"));

    // Coin for token, the pool's price, its fee kept in the pool.
    let expected = out(100_000, 10_000_000, 20_000_000);
    let traded = call(&mut chain, dex, "swap", 100_000, vec![at(COIN), at(token), wide(100_000), wide(expected)]);
    assert_eq!(returned(&traded), [wide(expected)]);
    let sync = traded.events.iter().find(|event| event.name == "Sync").unwrap();
    assert_eq!(sync.data[2..], [wide(10_100_000), wide(20_000_000 - expected)], "the pool after, in its order: coin first");
    refused(&call(&mut chain, dex, "swap", 100_000, vec![at(COIN), at(token), wide(100_000), wide(10u128.pow(9))]), "the price moved");
    refused(&call(&mut chain, dex, "swap", 0, vec![at(COIN), at(token), wide(1), wide(0)]), "send the coin");
    refused(&call(&mut chain, dex, "swap", 0, vec![at(token), at(COIN), wide(1_000), wide(0), TxCallArgumentType::I64(1)]), "deadline passed");

    // A second pool, the token against the ERC-20, and a trade through both: coin, token, ERC-20.
    assert!(call(&mut chain, dex, "add_liquidity", 0, vec![at(token), at(erc20), wide(5_000_000), wide(5_000_000), wide(0), wide(0)]).result.is_ok());
    let reserves = returned(&call(&mut chain, dex, "pool", 0, vec![at(COIN), at(token)]));
    let [TxCallArgumentType::U256(coin_held), TxCallArgumentType::U256(token_held), _] = reserves.as_slice() else { panic!("{reserves:?}") };
    let middle = out(50_000, u128::try_from(*coin_held).unwrap(), u128::try_from(*token_held).unwrap());
    let expected = out(middle, 5_000_000, 5_000_000);
    assert_eq!(returned(&call(&mut chain, dex, "quote", 0, vec![at(COIN), at(erc20), wide(50_000), at(token)])), [wide(expected)]);
    let held = erc20_balance(&mut chain, erc20, &me);
    assert_eq!(returned(&call(&mut chain, dex, "swap", 50_000, vec![at(COIN), at(erc20), wide(50_000), wide(expected), TxCallArgumentType::Nil, at(token)])), [wide(expected)]);
    assert_eq!(erc20_balance(&mut chain, erc20, &me) - held, u256::new(expected));
    assert_eq!(returned(&call(&mut chain, dex, "pool_count", 0, vec![])), [TxCallArgumentType::I64(2)]);

    // Half the shares out: half of what the pool holds, the shares burned.
    let reserves = returned(&call(&mut chain, dex, "pool", 0, vec![at(COIN), at(token)]));
    let [TxCallArgumentType::U256(coin_held), TxCallArgumentType::U256(token_held), _] = reserves.as_slice() else { panic!("{reserves:?}") };
    let (coin_held, token_held): (u128, u128) = (u128::try_from(*coin_held).unwrap(), u128::try_from(*token_held).unwrap());
    let half = minted / 2;
    let before = balance(&chain, &me);
    let removed = call(&mut chain, dex, "remove_liquidity", 0, vec![at(COIN), at(token), wide(half), wide(0), wide(0)]);
    let (coin_back, token_back) = (half * coin_held / (minted + 1_000), half * token_held / (minted + 1_000));
    assert_eq!(returned(&removed), [wide(coin_back), wide(token_back)]);
    assert_eq!(balance(&chain, &me) - before, u256::new(coin_back));
    assert_eq!(returned(&call(&mut chain, lp, "total_supply", 0, vec![])), [wide(minted + 1_000 - half)]);
    refused(&call(&mut chain, dex, "remove_liquidity", 0, vec![at(COIN), at(token), wide(minted), wide(0), wide(0)]), "insufficient balance");

    // What is not a pool is refused.
    refused(&call(&mut chain, dex, "create_pool", 0, vec![at(token), at(token)]), "two different");
    refused(&call(&mut chain, dex, "create_pool", 0, vec![at(token), at(COIN)]), "exists");
    refused(&call(&mut chain, dex, "swap", 0, vec![at(erc20), at(COIN), wide(1_000), wide(0)]), "no pool trades");

    assert_eq!(crate::registry::supply::adds_up(&chain.state()).unwrap(), Ok(()), "minted less burned is what is held and bonded");
}

/// Calls an EVM contract as the authority, sending coin: what it returned, or why it reverted.
fn evm_send(chain: &mut Chain, to: Address, value: u128, signature: &str, types: &[AbiType], args: &[AbiValue]) -> Result<Vec<u8>, String> {
    // No signature: a bare payment.
    let input = match signature {
        "" => Vec::new(),
        _ => [keccak(signature.as_bytes())[..4].to_vec(), encode_values(types, args).unwrap()].concat(),
    };
    let tx = Tx::EvmCall { to, value: Amount::U128(value), input };
    let receipt = advance(chain, vec![SignedTx::sign(tx, nonce(chain, &authority()), FeeOffer::NONE, &authority())]).remove(0);

    match receipt.result {
        Ok(Effect::Returned(values)) => match values.as_slice() {
            [TxCallArgumentType::Bytes(bytes)] => Ok(bytes.clone()),
            other => panic!("{other:?}"),
        },
        Err(failure) => Err(failure.to_string()),
        other => panic!("{other:?}"),
    }
}

fn create(chain: &mut Chain, bin: &str, constructor: Vec<u8>) -> Address {
    deploy(chain, Code::Evm([crate::structs::hex::from_hex(bin.trim()).unwrap(), constructor].concat()))
}

fn evm_address(address: &Address) -> AbiValue {
    AbiValue::Address(evm::evm_alias(address))
}

fn words(output: &[u8]) -> Vec<u256> {
    output.chunks(32).map(|word| u256::from_be_bytes(word.try_into().unwrap())).collect()
}

#[test]
fn the_evm_dex_wraps_the_coin_trades_along_paths_and_pays_shares_back() {
    use AbiType::{Address as Addr, Array, Uint};

    let db = Arc::new(MemoryDb::default());
    let mut chain = open(&db);
    let me = authority().public().address();
    let wdev = create(&mut chain, include_str!("../../../contracts/evm/WDEV.bin"), Vec::new());
    let factory = create(&mut chain, include_str!("../../../contracts/evm/DexFactory.bin"), Vec::new());
    let router = create(&mut chain, include_str!("../../../contracts/evm/DexRouter.bin"), encode_values(&[Addr, Addr], &[evm_address(&factory), evm_address(&wdev)]).unwrap());
    let supply = |symbol: &str| encode_values(&[AbiType::String, AbiType::String, Uint(256)], &[AbiValue::String(symbol.into()), AbiValue::String(symbol.into()), AbiValue::Uint(u256::new(10u128.pow(24)))]).unwrap();
    let (a, b) = (create(&mut chain, include_str!("../../../contracts/evm/Token.bin"), supply("A")), create(&mut chain, include_str!("../../../contracts/evm/Token.bin"), supply("B")));
    let deadline = AbiValue::Uint(u256::from(chain.tip().timestamp / 1_000 + 3_600));
    let n = |value: u128| AbiValue::Uint(u256::new(value));
    let path = |tokens: &[&Address]| AbiValue::Array(tokens.iter().map(|token| evm_address(token)).collect());
    let addresses = Array(Box::new(Addr));

    for token in [a, b] {
        evm_send(&mut chain, token, 0, "approve(address,uint256)", &[Addr, Uint(256)], &[evm_address(&router), AbiValue::Uint(u256::MAX)]).unwrap();
    }

    // A with the coin: the coin wrapped on the way in, what is sent past the price sent back.
    let added = evm_send(&mut chain, router, 3_000_000, "addLiquidityDEV(address,uint256,uint256,uint256,address,uint256)", &[Addr, Uint(256), Uint(256), Uint(256), Addr, Uint(256)], &[evm_address(&a), n(4_000_000), n(0), n(0), evm_address(&me), deadline.clone()]).unwrap();
    assert_eq!(words(&added), [u256::new(4_000_000), u256::new(3_000_000), u256::new(3_464_101 - 1_000)], "sqrt(4e6 * 3e6), less the thousand locked");
    let pair = words(&evm_send(&mut chain, factory, 0, "getPair(address,address)", &[Addr, Addr], &[evm_address(&a), evm_address(&wdev)]).unwrap())[0];
    assert_ne!(pair, u256::ZERO);

    // The coin for A, at the pair's price.
    let quoted = words(&evm_send(&mut chain, router, 0, "getAmountsOut(uint256,address[])", &[Uint(256), addresses.clone()], &[n(100_000), path(&[&wdev, &a])]).unwrap());
    let expected = out(100_000, 3_000_000, 4_000_000);
    assert_eq!(quoted[quoted.len() - 1], u256::new(expected));
    let held = erc20_balance(&mut chain, a, &me);
    evm_send(&mut chain, router, 100_000, "swapExactDEVForTokens(uint256,address[],address,uint256)", &[Uint(256), addresses.clone(), Addr, Uint(256)], &[n(expected), path(&[&wdev, &a]), evm_address(&me), deadline.clone()]).unwrap();
    assert_eq!(erc20_balance(&mut chain, a, &me) - held, u256::new(expected));

    // B with A, then B for the coin along B, A, WDEV, unwrapped to the caller.
    evm_send(&mut chain, router, 0, "addLiquidity(address,address,uint256,uint256,uint256,uint256,address,uint256)", &[Addr, Addr, Uint(256), Uint(256), Uint(256), Uint(256), Addr, Uint(256)], &[evm_address(&b), evm_address(&a), n(2_000_000), n(2_000_000), n(0), n(0), evm_address(&me), deadline.clone()]).unwrap();
    let amounts = words(&evm_send(&mut chain, router, 0, "getAmountsOut(uint256,address[])", &[Uint(256), addresses.clone()], &[n(50_000), path(&[&b, &a, &wdev])]).unwrap());
    let coin_out = amounts[amounts.len() - 1];
    let before = balance(&chain, &me);
    evm_send(&mut chain, router, 0, "swapExactTokensForDEV(uint256,uint256,address[],address,uint256)", &[Uint(256), Uint(256), addresses.clone(), Addr, Uint(256)], &[n(50_000), AbiValue::Uint(coin_out), path(&[&b, &a, &wdev]), evm_address(&me), deadline.clone()]).unwrap();
    assert_eq!(balance(&chain, &me) - before, coin_out, "the coin, unwrapped");

    // Shares out: approved to the router, burned, and both sides paid, the coin unwrapped.
    let pair = Address::Evm(pair.to_be_bytes()[12..].try_into().unwrap());
    let shares = erc20_balance(&mut chain, pair, &me);
    evm_send(&mut chain, pair, 0, "approve(address,uint256)", &[Addr, Uint(256)], &[evm_address(&router), AbiValue::Uint(shares)]).unwrap();
    let before = balance(&chain, &me);
    let removed = words(&evm_send(&mut chain, router, 0, "removeLiquidityDEV(address,uint256,uint256,uint256,address,uint256)", &[Addr, Uint(256), Uint(256), Uint(256), Addr, Uint(256)], &[evm_address(&a), AbiValue::Uint(shares), n(0), n(0), evm_address(&me), deadline.clone()]).unwrap());
    assert_eq!(balance(&chain, &me) - before, removed[1]);
    assert_eq!(erc20_balance(&mut chain, pair, &me), u256::ZERO);

    // Refused: a price that moved, a deadline passed, the coin sent straight to the router.
    let moved = evm_send(&mut chain, router, 0, "swapExactTokensForTokens(uint256,uint256,address[],address,uint256)", &[Uint(256), Uint(256), addresses.clone(), Addr, Uint(256)], &[n(1_000), n(10u128.pow(12)), path(&[&b, &a]), evm_address(&me), deadline.clone()]);
    assert!(moved.unwrap_err().contains("the price moved"));
    let late = evm_send(&mut chain, router, 0, "swapExactTokensForTokens(uint256,uint256,address[],address,uint256)", &[Uint(256), Uint(256), addresses, Addr, Uint(256)], &[n(1_000), n(0), path(&[&b, &a]), evm_address(&me), n(1)]);
    assert!(late.unwrap_err().contains("deadline passed"));
    assert!(evm_send(&mut chain, router, 5, "", &[], &[]).is_err(), "the router takes the coin from WDEV alone");

    assert_eq!(crate::registry::supply::adds_up(&chain.state()).unwrap(), Ok(()), "minted less burned is what is held and bonded");
}
