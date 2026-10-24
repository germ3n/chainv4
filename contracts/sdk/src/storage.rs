//! The contract's own key-value storage, kept between calls. Keys are at most 256 bytes and
//! values at most 64 KiB. A call that fails leaves storage as it was.
//!
//! Besides the plain keys get and set reach, storage has namespaces, each with keys of its own,
//! and the contract's own space, which code it delegates to may read but never write.

use {
    crate::{abort, sys},
    alloc::{vec, vec::Vec},
};

/// The longest namespace name, in bytes.
pub const MAX_NAMESPACE_LEN: usize = 64;

/// Reads a value.
pub fn get(key: &[u8]) -> Option<Vec<u8>> {
    taken(unsafe { sys::storage_get(key.as_ptr(), key.len() as u32) })
}

/// Writes a value.
pub fn set(key: &[u8], value: &[u8]) {
    unsafe { sys::storage_set(key.as_ptr(), key.len() as u32, value.as_ptr(), value.len() as u32) };
}

/// Deletes a key.
pub fn delete(key: &[u8]) {
    unsafe { sys::storage_delete(key.as_ptr(), key.len() as u32) };
}

/// Reads a value stored as an i64, little-endian.
pub fn get_i64(key: &[u8]) -> Option<i64> {
    get(key)?.try_into().ok().map(i64::from_le_bytes)
}

/// A namespace, by its name: keys of its own, apart from plain storage and from every other
/// namespace, so two pieces of code keeping state on one contract cannot clash by picking the
/// same key. Code that names the same namespace shares it, code the contract delegates to
/// included. Aborts for a name that is empty or longer than MAX_NAMESPACE_LEN.
pub fn namespace(name: &[u8]) -> Namespace<'_> {
    if name.is_empty() || name.len() > MAX_NAMESPACE_LEN {
        abort("namespace names are 1 to 64 bytes");
    }

    Namespace { name }
}

/// The contract's own space: keys apart from plain storage and every namespace, which only this
/// contract's own code may write. Code it delegates to may read them but never change them, so a
/// proxy keeps its owner and implementation here.
pub fn own() -> Namespace<'static> {
    Namespace { name: &[] }
}

/// One part of the contract's storage with keys of its own, from namespace or own. Its methods
/// work like the plain ones, and cost the same, plus the name's bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Namespace<'a> {
    /// Empty for the contract's own space.
    name: &'a [u8],
}

impl Namespace<'_> {
    /// Reads a value.
    pub fn get(&self, key: &[u8]) -> Option<Vec<u8>> {
        taken(unsafe { sys::namespace_get(self.name.as_ptr(), self.name.len() as u32, key.as_ptr(), key.len() as u32) })
    }

    /// Writes a value. Writing the own space from code the contract delegates to aborts.
    pub fn set(&self, key: &[u8], value: &[u8]) {
        unsafe {
            sys::namespace_set(
                self.name.as_ptr(),
                self.name.len() as u32,
                key.as_ptr(),
                key.len() as u32,
                value.as_ptr(),
                value.len() as u32,
            )
        };
    }

    /// Deletes a key.
    pub fn delete(&self, key: &[u8]) {
        unsafe { sys::namespace_delete(self.name.as_ptr(), self.name.len() as u32, key.as_ptr(), key.len() as u32) };
    }

    /// Reads a value stored as an i64, little-endian.
    pub fn get_i64(&self, key: &[u8]) -> Option<i64> {
        self.get(key)?.try_into().ok().map(i64::from_le_bytes)
    }
}

/// The value a read left in the result buffer, given the length it returned.
fn taken(len: i32) -> Option<Vec<u8>> {
    if len < 0 {
        return None;
    }

    let mut value = vec![0; len as usize];
    unsafe { sys::result_read(value.as_mut_ptr()) };

    Some(value)
}
