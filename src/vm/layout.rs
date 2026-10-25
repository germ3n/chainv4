//! Storage layouts: the namespaces a contract's code says it keeps its state in, and what each
//! holds. A namespace is a map, keys of one kind or any, with values of one kind, or a record, only
//! the keys it declares, each with its kind. A kind may name the unit its values are in. Code that
//! declares any may reach nothing else: no other namespace or key, no map key that is not its
//! kind, no value that is not its kind, and plain storage only to read and clear it. So the
//! declaration is what the code does, and a new version must keep every namespace and key the old
//! one declared, with what it holds, or retire it on purpose.
//!
//! Layouts are kept in contract records, under the state root, so a layout that uses neither key
//! kinds nor units is kept as it was before either existed: Map and Record. TypedMap and TypedRecord
//! hold the rest.

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

/// The longest unit, in bytes.
pub const MAX_UNIT_LEN: usize = 32;

#[document(
    summary = "The namespaces a contract's code keeps its state in, as it declares them: --@namespace, --@key and --@retired lines in Lua, the chainv4.layout custom section in WebAssembly. Code that declares any may use no other namespace, no key a record does not declare, and no value that is not its kind, and may only read and clear plain storage; its own space stays free. A new version, swapped in by Tx::Upgrade or by a proxy repointing, must keep every namespace and key with the same kind, or retire it.",
    category = "VM",
    see_also = "Layout::keeps, Layout::reaches, Kind, Space, LAYOUT_SECTION",
    example = "--@namespace balances address-text -> decimal:wei\n--@namespace config\n--@key config owner address-hex\n--@retired config fee_rate\n--@retired fees",
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
    #[document(summary = "A map whose keys are of a kind, or whose values name a unit: `namespace NAME KEYKIND -> KIND`, `namespace NAME KIND:UNIT`. key is what every key is, None for any; value what every value is. One with neither a key kind nor a unit is a Map, as it was before either existed.")]
    #[wincode(tag = 3)]
    TypedMap { key: Option<Typed>, value: Typed },
    #[document(summary = "A record with a key whose value names a unit: only these keys, sorted, each with its kind and unit. One whose keys name none is a Record.")]
    #[wincode(tag = 4)]
    TypedRecord(Vec<TypedField>),
}

impl Shape {
    /// A map's key kind, if it has one, and its values' kind, without their units: what the node
    /// checks on every access.
    fn map_kinds(&self) -> Option<(Option<Kind>, Kind)> {
        match self {
            Shape::Map(kind) => Some((None, *kind)),
            Shape::TypedMap { key, value } => Some((key.as_ref().map(|key| key.kind), value.kind)),
            _ => None,
        }
    }

    #[document(parent = "Shape", summary = "A map's keys and values, units and all, whichever way it is kept; None for a record or a retired namespace.")]
    pub fn map(&self) -> Option<(Option<Typed>, Typed)> {
        match self {
            Shape::Map(kind) => Some((None, Typed::new(*kind))),
            Shape::TypedMap { key, value } => Some((key.clone(), value.clone())),
            _ => None,
        }
    }

    /// The kind of one of a record's keys: None when it declares no such key, Some(None) when the
    /// key is retired.
    fn record_kind(&self, key: &[u8]) -> Option<Option<Kind>> {
        match self {
            Shape::Record(fields) => {
                let at = fields.binary_search_by(|field| field.key.as_bytes().cmp(key)).ok()?;

                Some(fields[at].kind)
            }
            Shape::TypedRecord(fields) => {
                let at = fields.binary_search_by(|field| field.key.as_bytes().cmp(key)).ok()?;

                Some(fields[at].value.as_ref().map(|value| value.kind))
            }
            _ => None,
        }
    }

    #[document(parent = "Shape", summary = "A record's keys, sorted, each with its kind and unit, or None once retired, whichever way it is kept; None for a map or a retired namespace.")]
    pub fn record(&self) -> Option<Vec<TypedField>> {
        match self {
            Shape::Record(fields) => Some(
                fields
                    .iter()
                    .map(|field| TypedField {
                        key: field.key.clone(),
                        value: field.kind.map(Typed::new),
                    })
                    .collect(),
            ),
            Shape::TypedRecord(fields) => Some(fields.clone()),
            _ => None,
        }
    }
}

#[document(summary = "One key a record declares, with its value's kind and unit.", category = "VM", see_also = "Shape::TypedRecord")]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct TypedField {
    #[document(summary = "The key: 1 to 256 bytes, without spaces.")]
    pub key: String,
    #[document(summary = "What its value is; None once it is retired.")]
    pub value: Option<Typed>,
}

#[document(
    summary = "A kind, and the unit its values are in when the layout names one: `decimal:wei`, `u256:points`. The chain checks the kind on every write. The unit is the layout's word for what a value means, which no write can check: an upgrade must keep it, so code that changes what its numbers count says so, retiring the old and declaring the new.",
    category = "VM",
    see_also = "Kind, Layout::keeps",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct Typed {
    #[document(summary = "How the value is written.")]
    pub kind: Kind,
    #[document(summary = "What it counts or means: 1 to 32 letters, digits, - _ or ., or None.")]
    pub unit: Option<String>,
}

impl Typed {
    #[document(parent = "Typed", summary = "A kind with no unit.")]
    pub fn new(kind: Kind) -> Typed {
        Typed { kind, unit: None }
    }

    #[document(
        parent = "Typed",
        summary = "Reads a kind as a layout writes it, with its unit after a colon when it has one.",
        returns = "It, or why not: a kind there is none of, or a unit that is empty, too long or has other characters.",
    )]
    pub fn parse(word: &str) -> Result<Typed, String> {
        let (kind, unit) = match word.split_once(':') {
            Some((kind, unit)) => (kind, Some(unit)),
            None => (word, None),
        };
        let kind = kind_named(kind)?;

        if let Some(unit) = unit
            && (unit.is_empty() || unit.len() > MAX_UNIT_LEN || !unit.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte)))
        {
            return Err(format!("a unit is 1 to {MAX_UNIT_LEN} letters, digits, - _ or .: {word}"));
        }

        Ok(Typed {
            kind,
            unit: unit.map(str::to_string),
        })
    }

    #[document(
        parent = "Typed",
        summary = "Whether a new version's declaration keeps this one: the same kind, and the same unit, or any where this named none, as a unit may be named once and is then kept.",
    )]
    pub fn kept_by(&self, now: &Typed) -> bool {
        self.kind == now.kind && (self.unit.is_none() || self.unit == now.unit)
    }
}

impl fmt::Display for Typed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.unit {
            Some(unit) => write!(f, "{}:{unit}", self.kind),
            None => write!(f, "{}", self.kind),
        }
    }
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
    #[document(summary = "number: a whole number within 2^53 either way, in decimal digits after a minus for one below zero, no leading zeros: 5, -42, as Lua's Namespace:set_number and the SDK's set_number write one. One spelling for each number, so as a map's key it is one entry.")]
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
    #[document(summary = "address: 33 bytes, the kind's tag then its bytes, zeros after secp256k1's 20, as the SDK's Address::encode writes one.")]
    #[wincode(tag = 9)]
    Address,
    #[document(summary = "address-text: an address in its text form, such as ed25519:0x…, as Address:text writes one and address.parse reads it.")]
    #[wincode(tag = 10)]
    AddressText,
    #[document(summary = "address-hex: 0x and 64 hex digits, or 40 for secp256k1, as Address:hex writes one.")]
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
            Kind::Number => text().is_some_and(is_whole_number),
            Kind::Decimal => text().is_some_and(|text| {
                !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()) && u256::from_str_radix(text, 10).is_ok()
            }),
            Kind::Bool => matches!(value, [0] | [1]),
            Kind::I64 | Kind::U64 => value.len() == 8,
            Kind::U128 => value.len() == 16,
            Kind::U256 => value.len() == 32,
            Kind::Address => Address::from_abi(value).is_some(),
            Kind::AddressText => text().is_some_and(|text| text.parse::<Address>().is_ok()),
            Kind::AddressHex => text().is_some_and(|text| {
                text.strip_prefix("0x").is_some_and(|hex| matches!(hex.len(), 40 | 64) && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
            }),
        }
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Whether text is a whole number within 2^53 either way, spelled one way: digits with no leading
/// zero, after a minus for one below zero, and no -0.
fn is_whole_number(text: &str) -> bool {
    let digits = text.strip_prefix('-').unwrap_or(text);
    let canonical = !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()) && (digits == "0" || !digits.starts_with('0'));

    canonical && text != "-0" && text.parse::<i64>().is_ok_and(|number| number.unsigned_abs() <= 1 << 53)
}

#[document(
    summary = "One line of a layout as code writes it, before the lines are put together into a Layout.",
    category = "VM",
    see_also = "Layout::new",
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    #[document(summary = "`namespace NAME KIND`, or `namespace NAME KEYKIND -> KIND`: a map, of any key or of keys of a kind.")]
    Map(String, Option<Typed>, Typed),
    #[document(summary = "`namespace NAME`: a record, whose keys key entries declare.")]
    Record(String),
    #[document(summary = "`key NAME KEY KIND`: one of a record's keys.")]
    Key(String, String, Typed),
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
            ["namespace", name, kind] => Entry::Map(name.to_string(), None, Typed::parse(kind)?),
            ["namespace", name, key, "->", kind] => Entry::Map(name.to_string(), Some(Typed::parse(key)?), Typed::parse(kind)?),
            ["namespace", name] => Entry::Record(name.to_string()),
            ["key", name, key, kind] => Entry::Key(name.to_string(), key.to_string(), Typed::parse(kind)?),
            ["retired", name] => Entry::Retired(name.to_string()),
            ["retired", name, key] => Entry::RetiredKey(name.to_string(), key.to_string()),
            ["namespace", ..] => {
                return Err("a namespace is declared as `namespace NAME KIND`, `namespace NAME KEYKIND -> KIND` or `namespace NAME`".to_string());
            }
            ["key", ..] => return Err("a key is declared as `key NAME KEY KIND`".to_string()),
            ["retired", ..] => return Err("a namespace is retired as `retired NAME`, a key as `retired NAME KEY`".to_string()),
            _ => return Err(format!("unknown layout entry `{}`", text.trim())),
        };
        let (name, key) = match &entry {
            Entry::Map(name, _, _) | Entry::Record(name) | Entry::Retired(name) => (name, None),
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
                // As it was kept before key kinds and units, when it has neither.
                Entry::Map(name, None, Typed { kind, unit: None }) => (name, Shape::Map(kind)),
                Entry::Map(name, key, value) => (name, Shape::TypedMap { key, value }),
                Entry::Record(name) => (name, Shape::TypedRecord(Vec::new())),
                Entry::Retired(name) => (name, Shape::Retired),
                Entry::Key(name, key, value) => {
                    fields.push((name, TypedField { key, value: Some(value) }));

                    continue;
                }
                Entry::RetiredKey(name, key) => {
                    fields.push((name, TypedField { key, value: None }));

                    continue;
                }
            };

            if namespaces.iter().any(|declared| declared.name == name) {
                return Err(format!("namespace {name} is declared twice"));
            }

            namespaces.push(Declared { name, shape });
        }

        for (name, field) in fields {
            let Some(Declared { shape: Shape::TypedRecord(keys), .. }) = namespaces.iter_mut().find(|declared| declared.name == name) else {
                return Err(format!("key {} names namespace {name}, which is not declared as a record: `namespace {name}`", field.key));
            };

            if keys.iter().any(|known| known.key == field.key) {
                return Err(format!("key {} of namespace {name} is declared twice", field.key));
            }

            keys.push(field);
        }

        namespaces.sort_by(|a, b| a.name.cmp(&b.name));

        for declared in &mut namespaces {
            if let Shape::TypedRecord(keys) = &mut declared.shape {
                keys.sort_by(|a, b| a.key.cmp(&b.key));

                // As it was kept before units, when none of its keys names one.
                if keys.iter().all(|field| field.value.as_ref().is_none_or(|value| value.unit.is_none())) {
                    let plain = keys
                        .drain(..)
                        .map(|field| Field {
                            key: field.key,
                            kind: field.value.map(|value| value.kind),
                        })
                        .collect();
                    declared.shape = Shape::Record(plain);
                }
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
        summary = "Whether code with this layout may do this with a key in a namespace: anything, when it declares none. Otherwise the namespace must be declared, and a record's key too, and a map's key must be its key kind, whatever is done with it; what is retired may only be read and cleared; and a value written must be its kind.",
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
        let kind = match (&declared.shape, declared.shape.map_kinds()) {
            (Shape::Retired, _) => None,
            (_, Some((key_kind, kind))) => {
                // Read, written or cleared: a key of another form is an entry the code never meant.
                if let Some(key_kind) = key_kind
                    && !key_kind.holds(key)
                {
                    return Err(format!("namespace {shown} takes keys of {key_kind}, and {} is not one", printable(key)));
                }

                Some(kind)
            }
            (shape, None) => {
                let Some(kind) = shape.record_kind(key) else {
                    return Err(format!("key {} is not in namespace {shown}'s layout", String::from_utf8_lossy(key)));
                };

                match kind {
                    Some(kind) => Some(kind),
                    None if matches!(access, Access::Write(_)) => {
                        return Err(format!(
                            "key {} of namespace {shown} is retired: it can be read and cleared, not written",
                            String::from_utf8_lossy(key)
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
        summary = "Whether this layout, a new version's, keeps old: every namespace old declares is still declared, the same shape and kind, or retired; a map's key kind kept, or declared where it had none; every key of a record still declared with the same kind, or retired; every unit kept, or named where there was none; and what old retired stays retired. New namespaces and keys may come in. Anything keeps a layout that declares nothing.",
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
                (_, Shape::Retired) => {}
                (Shape::Retired, _) => return Err(format!("namespace {name} was retired, and stays retired")),
                (was, is) => match (was.map(), is.map(), was.record(), is.record()) {
                    (Some(was), Some(is), _, _) => keeps_map(name, was, is)?,
                    (Some(_), None, _, _) => {
                        return Err(format!("namespace {name} was a map, and is now a record: retire it and declare another"));
                    }
                    (None, Some(_), _, _) => {
                        return Err(format!("namespace {name} was a record, and is now a map: retire it and declare another"));
                    }
                    (None, None, Some(was), Some(is)) => keeps_keys(name, &was, &is)?,
                    (None, None, _, _) => unreachable!("a namespace not retired is a map or a record"),
                },
            }
        }

        Ok(())
    }
}

/// Whether a map, as a new version declares it, keeps the old version's: its values' kind and
/// unit, and its keys' kind, which may come in where it had none, and then stays.
fn keeps_map(name: &str, (was_key, was): (Option<Typed>, Typed), (is_key, is): (Option<Typed>, Typed)) -> Result<(), String> {
    if !was.kept_by(&is) {
        return Err(format!("namespace {name} held {was}, and now holds {is}: retire it and declare another"));
    }

    match (was_key, is_key) {
        (Some(was), None) => Err(format!("namespace {name} took keys of {was}, and now takes any: retire it and declare another")),
        (Some(was), Some(is)) if !was.kept_by(&is) => {
            Err(format!("namespace {name} took keys of {was}, and now takes {is}: retire it and declare another"))
        }
        _ => Ok(()),
    }
}

/// Whether a record's keys, as a new version declares them, keep the old version's.
fn keeps_keys(name: &str, old: &[TypedField], new: &[TypedField]) -> Result<(), String> {
    for before in old {
        let key = &before.key;
        let Some(now) = new.iter().find(|field| field.key == *key) else {
            return Err(format!("key {key} of namespace {name} is dropped: keep it, or retire it"));
        };

        match (&before.value, &now.value) {
            (None, Some(_)) => return Err(format!("key {key} of namespace {name} was retired, and stays retired")),
            (Some(was), Some(is)) if !was.kept_by(is) => {
                return Err(format!("key {key} of namespace {name} held {was}, and now holds {is}: retire it and declare another"));
            }
            _ => {}
        }
    }

    Ok(())
}

/// A key as an error shows it: as text when it is printable, else in hex.
fn printable(key: &[u8]) -> String {
    match std::str::from_utf8(key) {
        Ok(text) if !text.chars().any(char::is_control) => text.to_string(),
        _ => format!("0x{}", key.iter().map(|byte| format!("{byte:02x}")).collect::<String>()),
    }
}

/// One entry a line, as code declares them: each namespace, then a record's keys.
impl fmt::Display for Layout {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for declared in &self.namespaces {
            let name = &declared.name;
            let shape = &declared.shape;

            match (shape, shape.map(), shape.record()) {
                (Shape::Retired, _, _) => writeln!(f, "retired {name}")?,
                (_, Some((Some(key), value)), _) => writeln!(f, "namespace {name} {key} -> {value}")?,
                (_, Some((None, value)), _) => writeln!(f, "namespace {name} {value}")?,
                (_, None, Some(keys)) => {
                    writeln!(f, "namespace {name}")?;

                    for field in keys {
                        match field.value {
                            Some(value) => writeln!(f, "key {name} {} {value}", field.key)?,
                            None => writeln!(f, "retired {name} {}", field.key)?,
                        }
                    }
                }
                (_, None, None) => unreachable!("a namespace not retired is a map or a record"),
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
        let typed = |kind, unit: Option<&str>| Typed { kind, unit: unit.map(str::to_string) };

        assert_eq!(Entry::parse("  namespace  balances decimal ").unwrap(), Entry::Map("balances".into(), None, typed(Kind::Decimal, None)));
        assert_eq!(
            Entry::parse("namespace balances address-text -> decimal:wei").unwrap(),
            Entry::Map("balances".into(), Some(typed(Kind::AddressText, None)), typed(Kind::Decimal, Some("wei")))
        );
        assert_eq!(Entry::parse("namespace config").unwrap(), Entry::Record("config".into()));
        assert_eq!(Entry::parse("key config owner address-hex").unwrap(), Entry::Key("config".into(), "owner".into(), typed(Kind::AddressHex, None)));
        assert_eq!(Entry::parse("key config fee u64:basis-points").unwrap(), Entry::Key("config".into(), "fee".into(), typed(Kind::U64, Some("basis-points"))));
        assert_eq!(Entry::parse("retired config fee").unwrap(), Entry::RetiredKey("config".into(), "fee".into()));
        assert_eq!(Entry::parse("retired fees").unwrap(), Entry::Retired("fees".into()));

        for (entry, expected) in [
            ("namespace balances u512", "there is no kind u512; the kinds are bytes, text"),
            ("namespace a b c", "`namespace NAME KIND`, `namespace NAME KEYKIND -> KIND` or `namespace NAME`"),
            ("namespace a text => decimal", "`namespace NAME KEYKIND -> KIND`"),
            ("namespace a text -> u512", "there is no kind u512"),
            ("namespace a decimal:", "a unit is 1 to 32"),
            ("key a k decimal:two words", "`key NAME KEY KIND`"),
            ("key a k decimal:wei/s", "a unit is 1 to 32"),
            (&format!("namespace a decimal:{}", "u".repeat(33)), "a unit is 1 to 32"),
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
            (Kind::Number, &[b"5", b"0", b"-42", b"9007199254740992", b"-9007199254740992"], &[b"", b"-2.5", b"1e+20", b"inf", b"nan", b"05", b"-0", b"+5", b"0x10", b"9007199254740993"]),
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

    #[test]
    fn layouts_without_key_kinds_or_units_are_kept_as_they_were() {
        // Contract records sit under the state root: what code could declare before key kinds and
        // units must encode as it did, so they come out as Map and Record.
        let plain = layout(&["namespace balances decimal", "namespace config", "key config owner address-hex", "retired config fee"]);

        assert_eq!(plain.namespaces[0].shape, Shape::Map(Kind::Decimal));
        assert_eq!(
            plain.namespaces[1].shape,
            Shape::Record(vec![
                Field { key: "fee".into(), kind: None },
                Field { key: "owner".into(), kind: Some(Kind::AddressHex) },
            ])
        );

        let typed = layout(&["namespace balances address-text -> decimal", "namespace config", "key config owner address-hex", "key config fee u64:bps"]);

        assert!(matches!(typed.namespaces[0].shape, Shape::TypedMap { key: Some(_), .. }));
        assert!(matches!(typed.namespaces[1].shape, Shape::TypedRecord(_)));
        assert_eq!(Layout::read(typed.to_string().lines()).unwrap(), typed, "what it writes reads back");
        assert_eq!(
            typed.to_string(),
            "namespace balances address-text -> decimal\nnamespace config\nkey config fee u64:bps\nkey config owner address-hex\n"
        );

        let bytes = wincode::serialize(&typed).unwrap();

        assert_eq!(wincode::deserialize::<Layout>(&bytes).unwrap(), typed);
    }

    #[test]
    fn a_maps_keys_are_checked_whatever_is_done_with_them() {
        let who = Address::Ed25519([7; 32]);
        let (text, hex) = (who.to_string(), format!("0x{}", "07".repeat(32)));
        let layout = layout(&["namespace points address-text -> decimal:points", "namespace config", "key config cap decimal:points"]);

        assert_eq!(layout.reaches(b"points", text.as_bytes(), Access::Write(b"5")), Ok(()));
        assert_eq!(layout.reaches(b"points", text.as_bytes(), Access::Read), Ok(()));

        for access in [Access::Read, Access::Clear, Access::Write(b"5")] {
            assert_eq!(
                layout.reaches(b"points", hex.as_bytes(), access),
                Err(format!("namespace points takes keys of address-text, and {hex} is not one")),
                "who:hex() where the map takes who:text()"
            );
        }

        assert!(layout.reaches(b"points", b"\x01\x02", Access::Read).unwrap_err().contains("0x0102 is not one"));
        assert!(layout.reaches(b"points", text.as_bytes(), Access::Write(b"lots")).unwrap_err().contains("the value is not decimal"));
        assert_eq!(layout.reaches(b"config", b"cap", Access::Write(b"100")), Ok(()), "a unit is no encoding: it checks nothing");
        assert!(layout.reaches(b"config", b"limit", Access::Read).unwrap_err().contains("key limit is not in namespace config's layout"));
    }

    #[test]
    fn key_kinds_and_units_come_in_once_and_then_stay() {
        let v1 = layout(&["namespace points decimal", "namespace config", "key config cap decimal"]);
        let v2 = layout(&["namespace points address-text -> decimal:points", "namespace config", "key config cap decimal:points"]);

        assert_eq!(v2.keeps(&v1), Ok(()), "a map may take a key kind, and a kind a unit, where it had none");
        assert_eq!(v2.keeps(&v2), Ok(()));

        let broken = [
            (layout(&["namespace points decimal:points", "namespace config", "key config cap decimal:points"]), "namespace points took keys of address-text, and now takes any"),
            (layout(&["namespace points address-hex -> decimal:points", "namespace config", "key config cap decimal:points"]), "took keys of address-text, and now takes address-hex"),
            (layout(&["namespace points address-text -> decimal:coins", "namespace config", "key config cap decimal:points"]), "held decimal:points, and now holds decimal:coins"),
            (layout(&["namespace points address-text -> decimal", "namespace config", "key config cap decimal:points"]), "held decimal:points, and now holds decimal"),
            (layout(&["namespace points address-text -> u256:points", "namespace config", "key config cap decimal:points"]), "held decimal:points, and now holds u256:points"),
            (layout(&["namespace points address-text -> decimal:points", "namespace config", "key config cap decimal"]), "key cap of namespace config held decimal:points, and now holds decimal"),
            (layout(&["namespace points address-text -> decimal:points", "namespace config address-text -> decimal"]), "namespace config was a record, and is now a map"),
        ];

        for (v3, expected) in broken {
            assert!(v3.keeps(&v2).unwrap_err().contains(expected), "{v3}: {:?}", v3.keeps(&v2));
        }

        assert_eq!(layout(&["retired points", "retired config"]).keeps(&v2), Ok(()), "retiring forgets what it held");
    }
}
