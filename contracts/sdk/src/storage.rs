//! The contract's own key-value storage, kept between calls. Keys are at most 256 bytes and
//! values at most 64 KiB. A call that fails leaves storage as it was.
//!
//! Besides the plain keys get and set reach, storage has namespaces, each with keys of its own,
//! and the contract's own space, which code it delegates to may read but never write.
//!
//! A contract may declare its storage layout with [namespace!](crate::namespace) and
//! [retired!](crate::retired): the namespaces it keeps its state in, each a map, of any key or of
//! keys of a kind, with values of one kind, or a record of declared keys. A kind may name the unit
//! its values are in. Code that declares one may reach nothing else: no other namespace or key, no
//! map key or value that is not its kind, and plain storage only to read and delete. New code, by
//! an upgrade or a proxy repointing, must keep every namespace and key with the same kinds and
//! units, or retire it.
//!
//! Lua contracts write the text kinds and Rust the binary ones, but either reads and writes
//! both: here get_decimal, get_text, get_address_text and get_address_hex, and in Lua get_u256,
//! get_address and the like. So a version on one engine keeps what a version on the other kept.

use {
    crate::{Address, I256, U256, abort, sys},
    alloc::{string::String, vec, vec::Vec},
};

/// The longest namespace name, in bytes.
pub const MAX_NAMESPACE_LEN: usize = 64;

/// The longest key a record declares, in bytes.
pub const MAX_KEY_LEN: usize = 256;

/// The longest unit a kind names, in bytes.
pub const MAX_UNIT_LEN: usize = 32;

/// The kinds a layout's keys and values may be, each an encoding the node checks on every write,
/// and on every access for a map's keys. The typed methods on [Namespace] read and write each.
/// A kind may name a unit after a colon, `decimal:wei`, which the node keeps across upgrades.
///
/// | Kind | Encoding | Methods |
/// | --- | --- | --- |
/// | `bytes` | anything | `get`, `set` |
/// | `text` | UTF-8 | `get_text`, `set_text` |
/// | `number` | a whole number in digits, within 2^53 either way | `get_number`, `set_number` |
/// | `decimal` | a whole number from 0 in decimal text, up to the largest u256 | `get_decimal`, `set_decimal` |
/// | `bool` | one byte, 0 or 1 | `get_bool`, `set_bool` |
/// | `i64`, `u64` | 8 bytes, little-endian | `get_i64`, `get_u64`… |
/// | `i128`, `u128` | 16 bytes, little-endian | `get_i128`, `get_u128`… |
/// | `i256` | 32 bytes, little-endian, as [I256::to_le_bytes] | `get_i256`, `set_i256` |
/// | `u256` | 32 bytes, little-endian, as [U256::to_le_bytes] | `get_u256`, `set_u256` |
/// | `address` | 33 bytes, as [Address::encode] | `get_address`, `set_address` |
/// | `address-text` | an address in text: `ed25519:0x…`, as [Address::text] | `get_address_text`, `set_address_text` |
/// | `address-hex` | `0x` and 64 hex digits, or 40, as [Address::hex] | `get_address_hex`, `set_address_hex` |
pub const KINDS: [&str; 14] = [
    "bytes", "text", "number", "decimal", "bool", "i64", "u64", "i128", "u128", "i256", "u256", "address", "address-text",
    "address-hex",
];

/// Declares a namespace in the contract's storage layout, and a constant to use it by. A map
/// takes keys of one kind, or any key, and values of one kind; a record takes only the keys it
/// lists, each with its kind (see [KINDS]). Code that declares a layout may reach nothing past it,
/// and the chain refuses an upgrade whose code drops a namespace or key or changes its kinds or
/// units, as do proxies that check with [check_layout](crate::check_layout).
///
/// ```ignore
/// chainv4_sdk::namespace!(BALANCES, "balances", "address" => "u256:wei");
/// chainv4_sdk::namespace!(NOTES, "notes", "text");
/// chainv4_sdk::namespace!(CONFIG, "config", { "owner": "address", "paused": "bool" });
///
/// BALANCES.set_u256(&who.encode(), amount);
/// CONFIG.set_bool(b"paused", true);
/// ```
#[macro_export]
macro_rules! namespace {
    ($vis:vis $handle:ident, $name:literal, $key:literal => $kind:literal) => {
        $crate::namespace!(@handle $vis $handle, $name);
        $crate::namespace!(@kind $key);
        $crate::namespace!(@kind $kind);
        $crate::__layout_entry!(concat!("namespace ", $name, " ", $key, " -> ", $kind));
    };
    ($vis:vis $handle:ident, $name:literal, $kind:literal) => {
        $crate::namespace!(@handle $vis $handle, $name);
        $crate::namespace!(@kind $kind);
        $crate::__layout_entry!(concat!("namespace ", $name, " ", $kind));
    };
    ($vis:vis $handle:ident, $name:literal, { $($key:literal : $kind:literal),* $(,)? }) => {
        $crate::namespace!(@handle $vis $handle, $name);
        $crate::__layout_entry!(concat!("namespace ", $name));

        $(
            const _: () = assert!(
                $crate::storage::is_word($key, $crate::storage::MAX_KEY_LEN),
                "a record's key is 1 to 256 bytes without spaces"
            );
            $crate::namespace!(@kind $kind);
            $crate::__layout_entry!(concat!("key ", $name, " ", $key, " ", $kind));
        )*
    };
    (@handle $vis:vis $handle:ident, $name:literal) => {
        $vis const $handle: $crate::storage::Namespace<'static> = $crate::storage::Namespace::declared($name);

        const _: () = assert!(
            $crate::storage::is_word($name, $crate::storage::MAX_NAMESPACE_LEN),
            "a namespace's name is 1 to 64 bytes without spaces"
        );
    };
    (@kind $kind:literal) => {
        const _: () = assert!(
            $crate::storage::is_kind($kind),
            "not a kind, or a unit after it that is not 1 to 32 letters, digits, - _ or .: see chainv4_sdk::storage::KINDS"
        );
    };
}

/// Retires a namespace earlier code declared, or one key of a record, which this code no longer
/// keeps state in. It may still be read and deleted, as migrate does moving its state elsewhere,
/// but never written, and later code must keep it retired.
///
/// ```ignore
/// chainv4_sdk::retired!("fees");
/// chainv4_sdk::retired!("config", "fee_rate");
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
    ($name:literal, $key:literal) => {
        const _: () = assert!(
            $crate::storage::is_word($key, $crate::storage::MAX_KEY_LEN),
            "a record's key is 1 to 256 bytes without spaces"
        );

        $crate::__layout_entry!(concat!("retired ", $name, " ", $key));
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

    /// Reads a value stored as an i64, little-endian: the `i64` kind.
    pub fn get_i64(&self, key: &[u8]) -> Option<i64> {
        self.get(key)?.try_into().ok().map(i64::from_le_bytes)
    }

    /// Writes an i64, little-endian: the `i64` kind.
    pub fn set_i64(&self, key: &[u8], value: i64) {
        self.set(key, &value.to_le_bytes());
    }

    /// Reads a value stored as a u64, little-endian: the `u64` kind.
    pub fn get_u64(&self, key: &[u8]) -> Option<u64> {
        self.get(key)?.try_into().ok().map(u64::from_le_bytes)
    }

    /// Writes a u64, little-endian: the `u64` kind.
    pub fn set_u64(&self, key: &[u8], value: u64) {
        self.set(key, &value.to_le_bytes());
    }

    /// Reads a value stored as a U256, little-endian: the `u256` kind.
    pub fn get_u256(&self, key: &[u8]) -> Option<U256> {
        self.get(key)?.try_into().ok().map(U256::from_le_bytes)
    }

    /// Writes a U256, little-endian: the `u256` kind.
    pub fn set_u256(&self, key: &[u8], value: U256) {
        self.set(key, &value.to_le_bytes());
    }

    /// Reads an address stored as Address::encode writes it: the `address` kind.
    pub fn get_address(&self, key: &[u8]) -> Option<Address> {
        Address::decode(self.get(key)?.as_slice().try_into().ok()?)
    }

    /// Writes an address as Address::encode does: the `address` kind.
    pub fn set_address(&self, key: &[u8], value: &Address) {
        self.set(key, &value.encode());
    }

    /// Reads a value stored as one byte, 0 or 1: the `bool` kind.
    pub fn get_bool(&self, key: &[u8]) -> Option<bool> {
        match self.get(key)?.as_slice() {
            [0] => Some(false),
            [1] => Some(true),
            _ => None,
        }
    }

    /// Writes one byte, 0 or 1: the `bool` kind.
    pub fn set_bool(&self, key: &[u8], value: bool) {
        self.set(key, &[u8::from(value)]);
    }

    /// Reads a value stored as an i128, little-endian: the `i128` kind.
    pub fn get_i128(&self, key: &[u8]) -> Option<i128> {
        self.get(key)?.try_into().ok().map(i128::from_le_bytes)
    }

    /// Writes an i128, little-endian: the `i128` kind.
    pub fn set_i128(&self, key: &[u8], value: i128) {
        self.set(key, &value.to_le_bytes());
    }

    /// Reads a value stored as an I256, little-endian two's complement: the `i256` kind.
    pub fn get_i256(&self, key: &[u8]) -> Option<I256> {
        self.get(key)?.try_into().ok().map(I256::from_le_bytes)
    }

    /// Writes an I256, little-endian two's complement: the `i256` kind.
    pub fn set_i256(&self, key: &[u8], value: I256) {
        self.set(key, &value.to_le_bytes());
    }

    /// Reads a value stored as a u128, little-endian: the `u128` kind.
    pub fn get_u128(&self, key: &[u8]) -> Option<u128> {
        self.get(key)?.try_into().ok().map(u128::from_le_bytes)
    }

    /// Writes a u128, little-endian: the `u128` kind.
    pub fn set_u128(&self, key: &[u8], value: u128) {
        self.set(key, &value.to_le_bytes());
    }

    /// Reads UTF-8 text: the `text` kind, as Lua writes a string.
    pub fn get_text(&self, key: &[u8]) -> Option<String> {
        String::from_utf8(self.get(key)?).ok()
    }

    /// Writes UTF-8 text: the `text` kind.
    pub fn set_text(&self, key: &[u8], value: &str) {
        self.set(key, value.as_bytes());
    }

    /// Reads a whole number in digits: the `number` kind, as Lua writes one with set_number.
    pub fn get_number(&self, key: &[u8]) -> Option<i64> {
        core::str::from_utf8(&self.get(key)?).ok()?.parse().ok()
    }

    /// Writes a whole number in digits: the `number` kind, which holds those within 2^53 either
    /// way, as a Lua number does; past that the node refuses the write.
    pub fn set_number(&self, key: &[u8], value: i64) {
        self.set(key, alloc::format!("{value}").as_bytes());
    }

    /// Reads a whole number in decimal digits: the `decimal` kind, as Lua writes tostring(u256).
    pub fn get_decimal(&self, key: &[u8]) -> Option<U256> {
        U256::from_decimal(core::str::from_utf8(&self.get(key)?).ok()?)
    }

    /// Writes a whole number in decimal digits: the `decimal` kind.
    pub fn set_decimal(&self, key: &[u8], value: U256) {
        self.set(key, value.to_decimal().as_bytes());
    }

    /// Reads an address's text form, `ed25519:0x…`: the `address-text` kind, as Lua writes
    /// Address:text.
    pub fn get_address_text(&self, key: &[u8]) -> Option<Address> {
        Address::parse(core::str::from_utf8(&self.get(key)?).ok()?)
    }

    /// Writes an address's text form: the `address-text` kind.
    pub fn set_address_text(&self, key: &[u8], value: &Address) {
        self.set(key, value.text().as_bytes());
    }

    /// Reads `0x` and hex: the `address-hex` kind, as Lua writes Address:hex. It keeps no kind: 64
    /// digits read as Ed25519, 40 as secp256k1.
    pub fn get_address_hex(&self, key: &[u8]) -> Option<Address> {
        Address::from_hex(core::str::from_utf8(&self.get(key)?).ok()?)
    }

    /// Writes `0x` and hex: the `address-hex` kind.
    pub fn set_address_hex(&self, key: &[u8], value: &Address) {
        self.set(key, value.hex().as_bytes());
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

/// Whether text names one of the KINDS, with a unit after a colon or none.
#[doc(hidden)]
pub const fn is_kind(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut colon = 0;

    while colon < bytes.len() && bytes[colon] != b':' {
        colon += 1;
    }

    let (kind, unit) = bytes.split_at(colon);

    is_kind_name(kind) && (unit.is_empty() || is_unit(unit.split_at(1).1))
}

const fn is_kind_name(name: &[u8]) -> bool {
    let mut at = 0;

    while at < KINDS.len() {
        if same(KINDS[at].as_bytes(), name) {
            return true;
        }

        at += 1;
    }

    false
}

/// 1 to MAX_UNIT_LEN letters, digits, - _ or ., as the node reads a unit.
const fn is_unit(unit: &[u8]) -> bool {
    let mut at = 0;

    while at < unit.len() {
        let byte = unit[at];

        if !(byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_' || byte == b'.') {
            return false;
        }

        at += 1;
    }

    !unit.is_empty() && unit.len() <= MAX_UNIT_LEN
}

const fn same(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }

    let mut at = 0;

    while at < a.len() {
        if a[at] != b[at] {
            return false;
        }

        at += 1;
    }

    true
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
        assert!(is_word("counter-v1", MAX_NAMESPACE_LEN));
        assert!(!is_word("", MAX_NAMESPACE_LEN));
        assert!(!is_word("two words", MAX_NAMESPACE_LEN));
        assert!(!is_word("toolong", 6));
        assert!(is_kind("address-hex") && is_kind("u256") && is_kind("i128") && is_kind("i256:debt"));
        assert!(!is_kind("counter-v1") && !is_kind("u25"));
        assert!(is_kind("decimal:wei") && is_kind("u64:basis-points") && is_kind("u256:Token.v2_units"));
        assert!(!is_kind("decimal:") && !is_kind("decimal:wei/s") && !is_kind("u25:wei") && !is_kind(":wei"));
        assert!(!is_kind(&alloc::format!("decimal:{}", "u".repeat(33))));
        assert_eq!(entry::<8>("retired"), *b"retired\0");
    }
}
