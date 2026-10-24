//! Write chainv4 contracts in Rust. A contract is a `cdylib` built for `wasm32-unknown-unknown`;
//! each function marked `#[export]` can be called by name from transactions and other contracts,
//! whichever engine runs them.
//!
//! ```ignore
//! use chainv4_sdk::{export, storage};
//!
//! #[export]
//! fn increment(by: Option<i64>) -> i64 {
//!     let count = storage::get_i64(b"count").unwrap_or(0) + by.unwrap_or(1);
//!     storage::set(b"count", &count.to_le_bytes());
//!     count
//! }
//! ```
//!
//! An exported function's parameters are read from the call's arguments, and what it returns
//! becomes the call's return values (see [args]). One that takes and returns nothing reads them
//! itself with [input] and sets them with [output].
//!
//! Every call runs in a fresh instance: nothing outside storage survives it. Floating-point
//! instructions are refused by the node, so contracts work in integers.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::{string::String, vec, vec::Vec};

pub mod args;
pub mod storage;
pub mod sys;
pub mod u256;
pub mod value;

pub use args::{FromValue, IntoOutput, IntoValue};
pub use chainv4_sdk_macros::export;
pub use u256::U256;
pub use value::{Address, AddressKind, Amount, Value, amount};

/// The arguments the call was made with. Aborts if the node sent something unreadable, which
/// it never does.
pub fn input() -> Vec<Value> {
    let mut bytes = vec![0; unsafe { sys::input_len() } as usize];
    unsafe { sys::input_read(bytes.as_mut_ptr()) };

    value::decode(&bytes).unwrap_or_else(|| abort("unreadable input"))
}

/// Sets what the call returns. The last call wins; a function that never calls it returns
/// nothing.
pub fn output(values: &[Value]) {
    let bytes = value::encode(values);
    unsafe { sys::output(bytes.as_ptr(), bytes.len() as u32) };
}

/// Who made the call: a transaction's sender, or the contract that called this one.
pub fn sender() -> Address {
    let mut bytes = [0; 33];
    unsafe { sys::sender(bytes.as_mut_ptr()) };

    Address::decode(&bytes).unwrap_or_else(|| abort("unreadable sender"))
}

/// This contract's own address.
pub fn this() -> Address {
    let mut bytes = [0; 33];
    unsafe { sys::contract(bytes.as_mut_ptr()) };

    Address::decode(&bytes).unwrap_or_else(|| abort("unreadable contract address"))
}

/// What the call sent, as a little-endian u256: the transaction's value, or what the calling
/// contract sent with call_with_value.
pub fn value() -> [u8; 32] {
    let mut bytes = [0; 32];
    unsafe { sys::value(bytes.as_mut_ptr()) };

    bytes
}

/// A hash algorithm the node computes for contracts. Each exists only when the node was built
/// with it; all are by default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Algorithm {
    Sha256 = 0,
    Blake3 = 1,
    Blake2s = 2,
    Blake2b = 3,
    /// Not cryptographic: the hash contract calls name functions by.
    Xxh3 = 4,
    Keccak256 = 5,
}

/// Hashes data on the node: 32 bytes, or 8 big-endian bytes for XXH3.
pub fn hash(algorithm: Algorithm, data: &[u8]) -> Vec<u8> {
    let mut out = [0; 32];
    let len = unsafe { sys::hash(algorithm as u32, data.as_ptr(), data.len() as u32, out.as_mut_ptr()) };

    out[..len as usize].to_vec()
}

/// What an address holds, as a little-endian u256.
pub fn balance(address: &Address) -> [u8; 32] {
    let address = address.encode();
    let mut out = [0; 32];
    unsafe { sys::balance(address.as_ptr(), out.as_mut_ptr()) };

    out
}

/// Sends funds from this contract's own balance. Kept only if the call succeeds.
///
/// Returns why not when the contract holds too little.
pub fn transfer(to: &Address, amount: [u8; 32]) -> Result<(), String> {
    let address = to.encode();

    match unsafe { sys::transfer(address.as_ptr(), amount.as_ptr()) } {
        0 => Ok(()),
        _ => Err(String::from_utf8_lossy(&result()).into_owned()),
    }
}

/// Calls an exported function on another contract, whichever engine runs it. Its writes are
/// kept only if it succeeds; its fuel comes out of this call's.
///
/// Returns its return values, or its error.
pub fn call(target: &Address, function: &str, args: &[Value]) -> Result<Vec<Value>, String> {
    call_with_value(target, [0; 32], function, args)
}

/// Like call, but also sends funds from this contract's balance, which the callee sees as its
/// value. They come back if the call fails.
pub fn call_with_value(target: &Address, value: [u8; 32], function: &str, args: &[Value]) -> Result<Vec<Value>, String> {
    let address = target.encode();
    let args = value::encode(args);
    let status = unsafe {
        sys::call(
            address.as_ptr(),
            value.as_ptr(),
            function.as_ptr(),
            function.len() as u32,
            args.as_ptr(),
            args.len() as u32,
        )
    };
    let result = result();

    match status {
        0 => Ok(value::decode(&result).unwrap_or_else(|| abort("unreadable return values"))),
        _ => Err(String::from_utf8_lossy(&result).into_owned()),
    }
}

/// Announces something for clients to follow. The event lands in the transaction's receipt only
/// if this call, and every call above it, succeeds. Names are 1 to 64 bytes.
pub fn emit(name: &str, data: &[Value]) {
    let data = value::encode(data);
    unsafe { sys::emit(name.as_ptr(), name.len() as u32, data.as_ptr(), data.len() as u32) };
}

/// Asks the chain's token registry to ask this contract's name(), symbol() and decimals() again,
/// as a deploy does, once the transaction succeeds; for a token whose answers just changed. An
/// answer that does not fit fails the transaction. Kept only if this call, and every call above
/// it, succeeds; does nothing for a contract that is not a token.
pub fn refresh_token() {
    unsafe { sys::refresh_token() }
}

/// Runs another contract's function as this contract's own: on this contract's storage and
/// balance, under this call's sender and value, with events in this contract's name. The EVM's
/// delegatecall, for proxies. Only functions the target exports; never its fallback or a hook.
///
/// Returns its return values, or its error.
pub fn delegate(target: &Address, selector: u64, args: &[Value]) -> Result<Vec<Value>, String> {
    let address = target.encode();
    let args = value::encode(args);
    let status = unsafe { sys::delegate(address.as_ptr(), selector, args.as_ptr(), args.len() as u32) };
    let result = result();

    match status {
        0 => Ok(value::decode(&result).unwrap_or_else(|| abort("unreadable return values"))),
        _ => Err(String::from_utf8_lossy(&result).into_owned()),
    }
}

/// Whether to's code keeps from's storage layout: every namespace from's code declares is
/// declared by to's too, with the same kind, or retired, the rule an upgrade's new code is held
/// to. What a proxy checks before it points at a new implementation.
///
/// Returns which namespace breaks it, or that either address holds no contract.
pub fn check_layout(from: &Address, to: &Address) -> Result<(), String> {
    let (from, to) = (from.encode(), to.encode());

    match unsafe { sys::check_layout(from.as_ptr(), to.as_ptr()) } {
        0 => Ok(()),
        _ => Err(String::from_utf8_lossy(&result()).into_owned()),
    }
}

/// The selector a call names a function by: XXH3-64 of its name, worked out on the node.
pub fn selector(name: &str) -> u64 {
    let digest = hash(Algorithm::Xxh3, name.as_bytes());

    u64::from_be_bytes(digest.try_into().unwrap_or_else(|_| abort("xxh3 gives 8 bytes")))
}

/// Stops the call with an error. Nothing it wrote is kept.
pub fn abort(message: &str) -> ! {
    unsafe { sys::abort(message.as_ptr(), message.len() as u32) }
}

/// The result buffer: what the last storage read or call left there.
fn result() -> Vec<u8> {
    let mut bytes = vec![0; unsafe { sys::result_len() } as usize];
    unsafe { sys::result_read(bytes.as_mut_ptr()) };

    bytes
}

/// Panics abort the call with their message.
#[cfg(all(target_arch = "wasm32", feature = "panic-handler"))]
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    abort(&alloc::format!("{}", info.message()))
}

/// Hands out memory from the end of the module's memory, growing it as needed, and never
/// frees: every call starts from a fresh instance, so nothing lives long enough to need it.
#[cfg(all(target_arch = "wasm32", feature = "allocator"))]
mod bump {
    use core::{
        alloc::{GlobalAlloc, Layout},
        arch::wasm32,
        sync::atomic::{AtomicUsize, Ordering},
    };

    const PAGE: usize = 64 * 1024;

    struct Bump {
        next: AtomicUsize,
    }

    #[global_allocator]
    static ALLOCATOR: Bump = Bump {
        next: AtomicUsize::new(0),
    };

    unsafe impl GlobalAlloc for Bump {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let mut next = self.next.load(Ordering::Relaxed);

            if next == 0 {
                next = wasm32::memory_size(0) * PAGE;
            }

            let start = (next + layout.align() - 1) & !(layout.align() - 1);
            let end = start + layout.size();
            let available = wasm32::memory_size(0) * PAGE;

            if end > available && wasm32::memory_grow(0, (end - available).div_ceil(PAGE)) == usize::MAX {
                return core::ptr::null_mut();
            }

            self.next.store(end, Ordering::Relaxed);

            start as *mut u8
        }

        unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {}
    }
}
