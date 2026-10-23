//! Calls between contracts, across engines.

use super::*;
use crate::db::{Column, MemoryDb, storage_key};
use crate::registry::Accounts;
use crate::structs::hex::to_hex;
use crate::registry::Contracts;
use crate::structs::code::Code;

const LIMITS: Limits = Limits {
    fuel: 5_000_000,
    memory: 16 * 1024 * 1024,
};

const DEPLOYER: Address = Address::Ed25519([7; 32]);

/// Counts calls in storage and returns the count as I64.
const WASM_COUNTER: &str = r#"
(module
  (import "chain" "storage_get" (func $storage_get (param i32 i32) (result i32)))
  (import "chain" "storage_set" (func $storage_set (param i32 i32 i32 i32)))
  (import "chain" "result_read" (func $result_read (param i32)))
  (import "chain" "output" (func $output (param i32 i32)))
  (memory (export "memory") 1)
  (data (i32.const 0) "count")
  (func (export "bump")
    (if (i32.ge_s (call $storage_get (i32.const 0) (i32.const 5)) (i32.const 0))
      (then (call $result_read (i32.const 16))))
    (i64.store (i32.const 16) (i64.add (i64.load (i32.const 16)) (i64.const 1)))
    (call $storage_set (i32.const 0) (i32.const 5) (i32.const 16) (i32.const 8))
    (i64.store (i32.const 32) (i64.const 1))
    (i32.store8 (i32.const 40) (i32.const 6))
    (i64.store (i32.const 41) (i64.load (i32.const 16)))
    (call $output (i32.const 32) (i32.const 17))))
"#;

/// Takes [Address(target)] and calls target's "hello" with no arguments, returning what it
/// returns, or aborting with its error.
const WASM_RELAY: &str = r#"
(module
  (import "chain" "input_read" (func $input_read (param i32)))
  (import "chain" "call" (func $call (param i32 i32 i32 i32 i32 i32) (result i32)))
  (import "chain" "result_len" (func $result_len (result i32)))
  (import "chain" "result_read" (func $result_read (param i32)))
  (import "chain" "output" (func $output (param i32 i32)))
  (import "chain" "abort" (func $abort (param i32 i32)))
  (memory (export "memory") 1)
  (data (i32.const 100) "hello")
  (func (export "relay")
    (local $status i32)
    (call $input_read (i32.const 0))
    (local.set $status
      (call $call (i32.const 9) (i32.const 300) (i32.const 100) (i32.const 5) (i32.const 200) (i32.const 8)))
    (call $result_read (i32.const 1000))
    (if (local.get $status)
      (then (call $abort (i32.const 1000) (call $result_len))))
    (call $output (i32.const 1000) (call $result_len))))
"#;

const LUA: &str = r#"
local M = {}

function M.hello()
    return "hi from " .. tostring(ctx.sender)
end

function M.bump_twice(counter)
    contracts.call(counter, "bump")
    return contracts.call(counter, "bump")
end

function M.write_then_fail()
    storage.set("touched", "yes")
    error("changed my mind")
end

function M.try(target, name)
    local ok, err = pcall(contracts.call, target, name)
    return ok, tostring(err)
end

function M.call_back(target)
    return contracts.call(target, "call_back", ctx.contract)
end

function M.bounce(target)
    return contracts.call(target, "bounce_back", ctx.contract)
end

function M.bounce_back(origin)
    return contracts.call(origin, "pong")
end

--@reentrant
function M.pong()
    return "pong"
end

function M.bump_by(counter)
    return contracts.call(counter, "increment", 7)
end

function M.ping_caller()
    return contracts.call(ctx.sender, "ping")
end

function M.forward_caller()
    return contracts.call(ctx.sender, "forward", ctx.contract, "hello")
end

function M.pay(to, amount)
    accounts.transfer(to, amount)
    return accounts.balance(ctx.contract)
end

function M.send_value(target, amount, name)
    return contracts.call_value(target, amount, name)
end

function M.try_send_value(target, amount, name)
    local ok = pcall(contracts.call_value, target, amount, name)
    return ok, accounts.balance(ctx.contract)
end

function M.receive()
    return ctx.value, accounts.balance(ctx.contract)
end

function M.shout(word)
    events.emit("Shout", word)
    return word
end

function M.shout_then_fail()
    events.emit("Lost")
    error("no")
end

function M.relay_shout(target)
    events.emit("Before")
    contracts.call(target, "shout", "hi")
    pcall(contracts.call, target, "shout_then_fail")
    events.emit("After")
end

function M.save(vault, amount)
    contracts.call_value(vault, amount, "deposit")
    contracts.call(vault, "withdraw", 20)
    return accounts.balance(ctx.contract), accounts.balance(vault)
end

function M.forward(...)
    local targets = { ... }
    if #targets == 0 then
        return 0
    end
    local next = table.remove(targets, 1)
    return contracts.call(next, "forward", table.unpack(targets)) + 1
end

return M
"#;

struct World {
    state: Arc<State>,
    vm: Arc<Vm>,
    nonce: u64,
}

impl World {
    fn new() -> World {
        World {
            state: Arc::new(State::new(Arc::new(MemoryDb::default()))),
            vm: Arc::new(Vm::new(&[Engine::Lua, Engine::Wasm]).unwrap()),
            nonce: 0,
        }
    }

    fn deploy(&mut self, code: Code) -> Address {
        self.nonce += 1;

        Contracts::new(&self.state)
            .deploy(&self.vm, &DEPLOYER, self.nonce, &code, LIMITS)
            .unwrap()
    }

    fn lua(&mut self) -> Address {
        self.deploy(Code::Lua(LUA.to_string()))
    }

    fn wasm(&mut self, wat: &str) -> Address {
        self.deploy(Code::Wasm(wat::parse_str(wat).unwrap()))
    }

    fn call(&self, contract: Address, function: &str, args: &[TxCallArgumentType]) -> Outcome {
        let context = CallContext {
            sender: DEPLOYER,
            contract,
            value: u256::ZERO,
        };
        let func = Hash::Xxh3_64(selector(function));

        Contracts::new(&self.state)
            .call(&self.vm, &context, &func, args, LIMITS)
            .unwrap()
    }
}

fn address(address: Address) -> TxCallArgumentType {
    TxCallArgumentType::Address(address)
}

fn wide(value: u64) -> TxCallArgumentType {
    TxCallArgumentType::U256(u256::from(value))
}

impl World {
    fn fund(&self, address: Address, amount: u64) {
        Accounts::new(&self.state).credit(&address, u256::from(amount)).unwrap();
    }

    fn balance(&self, address: Address) -> u256 {
        Accounts::new(&self.state).get(&address).unwrap().balance
    }
}

#[test]
fn contracts_send_from_their_own_balance() {
    let mut world = World::new();
    let lua = world.lua();
    let bob = Address::Ed25519([4; 32]);
    world.fund(lua, 100);

    assert_eq!(world.call(lua, "pay", &[address(bob), wide(30)]).result.unwrap(), [wide(70)]);
    assert_eq!(world.balance(bob), u256::from(30u64));

    let err = world.call(lua, "pay", &[address(bob), wide(1_000)]).result.unwrap_err();

    assert!(matches!(&err, VmError::Runtime(text) if text.contains("insufficient balance")), "{err:?}");
    assert_eq!(world.balance(lua), u256::from(70u64));
}

#[test]
fn value_travels_with_calls_and_comes_back_when_they_fail() {
    let mut world = World::new();
    let payer = world.lua();
    let payee = world.lua();
    world.fund(payer, 100);

    assert_eq!(
        world.call(payer, "send_value", &[address(payee), wide(40), TxCallArgumentType::String("receive".into())]).result.unwrap(),
        [wide(40), wide(40)]
    );
    assert_eq!(world.balance(payer), u256::from(60u64));

    let failing = TxCallArgumentType::String("write_then_fail".into());

    assert_eq!(
        world.call(payer, "try_send_value", &[address(payee), wide(10), failing]).result.unwrap(),
        [TxCallArgumentType::Bool(false), wide(60)]
    );
    assert_eq!(world.balance(payee), u256::from(40u64));

    let err = world
        .call(payer, "send_value", &[address(payee), wide(500), TxCallArgumentType::String("receive".into())])
        .result
        .unwrap_err();

    assert!(matches!(&err, VmError::Runtime(text) if text.contains("insufficient balance")), "{err:?}");
}

#[test]
fn lua_calls_wasm_and_its_writes_stick() {
    let mut world = World::new();
    let counter = world.wasm(WASM_COUNTER);
    let lua = world.lua();

    assert_eq!(
        world.call(lua, "bump_twice", &[address(counter)]).result.unwrap(),
        [TxCallArgumentType::Number(2.0)]
    );
    assert_eq!(
        world.call(counter, "bump", &[]).result.unwrap(),
        [TxCallArgumentType::I64(3)]
    );
}

#[test]
fn wasm_calls_lua_and_the_callee_sees_it_as_sender() {
    let mut world = World::new();
    let lua = world.lua();
    let relay = world.wasm(WASM_RELAY);

    assert_eq!(
        world.call(relay, "relay", &[address(lua)]).result.unwrap(),
        [TxCallArgumentType::String(format!("hi from {}", to_hex(relay.bytes())))]
    );
}

#[test]
fn a_failed_call_comes_back_as_an_error_the_caller_can_handle() {
    let mut world = World::new();
    let lua = world.lua();
    let relay = world.wasm(WASM_RELAY);
    let empty = world.wasm(r#"(module (memory (export "memory") 1))"#);
    let nobody = Address::Contract([9; 32]);

    for (target, expected) in [(empty, "no such function"), (nobody, "no contract")] {
        let err = world.call(relay, "relay", &[address(target)]).result.unwrap_err();

        assert!(matches!(&err, VmError::Runtime(text) if text.contains(expected)), "{err:?}");
    }

    // Lua catches the failure with pcall; what the callee wrote is gone.
    let other = world.lua();
    let result = world
        .call(lua, "try", &[address(other), TxCallArgumentType::String("write_then_fail".to_string())])
        .result
        .unwrap();

    assert!(matches!(
        &result[..],
        [TxCallArgumentType::Bool(false), TxCallArgumentType::String(err)] if err.contains("changed my mind")
    ), "{result:?}");
    assert_eq!(world.state.get(Column::Storage, &storage_key(&other, b"touched")).unwrap(), None);
}

#[test]
fn only_reentrant_functions_can_be_entered_twice() {
    let mut world = World::new();
    let first = world.lua();
    let second = world.lua();
    let err = world.call(first, "call_back", &[address(second)]).result.unwrap_err();

    assert!(matches!(&err, VmError::Runtime(text) if text.contains("not reentrant")), "{err:?}");
    assert_eq!(
        world.call(first, "bounce", &[address(second)]).result.unwrap(),
        [TxCallArgumentType::String("pong".to_string())]
    );
}

#[test]
fn calls_nest_only_so_deep() {
    let mut world = World::new();
    let chain: Vec<Address> = (0..MAX_CALL_DEPTH + 1).map(|_| world.lua()).collect();
    let args = |targets: &[Address]| targets.iter().copied().map(address).collect::<Vec<_>>();

    assert_eq!(
        world.call(chain[0], "forward", &args(&chain[1..MAX_CALL_DEPTH])).result.unwrap(),
        [TxCallArgumentType::Number((MAX_CALL_DEPTH - 1) as f64)]
    );

    let err = world.call(chain[0], "forward", &args(&chain[1..])).result.unwrap_err();

    assert!(matches!(&err, VmError::Runtime(text) if text.contains("deeper")), "{err:?}");
}

#[test]
fn the_callee_burns_the_caller_s_fuel() {
    let mut world = World::new();
    let counter = world.wasm(WASM_COUNTER);
    let lua = world.lua();
    let alone = world.call(counter, "bump", &[]).fuel_used;
    let through_lua = world.call(lua, "bump_twice", &[address(counter)]).fuel_used;

    assert!(through_lua > 2 * (alone + cost::CALL), "{through_lua} vs {alone}");
}

/// Builds the SDK's example contracts once per test run, for real, with cargo.
fn example(name: &str) -> Vec<u8> {
    static BUILT: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    let dir = BUILT.get_or_init(|| {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let target = root.join("target/contracts");
        let status = std::process::Command::new(env!("CARGO"))
            .args(["build", "--release", "--target", "wasm32-unknown-unknown", "--manifest-path"])
            .arg(root.join("contracts/Cargo.toml"))
            .arg("--target-dir")
            .arg(&target)
            .status()
            .expect("cargo runs");

        assert!(
            status.success(),
            "building the example contracts failed; is the target installed? rustup target add wasm32-unknown-unknown"
        );

        target.join("wasm32-unknown-unknown/release")
    });

    std::fs::read(dir.join(format!("{name}.wasm"))).unwrap()
}

#[test]
fn sdk_contracts_run_and_talk_to_lua() {
    let mut world = World::new();
    let counter = world.deploy(Code::Wasm(example("counter")));
    let proxy = world.deploy(Code::Wasm(example("proxy")));
    let lua = world.lua();
    let record = Contracts::new(&world.state).get(&proxy).unwrap().unwrap();

    assert_eq!(record.functions, ["forward", "ping"]);
    assert_eq!(record.reentrant, ["ping"]);
    let incremented = world.call(counter, "increment", &[TxCallArgumentType::I64(5)]);

    assert_eq!(incremented.events.len(), 1);
    assert_eq!(incremented.events[0].name, "Incremented");
    assert_eq!(incremented.events[0].data, [TxCallArgumentType::I64(5), address(DEPLOYER)]);
    assert_eq!(incremented.result.unwrap(), [TxCallArgumentType::I64(5)]);
    assert_eq!(world.call(counter, "last", &[]).result.unwrap(), [address(DEPLOYER)]);

    // Lua passes its numbers as floats; the SDK reads whole ones without float instructions.
    assert_eq!(
        world.call(lua, "bump_by", &[address(counter)]).result.unwrap(),
        [TxCallArgumentType::Number(12.0)]
    );
    assert_eq!(world.call(counter, "last", &[]).result.unwrap(), [address(lua)]);

    // Lua funds the vault with a valued call, then withdraws part of it again.
    let vault = world.deploy(Code::Wasm(example("vault")));
    world.fund(lua, 100);

    assert_eq!(world.call(lua, "save", &[address(vault), wide(50)]).result.unwrap(), [wide(70), wide(30)]);
    assert_eq!(
        world.call(vault, "total", &[]).result.unwrap(),
        [TxCallArgumentType::Amount(crate::structs::amount::Amount::U256(u256::from(30u64)))]
    );

    let forward = |target: Address, function: &str| {
        world
            .call(proxy, "forward", &[address(target), TxCallArgumentType::String(function.to_string())])
            .result
    };

    assert_eq!(forward(counter, "get").unwrap(), [TxCallArgumentType::I64(12)]);
    assert_eq!(
        forward(lua, "hello").unwrap(),
        [TxCallArgumentType::String(format!("hi from {}", to_hex(proxy.bytes())))]
    );
    assert!(matches!(forward(counter, "nope"), Err(VmError::Runtime(text)) if text.contains("no such function")));

    // The proxy forwards to Lua, which calls back into the proxy: ping is reentrant, forward is not.
    assert_eq!(forward(lua, "ping_caller").unwrap(), [TxCallArgumentType::String("pong".to_string())]);
    assert!(matches!(forward(lua, "forward_caller"), Err(VmError::Runtime(text)) if text.contains("not reentrant")));
}

#[test]
fn events_follow_the_calls_that_succeed() {
    let mut world = World::new();
    let outer = world.lua();
    let inner = world.lua();
    let outcome = world.call(outer, "relay_shout", &[address(inner)]);
    let events: Vec<_> = outcome
        .events
        .iter()
        .map(|event| (event.contract, event.name.as_str(), event.data.clone()))
        .collect();

    outcome.result.unwrap();
    assert_eq!(
        events,
        [
            (outer, "Before", Vec::new()),
            (inner, "Shout", vec![TxCallArgumentType::String("hi".to_string())]),
            (outer, "After", Vec::new()),
        ]
    );
    assert!(world.call(inner, "shout_then_fail", &[]).events.is_empty());
}

#[test]
fn wasm_contracts_emit_through_the_host() {
    // Emits "Hi" with [I64(7)].
    let wat = r#"
(module
  (import "chain" "emit" (func $emit (param i32 i32 i32 i32)))
  (memory (export "memory") 1)
  (data (i32.const 0) "Hi")
  (func (export "greet")
    (i64.store (i32.const 16) (i64.const 1))
    (i32.store8 (i32.const 24) (i32.const 6))
    (i64.store (i32.const 25) (i64.const 7))
    (call $emit (i32.const 0) (i32.const 2) (i32.const 16) (i32.const 17)))
  (func (export "too_long")
    (call $emit (i32.const 0) (i32.const 65) (i32.const 16) (i32.const 17))))
"#;
    let mut world = World::new();
    let greeter = world.wasm(wat);
    let outcome = world.call(greeter, "greet", &[]);

    outcome.result.unwrap();
    assert_eq!(outcome.events.len(), 1);
    assert_eq!(outcome.events[0].name, "Hi");
    assert_eq!(outcome.events[0].data, [TxCallArgumentType::I64(7)]);
    assert!(matches!(world.call(greeter, "too_long", &[]).result, Err(VmError::Runtime(text)) if text.contains("event names")));
}
