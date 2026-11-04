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
        standards: crate::registry::Standards {
            tokens: crate::chain::standard_token_functions().into(),
            collections: None,
        },
        staking: None,
        signers: None,
        governance: None,
        fees: Some(Arc::new(FeeConfig::dev().build())),
        pooled_fees: false,
        anchoring: Default::default(),
        names: None,
        vm: crate::params::vm::VmRules::default(),
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
        timestamp: 1,
        producer: producer().public().address(),
        base_price: FeeConfig::dev().build().pricing().genesis_price(),
        genesis: chain(),
    }
}

/// The chain the blocks here run on, by the genesis id their context names.
fn chain() -> crate::structs::hash::Hash {
    crate::crypto::hasher::Hasher::digest(crate::structs::hash::HashType::default_chain(), b"the chain these blocks run on")
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
    let sign = |tx: Tx, nonce: u64, fee: FeeOffer, keypair: &Keypair| SignedTx::sign(tx, nonce, fee, keypair, &chain());

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

/// Batches that conflict with each other and with what is beside them: a contract and a recipient
/// shared, a batch whose later step needs what one before it in the block paid, one that fails, and
/// two deploys in one.
fn tangled_batches() -> Vec<SignedTx> {
    let hash = HashType::default_chain();
    let counter = contract_address(&key(6).public().address(), 0, hash);
    let fee = FeeOffer { gas_limit: 2_000_000, max_price: u256::from(2_000_000_000u64), tip: u256::from(3u64) };
    let pay = |from: u8, to: Address, amount: u128| Tx::Transfer { from: key(from).public().address(), to, amount: Amount::U128(amount) };
    let call = |function: &str| Tx::ContractCall { to: counter, func: Hash::Xxh3_64(selector(function)), amount: Amount::U128(0), data: Vec::new() };
    let deploy = || Tx::Deploy { code: Code::Lua(COUNTER.to_string()) };
    let fresh = |seed: u8| key(seed).public().address();
    let batch = |steps: Vec<Tx>, nonce: u64, keypair: &Keypair| SignedTx::sign(Tx::Batch { steps }, nonce, fee, keypair, &chain());
    // More than a sender holds until the batch before it in the block pays it.
    let plenty = 2 * 10u128.pow(18);

    vec![
        batch(vec![deploy(), call("increment")], 0, &key(6)),
        batch(vec![call("increment"), pay(7, fresh(101), 5), call("increment")], 0, &key(7)),
        batch(vec![pay(1, key(2).public().address(), 10u128.pow(18) / 2), pay(1, fresh(102), 1)], 0, &key(1)),
        batch(vec![pay(2, fresh(103), 10u128.pow(18) + 1), call("increment")], 0, &key(2)),
        // Nothing in common with any of them.
        SignedTx::sign(pay(3, fresh(104), 1), 0, fee, &key(3), &chain()),
        // Fails at its second step, and its first does not stand.
        batch(vec![call("increment"), pay(8, fresh(105), plenty)], 0, &key(8)),
        batch(vec![call("increment"), deploy(), deploy()], 0, &key(9)),
        batch(vec![pay(4, producer().public().address(), 5), pay(4, key(5).public().address(), 6)], 0, &key(4)),
        batch(vec![pay(5, fresh(106), 7)], 0, &key(5)),
    ]
}

#[test]
fn batches_run_side_by_side_as_they_would_one_by_one() {
    let rules = rules();
    let vm = Arc::new(Vm::new(&[Engine::Lua], rules.hash).unwrap());
    let workers = Workers::new();
    let runner = Runner { rules: &rules, vm: &vm, workers: &workers };
    let db = funded();
    let txs = tangled_batches();
    let reference = Arc::new(State::new(db.clone()));
    let expected = one_by_one(&rules, &vm, &reference, &txs, &context(), Unfit::Skip).unwrap();
    let expected_state = reference.take_changes();

    assert_eq!(expected.kept.len(), txs.len());
    assert!(expected.receipts[3].result.is_ok(), "paid by the batch before it: {:?}", expected.receipts[3].result);
    assert!(matches!(&expected.receipts[5].result, Err(crate::chain::Failure::Step { step: 1, .. })), "{:?}", expected.receipts[5].result);
    assert!(expected.receipts[6].result.is_ok(), "{:?}", expected.receipts[6].result);

    for _ in 0..5 {
        let layer = Arc::new(State::new(db.clone()));
        let ran = runner.run(&layer, &txs, &context(), Unfit::Skip, None).unwrap();

        assert!(ran.again.is_empty(), "every dependency here is one a step names: {:?} ran again", ran.again);
        assert_eq!(ran.kept, expected.kept);
        assert_eq!(ran.receipts, expected.receipts);
        assert_eq!(ran.gas_used, expected.gas_used);
        assert_eq!(layer.take_changes(), expected_state);
    }
}

#[test]
fn a_batch_touches_what_all_its_steps_touch_each_once() {
    let hash = HashType::default_chain();
    let me = key(1).public().address();
    let (a, b, contract) = (key(2).public().address(), key(3).public().address(), Address::Contract([4; 32]));
    let steps = vec![
        Tx::Transfer { from: me, to: a, amount: Amount::U128(1) },
        Tx::Transfer { from: me, to: a, amount: Amount::U128(1) },
        Tx::ContractCall { to: contract, func: Hash::Xxh3_64(selector("x")), amount: Amount::U128(0), data: Vec::new() },
        Tx::Claim { height: 1, to: b, earned: u256::ONE, index: 0, path: Vec::new() },
        Tx::Name { name: "alice".to_string(), action: crate::registry::names::NameAction::Renew { years: 1 } },
        Tx::Deploy { code: Code::Lua(COUNTER.to_string()) },
        Tx::Deploy { code: Code::Lua(COUNTER.to_string()) },
        #[cfg(feature = "evm")]
        Tx::Deploy { code: Code::Evm(vec![0]) },
    ];
    let touched = touches_of(&Tx::Batch { steps: steps.clone() }, me, 7, hash);
    let of = |address: Address| Touch::Address(address);
    #[cfg_attr(not(feature = "evm"), allow(unused_mut))]
    let mut wanted = vec![
        of(me),
        of(a),
        of(contract),
        of(b),
        Touch::Pools,
        Touch::Names,
        Touch::Governance,
        of(contract_address(&me, 7, hash)),
        of(contract_address(&me, batch_deploy_nonce(7, 1), hash)),
    ];

    #[cfg(feature = "evm")]
    wanted.push(of(crate::vm::evm::create_address(&me, batch_deploy_nonce(7, 2))));

    assert_eq!(touched, wanted);

    // As a multisig's, as the multisig.
    let multisig = Address::Multisig([6; 32]);
    let carried = Tx::Multisig { account: multisig, tx: Box::new(Tx::Batch { steps: vec![Tx::Burn { from: multisig, amount: Amount::U128(1) }, Tx::Transfer { from: multisig, to: a, amount: Amount::U128(1) }] }), signatures: Vec::new() };

    assert_eq!(touches_of(&carried, multisig, 0, hash), [of(multisig), of(a)]);
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
        SignedTx::sign(Tx::Deploy { code: Code::Lua(PAYER.to_string()) }, 0, fee, &key(6), &chain()),
        // Waits for the deploy, which it names, then pays key 5, which it does not.
        SignedTx::sign(pay, 0, fee, &key(7), &chain()),
        // Names nothing the others do, so runs at once, on key 5's balance from before the payment.
        // The payment goes in first and changes that balance, so this runs again on the new one.
        SignedTx::sign(spend, 0, fee, &key(5), &chain()),
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

/// Keeps members in a namespace and counts them by iterating it; or adds one to another such
/// contract, which the transaction calling it never names.
const MEMBERS: &str = r#"
local M = {}
local members = storage.namespace("members")

function M.join()
    members:set(ctx.sender:text(), "1")
end

function M.count()
    local count = 0
    for _ in members:iter() do
        count = count + 1
    end
    storage.set("count", tostring(count))
    return count
end

function M.relay(club)
    contracts.call(club, "join")
end

return M
"#;

#[test]
fn a_scan_another_transaction_wrote_into_runs_again() {
    let rules = rules();
    let vm = Arc::new(Vm::new(&[Engine::Lua], rules.hash).unwrap());
    let workers = Workers::new();
    let runner = Runner {
        rules: &rules,
        vm: &vm,
        workers: &workers,
    };
    let (club, relay) = (contract_address(&key(6).public().address(), 0, rules.hash), contract_address(&key(6).public().address(), 1, rules.hash));
    let fee = FeeOffer {
        gas_limit: 1_000_000,
        max_price: u256::from(2_000_000_000u64),
        tip: u256::ZERO,
    };
    let call = |to: Address, function: &str, data: Vec<TxCallArgumentType>| Tx::ContractCall { to, func: Hash::Xxh3_64(selector(function)), amount: Amount::U128(0), data };
    let txs = vec![
        SignedTx::sign(Tx::Deploy { code: Code::Lua(MEMBERS.to_string()) }, 0, fee, &key(6), &chain()),
        SignedTx::sign(Tx::Deploy { code: Code::Lua(MEMBERS.to_string()) }, 1, fee, &key(6), &chain()),
        // Names the relay, and joins the club through it: a key the club never had.
        SignedTx::sign(call(relay, "relay", vec![TxCallArgumentType::Address(club)]), 0, fee, &key(7), &chain()),
        // Names the club, so runs beside the relay, counting no members. It read no key the join
        // wrote, as none was there, but its scan covered it, so it runs again and counts one.
        SignedTx::sign(call(club, "count", Vec::new()), 0, fee, &key(8), &chain()),
    ];
    let db = funded();
    let reference = Arc::new(State::new(db.clone()));
    let expected = one_by_one(&rules, &vm, &reference, &txs, &context(), Unfit::Skip).unwrap();
    let layer = Arc::new(State::new(db));
    let ran = runner.run(&layer, &txs, &context(), Unfit::Skip, None).unwrap();

    assert!(ran.again.contains(&3), "the count ran again: {:?}", ran.again);
    assert_eq!(ran.receipts, expected.receipts);
    assert_eq!(layer.take_changes(), reference.take_changes());
    assert_eq!(reference.get(crate::db::Column::Storage, &crate::db::storage_key(&club, b"count")).unwrap(), None, "taken already");
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
