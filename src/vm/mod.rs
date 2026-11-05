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
    crate::structs::receipt::Payment,
    crate::structs::hash::{Hash, HashType},
    crate::structs::trace::Trace,
    crate::structs::tx::TxCallArgumentType,
    crate::structs::u256::u256,
    r#macro::document,
    std::fmt,
    std::sync::Arc,
};

pub mod crypto;
mod host;
mod layout;
#[cfg(feature = "lvm")]
pub mod lvm;
#[cfg(feature = "wasm")]
pub mod wasm;
#[cfg(feature = "evm")]
pub mod evm;

pub use crate::params::Limits;
pub use host::{Host, MAX_CALL_DEPTH, MAX_EVENTS, MAX_EVENT_DATA, MAX_EVENT_NAME, MAX_KEY_LEN, MAX_NAMESPACE_LEN, MAX_SCAN, MAX_VALUE_LEN, Space};
pub use layout::{Access, Declared, Entry, Field, Kind, Layout, Shape, Typed, TypedField};

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
    summary = "The hook the chain runs when a call names a function the contract does not export: it gets the function's selector, as a u64, in front of the call's arguments, and answers in its place. What makes generic proxies possible: delegate the selector and arguments to an implementation. Only the chain may run it.",
    category = "VM",
    see_also = "Host::delegate, HOOKS",
    example = "function M.fallback(selector, ...)\n    return contracts.delegate(address(storage.get(\"implementation\")), selector, ...)\nend",
)]
pub const FALLBACK: &str = "fallback";

#[document(
    summary = "The functions only the chain may run. A transaction or contract that calls or delegates to one is refused with Reserved.",
    category = "VM",
    see_also = "INIT, MIGRATE, FALLBACK",
)]
pub const HOOKS: &[&str] = &[INIT, MIGRATE, FALLBACK];

#[document(
    summary = "How a call names the function it wants: by name, as contracts do, or by the XXH3-64 selector transactions carry and fallback receives.",
    category = "VM",
    see_also = "selector, FALLBACK",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FunctionId<'a> {
    #[document(summary = "The exported function's name.")]
    Name(&'a str),
    #[document(summary = "The XXH3-64 of the name.")]
    Selector(u64),
}

impl FunctionId<'_> {
    #[document(parent = "FunctionId", summary = "The selector, worked out from the name when need be.")]
    pub fn selector(&self) -> u64 {
        match self {
            FunctionId::Name(name) => selector(name),
            FunctionId::Selector(selector) => *selector,
        }
    }

    fn matches(&self, name: &str) -> bool {
        match self {
            FunctionId::Name(wanted) => *wanted == name,
            FunctionId::Selector(wanted) => selector(name) == *wanted,
        }
    }
}

/// Which function a call runs, and with what arguments: the one it names, or, when the contract
/// does not export it and fallback is allowed, its fallback with the selector put in front.
pub(crate) fn route<'r>(
    record: &'r ContractRecord,
    id: FunctionId,
    args: &[TxCallArgumentType],
    fallback: bool,
) -> Result<(&'r str, Vec<TxCallArgumentType>), VmError> {
    match record.functions.iter().find(|name| id.matches(name)) {
        Some(name) if HOOKS.contains(&name.as_str()) => Err(VmError::Reserved(name.clone())),
        Some(name) => Ok((name, args.to_vec())),
        None => match record.functions.iter().find(|name| *name == FALLBACK) {
            Some(name) if fallback => {
                let mut routed = vec![TxCallArgumentType::U64(id.selector())];
                routed.extend_from_slice(args);

                Ok((name, routed))
            }
            _ => Err(VmError::NoSuchFunction),
        },
    }
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
    #[document(summary = "The block the call runs in. A read-only call runs at the tip's.")]
    pub block: BlockAt,
}

#[document(
    summary = "The block a call runs in, as contracts see it: what names expire by, and what an EVM call made from a contract reads as its block.",
    category = "VM",
    see_also = "CallContext",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockAt {
    #[document(summary = "Its height.")]
    pub height: u64,
    #[document(summary = "Its time, in milliseconds since the Unix epoch.")]
    pub time: u64,
    #[document(summary = "Who produced it.")]
    pub producer: Address,
    #[document(summary = "The fuel one EVM gas burns there: the chain's gas price in fuel, 1 on a chain without fees.")]
    pub fuel_per_gas: u64,
    #[document(summary = "What its contracts run under: host costs, contract limits, the EVM's spec and the DEX's fee.")]
    pub vm: crate::params::vm::VmRules,
}

impl BlockAt {
    #[document(parent = "BlockAt", summary = "A block at this time and nothing else known of it: height 0, no producer, gas one fuel. For tests and tools.")]
    pub fn at(time: u64) -> BlockAt {
        BlockAt {
            height: 0,
            time,
            producer: Address::Ed25519([0; 32]),
            fuel_per_gas: 1,
            vm: crate::params::vm::VmRules::default(),
        }
    }
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
    #[document(summary = "Where the call went, for one a transaction or the chain started: its own hop and every hop under it, failed or not. Empty for a call one contract made to another, whose hops are in the trace of the run it belongs to.")]
    pub trace: Trace,
    #[document(summary = "The contracts that asked, with tokens.refresh or refresh_token, for the token registry to ask their name, symbol and decimals again once the transaction succeeds, sorted. Empty when the call failed, or asked for none.")]
    pub refresh_tokens: Vec<Address>,
    #[document(summary = "The coin the call and the calls it made paid from contracts' balances, in order: value sent with a call, transfers, and the EVM's own moves. Empty when it failed.")]
    pub payments: Vec<Payment>,
}

impl Outcome {
    /// A call that failed before it could burn anything.
    pub(crate) fn failed(err: VmError) -> Outcome {
        Outcome {
            fuel_used: 0,
            result: Err(err),
            events: Vec::new(),
            trace: Trace::default(),
            refresh_tokens: Vec::new(),
            payments: Vec::new(),
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
    #[document(summary = "The namespaces the code declares it keeps its state in.")]
    pub layout: Layout,
}

#[document(
    summary = "What loading code did: the fuel it burned, failing or not, and the stored form or why the code cannot be deployed.",
    category = "VM",
    see_also = "ScriptEngine::load",
)]
#[derive(Debug)]
pub struct Loading {
    #[document(summary = "Fuel for compiling the code, the block's load_byte a byte, and for what it ran while loading. All the fuel allowed when it ran out.")]
    pub fuel_used: u64,
    #[document(summary = "The stored form and its exports, or why the code cannot be deployed.")]
    pub result: Result<Loaded, VmError>,
}

impl Loading {
    /// Loading that failed having burned this much.
    pub(crate) fn failed(fuel_used: u64, err: VmError) -> Loading {
        Loading {
            fuel_used,
            result: Err(err),
        }
    }

    /// What compiling code this long costs, or None when the fuel allowed does not cover it.
    pub(crate) fn fee(code: &[u8], limits: Limits, rules: &crate::params::vm::VmRules) -> Option<u64> {
        let fee = (code.len() as u64).saturating_mul(rules.costs.load_byte);

        (fee <= limits.fuel).then_some(fee)
    }
}

#[document(
    summary = "An engine that runs one kind of contract. Engines never touch the database themselves: everything outside the contract goes through the Host they are handed.",
    category = "VM",
    see_also = "Engine, Host, Vm",
)]
pub trait ScriptEngine: Send + Sync {
    #[document(
        parent = "ScriptEngine",
        summary = "Checks deployed code and turns it into what is stored, charging the block's load_byte a byte for it, and for whatever it runs while it loads, within the block's module or source size. That runs before any call is attached, so it sees nothing of one.",
        params = {
            code = "The code from the deploy: Luau source, or a WebAssembly module.",
            limits = "What loading, and anything run while loading, may use.",
            rules = "What the block's contracts run under: its costs and contract limits.",
        },
        returns = "The fuel it burned, and the stored form and its exports or why the code cannot be deployed.",
    )]
    fn load_under(&self, code: &[u8], limits: Limits, rules: &crate::params::vm::VmRules) -> Loading;

    #[document(parent = "ScriptEngine", summary = "Checks code as load_under does, by the default costs and limits: for tools and tests, which run no block.")]
    fn load(&self, code: &[u8], limits: Limits) -> Loading {
        self.load_under(code, limits, &crate::params::vm::VmRules::default())
    }

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
    summary = "How big each engine's per-node resources are: Lua's pool of pristine states and the threads that build them, and WebAssembly's cache of compiled modules. They trade memory for throughput and change no call's outcome, so each node picks its own, with --lua-states, --lua-builders, --wasm-cache-mb and --wasm-cache-modules. The EVM builds its context fresh for every call and has nothing to size.",
    category = "VM",
    see_also = "Vm::with_pools",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EnginePools {
    #[document(summary = "How many pristine Lua states are kept ready: lvm::POOL_SIZE by default.")]
    pub lua_states: usize,
    #[document(summary = "How many threads build them: lvm::BUILDERS by default.")]
    pub lua_builders: usize,
    #[document(summary = "How many bytes of WebAssembly modules the cache holds: wasm::MODULE_CACHE_BYTES by default.")]
    pub wasm_bytes: usize,
    #[document(summary = "How many WebAssembly modules the cache holds at most: wasm::MODULE_CACHE_ENTRIES by default.")]
    pub wasm_modules: usize,
}

impl EnginePools {
    #[document(parent = "EnginePools", summary = "What every node had before the pools were sized on the node: 16 Lua states built by 2 threads, and 32 MiB and 1,024 WebAssembly modules.")]
    pub const DEFAULT: EnginePools = EnginePools {
        lua_states: 16,
        lua_builders: 2,
        wasm_bytes: 32 * 1024 * 1024,
        wasm_modules: 1_024,
    };
}

impl Default for EnginePools {
    fn default() -> EnginePools {
        EnginePools::DEFAULT
    }
}

#[cfg(feature = "lvm")]
const _: () = assert!(EnginePools::DEFAULT.lua_states == lvm::POOL_SIZE && EnginePools::DEFAULT.lua_builders == lvm::BUILDERS);
#[cfg(feature = "wasm")]
const _: () = assert!(EnginePools::DEFAULT.wasm_bytes == wasm::MODULE_CACHE_BYTES && EnginePools::DEFAULT.wasm_modules == wasm::MODULE_CACHE_ENTRIES);

#[document(
    summary = "The engines one chain runs contracts on, built once from its genesis and owned by the Chain. Every call, and every call those calls make, goes through the same Vm.",
    category = "VM",
    see_also = "ScriptEngine, Genesis, Chain::open",
)]
pub struct Vm {
    engines: Vec<(Engine, Box<dyn ScriptEngine>)>,
    /// Whether the chain runs EVM contracts, which go through vm::evm rather than a ScriptEngine.
    evm: bool,
    /// The chain's hash, which contract addresses are derived with.
    hash: HashType,
    /// The chain's names, which contracts resolve, or None on a chain without them.
    names: Option<crate::registry::names::NamesConfig>,
    /// Genesis's federation, version 0, until governance decides another.
    federation: Option<crate::registry::federation::Federation>,
    /// Genesis's anchoring settings, until governance decides others.
    anchoring: Option<crate::params::anchoring::AnchorSettings>,
    /// The chain's id, as the EVM's CHAINID reads it.
    chain_id: u64,
}

impl Vm {
    #[document(
        parent = "Vm",
        summary = "Builds the engines a chain allows.",
        params = {
            allowed = "The engines from the chain's genesis.",
            hash = "The chain's hash, from genesis, which contract addresses are derived with.",
        },
        returns = "The Vm, or the first engine this node was built without.",
    )]
    pub fn new(allowed: &[Engine], hash: HashType) -> Result<Vm, Engine> {
        Vm::with_pools(allowed, hash, EnginePools::DEFAULT)
    }

    #[document(
        parent = "Vm",
        summary = "Builds the engines a chain allows, each sized as this node's operator asked: Lua's pool of states and WebAssembly's module cache. What they hold changes no call's outcome.",
        params = {
            allowed = "The engines from the chain's genesis.",
            hash = "The chain's hash, from genesis, which contract addresses are derived with.",
            pools = "How big each engine's per-node resources are.",
        },
        returns = "The Vm, or the first engine this node was built without.",
    )]
    pub fn with_pools(allowed: &[Engine], hash: HashType, pools: EnginePools) -> Result<Vm, Engine> {
        let _ = pools;
        let mut engines: Vec<(Engine, Box<dyn ScriptEngine>)> = Vec::new();
        // revm runs a transaction start to end itself, so the EVM is no ScriptEngine.
        let evm = cfg!(feature = "evm") && allowed.contains(&Engine::Evm);

        for kind in allowed {
            let engine: Box<dyn ScriptEngine> = match kind {
                #[cfg(feature = "evm")]
                Engine::Evm => continue,
                #[cfg(feature = "lvm")]
                Engine::Lua => Box::new(lvm::LuaEngine::with_pool(pools.lua_states, pools.lua_builders)),
                #[cfg(feature = "wasm")]
                Engine::Wasm => Box::new(wasm::WasmEngine::with_cache_of(pools.wasm_bytes, pools.wasm_modules)),
                #[allow(unreachable_patterns)]
                _ => return Err(*kind),
            };
            engines.push((*kind, engine));
        }

        Ok(Vm { engines, evm, hash, names: None, federation: None, anchoring: None, chain_id: 0 })
    }

    #[document(parent = "Vm", summary = "The engines, with the chain's names for contracts to resolve: genesis's names, or None for a chain without them.")]
    pub fn with_names(mut self, names: Option<crate::registry::names::NamesConfig>) -> Vm {
        self.names = names;
        self
    }

    #[document(parent = "Vm", summary = "The engines, with genesis's federation for contracts to read until governance decides another: None for a chain that starts without one.")]
    pub fn with_federation(mut self, federation: Option<crate::registry::federation::Federation>) -> Vm {
        self.federation = federation;
        self
    }

    #[document(parent = "Vm", summary = "Genesis's federation, if any.")]
    pub fn federation(&self) -> Option<&crate::registry::federation::Federation> {
        self.federation.as_ref()
    }

    #[document(parent = "Vm", summary = "The engines, with genesis's anchoring settings, which name the Ethereum network contracts read until governance decides others: None for a chain that starts without them.")]
    pub fn with_anchoring(mut self, anchoring: Option<crate::params::anchoring::AnchorSettings>) -> Vm {
        self.anchoring = anchoring;
        self
    }

    #[document(parent = "Vm", summary = "Genesis's anchoring settings, if any.")]
    pub fn anchoring(&self) -> Option<&crate::params::anchoring::AnchorSettings> {
        self.anchoring.as_ref()
    }

    #[document(parent = "Vm", summary = "The engines, with the chain's id for EVM calls contracts make: chain_id of the genesis block's id.")]
    pub fn with_chain_id(mut self, chain_id: u64) -> Vm {
        self.chain_id = chain_id;
        self
    }

    #[document(parent = "Vm", summary = "The chain's id, as the EVM's CHAINID reads it; 0 unless with_chain_id set it.")]
    pub fn chain_id(&self) -> u64 {
        self.chain_id
    }

    #[document(parent = "Vm", summary = "The chain's names, which contracts resolve, or None on a chain without them.")]
    pub fn names(&self) -> Option<&crate::registry::names::NamesConfig> {
        self.names.as_ref()
    }

    #[document(parent = "Vm", summary = "Whether the chain runs EVM contracts, and this node was built with the evm feature.")]
    pub fn runs_evm(&self) -> bool {
        self.evm
    }

    #[document(parent = "Vm", summary = "The chain's hash.")]
    pub fn hash(&self) -> HashType {
        self.hash
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
        summary = "Calls a deployed contract on behalf of a transaction, whichever engine runs it. A selector the contract does not export goes to its fallback, if it has one. Its storage writes land in state only when the call succeeds.",
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
        let id = match func {
            Hash::Xxh3_64(selector) => FunctionId::Selector(*selector),
            #[allow(unreachable_patterns)]
            _ => return Outcome::failed(VmError::NoSuchFunction),
        };

        match route(record, id, args, true) {
            Ok((function, args)) => self.run(state, record, context, function, &args, limits),
            Err(err) => Outcome::failed(err),
        }
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

        host::run_traced(state, record, function, args, host, limits)
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
pub(crate) mod tests;
