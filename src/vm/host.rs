use {
    super::{Access, CallContext, FunctionId, Layout, Limits, Outcome, Vm, VmError, route},
    crate::db::{Column, Committed, DbError, State, namespace_key, storage_key},
    crate::registry::contracts::{ContractRecord, Contracts},
    crate::registry::{Accounts, RegistryError},
    crate::structs::address::Address,
    crate::structs::event::Event,
    crate::structs::receipt::Payment,
    crate::structs::trace::{Hop, HopKind, Trace},
    crate::structs::tx::TxCallArgumentType,
    crate::structs::u256::u256,
    r#macro::document,
    std::collections::BTreeSet,
    std::sync::{Arc, Mutex},
};

/// What resolving a name on a chain without names fails with, in every build alike.
fn no_names() -> VmError {
    VmError::Runtime("this chain has no names".to_string())
}

// The limits below are a chain's defaults, ContractLimits::DEFAULT's: what each call keeps to is
// its block's, which governance may raise.

/// The deepest calls between contracts may nest by default. A transaction's call is depth 1.
pub const MAX_CALL_DEPTH: usize = crate::params::vm::ContractLimits::DEFAULT.call_depth as usize;

/// The longest storage key by default, in bytes.
pub const MAX_KEY_LEN: usize = crate::params::vm::ContractLimits::DEFAULT.key_len as usize;

/// The largest storage value by default, in bytes.
pub const MAX_VALUE_LEN: usize = crate::params::vm::ContractLimits::DEFAULT.value_len as usize;

/// The longest namespace name, in bytes.
pub const MAX_NAMESPACE_LEN: usize = 64;

/// The most entries one storage scan gives by default: a page, of which a contract asks as many as
/// it needs.
pub const MAX_SCAN: usize = crate::params::vm::ContractLimits::DEFAULT.scan as usize;

/// A page of storage: each key, in its space, with its value.
pub type Page = Vec<(Vec<u8>, Vec<u8>)>;

/// The longest event name, in bytes.
pub const MAX_EVENT_NAME: usize = 64;

/// The largest event data by default, encoded, in bytes.
pub const MAX_EVENT_DATA: usize = crate::params::vm::ContractLimits::DEFAULT.event_data as usize;

/// The most events one call may emit itself by default, not counting calls it makes.
pub const MAX_EVENTS: usize = crate::params::vm::ContractLimits::DEFAULT.events as usize;

#[document(
    summary = "Which part of a contract's storage a key is in. Each part has keys of its own: the same key in two of them is two entries.",
    category = "VM",
    see_also = "Host::storage_get",
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Space {
    #[document(summary = "Plain storage, where most contracts keep everything.")]
    Plain,
    #[document(summary = "A namespace, by its name: 1 to 64 bytes. Any code running on the contract reaches it by name, delegated code included, so code that agrees on a name shares it.")]
    Named(Vec<u8>),
    #[document(summary = "The contract's own space. Only the contract's own code writes it; code it delegates to may read it, but never change it, so a proxy keeps its owner and implementation here, out of its implementations' reach.")]
    Own,
}

impl Space {
    #[document(
        parent = "Space",
        summary = "The space a WebAssembly contract names: the namespace by that name, or the contract's own space for an empty one.",
    )]
    pub fn from_name(name: &[u8]) -> Space {
        match name.is_empty() {
            true => Space::Own,
            false => Space::Named(name.to_vec()),
        }
    }

    #[document(parent = "Space", summary = "The namespace's name, which engines charge for with the key; empty for the others.")]
    pub fn name(&self) -> &[u8] {
        match self {
            Space::Named(name) => name,
            Space::Plain | Space::Own => &[],
        }
    }
}

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
    /// Whether the code running is another contract's, run by delegate on this one's storage.
    delegated: bool,
    /// The layout of the code running, which says which namespaces it may use.
    layout: Arc<Layout>,
    memory: usize,
    /// The first database failure anywhere in this tree of calls.
    fault: Arc<Mutex<Option<DbError>>>,
    /// The chain's engines, for calls to other contracts.
    vm: Arc<Vm>,
    /// What this call emitted, joined by what the calls it made emitted once they succeed.
    events: Arc<Mutex<Vec<Event>>>,
    /// How many events this call emitted itself.
    emitted: Arc<Mutex<usize>>,
    /// The coin this call paid, joined by what the calls it made paid once they succeed.
    payments: Arc<Mutex<Vec<Payment>>>,
    /// Every hop in this tree of calls so far, shared by all of them.
    trace: Arc<Mutex<Trace>>,
    /// Where this call's own hop is in the trace: None for one nobody traced, or past MAX_HOPS.
    hop: Option<usize>,
    /// Where this call's events start among the receipt's, if every call above it succeeds.
    offset: usize,
    /// The contracts that asked for their token registry entry to be asked again, this call's
    /// and those of the calls it made that succeeded.
    refresh: Arc<Mutex<BTreeSet<Address>>>,
}

impl Host {
    #[document(parent = "Host", summary = "What the call's block runs contracts under: costs, limits, the EVM's spec and the DEX's fee.")]
    pub fn rules(&self) -> crate::params::vm::VmRules {
        self.context.block.vm
    }

    #[document(parent = "Host", summary = "What host operations cost in the call's block.")]
    pub fn costs(&self) -> crate::params::vm::HostCosts {
        self.context.block.vm.costs
    }

    #[document(parent = "Host", summary = "The limits contracts keep within in the call's block.")]
    pub fn limits(&self) -> crate::params::vm::ContractLimits {
        self.context.block.vm.limits
    }

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
            delegated: false,
            layout: Arc::default(),
            memory,
            fault: Arc::new(Mutex::new(None)),
            vm,
            events: Arc::default(),
            payments: Arc::default(),
            emitted: Arc::default(),
            trace: Arc::default(),
            hop: None,
            offset: 0,
            refresh: Arc::default(),
        }
    }

    /// The host an EVM contract calls a native one from, through the native precompile: the
    /// calling contract's, over a layer with the EVM's changes so far, with the native contracts
    /// running above the EVM on its stack. Host::call from it makes the call.
    #[cfg(feature = "evm")]
    pub(crate) fn from_evm(state: Arc<State>, caller: Address, block: super::BlockAt, memory: usize, vm: Arc<Vm>, mut stack: Vec<Address>) -> Host {
        stack.push(caller);
        let context = CallContext {
            sender: caller,
            contract: caller,
            value: u256::ZERO,
            block,
        };

        Host { stack, ..Host::root(state, context, memory, vm) }
    }

    /// The call's state layer, for what a library writes down on the side, such as an alias.
    #[cfg(feature = "evm")]
    pub(crate) fn state(&self) -> &Arc<State> {
        &self.state
    }

    /// The hops the calls made from this host took, which the EVM puts under its frame that made them.
    #[cfg(feature = "evm")]
    pub(crate) fn take_trace(&self) -> Trace {
        std::mem::take(&mut *lock(&self.trace))
    }

    /// The tokens the calls made from this host asked to be asked again, once they succeeded.
    #[cfg(feature = "evm")]
    pub(crate) fn take_refresh(&self) -> Vec<Address> {
        std::mem::take(&mut *lock(&self.refresh)).into_iter().collect()
    }

    /// The coin the calls made from this host paid, once they succeeded.
    #[cfg(feature = "evm")]
    pub(crate) fn take_payments(&self) -> Vec<Payment> {
        std::mem::take(&mut *lock(&self.payments))
    }

    /// The events the calls made from this host emitted, once they succeeded.
    #[cfg(feature = "evm")]
    pub(crate) fn take_events(&self) -> Vec<Event> {
        std::mem::take(&mut *lock(&self.events))
    }

    #[document(parent = "Host", summary = "Who is calling, which contract, and what they sent.")]
    pub fn context(&self) -> &CallContext {
        &self.context
    }

    #[document(
        parent = "Host",
        summary = "Reads from the contract's storage. Engines charge cost::STORAGE_READ and the bytes moved, the namespace's name among them.",
        params = {
            space = "Which part of it.",
            key = "The key.",
        },
        returns = "The value, or None when the key has none. Runtime when the key or the namespace's name is too long, Storage when the database failed.",
    )]
    pub fn storage_get(&self, space: &Space, key: &[u8]) -> Result<Option<Vec<u8>>, VmError> {
        let (column, key) = self.locate(space, key, Access::Read)?;

        self.state.get(column, &key).map_err(|err| self.fail(err))
    }

    #[document(
        parent = "Host",
        summary = "Writes to the contract's storage, or deletes the key when value is None. Engines charge cost::STORAGE_WRITE and the bytes moved, the namespace's name among them.",
        params = {
            space = "Which part of it.",
            key = "The key.",
            value = "The value, or None to delete it.",
        },
        panics = "Runtime past a limit, for code run by delegate writing the contract's own space, or for code that declares a layout reaching past it: a namespace or key it does not declare, a value that is not its kind, or a write to something retired or to plain storage.",
    )]
    pub fn storage_set(&self, space: &Space, key: &[u8], value: Option<&[u8]>) -> Result<(), VmError> {
        let most = self.limits().value_len as usize;

        if value.is_some_and(|value| value.len() > most) {
            return Err(VmError::Runtime(format!("storage values are at most {most} bytes")));
        }

        let access = value.map_or(Access::Clear, Access::Write);
        let (column, key) = self.locate(space, key, access)?;

        if *space == Space::Own && self.delegated {
            return Err(VmError::Runtime(
                "a contract's own storage is written only by its own code, not by code it delegates to".to_string(),
            ));
        }

        match value {
            Some(value) => self.state.put(column, key, value.to_vec()),
            None => self.state.delete(column, key),
        }

        Ok(())
    }

    #[document(
        parent = "Host",
        summary = "A page of the contract's storage in key order: the keys in a space that start with a prefix, from a key on, each with its value, as the call's own writes leave them. Engines charge cost::STORAGE_SCAN, then cost::STORAGE_SCAN_ENTRY an entry and the bytes moved. Run ahead of its turn, a transaction that scanned is run again when one before it wrote anywhere in what it scanned.",
        params = {
            space = "Which part of it.",
            prefix = "What every key given starts with: empty for all of them.",
            start = "The first key it may give, the prefix itself for all of them: the key after the last one of the page before, to go on.",
            limit = "The most entries to give: 1 to MAX_SCAN.",
        },
        returns = "Each key, in the space, and its value. Runtime for a prefix or start too long, a limit past MAX_SCAN, or a namespace code that declares a layout does not declare; Storage when the database failed.",
    )]
    pub fn storage_scan(&self, space: &Space, prefix: &[u8], start: &[u8], limit: usize) -> Result<Page, VmError> {
        check_key(prefix, self.limits().key_len as usize)?;

        // One past the longest key, as the key right after a longest one, to go on from it.
        let longest = self.limits().key_len as usize;

        if start.len() > longest + 1 {
            return Err(VmError::Runtime(format!("a scan starts at a key of at most {longest} bytes, or one past it")));
        }

        let page = self.limits().scan as usize;

        if limit == 0 || limit > page {
            return Err(VmError::Runtime(format!("a scan gives 1 to {page} entries at a time")));
        }

        let contract = &self.context.contract;
        let (column, base) = match space {
            Space::Plain => (Column::Storage, storage_key(contract, &[])),
            Space::Own => (Column::Namespaces, namespace_key(contract, &[], &[])),
            Space::Named(name) => {
                check_namespace(name)?;

                if self.layout.is_declared() && self.layout.get(name).is_none() {
                    return Err(VmError::Runtime(format!("namespace {} is not in this code's layout", String::from_utf8_lossy(name))));
                }

                (Column::Namespaces, namespace_key(contract, name, &[]))
            }
        };
        let (full_prefix, full_start) = ([&base[..], prefix].concat(), [&base[..], start].concat());
        let mut found = Vec::new();

        self.state
            .scan(column, &full_prefix, &full_start, &mut |key, value| {
                found.push((key[base.len()..].to_vec(), value.to_vec()));

                found.len() < limit
            })
            .map_err(|err| self.fail(err))?;

        Ok(found)
    }

    /// Where a key in a space is kept, if the code running may do this with it there.
    fn locate(&self, space: &Space, key: &[u8], access: Access<'_>) -> Result<(Column, Vec<u8>), VmError> {
        check_key(key, self.limits().key_len as usize)?;
        let contract = &self.context.contract;

        match space {
            Space::Plain => {
                self.layout.reaches_plain(access).map_err(VmError::Runtime)?;

                Ok((Column::Storage, storage_key(contract, key)))
            }
            Space::Own => Ok((Column::Namespaces, namespace_key(contract, &[], key))),
            Space::Named(name) => {
                check_namespace(name)?;
                self.layout.reaches(name, key, access).map_err(VmError::Runtime)?;

                Ok((Column::Namespaces, namespace_key(contract, name, key)))
            }
        }
    }

    #[document(
        parent = "Host",
        summary = "Whether one contract's code keeps another's storage layout, as Tx::Upgrade asks of new code: what a proxy checks before it points at a new implementation. Engines charge cost::CHECK_LAYOUT.",
        params = {
            from = "The contract whose code would be replaced: a proxy's current implementation.",
            to = "The contract whose code would replace it.",
        },
        returns = "Runtime saying why when to does not keep from's layout, NoSuchContract when either holds no contract, Storage when the database failed.",
        see_also = "Layout::keeps",
    )]
    pub fn check_layout(&self, from: &Address, to: &Address) -> Result<(), VmError> {
        let contracts = Contracts::new(&self.state);
        let layout = |address: &Address| match contracts.get(address) {
            Ok(Some(record)) => Ok(record.layout),
            Ok(None) => Err(VmError::NoSuchContract(*address)),
            Err(err) => Err(self.fail(err)),
        };
        let (old, new) = (layout(from)?, layout(to)?);

        new.keeps(&old)
            .map_err(|why| VmError::Runtime(format!("{to} does not keep the layout of {from}: {why}")))
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
        summary = "The bridges' federation in force, with its version: the one governance decided last, else genesis's as version 0. None on a chain with neither. Engines charge cost::FEDERATION.",
    )]
    pub fn federation(&self) -> Result<Option<crate::registry::federation::DecidedFederation>, VmError> {
        let decided = crate::registry::changes::Changes::new(&self.state).federation().map_err(|err| self.fail(err))?;

        Ok(decided.or_else(|| self.vm.federation().map(|federation| crate::registry::federation::DecidedFederation { version: 0, federation: federation.clone() })))
    }

    #[document(
        parent = "Host",
        summary = "The bridges' federation of a version, in force or past, as the chain keeps every one it decided: what it was, the block that decided it, and the block that decided the next, once one did. Version 0 is genesis's, which a VM given it directly knows without a record. None for a version never decided. Engines charge cost::FEDERATION, as for the one in force.",
        params = { version = "Its version: 0 for genesis's, and one more for each decided since." },
    )]
    pub fn federation_at(&self, version: u64) -> Result<Option<crate::registry::federation::FederationAt>, VmError> {
        let changes = crate::registry::changes::Changes::new(&self.state);

        if let Some(found) = changes.federation_at(version).map_err(|err| self.fail(err))? {
            return Ok(Some(found));
        }

        // Genesis's federation, where the VM was given it rather than the state: replaced once
        // governance decided version 1.
        match (version, self.vm.federation()) {
            (0, Some(federation)) => {
                let replaced = changes.federation_record(1).map_err(|err| self.fail(err))?.map(|record| record.decided);

                Ok(Some(crate::registry::federation::FederationAt { version: 0, federation: federation.clone(), decided: None, replaced }))
            }
            _ => Ok(None),
        }
    }

    #[document(
        parent = "Host",
        summary = "The chain's id, as the gateways name it and the EVM's CHAINID answers: six bytes of its genesis id. Nothing a contract's owner sets, so a contract may bind what it signs or pays to this one chain. Engines charge cost::CHAIN_ID.",
    )]
    pub fn chain_id(&self) -> u64 {
        self.vm.chain_id()
    }

    #[document(parent = "Host", summary = "The chain's hash: what its roots, and so the proofs contracts check, are made with.")]
    pub fn chain_hash(&self) -> crate::structs::hash::HashType {
        self.vm.hash()
    }

    #[document(
        parent = "Host",
        summary = "The address a name resolves to at the call's block time, the name with or without the chain's suffix, in any case. None for a name nobody holds, one past its expiry, or text that is no name. Engines charge cost::NAME.",
        panics = "Runtime on a chain without names.",
    )]
    pub fn resolve_name(&self, name: &str) -> Result<Option<Address>, VmError> {
        let names = self.vm.names().ok_or_else(no_names)?;

        #[cfg(feature = "names")]
        {
            let name = names.strip(name);

            if crate::registry::names::name_problem(&name).is_some() {
                return Ok(None);
            }

            crate::registry::names::Names::new(&self.state)
                .resolve(&name, self.context.block.time, names.grace_ms)
                .map_err(|err| self.fail(err))
        }

        // A build without names opens no chain that has them.
        #[cfg(not(feature = "names"))]
        {
            let _ = (names, name);

            Err(no_names())
        }
    }

    #[document(
        parent = "Host",
        summary = "An address's primary name at the call's block time, with the chain's suffix, such as alice.dev: the name it chose, while that name still resolves to it. Engines charge cost::NAME.",
        panics = "Runtime on a chain without names.",
    )]
    pub fn name_of(&self, address: &Address) -> Result<Option<String>, VmError> {
        let names = self.vm.names().ok_or_else(no_names)?;

        #[cfg(feature = "names")]
        {
            crate::registry::names::Names::new(&self.state)
                .primary(address, self.context.block.time, names.grace_ms)
                .map(|name| name.map(|name| names.display(&name)))
                .map_err(|err| self.fail(err))
        }

        #[cfg(not(feature = "names"))]
        {
            let _ = (names, address);

            Err(no_names())
        }
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
        let hop = self.open(HopKind::Transfer, *to, "", amount);
        let sent = send(&self.state, &self.context.contract, to, amount).map_err(|err| self.refused(err));
        lock(&self.trace).close(hop, 0, sent.as_ref().err().map(ToString::to_string));

        if sent.is_ok() && amount > u256::ZERO {
            lock(&self.payments).push(Payment { from: self.context.contract, to: *to, amount });
        }

        sent
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
        let hop = self.open(HopKind::Call, *target, function, value);
        let outcome = self.enter(target, function, args, value, fuel, hop);
        self.close(hop, &outcome);

        outcome
    }

    fn enter(
        &self,
        target: &Address,
        function: &str,
        args: &[TxCallArgumentType],
        value: u256,
        fuel: u64,
        hop: Option<usize>,
    ) -> Outcome {
        if self.stack.len() >= self.limits().call_depth as usize {
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
            block: self.context.block,
        };
        let mut stack = self.stack.clone();
        stack.push(*target);
        let callee = Host {
            state: layer,
            context,
            stack,
            delegated: false,
            layout: Arc::default(),
            memory: self.memory,
            fault: self.fault.clone(),
            vm: self.vm.clone(),
            events: Arc::default(),
            payments: Arc::default(),
            emitted: Arc::default(),
            trace: self.trace.clone(),
            hop,
            offset: self.offset + lock(&self.events).len(),
            refresh: Arc::default(),
        };
        let limits = Limits {
            fuel,
            memory: self.memory,
        };

        let mut outcome = run(&self.state, &record, function, &args, callee, limits);

        if outcome.result.is_ok() {
            lock(&self.events).append(&mut outcome.events);
            lock(&self.refresh).extend(std::mem::take(&mut outcome.refresh_tokens));
            // The value it was sent first, then what it paid on.
            let mut payments = lock(&self.payments);

            if value > u256::ZERO {
                payments.push(Payment { from: self.context.contract, to: *target, amount: value });
            }

            payments.extend(outcome.payments.iter().cloned());
        }

        outcome
    }

    #[document(
        parent = "Host",
        summary = "Calls an EVM contract, or pays any account, with calldata, sending value from this contract's balance, as Tx::EvmCall does: the EVM sees this contract at its alias, as msg.sender. What it changes, and the value, stay only if it succeeds, and only if this call does. The EVM may call native contracts back through the native precompile, this one among them only through a function marked reentrant.",
        params = {
            to = "Whom to call: an EVM contract, or any account, which the EVM reaches at its 20 bytes.",
            input = "The calldata: a selector and ABI-encoded arguments, or nothing for a plain payment.",
            value = "What to send; zero for nothing. Engines charge cost::TRANSFER on top of cost::CALL when it is not zero.",
            fuel = "What it may burn: what the caller has left after paying for the call. EVM gas burns it at the block's fuel_per_gas.",
        },
        returns = "What the EVM returned, as one Bytes value, or why it failed: a revert's reason as Runtime(\"reverted: …\"), OutOfFuel when its gas ran out.",
        panics = "TooDeep past MAX_CALL_DEPTH across engines, Runtime on a chain without the EVM.",
        see_also = "NATIVE_PRECOMPILE, Bridge",
    )]
    pub fn evm_call(&self, to: &Address, input: &[u8], value: u256, fuel: u64) -> Outcome {
        let hop = self.open(HopKind::Call, *to, "evm", value);
        let outcome = self.enter_evm(hop, to, input, value, fuel);
        self.close(hop, &outcome);

        outcome
    }

    #[cfg(feature = "evm")]
    fn enter_evm(&self, hop: Option<usize>, to: &Address, input: &[u8], value: u256, fuel: u64) -> Outcome {
        use crate::vm::evm::{self, Bridge, EvmAction, EvmBlock, EvmReturned};

        if !self.vm.runs_evm() {
            return Outcome::failed(VmError::Runtime("this chain runs no EVM".to_string()));
        }

        if self.stack.len() >= self.limits().call_depth as usize {
            return Outcome::failed(VmError::TooDeep);
        }

        let me = self.context.contract;
        let block = self.context.block;
        let fuel_per_gas = block.fuel_per_gas.max(1);
        let layer = Arc::new(State::new(self.state.clone()));
        let nonce = match Accounts::new(&layer).get(&me) {
            Ok(account) => account.nonce,
            Err(err) => return Outcome::failed(self.fail(err)),
        };
        let evm_block = EvmBlock {
            height: block.height,
            timestamp: block.time,
            producer: block.producer,
            chain_id: self.vm.chain_id(),
            names: self.vm.names().cloned(),
            hash: self.vm.hash(),
            vm: self.context.block.vm,
        };
        let bridge = Bridge {
            vm: self.vm.clone(),
            fuel_per_gas,
            stack: self.stack.clone(),
        };
        let action = EvmAction::Call { to: *to, value, input: input.to_vec() };
        let ran = match evm::run(&layer, &me, nonce, action, &[], fuel / fuel_per_gas, self.memory as u64, evm_block, Some(&bridge)) {
            Ok(ran) => ran,
            Err(err) => return Outcome::failed(self.fail(err)),
        };
        let fuel_used = ran.gas_used.saturating_mul(fuel_per_gas).min(fuel);
        // Its frames under this call's hop, the top one being that hop itself: their events land
        // after the ones this tree emitted so far, where the EVM's own go.
        let base = self.offset + lock(&self.events).len();
        let mut frames = ran.trace;
        let top = (!frames.hops.is_empty()).then(|| frames.hops.remove(0));
        let mut trace = lock(&self.trace);

        for event in top.map(|top| top.events).unwrap_or_default() {
            trace.emitted(hop, base + event as usize);
        }

        trace.graft(frames, self.stack.len(), |event| u32::try_from(base + event as usize).ok());
        drop(trace);

        let result = match ran.result {
            Ok(EvmReturned::Output(output)) => {
                self.state.merge(layer.take_changes());
                lock(&self.refresh).extend(ran.refresh_tokens);
                let mut events = lock(&self.events);
                events.extend(ran.events);
                // What it sent the EVM first, then what the EVM paid on.
                let mut payments = lock(&self.payments);

                if value > u256::ZERO {
                    payments.push(Payment { from: self.context.contract, to: *to, amount: value });
                }

                payments.extend(ran.payments);

                Ok(vec![TxCallArgumentType::Bytes(output)])
            }
            Ok(EvmReturned::Created(_)) => Err(VmError::Runtime("a call created a contract".to_string())),
            Err(err) => Err(err),
        };

        Outcome {
            fuel_used,
            result,
            events: Vec::new(),
            trace: Trace::default(),
            refresh_tokens: Vec::new(),
            payments: Vec::new(),
        }
    }

    #[cfg(not(feature = "evm"))]
    fn enter_evm(&self, _: Option<usize>, _: &Address, _: &[u8], _: u256, _: u64) -> Outcome {
        Outcome::failed(VmError::Runtime("this node was built without the EVM".to_string()))
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
        note = "The code runs with full access to this contract but for its own space, which it may read and not write: delegate only to code you trust, and keep storage layouts in agreement, since both sides read and write the same plain keys and namespaces.",
    )]
    pub fn delegate(&self, target: &Address, function: FunctionId, args: &[TxCallArgumentType], fuel: u64) -> Outcome {
        let named = match function {
            FunctionId::Name(name) => name.to_string(),
            FunctionId::Selector(selector) => format!("0x{selector:016x}"),
        };
        let hop = self.open(HopKind::Delegate, *target, &named, u256::ZERO);
        let outcome = self.run_as_own(target, function, args, fuel, hop);
        self.close(hop, &outcome);

        outcome
    }

    fn run_as_own(&self, target: &Address, function: FunctionId, args: &[TxCallArgumentType], fuel: u64, hop: Option<usize>) -> Outcome {
        if self.stack.len() >= self.limits().call_depth as usize {
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
        lock(&self.trace).rename(hop, function);
        let mut stack = self.stack.clone();
        stack.push(self.context.contract);
        let delegated = Host {
            state: Arc::new(State::new(self.state.clone())),
            context: self.context,
            stack,
            delegated: true,
            layout: Arc::default(),
            memory: self.memory,
            fault: self.fault.clone(),
            vm: self.vm.clone(),
            events: Arc::default(),
            payments: Arc::default(),
            emitted: Arc::default(),
            trace: self.trace.clone(),
            hop,
            offset: self.offset + lock(&self.events).len(),
            refresh: Arc::default(),
        };
        let limits = Limits {
            fuel,
            memory: self.memory,
        };
        let mut outcome = run(&self.state, &record, function, &args, delegated, limits);

        if outcome.result.is_ok() {
            lock(&self.events).append(&mut outcome.events);
            lock(&self.refresh).extend(std::mem::take(&mut outcome.refresh_tokens));
            lock(&self.payments).extend(outcome.payments.iter().cloned());
        }

        outcome
    }

    #[document(
        parent = "Host",
        summary = "Creates a contract running another's code, as a factory does: a copy of the template's code at clone_address(this contract, the chain hash of salt), deployed by this contract, which then runs the copy's init with these arguments, this contract as its sender. Nothing of the template's storage or balance comes along. A clone that is a token goes in the token registry once the transaction succeeds. All of it stays only if init succeeds, and only if this call does.",
        params = {
            template = "The contract whose code to run: Lua or WebAssembly, not EVM.",
            salt = "This contract's pick, any bytes, fixing the address: the same salt, the same address.",
            args = "What init takes; none for a template without init.",
            fuel = "What init may burn: what the caller has left after paying cost::CLONE.",
        },
        returns = "The new contract's address, as one Address value, or why it failed. Subtract its fuel_used from the caller's fuel.",
        panics = "TooDeep past MAX_CALL_DEPTH, NoSuchContract when there is no template, Runtime when a contract lives at the address already, or when arguments are given to a template without init.",
        see_also = "clone_address, Host::cloned_at, INIT, Contracts::deploy",
    )]
    pub fn clone_contract(&self, template: &Address, salt: &[u8], args: &[TxCallArgumentType], fuel: u64) -> Outcome {
        let address = self.cloned_at(salt);
        let hop = self.open(HopKind::Clone, address, super::INIT, u256::ZERO);
        let outcome = self.create(template, address, args, fuel, hop);
        self.close(hop, &outcome);

        outcome
    }

    #[document(
        parent = "Host",
        summary = "Where this contract's clone with a salt is, or would be: clone_address of this contract and the chain hash of the salt. Engines charge cost::HASH and the salt's bytes.",
    )]
    pub fn cloned_at(&self, salt: &[u8]) -> Address {
        let salt: [u8; 32] = crate::crypto::hasher::Hasher::digest(self.vm.hash(), salt).to_bytes().try_into().expect("256-bit chain hashes");

        crate::structs::address::clone_address(&self.context.contract, &salt, self.vm.hash())
    }

    fn create(&self, template: &Address, address: Address, args: &[TxCallArgumentType], fuel: u64, hop: Option<usize>) -> Outcome {
        if self.stack.len() >= self.limits().call_depth as usize {
            return Outcome::failed(VmError::TooDeep);
        }

        let contracts = Contracts::new(&self.state);
        let template = match contracts.get(template) {
            Ok(Some(record)) => record,
            Ok(None) => return Outcome::failed(VmError::NoSuchContract(*template)),
            Err(err) => return Outcome::failed(self.fail(err)),
        };

        match contracts.get(&address) {
            Ok(None) => {}
            Ok(Some(_)) => return Outcome::failed(VmError::Runtime(format!("a contract lives at {address} already: clone with another salt"))),
            Err(err) => return Outcome::failed(self.fail(err)),
        }

        // Every node keeps the code again.
        let copying = template.code.len() as u64 / self.costs().storage_bytes_per_fuel.max(1);

        if copying > fuel {
            return Outcome { fuel_used: fuel, ..Outcome::failed(VmError::OutOfFuel) };
        }

        let init = template.functions.iter().any(|function| function == super::INIT);

        if !init && !args.is_empty() {
            return Outcome::failed(VmError::Runtime("the template has no init to take arguments".to_string()));
        }

        // The copy goes in a layer of its own, kept only if its init succeeds.
        let layer = Arc::new(State::new(self.state.clone()));
        let record = ContractRecord { deployer: self.context.contract, ..template };
        layer.put_value(Column::Contracts, crate::db::address_key(&address), &record);
        let mut outcome = Outcome {
            fuel_used: 0,
            result: Ok(Vec::new()),
            events: Vec::new(),
            trace: Trace::default(),
            refresh_tokens: Vec::new(),
            payments: Vec::new(),
        };

        if init {
            let context = CallContext {
                sender: self.context.contract,
                contract: address,
                value: u256::ZERO,
                block: self.context.block,
            };
            let mut stack = self.stack.clone();
            stack.push(address);
            let callee = Host {
                state: Arc::new(State::new(layer.clone())),
                context,
                stack,
                delegated: false,
                layout: Arc::default(),
                memory: self.memory,
                fault: self.fault.clone(),
                vm: self.vm.clone(),
                events: Arc::default(),
                payments: Arc::default(),
                emitted: Arc::default(),
                trace: self.trace.clone(),
                hop,
                offset: self.offset + lock(&self.events).len(),
                refresh: Arc::default(),
            };
            let limits = Limits {
                fuel: fuel - copying,
                memory: self.memory,
            };
            outcome = run(&layer, &record, super::INIT, args, callee, limits);
        }

        outcome.fuel_used += copying;

        if outcome.result.is_ok() {
            self.state.merge(layer.take_changes());
            lock(&self.events).append(&mut outcome.events);
            lock(&self.payments).extend(outcome.payments.iter().cloned());
            let mut refresh = lock(&self.refresh);
            refresh.extend(std::mem::take(&mut outcome.refresh_tokens));
            // A token among clones is registered as a deployed one is.
            refresh.insert(address);
            outcome.result = Ok(vec![TxCallArgumentType::Address(address)]);
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

        let largest = self.limits().event_data as usize;

        if size > largest {
            return Err(VmError::Runtime(format!("event data is at most {largest} bytes")));
        }

        let mut emitted = lock(&self.emitted);

        let most = self.limits().events as usize;

        if *emitted >= most {
            return Err(VmError::Runtime(format!("a call emits at most {most} events")));
        }

        *emitted += 1;
        let mut events = lock(&self.events);
        lock(&self.trace).emitted(self.hop, self.offset + events.len());
        events.push(Event {
            contract: self.context.contract,
            name: name.to_string(),
            data,
        });

        Ok(name.len() + size)
    }

    #[document(
        parent = "Host",
        summary = "Asks for this contract's token registry entry to be asked again: once the transaction succeeds, the chain asks its name(), symbol() and decimals() as a deploy does, and an answer that does not fit fails the transaction. Kept only if this call, and every call above it, succeeds. Does nothing for a contract that is not a token. Under delegate, the contract is the one delegating. Engines charge cost::TOKEN_REFRESH.",
        see_also = "Contracts::refresh",
    )]
    pub fn refresh_token(&self) {
        lock(&self.refresh).insert(self.context.contract);
    }

    #[document(
        parent = "Host",
        summary = "The first database failure in this tree of calls, if any.",
    )]
    pub fn fault(&self) -> Option<DbError> {
        self.fault.lock().expect("never poisoned").clone()
    }

    /// Records a hop this call starts. Its depth is the calls running, this one included.
    fn open(&self, kind: HopKind, to: Address, function: &str, value: u256) -> Option<usize> {
        lock(&self.trace).open(Hop::start(self.stack.len(), kind, self.context.contract, to, function, value))
    }

    fn close(&self, hop: Option<usize>, outcome: &Outcome) {
        close(&self.trace, hop, outcome);
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

fn close(trace: &Mutex<Trace>, hop: Option<usize>, outcome: &Outcome) {
    lock(trace).close(hop, outcome.fuel_used, outcome.result.as_ref().err().map(ToString::to_string));
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().expect("never poisoned")
}

fn check_key(key: &[u8], longest: usize) -> Result<(), VmError> {
    if key.len() > longest {
        return Err(VmError::Runtime(format!("storage keys are at most {longest} bytes")));
    }

    Ok(())
}

fn check_namespace(name: &[u8]) -> Result<(), VmError> {
    if name.is_empty() || name.len() > MAX_NAMESPACE_LEN {
        return Err(VmError::Runtime(format!("namespace names are 1 to {MAX_NAMESPACE_LEN} bytes")));
    }

    Ok(())
}

/// Runs a call that a transaction or the chain itself started, as run does, and traces it: its
/// own hop at depth 0, from its sender, and every hop under it.
pub(super) fn run_traced(
    parent: &Arc<State>,
    record: &ContractRecord,
    function: &str,
    args: &[TxCallArgumentType],
    mut host: Host,
    limits: Limits,
) -> Outcome {
    let trace = host.trace.clone();
    let root = Hop::start(0, HopKind::Call, host.context.sender, host.context.contract, function, host.context.value);
    host.hop = lock(&trace).open(root);
    let hop = host.hop;
    let mut outcome = run(parent, record, function, args, host, limits);
    close(&trace, hop, &outcome);
    outcome.trace = std::mem::take(&mut *lock(&trace));

    outcome
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
    let host = Host {
        layout: Arc::new(record.layout.clone()),
        ..host
    };
    let layer = host.state.clone();
    let fault = host.fault.clone();
    let events = host.events.clone();
    let payments = host.payments.clone();
    let refresh = host.refresh.clone();
    let mut outcome = engine.run(&record.code, function, args, host, limits);

    if let Some(err) = fault.lock().expect("never poisoned").clone() {
        outcome.result = Err(VmError::Storage(err));
    }

    if outcome.result.is_ok() {
        parent.merge(layer.take_changes());
        outcome.events = std::mem::take(&mut *lock(&events));
        outcome.payments = std::mem::take(&mut *lock(&payments));
        outcome.refresh_tokens = std::mem::take(&mut *lock(&refresh)).into_iter().collect();
    }

    outcome
}
