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
    crate::vm::{FunctionId, Host, MAX_EVENT_NAME, MAX_NAMESPACE_LEN, Space, VmError},
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
    "storage_scan",
    "namespace_scan",
    "sender",
    "contract",
    "value",
    "block_time",
    "block_height",
    "hash",
    "balance",
    "transfer",
    "burn",
    "call",
    "delegate",
    "clone",
    "cloned_at",
    "check_layout",
    "emit",
    "refresh_token",
    "resolve_name",
    "name_of",
    "evm_call",
    "abi_encode",
    "abi_decode",
    "verify",
    "recover",
    "key_address",
    "crypto_hash",
    "check_state",
    "check_merkle",
    "fast_aggregate_verify",
    "federation",
    "eth_network",
    "chain_id",
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
    name = "storage_scan",
    summary = "A page of the contract's plain keys in byte order, as this call's own writes leave them, into the result buffer: for each, its key's length as a u32, the key, its value's length as a u32 and the value, lengths little-endian. To go on, start the next page at the last key with a 0 byte after it. Costs 100 fuel, plus 40 an entry and 1 per 16 bytes of what it reads and gives. A transaction that scanned runs again, when blocks run transactions side by side, if one before it wrote any key in what it saw.",
    params = {
        prefix = { ty = "i32", desc = "What every key given starts with." },
        prefix_len = { ty = "i32", desc = "Its length: at most 256; 0 for every key." },
        start = { ty = "i32", desc = "The first key it may give." },
        start_len = { ty = "i32", desc = "Its length: at most 257; 0 to start at the prefix." },
        limit = { ty = "i32", desc = "The most entries to give: 1 to 256." },
    },
    returns = { ty = "i32", desc = "How many entries it gave: fewer than limit at the end." },
    see_also = "chain.result_read, chain.namespace_scan",
)]
fn chain_storage_scan() {}

#[document(
    wasm,
    parent = "chain",
    name = "namespace_scan",
    summary = "A page of one of the contract's namespaces, or its own space, as storage_scan gives plain keys. A module that declares a storage layout scans only the namespaces it declares. Costs what storage_scan does, plus 1 per 16 bytes of the name.",
    params = {
        name = { ty = "i32", desc = "The namespace's name." },
        name_len = { ty = "i32", desc = "Its length: at most 64. 0 is the contract's own space." },
        prefix = { ty = "i32", desc = "What every key given starts with." },
        prefix_len = { ty = "i32", desc = "Its length: at most 256; 0 for every key." },
        start = { ty = "i32", desc = "The first key it may give." },
        start_len = { ty = "i32", desc = "Its length: at most 257; 0 to start at the prefix." },
        limit = { ty = "i32", desc = "The most entries to give: 1 to 256." },
    },
    returns = { ty = "i32", desc = "How many entries it gave." },
    see_also = "chain.result_read, chain.storage_scan",
)]
fn chain_namespace_scan() {}

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
    name = "block_time",
    summary = "The time of the block the call runs in, in milliseconds since the Unix epoch: the same on every node, so a contract can keep deadlines by it.",
    returns = { ty = "i64", desc = "The block's timestamp." },
)]
fn chain_block_time() {}

#[document(
    wasm,
    parent = "chain",
    name = "block_height",
    summary = "The height of the block the call runs in.",
    returns = { ty = "i64", desc = "The block's height." },
)]
fn chain_block_height() {}

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
    name = "burn",
    summary = "Burns coin from this contract's own balance, out of the supply. Kept only if the call succeeds. Costs what a transfer does.",
    params = { amount = { ty = "i32", desc = "How much: 32 bytes, a little-endian u256." } },
    returns = { ty = "i32", desc = "0 when it burned; 1 when the contract holds too little, with the reason in the result buffer." },
)]
fn chain_burn() {}

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
    name = "clone",
    summary = "Creates a contract running another's code, as a factory does: a copy of the template's code, deployed by this contract at the address cloned_at gives for the salt, whose init then runs with the arguments, this contract as its sender. Nothing of the template's storage or balance comes along; a clone that is a token goes in the token registry once the transaction succeeds. Kept only if init succeeds, and only if this call does. Costs 5000 fuel, 1 more per 16 bytes of the template's compiled code, the salt's hash and 1 per 64 bytes of arguments, on top of what init burns.",
    params = {
        template = { ty = "i32", desc = "The contract whose code to run: 33 bytes, Lua or WebAssembly." },
        salt = { ty = "i32", desc = "Any bytes this contract picks, fixing the address." },
        salt_len = { ty = "i32", desc = "Their length." },
        args = { ty = "i32", desc = "What init takes, encoded." },
        args_len = { ty = "i32", desc = "Their length." },
        out = { ty = "i32", desc = "Where the new contract's 33 bytes go." },
    },
    returns = { ty = "i32", desc = "0 when it was made; 1 when it failed, with the error text in the result buffer." },
    see_also = "chain.cloned_at, rust/Host::clone_contract",
)]
fn chain_clone() {}

#[document(
    wasm,
    parent = "chain",
    name = "cloned_at",
    summary = "Where this contract's clone with a salt is, or would be. Costs 20 fuel, 1 more per 64 bytes of salt.",
    params = {
        salt = { ty = "i32", desc = "The salt." },
        salt_len = { ty = "i32", desc = "Its length." },
        out = { ty = "i32", desc = "Where the 33 bytes of the address go." },
    },
    see_also = "chain.clone, rust/clone_address",
)]
fn chain_cloned_at() {}

#[document(
    wasm,
    parent = "chain",
    name = "verify",
    summary = "Whether a signature over a message is a key's, by its scheme's rules, as chainv4's keys sign. Costs 3,000 fuel, plus 1 per 64 bytes of message. Traps for a scheme the chain lacks or a key of the wrong length.",
    params = {
        scheme = { ty = "i32", desc = "The scheme's name, UTF-8: ed25519, edblake, secp256k1 or bitcoin." },
        scheme_len = { ty = "i32", desc = "Its length." },
        key = { ty = "i32", desc = "The key: 32 bytes, or 33 compressed for secp256k1 and Bitcoin." },
        key_len = { ty = "i32", desc = "Its length." },
        message = { ty = "i32", desc = "The bytes signed." },
        message_len = { ty = "i32", desc = "Their length." },
        signature = { ty = "i32", desc = "The signature, 64 bytes." },
        signature_len = { ty = "i32", desc = "Its length." },
    },
    returns = { ty = "i32", desc = "1 when it holds, 0 when it does not." },
    see_also = "rust/check_signature",
)]
fn chain_verify() {}

#[document(
    wasm,
    parent = "chain",
    name = "recover",
    summary = "The secp256k1 key that signed a 32-byte digest, as Ethereum's ecrecover finds it. Costs 3,000 fuel.",
    params = {
        digest = { ty = "i32", desc = "The digest, 32 bytes." },
        signature = { ty = "i32", desc = "r, s and v, 65 bytes, v 0, 1, 27 or 28." },
        signature_len = { ty = "i32", desc = "Its length." },
        out = { ty = "i32", desc = "Where the key goes: 33 bytes, compressed." },
    },
    returns = { ty = "i32", desc = "1 with the key at out, 0 for a signature that recovers none." },
    see_also = "rust/recover_key",
)]
fn chain_recover() {}

#[document(
    wasm,
    parent = "chain",
    name = "key_address",
    summary = "The account a key controls. Costs 100 fuel. Traps as verify does.",
    params = {
        scheme = { ty = "i32", desc = "The scheme's name, UTF-8." },
        scheme_len = { ty = "i32", desc = "Its length." },
        key = { ty = "i32", desc = "The key." },
        key_len = { ty = "i32", desc = "Its length." },
        out = { ty = "i32", desc = "Where the address goes: 33 bytes, as sender writes them." },
    },
    see_also = "rust/key_address",
)]
fn chain_key_address() {}

#[document(
    wasm,
    parent = "chain",
    name = "crypto_hash",
    summary = "ripemd160, hash160 or sha256d of some bytes. Costs what hash does. Traps for another name.",
    params = {
        name = { ty = "i32", desc = "The hash's name, UTF-8." },
        name_len = { ty = "i32", desc = "Its length." },
        data = { ty = "i32", desc = "The bytes." },
        len = { ty = "i32", desc = "Their length." },
        out = { ty = "i32", desc = "Where the digest goes: 32 bytes of room." },
    },
    returns = { ty = "i32", desc = "The digest's length: 20, or 32 for sha256d." },
    see_also = "rust/digest",
)]
fn chain_crypto_hash() {}

#[document(
    wasm,
    parent = "chain",
    name = "check_state",
    summary = "Whether a state proof shows a chain-state entry holds a value, or nothing, under a state root. Costs 200 fuel, plus 60 per 32 bytes of proof.",
    params = {
        root = { ty = "i32", desc = "The state root." },
        root_len = { ty = "i32", desc = "Its length, 32." },
        column = { ty = "i32", desc = "The column's name, UTF-8, such as accounts." },
        column_len = { ty = "i32", desc = "Its length." },
        key = { ty = "i32", desc = "The entry's key." },
        key_len = { ty = "i32", desc = "Its length." },
        value = { ty = "i32", desc = "What it holds." },
        value_len = { ty = "i32", desc = "Its length." },
        present = { ty = "i32", desc = "0 to check it holds nothing, value ignored; else 1." },
        proof = { ty = "i32", desc = "The proof, as GET /proofs encodes it." },
        proof_len = { ty = "i32", desc = "Its length." },
    },
    returns = { ty = "i32", desc = "1 when it shows that, else 0." },
    see_also = "rust/check_state",
)]
fn chain_check_state() {}

#[document(
    wasm,
    parent = "chain",
    name = "check_merkle",
    summary = "Whether some bytes are the index-th of count leaves under a Merkle root, by an audit path. Costs 200 fuel, plus 60 per 32 bytes of path.",
    params = {
        root = { ty = "i32", desc = "The root." },
        root_len = { ty = "i32", desc = "Its length, 32." },
        data = { ty = "i32", desc = "The leaf's bytes." },
        data_len = { ty = "i32", desc = "Their length." },
        index = { ty = "i64", desc = "Its place, from 0." },
        count = { ty = "i64", desc = "How many leaves." },
        path = { ty = "i32", desc = "The siblings, 32 bytes each, from the leaf up." },
        path_len = { ty = "i32", desc = "Its length." },
    },
    returns = { ty = "i32", desc = "1 when it is so, else 0." },
    see_also = "rust/check_merkle",
)]
fn chain_check_merkle() {}

#[document(
    wasm,
    parent = "chain",
    name = "fast_aggregate_verify",
    summary = "FastAggregateVerify as Ethereum checks its sync committees: whether a BLS signature is the aggregate of every key's over one message, by the ciphersuite BLS_SIG_BLS12381G2_XMD:SHA-256_SSWU_RO_POP_. Costs 103,000 fuel, plus 5,000 per public key and 1 per 64 bytes of message. Traps in a build without bls12-381.",
    params = {
        pubkeys = { ty = "i32", desc = "The public keys, 48 bytes each, compressed, back to back." },
        pubkeys_len = { ty = "i32", desc = "Their length." },
        message = { ty = "i32", desc = "The bytes signed." },
        message_len = { ty = "i32", desc = "Their length." },
        signature = { ty = "i32", desc = "The aggregate signature, 96 bytes, compressed." },
        signature_len = { ty = "i32", desc = "Its length." },
    },
    returns = { ty = "i32", desc = "1 when it holds; 0 when not, and for no keys, wrong lengths, a key or signature that is no point of its group or is the point at infinity, and keys that sum to infinity." },
    see_also = "rust/fast_aggregate_verify",
)]
fn chain_fast_aggregate_verify() {}

#[document(
    wasm,
    parent = "chain",
    name = "federation",
    summary = "The bridges' federation in force, into the result buffer: its version as a u64, its threshold and how many signers as u32s, little-endian, then each signer's 33-byte compressed secp256k1 key. Costs 200 fuel.",
    returns = { ty = "i32", desc = "The length in the result buffer, or -1 on a chain with no federation." },
    see_also = "chain.result_read, rust/Host::federation",
)]
fn chain_federation() {}

#[document(
    wasm,
    parent = "chain",
    name = "eth_network",
    summary = "The Ethereum beacon network anchoring's settings in force name, into the result buffer: its definition as SSZ, as ethlight's Network writes it, a preset's built-in one or the one governance carried. What an Ethereum light client contract follows, so a vote changes its fork schedule without a redeploy. Costs 200 fuel, as federation does.",
    returns = { ty = "i32", desc = "The length in the result buffer, or -1 while the settings name no Ethereum network." },
    see_also = "chain.result_read, rust/Host::eth_network",
)]
fn chain_eth_network() {}

#[document(
    wasm,
    parent = "chain",
    name = "chain_id",
    summary = "The chain's id: what its EVM answers CHAINID with and the bridges name it by, six bytes of its genesis id, and nothing a contract's owner sets. Costs 20 fuel.",
    returns = { ty = "i64", desc = "A whole number under 2^48, at least 1." },
    see_also = "rust/Host::chain_id",
)]
fn chain_chain_id() {}

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
    linker.func_wrap(IMPORT_MODULE, "storage_scan", storage_scan).expect(defined);
    linker.func_wrap(IMPORT_MODULE, "namespace_scan", namespace_scan).expect(defined);
    linker.func_wrap(IMPORT_MODULE, "sender", sender).expect(defined);
    linker.func_wrap(IMPORT_MODULE, "contract", contract).expect(defined);
    linker.func_wrap(IMPORT_MODULE, "value", value).expect(defined);
    linker.func_wrap(IMPORT_MODULE, "block_time", block_time).expect(defined);
    linker.func_wrap(IMPORT_MODULE, "block_height", block_height).expect(defined);
    linker.func_wrap(IMPORT_MODULE, "hash", hash).expect(defined);
    linker.func_wrap(IMPORT_MODULE, "balance", balance).expect(defined);
    linker.func_wrap(IMPORT_MODULE, "transfer", transfer).expect(defined);
    linker.func_wrap(IMPORT_MODULE, "burn", burn).expect(defined);
    linker.func_wrap(IMPORT_MODULE, "call", call).expect(defined);
    linker.func_wrap(IMPORT_MODULE, "delegate", delegate).expect(defined);
    linker.func_wrap(IMPORT_MODULE, "clone", clone).expect(defined);
    linker.func_wrap(IMPORT_MODULE, "cloned_at", cloned_at).expect(defined);
    linker.func_wrap(IMPORT_MODULE, "check_layout", check_layout).expect(defined);
    linker.func_wrap(IMPORT_MODULE, "emit", emit).expect(defined);
    linker.func_wrap(IMPORT_MODULE, "refresh_token", refresh_token).expect(defined);
    linker.func_wrap(IMPORT_MODULE, "resolve_name", resolve_name).expect(defined);
    linker.func_wrap(IMPORT_MODULE, "name_of", name_of).expect(defined);
    linker.func_wrap(IMPORT_MODULE, "evm_call", evm_call).expect(defined);
    linker.func_wrap(IMPORT_MODULE, "abi_encode", abi_encode).expect(defined);
    linker.func_wrap(IMPORT_MODULE, "abi_decode", abi_decode).expect(defined);
    linker.func_wrap(IMPORT_MODULE, "verify", verify).expect(defined);
    linker.func_wrap(IMPORT_MODULE, "recover", recover).expect(defined);
    linker.func_wrap(IMPORT_MODULE, "key_address", key_address).expect(defined);
    linker.func_wrap(IMPORT_MODULE, "crypto_hash", crypto_hash).expect(defined);
    linker.func_wrap(IMPORT_MODULE, "check_state", check_state).expect(defined);
    linker.func_wrap(IMPORT_MODULE, "check_merkle", check_merkle).expect(defined);
    linker.func_wrap(IMPORT_MODULE, "fast_aggregate_verify", fast_aggregate_verify).expect(defined);
    linker.func_wrap(IMPORT_MODULE, "federation", federation).expect(defined);
    linker.func_wrap(IMPORT_MODULE, "eth_network", eth_network).expect(defined);
    linker.func_wrap(IMPORT_MODULE, "chain_id", chain_id).expect(defined);
    linker.func_wrap(IMPORT_MODULE, "abort", abort).expect(defined);

    linker
}

fn input_len(caller: Caller<'_, Run>) -> Result<u32> {
    Ok(caller.data().input.len() as u32)
}

fn input_read(mut caller: Caller<'_, Run>, ptr: u32) -> Result<()> {
    let input = caller.data().input.clone();
    let fuel = { let costs = costs(&caller); input.len() as u64 / costs.copy_bytes_per_fuel };
    charge(&mut caller, fuel)?;

    write(&mut caller, ptr, &input)
}

fn output(mut caller: Caller<'_, Run>, ptr: u32, len: u32) -> Result<()> {
    let fuel = { let costs = costs(&caller); u64::from(len) / costs.copy_bytes_per_fuel };
    charge(&mut caller, fuel)?;
    let bytes = read(&mut caller, ptr, len)?;
    caller.data_mut().output = Some(bytes);

    Ok(())
}

fn result_len(caller: Caller<'_, Run>) -> Result<u32> {
    Ok(caller.data().result.len() as u32)
}

fn result_read(mut caller: Caller<'_, Run>, ptr: u32) -> Result<()> {
    let result = caller.data().result.clone();
    let fuel = { let costs = costs(&caller); result.len() as u64 / costs.copy_bytes_per_fuel };
    charge(&mut caller, fuel)?;

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

fn storage_scan(mut caller: Caller<'_, Run>, prefix: u32, prefix_len: u32, start: u32, start_len: u32, limit: u32) -> Result<i32> {
    scan_in(&mut caller, &Space::Plain, (prefix, prefix_len), (start, start_len), limit)
}

#[allow(clippy::too_many_arguments)]
fn namespace_scan(mut caller: Caller<'_, Run>, name: u32, name_len: u32, prefix: u32, prefix_len: u32, start: u32, start_len: u32, limit: u32) -> Result<i32> {
    let space = space(&mut caller, name, name_len)?;

    scan_in(&mut caller, &space, (prefix, prefix_len), (start, start_len), limit)
}

/// A page of a space into the result buffer, each entry its key's and value's lengths before
/// them; how many it gave.
fn scan_in(caller: &mut Caller<'_, Run>, space: &Space, (prefix, prefix_len): (u32, u32), (start, start_len): (u32, u32), limit: u32) -> Result<i32> {
    let host = host(caller)?;
    let longest = limits(caller).key_len as usize;
    check_len(caller, prefix_len, longest, "scan prefixes")?;
    check_len(caller, start_len, longest + 1, "scan starts")?;
    let asked = space.name().len() as u64 + u64::from(prefix_len) + u64::from(start_len);
    let fuel = { let costs = costs(caller); costs.storage_scan + asked / costs.storage_bytes_per_fuel };
    charge(caller, fuel)?;
    let prefix = read(caller, prefix, prefix_len)?;
    let start = match start_len {
        0 => prefix.clone(),
        _ => read(caller, start, start_len)?,
    };
    let found = host.storage_scan(space, &prefix, &start, limit as usize).map_err(|err| stop(caller, err))?;
    let moved: usize = found.iter().map(|(key, value)| key.len() + value.len()).sum();
    let fuel = { let costs = costs(caller); costs.storage_scan_entry * found.len() as u64 + moved as u64 / costs.storage_bytes_per_fuel };
    charge(caller, fuel)?;
    let mut result = Vec::with_capacity(moved + 8 * found.len());

    for (key, value) in &found {
        result.extend_from_slice(&(key.len() as u32).to_le_bytes());
        result.extend_from_slice(key);
        result.extend_from_slice(&(value.len() as u32).to_le_bytes());
        result.extend_from_slice(value);
    }

    caller.data_mut().result = result;

    Ok(found.len() as i32)
}

/// The space a namespace import names: an empty name is the contract's own.
fn space(caller: &mut Caller<'_, Run>, name: u32, name_len: u32) -> Result<Space> {
    check_len(caller, name_len, MAX_NAMESPACE_LEN, "namespace names")?;
    let name = read(caller, name, name_len)?;

    Ok(Space::from_name(&name))
}

fn get_in(caller: &mut Caller<'_, Run>, space: &Space, key: u32, key_len: u32) -> Result<i32> {
    let host = host(caller)?;
    check_len(caller, key_len, limits(caller).key_len as usize, "storage keys")?;
    let named = space.name().len() as u64 + u64::from(key_len);
    let fuel = { let costs = costs(caller); costs.storage_read + named / costs.storage_bytes_per_fuel };
    charge(caller, fuel)?;
    let key = read(caller, key, key_len)?;
    let value = host.storage_get(space, &key).map_err(|err| stop(caller, err))?;

    let Some(value) = value else {
        return Ok(-1);
    };

    let fuel = { let costs = costs(caller); value.len() as u64 / costs.storage_bytes_per_fuel };
    charge(caller, fuel)?;
    let len = value.len() as i32;
    caller.data_mut().result = value;

    Ok(len)
}

/// Writes the value at value's pointer and length, or deletes the key for None.
fn set_in(caller: &mut Caller<'_, Run>, space: &Space, key: u32, key_len: u32, value: Option<(u32, u32)>) -> Result<()> {
    let host = host(caller)?;
    check_len(caller, key_len, limits(caller).key_len as usize, "storage keys")?;
    let value_len = value.map_or(0, |(_, len)| len);
    check_len(caller, value_len, limits(caller).value_len as usize, "storage values")?;
    let moved = space.name().len() as u64 + u64::from(key_len) + u64::from(value_len);
    let fuel = { let costs = costs(caller); costs.storage_write + moved / costs.storage_bytes_per_fuel };
    charge(caller, fuel)?;
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

fn block_time(mut caller: Caller<'_, Run>) -> Result<u64> {
    Ok(host(&mut caller)?.context().block.time)
}

fn block_height(mut caller: Caller<'_, Run>) -> Result<u64> {
    Ok(host(&mut caller)?.context().block.height)
}

fn value(mut caller: Caller<'_, Run>, ptr: u32) -> Result<()> {
    let value = host(&mut caller)?.context().value;

    write(&mut caller, ptr, &value.to_le_bytes())
}

fn hash(mut caller: Caller<'_, Run>, algorithm: u32, data: u32, len: u32, out: u32) -> Result<u32> {
    let fuel = { let costs = costs(&caller); costs.hash + u64::from(len) / costs.hash_bytes_per_fuel };
    charge(&mut caller, fuel)?;
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
    let fuel = { let costs = costs(&caller); costs.balance };
    charge(&mut caller, fuel)?;
    let address = address_at(&mut caller, address)?;
    let balance = host.balance(&address).map_err(|err| stop(&mut caller, err))?;

    write(&mut caller, out, &balance.to_le_bytes())
}

fn transfer(mut caller: Caller<'_, Run>, address: u32, amount: u32) -> Result<u32> {
    let host = host(&mut caller)?;
    let fuel = { let costs = costs(&caller); costs.transfer };
    charge(&mut caller, fuel)?;
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

fn burn(mut caller: Caller<'_, Run>, amount: u32) -> Result<u32> {
    let host = host(&mut caller)?;
    let fuel = { let costs = costs(&caller); costs.transfer };
    charge(&mut caller, fuel)?;
    let amount = amount_at(&mut caller, amount)?;

    match host.burn(amount) {
        Ok(()) => Ok(0),
        Err(err @ VmError::Storage(_)) => Err(stop(&mut caller, err)),
        Err(err) => {
            caller.data_mut().result = err.to_string().into_bytes();

            Ok(1)
        }
    }
}

/// Text a contract hands over: what is not UTF-8 stops it.
fn text_at(caller: &mut Caller<'_, Run>, ptr: u32, len: u32, what: &str) -> Result<String> {
    let bytes = read(caller, ptr, len)?;

    String::from_utf8(bytes).map_err(|_| stop(caller, VmError::BadArgument(format!("{what} is not UTF-8"))))
}

#[allow(clippy::too_many_arguments)]
fn verify(mut caller: Caller<'_, Run>, scheme: u32, scheme_len: u32, key: u32, key_len: u32, message: u32, message_len: u32, signature: u32, signature_len: u32) -> Result<u32> {
    let fuel = { let costs = costs(&caller); costs.verify + u64::from(message_len) / costs.hash_bytes_per_fuel };
    charge(&mut caller, fuel)?;
    let scheme = text_at(&mut caller, scheme, scheme_len, "the scheme")?;
    let (key, message, signature) = (read(&mut caller, key, key_len)?, read(&mut caller, message, message_len)?, read(&mut caller, signature, signature_len)?);

    match crate::vm::crypto::check_signature(&scheme, &key, &message, &signature) {
        Ok(holds) => Ok(u32::from(holds)),
        Err(why) => Err(stop(&mut caller, VmError::BadArgument(why))),
    }
}

fn recover(mut caller: Caller<'_, Run>, digest: u32, signature: u32, signature_len: u32, out: u32) -> Result<u32> {
    let fuel = { let costs = costs(&caller); costs.recover };
    charge(&mut caller, fuel)?;
    let digest: [u8; 32] = read(&mut caller, digest, 32)?.try_into().expect("32 bytes read");
    let signature = read(&mut caller, signature, signature_len)?;

    #[cfg(feature = "secp256k1")]
    if let Some(key) = crate::vm::crypto::recover_key(&digest, &signature) {
        write(&mut caller, out, &key)?;

        return Ok(1);
    }

    #[cfg(not(feature = "secp256k1"))]
    let _ = (digest, signature, out);

    Ok(0)
}

fn key_address(mut caller: Caller<'_, Run>, scheme: u32, scheme_len: u32, key: u32, key_len: u32, out: u32) -> Result<()> {
    let fuel = { let costs = costs(&caller); costs.key_address };
    charge(&mut caller, fuel)?;
    let scheme = text_at(&mut caller, scheme, scheme_len, "the scheme")?;
    let key = read(&mut caller, key, key_len)?;

    match crate::vm::crypto::key_address(&scheme, &key) {
        Ok(address) => write(&mut caller, out, &address.to_abi()),
        Err(why) => Err(stop(&mut caller, VmError::BadArgument(why))),
    }
}

fn crypto_hash(mut caller: Caller<'_, Run>, name: u32, name_len: u32, data: u32, len: u32, out: u32) -> Result<u32> {
    let fuel = { let costs = costs(&caller); costs.hash + u64::from(len) / costs.hash_bytes_per_fuel };
    charge(&mut caller, fuel)?;
    let name = text_at(&mut caller, name, name_len, "the hash's name")?;
    let data = read(&mut caller, data, len)?;
    let Some(digest) = crate::vm::crypto::digest(&name, &data) else {
        return Err(stop(&mut caller, VmError::BadArgument(format!("{name} is not in this build"))));
    };
    write(&mut caller, out, &digest)?;

    Ok(digest.len() as u32)
}

#[allow(clippy::too_many_arguments)]
fn check_state(
    mut caller: Caller<'_, Run>,
    root: u32,
    root_len: u32,
    column: u32,
    column_len: u32,
    key: u32,
    key_len: u32,
    value: u32,
    value_len: u32,
    present: u32,
    proof: u32,
    proof_len: u32,
) -> Result<u32> {
    let fuel = { let costs = costs(&caller); costs.check_proof + costs.proof_sibling * (u64::from(proof_len) / 32) };
    charge(&mut caller, fuel)?;
    let hash = host(&mut caller)?.chain_hash();
    let column = text_at(&mut caller, column, column_len, "the column")?;
    let (root, key, value, proof) = (read(&mut caller, root, root_len)?, read(&mut caller, key, key_len)?, read(&mut caller, value, value_len)?, read(&mut caller, proof, proof_len)?);

    Ok(u32::from(crate::vm::crypto::check_state(hash, &root, &column, &key, (present != 0).then_some(value.as_slice()), &proof)))
}

#[allow(clippy::too_many_arguments)]
fn check_merkle(mut caller: Caller<'_, Run>, root: u32, root_len: u32, data: u32, data_len: u32, index: u64, count: u64, path: u32, path_len: u32) -> Result<u32> {
    let fuel = { let costs = costs(&caller); costs.check_proof + costs.proof_sibling * (u64::from(path_len) / 32) };
    charge(&mut caller, fuel)?;
    let hash = host(&mut caller)?.chain_hash();
    let (root, data, path) = (read(&mut caller, root, root_len)?, read(&mut caller, data, data_len)?, read(&mut caller, path, path_len)?);

    Ok(u32::from(crate::vm::crypto::check_merkle(hash, &root, &data, index, count, &path)))
}

fn fast_aggregate_verify(mut caller: Caller<'_, Run>, pubkeys: u32, pubkeys_len: u32, message: u32, message_len: u32, signature: u32, signature_len: u32) -> Result<u32> {
    let fuel = crate::vm::crypto::bls_fuel(&costs(&caller), u64::from(pubkeys_len), u64::from(message_len));
    charge(&mut caller, fuel)?;
    let (pubkeys, message, signature) = (read(&mut caller, pubkeys, pubkeys_len)?, read(&mut caller, message, message_len)?, read(&mut caller, signature, signature_len)?);

    match crate::vm::crypto::fast_aggregate_verify(&pubkeys, &message, &signature) {
        Ok(holds) => Ok(u32::from(holds)),
        Err(why) => Err(stop(&mut caller, VmError::BadArgument(why))),
    }
}

/// The federation in force into the result buffer: its version, u64, its threshold, u32, how many
/// signers, u32, all little-endian, then their 33-byte keys. Its length, or -1 for none.
fn federation(mut caller: Caller<'_, Run>) -> Result<i32> {
    let host = host(&mut caller)?;
    let fuel = { let costs = costs(&caller); costs.federation };
    charge(&mut caller, fuel)?;

    match host.federation() {
        Ok(Some(decided)) => {
            let mut bytes = decided.version.to_le_bytes().to_vec();
            bytes.extend(decided.federation.threshold.to_le_bytes());
            bytes.extend((decided.federation.signers.len() as u32).to_le_bytes());
            bytes.extend(decided.federation.signers.iter().flatten());
            let len = bytes.len() as i32;
            caller.data_mut().result = bytes;

            Ok(len)
        }
        Ok(None) => Ok(-1),
        Err(err) => Err(stop(&mut caller, err)),
    }
}

/// The Ethereum network anchoring names into the result buffer, as SSZ: its length, or -1 for none.
fn eth_network(mut caller: Caller<'_, Run>) -> Result<i32> {
    let host = host(&mut caller)?;
    let fuel = { let costs = costs(&caller); costs.federation };
    charge(&mut caller, fuel)?;

    match host.eth_network() {
        Ok(Some(bytes)) => {
            let len = bytes.len() as i32;
            caller.data_mut().result = bytes;

            Ok(len)
        }
        Ok(None) => Ok(-1),
        Err(err) => Err(stop(&mut caller, err)),
    }
}

fn chain_id(mut caller: Caller<'_, Run>) -> Result<u64> {
    let host = host(&mut caller)?;
    let fuel = { let costs = costs(&caller); costs.chain_id };
    charge(&mut caller, fuel)?;

    Ok(host.chain_id())
}

fn resolve_name(mut caller: Caller<'_, Run>, name: u32, len: u32, out: u32) -> Result<u32> {
    let host = host(&mut caller)?;
    let fuel = { let costs = costs(&caller); costs.name + u64::from(len) / costs.copy_bytes_per_fuel };
    charge(&mut caller, fuel)?;
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
    let fuel = { let costs = costs(&caller); costs.name };
    charge(&mut caller, fuel)?;
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
    let fuel = { let costs = costs(&caller); costs.check_layout };
    charge(&mut caller, fuel)?;
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
    let transfer = if value == u256::ZERO { 0 } else { costs(&caller).transfer };
    let fuel = { let costs = costs(&caller); costs.call + transfer + moved / costs.copy_bytes_per_fuel };
    charge(&mut caller, fuel)?;
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
    let transfer = if value == u256::ZERO { 0 } else { costs(&caller).transfer };
    let fuel = { let costs = costs(&caller); costs.call + transfer + u64::from(input_len) / costs.copy_bytes_per_fuel };
    charge(&mut caller, fuel)?;
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
    let fuel = { let costs = costs(&caller); costs.abi + (u64::from(signature_len) + u64::from(values_len)) / costs.copy_bytes_per_fuel };
    charge(&mut caller, fuel)?;
    let signature = abi_text(&mut caller, signature, signature_len)?;
    let values = read(&mut caller, values, values_len)?;
    let encoded = signature.and_then(|signature| {
        let values = wincode::deserialize::<Vec<TxCallArgumentType>>(&values).map_err(|err| VmError::BadArgument(err.to_string()))?;

        encode(&host, &signature, &values)
    });
    let (status, result) = match encoded {
        Ok(bytes) => {
            let fuel = { let costs = costs(&caller); bytes.len() as u64 / costs.copy_bytes_per_fuel };
            charge(&mut caller, fuel)?;
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
    let fuel = { let costs = costs(&caller); costs.abi + (u64::from(types_len) + u64::from(data_len)) / costs.copy_bytes_per_fuel };
    charge(&mut caller, fuel)?;
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
    let fuel = { let costs = costs(&caller); costs.call + u64::from(args_len) / costs.copy_bytes_per_fuel };
    charge(&mut caller, fuel)?;
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

fn clone(mut caller: Caller<'_, Run>, template: u32, salt: u32, salt_len: u32, args: u32, args_len: u32, out: u32) -> Result<u32> {
    let host = host(&mut caller)?;
    let template = address_at(&mut caller, template)?;
    let fuel = { let costs = costs(&caller); costs.clone + costs.hash + u64::from(salt_len) / costs.hash_bytes_per_fuel + u64::from(args_len) / costs.copy_bytes_per_fuel };
    charge(&mut caller, fuel)?;
    let salt = read(&mut caller, salt, salt_len)?;
    let args = read(&mut caller, args, args_len)?;
    let args = match wincode::deserialize::<Vec<TxCallArgumentType>>(&args) {
        Ok(args) => args,
        Err(err) => return Err(stop(&mut caller, VmError::BadArgument(err.to_string()))),
    };
    let fuel = caller.get_fuel()?;
    let outcome = host.clone_contract(&template, &salt, &args, fuel);
    caller.set_fuel(fuel.saturating_sub(outcome.fuel_used))?;

    match outcome.result {
        Ok(values) => match values.as_slice() {
            [TxCallArgumentType::Address(address)] => {
                write(&mut caller, out, &address.to_abi())?;

                Ok(0)
            }
            _ => Err(stop(&mut caller, VmError::Runtime("no address came back from a clone".to_string()))),
        },
        Err(VmError::Storage(err)) => Err(stop(&mut caller, VmError::Storage(err))),
        Err(err) => {
            caller.data_mut().result = err.to_string().into_bytes();

            Ok(1)
        }
    }
}

fn cloned_at(mut caller: Caller<'_, Run>, salt: u32, salt_len: u32, out: u32) -> Result<()> {
    let host = host(&mut caller)?;
    let fuel = { let costs = costs(&caller); costs.hash + u64::from(salt_len) / costs.hash_bytes_per_fuel };
    charge(&mut caller, fuel)?;
    let salt = read(&mut caller, salt, salt_len)?;

    write(&mut caller, out, &host.cloned_at(&salt).to_abi())
}

fn emit(mut caller: Caller<'_, Run>, name: u32, name_len: u32, data: u32, data_len: u32) -> Result<()> {
    let host = host(&mut caller)?;
    check_len(&mut caller, name_len, MAX_EVENT_NAME, "event names")?;
    let largest = host.limits().event_data as usize;
    check_len(&mut caller, data_len, largest, "event data")?;
    let size = u64::from(name_len) + u64::from(data_len);
    let fuel = { let costs = costs(&caller); costs.event + size / costs.event_bytes_per_fuel };
    charge(&mut caller, fuel)?;
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
    let fuel = { let costs = costs(&caller); costs.token_refresh };
    charge(&mut caller, fuel)?;
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

/// What host operations cost in the call's block: the defaults while no call is attached, as
/// while a deploy checks its module.
fn costs(caller: &Caller<'_, Run>) -> crate::params::vm::HostCosts {
    caller.data().host.as_ref().map_or(crate::params::vm::HostCosts::DEFAULT, Host::costs)
}

/// The limits contracts keep within in the call's block.
fn limits(caller: &Caller<'_, Run>) -> crate::params::vm::ContractLimits {
    caller.data().host.as_ref().map_or(crate::params::vm::ContractLimits::DEFAULT, Host::limits)
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
