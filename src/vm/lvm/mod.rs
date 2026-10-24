//! The Lua VM. Contracts are Luau scripts run on the forked Luau, which charges fuel for every
//! instruction. Every call gets a fresh Lua state, so nothing one call leaves behind can reach
//! the next, and a node that restarts computes exactly what one that never stopped does. Fresh
//! matters for more than globals: a script's top-level locals live on as upvalues, and the memory
//! limit counts the whole heap, so a state that ran something before could fail a call another
//! node passes. Building a state is most of what a call costs, so the engine builds them ahead,
//! on a thread of its own, and still uses each one once.

use {
    super::{Host, Limits, Loaded, Outcome, ScriptEngine, VmError, selector},
    crate::structs::hash::Hash,
    crate::structs::hex::{from_hex, to_hex},
    crate::structs::trace::Trace,
    crate::structs::tx::TxCallArgumentType,
    libs::{
        ctx::CallArgs, register_accounts_lib, register_address_lib, register_contracts_lib, register_ctx_lib,
        register_events_lib, register_hash_lib, register_storage_lib, register_u256_lib, values::Given,
    },
    mlua::{ChunkMode, Function, Lua, LuaOptions, MultiValue, StdLib, Table, Value},
    r#macro::document,
    std::collections::HashMap,
    std::sync::{Mutex, OnceLock, mpsc},
};

pub mod annotations;
pub mod compiler;
pub mod libs;
mod value;

/// How many pristine states the engine keeps ready, and how many threads build them. Building one
/// takes about twice as long as a call that uses it, so two keep up with calls back to back.
const POOL_SIZE: usize = 16;
const BUILDERS: usize = 2;

/// The most fuel one instruction costs in the forked Luau. When an out-of-fuel error leaves less
/// than this, the script really ran dry.
const MAX_INSTRUCTION_COST: u64 = 10;

/// Globals a script may use. Everything else is left out, including anything that reads the
/// clock, the platform or the garbage collector, since nodes would disagree on those.
const GLOBALS: &[&str] = &[
    "assert", "error", "getmetatable", "ipairs", "next", "pairs", "pcall", "print", "rawequal",
    "rawget", "rawlen", "rawset", "select", "setmetatable", "tonumber", "tostring", "type",
    "typeof", "unpack", "xpcall", "string", "table", "utf8", "bit32", "address", "hash", "u256",
    "ctx", "storage", "contracts", "accounts", "events",
];

/// The math functions that give the same result on every platform. The trigonometric,
/// logarithmic and power functions come from the platform's C library and can differ in the
/// last bit, and random needs no explanation.
const MATH: &[&str] = &[
    "abs", "ceil", "clamp", "floor", "fmod", "frexp", "huge", "ldexp", "max", "min", "modf", "pi",
    "round", "sign", "sqrt",
];

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
        let state = Pristine::new()?.ready(limits)?;
        let module = instantiate(&state, &bytecode)?;
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

        match Pristine::new().and_then(|state| state.ready(limits)) {
            Ok(state) => call(&state, &self.bytecode, name, args, host, limits),
            Err(err) => Outcome::failed(err),
        }
    }
}

#[document(
    summary = "The Lua engine: contracts are Luau source, compiled by every node with the same settings, and each call runs in a fresh, sandboxed Lua state. It keeps a pool of states built ahead on a thread of its own, started on first use, so a call takes one ready instead of building it. Each is used once, then closed on another thread.",
    category = "Lua VM",
    see_also = "Contract, ScriptEngine, compile",
)]
#[derive(Default)]
pub struct LuaEngine {
    pool: OnceLock<Pool>,
}

impl LuaEngine {
    #[document(parent = "LuaEngine", summary = "The engine, its pool still empty: it starts filling on the first call.")]
    pub fn new() -> LuaEngine {
        LuaEngine::default()
    }

    fn pool(&self) -> &Pool {
        self.pool.get_or_init(Pool::start)
    }
}

impl ScriptEngine for LuaEngine {
    fn load(&self, code: &[u8], limits: Limits) -> Result<Loaded, VmError> {
        let source = std::str::from_utf8(code).map_err(|err| VmError::Compile(format!("source is not UTF-8: {err}")))?;
        let contract = Contract::load(source, limits)?;
        let functions = contract.function_names();
        let reentrant = annotations::reentrant(source, &functions)?;

        Ok(Loaded {
            code: contract.bytecode,
            functions,
            reentrant,
        })
    }

    fn run(&self, code: &[u8], function: &str, args: &[TxCallArgumentType], host: Host, limits: Limits) -> Outcome {
        let pool = self.pool();

        match pool.take().and_then(|state| state.ready(limits)) {
            Ok(state) => {
                let outcome = call(&state, code, function, args, host, limits);
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
        register_u256_lib(&lua);
        register_ctx_lib(&lua);
        register_storage_lib(&lua);
        register_contracts_lib(&lua);
        register_accounts_lib(&lua);
        register_events_lib(&lua);
        lua.sandbox(true)?;
        let env = environment(&lua)?;

        Ok(Pristine { lua, env })
    }

    /// Sets what the one call this state is for may use. The memory limit counts the whole heap,
    /// which is the same in every pristine state, so every node gives a call the same room.
    fn ready(self, limits: Limits) -> Result<Pristine, VmError> {
        self.lua.set_memory_limit(limits.memory)?;
        self.lua.set_fuel(limits.fuel)?;

        Ok(self)
    }
}

/// Pristine states built ahead on threads of their own, POOL_SIZE of them waiting at most, and
/// used ones closed on another, since closing a state costs a good part of a call too. The
/// threads stop when the pool, and with it their channels, is dropped.
struct Pool {
    ready: Mutex<mpsc::Receiver<Pristine>>,
    used: mpsc::SyncSender<Pristine>,
}

impl Pool {
    fn start() -> Pool {
        let (built, ready) = mpsc::sync_channel(POOL_SIZE);
        let (used, closing) = mpsc::sync_channel::<Pristine>(POOL_SIZE);
        // If a thread cannot start, or stops, calls build and close their own states.
        for _ in 0..BUILDERS {
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
fn call(state: &Pristine, bytecode: &[u8], name: &str, args: &[TxCallArgumentType], host: Host, limits: Limits) -> Outcome {
    state.lua.set_app_data(host);
    state.lua.set_app_data(CallArgs::new(name, args));
    let result = run(state, bytecode, name, args);
    // The call's host goes now, wherever the state is closed later.
    state.lua.remove_app_data::<Host>();
    state.lua.remove_app_data::<CallArgs>();
    let remaining = state.lua.get_fuel().unwrap_or(0);
    let result = match result {
        // A script can raise "out of fuel" itself; only a nearly empty tank makes it real.
        Err(VmError::Runtime(text)) if text.contains("out of fuel") && remaining < MAX_INSTRUCTION_COST => {
            Err(VmError::OutOfFuel)
        }
        other => other,
    };

    Outcome {
        fuel_used: limits.fuel - remaining,
        result,
        events: Vec::new(),
        trace: Trace::default(),
    }
}

fn run(
    state: &Pristine,
    bytecode: &[u8],
    name: &str,
    args: &[TxCallArgumentType],
) -> Result<Vec<TxCallArgumentType>, VmError> {
    let module = instantiate(state, bytecode)?;
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

        VmError::Runtime(err.to_string())
    }
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
