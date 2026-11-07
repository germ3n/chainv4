use {
    super::RegistryError,
    crate::crypto::hashable::Hashable,
    crate::db::{Column, Committed, DbError, State, address_key},
    crate::registry::Standards,
    crate::registry::collections::{COLLECTION_METADATA_FUNCTIONS, Collection, Collections},
    crate::registry::tokens::{METADATA_FUNCTIONS, Token, Tokens},
    crate::structs::address::Address,
    crate::structs::code::{Code, Engine},
    crate::structs::event::Event,
    crate::structs::hash::Hash,
    crate::structs::trace::Trace,
    crate::structs::tx::TxCallArgumentType,
    crate::structs::u256::u256,
    crate::vm::{BlockAt, CHECK_PERMS, CallContext, INIT, Layout, Limits, MIGRATE, Outcome, REMOVE, UPGRADE, Vm, VmError},
    r#macro::document,
    std::sync::Arc,
    wincode::{SchemaRead, SchemaWrite},
};

/// Separates contract addresses from every other hash the chain takes.

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
    summary = "What the chain keeps of a removed contract, at removed_key in Column::Contracts, so light clients prove it and nothing is deployed or cloned at its address again: when, by whom, and where its balance went.",
    category = "Registries",
    see_also = "Tx::Remove, Contracts::remove, removed_key",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct Removal {
    #[document(summary = "The block that removed it.")]
    pub height: u64,
    #[document(summary = "Who removed it: the sender check_perms allowed.")]
    pub by: Address,
    #[document(summary = "Who got its balance.")]
    pub beneficiary: Address,
    #[document(summary = "The balance it had, in the coin's smallest unit.")]
    #[wincode(with = "crate::structs::u256::U256Le")]
    pub balance: u256,
    #[document(summary = "The engine that ran it.")]
    pub engine: Engine,
    #[document(summary = "How many entries of its storage were deleted, plain and namespaced.")]
    pub entries: u64,
}

impl Hashable for Removal {}

#[document(
    summary = "Where a removed contract's Removal is kept in Column::Contracts: 0xff and then the address's own key, which no address's key starts with.",
    category = "Registries",
    see_also = "Removal, Contracts::removed",
)]
pub fn removed_key(address: &Address) -> Vec<u8> {
    let mut key = vec![0xff];
    key.extend_from_slice(&address_key(address));

    key
}

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
        summary = "Has the contract's engine check and load its code, stores it at an address derived from the deployer and their nonce, and runs its init, if it exports one, with the deployer as sender. A contract that exports every one of the standards' token functions then goes in the token registry, with what its name(), symbol() and decimals() answer, for those it exports; one that exports a collection standard's goes in the collection registry, with its name() and symbol().",
        params = {
            vm = "The chain's engines.",
            deployer = "The address deploying it.",
            nonce = "The deployer's nonce for this transaction, which makes the address unique.",
            code = "The contract: Luau source, which nodes compile themselves, or a WebAssembly module.",
            limits = "What loading and init may use.",
            standards = "What makes a contract a token or a collection, as genesis and governance say.",
        },
        returns = "The new contract's address, and what loading the code, init and the registries' questions burned, and what init emitted. A failure carries what was burned before it.",
        panics = "Vm when the code does not load, init fails or the chain does not run its engine, AlreadyDeployed when the address is taken, BadToken when it is a token but its name(), symbol() or decimals() answers badly, BadCollection when it is a collection but its name() or symbol() does.",
        see_also = "contract_address, ScriptEngine::load, INIT, Tokens, Collections",
    )]
    #[allow(clippy::too_many_arguments)]
    pub fn deploy(
        &self,
        vm: &Arc<Vm>,
        deployer: &Address,
        nonce: u64,
        code: &Code,
        limits: Limits,
        standards: &Standards,
        block: BlockAt,
    ) -> Result<(Address, Hooked), Failed> {
        let address = contract_address(deployer, nonce, vm.hash());

        if self.get(&address)?.is_some() {
            return Err(RegistryError::AlreadyDeployed(address).into());
        }

        if self.removed(&address)?.is_some() {
            return Err(RegistryError::Removed(address).into());
        }

        let (record, loaded) = load(vm, *deployer, code, limits, &block)?;
        self.state.put_value(Column::Contracts, address_key(&address), &record);
        let context = CallContext {
            sender: *deployer,
            contract: address,
            value: u256::ZERO,
            block,
        };
        let mut hooked = self
            .hook(vm, &record, context, INIT, spent(limits, loaded))
            .map_err(|failed| failed.after(loaded, Trace::default()))?;
        hooked.fuel_used += loaded;
        self.register(vm, context, limits, standards, &mut hooked)?;

        Ok((address, hooked))
    }

    #[document(
        parent = "Contracts",
        summary = "Replaces a contract's code, keeping its address, storage and balance. The contract decides who may: its check_perms is asked with \"upgrade\" and the sender, and must return true. The new code must keep the old code's storage layout. Then the new code's migrate runs, if it exports one, with the sender as sender, and the token and collection registries are brought in line with what the new code exports. If anything fails, nothing changes.",
        params = {
            vm = "The chain's engines.",
            sender = "Who asks for the upgrade.",
            contract = "The contract to upgrade.",
            code = "The new code, for either engine.",
            limits = "What check_perms, loading and migrate may use together.",
            standards = "What makes a contract a token or a collection, as genesis and governance say.",
        },
        returns = "What check_perms, loading the new code, migrate and the registries' questions burned, and what migrate emitted. A failure carries what was burned before it.",
        panics = "NotPermitted when the contract has no check_perms or it does not return true, NoSuchContract when there is nothing to upgrade, Vm when the code does not load or migrate fails, Layout when the new code does not keep the old layout, BadToken when the new code is a token but its name(), symbol() or decimals() answers badly, BadCollection when it is a collection but its name() or symbol() does.",
        see_also = "CHECK_PERMS, MIGRATE, Tx::Upgrade, Tokens, Collections, Layout::keeps",
    )]
    #[allow(clippy::too_many_arguments)]
    pub fn upgrade(
        &self,
        vm: &Arc<Vm>,
        sender: &Address,
        contract: &Address,
        code: &Code,
        limits: Limits,
        standards: &Standards,
        block: BlockAt,
    ) -> Result<Hooked, Failed> {
        let current = self.record(contract)?;
        let context = CallContext {
            sender: *sender,
            contract: *contract,
            value: u256::ZERO,
            block,
        };
        // What check_perms is shown of the new code: the chain's hash of its bytes, as the
        // contract's own hash function gives it.
        let hash = crate::crypto::hasher::Hasher::digest(vm.hash(), code.bytes()).to_bytes();
        let (asked, mut trace) = self.permits(vm, &current, context, UPGRADE, TxCallArgumentType::Bytes(hash), limits)?;
        let (record, loaded) = load(vm, current.deployer, code, spent(limits, asked), &block)
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
        self.register(vm, context, limits, standards, &mut hooked)?;

        Ok(hooked)
    }

    #[document(
        parent = "Contracts",
        summary = "Removes a Lua or WebAssembly contract for good, as Tx::Remove asks. Its check_perms is asked with \"remove\", the sender and the beneficiary, on a throwaway copy of state, and must return true. Then every entry of its storage, plain and namespaced, is deleted, each for the fuel a storage write costs; its balance goes to the beneficiary; its code is deleted; it leaves the token and collection registries; and a Removal is kept at removed_key, so the address is never deployed or cloned to again. If anything fails, nothing changes.",
        params = {
            vm = "The chain's engines.",
            sender = "Who asks for the removal.",
            contract = "The contract to remove.",
            beneficiary = "Who gets its balance: credited, never called.",
            limits = "What check_perms and the deleting may use together.",
        },
        returns = "What it did: the fuel check_perms and the deleting burned, where check_perms went, and the balance paid to the beneficiary.",
        panics = "NotPermitted when the contract has no check_perms or it does not return true, NoSuchContract when no contract lives there, Removed when it was removed already, Vm(OutOfFuel) when its storage is more than the limit deletes: clear some first, through its own functions.",
        see_also = "Tx::Remove, Removal, removed_key, CHECK_PERMS",
    )]
    pub fn remove(&self, vm: &Arc<Vm>, sender: &Address, contract: &Address, beneficiary: &Address, limits: Limits, block: BlockAt) -> Result<Removed, Failed> {
        let record = self.record(contract)?;
        let context = CallContext {
            sender: *sender,
            contract: *contract,
            value: u256::ZERO,
            block,
        };
        let (asked, trace) = self.permits(vm, &record, context, REMOVE, TxCallArgumentType::Address(*beneficiary), limits)?;
        let per_entry = block.vm.costs.storage_write.max(1);
        let mut fuel_used = asked;
        let mut doomed = Vec::new();
        let prefix = address_key(contract);

        // Every entry it keeps, plain and in namespaces, its own space too: counted before any
        // goes, so a contract with more than the fuel deletes is left whole.
        for column in [Column::Storage, Column::Namespaces] {
            let mut short = false;
            self.state.scan(column, &prefix, &[], &mut |key, _| {
                if fuel_used.saturating_add(per_entry) > limits.fuel {
                    short = true;

                    return false;
                }

                fuel_used += per_entry;
                doomed.push((column, key.to_vec()));

                true
            })?;

            if short {
                return Err(Failed {
                    error: RegistryError::Vm(VmError::OutOfFuel),
                    fuel_used: limits.fuel,
                    trace,
                });
            }
        }

        let entries = doomed.len() as u64;

        for (column, key) in doomed {
            self.state.delete(column, key);
        }

        let accounts = crate::registry::accounts::Accounts::new(self.state);
        let balance = accounts.get(contract)?.balance;

        if balance != u256::ZERO {
            accounts.transfer(contract, beneficiary, balance).map_err(Failed::from)?;
        }

        self.state.delete(Column::Contracts, address_key(contract));
        Tokens::new(self.state).unregister(contract)?;
        Collections::new(self.state).unregister(contract)?;
        let removal = Removal {
            height: block.height,
            by: *sender,
            beneficiary: *beneficiary,
            balance,
            engine: record.engine,
            entries,
        };
        self.state.put_value(Column::Contracts, removed_key(contract), &removal);

        Ok(Removed { fuel_used, trace, balance })
    }

    #[document(
        parent = "Contracts",
        summary = "What the chain keeps of a removed contract.",
        returns = "Its Removal, or None when nothing was removed at the address.",
    )]
    pub fn removed(&self, address: &Address) -> Result<Option<Removal>, DbError> {
        self.state.get_value(Column::Contracts, &removed_key(address))
    }

    /// A contract's record, or why there is none: never one, or one removed.
    fn record(&self, address: &Address) -> Result<ContractRecord, RegistryError> {
        match self.get(address)? {
            Some(record) => Ok(record),
            None => Err(self.missing(address)),
        }
    }

    #[document(
        parent = "Contracts",
        summary = "Why no contract lives at an address, as a call to it fails: removed, or never there.",
        returns = "RegistryError::Removed for a removed contract, else NoSuchContract; Db when the read failed.",
    )]
    pub fn missing(&self, address: &Address) -> RegistryError {
        match self.removed(address) {
            Ok(Some(_)) => RegistryError::Removed(*address),
            Ok(None) => RegistryError::NoSuchContract(*address),
            Err(err) => RegistryError::Db(err),
        }
    }

    #[document(
        parent = "Contracts",
        summary = "Brings a contract's registry entries in line with it. One that exports every token function is a token, with what those of name(), symbol() and decimals() it exports answer; one that exports a collection standard's is a collection, with its name() and symbol(); one that does not is taken off. Each question is asked once, for both registries, on a throwaway copy of state, so whatever they write or emit is gone. Deploys and upgrades do this themselves, and a transaction does it for the contracts that asked with tokens.refresh.",
        params = {
            vm = "The chain's engines.",
            contract = "The contract.",
            sender = "Who the questions come from: the transaction's sender.",
            limits = "What the questions may use together.",
            standards = "What makes a contract a token or a collection; no token functions and no collections keep no registry, and do nothing.",
        },
        returns = "What the questions burned and where they went. A failure carries those too.",
        panics = "BadToken or BadCollection when an answer does not fit, Vm(OutOfFuel) when the questions run out, NoSuchContract when no contract lives there.",
        see_also = "Tokens, Collections, Host::refresh_token",
    )]
    pub fn refresh(
        &self,
        vm: &Arc<Vm>,
        contract: &Address,
        sender: Address,
        limits: Limits,
        standards: &Standards,
        block: BlockAt,
    ) -> Result<(u64, Trace), Failed> {
        let (tokens, collections) = (Tokens::new(self.state), Collections::new(self.state));

        if standards.tokens.is_empty() && standards.collections.is_none() {
            return Ok((0, Trace::default()));
        }

        let record = self.record(contract)?;
        let exports = |function: &str| record.functions.iter().any(|name| name == function);
        let token = !standards.tokens.is_empty() && standards.tokens.iter().all(|function| exports(function));
        let kind = standards.collections.as_ref().and_then(|functions| functions.kind(exports));

        if !token {
            tokens.unregister(contract)?;
        }

        if kind.is_none() {
            collections.unregister(contract)?;
        }

        if !token && kind.is_none() {
            return Ok((0, Trace::default()));
        }

        let context = CallContext {
            sender,
            contract: *contract,
            value: u256::ZERO,
            block,
        };
        let scratch = Arc::new(State::new(self.state.clone()));
        let (mut fuel_used, mut trace) = (0, Trace::default());
        let mut answers: Vec<(&str, Vec<TxCallArgumentType>)> = Vec::new();
        let wanted = |function: &str| {
            (token && METADATA_FUNCTIONS.contains(&function)) || (kind.is_some() && COLLECTION_METADATA_FUNCTIONS.contains(&function))
        };
        let bad = |why: String| if token { RegistryError::BadToken(why) } else { RegistryError::BadCollection(why) };

        // name and symbol first, as tokens have always asked them, then decimals.
        for function in METADATA_FUNCTIONS {
            if !wanted(function) || !exports(function) {
                continue;
            }

            let mut outcome = vm.run(&scratch, &record, context, function, &[], spent(limits, fuel_used));
            outcome.trace.forget_events();
            fuel_used += outcome.fuel_used;
            trace.append(outcome.trace);
            let error = match outcome.result {
                Ok(answer) => {
                    answers.push((function, answer));

                    continue;
                }
                Err(VmError::Storage(err)) => RegistryError::Db(err),
                Err(VmError::OutOfFuel) => RegistryError::Vm(VmError::OutOfFuel),
                Err(err) => bad(format!("{function}() failed: {err}")),
            };

            return Err(Failed { error, fuel_used, trace });
        }

        let answer = |function: &str| answers.iter().find(|(asked, _)| *asked == function).map(|(_, answer)| answer.as_slice());
        let fail = |error: RegistryError, trace: Trace| Err(Failed { error, fuel_used, trace });

        if token {
            match Token::from_answers(*contract, answer("name"), answer("symbol"), answer("decimals")) {
                Ok(found) => tokens.register(&found)?,
                Err(why) => return fail(RegistryError::BadToken(why), trace),
            }
        }

        if let Some(kind) = kind {
            match Collection::from_answers(*contract, kind, answer("name"), answer("symbol")) {
                Ok(found) => collections.register(&found)?,
                Err(why) => return fail(RegistryError::BadCollection(why), trace),
            }
        }

        Ok((fuel_used, trace))
    }

    /// Refreshes the registries for a contract a deploy or upgrade just left, after what it did
    /// so far.
    fn register(&self, vm: &Arc<Vm>, context: CallContext, limits: Limits, standards: &Standards, hooked: &mut Hooked) -> Result<(), Failed> {
        let (asked, trace) = self
            .refresh(vm, &context.contract, context.sender, spent(limits, hooked.fuel_used), standards, context.block)
            .map_err(|failed| failed.after(hooked.fuel_used, std::mem::take(&mut hooked.trace)))?;
        hooked.fuel_used += asked;
        hooked.trace.append(trace);

        Ok(())
    }

    /// Asks a contract's check_perms whether context.sender has a permission, with what it is for,
    /// on a throwaway copy of state. Returns the fuel the question burned and where it went when
    /// the answer is yes; a no burned it all the same.
    fn permits(
        &self,
        vm: &Arc<Vm>,
        record: &ContractRecord,
        context: CallContext,
        permission: &str,
        detail: TxCallArgumentType,
        limits: Limits,
    ) -> Result<(u64, Trace), Failed> {
        if !record.functions.iter().any(|name| name == CHECK_PERMS) {
            return Err(RegistryError::NotPermitted.into());
        }

        let scratch = Arc::new(State::new(self.state.clone()));
        let args = [
            TxCallArgumentType::String(permission.to_string()),
            TxCallArgumentType::Address(context.sender),
            detail,
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
            payments: outcome.payments,
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
        let record = self.record(&context.contract)?;

        Ok(vm.call(self.state, &record, *context, func, args, limits))
    }
}

#[document(
    summary = "What a removal did: the fuel check_perms and the deleting burned, where check_perms went, and the balance the beneficiary got.",
    category = "Registries",
    see_also = "Contracts::remove",
)]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Removed {
    #[document(summary = "Fuel burned by check_perms and by deleting each entry of storage.")]
    pub fuel_used: u64,
    #[document(summary = "Where check_perms went.")]
    pub trace: Trace,
    #[document(summary = "What went to the beneficiary.")]
    pub balance: u256,
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
    #[document(summary = "What init or migrate paid from contracts' balances, in order.")]
    pub payments: Vec<crate::structs::receipt::Payment>,
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
fn load(vm: &Vm, deployer: Address, code: &Code, limits: Limits, block: &BlockAt) -> Result<(ContractRecord, u64), Failed> {
    if !block.vm.engines.contains(code.engine()) {
        return Err(RegistryError::Vm(VmError::EngineOff(code.engine())).into());
    }

    let engine = vm
        .engine(code.engine())
        .ok_or(RegistryError::Vm(VmError::EngineMissing(code.engine())))?;
    let loading = engine.load_under(code.bytes(), limits, &block.vm);
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

pub use crate::structs::address::contract_address;

#[cfg(all(test, feature = "lvm"))]
mod tests {
    use super::*;
    use crate::db::{Database, MemoryDb};
    use crate::structs::hash::HashType;
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
            block: BlockAt::at(0),
        }
    }

    fn increment(contracts: &Contracts, contract: Address, by: i64) -> Outcome {
        let func = Hash::Xxh3_64(selector("increment"));

        contracts
            .call(&vm(), &context(contract), &func, &[TxCallArgumentType::I64(by)], LIMITS)
            .unwrap()
    }

    fn count(outcome: Outcome) -> i64 {
        match outcome.result.unwrap()[..] {
            [TxCallArgumentType::I64(count)] => count,
            ref other => panic!("{other:?}"),
        }
    }

    #[test]
    fn storage_persists_between_calls() {
        let state = state();
        let contracts = Contracts::new(&state);
        let counter = contracts.deploy(&vm(), &DEPLOYER, 0, &lua(COUNTER), LIMITS, &crate::registry::Standards::default(), BlockAt::at(0)).unwrap().0;

        assert_eq!(count(increment(&contracts, counter, 2)), 2);
        assert_eq!(count(increment(&contracts, counter, 3)), 5);
    }

    #[test]
    fn failed_calls_leave_storage_alone() {
        let state = state();
        let contracts = Contracts::new(&state);
        let counter = contracts.deploy(&vm(), &DEPLOYER, 0, &lua(COUNTER), LIMITS, &crate::registry::Standards::default(), BlockAt::at(0)).unwrap().0;
        increment(&contracts, counter, 1);
        let func = Hash::Xxh3_64(selector("fail_after_write"));
        let outcome = contracts.call(&vm(), &context(counter), &func, &[], LIMITS).unwrap();

        assert!(matches!(outcome.result, Err(VmError::Runtime(_))));
        assert_eq!(count(increment(&contracts, counter, 1)), 2);
    }

    #[test]
    fn each_contract_keeps_its_own_storage() {
        let state = state();
        let contracts = Contracts::new(&state);
        let first = contracts.deploy(&vm(), &DEPLOYER, 0, &lua(COUNTER), LIMITS, &crate::registry::Standards::default(), BlockAt::at(0)).unwrap().0;
        let second = contracts.deploy(&vm(), &DEPLOYER, 1, &lua(COUNTER), LIMITS, &crate::registry::Standards::default(), BlockAt::at(0)).unwrap().0;
        increment(&contracts, first, 10);

        assert_ne!(first, second);
        assert_eq!(count(increment(&contracts, second, 1)), 1);
    }

    #[test]
    fn contracts_survive_a_commit() {
        let db = Arc::new(MemoryDb::default());
        let state = Arc::new(State::new(db.clone()));
        let counter = Contracts::new(&state).deploy(&vm(), &DEPLOYER, 0, &lua(COUNTER), LIMITS, &crate::registry::Standards::default(), BlockAt::at(0)).unwrap().0;
        increment(&Contracts::new(&state), counter, 4);
        db.commit(state.take_changes()).unwrap();
        let fresh = Arc::new(State::new(db));
        let record = Contracts::new(&fresh).get(&counter).unwrap().unwrap();

        assert_eq!(record.deployer, DEPLOYER);
        assert_eq!(record.functions, ["fail_after_write", "increment"]);
        assert_eq!(count(increment(&Contracts::new(&fresh), counter, 1)), 5);
    }

    #[test]
    fn addresses_are_derived_and_unique() {
        let state = state();
        let contracts = Contracts::new(&state);
        let address = contracts.deploy(&vm(), &DEPLOYER, 0, &lua(COUNTER), LIMITS, &crate::registry::Standards::default(), BlockAt::at(0)).unwrap().0;

        assert_eq!(address, contract_address(&DEPLOYER, 0, HashType::default_chain()));
        assert!(matches!(address, Address::Contract(_)));
        assert_ne!(address, contract_address(&DEPLOYER, 1, HashType::default_chain()));
        assert_ne!(address, contract_address(&Address::Ed25519([8; 32]), 0, HashType::default_chain()));
        assert_eq!(
            contracts.deploy(&vm(), &DEPLOYER, 0, &lua(COUNTER), LIMITS, &crate::registry::Standards::default(), BlockAt::at(0)).map(|(address, _)| address).map_err(|failed| failed.error),
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
            contracts.deploy(&vm(), &DEPLOYER, 0, &lua("local = 1"), LIMITS, &crate::registry::Standards::default(), crate::vm::BlockAt::at(0)).map_err(|failed| failed.error),
            Err(RegistryError::Vm(VmError::Compile(_)))
        ));
        assert_eq!(
            contracts.call(&vm(), &context(nowhere), &func, &[], LIMITS).unwrap_err(),
            RegistryError::NoSuchContract(nowhere)
        );
    }
}
