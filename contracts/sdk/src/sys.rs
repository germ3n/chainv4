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
    /// Writes 33 bytes: the address's kind, then its 32 bytes.
    pub fn sender(ptr: *mut u8);
    pub fn contract(ptr: *mut u8);
    /// Writes 32 bytes, little-endian.
    pub fn value(ptr: *mut u8);
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
    /// 0 when to's code keeps from's storage layout; 1 when not, with why in the result buffer.
    pub fn check_layout(from: *const u8, to: *const u8) -> u32;
    /// Emits an event; data is an encoded list of values.
    pub fn emit(name: *const u8, name_len: u32, data: *const u8, data_len: u32);
    /// Asks the token registry to ask this contract's name, symbol and decimals again.
    pub fn refresh_token();
    pub fn abort(message: *const u8, len: u32) -> !;
}
