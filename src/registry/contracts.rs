use {
    super::RegistryError,
    crate::crypto::hashable::Hashable,
    crate::crypto::hasher::Hasher,
    crate::db::{Column, DbError, State, address_key},
    crate::structs::address::Address,
    crate::structs::code::{Code, Engine},
    crate::structs::event::Event,
    crate::structs::hash::{Hash, HashType},
    crate::structs::tx::TxCallArgumentType,
    crate::structs::u256::u256,
    crate::vm::{CHECK_PERMS, CallContext, INIT, Limits, MIGRATE, Outcome, Vm, VmError},
    r#macro::document,
    std::sync::Arc,
    wincode::{SchemaRead, SchemaWrite},
};

/// Separates contract addresses from every other BLAKE3 hash the chain takes.
const ADDRESS_DOMAIN: &[u8] = b"chainv4 contract address";

#[document(
    summary = "A deployed contract as stored: who deployed it, which engine runs it, what the engine made of its code, and the names it exports.",
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
        summary = "Has the contract's engine check and load its code, stores it at an address derived from the deployer and their nonce, and runs its init, if it exports one, with the deployer as sender.",
        params = {
            vm = "The chain's engines.",
            deployer = "The address deploying it.",
            nonce = "The deployer's nonce for this transaction, which makes the address unique.",
            code = "The contract: Luau source, which nodes compile themselves, or a WebAssembly module.",
            limits = "What loading and init may use.",
        },
        returns = "The new contract's address, and what init burned and emitted.",
        panics = "Vm when the code does not load, init fails or the chain does not run its engine, AlreadyDeployed when the address is taken.",
        see_also = "contract_address, ScriptEngine::load, INIT",
    )]
    pub fn deploy(
        &self,
        vm: &Arc<Vm>,
        deployer: &Address,
        nonce: u64,
        code: &Code,
        limits: Limits,
    ) -> Result<(Address, Hooked), RegistryError> {
        let address = contract_address(deployer, nonce, vm.hash());

        if self.get(&address)?.is_some() {
            return Err(RegistryError::AlreadyDeployed(address));
        }

        let record = load(vm, *deployer, code, limits)?;
        self.state.put_value(Column::Contracts, address_key(&address), &record);
        let context = CallContext {
            sender: *deployer,
            contract: address,
            value: u256::ZERO,
        };
        let hooked = self.hook(vm, &record, context, INIT, limits)?;

        Ok((address, hooked))
    }

    #[document(
        parent = "Contracts",
        summary = "Replaces a contract's code, keeping its address, storage and balance. The contract decides who may: its check_perms is asked with \"upgrade\" and the sender, and must return true. Then the new code's migrate runs, if it exports one, with the sender as sender. If anything fails, nothing changes.",
        params = {
            vm = "The chain's engines.",
            sender = "Who asks for the upgrade.",
            contract = "The contract to upgrade.",
            code = "The new code, for either engine.",
            limits = "What check_perms, loading and migrate may use together.",
        },
        returns = "What check_perms and migrate burned, and what migrate emitted.",
        panics = "NotPermitted when the contract has no check_perms or it does not return true, NoSuchContract when there is nothing to upgrade, Vm when the code does not load or migrate fails.",
        see_also = "CHECK_PERMS, MIGRATE, Tx::Upgrade",
    )]
    pub fn upgrade(
        &self,
        vm: &Arc<Vm>,
        sender: &Address,
        contract: &Address,
        code: &Code,
        limits: Limits,
    ) -> Result<Hooked, RegistryError> {
        let current = self.get(contract)?.ok_or(RegistryError::NoSuchContract(*contract))?;
        let context = CallContext {
            sender: *sender,
            contract: *contract,
            value: u256::ZERO,
        };
        let asked = self.permits(vm, &current, context, "upgrade", limits)?;
        let record = load(vm, current.deployer, code, limits)?;
        self.state.put_value(Column::Contracts, address_key(contract), &record);
        let remaining = Limits {
            fuel: limits.fuel.saturating_sub(asked),
            ..limits
        };
        let mut hooked = self.hook(vm, &record, context, MIGRATE, remaining)?;
        hooked.fuel_used += asked;

        Ok(hooked)
    }

    /// Asks a contract's check_perms whether context.sender has a permission, on a throwaway
    /// copy of state. Returns the fuel the question burned when the answer is yes.
    fn permits(
        &self,
        vm: &Arc<Vm>,
        record: &ContractRecord,
        context: CallContext,
        permission: &str,
        limits: Limits,
    ) -> Result<u64, RegistryError> {
        if !record.functions.iter().any(|name| name == CHECK_PERMS) {
            return Err(RegistryError::NotPermitted);
        }

        let scratch = Arc::new(State::new(self.state.clone()));
        let args = [
            TxCallArgumentType::String(permission.to_string()),
            TxCallArgumentType::Address(context.sender),
        ];
        let outcome = vm.run(&scratch, record, context, CHECK_PERMS, &args, limits);

        match outcome.result {
            Ok(answer) if answer == [TxCallArgumentType::Bool(true)] => Ok(outcome.fuel_used),
            Err(VmError::Storage(err)) => Err(RegistryError::Db(err)),
            _ => Err(RegistryError::NotPermitted),
        }
    }

    /// Runs a hook the contract may export, keeping what it does only if it succeeds.
    fn hook(
        &self,
        vm: &Arc<Vm>,
        record: &ContractRecord,
        context: CallContext,
        name: &str,
        limits: Limits,
    ) -> Result<Hooked, RegistryError> {
        if !record.functions.iter().any(|function| function == name) {
            return Ok(Hooked::default());
        }

        let outcome = vm.run(self.state, record, context, name, &[], limits);
        outcome.result?;

        Ok(Hooked {
            fuel_used: outcome.fuel_used,
            events: outcome.events,
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
}

/// Checks and loads code into the record that stores it.
fn load(vm: &Vm, deployer: Address, code: &Code, limits: Limits) -> Result<ContractRecord, RegistryError> {
    let engine = vm.engine(code.engine()).ok_or(VmError::EngineMissing(code.engine()))?;
    let loaded = engine.load(code.bytes(), limits)?;

    Ok(ContractRecord {
        deployer,
        engine: code.engine(),
        code: loaded.code,
        functions: loaded.functions,
        reentrant: loaded.reentrant,
    })
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
        let counter = contracts.deploy(&vm(), &DEPLOYER, 0, &lua(COUNTER), LIMITS).unwrap().0;

        assert_eq!(count(increment(&contracts, counter, 2)), 2.0);
        assert_eq!(count(increment(&contracts, counter, 3)), 5.0);
    }

    #[test]
    fn failed_calls_leave_storage_alone() {
        let state = state();
        let contracts = Contracts::new(&state);
        let counter = contracts.deploy(&vm(), &DEPLOYER, 0, &lua(COUNTER), LIMITS).unwrap().0;
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
        let first = contracts.deploy(&vm(), &DEPLOYER, 0, &lua(COUNTER), LIMITS).unwrap().0;
        let second = contracts.deploy(&vm(), &DEPLOYER, 1, &lua(COUNTER), LIMITS).unwrap().0;
        increment(&contracts, first, 10);

        assert_ne!(first, second);
        assert_eq!(count(increment(&contracts, second, 1)), 1.0);
    }

    #[test]
    fn contracts_survive_a_commit() {
        let db = Arc::new(MemoryDb::default());
        let state = Arc::new(State::new(db.clone()));
        let counter = Contracts::new(&state).deploy(&vm(), &DEPLOYER, 0, &lua(COUNTER), LIMITS).unwrap().0;
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
        let address = contracts.deploy(&vm(), &DEPLOYER, 0, &lua(COUNTER), LIMITS).unwrap().0;

        assert_eq!(address, contract_address(&DEPLOYER, 0, HashType::default_chain()));
        assert!(matches!(address, Address::Contract(_)));
        assert_ne!(address, contract_address(&DEPLOYER, 1, HashType::default_chain()));
        assert_ne!(address, contract_address(&Address::Ed25519([8; 32]), 0, HashType::default_chain()));
        assert_eq!(
            contracts.deploy(&vm(), &DEPLOYER, 0, &lua(COUNTER), LIMITS).map(|(address, _)| address),
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
            contracts.deploy(&vm(), &DEPLOYER, 0, &lua("local = 1"), LIMITS),
            Err(RegistryError::Vm(VmError::Compile(_)))
        ));
        assert_eq!(
            contracts.call(&vm(), &context(nowhere), &func, &[], LIMITS).unwrap_err(),
            RegistryError::NoSuchContract(nowhere)
        );
    }
}
