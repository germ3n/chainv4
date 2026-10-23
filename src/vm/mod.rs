//! Contract execution. Each engine runs one kind of contract, Luau scripts or WebAssembly modules,
//! behind ScriptEngine. What they share lives here: the limits a call runs under, who is calling,
//! what came of it, what host operations cost, and the Host a running contract reaches the chain
//! through: its own storage, and other contracts, whichever engine runs them.

use {
    crate::crypto::hasher::Hasher,
    crate::db::{DbError, State},
    crate::registry::contracts::ContractRecord,
    crate::structs::address::Address,
    crate::structs::code::Engine,
    crate::structs::event::Event,
    crate::structs::hash::{Hash, HashType},
    crate::structs::tx::TxCallArgumentType,
    crate::structs::u256::u256,
    r#macro::document,
    std::fmt,
    std::sync::Arc,
};

mod host;
#[cfg(feature = "lvm")]
pub mod lvm;
#[cfg(feature = "wasm")]
pub mod wasm;

pub use host::{Host, MAX_CALL_DEPTH, MAX_EVENTS, MAX_EVENT_DATA, MAX_EVENT_NAME, MAX_KEY_LEN, MAX_VALUE_LEN};

#[document(
    summary = "The hook the chain runs once, when the contract is deployed, with the deployer as sender and no arguments. The place to record an owner or mint an initial supply. If it fails, so does the deploy. Only the chain may run it.",
    category = "VM",
    see_also = "Contracts::deploy, HOOKS",
)]
pub const INIT: &str = "init";

#[document(
    summary = "The hook the chain runs once when a contract is upgraded to code that exports it, with the upgrader as sender and no arguments. The place to move storage to a new layout. If it fails, so does the upgrade. Only the chain may run it.",
    category = "VM",
    see_also = "Contracts::upgrade, HOOKS",
)]
pub const MIGRATE: &str = "migrate";

#[document(
    summary = "The function the chain asks before a privileged action on a contract, with the permission's name and the address asking, such as (\"upgrade\", ed25519:0x…). Only a single true lets the action through, and what it writes is thrown away. A contract that does not export it can never be upgraded.",
    category = "VM",
    see_also = "Contracts::upgrade",
    example = "function M.check_perms(permission, who)\n    return permission == \"upgrade\" and who:hex() == storage.get(\"owner\")\nend",
)]
pub const CHECK_PERMS: &str = "check_perms";

#[document(
    summary = "The functions only the chain may run. A transaction or contract that calls one is refused with Reserved.",
    category = "VM",
    see_also = "INIT, MIGRATE",
)]
pub const HOOKS: &[&str] = &[INIT, MIGRATE];

/// What host operations cost in fuel. The same in every engine, so moving a contract from one
/// engine to another never changes what its storage or calls cost.
pub mod cost {
    /// A storage read, plus 1 for every STORAGE_BYTES_PER_FUEL bytes of key and value.
    pub const STORAGE_READ: u64 = 100;
    /// A storage write or delete. Dearer than a read: every node keeps what is written.
    pub const STORAGE_WRITE: u64 = 500;
    pub const STORAGE_BYTES_PER_FUEL: u64 = 16;
    /// One hash, plus 1 for every HASH_BYTES_PER_FUEL bytes hashed, a block of most algorithms.
    pub const HASH: u64 = 20;
    pub const HASH_BYTES_PER_FUEL: u64 = 64;
    /// Calling another contract, on top of the fuel the callee burns.
    pub const CALL: u64 = 1_000;
    /// Reading a balance: one account read.
    pub const BALANCE: u64 = 100;
    /// Moving funds, by transfer or with a call: two account writes.
    pub const TRANSFER: u64 = 1_000;
    /// Emitting an event, plus 1 for every EVENT_BYTES_PER_FUEL bytes of name and data: every
    /// node keeps it in the receipt.
    pub const EVENT: u64 = 200;
    pub const EVENT_BYTES_PER_FUEL: u64 = 16;
    /// Copying bytes between a WebAssembly contract's memory and the host.
    pub const COPY_BYTES_PER_FUEL: u64 = 64;
}

#[document(
    summary = "What one call may use. Running out of either stops the contract.",
    category = "VM",
    see_also = "ScriptEngine::run, Host::call",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    #[document(summary = "Instruction budget, charged per instruction by the engine and per byte by host operations that do bulk work. A call to another contract spends from the caller's budget.")]
    pub fuel: u64,
    #[document(summary = "Most bytes one call's engine may allocate. Each contract in a chain of calls gets its own.")]
    pub memory: usize,
}

#[document(
    summary = "Who is calling, which contract, and what they sent. Lua scripts read it through ctx, WebAssembly modules through the sender, contract and value host functions.",
    category = "VM",
    see_also = "Host",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CallContext {
    #[document(summary = "The address making the call: the transaction's sender, or the calling contract.")]
    pub sender: Address,
    #[document(summary = "The address of the contract being called.")]
    pub contract: Address,
    #[document(summary = "The amount sent with the call. Calls between contracts send nothing.")]
    pub value: u256,
}

#[document(
    summary = "What a call did.",
    category = "VM",
    see_also = "ScriptEngine::run",
)]
#[derive(Debug)]
pub struct Outcome {
    #[document(summary = "Fuel the call burned, including any contracts it called. Charged whether or not the call succeeded.")]
    pub fuel_used: u64,
    #[document(summary = "The function's return values, or why it failed.")]
    pub result: Result<Vec<TxCallArgumentType>, VmError>,
    #[document(summary = "What the call and the calls it made emitted, in order. Empty when it failed.")]
    pub events: Vec<Event>,
}

impl Outcome {
    /// A call that failed before it could burn anything.
    pub(crate) fn failed(err: VmError) -> Outcome {
        Outcome {
            fuel_used: 0,
            result: Err(err),
            events: Vec::new(),
        }
    }
}

#[document(
    summary = "A contract checked and ready to store: what the engine runs, and the functions it exports.",
    category = "VM",
    see_also = "ScriptEngine::load",
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Loaded {
    #[document(summary = "Lua bytecode compiled by this node, or the validated WebAssembly module.")]
    pub code: Vec<u8>,
    #[document(summary = "The exported function names, sorted so every node stores the same list.")]
    pub functions: Vec<String>,
    #[document(summary = "The exported functions marked reentrant, sorted. Every other function refuses to run while its contract is already running further up the chain of calls.")]
    pub reentrant: Vec<String>,
}

#[document(
    summary = "An engine that runs one kind of contract. Engines never touch the database themselves: everything outside the contract goes through the Host they are handed.",
    category = "VM",
    see_also = "Engine, Host, Vm",
)]
pub trait ScriptEngine: Send + Sync {
    #[document(
        parent = "ScriptEngine",
        summary = "Checks deployed code and turns it into what is stored.",
        params = {
            code = "The code from the deploy: Luau source, or a WebAssembly module.",
            limits = "What any code run while loading may use.",
        },
        returns = "The stored form and its exports, or why the code cannot be deployed.",
    )]
    fn load(&self, code: &[u8], limits: Limits) -> Result<Loaded, VmError>;

    #[document(
        parent = "ScriptEngine",
        summary = "Runs one exported function from scratch: nothing a call leaves behind reaches the next.",
        params = {
            code = "The stored form, from load.",
            function = "The exported function's name.",
            args = "The arguments.",
            host = "The call's storage, context and way out to other contracts.",
            limits = "What the call may use.",
        },
        returns = "The fuel used, and the return values or why the call failed.",
    )]
    fn run(&self, code: &[u8], function: &str, args: &[TxCallArgumentType], host: Host, limits: Limits) -> Outcome;
}

#[document(
    summary = "The engines one chain runs contracts on, built once from its genesis and owned by the Chain. Every call, and every call those calls make, goes through the same Vm.",
    category = "VM",
    see_also = "ScriptEngine, Genesis, Chain::open",
)]
pub struct Vm {
    engines: Vec<(Engine, Box<dyn ScriptEngine>)>,
}

impl Vm {
    #[document(
        parent = "Vm",
        summary = "Builds the engines a chain allows.",
        params = { allowed = "The engines from the chain's genesis." },
        returns = "The Vm, or the first engine this node was built without.",
    )]
    pub fn new(allowed: &[Engine]) -> Result<Vm, Engine> {
        let mut engines: Vec<(Engine, Box<dyn ScriptEngine>)> = Vec::new();

        for kind in allowed {
            let engine: Box<dyn ScriptEngine> = match kind {
                #[cfg(feature = "lvm")]
                Engine::Lua => Box::new(lvm::LuaEngine),
                #[cfg(feature = "wasm")]
                Engine::Wasm => Box::new(wasm::WasmEngine::new()),
                #[allow(unreachable_patterns)]
                _ => return Err(*kind),
            };
            engines.push((*kind, engine));
        }

        Ok(Vm { engines })
    }

    #[document(
        parent = "Vm",
        summary = "The engine for a kind of contract.",
        returns = "The engine, or None when the chain does not allow that kind.",
    )]
    pub fn engine(&self, kind: Engine) -> Option<&dyn ScriptEngine> {
        self.engines
            .iter()
            .find(|(candidate, _)| *candidate == kind)
            .map(|(_, engine)| engine.as_ref())
    }

    #[document(
        parent = "Vm",
        summary = "Calls a deployed contract on behalf of a transaction, whichever engine runs it. Its storage writes land in state only when the call succeeds.",
        params = {
            state = "The transaction's state layer.",
            record = "The contract, as stored.",
            context = "Who is calling, which contract, and how much they sent.",
            func = "The XXH3-64 of the function's name, as in Tx::ContractCall.",
            args = "The arguments.",
            limits = "What the whole call, contracts it calls included, may use.",
        },
        see_also = "Contracts::call, Host::call",
    )]
    pub fn call(
        self: &Arc<Self>,
        state: &Arc<State>,
        record: &ContractRecord,
        context: CallContext,
        func: &Hash,
        args: &[TxCallArgumentType],
        limits: Limits,
    ) -> Outcome {
        let Some(function) = record.functions.iter().find(|name| Hash::Xxh3_64(selector(name)) == *func) else {
            return Outcome::failed(VmError::NoSuchFunction);
        };

        if HOOKS.contains(&function.as_str()) {
            return Outcome::failed(VmError::Reserved(function.clone()));
        }

        self.run(state, record, context, function, args, limits)
    }

    #[document(
        parent = "Vm",
        summary = "Runs a contract function by name, hooks such as init and migrate included. For the chain itself; transactions go through call.",
        params = {
            state = "The state layer the run's writes land in, if it succeeds.",
            record = "The contract, as stored.",
            context = "Who is asking, which contract, and what was sent.",
            function = "The function's name.",
            args = "The arguments.",
            limits = "What the run, contracts it calls included, may use.",
        },
        see_also = "Contracts::deploy, Contracts::upgrade",
    )]
    pub fn run(
        self: &Arc<Self>,
        state: &Arc<State>,
        record: &ContractRecord,
        context: CallContext,
        function: &str,
        args: &[TxCallArgumentType],
        limits: Limits,
    ) -> Outcome {
        let layer = Arc::new(State::new(state.clone()));
        let host = Host::root(layer, context, limits.memory, self.clone());

        host::run(state, record, function, args, host, limits)
    }
}

#[document(
    summary = "The hash a contract call names a function by: XXH3-64 of the name.",
    category = "VM",
    see_also = "Tx::ContractCall",
)]
pub fn selector(name: &str) -> u64 {
    match Hasher::digest(HashType::Xxh3_64, name.as_bytes()) {
        Hash::Xxh3_64(selector) => selector,
        #[allow(unreachable_patterns)]
        _ => unreachable!("XXH3-64 always gives an XXH3-64 hash"),
    }
}

#[document(
    summary = "Why a contract could not be loaded or a call failed.",
    category = "VM",
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VmError {
    #[document(summary = "The code does not compile or validate.")]
    Compile(String),
    #[document(summary = "A Lua script's top level returned something other than a table of functions.")]
    NotAModule,
    #[document(summary = "Two exported functions have the same XXH3-64 hash, so a call could not tell them apart.")]
    SelectorClash(String, String),
    #[document(summary = "No exported function has the name or hash the call asked for.")]
    NoSuchFunction,
    #[document(summary = "The contract used all its fuel.")]
    OutOfFuel,
    #[document(summary = "The contract tried to use more memory than its limit.")]
    OutOfMemory,
    #[document(summary = "The contract raised an error or trapped, or a host operation refused it.")]
    Runtime(String),
    #[document(summary = "An argument has no equivalent in the contract's engine.")]
    BadArgument(String),
    #[document(summary = "A return value has no TxCallArgumentType equivalent.")]
    BadReturn(String),
    #[document(summary = "A contract called an address that holds no contract.")]
    NoSuchContract(Address),
    #[document(summary = "A transaction or contract called a hook only the chain runs: init or migrate.")]
    Reserved(String),
    #[document(summary = "A contract called one that is already running further up the chain of calls, and the function it called is not marked reentrant.")]
    Reentrant(Address),
    #[document(summary = "The chain of calls would go deeper than MAX_CALL_DEPTH.")]
    TooDeep,
    #[document(summary = "A contract tried to send more than it holds, by transfer or with a call.")]
    InsufficientBalance,
    #[document(summary = "The chain does not run the engine the contract needs.")]
    EngineMissing(Engine),
    #[document(summary = "The database failed while the contract ran. The node's fault, not the contract's, so the node should stop rather than record the call as failed.")]
    Storage(DbError),
}

impl fmt::Display for VmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VmError::Compile(text) => write!(f, "compile error: {text}"),
            VmError::NotAModule => write!(f, "script must return a table of functions"),
            VmError::SelectorClash(first, second) => {
                write!(f, "functions {first} and {second} have the same hash")
            }
            VmError::NoSuchFunction => write!(f, "no such function"),
            VmError::OutOfFuel => write!(f, "out of fuel"),
            VmError::OutOfMemory => write!(f, "out of memory"),
            VmError::Runtime(text) => write!(f, "{text}"),
            VmError::BadArgument(text) => write!(f, "bad argument: {text}"),
            VmError::BadReturn(text) => write!(f, "bad return value: {text}"),
            VmError::NoSuchContract(address) => write!(f, "no contract at {address}"),
            VmError::Reserved(name) => write!(f, "{name} is only run by the chain"),
            VmError::Reentrant(address) => {
                write!(f, "{address} is already running and the function is not reentrant")
            }
            VmError::TooDeep => write!(f, "calls nest deeper than {MAX_CALL_DEPTH}"),
            VmError::InsufficientBalance => write!(f, "insufficient balance"),
            VmError::EngineMissing(engine) => write!(f, "this chain does not run {} contracts", engine.name()),
            VmError::Storage(err) => write!(f, "storage failed: {err}"),
        }
    }
}

impl std::error::Error for VmError {}

impl From<DbError> for VmError {
    fn from(err: DbError) -> VmError {
        VmError::Storage(err)
    }
}

#[cfg(all(test, feature = "lvm", feature = "wasm"))]
mod tests;
