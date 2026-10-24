//! Calls between contracts, across engines.

use super::*;
use crate::db::{Column, MemoryDb, storage_key};
use crate::registry::Accounts;
use crate::structs::hex::to_hex;
use crate::registry::Contracts;
use crate::registry::tokens::{Token, Tokens};
use crate::structs::code::Code;
use crate::structs::trace::{Hop, HopKind};

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
            vm: Arc::new(Vm::new(&[Engine::Lua, Engine::Wasm], HashType::default_chain()).unwrap()),
            nonce: 0,
        }
    }

    fn deploy(&mut self, code: Code) -> Address {
        self.nonce += 1;

        Contracts::new(&self.state)
            .deploy(&self.vm, &DEPLOYER, self.nonce, &code, LIMITS, &standard())
            .unwrap()
            .0
    }

    fn upgrade(&self, sender: Address, contract: Address, code: Code) -> Result<crate::registry::contracts::Hooked, crate::registry::RegistryError> {
        Contracts::new(&self.state)
            .upgrade(&self.vm, &sender, &contract, &code, LIMITS, &standard())
            .map_err(|failed| failed.error)
    }

    fn lua(&mut self) -> Address {
        self.deploy(Code::Lua(LUA.to_string()))
    }

    fn wasm(&mut self, wat: &str) -> Address {
        self.deploy(Code::Wasm(wat::parse_str(wat).unwrap()))
    }

    fn call(&self, contract: Address, function: &str, args: &[TxCallArgumentType]) -> Outcome {
        self.call_as(DEPLOYER, contract, function, args)
    }

    fn call_as(&self, sender: Address, contract: Address, function: &str, args: &[TxCallArgumentType]) -> Outcome {
        let context = CallContext {
            sender,
            contract,
            value: u256::ZERO,
        };
        let func = Hash::Xxh3_64(selector(function));

        Contracts::new(&self.state)
            .call(&self.vm, &context, &func, args, LIMITS)
            .unwrap()
    }
}

/// What makes a contract a token on the chains these tests stand for: ERC-20's six.
fn standard() -> Vec<String> {
    crate::registry::tokens::STANDARD_TOKEN_FUNCTIONS.map(str::to_string).to_vec()
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
fn traces_show_every_call_and_which_events_stayed() {
    let mut world = World::new();
    let outer = world.lua();
    let inner = world.lua();
    let outcome = world.call(outer, "relay_shout", &[address(inner)]);
    let hops: Vec<_> = outcome
        .trace
        .hops
        .iter()
        .map(|hop| (hop.depth, hop.kind, hop.from, hop.to, hop.function.as_str(), hop.error.is_some(), hop.events.clone()))
        .collect();

    // Before and After are the receipt's first and third events; Shout, between them, is the
    // inner call's. The failed call's event was dropped.
    assert_eq!(
        hops,
        [
            (0, HopKind::Call, DEPLOYER, outer, "relay_shout", false, vec![0, 2]),
            (1, HopKind::Call, outer, inner, "shout", false, vec![1]),
            (1, HopKind::Call, outer, inner, "shout_then_fail", true, vec![]),
        ]
    );
    assert_eq!(outcome.events[1].name, "Shout");
    assert_eq!(outcome.trace.hops[0].fuel_used, outcome.fuel_used);
    assert!(outcome.trace.hops[1].fuel_used > 0 && outcome.trace.hops[1].fuel_used < outcome.fuel_used);
    assert_eq!(outcome.trace.dropped, 0);
}

#[test]
fn traces_show_transfers_and_where_a_run_failed() {
    let mut world = World::new();
    let payer = world.lua();
    let bob = Address::Ed25519([4; 32]);
    world.fund(payer, 100);
    let paid = world.call(payer, "pay", &[address(bob), wide(30)]);

    paid.result.unwrap();
    assert_eq!(
        paid.trace.hops[1],
        Hop {
            depth: 1,
            kind: HopKind::Transfer,
            from: payer,
            to: bob,
            function: String::new(),
            value: u256::from(30u64),
            fuel_used: 0,
            error: None,
            events: Vec::new(),
        }
    );

    let broke = world.call(payer, "pay", &[address(bob), wide(1_000)]);

    assert!(broke.result.is_err());
    assert!(broke.trace.hops[0].error.is_some());
    assert!(broke.trace.hops[1].error.as_ref().is_some_and(|error| error.contains("insufficient balance")));
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

/// Records its deployer as owner, and lets only the owner upgrade it.
const OWNED_V1: &str = r#"
local M = {}

function M.init()
    storage.set("owner", ctx.sender:hex())
    events.emit("Created", ctx.sender)
end

function M.check_perms(permission, who)
    storage.set("asked", "yes")
    return permission == "upgrade" and who:hex() == storage.get("owner")
end

function M.version()
    return 1
end

function M.call_init(target)
    return contracts.call(target, "init")
end

return M
"#;

/// The next version: keeps the owner, moves storage along, and can be upgraded again.
const OWNED_V2: &str = r#"
local M = {}

function M.migrate()
    storage.set("migrated", "yes")
    events.emit("Migrated", 2)
end

function M.check_perms(permission, who)
    return permission == "upgrade" and who:hex() == storage.get("owner")
end

function M.version()
    return 2, storage.get("owner"), storage.get("migrated")
end

return M
"#;

fn stored(world: &World, contract: Address, key: &[u8]) -> Option<Vec<u8>> {
    world.state.get(Column::Storage, &storage_key(&contract, key)).unwrap()
}

#[test]
fn init_runs_once_at_deploy_and_only_the_chain_may_run_it() {
    let mut world = World::new();
    world.nonce += 1;
    let (owned, hooked) = Contracts::new(&world.state)
        .deploy(&world.vm, &DEPLOYER, world.nonce, &Code::Lua(OWNED_V1.to_string()), LIMITS, &standard())
        .unwrap();

    assert!(hooked.fuel_used > 0);
    assert_eq!(hooked.events.len(), 1);
    assert_eq!(hooked.events[0].name, "Created");
    assert_eq!(stored(&world, owned, b"owner"), Some(to_hex(DEPLOYER.bytes()).into_bytes()));

    let direct = world.call(owned, "init", &[]).result.unwrap_err();
    let other = world.deploy(Code::Lua(OWNED_V1.to_string()));
    let nested = world.call(other, "call_init", &[address(owned)]).result.unwrap_err();

    assert_eq!(direct, VmError::Reserved("init".to_string()));
    assert!(matches!(&nested, VmError::Runtime(text) if text.contains("only run by the chain")), "{nested:?}");
}

#[test]
fn upgrades_need_check_perms_and_keep_storage() {
    let mut world = World::new();
    let owned = world.deploy(Code::Lua(OWNED_V1.to_string()));
    let stranger = Address::Ed25519([5; 32]);

    assert_eq!(
        world.upgrade(stranger, owned, Code::Lua(OWNED_V2.to_string())),
        Err(crate::registry::RegistryError::NotPermitted)
    );
    // What check_perms wrote while being asked is thrown away.
    assert_eq!(stored(&world, owned, b"asked"), None);

    let hooked = world.upgrade(DEPLOYER, owned, Code::Lua(OWNED_V2.to_string())).unwrap();
    let owner = to_hex(DEPLOYER.bytes());

    assert_eq!(hooked.events.len(), 1);
    assert_eq!(hooked.events[0].name, "Migrated");
    assert_eq!(
        world.call(owned, "version", &[]).result.unwrap(),
        [
            TxCallArgumentType::Number(2.0),
            TxCallArgumentType::String(owner),
            TxCallArgumentType::String("yes".to_string()),
        ]
    );
    assert_eq!(world.call(owned, "migrate", &[]).result.unwrap_err(), VmError::Reserved("migrate".to_string()));
}

#[test]
fn contracts_without_check_perms_are_immutable_and_upgrades_may_switch_engines() {
    let mut world = World::new();
    let fixed = world.lua();

    assert_eq!(
        world.upgrade(DEPLOYER, fixed, Code::Lua(OWNED_V2.to_string())),
        Err(crate::registry::RegistryError::NotPermitted)
    );

    // From Lua to WebAssembly; the module has no check_perms, so this is the last upgrade.
    let owned = world.deploy(Code::Lua(OWNED_V1.to_string()));
    world.upgrade(DEPLOYER, owned, Code::Wasm(wat::parse_str(WASM_COUNTER).unwrap())).unwrap();
    let record = Contracts::new(&world.state).get(&owned).unwrap().unwrap();

    assert_eq!(record.engine, Engine::Wasm);
    assert_eq!(record.deployer, DEPLOYER);
    assert_eq!(world.call(owned, "bump", &[]).result.unwrap(), [TxCallArgumentType::I64(1)]);
    assert_eq!(
        world.upgrade(DEPLOYER, owned, Code::Lua(OWNED_V1.to_string())),
        Err(crate::registry::RegistryError::NotPermitted)
    );
}

/// Runs the token standard (docs/token-standard.md) against an example token, whichever engine
/// runs it.
fn conforms_to_the_token_standard(code: Code) {
    let mut world = World::new();
    world.nonce += 1;
    let (token, hooked) = Contracts::new(&world.state)
        .deploy(&world.vm, &DEPLOYER, world.nonce, &code, LIMITS, &standard())
        .unwrap();
    let initial = u256::from(1_000_000u64) * u256::from(10u64).pow(18);
    let zero = address(Address::Ed25519([0; 32]));
    let (alice, bob, carol) = (Address::Ed25519([0xa; 32]), Address::Ed25519([0xb; 32]), Address::Ed25519([0xc; 32]));
    let amount = |value: u64| TxCallArgumentType::U256(u256::from(value));
    let yes = [TxCallArgumentType::Bool(true)];
    let ok = |sender: Address, function: &str, args: &[TxCallArgumentType]| {
        let outcome = world.call_as(sender, token, function, args);
        let events: Vec<_> = outcome.events.iter().map(|event| (event.name.clone(), event.data.clone())).collect();

        (outcome.result.unwrap_or_else(|err| panic!("{function}: {err}")), events)
    };
    let refused = |sender: Address, function: &str, args: &[TxCallArgumentType]| {
        world.call_as(sender, token, function, args).result.unwrap_err().to_string()
    };
    let balance = |owner: Address| ok(DEPLOYER, "balance_of", &[address(owner)]).0;
    let transfer = |from: TxCallArgumentType, to: Address, value: TxCallArgumentType| {
        ("Transfer".to_string(), vec![from, address(to), value])
    };

    // It is a token, as it said.
    assert_eq!(
        Tokens::new(&world.state).get(&token).unwrap(),
        Some(Token {
            address: token,
            name: Some("Example Token".to_string()),
            symbol: Some("EXT".to_string()),
            decimals: Some(18),
        })
    );

    // init minted the whole initial supply to the deployer.
    assert_eq!(hooked.events.len(), 1);
    assert_eq!(hooked.events[0].name, "Transfer");
    assert_eq!(hooked.events[0].data, [zero, address(DEPLOYER), TxCallArgumentType::U256(initial)]);
    assert_eq!(ok(DEPLOYER, "name", &[]).0, [TxCallArgumentType::String("Example Token".to_string())]);
    assert_eq!(ok(DEPLOYER, "symbol", &[]).0, [TxCallArgumentType::String("EXT".to_string())]);
    assert!(matches!(
        ok(DEPLOYER, "decimals", &[]).0[..],
        [TxCallArgumentType::Number(18.0)] | [TxCallArgumentType::U64(18)]
    ));
    assert_eq!(ok(DEPLOYER, "total_supply", &[]).0, [TxCallArgumentType::U256(initial)]);
    assert_eq!(balance(DEPLOYER), [TxCallArgumentType::U256(initial)]);

    // transfer moves the sender's own tokens.
    assert_eq!(
        ok(DEPLOYER, "transfer", &[address(alice), amount(100)]),
        (yes.to_vec(), vec![transfer(address(DEPLOYER), alice, amount(100))])
    );
    assert_eq!(balance(alice), [amount(100)]);
    assert!(refused(alice, "transfer", &[address(bob), amount(101)]).contains("insufficient balance"));

    // approve lets a spender move up to an allowance with transfer_from.
    assert_eq!(
        ok(alice, "approve", &[address(bob), amount(50)]),
        (yes.to_vec(), vec![("Approval".to_string(), vec![address(alice), address(bob), amount(50)])])
    );
    assert_eq!(ok(DEPLOYER, "allowance", &[address(alice), address(bob)]).0, [amount(50)]);
    assert_eq!(
        ok(bob, "transfer_from", &[address(alice), address(carol), amount(30)]),
        (yes.to_vec(), vec![transfer(address(alice), carol, amount(30))])
    );
    assert_eq!(ok(DEPLOYER, "allowance", &[address(alice), address(bob)]).0, [amount(20)]);
    assert_eq!((balance(alice), balance(carol)), (vec![amount(70)], vec![amount(30)]));
    assert!(refused(bob, "transfer_from", &[address(alice), address(carol), amount(21)]).contains("insufficient allowance"));

    // Only the owner mints, and nobody calls init again.
    assert!(refused(bob, "mint", &[address(bob), amount(5)]).contains("only the owner mints"));
    ok(DEPLOYER, "mint", &[address(bob), amount(5)]);
    assert_eq!(
        ok(DEPLOYER, "total_supply", &[]).0,
        [TxCallArgumentType::U256(initial + u256::from(5u64))]
    );
    assert_eq!(world.call(token, "init", &[]).result.unwrap_err(), VmError::Reserved("init".to_string()));

    // Only the owner upgrades.
    assert_eq!(world.upgrade(bob, token, code.clone()), Err(crate::registry::RegistryError::NotPermitted));
    world.upgrade(DEPLOYER, token, code).unwrap();
    assert_eq!(balance(carol), [amount(30)]);
}

/// The six functions that make a token, doing nothing, and a check_perms that lets anyone upgrade.
const BARE_TOKEN: &str = r#"
local M = {}

function M.total_supply() return 0 end
function M.balance_of(owner) return 0 end
function M.transfer(to, amount) return true end
function M.approve(spender, amount) return true end
function M.allowance(owner, spender) return 0 end
function M.transfer_from(from, to, amount) return true end
function M.check_perms(permission, who) return true end

return M
"#;

/// BARE_TOKEN with more functions, or with one taken out.
fn token_with(extra: &str, without: Option<&str>) -> Code {
    let mut source = BARE_TOKEN.replace("\nreturn M", &format!("\n{extra}\nreturn M"));

    if let Some(function) = without {
        source = source.lines().filter(|line| !line.starts_with(&format!("function M.{function}("))).collect::<Vec<_>>().join("\n");
    }

    Code::Lua(source)
}

#[test]
fn contracts_that_export_the_six_are_tokens_and_their_metadata_is_optional() {
    let mut world = World::new();
    let tokens = |world: &World, address: Address| Tokens::new(&world.state).get(&address).unwrap();

    // The six alone make a token; name, symbol and decimals are extras.
    let bare = world.deploy(token_with("", None));

    assert_eq!(tokens(&world, bare), Some(Token { address: bare, name: None, symbol: None, decimals: None }));
    let plain = world.lua();

    assert_eq!(tokens(&world, plain), None, "a contract short of them is not one");

    // Those it exports are asked, on a copy of state that is thrown away.
    let described = world.deploy(token_with(
        r#"function M.symbol() storage.set("asked", "yes") events.emit("Asked") return "BARE" end
function M.decimals() return 6 end"#,
        None,
    ));

    assert_eq!(tokens(&world, described).unwrap().symbol.as_deref(), Some("BARE"));
    assert_eq!(tokens(&world, described).unwrap().decimals, Some(6));
    assert_eq!(stored(&world, described, b"asked"), None);

    // Answers the registry cannot keep fail the deploy.
    for (extra, why) in [
        ("function M.symbol() return 5 end", "symbol() must return one string"),
        ("function M.symbol() return \"\" end", "symbol() must give 1 to 16 bytes, not 0"),
        ("function M.name() return string.rep(\"n\", 65) end", "name() must give 1 to 64 bytes, not 65"),
        ("function M.name() return \"two\\nlines\" end", "name() gave control characters"),
        ("function M.decimals() return 78 end", "decimals() must return one whole number from 0 to 77"),
        ("function M.decimals() return 1.5 end", "decimals() must return one whole number from 0 to 77"),
        ("function M.name() error(\"no\") end", "name() failed"),
    ] {
        world.nonce += 1;
        let failed = Contracts::new(&world.state)
            .deploy(&world.vm, &DEPLOYER, world.nonce, &token_with(extra, None), LIMITS, &standard())
            .unwrap_err();

        assert!(failed.fuel_used > 0, "{extra}: asking cost fuel");
        assert!(
            matches!(&failed.error, crate::registry::RegistryError::BadToken(text) if text.contains(why)),
            "{extra}: {}",
            failed.error
        );
    }

    // An upgrade short of one of the six takes it off; one that brings them back puts it on again.
    world.upgrade(DEPLOYER, bare, token_with("", Some("approve"))).unwrap();

    assert_eq!(tokens(&world, bare), None);

    world.upgrade(DEPLOYER, bare, token_with("function M.name() return \"Bare\" end", None)).unwrap();

    assert_eq!(tokens(&world, bare).unwrap().name.as_deref(), Some("Bare"));

    // Genesis says what makes a token: other functions, or none, which keeps no registry.
    world.nonce += 1;
    let (minimal, _) = Contracts::new(&world.state)
        .deploy(&world.vm, &DEPLOYER, world.nonce, &token_with("", Some("approve")), LIMITS, &["transfer".to_string()])
        .unwrap();
    world.nonce += 1;
    let (unlisted, _) = Contracts::new(&world.state)
        .deploy(&world.vm, &DEPLOYER, world.nonce, &token_with("", None), LIMITS, &[])
        .unwrap();

    assert!(tokens(&world, minimal).is_some());
    assert_eq!(tokens(&world, unlisted), None);
}

/// A token whose name its caller can change, asking the registry to ask again when it does.
const RENAMING: &str = r#"function M.name() return storage.get("name") or "First" end
function M.rename(name) storage.set("name", name) tokens.refresh() end"#;

#[test]
fn a_token_s_request_to_be_asked_again_reaches_the_transaction_only_if_its_calls_succeed() {
    let mut world = World::new();
    let token = world.deploy(token_with(RENAMING, None));
    let caller = world.deploy(Code::Lua(
        r#"
local M = {}

function M.rename(token, name, fail)
    contracts.call(token, "rename", name)
    if fail then error("changed my mind") end
end

return M
"#
        .to_string(),
    ));
    let rename = |name: &str, fail: bool| {
        world.call(caller, "rename", &[address(token), TxCallArgumentType::String(name.to_string()), TxCallArgumentType::Bool(fail)])
    };

    assert_eq!(world.call(token, "rename", &[TxCallArgumentType::String("Second".into())]).refresh_tokens, [token]);
    assert_eq!(rename("Third", false).refresh_tokens, [token], "from a call it made");

    let failed = rename("Fourth", true);

    assert!(failed.result.is_err());
    assert!(failed.refresh_tokens.is_empty(), "gone with the call that failed");

    // WebAssembly asks the same way, and it costs fuel.
    let poke = world.wasm(
        r#"(module (import "chain" "refresh_token" (func $refresh_token)) (memory (export "memory") 1)
             (func (export "poke") (call $refresh_token)))"#,
    );
    let poked = world.call(poke, "poke", &[]);

    assert_eq!(poked.refresh_tokens, [poke]);
    assert!(poked.fuel_used >= crate::vm::cost::TOKEN_REFRESH);

    // The registry asks again when told to, and the answer stands.
    Contracts::new(&world.state).refresh(&world.vm, &token, DEPLOYER, LIMITS, &standard()).unwrap();

    assert_eq!(Tokens::new(&world.state).get(&token).unwrap().unwrap().name.as_deref(), Some("Third"));
}

#[test]
fn the_lua_token_follows_the_standard() {
    conforms_to_the_token_standard(Code::Lua(include_str!("../../contracts/lua/token.lua").to_string()));
}

#[test]
fn typed_exports_refuse_arguments_that_do_not_fit() {
    let mut world = World::new();
    let token = world.deploy(Code::Wasm(example("token")));
    let counter = world.deploy(Code::Wasm(example("counter")));
    let refused = |contract: Address, function: &str, args: &[TxCallArgumentType]| match world.call(contract, function, args).result {
        Err(VmError::Runtime(text)) => text,
        other => panic!("{function}: {other:?}"),
    };
    let bob = address(Address::Ed25519([0xb; 32]));

    assert!(refused(token, "transfer", &[bob.clone(), TxCallArgumentType::String("ten".into())]).contains("amount: expected an amount"));
    assert!(refused(token, "transfer", std::slice::from_ref(&bob)).contains("amount: missing"));
    assert!(refused(token, "transfer", &[bob.clone(), TxCallArgumentType::U64(1), TxCallArgumentType::U64(2)]).contains("transfer takes 2 arguments, not 3"));
    assert!(refused(token, "name", &[TxCallArgumentType::U64(1)]).contains("name takes 0 arguments, not 1"));
    // A Result's error aborts the call.
    let minted = world.call_as(Address::Ed25519([0xb; 32]), token, "mint", &[bob, TxCallArgumentType::U64(1)]);

    assert!(matches!(minted.result, Err(VmError::Runtime(text)) if text.contains("only the owner mints")));

    // Option parameters may be left out or passed nil, and None returns nil.
    assert_eq!(world.call(counter, "last", &[]).result.unwrap(), [TxCallArgumentType::Nil]);
    assert_eq!(world.call(counter, "increment", &[]).result.unwrap(), [TxCallArgumentType::I64(1)]);
    assert_eq!(world.call(counter, "increment", &[TxCallArgumentType::Nil]).result.unwrap(), [TxCallArgumentType::I64(2)]);
    assert_eq!(world.call(counter, "increment", &[TxCallArgumentType::Number(3.0)]).result.unwrap(), [TxCallArgumentType::I64(5)]);
}

#[test]
fn the_rust_token_follows_the_standard() {
    conforms_to_the_token_standard(Code::Wasm(example("token")));
}

const COUNTER_V1: &str = r#"
local M = {}

function M.increment(by)
    local count = tonumber(storage.get("count") or "0") + by
    storage.set("count", tostring(count))
    events.emit("Counted", count, ctx.sender, ctx.contract)
    return count
end

function M.count()
    return tonumber(storage.get("count") or "0")
end

function M.paid()
    return ctx.value, accounts.balance(ctx.contract)
end

function M.init()
    storage.set("count", "1000")
end

return M
"#;

/// Reads the same storage as V1, and adds decrement.
const COUNTER_V2: &str = r#"
local M = {}

function M.count()
    return tonumber(storage.get("count") or "0")
end

function M.decrement(by)
    local count = tonumber(storage.get("count") or "0") - by
    storage.set("count", tostring(count))
    return count
end

return M
"#;

const DELEGATOR: &str = r#"
local M = {}

function M.delegate_to(target, name)
    return contracts.delegate(target, name)
end

function M.pay(proxy, amount)
    return contracts.call_value(proxy, amount, "paid")
end

return M
"#;

fn proxy(world: &mut World, code: Code, implementation: Address) -> Address {
    let proxy = world.deploy(code);
    world.call(proxy, "set_implementation", &[address(implementation)]).result.unwrap();

    proxy
}

fn upgradable_proxies_run_their_implementation_on_their_own_storage(proxy_code: Code) {
    let mut world = World::new();
    let alice = Address::Ed25519([0xa; 32]);
    let v1 = world.lua_code(COUNTER_V1);
    let proxy = proxy(&mut world, proxy_code, v1);
    let increment = world.call_as(alice, proxy, "increment", &[TxCallArgumentType::I64(5)]);

    // The code is V1's, but the storage, the sender and the events are the proxy's.
    assert_eq!(increment.result.unwrap(), [TxCallArgumentType::Number(5.0)]);
    assert_eq!(increment.events.len(), 1);
    assert_eq!(increment.events[0].contract, proxy);
    assert_eq!(increment.events[0].data, [TxCallArgumentType::Number(5.0), address(alice), address(proxy)]);
    assert_eq!(stored(&world, proxy, b"count"), Some(b"5".to_vec()));
    assert_eq!(stored(&world, v1, b"count"), Some(b"1000".to_vec()));

    // Upgrading is repointing: storage stays, and the new code's functions appear.
    let v2 = world.lua_code(COUNTER_V2);
    let refused = world.call_as(alice, proxy, "set_implementation", &[address(v2)]).result.unwrap_err();

    assert!(refused.to_string().contains("only the owner"), "{refused}");

    world.call(proxy, "set_implementation", &[address(v2)]).result.unwrap();

    assert_eq!(world.call(proxy, "decrement", &[TxCallArgumentType::I64(2)]).result.unwrap(), [TxCallArgumentType::Number(3.0)]);
    assert!(matches!(world.call(proxy, "increment", &[TxCallArgumentType::I64(1)]).result, Err(VmError::Runtime(text)) if text.contains("no such function")));

    // Value and balance are the proxy's own.
    world.call(proxy, "set_implementation", &[address(v1)]).result.unwrap();
    let payer = world.lua_code(DELEGATOR);
    world.fund(payer, 100);

    assert_eq!(world.call(payer, "pay", &[address(proxy), wide(40)]).result.unwrap(), [wide(40), wide(40)]);
}

impl World {
    fn lua_code(&mut self, source: &str) -> Address {
        self.deploy(Code::Lua(source.to_string()))
    }
}

#[test]
fn a_lua_proxy_delegates_through_fallback() {
    upgradable_proxies_run_their_implementation_on_their_own_storage(Code::Lua(include_str!("../../contracts/lua/proxy.lua").to_string()));
}

#[test]
fn a_rust_proxy_delegates_to_lua_code() {
    upgradable_proxies_run_their_implementation_on_their_own_storage(Code::Wasm(example("upgradable")));
}

#[test]
fn a_lua_proxy_delegates_to_rust_code() {
    let mut world = World::new();
    let counter = world.deploy(Code::Wasm(example("counter")));
    let proxy = proxy(&mut world, Code::Lua(include_str!("../../contracts/lua/proxy.lua").to_string()), counter);
    let alice = Address::Ed25519([0xa; 32]);
    let increment = world.call_as(alice, proxy, "increment", &[TxCallArgumentType::I64(7)]);

    // Values pass through the Lua proxy untouched, so the counter's I64 comes back an I64.
    assert_eq!(increment.result.unwrap(), [TxCallArgumentType::I64(7)]);
    assert_eq!(increment.events[0].contract, proxy);
    assert_eq!(increment.events[0].data, [TxCallArgumentType::I64(7), address(alice)]);
    assert_eq!(world.call(proxy, "last", &[]).result.unwrap(), [address(alice)]);
    assert_eq!(world.call(counter, "get", &[]).result.unwrap(), [TxCallArgumentType::I64(0)]);

    // The proxy delegated by selector; the trace names what it ran, and the event is its own.
    let hops = &increment.trace.hops;

    assert_eq!((hops[0].from, hops[0].to, hops[0].function.as_str()), (alice, proxy, "fallback"));
    assert_eq!(hops[0].events, Vec::<u32>::new());
    assert_eq!(hops.last().map(|hop| (hop.kind, hop.from, hop.to, hop.function.as_str())), Some((HopKind::Delegate, proxy, counter, "increment")));
    assert_eq!(hops.last().unwrap().events, [0]);
}

#[test]
fn hooks_and_fallbacks_are_never_called_or_delegated_to() {
    let mut world = World::new();
    let v1 = world.lua_code(COUNTER_V1);
    let proxy = proxy(&mut world, Code::Lua(include_str!("../../contracts/lua/proxy.lua").to_string()), v1);
    let delegator = world.lua_code(DELEGATOR);
    let delegated = |target: Address, name: &str| {
        world
            .call(delegator, "delegate_to", &[address(target), TxCallArgumentType::String(name.to_string())])
            .result
            .unwrap_err()
            .to_string()
    };

    assert_eq!(world.call(proxy, "fallback", &[]).result.unwrap_err(), VmError::Reserved("fallback".to_string()));
    assert!(delegated(v1, "init").contains("init is only run by the chain"));
    assert!(delegated(proxy, "fallback").contains("fallback is only run by the chain"));
    // Delegating never falls back: the target must export the function itself.
    assert!(delegated(proxy, "increment").contains("no such function"));
    assert!(delegated(Address::Contract([9; 32]), "count").contains("no contract"));
}

/// Passes calls on with values lists, and shows what converting them would do instead.
const RELAY: &str = r#"
local M = {}

function M.point_at(target)
    storage.set("target", target:text())
end

-- Every call it does not export goes on to the target's echo, values untouched both ways.
function M.fallback(selector)
    return contracts.call(address.parse(storage.get("target")), "echo", ctx.args)
end

function M.echo()
    return ctx.args
end

function M.converted(...)
    return ...
end

function M.unpacked()
    return ctx.args:unpack()
end

function M.nothing()
    return nil
end

function M.describe()
    local list = ctx.args

    return #list, list:type(1), list:type(#list + 1) == nil, tostring(list), rawequal(list, ctx.args)
end

function M.announce()
    events.emit("Raw", ctx.args)
end

function M.mixed()
    return 1, ctx.args
end

return M
"#;

#[test]
fn values_lists_pass_through_lua_untouched() {
    let mut world = World::new();
    let relay = world.lua_code(RELAY);
    let target = world.lua_code(RELAY);
    world.call(relay, "point_at", &[address(target)]).result.unwrap();
    let sent = vec![
        TxCallArgumentType::I64(-5),
        TxCallArgumentType::Bytes(b"hi".to_vec()),
        TxCallArgumentType::U64(7),
        TxCallArgumentType::Amount(crate::structs::amount::Amount::U128(9)),
        TxCallArgumentType::Address(DEPLOYER),
    ];

    // Through the relay's fallback, a call, the target's echo and back: nothing changes.
    assert_eq!(world.call(relay, "anything", &sent).result.unwrap(), sent);
    assert_eq!(world.call(target, "echo", &sent).result.unwrap(), sent);

    // Converted through Lua, the same values change type.
    assert_eq!(
        world.call(target, "converted", &sent).result.unwrap(),
        [
            TxCallArgumentType::Number(-5.0),
            TxCallArgumentType::String("hi".to_string()),
            TxCallArgumentType::U256(u256::from(7u64)),
            TxCallArgumentType::U256(u256::from(9u64)),
            TxCallArgumentType::Address(DEPLOYER),
        ]
    );
    assert_eq!(world.call(target, "unpacked", &sent).result.unwrap()[0], TxCallArgumentType::Number(-5.0));
    // Its length and types can be read, and ctx.args is one list however often it is read.
    assert_eq!(
        world.call(target, "describe", &sent).result.unwrap(),
        [
            TxCallArgumentType::Number(5.0),
            TxCallArgumentType::String("i64".to_string()),
            TxCallArgumentType::Bool(true),
            TxCallArgumentType::String("values(5)".to_string()),
            TxCallArgumentType::Bool(true),
        ]
    );

    // nil crosses both ways, among other values or alone, and a values list names it.
    let with_nil = [TxCallArgumentType::I64(1), TxCallArgumentType::Nil, TxCallArgumentType::Bool(true)];

    assert_eq!(
        world.call(target, "converted", &with_nil).result.unwrap(),
        [TxCallArgumentType::Number(1.0), TxCallArgumentType::Nil, TxCallArgumentType::Bool(true)]
    );
    assert_eq!(world.call(relay, "anything", &with_nil).result.unwrap(), with_nil);
    assert_eq!(world.call(target, "nothing", &[]).result.unwrap(), [TxCallArgumentType::Nil]);

    let announced = world.call(target, "announce", &sent);

    assert_eq!(announced.events[0].data, sent);
    assert!(matches!(world.call(target, "mixed", &sent).result, Err(err) if err.to_string().contains("goes alone")));
}

#[test]
fn deploys_and_upgrades_pay_for_loading_and_for_failing() {
    use crate::registry::contracts::Failed;

    let mut world = World::new();
    let deploy = |world: &mut World, source: &str| {
        world.nonce += 1;

        Contracts::new(&world.state).deploy(&world.vm, &DEPLOYER, world.nonce, &Code::Lua(source.to_string()), LIMITS, &standard())
    };

    // A deploy pays for loading its code as well as for init.
    let (owned, hooked) = deploy(&mut world, OWNED_V1).unwrap();

    assert!(hooked.fuel_used > OWNED_V1.len() as u64 * cost::LOAD_BYTE);

    // A top level that never ends burns everything; an init that fails burns what it got to.
    let Err(Failed { error, fuel_used, .. }) = deploy(&mut world, "while true do end") else {
        panic!("an endless top level deploys");
    };

    assert_eq!(error, crate::registry::RegistryError::Vm(VmError::OutOfFuel));
    assert!(fuel_used > LIMITS.fuel - 1_000);

    let failing_init = "local M = {}\nfunction M.init()\n    for i = 1, 5000 do end\n    error(\"no\")\nend\nreturn M";
    let Err(failed) = deploy(&mut world, failing_init) else {
        panic!("a failing init deploys");
    };

    assert!(failed.fuel_used > failing_init.len() as u64 + 5_000);
    assert_eq!(failed.trace.hops[0].function, "init");
    assert!(failed.trace.hops[0].error.is_some());

    // check_perms saying no still pays for asking it.
    let stranger = Address::Ed25519([0xb; 32]);
    let refused = Contracts::new(&world.state)
        .upgrade(&world.vm, &stranger, &owned, &Code::Lua(OWNED_V1.to_string()), LIMITS, &standard())
        .unwrap_err();

    assert_eq!(refused.error, crate::registry::RegistryError::NotPermitted);
    assert!(refused.fuel_used > 0);
    assert_eq!(refused.trace.hops[0].function, "check_perms");
}

/// Peeks at the call from its top level, which runs before the call is attached.
const PEEKER: &str = r#"
local seen = pcall(function()
    return ctx.sender
end)

local M = {}

function M.saw()
    return seen
end

return M
"#;

#[test]
fn a_top_level_sees_nothing_of_the_call() {
    let mut world = World::new();
    let peeker = world.lua_code(PEEKER);

    assert_eq!(world.call(peeker, "saw", &[]).result.unwrap(), [TxCallArgumentType::Bool(false)]);
}
