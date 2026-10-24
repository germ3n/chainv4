//! Storage layouts: the namespaces a contract's code says it keeps its state in, and what each
//! holds. A namespace is a map, any key with values of one kind, or a record, only the keys it
//! declares, each with its kind. Code that declares any may reach nothing else: no other
//! namespace or key, no value that is not its kind, and plain storage only to read and clear it.
//! So the declaration is what the code does, and a new version must keep every namespace and key
//! the old one declared, with what it holds, or retire it on purpose.

use {
    super::MAX_NAMESPACE_LEN,
    crate::structs::address::Address,
    crate::structs::u256::u256,
    r#macro::document,
    std::fmt,
    wincode::{SchemaRead, SchemaWrite},
};

/// The longest key a record declares, in bytes, as storage keys may be.
const MAX_DECLARED_KEY_LEN: usize = super::MAX_KEY_LEN;

#[document(
    summary = "The namespaces a contract's code keeps its state in, as it declares them: --@namespace, --@key and --@retired lines in Lua, the chainv4.layout custom section in WebAssembly. Code that declares any may use no other namespace, no key a record does not declare, and no value that is not its kind, and may only read and clear plain storage; its own space stays free. A new version, swapped in by Tx::Upgrade or by a proxy repointing, must keep every namespace and key with the same kind, or retire it.",
    category = "VM",
    see_also = "Layout::keeps, Layout::reaches, Kind, Space, LAYOUT_SECTION",
    example = "--@namespace balances decimal\n--@namespace config\n--@key config owner address-hex\n--@retired config fee_rate\n--@retired fees",
)]
#[derive(Clone, Debug, Default, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct Layout {
    #[document(summary = "Every namespace declared, sorted by name. Empty for code that declares none, which may use any.")]
    pub namespaces: Vec<Declared>,
}

#[document(summary = "One namespace a layout declares.", category = "VM", see_also = "Layout")]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct Declared {
    #[document(summary = "The namespace's name: 1 to 64 bytes, without spaces.")]
    pub name: String,
    #[document(summary = "What it holds.")]
    pub shape: Shape,
}

#[document(summary = "What a declared namespace holds.", category = "VM", see_also = "Declared")]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub enum Shape {
    #[document(summary = "Any key, every value of this kind: `namespace NAME KIND`.")]
    #[wincode(tag = 0)]
    Map(Kind),
    #[document(summary = "Only these keys, sorted, each with its kind: `namespace NAME`, then `key NAME KEY KIND` for each.")]
    #[wincode(tag = 1)]
    Record(Vec<Field>),
    #[document(summary = "Nothing any more: `retired NAME`. It may be read and cleared, never written.")]
    #[wincode(tag = 2)]
    Retired,
}

#[document(summary = "One key a record namespace declares.", category = "VM", see_also = "Shape::Record")]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct Field {
    #[document(summary = "The key: 1 to 256 bytes, without spaces.")]
    pub key: String,
    #[document(summary = "What its value is; None once it is retired, `retired NAME KEY`: then it may be read and cleared, never written.")]
    pub kind: Option<Kind>,
}

#[document(
    summary = "What a declared value is: an encoding the chain checks on every write, so a value that is not its kind is refused. Each has the name a layout gives it.",
    category = "VM",
    see_also = "Layout",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub enum Kind {
    #[document(summary = "bytes: anything.")]
    #[wincode(tag = 0)]
    Bytes,
    #[document(summary = "text: UTF-8.")]
    #[wincode(tag = 1)]
    Text,
    #[document(summary = "number: a number in text, as Lua's tostring writes one: 5, -2.5, 1e+20, inf or nan.")]
    #[wincode(tag = 2)]
    Number,
    #[document(summary = "decimal: a whole number from 0 in decimal text, no larger than the largest u256, as tostring writes a U256.")]
    #[wincode(tag = 3)]
    Decimal,
    #[document(summary = "bool: one byte, 0 or 1.")]
    #[wincode(tag = 4)]
    Bool,
    #[document(summary = "i64: 8 bytes, little-endian.")]
    #[wincode(tag = 5)]
    I64,
    #[document(summary = "u64: 8 bytes, little-endian.")]
    #[wincode(tag = 6)]
    U64,
    #[document(summary = "u128: 16 bytes, little-endian.")]
    #[wincode(tag = 7)]
    U128,
    #[document(summary = "u256: 32 bytes, little-endian, as the SDK's U256 writes one.")]
    #[wincode(tag = 8)]
    U256,
    #[document(summary = "address: 33 bytes, the kind's tag then its 32, as the SDK's Address::encode writes one.")]
    #[wincode(tag = 9)]
    Address,
    #[document(summary = "address-text: an address in its text form, such as ed25519:0x…, as Address:text writes one and address.parse reads it.")]
    #[wincode(tag = 10)]
    AddressText,
    #[document(summary = "address-hex: 0x and 64 hex digits, as Address:hex writes one.")]
    #[wincode(tag = 11)]
    AddressHex,
}

impl Kind {
    /// Every kind, in the order the docs list them.
    pub const ALL: [Kind; 12] = [
        Kind::Bytes,
        Kind::Text,
        Kind::Number,
        Kind::Decimal,
        Kind::Bool,
        Kind::I64,
        Kind::U64,
        Kind::U128,
        Kind::U256,
        Kind::Address,
        Kind::AddressText,
        Kind::AddressHex,
    ];

    #[document(parent = "Kind", summary = "The name a layout gives it.")]
    pub fn name(self) -> &'static str {
        match self {
            Kind::Bytes => "bytes",
            Kind::Text => "text",
            Kind::Number => "number",
            Kind::Decimal => "decimal",
            Kind::Bool => "bool",
            Kind::I64 => "i64",
            Kind::U64 => "u64",
            Kind::U128 => "u128",
            Kind::U256 => "u256",
            Kind::Address => "address",
            Kind::AddressText => "address-text",
            Kind::AddressHex => "address-hex",
        }
    }

    #[document(parent = "Kind", summary = "The kind by its name, if there is one.")]
    pub fn from_name(name: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|kind| kind.name() == name)
    }

    #[document(parent = "Kind", summary = "Whether a value is of this kind.")]
    pub fn holds(self, value: &[u8]) -> bool {
        let text = || std::str::from_utf8(value).ok();

        match self {
            Kind::Bytes => true,
            Kind::Text => text().is_some(),
            Kind::Number => text().is_some_and(is_number),
            Kind::Decimal => text().is_some_and(|text| {
                !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()) && u256::from_str_radix(text, 10).is_ok()
            }),
            Kind::Bool => matches!(value, [0] | [1]),
            Kind::I64 | Kind::U64 => value.len() == 8,
            Kind::U128 => value.len() == 16,
            Kind::U256 => value.len() == 32,
            Kind::Address => value.len() == 33 && wincode::deserialize::<Address>(value).is_ok(),
            Kind::AddressText => text().is_some_and(|text| text.parse::<Address>().is_ok()),
            Kind::AddressHex => text().is_some_and(|text| {
                text.strip_prefix("0x").is_some_and(|hex| hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
            }),
        }
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Whether text is a number as Lua's tostring writes one: an optional minus, digits, an optional
/// fraction and an optional exponent; or inf or nan.
fn is_number(text: &str) -> bool {
    let unsigned = text.strip_prefix('-').unwrap_or(text);

    if matches!(unsigned, "inf" | "nan") {
        return true;
    }

    let (mantissa, exponent) = match unsigned.split_once(['e', 'E']) {
        Some((mantissa, exponent)) => (mantissa, Some(exponent)),
        None => (unsigned, None),
    };
    let (whole, fraction) = match mantissa.split_once('.') {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (mantissa, None),
    };
    let digits = |part: &str| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
    let exponent_ok = exponent.is_none_or(|exponent| digits(exponent.strip_prefix(['+', '-']).unwrap_or(exponent)));

    digits(whole) && fraction.is_none_or(digits) && exponent_ok
}

#[document(
    summary = "One line of a layout as code writes it, before the lines are put together into a Layout.",
    category = "VM",
    see_also = "Layout::new",
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    #[document(summary = "`namespace NAME KIND`: a map.")]
    Map(String, Kind),
    #[document(summary = "`namespace NAME`: a record, whose keys key entries declare.")]
    Record(String),
    #[document(summary = "`key NAME KEY KIND`: one of a record's keys.")]
    Key(String, String, Kind),
    #[document(summary = "`retired NAME`: a namespace no longer used.")]
    Retired(String),
    #[document(summary = "`retired NAME KEY`: one of a record's keys no longer used.")]
    RetiredKey(String, String),
}

impl Entry {
    #[document(
        parent = "Entry",
        summary = "Reads one entry as code writes it. Words are separated by spaces.",
        returns = "The entry, or what is wrong with it: an unknown word, the wrong number of words, a name or key too long, or a kind there is none of.",
    )]
    pub fn parse(text: &str) -> Result<Entry, String> {
        let words: Vec<&str> = text.split_whitespace().collect();
        let entry = match words[..] {
            ["namespace", name, kind] => Entry::Map(name.to_string(), kind_named(kind)?),
            ["namespace", name] => Entry::Record(name.to_string()),
            ["key", name, key, kind] => Entry::Key(name.to_string(), key.to_string(), kind_named(kind)?),
            ["retired", name] => Entry::Retired(name.to_string()),
            ["retired", name, key] => Entry::RetiredKey(name.to_string(), key.to_string()),
            ["namespace", ..] => return Err("a namespace is declared as `namespace NAME KIND` or `namespace NAME`".to_string()),
            ["key", ..] => return Err("a key is declared as `key NAME KEY KIND`".to_string()),
            ["retired", ..] => return Err("a namespace is retired as `retired NAME`, a key as `retired NAME KEY`".to_string()),
            _ => return Err(format!("unknown layout entry `{}`", text.trim())),
        };
        let (name, key) = match &entry {
            Entry::Map(name, _) | Entry::Record(name) | Entry::Retired(name) => (name, None),
            Entry::Key(name, key, _) | Entry::RetiredKey(name, key) => (name, Some(key)),
        };

        if name.len() > MAX_NAMESPACE_LEN {
            return Err(format!("namespace names are 1 to {MAX_NAMESPACE_LEN} bytes"));
        }

        if key.is_some_and(|key| key.len() > MAX_DECLARED_KEY_LEN) {
            return Err(format!("keys are 1 to {MAX_DECLARED_KEY_LEN} bytes"));
        }

        Ok(entry)
    }
}

fn kind_named(name: &str) -> Result<Kind, String> {
    Kind::from_name(name).ok_or_else(|| {
        let known: Vec<&str> = Kind::ALL.iter().map(|kind| kind.name()).collect();

        format!("there is no kind {name}; the kinds are {}", known.join(", "))
    })
}

#[document(
    summary = "What code does with a key, for Layout::reaches to judge.",
    category = "VM",
    see_also = "Layout::reaches",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access<'a> {
    #[document(summary = "Reads it.")]
    Read,
    #[document(summary = "Deletes it.")]
    Clear,
    #[document(summary = "Writes this value to it.")]
    Write(&'a [u8]),
}

impl Layout {
    #[document(
        parent = "Layout",
        summary = "Puts entries together into a layout, sorted by name and key, whatever order they came in.",
        returns = "The layout, or an error naming a namespace or key declared twice, or a key of a namespace that is not declared as a record.",
    )]
    pub fn new(entries: Vec<Entry>) -> Result<Layout, String> {
        let mut namespaces: Vec<Declared> = Vec::new();
        let mut fields = Vec::new();

        for entry in entries {
            let (name, shape) = match entry {
                Entry::Map(name, kind) => (name, Shape::Map(kind)),
                Entry::Record(name) => (name, Shape::Record(Vec::new())),
                Entry::Retired(name) => (name, Shape::Retired),
                Entry::Key(name, key, kind) => {
                    fields.push((name, Field { key, kind: Some(kind) }));

                    continue;
                }
                Entry::RetiredKey(name, key) => {
                    fields.push((name, Field { key, kind: None }));

                    continue;
                }
            };

            if namespaces.iter().any(|declared| declared.name == name) {
                return Err(format!("namespace {name} is declared twice"));
            }

            namespaces.push(Declared { name, shape });
        }

        for (name, field) in fields {
            let Some(Declared { shape: Shape::Record(keys), .. }) = namespaces.iter_mut().find(|declared| declared.name == name) else {
                return Err(format!("key {} names namespace {name}, which is not declared as a record: `namespace {name}`", field.key));
            };

            if keys.iter().any(|known| known.key == field.key) {
                return Err(format!("key {} of namespace {name} is declared twice", field.key));
            }

            keys.push(field);
        }

        namespaces.sort_by(|a, b| a.name.cmp(&b.name));

        for declared in &mut namespaces {
            if let Shape::Record(keys) = &mut declared.shape {
                keys.sort_by(|a, b| a.key.cmp(&b.key));
            }
        }

        Ok(Layout { namespaces })
    }

    #[document(
        parent = "Layout",
        summary = "Reads a layout from its entries, one each, as the chainv4.layout section and chainv4 layout hold them.",
        returns = "The layout, or what is wrong with an entry or with how they fit together.",
    )]
    pub fn read<'a>(entries: impl IntoIterator<Item = &'a str>) -> Result<Layout, String> {
        Layout::new(entries.into_iter().map(Entry::parse).collect::<Result<_, _>>()?)
    }

    #[document(parent = "Layout", summary = "Whether the code declares any namespace, and so may reach only what it declares.")]
    pub fn is_declared(&self) -> bool {
        !self.namespaces.is_empty()
    }

    #[document(parent = "Layout", summary = "The namespace by this name, if the layout declares it.")]
    pub fn get(&self, name: &[u8]) -> Option<&Declared> {
        self.namespaces
            .binary_search_by(|declared| declared.name.as_bytes().cmp(name))
            .ok()
            .map(|at| &self.namespaces[at])
    }

    #[document(
        parent = "Layout",
        summary = "Whether code with this layout may do this with a key in a namespace: anything, when it declares none. Otherwise the namespace must be declared, and a record's key too; what is retired may only be read and cleared; and a value written must be its kind.",
        params = {
            name = "The namespace's name.",
            key = "The key.",
            access = "What the code does with it.",
        },
        returns = "Why not, when it may not.",
    )]
    pub fn reaches(&self, name: &[u8], key: &[u8], access: Access<'_>) -> Result<(), String> {
        if !self.is_declared() {
            return Ok(());
        }

        let shown = String::from_utf8_lossy(name);
        let Some(declared) = self.get(name) else {
            return Err(format!("namespace {shown} is not in this code's layout"));
        };
        let kind = match &declared.shape {
            Shape::Retired => None,
            Shape::Map(kind) => Some(*kind),
            Shape::Record(keys) => {
                let Ok(at) = keys.binary_search_by(|field| field.key.as_bytes().cmp(key)) else {
                    return Err(format!("key {} is not in namespace {shown}'s layout", String::from_utf8_lossy(key)));
                };

                match keys[at].kind {
                    Some(kind) => Some(kind),
                    None if matches!(access, Access::Write(_)) => {
                        return Err(format!(
                            "key {} of namespace {shown} is retired: it can be read and cleared, not written",
                            keys[at].key
                        ));
                    }
                    None => return Ok(()),
                }
            }
        };

        match (kind, access) {
            (None, Access::Write(_)) => Err(format!("namespace {shown} is retired: it can be read and cleared, not written")),
            (Some(kind), Access::Write(value)) if !kind.holds(value) => Err(format!(
                "namespace {shown}, key {}: the value is not {kind}",
                String::from_utf8_lossy(key)
            )),
            _ => Ok(()),
        }
    }

    #[document(
        parent = "Layout",
        summary = "Whether code with this layout may do this with a plain key: anything, when it declares none; otherwise only read and clear it, since declared state lives in namespaces.",
        returns = "Why not, when it may not.",
    )]
    pub fn reaches_plain(&self, access: Access<'_>) -> Result<(), String> {
        match (self.is_declared(), access) {
            (true, Access::Write(_)) => Err(
                "code that declares a layout keeps its state in namespaces: plain storage can be read and cleared, not written"
                    .to_string(),
            ),
            _ => Ok(()),
        }
    }

    #[document(
        parent = "Layout",
        summary = "Whether this layout, a new version's, keeps old: every namespace old declares is still declared, the same shape and kind, or retired; every key of a record still declared with the same kind, or retired; and what old retired stays retired. New namespaces and keys may come in. Anything keeps a layout that declares nothing.",
        params = { old = "The layout of the code it replaces." },
        returns = "Why not, naming the first namespace or key that breaks it.",
        see_also = "Contracts::upgrade, Host::check_layout",
    )]
    pub fn keeps(&self, old: &Layout) -> Result<(), String> {
        for before in &old.namespaces {
            let name = &before.name;
            let Some(now) = self.get(name.as_bytes()) else {
                return Err(format!("namespace {name} is dropped: keep it, or retire it"));
            };

            match (&before.shape, &now.shape) {
                (Shape::Retired, Shape::Retired) | (Shape::Map(_) | Shape::Record(_), Shape::Retired) => {}
                (Shape::Retired, _) => return Err(format!("namespace {name} was retired, and stays retired")),
                (Shape::Map(was), Shape::Map(is)) if was != is => {
                    return Err(format!("namespace {name} held {was}, and now holds {is}: retire it and declare another"));
                }
                (Shape::Map(_), Shape::Map(_)) => {}
                (Shape::Map(_), Shape::Record(_)) => {
                    return Err(format!("namespace {name} was a map, and is now a record: retire it and declare another"));
                }
                (Shape::Record(_), Shape::Map(_)) => {
                    return Err(format!("namespace {name} was a record, and is now a map: retire it and declare another"));
                }
                (Shape::Record(was), Shape::Record(is)) => keeps_keys(name, was, is)?,
            }
        }

        Ok(())
    }
}

/// Whether a record's keys, as a new version declares them, keep the old version's.
fn keeps_keys(name: &str, old: &[Field], new: &[Field]) -> Result<(), String> {
    for before in old {
        let key = &before.key;
        let Some(now) = new.iter().find(|field| field.key == *key) else {
            return Err(format!("key {key} of namespace {name} is dropped: keep it, or retire it"));
        };

        match (before.kind, now.kind) {
            (None, Some(_)) => return Err(format!("key {key} of namespace {name} was retired, and stays retired")),
            (Some(was), Some(is)) if was != is => {
                return Err(format!("key {key} of namespace {name} held {was}, and now holds {is}: retire it and declare another"));
            }
            _ => {}
        }
    }

    Ok(())
}

/// One entry a line, as code declares them: each namespace, then a record's keys.
impl fmt::Display for Layout {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for declared in &self.namespaces {
            let name = &declared.name;

            match &declared.shape {
                Shape::Map(kind) => writeln!(f, "namespace {name} {kind}")?,
                Shape::Retired => writeln!(f, "retired {name}")?,
                Shape::Record(keys) => {
                    writeln!(f, "namespace {name}")?;

                    for field in keys {
                        match field.kind {
                            Some(kind) => writeln!(f, "key {name} {} {kind}", field.key)?,
                            None => writeln!(f, "retired {name} {}", field.key)?,
                        }
                    }
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(entries: &[&str]) -> Layout {
        Layout::read(entries.iter().copied()).unwrap()
    }

    #[test]
    fn entries_read_as_code_writes_them() {
        assert_eq!(Entry::parse("  namespace  balances decimal ").unwrap(), Entry::Map("balances".into(), Kind::Decimal));
        assert_eq!(Entry::parse("namespace config").unwrap(), Entry::Record("config".into()));
        assert_eq!(Entry::parse("key config owner address-hex").unwrap(), Entry::Key("config".into(), "owner".into(), Kind::AddressHex));
        assert_eq!(Entry::parse("retired config fee").unwrap(), Entry::RetiredKey("config".into(), "fee".into()));
        assert_eq!(Entry::parse("retired fees").unwrap(), Entry::Retired("fees".into()));

        for (entry, expected) in [
            ("namespace balances u512", "there is no kind u512; the kinds are bytes, text"),
            ("namespace a b c", "`namespace NAME KIND` or `namespace NAME`"),
            ("key config owner", "`key NAME KEY KIND`"),
            ("retired a b c", "`retired NAME KEY`"),
            ("reentrant", "unknown"),
            (&format!("retired {}", "n".repeat(65)), "1 to 64"),
            (&format!("retired n {}", "k".repeat(257)), "1 to 256"),
        ] {
            assert!(Entry::parse(entry).unwrap_err().contains(expected), "{entry}");
        }
    }

    #[test]
    fn entries_come_together_in_any_order() {
        let layout = layout(&["key config owner address-hex", "retired fees", "retired config fee", "namespace config", "namespace balances decimal"]);

        assert_eq!(
            layout.to_string(),
            "namespace balances decimal\nnamespace config\nretired config fee\nkey config owner address-hex\nretired fees\n"
        );
        assert_eq!(Layout::read(layout.to_string().lines()).unwrap(), layout, "what it writes reads back");

        for (entries, expected) in [
            (&["namespace a u256", "retired a"][..], "namespace a is declared twice"),
            (&["namespace a", "key a k u64", "retired a k"], "key k of namespace a is declared twice"),
            (&["namespace a u256", "key a k u64"], "not declared as a record"),
            (&["key a k u64"], "not declared as a record"),
        ] {
            assert!(Layout::read(entries.iter().copied()).unwrap_err().contains(expected), "{entries:?}");
        }
    }

    #[test]
    fn kinds_check_values() {
        let address = Address::Ed25519([7; 32]);
        let (text, encoded) = (address.to_string(), wincode::serialize(&address).unwrap());
        let (hex, bare) = (format!("0x{}", "aB".repeat(32)), "ab".repeat(32));
        // A kind, values it holds, and values it does not.
        type Case<'a> = (Kind, &'a [&'a [u8]], &'a [&'a [u8]]);
        let cases: &[Case] = &[
            (Kind::Bytes, &[b"", b"\xff"], &[]),
            (Kind::Text, &["héllo".as_bytes()], &[b"\xff"]),
            (Kind::Number, &[b"5", b"-2.5", b"1e+20", b"1E-3", b"inf", b"-nan", b"0.1"], &[b"", b"5.", b".5", b"1e", b"0x10", b"five", b"+5"]),
            (Kind::Decimal, &[b"0", b"1000000000000000000000000"], &[b"", b"-1", b"1.5", b"0x10", &[b'9'; 79]]),
            (Kind::Bool, &[b"\x00", b"\x01"], &[b"\x02", b"", b"true"]),
            (Kind::I64, &[&[0; 8]], &[&[0; 7]]),
            (Kind::U128, &[&[0; 16]], &[&[0; 8]]),
            (Kind::U256, &[&[0; 32]], &[&[0; 33]]),
            (Kind::Address, &[&encoded], &[&[0xff; 33], &[0; 32]]),
            (Kind::AddressText, &[text.as_bytes()], &[b"ed25519:0x12"]),
            (Kind::AddressHex, &[hex.as_bytes()], &[bare.as_bytes(), b"0x12"]),
        ];

        for (kind, good, bad) in cases {
            assert_eq!(Kind::from_name(kind.name()), Some(*kind));

            for value in *good {
                assert!(kind.holds(value), "{kind} should hold {value:?}");
            }

            for value in *bad {
                assert!(!kind.holds(value), "{kind} should not hold {value:?}");
            }
        }
    }

    #[test]
    fn declared_code_reaches_only_what_it_declares() {
        let layout = layout(&["namespace balances decimal", "namespace config", "key config paused bool", "retired config fee", "retired fees"]);

        assert_eq!(layout.reaches(b"balances", b"anyone", Access::Write(b"12")), Ok(()), "a map takes any key");
        assert!(layout.reaches(b"balances", b"anyone", Access::Write(b"12a")).unwrap_err().contains("namespace balances, key anyone: the value is not decimal"));
        assert_eq!(layout.reaches(b"config", b"paused", Access::Write(b"\x01")), Ok(()));
        assert!(layout.reaches(b"config", b"pasued", Access::Read).unwrap_err().contains("key pasued is not in namespace config's layout"));
        assert_eq!(layout.reaches(b"config", b"fee", Access::Clear), Ok(()));
        assert!(layout.reaches(b"config", b"fee", Access::Write(b"1")).unwrap_err().contains("key fee of namespace config is retired"));
        assert_eq!(layout.reaches(b"fees", b"k", Access::Read), Ok(()));
        assert!(layout.reaches(b"fees", b"k", Access::Write(b"1")).unwrap_err().contains("namespace fees is retired"));
        assert!(layout.reaches(b"other", b"k", Access::Read).unwrap_err().contains("not in this code's layout"));
        assert_eq!(layout.reaches_plain(Access::Read), Ok(()));
        assert_eq!(layout.reaches_plain(Access::Clear), Ok(()));
        assert!(layout.reaches_plain(Access::Write(b"1")).unwrap_err().contains("plain storage can be read and cleared"));

        let free = Layout::default();

        assert_eq!(free.reaches(b"other", b"k", Access::Write(b"\xff")), Ok(()), "code that declares nothing reaches anything");
        assert_eq!(free.reaches_plain(Access::Write(b"1")), Ok(()));
    }

    #[test]
    fn a_new_version_keeps_every_namespace_and_key_or_retires_it() {
        let v1 = layout(&["namespace balances decimal", "namespace config", "key config owner address-hex", "key config fee u64"]);
        let kept = [
            layout(&["namespace balances decimal", "namespace config", "key config owner address-hex", "key config fee u64", "key config paused bool", "namespace votes u64"]),
            layout(&["namespace balances decimal", "namespace config", "key config owner address-hex", "retired config fee"]),
            layout(&["retired balances", "retired config"]),
        ];

        for v2 in &kept {
            assert_eq!(v2.keeps(&v1), Ok(()), "{v2}");
        }

        assert_eq!(v1.keeps(&Layout::default()), Ok(()), "anything keeps no layout");

        let broken = [
            (layout(&["namespace config", "key config owner address-hex", "key config fee u64"]), "namespace balances is dropped"),
            (layout(&["namespace balances u256", "namespace config", "key config owner address-hex", "key config fee u64"]), "held decimal, and now holds u256"),
            (layout(&["namespace balances", "namespace config", "key config owner address-hex", "key config fee u64"]), "was a map, and is now a record"),
            (layout(&["namespace balances decimal", "namespace config text"]), "was a record, and is now a map"),
            (layout(&["namespace balances decimal", "namespace config", "key config owner address-hex"]), "key fee of namespace config is dropped"),
            (layout(&["namespace balances decimal", "namespace config", "key config owner address-text", "key config fee u64"]), "key owner of namespace config held address-hex, and now holds address-text"),
            (Layout::default(), "is dropped"),
        ];

        for (v2, expected) in broken {
            assert!(v2.keeps(&v1).unwrap_err().contains(expected), "{v2}");
        }

        let v2 = &kept[1];

        assert!(v1.keeps(v2).unwrap_err().contains("key fee of namespace config was retired, and stays retired"));
        assert!(v1.keeps(&kept[2]).unwrap_err().contains("namespace balances was retired, and stays retired"));
    }
}
