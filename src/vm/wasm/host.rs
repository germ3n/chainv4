//! The functions a module imports from "chain". Pointers and lengths are i32s into the module's
//! exported memory. Each function charges its fuel before doing the work, and anything that
//! stops the module records why in Run::error, which says more than the trap.

use {
    super::{IMPORT_MODULE, MEMORY_EXPORT, Run},
    crate::crypto::hasher::Hasher,
    crate::structs::address::Address,
    crate::structs::hash::HashType,
    crate::structs::tx::TxCallArgumentType,
    crate::vm::{Host, MAX_KEY_LEN, MAX_VALUE_LEN, VmError, cost},
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
    "sender",
    "contract",
    "value",
    "hash",
    "call",
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
    example = "use chainv4_sdk::{Value, export, input, output, storage};\n\n#[export]\nfn increment() {\n    let by = input().first().and_then(Value::as_i64).unwrap_or(1);\n    let count = storage::get_i64(b\"count\").unwrap_or(0) + by;\n    storage::set(b\"count\", &count.to_le_bytes());\n    output(&[Value::I64(count)]);\n}",
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
    summary = "Writes to the contract's own storage. Kept only if the call succeeds. Costs 500 fuel, plus 1 per 16 bytes of key and value.",
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
    name = "sender",
    summary = "Who made the call: a transaction's sender, or the contract that called this one.",
    params = { ptr = { ty = "i32", desc = "Where to write 33 bytes: the address's kind (0 Ed25519, 1 contract, 2 EdBlake), then its 32 bytes." } },
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
    summary = "What the call sent. Calls between contracts send nothing.",
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
    name = "call",
    summary = "Calls an exported function on another contract, whichever engine runs it. The callee sees this contract as its sender, receives no value, and burns fuel from this call's. Its writes are kept only if it succeeds. Costs 1000 fuel, plus 1 per 64 bytes of name and arguments.",
    params = {
        address = { ty = "i32", desc = "The contract to call: 33 bytes, as sender writes them." },
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
    linker.func_wrap(IMPORT_MODULE, "sender", sender).expect(defined);
    linker.func_wrap(IMPORT_MODULE, "contract", contract).expect(defined);
    linker.func_wrap(IMPORT_MODULE, "value", value).expect(defined);
    linker.func_wrap(IMPORT_MODULE, "hash", hash).expect(defined);
    linker.func_wrap(IMPORT_MODULE, "call", call).expect(defined);
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
    let host = host(&mut caller)?;
    check_len(&mut caller, key_len, MAX_KEY_LEN, "storage keys")?;
    charge(&mut caller, cost::STORAGE_READ + u64::from(key_len) / cost::STORAGE_BYTES_PER_FUEL)?;
    let key = read(&mut caller, key, key_len)?;
    let value = host.storage_get(&key).map_err(|err| stop(&mut caller, err))?;

    let Some(value) = value else {
        return Ok(-1);
    };

    charge(&mut caller, value.len() as u64 / cost::STORAGE_BYTES_PER_FUEL)?;
    let len = value.len() as i32;
    caller.data_mut().result = value;

    Ok(len)
}

fn storage_set(mut caller: Caller<'_, Run>, key: u32, key_len: u32, value: u32, value_len: u32) -> Result<()> {
    let host = host(&mut caller)?;
    check_len(&mut caller, key_len, MAX_KEY_LEN, "storage keys")?;
    check_len(&mut caller, value_len, MAX_VALUE_LEN, "storage values")?;
    let moved = u64::from(key_len) + u64::from(value_len);
    charge(&mut caller, cost::STORAGE_WRITE + moved / cost::STORAGE_BYTES_PER_FUEL)?;
    let key = read(&mut caller, key, key_len)?;
    let value = read(&mut caller, value, value_len)?;

    host.storage_set(&key, Some(&value)).map_err(|err| stop(&mut caller, err))
}

fn storage_delete(mut caller: Caller<'_, Run>, key: u32, key_len: u32) -> Result<()> {
    let host = host(&mut caller)?;
    check_len(&mut caller, key_len, MAX_KEY_LEN, "storage keys")?;
    charge(&mut caller, cost::STORAGE_WRITE + u64::from(key_len) / cost::STORAGE_BYTES_PER_FUEL)?;
    let key = read(&mut caller, key, key_len)?;

    host.storage_set(&key, None).map_err(|err| stop(&mut caller, err))
}

fn sender(mut caller: Caller<'_, Run>, ptr: u32) -> Result<()> {
    let sender = host(&mut caller)?.context().sender;

    write(&mut caller, ptr, &address_bytes(&sender))
}

fn contract(mut caller: Caller<'_, Run>, ptr: u32) -> Result<()> {
    let contract = host(&mut caller)?.context().contract;

    write(&mut caller, ptr, &address_bytes(&contract))
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

fn call(
    mut caller: Caller<'_, Run>,
    address: u32,
    name: u32,
    name_len: u32,
    args: u32,
    args_len: u32,
) -> Result<u32> {
    let host = host(&mut caller)?;
    let moved = u64::from(name_len) + u64::from(args_len);
    charge(&mut caller, cost::CALL + moved / cost::COPY_BYTES_PER_FUEL)?;
    let address = read(&mut caller, address, ADDRESS_LEN)?;
    let name = read(&mut caller, name, name_len)?;
    let args = read(&mut caller, args, args_len)?;
    let Ok(target) = wincode::deserialize::<Address>(&address) else {
        return Err(stop(&mut caller, VmError::BadArgument("not an address".to_string())));
    };
    let Ok(name) = String::from_utf8(name) else {
        return Err(stop(&mut caller, VmError::BadArgument("a function name is UTF-8".to_string())));
    };
    let args = match wincode::deserialize::<Vec<TxCallArgumentType>>(&args) {
        Ok(args) => args,
        Err(err) => return Err(stop(&mut caller, VmError::BadArgument(err.to_string()))),
    };
    let fuel = caller.get_fuel()?;
    let outcome = host.call(&target, &name, &args, fuel);
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

fn address_bytes(address: &Address) -> Vec<u8> {
    wincode::serialize(address).expect("addresses always encode")
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
