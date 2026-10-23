use {
    super::RegistryError,
    crate::crypto::hashable::Hashable,
    crate::crypto::hasher::Hasher,
    crate::db::{Column, DbError, State, address_key},
    crate::structs::address::Address,
    crate::structs::hash::{Hash, HashType},
    crate::structs::tx::TxCallArgumentType,
    crate::vm::lvm::{CallContext, Contract, Limits, Outcome},
    r#macro::document,
    std::sync::Arc,
    wincode::{SchemaRead, SchemaWrite},
};

/// Separates contract addresses from every other BLAKE3 hash the chain takes.
const ADDRESS_DOMAIN: &[u8] = b"chainv4 contract address";

#[document(
    summary = "A deployed contract as stored: who deployed it, the bytecode this node compiled from its source, and the names it exports.",
    category = "Registries",
    see_also = "Contracts",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct ContractRecord {
    #[document(summary = "The address that deployed it.")]
    pub deployer: Address,
    #[document(summary = "The compiled bytecode.")]
    pub bytecode: Vec<u8>,
    #[document(summary = "The exported function names, sorted.")]
    pub functions: Vec<String>,
}

impl Hashable for ContractRecord {}

#[document(
    summary = "Deployed contracts by address: deploys them and calls them against the state it was given.",
    category = "Registries",
    see_also = "ContractRecord, Contract, Accounts",
    note = "Moving the value a call sends is not done here: debit the sender and credit the contract with Accounts before calling, in the same transaction layer, so a failed call takes both back.",
    example = "let contracts = Contracts::new(&tx);\nlet address = contracts.deploy(&deployer, nonce, source, limits)?;\nlet outcome = contracts.call(&context, &func, &args, limits)?;",
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
        summary = "Compiles a contract, runs its top level once to check it, and stores it at an address derived from the deployer and their nonce.",
        params = {
            deployer = "The address deploying it.",
            nonce = "The deployer's nonce for this transaction, which makes the address unique.",
            source = "The Luau source. Nodes compile it themselves; bytecode is never accepted.",
            limits = "What running the top level may use.",
        },
        returns = "The new contract's address.",
        panics = "Lvm when the source does not compile or load, AlreadyDeployed when the address is taken.",
        see_also = "contract_address",
    )]
    pub fn deploy(
        &self,
        deployer: &Address,
        nonce: u64,
        source: &str,
        limits: Limits,
    ) -> Result<Address, RegistryError> {
        let address = contract_address(deployer, nonce);

        if self.get(&address)?.is_some() {
            return Err(RegistryError::AlreadyDeployed(address));
        }

        let contract = Contract::load(source, limits)?;
        let record = ContractRecord {
            deployer: *deployer,
            bytecode: contract.bytecode().to_vec(),
            functions: contract.function_names(),
        };
        self.state.put_value(Column::Contracts, address_key(&address), &record);

        Ok(address)
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
        summary = "Calls a deployed contract. Its storage writes land in this registry's state only when the call succeeds.",
        params = {
            context = "Who is calling, which contract, and how much they sent.",
            func = "The XXH3-64 of the function's name.",
            args = "The arguments.",
            limits = "What the call may use.",
        },
        returns = "What the call did, or NoSuchContract when context.contract holds none.",
    )]
    pub fn call(
        &self,
        context: &CallContext,
        func: &Hash,
        args: &[TxCallArgumentType],
        limits: Limits,
    ) -> Result<Outcome, RegistryError> {
        let record = self
            .get(&context.contract)?
            .ok_or(RegistryError::NoSuchContract(context.contract))?;
        let contract = Contract::from_compiled(record.bytecode, record.functions);

        Ok(contract.call(context, Some(self.state), func, args, limits))
    }
}

#[document(
    summary = "Where a deploy puts its contract: BLAKE3 of a domain tag, the deployer's address and their nonce. The same deployer and nonce always give the same address, and no key controls it.",
    category = "Registries",
    see_also = "Contracts::deploy, Address::Contract",
)]
pub fn contract_address(deployer: &Address, nonce: u64) -> Address {
    let mut hasher = Hasher::new(HashType::Blake3_256);
    hasher.update(ADDRESS_DOMAIN);
    hasher.update(&deployer.hashable_bytes_with_tag());
    hasher.update(&nonce.to_le_bytes());

    match hasher.finalize() {
        Hash::Blake3_256(bytes) => Address::Contract(bytes),
        #[allow(unreachable_patterns)]
        _ => unreachable!("BLAKE3 always gives a BLAKE3 hash"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Database, MemoryDb};
    use crate::structs::u256::u256;
    use crate::vm::lvm::{LvmError, selector};

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
            .call(&context(contract), &func, &[TxCallArgumentType::I64(by)], LIMITS)
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
        let counter = contracts.deploy(&DEPLOYER, 0, COUNTER, LIMITS).unwrap();

        assert_eq!(count(increment(&contracts, counter, 2)), 2.0);
        assert_eq!(count(increment(&contracts, counter, 3)), 5.0);
    }

    #[test]
    fn failed_calls_leave_storage_alone() {
        let state = state();
        let contracts = Contracts::new(&state);
        let counter = contracts.deploy(&DEPLOYER, 0, COUNTER, LIMITS).unwrap();
        increment(&contracts, counter, 1);
        let func = Hash::Xxh3_64(selector("fail_after_write"));
        let outcome = contracts.call(&context(counter), &func, &[], LIMITS).unwrap();

        assert!(matches!(outcome.result, Err(LvmError::Runtime(_))));
        assert_eq!(count(increment(&contracts, counter, 1)), 2.0);
    }

    #[test]
    fn each_contract_keeps_its_own_storage() {
        let state = state();
        let contracts = Contracts::new(&state);
        let first = contracts.deploy(&DEPLOYER, 0, COUNTER, LIMITS).unwrap();
        let second = contracts.deploy(&DEPLOYER, 1, COUNTER, LIMITS).unwrap();
        increment(&contracts, first, 10);

        assert_ne!(first, second);
        assert_eq!(count(increment(&contracts, second, 1)), 1.0);
    }

    #[test]
    fn contracts_survive_a_commit() {
        let db = Arc::new(MemoryDb::default());
        let state = Arc::new(State::new(db.clone()));
        let counter = Contracts::new(&state).deploy(&DEPLOYER, 0, COUNTER, LIMITS).unwrap();
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
        let address = contracts.deploy(&DEPLOYER, 0, COUNTER, LIMITS).unwrap();

        assert_eq!(address, contract_address(&DEPLOYER, 0));
        assert!(matches!(address, Address::Contract(_)));
        assert_ne!(address, contract_address(&DEPLOYER, 1));
        assert_ne!(address, contract_address(&Address::Ed25519([8; 32]), 0));
        assert_eq!(
            contracts.deploy(&DEPLOYER, 0, COUNTER, LIMITS),
            Err(RegistryError::AlreadyDeployed(address))
        );
    }

    #[test]
    fn bad_deploys_and_missing_contracts_fail() {
        let state = state();
        let contracts = Contracts::new(&state);
        let nowhere = contract_address(&DEPLOYER, 99);
        let func = Hash::Xxh3_64(selector("increment"));

        assert!(matches!(
            contracts.deploy(&DEPLOYER, 0, "local = 1", LIMITS),
            Err(RegistryError::Lvm(LvmError::Compile(_)))
        ));
        assert_eq!(
            contracts.call(&context(nowhere), &func, &[], LIMITS).unwrap_err(),
            RegistryError::NoSuchContract(nowhere)
        );
    }
}
