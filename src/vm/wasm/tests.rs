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
  (import "chain" "namespace_get" (func $namespace_get (param i32 i32 i32 i32) (result i32)))
  (import "chain" "namespace_set" (func $namespace_set (param i32 i32 i32 i32 i32 i32)))
  (import "chain" "namespace_delete" (func $namespace_delete (param i32 i32 i32 i32)))
  (import "chain" "storage_scan" (func $storage_scan (param i32 i32 i32 i32 i32) (result i32)))
  (import "chain" "namespace_scan" (func $namespace_scan (param i32 i32 i32 i32 i32 i32 i32) (result i32)))
  (import "chain" "sender" (func $sender (param i32)))
  (import "chain" "contract" (func $contract (param i32)))
  (import "chain" "value" (func $value (param i32)))
  (import "chain" "block_time" (func $block_time (result i64)))
  (import "chain" "block_height" (func $block_height (result i64)))
  (import "chain" "hash" (func $hash (param i32 i32 i32 i32) (result i32)))
  (import "chain" "balance" (func $balance (param i32 i32)))
  (import "chain" "transfer" (func $transfer (param i32 i32) (result i32)))
  (import "chain" "burn" (func $burn (param i32) (result i32)))
  (import "chain" "call" (func $call (param i32 i32 i32 i32 i32 i32) (result i32)))
  (import "chain" "delegate" (func $delegate (param i32 i64 i32 i32) (result i32)))
  (import "chain" "clone" (func $clone (param i32 i32 i32 i32 i32 i32) (result i32)))
  (import "chain" "cloned_at" (func $cloned_at (param i32 i32 i32)))
  (import "chain" "check_layout" (func $check_layout (param i32 i32) (result i32)))
  (import "chain" "emit" (func $emit (param i32 i32 i32 i32)))
  (import "chain" "refresh_token" (func $refresh_token))
  (import "chain" "resolve_name" (func $resolve_name (param i32 i32 i32) (result i32)))
  (import "chain" "name_of" (func $name_of (param i32) (result i32)))
  (import "chain" "evm_call" (func $evm_call (param i32 i32 i32 i32) (result i32)))
  (import "chain" "abi_encode" (func $abi_encode (param i32 i32 i32 i32) (result i32)))
  (import "chain" "abi_decode" (func $abi_decode (param i32 i32 i32 i32) (result i32)))
  (import "chain" "verify" (func $verify (param i32 i32 i32 i32 i32 i32 i32 i32) (result i32)))
  (import "chain" "recover" (func $recover (param i32 i32 i32 i32) (result i32)))
  (import "chain" "key_address" (func $key_address (param i32 i32 i32 i32 i32)))
  (import "chain" "crypto_hash" (func $crypto_hash (param i32 i32 i32 i32 i32) (result i32)))
  (import "chain" "check_state" (func $check_state (param i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32) (result i32)))
  (import "chain" "check_merkle" (func $check_merkle (param i32 i32 i32 i32 i64 i64 i32 i32) (result i32)))
  (import "chain" "fast_aggregate_verify" (func $fast_aggregate_verify (param i32 i32 i32 i32 i32 i32) (result i32)))
  (import "chain" "federation" (func $federation (result i32)))
  (import "chain" "eth_network" (func $eth_network (result i32)))
  (import "chain" "chain_id" (func $chain_id (result i64)))
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
        block: crate::vm::BlockAt { height: 42, ..crate::vm::BlockAt::at(1_793_000_000_123) },
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
    let loaded = engine.load(&module(body), LIMITS).result.unwrap();

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
    let loaded = WasmEngine::new().load(&module(body), LIMITS).result.unwrap();

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
fn namespaces_keep_keys_apart_from_plain_storage() {
    use crate::db::{Column, namespace_key, storage_key};

    // Writes "k" three times: plain, in namespace "ns", and in the own space (no name).
    let body = r#"
      (data (i32.const 0) "k")
      (data (i32.const 8) "ns")
      (data (i32.const 16) "plainnamedown")
      (func (export "write")
        (call $storage_set (i32.const 0) (i32.const 1) (i32.const 16) (i32.const 5))
        (call $namespace_set (i32.const 8) (i32.const 2) (i32.const 0) (i32.const 1) (i32.const 21) (i32.const 5))
        (call $namespace_set (i32.const 8) (i32.const 0) (i32.const 0) (i32.const 1) (i32.const 26) (i32.const 3)))
      (func (export "forget")
        (call $namespace_delete (i32.const 8) (i32.const 2) (i32.const 0) (i32.const 1)))
      (func (export "long_name")
        (drop (call $namespace_get (i32.const 0) (i32.const 65) (i32.const 0) (i32.const 1))))
    "#;
    let state = state();
    let contract = context().contract;
    run_in(&state, body, "write", &[]).result.unwrap();
    let read = |column, key: Vec<u8>| state.get(column, &key).unwrap();

    assert_eq!(read(Column::Storage, storage_key(&contract, b"k")), Some(b"plain".to_vec()));
    assert_eq!(read(Column::Namespaces, namespace_key(&contract, b"ns", b"k")), Some(b"named".to_vec()));
    assert_eq!(read(Column::Namespaces, namespace_key(&contract, b"", b"k")), Some(b"own".to_vec()));

    run_in(&state, body, "forget", &[]).result.unwrap();

    assert_eq!(read(Column::Namespaces, namespace_key(&contract, b"ns", b"k")), None);
    assert_eq!(read(Column::Storage, storage_key(&contract, b"k")), Some(b"plain".to_vec()));
    assert!(matches!(run(body, "long_name", &[]).result, Err(VmError::Runtime(text)) if text.contains("namespace names are at most 64")));
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
fn the_block_is_readable() {
    // Returns the block's time and height as two I64s.
    let body = r#"
      (func (export "when")
        (i64.store (i32.const 0) (i64.const 2))
        (i32.store8 (i32.const 8) (i32.const 6))
        (i64.store (i32.const 9) (call $block_time))
        (i32.store8 (i32.const 17) (i32.const 6))
        (i64.store (i32.const 18) (call $block_height))
        (call $output (i32.const 0) (i32.const 26)))
    "#;

    assert_eq!(run(body, "when", &[]).result.unwrap(), [TxCallArgumentType::I64(1_793_000_000_123), TxCallArgumentType::I64(42)], "the block's own");
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
    assert!(WasmEngine::new().load(&huge, LIMITS).result.is_err());
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
fn loading_pays_for_the_bytes() {
    let engine = WasmEngine::new();
    let plain = module(r#"(func (export "f"))"#);
    let loading = engine.load(&plain, LIMITS);

    assert!(loading.result.is_ok(), "{:?}", loading.result);
    assert_eq!(loading.fuel_used, plain.len() as u64 * crate::params::vm::HostCosts::DEFAULT.load_byte, "nothing ran");

    // A start function would run while loading, so modules may not have one; refusing it still
    // pays for the bytes.
    let started = module(r#"(func $begin) (start $begin) (func (export "f"))"#);
    let refused = engine.load(&started, LIMITS);

    assert!(matches!(refused.result, Err(VmError::Compile(text)) if text.contains("start")));
    assert_eq!(refused.fuel_used, started.len() as u64 * crate::params::vm::HostCosts::DEFAULT.load_byte);

    let starved = engine.load(&plain, Limits { fuel: 5, ..LIMITS });

    assert_eq!((starved.result.unwrap_err(), starved.fuel_used), (VmError::OutOfFuel, 5));
    assert_eq!(engine.load(&[0, 1, 2], LIMITS).fuel_used, 3, "a module that does not compile pays for its bytes");
}

#[test]
fn modules_that_could_disagree_or_misbehave_are_refused() {
    let engine = WasmEngine::new();
    let refused = |wat: &str| engine.load(&wat::parse_str(wat).unwrap(), LIMITS).result.unwrap_err();

    assert!(matches!(refused(r#"(module (memory (export "memory") 1) (func (export "f") (drop (f64.const 1))))"#), VmError::Compile(_)));
    assert!(matches!(refused(r#"(module (memory (export "memory") 1) (func $f) (start $f))"#), VmError::Compile(_)));
    assert!(matches!(refused(r#"(module (import "env" "time" (func)) (memory (export "memory") 1))"#), VmError::Compile(text) if text.contains("unknown import")));
    assert!(matches!(refused(r#"(module (import "chain" "nope" (func)) (memory (export "memory") 1))"#), VmError::Compile(text) if text.contains("unknown import")));
    assert!(matches!(refused(r#"(module (func (export "f")))"#), VmError::Compile(text) if text.contains("memory")));
    assert!(matches!(refused(r#"(module (import "chain" "abort" (func (param i64))) (memory (export "memory") 1))"#), VmError::Runtime(_) | VmError::Compile(_)));
    assert!(matches!(engine.load(&[0, 1, 2], LIMITS).result, Err(VmError::Compile(_))));
    assert!(matches!(engine.load(&vec![0; MAX_MODULE_LEN + 1], LIMITS).result, Err(VmError::Compile(text)) if text.contains("allowed")));
}

#[test]
fn every_host_function_links() {
    assert_eq!(IMPORTS.matches("(import").count(), host::NAMES.len());
    WasmEngine::new().load(&module(""), LIMITS).result.unwrap();
}

#[test]
fn the_reentrant_section_marks_exported_functions() {
    let engine = WasmEngine::new();
    let load = |sections: &str| {
        let wat = format!(
            r#"(module (memory (export "memory") 1) (func (export "a")) (func (export "b")) {sections})"#
        );

        engine.load(&wat::parse_str(wat).unwrap(), LIMITS).result
    };

    assert_eq!(load("").unwrap().reentrant, Vec::<String>::new());
    assert_eq!(load(r#"(@custom "chainv4.reentrant" "b\00a\00") (@custom "chainv4.reentrant" "b\00")"#).unwrap().reentrant, ["a", "b"]);
    assert!(matches!(load(r#"(@custom "chainv4.reentrant" "c\00")"#), Err(VmError::Compile(text)) if text.contains("does not export")));
    assert!(matches!(load(r#"(@custom "chainv4.reentrant" "a")"#), Err(VmError::Compile(text)) if text.contains("zero byte")));
}

#[test]
fn the_layout_section_declares_namespaces() {
    let engine = WasmEngine::new();
    let load = |sections: &str| {
        let wat = format!(r#"(module (memory (export "memory") 1) {sections})"#);

        engine.load(&wat::parse_str(wat).unwrap(), LIMITS).result
    };
    let declared = load(r#"(@custom "chainv4.layout" "retired fees\00key config owner address\00") (@custom "chainv4.layout" "namespace balances u256\00namespace config\00")"#).unwrap();

    assert_eq!(declared.layout.to_string(), "namespace balances u256\nnamespace config\nkey config owner address\nretired fees\n");
    assert_eq!(load("").unwrap().layout, crate::vm::Layout::default());
    assert!(matches!(load(r#"(@custom "chainv4.layout" "namespace balances int\00")"#), Err(VmError::Compile(text)) if text.contains("no kind int")));
    assert!(matches!(load(r#"(@custom "chainv4.layout" "retired a\00retired a\00")"#), Err(VmError::Compile(text)) if text.contains("declared twice")));
    assert!(matches!(load(r#"(@custom "chainv4.layout" "retired a")"#), Err(VmError::Compile(text)) if text.contains("zero byte")));
}

#[test]
fn compiled_modules_are_cached_by_their_code() {
    let engine = WasmEngine::new();
    let counter = module(r#"(func (export "nothing"))"#);
    let other = module(r#"(func (export "else"))"#);
    let loaded = engine.load(&counter, LIMITS).result.unwrap();
    let fresh = WasmEngine::new().run(&loaded.code, "nothing", &[], host(state()), LIMITS);

    assert_eq!(engine.cache().entries.len(), 1, "deploying compiles it once");

    for _ in 0..3 {
        let cached = engine.run(&loaded.code, "nothing", &[], host(state()), LIMITS);

        assert_eq!((cached.result, cached.fuel_used), (fresh.result.clone(), fresh.fuel_used));
    }

    // The same code at another address shares the entry; other code, such as an upgrade, has its own.
    engine.load(&counter, LIMITS).result.unwrap();
    engine.run(&other, "else", &[], host(state()), LIMITS);

    assert_eq!(engine.cache().entries.len(), 2);
}

#[test]
fn a_full_cache_lets_go_of_the_module_used_longest_ago() {
    let [a, b, c] = ["a", "b", "c"].map(|name| module(&format!(r#"(func (export "{name}"))"#)));
    let engine = WasmEngine::with_cache(a.len() + b.len());

    for code in [&a, &b] {
        engine.load(code, LIMITS).result.unwrap();
    }

    engine.run(&a, "a", &[], host(state()), LIMITS);
    engine.load(&c, LIMITS).result.unwrap();
    let cache = engine.cache();

    assert!(cache.entries.contains_key(&a[..]), "used since b was");
    assert!(!cache.entries.contains_key(&b[..]));
    assert!(cache.entries.contains_key(&c[..]));
    assert!(cache.bytes <= a.len() + b.len());
}

#[test]
fn a_cache_sized_on_the_node_holds_no_more_modules_than_it_was_given() {
    let [a, b, c] = ["a", "b", "c"].map(|name| module(&format!(r#"(func (export "{name}"))"#)));
    let engine = WasmEngine::with_cache_of(MODULE_CACHE_BYTES, 2);

    for code in [&a, &b, &c] {
        engine.load(code, LIMITS).result.unwrap();
    }

    // Room for all three by bytes, but only two by count: the one used longest ago goes.
    assert_eq!(engine.cache().entries.len(), 2);
    assert!(!engine.cache().entries.contains_key(&a[..]));
    assert_eq!(engine.run(&a, "a", &[], host(state()), LIMITS).result, WasmEngine::new().run(&a, "a", &[], host(state()), LIMITS).result, "and a call runs the same whatever it holds");
}
