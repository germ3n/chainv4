//! The WebAssembly engine, on wasmi: an interpreter with fuel metering built in, whose stacks are
//! its own rather than the machine's, so every node runs a module the same way. Everything here
//! is consensus: the wasmi version, the features a module may use, and the stack limits. Changing
//! any of them changes what some module does or what it costs.
//!
//! Compiling a module is nearly all of what a call costs, so compiled modules are cached by their
//! code. That is not consensus: the same bytes always compile to the same module, so a cached one
//! runs exactly as a fresh one would. Every call still gets a fresh instance, with fresh memory.

use {
    super::{Host, Limits, Loaded, Outcome, ScriptEngine, VmError, selector},
    crate::structs::trace::Trace,
    crate::structs::tx::TxCallArgumentType,
    r#macro::document,
    std::collections::HashMap,
    std::sync::{Arc, Mutex},
    xxhash_rust::xxh3::Xxh3Builder,
    wasmi::{
        CompilationMode, Config, EnforcedLimits, Engine, ExternType, Linker, Module, Store, StoreLimits,
        StoreLimitsBuilder, TrapCode,
    },
};

mod host;

#[document(
    summary = "The largest module a deploy may carry. Validating and compiling a module is not metered by fuel, so this bounds the work one deploy, or one call, can cause.",
    category = "WASM VM",
)]
pub const MAX_MODULE_LEN: usize = 512 * 1024;
#[document(
    summary = "The module every host function is imported from.",
    category = "WASM VM",
    see_also = "wasm/chain",
)]
pub const IMPORT_MODULE: &str = "chain";
#[document(
    summary = "The name a module must export its memory under, for the host to read and write.",
    category = "WASM VM",
)]
pub const MEMORY_EXPORT: &str = "memory";
#[document(
    summary = "The custom section naming the functions that may be entered while their contract is already running: names, each ending in a zero byte. The SDK's #[export(reentrant)] writes it, and the linker joins the pieces from every function into one section.",
    category = "WASM VM",
)]
pub const REENTRANT_SECTION: &str = "chainv4.reentrant";
/// How much code the module cache holds, in bytes of modules, and how many modules at most.
const MODULE_CACHE_BYTES: usize = 32 * 1024 * 1024;
const MODULE_CACHE_ENTRIES: usize = 1_024;

/// How deep functions may recurse, and how many values the stack may hold.
const MAX_RECURSION_DEPTH: usize = 1_000;
const MAX_STACK_HEIGHT: usize = 1_000_000;

#[document(
    summary = "The WebAssembly engine. A contract is a module that imports host functions from \"chain\", exports its memory as \"memory\", and exports each callable function by name, taking and returning nothing: arguments and return values travel through input and output.",
    category = "WASM VM",
    see_also = "ScriptEngine, Host, Engine::Wasm, wasm/chain",
    note = "Floating-point instructions, SIMD and start functions are refused at deploy, since floats are the one part of WebAssembly that machines may disagree on. The host functions a contract imports are in the WASM reference, under chain.",
)]
pub struct WasmEngine {
    /// The interpreter, with the chain's settings. Modules and stores must come from the same one.
    engine: Engine,
    /// The host functions, resolved against every module's imports when it is instantiated.
    linker: Linker<Run>,
    /// Modules already compiled, by their code.
    modules: Mutex<Modules>,
}

/// Compiled modules by their code. One compiled module serves every call to every contract with
/// that code; an upgrade is new code, which compiles once and takes a place of its own. Once the
/// cache is full, the module no call used for longest leaves first.
struct Modules {
    entries: HashMap<Arc<[u8]>, Cached, Xxh3Builder>,
    /// The code the entries hold, in bytes.
    bytes: usize,
    /// Counts lookups, to tell which entry was used longest ago.
    clock: u64,
    budget: usize,
}

struct Cached {
    module: Module,
    used: u64,
}

impl Modules {
    fn new(budget: usize) -> Modules {
        Modules {
            entries: HashMap::with_hasher(Xxh3Builder::new()),
            bytes: 0,
            clock: 0,
            budget,
        }
    }

    fn get(&mut self, code: &[u8]) -> Option<Module> {
        self.clock += 1;
        let entry = self.entries.get_mut(code)?;
        entry.used = self.clock;

        Some(entry.module.clone())
    }

    fn insert(&mut self, code: &[u8], module: Module) {
        if code.len() > self.budget || self.entries.contains_key(code) {
            return;
        }

        while self.bytes + code.len() > self.budget || self.entries.len() >= MODULE_CACHE_ENTRIES {
            let Some(oldest) = self.entries.iter().min_by_key(|(_, entry)| entry.used).map(|(code, _)| code.clone()) else {
                break;
            };
            self.entries.remove(&oldest);
            self.bytes -= oldest.len();
        }

        self.clock += 1;
        self.bytes += code.len();
        self.entries.insert(Arc::from(code), Cached {
            module,
            used: self.clock,
        });
    }
}

impl WasmEngine {
    #[document(
        parent = "WasmEngine",
        summary = "Sets up the interpreter with the chain's settings and defines the host functions. Vm::new does this once per chain.",
    )]
    pub fn new() -> WasmEngine {
        WasmEngine::with_cache(MODULE_CACHE_BYTES)
    }

    /// The engine, with room for this many bytes of compiled modules.
    fn with_cache(budget: usize) -> WasmEngine {
        let mut config = Config::default();
        config
            .consume_fuel(true)
            .floats(false)
            .allow_start_fn(false)
            .wasm_multi_memory(false)
            .compilation_mode(CompilationMode::Eager)
            .enforced_limits(EnforcedLimits::strict())
            .set_max_recursion_depth(MAX_RECURSION_DEPTH)
            .set_max_stack_height(MAX_STACK_HEIGHT);
        let engine = Engine::new(&config);
        let linker = host::linker(&engine);

        WasmEngine {
            engine,
            linker,
            modules: Mutex::new(Modules::new(budget)),
        }
    }

    /// The compiled module for some code: from the cache, or compiled now and cached.
    fn module(&self, code: &[u8]) -> Result<Module, VmError> {
        if let Some(module) = self.cache().get(code) {
            return Ok(module);
        }

        let module = Module::new(&self.engine, code).map_err(|err| VmError::Compile(err.to_string()))?;
        self.cache().insert(code, module.clone());

        Ok(module)
    }

    fn cache(&self) -> std::sync::MutexGuard<'_, Modules> {
        self.modules.lock().expect("the module cache never panics")
    }

    fn store(&self, host: Option<Host>, input: Vec<u8>, limits: Limits) -> Store<Run> {
        let run = Run {
            host,
            input,
            output: None,
            result: Vec::new(),
            limits: StoreLimitsBuilder::new()
                .memory_size(limits.memory)
                .memories(1)
                .tables(1)
                .instances(1)
                .trap_on_grow_failure(true)
                .build(),
            error: None,
        };
        let mut store = Store::new(&self.engine, run);
        store.limiter(|run| &mut run.limits);
        store.set_fuel(limits.fuel).expect("the engine meters fuel");

        store
    }
}

impl Default for WasmEngine {
    fn default() -> WasmEngine {
        WasmEngine::new()
    }
}

impl ScriptEngine for WasmEngine {
    fn load(&self, code: &[u8], limits: Limits) -> Result<Loaded, VmError> {
        if code.len() > MAX_MODULE_LEN {
            return Err(VmError::Compile(format!(
                "module is {} bytes, more than the {MAX_MODULE_LEN} allowed",
                code.len()
            )));
        }

        // Not cached until it passes every check, since a deploy that fails never runs.
        let module = Module::new(&self.engine, code).map_err(|err| VmError::Compile(err.to_string()))?;

        for import in module.imports() {
            if import.module() != IMPORT_MODULE || !host::NAMES.contains(&import.name()) {
                return Err(VmError::Compile(format!(
                    "unknown import {}.{}",
                    import.module(),
                    import.name()
                )));
            }
        }

        let mut functions: HashMap<u64, String> = HashMap::new();
        let mut memory = false;

        for export in module.exports() {
            match export.ty() {
                ExternType::Memory(_) if export.name() == MEMORY_EXPORT => memory = true,
                ExternType::Func(ty) if ty.params().is_empty() && ty.results().is_empty() => {
                    let name = export.name().to_string();

                    if let Some(previous) = functions.insert(selector(&name), name.clone()) {
                        return Err(VmError::SelectorClash(previous, name));
                    }
                }
                _ => {}
            }
        }

        if !memory {
            return Err(VmError::Compile(format!("a contract must export its memory as {MEMORY_EXPORT}")));
        }

        // Instantiating links every import and sizes the memory, so a module that cannot run
        // fails here rather than in its first call.
        let mut store = self.store(None, Vec::new(), limits);
        self.linker
            .instantiate_and_start(&mut store, &module)
            .map_err(|err| store.data_mut().error.take().unwrap_or_else(|| trapped(&err)))?;

        let mut functions: Vec<String> = functions.into_values().collect();
        functions.sort();
        let reentrant = reentrant(&module, &functions)?;
        self.cache().insert(code, module);

        Ok(Loaded {
            code: code.to_vec(),
            functions,
            reentrant,
        })
    }

    fn run(&self, code: &[u8], function: &str, args: &[TxCallArgumentType], host: Host, limits: Limits) -> Outcome {
        let module = match self.module(code) {
            Ok(module) => module,
            Err(err) => return Outcome::failed(err),
        };
        let input = wincode::serialize(&args.to_vec()).expect("arguments always encode");
        let mut store = self.store(Some(host), input, limits);
        let result = self
            .linker
            .instantiate_and_start(&mut store, &module)
            .and_then(|instance| instance.get_typed_func::<(), ()>(&store, function))
            .and_then(|func| func.call(&mut store, ()));
        let remaining = store.get_fuel().unwrap_or(0);
        let run = store.into_data();
        let result = match result {
            Ok(()) => match run.output {
                Some(output) => wincode::deserialize::<Vec<TxCallArgumentType>>(&output)
                    .map_err(|err| VmError::BadReturn(err.to_string())),
                None => Ok(Vec::new()),
            },
            Err(err) => Err(run.error.unwrap_or_else(|| trapped(&err))),
        };

        Outcome {
            fuel_used: limits.fuel - remaining,
            result,
            events: Vec::new(),
            trace: Trace::default(),
        }
    }
}

/// What one instance's host functions work with.
struct Run {
    /// None while a deploy checks the module, when nothing may be called.
    host: Option<Host>,
    input: Vec<u8>,
    output: Option<Vec<u8>>,
    /// What the last storage_get or call produced, for result_read.
    result: Vec<u8>,
    limits: StoreLimits,
    /// Why a host function stopped the module, which says more than the trap does.
    error: Option<VmError>,
}

/// The functions the module's chainv4.reentrant sections name, sorted. Each must be exported.
fn reentrant(module: &Module, exported: &[String]) -> Result<Vec<String>, VmError> {
    let mut marked = Vec::new();

    for section in module.custom_sections().filter(|section| section.name() == REENTRANT_SECTION) {
        let Some(names) = section.data().strip_suffix(&[0]) else {
            return Err(VmError::Compile(format!("{REENTRANT_SECTION} must end each name with a zero byte")));
        };

        for name in names.split(|byte| *byte == 0) {
            let name = String::from_utf8_lossy(name).into_owned();

            if !exported.contains(&name) {
                return Err(VmError::Compile(format!("{REENTRANT_SECTION} names {name}, which the module does not export")));
            }

            marked.push(name);
        }
    }

    marked.sort();
    marked.dedup();

    Ok(marked)
}

fn trapped(err: &wasmi::Error) -> VmError {
    match err.as_trap_code() {
        Some(TrapCode::OutOfFuel) => VmError::OutOfFuel,
        Some(TrapCode::GrowthOperationLimited) => VmError::OutOfMemory,
        _ => VmError::Runtime(err.to_string()),
    }
}

#[cfg(test)]
mod tests;
