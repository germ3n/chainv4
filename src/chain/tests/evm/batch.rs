//! EVM steps in a batch. Each runs as a transaction of its own to the EVM, on the layer the steps
//! before it wrote: so it sees all they did, starts its gas from what they left, and whatever the
//! EVM keeps for one transaction, transient storage, what is warm, refunds and what SELFDESTRUCT
//! counts as new, starts afresh with it.

use {
    super::*,
    crate::structs::receipt::StepEffect,
    crate::structs::tx::batch_deploy_nonce,
    crate::vm::Limits,
};

/// A contract that keeps a word in transient storage when called with any data, and returns what
/// transient storage holds: 1 only in the call that kept it, in one EVM transaction.
fn transient_init() -> Vec<u8> {
    let runtime = crate::structs::hex::from_hex("36601057" /* CALLDATASIZE, PUSH1 0x10, JUMPI */).unwrap();
    let load = crate::structs::hex::from_hex("5b60005c60005260206000f3" /* JUMPDEST, TLOAD(0), MSTORE(0), RETURN(0, 32) */).unwrap();
    let store = crate::structs::hex::from_hex("5b600160005d600456" /* JUMPDEST, TSTORE(0, 1), JUMP back to load */).unwrap();
    let runtime = [runtime, load, store].concat();

    assert_eq!(runtime.len(), 25);

    [crate::structs::hex::from_hex("601980600b6000396000f3" /* CODECOPY the 25 bytes after these 11, RETURN them */).unwrap(), runtime].concat()
}

/// Sends what it holds to its caller and removes itself: a CALLER, SELFDESTRUCT.
fn selfdestruct_init() -> Vec<u8> {
    crate::structs::hex::from_hex("600280600b6000396000f333ff").unwrap()
}

fn of_authority() -> Address {
    authority().public().address()
}

fn evm_step(to: Address, input: Vec<u8>) -> Tx {
    Tx::EvmCall { to, value: Amount::U128(0), input }
}

fn counter_init(start: u128) -> Vec<u8> {
    [crate::structs::hex::from_hex(COUNTER_BIN.trim()).unwrap(), word(start)].concat()
}

fn batch_of(chain: &Chain, steps: Vec<Tx>) -> SignedTx {
    sign(chain, &authority(), Tx::Batch { steps })
}

/// Sends one transaction from the authority in the next block, and says how it went.
fn send(chain: &mut Chain, tx: Tx) -> Receipt {
    let signed = sign(chain, &authority(), tx);

    advance(chain, vec![signed]).remove(0)
}

/// Sends a batch from the authority in the next block, and says how it went.
fn send_batch(chain: &mut Chain, steps: Vec<Tx>) -> Receipt {
    send(chain, Tx::Batch { steps })
}

/// What each step of a batch that went through did.
fn outcomes(receipt: &Receipt) -> Vec<StepEffect> {
    match &receipt.result {
        Ok(Effect::Batch(steps)) => steps.iter().map(|step| step.effect.clone()).collect(),
        other => panic!("expected a batch to go through, got {other:?}"),
    }
}

/// What a step's EVM call returned, as bytes.
fn step_returned(effect: &StepEffect) -> Vec<u8> {
    match effect {
        StepEffect::Returned(values) => match values.as_slice() {
            [TxCallArgumentType::Bytes(bytes)] => bytes.clone(),
            other => panic!("expected bytes, got {other:?}"),
        },
        other => panic!("expected a return, got {other:?}"),
    }
}

#[test]
fn two_evm_deploys_in_one_batch_land_apart_and_the_first_where_it_would_alone() {
    let mut chain = open(&Arc::new(MemoryDb::default()));
    let at = nonce(&chain, &authority());
    let first = evm::create_address(&of_authority(), at);
    let second = evm::create_address(&of_authority(), batch_deploy_nonce(at, 1));
    let signed = batch_of(
        &chain,
        vec![
            Tx::Deploy { code: Code::Evm(counter_init(5)) },
            Tx::Deploy { code: Code::Evm(counter_init(7)) },
            // Set up in the same batch, at the address worked out before signing.
            evm_step(second, calldata("bump(uint256)", &[word(1)])),
        ],
    );
    let receipt = advance(&mut chain, vec![signed]).remove(0);
    let steps = outcomes(&receipt);

    assert_eq!(steps[0], StepEffect::Deployed(first));
    assert_eq!(steps[1], StepEffect::Deployed(second));
    assert_ne!(first, second);
    assert_eq!(count(&mut chain, first), word(5));
    assert_eq!(count(&mut chain, second), word(8));
    assert_eq!(nonce(&chain, &authority()), at + 3, "its one nonce, and the two looks at the counts");
}

#[test]
fn native_and_evm_deploys_share_the_count_of_deploys_before_them() {
    let mut chain = open(&Arc::new(MemoryDb::default()));
    let at = nonce(&chain, &authority());
    let signed = batch_of(&chain, vec![Tx::Deploy { code: Code::Lua(NATIVE_COUNTER.to_string()) }, Tx::Deploy { code: Code::Evm(counter_init(1)) }]);
    let steps = outcomes(&advance(&mut chain, vec![signed]).remove(0));

    assert_eq!(steps[0], StepEffect::Deployed(contract_address(&of_authority(), at, chain.hash())));
    assert_eq!(steps[1], StepEffect::Deployed(evm::create_address(&of_authority(), batch_deploy_nonce(at, 1))));
}

#[test]
fn a_contract_s_create_in_a_later_step_goes_on_from_its_nonce_in_the_step_before() {
    let mut chain = open(&Arc::new(MemoryDb::default()));
    let counter = deploy(&mut chain, 0);
    let spawn = || evm_step(counter, calldata("spawn()", &[]));
    let signed = batch_of(&chain, vec![spawn(), spawn()]);
    let steps = outcomes(&advance(&mut chain, vec![signed]).remove(0));

    assert_eq!(step_returned(&steps[0]), address_word(&evm::create_address(&counter, 1)));
    assert_eq!(step_returned(&steps[1]), address_word(&evm::create_address(&counter, 2)), "the first step's nonce was written back");
}

#[test]
fn several_evm_calls_share_one_gas_allowance_and_a_later_one_gets_only_what_is_left() {
    let paid = Genesis::dev_authority(authority().public());
    let mut chain = Chain::open(Arc::new(MemoryDb::default()), &paid).unwrap();
    let genesis_id = chain.genesis();
    let fees = paid.fees.clone().unwrap().build();
    let high = FeeOffer { gas_limit: 3_000_000, max_price: u256::new(1_000_000_000), tip: u256::ZERO };
    let at = nonce(&chain, &authority());
    let deployed = SignedTx::sign(Tx::Deploy { code: Code::Evm(counter_init(1)) }, at, high, &authority(), &genesis_id);
    let counter = evm::create_address(&of_authority(), at);
    assert!(advance(&mut chain, vec![deployed])[0].result.is_ok());
    let bump = || evm_step(counter, calldata("bump(uint256)", &[word(1)]));
    // Its storage is written already, so every bump from here costs the same.
    let alone = SignedTx::sign(bump(), at + 1, high, &authority(), &genesis_id);
    let used = fees.schedule().for_fuel(advance(&mut chain, vec![alone]).remove(0).fuel_used);
    assert!(used > 21_000, "{used}");

    let two = || Tx::Batch { steps: vec![bump(), bump()] };
    let intrinsic = fees.schedule().intrinsic(&SignedTx::sign(two(), at + 2, high, &authority(), &genesis_id));
    let limited = |limit: u64, nonce: u64| SignedTx::sign(two(), nonce, FeeOffer { gas_limit: limit, ..high }, &authority(), &genesis_id);

    // Room for one and a half: the first goes, the second finds only what it left, and all goes.
    let short = limited(intrinsic + used + used / 2, at + 2);
    let failed = advance(&mut chain, vec![short]).remove(0);

    assert!(matches!(&failed.result, Err(Failure::Step { step: 1, .. })), "{:?}", failed.result);
    assert_eq!(failed.gas_used, intrinsic + used + used / 2, "it pays all it was allowed");
    assert_eq!(evm::storage_word(&chain.state(), bytes_of(&counter), [0; 32]).unwrap().to_vec(), word(2), "the first bump went too");

    // Room for both: what they cost is what one costs, twice.
    let enough = limited(intrinsic + used * 2, at + 3);
    let receipt = advance(&mut chain, vec![enough]).remove(0);

    assert!(receipt.result.is_ok(), "{:?}", receipt.result);
    assert_eq!(fees.schedule().for_fuel(receipt.fuel_used), used * 2);
    assert_eq!(receipt.gas_used, intrinsic + used * 2);
    assert_eq!(evm::storage_word(&chain.state(), bytes_of(&counter), [0; 32]).unwrap().to_vec(), word(4));
}

#[test]
fn native_fuel_spent_in_a_step_is_gone_for_the_evm_step_after_it() {
    const BURNER: &str = r#"
local M = {}

function M.burn(n)
    local total = 0
    for i = 1, n do
        total = total + i
    end
    return total
end

return M
"#;
    let genesis = genesis();
    let db = Arc::new(MemoryDb::default());
    let mut chain = Chain::open(db, &Genesis { tx_limits: Limits { fuel: 3_000_000, ..genesis.tx_limits }, ..genesis.clone() }).unwrap();
    let counter = deploy(&mut chain, 0);
    let burner = deploy_lua(&mut chain, BURNER);
    let burning = |n: f64| Tx::ContractCall { to: burner, func: Hash::Xxh3_64(crate::vm::selector("burn")), amount: Amount::U128(0), data: vec![TxCallArgumentType::Number(n)] };

    // What the EVM step costs, and how far a burn goes, alone.
    let bump = evm_step(counter, calldata("bump(uint256)", &[word(1)]));
    send(&mut chain, bump.clone());
    let bumped = send(&mut chain, bump.clone()).fuel_used;
    let per = send(&mut chain, burning(100_000.0)).fuel_used;
    // A burn that leaves less than a bump: each fits the limit alone, not the two together.
    let turns = ((3_000_000 - bumped / 2) as f64 / per as f64 * 100_000.0).floor();
    let alone = send(&mut chain, burning(turns));
    assert!(alone.result.is_ok() && alone.fuel_used + bumped > 3_000_000, "{:?} {} {bumped}", alone.result, alone.fuel_used);

    let before = count(&mut chain, counter);
    let signed = batch_of(&chain, vec![burning(turns), bump]);
    let failed = advance(&mut chain, vec![signed]).remove(0);

    assert!(matches!(&failed.result, Err(Failure::Step { step: 1, .. })), "{:?}", failed.result);
    assert!(failed.fuel_used <= 3_000_000, "{}", failed.fuel_used);
    assert_eq!(count(&mut chain, counter), before);
}

#[test]
fn a_step_too_expensive_only_at_the_end_undoes_all_those_before_it_and_pays_the_whole_limit() {
    let db = Arc::new(MemoryDb::default());
    let paid = Genesis::dev_authority(authority().public());
    let mut chain = Chain::open(db, &paid).unwrap();
    let genesis_id = chain.genesis();
    let fees = paid.fees.clone().unwrap().build();
    let high = FeeOffer { gas_limit: 3_000_000, max_price: u256::new(1_000_000_000), tip: u256::ZERO };
    let init = counter_init(0);
    let deployed = SignedTx::sign(Tx::Deploy { code: Code::Evm(init) }, 0, high, &authority(), &genesis_id);
    let counter = evm::create_address(&of_authority(), 0);
    assert!(advance(&mut chain, vec![deployed])[0].result.is_ok());
    let bump = || evm_step(counter, calldata("bump(uint256)", &[word(1)]));
    let at = nonce(&chain, &authority());
    let looking = |limit| FeeOffer { gas_limit: limit, ..high };
    let probe = SignedTx::sign(Tx::Batch { steps: vec![bump(), bump()] }, at, looking(1_000_000), &authority(), &genesis_id);
    let intrinsic = fees.schedule().intrinsic(&probe);
    // Enough for the first bump, with its storage to write, and not for the second.
    let limit = intrinsic + 60_000;
    let signed = SignedTx::sign(Tx::Batch { steps: vec![bump(), bump()] }, at, looking(limit), &authority(), &genesis_id);
    let (block, now) = next_block(&chain, vec![signed]);
    let receipt = extended(chain.import(&block, now)).remove(0);

    assert!(matches!(&receipt.result, Err(Failure::Step { step: 1, .. })), "{:?}", receipt.result);
    assert_eq!(receipt.gas_used, limit, "out of gas pays its whole limit");
    assert_eq!(evm::storage_word(&chain.state(), bytes_of(&counter), [0; 32]).unwrap().to_vec(), word(0), "the first bump went too");
    assert_eq!(crate::registry::supply::adds_up(&chain.state()).unwrap(), Ok(()));
}

fn bytes_of(address: &Address) -> [u8; 20] {
    match address {
        Address::Evm(bytes) => *bytes,
        other => panic!("{other} is no EVM contract"),
    }
}

#[test]
fn transient_storage_lasts_a_step_and_is_gone_for_the_next() {
    let mut chain = open(&Arc::new(MemoryDb::default()));
    let at = nonce(&chain, &authority());
    let deployed = sign(&chain, &authority(), Tx::Deploy { code: Code::Evm(transient_init()) });
    let contract = evm::create_address(&of_authority(), at);
    assert_eq!(advance(&mut chain, vec![deployed])[0].result, Ok(Effect::Deployed(contract)));

    let steps = outcomes(&send_batch(&mut chain, vec![evm_step(contract, vec![1]), evm_step(contract, Vec::new())]));

    assert_eq!(step_returned(&steps[0]), word(1), "what the step kept, it reads");
    assert_eq!(step_returned(&steps[1]), word(0), "a step is its own transaction to the EVM");
}

#[test]
fn what_is_warm_and_refunds_start_afresh_with_each_step() {
    let mut chain = open(&Arc::new(MemoryDb::default()));
    let counter = deploy(&mut chain, 3);
    let read = || evm_step(counter, calldata("count()", &[]));
    let one = send(&mut chain, read()).fuel_used;
    let two = send_batch(&mut chain, vec![read(), read()]).fuel_used;

    assert_eq!(two, one * 2, "the second read's storage and contract are cold again, each with its own 21,000");
}

#[test]
fn a_contract_made_in_one_step_is_not_new_to_selfdestruct_in_the_next() {
    let mut chain = open(&Arc::new(MemoryDb::default()));
    let at = nonce(&chain, &authority());
    let contract = evm::create_address(&of_authority(), at);
    let signed = batch_of(&chain, vec![Tx::Deploy { code: Code::Evm(selfdestruct_init()) }, evm_step(contract, Vec::new())]);
    let receipt = advance(&mut chain, vec![signed]).remove(0);

    assert_eq!(outcomes(&receipt)[0], StepEffect::Deployed(contract));
    assert!(
        crate::registry::Contracts::new(&Arc::new(chain.state())).get(&contract).unwrap().is_some(),
        "EIP-6780 removes a contract only in the transaction that made it, and each step is its own"
    );
}

#[test]
fn a_later_step_failing_undoes_an_earlier_evm_write_payment_and_native_call() {
    let db = Arc::new(MemoryDb::default());
    let mut chain = open(&db);
    let (bridge, counter, _) = bridged(&mut chain);
    let first = call(&chain, &authority(), bridge, 0, calldata("add(bytes32,uint64)", &[contract_word(&counter), word(2)]));
    advance(&mut chain, vec![first]);
    let (count_before, total_before, paid_before, mine_before) = (bridge_count(&chain, &bridge), total(&chain, &counter), balance(&chain, &counter), balance(&chain, &of_authority()));
    let other = Address::Ed25519([9; 32]);
    let signed = batch_of(
        &chain,
        vec![
            // An EVM write, a coin payment through it, and a native call it crosses into.
            Tx::EvmCall { to: bridge, value: Amount::U128(300), input: calldata("add(bytes32,uint64)", &[contract_word(&counter), word(5)]) },
            Tx::Transfer { from: of_authority(), to: other, amount: Amount::U128(11) },
            // Fails, once all of that stood.
            evm_step(bridge, calldata("addThenRevert(bytes32,uint64)", &[contract_word(&counter), word(7)])),
        ],
    );
    let receipt = advance(&mut chain, vec![signed]).remove(0);

    assert!(matches!(&receipt.result, Err(Failure::Step { step: 2, failure }) if **failure == Failure::Contract(VmError::Runtime("reverted: changed my mind".to_string()))), "{:?}", receipt.result);
    assert_eq!(bridge_count(&chain, &bridge), count_before, "the EVM's write");
    assert_eq!(total(&chain, &counter), total_before, "the native call it crossed into");
    assert_eq!(balance(&chain, &counter), paid_before, "what it paid on");
    assert_eq!(balance(&chain, &other), u256::ZERO);
    assert_eq!(balance(&chain, &of_authority()), mine_before);
    assert!(receipt.events.is_empty() && receipt.payments.is_empty());
}

#[test]
fn each_step_sees_what_the_one_before_it_wrote_across_engines() {
    let db = Arc::new(MemoryDb::default());
    let mut chain = open(&db);
    let (bridge, counter, _) = bridged(&mut chain);
    let lua = |function: &str, data: Vec<TxCallArgumentType>| Tx::ContractCall { to: counter, func: Hash::Xxh3_64(crate::vm::selector(function)), amount: Amount::U128(0), data };

    // A native write, then the EVM reading it through the bridge.
    let peek = calldata("peek(bytes32,string)", &[contract_word(&counter), word(64), word(5), [b"total".to_vec(), vec![0; 27]].concat()]);
    let steps = outcomes(&send_batch(&mut chain, vec![lua("add", vec![TxCallArgumentType::U64(40)]), evm_step(bridge, peek)]));

    assert_eq!(step_returned(&steps[1]), word(40), "the EVM read what the Lua step wrote");

    // An EVM write, then a native contract reading it through the EVM.
    let before = bridge_count(&chain, &bridge);
    let steps = outcomes(&send_batch(&mut chain, vec![evm_step(bridge, calldata("bump(uint256)", &[word(5)])), lua("poke", vec![TxCallArgumentType::Address(bridge), TxCallArgumentType::U64(1)])]));
    let expected = u256::from_be_bytes(before.try_into().unwrap()) + u256::new(6);

    assert!(matches!(&steps[1], StepEffect::Returned(values) if format!("{values:?}").contains(&expected.to_string())), "{steps:?}");

    // Coin the same way: sent to a contract by one step, spent by that contract's call in the next.
    let token = deploy_token(&mut chain);
    let me = of_authority();
    let them = Address::Ed25519([8; 32]);
    let approve = evm_step(token, calldata("approve(address,uint256)", &[address_word(&me), word(5)]));
    let spend = evm_step(token, calldata("transferFrom(address,address,uint256)", &[address_word(&me), address_word(&them), word(5)]));
    let receipt = send_batch(&mut chain, vec![approve, spend]);

    assert_eq!(outcomes(&receipt).len(), 2, "approve then spend, as a swap needs");
    assert_eq!(receipt.events.len(), 2, "each step's log, in order");
}

#[test]
fn an_evm_step_s_logs_keep_their_order_after_those_of_the_step_before() {
    let mut chain = open(&Arc::new(MemoryDb::default()));
    let token = deploy_token(&mut chain);
    let them = Address::Ed25519([8; 32]);
    let sending = |value| evm_step(token, calldata("transfer(address,uint256)", &[address_word(&them), word(value)]));
    let receipt = send_batch(&mut chain, vec![sending(1), sending(2), sending(3)]);
    let values: Vec<Vec<u8>> = receipt
        .events
        .iter()
        .map(|event| match event.data.last() {
            Some(TxCallArgumentType::Bytes(bytes)) => bytes.clone(),
            other => panic!("{other:?}"),
        })
        .collect();

    assert_eq!(values, [word(1), word(2), word(3)]);
}
