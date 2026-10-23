//! The Lua VM. Contracts are Luau scripts run on the forked Luau, which charges fuel for every
//! instruction. Every call gets a fresh Lua state, so nothing one call leaves behind can reach
//! the next, and a node that restarts computes exactly what one that never stopped does.

use {
    super::{Host, Limits, Loaded, Outcome, ScriptEngine, VmError, selector},
    crate::structs::hash::Hash,
    crate::structs::hex::{from_hex, to_hex},
    crate::structs::tx::TxCallArgumentType,
    libs::{
        register_address_lib, register_contracts_lib, register_ctx_lib, register_hash_lib,
        register_storage_lib, register_u256_lib,
    },
    mlua::{ChunkMode, Function, Lua, LuaOptions, MultiValue, StdLib, Table, Value},
    r#macro::document,
    std::collections::HashMap,
};

pub mod annotations;
pub mod compiler;
pub mod libs;
mod value;

/// The most fuel one instruction costs in the forked Luau. When an out-of-fuel error leaves less
/// than this, the script really ran dry.
const MAX_INSTRUCTION_COST: u64 = 10;

/// Globals a script may use. Everything else is left out, including anything that reads the
/// clock, the platform or the garbage collector, since nodes would disagree on those.
const GLOBALS: &[&str] = &[
    "assert", "error", "getmetatable", "ipairs", "next", "pairs", "pcall", "print", "rawequal",
    "rawget", "rawlen", "rawset", "select", "setmetatable", "tonumber", "tostring", "type",
    "typeof", "unpack", "xpcall", "string", "table", "utf8", "bit32", "address", "hash", "u256",
    "ctx", "storage", "contracts",
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
        let lua = new_state(limits)?;
        let module = instantiate(&lua, &bytecode)?;
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
        match self.function_name(func) {
            Some(name) => call(&self.bytecode, name, args, host, limits),
            None => Outcome::failed(VmError::NoSuchFunction),
        }
    }
}

#[document(
    summary = "The Lua engine: contracts are Luau source, compiled by every node with the same settings, and each call runs in a fresh, sandboxed Lua state.",
    category = "Lua VM",
    see_also = "Contract, ScriptEngine, compile",
)]
pub struct LuaEngine;

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
        call(code, function, args, host, limits)
    }
}

/// Runs one exported function of compiled bytecode in a fresh state.
fn call(bytecode: &[u8], name: &str, args: &[TxCallArgumentType], host: Host, limits: Limits) -> Outcome {
    let lua = match new_state(limits) {
        Ok(lua) => lua,
        Err(err) => return Outcome::failed(err),
    };
    lua.set_app_data(host);
    let result = run(&lua, bytecode, name, args);
    let remaining = lua.get_fuel().unwrap_or(0);
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
    }
}

fn run(
    lua: &Lua,
    bytecode: &[u8],
    name: &str,
    args: &[TxCallArgumentType],
) -> Result<Vec<TxCallArgumentType>, VmError> {
    let module = instantiate(lua, bytecode)?;
    let function: Function = module.get(name)?;
    let mut lua_args = Vec::new();

    for arg in args {
        lua_args.push(value::to_lua(lua, arg)?);
    }

    let returns: MultiValue = function.call(MultiValue::from_vec(lua_args))?;

    returns.into_iter().map(value::from_lua).collect()
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

/// A Lua state with the chain libraries, locked down. Libraries register into the globals
/// before the sandbox makes them read-only.
fn new_state(limits: Limits) -> Result<Lua, VmError> {
    let libs = StdLib::TABLE | StdLib::STRING | StdLib::UTF8 | StdLib::BIT | StdLib::MATH;
    let lua = Lua::new_with(libs, LuaOptions::new())?;
    // A new state has no fuel, and mlua runs a little Lua of its own while libraries register.
    // That setup is the node's cost, not the script's, so the script's budget starts after it.
    lua.set_fuel(u64::MAX)?;
    lua.set_memory_limit(limits.memory)?;
    register_address_lib(&lua);
    register_hash_lib(&lua);
    register_u256_lib(&lua);
    register_ctx_lib(&lua);
    register_storage_lib(&lua);
    register_contracts_lib(&lua);
    lua.sandbox(true)?;
    lua.set_fuel(limits.fuel)?;

    Ok(lua)
}

/// Runs the script's top level in a fresh environment and returns the table it exports.
fn instantiate(lua: &Lua, bytecode: &[u8]) -> Result<Table, VmError> {
    let env = environment(lua)?;
    let module: Value = lua
        .load(bytecode)
        .set_name("=contract")
        .set_mode(ChunkMode::Binary)
        .set_environment(env)
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
