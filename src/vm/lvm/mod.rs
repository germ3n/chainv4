//! The Lua VM. Contracts are Luau scripts run on the forked Luau, which charges fuel for every
//! instruction. Every call gets a fresh Lua state, so nothing one call leaves behind can reach
//! the next, and a node that restarts computes exactly what one that never stopped does. Fresh
//! matters for more than globals: a script's top-level locals live on as upvalues, and a state
//! that ran something before holds its garbage, which the collector frees at its own pace, so it
//! could fail a call another node passes. Building a state is most of what a call costs, so the
//! engine builds them ahead, on a thread of its own, and still uses each one once.

use crate::params::vm::{LUA_FUEL_OPCODES, LuaCosts};
use {
    super::{Host, Limits, Loaded, Loading, Outcome, ScriptEngine, VmError, selector},
    crate::structs::hash::Hash,
    crate::structs::hex::{from_hex, to_hex},
    crate::structs::trace::Trace,
    crate::structs::tx::TxCallArgumentType,
    libs::{
        ctx::CallArgs, register_accounts_lib, register_address_lib, register_contracts_lib, register_ctx_lib,
        register_events_lib, register_hash_lib, register_storage_lib, register_tokens_lib, register_crypto_lib, register_federation_lib, register_chain_lib, register_integer_libs, register_names_lib, register_evm_lib, register_perms_lib, perms::Module, values::Given,
    },
    mlua::{chunk::ChunkMode, Function, Lua, LuaOptions, MultiValue, StdLib, Table, Value},
    r#macro::document,
    std::collections::HashMap,
    std::sync::{Mutex, OnceLock, mpsc},
};

pub mod annotations;
pub mod compiler;
pub mod libs;
mod value;

#[document(summary = "How many pristine Lua states the engine keeps ready, unless --lua-states says otherwise: 16.", category = "Lua VM", see_also = "EnginePools::lua_states")]
pub const POOL_SIZE: usize = 16;
#[document(summary = "How many threads build pristine Lua states, unless --lua-builders says otherwise: 2. Building one takes about twice as long as a call that uses it, so two keep up with calls back to back.", category = "Lua VM", see_also = "EnginePools::lua_builders")]
pub const BUILDERS: usize = 2;


/// Globals a script may use. Everything else is left out, including anything that reads the
/// clock, the platform or the garbage collector, since nodes would disagree on those.
const GLOBALS: &[&str] = &[
    "assert", "error", "getmetatable", "ipairs", "next", "pairs", "pcall", "print", "rawequal",
    "rawget", "rawlen", "rawset", "select", "setmetatable", "tonumber", "tostring", "type",
    "typeof", "unpack", "xpcall", "string", "table", "utf8", "bit32", "address", "hash", "u64",
    "u128", "u256", "i64", "i128", "i256", "ctx", "storage", "contracts", "accounts", "events",
    "tokens", "collections", "names", "evm", "crypto", "federation", "chain", "perms",
];

/// What scripts keep of math: what rounds to and works on whole numbers. Contracts deal in whole
/// numbers, so none of its float functions, sqrt, pi, huge, fmod, modf, frexp or ldexp, is there
/// to make fractions with; the trigonometric, logarithmic and power functions also come from the
/// platform's C library and can differ in the last bit, and random needs no explanation.
const MATH: &[&str] = &["abs", "ceil", "clamp", "floor", "max", "min", "round", "sign"];

#[document(
    summary = "A compiled contract script. The script returns a table, and each function in it can be called by the XXH3-64 of its name.",
    category = "Lua VM",
    see_also = "Limits, Outcome, CallContext",
    example = "local M = {}\n\nfunction M.balance_of(who)\n    return u256(0)\nend\n\nreturn M",
)]
pub struct Contract {
    bytecode: Vec<u8>,
    functions: HashMap<u64, String>,
}

impl Contract {
    #[document(
        parent = "Contract",
        summary = "Compiles a script and runs it once to find the functions it exports.",
        params = {
            source = "The Luau source.",
            limits = "What running the script's top level may use.",
        },
        returns = "The contract, or why it cannot be deployed: a syntax error, a top level that fails or returns something other than a table, or two exported names with the same hash.",
        see_also = "compile",
    )]
    pub fn load(source: &str, limits: Limits) -> Result<Contract, VmError> {
        Contract::from_bytecode(compiler::compile(source)?, limits)
    }

    #[document(
        parent = "Contract",
        summary = "Runs compiled bytecode once to find the functions it exports.",
        params = {
            bytecode = "Bytecode from compile on this node. Never bytecode from anyone else: Luau does not check it, and bad bytecode can corrupt memory.",
            limits = "What running the script's top level may use.",
        },
    )]
    pub fn from_bytecode(bytecode: Vec<u8>, limits: Limits) -> Result<Contract, VmError> {
        Contract::explore(bytecode, limits, &LuaCosts::DEFAULT).1
    }

    /// Runs compiled bytecode's top level once to find the functions it exports, and says what
    /// that burned, whether or not it worked.
    fn explore(bytecode: Vec<u8>, limits: Limits, costs: &LuaCosts) -> (u64, Result<Contract, VmError>) {
        let state = match Pristine::new().and_then(|state| state.ready(limits, costs)) {
            Ok(state) => state,
            Err(err) => return (0, Err(err)),
        };
        let result = Contract::exports(&state, bytecode);
        let remaining = state.lua.get_fuel().unwrap_or(0);

        (limits.fuel - remaining, fueled(result, remaining, costs))
    }

    fn exports(state: &Pristine, bytecode: Vec<u8>) -> Result<Contract, VmError> {
        let module = instantiate(state, &bytecode)?;
        let mut functions: HashMap<u64, String> = HashMap::new();

        for pair in module.pairs::<Value, Value>() {
            let (Value::String(name), Value::Function(_)) = pair? else {
                continue;
            };
            let name = name.to_str()?.to_string();

            if let Some(previous) = functions.insert(selector(&name), name.clone()) {
                return Err(VmError::SelectorClash(previous, name));
            }
        }

        Ok(Contract {
            bytecode,
            functions,
        })
    }

    #[document(
        parent = "Contract",
        summary = "Rebuilds a contract from what a registry stored, without running it again.",
        params = {
            bytecode = "Bytecode this node compiled.",
            functions = "The exported names, from function_names.",
        },
    )]
    pub fn from_compiled(bytecode: Vec<u8>, functions: Vec<String>) -> Contract {
        let functions = functions.into_iter().map(|name| (selector(&name), name)).collect();

        Contract {
            bytecode,
            functions,
        }
    }

    #[document(
        parent = "Contract",
        summary = "The compiled bytecode.",
    )]
    pub fn bytecode(&self) -> &[u8] {
        &self.bytecode
    }

    #[document(
        parent = "Contract",
        summary = "The exported function names, sorted so the list is the same on every node.",
    )]
    pub fn function_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.functions.values().cloned().collect();
        names.sort();

        names
    }

    #[document(
        parent = "Contract",
        summary = "The name of the exported function a hash selects, if any.",
        params = { func = "The function hash from a contract call." },
    )]
    pub fn function_name(&self, func: &Hash) -> Option<&str> {
        match func {
            Hash::Xxh3_64(selector) => self.functions.get(selector).map(String::as_str),
            #[allow(unreachable_patterns)]
            _ => None,
        }
    }

    #[document(
        parent = "Contract",
        summary = "Calls an exported function in a fresh Lua state.",
        params = {
            func = "The XXH3-64 of the function's name, as in Tx::ContractCall.",
            args = "The arguments, converted to Lua values.",
            host = "The call's context and storage, and the way out to other contracts.",
            limits = "What the call may use.",
        },
        returns = "The fuel used, and the function's return values or why it failed.",
    )]
    pub fn call(&self, func: &Hash, args: &[TxCallArgumentType], host: Host, limits: Limits) -> Outcome {
        let Some(name) = self.function_name(func) else {
            return Outcome::failed(VmError::NoSuchFunction);
        };

        let costs = host.rules().lua_costs;

        match Pristine::new().and_then(|state| state.ready(limits, &costs)) {
            Ok(state) => call(&state, &self.bytecode, name, args, host, limits, &costs),
            Err(err) => Outcome::failed(err),
        }
    }
}

#[document(
    summary = "The Lua engine: contracts are Luau source, compiled by every node with the same settings, and each call runs in a fresh, sandboxed Lua state. It keeps a pool of states built ahead on a thread of its own, started on first use, so a call takes one ready instead of building it. Each is used once, then closed on another thread.",
    category = "Lua VM",
    see_also = "Contract, ScriptEngine, compile",
)]
pub struct LuaEngine {
    pool: OnceLock<Pool>,
    /// How many states the pool keeps ready, and how many threads build them.
    size: usize,
    builders: usize,
}

impl Default for LuaEngine {
    fn default() -> LuaEngine {
        LuaEngine::with_pool(POOL_SIZE, BUILDERS)
    }
}

impl LuaEngine {
    #[document(parent = "LuaEngine", summary = "The engine, its pool still empty: it starts filling on the first call.")]
    pub fn new() -> LuaEngine {
        LuaEngine::default()
    }

    #[document(
        parent = "LuaEngine",
        summary = "The engine, its pool keeping this many states ready, built by this many threads, once it starts on the first call: what --lua-states and --lua-builders size. More takes memory for calls back to back; the pool never changes what a call does, as every state is built the same.",
    )]
    pub fn with_pool(size: usize, builders: usize) -> LuaEngine {
        LuaEngine { pool: OnceLock::new(), size: size.max(1), builders: builders.max(1) }
    }

    fn pool(&self) -> &Pool {
        self.pool.get_or_init(|| Pool::start(self.size, self.builders))
    }
}

impl ScriptEngine for LuaEngine {
    fn load_under(&self, code: &[u8], limits: Limits, rules: &crate::params::vm::VmRules) -> Loading {
        let Some(fee) = Loading::fee(code, limits, rules) else {
            return Loading::failed(limits.fuel, VmError::OutOfFuel);
        };
        let source = match std::str::from_utf8(code) {
            Ok(source) => source,
            Err(err) => return Loading::failed(fee, VmError::Compile(format!("source is not UTF-8: {err}"))),
        };
        let bytecode = match compiler::compile_within(source, rules.limits.source_len as usize) {
            Ok(bytecode) => bytecode,
            Err(err) => return Loading::failed(fee, err),
        };
        let (ran, contract) = Contract::explore(
            bytecode,
            Limits {
                fuel: limits.fuel - fee,
                ..limits
            },
            &rules.lua_costs,
        );
        let result = contract.and_then(|contract| {
            let functions = contract.function_names();
            let reentrant = annotations::reentrant(source, &functions)?;
            let layout = annotations::layout(source)?;

            Ok(Loaded {
                code: contract.bytecode,
                functions,
                reentrant,
                layout,
            })
        });

        Loading {
            fuel_used: fee + ran,
            result,
        }
    }

    fn run(&self, code: &[u8], function: &str, args: &[TxCallArgumentType], host: Host, limits: Limits) -> Outcome {
        let pool = self.pool();

        let costs = host.rules().lua_costs;

        match pool.take().and_then(|state| state.ready(limits, &costs)) {
            Ok(state) => {
                let outcome = call(&state, code, function, args, host, limits, &costs);
                pool.retire(state);

                outcome
            }
            Err(err) => Outcome::failed(err),
        }
    }
}

/// A state nothing has run in yet: the libraries registered, the sandbox locked, and the
/// environment a script runs in built. Every one is built the same way, so they are all the same,
/// wherever and whenever they were built, and a call gives the same result in any of them.
struct Pristine {
    lua: Lua,
    env: Table,
}

impl Pristine {
    fn new() -> Result<Pristine, VmError> {
        let libs = StdLib::TABLE | StdLib::STRING | StdLib::UTF8 | StdLib::BIT | StdLib::MATH;
        let lua = Lua::new_with(libs, LuaOptions::new())?;
        // A new state has no fuel, and mlua runs a little Lua of its own while libraries register.
        // That setup is the node's cost, not the script's, so the script's budget starts later.
        lua.set_fuel(u64::MAX)?;
        register_address_lib(&lua);
        register_hash_lib(&lua);
        register_integer_libs(&lua);
        register_ctx_lib(&lua);
        register_storage_lib(&lua);
        register_contracts_lib(&lua);
        register_accounts_lib(&lua);
        register_events_lib(&lua);
        register_tokens_lib(&lua);
        register_crypto_lib(&lua);
        register_federation_lib(&lua);
        register_chain_lib(&lua);
        register_names_lib(&lua);
        register_evm_lib(&lua);
        register_perms_lib(&lua);
        replace_tostring(&lua)?;
        lua.sandbox(true)?;
        let env = environment(&lua)?;
        // What building left behind goes now, so every state waits with the same heap and the
        // collector paused, its next cycle set by that heap alone: a call's collections then
        // come at the same allocations in any state, wherever and however long it waited.
        lua.gc_collect()?;

        Ok(Pristine { lua, env })
    }

    /// Sets what the one call this state is for may use. The memory limit counts from zero here:
    /// the bytes of what the call makes, less what is freed, as the forked Luau counts them, by
    /// the sizes it asks for. Neither the allocator's pages nor when the collector runs, which
    /// follows those bytes alone, changes where a call runs out, so every node fails it at the
    /// same allocation, with the same fuel used.
    /// Its fuel costs are the block's, which the setup before ran by the fork's own.
    fn ready(self, limits: Limits, costs: &LuaCosts) -> Result<Pristine, VmError> {
        self.lua.set_memory_limit(limits.memory)?;
        self.lua.set_fuel_costs(&costs.slots);
        self.lua.set_fuel(limits.fuel)?;

        Ok(self)
    }
}

/// Pristine states built ahead on threads of their own, the pool's size of them waiting at most, and
/// used ones closed on another, since closing a state costs a good part of a call too. The
/// threads stop when the pool, and with it their channels, is dropped.
struct Pool {
    ready: Mutex<mpsc::Receiver<Pristine>>,
    used: mpsc::SyncSender<Pristine>,
}

impl Pool {
    fn start(size: usize, builders: usize) -> Pool {
        let (built, ready) = mpsc::sync_channel(size);
        let (used, closing) = mpsc::sync_channel::<Pristine>(size);
        // If a thread cannot start, or stops, calls build and close their own states.
        for _ in 0..builders {
            let built = built.clone();
            let _ = std::thread::Builder::new().name("lua states".to_string()).spawn(move || {
                while let Ok(state) = Pristine::new() {
                    if built.send(state).is_err() {
                        break;
                    }
                }
            });
        }

        let _ = std::thread::Builder::new()
            .name("lua cleanup".to_string())
            .spawn(move || closing.into_iter().for_each(drop));

        Pool {
            ready: Mutex::new(ready),
            used,
        }
    }

    /// A pristine state: one that is waiting, or one built here when none is.
    fn take(&self) -> Result<Pristine, VmError> {
        let ready = self.ready.lock().expect("taking a state never panics").try_recv();

        match ready {
            Ok(state) => Ok(state),
            Err(_) => Pristine::new(),
        }
    }

    /// Hands a used state over to be closed, or closes it here when the other thread is behind.
    fn retire(&self, state: Pristine) {
        let _ = self.used.try_send(state);
    }
}

/// Runs one exported function of compiled bytecode in a state readied for it. The state is not
/// to be used again.
fn call(state: &Pristine, bytecode: &[u8], name: &str, args: &[TxCallArgumentType], host: Host, limits: Limits, costs: &LuaCosts) -> Outcome {
    let result = run(state, bytecode, name, args, host);
    // The call's host goes now, wherever the state is closed later.
    state.lua.remove_app_data::<Host>();
    state.lua.remove_app_data::<CallArgs>();
    let remaining = state.lua.get_fuel().unwrap_or(0);
    let result = fueled(result, remaining, costs);

    Outcome {
        fuel_used: limits.fuel - remaining,
        result,
        events: Vec::new(),
        trace: Trace::default(),
        refresh_tokens: Vec::new(),
        payments: Vec::new(),
    }
}

/// A script's own "out of fuel" error is only running out when the tank is nearly empty: less left
/// than the dearest instruction costs, by the costs in force.
fn fueled<T>(result: Result<T, VmError>, remaining: u64, costs: &LuaCosts) -> Result<T, VmError> {
    let dearest = costs.slots[..LUA_FUEL_OPCODES].iter().copied().max().unwrap_or(0);

    match result {
        Err(VmError::Runtime(text)) if text.contains("out of fuel") && remaining < dearest.max(1) => {
            Err(VmError::OutOfFuel)
        }
        other => other,
    }
}

fn run(
    state: &Pristine,
    bytecode: &[u8],
    name: &str,
    args: &[TxCallArgumentType],
    host: Host,
) -> Result<Vec<TxCallArgumentType>, VmError> {
    // The top level runs before the call is attached, so it sees nothing of it: it is the same
    // code, doing the same work, on every call as at deploy.
    let module = instantiate(state, bytecode)?;
    state.lua.set_app_data(host);
    state.lua.set_app_data(CallArgs::new(name, args));
    state.lua.set_app_data(Module(module.clone()));
    let function: Function = module.get(name)?;
    let mut lua_args = Vec::new();

    for arg in args {
        lua_args.push(value::to_lua(&state.lua, arg)?);
    }

    let returns: MultiValue = function.call(MultiValue::from_vec(lua_args))?;

    // A values list returned alone goes back untouched.
    Ok(Given::read(returns)?.into_values())
}

impl From<mlua::Error> for VmError {
    fn from(err: mlua::Error) -> VmError {
        if is_memory_error(&err) {
            return VmError::OutOfMemory;
        }

        // Failures go into receipts, which every node must agree on.
        VmError::Runtime(without_addresses(&err.to_string()))
    }
}

/// tostring as scripts see it: Luau's, except that a table, function, thread, buffer or userdata
/// without __tostring reads as its type's name, where Luau prints its memory address, which no two
/// nodes share. A script could otherwise return, store or emit one, and split the chain.
fn replace_tostring(lua: &Lua) -> mlua::Result<()> {
    let globals = lua.globals();
    let luau: Function = globals.get("tostring")?;
    let tostring = lua.create_function(move |lua, value: Value| -> mlua::Result<Value> {
        let has_tostring = |meta: Option<Table>| meta.and_then(|meta| meta.raw_get::<Value>("__tostring").ok()).is_some_and(|found| !found.is_nil());
        let named = match &value {
            Value::Table(table) => (!has_tostring(table.metatable())).then_some("table"),
            Value::UserData(data) => {
                let found = data.metatable().and_then(|meta| meta.get::<Value>("__tostring")).is_ok_and(|found| !found.is_nil());

                (!found).then_some("userdata")
            }
            Value::LightUserData(_) => Some("userdata"),
            Value::Function(_) => Some("function"),
            Value::Thread(_) => Some("thread"),
            Value::Buffer(_) => Some("buffer"),
            _ => None,
        };

        match named {
            Some(name) => lua.create_string(name).map(Value::String),
            None => luau.call(value),
        }
    })?;

    globals.set("tostring", tostring)
}

/// Text with each `table: 0x…`, `function: 0x…` and the like cut to the type's name: Luau shows a
/// value's address there, as in the message of an error raised with a table, and no two nodes
/// share one.
fn without_addresses(text: &str) -> String {
    const NAMED: [&str; 5] = ["table", "function", "userdata", "thread", "buffer"];
    let mut kept = String::with_capacity(text.len());
    let mut rest = text;

    while let Some(at) = rest.find(": 0x") {
        let (before, after) = rest.split_at(at);
        let digits = after[4..].bytes().take_while(u8::is_ascii_hexdigit).count();

        if digits > 0 && NAMED.iter().any(|name| before.ends_with(name)) {
            kept.push_str(before);
            rest = &after[4 + digits..];
        } else {
            kept.push_str(before);
            kept.push_str(": 0x");
            rest = &after[4..];
        }
    }

    kept.push_str(rest);

    kept
}

fn is_memory_error(err: &mlua::Error) -> bool {
    match err {
        mlua::Error::MemoryError(_) => true,
        mlua::Error::CallbackError { cause, .. } => is_memory_error(cause),
        _ => false,
    }
}

/// Runs the script's top level in the state's environment and returns the table it exports.
/// Running it again for every call gives every call fresh top-level locals.
fn instantiate(state: &Pristine, bytecode: &[u8]) -> Result<Table, VmError> {
    let module: Value = state
        .lua
        .load(bytecode)
        .set_name("=contract")
        .set_mode(ChunkMode::Binary)
        .set_environment(state.env.clone())
        .eval()?;

    match module {
        Value::Table(module) => Ok(module),
        _ => Err(VmError::NotAModule),
    }
}

/// The allowed globals only, read-only, so a script keeps its state in locals and cannot
/// change what other code sees.
/// A Lua state for chainv4 test, never a chain's: the libraries and globals a contract has, in an
/// environment left open for a test's own globals and `t`, and no fuel limit, as tests run off
/// chain and only what they send to the chain is metered.
#[cfg(feature = "node")]
pub(crate) fn test_state() -> Result<(Lua, Table), VmError> {
    let pristine = Pristine::new()?;
    pristine.lua.set_fuel(u64::MAX)?;
    let env = pristine.lua.create_table()?;

    for pair in pristine.env.pairs::<Value, Value>() {
        let (name, value) = pair?;
        env.raw_set(name, value)?;
    }

    Ok((pristine.lua, env))
}

/// A call's argument as a Lua value, as a contract receives one: for chainv4 test.
#[cfg(feature = "node")]
pub(crate) fn argument_to_lua(lua: &Lua, argument: &TxCallArgumentType) -> Result<Value, VmError> {
    value::to_lua(lua, argument)
}

/// A Lua value as a call's argument, as a contract returns one: for chainv4 test.
#[cfg(feature = "node")]
pub(crate) fn argument_from_lua(value: Value) -> Result<TxCallArgumentType, VmError> {
    value::from_lua(value)
}

fn environment(lua: &Lua) -> mlua::Result<Table> {
    // The sandbox leaves an empty globals table that reads through to the locked originals,
    // so these lookups go through __index rather than raw.
    let globals = lua.globals();
    let env = lua.create_table()?;

    for name in GLOBALS {
        env.raw_set(*name, globals.get::<Value>(*name)?)?;
    }

    let math: Table = globals.get("math")?;
    let safe_math = lua.create_table()?;

    for name in MATH {
        safe_math.raw_set(*name, math.raw_get::<Value>(*name)?)?;
    }

    safe_math.set_readonly(true);
    env.raw_set("math", safe_math)?;
    env.set_readonly(true);

    Ok(env)
}

/// What host operations cost in the call's block: the defaults while no call is attached, as
/// while a deploy loads.
fn costs(lua: &Lua) -> crate::params::vm::HostCosts {
    lua.app_data_ref::<Host>().map_or(crate::params::vm::HostCosts::DEFAULT, |host| host.costs())
}

/// Library functions do work the per-instruction fuel never sees, so they charge for it here.
fn charge(lua: &Lua, cost: u64) -> mlua::Result<()> {
    let fuel = lua.get_fuel()?;

    if fuel < cost {
        lua.set_fuel(0)?;

        return Err(mlua::Error::runtime("out of fuel"));
    }

    lua.set_fuel(fuel - cost)
}

#[cfg(test)]
mod tests;
