use {
    super::RegistryError,
    crate::crypto::hashable::Hashable,
    crate::crypto::hasher::Hasher,
    crate::db::{Column, DbError, State, address_key},
    crate::registry::tokens::{METADATA_FUNCTIONS, Token, Tokens},
    crate::structs::address::Address,
    crate::structs::code::{Code, Engine},
    crate::structs::event::Event,
    crate::structs::hash::{Hash, HashType},
    crate::structs::trace::Trace,
    crate::structs::tx::TxCallArgumentType,
    crate::structs::u256::u256,
    crate::vm::{CHECK_PERMS, CallContext, INIT, Layout, Limits, MIGRATE, Outcome, Vm, VmError},
    r#macro::document,
    std::sync::Arc,
    wincode::{SchemaRead, SchemaWrite},
};

/// Separates contract addresses from every other hash the chain takes.
const ADDRESS_DOMAIN: &[u8] = b"chainv4 contract address";

#[document(
    summary = "A deployed contract as stored: who deployed it, which engine runs it, what the engine made of its code, the names it exports, and the storage layout it declares.",
    category = "Registries",
    see_also = "Contracts, Loaded",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct ContractRecord {
    #[document(summary = "The address that deployed it.")]
    pub deployer: Address,
    #[document(summary = "The engine that runs it.")]
    pub engine: Engine,
    #[document(summary = "Lua bytecode this node compiled from the source, or the validated WebAssembly module.")]
    pub code: Vec<u8>,
    #[document(summary = "The exported function names, sorted.")]
    pub functions: Vec<String>,
    #[document(summary = "The exported functions that may be entered while the contract is already running, sorted.")]
    pub reentrant: Vec<String>,
    #[document(summary = "The namespaces its code declares it keeps its state in. New code must keep them.")]
    pub layout: Layout,
}

impl Hashable for ContractRecord {}

#[document(
    summary = "Deployed contracts by address: deploys them and calls them against the state it was given, whichever engine runs them.",
    category = "Registries",
    see_also = "ContractRecord, Contract, Accounts",
    note = "Moving the value a call sends is not done here: debit the sender and credit the contract with Accounts before calling, in the same transaction layer, so a failed call takes both back.",
    example = "let contracts = Contracts::new(&tx);\nlet address = contracts.deploy(&vm, &deployer, nonce, &code, limits)?;\nlet outcome = contracts.call(&vm, &context, &func, &args, limits)?;",
)]
pub struct Contracts<'a> {
    state: &'a Arc<State>,
}

impl<'a> Contracts<'a> {
    #[document(
        parent = "Contracts",
        summary = "Reads, writes and runs contracts in the given state layer.",
    )]
    pub fn new(state: &'a Arc<State>) -> Contracts<'a> {
        Contracts { state }
    }

    #[document(
        parent = "Contracts",
        summary = "Has the contract's engine check and load its code, stores it at an address derived from the deployer and their nonce, and runs its init, if it exports one, with the deployer as sender. A contract that exports every one of token_functions then goes in the token registry, with what its name(), symbol() and decimals() answer, for those it exports.",
        params = {
            vm = "The chain's engines.",
            deployer = "The address deploying it.",
            nonce = "The deployer's nonce for this transaction, which makes the address unique.",
            code = "The contract: Luau source, which nodes compile themselves, or a WebAssembly module.",
            limits = "What loading and init may use.",
            token_functions = "What makes a contract a token, as genesis says; empty keeps no registry.",
        },
        returns = "The new contract's address, and what loading the code, init and the token registry's questions burned, and what init emitted. A failure carries what was burned before it.",
        panics = "Vm when the code does not load, init fails or the chain does not run its engine, AlreadyDeployed when the address is taken, BadToken when it is a token but its name(), symbol() or decimals() answers badly.",
        see_also = "contract_address, ScriptEngine::load, INIT, Tokens",
    )]
    pub fn deploy(
        &self,
        vm: &Arc<Vm>,
        deployer: &Address,
        nonce: u64,
        code: &Code,
        limits: Limits,
        token_functions: &[String],
    ) -> Result<(Address, Hooked), Failed> {
        let address = contract_address(deployer, nonce, vm.hash());

        if self.get(&address)?.is_some() {
            return Err(RegistryError::AlreadyDeployed(address).into());
        }

        let (record, loaded) = load(vm, *deployer, code, limits)?;
        self.state.put_value(Column::Contracts, address_key(&address), &record);
        let context = CallContext {
            sender: *deployer,
            contract: address,
            value: u256::ZERO,
        };
        let mut hooked = self
            .hook(vm, &record, context, INIT, spent(limits, loaded))
            .map_err(|failed| failed.after(loaded, Trace::default()))?;
        hooked.fuel_used += loaded;
        self.register(vm, context, limits, token_functions, &mut hooked)?;

        Ok((address, hooked))
    }

    #[document(
        parent = "Contracts",
        summary = "Replaces a contract's code, keeping its address, storage and balance. The contract decides who may: its check_perms is asked with \"upgrade\" and the sender, and must return true. The new code must keep the old code's storage layout. Then the new code's migrate runs, if it exports one, with the sender as sender, and the token registry is brought in line with what the new code exports. If anything fails, nothing changes.",
        params = {
            vm = "The chain's engines.",
            sender = "Who asks for the upgrade.",
            contract = "The contract to upgrade.",
            code = "The new code, for either engine.",
            limits = "What check_perms, loading and migrate may use together.",
            token_functions = "What makes a contract a token, as genesis says; empty keeps no registry.",
        },
        returns = "What check_perms, loading the new code, migrate and the token registry's questions burned, and what migrate emitted. A failure carries what was burned before it.",
        panics = "NotPermitted when the contract has no check_perms or it does not return true, NoSuchContract when there is nothing to upgrade, Vm when the code does not load or migrate fails, Layout when the new code does not keep the old layout, BadToken when the new code is a token but its name(), symbol() or decimals() answers badly.",
        see_also = "CHECK_PERMS, MIGRATE, Tx::Upgrade, Tokens, Layout::keeps",
    )]
    pub fn upgrade(
        &self,
        vm: &Arc<Vm>,
        sender: &Address,
        contract: &Address,
        code: &Code,
        limits: Limits,
        token_functions: &[String],
    ) -> Result<Hooked, Failed> {
        let current = self.get(contract)?.ok_or(RegistryError::NoSuchContract(*contract))?;
        let context = CallContext {
            sender: *sender,
            contract: *contract,
            value: u256::ZERO,
        };
        let (asked, mut trace) = self.permits(vm, &current, context, "upgrade", limits)?;
        let (record, loaded) = load(vm, current.deployer, code, spent(limits, asked))
            .map_err(|failed| failed.after(asked, trace.clone()))?;
        let before = asked + loaded;

        if let Err(why) = record.layout.keeps(&current.layout) {
            return Err(Failed {
                error: RegistryError::Layout(why),
                fuel_used: before,
                trace,
            });
        }

        self.state.put_value(Column::Contracts, address_key(contract), &record);
        let mut hooked = self
            .hook(vm, &record, context, MIGRATE, spent(limits, before))
            .map_err(|failed| failed.after(before, trace.clone()))?;
        hooked.fuel_used += before;
        trace.append(std::mem::take(&mut hooked.trace));
        hooked.trace = trace;
        self.register(vm, context, limits, token_functions, &mut hooked)?;

        Ok(hooked)
    }

    #[document(
        parent = "Contracts",
        summary = "Brings a contract's token registry entry in line with it: a contract that exports every one of token_functions is registered, with what those of name(), symbol() and decimals() it exports answer; one that does not is taken off. The questions run on a throwaway copy of state, so whatever they write or emit is gone. Deploys and upgrades do this themselves, and a transaction does it for the tokens that asked with tokens.refresh.",
        params = {
            vm = "The chain's engines.",
            contract = "The contract.",
            sender = "Who the questions come from: the transaction's sender.",
            limits = "What the questions may use together.",
            token_functions = "What makes a contract a token, as genesis says; empty keeps no registry, and does nothing.",
        },
        returns = "What the questions burned and where they went. A failure carries those too.",
        panics = "BadToken when an answer does not fit, Vm(OutOfFuel) when the questions run out, NoSuchContract when no contract lives there.",
        see_also = "Tokens, Host::refresh_token",
    )]
    pub fn refresh(
        &self,
        vm: &Arc<Vm>,
        contract: &Address,
        sender: Address,
        limits: Limits,
        token_functions: &[String],
    ) -> Result<(u64, Trace), Failed> {
        let tokens = Tokens::new(self.state);

        if token_functions.is_empty() {
            return Ok((0, Trace::default()));
        }

        let record = self.get(contract)?.ok_or(RegistryError::NoSuchContract(*contract))?;
        let exports = |function: &str| record.functions.iter().any(|name| name == function);

        if !token_functions.iter().all(|function| exports(function)) {
            tokens.unregister(contract)?;

            return Ok((0, Trace::default()));
        }

        let context = CallContext {
            sender,
            contract: *contract,
            value: u256::ZERO,
        };
        let scratch = Arc::new(State::new(self.state.clone()));
        let (mut fuel_used, mut trace) = (0, Trace::default());
        let mut answers = Vec::with_capacity(METADATA_FUNCTIONS.len());

        for function in METADATA_FUNCTIONS {
            if !exports(function) {
                answers.push(None);

                continue;
            }

            let mut outcome = vm.run(&scratch, &record, context, function, &[], spent(limits, fuel_used));
            outcome.trace.forget_events();
            fuel_used += outcome.fuel_used;
            trace.append(outcome.trace);
            let error = match outcome.result {
                Ok(answer) => {
                    answers.push(Some(answer));

                    continue;
                }
                Err(VmError::Storage(err)) => RegistryError::Db(err),
                Err(VmError::OutOfFuel) => RegistryError::Vm(VmError::OutOfFuel),
                Err(err) => RegistryError::BadToken(format!("{function}() failed: {err}")),
            };

            return Err(Failed { error, fuel_used, trace });
        }

        match Token::from_answers(*contract, answers[0].as_deref(), answers[1].as_deref(), answers[2].as_deref()) {
            Ok(token) => {
                tokens.register(&token)?;

                Ok((fuel_used, trace))
            }
            Err(why) => Err(Failed {
                error: RegistryError::BadToken(why),
                fuel_used,
                trace,
            }),
        }
    }

    /// Refreshes the token registry for a contract a deploy or upgrade just left, after what it
    /// did so far.
    fn register(&self, vm: &Arc<Vm>, context: CallContext, limits: Limits, token_functions: &[String], hooked: &mut Hooked) -> Result<(), Failed> {
        let (asked, trace) = self
            .refresh(vm, &context.contract, context.sender, spent(limits, hooked.fuel_used), token_functions)
            .map_err(|failed| failed.after(hooked.fuel_used, std::mem::take(&mut hooked.trace)))?;
        hooked.fuel_used += asked;
        hooked.trace.append(trace);

        Ok(())
    }

    /// Asks a contract's check_perms whether context.sender has a permission, on a throwaway
    /// copy of state. Returns the fuel the question burned and where it went when the answer is
    /// yes; a no burned it all the same.
    fn permits(
        &self,
        vm: &Arc<Vm>,
        record: &ContractRecord,
        context: CallContext,
        permission: &str,
        limits: Limits,
    ) -> Result<(u64, Trace), Failed> {
        if !record.functions.iter().any(|name| name == CHECK_PERMS) {
            return Err(RegistryError::NotPermitted.into());
        }

        let scratch = Arc::new(State::new(self.state.clone()));
        let args = [
            TxCallArgumentType::String(permission.to_string()),
            TxCallArgumentType::Address(context.sender),
        ];
        let mut outcome = vm.run(&scratch, record, context, CHECK_PERMS, &args, limits);
        // What it emitted was thrown away with the scratch state.
        outcome.trace.forget_events();

        let error = match outcome.result {
            Ok(answer) if answer == [TxCallArgumentType::Bool(true)] => return Ok((outcome.fuel_used, outcome.trace)),
            Err(VmError::Storage(err)) => RegistryError::Db(err),
            _ => RegistryError::NotPermitted,
        };

        Err(Failed {
            error,
            fuel_used: outcome.fuel_used,
            trace: outcome.trace,
        })
    }

    /// Runs a hook the contract may export, keeping what it does only if it succeeds.
    fn hook(
        &self,
        vm: &Arc<Vm>,
        record: &ContractRecord,
        context: CallContext,
        name: &str,
        limits: Limits,
    ) -> Result<Hooked, Failed> {
        if !record.functions.iter().any(|function| function == name) {
            return Ok(Hooked::default());
        }

        let outcome = vm.run(self.state, record, context, name, &[], limits);

        if let Err(err) = outcome.result {
            return Err(Failed {
                error: err.into(),
                fuel_used: outcome.fuel_used,
                trace: outcome.trace,
            });
        }

        Ok(Hooked {
            fuel_used: outcome.fuel_used,
            events: outcome.events,
            trace: outcome.trace,
        })
    }

    #[document(
        parent = "Contracts",
        summary = "The stored record of a contract.",
        returns = "The record, or None when no contract lives at the address.",
    )]
    pub fn get(&self, address: &Address) -> Result<Option<ContractRecord>, DbError> {
        self.state.get_value(Column::Contracts, &address_key(address))
    }

    #[document(
        parent = "Contracts",
        summary = "Calls a deployed contract, whichever engine runs it. Its storage writes, and those of contracts it calls, land in this registry's state only when the call succeeds.",
        params = {
            vm = "The chain's engines.",
            context = "Who is calling, which contract, and how much they sent.",
            func = "The XXH3-64 of the function's name.",
            args = "The arguments.",
            limits = "What the call may use.",
        },
        returns = "What the call did, or NoSuchContract when context.contract holds none.",
    )]
    pub fn call(
        &self,
        vm: &Arc<Vm>,
        context: &CallContext,
        func: &Hash,
        args: &[TxCallArgumentType],
        limits: Limits,
    ) -> Result<Outcome, RegistryError> {
        let record = self
            .get(&context.contract)?
            .ok_or(RegistryError::NoSuchContract(context.contract))?;

        Ok(vm.call(self.state, &record, *context, func, args, limits))
    }
}

#[document(
    summary = "What the hooks a deploy or upgrade ran did: the fuel they burned, and what they emitted.",
    category = "Registries",
    see_also = "Contracts::deploy, Contracts::upgrade",
)]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Hooked {
    #[document(summary = "Fuel burned by init, or by check_perms and migrate.")]
    pub fuel_used: u64,
    #[document(summary = "What init or migrate emitted, in order.")]
    pub events: Vec<Event>,
    #[document(summary = "Where init, or check_perms and then migrate, went.")]
    pub trace: Trace,
}

#[document(
    summary = "Why a deploy or upgrade failed, with the fuel it burned before it did and where its hooks went: a failure still pays for the work it made every node do.",
    category = "Registries",
    see_also = "Contracts::deploy, Contracts::upgrade",
)]
#[derive(Clone, Debug, PartialEq)]
pub struct Failed {
    #[document(summary = "Why it failed.")]
    pub error: RegistryError,
    #[document(summary = "Fuel burned before it failed: by check_perms, loading the code, and init or migrate, as far as each got.")]
    pub fuel_used: u64,
    #[document(summary = "Where check_perms, and init or migrate, went before it failed.")]
    pub trace: Trace,
}

impl Failed {
    /// The same failure, after something else burned fuel and went somewhere first.
    fn after(mut self, fuel_used: u64, mut trace: Trace) -> Failed {
        self.fuel_used = self.fuel_used.saturating_add(fuel_used);
        trace.append(self.trace);
        self.trace = trace;
        self
    }
}

impl From<RegistryError> for Failed {
    fn from(error: RegistryError) -> Failed {
        Failed {
            error,
            fuel_used: 0,
            trace: Trace::default(),
        }
    }
}

impl From<DbError> for Failed {
    fn from(err: DbError) -> Failed {
        RegistryError::Db(err).into()
    }
}

/// What is left of limits once fuel_used is spent.
fn spent(limits: Limits, fuel_used: u64) -> Limits {
    Limits {
        fuel: limits.fuel.saturating_sub(fuel_used),
        ..limits
    }
}

/// Checks and loads code into the record that stores it, and says what loading burned.
fn load(vm: &Vm, deployer: Address, code: &Code, limits: Limits) -> Result<(ContractRecord, u64), Failed> {
    let engine = vm
        .engine(code.engine())
        .ok_or(RegistryError::Vm(VmError::EngineMissing(code.engine())))?;
    let loading = engine.load(code.bytes(), limits);
    let loaded = loading.result.map_err(|err| Failed {
        error: err.into(),
        fuel_used: loading.fuel_used,
        trace: Trace::default(),
    })?;
    let record = ContractRecord {
        deployer,
        engine: code.engine(),
        code: loaded.code,
        functions: loaded.functions,
        reentrant: loaded.reentrant,
        layout: loaded.layout,
    };

    Ok((record, loading.fuel_used))
}

#[document(
    summary = "Where a deploy puts its contract: the chain hash of a domain tag, the deployer's address and their nonce. The same deployer and nonce always give the same address on a chain, and no key controls it.",
    category = "Registries",
    params = {
        deployer = "Who deploys.",
        nonce = "The deploy's nonce.",
        hash = "The chain's hash: any secure, 256-bit one.",
    },
    see_also = "Contracts::deploy, Address::Contract",
)]
pub fn contract_address(deployer: &Address, nonce: u64, hash: HashType) -> Address {
    let mut hasher = Hasher::new(hash);
    hasher.update(ADDRESS_DOMAIN);
    hasher.update(&deployer.hashable_bytes_with_tag());
    hasher.update(&nonce.to_le_bytes());
    let bytes: [u8; 32] = hasher
        .finalize()
        .to_bytes()
        .try_into()
        .expect("genesis only allows 256-bit chain hashes");

    Address::Contract(bytes)
}

#[cfg(all(test, feature = "lvm"))]
mod tests {
    use super::*;
    use crate::db::{Database, MemoryDb};
    use crate::structs::u256::u256;
    use crate::vm::selector;

    const DEPLOYER: Address = Address::Ed25519([7; 32]);
    const LIMITS: Limits = Limits {
        fuel: 1_000_000,
        memory: 16 * 1024 * 1024,
    };

    const COUNTER: &str = r#"
local M = {}

function M.increment(by)
    local count = tonumber(storage.get("count") or "0") + by
    storage.set("count", tostring(count))
    return count
end

function M.fail_after_write()
    storage.set("count", "999")
    error("nope")
end

return M
"#;

    fn state() -> Arc<State> {
        Arc::new(State::new(Arc::new(MemoryDb::default())))
    }

    fn lua(source: &str) -> Code {
        Code::Lua(source.to_string())
    }

    fn vm() -> Arc<Vm> {
        Arc::new(Vm::new(&[Engine::Lua], HashType::default_chain()).unwrap())
    }

    fn context(contract: Address) -> CallContext {
        CallContext {
            sender: DEPLOYER,
            contract,
            value: u256::ZERO,
        }
    }

    fn increment(contracts: &Contracts, contract: Address, by: i64) -> Outcome {
        let func = Hash::Xxh3_64(selector("increment"));

        contracts
            .call(&vm(), &context(contract), &func, &[TxCallArgumentType::I64(by)], LIMITS)
            .unwrap()
    }

    fn count(outcome: Outcome) -> f64 {
        match outcome.result.unwrap()[..] {
            [TxCallArgumentType::Number(count)] => count,
            ref other => panic!("{other:?}"),
        }
    }

    #[test]
    fn storage_persists_between_calls() {
        let state = state();
        let contracts = Contracts::new(&state);
        let counter = contracts.deploy(&vm(), &DEPLOYER, 0, &lua(COUNTER), LIMITS, &[]).unwrap().0;

        assert_eq!(count(increment(&contracts, counter, 2)), 2.0);
        assert_eq!(count(increment(&contracts, counter, 3)), 5.0);
    }

    #[test]
    fn failed_calls_leave_storage_alone() {
        let state = state();
        let contracts = Contracts::new(&state);
        let counter = contracts.deploy(&vm(), &DEPLOYER, 0, &lua(COUNTER), LIMITS, &[]).unwrap().0;
        increment(&contracts, counter, 1);
        let func = Hash::Xxh3_64(selector("fail_after_write"));
        let outcome = contracts.call(&vm(), &context(counter), &func, &[], LIMITS).unwrap();

        assert!(matches!(outcome.result, Err(VmError::Runtime(_))));
        assert_eq!(count(increment(&contracts, counter, 1)), 2.0);
    }

    #[test]
    fn each_contract_keeps_its_own_storage() {
        let state = state();
        let contracts = Contracts::new(&state);
        let first = contracts.deploy(&vm(), &DEPLOYER, 0, &lua(COUNTER), LIMITS, &[]).unwrap().0;
        let second = contracts.deploy(&vm(), &DEPLOYER, 1, &lua(COUNTER), LIMITS, &[]).unwrap().0;
        increment(&contracts, first, 10);

        assert_ne!(first, second);
        assert_eq!(count(increment(&contracts, second, 1)), 1.0);
    }

    #[test]
    fn contracts_survive_a_commit() {
        let db = Arc::new(MemoryDb::default());
        let state = Arc::new(State::new(db.clone()));
        let counter = Contracts::new(&state).deploy(&vm(), &DEPLOYER, 0, &lua(COUNTER), LIMITS, &[]).unwrap().0;
        increment(&Contracts::new(&state), counter, 4);
        db.commit(state.take_changes()).unwrap();
        let fresh = Arc::new(State::new(db));
        let record = Contracts::new(&fresh).get(&counter).unwrap().unwrap();

        assert_eq!(record.deployer, DEPLOYER);
        assert_eq!(record.functions, ["fail_after_write", "increment"]);
        assert_eq!(count(increment(&Contracts::new(&fresh), counter, 1)), 5.0);
    }

    #[test]
    fn addresses_are_derived_and_unique() {
        let state = state();
        let contracts = Contracts::new(&state);
        let address = contracts.deploy(&vm(), &DEPLOYER, 0, &lua(COUNTER), LIMITS, &[]).unwrap().0;

        assert_eq!(address, contract_address(&DEPLOYER, 0, HashType::default_chain()));
        assert!(matches!(address, Address::Contract(_)));
        assert_ne!(address, contract_address(&DEPLOYER, 1, HashType::default_chain()));
        assert_ne!(address, contract_address(&Address::Ed25519([8; 32]), 0, HashType::default_chain()));
        assert_eq!(
            contracts.deploy(&vm(), &DEPLOYER, 0, &lua(COUNTER), LIMITS, &[]).map(|(address, _)| address).map_err(|failed| failed.error),
            Err(RegistryError::AlreadyDeployed(address))
        );
    }

    #[test]
    fn bad_deploys_and_missing_contracts_fail() {
        let state = state();
        let contracts = Contracts::new(&state);
        let nowhere = contract_address(&DEPLOYER, 99, HashType::default_chain());
        let func = Hash::Xxh3_64(selector("increment"));

        assert!(matches!(
            contracts.deploy(&vm(), &DEPLOYER, 0, &lua("local = 1"), LIMITS, &[]).map_err(|failed| failed.error),
            Err(RegistryError::Vm(VmError::Compile(_)))
        ));
        assert_eq!(
            contracts.call(&vm(), &context(nowhere), &func, &[], LIMITS).unwrap_err(),
            RegistryError::NoSuchContract(nowhere)
        );
    }
}
