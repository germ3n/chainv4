//! The raw host functions, as the node defines them. Pointers and lengths are into this module's
//! memory. Use the safe wrappers in the crate root instead.

#[link(wasm_import_module = "chain")]
unsafe extern "C" {
    pub fn input_len() -> u32;
    pub fn input_read(ptr: *mut u8);
    pub fn output(ptr: *const u8, len: u32);
    pub fn result_len() -> u32;
    pub fn result_read(ptr: *mut u8);
    /// The value's length, or -1 when the key has none. The value lands in the result buffer.
    pub fn storage_get(key: *const u8, key_len: u32) -> i32;
    pub fn storage_set(key: *const u8, key_len: u32, value: *const u8, value_len: u32);
    pub fn storage_delete(key: *const u8, key_len: u32);
    /// As storage_get, in the namespace by that name; an empty name is the contract's own space.
    pub fn namespace_get(name: *const u8, name_len: u32, key: *const u8, key_len: u32) -> i32;
    pub fn namespace_set(name: *const u8, name_len: u32, key: *const u8, key_len: u32, value: *const u8, value_len: u32);
    pub fn namespace_delete(name: *const u8, name_len: u32, key: *const u8, key_len: u32);
    /// A page of plain keys from start, those with the prefix, into the result buffer: each its
    /// key's length, u32, the key, its value's length, u32, and the value. How many it gave.
    pub fn storage_scan(prefix: *const u8, prefix_len: u32, start: *const u8, start_len: u32, limit: u32) -> i32;
    /// As storage_scan, in the namespace by that name; an empty name is the contract's own space.
    pub fn namespace_scan(name: *const u8, name_len: u32, prefix: *const u8, prefix_len: u32, start: *const u8, start_len: u32, limit: u32) -> i32;
    /// Writes 33 bytes: the address's kind, then its 32 bytes.
    pub fn sender(ptr: *mut u8);
    pub fn contract(ptr: *mut u8);
    /// Writes 32 bytes, little-endian.
    pub fn value(ptr: *mut u8);
    /// The block's time, in milliseconds since the Unix epoch.
    pub fn block_time() -> u64;
    pub fn block_height() -> u64;
    /// Writes the digest and returns its length: 32, or 8 for XXH3.
    pub fn hash(algorithm: u32, data: *const u8, len: u32, out: *mut u8) -> u32;
    /// Writes 32 bytes, little-endian.
    pub fn balance(address: *const u8, out: *mut u8);
    /// 0 when the funds moved; 1 when the contract holds too little, with the reason in the
    /// result buffer.
    pub fn transfer(address: *const u8, amount: *const u8) -> u32;
    /// 0 when the call succeeded, with its encoded return values in the result buffer; 1 when
    /// it failed, with the error text there.
    pub fn call(
        address: *const u8,
        value: *const u8,
        name: *const u8,
        name_len: u32,
        args: *const u8,
        args_len: u32,
    ) -> u32;
    /// Runs another contract's function on this contract's storage; status and result as call.
    pub fn delegate(address: *const u8, selector: u64, args: *const u8, args_len: u32) -> u32;
    /// 0 when a clone of template was made, its address at out; 1 when not, with why in the
    /// result buffer.
    pub fn clone(template: *const u8, salt: *const u8, salt_len: u32, args: *const u8, args_len: u32, out: *mut u8) -> u32;
    /// The address this contract's clone with salt is, or would be, at out.
    pub fn cloned_at(salt: *const u8, salt_len: u32, out: *mut u8);
    /// 0 when to's code keeps from's storage layout; 1 when not, with why in the result buffer.
    pub fn check_layout(from: *const u8, to: *const u8) -> u32;
    /// Emits an event; data is an encoded list of values.
    pub fn emit(name: *const u8, name_len: u32, data: *const u8, data_len: u32);
    /// Asks the token registry to ask this contract's name, symbol and decimals again.
    pub fn refresh_token();
    /// 1 when the name resolves, with its 33-byte address written to out; 0 when it does not.
    pub fn resolve_name(name: *const u8, len: u32, out: *mut u8) -> u32;
    /// The primary name's length, with the name in the result buffer, or -1 for none.
    pub fn name_of(address: *const u8) -> i32;
    /// 0 when the EVM call succeeded, with what it returned in the result buffer; 1 when it
    /// reverted or failed, with why there.
    pub fn evm_call(address: *const u8, value: *const u8, input: *const u8, input_len: u32) -> u32;
    /// 0 with the ABI-encoded bytes in the result buffer; 1 with why a value does not fit.
    pub fn abi_encode(signature: *const u8, signature_len: u32, values: *const u8, values_len: u32) -> u32;
    /// 0 with the decoded values, an encoded list, in the result buffer; 1 with why not.
    pub fn abi_decode(types: *const u8, types_len: u32, data: *const u8, data_len: u32) -> u32;
    /// 1 when the signature over message is key's, by scheme's rules; 0 when not.
    #[allow(clippy::too_many_arguments)]
    pub fn verify(scheme: *const u8, scheme_len: u32, key: *const u8, key_len: u32, message: *const u8, message_len: u32, signature: *const u8, signature_len: u32) -> u32;
    /// 1 with the 33-byte compressed key that signed the 32-byte digest at out; 0 for none.
    pub fn recover(digest: *const u8, signature: *const u8, signature_len: u32, out: *mut u8) -> u32;
    /// The 33-byte address a key controls, at out.
    pub fn key_address(scheme: *const u8, scheme_len: u32, key: *const u8, key_len: u32, out: *mut u8);
    /// ripemd160, hash160 or sha256d of data at out; its length.
    pub fn crypto_hash(name: *const u8, name_len: u32, data: *const u8, len: u32, out: *mut u8) -> u32;
    /// 1 when the proof shows the entry holds value, or nothing when present is 0.
    #[allow(clippy::too_many_arguments)]
    pub fn check_state(
        root: *const u8,
        root_len: u32,
        column: *const u8,
        column_len: u32,
        key: *const u8,
        key_len: u32,
        value: *const u8,
        value_len: u32,
        present: u32,
        proof: *const u8,
        proof_len: u32,
    ) -> u32;
    /// 1 when data is the index-th of count leaves under root by path.
    #[allow(clippy::too_many_arguments)]
    pub fn check_merkle(root: *const u8, root_len: u32, data: *const u8, data_len: u32, index: u64, count: u64, path: *const u8, path_len: u32) -> u32;
    /// 1 when signature is the BLS aggregate of every 48-byte key in pubkeys over message, as
    /// Ethereum's FastAggregateVerify has it; 0 when not.
    pub fn fast_aggregate_verify(pubkeys: *const u8, pubkeys_len: u32, message: *const u8, message_len: u32, signature: *const u8, signature_len: u32) -> u32;
    /// The federation's length in the result buffer, encoded, or -1 for none.
    pub fn federation() -> i32;
    /// The chain's id: what its EVM answers CHAINID with.
    pub fn chain_id() -> u64;
    pub fn abort(message: *const u8, len: u32) -> !;
}
