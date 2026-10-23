//! The Lua VM. Contracts are Luau scripts run on the forked Luau, which charges fuel for every
//! instruction. Every call gets a fresh Lua state, so nothing one call leaves behind can reach
//! the next, and a node that restarts computes exactly what one that never stopped does.

use {
    crate::crypto::hasher::Hasher,
    crate::structs::address::Address,
    crate::structs::hash::Hash,
    crate::structs::hash::HashType,
    crate::structs::tx::TxCallArgumentType,
    crate::structs::u256::u256,
    libs::{register_address_lib, register_ctx_lib, register_hash_lib, register_u256_lib},
    mlua::{ChunkMode, Compiler, Function, Lua, LuaOptions, MultiValue, StdLib, Table, Value},
    r#macro::document,
    std::collections::HashMap,
    std::fmt,
};

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
    "ctx",
];

/// The math functions that give the same result on every platform. The trigonometric,
/// logarithmic and power functions come from the platform's C library and can differ in the
/// last bit, and random needs no explanation.
const MATH: &[&str] = &[
    "abs", "ceil", "clamp", "floor", "fmod", "frexp", "huge", "ldexp", "max", "min", "modf", "pi",
    "round", "sign", "sqrt",
];

#[document(
    summary = "What one load or call may use. Running out of either stops the script.",
    category = "Lua VM",
    see_also = "Contract::load, Contract::call",
)]
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    #[document(summary = "Instruction budget, charged per Luau opcode by the forked VM and per byte by library functions that do bulk work.")]
    pub fuel: u64,
    #[document(summary = "Most bytes the Lua state may allocate.")]
    pub memory: usize,
}

#[document(
    summary = "Who is calling, which contract, and what they sent. Scripts read it through ctx.",
    category = "Lua VM",
    see_also = "Contract::call",
)]
#[derive(Clone, Copy, Debug)]
pub struct CallContext {
    #[document(summary = "The address making the call.")]
    pub sender: Address,
    #[document(summary = "The address of the contract being called.")]
    pub contract: Address,
    #[document(summary = "The amount sent with the call.")]
    pub value: u256,
}

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
    )]
    pub fn load(source: &str, limits: Limits) -> Result<Contract, LvmError> {
        let bytecode = Compiler::new()
            .compile(source)
            .map_err(|err| LvmError::Compile(err.to_string()))?;
        let lua = new_state(limits)?;
        let module = instantiate(&lua, &bytecode)?;
        let mut functions: HashMap<u64, String> = HashMap::new();

        for pair in module.pairs::<Value, Value>() {
            let (Value::String(name), Value::Function(_)) = pair? else {
                continue;
            };
            let name = name.to_str()?.to_string();

            if let Some(previous) = functions.insert(selector(&name), name.clone()) {
                return Err(LvmError::SelectorClash(previous, name));
            }
        }

        Ok(Contract {
            bytecode,
            functions,
        })
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
            context = "Who is calling, which contract, and how much they sent.",
            func = "The XXH3-64 of the function's name, as in Tx::ContractCall.",
            args = "The arguments, converted to Lua values.",
            limits = "What the call may use.",
        },
        returns = "The fuel used, and the function's return values or why it failed.",
    )]
    pub fn call(
        &self,
        context: &CallContext,
        func: &Hash,
        args: &[TxCallArgumentType],
        limits: Limits,
    ) -> Outcome {
        let Some(name) = self.function_name(func) else {
            return Outcome {
                fuel_used: 0,
                result: Err(LvmError::NoSuchFunction),
            };
        };
        let lua = match new_state(limits) {
            Ok(lua) => lua,
            Err(err) => {
                return Outcome {
                    fuel_used: 0,
                    result: Err(err),
                };
            }
        };
        lua.set_app_data(*context);
        let result = run(&lua, &self.bytecode, name, args);
        let remaining = lua.get_fuel().unwrap_or(0);
        let result = match result {
            // A script can raise "out of fuel" itself; only a nearly empty tank makes it real.
            Err(LvmError::Runtime(text))
                if text.contains("out of fuel") && remaining < MAX_INSTRUCTION_COST =>
            {
                Err(LvmError::OutOfFuel)
            }
            other => other,
        };

        Outcome {
            fuel_used: limits.fuel - remaining,
            result,
        }
    }
}

fn run(
    lua: &Lua,
    bytecode: &[u8],
    name: &str,
    args: &[TxCallArgumentType],
) -> Result<Vec<TxCallArgumentType>, LvmError> {
    let module = instantiate(lua, bytecode)?;
    let function: Function = module.get(name)?;
    let mut lua_args = Vec::new();

    for arg in args {
        lua_args.push(value::to_lua(lua, arg)?);
    }

    let returns: MultiValue = function.call(MultiValue::from_vec(lua_args))?;

    returns.into_iter().map(value::from_lua).collect()
}

#[document(
    summary = "What a call did.",
    category = "Lua VM",
    see_also = "Contract::call",
)]
#[derive(Debug)]
pub struct Outcome {
    #[document(summary = "Fuel the call burned, including running the script's top level. Charged whether or not the call succeeded.")]
    pub fuel_used: u64,
    #[document(summary = "The function's return values, or why it failed.")]
    pub result: Result<Vec<TxCallArgumentType>, LvmError>,
}

#[document(
    summary = "Why a contract could not be loaded or a call failed.",
    category = "Lua VM",
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LvmError {
    #[document(summary = "The source does not compile.")]
    Compile(String),
    #[document(summary = "The script's top level returned something other than a table of functions.")]
    NotAModule,
    #[document(summary = "Two exported functions have the same XXH3-64 hash, so a call could not tell them apart.")]
    SelectorClash(String, String),
    #[document(summary = "No exported function has the hash the call asked for.")]
    NoSuchFunction,
    #[document(summary = "The script used all its fuel.")]
    OutOfFuel,
    #[document(summary = "The script tried to use more memory than its limit.")]
    OutOfMemory,
    #[document(summary = "The script raised an error, or a library function it called did.")]
    Runtime(String),
    #[document(summary = "An argument has no Lua equivalent.")]
    BadArgument(String),
    #[document(summary = "A return value has no TxCallArgumentType equivalent.")]
    BadReturn(String),
}

impl fmt::Display for LvmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LvmError::Compile(text) => write!(f, "compile error: {text}"),
            LvmError::NotAModule => write!(f, "script must return a table of functions"),
            LvmError::SelectorClash(first, second) => {
                write!(f, "functions {first} and {second} have the same hash")
            }
            LvmError::NoSuchFunction => write!(f, "no such function"),
            LvmError::OutOfFuel => write!(f, "out of fuel"),
            LvmError::OutOfMemory => write!(f, "out of memory"),
            LvmError::Runtime(text) => write!(f, "{text}"),
            LvmError::BadArgument(text) => write!(f, "bad argument: {text}"),
            LvmError::BadReturn(text) => write!(f, "bad return value: {text}"),
        }
    }
}

impl std::error::Error for LvmError {}

impl From<mlua::Error> for LvmError {
    fn from(err: mlua::Error) -> LvmError {
        if is_memory_error(&err) {
            return LvmError::OutOfMemory;
        }

        LvmError::Runtime(err.to_string())
    }
}

fn is_memory_error(err: &mlua::Error) -> bool {
    match err {
        mlua::Error::MemoryError(_) => true,
        mlua::Error::CallbackError { cause, .. } => is_memory_error(cause),
        _ => false,
    }
}

/// The hash a contract call names a function by.
pub fn selector(name: &str) -> u64 {
    match Hasher::digest(HashType::Xxh3_64, name.as_bytes()) {
        Hash::Xxh3_64(selector) => selector,
        #[allow(unreachable_patterns)]
        _ => unreachable!("XXH3-64 always gives an XXH3-64 hash"),
    }
}

/// A Lua state with the chain libraries, locked down. Libraries register into the globals
/// before the sandbox makes them read-only.
fn new_state(limits: Limits) -> Result<Lua, LvmError> {
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
    lua.sandbox(true)?;
    lua.set_fuel(limits.fuel)?;

    Ok(lua)
}

/// Runs the script's top level in a fresh environment and returns the table it exports.
fn instantiate(lua: &Lua, bytecode: &[u8]) -> Result<Table, LvmError> {
    let env = environment(lua)?;
    let module: Value = lua
        .load(bytecode)
        .set_name("=contract")
        .set_mode(ChunkMode::Binary)
        .set_environment(env)
        .eval()?;

    match module {
        Value::Table(module) => Ok(module),
        _ => Err(LvmError::NotAModule),
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

/// Lowercase hex with a 0x prefix.
fn to_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(2 + bytes.len() * 2);
    out.push_str("0x");

    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }

    out
}

/// Hex with or without a 0x prefix, in either case.
fn from_hex(text: &str) -> Option<Vec<u8>> {
    let text = text.strip_prefix("0x").unwrap_or(text);

    if !text.len().is_multiple_of(2) {
        return None;
    }

    (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(text.get(index..index + 2)?, 16).ok())
        .collect()
}

#[cfg(test)]
mod tests;
