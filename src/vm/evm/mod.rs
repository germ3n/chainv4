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
    revm::context_interface::either::Either,
    revm::context_interface::transaction::{AccessList, AccessListItem, Authorization, RecoveredAuthority, RecoveredAuthorization},
    crate::registry::Accounts,
    crate::registry::contracts::ContractRecord,
    crate::structs::address::Address,
    crate::structs::code::Engine,
    crate::structs::event::Event,
    crate::structs::hash::Hash,
    crate::structs::receipt::Payment,
    crate::structs::trace::{Hop, Trace},
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
pub const LOG: &str = crate::structs::event::EVM_LOG;

/// Where an alias is written down, in the storage column: a prefix no address key starts with.
const ALIAS: &[u8] = b"evm alias";
/// Where an account's EIP-7702 delegation is kept, by its 20 bytes: the code its authorization set,
/// 0xef0100 and the address it runs. The account stays the account it is, with no contract record.
const DELEGATION: &[u8] = b"evm delegation";

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

fn delegation_key(bytes: &[u8; 20]) -> Vec<u8> {
    [DELEGATION, bytes].concat()
}

#[document(
    summary = "The code an account's EIP-7702 delegation set: 0xef0100 and the address whose code it runs as its own. None for an account that never delegated, or cleared it.",
    category = "VM",
    see_also = "delegated_to, Authorization",
)]
pub fn delegation(state: &State, bytes: [u8; 20]) -> Result<Option<Vec<u8>>, DbError> {
    state.get(Column::Storage, &delegation_key(&bytes))
}

#[document(
    summary = "The address an account delegates to by EIP-7702: whose code runs when it is called. None for one that does not.",
    category = "VM",
    see_also = "delegation",
)]
pub fn delegated_to(state: &State, bytes: [u8; 20]) -> Result<Option<[u8; 20]>, DbError> {
    Ok(delegation(state, bytes)?.and_then(|code| code.strip_prefix(&[0xef, 0x01, 0x00][..]).and_then(|target| target.try_into().ok())))
}

#[document(
    summary = "What an EVM run's authorizations left an account with, whether the run succeeded or not, as Ethereum keeps them: its delegation, 0xef0100 and an address, or None once cleared, and its nonce.",
    category = "VM",
    see_also = "EvmRan::delegated, keep_delegated",
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Delegated {
    #[document(summary = "The account's 20 bytes.")]
    pub account: [u8; 20],
    #[document(summary = "Its code: 0xef0100 and the address it runs, or None.")]
    pub code: Option<Vec<u8>>,
    #[document(summary = "How many times its authorizations bumped its nonce, past the transaction's own bump of its sender's.")]
    pub bumped: u64,
}

#[document(
    summary = "Writes what authorizations left accounts with into a state: each delegation set or cleared, and the nonce it bumped. What a failed EVM transaction keeps of its run, as its fee and nonce are kept.",
    category = "VM",
    see_also = "Delegated",
)]
pub fn keep_delegated(state: &State, delegated: &[Delegated]) -> Result<(), DbError> {
    let accounts = Accounts::new(state);

    for kept in delegated {
        match &kept.code {
            Some(code) => state.put(Column::Storage, delegation_key(&kept.account), code.clone()),
            None => state.delete(Column::Storage, delegation_key(&kept.account)),
        }

        let address = account_of(state, kept.account)?;
        let mut stored = accounts.get(&address)?;
        stored.nonce = stored.nonce.saturating_add(kept.bumped);
        accounts.set(&address, &stored);
    }

    Ok(())
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
            // An account that delegated by EIP-7702 shows the EVM its delegation as its code.
            _ => delegation(self.state, bytes).map_err(Failing)?.map(|code| Bytecode::new_raw(Bytes::from(code))),
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
    #[document(summary = "What the block's contracts run under: the EVM's spec, which hard fork's opcodes and gas it runs, and what the precompiles cost.")]
    pub vm: crate::params::vm::VmRules,
}

#[document(
    summary = "The revm spec a chain's EVM spec runs as.",
    category = "VM",
    see_also = "EvmSpec",
)]
pub fn spec_id(spec: crate::params::vm::EvmSpec) -> SpecId {
    match spec {
        crate::params::vm::EvmSpec::Shanghai => SpecId::SHANGHAI,
        crate::params::vm::EvmSpec::Cancun => SpecId::CANCUN,
        crate::params::vm::EvmSpec::Prague => SpecId::PRAGUE,
        crate::params::vm::EvmSpec::Osaka => SpecId::OSAKA,
    }
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
    #[document(summary = "Where it went: each EVM frame as a hop, from depth 0 for the run's own call, with the native calls it made through the native precompile under the frame that made them, their own hops and all. Kept when it failed too, with no events.")]
    pub trace: Trace,
    #[document(summary = "The tokens the native calls it made asked to be asked again with tokens.refresh, of those that stand; empty when it failed.")]
    pub refresh_tokens: Vec<Address>,
    #[document(summary = "What its EIP-7702 authorizations left each account they named with, written already when it succeeded; kept by the chain when it failed, as Ethereum keeps them.")]
    pub delegated: Vec<Delegated>,
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
    authorizations: &[crate::crypto::ethereum::Authorization],
    gas_limit: u64,
    memory: u64,
    block: EvmBlock,
    bridge: Option<&Bridge>,
) -> Result<EvmRan, DbError> {
    register_alias(state, sender);
    let caller = evm_alias(sender);
    // Each authorization's account, recovered here as Ethereum recovers it: one whose signature
    // recovers no key, or a high s, is skipped by revm as Ethereum skips it.
    let authorized: Vec<RecoveredAuthorization> = authorizations
        .iter()
        .map(|authorization| {
            let inner = Authorization {
                chain_id: to_word(authorization.chain_id),
                address: Word::from(authorization.address),
                nonce: authorization.nonce,
            };
            let authority = authorization
                .signer()
                .and_then(|key| crate::crypto::secp256k1::address(&key))
                .map_or(RecoveredAuthority::Invalid, |bytes| RecoveredAuthority::Valid(Word::from(bytes)));

            RecoveredAuthorization::new_unchecked(inner, authority)
        })
        .collect();
    let mut authorities: Vec<[u8; 20]> = authorized.iter().filter_map(|authorization| authorization.authority()).map(|bytes| bytes.into_array()).collect();
    authorities.sort();
    authorities.dedup();
    // Each authority's nonce as revm will find it: the caller's as shown to it, bumped once by the
    // transaction itself, and any other's as the state holds it.
    let mut before = Vec::new();

    for bytes in &authorities {
        let start = match *bytes == caller {
            true => nonce.saturating_add(1),
            false => Accounts::new(state).get(&account_of(state, *bytes)?)?.nonce,
        };
        before.push(start);
    }
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
    let mut cfg = CfgEnv::new_with_spec(spec_id(block.vm.evm_spec));
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
        // EIP-7702's type for one with authorizations, else EIP-2930's for one with an access list;
        // the fee is the chain's, so no other.
        tx_type: if authorized.is_empty() { u8::from(!access_list.0.is_empty()) } else { 4 },
        authorization_list: authorized.into_iter().map(Either::Right).collect(),
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
    let traced = paying.traced.clone();
    let fuel_per_gas = bridge.map_or(1, |bridge| bridge.fuel_per_gas.max(1));
    let evm = Context::mainnet().with_db(db).with_block(block_env).with_cfg(cfg).build_mainnet_with_inspector(paying);
    #[cfg(feature = "names")]
    let names = block.names.clone().map(|config| names::NamesAt {
        state,
        config,
        costs: block.vm.costs,
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
            vm: block.vm,
        },
        memory: usize::try_from(memory).unwrap_or(usize::MAX),
        caller,
        sender: *sender,
        crossed: Default::default(),
        fault: Default::default(),
        paid: paid.clone(),
        frames: traced.clone(),
        hops: Default::default(),
        kept_hops: Default::default(),
    });
    let eth = evm.precompiles.clone();
    let mut evm = evm.with_precompiles(precompiles::ChainPrecompiles::new(
        eth,
        #[cfg(feature = "names")]
        names,
        native,
        crypto::CryptoAt { hash: block.hash, costs: block.vm.costs },
    ));

    let transacted = evm.inspect_tx(tx);

    // A lookup the database failed is the node's fault, not the transaction's.
    if let Some(err) = evm.precompiles.fault() {
        return Err(err);
    }

    let crossed = evm.precompiles.native().map(|native| native.crossed.take()).unwrap_or_default();
    let native_hops = evm.precompiles.native().map(|native| native.hops.take()).unwrap_or_default();
    let frames = std::mem::take(&mut *traced.borrow_mut());

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
                trace: Trace::default(),
                refresh_tokens: Vec::new(),
                delegated: Vec::new(),
            });
        }
    };

    let gas_used = outcome.result.tx_gas_used();
    // What the authorizations left their accounts with, which stands whatever the call did.
    let delegated: Vec<Delegated> = authorities
        .iter()
        .zip(&before)
        .filter_map(|(bytes, start)| outcome.state.get(&Word::from(*bytes)).map(|account| (bytes, start, account)))
        .map(|(bytes, start, account)| Delegated {
            account: *bytes,
            code: account.info.code.as_ref().filter(|code| code.is_eip7702()).map(|code| code.original_bytes().to_vec()),
            bumped: account.info.nonce.saturating_sub(*start),
        })
        .collect();
    let result = match outcome.result {
        ExecutionResult::Success { output, logs, .. } => {
            // What the native calls that still stand wrote lands first: an EVM contract a nested
            // call created is there for the EVM's changes to find.
            let mut standing = native::standing(crossed, &outcome.state);

            for (_, crossed) in &standing {
                state.merge(crossed.changes.clone());
            }

            apply(state, &outcome.state, caller, Some(nonce), sender)?;
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
            let placed = Placed { standing: standing.iter().map(|(index, crossed)| (*index, crossed.log_at, crossed.events.len())).collect() };
            let trace = traced_run(state, &frames, native_hops, fuel_per_gas, Some(&placed))?;
            let refresh_tokens = standing.iter_mut().flat_map(|(_, crossed)| std::mem::take(&mut crossed.refresh)).collect();
            let events = native::interleave(logs, &mut standing);
            let moved = paid.borrow().clone();
            let payments = native::interleave_payments(state, moved, &mut standing)?;
            let returned = match output {
                Output::Create(_, Some(created)) => EvmReturned::Created(Address::Evm(created.into_array())),
                Output::Create(_, None) => {
                    return Ok(EvmRan { result: Err(VmError::Runtime("the contract was not created".to_string())), gas_used, events: Vec::new(), reverted: Vec::new(), payments: Vec::new(), trace, refresh_tokens: Vec::new(), delegated: Vec::new() });
                }
                Output::Call(bytes) => EvmReturned::Output(bytes.to_vec()),
            };

            return Ok(EvmRan { result: Ok(returned), gas_used, events, reverted: Vec::new(), payments, trace, refresh_tokens, delegated });
        }
        ExecutionResult::Revert { output, .. } => {
            return Ok(EvmRan {
                result: Err(VmError::Runtime(format!("reverted: {}", revert_reason(&output)))),
                gas_used,
                events: Vec::new(),
                reverted: output.to_vec(),
                payments: Vec::new(),
                trace: traced_run(state, &frames, native_hops, fuel_per_gas, None)?,
                refresh_tokens: Vec::new(),
                delegated,
            });
        }
        ExecutionResult::Halt { reason, .. } => Err(match reason {
            revm::context::result::HaltReason::OutOfGas(_) => VmError::OutOfFuel,
            other => VmError::Runtime(format!("halted: {other:?}")),
        }),
    };
    let trace = traced_run(state, &frames, native_hops, fuel_per_gas, None)?;

    Ok(EvmRan { result, gas_used, events: Vec::new(), reverted: Vec::new(), payments: Vec::new(), trace, refresh_tokens: Vec::new(), delegated })
}

/// Where the events of a run that succeeded land in the receipt: the EVM's logs in order, with the
/// events of each native call that stands put in after the logs emitted before it returned. Each
/// standing call by its place among all the crossed, how many logs came before it, and its events.
struct Placed {
    standing: Vec<(usize, usize, usize)>,
}

impl Placed {
    /// Where the run's nth log that stands lands.
    fn log(&self, nth: usize) -> usize {
        nth + self.standing.iter().filter(|(_, log_at, _)| *log_at <= nth).map(|(_, _, events)| events).sum::<usize>()
    }

    /// Where a native call's own event lands, the call by its place among the crossed: None for
    /// one that does not stand, whose events were dropped.
    fn native(&self, crossed: usize, event: usize) -> Option<usize> {
        let mut before = 0;

        for (index, log_at, events) in &self.standing {
            if *index == crossed {
                return Some(log_at + before + event);
            }

            before += events;
        }

        None
    }
}

/// An EVM address in a hop, told as the account it is: a precompile, at an address no key or
/// contract has, as itself.
fn hop_address(state: &State, bytes: [u8; 20]) -> Result<Address, DbError> {
    match bytes[..18].iter().all(|byte| *byte == 0) {
        true => Ok(Address::Evm(bytes)),
        false => account_of(state, bytes),
    }
}

/// The run's trace: each frame a hop, in the order they began, fuel being gas at the chain's rate,
/// with the logs it emitted that stand placed where they land; and under each frame that called
/// the native precompile, that native call's own hops. With placed None, as for a run that
/// failed, no hop keeps any event.
fn traced_run(state: &State, frames: &payments::Frames, native_hops: Vec<native::NativeHops>, fuel_per_gas: u64, placed: Option<&Placed>) -> Result<Trace, DbError> {
    let mut logged: Vec<Vec<u32>> = vec![Vec::new(); frames.frames.len()];

    if let Some(placed) = placed {
        for (nth, frame) in frames.logs.iter().enumerate() {
            if let Some(logs) = logged.get_mut(*frame) {
                logs.push(u32::try_from(placed.log(nth)).unwrap_or(u32::MAX));
            }
        }
    }

    let mut under: Vec<Vec<native::NativeHops>> = (0..frames.frames.len()).map(|_| Vec::new()).collect();
    let mut loose = Vec::new();

    for hops in native_hops {
        match hops.under.and_then(|at| under.get_mut(at)) {
            Some(list) => list.push(hops),
            None => loose.push(hops),
        }
    }

    let mut trace = Trace { hops: Vec::new(), dropped: frames.dropped };
    let graft = |trace: &mut Trace, hops: native::NativeHops, depth: usize| {
        let mut sub = hops.trace;
        sub.hops.iter_mut().for_each(|hop| hop.depth = hop.depth.saturating_sub(u8::try_from(hops.first).unwrap_or(u8::MAX)));
        let crossed = hops.crossed;
        trace.graft(sub, depth, |event| {
            let (placed, crossed) = (placed?, crossed?);

            placed.native(crossed, event as usize).map(|at| u32::try_from(at).unwrap_or(u32::MAX))
        });
    };

    for ((frame, logs), natives) in frames.frames.iter().zip(logged).zip(under) {
        trace.open(Hop {
            depth: u8::try_from(frame.depth).unwrap_or(u8::MAX),
            kind: frame.kind,
            from: hop_address(state, frame.from)?,
            to: hop_address(state, frame.to)?,
            function: frame.function.clone(),
            value: from_word(frame.value),
            fuel_used: frame.gas.saturating_mul(fuel_per_gas),
            error: frame.error.clone(),
            events: logs,
        });

        for hops in natives {
            graft(&mut trace, hops, frame.depth + 1);
        }
    }

    for hops in loose {
        graft(&mut trace, hops, 1);
    }

    Ok(trace)
}

/// A revert's reason: Solidity's Error(string) read out, else its bytes in hex.
pub(super) fn revert_reason(output: &[u8]) -> String {
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
fn apply(state: &State, changes: &EvmState, caller: [u8; 20], shown: Option<u64>, sender: &Address) -> Result<(), DbError> {
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

        // The chain bumped the sender's nonce already, from the one revm was shown; an
        // authorization of its own bumps it once more, as Ethereum does, which is added. A native
        // call's view mid-run, shown None, keeps the chain's.
        stored.nonce = match (bytes == caller, shown) {
            (true, Some(shown)) => stored.nonce.saturating_add(account.info.nonce.saturating_sub(shown.saturating_add(1))),
            (true, None) => stored.nonce,
            (false, _) => account.info.nonce,
        };

        // An account's EIP-7702 delegation, set or cleared by an authorization.
        if !matches!(address, Address::Evm(_)) && !marks {
            let code = account.info.code.as_ref().filter(|code| code.is_eip7702()).map(|code| code.original_bytes().to_vec());

            if delegation(state, bytes)? != code {
                match code {
                    Some(code) => state.put(Column::Storage, delegation_key(&bytes), code),
                    None => state.delete(Column::Storage, delegation_key(&bytes)),
                }
            }
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
