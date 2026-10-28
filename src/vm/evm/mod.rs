//! The Ethereum Virtual Machine, on revm. EVM contracts run as Ethereum runs them, against
//! chainv4's own state: one balance and nonce per account across every engine, EVM code in the
//! contract registry, and EVM storage in the storage column, keyed by the contract and the slot.
//!
//! **Addresses.** The EVM knows 20-byte addresses. A contract it creates lives at Ethereum's
//! address for it, as Address::Evm. A secp256k1 account is itself inside the EVM, its 20 bytes
//! being Ethereum's. Any other account, Ed25519, EdBlake or a Lua or wasm contract, takes part
//! under an alias: the last 20 bytes of the keccak256 of its key. The alias is written down the
//! first time the account sends a transaction of any kind, so the EVM can pay it back; until then,
//! value sent to its alias goes to the secp256k1 account with those bytes instead.
//!
//! **Running.** revm runs a transaction against a database over the transaction's state layer,
//! without committing; this module then writes what it changed back, through the registries. The
//! chain keeps its own nonce and fee handling: revm charges no fee, and the sender's nonce, which
//! the chain bumped already, is shown to revm as it was, so CREATE takes the address Ethereum
//! would. Gas is the chain's gas, one for one, and burns fuel at the chain's rate.
//!
//! **Logs** become events named "log": each topic as 32 bytes, then the data.
//!
//! **Native contracts.** EVM contracts call Lua and wasm ones through the native precompile, and
//! those call EVM contracts back through Host::evm_call, nesting to MAX_CALL_DEPTH across engines;
//! a revert on either side unwinds both.
//!
//! **Not yet:** blob transactions, and the KZG point evaluation precompile, which needs blobs.

mod crypto;
#[cfg(feature = "names")]
mod names;
mod native;
mod payments;
mod precompiles;
pub use crypto::CRYPTO_PRECOMPILE;
#[cfg(feature = "names")]
pub use names::NAMES_PRECOMPILE;
pub use native::{Bridge, NATIVE_PRECOMPILE, abi_value, native_from_abi, native_type};

use {
    crate::crypto::ethereum::AccessItem,
    crate::db::{Column, DbError, State, address_key},
    revm::context_interface::transaction::{AccessList, AccessListItem},
    crate::registry::Accounts,
    crate::registry::contracts::ContractRecord,
    crate::structs::address::Address,
    crate::structs::code::Engine,
    crate::structs::event::Event,
    crate::structs::hash::Hash,
    crate::structs::receipt::Payment,
    crate::structs::tx::TxCallArgumentType,
    crate::structs::u256::u256,
    crate::vm::VmError,
    r#macro::document,
    revm::context::result::{ExecutionResult, Output},
    revm::context::{BlockEnv, CfgEnv, Context, TxEnv},
    revm::database_interface::DBErrorMarker,
    revm::primitives::hardfork::SpecId,
    revm::primitives::{Address as Word, B256, Bytes, KECCAK_EMPTY, TxKind, U256},
    revm::state::{AccountInfo, Bytecode, EvmState},
    revm::{Database, InspectEvm, MainBuilder, MainContext},
    std::sync::Arc,
};

/// What an EVM log is called as an event.
pub const LOG: &str = "log";

/// Where an alias is written down, in the storage column: a prefix no address key starts with.
const ALIAS: &[u8] = b"evm alias";

#[document(
    summary = "The 20 bytes an address goes by inside the EVM: a secp256k1 account's or an EVM contract's own, and for any other account, the last 20 bytes of the keccak256 of its key, as stored.",
    category = "VM",
    see_also = "Tx::EvmCall, Address::Evm",
)]
pub fn evm_alias(address: &Address) -> [u8; 20] {
    crate::abi::evm_address(address)
}

#[document(
    summary = "Where an EVM contract a deployer creates goes, as Ethereum's CREATE puts it: from the deployer's 20 bytes and its nonce.",
    category = "VM",
    see_also = "evm_alias, Code::Evm",
)]
pub fn create_address(deployer: &Address, nonce: u64) -> Address {
    Address::Evm(Word::from(evm_alias(deployer)).create(nonce).into_array())
}

pub use crate::crypto::ethereum::chain_id;

fn alias_key(alias: &[u8; 20]) -> Vec<u8> {
    [ALIAS, alias].concat()
}

#[document(
    summary = "Writes down whose an alias is, so the EVM pays the account itself; nothing for a secp256k1 account or an EVM contract, which are their own 20 bytes.",
    category = "VM",
    see_also = "evm_alias",
)]
pub fn register_alias(state: &State, address: &Address) {
    if !matches!(address, Address::Secp256k1(_) | Address::Evm(_)) {
        state.put_value(Column::Storage, alias_key(&evm_alias(address)), address);
    }
}

#[document(
    summary = "Writes an account's alias down unless it is there already, as every transaction on a chain with the EVM does for its sender: from its first transaction of any kind on, coin the EVM sends its alias reaches it.",
    category = "VM",
    see_also = "register_alias, evm_alias",
)]
pub fn remember_alias(state: &State, address: &Address) -> Result<(), DbError> {
    if matches!(address, Address::Secp256k1(_) | Address::Evm(_)) || state.get(Column::Storage, &alias_key(&evm_alias(address)))?.is_some() {
        return Ok(());
    }

    register_alias(state, address);

    Ok(())
}

#[document(
    summary = "The account behind 20 EVM bytes: the EVM contract there, the account whose alias they are, or else the secp256k1 account with those bytes.",
    category = "VM",
    see_also = "evm_alias, register_alias",
)]
pub fn account_of(state: &State, bytes: [u8; 20]) -> Result<Address, DbError> {
    if state.get(Column::Contracts, &address_key(&Address::Evm(bytes)))?.is_some() {
        return Ok(Address::Evm(bytes));
    }

    Ok(state.get_value::<Address>(Column::Storage, &alias_key(&bytes))?.unwrap_or(Address::Secp256k1(bytes)))
}

#[document(
    summary = "A slot of an EVM contract's storage as its 32 bytes, zero where nothing is kept, as eth_getStorageAt reads it.",
    category = "VM",
    see_also = "account_of",
)]
pub fn storage_word(state: &State, contract: [u8; 20], slot: [u8; 32]) -> Result<[u8; 32], DbError> {
    let held = state.get(Column::Storage, &slot_key(contract, &U256::from_be_bytes(slot)))?;

    Ok(held.and_then(|bytes| bytes.try_into().ok()).unwrap_or([0; 32]))
}

fn slot_key(contract: [u8; 20], slot: &U256) -> Vec<u8> {
    [address_key(&Address::Evm(contract)), slot.to_be_bytes::<32>().to_vec()].concat()
}

fn to_word(value: u256) -> U256 {
    U256::from_be_bytes(value.to_be_bytes())
}

fn from_word(value: U256) -> u256 {
    u256::from_be_bytes(value.to_be_bytes::<32>())
}

/// A database failing under revm.
#[derive(Debug)]
struct Failing(DbError);

impl std::fmt::Display for Failing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for Failing {}

impl DBErrorMarker for Failing {}

/// revm's view of the chain: a transaction's state layer, which records what is read, so running
/// side by side stays exact.
struct ChainDb<'a> {
    state: &'a State,
    /// The sender, whose nonce revm sees as it was before the chain bumped it.
    caller: [u8; 20],
    caller_nonce: u64,
}

impl Database for ChainDb<'_> {
    type Error = Failing;

    fn basic(&mut self, bytes: Word) -> Result<Option<AccountInfo>, Failing> {
        let bytes = bytes.into_array();
        let address = account_of(self.state, bytes).map_err(Failing)?;
        let account = Accounts::new(self.state).get(&address).map_err(Failing)?;
        let nonce = if bytes == self.caller { self.caller_nonce } else { account.nonce };
        let code = match address {
            Address::Evm(_) => self
                .state
                .get_value::<ContractRecord>(Column::Contracts, &address_key(&address))
                .map_err(Failing)?
                .map(|record| Bytecode::new_raw(Bytes::from(record.code))),
            _ => None,
        };

        if account.balance == u256::ZERO && nonce == 0 && code.is_none() {
            return Ok(None);
        }

        let code_hash = code.as_ref().map_or(KECCAK_EMPTY, Bytecode::hash_slow);

        Ok(Some(AccountInfo {
            balance: to_word(account.balance),
            nonce,
            code_hash,
            account_id: None,
            code,
        }))
    }

    /// basic hands every account its code, so revm never looks code up by hash.
    fn code_by_hash(&mut self, _code_hash: B256) -> Result<Bytecode, Failing> {
        Ok(Bytecode::default())
    }

    fn storage(&mut self, bytes: Word, slot: U256) -> Result<U256, Failing> {
        let value = self.state.get(Column::Storage, &slot_key(bytes.into_array(), &slot)).map_err(Failing)?;

        Ok(value.map_or(U256::ZERO, |value| U256::from_be_slice(&value)))
    }

    /// The main chain's block id at a height: BLOCKHASH. Every chain hash a genesis allows is 32 bytes.
    fn block_hash(&mut self, number: u64) -> Result<B256, Failing> {
        let id = self.state.get_value::<Hash>(Column::Heights, &number.to_be_bytes()).map_err(Failing)?;

        Ok(id.map(|id| id.to_bytes()).filter(|bytes| bytes.len() == 32).map_or(B256::ZERO, |bytes| B256::from_slice(&bytes)))
    }
}

#[document(
    summary = "What an EVM transaction asks: to create a contract from init code, or to call an address with calldata.",
    category = "VM",
    see_also = "run",
)]
#[derive(Clone, Debug)]
pub enum EvmAction {
    #[document(summary = "Runs init code, sending value to the contract it makes, and keeps what it returns as the contract's code.")]
    Create { init: Vec<u8>, value: u256 },
    #[document(summary = "Calls an address with calldata, sending value.")]
    Call { to: Address, value: u256, input: Vec<u8> },
}

#[document(
    summary = "The block an EVM transaction runs in, as the EVM sees it.",
    category = "VM",
    see_also = "run",
)]
#[derive(Clone, Debug)]
pub struct EvmBlock {
    #[document(summary = "Its height: NUMBER.")]
    pub height: u64,
    #[document(summary = "Its timestamp, in milliseconds: TIMESTAMP reads it in seconds.")]
    pub timestamp: u64,
    #[document(summary = "Its producer, whose 20 bytes COINBASE reads.")]
    pub producer: Address,
    #[document(summary = "The chain id: CHAINID.")]
    pub chain_id: u64,
    #[document(summary = "The chain's names, which the names precompile resolves, or None on a chain without them, where there is none.")]
    pub names: Option<crate::registry::names::NamesConfig>,
    #[document(summary = "The chain's hash, which the crypto precompile checks proofs by.")]
    pub hash: crate::structs::hash::HashType,
}

#[document(
    summary = "What an EVM transaction did: what it returned or created, or why it failed, the gas it used, and its logs as events.",
    category = "VM",
    see_also = "run",
)]
#[derive(Debug)]
pub struct EvmRan {
    #[document(summary = "The created contract, or what the call returned; a revert's reason, or why it halted.")]
    pub result: Result<EvmReturned, VmError>,
    #[document(summary = "EVM gas used, its intrinsic gas included.")]
    pub gas_used: u64,
    #[document(summary = "Its logs, as events named log; empty when it failed.")]
    pub events: Vec<Event>,
    #[document(summary = "What a revert returned, as Solidity encodes its reason or a custom error, which eth_call hands back; empty for anything else.")]
    pub reverted: Vec<u8>,
    #[document(summary = "The coin its frames below the top paid, and the native contracts it called, in the order they paid it, each address told as the account it is; empty when it failed.")]
    pub payments: Vec<Payment>,
}

#[document(
    summary = "What a successful EVM transaction gave back.",
    category = "VM",
    see_also = "EvmRan",
)]
#[derive(Clone, Debug, PartialEq)]
pub enum EvmReturned {
    #[document(summary = "A contract was created at this address.")]
    Created(Address),
    #[document(summary = "A call returned these bytes.")]
    Output(Vec<u8>),
}

#[document(
    summary = "Runs an EVM transaction on a state layer, and writes what it changed there: balances, nonces, new contracts' code, and storage. On failure nothing it did is written, but the gas it used is still reported, as Ethereum charges it.",
    category = "VM",
    params = {
        state = "The transaction's layer: kept by the caller only on success.",
        sender = "Who sends it: its alias is written down, so the EVM can pay it.",
        nonce = "The sender's nonce for it, before the chain bumped it.",
        action = "A create or a call.",
        gas_limit = "The most EVM gas it may use, intrinsic gas included.",
        access_list = "What it says it reads ahead of time, as Ethereum's transactions of type 1 and 2 do: the addresses and slots start warm, at their price. Empty for any other.",
        memory = "The most bytes its memory may grow to, every call frame's together.",
        block = "The block it runs in.",
        bridge = "How it calls native contracts through the native precompile; None leaves the precompile out, so a call there finds an empty account.",
    },
    returns = "What it did; an error only when the database fails, which is the node's fault.",
)]
#[allow(clippy::too_many_arguments)]
pub fn run(
    state: &Arc<State>,
    sender: &Address,
    nonce: u64,
    action: EvmAction,
    access_list: &[AccessItem],
    gas_limit: u64,
    memory: u64,
    block: EvmBlock,
    bridge: Option<&Bridge>,
) -> Result<EvmRan, DbError> {
    register_alias(state, sender);
    let caller = evm_alias(sender);
    let (kind, value, input) = match action {
        EvmAction::Create { init, value } => (TxKind::Create, to_word(value), init),
        EvmAction::Call { to, value, input } => (TxKind::Call(Word::from(evm_alias(&to))), to_word(value), input),
    };
    let access_list = AccessList(
        access_list
            .iter()
            .map(|item| AccessListItem {
                address: Word::from(item.address),
                storage_keys: item.slots.iter().map(|slot| B256::from(*slot)).collect(),
            })
            .collect(),
    );
    let mut cfg = CfgEnv::new_with_spec(SpecId::PRAGUE);
    cfg.chain_id = block.chain_id;
    cfg.disable_nonce_check = true;
    cfg.disable_fee_charge = true;
    cfg.disable_base_fee = true;
    cfg.disable_block_gas_limit = true;
    cfg.memory_limit = memory;
    let block_env = BlockEnv {
        number: U256::from(block.height),
        beneficiary: Word::from(evm_alias(&block.producer)),
        timestamp: U256::from(block.timestamp / 1_000),
        gas_limit,
        basefee: 0,
        prevrandao: Some(B256::ZERO),
        ..Default::default()
    };
    let tx = TxEnv {
        // EIP-2930's type, for one with an access list; the fee is the chain's, so no other.
        tx_type: u8::from(!access_list.0.is_empty()),
        access_list,
        caller: Word::from(caller),
        gas_limit,
        gas_price: 0,
        kind,
        value,
        data: Bytes::from(input),
        nonce,
        chain_id: Some(block.chain_id),
        ..Default::default()
    };
    let db = ChainDb {
        state,
        caller,
        caller_nonce: nonce,
    };
    let paying = payments::Payments::default();
    let paid = paying.paid.clone();
    let evm = Context::mainnet().with_db(db).with_block(block_env).with_cfg(cfg).build_mainnet_with_inspector(paying);
    #[cfg(feature = "names")]
    let names = block.names.clone().map(|config| names::NamesAt {
        state,
        config,
        now: block.timestamp,
        fault: Default::default(),
    });
    let native = bridge.map(|bridge| native::NativeAt {
        state,
        bridge,
        block: crate::vm::BlockAt {
            height: block.height,
            time: block.timestamp,
            producer: block.producer,
            fuel_per_gas: bridge.fuel_per_gas,
        },
        memory: usize::try_from(memory).unwrap_or(usize::MAX),
        caller,
        sender: *sender,
        crossed: Default::default(),
        fault: Default::default(),
        paid: paid.clone(),
    });
    let eth = evm.precompiles.clone();
    let mut evm = evm.with_precompiles(precompiles::ChainPrecompiles::new(
        eth,
        #[cfg(feature = "names")]
        names,
        native,
        crypto::CryptoAt { hash: block.hash },
    ));

    let transacted = evm.inspect_tx(tx);

    // A lookup the database failed is the node's fault, not the transaction's.
    if let Some(err) = evm.precompiles.fault() {
        return Err(err);
    }

    let crossed = evm.precompiles.native().map(|native| native.crossed.take()).unwrap_or_default();

    let outcome = match transacted {
        Ok(outcome) => outcome,
        Err(revm::context::result::EVMError::Database(Failing(err))) => return Err(err),
        Err(err) => {
            return Ok(EvmRan {
                result: Err(VmError::Runtime(format!("the EVM refused it: {err}"))),
                gas_used: gas_limit,
                events: Vec::new(),
                reverted: Vec::new(),
                payments: Vec::new(),
            });
        }
    };

    let gas_used = outcome.result.tx_gas_used();
    let result = match outcome.result {
        ExecutionResult::Success { output, logs, .. } => {
            // What the native calls that still stand wrote lands first: an EVM contract a nested
            // call created is there for the EVM's changes to find.
            let mut standing = native::standing(crossed, &outcome.state);

            for crossed in &standing {
                state.merge(crossed.changes.clone());
            }

            apply(state, &outcome.state, caller, sender)?;
            let logs = logs
                .iter()
                .map(|log| Event {
                    contract: Address::Evm(log.address.into_array()),
                    name: LOG.to_string(),
                    data: log
                        .topics()
                        .iter()
                        .map(|topic| TxCallArgumentType::Bytes(topic.to_vec()))
                        .chain([TxCallArgumentType::Bytes(log.data.data.to_vec())])
                        .collect(),
                })
                .collect();
            let events = native::interleave(logs, &mut standing);
            let moved = paid.borrow().clone();
            let payments = native::interleave_payments(state, moved, &mut standing)?;
            let returned = match output {
                Output::Create(_, Some(created)) => EvmReturned::Created(Address::Evm(created.into_array())),
                Output::Create(_, None) => return Ok(EvmRan { result: Err(VmError::Runtime("the contract was not created".to_string())), gas_used, events: Vec::new(), reverted: Vec::new(), payments: Vec::new() }),
                Output::Call(bytes) => EvmReturned::Output(bytes.to_vec()),
            };

            return Ok(EvmRan { result: Ok(returned), gas_used, events, reverted: Vec::new(), payments });
        }
        ExecutionResult::Revert { output, .. } => {
            return Ok(EvmRan {
                result: Err(VmError::Runtime(format!("reverted: {}", revert_reason(&output)))),
                gas_used,
                events: Vec::new(),
                reverted: output.to_vec(),
                payments: Vec::new(),
            });
        }
        ExecutionResult::Halt { reason, .. } => Err(match reason {
            revm::context::result::HaltReason::OutOfGas(_) => VmError::OutOfFuel,
            other => VmError::Runtime(format!("halted: {other:?}")),
        }),
    };

    Ok(EvmRan { result, gas_used, events: Vec::new(), reverted: Vec::new(), payments: Vec::new() })
}

/// A revert's reason: Solidity's Error(string) read out, else its bytes in hex.
fn revert_reason(output: &[u8]) -> String {
    const ERROR: [u8; 4] = [0x08, 0xc3, 0x79, 0xa0];

    if output.len() >= 68 && output[..4] == ERROR {
        let length = U256::from_be_slice(&output[36..68]);

        if let Ok(length) = usize::try_from(length)
            && let Some(text) = output.get(68..68 + length)
            && let Ok(text) = std::str::from_utf8(text)
        {
            return text.to_string();
        }
    }

    if output.is_empty() {
        return "without a reason".to_string();
    }

    crate::structs::hex::to_hex(output)
}

/// Writes what a successful EVM transaction changed: balances, nonces but the sender's, which the
/// chain keeps, new contracts' code, and storage. Also how a native call sees what the EVM did so
/// far. The native precompile's marker slots are the journal's alone, and never written.
fn apply(state: &State, changes: &EvmState, caller: [u8; 20], sender: &Address) -> Result<(), DbError> {
    let accounts = Accounts::new(state);

    for (bytes, account) in changes {
        if !account.is_touched() {
            continue;
        }

        let bytes = bytes.into_array();
        let marks = bytes == NATIVE_PRECOMPILE;
        let address = match account.is_created() {
            true => Address::Evm(bytes),
            false => account_of(state, bytes)?,
        };

        if account.is_selfdestructed() && !marks {
            for slot in account.storage.keys() {
                state.delete(Column::Storage, slot_key(bytes, slot));
            }

            state.delete(Column::Contracts, address_key(&address));
            accounts.set(&address, &Default::default());

            continue;
        }

        let mut stored = accounts.get(&address)?;

        // A contract made where coin was sent before it existed takes that coin over: what the EVM
        // saw sat with the account the bytes stood for until now, which keeps none of it, and what
        // was sent to the contract's address here the EVM never saw, so it is added.
        stored.balance = match account.is_created() {
            true => {
                let before = account_of(state, bytes)?;

                if before != address {
                    let mut held = accounts.get(&before)?;
                    held.balance = u256::ZERO;
                    accounts.set(&before, &held);
                }

                from_word(account.info.balance) + stored.balance
            }
            false => from_word(account.info.balance),
        };

        if bytes != caller {
            stored.nonce = account.info.nonce;
        }

        accounts.set(&address, &stored);

        if account.is_created()
            && let Some(code) = &account.info.code
        {
            let record = ContractRecord {
                deployer: *sender,
                engine: Engine::Evm,
                code: code.original_bytes().to_vec(),
                functions: Vec::new(),
                reentrant: Vec::new(),
                layout: Default::default(),
            };
            state.put_value(Column::Contracts, address_key(&address), &record);
        }

        for (slot, value) in account.changed_storage_slots().filter(|_| !marks) {
            let key = slot_key(bytes, slot);

            match value.present_value.is_zero() {
                true => state.delete(Column::Storage, key),
                false => state.put(Column::Storage, key, value.present_value.to_be_bytes::<32>().to_vec()),
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests;
