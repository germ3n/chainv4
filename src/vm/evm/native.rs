//! The native precompile: Lua and WebAssembly contracts for EVM contracts. Solidity calls it like
//! any contract, through an interface:
//!
//! ```solidity
//! interface Native {
//!     function callNative(bytes32 target, string calldata name, string calldata types, bytes calldata args)
//!         external payable returns (bytes memory);
//! }
//! ```
//!
//! It lives at NATIVE_PRECOMPILE on every chain that runs the EVM. types lists the native types
//! of the arguments, such as "u64,string,address"; args are ABI-encoded as those types say, and
//! what the function returns comes back ABI-encoded, one value after another. The value sent with
//! the call goes to the native contract, which sees the calling EVM contract as its sender.
//!
//! **Unwinding.** revm keeps what the EVM changed in its journal, which reverts a frame's changes
//! when the frame reverts. A native call runs on a layer with the journal written into it, so it
//! sees everything the EVM did so far; what it changed that the EVM sees, balances, nonces, EVM
//! storage and code, goes back into the journal, and reverts with it. Everything else, native
//! storage, names and the rest, is kept aside with a marker slot the call sets in the journal: a
//! frame that reverts takes the marker with it, and at the end of the transaction only what still
//! has its marker is kept. So a revert on either side unwinds both.

use {
    super::{account_of, apply, evm_alias, from_word, remember_alias, to_word},
    crate::abi::{AbiType, AbiValue, decode_values, encode_values},
    crate::db::{Changes, Column, DbError, State},
    crate::registry::Accounts,
    crate::registry::contracts::ContractRecord,
    crate::structs::account::Account,
    crate::structs::address::Address,
    crate::structs::event::Event,
    crate::structs::receipt::Payment,
    crate::structs::tx::TxCallArgumentType,
    crate::structs::u256::{i256, u256},
    crate::vm::host::Host,
    crate::vm::{BlockAt, Limits, Vm, VmError},
    r#macro::document,
    revm::context_interface::journaled_state::account::JournaledAccountTr,
    revm::context_interface::{ContextTr, JournalTr},
    revm::interpreter::CallInputs,
    revm::precompile::{PrecompileHalt, PrecompileOutput},
    revm::primitives::{Address as Word, Bytes, U256, keccak256},
    revm::state::{Bytecode, EvmState},
    std::cell::RefCell,
    std::sync::Arc,
};

#[document(
    summary = "Where the native precompile lives, on every chain that runs the EVM: 0x…4e43, NC in ASCII, beside the names precompile.",
    category = "VM",
    see_also = "Bridge",
)]
pub const NATIVE_PRECOMPILE: [u8; 20] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x4e, 0x43];


const CALL_NATIVE: &str = "callNative(bytes32,string,string,bytes)";
/// The DEX's fee in basis points, as governance set it: what the EVM DEX charges.
const DEX_FEE: &str = "dexFeeBps()";

const USAGE: &str = "call callNative(bytes32 target, string name, string types, bytes args)";

#[document(
    summary = "What an EVM run needs to call native contracts through the native precompile: the chain's engines, the rate gas burns fuel at, and the native contracts already running, outermost first, which a call from the EVM may enter again only through a function marked reentrant.",
    category = "VM",
    see_also = "NATIVE_PRECOMPILE, run, Host::evm_call",
)]
#[derive(Clone)]
pub struct Bridge {
    #[document(summary = "The chain's engines.")]
    pub vm: Arc<Vm>,
    #[document(summary = "The fuel one EVM gas burns.")]
    pub fuel_per_gas: u64,
    #[document(summary = "The contracts running above the EVM, outermost first: empty for a transaction's own EVM call. Each crossing between engines adds one, so calls nest at most MAX_CALL_DEPTH deep across them.")]
    pub stack: Vec<Address>,
}

/// A native call the EVM made that succeeded: what it wrote that the EVM does not see, and how
/// many logs the EVM had emitted when it returned, which places its events among them.
pub(super) struct Crossed {
    pub changes: Changes,
    pub events: Vec<Event>,
    pub log_at: usize,
    /// What it paid, and how many moves the EVM had made when it returned, which places them.
    pub payments: Vec<Payment>,
    pub pay_at: usize,
}

/// The native calls one EVM run makes through the precompile.
pub(super) struct NativeAt<'a> {
    /// The run's own state layer, under the EVM's journal.
    pub state: &'a Arc<State>,
    pub bridge: &'a Bridge,
    pub block: BlockAt,
    pub memory: usize,
    /// The transaction's sender inside the EVM, whose nonce the chain keeps.
    pub caller: [u8; 20],
    pub sender: Address,
    /// Every native call that succeeded, in order: the nth set marker slot n.
    pub crossed: RefCell<Vec<Crossed>>,
    /// The first database failure, which stops the node rather than the transaction.
    pub fault: RefCell<Option<DbError>>,
    /// What the run's frames paid so far, as the inspector keeps it.
    pub paid: super::payments::Paid,
}

enum Answer {
    Returned(Vec<u8>, u64),
    Refused(String, u64),
    OutOfGas,
}

/// A journal failing under revm, as the chain's database error.
fn journal_failed(err: impl std::fmt::Display) -> DbError {
    DbError::Backend(err.to_string())
}

impl NativeAt<'_> {
    /// Answers one callNative. Bad input and failed native calls revert with the reason, as
    /// Solidity's Error(string); running out of gas halts.
    pub fn answer<CTX>(&self, context: &mut CTX, inputs: &CallInputs) -> PrecompileOutput
    where
        CTX: ContextTr<Journal: JournalTr<State = EvmState>>,
    {
        let input = inputs.input.as_bytes(context).to_vec();
        let costs = self.block.vm.costs;

        // The DEX's fee: one of the chain's settings, read as a native contract reads it.
        if input.as_slice() == &keccak256(DEX_FEE.as_bytes())[..4] {
            if costs.federation > inputs.gas_limit {
                return PrecompileOutput::halt(PrecompileHalt::OutOfGas, inputs.reservoir);
            }

            let mut word = [0u8; 32];
            word[28..].copy_from_slice(&self.block.vm.dex.fee_bps.to_be_bytes());

            return PrecompileOutput::new(costs.federation, Bytes::from(word.to_vec()), inputs.reservoir);
        }

        // What callNative costs before the native call burns any, about a cold call, and for
        // every 32 bytes of input: the block's.
        let gas = costs.evm_native_call + costs.evm_native_word * (input.len() as u64).div_ceil(32);

        if gas > inputs.gas_limit {
            return PrecompileOutput::halt(PrecompileHalt::OutOfGas, inputs.reservoir);
        }

        match self.cross(context, inputs, &input, gas) {
            Ok(Answer::Returned(output, used)) => PrecompileOutput::new(used, Bytes::from(output), inputs.reservoir),
            Ok(Answer::Refused(reason, used)) => PrecompileOutput::revert(used, Bytes::from(error_string(&reason)), inputs.reservoir),
            Ok(Answer::OutOfGas) => PrecompileOutput::halt(PrecompileHalt::OutOfGas, inputs.reservoir),
            Err(err) => {
                self.fault.borrow_mut().get_or_insert(err);

                PrecompileOutput::halt(PrecompileHalt::OutOfGas, inputs.reservoir)
            }
        }
    }

    fn cross<CTX>(&self, context: &mut CTX, inputs: &CallInputs, input: &[u8], gas: u64) -> Result<Answer, DbError>
    where
        CTX: ContextTr<Journal: JournalTr<State = EvmState>>,
    {
        let refused = |why: String| Ok(Answer::Refused(why, gas));

        if inputs.target_address != Word::from(NATIVE_PRECOMPILE) {
            return refused("callNative is called, never delegated to".to_string());
        }

        let Some(args) = input.strip_prefix(&keccak256(CALL_NATIVE.as_bytes())[..4]) else {
            return refused(USAGE.to_string());
        };
        let call = [AbiType::FixedBytes(32), AbiType::String, AbiType::String, AbiType::Bytes];
        let Ok([AbiValue::FixedBytes(target), AbiValue::String(name), AbiValue::String(types), AbiValue::Bytes(args)]) =
            decode_values(&call, args).map(<[AbiValue; 4]>::try_from).map(|decoded| decoded.unwrap_or_else(|_| unreachable!("four types decode four values")))
        else {
            return refused(USAGE.to_string());
        };
        let target = Address::Contract(target.try_into().expect("bytes32 is 32 bytes"));
        let kinds: Vec<&str> = types.split(',').map(str::trim).filter(|kind| !kind.is_empty()).collect();
        let abi_types = match kinds.iter().map(|kind| native_type(kind)).collect::<Result<Vec<_>, _>>() {
            Ok(abi_types) => abi_types,
            Err(err) => return refused(err),
        };
        let values = match decode_values(&abi_types, &args) {
            Ok(values) => values,
            Err(err) => return refused(format!("the arguments are not {types}: {err}")),
        };

        // What the native call sees: the run's state, the native calls before it that still
        // hold, and what the EVM changed so far.
        let seen = Arc::new(State::new(self.state.clone()));

        for (index, crossed) in self.crossed.borrow().iter().enumerate() {
            if holds(context, index)? {
                seen.merge(crossed.changes.clone());
            }
        }

        apply(&seen, context.journal().evm_state(), self.caller, &self.sender)?;
        let layer = Arc::new(State::new(seen));
        let caller = account_of(&layer, inputs.caller.into_array())?;
        let mut native = Vec::with_capacity(values.len());

        for (kind, value) in kinds.iter().zip(values) {
            match native_value(kind, value, &layer) {
                Ok(value) => native.push(value),
                Err(VmError::Storage(err)) => return Err(err),
                Err(err) => return refused(err.to_string()),
            }
        }

        // The value sent came to the precompile; it goes on to the contract, from the caller.
        let value = from_word(inputs.transfer_value().unwrap_or_default());

        if value > u256::ZERO {
            let accounts = Accounts::new(&layer);
            let here = account_of(&layer, NATIVE_PRECOMPILE)?;
            accounts.transfer(&here, &caller, value).map_err(journal_failed)?;
        }

        let fuel_per_gas = self.bridge.fuel_per_gas.max(1);
        let limits = Limits {
            fuel: (inputs.gas_limit - gas).saturating_mul(fuel_per_gas),
            memory: self.memory,
        };
        let host = Host::from_evm(layer.clone(), caller, self.block, limits.memory, self.bridge.vm.clone(), self.bridge.stack.clone());
        let outcome = host.call(&target, &name, &native, value, limits.fuel);
        let used = gas.saturating_add(outcome.fuel_used.div_ceil(fuel_per_gas)).min(inputs.gas_limit);
        let returned = match outcome.result {
            Ok(returned) => returned,
            Err(VmError::Storage(err)) => return Err(err),
            Err(VmError::OutOfFuel) => return Ok(Answer::OutOfGas),
            Err(err) => return Ok(Answer::Refused(format!("{name} on {target} failed: {err}"), used)),
        };
        let changes = layer.take_changes();
        let events = host.take_events();
        let payments = host.take_payments();

        if inputs.is_static && (!changes.is_empty() || !events.is_empty()) {
            return Ok(Answer::Refused(format!("{name} on {target} changed state inside a static call"), used));
        }

        let mut out_types = Vec::with_capacity(returned.len());
        let mut out_values = Vec::with_capacity(returned.len());

        for value in &returned {
            match abi_of(value, self.state) {
                Ok((ty, value)) => {
                    out_types.push(ty);
                    out_values.push(value);
                }
                Err(VmError::Storage(err)) => return Err(err),
                Err(err) => return Ok(Answer::Refused(format!("{name} on {target} returned {err}"), used)),
            }
        }

        // As the interface declares it: one bytes, holding the values encoded.
        let values = encode_values(&out_types, &out_values).map_err(DbError::Corrupt)?;
        let output = encode_values(&[AbiType::Bytes], &[AbiValue::Bytes(values)]).map_err(DbError::Corrupt)?;
        let kept = self.push(context, changes)?;
        let index = self.crossed.borrow().len();
        let journal = context.journal_mut();
        journal.load_account(Word::from(NATIVE_PRECOMPILE)).map_err(journal_failed)?;
        journal.sstore(Word::from(NATIVE_PRECOMPILE), U256::from(index), U256::from(1)).map_err(journal_failed)?;
        let log_at = journal.logs().len();
        let pay_at = self.paid.borrow().len();
        self.crossed.borrow_mut().push(Crossed { changes: kept, events, log_at, payments, pay_at });

        Ok(Answer::Returned(output, used))
    }

    /// Writes what a native call changed that the EVM sees into the journal: balances and
    /// nonces, EVM storage, and EVM contracts' code, which a nested EVM call may have created.
    /// Aliases go straight to the run's state, as facts that never need undoing. Returns the rest.
    fn push<CTX>(&self, context: &mut CTX, changes: Changes) -> Result<Changes, DbError>
    where
        CTX: ContextTr<Journal: JournalTr<State = EvmState>>,
    {
        let journal = context.journal_mut();
        let mut kept = Changes::new();

        for ((column, key), value) in changes {
            match column {
                Column::Accounts => {
                    let address = decode::<Address>(&key)?;
                    let account = value.as_deref().map(decode::<Account>).transpose()?.unwrap_or_default();
                    let bytes = evm_alias(&address);
                    remember_alias(self.state, &address)?;
                    let mut loaded = journal.load_account_mut(Word::from(bytes)).map_err(journal_failed)?.data;
                    loaded.set_balance(to_word(account.balance));

                    if bytes != self.caller && loaded.nonce() != account.nonce {
                        loaded.set_nonce(account.nonce);
                    }
                }
                Column::Storage if key.starts_with(super::ALIAS) => match value {
                    Some(value) => self.state.put(column, key, value),
                    None => self.state.delete(column, key),
                },
                Column::Storage if let Some((contract, slot)) = evm_slot(&key) => {
                    let word = value.as_deref().map_or(U256::ZERO, U256::from_be_slice);
                    journal.load_account(contract).map_err(journal_failed)?;
                    journal.sstore(contract, slot, word).map_err(journal_failed)?;
                }
                Column::Contracts => {
                    if let Address::Evm(bytes) = decode::<Address>(&key)?
                        && let Some(record) = value.as_deref().map(decode::<ContractRecord>).transpose()?
                    {
                        let mut loaded = journal.load_account_mut(Word::from(bytes)).map_err(journal_failed)?.data;
                        loaded.set_code_and_hash_slow(Bytecode::new_raw(Bytes::from(record.code)));
                    }

                    // The record, with its deployer, lands with the rest, ahead of the EVM's
                    // changes, so they find the contract there.
                    kept.insert((column, key), value);
                }
                _ => {
                    kept.insert((column, key), value);
                }
            }
        }

        Ok(kept)
    }
}

/// Whether the nth native call's marker still stands: no frame it ran in has reverted.
fn holds<CTX>(context: &mut CTX, index: usize) -> Result<bool, DbError>
where
    CTX: ContextTr<Journal: JournalTr<State = EvmState>>,
{
    let journal = context.journal_mut();
    journal.load_account(Word::from(NATIVE_PRECOMPILE)).map_err(journal_failed)?;

    Ok(journal.sload(Word::from(NATIVE_PRECOMPILE), U256::from(index)).map_err(journal_failed)?.data == U256::from(1))
}

/// The native calls whose markers stand when the run ends, in order.
pub(super) fn standing(crossed: Vec<Crossed>, state: &EvmState) -> Vec<Crossed> {
    let marks = state.get(&Word::from(NATIVE_PRECOMPILE));

    crossed
        .into_iter()
        .enumerate()
        .filter(|(index, _)| marks.and_then(|account| account.storage.get(&U256::from(*index))).is_some_and(|slot| slot.present_value == U256::from(1)))
        .map(|(_, crossed)| crossed)
        .collect()
}

/// The EVM's logs as events, with the events of the native calls that stand placed where they ran.
pub(super) fn interleave(logs: Vec<Event>, standing: &mut [Crossed]) -> Vec<Event> {
    let mut events = Vec::with_capacity(logs.len());
    let mut logs = logs.into_iter();
    let mut emitted = 0;

    for crossed in standing {
        while emitted < crossed.log_at {
            let Some(log) = logs.next() else { break };
            events.push(log);
            emitted += 1;
        }

        events.append(&mut crossed.events);
    }

    events.extend(logs);

    events
}

/// What the run paid: the EVM's own moves, each address told as the account it is, with the
/// payments of the native calls that stand placed where they ran.
pub(super) fn interleave_payments(state: &State, moved: Vec<super::payments::Moved>, standing: &mut [Crossed]) -> Result<Vec<Payment>, DbError> {
    let mut payments = Vec::with_capacity(moved.len());
    let mut moved = moved.into_iter();
    let mut made = 0;
    let told = |movement: super::payments::Moved| -> Result<Payment, DbError> {
        Ok(Payment { from: account_of(state, movement.from)?, to: account_of(state, movement.to)?, amount: from_word(movement.amount) })
    };

    for crossed in standing {
        while made < crossed.pay_at {
            let Some(movement) = moved.next() else { break };
            payments.push(told(movement)?);
            made += 1;
        }

        payments.append(&mut crossed.payments);
    }

    for movement in moved {
        payments.push(told(movement)?);
    }

    Ok(payments)
}

/// An EVM storage slot's key, as the EVM module writes it: the contract's address key, 21 bytes,
/// then the slot's 32.
fn evm_slot(key: &[u8]) -> Option<(Word, U256)> {
    if key.len() != 53 || key[0] != 4 {
        return None;
    }

    Some((Word::from_slice(&key[1..21]), U256::from_be_slice(&key[21..])))
}

fn decode<T>(bytes: &[u8]) -> Result<T, DbError>
where
    T: for<'de> wincode::SchemaRead<'de, wincode::config::DefaultConfig, Dst = T>,
{
    wincode::deserialize(bytes).map_err(|err| DbError::Corrupt(err.to_string()))
}

/// Solidity's Error(string), which a caller's revert bubbles up and tools read.
fn error_string(reason: &str) -> Vec<u8> {
    let mut out = keccak256(b"Error(string)")[..4].to_vec();
    out.extend(encode_values(&[AbiType::String], &[AbiValue::String(reason.to_string())]).expect("a string always encodes"));

    out
}

#[document(
    summary = "The ABI type a native type crosses between engines as, by its name: address as the EVM's address, contract as bytes32, the integers at their widths, amount as uint256, number as int64, and bool, string and bytes as themselves.",
    category = "VM",
    see_also = "NATIVE_PRECOMPILE",
)]
pub fn native_type(kind: &str) -> Result<AbiType, String> {
    Ok(match kind {
        "address" => AbiType::Address,
        "contract" => AbiType::FixedBytes(32),
        "amount" | "u256" => AbiType::Uint(256),
        "u128" => AbiType::Uint(128),
        "u64" => AbiType::Uint(64),
        "i256" => AbiType::Int(256),
        "i128" => AbiType::Int(128),
        "i64" | "number" => AbiType::Int(64),
        "bool" => AbiType::Bool,
        "string" => AbiType::String,
        "bytes" => AbiType::Bytes,
        other => return Err(format!("{other} is no native type: use address, contract, amount, bytes, bool, string, number, i64, u64, i128, u128, i256 or u256")),
    })
}

/// An ABI value as the native type named, decoded by native_type's: an address as the account
/// behind its 20 bytes.
fn native_value(kind: &str, value: AbiValue, state: &State) -> Result<TxCallArgumentType, VmError> {
    let value = match (kind, value) {
        ("contract", AbiValue::FixedBytes(bytes)) => TxCallArgumentType::Address(Address::Contract(bytes.try_into().expect("bytes32"))),
        ("amount", AbiValue::Uint(amount)) => TxCallArgumentType::Amount(crate::structs::amount::Amount::U256(amount)),
        (_, value) => return native_from_abi(&native_type(kind).map_err(VmError::BadArgument)?, value, state),
    };

    Ok(value)
}

#[document(
    summary = "An ABI value as a native one, by its type: unsigned integers up to 64 bits as U64, up to 128 as U128, wider as U256, and the signed ones alike; an address as the account behind its 20 bytes; fixed bytes and bytes as Bytes. Arrays and tuples have no native value.",
    category = "VM",
    see_also = "abi_value",
)]
pub fn native_from_abi(ty: &AbiType, value: AbiValue, state: &State) -> Result<TxCallArgumentType, VmError> {
    let value = match (ty, value) {
        (AbiType::Uint(bits), AbiValue::Uint(number)) => match *bits {
            0..=64 => TxCallArgumentType::U64(u64::try_from(number).map_err(|_| VmError::BadArgument(format!("{number} is no uint{bits}")))?),
            65..=128 => TxCallArgumentType::U128(u128::try_from(number).map_err(|_| VmError::BadArgument(format!("{number} is no uint{bits}")))?),
            _ => TxCallArgumentType::U256(number),
        },
        (AbiType::Int(bits), AbiValue::Int(number)) => match *bits {
            0..=64 => TxCallArgumentType::I64(i64::try_from(number).map_err(|_| VmError::BadArgument(format!("{number} is no int{bits}")))?),
            65..=128 => TxCallArgumentType::I128(i128::try_from(number).map_err(|_| VmError::BadArgument(format!("{number} is no int{bits}")))?),
            _ => TxCallArgumentType::I256(number),
        },
        (_, AbiValue::Address(bytes)) => TxCallArgumentType::Address(account_of(state, bytes).map_err(VmError::Storage)?),
        (_, AbiValue::Bool(flag)) => TxCallArgumentType::Bool(flag),
        (_, AbiValue::String(text)) => TxCallArgumentType::String(text),
        (_, AbiValue::Bytes(bytes) | AbiValue::FixedBytes(bytes)) => TxCallArgumentType::Bytes(bytes),
        (ty, _) => return Err(VmError::BadArgument(format!("a {ty} has no native value"))),
    };

    Ok(value)
}

/// A native value as the ABI type it crosses as, with that value: what callNative returns. A
/// contract goes as bytes32 and any other address as its 20 bytes, its alias written down so the
/// EVM can pay it.
fn abi_of(value: &TxCallArgumentType, state: &State) -> Result<(AbiType, AbiValue), VmError> {
    let ty = match value {
        TxCallArgumentType::Address(Address::Contract(_)) => AbiType::FixedBytes(32),
        TxCallArgumentType::Address(_) => AbiType::Address,
        TxCallArgumentType::Amount(_) | TxCallArgumentType::U256(_) => AbiType::Uint(256),
        TxCallArgumentType::U128(_) => AbiType::Uint(128),
        TxCallArgumentType::U64(_) => AbiType::Uint(64),
        TxCallArgumentType::I256(_) => AbiType::Int(256),
        TxCallArgumentType::I128(_) => AbiType::Int(128),
        TxCallArgumentType::I64(_) | TxCallArgumentType::Number(_) => AbiType::Int(64),
        TxCallArgumentType::Bool(_) => AbiType::Bool,
        TxCallArgumentType::String(_) => AbiType::String,
        TxCallArgumentType::Bytes(_) => AbiType::Bytes,
        TxCallArgumentType::Nil => return Err(VmError::BadReturn("nil, which has no ABI value".to_string())),
    };
    let value = abi_value(&ty, value, Some(state))?;

    Ok((ty, value))
}

#[document(
    summary = "A native value as an ABI value of a type: any integer or whole number in range for an integer type, an address as its 20 bytes inside the EVM, bytes or a string for bytes, exactly as many for fixed bytes, a contract's address for bytes32. With state, an address's alias is written down, so the EVM can pay it.",
    category = "VM",
    see_also = "native_from_abi, evm_alias",
)]
pub fn abi_value(ty: &AbiType, value: &TxCallArgumentType, state: Option<&State>) -> Result<AbiValue, VmError> {
    let not = || VmError::BadArgument(format!("{} is no {ty}", show(value)));
    let value = match ty {
        AbiType::Uint(_) | AbiType::Int(_) => {
            let value = match ty {
                AbiType::Uint(_) => AbiValue::Uint(unsigned(value).ok_or_else(not)?),
                _ => AbiValue::Int(signed(value).ok_or_else(not)?),
            };
            // Encoding checks the width.
            encode_values(std::slice::from_ref(ty), std::slice::from_ref(&value)).map_err(|_| not())?;

            value
        }
        AbiType::Address => match value {
            TxCallArgumentType::Address(address) => {
                if let Some(state) = state {
                    remember_alias(state, address).map_err(VmError::Storage)?;
                }

                AbiValue::Address(evm_alias(address))
            }
            TxCallArgumentType::Bytes(bytes) if bytes.len() == 20 => AbiValue::Address(bytes.as_slice().try_into().expect("20 bytes")),
            _ => return Err(not()),
        },
        AbiType::Bool => match value {
            TxCallArgumentType::Bool(flag) => AbiValue::Bool(*flag),
            _ => return Err(not()),
        },
        AbiType::String => match value {
            TxCallArgumentType::String(text) => AbiValue::String(text.clone()),
            _ => return Err(not()),
        },
        AbiType::Bytes => match value {
            TxCallArgumentType::Bytes(bytes) => AbiValue::Bytes(bytes.clone()),
            TxCallArgumentType::String(text) => AbiValue::Bytes(text.as_bytes().to_vec()),
            _ => return Err(not()),
        },
        AbiType::FixedBytes(size) => match value {
            TxCallArgumentType::Address(Address::Contract(bytes)) if *size == 32 => AbiValue::FixedBytes(bytes.to_vec()),
            TxCallArgumentType::Bytes(bytes) if bytes.len() == usize::from(*size) => AbiValue::FixedBytes(bytes.clone()),
            TxCallArgumentType::String(text) if text.len() == usize::from(*size) => AbiValue::FixedBytes(text.as_bytes().to_vec()),
            _ => return Err(not()),
        },
        AbiType::Array(_) | AbiType::FixedArray(..) | AbiType::Tuple(_) => return Err(VmError::BadArgument(format!("a {ty} has no native value"))),
    };

    Ok(value)
}

/// Any native integer, or a whole number, that is not below zero, as a u256.
fn unsigned(value: &TxCallArgumentType) -> Option<u256> {
    match value {
        TxCallArgumentType::U256(number) => Some(*number),
        TxCallArgumentType::Amount(amount) => Some(amount.to_u256()),
        other => u256::try_from(signed(other)?).ok(),
    }
}

/// Any native integer, or a whole number, as an i256: None for a u256 past i256's range.
fn signed(value: &TxCallArgumentType) -> Option<i256> {
    Some(match value {
        TxCallArgumentType::U64(number) => i256::from(*number),
        TxCallArgumentType::U128(number) => i256::from(*number),
        TxCallArgumentType::U256(number) => i256::try_from(*number).ok()?,
        TxCallArgumentType::Amount(amount) => i256::try_from(amount.to_u256()).ok()?,
        TxCallArgumentType::I64(number) => i256::from(*number),
        TxCallArgumentType::I128(number) => i256::from(*number),
        TxCallArgumentType::I256(number) => *number,
        TxCallArgumentType::Number(number) if number.fract() == 0.0 && number.abs() <= 9_007_199_254_740_992.0 => i256::from(*number as i64),
        _ => return None,
    })
}

fn show(value: &TxCallArgumentType) -> String {
    match value {
        TxCallArgumentType::String(text) => format!("{text:?}"),
        TxCallArgumentType::Bytes(bytes) => crate::structs::hex::to_hex(bytes),
        TxCallArgumentType::Address(address) => address.to_string(),
        other => format!("{other:?}"),
    }
}
