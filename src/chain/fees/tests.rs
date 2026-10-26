//! Fees on a chain: what transactions pay and get back, where fees go, the block gas limit, the
//! base price, bids, and a scorer contract deciding what is charged.

use super::*;
use crate::chain::*;
use crate::consensus::{AuthorityProducer, DevAuthority, Producer};
use crate::crypto::keypair::Keypair;
use crate::db::MemoryDb;
use crate::registry::Accounts;
use crate::registry::contracts::contract_address;
use crate::structs::amount::Amount;
use crate::structs::block::Block;
use crate::structs::code::Code;
use crate::structs::hash::Hash;
use crate::structs::tx::{Tx, TxCallArgumentType};
use crate::vm::selector;

const SLOT: u64 = 2_000;
/// What alice holds from genesis: one coin.
const COIN: u64 = 1_000_000_000_000_000_000;

/// Burns as much fuel as it is asked to: n turns of a loop.
const SPINNER: &str = r#"
local M = {}

function M.spin(n)
    local total = 0
    for i = 1, n do
        total = total + i
    end
    return total
end

return M
"#;

/// Halves what transfers cost, charges burns far past any limit, fails for mints, and leaves
/// everything else as weighed.
const SCORER: &str = r#"
local M = {}

function M.score(sender, kind, target, bytes, fuel, gas, failed)
    if kind == "transfer" then
        return gas / 2
    end
    if kind == "burn" then
        return gas * 1000000000
    end
    if kind == "mint" then
        error("no opinion")
    end
    return gas
end

return M
"#;

fn key(seed: u8) -> Keypair {
    Keypair::from_seed([seed; 32])
}

/// The development key, which produces every block here, so fees' producer share goes to it.
fn dev() -> Keypair {
    DevAuthority::dev_keypair()
}

/// Alice sends the transactions here, so what she pays and what the producer gets stay apart.
fn alice() -> Keypair {
    key(21)
}

fn genesis(fees: FeeConfig) -> Genesis {
    let mut genesis = Genesis {
        fees: Some(fees),
        finality: None,
        ..Genesis::dev_authority(dev().public())
    };
    genesis.allocations.push(Allocation {
        address: alice().public().address(),
        amount: u256::from(COIN),
    });

    genesis
}

fn open(fees: FeeConfig) -> Chain {
    Chain::open(Arc::new(MemoryDb::default()), &genesis(fees)).unwrap()
}

fn balance(chain: &Chain, keypair: &Keypair) -> u256 {
    Accounts::new(&chain.state()).get(&keypair.public().address()).unwrap().balance
}

fn nonce(chain: &Chain, keypair: &Keypair) -> u64 {
    Accounts::new(&chain.state()).get(&keypair.public().address()).unwrap().nonce
}

/// An offer of this much gas at twice the next base price, which covers any block soon.
fn offer(chain: &Chain, gas_limit: u64) -> FeeOffer {
    FeeOffer {
        gas_limit,
        max_price: chain.next_base_price() * u256::from(2u64),
        tip: u256::ZERO,
    }
}

/// Signs transactions from one sender with its next nonces.
fn signed(chain: &Chain, keypair: &Keypair, txs: Vec<Tx>, fee: FeeOffer) -> Vec<SignedTx> {
    let first = nonce(chain, keypair);

    txs.into_iter()
        .enumerate()
        .map(|(index, tx)| SignedTx::sign(tx, first + index as u64, fee, keypair))
        .collect()
}

/// Builds the next block from these transactions, keeping what can run, and imports it.
fn advance(chain: &mut Chain, txs: Vec<SignedTx>) -> (Block, Vec<Receipt>) {
    let now = chain.tip().timestamp + SLOT;
    let mut producer = AuthorityProducer::new(dev(), SLOT);
    let mut block = chain.build(txs, Vec::new(), producer.address(), now).unwrap();
    block.seal = producer.seal(&block.header).unwrap();

    match chain.import(&block, now).unwrap() {
        Imported::Extended(receipts) => (block, receipts),
        other => panic!("expected the tip to be extended, got {other:?}"),
    }
}

/// Imports a block holding exactly these transactions, as a producer breaking the rules would
/// send it, with a chance to tamper with its header. A block that breaks them changes nothing.
fn forced(chain: &mut Chain, txs: Vec<SignedTx>, tamper: impl FnOnce(&mut BlockHeader)) -> Result<Imported, ImportError> {
    let now = chain.tip().timestamp + SLOT;
    let mut producer = AuthorityProducer::new(dev(), SLOT);
    let mut block = chain.build(Vec::new(), Vec::new(), producer.address(), now).unwrap();
    block.header.tx_root = Block::tx_root(chain.hash(), &txs);
    block.txs = txs;
    tamper(&mut block.header);
    block.seal = producer.seal(&block.header).unwrap();

    chain.import(&block, now)
}

fn transfer(to: &Keypair, amount: u128) -> Tx {
    transfer_from(&alice(), to, amount)
}

fn transfer_from(from: &Keypair, to: &Keypair, amount: u128) -> Tx {
    Tx::Transfer {
        from: from.public().address(),
        to: to.public().address(),
        amount: Amount::U128(amount),
    }
}

#[test]
fn transactions_pay_for_the_gas_they_use_and_get_the_rest_back() {
    let mut chain = open(FeeConfig::dev());
    let (alice, bob, dev) = (alice(), key(22), dev());
    let base = chain.next_base_price();
    let tip = u256::from(5u64);
    let fee = FeeOffer {
        tip,
        ..offer(&chain, 100_000)
    };
    let tx = signed(&chain, &alice, vec![transfer(&bob, 100)], fee).remove(0);
    let intrinsic = chain.fees().unwrap().schedule().intrinsic(&tx);
    let (before, producer_before) = (balance(&chain, &alice), balance(&chain, &dev));
    let (block, receipts) = advance(&mut chain, vec![tx]);
    let receipt = &receipts[0];
    let paid = u256::from(intrinsic) * (base + tip);
    let burned = u256::from(intrinsic) * base / u256::from(2u64);

    assert_eq!(receipt.result, Ok(Effect::Done));
    assert_eq!((receipt.gas_used, receipt.fee), (intrinsic, paid));
    assert_eq!((block.header.gas_used, block.header.base_price), (intrinsic, base));
    // Only what it used is gone; the rest of its 100,000-gas reserve came back.
    assert_eq!(balance(&chain, &alice), before - u256::from(100u64) - paid);
    // The producer keeps the tip and half the base part; the other half is burned.
    assert_eq!(balance(&chain, &dev), producer_before + paid - burned);

    let stored = chain.receipt(&receipt.id).unwrap().unwrap();

    assert_eq!((stored.gas_used, stored.fee), (intrinsic, paid));
}

#[test]
fn failed_transactions_still_pay() {
    let mut chain = open(FeeConfig::dev());
    let alice = alice();
    let burn = Tx::Burn {
        from: alice.public().address(),
        amount: Amount::U128(u128::from(COIN) * 2),
    };
    let before = balance(&chain, &alice);
    let txs = signed(&chain, &alice, vec![burn], offer(&chain, 100_000));
    let (_, receipts) = advance(&mut chain, txs);

    assert!(receipts[0].result.is_err());
    assert!(receipts[0].fee > u256::ZERO);
    assert_eq!(balance(&chain, &alice), before - receipts[0].fee);
}

#[test]
fn contracts_pay_for_their_fuel_up_to_the_gas_limit() {
    let mut chain = open(FeeConfig::dev());
    let alice = alice();
    let deploy = Tx::Deploy {
        code: Code::Lua(SPINNER.to_string()),
    };
    let contract = contract_address(&alice.public().address(), nonce(&chain, &alice), chain.hash());
    let txs = signed(&chain, &alice, vec![deploy], offer(&chain, 1_000_000));
    let (_, receipts) = advance(&mut chain, txs);

    assert_eq!(receipts[0].result, Ok(Effect::Deployed(contract)));

    let spin = |turns: f64| Tx::ContractCall {
        to: contract,
        func: Hash::Xxh3_64(selector("spin")),
        amount: Amount::U128(0),
        data: vec![TxCallArgumentType::Number(turns)],
    };
    let call = signed(&chain, &alice, vec![spin(100.0)], offer(&chain, 1_000_000)).remove(0);
    let intrinsic = chain.fees().unwrap().schedule().intrinsic(&call);
    let (_, receipts) = advance(&mut chain, vec![call]);

    // One gas for each fuel, on top of the intrinsic cost.
    assert!(receipts[0].result.is_ok());
    assert!(receipts[0].fuel_used > 0);
    assert_eq!(receipts[0].gas_used, intrinsic + receipts[0].fuel_used);

    // A gas limit with room for little fuel stops the contract there, and pays all of it.
    let tight = signed(&chain, &alice, vec![spin(1_000_000.0)], offer(&chain, intrinsic + 50)).remove(0);
    let (_, receipts) = advance(&mut chain, vec![tight]);

    assert!(matches!(&receipts[0].result, Err(Failure::Contract(crate::vm::VmError::OutOfFuel))));
    assert_eq!(receipts[0].gas_used, intrinsic + 50);
}

#[test]
fn transactions_that_cannot_pay_are_left_out_and_make_a_block_invalid() {
    let mut chain = open(FeeConfig::dev());
    let (alice, poor) = (alice(), key(23));
    let good = offer(&chain, 100_000);
    let cases = [
        (alice.clone(), FeeOffer { gas_limit: 10, ..good }, "costs"),
        (alice.clone(), FeeOffer { max_price: u256::ONE, ..good }, "base price"),
        (poor.clone(), good, "cannot pay"),
    ];

    for (sender, fee, expected) in cases {
        let tx = signed(&chain, &sender, vec![transfer_from(&sender, &key(22), 1)], fee).remove(0);

        match forced(&mut chain, vec![tx.clone()], |_| {}) {
            Err(ImportError::Invalid(BlockError::Tx { index: 0, error })) => {
                assert!(error.to_string().contains(expected), "{error}")
            }
            other => panic!("expected {expected}, got {other:?}"),
        }

        let (block, _) = advance(&mut chain, vec![tx]);

        assert!(block.txs.is_empty(), "building leaves out what cannot pay: {expected}");
    }
}

#[test]
fn blocks_hold_no_more_than_their_gas_limit() {
    let mut chain = open(FeeConfig {
        block_gas_limit: 10_000,
        ..FeeConfig::dev()
    });
    let alice = alice();
    let txs = signed(&chain, &alice, (1..=3).map(|amount| transfer(&key(22), amount)).collect(), offer(&chain, 4_000));

    // Two fit; the third's gas limit is more than what is left, so it waits.
    assert!(matches!(
        forced(&mut chain, txs.clone(), |_| {}),
        Err(ImportError::Invalid(BlockError::Tx { index: 2, error: TxError::BlockFull { .. } }))
    ));

    let (block, _) = advance(&mut chain, txs.clone());

    assert_eq!(block.txs, txs[..2]);

    // The header must say what the block used, at the price its parent makes.
    let next = signed(&chain, &alice, vec![transfer(&key(22), 9)], offer(&chain, 4_000));

    assert!(matches!(
        forced(&mut chain, next.clone(), |header| header.gas_used += 1),
        Err(ImportError::Invalid(BlockError::GasUsed { .. }))
    ));
    assert!(matches!(
        forced(&mut chain, next.clone(), |header| header.base_price += u256::ONE),
        Err(ImportError::Invalid(BlockError::BasePrice { .. }))
    ));

    let (block, _) = advance(&mut chain, vec![next[0].clone(), txs[2].clone()]);

    assert_eq!(block.txs.len(), 1, "the third goes in once it fits after its turn");
}

#[test]
fn the_base_price_follows_how_full_blocks_are() {
    let base_fee = BaseFee::dev();
    let start = base_fee.initial;
    let eighth = start / u256::from(8u64);

    // A full block is twice the target, so the price rises by the most it can, an eighth.
    assert_eq!(base_fee.next_price(start, 1_000, 1_000), start + eighth);
    assert_eq!(base_fee.next_price(start, 500, 1_000), start);
    assert_eq!(base_fee.next_price(start, 0, 1_000), start - eighth);
    assert_eq!(base_fee.next_price(base_fee.min, 0, 1_000), base_fee.min, "never under the min");
    assert_eq!(BaseFee { min: u256::ONE, ..base_fee }.next_price(u256::ONE, 1_000, 1_000), u256::from(2u64), "always up by 1 at least");

    // On a chain: empty blocks bring it down, and blocks over the target push it up.
    let mut chain = open(FeeConfig {
        block_gas_limit: 25_000,
        ..FeeConfig::dev()
    });
    let alice = alice();
    let genesis_price = chain.tip().base_price;
    advance(&mut chain, Vec::new());

    assert_eq!(chain.tip().base_price, genesis_price - genesis_price / u256::from(8u64));

    let busy = signed(&chain, &alice, (1..=6).map(|amount| transfer(&key(22), amount)).collect(), offer(&chain, 4_000));
    let (block, _) = advance(&mut chain, busy);

    assert!(block.header.gas_used > 12_500, "over the target");
    assert!(chain.next_base_price() > block.header.base_price);
}

#[test]
fn bidders_pay_their_whole_bid() {
    let fees = FeeConfig {
        pricing: PricingConfig::Bid(Bid::dev()),
        ..FeeConfig::dev()
    };
    let mut chain = open(fees);
    let (alice, dev) = (alice(), dev());
    let minimum = chain.next_base_price();
    let bid = minimum * u256::from(3u64);
    let fee = FeeOffer {
        gas_limit: 100_000,
        max_price: bid,
        tip: u256::from(123u64),
    };
    let producer_before = balance(&chain, &dev);
    let txs = signed(&chain, &alice, vec![transfer(&key(22), 1)], fee);
    let (_, receipts) = advance(&mut chain, txs);
    let gas = u256::from(receipts[0].gas_used);

    assert_eq!(minimum, Bid::dev().min_price);
    assert_eq!(receipts[0].fee, gas * bid, "the tip means nothing to a bid");
    // The producer gets everything above the minimum, and half of the minimum's part.
    assert_eq!(balance(&chain, &dev), producer_before + gas * (bid - minimum) + gas * minimum / u256::from(2u64));

    let low = FeeOffer {
        max_price: minimum - u256::ONE,
        ..fee
    };

    let txs = signed(&chain, &alice, vec![transfer(&key(22), 1)], low);

    assert!(matches!(
        forced(&mut chain, txs, |_| {}),
        Err(ImportError::Invalid(BlockError::Tx { error: TxError::PriceTooLow { .. }, .. }))
    ));
}

#[test]
fn a_scorer_contract_decides_what_is_charged() {
    let alice = alice();
    // Alice's second transaction, after one the plain weights still score, deploys the scorer.
    let scorer = contract_address(&alice.public().address(), 1, HashType::default_chain());
    let fees = FeeConfig {
        schedule: ScheduleConfig::Contract(ContractScorer {
            weights: Weighted::dev(),
            contract: scorer,
            fuel: 100_000,
            kinds: Vec::new(),
        }),
        ..FeeConfig::dev()
    };
    let mut chain = open(fees);
    let charged = |receipt: &Receipt, block: &Block| receipt.fee / block.header.base_price;
    let early = signed(&chain, &alice, vec![transfer(&key(22), 1)], offer(&chain, 100_000));
    let (block, receipts) = advance(&mut chain, early);

    assert_eq!(charged(&receipts[0], &block), u256::from(receipts[0].gas_used), "no scorer yet");

    let deploy = Tx::Deploy {
        code: Code::Lua(SCORER.to_string()),
    };
    let txs = signed(&chain, &alice, vec![deploy], offer(&chain, 1_000_000));
    let (_, receipts) = advance(&mut chain, txs);

    assert_eq!(receipts[0].result, Ok(Effect::Deployed(scorer)));

    // Transfers at half; a burn charged to its whole gas limit and no further; a mint the scorer
    // fails on, as weighed. Blocks count the gas used all the same.
    let burn = Tx::Burn {
        from: alice.public().address(),
        amount: Amount::U128(1),
    };
    let txs = signed(&chain, &alice, vec![transfer(&key(22), 1), burn], offer(&chain, 100_000));
    let (block, receipts) = advance(&mut chain, txs);

    assert_eq!(charged(&receipts[0], &block), u256::from(receipts[0].gas_used / 2));
    assert_eq!(charged(&receipts[1], &block), u256::from(100_000u64));
    assert_eq!(block.header.gas_used, receipts[0].gas_used + receipts[1].gas_used);

    let mint = Tx::Mint {
        to: alice.public().address(),
        amount: Amount::U128(1),
    };
    let txs = signed(&chain, &dev(), vec![mint], offer(&chain, 100_000));
    let (block, receipts) = advance(&mut chain, txs);

    assert_eq!(receipts[0].result, Ok(Effect::Done));
    assert_eq!(charged(&receipts[0], &block), u256::from(receipts[0].gas_used));
}

#[test]
fn a_scorer_is_only_called_for_the_kinds_it_names() {
    let alice = alice();
    let scorer = contract_address(&alice.public().address(), 0, HashType::default_chain());
    let fees = FeeConfig {
        schedule: ScheduleConfig::Contract(ContractScorer {
            weights: Weighted::dev(),
            contract: scorer,
            fuel: 100_000,
            kinds: vec!["transfer".to_string()],
        }),
        ..FeeConfig::dev()
    };
    let mut chain = open(fees);
    let charged = |receipt: &Receipt, block: &Block| receipt.fee / block.header.base_price;
    let deploy = Tx::Deploy {
        code: Code::Lua(SCORER.to_string()),
    };
    let txs = signed(&chain, &alice, vec![deploy], offer(&chain, 1_000_000));
    advance(&mut chain, txs);

    // The scorer halves transfers; it would charge a burn a billionfold, but burns are not its
    // to score, so the weights alone count.
    let burn = Tx::Burn {
        from: alice.public().address(),
        amount: Amount::U128(1),
    };
    let txs = signed(&chain, &alice, vec![transfer(&key(22), 1), burn], offer(&chain, 100_000));
    let (block, receipts) = advance(&mut chain, txs);

    assert_eq!(charged(&receipts[0], &block), u256::from(receipts[0].gas_used / 2));
    assert_eq!(charged(&receipts[1], &block), u256::from(receipts[1].gas_used));

    // Kinds must be kinds, each once.
    for (kinds, expected) in [(vec!["calls".to_string()], "kinds of transaction"), (vec!["call".to_string(), "call".to_string()], "twice")] {
        let bad = FeeConfig {
            schedule: ScheduleConfig::Contract(ContractScorer {
                weights: Weighted::dev(),
                contract: scorer,
                fuel: 100_000,
                kinds,
            }),
            ..FeeConfig::dev()
        };

        match Chain::open(Arc::new(MemoryDb::default()), &genesis(bad)) {
            Err(ChainError::BadGenesis(why)) => assert!(why.contains(expected), "{why}"),
            Err(err) => panic!("expected {expected}, got {err}"),
            Ok(_) => panic!("expected {expected}, got a chain"),
        }
    }
}

#[test]
fn fees_split_their_base_part_and_keep_the_rest_for_the_producer() {
    let fees = FeeConfig {
        burn_permille: 250,
        ..FeeConfig::dev()
    }
    .build();
    let settled = fees.settle(u256::from(10u64), u256::from(15u64), 120, 100);

    assert_eq!(settled.fee, u256::from(1_500u64));
    assert_eq!(settled.burned, u256::from(250u64), "a quarter of the base part, 100 gas at 10");
    assert_eq!(settled.to_producer, u256::from(1_250u64));
    assert_eq!(settled.gas_used, 120);
    assert_eq!(fees.priority(u256::from(10u64), &FeeOffer {
        gas_limit: 1,
        max_price: u256::from(40u64),
        tip: u256::from(7u64),
    }), u256::from(7u64));
}

#[test]
fn fee_settings_that_cannot_work_are_refused() {
    let dev = FeeConfig::dev();
    let base_fee = BaseFee::dev();
    let weights = Weighted::dev();
    let cases = [
        (FeeConfig { block_gas_limit: 0, ..dev.clone() }, "block_gas_limit"),
        (FeeConfig { burn_permille: 1_001, ..dev.clone() }, "burn_permille"),
        (FeeConfig { schedule: ScheduleConfig::Weighted(Weighted { fuel_per_gas: 0, ..weights }), ..dev.clone() }, "fuel_per_gas"),
        (FeeConfig { pricing: PricingConfig::BaseFee(BaseFee { min: u256::ZERO, ..base_fee }), ..dev.clone() }, "never rise"),
        (FeeConfig { pricing: PricingConfig::BaseFee(BaseFee { initial: u256::ONE, ..base_fee }), ..dev.clone() }, "at least its min"),
        (FeeConfig { pricing: PricingConfig::BaseFee(BaseFee { elasticity: 0, ..base_fee }), ..dev.clone() }, "elasticity"),
        (
            FeeConfig {
                schedule: ScheduleConfig::Contract(ContractScorer { weights, contract: alice().public().address(), fuel: 1, kinds: Vec::new() }),
                ..dev.clone()
            },
            "contract address",
        ),
    ];

    for (fees, expected) in cases {
        match Chain::open(Arc::new(MemoryDb::default()), &genesis(fees)) {
            Err(ChainError::BadGenesis(why)) => assert!(why.contains(expected), "{why}"),
            Err(err) => panic!("expected {expected}, got {err}"),
            Ok(_) => panic!("expected {expected}, got a chain"),
        }
    }
}
