use {
    super::{CallContext, FunctionId, Limits, Outcome, Vm, VmError, route},
    crate::db::{Column, DbError, State, storage_key},
    crate::registry::contracts::{ContractRecord, Contracts},
    crate::registry::{Accounts, RegistryError},
    crate::structs::address::Address,
    crate::structs::event::Event,
    crate::structs::tx::TxCallArgumentType,
    crate::structs::u256::u256,
    r#macro::document,
    std::sync::{Arc, Mutex},
};

/// The deepest calls between contracts may nest. A transaction's call is depth 1.
pub const MAX_CALL_DEPTH: usize = 8;

/// The longest storage key, in bytes.
pub const MAX_KEY_LEN: usize = 256;

/// The largest storage value, in bytes.
pub const MAX_VALUE_LEN: usize = 64 * 1024;

/// The longest event name, in bytes.
pub const MAX_EVENT_NAME: usize = 64;

/// The largest event data, encoded, in bytes.
pub const MAX_EVENT_DATA: usize = 16 * 1024;

/// The most events one call may emit itself, not counting calls it makes.
pub const MAX_EVENTS: usize = 256;

#[document(
    summary = "A running call's way out of its engine: the call's context, the contract's own storage and funds, and calls to other contracts. Each call gets its own, over a state layer of its own, so whatever a failed call wrote or sent is dropped with it.",
    category = "VM",
    see_also = "ScriptEngine::run, CallContext",
    note = "Engines charge fuel for host operations themselves, from vm::cost, since fuel lives in the engine. The Host checks sizes and records database failures, which stop the whole transaction even if a contract catches the error.",
)]
#[derive(Clone)]
pub struct Host {
    state: Arc<State>,
    context: CallContext,
    /// The contracts running, outermost first, ending with this one.
    stack: Vec<Address>,
    memory: usize,
    /// The first database failure anywhere in this tree of calls.
    fault: Arc<Mutex<Option<DbError>>>,
    /// The chain's engines, for calls to other contracts.
    vm: Arc<Vm>,
    /// What this call emitted, joined by what the calls it made emitted once they succeed.
    events: Arc<Mutex<Vec<Event>>>,
    /// How many events this call emitted itself.
    emitted: Arc<Mutex<usize>>,
}

impl Host {
    #[document(
        parent = "Host",
        summary = "The host for a call made by a transaction rather than by another contract.",
        params = {
            state = "The state the call reads and writes.",
            context = "Who is calling, which contract, and how much they sent.",
            memory = "The memory limit for every call in the chain this one starts.",
            vm = "The chain's engines, which run any contract this one calls.",
        },
    )]
    pub fn root(state: Arc<State>, context: CallContext, memory: usize, vm: Arc<Vm>) -> Host {
        Host {
            state,
            context,
            stack: vec![context.contract],
            memory,
            fault: Arc::new(Mutex::new(None)),
            vm,
            events: Arc::default(),
            emitted: Arc::default(),
        }
    }

    #[document(parent = "Host", summary = "Who is calling, which contract, and what they sent.")]
    pub fn context(&self) -> &CallContext {
        &self.context
    }

    #[document(
        parent = "Host",
        summary = "Reads from the contract's storage. Engines charge cost::STORAGE_READ and the bytes moved.",
        returns = "The value, or None when the key has none. Runtime when the key is too long, Storage when the database failed.",
    )]
    pub fn storage_get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, VmError> {
        check_key(key)?;

        self.state
            .get(Column::Storage, &storage_key(&self.context.contract, key))
            .map_err(|err| self.fail(err))
    }

    #[document(
        parent = "Host",
        summary = "Writes to the contract's storage, or deletes the key when value is None. Engines charge cost::STORAGE_WRITE and the bytes moved.",
    )]
    pub fn storage_set(&self, key: &[u8], value: Option<&[u8]>) -> Result<(), VmError> {
        check_key(key)?;

        if value.is_some_and(|value| value.len() > MAX_VALUE_LEN) {
            return Err(VmError::Runtime(format!("storage values are at most {MAX_VALUE_LEN} bytes")));
        }

        let key = storage_key(&self.context.contract, key);

        match value {
            Some(value) => self.state.put(Column::Storage, key, value.to_vec()),
            None => self.state.delete(Column::Storage, key),
        }

        Ok(())
    }

    #[document(
        parent = "Host",
        summary = "The balance of any address. Engines charge cost::BALANCE.",
    )]
    pub fn balance(&self, address: &Address) -> Result<u256, VmError> {
        Accounts::new(&self.state)
            .get(address)
            .map(|account| account.balance)
            .map_err(|err| self.fail(err))
    }

    #[document(
        parent = "Host",
        summary = "Sends funds from this contract's own balance. Kept only if the call succeeds. Engines charge cost::TRANSFER.",
        params = {
            to = "Who receives them: any address.",
            amount = "How much.",
        },
        panics = "InsufficientBalance when the contract holds less than amount.",
    )]
    pub fn transfer(&self, to: &Address, amount: u256) -> Result<(), VmError> {
        send(&self.state, &self.context.contract, to, amount).map_err(|err| self.refused(err))
    }

    #[document(
        parent = "Host",
        summary = "Calls another contract, whichever engine runs it, and sends it value from this contract's balance. The callee sees this contract as its sender and the value in its context. Its writes, and the value, stay only if it succeeds. A contract already running further up the chain can only be entered again through a function marked reentrant.",
        params = {
            target = "The contract to call.",
            function = "The exported function's name.",
            args = "The arguments.",
            value = "What to send with the call; zero for nothing. Engines charge cost::TRANSFER on top of cost::CALL when it is not zero.",
            fuel = "What the callee may burn: what the caller has left after paying for the call.",
        },
        returns = "What the callee did. Subtract its fuel_used from the caller's fuel.",
        panics = "Reentrant when target is already running and the function is not marked reentrant, TooDeep past MAX_CALL_DEPTH, NoSuchContract or NoSuchFunction when there is nothing to call, InsufficientBalance when this contract cannot cover value.",
    )]
    pub fn call(
        &self,
        target: &Address,
        function: &str,
        args: &[TxCallArgumentType],
        value: u256,
        fuel: u64,
    ) -> Outcome {
        if self.stack.len() >= MAX_CALL_DEPTH {
            return Outcome::failed(VmError::TooDeep);
        }

        let record = match Contracts::new(&self.state).get(target) {
            Ok(Some(record)) => record,
            Ok(None) => return Outcome::failed(VmError::NoSuchContract(*target)),
            Err(err) => return Outcome::failed(self.fail(err)),
        };

        let (function, args) = match route(&record, FunctionId::Name(function), args, true) {
            Ok(routed) => routed,
            Err(err) => return Outcome::failed(err),
        };

        if self.stack.contains(target) && !record.reentrant.iter().any(|name| name == function) {
            return Outcome::failed(VmError::Reentrant(*target));
        }

        // The value moves in the callee's layer, so a failed call sends it back.
        let layer = Arc::new(State::new(self.state.clone()));

        if let Err(err) = send(&layer, &self.context.contract, target, value) {
            return Outcome::failed(self.refused(err));
        }

        let context = CallContext {
            sender: self.context.contract,
            contract: *target,
            value,
        };
        let mut stack = self.stack.clone();
        stack.push(*target);
        let callee = Host {
            state: layer,
            context,
            stack,
            memory: self.memory,
            fault: self.fault.clone(),
            vm: self.vm.clone(),
            events: Arc::default(),
            emitted: Arc::default(),
        };
        let limits = Limits {
            fuel,
            memory: self.memory,
        };

        let mut outcome = run(&self.state, &record, function, &args, callee, limits);

        if outcome.result.is_ok() {
            lock(&self.events).append(&mut outcome.events);
        }

        outcome
    }

    #[document(
        parent = "Host",
        summary = "Runs another contract's code as this contract's own: on this contract's storage and balance, under this call's sender and value, with events in this contract's name. The EVM's delegatecall, and what proxies are built on. What it does stays only if it succeeds.",
        params = {
            target = "The contract whose code to run.",
            function = "The function, by name or selector. It must be exported by target; neither its fallback nor a hook is ever run this way.",
            args = "The arguments.",
            fuel = "What it may burn: what the caller has left after paying cost::CALL.",
        },
        returns = "What the code did. Subtract its fuel_used from the caller's fuel.",
        panics = "TooDeep past MAX_CALL_DEPTH, NoSuchContract or NoSuchFunction when there is nothing to run, Reserved for a hook.",
        note = "The code runs with full access to this contract: delegate only to code you trust, and keep storage layouts in agreement, since both sides read and write the same keys.",
    )]
    pub fn delegate(&self, target: &Address, function: FunctionId, args: &[TxCallArgumentType], fuel: u64) -> Outcome {
        if self.stack.len() >= MAX_CALL_DEPTH {
            return Outcome::failed(VmError::TooDeep);
        }

        let record = match Contracts::new(&self.state).get(target) {
            Ok(Some(record)) => record,
            Ok(None) => return Outcome::failed(VmError::NoSuchContract(*target)),
            Err(err) => return Outcome::failed(self.fail(err)),
        };
        let (function, args) = match route(&record, function, args, false) {
            Ok(routed) => routed,
            Err(err) => return Outcome::failed(err),
        };
        let mut stack = self.stack.clone();
        stack.push(self.context.contract);
        let delegated = Host {
            state: Arc::new(State::new(self.state.clone())),
            context: self.context,
            stack,
            memory: self.memory,
            fault: self.fault.clone(),
            vm: self.vm.clone(),
            events: Arc::default(),
            emitted: Arc::default(),
        };
        let limits = Limits {
            fuel,
            memory: self.memory,
        };
        let mut outcome = run(&self.state, &record, function, &args, delegated, limits);

        if outcome.result.is_ok() {
            lock(&self.events).append(&mut outcome.events);
        }

        outcome
    }

    #[document(
        parent = "Host",
        summary = "Emits an event from this contract. It reaches the receipt only if this call, and every call above it, succeeds. Engines charge cost::EVENT and the bytes it takes.",
        params = {
            name = "What happened: at most MAX_EVENT_NAME bytes.",
            data = "The details: at most MAX_EVENT_DATA bytes encoded.",
        },
        returns = "The encoded size, for charging, or a Runtime error past a limit.",
    )]
    pub fn emit(&self, name: &str, data: Vec<TxCallArgumentType>) -> Result<usize, VmError> {
        if name.is_empty() || name.len() > MAX_EVENT_NAME {
            return Err(VmError::Runtime(format!("event names are 1 to {MAX_EVENT_NAME} bytes")));
        }

        let size = wincode::serialize(&data).expect("values always encode").len();

        if size > MAX_EVENT_DATA {
            return Err(VmError::Runtime(format!("event data is at most {MAX_EVENT_DATA} bytes")));
        }

        let mut emitted = lock(&self.emitted);

        if *emitted >= MAX_EVENTS {
            return Err(VmError::Runtime(format!("a call emits at most {MAX_EVENTS} events")));
        }

        *emitted += 1;
        lock(&self.events).push(Event {
            contract: self.context.contract,
            name: name.to_string(),
            data,
        });

        Ok(name.len() + size)
    }

    #[document(
        parent = "Host",
        summary = "The first database failure in this tree of calls, if any.",
    )]
    pub fn fault(&self) -> Option<DbError> {
        self.fault.lock().expect("never poisoned").clone()
    }

    fn fail(&self, err: DbError) -> VmError {
        self.fault.lock().expect("never poisoned").get_or_insert(err.clone());

        VmError::Storage(err)
    }

    fn refused(&self, err: RegistryError) -> VmError {
        match err {
            RegistryError::InsufficientBalance => VmError::InsufficientBalance,
            RegistryError::Db(err) => self.fail(err),
            other => VmError::Runtime(other.to_string()),
        }
    }
}

/// Moves funds in a state layer. Nothing to do for zero, which also spares the reads.
fn send(state: &State, from: &Address, to: &Address, amount: u256) -> Result<(), RegistryError> {
    if amount == u256::ZERO {
        return Ok(());
    }

    Accounts::new(state).transfer(from, to, amount)
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().expect("never poisoned")
}

fn check_key(key: &[u8]) -> Result<(), VmError> {
    if key.len() > MAX_KEY_LEN {
        return Err(VmError::Runtime(format!("storage keys are at most {MAX_KEY_LEN} bytes")));
    }

    Ok(())
}

/// Runs one call in host's state layer, which sits over parent, and merges the layer into parent
/// only if the call succeeds. A database failure anywhere below fails it, whatever the contract
/// made of it.
pub(super) fn run(
    parent: &Arc<State>,
    record: &ContractRecord,
    function: &str,
    args: &[TxCallArgumentType],
    host: Host,
    limits: Limits,
) -> Outcome {
    let vm = host.vm.clone();
    let Some(engine) = vm.engine(record.engine) else {
        return Outcome::failed(VmError::EngineMissing(record.engine));
    };
    let layer = host.state.clone();
    let fault = host.fault.clone();
    let events = host.events.clone();
    let mut outcome = engine.run(&record.code, function, args, host, limits);

    if let Some(err) = fault.lock().expect("never poisoned").clone() {
        outcome.result = Err(VmError::Storage(err));
    }

    if outcome.result.is_ok() {
        parent.merge(layer.take_changes());
        outcome.events = std::mem::take(&mut *lock(&events));
    }

    outcome
}
