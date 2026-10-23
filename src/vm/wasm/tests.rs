use super::*;
use crate::crypto::hashable::Hashable;
use crate::crypto::hasher::Hasher;
use crate::db::{MemoryDb, State};
use crate::structs::address::Address;
use crate::structs::code::Engine as Kind;
use crate::structs::hash::HashType;
use crate::structs::u256::u256;
use crate::vm::{CallContext, Vm};
use std::sync::Arc;

const LIMITS: Limits = Limits {
    fuel: 1_000_000,
    memory: 1024 * 1024,
};

/// Imports for every host function, so tests only write the functions they need.
const IMPORTS: &str = r#"
  (import "chain" "input_len" (func $input_len (result i32)))
  (import "chain" "input_read" (func $input_read (param i32)))
  (import "chain" "output" (func $output (param i32 i32)))
  (import "chain" "result_len" (func $result_len (result i32)))
  (import "chain" "result_read" (func $result_read (param i32)))
  (import "chain" "storage_get" (func $storage_get (param i32 i32) (result i32)))
  (import "chain" "storage_set" (func $storage_set (param i32 i32 i32 i32)))
  (import "chain" "storage_delete" (func $storage_delete (param i32 i32)))
  (import "chain" "sender" (func $sender (param i32)))
  (import "chain" "contract" (func $contract (param i32)))
  (import "chain" "value" (func $value (param i32)))
  (import "chain" "hash" (func $hash (param i32 i32 i32 i32) (result i32)))
  (import "chain" "balance" (func $balance (param i32 i32)))
  (import "chain" "transfer" (func $transfer (param i32 i32) (result i32)))
  (import "chain" "call" (func $call (param i32 i32 i32 i32 i32 i32) (result i32)))
  (import "chain" "delegate" (func $delegate (param i32 i64 i32 i32) (result i32)))
  (import "chain" "emit" (func $emit (param i32 i32 i32 i32)))
  (import "chain" "abort" (func $abort (param i32 i32)))
"#;

/// A module with every import, one page of memory, and the given body.
fn module(body: &str) -> Vec<u8> {
    wat::parse_str(format!("(module {IMPORTS} (memory (export \"memory\") 1) {body})")).unwrap()
}

fn context() -> CallContext {
    CallContext {
        sender: Address::Ed25519([1; 32]),
        contract: Address::Contract([2; 32]),
        value: u256::new(500),
    }
}

fn host(state: Arc<State>) -> Host {
    Host::root(state, context(), LIMITS.memory, Arc::new(Vm::new(&[Kind::Wasm], HashType::default_chain()).unwrap()))
}

fn state() -> Arc<State> {
    Arc::new(State::new(Arc::new(MemoryDb::default())))
}

fn run_in(state: &Arc<State>, body: &str, function: &str, args: &[TxCallArgumentType]) -> Outcome {
    let engine = WasmEngine::new();
    let loaded = engine.load(&module(body), LIMITS).unwrap();

    engine.run(&loaded.code, function, args, host(state.clone()), LIMITS)
}

fn run(body: &str, function: &str, args: &[TxCallArgumentType]) -> Outcome {
    run_in(&state(), body, function, args)
}

const ECHO: &str = r#"
  (func (export "echo")
    (call $input_read (i32.const 0))
    (call $output (i32.const 0) (call $input_len)))
"#;

#[test]
fn arguments_come_in_and_go_back_out_encoded() {
    let args = [
        TxCallArgumentType::I64(-7),
        TxCallArgumentType::String("hi".to_string()),
        TxCallArgumentType::Address(Address::EdBlake([3; 32])),
        TxCallArgumentType::U256(u256::MAX),
    ];

    assert_eq!(run(ECHO, "echo", &args).result.unwrap(), args);
}

#[test]
fn exports_are_the_functions_without_parameters() {
    let body = r#"
      (func (export "b"))
      (func (export "a"))
      (func (export "takes_one") (param i32))
      (func (export "gives_one") (result i32) (i32.const 1))
    "#;
    let loaded = WasmEngine::new().load(&module(body), LIMITS).unwrap();

    assert_eq!(loaded.functions, ["a", "b"]);
    assert_eq!(run(body, "missing", &[]).result.map_err(|err| matches!(err, VmError::Runtime(_))), Err(true));
}

#[test]
fn storage_persists_through_the_host() {
    // Reads an i64 counter, adds one, writes it back and returns it as I64.
    let body = r#"
      (data (i32.const 0) "count")
      (func (export "bump")
        (if (i32.ge_s (call $storage_get (i32.const 0) (i32.const 5)) (i32.const 0))
          (then (call $result_read (i32.const 16))))
        (i64.store (i32.const 16) (i64.add (i64.load (i32.const 16)) (i64.const 1)))
        (call $storage_set (i32.const 0) (i32.const 5) (i32.const 16) (i32.const 8))
        (i64.store (i32.const 32) (i64.const 1))
        (i32.store8 (i32.const 40) (i32.const 6))
        (i64.store (i32.const 41) (i64.load (i32.const 16)))
        (call $output (i32.const 32) (i32.const 17)))
      (func (export "forget")
        (call $storage_delete (i32.const 0) (i32.const 5)))
    "#;
    let state = state();

    for expected in 1..=3 {
        assert_eq!(run_in(&state, body, "bump", &[]).result.unwrap(), [TxCallArgumentType::I64(expected)]);
    }

    run_in(&state, body, "forget", &[]).result.unwrap();

    assert_eq!(run_in(&state, body, "bump", &[]).result.unwrap(), [TxCallArgumentType::I64(1)]);
}

#[test]
fn the_context_is_readable() {
    // Returns [Address(sender), Address(contract)], then the value as Bytes.
    let body = r#"
      (func (export "who")
        (i64.store (i32.const 0) (i64.const 3))
        (i32.store8 (i32.const 8) (i32.const 0))
        (call $sender (i32.const 9))
        (i32.store8 (i32.const 42) (i32.const 0))
        (call $contract (i32.const 43))
        (i32.store8 (i32.const 76) (i32.const 2))
        (i64.store (i32.const 77) (i64.const 32))
        (call $value (i32.const 85))
        (call $output (i32.const 0) (i32.const 117)))
    "#;
    let mut value = vec![0; 32];
    value[..2].copy_from_slice(&500u16.to_le_bytes());

    assert_eq!(
        run(body, "who", &[]).result.unwrap(),
        [
            TxCallArgumentType::Address(context().sender),
            TxCallArgumentType::Address(context().contract),
            TxCallArgumentType::Bytes(value),
        ]
    );
}

#[test]
fn hashes_match_the_hasher() {
    // Every 32-byte algorithm this build has, by tag, hashing "abc" and returning it as Bytes.
    for &algorithm in HashType::ALL.iter().filter(|algorithm| algorithm.secure()) {
        let tag = algorithm.hashable_bytes_with_tag()[0];
        let body = format!(
            r#"
      (data (i32.const 100) "abc")
      (func (export "digest")
        (i64.store (i32.const 0) (i64.const 1))
        (i32.store8 (i32.const 8) (i32.const 2))
        (i64.store (i32.const 9) (i64.extend_i32_u
          (call $hash (i32.const {tag}) (i32.const 100) (i32.const 3) (i32.const 17))))
        (call $output (i32.const 0) (i32.const 49)))
    "#
        );
        let expected = Hasher::digest(algorithm, b"abc").to_bytes();

        assert_eq!(run(&body, "digest", &[]).result.unwrap(), [TxCallArgumentType::Bytes(expected)], "{}", algorithm.name());
    }

    let bad = r#"(func (export "bad_algorithm") (drop (call $hash (i32.const 99) (i32.const 100) (i32.const 3) (i32.const 17))))"#;

    assert!(matches!(run(bad, "bad_algorithm", &[]).result, Err(VmError::Runtime(text)) if text.contains("algorithm")));
}

#[test]
fn an_endless_loop_runs_out_of_fuel() {
    let outcome = run(r#"(func (export "spin") (loop $again (br $again)))"#, "spin", &[]);

    assert_eq!(outcome.result.unwrap_err(), VmError::OutOfFuel);
    // The last instruction may cost more than what is left.
    assert!(outcome.fuel_used > LIMITS.fuel - 10, "{}", outcome.fuel_used);
}

#[test]
fn the_same_call_always_costs_the_same() {
    let first = run(ECHO, "echo", &[TxCallArgumentType::U64(1)]);
    let second = run(ECHO, "echo", &[TxCallArgumentType::U64(1)]);

    assert!(first.fuel_used > 0);
    assert_eq!(first.fuel_used, second.fuel_used);
}

#[test]
fn memory_is_capped() {
    let grow = r#"(func (export "grow") (drop (memory.grow (i32.const 1000))))"#;
    let huge = wat::parse_str(r#"(module (memory (export "memory") 100))"#).unwrap();

    assert_eq!(run(grow, "grow", &[]).result.unwrap_err(), VmError::OutOfMemory);
    assert!(WasmEngine::new().load(&huge, LIMITS).is_err());
}

#[test]
fn aborts_and_traps_fail_the_call() {
    let body = r#"
      (data (i32.const 0) "no way")
      (func (export "refuse") (call $abort (i32.const 0) (i32.const 6)))
      (func (export "crash") unreachable)
      (func (export "wander") (call $output (i32.const 65530) (i32.const 100)))
      (func (export "garbage") (call $output (i32.const 0) (i32.const 6)))
    "#;

    assert_eq!(run(body, "refuse", &[]).result.unwrap_err(), VmError::Runtime("aborted: no way".to_string()));
    assert!(matches!(run(body, "crash", &[]).result, Err(VmError::Runtime(_))));
    assert!(matches!(run(body, "wander", &[]).result, Err(VmError::Runtime(text)) if text.contains("out of bounds")));
    assert!(matches!(run(body, "garbage", &[]).result, Err(VmError::BadReturn(_))));
}

#[test]
fn storage_limits_are_enforced() {
    let body = r#"
      (func (export "long_key") (drop (call $storage_get (i32.const 0) (i32.const 257))))
      (func (export "big_value") (call $storage_set (i32.const 0) (i32.const 1) (i32.const 0) (i32.const 65537)))
    "#;

    for function in ["long_key", "big_value"] {
        assert!(matches!(run(body, function, &[]).result, Err(VmError::Runtime(text)) if text.contains("at most")));
    }
}

#[test]
fn modules_that_could_disagree_or_misbehave_are_refused() {
    let engine = WasmEngine::new();
    let refused = |wat: &str| engine.load(&wat::parse_str(wat).unwrap(), LIMITS).unwrap_err();

    assert!(matches!(refused(r#"(module (memory (export "memory") 1) (func (export "f") (drop (f64.const 1))))"#), VmError::Compile(_)));
    assert!(matches!(refused(r#"(module (memory (export "memory") 1) (func $f) (start $f))"#), VmError::Compile(_)));
    assert!(matches!(refused(r#"(module (import "env" "time" (func)) (memory (export "memory") 1))"#), VmError::Compile(text) if text.contains("unknown import")));
    assert!(matches!(refused(r#"(module (import "chain" "nope" (func)) (memory (export "memory") 1))"#), VmError::Compile(text) if text.contains("unknown import")));
    assert!(matches!(refused(r#"(module (func (export "f")))"#), VmError::Compile(text) if text.contains("memory")));
    assert!(matches!(refused(r#"(module (import "chain" "abort" (func (param i64))) (memory (export "memory") 1))"#), VmError::Runtime(_) | VmError::Compile(_)));
    assert!(matches!(engine.load(&[0, 1, 2], LIMITS), Err(VmError::Compile(_))));
    assert!(matches!(engine.load(&vec![0; MAX_MODULE_LEN + 1], LIMITS), Err(VmError::Compile(text)) if text.contains("allowed")));
}

#[test]
fn every_host_function_links() {
    assert_eq!(IMPORTS.matches("(import").count(), host::NAMES.len());
    WasmEngine::new().load(&module(""), LIMITS).unwrap();
}

#[test]
fn the_reentrant_section_marks_exported_functions() {
    let engine = WasmEngine::new();
    let load = |sections: &str| {
        let wat = format!(
            r#"(module (memory (export "memory") 1) (func (export "a")) (func (export "b")) {sections})"#
        );

        engine.load(&wat::parse_str(wat).unwrap(), LIMITS)
    };

    assert_eq!(load("").unwrap().reentrant, Vec::<String>::new());
    assert_eq!(load(r#"(@custom "chainv4.reentrant" "b\00a\00") (@custom "chainv4.reentrant" "b\00")"#).unwrap().reentrant, ["a", "b"]);
    assert!(matches!(load(r#"(@custom "chainv4.reentrant" "c\00")"#), Err(VmError::Compile(text)) if text.contains("does not export")));
    assert!(matches!(load(r#"(@custom "chainv4.reentrant" "a")"#), Err(VmError::Compile(text)) if text.contains("zero byte")));
}
