//! The functions a module imports from "chain". Pointers and lengths are i32s into the module's
//! exported memory. Each function charges its fuel before doing the work, and anything that
//! stops the module records why in Run::error, which says more than the trap.

use {
    super::{IMPORT_MODULE, MEMORY_EXPORT, Run},
    crate::crypto::hasher::Hasher,
    crate::structs::address::Address,
    crate::structs::hash::HashType,
    crate::structs::tx::TxCallArgumentType,
    crate::structs::u256::u256,
    crate::vm::{FunctionId, Host, MAX_EVENT_DATA, MAX_EVENT_NAME, MAX_KEY_LEN, MAX_NAMESPACE_LEN, MAX_VALUE_LEN, Space, VmError, cost},
    r#macro::document,
    wasmi::{Caller, Engine, Extern, Linker, Memory},
};

/// Every function the linker defines. A module importing anything else is refused at deploy.
pub(super) const NAMES: &[&str] = &[
    "input_len",
    "input_read",
    "output",
    "result_len",
    "result_read",
    "storage_get",
    "storage_set",
    "storage_delete",
    "namespace_get",
    "namespace_set",
    "namespace_delete",
    "sender",
    "contract",
    "value",
    "hash",
    "balance",
    "transfer",
    "call",
    "delegate",
    "check_layout",
    "emit",
    "refresh_token",
    "resolve_name",
    "name_of",
    "evm_call",
    "abi_encode",
    "abi_decode",
    "abort",
];

/// An address in memory: its kind tag, then its 32 bytes.
const ADDRESS_LEN: u32 = 33;
/// The most of an abort message that is kept.
const MAX_MESSAGE_LEN: u32 = 1024;

type Result<T> = std::result::Result<T, wasmi::Error>;

#[document(
    wasm,
    kind = "library",
    name = "chain",
    category = "Host",
    summary = "Everything a WebAssembly contract can do outside itself. Import these from the \"chain\" module; pointers and lengths are i32s into the memory the contract exports as \"memory\". Each call starts from a fresh instance, so nothing outside storage outlives it.",
    note = "Arguments and return values are lists of TxCallArgumentType, encoded the way the node encodes them: a u64 count, then each value's u8 tag and contents, all little-endian. Floating-point instructions, SIMD and start functions are refused at deploy. A function may only be entered while its contract is already running further up the chain of calls if the chainv4.reentrant custom section names it. Out-of-bounds pointers, oversized keys and running out of fuel all trap.",
    example = "use chainv4_sdk::{export, storage};\n\n#[export]\nfn increment(by: Option<i64>) -> i64 {\n    let count = storage::get_i64(b\"count\").unwrap_or(0) + by.unwrap_or(1);\n    storage::set(b\"count\", &count.to_le_bytes());\n    count\n}",
    see_also = "rust/WasmEngine, rust/TxCallArgumentType",
)]
fn chain_module() {}

#[document(
    wasm,
    parent = "chain",
    name = "input_len",
    summary = "How many bytes the call's arguments take.",
    returns = { ty = "i32", desc = "The length of the encoded arguments." },
    see_also = "chain.input_read",
)]
fn chain_input_len() {}

#[document(
    wasm,
    parent = "chain",
    name = "input_read",
    summary = "Copies the call's encoded arguments into memory. Costs 1 fuel per 64 bytes.",
    params = { ptr = { ty = "i32", desc = "Where to write input_len bytes." } },
)]
fn chain_input_read() {}

#[document(
    wasm,
    parent = "chain",
    name = "output",
    summary = "Sets what the call returns. The last one wins; a call that never sets it returns nothing. Costs 1 fuel per 64 bytes.",
    params = {
        ptr = { ty = "i32", desc = "The encoded return values." },
        len = { ty = "i32", desc = "Their length." },
    },
)]
fn chain_output() {}

#[document(
    wasm,
    parent = "chain",
    name = "result_len",
    summary = "How many bytes the result buffer holds: what the last storage_get or call left there.",
    returns = { ty = "i32", desc = "The length." },
    see_also = "chain.result_read",
)]
fn chain_result_len() {}

#[document(
    wasm,
    parent = "chain",
    name = "result_read",
    summary = "Copies the result buffer into memory. Costs 1 fuel per 64 bytes.",
    params = { ptr = { ty = "i32", desc = "Where to write result_len bytes." } },
)]
fn chain_result_read() {}

#[document(
    wasm,
    parent = "chain",
    name = "storage_get",
    summary = "Reads from the contract's own storage and leaves the value in the result buffer. Costs 100 fuel, plus 1 per 16 bytes of key and value.",
    params = {
        key = { ty = "i32", desc = "The key." },
        key_len = { ty = "i32", desc = "Its length: at most 256." },
    },
    returns = { ty = "i32", desc = "The value's length, or -1 when the key has none." },
    see_also = "chain.result_read",
)]
fn chain_storage_get() {}

#[document(
    wasm,
    parent = "chain",
    name = "storage_set",
    summary = "Writes to the contract's own storage. Kept only if the call succeeds. A module that declares a storage layout may only read and delete plain keys: writing one traps. Costs 500 fuel, plus 1 per 16 bytes of key and value.",
    params = {
        key = { ty = "i32", desc = "The key." },
        key_len = { ty = "i32", desc = "Its length: at most 256." },
        value = { ty = "i32", desc = "The value." },
        value_len = { ty = "i32", desc = "Its length: at most 65536." },
    },
)]
fn chain_storage_set() {}

#[document(
    wasm,
    parent = "chain",
    name = "storage_delete",
    summary = "Deletes a key from the contract's own storage. Costs what a write does.",
    params = {
        key = { ty = "i32", desc = "The key." },
        key_len = { ty = "i32", desc = "Its length: at most 256." },
    },
)]
fn chain_storage_delete() {}

#[document(
    wasm,
    parent = "chain",
    name = "namespace_get",
    summary = "Reads from one of the contract's namespaces, or its own space, and leaves the value in the result buffer. Each has keys of its own, apart from plain storage and from each other. Costs what storage_get does, plus 1 per 16 bytes of the name.",
    params = {
        name = { ty = "i32", desc = "The namespace's name." },
        name_len = { ty = "i32", desc = "Its length: at most 64. 0 is the contract's own space." },
        key = { ty = "i32", desc = "The key." },
        key_len = { ty = "i32", desc = "Its length: at most 256." },
    },
    returns = { ty = "i32", desc = "The value's length, or -1 when the key has none." },
    see_also = "chain.result_read, chain.namespace_set",
)]
fn chain_namespace_get() {}

#[document(
    wasm,
    parent = "chain",
    name = "namespace_set",
    summary = "Writes to one of the contract's namespaces, or its own space. Kept only if the call succeeds. Code run by delegate may not write the own space, and a module that declares a storage layout may not reach past it, to a namespace or key it does not declare, a retired one, or a value that is not its kind: those trap. Costs what storage_set does, plus 1 per 16 bytes of the name.",
    params = {
        name = { ty = "i32", desc = "The namespace's name." },
        name_len = { ty = "i32", desc = "Its length: at most 64. 0 is the contract's own space." },
        key = { ty = "i32", desc = "The key." },
        key_len = { ty = "i32", desc = "Its length: at most 256." },
        value = { ty = "i32", desc = "The value." },
        value_len = { ty = "i32", desc = "Its length: at most 65536." },
    },
)]
fn chain_namespace_set() {}

#[document(
    wasm,
    parent = "chain",
    name = "namespace_delete",
    summary = "Deletes a key from one of the contract's namespaces, or its own space, as namespace_set writes one. Costs what a write does.",
    params = {
        name = { ty = "i32", desc = "The namespace's name." },
        name_len = { ty = "i32", desc = "Its length: at most 64. 0 is the contract's own space." },
        key = { ty = "i32", desc = "The key." },
        key_len = { ty = "i32", desc = "Its length: at most 256." },
    },
)]
fn chain_namespace_delete() {}

#[document(
    wasm,
    parent = "chain",
    name = "sender",
    summary = "Who made the call: a transaction's sender, or the contract that called this one.",
    params = { ptr = { ty = "i32", desc = "Where to write 33 bytes: the address's kind (0 Ed25519, 1 contract, 2 EdBlake, 3 secp256k1), then its bytes, then zeros after secp256k1's 20." } },
)]
fn chain_sender() {}

#[document(
    wasm,
    parent = "chain",
    name = "contract",
    summary = "This contract's own address.",
    params = { ptr = { ty = "i32", desc = "Where to write 33 bytes, as sender does." } },
)]
fn chain_contract() {}

#[document(
    wasm,
    parent = "chain",
    name = "value",
    summary = "What the call sent: the transaction's value, or what the calling contract sent with call.",
    params = { ptr = { ty = "i32", desc = "Where to write the amount: 32 bytes, a little-endian u256." } },
)]
fn chain_value() {}

#[document(
    wasm,
    parent = "chain",
    name = "hash",
    summary = "Hashes bytes on the node. Costs 20 fuel, plus 1 per 64 bytes hashed.",
    params = {
        algorithm = { ty = "i32", desc = "A HashType tag: 0 SHA-256, 1 BLAKE3, 2 BLAKE2s, 3 BLAKE2b, 4 XXH3, 5 Keccak-256. One the node was built without traps." },
        data = { ty = "i32", desc = "The bytes." },
        len = { ty = "i32", desc = "How many." },
        out = { ty = "i32", desc = "Where to write the digest; leave room for 32 bytes." },
    },
    returns = { ty = "i32", desc = "The digest's length: 32, or 8 big-endian bytes for XXH3." },
)]
fn chain_hash() {}

#[document(
    wasm,
    parent = "chain",
    name = "balance",
    summary = "What an address holds. Costs 100 fuel.",
    params = {
        address = { ty = "i32", desc = "Any address: 33 bytes, as sender writes them." },
        out = { ty = "i32", desc = "Where to write the balance: 32 bytes, a little-endian u256." },
    },
)]
fn chain_balance() {}

#[document(
    wasm,
    parent = "chain",
    name = "transfer",
    summary = "Sends funds from this contract's own balance. Kept only if the call succeeds. Costs 1000 fuel.",
    params = {
        address = { ty = "i32", desc = "Who receives them: 33 bytes, as sender writes them." },
        amount = { ty = "i32", desc = "How much: 32 bytes, a little-endian u256." },
    },
    returns = { ty = "i32", desc = "0 when the funds moved; 1 when the contract holds too little, with the reason in the result buffer." },
)]
fn chain_transfer() {}

#[document(
    wasm,
    parent = "chain",
    name = "call",
    summary = "Calls an exported function on another contract, whichever engine runs it, and sends it value from this contract's balance. The callee sees this contract as its sender and the value as its own, and burns fuel from this call's. Its writes, and the value, are kept only if it succeeds. Costs 1000 fuel, 1000 more when value is not zero, plus 1 per 64 bytes of name and arguments.",
    params = {
        address = { ty = "i32", desc = "The contract to call: 33 bytes, as sender writes them." },
        value = { ty = "i32", desc = "What to send: 32 bytes, a little-endian u256; zero for nothing." },
        name = { ty = "i32", desc = "The function's name, UTF-8." },
        name_len = { ty = "i32", desc = "Its length." },
        args = { ty = "i32", desc = "The encoded arguments." },
        args_len = { ty = "i32", desc = "Their length." },
    },
    returns = { ty = "i32", desc = "0 when the call succeeded, with its encoded return values in the result buffer; 1 when it failed, with the error text there." },
    see_also = "chain.result_read, rust/Host::call",
)]
fn chain_call() {}

#[document(
    wasm,
    parent = "chain",
    name = "delegate",
    summary = "Runs another contract's function as this contract's own: on this contract's storage and balance, under this call's sender and value, with events in this contract's name. The EVM's delegatecall, for proxies. What it does is kept only if it succeeds. Costs 1000 fuel, plus 1 per 64 bytes of arguments.",
    params = {
        address = { ty = "i32", desc = "The contract whose code to run: 33 bytes, as sender writes them." },
        selector = { ty = "i64", desc = "The function's selector, XXH3-64 of its name, as a fallback receives it. Never a fallback or a hook." },
        args = { ty = "i32", desc = "The encoded arguments." },
        args_len = { ty = "i32", desc = "Their length." },
    },
    returns = { ty = "i32", desc = "0 when it succeeded, with its encoded return values in the result buffer; 1 when it failed, with the error text there." },
)]
fn chain_delegate() {}

#[document(
    wasm,
    parent = "chain",
    name = "resolve_name",
    summary = "The address a name resolves to at the block's time, such as alice.dev, with or without the chain's suffix, in any case. Costs 500 fuel, plus 1 per 64 bytes of name. Traps on a chain without names.",
    params = {
        name = { ty = "i32", desc = "The name, UTF-8." },
        len = { ty = "i32", desc = "Its length." },
        out = { ty = "i32", desc = "Where to write the address: 33 bytes, as sender writes them." },
    },
    returns = { ty = "i32", desc = "1 when it resolves, with the address at out; 0 for a name nobody holds, one that expired, or text that is no name." },
    see_also = "rust/Host::resolve_name",
)]
fn chain_resolve_name() {}

#[document(
    wasm,
    parent = "chain",
    name = "name_of",
    summary = "The name an address goes by: its primary name with the chain's suffix, while that name resolves to it. Costs 500 fuel. Traps on a chain without names.",
    params = { address = { ty = "i32", desc = "Any address: 33 bytes, as sender writes them." } },
    returns = { ty = "i32", desc = "The name's length, with the name in the result buffer; -1 for none." },
    see_also = "chain.result_read, rust/Host::name_of",
)]
fn chain_name_of() {}

#[document(
    wasm,
    parent = "chain",
    name = "evm_call",
    summary = "Calls an EVM contract with calldata, or pays any account, sending value from this contract's balance, as an EVM call transaction does. The EVM sees this contract at its alias, as msg.sender. What it changes, and the value, are kept only if it succeeds, and only if this call does. Costs 1000 fuel, 1000 more when value is not zero, plus 1 per 64 bytes of calldata, and the EVM gas it burns at the block's rate of fuel to gas.",
    params = {
        address = { ty = "i32", desc = "Whom to call: 33 bytes, as sender writes them. An EVM contract, or any account, which the EVM reaches at its 20 bytes." },
        value = { ty = "i32", desc = "What to send: 32 bytes, a little-endian u256; zero for nothing." },
        input = { ty = "i32", desc = "The calldata: a selector and ABI-encoded arguments, as abi_encode writes them." },
        input_len = { ty = "i32", desc = "Its length." },
    },
    returns = { ty = "i32", desc = "0 when it succeeded, with what the EVM returned in the result buffer; 1 when it reverted or failed, with why there, a revert's reason read out." },
    see_also = "chain.abi_encode, chain.abi_decode, rust/Host::evm_call",
)]
fn chain_evm_call() {}

#[document(
    wasm,
    parent = "chain",
    name = "abi_encode",
    summary = "Encodes values in Solidity's ABI. A signature with a name, transfer(address,uint256), gives calldata: the selector, then the arguments; types alone in parentheses give the arguments only. Costs 50 fuel, plus 1 per 64 bytes of signature, values and result.",
    params = {
        signature = { ty = "i32", desc = "The signature, UTF-8." },
        signature_len = { ty = "i32", desc = "Its length." },
        values = { ty = "i32", desc = "The values: an encoded list, one for each type. Any integer for an integer type in range, an address for address, which goes as its 20 bytes inside the EVM, bytes or a string for bytes and string. Arrays and tuples have no value of their own here." },
        values_len = { ty = "i32", desc = "Their length." },
    },
    returns = { ty = "i32", desc = "0 with the bytes in the result buffer; 1 with why a value does not fit there." },
)]
fn chain_abi_encode() {}

#[document(
    wasm,
    parent = "chain",
    name = "abi_decode",
    summary = "Decodes bytes in Solidity's ABI: what an EVM call returned, by types in parentheses, or calldata, by a signature with a name, whose selector it checks. Costs 50 fuel, plus 1 per 64 bytes of types and data.",
    params = {
        types = { ty = "i32", desc = "The types or the signature, UTF-8." },
        types_len = { ty = "i32", desc = "Its length." },
        data = { ty = "i32", desc = "The bytes." },
        data_len = { ty = "i32", desc = "Their length." },
    },
    returns = { ty = "i32", desc = "0 with the values in the result buffer, an encoded list: integers as U64, U128 or U256 and their signed kinds by width, an address as the account behind it, bytes as Bytes; 1 with why the bytes are not of the types, or a type has no value here." },
)]
fn chain_abi_decode() {}

#[document(
    wasm,
    parent = "chain",
    name = "check_layout",
    summary = "Whether one contract's code keeps another's storage layout, by the rule Tx::Upgrade holds new code to: what a proxy checks before it points at a new implementation. Costs 2000 fuel.",
    params = {
        from = { ty = "i32", desc = "The 33-byte address of the contract whose code would be replaced: a proxy's current implementation." },
        to = { ty = "i32", desc = "The 33-byte address of the contract whose code would replace it." },
    },
    returns = { ty = "i32", desc = "0 when it keeps it; 1 when it does not, or either holds no contract, with why in the result buffer." },
    see_also = "rust/Layout",
)]
fn chain_check_layout() {}

#[document(
    wasm,
    parent = "chain",
    name = "emit",
    summary = "Emits an event from this contract. It lands in the transaction's receipt only if this call, and every call above it, succeeds. Costs 200 fuel, plus 1 per 16 bytes of name and data.",
    params = {
        name = { ty = "i32", desc = "What happened, UTF-8, 1 to 64 bytes." },
        name_len = { ty = "i32", desc = "Its length." },
        data = { ty = "i32", desc = "The details: an encoded list of values, at most 16 KiB." },
        data_len = { ty = "i32", desc = "Their length." },
    },
)]
fn chain_emit() {}

#[document(
    wasm,
    parent = "chain",
    name = "refresh_token",
    summary = "Asks the token registry to ask this contract's name, symbol and decimals again, as a deploy does, once the transaction succeeds. An answer that does not fit fails the transaction. Kept only if this call, and every call above it, succeeds; does nothing for a contract that is not a token. Costs 100 fuel, and the questions cost what they burn.",
)]
fn chain_refresh_token() {}

#[document(
    wasm,
    parent = "chain",
    name = "abort",
    summary = "Stops the call with an error. Nothing it wrote is kept. Only the first 1024 bytes of the message are.",
    params = {
        message = { ty = "i32", desc = "The message, UTF-8." },
        len = { ty = "i32", desc = "Its length." },
    },
)]
fn chain_abort() {}

pub(super) fn linker(engine: &Engine) -> Linker<Run> {
    let mut linker = Linker::new(engine);
    let defined = "every host function has its own name";

    linker.func_wrap(IMPORT_MODULE, "input_len", input_len).expect(defined);
    linker.func_wrap(IMPORT_MODULE, "input_read", input_read).expect(defined);
    linker.func_wrap(IMPORT_MODULE, "output", output).expect(defined);
    linker.func_wrap(IMPORT_MODULE, "result_len", result_len).expect(defined);
    linker.func_wrap(IMPORT_MODULE, "result_read", result_read).expect(defined);
    linker.func_wrap(IMPORT_MODULE, "storage_get", storage_get).expect(defined);
    linker.func_wrap(IMPORT_MODULE, "storage_set", storage_set).expect(defined);
    linker.func_wrap(IMPORT_MODULE, "storage_delete", storage_delete).expect(defined);
    linker.func_wrap(IMPORT_MODULE, "namespace_get", namespace_get).expect(defined);
    linker.func_wrap(IMPORT_MODULE, "namespace_set", namespace_set).expect(defined);
    linker.func_wrap(IMPORT_MODULE, "namespace_delete", namespace_delete).expect(defined);
    linker.func_wrap(IMPORT_MODULE, "sender", sender).expect(defined);
    linker.func_wrap(IMPORT_MODULE, "contract", contract).expect(defined);
    linker.func_wrap(IMPORT_MODULE, "value", value).expect(defined);
    linker.func_wrap(IMPORT_MODULE, "hash", hash).expect(defined);
    linker.func_wrap(IMPORT_MODULE, "balance", balance).expect(defined);
    linker.func_wrap(IMPORT_MODULE, "transfer", transfer).expect(defined);
    linker.func_wrap(IMPORT_MODULE, "call", call).expect(defined);
    linker.func_wrap(IMPORT_MODULE, "delegate", delegate).expect(defined);
    linker.func_wrap(IMPORT_MODULE, "check_layout", check_layout).expect(defined);
    linker.func_wrap(IMPORT_MODULE, "emit", emit).expect(defined);
    linker.func_wrap(IMPORT_MODULE, "refresh_token", refresh_token).expect(defined);
    linker.func_wrap(IMPORT_MODULE, "resolve_name", resolve_name).expect(defined);
    linker.func_wrap(IMPORT_MODULE, "name_of", name_of).expect(defined);
    linker.func_wrap(IMPORT_MODULE, "evm_call", evm_call).expect(defined);
    linker.func_wrap(IMPORT_MODULE, "abi_encode", abi_encode).expect(defined);
    linker.func_wrap(IMPORT_MODULE, "abi_decode", abi_decode).expect(defined);
    linker.func_wrap(IMPORT_MODULE, "abort", abort).expect(defined);

    linker
}

fn input_len(caller: Caller<'_, Run>) -> Result<u32> {
    Ok(caller.data().input.len() as u32)
}

fn input_read(mut caller: Caller<'_, Run>, ptr: u32) -> Result<()> {
    let input = caller.data().input.clone();
    charge(&mut caller, input.len() as u64 / cost::COPY_BYTES_PER_FUEL)?;

    write(&mut caller, ptr, &input)
}

fn output(mut caller: Caller<'_, Run>, ptr: u32, len: u32) -> Result<()> {
    charge(&mut caller, u64::from(len) / cost::COPY_BYTES_PER_FUEL)?;
    let bytes = read(&mut caller, ptr, len)?;
    caller.data_mut().output = Some(bytes);

    Ok(())
}

fn result_len(caller: Caller<'_, Run>) -> Result<u32> {
    Ok(caller.data().result.len() as u32)
}

fn result_read(mut caller: Caller<'_, Run>, ptr: u32) -> Result<()> {
    let result = caller.data().result.clone();
    charge(&mut caller, result.len() as u64 / cost::COPY_BYTES_PER_FUEL)?;

    write(&mut caller, ptr, &result)
}

fn storage_get(mut caller: Caller<'_, Run>, key: u32, key_len: u32) -> Result<i32> {
    get_in(&mut caller, &Space::Plain, key, key_len)
}

fn storage_set(mut caller: Caller<'_, Run>, key: u32, key_len: u32, value: u32, value_len: u32) -> Result<()> {
    set_in(&mut caller, &Space::Plain, key, key_len, Some((value, value_len)))
}

fn storage_delete(mut caller: Caller<'_, Run>, key: u32, key_len: u32) -> Result<()> {
    set_in(&mut caller, &Space::Plain, key, key_len, None)
}

fn namespace_get(mut caller: Caller<'_, Run>, name: u32, name_len: u32, key: u32, key_len: u32) -> Result<i32> {
    let space = space(&mut caller, name, name_len)?;

    get_in(&mut caller, &space, key, key_len)
}

fn namespace_set(
    mut caller: Caller<'_, Run>,
    name: u32,
    name_len: u32,
    key: u32,
    key_len: u32,
    value: u32,
    value_len: u32,
) -> Result<()> {
    let space = space(&mut caller, name, name_len)?;

    set_in(&mut caller, &space, key, key_len, Some((value, value_len)))
}

fn namespace_delete(mut caller: Caller<'_, Run>, name: u32, name_len: u32, key: u32, key_len: u32) -> Result<()> {
    let space = space(&mut caller, name, name_len)?;

    set_in(&mut caller, &space, key, key_len, None)
}

/// The space a namespace import names: an empty name is the contract's own.
fn space(caller: &mut Caller<'_, Run>, name: u32, name_len: u32) -> Result<Space> {
    check_len(caller, name_len, MAX_NAMESPACE_LEN, "namespace names")?;
    let name = read(caller, name, name_len)?;

    Ok(Space::from_name(&name))
}

fn get_in(caller: &mut Caller<'_, Run>, space: &Space, key: u32, key_len: u32) -> Result<i32> {
    let host = host(caller)?;
    check_len(caller, key_len, MAX_KEY_LEN, "storage keys")?;
    let named = space.name().len() as u64 + u64::from(key_len);
    charge(caller, cost::STORAGE_READ + named / cost::STORAGE_BYTES_PER_FUEL)?;
    let key = read(caller, key, key_len)?;
    let value = host.storage_get(space, &key).map_err(|err| stop(caller, err))?;

    let Some(value) = value else {
        return Ok(-1);
    };

    charge(caller, value.len() as u64 / cost::STORAGE_BYTES_PER_FUEL)?;
    let len = value.len() as i32;
    caller.data_mut().result = value;

    Ok(len)
}

/// Writes the value at value's pointer and length, or deletes the key for None.
fn set_in(caller: &mut Caller<'_, Run>, space: &Space, key: u32, key_len: u32, value: Option<(u32, u32)>) -> Result<()> {
    let host = host(caller)?;
    check_len(caller, key_len, MAX_KEY_LEN, "storage keys")?;
    let value_len = value.map_or(0, |(_, len)| len);
    check_len(caller, value_len, MAX_VALUE_LEN, "storage values")?;
    let moved = space.name().len() as u64 + u64::from(key_len) + u64::from(value_len);
    charge(caller, cost::STORAGE_WRITE + moved / cost::STORAGE_BYTES_PER_FUEL)?;
    let key = read(caller, key, key_len)?;
    let value = match value {
        Some((ptr, len)) => Some(read(caller, ptr, len)?),
        None => None,
    };

    host.storage_set(space, &key, value.as_deref()).map_err(|err| stop(caller, err))
}

fn sender(mut caller: Caller<'_, Run>, ptr: u32) -> Result<()> {
    let sender = host(&mut caller)?.context().sender;

    write(&mut caller, ptr, &sender.to_abi())
}

fn contract(mut caller: Caller<'_, Run>, ptr: u32) -> Result<()> {
    let contract = host(&mut caller)?.context().contract;

    write(&mut caller, ptr, &contract.to_abi())
}

fn value(mut caller: Caller<'_, Run>, ptr: u32) -> Result<()> {
    let value = host(&mut caller)?.context().value;

    write(&mut caller, ptr, &value.to_le_bytes())
}

fn hash(mut caller: Caller<'_, Run>, algorithm: u32, data: u32, len: u32, out: u32) -> Result<u32> {
    charge(&mut caller, cost::HASH + u64::from(len) / cost::HASH_BYTES_PER_FUEL)?;
    let algorithm = u8::try_from(algorithm)
        .ok()
        .and_then(|tag| wincode::deserialize::<HashType>(&[tag]).ok());
    let Some(algorithm) = algorithm else {
        return Err(stop(&mut caller, VmError::Runtime("unknown hash algorithm".to_string())));
    };
    let data = read(&mut caller, data, len)?;
    let digest = Hasher::digest(algorithm, &data).to_bytes();
    write(&mut caller, out, &digest)?;

    Ok(digest.len() as u32)
}

fn balance(mut caller: Caller<'_, Run>, address: u32, out: u32) -> Result<()> {
    let host = host(&mut caller)?;
    charge(&mut caller, cost::BALANCE)?;
    let address = address_at(&mut caller, address)?;
    let balance = host.balance(&address).map_err(|err| stop(&mut caller, err))?;

    write(&mut caller, out, &balance.to_le_bytes())
}

fn transfer(mut caller: Caller<'_, Run>, address: u32, amount: u32) -> Result<u32> {
    let host = host(&mut caller)?;
    charge(&mut caller, cost::TRANSFER)?;
    let to = address_at(&mut caller, address)?;
    let amount = amount_at(&mut caller, amount)?;

    match host.transfer(&to, amount) {
        Ok(()) => Ok(0),
        Err(err @ VmError::Storage(_)) => Err(stop(&mut caller, err)),
        Err(err) => {
            caller.data_mut().result = err.to_string().into_bytes();

            Ok(1)
        }
    }
}

fn resolve_name(mut caller: Caller<'_, Run>, name: u32, len: u32, out: u32) -> Result<u32> {
    let host = host(&mut caller)?;
    charge(&mut caller, cost::NAME + u64::from(len) / cost::COPY_BYTES_PER_FUEL)?;
    let name = read(&mut caller, name, len)?;
    // Text that is no name resolves to nothing, as in Lua.
    let Ok(name) = String::from_utf8(name) else {
        return Ok(0);
    };

    match host.resolve_name(&name) {
        Ok(Some(address)) => {
            write(&mut caller, out, &address.to_abi())?;

            Ok(1)
        }
        Ok(None) => Ok(0),
        Err(err) => Err(stop(&mut caller, err)),
    }
}

fn name_of(mut caller: Caller<'_, Run>, address: u32) -> Result<i32> {
    let host = host(&mut caller)?;
    charge(&mut caller, cost::NAME)?;
    let address = address_at(&mut caller, address)?;

    match host.name_of(&address) {
        Ok(Some(name)) => {
            let len = name.len() as i32;
            caller.data_mut().result = name.into_bytes();

            Ok(len)
        }
        Ok(None) => Ok(-1),
        Err(err) => Err(stop(&mut caller, err)),
    }
}

fn check_layout(mut caller: Caller<'_, Run>, from: u32, to: u32) -> Result<u32> {
    let host = host(&mut caller)?;
    charge(&mut caller, cost::CHECK_LAYOUT)?;
    let from = address_at(&mut caller, from)?;
    let to = address_at(&mut caller, to)?;

    match host.check_layout(&from, &to) {
        Ok(()) => Ok(0),
        Err(err @ VmError::Storage(_)) => Err(stop(&mut caller, err)),
        Err(err) => {
            caller.data_mut().result = err.to_string().into_bytes();

            Ok(1)
        }
    }
}

fn call(
    mut caller: Caller<'_, Run>,
    address: u32,
    value: u32,
    name: u32,
    name_len: u32,
    args: u32,
    args_len: u32,
) -> Result<u32> {
    let host = host(&mut caller)?;
    let target = address_at(&mut caller, address)?;
    let value = amount_at(&mut caller, value)?;
    let moved = u64::from(name_len) + u64::from(args_len);
    let transfer = if value == u256::ZERO { 0 } else { cost::TRANSFER };
    charge(&mut caller, cost::CALL + transfer + moved / cost::COPY_BYTES_PER_FUEL)?;
    let name = read(&mut caller, name, name_len)?;
    let args = read(&mut caller, args, args_len)?;
    let Ok(name) = String::from_utf8(name) else {
        return Err(stop(&mut caller, VmError::BadArgument("a function name is UTF-8".to_string())));
    };
    let args = match wincode::deserialize::<Vec<TxCallArgumentType>>(&args) {
        Ok(args) => args,
        Err(err) => return Err(stop(&mut caller, VmError::BadArgument(err.to_string()))),
    };
    let fuel = caller.get_fuel()?;
    let outcome = host.call(&target, &name, &args, value, fuel);
    caller.set_fuel(fuel.saturating_sub(outcome.fuel_used))?;
    let (status, result) = match outcome.result {
        Ok(returns) => (0, wincode::serialize(&returns).expect("return values always encode")),
        // The node failed; no contract may carry on past that.
        Err(VmError::Storage(err)) => return Err(stop(&mut caller, VmError::Storage(err))),
        Err(err) => (1, err.to_string().into_bytes()),
    };
    caller.data_mut().result = result;

    Ok(status)
}

fn evm_call(mut caller: Caller<'_, Run>, address: u32, value: u32, input: u32, input_len: u32) -> Result<u32> {
    let host = host(&mut caller)?;
    let target = address_at(&mut caller, address)?;
    let value = amount_at(&mut caller, value)?;
    let transfer = if value == u256::ZERO { 0 } else { cost::TRANSFER };
    charge(&mut caller, cost::CALL + transfer + u64::from(input_len) / cost::COPY_BYTES_PER_FUEL)?;
    let input = read(&mut caller, input, input_len)?;
    let fuel = caller.get_fuel()?;
    let outcome = host.evm_call(&target, &input, value, fuel);
    caller.set_fuel(fuel.saturating_sub(outcome.fuel_used))?;
    let (status, result) = match outcome.result {
        Ok(mut returned) => match returned.pop() {
            Some(TxCallArgumentType::Bytes(output)) => (0, output),
            _ => (1, b"the EVM returned no bytes".to_vec()),
        },
        Err(VmError::Storage(err)) => return Err(stop(&mut caller, VmError::Storage(err))),
        Err(err) => (1, err.to_string().into_bytes()),
    };
    caller.data_mut().result = result;

    Ok(status)
}

/// A signature or types, read as UTF-8, or why not.
fn abi_text(caller: &mut Caller<'_, Run>, ptr: u32, len: u32) -> Result<std::result::Result<String, VmError>> {
    Ok(String::from_utf8(read(caller, ptr, len)?).map_err(|_| VmError::BadArgument("a signature is UTF-8".to_string())))
}

fn abi_encode(mut caller: Caller<'_, Run>, signature: u32, signature_len: u32, values: u32, values_len: u32) -> Result<u32> {
    let host = host(&mut caller)?;
    charge(&mut caller, cost::ABI + (u64::from(signature_len) + u64::from(values_len)) / cost::COPY_BYTES_PER_FUEL)?;
    let signature = abi_text(&mut caller, signature, signature_len)?;
    let values = read(&mut caller, values, values_len)?;
    let encoded = signature.and_then(|signature| {
        let values = wincode::deserialize::<Vec<TxCallArgumentType>>(&values).map_err(|err| VmError::BadArgument(err.to_string()))?;

        encode(&host, &signature, &values)
    });
    let (status, result) = match encoded {
        Ok(bytes) => {
            charge(&mut caller, bytes.len() as u64 / cost::COPY_BYTES_PER_FUEL)?;
            (0, bytes)
        }
        Err(VmError::Storage(err)) => return Err(stop(&mut caller, VmError::Storage(err))),
        Err(err) => (1, err.to_string().into_bytes()),
    };
    caller.data_mut().result = result;

    Ok(status)
}

fn abi_decode(mut caller: Caller<'_, Run>, types: u32, types_len: u32, data: u32, data_len: u32) -> Result<u32> {
    let host = host(&mut caller)?;
    charge(&mut caller, cost::ABI + (u64::from(types_len) + u64::from(data_len)) / cost::COPY_BYTES_PER_FUEL)?;
    let types = abi_text(&mut caller, types, types_len)?;
    let data = read(&mut caller, data, data_len)?;
    let (status, result) = match types.and_then(|types| decode(&host, &types, &data)) {
        Ok(values) => (0, wincode::serialize(&values).expect("values always encode")),
        Err(VmError::Storage(err)) => return Err(stop(&mut caller, VmError::Storage(err))),
        Err(err) => (1, err.to_string().into_bytes()),
    };
    caller.data_mut().result = result;

    Ok(status)
}

#[cfg(feature = "evm")]
fn encode(host: &Host, signature: &str, values: &[TxCallArgumentType]) -> std::result::Result<Vec<u8>, VmError> {
    let (selector, types) = crate::abi::parse_signature(signature).map_err(VmError::BadArgument)?;

    if values.len() != types.len() {
        return Err(VmError::BadArgument(format!("{signature} takes {} values, not {}", types.len(), values.len())));
    }

    let values = types.iter().zip(values).map(|(ty, value)| crate::vm::evm::abi_value(ty, value, Some(host.state()))).collect::<std::result::Result<Vec<_>, _>>()?;
    let encoded = crate::abi::encode_values(&types, &values).map_err(VmError::BadArgument)?;

    Ok([selector.map(Vec::from).unwrap_or_default(), encoded].concat())
}

#[cfg(feature = "evm")]
fn decode(host: &Host, types: &str, data: &[u8]) -> std::result::Result<Vec<TxCallArgumentType>, VmError> {
    let text = match types.trim().contains('(') {
        true => types.to_string(),
        false => format!("({types})"),
    };
    let (selector, types) = crate::abi::parse_signature(&text).map_err(VmError::BadArgument)?;
    let data = match selector {
        Some(selector) => data.strip_prefix(&selector[..]).ok_or_else(|| VmError::BadArgument(format!("the calldata is not for {text}")))?,
        None => data,
    };
    let values = crate::abi::decode_values(&types, data).map_err(VmError::BadArgument)?;

    types.iter().zip(values).map(|(ty, value)| crate::vm::evm::native_from_abi(ty, value, host.state())).collect()
}

#[cfg(not(feature = "evm"))]
fn encode(_: &Host, _: &str, _: &[TxCallArgumentType]) -> std::result::Result<Vec<u8>, VmError> {
    Err(VmError::Runtime("this node was built without the EVM".to_string()))
}

#[cfg(not(feature = "evm"))]
fn decode(_: &Host, _: &str, _: &[u8]) -> std::result::Result<Vec<TxCallArgumentType>, VmError> {
    Err(VmError::Runtime("this node was built without the EVM".to_string()))
}

fn delegate(mut caller: Caller<'_, Run>, address: u32, selector: u64, args: u32, args_len: u32) -> Result<u32> {
    let host = host(&mut caller)?;
    let target = address_at(&mut caller, address)?;
    charge(&mut caller, cost::CALL + u64::from(args_len) / cost::COPY_BYTES_PER_FUEL)?;
    let args = read(&mut caller, args, args_len)?;
    let args = match wincode::deserialize::<Vec<TxCallArgumentType>>(&args) {
        Ok(args) => args,
        Err(err) => return Err(stop(&mut caller, VmError::BadArgument(err.to_string()))),
    };
    let fuel = caller.get_fuel()?;
    let outcome = host.delegate(&target, FunctionId::Selector(selector), &args, fuel);
    caller.set_fuel(fuel.saturating_sub(outcome.fuel_used))?;
    let (status, result) = match outcome.result {
        Ok(returns) => (0, wincode::serialize(&returns).expect("return values always encode")),
        Err(VmError::Storage(err)) => return Err(stop(&mut caller, VmError::Storage(err))),
        Err(err) => (1, err.to_string().into_bytes()),
    };
    caller.data_mut().result = result;

    Ok(status)
}

fn emit(mut caller: Caller<'_, Run>, name: u32, name_len: u32, data: u32, data_len: u32) -> Result<()> {
    let host = host(&mut caller)?;
    check_len(&mut caller, name_len, MAX_EVENT_NAME, "event names")?;
    check_len(&mut caller, data_len, MAX_EVENT_DATA, "event data")?;
    let size = u64::from(name_len) + u64::from(data_len);
    charge(&mut caller, cost::EVENT + size / cost::EVENT_BYTES_PER_FUEL)?;
    let name = read(&mut caller, name, name_len)?;
    let data = read(&mut caller, data, data_len)?;
    let Ok(name) = String::from_utf8(name) else {
        return Err(stop(&mut caller, VmError::BadArgument("an event name is UTF-8".to_string())));
    };
    let data = match wincode::deserialize::<Vec<TxCallArgumentType>>(&data) {
        Ok(data) => data,
        Err(err) => return Err(stop(&mut caller, VmError::BadArgument(err.to_string()))),
    };

    host.emit(&name, data).map(|_| ()).map_err(|err| stop(&mut caller, err))
}

fn refresh_token(mut caller: Caller<'_, Run>) -> Result<()> {
    let host = host(&mut caller)?;
    charge(&mut caller, cost::TOKEN_REFRESH)?;
    host.refresh_token();

    Ok(())
}

fn abort(mut caller: Caller<'_, Run>, message: u32, len: u32) -> Result<()> {
    let message = read(&mut caller, message, len.min(MAX_MESSAGE_LEN))?;
    let message = String::from_utf8_lossy(&message).into_owned();

    Err(stop(&mut caller, VmError::Runtime(format!("aborted: {message}"))))
}

/// Records why the module is being stopped and returns the trap that stops it.
fn stop(caller: &mut Caller<'_, Run>, err: VmError) -> wasmi::Error {
    let message = err.to_string();
    caller.data_mut().error.get_or_insert(err);

    wasmi::Error::new(message)
}

fn charge(caller: &mut Caller<'_, Run>, cost: u64) -> Result<()> {
    let fuel = caller.get_fuel()?;

    if fuel < cost {
        caller.set_fuel(0)?;

        return Err(stop(caller, VmError::OutOfFuel));
    }

    caller.set_fuel(fuel - cost)
}

fn host(caller: &mut Caller<'_, Run>) -> Result<Host> {
    match &caller.data().host {
        Some(host) => Ok(host.clone()),
        None => Err(stop(
            caller,
            VmError::Runtime("host functions are only available inside a call".to_string()),
        )),
    }
}

fn check_len(caller: &mut Caller<'_, Run>, len: u32, max: usize, what: &str) -> Result<()> {
    if len as usize > max {
        return Err(stop(caller, VmError::Runtime(format!("{what} are at most {max} bytes"))));
    }

    Ok(())
}

/// The 33-byte address at ptr, as Address::to_abi writes one.
fn address_at(caller: &mut Caller<'_, Run>, ptr: u32) -> Result<Address> {
    let bytes = read(caller, ptr, ADDRESS_LEN)?;

    Address::from_abi(&bytes).ok_or_else(|| stop(caller, VmError::BadArgument("not an address".to_string())))
}

/// The 32-byte little-endian amount at ptr.
fn amount_at(caller: &mut Caller<'_, Run>, ptr: u32) -> Result<u256> {
    let bytes: [u8; 32] = read(caller, ptr, 32)?.try_into().expect("read returns what it was asked for");

    Ok(u256::from_le_bytes(bytes))
}

fn memory(caller: &mut Caller<'_, Run>) -> Result<Memory> {
    match caller.get_export(MEMORY_EXPORT).and_then(Extern::into_memory) {
        Some(memory) => Ok(memory),
        None => Err(stop(caller, VmError::Runtime(format!("the module exports no {MEMORY_EXPORT}")))),
    }
}

fn read(caller: &mut Caller<'_, Run>, ptr: u32, len: u32) -> Result<Vec<u8>> {
    let memory = memory(caller)?;
    let start = ptr as usize;
    let bytes = start
        .checked_add(len as usize)
        .and_then(|end| memory.data(&*caller).get(start..end))
        .map(<[u8]>::to_vec);

    bytes.ok_or_else(|| stop(caller, VmError::Runtime("memory access out of bounds".to_string())))
}

fn write(caller: &mut Caller<'_, Run>, ptr: u32, bytes: &[u8]) -> Result<()> {
    let memory = memory(caller)?;
    let start = ptr as usize;
    let written = start
        .checked_add(bytes.len())
        .and_then(|end| memory.data_mut(&mut *caller).get_mut(start..end))
        .map(|target| target.copy_from_slice(bytes))
        .is_some();

    if !written {
        return Err(stop(caller, VmError::Runtime("memory access out of bounds".to_string())));
    }

    Ok(())
}
