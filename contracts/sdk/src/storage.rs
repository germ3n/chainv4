//! The contract's own key-value storage, kept between calls. Keys are at most 256 bytes and
//! values at most 64 KiB. A call that fails leaves storage as it was.
//!
//! Besides the plain keys get and set reach, storage has namespaces, each with keys of its own,
//! and the contract's own space, which code it delegates to may read but never write.
//!
//! A contract may declare its storage layout with [namespace!](crate::namespace) and
//! [retired!](crate::retired): the namespaces it keeps its state in, and what each holds. Code
//! that declares one may use no other namespace, and new code, by an upgrade or a proxy
//! repointing, must keep every namespace with the same kind, or retire it.

use {
    crate::{abort, sys},
    alloc::{vec, vec::Vec},
};

/// The longest namespace name, in bytes.
pub const MAX_NAMESPACE_LEN: usize = 64;

/// The longest kind a layout gives a namespace, in bytes.
pub const MAX_KIND_LEN: usize = 64;

/// Declares a namespace in the contract's storage layout, and a constant to use it by: its name,
/// then what it holds, as one word, such as `"u256"` or `"counter-v1"`. Code that declares a
/// layout may use only the namespaces it declares. The chain refuses an upgrade whose code drops
/// one or changes its kind, and so do proxies that check with [check_layout](crate::check_layout).
///
/// ```ignore
/// chainv4_sdk::namespace!(BALANCES, "balances", "u256");
///
/// BALANCES.set(&who.encode(), &amount);
/// ```
#[macro_export]
macro_rules! namespace {
    ($vis:vis $handle:ident, $name:literal, $kind:literal) => {
        $vis const $handle: $crate::storage::Namespace<'static> = $crate::storage::Namespace::declared($name);

        const _: () = assert!(
            $crate::storage::is_word($name, $crate::storage::MAX_NAMESPACE_LEN),
            "a namespace's name is 1 to 64 bytes without spaces"
        );
        const _: () = assert!(
            $crate::storage::is_word($kind, $crate::storage::MAX_KIND_LEN),
            "a namespace's kind is 1 to 64 bytes without spaces"
        );

        $crate::__layout_entry!(concat!("namespace ", $name, " ", $kind));
    };
}

/// Retires a namespace earlier code declared, which this code no longer keeps state in. It may
/// still be read and cleared, as migrate does moving its state elsewhere, but never written, and
/// later code must keep it retired.
///
/// ```ignore
/// chainv4_sdk::retired!("fees");
/// ```
#[macro_export]
macro_rules! retired {
    ($name:literal) => {
        const _: () = assert!(
            $crate::storage::is_word($name, $crate::storage::MAX_NAMESPACE_LEN),
            "a namespace's name is 1 to 64 bytes without spaces"
        );

        $crate::__layout_entry!(concat!("retired ", $name));
    };
}

/// Puts one layout entry in the chainv4.layout custom section, which the linker joins into one.
#[doc(hidden)]
#[macro_export]
macro_rules! __layout_entry {
    ($text:expr) => {
        #[cfg(target_arch = "wasm32")]
        const _: () = {
            const TEXT: &str = $text;

            #[used]
            #[unsafe(link_section = "chainv4.layout")]
            static ENTRY: [u8; TEXT.len() + 1] = $crate::storage::entry(TEXT);
        };
    };
}

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

impl Namespace<'static> {
    /// The namespace namespace! declares, by a name it checked. Used by the code namespace!
    /// writes; not meant to be named.
    #[doc(hidden)]
    pub const fn declared(name: &'static str) -> Namespace<'static> {
        Namespace { name: name.as_bytes() }
    }
}

/// Whether text is 1 to max bytes without whitespace, as layout entries need their words.
#[doc(hidden)]
pub const fn is_word(text: &str, max: usize) -> bool {
    let bytes = text.as_bytes();
    let mut at = 0;

    while at < bytes.len() {
        if bytes[at].is_ascii_whitespace() {
            return false;
        }

        at += 1;
    }

    !bytes.is_empty() && bytes.len() <= max
}

/// A layout entry as the chainv4.layout section holds it: its text, then a zero byte.
#[doc(hidden)]
pub const fn entry<const N: usize>(text: &str) -> [u8; N] {
    let bytes = text.as_bytes();
    let mut out = [0; N];
    let mut at = 0;

    while at < bytes.len() {
        out[at] = bytes[at];
        at += 1;
    }

    out
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_entries_are_words_ending_in_a_zero_byte() {
        assert!(is_word("counter-v1", MAX_KIND_LEN));
        assert!(!is_word("", MAX_KIND_LEN));
        assert!(!is_word("two words", MAX_KIND_LEN));
        assert!(!is_word("toolong", 6));
        assert_eq!(entry::<8>("retired"), *b"retired\0");
    }
}
