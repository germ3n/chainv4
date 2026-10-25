use super::*;
use crate::crypto::hasher::Hasher;
use crate::db::{MemoryDb, State};
use crate::structs::address::Address;
use crate::structs::amount::Amount;
use crate::structs::hash::HashType;
use crate::structs::u256::{i256, u256};
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

function M.half()
    return 1 / 2
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

function M.kinds(...)
    local kinds = {}

    for index = 1, select('#', ...) do
        kinds[index] = typeof((select(index, ...)))
    end

    return table.concat(kinds, " ")
end

-- Every comparison with a 5 of any integer type holds, through the compiled jumps.
function M.compare(x)
    local n, m = tonumber("5"), tonumber("6")
    local hits = 0

    if x == 5 then hits += 1 end
    if 5 == x then hits += 1 end
    if x ~= 6 then hits += 1 end
    if x == n then hits += 1 end
    if x ~= m then hits += 1 end
    if x < 6 then hits += 1 end
    if x > 4 then hits += 1 end
    if x <= n then hits += 1 end
    if x >= n then hits += 1 end
    if x < m then hits += 1 end
    if x == u64(5) then hits += 1 end
    if x == i256(5) then hits += 1 end
    if x < u256(6) and x > i64(4) then hits += 1 end
    if x ~= 5 or x < 5 or x > 5 or x == 4 then hits += 100 end

    return hits
end

function M.arithmetic(a, b)
    return a + b, a - 1, a * 2, a // 2, b:checked_add(a), (a:wrapping_sub(a + 1))
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
    let vm = Arc::new(Vm::new(&[Engine::Lua], HashType::default_chain()).unwrap());

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

    assert_eq!(sum, [TxCallArgumentType::U64((1 << 60) + 3)]);
    assert_eq!(contract().function_name(&func("add")), Some("add"));
}

#[test]
fn unknown_functions_are_rejected() {
    assert_eq!(call("nope", &[]).result.unwrap_err(), VmError::NoSuchFunction);

    // Only XXH3 names functions, so the right name under any other hash is no function at all.
    let wrong = Hasher::digest(HashType::default_chain(), b"add");

    assert_eq!(
        contract().call(&wrong, &[], host(), LIMITS).result.unwrap_err(),
        VmError::NoSuchFunction
    );
}

#[test]
fn every_call_starts_from_a_fresh_state() {
    let contract = contract();

    for _ in 0..2 {
        let result = contract.call(&func("count"), &[], host(), LIMITS).result.unwrap();

        assert_eq!(result, [TxCallArgumentType::I64(1)]);
    }
}

#[test]
fn states_from_the_engine_s_pool_are_as_fresh_as_new_ones() {
    let engine = LuaEngine::new();
    let loaded = engine.load(SCRIPT.as_bytes(), LIMITS).result.unwrap();
    let fresh = contract().call(&func("count"), &[], host(), LIMITS);

    // More calls than the pool holds, so some take a waiting state and some build their own.
    for _ in 0..POOL_SIZE * 3 {
        let pooled = engine.run(&loaded.code, "count", &[], host(), LIMITS);

        assert_eq!(pooled.result, fresh.result, "top-level locals start over every call");
        assert_eq!(pooled.fuel_used, fresh.fuel_used, "and so does the fuel");
    }

    // The memory limit gives every call the same room.
    let hog = contract().call(&func("hog"), &[], host(), LIMITS);

    assert_eq!(hog.result, Err(VmError::OutOfMemory));
    assert_eq!(engine.run(&loaded.code, "hog", &[], host(), LIMITS).result, hog.result);
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
        TxCallArgumentType::Number(2.0),
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
            TxCallArgumentType::I64(2),
            TxCallArgumentType::I64(-5),
            TxCallArgumentType::Address(Address::Ed25519([9, ..])),
            TxCallArgumentType::U256(seven),
        ] if text == "hi" && bytes[..] == [0xff, 0x00] && *seven == u256::new(7)
    ), "numbers come back whole, as i64: {result:?}");
}

#[test]
fn numbers_cross_a_contracts_edges_whole() {
    for fraction in [1.5, f64::INFINITY, f64::NAN, 9007199254740994.0] {
        let err = call("echo", &[TxCallArgumentType::Number(fraction)]).result.unwrap_err();

        assert!(err.to_string().contains("a Lua contract takes whole numbers"), "{fraction}: {err}");
    }

    let err = call("half", &[]).result.unwrap_err();

    assert!(err.to_string().contains("numbers leave a contract whole"), "{err}");
}

#[test]
fn integers_cross_as_their_own_types_and_come_back_exact() {
    let args = [
        TxCallArgumentType::U64(u64::MAX),
        TxCallArgumentType::U128(u128::MAX),
        TxCallArgumentType::U256(u256::MAX),
        TxCallArgumentType::I64(i64::MIN),
        TxCallArgumentType::I128(i128::MIN),
        TxCallArgumentType::I256(i256::MIN),
        TxCallArgumentType::I64(i64::MAX),
        TxCallArgumentType::I256(i256::MAX),
    ];

    assert_eq!(returns("echo", &args), args);

    let kinds = [
        args.to_vec(),
        vec![TxCallArgumentType::Amount(Amount::U128(7)), TxCallArgumentType::Number(3.0)],
    ]
    .concat();

    assert_eq!(
        returns("kinds", &kinds),
        [TxCallArgumentType::String("U64 U128 U256 I64 I128 I256 I64 I256 U256 number".to_string())]
    );
}

#[test]
fn integers_compare_with_numbers_and_each_other_in_a_contract() {
    let fives = [
        TxCallArgumentType::U64(5),
        TxCallArgumentType::U128(5),
        TxCallArgumentType::U256(u256::new(5)),
        TxCallArgumentType::I64(5),
        TxCallArgumentType::I128(5),
        TxCallArgumentType::I256(i256::new(5)),
        TxCallArgumentType::Amount(Amount::U128(5)),
        TxCallArgumentType::Number(5.0),
    ];

    for five in fives {
        assert_eq!(returns("compare", std::slice::from_ref(&five)), [TxCallArgumentType::I64(13)], "{five:?}");
    }
}

#[test]
fn arithmetic_results_leave_as_their_type() {
    assert_eq!(
        returns("arithmetic", &[TxCallArgumentType::I64(-7), TxCallArgumentType::I128(i128::MAX)]),
        [
            TxCallArgumentType::I128(i128::MAX - 7),
            TxCallArgumentType::I64(-8),
            TxCallArgumentType::I64(-14),
            TxCallArgumentType::I64(-4),
            TxCallArgumentType::I128(i128::MAX - 7),
            TxCallArgumentType::I64(-1),
        ]
    );
    assert_eq!(
        returns("arithmetic", &[TxCallArgumentType::U64(10), TxCallArgumentType::U128(u128::MAX - 10)]),
        [
            TxCallArgumentType::U128(u128::MAX),
            TxCallArgumentType::U64(9),
            TxCallArgumentType::U64(20),
            TxCallArgumentType::U64(5),
            TxCallArgumentType::U128(u128::MAX),
            TxCallArgumentType::U64(u64::MAX),
        ]
    );

    let err = call("arithmetic", &[TxCallArgumentType::U64(u64::MAX), TxCallArgumentType::U64(1)]).result.unwrap_err();

    assert!(err.to_string().contains("u64 overflow"), "{err}");

    let err = call("arithmetic", &[TxCallArgumentType::U64(1), TxCallArgumentType::I64(1)]).result.unwrap_err();

    assert!(err.to_string().contains("U64 and I64 do not mix"), "{err}");
}

#[test]
fn an_endless_loop_runs_out_of_fuel() {
    let outcome = call("spin", &[]);

    assert_eq!(outcome.result.unwrap_err(), VmError::OutOfFuel);
    assert!(outcome.fuel_used > LIMITS.fuel - MAX_INSTRUCTION_COST);
    assert!(matches!(Contract::load("while true do end", LIMITS), Err(VmError::OutOfFuel)));
}

#[test]
fn loading_pays_for_compiling_and_the_top_level() {
    let engine = LuaEngine::new();
    let light = "local M = {}\nfunction M.f() end\nreturn M";
    let heavy = "local n = 0\nfor i = 1, 20000 do n = n + i end\nlocal M = {}\nfunction M.f() end\nreturn M";
    let light_loading = engine.load(light.as_bytes(), LIMITS);
    let heavy_loading = engine.load(heavy.as_bytes(), LIMITS);

    assert!(light_loading.result.is_ok() && heavy_loading.result.is_ok());
    assert!(light_loading.fuel_used > light.len() as u64 * crate::vm::cost::LOAD_BYTE, "the bytes, and the top level");
    assert!(heavy_loading.fuel_used > light_loading.fuel_used + 20_000, "the loop is paid for");

    // Failing to load still pays: a compile error for its bytes, an endless top level for all of
    // its fuel, and code the fuel cannot even compile for all there was.
    let broken = engine.load(b"local = 1", LIMITS);

    assert!(matches!(broken.result, Err(VmError::Compile(_))));
    assert_eq!(broken.fuel_used, 9 * crate::vm::cost::LOAD_BYTE);

    let endless = engine.load(b"while true do end", LIMITS);

    assert_eq!(endless.result.unwrap_err(), VmError::OutOfFuel);
    assert!(endless.fuel_used > LIMITS.fuel - MAX_INSTRUCTION_COST);

    let starved = engine.load(light.as_bytes(), Limits { fuel: 10, ..LIMITS });

    assert_eq!((starved.result.unwrap_err(), starved.fuel_used), (VmError::OutOfFuel, 10));
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

#[test]
fn no_memory_address_reaches_a_result_or_a_failure() {
    let script = r#"
local M = {}

function M.texts()
    local named = setmetatable({}, { __tostring = function() return "named" end })

    return tostring({}), tostring(print), tostring(named), tostring(u256(5)), tostring(address("0x" .. string.rep("07", 32)))
end

function M.thrown()
    error({})
end

return M
"#;
    // Other states alive alongside move each run's objects to other addresses.
    let run = |name: &str, others: usize| {
        let _others: Vec<Contract> = (0..others).map(|_| Contract::load("return {}", LIMITS).unwrap()).collect();

        Contract::load(script, LIMITS).unwrap().call(&func(name), &[], host(), LIMITS).result
    };
    let texts = run("texts", 0).unwrap();
    let text = |value: &str| TxCallArgumentType::String(value.to_string());

    assert_eq!(texts, [text("table"), text("function"), text("named"), text("5"), text(&format!("0x{}", "07".repeat(32)))]);
    assert_eq!(run("texts", 3).unwrap(), texts, "the same in any state");

    let thrown = run("thrown", 0).unwrap_err().to_string();

    assert!(thrown.contains("runtime error: table\n") && !thrown.contains("0x"), "{thrown}");
    assert_eq!(run("thrown", 3).unwrap_err().to_string(), thrown);
}

#[test]
fn addresses_are_cut_from_error_text() {
    assert_eq!(without_addresses("runtime error: table: 0x7cb4c831b8\nmore"), "runtime error: table\nmore");
    assert_eq!(without_addresses("a function: 0x00ff and userdata: 0xAB"), "a function and userdata");
    assert_eq!(without_addresses("sent to ed25519:0xab12, 7: 0x"), "sent to ed25519:0xab12, 7: 0x", "only a value's address");
}

#[test]
fn tables_keyed_by_tables_iterate_alike_in_any_state() {
    let script = r#"
local M = {}

-- Strings sized like a table, 48 bytes and 56 with its id, take the slots tables would have had,
-- so the tables made after sit elsewhere; strings have no id, so the tables' ids stay the same.
function M.order(padding)
    local held = {}

    for i = 1, padding do
        held[2 * i - 1] = string.format("%023d", i)
        held[2 * i] = string.format("%031d", i)
    end

    local byTable, byFunction = {}, {}

    for i = 1, 16 do
        byTable[{ i }] = i
        byFunction[function() return i end] = i
    end

    local seen = {}

    for _, value in pairs(byTable) do
        table.insert(seen, value)
    end

    for _, value in byFunction do
        table.insert(seen, value)
    end

    return table.concat(seen, ",")
end

function M.keyed_by_address()
    local held = {}
    held[ctx.sender] = 1
end

function M.read_by_address()
    local held = { [ctx.sender:text()] = 1 }

    return held[ctx.sender] == nil
end

return M
"#;
    let run = |name: &str, args: &[TxCallArgumentType]| Contract::load(script, LIMITS).unwrap().call(&func(name), args, host(), LIMITS).result;
    let order = |padding: i64| run("order", &[TxCallArgumentType::Number(padding as f64)]).unwrap();
    let first = order(0);

    for padding in 1..12 {
        assert_eq!(order(padding), first, "hashed by the order they were made in, not where they sit");
    }

    let refused = run("keyed_by_address", &[]).unwrap_err().to_string();

    assert!(refused.contains("table index is a userdata; use a string or a number"), "{refused}");
    assert_eq!(run("read_by_address", &[]).unwrap(), [TxCallArgumentType::Bool(true)], "reading by one finds nothing");
}

