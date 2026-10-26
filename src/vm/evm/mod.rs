//! The Ethereum Virtual Machine, on revm. EVM contracts run as Ethereum runs them, against
//! chainv4's own state: one balance and nonce per account across every engine, EVM code in the
//! contract registry, and EVM storage in the storage column, keyed by the contract and the slot.
//!
//! **Addresses.** The EVM knows 20-byte addresses. A contract it creates lives at Ethereum's
//! address for it, as Address::Evm. A secp256k1 account is itself inside the EVM, its 20 bytes
//! being Ethereum's. Any other account, Ed25519, EdBlake or a Lua or wasm contract, takes part
//! under an alias: the last 20 bytes of the keccak256 of its key. The alias is written down the
//! first time the account sends an EVM transaction, so the EVM can pay it back; until then, value
//! sent to its alias goes to the secp256k1 account with those bytes instead.
//!
//! **Running.** revm runs a transaction against a database over the transaction's state layer,
//! without committing; this module then writes what it changed back, through the registries. The
//! chain keeps its own nonce and fee handling: revm charges no fee, and the sender's nonce, which
//! the chain bumped already, is shown to revm as it was, so CREATE takes the address Ethereum
//! would. Gas is the chain's gas, one for one, and burns fuel at the chain's rate.
//!
//! **Logs** become events named "log": each topic as 32 bytes, then the data.
//!
//! **Not yet:** calls between EVM contracts and Lua or wasm ones, blob transactions, and the KZG
//! point evaluation precompile, which needs blobs.

use {
    crate::db::{Column, DbError, State, address_key},
    crate::registry::Accounts,
    crate::registry::contracts::ContractRecord,
    crate::structs::address::Address,
    crate::structs::code::Engine,
    crate::structs::event::Event,
    crate::structs::hash::Hash,
    crate::structs::tx::TxCallArgumentType,
    crate::structs::u256::u256,
    crate::vm::VmError,
    r#macro::document,
    revm::context::result::{ExecutionResult, Output},
    revm::context::{BlockEnv, CfgEnv, Context, TxEnv},
    revm::database_interface::DBErrorMarker,
    revm::primitives::hardfork::SpecId,
    revm::primitives::{Address as Word, B256, Bytes, KECCAK_EMPTY, TxKind, U256, keccak256},
    revm::state::{AccountInfo, Bytecode, EvmState},
    revm::{Database, ExecuteEvm, MainBuilder, MainContext},
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
    match address {
        Address::Secp256k1(bytes) | Address::Evm(bytes) => *bytes,
        other => keccak256(address_key(other)).0[12..].try_into().expect("32 bytes hold 20"),
    }
}

#[document(
    summary = "Where an EVM contract a deployer creates goes, as Ethereum's CREATE puts it: from the deployer's 20 bytes and its nonce.",
    category = "VM",
    see_also = "evm_alias, Code::Evm",
)]
pub fn create_address(deployer: &Address, nonce: u64) -> Address {
    Address::Evm(Word::from(evm_alias(deployer)).create(nonce).into_array())
}

#[document(
    summary = "The chain id EVM transactions are signed for, EIP-155's: the first six bytes of the genesis id, so every chain has its own and MetaMask, which holds ids to 53 bits, takes it.",
    category = "VM",
    see_also = "evm_alias",
)]
pub fn chain_id(genesis: &Hash) -> u64 {
    let bytes = genesis.to_bytes();
    let mut id = [0; 8];
    id[2..].copy_from_slice(&bytes[..6]);

    u64::from_be_bytes(id).max(1)
}

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
    #[document(summary = "Runs init code, and keeps what it returns as a new contract's code.")]
    Create(Vec<u8>),
    #[document(summary = "Calls an address with calldata, sending value.")]
    Call { to: Address, value: u256, input: Vec<u8> },
}

#[document(
    summary = "The block an EVM transaction runs in, as the EVM sees it.",
    category = "VM",
    see_also = "run",
)]
#[derive(Clone, Copy, Debug)]
pub struct EvmBlock {
    #[document(summary = "Its height: NUMBER.")]
    pub height: u64,
    #[document(summary = "Its timestamp, in milliseconds: TIMESTAMP reads it in seconds.")]
    pub timestamp: u64,
    #[document(summary = "Its producer, whose 20 bytes COINBASE reads.")]
    pub producer: Address,
    #[document(summary = "The chain id: CHAINID.")]
    pub chain_id: u64,
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
        memory = "The most bytes its memory may grow to, every call frame's together.",
        block = "The block it runs in.",
    },
    returns = "What it did; an error only when the database fails, which is the node's fault.",
)]
pub fn run(state: &Arc<State>, sender: &Address, nonce: u64, action: EvmAction, gas_limit: u64, memory: u64, block: EvmBlock) -> Result<EvmRan, DbError> {
    register_alias(state, sender);
    let caller = evm_alias(sender);
    let (kind, value, input) = match action {
        EvmAction::Create(init) => (TxKind::Create, U256::ZERO, init),
        EvmAction::Call { to, value, input } => (TxKind::Call(Word::from(evm_alias(&to))), to_word(value), input),
    };
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
    let mut evm = Context::mainnet().with_db(db).with_block(block_env).with_cfg(cfg).build_mainnet();

    let outcome = match evm.transact(tx) {
        Ok(outcome) => outcome,
        Err(revm::context::result::EVMError::Database(Failing(err))) => return Err(err),
        Err(err) => {
            return Ok(EvmRan {
                result: Err(VmError::Runtime(format!("the EVM refused it: {err}"))),
                gas_used: gas_limit,
                events: Vec::new(),
            });
        }
    };

    let gas_used = outcome.result.tx_gas_used();
    let result = match outcome.result {
        ExecutionResult::Success { output, logs, .. } => {
            apply(state, outcome.state, caller, sender)?;
            let events = logs
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
            let returned = match output {
                Output::Create(_, Some(created)) => EvmReturned::Created(Address::Evm(created.into_array())),
                Output::Create(_, None) => return Ok(EvmRan { result: Err(VmError::Runtime("the contract was not created".to_string())), gas_used, events: Vec::new() }),
                Output::Call(bytes) => EvmReturned::Output(bytes.to_vec()),
            };

            return Ok(EvmRan { result: Ok(returned), gas_used, events });
        }
        ExecutionResult::Revert { output, .. } => Err(VmError::Runtime(format!("reverted: {}", revert_reason(&output)))),
        ExecutionResult::Halt { reason, .. } => Err(match reason {
            revm::context::result::HaltReason::OutOfGas(_) => VmError::OutOfFuel,
            other => VmError::Runtime(format!("halted: {other:?}")),
        }),
    };

    Ok(EvmRan { result, gas_used, events: Vec::new() })
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
/// chain keeps, new contracts' code, and storage.
fn apply(state: &State, changes: EvmState, caller: [u8; 20], sender: &Address) -> Result<(), DbError> {
    let accounts = Accounts::new(state);

    for (bytes, account) in changes {
        if !account.is_touched() {
            continue;
        }

        let bytes = bytes.into_array();
        let address = match account.is_created() {
            true => Address::Evm(bytes),
            false => account_of(state, bytes)?,
        };

        if account.is_selfdestructed() {
            for slot in account.storage.keys() {
                state.delete(Column::Storage, slot_key(bytes, slot));
            }

            state.delete(Column::Contracts, address_key(&address));
            accounts.set(&address, &Default::default());

            continue;
        }

        let mut stored = accounts.get(&address)?;
        stored.balance = from_word(account.info.balance);

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

        for (slot, value) in account.changed_storage_slots() {
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
