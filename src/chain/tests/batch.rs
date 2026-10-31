//! Batches through the chain: several steps under one signature, nonce and fee offer, all or
//! nothing, and what they may not hold. EVM steps are in evm/batch.rs.

use {
    super::*,
    crate::crypto::verifiable::{Verifiable, VerifyError},
    crate::registry::multisig::{MultisigBody, MultisigMember},
    crate::structs::address::multisig_address,
    crate::structs::receipt::{StepEffect, StepOutcome},
    crate::structs::tx::{MAX_BATCH_STEPS, MemberSignature, batch_deploy_nonce},
    crate::vm::VmError,
};

fn me() -> Address {
    authority().public().address()
}

fn to(seed: u8) -> Address {
    Keypair::from_seed([seed; 32]).public().address()
}

fn pay(from: Address, seed: u8, amount: u128) -> Tx {
    Tx::Transfer { from, to: to(seed), amount: Amount::U128(amount) }
}

fn call(contract: Address, function: &str, data: Vec<TxCallArgumentType>) -> Tx {
    Tx::ContractCall { to: contract, func: Hash::Xxh3_64(selector(function)), amount: Amount::U128(0), data }
}

fn deploy(source: &str) -> Tx {
    Tx::Deploy { code: Code::Lua(source.to_string()) }
}

fn batch(chain: &Chain, steps: Vec<Tx>) -> SignedTx {
    SignedTx::sign(Tx::Batch { steps }, nonce(chain, &authority()), FeeOffer::NONE, &authority())
}

/// Deploys the counter, and returns where it landed.
fn counter(chain: &mut Chain) -> Address {
    let at = contract_address(&me(), nonce(chain, &authority()), chain.hash());
    let deployed = SignedTx::sign(deploy(COUNTER), nonce(chain, &authority()), FeeOffer::NONE, &authority());

    assert_eq!(advance(chain, vec![deployed])[0].result, Ok(Effect::Deployed(at)));

    at
}

fn count(chain: &Chain, contract: &Address) -> Option<String> {
    chain.state().get(crate::db::Column::Storage, &crate::db::storage_key(contract, b"count")).unwrap().map(|bytes| String::from_utf8(bytes).unwrap())
}

fn refused(chain: &mut Chain, tx: SignedTx, why: &str) {
    let (block, now) = forced_block(chain, vec![tx]);

    match chain.import(&block, now) {
        Err(ImportError::Invalid(BlockError::Tx { error, .. })) => assert!(error.to_string().contains(why), "{why}: {error}"),
        other => panic!("{why}: expected the block refused, got {other:?}"),
    }
}

#[test]
fn every_step_of_a_batch_lands_under_one_nonce_and_says_what_it_did() {
    let mut chain = open(&Arc::new(MemoryDb::default()));
    let contract = counter(&mut chain);
    let before = balance(&chain, &me());
    let at = nonce(&chain, &authority());
    let steps = vec![
        pay(me(), 1, 10),
        call(contract, "increment", vec![TxCallArgumentType::I64(2)]),
        Tx::Burn { from: me(), amount: Amount::U128(5) },
        pay(me(), 2, 20),
    ];
    let signed = batch(&chain, steps);
    let receipt = advance(&mut chain, vec![signed.clone()]).remove(0);

    let Ok(Effect::Batch(outcomes)) = &receipt.result else {
        panic!("{:?}", receipt.result);
    };

    assert_eq!(outcomes.len(), 4);
    assert_eq!(outcomes[0].effect, StepEffect::Done);
    assert!(matches!(&outcomes[1].effect, StepEffect::Returned(values) if values.len() == 1), "{:?}", outcomes[1]);
    assert_eq!(nonce(&chain, &authority()), at + 1, "one nonce, whatever it holds");
    assert_eq!(balance(&chain, &to(1)), u256::new(10));
    assert_eq!(balance(&chain, &to(2)), u256::new(20));
    assert_eq!(balance(&chain, &me()), before - u256::new(35));
    assert_eq!(count(&chain, &contract).as_deref(), Some("2"));
    assert!(matches!(chain.receipt(&signed.id(chain.hash())).unwrap().unwrap().status, TxStatus::Batch(steps) if steps.len() == 4));
}

#[test]
fn the_second_of_three_failing_undoes_all_three_and_says_which_it_was() {
    let db = Arc::new(MemoryDb::default());
    let paid = Genesis::dev_authority(authority().public());
    let mut chain = Chain::open(db, &paid).unwrap();
    let fees = paid.fees.clone().unwrap().build();
    let offer = FeeOffer { gas_limit: 3_000_000, max_price: u256::new(1_000_000_000), tip: u256::ZERO };
    // Alice sends, so what she pays stays apart from what the producer earns.
    let alice = Keypair::from_seed([21; 32]);
    let me = alice.public().address();
    let sign = |tx: Tx, nonce: u64, keypair: &Keypair| SignedTx::sign(tx, nonce, offer, keypair);
    let coin = u256::from(10u64).pow(19);
    let funded = sign(Tx::Transfer { from: self::me(), to: me, amount: Amount::U256(coin) }, nonce(&chain, &authority()), &authority());
    assert!(advance(&mut chain, vec![funded])[0].result.is_ok());
    let contract = contract_address(&me, 0, chain.hash());
    assert!(advance(&mut chain, vec![sign(deploy(COUNTER), 0, &alice)])[0].result.is_ok());
    let increment = |by| call(contract, "increment", vec![TxCallArgumentType::I64(by)]);
    assert!(advance(&mut chain, vec![sign(increment(1), 1, &alice)])[0].result.is_ok());

    let before = balance(&chain, &me);
    let (total, others) = (count(&chain, &contract), (balance(&chain, &to(2)), balance(&chain, &to(3))));
    let too_much = Tx::Transfer { from: me, to: to(2), amount: Amount::U256(before + u256::ONE) };
    let signed = sign(Tx::Batch { steps: vec![increment(5), too_much, pay(me, 3, 7)] }, 2, &alice);
    let intrinsic = fees.schedule().intrinsic(&signed);
    let receipt = advance(&mut chain, vec![signed.clone()]).remove(0);

    let Err(Failure::Step { step: 1, failure }) = &receipt.result else {
        panic!("{:?}", receipt.result);
    };
    assert_eq!(**failure, Failure::Registry(RegistryError::InsufficientBalance));

    // What the first step did went, and the third never ran.
    assert_eq!(count(&chain, &contract), total);
    assert_eq!((balance(&chain, &to(2)), balance(&chain, &to(3))), others);
    assert_eq!(nonce(&chain, &alice), 3, "it used its nonce");
    assert!(receipt.events.is_empty());
    assert!(receipt.payments.is_empty());

    // Its fee, for the fuel the step that did run burned, as any failed transaction pays: all she
    // lost, to the unit.
    assert!(receipt.fuel_used > 0, "the first step ran before the second failed");
    assert_eq!(receipt.gas_used, intrinsic + fees.schedule().for_fuel(receipt.fuel_used));
    assert!(receipt.fee > u256::ZERO);
    assert_eq!(balance(&chain, &me), before - receipt.fee);

    let stored = chain.receipt(&signed.id(chain.hash())).unwrap().unwrap();

    assert!(matches!(&stored.status, TxStatus::BatchFailed { step: 1, why } if why.contains("balance")), "{:?}", stored.status);
    assert_eq!(stored.committed(signed.id(chain.hash())).outcome, CommittedOutcome::BatchFailed { step: 1 }, "which step is proven with it; why is not");
    assert!(stored.committed(signed.id(chain.hash())).outcome.failed());
    assert_eq!(crate::registry::supply::adds_up(&chain.state()).unwrap(), Ok(()), "minted less burned is what is held and bonded");
}

#[test]
fn a_batch_may_hold_only_what_a_user_does_and_is_refused_before_it_runs() {
    let voter = Keypair::from_seed([9; 32]).public();
    let multisig = Address::Multisig([6; 32]);
    let name = |tx: &Tx| crate::structs::json::tx(tx)["type"].as_str().unwrap_or_default().to_string();
    let genesis_id = Hash::zero(HashType::default_chain());
    let vote = |block: u8| {
        let vote = crate::structs::vote::Vote {
            voter,
            source: crate::structs::vote::Checkpoint { epoch: 0, block: Hash::zero(HashType::default_chain()) },
            target: crate::structs::vote::Checkpoint { epoch: 1, block: crate::crypto::hasher::Hasher::digest(HashType::default_chain(), &[block]) },
        };

        Box::new(crate::structs::vote::SignedVote::sign(vote, &Keypair::from_seed([9; 32]), &genesis_id))
    };
    let not_allowed = [
        Tx::Slash { first: vote(1), second: vote(2) },
        Tx::Mint { to: to(1), amount: Amount::U128(1) },
        Tx::Bond { voter, amount: Amount::U128(1) },
        Tx::Unbond { voter },
        Tx::Withdraw { voter },
        Tx::ApproveSigners { version: 0, signers: Vec::new(), threshold: 1, change_threshold: 1 },
        Tx::WithdrawApproval { version: 0 },
        Tx::ApproveChange { version: 0, change: crate::governance::Change::Name("x".to_string()) },
        Tx::WithdrawChange { kind: crate::governance::ChangeKind::Name, version: 0 },
        Tx::Ethereum { raw: Vec::new() },
        Tx::Multisig { account: multisig, tx: Box::new(pay(multisig, 1, 1)), signatures: Vec::new() },
        Tx::CreateMultisig { members: Vec::new(), threshold: 1, testnet: false },
        Tx::ChangeMultisig { members: Vec::new(), threshold: 1, testnet: false },
        Tx::ApproveMultisig { account: multisig, nonce: 0, fee: FeeOffer::NONE, tx: Box::new(pay(multisig, 1, 1)) },
        Tx::WithdrawMultisig { account: multisig, proposal: Hash::zero(HashType::default_chain()) },
        Tx::SetDirectory { directory: Default::default() },
        Tx::HandDirectory { to: None },
        Tx::Batch { steps: vec![pay(me(), 1, 1)] },
    ];
    let mut chain = open(&Arc::new(MemoryDb::default()));

    for step in not_allowed {
        let tx = Tx::Batch { steps: vec![pay(me(), 1, 1), step.clone()] };

        assert_eq!(tx.verify(), Err(VerifyError::BatchStep), "{}", name(&step));

        let signed = batch(&chain, vec![pay(me(), 1, 1), step]);

        assert_eq!(signed.verify(), Err(VerifyError::BatchStep));
        refused(&mut chain, signed, "a batch holds only");
    }
}

#[test]
fn a_batch_holds_one_to_thirty_two_steps() {
    let mut chain = open(&Arc::new(MemoryDb::default()));
    let empty = batch(&chain, Vec::new());

    assert_eq!(empty.verify(), Err(VerifyError::BatchSize));
    refused(&mut chain, empty, "1 to 32 steps");

    let too_many = batch(&chain, vec![pay(me(), 1, 1); MAX_BATCH_STEPS + 1]);

    assert_eq!(too_many.verify(), Err(VerifyError::BatchSize));
    refused(&mut chain, too_many, "1 to 32 steps");

    let most = batch(&chain, vec![pay(me(), 1, 1); MAX_BATCH_STEPS]);
    let receipt = advance(&mut chain, vec![most]).remove(0);

    assert!(matches!(&receipt.result, Ok(Effect::Batch(steps)) if steps.len() == MAX_BATCH_STEPS));
    assert_eq!(balance(&chain, &to(1)), u256::new(MAX_BATCH_STEPS as u128));

    let one = batch(&chain, vec![pay(me(), 1, 1)]);

    assert!(advance(&mut chain, vec![one])[0].result.is_ok());
}

#[test]
fn every_step_must_take_from_the_batchs_sender_or_the_whole_of_it_is_invalid() {
    let mut chain = open(&Arc::new(MemoryDb::default()));
    let victim = to(9);

    for theft in [Tx::Transfer { from: victim, to: me(), amount: Amount::U128(1) }, Tx::Burn { from: victim, amount: Amount::U128(1) }] {
        let signed = batch(&chain, vec![pay(me(), 1, 1), theft]);

        assert_eq!(signed.verify(), Err(VerifyError::NotSender));
        refused(&mut chain, signed, "only the sender's own funds");
    }

    // A dry run takes no signature, so it checks the same: a step never moves another's funds.
    let genesis = genesis();
    let db = Arc::new(MemoryDb::default());
    Chain::open(db.clone(), &genesis).unwrap();
    let vm = Arc::new(Vm::new(&genesis.engines, genesis.hash).unwrap());
    let probe = SignedTx::sign(Tx::Batch { steps: vec![Tx::Transfer { from: victim, to: me(), amount: Amount::U128(1) }] }, 0, FeeOffer::NONE, &authority());
    let ran = dry_run(db, &genesis.rules(), &vm, &probe, 0, u256::ZERO).unwrap();

    assert!(ran.failure.is_some_and(|why| why.contains("only the sender's own funds")));
}

#[test]
fn steps_invalid_in_themselves_make_the_batch_invalid() {
    let mut chain = open(&Arc::new(MemoryDb::default()));
    let nothing = Tx::Transfer { from: me(), to: to(1), amount: Amount::U128(0) };
    let signed = batch(&chain, vec![pay(me(), 1, 1), nothing]);

    assert_eq!(signed.verify(), Err(VerifyError::ZeroAmount));
    refused(&mut chain, signed, "greater than zero");

    let name = Tx::Name { name: "NOT A NAME".to_string(), action: crate::registry::names::NameAction::Register { years: 1 } };

    assert_eq!(batch(&chain, vec![name]).verify(), Err(VerifyError::BadName));
}

#[test]
fn steps_share_one_fuel_limit_and_a_later_one_never_gets_a_fresh_allowance() {
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
    let genesis = genesis();
    let mut limits = genesis.tx_limits;
    limits.fuel = 10_000_000;
    let mut chain = Chain::open(Arc::new(MemoryDb::default()), &Genesis { tx_limits: limits, ..genesis.clone() }).unwrap();
    let spinner_at = contract_address(&me(), 0, chain.hash());
    let deployed = SignedTx::sign(deploy(SPINNER), 0, FeeOffer::NONE, &authority());
    assert!(advance(&mut chain, vec![deployed])[0].result.is_ok());
    let spin = |n: f64| call(spinner_at, "spin", vec![TxCallArgumentType::Number(n)]);

    // What one turn costs, by how far one spin goes.
    let alone = SignedTx::sign(spin(10_000.0), 1, FeeOffer::NONE, &authority());
    let used = advance(&mut chain, vec![alone]).remove(0).fuel_used;
    assert!(used > 1_000);

    // Each of these goes through alone; their limit is for all of them together.
    let turns = (10_000_000.0 / used as f64 * 10_000.0 * 0.55).floor();
    let half = SignedTx::sign(spin(turns), 2, FeeOffer::NONE, &authority());
    let receipt = advance(&mut chain, vec![half]).remove(0);
    assert!(receipt.result.is_ok(), "{:?}", receipt.result);
    assert!(receipt.fuel_used < 10_000_000);
    assert!(receipt.fuel_used * 2 > 10_000_000, "two of them pass the limit: {}", receipt.fuel_used);

    let before = balance(&chain, &to(1));
    let both = SignedTx::sign(Tx::Batch { steps: vec![pay(me(), 1, 1), spin(turns), spin(turns)] }, 3, FeeOffer::NONE, &authority());
    let failed = advance(&mut chain, vec![both]).remove(0);

    let Err(Failure::Step { step: 2, failure }) = &failed.result else {
        panic!("{:?}", failed.result);
    };
    assert_eq!(failed.result.as_ref().err().map(ToString::to_string).map(|why| why.contains("step 3")), Some(true));
    assert_eq!(**failure, Failure::Contract(VmError::OutOfFuel));
    assert!(failed.fuel_used <= 10_000_000, "never past the one limit: {}", failed.fuel_used);
    assert_eq!(balance(&chain, &to(1)), before, "the first step went too");

    // However many steps come after the fuel ran out, a contract step finds none: 32 cheap spins
    // never come to more than the limit either.
    let many = SignedTx::sign(Tx::Batch { steps: vec![spin((turns / 8.0).floor()); MAX_BATCH_STEPS] }, 4, FeeOffer::NONE, &authority());
    let failed = advance(&mut chain, vec![many]).remove(0);

    assert!(failed.result.is_err());
    assert!(failed.fuel_used <= 10_000_000, "{}", failed.fuel_used);
}

#[test]
fn a_step_past_the_memory_limit_fails_the_batch() {
    const HOARDER: &str = r#"
local M = {}

function M.hoard(n)
    return #string.rep("x", n)
end

return M
"#;
    let genesis = genesis();
    let mut limits = genesis.tx_limits;
    limits.memory = 4 * 1024 * 1024;
    let mut chain = Chain::open(Arc::new(MemoryDb::default()), &Genesis { tx_limits: limits, ..genesis.clone() }).unwrap();
    let at = contract_address(&me(), 0, chain.hash());
    let deployed = SignedTx::sign(deploy(HOARDER), 0, FeeOffer::NONE, &authority());
    assert!(advance(&mut chain, vec![deployed])[0].result.is_ok());
    let hoard = |n: f64| call(at, "hoard", vec![TxCallArgumentType::Number(n)]);

    // A fraction of the limit, many times over, is no more than the limit at any one moment.
    let small = SignedTx::sign(Tx::Batch { steps: vec![hoard(1024.0 * 1024.0); 3] }, 1, FeeOffer::NONE, &authority());
    let receipt = advance(&mut chain, vec![small]).remove(0);
    assert!(receipt.result.is_ok(), "{:?}", receipt.result);

    let big = SignedTx::sign(Tx::Batch { steps: vec![pay(me(), 1, 1), hoard(1024.0 * 1024.0), hoard(64.0 * 1024.0 * 1024.0)] }, 2, FeeOffer::NONE, &authority());
    let failed = advance(&mut chain, vec![big]).remove(0);

    assert!(matches!(&failed.result, Err(Failure::Step { step: 2, .. })), "{:?}", failed.result);
    assert_eq!(balance(&chain, &to(1)), u256::ZERO);
}

#[test]
fn steps_emit_no_more_together_than_one_call_may() {
    const EMITTER: &str = r#"
local M = {}

function M.emit(n)
    for i = 1, n do
        events.emit("Tick", i)
    end
end

return M
"#;
    let mut chain = open(&Arc::new(MemoryDb::default()));
    let at = contract_address(&me(), 0, chain.hash());
    assert!(advance(&mut chain, vec![SignedTx::sign(deploy(EMITTER), 0, FeeOffer::NONE, &authority())])[0].result.is_ok());
    let emit = |n: f64| call(at, "emit", vec![TxCallArgumentType::Number(n)]);
    let fits = SignedTx::sign(Tx::Batch { steps: vec![emit(100.0), emit(156.0)] }, 1, FeeOffer::NONE, &authority());
    let receipt = advance(&mut chain, vec![fits]).remove(0);

    assert!(matches!(&receipt.result, Ok(Effect::Batch(steps)) if steps[0].events == 100 && steps[1].events == 156), "{:?}", receipt.result);
    assert_eq!(receipt.events.len(), 256, "in order, each step's after those before it");

    let over = SignedTx::sign(Tx::Batch { steps: vec![emit(200.0), emit(200.0)] }, 2, FeeOffer::NONE, &authority());
    let failed = advance(&mut chain, vec![over]).remove(0);

    assert!(matches!(&failed.result, Err(Failure::Step { step: 1, .. })), "{:?}", failed.result);
    assert!(failed.events.is_empty());
}

#[test]
fn a_batch_of_steps_keeps_their_events_payments_and_trace_in_order() {
    const SPLIT: &str = r#"
local M = {}

function M.pay(to, label)
    events.emit("Paid", label)
    accounts.transfer(to, 1)
end

return M
"#;
    let mut chain = open(&Arc::new(MemoryDb::default()));
    let at = contract_address(&me(), 0, chain.hash());
    assert!(advance(&mut chain, vec![SignedTx::sign(deploy(SPLIT), 0, FeeOffer::NONE, &authority())])[0].result.is_ok());
    let paying = |to_seed: u8, label: &str| Tx::ContractCall {
        to: at,
        func: Hash::Xxh3_64(selector("pay")),
        amount: Amount::U128(1),
        data: vec![TxCallArgumentType::Address(to(to_seed)), TxCallArgumentType::String(label.to_string())],
    };
    let signed = SignedTx::sign(Tx::Batch { steps: vec![paying(1, "first"), pay(me(), 3, 1), paying(2, "second")] }, 1, FeeOffer::NONE, &authority());
    let receipt = advance(&mut chain, vec![signed]).remove(0);

    let Ok(Effect::Batch(outcomes)) = &receipt.result else {
        panic!("{:?}", receipt.result);
    };
    let counts: Vec<(u32, u32)> = outcomes.iter().map(|StepOutcome { events, payments, .. }| (*events, *payments)).collect();

    assert_eq!(counts, [(1, 1), (0, 0), (1, 1)]);
    assert_eq!(receipt.events[0].data, [TxCallArgumentType::String("first".to_string())]);
    assert_eq!(receipt.events[1].data, [TxCallArgumentType::String("second".to_string())]);
    assert_eq!((receipt.payments[0].to, receipt.payments[1].to), (to(1), to(2)));
    assert!(receipt.trace.hops.len() >= 2, "both calls' hops: {:?}", receipt.trace);
}

#[test]
fn two_deploys_in_one_batch_land_apart_where_anyone_can_work_out() {
    let mut chain = open(&Arc::new(MemoryDb::default()));
    let at = nonce(&chain, &authority());
    let hash = chain.hash();
    let first = contract_address(&me(), at, hash);
    let second = contract_address(&me(), batch_deploy_nonce(at, 1), hash);
    let third = contract_address(&me(), batch_deploy_nonce(at, 2), hash);

    assert_eq!(batch_deploy_nonce(at, 0), at, "the first lands where one deploy alone would");
    assert_ne!(first, second);
    assert_ne!(second, third);

    let signed = batch(
        &chain,
        vec![
            deploy(COUNTER),
            // Set up in the same batch, at the address it could be told before signing.
            call(first, "increment", vec![TxCallArgumentType::I64(3)]),
            deploy(COUNTER),
            pay(me(), 1, 1),
            deploy(COUNTER),
            call(third, "increment", vec![TxCallArgumentType::I64(4)]),
        ],
    );
    let receipt = advance(&mut chain, vec![signed]).remove(0);
    let Ok(Effect::Batch(outcomes)) = &receipt.result else {
        panic!("{:?}", receipt.result);
    };

    assert_eq!(outcomes[0].effect, StepEffect::Deployed(first));
    assert_eq!(outcomes[2].effect, StepEffect::Deployed(second));
    assert_eq!(outcomes[4].effect, StepEffect::Deployed(third));
    assert_eq!(count(&chain, &first).as_deref(), Some("3"));
    assert_eq!(count(&chain, &second), None);
    assert_eq!(count(&chain, &third).as_deref(), Some("4"));

    // The next transaction's own deploy lands apart from all of them: no nonce reaches the numbers
    // later deploys in a batch use.
    let next = SignedTx::sign(deploy(COUNTER), at + 1, FeeOffer::NONE, &authority());
    let next_at = contract_address(&me(), at + 1, hash);

    assert!(![first, second, third].contains(&next_at));
    assert_eq!(advance(&mut chain, vec![next])[0].result, Ok(Effect::Deployed(next_at)));

    // A second batch's deploys land apart from the first's too.
    let again = batch(&chain, vec![deploy(COUNTER), deploy(COUNTER)]);
    let receipt = advance(&mut chain, vec![again]).remove(0);
    let Ok(Effect::Batch(outcomes)) = &receipt.result else {
        panic!("{:?}", receipt.result);
    };

    assert_eq!(outcomes[1].effect, StepEffect::Deployed(contract_address(&me(), batch_deploy_nonce(at + 2, 1), hash)));
}

#[test]
fn a_contract_deployed_upgraded_and_named_in_one_batch_stands_as_if_sent_apart() {
    const OWNED: &str = r#"
local M = {}

function M.check_perms(action, sender)
    return true
end

function M.version()
    return 1
end

return M
"#;
    const NEWER: &str = r#"
local M = {}

function M.check_perms(action, sender)
    return true
end

function M.version()
    return 2
end

return M
"#;
    let mut chain = open(&Arc::new(MemoryDb::default()));
    let at = contract_address(&me(), 0, chain.hash());
    let signed = SignedTx::sign(
        Tx::Batch { steps: vec![deploy(OWNED), Tx::Upgrade { contract: at, code: Code::Lua(NEWER.to_string()) }, call(at, "version", Vec::new())] },
        0,
        FeeOffer::NONE,
        &authority(),
    );
    let receipt = advance(&mut chain, vec![signed]).remove(0);
    let Ok(Effect::Batch(outcomes)) = &receipt.result else {
        panic!("{:?}", receipt.result);
    };

    assert_eq!(outcomes[1].effect, StepEffect::Upgraded(at));
    assert!(matches!(&outcomes[2].effect, StepEffect::Returned(values) if format!("{values:?}").contains('2')), "{:?}", outcomes[2]);
}

#[test]
fn a_multisig_sends_a_batch_on_one_bundle_of_its_members_signatures() {
    let members = [Keypair::from_seed([31; 32]), Keypair::from_seed([32; 32]), Keypair::from_seed([33; 32])];
    let mut chain = open(&Arc::new(MemoryDb::default()));
    let create = Tx::CreateMultisig { members: members.iter().map(|key| MultisigMember { key: key.public(), bitcoin: None, evm: None }).collect(), threshold: 2, testnet: false };
    let at = nonce(&chain, &authority());
    assert!(advance(&mut chain, vec![SignedTx::sign(create, at, FeeOffer::NONE, &authority())])[0].result.is_ok());
    let account = multisig_address(&me(), at, chain.hash());
    let funded = transfer(&chain, &authority(), &account, 1_000);
    advance(&mut chain, vec![funded]);

    let steps = Tx::Batch { steps: vec![pay(account, 1, 10), pay(account, 2, 20), deploy(COUNTER)] };
    let body = MultisigBody { account, tx: steps.clone(), nonce: 0, fee: FeeOffer::NONE };
    let genesis_id = chain.genesis();
    let signing = |keys: &[&Keypair]| keys.iter().map(|key| MemberSignature::sign(&body, key, &genesis_id)).collect::<Vec<_>>();
    let wrapped = |signatures| SignedTx::sign(Tx::Multisig { account, tx: Box::new(steps.clone()), signatures }, 0, FeeOffer::NONE, &members[0]);

    assert_eq!(wrapped(signing(&[&members[0], &members[1]])).sender(), account);

    // One signature is not enough, as for anything a multisig sends.
    let short = wrapped(signing(&[&members[0]]));
    refused(&mut chain, short, "");

    let receipt = advance(&mut chain, vec![wrapped(signing(&[&members[0], &members[1]]))]).remove(0);

    assert!(matches!(&receipt.result, Ok(Effect::Batch(outcomes)) if outcomes[2].effect == StepEffect::Deployed(contract_address(&account, 0, chain.hash()))), "{:?}", receipt.result);
    assert_eq!(balance(&chain, &account), u256::new(970));
    assert_eq!(Accounts::new(&chain.state()).get(&account).unwrap().nonce, 1, "the multisig's one nonce");
}

#[test]
fn a_multisigs_batch_takes_from_the_multisig_alone_and_never_the_other_way_round() {
    let account = Address::Multisig([6; 32]);
    let signer = Keypair::from_seed([31; 32]);
    let theft = Tx::Batch { steps: vec![Tx::Transfer { from: signer.public().address(), to: to(1), amount: Amount::U128(1) }] };
    let sent = Tx::Multisig { account, tx: Box::new(theft.clone()), signatures: Vec::new() };

    assert_eq!(sent.verify(), Err(VerifyError::NotSender), "its members never sign another's funds away");
    assert_eq!(Tx::ApproveMultisig { account, nonce: 0, fee: FeeOffer::NONE, tx: Box::new(theft) }.verify(), Err(VerifyError::NotSender));

    let reverse = Tx::Batch { steps: vec![Tx::Multisig { account, tx: Box::new(pay(account, 1, 1)), signatures: Vec::new() }] };

    assert_eq!(reverse.verify(), Err(VerifyError::BatchStep), "a batch holds no multisig's transaction");

    let approved = Tx::ApproveMultisig { account, nonce: 0, fee: FeeOffer::NONE, tx: Box::new(Tx::Batch { steps: vec![pay(account, 1, 1)] }) };

    assert_eq!(approved.verify(), Ok(()), "a member may approve the multisig's own");
}

#[test]
fn a_chain_replays_its_batches_to_the_same_state() {
    let db = Arc::new(MemoryDb::default());
    let mut chain = open(&db);
    let mut blocks = Vec::new();
    let contract = contract_address(&me(), 0, chain.hash());
    let txs = [
        vec![batch(&chain, vec![deploy(COUNTER), call(contract, "increment", vec![TxCallArgumentType::I64(1)]), deploy(COUNTER)])],
        vec![SignedTx::sign(Tx::Batch { steps: vec![pay(me(), 1, 5), call(contract, "nope", Vec::new())] }, 1, FeeOffer::NONE, &authority())],
        vec![SignedTx::sign(Tx::Batch { steps: vec![pay(me(), 2, 7), call(contract, "increment", vec![TxCallArgumentType::I64(1)])] }, 2, FeeOffer::NONE, &authority())],
    ];

    for tx in txs {
        let (block, now) = next_block(&chain, tx);
        extended(chain.import(&block, now));
        blocks.push((block, now));
    }

    let replay_db = Arc::new(MemoryDb::default());
    let mut replay = open(&replay_db);

    for (block, now) in &blocks {
        extended(replay.import(block, *now));
    }

    assert_eq!(replay.tip().id(), chain.tip().id(), "every root in every header came out the same");
    assert_eq!(count(&replay, &contract), count(&chain, &contract));
    assert_eq!(balance(&replay, &to(1)), u256::ZERO, "the failed batch's first step went");
    assert_eq!(balance(&replay, &to(2)), u256::new(7));

    let reopened = open(&replay_db);

    assert_eq!(reopened.tip().id(), chain.tip().id());
}

#[test]
fn everyone_a_batchs_steps_touch_is_listed_in_the_address_index() {
    let (contract, deployed) = (Address::Contract([3; 32]), Address::Contract([4; 32]));
    let signed = SignedTx::sign(
        Tx::Batch {
            steps: vec![
                pay(me(), 1, 1),
                Tx::ContractCall { to: contract, func: Hash::Xxh3_64(selector("transfer")), amount: Amount::U128(0), data: vec![TxCallArgumentType::Address(to(2))] },
                Tx::EvmCall { to: Address::Evm([5; 20]), value: Amount::U128(0), input: Vec::new() },
                Tx::Claim { height: 1, to: to(6), earned: u256::ONE, index: 0, path: Vec::new() },
                Tx::Name { name: "alice".to_string(), action: crate::registry::names::NameAction::Transfer { to: to(7) } },
                deploy(COUNTER),
            ],
        },
        0,
        FeeOffer::NONE,
        &authority(),
    );
    let done = TxStatus::Batch(vec![StepOutcome { effect: StepEffect::Deployed(deployed), events: 0, payments: 0 }]);
    let paid = crate::structs::receipt::Payment { from: contract, to: to(8), amount: u256::ONE };
    let listed = touched(&signed, &done, &[paid]);

    for address in [me(), to(1), contract, to(2), Address::Evm([5; 20]), to(6), to(7), deployed, to(8)] {
        assert!(listed.contains(&address), "{address} in {listed:?}");
    }

    // A batch that failed deployed nothing, and still shows to those it asked to pay.
    let failed = touched(&signed, &TxStatus::BatchFailed { step: 0, why: "no".to_string() }, &[]);

    assert!(failed.contains(&to(1)) && !failed.contains(&deployed));
}

#[test]
fn the_mempool_refuses_what_no_block_could_hold_and_weighs_what_it_takes_whole() {
    use crate::mempool::{Mempool, MempoolError};

    let paid = Genesis::dev_authority(authority().public());
    let chain = Chain::open(Arc::new(MemoryDb::default()), &paid).unwrap();
    let fees = paid.fees.clone().unwrap().build();
    let genesis_id = chain.genesis();
    let offer = FeeOffer { gas_limit: 1_000_000, max_price: u256::new(1_000_000_000_000), tip: u256::ZERO };
    let signed = |steps: Vec<Tx>, offer: FeeOffer| SignedTx::sign(Tx::Batch { steps }, 0, offer, &authority());
    let add = |tx: SignedTx| Mempool::new(10).add(tx, &genesis_id, &chain.state(), Some(&fees));

    assert_eq!(add(signed(Vec::new(), offer)), Err(MempoolError::Invalid(VerifyError::BatchSize)));
    assert_eq!(add(signed(vec![Tx::Mint { to: to(1), amount: Amount::U128(1) }], offer)), Err(MempoolError::Invalid(VerifyError::BatchStep)));
    assert_eq!(add(signed(vec![Tx::Transfer { from: to(9), to: me(), amount: Amount::U128(1) }], offer)), Err(MempoolError::Invalid(VerifyError::NotSender)));

    // Enough gas for one step's cost is not enough for ten's.
    let steps = vec![pay(me(), 1, 1); 10];
    let one = fees.schedule().intrinsic(&SignedTx::sign(pay(me(), 1, 1), 0, offer, &authority()));
    let short = FeeOffer { gas_limit: one, ..offer };

    assert_eq!(add(signed(steps.clone(), short)), Err(MempoolError::BadGasLimit));
    assert_eq!(add(signed(steps, offer)), Ok(()));
}

