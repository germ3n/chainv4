//! The contract's own key-value storage, kept between calls. Keys are at most 256 bytes and
//! values at most 64 KiB. A call that fails leaves storage as it was.

use {
    crate::sys,
    alloc::{vec, vec::Vec},
};

/// Reads a value.
pub fn get(key: &[u8]) -> Option<Vec<u8>> {
    let len = unsafe { sys::storage_get(key.as_ptr(), key.len() as u32) };

    if len < 0 {
        return None;
    }

    let mut value = vec![0; len as usize];
    unsafe { sys::result_read(value.as_mut_ptr()) };

    Some(value)
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
