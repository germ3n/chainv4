//! The DEXes through the whole chain. contracts/lua/dex.lua: pools of the coin, a token-standard
//! token and an ERC-20 on the EVM, at fee tiers, each pool's share token a clone of
//! contracts/lua/lp_token.lua, trades through one pool and two, shares added and taken out, the
//! protocol's share of fees burned, cumulative prices and flash swaps. Dex.sol the same on the EVM,
//! and contracts/lua/router.lua dividing one trade between both.

use {
    super::*,
    crate::abi::{AbiType, AbiValue, encode_values},
    crate::crypto::secp256k1::keccak,
    crate::vm::evm,
};

const COIN: Address = Address::Contract([0; 32]);

fn deploy(chain: &mut Chain, code: Code) -> Address {
    let receipt = advance(chain, vec![SignedTx::sign(Tx::Deploy { code }, nonce(chain, &authority()), FeeOffer::NONE, &authority(), &chain.genesis())]).remove(0);

    match receipt.result {
        Ok(Effect::Deployed(address)) => address,
        other => panic!("expected a deploy, got {other:?}"),
    }
}

/// Calls a native contract as the authority, sending coin, and returns its receipt.
fn call(chain: &mut Chain, to: Address, function: &str, coin: u128, data: Vec<TxCallArgumentType>) -> Receipt {
    let tx = Tx::ContractCall { to, func: Hash::Xxh3_64(crate::vm::selector(function)), amount: Amount::U128(coin), data };

    advance(chain, vec![SignedTx::sign(tx, nonce(chain, &authority()), FeeOffer::NONE, &authority(), &chain.genesis())]).remove(0)
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
    let receipt = advance(chain, vec![SignedTx::sign(tx, nonce(chain, &authority()), FeeOffer::NONE, &authority(), &chain.genesis())]).remove(0);

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

/// A fee tier, as the DEX takes it.
fn tier(bps: i64) -> TxCallArgumentType {
    TxCallArgumentType::I64(bps)
}

/// The 0.3% tier the tests trade at.
fn mid() -> TxCallArgumentType {
    tier(30)
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
    refused(&call(&mut chain, dex, "create_pool", 0, vec![at(COIN), at(token), mid()]), "not set up");
    assert!(call(&mut chain, dex, "setup", 0, vec![at(template)]).result.is_ok());
    refused(&call(&mut chain, dex, "setup", 0, vec![at(template)]), "set up already");

    // Approve the DEX: the token by its address, the ERC-20 at the DEX's EVM address.
    assert!(call(&mut chain, token, "approve", 0, vec![at(dex), wide(u128::MAX)]).result.is_ok());
    evm_call(&mut chain, erc20, "approve(address,uint256)", &[AbiValue::Address(evm::evm_alias(&dex)), AbiValue::Uint(u256::MAX)], &[AbiType::Address, AbiType::Uint(256)]);

    // A pool of the coin and the token, opened by its first deposit at the price it sets; what
    // the call sent beyond what went in comes back.
    let before = balance(&chain, &me);
    let added = call(&mut chain, dex, "add_liquidity", 12_000_000, vec![at(COIN), at(token), mid(), wide(10_000_000), wide(20_000_000), wide(0), wide(0)]);
    let minted = 14_142_135 - 1_000;
    assert_eq!(returned(&added), [wide(10_000_000), wide(20_000_000), wide(minted)]);
    assert_eq!(before - balance(&chain, &me), u256::new(10_000_000), "the extra 2,000,000 sent back");
    let [_, _, TxCallArgumentType::Address(lp), _, _] = <[_; 5]>::try_from(returned(&call(&mut chain, dex, "pool", 0, vec![at(token), at(COIN), mid()]))).unwrap() else { panic!() };
    assert_eq!(returned(&call(&mut chain, lp, "balance_of", 0, vec![at(me)])), [wide(minted)]);
    assert_eq!(returned(&call(&mut chain, lp, "balance_of", 0, vec![at(dex)])), [wide(1_000)], "the first thousand never come out");
    assert_eq!(crate::registry::Tokens::new(&chain.state()).get(&lp).unwrap().and_then(|token| token.symbol), Some("DEX-LP".to_string()), "the share is a token in the registry");
    refused(&call(&mut chain, lp, "mint", 0, vec![at(me), wide(1)]), "only the pool");
    assert!(added.events.iter().any(|event| event.name == "Sync"));

    // Coin for token, the pool's price, its fee kept in the pool.
    let expected = out(100_000, 10_000_000, 20_000_000);
    let traded = call(&mut chain, dex, "swap", 100_000, vec![at(COIN), at(token), mid(), wide(100_000), wide(expected)]);
    assert_eq!(returned(&traded), [wide(expected)]);
    let sync = traded.events.iter().find(|event| event.name == "Sync").unwrap();
    assert_eq!(sync.data[2..], [wide(10_100_000), wide(20_000_000 - expected), tier(30)], "the pool after, in its order: coin first, and its tier");
    assert_eq!(traded.events.iter().find(|event| event.name == "Swap").unwrap().data.last(), Some(&tier(30)), "a trade names its pool's tier");
    refused(&call(&mut chain, dex, "swap", 100_000, vec![at(COIN), at(token), mid(), wide(100_000), wide(10u128.pow(9))]), "the price moved");
    refused(&call(&mut chain, dex, "swap", 0, vec![at(COIN), at(token), mid(), wide(1), wide(0)]), "send the coin");
    refused(&call(&mut chain, dex, "swap", 0, vec![at(token), at(COIN), mid(), wide(1_000), wide(0), TxCallArgumentType::I64(1)]), "deadline passed");

    // A second pool, the token against the ERC-20, and a trade through both: coin, token, ERC-20.
    assert!(call(&mut chain, dex, "add_liquidity", 0, vec![at(token), at(erc20), mid(), wide(5_000_000), wide(5_000_000), wide(0), wide(0)]).result.is_ok());
    let reserves = returned(&call(&mut chain, dex, "pool", 0, vec![at(COIN), at(token), mid()]));
    let [TxCallArgumentType::U256(coin_held), TxCallArgumentType::U256(token_held), ..] = reserves.as_slice() else { panic!("{reserves:?}") };
    let middle = out(50_000, u128::try_from(*coin_held).unwrap(), u128::try_from(*token_held).unwrap());
    let expected = out(middle, 5_000_000, 5_000_000);
    assert_eq!(returned(&call(&mut chain, dex, "quote", 0, vec![at(COIN), at(erc20), mid(), wide(50_000), at(token), mid()])), [wide(expected)]);
    let held = erc20_balance(&mut chain, erc20, &me);
    assert_eq!(returned(&call(&mut chain, dex, "swap", 50_000, vec![at(COIN), at(erc20), mid(), wide(50_000), wide(expected), TxCallArgumentType::Nil, at(token), mid()])), [wide(expected)]);
    assert_eq!(erc20_balance(&mut chain, erc20, &me) - held, u256::new(expected));
    assert_eq!(returned(&call(&mut chain, dex, "pool_count", 0, vec![])), [TxCallArgumentType::I64(2)]);

    // Half the shares out: half of what the pool holds, the shares burned.
    let reserves = returned(&call(&mut chain, dex, "pool", 0, vec![at(COIN), at(token), mid()]));
    let [TxCallArgumentType::U256(coin_held), TxCallArgumentType::U256(token_held), ..] = reserves.as_slice() else { panic!("{reserves:?}") };
    let (coin_held, token_held): (u128, u128) = (u128::try_from(*coin_held).unwrap(), u128::try_from(*token_held).unwrap());
    let half = minted / 2;
    let before = balance(&chain, &me);
    let removed = call(&mut chain, dex, "remove_liquidity", 0, vec![at(COIN), at(token), mid(), wide(half), wide(0), wide(0)]);
    let (coin_back, token_back) = (half * coin_held / (minted + 1_000), half * token_held / (minted + 1_000));
    assert_eq!(returned(&removed), [wide(coin_back), wide(token_back)]);
    assert_eq!(balance(&chain, &me) - before, u256::new(coin_back));
    assert_eq!(returned(&call(&mut chain, lp, "total_supply", 0, vec![])), [wide(minted + 1_000 - half)]);
    refused(&call(&mut chain, dex, "remove_liquidity", 0, vec![at(COIN), at(token), mid(), wide(minted), wide(0), wide(0)]), "insufficient balance");

    // What is not a pool is refused.
    refused(&call(&mut chain, dex, "create_pool", 0, vec![at(token), at(token), mid()]), "two different");
    refused(&call(&mut chain, dex, "create_pool", 0, vec![at(token), at(COIN), mid()]), "exists");
    refused(&call(&mut chain, dex, "swap", 0, vec![at(erc20), at(COIN), mid(), wide(1_000), wide(0)]), "no pool trades");

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
    let receipt = advance(chain, vec![SignedTx::sign(tx, nonce(chain, &authority()), FeeOffer::NONE, &authority(), &chain.genesis())]).remove(0);

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
    // The 0.3% tier at each step.
    let fees = |steps: usize| AbiValue::Array(vec![n(30); steps]);
    let fees_type = Array(Box::new(Uint(256)));

    for token in [a, b] {
        evm_send(&mut chain, token, 0, "approve(address,uint256)", &[Addr, Uint(256)], &[evm_address(&router), AbiValue::Uint(u256::MAX)]).unwrap();
    }

    // A with the coin: the coin wrapped on the way in, what is sent past the price sent back.
    let added = evm_send(&mut chain, router, 3_000_000, "addLiquidityDEV(address,uint256,uint256,uint256,uint256,address,uint256)", &[Addr, Uint(256), Uint(256), Uint(256), Uint(256), Addr, Uint(256)], &[evm_address(&a), n(30), n(4_000_000), n(0), n(0), evm_address(&me), deadline.clone()]).unwrap();
    assert_eq!(words(&added), [u256::new(4_000_000), u256::new(3_000_000), u256::new(3_464_101 - 1_000)], "sqrt(4e6 * 3e6), less the thousand locked");
    let pair = words(&evm_send(&mut chain, factory, 0, "getPair(address,address,uint256)", &[Addr, Addr, Uint(256)], &[evm_address(&a), evm_address(&wdev), n(30)]).unwrap())[0];
    assert_ne!(pair, u256::ZERO);

    // The coin for A, at the pair's price.
    let quoted = words(&evm_send(&mut chain, router, 0, "getAmountsOut(uint256,address[],uint256[])", &[Uint(256), addresses.clone(), fees_type.clone()], &[n(100_000), path(&[&wdev, &a]), fees(1)]).unwrap());
    let expected = out(100_000, 3_000_000, 4_000_000);
    assert_eq!(quoted[quoted.len() - 1], u256::new(expected));
    let held = erc20_balance(&mut chain, a, &me);
    evm_send(&mut chain, router, 100_000, "swapExactDEVForTokens(uint256,address[],uint256[],address,uint256)", &[Uint(256), addresses.clone(), fees_type.clone(), Addr, Uint(256)], &[n(expected), path(&[&wdev, &a]), fees(1), evm_address(&me), deadline.clone()]).unwrap();
    assert_eq!(erc20_balance(&mut chain, a, &me) - held, u256::new(expected));

    // B with A, then B for the coin along B, A, WDEV, unwrapped to the caller.
    evm_send(&mut chain, router, 0, "addLiquidity(address,address,uint256,uint256,uint256,uint256,uint256,address,uint256)", &[Addr, Addr, Uint(256), Uint(256), Uint(256), Uint(256), Uint(256), Addr, Uint(256)], &[evm_address(&b), evm_address(&a), n(30), n(2_000_000), n(2_000_000), n(0), n(0), evm_address(&me), deadline.clone()]).unwrap();
    let amounts = words(&evm_send(&mut chain, router, 0, "getAmountsOut(uint256,address[],uint256[])", &[Uint(256), addresses.clone(), fees_type.clone()], &[n(50_000), path(&[&b, &a, &wdev]), fees(2)]).unwrap());
    let coin_out = amounts[amounts.len() - 1];
    let before = balance(&chain, &me);
    evm_send(&mut chain, router, 0, "swapExactTokensForDEV(uint256,uint256,address[],uint256[],address,uint256)", &[Uint(256), Uint(256), addresses.clone(), fees_type.clone(), Addr, Uint(256)], &[n(50_000), AbiValue::Uint(coin_out), path(&[&b, &a, &wdev]), fees(2), evm_address(&me), deadline.clone()]).unwrap();
    assert_eq!(balance(&chain, &me) - before, coin_out, "the coin, unwrapped");

    // Shares out: approved to the router, burned, and both sides paid, the coin unwrapped.
    let pair = Address::Evm(pair.to_be_bytes()[12..].try_into().unwrap());
    let shares = erc20_balance(&mut chain, pair, &me);
    evm_send(&mut chain, pair, 0, "approve(address,uint256)", &[Addr, Uint(256)], &[evm_address(&router), AbiValue::Uint(shares)]).unwrap();
    let before = balance(&chain, &me);
    let removed = words(&evm_send(&mut chain, router, 0, "removeLiquidityDEV(address,uint256,uint256,uint256,uint256,address,uint256)", &[Addr, Uint(256), Uint(256), Uint(256), Uint(256), Addr, Uint(256)], &[evm_address(&a), n(30), AbiValue::Uint(shares), n(0), n(0), evm_address(&me), deadline.clone()]).unwrap());
    assert_eq!(balance(&chain, &me) - before, removed[1]);
    assert_eq!(erc20_balance(&mut chain, pair, &me), u256::ZERO);

    // Refused: a price that moved, a deadline passed, the coin sent straight to the router.
    let moved = evm_send(&mut chain, router, 0, "swapExactTokensForTokens(uint256,uint256,address[],uint256[],address,uint256)", &[Uint(256), Uint(256), addresses.clone(), fees_type.clone(), Addr, Uint(256)], &[n(1_000), n(10u128.pow(12)), path(&[&b, &a]), fees(1), evm_address(&me), deadline.clone()]);
    assert!(moved.unwrap_err().contains("the price moved"));
    let late = evm_send(&mut chain, router, 0, "swapExactTokensForTokens(uint256,uint256,address[],uint256[],address,uint256)", &[Uint(256), Uint(256), addresses, fees_type, Addr, Uint(256)], &[n(1_000), n(0), path(&[&b, &a]), fees(1), evm_address(&me), n(1)]);
    assert!(late.unwrap_err().contains("deadline passed"));
    assert!(evm_send(&mut chain, router, 5, "", &[], &[]).is_err(), "the router takes the coin from WDEV alone");

    assert_eq!(crate::registry::supply::adds_up(&chain.state()).unwrap(), Ok(()), "minted less burned is what is held and bonded");
}

#[test]
fn both_dexes_open_pools_only_at_tiers_the_chain_offers_and_each_charges_its_own() {
    use AbiType::{Address as Addr, Uint};

    // A chain whose genesis offers 1% alone.
    let genesis = Genesis { dex: Some(crate::params::vm::DexSettings { tiers: [100, 0, 0, 0], protocol_permille: 0 }), ..genesis() };
    let mut chain = Chain::open(Arc::new(MemoryDb::default()), &genesis).unwrap();
    let at_one_percent = |amount_in: u128, reserve_in: u128, reserve_out: u128| crate::dex::amount_out(u256::new(amount_in), u256::new(reserve_in), u256::new(reserve_out), 100).unwrap();
    assert!(at_one_percent(100_000, 10_000_000, 20_000_000) < u256::new(out(100_000, 10_000_000, 20_000_000)));

    // The native DEX: no 0.3% pool, a 1% one that charges 1%.
    let token = deploy(&mut chain, Code::Lua(include_str!("../../../contracts/lua/token.lua").to_string()));
    let template = deploy(&mut chain, Code::Lua(include_str!("../../../contracts/lua/lp_token.lua").to_string()));
    let dex = deploy(&mut chain, Code::Lua(include_str!("../../../contracts/lua/dex.lua").to_string()));
    assert!(call(&mut chain, dex, "setup", 0, vec![at(template)]).result.is_ok());
    assert!(call(&mut chain, token, "approve", 0, vec![at(dex), wide(u128::MAX)]).result.is_ok());
    refused(&call(&mut chain, dex, "create_pool", 0, vec![at(COIN), at(token), mid()]), "offers no 30 basis point tier");
    assert!(call(&mut chain, dex, "add_liquidity", 10_000_000, vec![at(COIN), at(token), tier(100), wide(10_000_000), wide(20_000_000), wide(0), wide(0)]).result.is_ok());
    let expected = at_one_percent(100_000, 10_000_000, 20_000_000);
    let traded = call(&mut chain, dex, "swap", 100_000, vec![at(COIN), at(token), tier(100), wide(100_000), wide(0)]);
    assert_eq!(returned(&traded), [TxCallArgumentType::U256(expected)]);
    let listed = returned(&call(&mut chain, dex, "pool_at", 0, vec![TxCallArgumentType::I64(0)]));
    assert_eq!(listed[2], TxCallArgumentType::I64(100), "pool_at says its tier");

    // The EVM DEX: the factory opens no 0.3% pair, and the router quotes a pair by its own fee.
    let wdev = create(&mut chain, include_str!("../../../contracts/evm/WDEV.bin"), Vec::new());
    let factory = create(&mut chain, include_str!("../../../contracts/evm/DexFactory.bin"), Vec::new());
    let router = create(&mut chain, include_str!("../../../contracts/evm/DexRouter.bin"), encode_values(&[Addr, Addr], &[evm_address(&factory), evm_address(&wdev)]).unwrap());
    let supply = encode_values(&[AbiType::String, AbiType::String, Uint(256)], &[AbiValue::String("A".into()), AbiValue::String("A".into()), AbiValue::Uint(u256::new(10u128.pow(24)))]).unwrap();
    let a = create(&mut chain, include_str!("../../../contracts/evm/Token.bin"), supply);
    let refused_pair = evm_send(&mut chain, factory, 0, "createPair(address,address,uint256)", &[Addr, Addr, Uint(256)], &[evm_address(&a), evm_address(&wdev), AbiValue::Uint(u256::new(30))]);
    assert!(refused_pair.unwrap_err().contains("no such fee tier"));
    assert!(evm_send(&mut chain, factory, 0, "createPair(address,address,uint256)", &[Addr, Addr, Uint(256)], &[evm_address(&a), evm_address(&wdev), AbiValue::Uint(u256::new(100))]).is_ok());
    let quoted = evm_call(&mut chain, router, "getAmountOut(uint256,uint256,uint256,uint256)", &[AbiValue::Uint(u256::new(100_000)), AbiValue::Uint(u256::new(10_000_000)), AbiValue::Uint(u256::new(20_000_000)), AbiValue::Uint(u256::new(100))], &[Uint(256), Uint(256), Uint(256), Uint(256)]);
    assert_eq!(words(&quoted), [expected]);
}

/// A native DEX set up with the token-standard token, approved, and its address and the token's.
/// A pool keeps trading at a tier governance takes off the list, as V3's pools keep theirs; only
/// a new pool is held to the list, and contracts read the list as it now stands.
#[cfg(all(feature = "finality-ffg", feature = "governance-council"))]
#[test]
fn a_tier_voted_off_keeps_its_pools_trading_and_opens_no_new_ones() {
    use super::settings::{decide, governed};

    let mut chain = governed(genesis());
    let (dex, token) = native_dex(&mut chain);
    assert!(call(&mut chain, dex, "add_liquidity", 10_000_000, vec![at(COIN), at(token), mid(), wide(10_000_000), wide(20_000_000), wide(0), wide(0)]).result.is_ok());

    let only_one_percent = crate::params::vm::DexSettings { tiers: [100, 0, 0, 0], protocol_permille: 0 };
    decide(&mut chain, crate::governance::Change::Dex(only_one_percent));

    let traded = call(&mut chain, dex, "swap", 100_000, vec![at(COIN), at(token), mid(), wide(100_000), wide(0)]);
    assert_eq!(returned(&traded), [wide(out(100_000, 10_000_000, 20_000_000))], "the 0.3% pool trades on at 0.3%");
    let other = deploy(&mut chain, Code::Lua(include_str!("../../../contracts/lua/token.lua").to_string()));
    assert!(call(&mut chain, other, "approve", 0, vec![at(dex), wide(u128::MAX)]).result.is_ok());
    refused(&call(&mut chain, dex, "add_liquidity", 1_000_000, vec![at(COIN), at(other), mid(), wide(1_000_000), wide(1_000_000), wide(0), wide(0)]), "offers no 30 basis point tier");
    assert!(call(&mut chain, dex, "add_liquidity", 1_000_000, vec![at(COIN), at(other), tier(100), wide(1_000_000), wide(1_000_000), wide(0), wide(0)]).result.is_ok());

    // What a contract reads of them.
    let reader = deploy(&mut chain, Code::Lua("local M = {}\nfunction M.read()\n    local dex = chain.dex()\n    return #dex.tiers, dex.tiers[1], dex.protocol_permille\nend\nreturn M".to_string()));
    assert_eq!(returned(&call(&mut chain, reader, "read", 0, vec![])), [TxCallArgumentType::I64(1), TxCallArgumentType::I64(100), TxCallArgumentType::I64(0)]);
}

/// Borrows the coin from the DEX's pool and pays it back, with the fee, through repay.
const COIN_BORROWER: &str = r#"
local M = {}

function M.borrow(dex, a, b, fee, amount)
    storage.set("dex", dex:text())
    return contracts.call(dex, "flash", a, b, fee, amount, 0, ctx.contract, "")
end

--@reentrant
function M.on_flash(sender, a, b, amount_a, amount_b, data)
    local owed = amount_a * u256(10000) / u256(9970) + u256(1)
    -- More than owed, and says what it pays: the rest comes back.
    contracts.call_value(address.parse(storage.get("dex")), owed + u256(500), "repay")
    return owed, u256(0)
end

return M
"#;

#[test]
fn a_flash_loan_of_the_coin_comes_back_through_repay() {
    let mut chain = open(&Arc::new(MemoryDb::default()));
    let (dex, token) = native_dex(&mut chain);
    assert!(call(&mut chain, dex, "add_liquidity", 10_000_000, vec![at(COIN), at(token), mid(), wide(10_000_000), wide(20_000_000), wide(0), wide(0)]).result.is_ok());
    let borrower = deploy(&mut chain, Code::Lua(COIN_BORROWER.to_string()));

    // Sent enough of the coin with the call to pay the fee from.
    let flashed = call(&mut chain, borrower, "borrow", 10_000, vec![at(dex), at(COIN), at(token), mid(), wide(1_000_000)]);
    assert!(flashed.result.is_ok(), "{:?}", flashed.result);
    let fee = 1_000_000 * 10_000 / 9_970 + 1 - 1_000_000;
    let held = returned(&call(&mut chain, dex, "pool", 0, vec![at(COIN), at(token), mid()]));
    assert_eq!(held[0], wide(10_000_000 + fee), "the coin back, and its fee");
    assert_eq!(balance(&chain, &borrower), u256::new(10_000 - fee), "what it sent past what it paid, back");
}

#[test]
fn the_evm_dex_sends_the_protocols_share_of_each_fee_to_dead() {
    use AbiType::{Address as Addr, Uint};

    // Half of each fee burned.
    let genesis = Genesis { dex: Some(crate::params::vm::DexSettings { tiers: [30, 0, 0, 0], protocol_permille: 500 }), ..genesis() };
    let mut chain = Chain::open(Arc::new(MemoryDb::default()), &genesis).unwrap();
    let wdev = create(&mut chain, include_str!("../../../contracts/evm/WDEV.bin"), Vec::new());
    let factory = create(&mut chain, include_str!("../../../contracts/evm/DexFactory.bin"), Vec::new());
    let router = create(&mut chain, include_str!("../../../contracts/evm/DexRouter.bin"), encode_values(&[Addr, Addr], &[evm_address(&factory), evm_address(&wdev)]).unwrap());
    let supply = |symbol: &str| encode_values(&[AbiType::String, AbiType::String, Uint(256)], &[AbiValue::String(symbol.into()), AbiValue::String(symbol.into()), AbiValue::Uint(u256::new(10u128.pow(24)))]).unwrap();
    let (x, y) = (create(&mut chain, include_str!("../../../contracts/evm/Token.bin"), supply("X")), create(&mut chain, include_str!("../../../contracts/evm/Token.bin"), supply("Y")));
    let deadline = AbiValue::Uint(u256::from(chain.tip().timestamp / 1_000 + 3_600));
    let n = |value: u128| AbiValue::Uint(u256::new(value));
    let me = authority().public().address();

    for token in [x, y] {
        evm_send(&mut chain, token, 0, "approve(address,uint256)", &[Addr, Uint(256)], &[evm_address(&router), AbiValue::Uint(u256::MAX)]).unwrap();
    }

    evm_send(&mut chain, router, 0, "addLiquidity(address,address,uint256,uint256,uint256,uint256,uint256,address,uint256)", &[Addr, Addr, Uint(256), Uint(256), Uint(256), Uint(256), Uint(256), Addr, Uint(256)], &[evm_address(&x), evm_address(&y), n(30), n(5_000_000), n(5_000_000), n(0), n(0), evm_address(&me), deadline.clone()]).unwrap();
    let dead = Address::Evm(*b"\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\xde\xad");
    let path = AbiValue::Array(vec![evm_address(&x), evm_address(&y)]);
    let (addresses, tiers) = (AbiType::Array(Box::new(Addr)), AbiType::Array(Box::new(Uint(256))));
    evm_send(&mut chain, router, 0, "swapExactTokensForTokens(uint256,uint256,address[],uint256[],address,uint256)", &[Uint(256), Uint(256), addresses, tiers, Addr, Uint(256)], &[n(100_000), n(0), path, AbiValue::Array(vec![n(30)]), evm_address(&me), deadline]).unwrap();

    // The fee is 300 of 100,000 X: half of it to 0x…dEaD, none of Y.
    assert_eq!(erc20_balance(&mut chain, x, &dead), u256::new(150));
    assert_eq!(erc20_balance(&mut chain, y, &dead), u256::ZERO);
}

fn native_dex(chain: &mut Chain) -> (Address, Address) {
    let token = deploy(chain, Code::Lua(include_str!("../../../contracts/lua/token.lua").to_string()));
    let template = deploy(chain, Code::Lua(include_str!("../../../contracts/lua/lp_token.lua").to_string()));
    let dex = deploy(chain, Code::Lua(include_str!("../../../contracts/lua/dex.lua").to_string()));
    assert!(call(chain, dex, "setup", 0, vec![at(template)]).result.is_ok());
    assert!(call(chain, token, "approve", 0, vec![at(dex), wide(u128::MAX)]).result.is_ok());

    (dex, token)
}

#[test]
fn the_protocols_share_of_each_fee_is_burned_the_coin_out_of_the_supply_a_token_to_nobody() {
    // Half of each fee burned.
    let genesis = Genesis { dex: Some(crate::params::vm::DexSettings { tiers: [30, 0, 0, 0], protocol_permille: 500 }), ..genesis() };
    let mut chain = Chain::open(Arc::new(MemoryDb::default()), &genesis).unwrap();
    let (dex, token) = native_dex(&mut chain);
    assert!(call(&mut chain, dex, "add_liquidity", 10_000_000, vec![at(COIN), at(token), mid(), wide(10_000_000), wide(20_000_000), wide(0), wide(0)]).result.is_ok());
    let burned_before = crate::registry::supply::Supply::of(&chain.state()).unwrap().burned;

    // The coin in: the fee is 300 of 100,000, half of it burned, the other half kept in the pool.
    let traded = call(&mut chain, dex, "swap", 100_000, vec![at(COIN), at(token), mid(), wide(100_000), wide(0)]);
    assert_eq!(returned(&traded), [wide(out(100_000, 10_000_000, 20_000_000))], "what comes out is as ever");
    let burned = crate::registry::supply::Supply::of(&chain.state()).unwrap().burned - burned_before;
    assert!(burned >= u256::new(150), "the coin burned, past what the transaction's fee burns: {burned}");
    let held = returned(&call(&mut chain, dex, "pool", 0, vec![at(COIN), at(token), mid()]));
    assert_eq!(held[0], wide(10_000_000 + 100_000 - 150), "the pool keeps what came in, less the burned half of the fee");

    // A token in: its burned share goes to the zero address, the token standard's nobody, which no
    // transaction may name, so read where the token keeps balances provable.
    let nobody = Address::Ed25519([0; 32]);
    let held_by_nobody = |chain: &Chain| {
        let kept = chain.state().get(crate::db::Column::Namespaces, &crate::light::token_balance_key(&token, &nobody)).unwrap();
        kept.map_or(u256::ZERO, |value| crate::light::token_balance(&value).unwrap())
    };
    let before = held_by_nobody(&chain);
    assert!(call(&mut chain, dex, "swap", 0, vec![at(token), at(COIN), mid(), wide(200_000), wide(0)]).result.is_ok());
    assert_eq!(held_by_nobody(&chain), before + u256::new(300), "half of 600");
    assert_eq!(crate::registry::supply::adds_up(&chain.state()).unwrap(), Ok(()), "minted less burned is what is held and bonded");
}

#[test]
fn a_pool_sums_its_prices_over_time_for_a_time_weighted_price() {
    let mut chain = open(&Arc::new(MemoryDb::default()));
    let (dex, token) = native_dex(&mut chain);
    assert!(call(&mut chain, dex, "add_liquidity", 10_000_000, vec![at(COIN), at(token), mid(), wide(10_000_000), wide(20_000_000), wide(0), wide(0)]).result.is_ok());
    let observe = |chain: &mut Chain| match returned(&call(chain, dex, "pool", 0, vec![at(COIN), at(token), mid()])).as_slice() {
        [_, _, _, TxCallArgumentType::U256(price_coin), TxCallArgumentType::U256(price_token)] => (*price_coin, *price_token, chain.tip().timestamp),
        other => panic!("{other:?}"),
    };
    let (first_coin, first_token, first_time) = observe(&mut chain);
    for _ in 0..3 {
        advance(&mut chain, Vec::new());
    }
    let (coin, token_price, time) = observe(&mut chain);
    let elapsed = u256::from(time - first_time);
    let one = u256::from(10u64).pow(18);

    // At 2 tokens a coin all along, the average is exactly that, and its inverse.
    assert_eq!((coin - first_coin) / elapsed, one * u256::from(2u64));
    assert_eq!((token_price - first_token) / elapsed, one / u256::from(2u64));
}

const BORROWER: &str = r#"
local M = {}

-- Borrows amount of a token from the DEX's pool, repaying it with the fee, short by `short`.
function M.borrow(dex, a, b, fee, amount, short)
    storage.set("short", tostring(short))
    return contracts.call(dex, "flash", a, b, fee, amount, 0, ctx.contract, "")
end

--@reentrant
function M.on_flash(sender, a, b, amount_a, amount_b, data)
    local owed = amount_a * u256(10000) / u256(9970) + u256(1) - u256(storage.get("short"))
    contracts.call(a, "approve", ctx.sender, owed)
    return owed, u256(0)
end

return M
"#;

#[test]
fn a_flash_swap_lends_within_one_call_and_must_come_back_with_its_fee_on_either_dex() {
    use AbiType::{Address as Addr, Uint};

    let mut chain = open(&Arc::new(MemoryDb::default()));
    let (dex, token) = native_dex(&mut chain);
    assert!(call(&mut chain, dex, "add_liquidity", 10_000_000, vec![at(token), at(COIN), mid(), wide(20_000_000), wide(10_000_000), wide(0), wide(0)]).result.is_ok());
    let borrower = deploy(&mut chain, Code::Lua(BORROWER.to_string()));
    // Enough of the token to pay the fee with.
    assert!(call(&mut chain, token, "transfer", 0, vec![at(borrower), wide(1_000_000)]).result.is_ok());

    // Repaid with the fee: the pool holds more than before, the fee in it.
    let flashed = call(&mut chain, borrower, "borrow", 0, vec![at(dex), at(token), at(COIN), mid(), wide(1_000_000), wide(0)]);
    assert!(flashed.result.is_ok(), "{:?}", flashed.result);
    let held = returned(&call(&mut chain, dex, "pool", 0, vec![at(token), at(COIN), mid()]));
    assert_eq!(held[0], wide(20_000_000 + 1_000_000 * 10_000 / 9_970 + 1 - 1_000_000));
    assert!(flashed.events.iter().any(|event| event.name == "Flash"));

    // A unit short, it is all undone; and no one else repays a loan.
    refused(&call(&mut chain, borrower, "borrow", 0, vec![at(dex), at(token), at(COIN), mid(), wide(1_000_000), wide(2)]), "must repay");
    refused(&call(&mut chain, dex, "repay", 5, vec![]), "only the contract a flash swap lent to");

    // The EVM: a pair lends to FlashBorrower.sol, which repays with the fee, or a unit short.
    let wdev = create(&mut chain, include_str!("../../../contracts/evm/WDEV.bin"), Vec::new());
    let factory = create(&mut chain, include_str!("../../../contracts/evm/DexFactory.bin"), Vec::new());
    let router = create(&mut chain, include_str!("../../../contracts/evm/DexRouter.bin"), encode_values(&[Addr, Addr], &[evm_address(&factory), evm_address(&wdev)]).unwrap());
    let supply = |symbol: &str| encode_values(&[AbiType::String, AbiType::String, Uint(256)], &[AbiValue::String(symbol.into()), AbiValue::String(symbol.into()), AbiValue::Uint(u256::new(10u128.pow(24)))]).unwrap();
    let (x, y) = (create(&mut chain, include_str!("../../../contracts/evm/Token.bin"), supply("X")), create(&mut chain, include_str!("../../../contracts/evm/Token.bin"), supply("Y")));
    let deadline = AbiValue::Uint(u256::from(chain.tip().timestamp / 1_000 + 3_600));
    let n = |value: u128| AbiValue::Uint(u256::new(value));
    let me = authority().public().address();

    for token in [x, y] {
        evm_send(&mut chain, token, 0, "approve(address,uint256)", &[Addr, Uint(256)], &[evm_address(&router), AbiValue::Uint(u256::MAX)]).unwrap();
    }

    evm_send(&mut chain, router, 0, "addLiquidity(address,address,uint256,uint256,uint256,uint256,uint256,address,uint256)", &[Addr, Addr, Uint(256), Uint(256), Uint(256), Uint(256), Uint(256), Addr, Uint(256)], &[evm_address(&x), evm_address(&y), n(30), n(5_000_000), n(5_000_000), n(0), n(0), evm_address(&me), deadline]).unwrap();
    let pair = words(&evm_send(&mut chain, factory, 0, "getPair(address,address,uint256)", &[Addr, Addr, Uint(256)], &[evm_address(&x), evm_address(&y), n(30)]).unwrap())[0];
    let pair = Address::Evm(pair.to_be_bytes()[12..].try_into().unwrap());
    let token0 = Address::Evm(words(&evm_send(&mut chain, pair, 0, "token0()", &[], &[]).unwrap())[0].to_be_bytes()[12..].try_into().unwrap());
    let flash = create(&mut chain, include_str!("../../../contracts/evm/FlashBorrower.bin"), Vec::new());
    evm_send(&mut chain, token0, 0, "transfer(address,uint256)", &[Addr, Uint(256)], &[evm_address(&flash), n(100_000)]).unwrap();

    assert!(evm_send(&mut chain, flash, 0, "borrow(address,uint256,uint256)", &[Addr, Uint(256), Uint(256)], &[evm_address(&pair), n(1_000_000), n(0)]).is_ok());
    let short = evm_send(&mut chain, flash, 0, "borrow(address,uint256,uint256)", &[Addr, Uint(256), Uint(256)], &[evm_address(&pair), n(1_000_000), n(2)]);
    assert!(short.unwrap_err().contains("takes more than it gives"));
}

#[test]
fn the_router_divides_one_trade_between_both_dexes_for_more_than_either_pays_alone() {
    use AbiType::{Address as Addr, Array, Uint};

    let mut chain = open(&Arc::new(MemoryDb::default()));
    let me = authority().public().address();
    let supply = |symbol: &str| encode_values(&[AbiType::String, AbiType::String, Uint(256)], &[AbiValue::String(symbol.into()), AbiValue::String(symbol.into()), AbiValue::Uint(u256::new(10u128.pow(24)))]).unwrap();
    let (x, y) = (create(&mut chain, include_str!("../../../contracts/evm/Token.bin"), supply("X")), create(&mut chain, include_str!("../../../contracts/evm/Token.bin"), supply("Y")));
    let n = |value: u128| AbiValue::Uint(u256::new(value));
    let deadline_s = AbiValue::Uint(u256::from(chain.tip().timestamp / 1_000 + 3_600));

    // The same two ERC-20s, a million each, in a pool on each DEX.
    let (dex, _) = native_dex(&mut chain);
    let wdev = create(&mut chain, include_str!("../../../contracts/evm/WDEV.bin"), Vec::new());
    let factory = create(&mut chain, include_str!("../../../contracts/evm/DexFactory.bin"), Vec::new());
    let evm_router = create(&mut chain, include_str!("../../../contracts/evm/DexRouter.bin"), encode_values(&[Addr, Addr], &[evm_address(&factory), evm_address(&wdev)]).unwrap());
    let router = deploy(&mut chain, Code::Lua(include_str!("../../../contracts/lua/router.lua").to_string()));
    assert!(call(&mut chain, router, "setup", 0, vec![at(dex), at(evm_router), at(wdev)]).result.is_ok());

    for token in [x, y] {
        for spender in [dex, evm_router, router] {
            evm_send(&mut chain, token, 0, "approve(address,uint256)", &[Addr, Uint(256)], &[evm_address(&spender), AbiValue::Uint(u256::MAX)]).unwrap();
        }
    }

    assert!(call(&mut chain, dex, "add_liquidity", 0, vec![at(x), at(y), mid(), wide(1_000_000), wide(1_000_000), wide(0), wide(0)]).result.is_ok());
    evm_send(&mut chain, evm_router, 0, "addLiquidity(address,address,uint256,uint256,uint256,uint256,uint256,address,uint256)", &[Addr, Addr, Uint(256), Uint(256), Uint(256), Uint(256), Uint(256), Addr, Uint(256)], &[evm_address(&x), evm_address(&y), n(30), n(1_000_000), n(1_000_000), n(0), n(0), evm_address(&me), deadline_s]).unwrap();

    // 200,000 of X: on one DEX it moves the price far; halved, each half moves it less.
    let whole = out(200_000, 1_000_000, 1_000_000);
    let halves = 2 * out(100_000, 1_000_000, 1_000_000);
    assert!(halves > whole);
    let evm_route = encode_values(&[Array(Box::new(Addr)), Array(Box::new(Uint(256)))], &[AbiValue::Array(vec![evm_address(&x), evm_address(&y)]), AbiValue::Array(vec![n(30)])]).unwrap();
    let route = |least: u128| {
        vec![at(x), at(y), wide(200_000), wide(least), TxCallArgumentType::Nil, wide(100_000), mid(), TxCallArgumentType::Nil, TxCallArgumentType::Nil, wide(100_000), TxCallArgumentType::Bytes(evm_route.clone())]
    };
    refused(&call(&mut chain, router, "route", 0, route(halves + 1)), "the price moved");
    let before = erc20_balance(&mut chain, y, &me);
    let routed = call(&mut chain, router, "route", 0, route(halves));
    assert_eq!(returned(&routed), [wide(halves)], "{:?}", routed.result);
    assert_eq!(erc20_balance(&mut chain, y, &me) - before, u256::new(halves));

    // The two parts are the whole.
    let mut uneven = route(0);
    uneven[5] = wide(150_000);
    refused(&call(&mut chain, router, "route", 0, uneven), "add up to the amount");
}

/// What `chainv4 dex add` sends: the amounts given as the most, and the pair `dex::add_amounts`
/// works out from the reserves as the least, here with no slippage at all. Each DEX takes that pair
/// to the unit, off any round ratio and either way round, where asking for at least the amounts
/// given less a slippage, as the CLI once did, is refused.
#[test]
fn both_dexes_take_the_pair_worked_out_for_amounts_off_their_ratio() {
    use AbiType::{Address as Addr, Uint};

    let db = Arc::new(MemoryDb::default());
    let mut chain = open(&db);
    let me = authority().public().address();
    let n = u256::new;
    let worked_out = |held: (u128, u128), most: (u128, u128)| crate::dex::add_amounts(n(held.0), n(held.1), n(most.0), n(most.1)).unwrap();
    let grown = |held: (u128, u128), put: (u256, u256)| (held.0 + u128::try_from(put.0).unwrap(), held.1 + u128::try_from(put.1).unwrap());

    // The native DEX, a pool of the coin and a token at an awkward price.
    let token = deploy(&mut chain, Code::Lua(include_str!("../../../contracts/lua/token.lua").to_string()));
    let template = deploy(&mut chain, Code::Lua(include_str!("../../../contracts/lua/lp_token.lua").to_string()));
    let dex = deploy(&mut chain, Code::Lua(include_str!("../../../contracts/lua/dex.lua").to_string()));
    assert!(call(&mut chain, dex, "setup", 0, vec![at(template)]).result.is_ok());
    assert!(call(&mut chain, token, "approve", 0, vec![at(dex), wide(u128::MAX)]).result.is_ok());
    assert!(call(&mut chain, dex, "add_liquidity", 7_000_003, vec![at(COIN), at(token), mid(), wide(7_000_003), wide(3_000_017), wide(0), wide(0)]).result.is_ok());
    let held = |chain: &mut Chain, a: Address, b: Address| match returned(&call(chain, dex, "pool", 0, vec![at(a), at(b), mid()])).as_slice() {
        [TxCallArgumentType::U256(a), TxCallArgumentType::U256(b), ..] => (u128::try_from(*a).unwrap(), u128::try_from(*b).unwrap()),
        other => panic!("{other:?}"),
    };

    // The old least, the amounts given less 0.5%, is refused off the ratio.
    refused(&call(&mut chain, dex, "add_liquidity", 1_000_000, vec![at(COIN), at(token), mid(), wide(1_000_000), wide(1_000_000), wide(995_000), wide(995_000)]), "the pool's price moved");

    // Too much of the token, too much of the coin, and the same with the pair named the other way.
    for (a, b, most) in [(COIN, token, (1_000_000, 1_000_000)), (COIN, token, (1_000_000, 100_000)), (token, COIN, (1_000_000, 333_333)), (token, COIN, (10_007, 999_999))] {
        let (put_a, put_b) = worked_out(held(&mut chain, a, b), most);
        let coin = if a == COIN { most.0 } else { most.1 };
        let before = balance(&chain, &me);
        let added = call(&mut chain, dex, "add_liquidity", coin, vec![at(a), at(b), mid(), wide(most.0), wide(most.1), TxCallArgumentType::U256(put_a), TxCallArgumentType::U256(put_b)]);
        assert_eq!(returned(&added)[..2], [TxCallArgumentType::U256(put_a), TxCallArgumentType::U256(put_b)], "{a} and {b}, at most {most:?}");
        assert_eq!(before - balance(&chain, &me), if a == COIN { put_a } else { put_b }, "the coin beyond the pair sent back");
    }

    // The EVM DEX: two ERC-20s, then one against the coin, each at an awkward price.
    let wdev = create(&mut chain, include_str!("../../../contracts/evm/WDEV.bin"), Vec::new());
    let factory = create(&mut chain, include_str!("../../../contracts/evm/DexFactory.bin"), Vec::new());
    let router = create(&mut chain, include_str!("../../../contracts/evm/DexRouter.bin"), encode_values(&[Addr, Addr], &[evm_address(&factory), evm_address(&wdev)]).unwrap());
    let supply = |symbol: &str| encode_values(&[AbiType::String, AbiType::String, Uint(256)], &[AbiValue::String(symbol.into()), AbiValue::String(symbol.into()), AbiValue::Uint(u256::new(10u128.pow(24)))]).unwrap();
    let (x, y) = (create(&mut chain, include_str!("../../../contracts/evm/Token.bin"), supply("X")), create(&mut chain, include_str!("../../../contracts/evm/Token.bin"), supply("Y")));
    let deadline = AbiValue::Uint(u256::from(chain.tip().timestamp / 1_000 + 3_600));
    let v = AbiValue::Uint;

    for token in [x, y] {
        evm_send(&mut chain, token, 0, "approve(address,uint256)", &[Addr, Uint(256)], &[evm_address(&router), AbiValue::Uint(u256::MAX)]).unwrap();
    }

    let add = "addLiquidity(address,address,uint256,uint256,uint256,uint256,uint256,address,uint256)";
    let add_types = [Addr, Addr, Uint(256), Uint(256), Uint(256), Uint(256), Uint(256), Addr, Uint(256)];
    let adding = |first: &Address, second: &Address, most: (u128, u128), least: (u256, u256)| [evm_address(first), evm_address(second), v(n(30)), v(n(most.0)), v(n(most.1)), v(least.0), v(least.1), evm_address(&me), deadline.clone()];
    evm_send(&mut chain, router, 0, add, &add_types, &adding(&x, &y, (7_000_003, 3_000_017), (u256::ZERO, u256::ZERO))).unwrap();
    let mut pair = (7_000_003u128, 3_000_017u128);
    let old = evm_send(&mut chain, router, 0, add, &add_types, &adding(&x, &y, (1_000_000, 1_000_000), (n(995_000), n(995_000))));
    assert!(old.unwrap_err().contains("the pair's price moved"));

    for most in [(1_000_000, 1_000_000), (1_000_000, 100_000)] {
        let put = worked_out(pair, most);
        let added = words(&evm_send(&mut chain, router, 0, add, &add_types, &adding(&x, &y, most, put)).unwrap());
        assert_eq!(added[..2], [put.0, put.1], "at most {most:?}");
        pair = grown(pair, put);
    }

    // Y named first, against the order the pair keeps them in.
    let (put_y, put_x) = worked_out((pair.1, pair.0), (77_777, 1_000_000));
    let added = words(&evm_send(&mut chain, router, 0, add, &add_types, &adding(&y, &x, (77_777, 1_000_000), (put_y, put_x))).unwrap());
    assert_eq!(added[..2], [put_y, put_x]);

    // X and the coin: what is sent beyond the pair comes back.
    let add_dev = "addLiquidityDEV(address,uint256,uint256,uint256,uint256,address,uint256)";
    let dev_types = [Addr, Uint(256), Uint(256), Uint(256), Uint(256), Addr, Uint(256)];
    evm_send(&mut chain, router, 3_000_001, add_dev, &dev_types, &[evm_address(&x), v(n(30)), v(n(4_000_000)), v(u256::ZERO), v(u256::ZERO), evm_address(&me), deadline.clone()]).unwrap();
    let mut with_coin = (4_000_000u128, 3_000_001u128);

    for (most_x, most_coin) in [(1_000_000u128, 1_000_000u128), (100_000, 1_000_000)] {
        let put = worked_out(with_coin, (most_x, most_coin));
        let before = balance(&chain, &me);
        let added = words(&evm_send(&mut chain, router, most_coin, add_dev, &dev_types, &[evm_address(&x), v(n(30)), v(n(most_x)), v(put.0), v(put.1), evm_address(&me), deadline.clone()]).unwrap());
        assert_eq!(added[..2], [put.0, put.1], "at most {most_x} and {most_coin}");
        assert_eq!(before - balance(&chain, &me), put.1, "the coin beyond the pair sent back");
        with_coin = grown(with_coin, put);
    }
}

