use super::*;
use crate::chain::fees::FeeConfig;
use crate::crypto::keypair::Keypair;
use crate::db::{Database, MemoryDb};
use crate::registry::contracts::contract_address;
use crate::structs::address::Address;
use crate::structs::amount::Amount;
use crate::structs::code::{Code, Engine};
use crate::structs::hash::{Hash, HashType};
use crate::structs::tx::{FeeOffer, Tx, TxCallArgumentType};
use crate::vm::{Limits, selector};

const COUNTER: &str = r#"
local M = {}

function M.increment()
    local count = tonumber(storage.get("count") or "0") + 1
    storage.set("count", tostring(count))
    return count
end

return M
"#;

fn key(seed: u8) -> Keypair {
    Keypair::from_seed([seed; 32])
}

fn rules() -> Rules {
    Rules {
        hash: HashType::default_chain(),
        minter: None,
        limits: Limits {
            fuel: 1_000_000,
            memory: 16 * 1024 * 1024,
        },
        token_functions: crate::chain::standard_token_functions().into(),
        staking: None,
        signers: None,
        fees: Some(Arc::new(FeeConfig::dev().build())),
        pooled_fees: false,
    }
}

fn producer() -> Keypair {
    key(30)
}

/// A database where keys 1 to 9 and the producer hold a coin each.
fn funded() -> Arc<dyn Database> {
    let db: Arc<dyn Database> = Arc::new(MemoryDb::default());
    let state = State::new(db.clone());
    let accounts = Accounts::new(&state);

    for keypair in (1..=9).map(key).chain([producer()]) {
        accounts.credit(&keypair.public().address(), u256::from(10u64).pow(18)).unwrap();
    }

    db.commit(state.take_changes()).unwrap();

    db
}

fn context() -> BlockContext {
    BlockContext {
        height: 1,
        producer: producer().public().address(),
        base_price: FeeConfig::dev().build().pricing().genesis_price(),
        genesis: crate::structs::hash::Hash::zero(crate::structs::hash::HashType::default_chain()),
    }
}

/// What running the block's transactions strictly one by one gives, the way blocks always ran:
/// each on the block's layer in turn, the producer credited after each.
fn one_by_one(rules: &Rules, vm: &Arc<Vm>, layer: &Arc<State>, txs: &[SignedTx], context: &BlockContext, unfit: Unfit) -> Result<Ran, String> {
    let runner = Runner {
        rules,
        vm,
        workers: &Workers::new(),
    };
    let mut ran = Ran::default();

    for (index, tx) in txs.iter().enumerate() {
        if let Some(left) = runner.gas_left(ran.gas_used)
            && tx.fee.gas_limit > left
        {
            match unfit {
                Unfit::Refuse => return Err(format!("{index}: block full")),
                Unfit::Skip => continue,
            }
        }

        match execute::execute(layer, tx, rules, vm, context) {
            Ok(executed) => {
                if executed.to_producer > u256::ZERO {
                    Accounts::new(layer).credit(&context.producer, executed.to_producer).unwrap();
                }

                ran.gas_used += executed.receipt.gas_used;
                ran.kept.push(index);
                ran.receipts.push(executed.receipt);
            }
            Err(ExecuteError::Invalid(error)) if unfit == Unfit::Refuse => return Err(format!("{index}: {error}")),
            Err(ExecuteError::Invalid(_)) => {}
            Err(ExecuteError::Db(err)) => panic!("{err}"),
        }
    }

    Ok(ran)
}

/// A block that conflicts every way it can, besides transactions that touch nothing in common.
fn tangled() -> Vec<SignedTx> {
    let hash = HashType::default_chain();
    let counter = contract_address(&key(6).public().address(), 0, hash);
    let small = FeeOffer {
        gas_limit: 100_000,
        max_price: u256::from(2_000_000_000u64),
        tip: u256::from(3u64),
    };
    let large = FeeOffer {
        gas_limit: 1_000_000,
        ..small
    };
    let transfer = |from: &Keypair, to: Address, amount: u128| Tx::Transfer {
        from: from.public().address(),
        to,
        amount: Amount::U128(amount),
    };
    let call = |function: &str| Tx::ContractCall {
        to: counter,
        func: Hash::Xxh3_64(selector(function)),
        amount: Amount::U128(0),
        data: Vec::new(),
    };
    let fresh = |seed: u8| key(seed).public().address();
    let sign = |tx: Tx, nonce: u64, fee: FeeOffer, keypair: &Keypair| SignedTx::sign(tx, nonce, fee, keypair);

    vec![
        // Nothing in common with anything.
        sign(transfer(&key(1), fresh(101), 5), 0, small, &key(1)),
        sign(transfer(&key(2), fresh(102), 5), 0, small, &key(2)),
        // One sender's run: each depends on the nonce before it.
        sign(transfer(&key(3), fresh(103), 1), 0, small, &key(3)),
        sign(transfer(&key(3), fresh(103), 2), 1, small, &key(3)),
        sign(transfer(&key(3), fresh(104), 3), 2, small, &key(3)),
        // A recipient that then sends.
        sign(transfer(&key(4), key(5).public().address(), 7), 0, small, &key(4)),
        sign(transfer(&key(5), fresh(105), 7), 0, small, &key(5)),
        // A deploy, and calls to it in the same block, every one writing what the next reads.
        sign(Tx::Deploy { code: Code::Lua(COUNTER.to_string()) }, 0, large, &key(6)),
        sign(call("increment"), 0, large, &key(7)),
        sign(call("increment"), 0, large, &key(8)),
        sign(call("increment"), 1, large, &key(6)),
        // Into the producer's balance, which every fee credits, and out of it.
        sign(transfer(&key(9), producer().public().address(), 11), 0, small, &key(9)),
        sign(transfer(&producer(), fresh(106), 13), 0, small, &producer()),
        // A call that fails, and still pays.
        sign(call("nope"), 1, large, &key(7)),
        // A nonce used already: it cannot run at all.
        sign(transfer(&key(2), fresh(107), 1), 0, small, &key(2)),
        sign(transfer(&key(1), fresh(108), 9), 1, small, &key(1)),
    ]
}

#[test]
fn running_side_by_side_gives_what_running_one_by_one_does() {
    let rules = rules();
    let vm = Arc::new(Vm::new(&[Engine::Lua], rules.hash).unwrap());
    let workers = Workers::new();
    let runner = Runner {
        rules: &rules,
        vm: &vm,
        workers: &workers,
    };
    let db = funded();
    let txs = tangled();
    let reference = Arc::new(State::new(db.clone()));
    let expected = one_by_one(&rules, &vm, &reference, &txs, &context(), Unfit::Skip).unwrap();
    let expected_state = reference.take_changes();

    // The replayed transaction is the only one left out, and the calls all counted.
    assert_eq!(expected.kept.len(), txs.len() - 1);
    assert!(!expected.kept.contains(&14));
    assert!(expected.receipts[13].result.is_err(), "the call to nope fails");

    // Threads finish in any order, so run it a few times.
    for _ in 0..5 {
        let layer = Arc::new(State::new(db.clone()));
        let ran = runner.run(&layer, &txs, &context(), Unfit::Skip, None).unwrap();

        // Every dependency here is one the transactions name, so each waited for what it
        // depends on and nothing ran twice.
        assert!(ran.again.is_empty(), "{:?} ran again", ran.again);
        assert!(!ran.cut);
        assert_eq!(ran.kept, expected.kept);
        assert_eq!(ran.receipts, expected.receipts);
        assert_eq!(ran.gas_used, expected.gas_used);
        assert_eq!(layer.take_changes(), expected_state);
    }

    // Importing refuses the block at the same transaction, for the same reason.
    let layer = Arc::new(State::new(db.clone()));
    let refused = runner.run(&layer, &txs, &context(), Unfit::Refuse, None).unwrap_err();
    let reference = Arc::new(State::new(db));
    let expected = one_by_one(&rules, &vm, &reference, &txs, &context(), Unfit::Refuse).unwrap_err();

    assert!(matches!(&refused, ImportError::Invalid(BlockError::Tx { index: 14, .. })), "{refused:?}");
    assert!(expected.starts_with("14: nonce 0"), "{expected}");
}

/// Pays whoever it is told, from what it was sent: an account no transaction calling it names.
const PAYER: &str = r#"
local M = {}

function M.pay(to)
    accounts.transfer(to, 1)
end

return M
"#;

#[test]
fn a_contract_reaching_what_nobody_named_makes_only_the_reader_run_again() {
    let rules = rules();
    let vm = Arc::new(Vm::new(&[Engine::Lua], rules.hash).unwrap());
    let workers = Workers::new();
    let runner = Runner {
        rules: &rules,
        vm: &vm,
        workers: &workers,
    };
    let payer = contract_address(&key(6).public().address(), 0, rules.hash);
    let fee = FeeOffer {
        gas_limit: 1_000_000,
        max_price: u256::from(2_000_000_000u64),
        tip: u256::ZERO,
    };
    let pay = Tx::ContractCall {
        to: payer,
        func: Hash::Xxh3_64(selector("pay")),
        amount: Amount::U128(10),
        data: vec![TxCallArgumentType::Address(key(5).public().address())],
    };
    let spend = Tx::Transfer {
        from: key(5).public().address(),
        to: key(105).public().address(),
        amount: Amount::U128(1_000),
    };
    let txs = vec![
        SignedTx::sign(Tx::Deploy { code: Code::Lua(PAYER.to_string()) }, 0, fee, &key(6)),
        // Waits for the deploy, which it names, then pays key 5, which it does not.
        SignedTx::sign(pay, 0, fee, &key(7)),
        // Names nothing the others do, so runs at once, on key 5's balance from before the payment.
        // The payment goes in first and changes that balance, so this runs again on the new one.
        SignedTx::sign(spend, 0, fee, &key(5)),
    ];
    let db = funded();
    let reference = Arc::new(State::new(db.clone()));
    let expected = one_by_one(&rules, &vm, &reference, &txs, &context(), Unfit::Skip).unwrap();
    let layer = Arc::new(State::new(db));
    let ran = runner.run(&layer, &txs, &context(), Unfit::Skip, None).unwrap();

    assert_eq!(ran.again, [2], "only the spend read what the payment wrote");
    assert_eq!(ran.receipts, expected.receipts);
    assert_eq!(layer.take_changes(), reference.take_changes());
}

#[test]
fn workers_keep_the_order_of_what_they_map() {
    let workers = Workers::new();
    let numbers: Vec<u64> = (0..1_000).collect();

    assert_eq!(workers.map(&numbers, |n| n * 2), numbers.iter().map(|n| n * 2).collect::<Vec<_>>());
    assert_eq!(workers.map(&[7u64], |n| n + 1), [8]);
    assert!(workers.map(&[] as &[u64], |n| *n).is_empty());
}

#[test]
fn a_deadline_keeps_what_went_in_by_then_and_leaves_the_rest_out() {
    use std::time::{Duration, Instant};

    let rules = rules();
    let vm = Arc::new(Vm::new(&[Engine::Lua], rules.hash).unwrap());
    let workers = Workers::new();
    let runner = Runner {
        rules: &rules,
        vm: &vm,
        workers: &workers,
    };
    let db = funded();
    let txs = tangled();

    // A deadline already passed: the first transaction to go in is the last.
    let layer = Arc::new(State::new(db.clone()));
    let cut = runner.run(&layer, &txs, &context(), Unfit::Skip, Some(Instant::now())).unwrap();
    let reference = Arc::new(State::new(db.clone()));
    let expected = one_by_one(&rules, &vm, &reference, &txs[..1], &context(), Unfit::Skip).unwrap();

    assert!(cut.cut);
    assert_eq!(cut.kept, [0]);
    assert_eq!(cut.receipts, expected.receipts);
    assert_eq!(cut.gas_used, expected.gas_used);
    assert_eq!(layer.take_changes(), reference.take_changes(), "the state is that of the transactions kept, and nothing of the rest");

    // One far off changes nothing.
    let layer = Arc::new(State::new(db.clone()));
    let whole = runner.run(&layer, &txs, &context(), Unfit::Skip, Some(Instant::now() + Duration::from_secs(3_600))).unwrap();
    let reference = Arc::new(State::new(db));
    let expected = one_by_one(&rules, &vm, &reference, &txs, &context(), Unfit::Skip).unwrap();

    assert!(!whole.cut);
    assert_eq!(whole.kept, expected.kept);
    assert_eq!(layer.take_changes(), reference.take_changes());
}
