use super::*;
use crate::crypto::hasher::Hasher;
use crate::db::{MemoryDb, State};
use crate::structs::address::Address;
use crate::structs::amount::Amount;
use crate::structs::hash::HashType;
use crate::structs::u256::u256;
use crate::structs::code::Engine;
use crate::vm::{CallContext, Vm};
use std::sync::Arc;

const LIMITS: Limits = Limits {
    fuel: 1_000_000,
    memory: 16 * 1024 * 1024,
};

const SCRIPT: &str = r#"
local M = {}

local calls = 0

function M.add(a, b)
    return a + b
end

function M.count()
    calls += 1
    return calls
end

function M.whoami()
    return ctx.sender, ctx.contract, ctx.value
end

function M.echo(...)
    return ...
end

function M.spin()
    while true do end
end

function M.hog()
    return string.rep("x", 64 * 1024 * 1024)
end

function M.boom()
    error("boom")
end

function M.fake_fuel()
    error("out of fuel")
end

function M.overflow()
    return u256.max + 1
end

function M.leak()
    leaked = true
end

function M.hidden()
    return type(os), type(io), type(debug), type(require), type(loadstring), type(getfenv),
        type(collectgarbage), type(coroutine), type(math.random), type(math.sin)
end

return M
"#;

fn contract() -> Contract {
    Contract::load(SCRIPT, LIMITS).unwrap()
}

fn context() -> CallContext {
    CallContext {
        sender: Address::Ed25519([1; 32]),
        contract: Address::Ed25519([2; 32]),
        value: u256::new(500),
    }
}

fn func(name: &str) -> Hash {
    Hash::Xxh3_64(selector(name))
}

fn host() -> Host {
    let vm = Arc::new(Vm::new(&[Engine::Lua]).unwrap());

    Host::root(Arc::new(State::new(Arc::new(MemoryDb::default()))), context(), LIMITS.memory, vm)
}

fn call(name: &str, args: &[TxCallArgumentType]) -> Outcome {
    contract().call(&func(name), args, host(), LIMITS)
}

fn returns(name: &str, args: &[TxCallArgumentType]) -> Vec<TxCallArgumentType> {
    call(name, args).result.unwrap()
}

#[test]
fn calls_reach_exported_functions_by_hash() {
    let sum = returns(
        "add",
        &[TxCallArgumentType::U64(1 << 60), TxCallArgumentType::U64(3)],
    );

    assert!(matches!(sum[..], [TxCallArgumentType::U256(value)] if value == u256::new((1 << 60) + 3)));
    assert_eq!(contract().function_name(&func("add")), Some("add"));
}

#[test]
fn unknown_functions_are_rejected() {
    assert_eq!(call("nope", &[]).result.unwrap_err(), VmError::NoSuchFunction);

    #[cfg(feature = "blake3")]
    {
        let wrong = Hasher::digest(HashType::Blake3_256, b"add");

        assert_eq!(
            contract().call(&wrong, &[], host(), LIMITS).result.unwrap_err(),
            VmError::NoSuchFunction
        );
    }
}

#[test]
fn every_call_starts_from_a_fresh_state() {
    let contract = contract();

    for _ in 0..2 {
        let result = contract.call(&func("count"), &[], host(), LIMITS).result.unwrap();

        assert!(matches!(result[..], [TxCallArgumentType::Number(n)] if n == 1.0));
    }
}

#[test]
fn ctx_describes_the_call() {
    let result = returns("whoami", &[]);

    assert!(matches!(
        result[..],
        [
            TxCallArgumentType::Address(Address::Ed25519([1, ..])),
            TxCallArgumentType::Address(Address::Ed25519([2, ..])),
            TxCallArgumentType::U256(value),
        ] if value == u256::new(500)
    ));
}

#[test]
fn values_cross_into_lua_and_back() {
    let args = [
        TxCallArgumentType::Bool(true),
        TxCallArgumentType::String("hi".to_string()),
        TxCallArgumentType::Bytes(vec![0xff, 0x00]),
        TxCallArgumentType::Number(1.5),
        TxCallArgumentType::I64(-5),
        TxCallArgumentType::Address(Address::Ed25519([9; 32])),
        TxCallArgumentType::Amount(Amount::U128(7)),
    ];
    let result = returns("echo", &args);

    assert!(matches!(
        &result[..],
        [
            TxCallArgumentType::Bool(true),
            TxCallArgumentType::String(text),
            TxCallArgumentType::Bytes(bytes),
            TxCallArgumentType::Number(half),
            TxCallArgumentType::Number(minus_five),
            TxCallArgumentType::Address(Address::Ed25519([9, ..])),
            TxCallArgumentType::U256(seven),
        ] if text == "hi" && bytes[..] == [0xff, 0x00] && *half == 1.5 && *minus_five == -5.0
            && *seven == u256::new(7)
    ));
}

#[test]
fn integers_a_lua_number_would_round_are_refused() {
    let err = call("echo", &[TxCallArgumentType::I64(i64::MAX)]).result.unwrap_err();

    assert!(matches!(err, VmError::BadArgument(_)), "{err:?}");
}

#[test]
fn an_endless_loop_runs_out_of_fuel() {
    let outcome = call("spin", &[]);

    assert_eq!(outcome.result.unwrap_err(), VmError::OutOfFuel);
    assert!(outcome.fuel_used > LIMITS.fuel - MAX_INSTRUCTION_COST);
    assert!(matches!(
        Contract::load("while true do end", LIMITS),
        Err(VmError::Runtime(text)) if text.contains("out of fuel")
    ));
}

#[test]
fn raising_out_of_fuel_by_hand_is_just_an_error() {
    let outcome = call("fake_fuel", &[]);

    assert!(matches!(outcome.result, Err(VmError::Runtime(_))), "{outcome:?}");
    assert!(outcome.fuel_used < LIMITS.fuel / 2);
}

#[test]
fn memory_is_capped() {
    assert_eq!(call("hog", &[]).result.unwrap_err(), VmError::OutOfMemory);
}

#[test]
fn errors_come_back_with_their_message() {
    let boom = call("boom", &[]).result.unwrap_err();
    let overflow = call("overflow", &[]).result.unwrap_err();

    assert!(matches!(&boom, VmError::Runtime(text) if text.contains("boom")), "{boom:?}");
    assert!(matches!(&overflow, VmError::Runtime(text) if text.contains("u256 overflow")), "{overflow:?}");
}

#[test]
fn scripts_cannot_set_globals() {
    let err = call("leak", &[]).result.unwrap_err();

    assert!(matches!(&err, VmError::Runtime(text) if text.contains("readonly")), "{err:?}");
}

#[test]
fn nondeterministic_globals_are_gone() {
    let result = returns("hidden", &[]);

    assert_eq!(result.len(), 10);

    for value in result {
        assert!(matches!(&value, TxCallArgumentType::String(text) if text == "nil"), "{value:?}");
    }
}

#[test]
fn the_same_call_always_costs_the_same() {
    let first = call("add", &[TxCallArgumentType::U64(1), TxCallArgumentType::U64(2)]);
    let second = call("add", &[TxCallArgumentType::U64(1), TxCallArgumentType::U64(2)]);

    assert!(first.fuel_used > 0);
    assert_eq!(first.fuel_used, second.fuel_used);
}

#[test]
fn bad_scripts_do_not_load() {
    assert!(matches!(Contract::load("local = 1", LIMITS), Err(VmError::Compile(_))));
    assert!(matches!(Contract::load("return 5", LIMITS), Err(VmError::NotAModule)));
    assert!(matches!(Contract::load("error('no')", LIMITS), Err(VmError::Runtime(_))));
}
