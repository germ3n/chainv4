use {
    super::accounts::refused,
    super::address::LuaAddress,
    super::integers::{LuaInt, Prim, exact_as},
    crate::structs::address::Address,
    crate::structs::u256::{i256, u256},
    crate::vm::lvm::{charge, costs},
    crate::vm::{Host, MAX_NAMESPACE_LEN, Space},
    std::collections::VecDeque,
    mlua::{Error, Lua, UserData, UserDataMethods, Value},
    r#macro::document,
};

/// The largest whole number a Lua number holds exactly: 2^53.
const EXACT: i64 = 1 << 53;

/// How many entries an iterator asks for at a time: a loop that breaks early pays for, and
/// depends on, no more than the page it stopped in.
const ITER_PAGE: usize = 64;

/// One part of the contract's storage with keys of its own: a namespace, from storage.namespace,
/// or the contract's own space, storage.own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LuaNamespace(pub Space);

impl UserData for LuaNamespace {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("get", |lua, this, key: mlua::String| storage_get(lua, &this.0, &key.as_bytes()));
        methods.add_method("iter", |lua, this, (prefix, start): (Option<mlua::String>, Option<mlua::String>)| {
            storage_iter(lua, this.0.clone(), prefix.map(|prefix| prefix.as_bytes().to_vec()), start.map(|start| start.as_bytes().to_vec()))
        });
        methods.add_method("set", |lua, this, (key, value): (mlua::String, Option<mlua::String>)| {
            let value = value.as_ref().map(|value| value.as_bytes());

            storage_set(lua, &this.0, &key.as_bytes(), value.as_deref())
        });

        // The number kind, whole numbers written one way, which tostring does not keep to: 1e15
        // comes out as 1e+15.
        methods.add_method("get_number", |lua, this, key: mlua::String| {
            let Some(value) = storage_get(lua, &this.0, &key.as_bytes())? else {
                return Ok(None);
            };

            std::str::from_utf8(&value.as_bytes())
                .ok()
                .filter(|text| crate::vm::Kind::Number.holds(text.as_bytes()))
                .and_then(|text| text.parse::<i64>().ok())
                .map(|number| Some(number as f64))
                .ok_or_else(|| Error::runtime("the value is not a number: a whole number within 2^53 either way, in digits"))
        });
        methods.add_method("set_number", |lua, this, (key, value): (mlua::String, Option<f64>)| {
            let text = match value {
                None => None,
                Some(number) if number.fract() == 0.0 && (-EXACT as f64..=EXACT as f64).contains(&number) => Some((number as i64).to_string()),
                Some(number) => return Err(Error::runtime(format!("set_number takes a whole number within 2^53 either way, not {number}"))),
            };

            storage_set(lua, &this.0, &key.as_bytes(), text.as_ref().map(String::as_bytes))
        });

        // The binary kinds Rust contracts write, so a Lua version keeps what a Rust one stored.
        // Each integer kind reads back as its own type, and takes any integer or whole number
        // it holds.
        methods.add_method("get_u256", |lua, this, key: mlua::String| {
            Ok(fixed::<32>(lua, &this.0, &key.as_bytes(), "u256")?.map(|bytes| LuaInt(u256::from_le_bytes(bytes))))
        });
        methods.add_method("set_u256", |lua, this, (key, value): (mlua::String, Value)| {
            let bytes = integer::<u256>(&value)?.map(u256::to_le_bytes);

            storage_set(lua, &this.0, &key.as_bytes(), bytes.as_ref().map(|bytes| &bytes[..]))
        });
        methods.add_method("get_i256", |lua, this, key: mlua::String| {
            Ok(fixed::<32>(lua, &this.0, &key.as_bytes(), "i256")?.map(|bytes| LuaInt(i256::from_le_bytes(bytes))))
        });
        methods.add_method("set_i256", |lua, this, (key, value): (mlua::String, Value)| {
            let bytes = integer::<i256>(&value)?.map(i256::to_le_bytes);

            storage_set(lua, &this.0, &key.as_bytes(), bytes.as_ref().map(|bytes| &bytes[..]))
        });
        methods.add_method("get_u128", |lua, this, key: mlua::String| {
            Ok(fixed::<16>(lua, &this.0, &key.as_bytes(), "u128")?.map(|bytes| LuaInt(u128::from_le_bytes(bytes))))
        });
        methods.add_method("set_u128", |lua, this, (key, value): (mlua::String, Value)| {
            let bytes = integer::<u128>(&value)?.map(u128::to_le_bytes);

            storage_set(lua, &this.0, &key.as_bytes(), bytes.as_ref().map(|bytes| &bytes[..]))
        });
        methods.add_method("get_i128", |lua, this, key: mlua::String| {
            Ok(fixed::<16>(lua, &this.0, &key.as_bytes(), "i128")?.map(|bytes| LuaInt(i128::from_le_bytes(bytes))))
        });
        methods.add_method("set_i128", |lua, this, (key, value): (mlua::String, Value)| {
            let bytes = integer::<i128>(&value)?.map(i128::to_le_bytes);

            storage_set(lua, &this.0, &key.as_bytes(), bytes.as_ref().map(|bytes| &bytes[..]))
        });
        methods.add_method("get_u64", |lua, this, key: mlua::String| {
            Ok(fixed::<8>(lua, &this.0, &key.as_bytes(), "u64")?.map(|bytes| LuaInt(u64::from_le_bytes(bytes))))
        });
        methods.add_method("set_u64", |lua, this, (key, value): (mlua::String, Value)| {
            let bytes = integer::<u64>(&value)?.map(u64::to_le_bytes);

            storage_set(lua, &this.0, &key.as_bytes(), bytes.as_ref().map(|bytes| &bytes[..]))
        });
        methods.add_method("get_i64", |lua, this, key: mlua::String| {
            Ok(fixed::<8>(lua, &this.0, &key.as_bytes(), "i64")?.map(|bytes| LuaInt(i64::from_le_bytes(bytes))))
        });
        methods.add_method("set_i64", |lua, this, (key, value): (mlua::String, Value)| {
            let bytes = integer::<i64>(&value)?.map(i64::to_le_bytes);

            storage_set(lua, &this.0, &key.as_bytes(), bytes.as_ref().map(|bytes| &bytes[..]))
        });
        methods.add_method("get_bool", |lua, this, key: mlua::String| match fixed::<1>(lua, &this.0, &key.as_bytes(), "bool")? {
            None => Ok(None),
            Some([0]) => Ok(Some(false)),
            Some([1]) => Ok(Some(true)),
            Some(_) => Err(Error::runtime("the value is not a bool: one byte, 0 or 1")),
        });
        methods.add_method("set_bool", |lua, this, (key, value): (mlua::String, Option<bool>)| {
            let byte = value.map(|flag| [u8::from(flag)]);

            storage_set(lua, &this.0, &key.as_bytes(), byte.as_ref().map(|byte| &byte[..]))
        });
        methods.add_method("get_address", |lua, this, key: mlua::String| {
            let Some(bytes) = fixed::<33>(lua, &this.0, &key.as_bytes(), "address")? else {
                return Ok(None);
            };

            Address::from_abi(&bytes)
                .map(|address| Some(LuaAddress(address)))
                .ok_or_else(|| Error::runtime("the value is not an address: 33 bytes, its kind's tag then its bytes"))
        });
        methods.add_method("set_address", |lua, this, (key, value): (mlua::String, Option<mlua::AnyUserData>)| {
            let bytes = match value {
                Some(data) => Some(
                    data.borrow::<LuaAddress>()
                        .map_err(|_| Error::runtime("set_address takes an Address"))?
                        .0
                        .to_abi(),
                ),
                None => None,
            };

            storage_set(lua, &this.0, &key.as_bytes(), bytes.as_ref().map(|bytes| &bytes[..]))
        });
    }
}

/// Reads a value in one of the binary kinds, which is so many bytes long.
fn fixed<const N: usize>(lua: &Lua, space: &Space, key: &[u8], kind: &str) -> mlua::Result<Option<[u8; N]>> {
    let Some(value) = storage_get(lua, space, key)? else {
        return Ok(None);
    };

    <[u8; N]>::try_from(&*value.as_bytes())
        .map(Some)
        .map_err(|_| {
            let article = if kind.starts_with(['a', 'i']) { "an" } else { "a" };

            Error::runtime(format!("the value is not {article} {kind}: {} bytes, not {N}", value.as_bytes().len()))
        })
}

/// An integer to write, of any type or a whole number, that the kind holds; None for nil, which
/// deletes.
fn integer<T: Prim>(value: &Value) -> mlua::Result<Option<T>> {
    if value.is_nil() {
        return Ok(None);
    }

    exact_as::<T>(value).map(Some)
}

fn host(lua: &Lua) -> mlua::Result<Host> {
    lua.app_data_ref::<Host>()
        .map(|host| host.clone())
        .ok_or_else(|| Error::runtime("storage is only available inside a call"))
}

fn storage_get(lua: &Lua, space: &Space, key: &[u8]) -> mlua::Result<Option<mlua::String>> {
    let host = host(lua)?;

    let longest = host.limits().key_len as usize;

    if key.len() > longest {
        return Err(Error::runtime(format!("storage keys are at most {longest} bytes")));
    }

    let named = (space.name().len() + key.len()) as u64;
    charge(lua, costs(lua).storage_read + named / costs(lua).storage_bytes_per_fuel)?;

    let Some(value) = host.storage_get(space, key).map_err(refused)? else {
        return Ok(None);
    };

    charge(lua, value.len() as u64 / costs(lua).storage_bytes_per_fuel)?;
    lua.create_string(&value).map(Some)
}

fn storage_set(lua: &Lua, space: &Space, key: &[u8], value: Option<&[u8]>) -> mlua::Result<()> {
    let host = host(lua)?;
    let len = value.map_or(0, <[u8]>::len);

    if key.len() > host.limits().key_len as usize || len > host.limits().value_len as usize {
        // The Host words the error.
        return host.storage_set(space, key, value).map_err(refused);
    }

    let moved = (space.name().len() + key.len() + len) as u64;
    charge(lua, costs(lua).storage_write + moved / costs(lua).storage_bytes_per_fuel)?;
    host.storage_set(space, key, value).map_err(refused)
}

/// A page of a space's keys in order, charged: the seek, then each entry and its bytes.
fn storage_scan(lua: &Lua, space: &Space, prefix: &[u8], start: &[u8]) -> mlua::Result<Vec<(Vec<u8>, Vec<u8>)>> {
    let host = host(lua)?;
    let asked = (space.name().len() + prefix.len() + start.len()) as u64;
    charge(lua, costs(lua).storage_scan + asked / costs(lua).storage_bytes_per_fuel)?;
    let found = host.storage_scan(space, prefix, start, ITER_PAGE).map_err(refused)?;
    let moved: usize = found.iter().map(|(key, value)| key.len() + value.len()).sum();
    charge(lua, costs(lua).storage_scan_entry * found.len() as u64 + moved as u64 / costs(lua).storage_bytes_per_fuel)?;

    Ok(found)
}

/// An iterator over a space's keys that start with prefix, from start on, in byte order, for a
/// generic for: each step gives a key and its value, a page asked for at a time.
fn storage_iter(lua: &Lua, space: Space, prefix: Option<Vec<u8>>, start: Option<Vec<u8>>) -> mlua::Result<mlua::Function> {
    host(lua)?;
    let prefix = prefix.unwrap_or_default();
    let mut next = start.unwrap_or_else(|| prefix.clone());
    let mut page: VecDeque<(Vec<u8>, Vec<u8>)> = VecDeque::new();
    let mut ended = false;

    lua.create_function_mut(move |lua, _: mlua::MultiValue| {
        if page.is_empty() && !ended {
            let found = storage_scan(lua, &space, &prefix, &next)?;
            ended = found.len() < ITER_PAGE;

            // The page after starts right after its last key.
            if let Some((last, _)) = found.last() {
                next = [&last[..], &[0]].concat();
            }

            page.extend(found);
        }

        match page.pop_front() {
            Some((key, value)) => Ok((Some(lua.create_string(&key)?), Some(lua.create_string(&value)?))),
            None => Ok((None, None)),
        }
    })
}

fn namespace(name: &[u8]) -> mlua::Result<LuaNamespace> {
    if name.is_empty() || name.len() > MAX_NAMESPACE_LEN {
        return Err(Error::runtime(format!("namespace names are 1 to {MAX_NAMESPACE_LEN} bytes")));
    }

    Ok(LuaNamespace(Space::Named(name.to_vec())))
}

#[document(
    lua,
    kind = "library",
    name = "storage",
    category = "Chain",
    summary = "The contract's own key-value storage, kept between calls. Every contract has its own; keys and values are strings of bytes. A call that fails leaves storage as it was. Besides the plain keys storage.get and storage.set reach, it has namespaces, each with keys of its own, and the contract's own space, which code it delegates to cannot write. A script that declares a storage layout, with --@namespace, --@key and --@retired lines, reaches only what it declares, and only reads and clears plain keys.",
    note = "A read costs 100 fuel and a write 500, plus 1 for every 16 bytes of key and value, and of a namespace's name. Keys are at most 256 bytes and values at most 64 KiB. Store numbers as strings, for example tostring(balance) and u256(text).",
    example = "local count = tonumber(storage.get(\"count\") or \"0\")\nstorage.set(\"count\", tostring(count + 1))",
)]
fn storage_lib() {}

#[document(
    lua,
    parent = "storage",
    name = "get",
    summary = "Reads a value.",
    params = { key = { ty = "string", desc = "The key." } },
    returns = { ty = "string?", desc = "The value, or nil when the key has none." },
    see_also = "storage.set",
)]
fn storage_get_doc() {}

#[document(
    lua,
    parent = "storage",
    name = "set",
    summary = "Writes a value, or deletes the key when value is nil.",
    params = {
        key = { ty = "string", desc = "The key." },
        value = { ty = "string?", desc = "The value, or nil to delete.", optional = true },
    },
    returns = { ty = "nil", desc = "" },
    panics = "Errors writing a value in a script that declares a storage layout: its state lives in namespaces, and plain keys can only be read and deleted.",
    see_also = "storage.get",
)]
fn storage_set_doc() {}

#[document(
    lua,
    parent = "storage",
    name = "iter",
    summary = "Iterates over plain keys in byte order, for a generic for: each step gives a key and its value, as this call's own writes leave them. It asks for 64 at a time, so a loop that breaks early pays for only the pages it read.",
    params = {
        prefix = { ty = "string?", desc = "What every key given starts with: all of them when nil.", optional = true },
        start = { ty = "string?", desc = "The first key it may give: the prefix when nil. To go on later where a loop stopped, the last key with \0 after it.", optional = true },
    },
    returns = { ty = "function", desc = "The iterator: a key and its value each step, nil after the last." },
    note = "A page costs 100 fuel, plus 40 an entry and 1 for every 16 bytes of the keys and values it gives, and of the prefix and start. A transaction that iterated is run again, when blocks run transactions side by side, if one before it wrote any key in the range it saw.",
    example = "for key, value in storage.iter(\"member:\") do\n    count = count + 1\nend",
    see_also = "Namespace:iter",
)]
fn storage_iter_doc() {}

#[document(
    lua,
    parent = "storage",
    name = "namespace",
    summary = "A namespace: keys of its own, apart from plain storage and from every other namespace, so two pieces of code that keep state on one contract cannot clash by picking the same key. Code that names the same namespace shares it, delegated code included.",
    params = { name = { ty = "string", desc = "Its name: 1 to 64 bytes. Something long enough not to clash, such as \"my-token.balances\"." } },
    returns = { ty = "Namespace", desc = "The namespace." },
    panics = "Errors when the name is empty or longer than 64 bytes.",
    example = "local balances = storage.namespace(\"token.balances\")\nbalances:set(ctx.sender:text(), tostring(amount))",
    see_also = "storage.own, Namespace",
)]
fn storage_namespace_doc() {}

#[document(
    lua,
    parent = "storage",
    name = "own",
    kind = "field",
    summary = "The contract's own space: keys apart from plain storage and every namespace, which only this contract's own code may write. Code it runs with contracts.delegate may read them, but never change them, so a proxy keeps its owner and implementation here.",
    returns = { ty = "Namespace", desc = "The own space." },
    example = "function M.init()\n    storage.own:set(\"owner\", ctx.sender:text())\nend",
    see_also = "storage.namespace, contracts.delegate",
)]
fn storage_own_doc() {}

#[document(
    lua,
    kind = "class",
    name = "Namespace",
    category = "Chain",
    summary = "One part of the contract's storage with keys of its own, from storage.namespace or storage.own. Its methods work like storage.get and storage.set, and cost the same, plus the name's bytes.",
    see_also = "storage.namespace, storage.own",
)]
fn namespace_class() {}

#[document(
    lua,
    parent = "Namespace",
    name = "get",
    kind = "method",
    summary = "Reads a value.",
    params = { key = { ty = "string", desc = "The key." } },
    returns = { ty = "string?", desc = "The value, or nil when the key has none." },
    see_also = "Namespace:set",
)]
fn namespace_get_doc() {}

#[document(
    lua,
    parent = "Namespace",
    name = "iter",
    kind = "method",
    summary = "Iterates over the namespace's keys in byte order, for a generic for, as storage.iter does plain keys. A script that declares a storage layout iterates only the namespaces it declares.",
    params = {
        prefix = { ty = "string?", desc = "What every key given starts with: all of them when nil.", optional = true },
        start = { ty = "string?", desc = "The first key it may give: the prefix when nil.", optional = true },
    },
    returns = { ty = "function", desc = "The iterator: a key and its value each step, nil after the last." },
    example = "local balances = storage.namespace(\"token.balances\")\nfor holder, held in balances:iter() do\n    total = total + u256(held)\nend",
    see_also = "storage.iter",
)]
fn namespace_iter_doc() {}

#[document(
    lua,
    parent = "Namespace",
    name = "set",
    kind = "method",
    summary = "Writes a value, or deletes the key when value is nil.",
    params = {
        key = { ty = "string", desc = "The key." },
        value = { ty = "string?", desc = "The value, or nil to delete.", optional = true },
    },
    returns = { ty = "nil", desc = "" },
    panics = "Errors for code run by contracts.delegate writing storage.own, and for a script that declares a storage layout reaching past it: a namespace or key it does not declare, a retired one, or a value that is not its kind.",
    see_also = "Namespace:get",
)]
fn namespace_set_doc() {}

#[document(
    lua,
    parent = "Namespace",
    name = "get_u256",
    kind = "method",
    summary = "Reads a u256 kind's value: 32 bytes, little-endian, as a U256, as Rust contracts write one with set_u256. With the other integer kinds' getters, get_bool and get_address, it lets a Lua version keep the namespaces a Rust version kept, whose kinds are binary; Rust reads the text kinds Lua writes with get_decimal and the like.",
    params = { key = { ty = "string", desc = "The key." } },
    returns = { ty = "U256?", desc = "The value, or nil when the key has none." },
    panics = "Errors when the value is not 32 bytes long.",
    example = "--@namespace balances address -> u256\nlocal balances = storage.namespace(\"balances\")\nlocal held = balances:get_u256(ctx.sender:encode()) or u256.zero",
    see_also = "Namespace:set_u256, Address:encode",
)]
fn namespace_get_u256_doc() {}

#[document(
    lua,
    parent = "Namespace",
    name = "get_number",
    kind = "method",
    summary = "Reads a number kind's value: a whole number in digits. set_number writes one, spelled the one way the kind takes, which tostring does not keep to past 10^15: tostring(1e15) is 1e+15.",
    params = { key = { ty = "string", desc = "The key." } },
    returns = { ty = "number?", desc = "The value, or nil when the key has none." },
    panics = "Errors for a value that is not a whole number in digits, and set_number for a number with a fraction or past 2^53 either way.",
    example = "--@namespace scores text -> number\nlocal scores = storage.namespace(\"scores\")\nscores:set_number(\"alice\", (scores:get_number(\"alice\") or 0) + 1)",
)]
fn namespace_get_number_doc() {}

#[document(
    lua,
    parent = "Namespace",
    name = "set_u256",
    kind = "method",
    summary = "Writes a u256 kind's value, 32 bytes little-endian, or deletes the key for nil. Every integer kind's setter takes any integer type, or a whole number, that the kind holds.",
    params = {
        key = { ty = "string", desc = "The key." },
        value = { ty = "Integer?", desc = "Any integer or whole number from 0 to 2^256 - 1; nil to delete.", optional = true },
    },
    returns = { ty = "nil", desc = "" },
    panics = "Errors on a value the kind does not hold, such as -1.",
    see_also = "Namespace:get_u256",
)]
fn namespace_set_u256_doc() {}

#[document(
    lua,
    parent = "Namespace",
    name = "get_u128",
    kind = "method",
    summary = "Reads a u128 kind's value, 16 bytes little-endian, as a U128. set_u128 writes one.",
    params = { key = { ty = "string", desc = "The key." } },
    returns = { ty = "U128?", desc = "The value, or nil when the key has none." },
    see_also = "Namespace:set_u128",
)]
fn namespace_get_u128_doc() {}

#[document(
    lua,
    parent = "Namespace",
    name = "set_u128",
    kind = "method",
    summary = "Writes a u128 kind's value, 16 bytes little-endian, or deletes the key for nil.",
    params = {
        key = { ty = "string", desc = "The key." },
        value = { ty = "Integer?", desc = "Any integer or whole number from 0 to 2^128 - 1; nil to delete.", optional = true },
    },
    returns = { ty = "nil", desc = "" },
    panics = "Errors on a value the kind does not hold.",
    see_also = "Namespace:get_u128",
)]
fn namespace_set_u128_doc() {}

#[document(
    lua,
    parent = "Namespace",
    name = "get_i128",
    kind = "method",
    summary = "Reads an i128 kind's value, 16 bytes little-endian, as an I128. set_i128 writes one.",
    params = { key = { ty = "string", desc = "The key." } },
    returns = { ty = "I128?", desc = "The value, or nil when the key has none." },
    see_also = "Namespace:set_i128",
)]
fn namespace_get_i128_doc() {}

#[document(
    lua,
    parent = "Namespace",
    name = "set_i128",
    kind = "method",
    summary = "Writes an i128 kind's value, 16 bytes little-endian, or deletes the key for nil.",
    params = {
        key = { ty = "string", desc = "The key." },
        value = { ty = "Integer?", desc = "Any integer or whole number from -2^127 to 2^127 - 1; nil to delete.", optional = true },
    },
    returns = { ty = "nil", desc = "" },
    panics = "Errors on a value the kind does not hold.",
    see_also = "Namespace:get_i128",
)]
fn namespace_set_i128_doc() {}

#[document(
    lua,
    parent = "Namespace",
    name = "get_u64",
    kind = "method",
    summary = "Reads a u64 kind's value, 8 bytes little-endian, as a U64. set_u64 writes one.",
    params = { key = { ty = "string", desc = "The key." } },
    returns = { ty = "U64?", desc = "The value, or nil when the key has none." },
    see_also = "Namespace:set_u64",
)]
fn namespace_get_u64_doc() {}

#[document(
    lua,
    parent = "Namespace",
    name = "set_u64",
    kind = "method",
    summary = "Writes a u64 kind's value, 8 bytes little-endian, or deletes the key for nil.",
    params = {
        key = { ty = "string", desc = "The key." },
        value = { ty = "Integer?", desc = "Any integer or whole number from 0 to 2^64 - 1; nil to delete.", optional = true },
    },
    returns = { ty = "nil", desc = "" },
    panics = "Errors on a value the kind does not hold.",
    see_also = "Namespace:get_u64",
)]
fn namespace_set_u64_doc() {}

#[document(
    lua,
    parent = "Namespace",
    name = "get_i64",
    kind = "method",
    summary = "Reads an i64 kind's value, 8 bytes little-endian, as an I64. set_i64 writes one.",
    params = { key = { ty = "string", desc = "The key." } },
    returns = { ty = "I64?", desc = "The value, or nil when the key has none." },
    see_also = "Namespace:set_i64",
)]
fn namespace_get_i64_doc() {}

#[document(
    lua,
    parent = "Namespace",
    name = "set_i64",
    kind = "method",
    summary = "Writes an i64 kind's value, 8 bytes little-endian, or deletes the key for nil.",
    params = {
        key = { ty = "string", desc = "The key." },
        value = { ty = "Integer?", desc = "Any integer or whole number from -2^63 to 2^63 - 1; nil to delete.", optional = true },
    },
    returns = { ty = "nil", desc = "" },
    panics = "Errors on a value the kind does not hold.",
    see_also = "Namespace:get_i64",
)]
fn namespace_set_i64_doc() {}

#[document(
    lua,
    parent = "Namespace",
    name = "get_i256",
    kind = "method",
    summary = "Reads an i256 kind's value, 32 bytes little-endian, two's complement, as an I256. set_i256 writes one.",
    params = { key = { ty = "string", desc = "The key." } },
    returns = { ty = "I256?", desc = "The value, or nil when the key has none." },
    see_also = "Namespace:set_i256",
)]
fn namespace_get_i256_doc() {}

#[document(
    lua,
    parent = "Namespace",
    name = "set_i256",
    kind = "method",
    summary = "Writes an i256 kind's value, 32 bytes little-endian, two's complement, or deletes the key for nil.",
    params = {
        key = { ty = "string", desc = "The key." },
        value = { ty = "Integer?", desc = "Any integer or whole number from -2^255 to 2^255 - 1; nil to delete.", optional = true },
    },
    returns = { ty = "nil", desc = "" },
    panics = "Errors on a value the kind does not hold.",
    see_also = "Namespace:get_i256",
)]
fn namespace_set_i256_doc() {}

#[document(
    lua,
    parent = "Namespace",
    name = "get_bool",
    kind = "method",
    summary = "Reads a bool kind's value, one byte 0 or 1, as a boolean. set_bool writes one.",
    params = { key = { ty = "string", desc = "The key." } },
    returns = { ty = "boolean?", desc = "The value, or nil when the key has none." },
)]
fn namespace_get_bool_doc() {}

#[document(
    lua,
    parent = "Namespace",
    name = "get_address",
    kind = "method",
    summary = "Reads an address kind's value: 33 bytes, its kind's tag then its bytes, as Rust contracts write one with set_address. set_address writes one from an Address.",
    params = { key = { ty = "string", desc = "The key." } },
    returns = { ty = "Address?", desc = "The address, or nil when the key has none." },
    panics = "Errors when the value is not an address in that form.",
    see_also = "Address:encode, address.decode",
)]
fn namespace_get_address_doc() {}

pub fn register_storage_lib(lua: &Lua) {
    let storage_table = lua
        .create_table()
        .expect("[storage] Failed to create storage table");
    storage_table
        .set(
            "get",
            lua.create_function(|lua, key: mlua::String| storage_get(lua, &Space::Plain, &key.as_bytes()))
                .expect("[storage] Failed to create get"),
        )
        .expect("[storage] Failed to set get");
    storage_table
        .set(
            "set",
            lua.create_function(|lua, (key, value): (mlua::String, Option<mlua::String>)| {
                let value = value.as_ref().map(|value| value.as_bytes());

                storage_set(lua, &Space::Plain, &key.as_bytes(), value.as_deref())
            })
            .expect("[storage] Failed to create set"),
        )
        .expect("[storage] Failed to set set");
    storage_table
        .set(
            "iter",
            lua.create_function(|lua, (prefix, start): (Option<mlua::String>, Option<mlua::String>)| {
                storage_iter(lua, Space::Plain, prefix.map(|prefix| prefix.as_bytes().to_vec()), start.map(|start| start.as_bytes().to_vec()))
            })
            .expect("[storage] Failed to create iter"),
        )
        .expect("[storage] Failed to set iter");
    storage_table
        .set(
            "namespace",
            lua.create_function(|_, name: mlua::String| namespace(&name.as_bytes()))
                .expect("[storage] Failed to create namespace"),
        )
        .expect("[storage] Failed to set namespace");
    storage_table
        .set("own", LuaNamespace(Space::Own))
        .expect("[storage] Failed to set own");
    lua.globals()
        .set("storage", storage_table)
        .expect("[storage] Failed to set storage table");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::structs::hash::HashType;
    use crate::db::{Column, MemoryDb, State, namespace_key, storage_key};
    use crate::structs::address::Address;
    use crate::structs::u256::u256;
    use crate::vm::{CallContext, Vm};
    use std::sync::Arc;

    fn setup(contract: Address) -> (Lua, Arc<State>) {
        let lua = Lua::new();
        lua.set_fuel(u64::MAX).unwrap();
        register_storage_lib(&lua);
        let state = Arc::new(State::new(Arc::new(MemoryDb::default())));
        let context = CallContext {
            sender: Address::Ed25519([0; 32]),
            contract,
            value: u256::ZERO,
            block: crate::vm::BlockAt::at(0),
        };
        lua.set_app_data(Host::root(state.clone(), context, 1 << 20, Arc::new(Vm::new(&[], HashType::default_chain()).unwrap())));

        (lua, state)
    }

    #[test]
    fn values_round_trip_and_delete() {
        let (lua, _) = setup(Address::Contract([1; 32]));
        let code = r#"
            local before = storage.get("k")
            storage.set("k", "v")
            local after = storage.get("k")
            storage.set("k", nil)
            return before, after, storage.get("k")
        "#;
        let (before, after, deleted): (Option<String>, String, Option<String>) =
            lua.load(code).eval().unwrap();

        assert_eq!(before, None);
        assert_eq!(after, "v");
        assert_eq!(deleted, None);
    }

    #[test]
    fn each_contract_has_its_own_keys() {
        let (lua, state) = setup(Address::Contract([1; 32]));
        lua.load("storage.set('k', 'one')").exec().unwrap();

        assert_eq!(
            state.get(Column::Storage, &storage_key(&Address::Contract([1; 32]), b"k")).unwrap(),
            Some(b"one".to_vec())
        );
        assert_eq!(
            state.get(Column::Storage, &storage_key(&Address::Contract([2; 32]), b"k")).unwrap(),
            None
        );
    }

    #[test]
    fn namespaces_and_the_own_space_keep_their_keys_apart() {
        let contract = Address::Contract([1; 32]);
        let (lua, state) = setup(contract);
        let code = r#"
            storage.set("k", "plain")
            storage.namespace("a"):set("k", "a")
            storage.namespace("b"):set("k", "b")
            storage.own:set("k", "own")
            local read = { storage.get("k"), storage.namespace("a"):get("k"), storage.namespace("b"):get("k"), storage.own:get("k") }
            storage.namespace("a"):set("k", nil)
            return read[1], read[2], read[3], read[4], storage.namespace("a"):get("k")
        "#;
        let read: (String, String, String, String, Option<String>) = lua.load(code).eval().unwrap();

        assert_eq!(read, ("plain".into(), "a".into(), "b".into(), "own".into(), None));
        assert_eq!(state.get(Column::Namespaces, &namespace_key(&contract, b"b", b"k")).unwrap(), Some(b"b".to_vec()));
        assert_eq!(state.get(Column::Namespaces, &namespace_key(&contract, b"", b"k")).unwrap(), Some(b"own".to_vec()));
    }

    #[test]
    fn iteration_goes_in_key_order_over_what_the_call_left() {
        let contract = Address::Contract([1; 32]);
        let (lua, state) = setup(contract);

        // Some keys committed below the call's layer, then changed by it.
        for key in ["m:b", "m:d", "m:f", "other"] {
            state.put(Column::Storage, storage_key(&contract, key.as_bytes()), b"below".to_vec());
        }

        let code = r#"
            storage.set("m:a", "call")
            storage.set("m:d", "call")
            storage.set("m:f", nil)
            storage.namespace("n"):set("m:z", "elsewhere")
            local seen = {}
            for key, value in storage.iter("m:") do
                table.insert(seen, key .. "=" .. value)
            end
            local from = {}
            for key in storage.iter("m:", "m:c") do
                table.insert(from, key)
            end
            return table.concat(seen, " "), table.concat(from, " ")
        "#;
        let (seen, from): (String, String) = lua.load(code).eval().unwrap();

        assert_eq!(seen, "m:a=call m:b=below m:d=call");
        assert_eq!(from, "m:d");
    }

    #[test]
    fn iteration_pages_and_stops_where_the_loop_breaks() {
        let (lua, _) = setup(Address::Contract([1; 32]));
        let code = r#"
            local balances = storage.namespace("balances")
            for i = 1, 150 do
                balances:set(string.format("k%03d", i), tostring(i))
            end
            local count, last = 0, nil
            for key, value in balances:iter() do
                count = count + 1
                last = value
            end
            local first
            for key in balances:iter() do
                first = key
                break
            end
            local under = 0
            for key in balances:iter("k1") do
                under = under + 1
            end
            return count, last, first, under
        "#;
        let (count, last, first, under): (u32, String, String, u32) = lua.load(code).eval().unwrap();

        assert_eq!((count, last.as_str(), first.as_str(), under), (150, "150", "k001", 51), "past two pages of 64, and k100 to k150 under k1");
    }

    #[test]
    fn namespace_names_are_one_to_64_bytes() {
        let (lua, _) = setup(Address::Contract([1; 32]));
        let long = format!("storage.namespace(string.rep('n', {}))", MAX_NAMESPACE_LEN + 1);

        assert!(lua.load("storage.namespace('')").exec().unwrap_err().to_string().contains("1 to 64 bytes"));
        assert!(lua.load(long).exec().unwrap_err().to_string().contains("1 to 64 bytes"));
        lua.load(format!("storage.namespace(string.rep('n', {MAX_NAMESPACE_LEN})):set('k', 'v')")).exec().unwrap();
    }

    #[test]
    fn limits_and_missing_access_are_errors() {
        let (lua, _) = setup(Address::Contract([1; 32]));
        let long_key = format!("storage.set(string.rep('k', {}), 'v')", crate::vm::MAX_KEY_LEN + 1);
        let long_value = format!("storage.set('k', string.rep('v', {}))", crate::vm::MAX_VALUE_LEN + 1);

        assert!(lua.load(long_key).exec().unwrap_err().to_string().contains("at most"));
        assert!(lua.load(long_value).exec().unwrap_err().to_string().contains("at most"));

        let bare = Lua::new();
        bare.set_fuel(u64::MAX).unwrap();
        register_storage_lib(&bare);

        assert!(bare.load("storage.get('k')").exec().unwrap_err().to_string().contains("inside a call"));
    }

    #[test]
    fn writes_cost_more_than_reads() {
        let (lua, _) = setup(Address::Contract([1; 32]));
        let mut costs = Vec::new();

        for code in ["local _ = storage.get('k')", "storage.set('k', 'v')"] {
            lua.set_fuel(1_000_000).unwrap();
            lua.load(code).exec().unwrap();
            costs.push(1_000_000 - lua.get_fuel().unwrap());
        }

        assert!(costs[1] > costs[0] + 300, "{costs:?}");
    }

    #[test]
    fn the_binary_kinds_round_trip_as_rust_writes_them() {
        let contract = Address::Contract([1; 32]);
        let (lua, state) = setup(contract);
        super::super::integers::register_integer_libs(&lua);
        super::super::address::register_address_lib(&lua);
        let code = r#"
            local ns = storage.namespace("binary")
            local who = address.parse("secp256k1:0x00000000000000000000000000000000000000ff")
            ns:set_u256("u256", u256("340282366920938463463374607431768211456"))
            ns:set_u128("u128", u256("340282366920938463463374607431768211455"))
            ns:set_u64("u64", 7)
            ns:set_i64("i64", -42)
            ns:set_bool("bool", true)
            ns:set_address("address", who)
            ns:set_address(who:encode(), who)
            local back = address.decode(who:encode())
            local read = {
                tostring(ns:get_u256("u256")), tostring(ns:get_u128("u128")), tostring(ns:get_u64("u64")),
                tostring(ns:get_i64("i64")), ns:get_bool("bool"), ns:get_address("address"):text(),
                ns:get_address(who:encode()) == who, back == who, ns:get_u256("missing") == nil,
            }
            ns:set_u256("u256", nil)
            return read[1], read[2], read[3], read[4], read[5], read[6], read[7], read[8], read[9], ns:get_u256("u256") == nil
        "#;
        let read: (String, String, String, String, bool, String, bool, bool, bool, bool) = lua.load(code).eval().unwrap();
        let stored = |key: &[u8]| state.get(Column::Namespaces, &namespace_key(&contract, b"binary", key)).unwrap().unwrap();

        assert_eq!(read.0, "340282366920938463463374607431768211456");
        assert_eq!(read.1, u128::MAX.to_string());
        assert_eq!((read.2.as_str(), read.3.as_str(), read.4), ("7", "-42", true));
        assert_eq!(read.5, "secp256k1:0x00000000000000000000000000000000000000ff");
        assert!(read.6 && read.7 && read.8 && read.9, "{read:?}");
        // Byte for byte what the SDK's set_u64, set_i64, set_bool and set_address write.
        assert_eq!(stored(b"u64"), 7u64.to_le_bytes());
        assert_eq!(stored(b"i64"), (-42i64).to_le_bytes());
        assert_eq!(stored(b"bool"), [1]);
        let mut who = [0; 20];
        who[19] = 0xff;

        assert_eq!(stored(b"address"), Address::Secp256k1(who).to_abi());
    }

    #[test]
    fn every_integer_kind_reads_back_as_its_own_type() {
        let contract = Address::Contract([1; 32]);
        let (lua, state) = setup(contract);
        super::super::integers::register_integer_libs(&lua);
        let code = r#"
            local ns = storage.namespace("ints")
            ns:set_u64("u64", u64.max)
            ns:set_i64("i64", i64.min)
            ns:set_u128("u128", u128.max)
            ns:set_i128("i128", i128.min)
            ns:set_u256("u256", u256.max)
            ns:set_i256("i256", i256.min)
            -- any integer type or whole number the kind holds
            ns:set_u64("small", u256(5))
            ns:set_i256("negative", -5)
            ns:set_i128("wide", i256(-7))
            local read = {}

            for _, name in { "u64", "i64", "u128", "i128", "u256", "i256" } do
                local value = ns["get_" .. name](ns, name)
                table.insert(read, typeof(value) .. " " .. tostring(value))
            end

            table.insert(read, typeof(ns:get_u64("small")) .. " " .. tostring(ns:get_u64("small")))
            table.insert(read, tostring(ns:get_i256("negative")))
            table.insert(read, tostring(ns:get_i128("wide")))
            ns:set_i256("i256", nil)
            table.insert(read, tostring(ns:get_i256("i256")))

            return table.concat(read, ",")
        "#;
        let read: String = lua.load(code).eval().unwrap();
        let stored = |key: &[u8]| state.get(Column::Namespaces, &namespace_key(&contract, b"ints", key)).unwrap().unwrap();

        assert_eq!(
            read.split(',').collect::<Vec<_>>(),
            [
                format!("U64 {}", u64::MAX),
                format!("I64 {}", i64::MIN),
                format!("U128 {}", u128::MAX),
                format!("I128 {}", i128::MIN),
                format!("U256 {}", u256::MAX),
                format!("I256 {}", i256::MIN),
                "U64 5".to_string(),
                "-5".to_string(),
                "-7".to_string(),
                "nil".to_string(),
            ]
        );
        // byte for byte what Rust's to_le_bytes, and so the SDK, writes
        assert_eq!(stored(b"u64"), u64::MAX.to_le_bytes());
        assert_eq!(stored(b"i64"), i64::MIN.to_le_bytes());
        assert_eq!(stored(b"u128"), u128::MAX.to_le_bytes());
        assert_eq!(stored(b"i128"), i128::MIN.to_le_bytes());
        assert_eq!(stored(b"u256"), u256::MAX.to_le_bytes());
        assert_eq!(stored(b"negative"), i256::new(-5).to_le_bytes());
        assert_eq!(stored(b"negative"), [[0xfb].as_slice(), &[0xff; 31]].concat());
    }

    #[test]
    fn the_binary_kinds_refuse_what_they_cannot_hold() {
        let (lua, _) = setup(Address::Contract([1; 32]));
        super::super::integers::register_integer_libs(&lua);
        super::super::address::register_address_lib(&lua);
        let error = |code: &str| lua.load(code).exec().unwrap_err().to_string();
        lua.load(r#"storage.namespace("n"):set("text", "hello")"#).exec().unwrap();

        assert!(error(r#"storage.namespace("n"):get_u256("text")"#).contains("the value is not a u256: 5 bytes, not 32"));
        assert!(error(r#"storage.namespace("n"):set_u64("k", u256("18446744073709551616"))"#).contains("18446744073709551616 is outside what a U64 holds, 0 to 18446744073709551615"));
        assert!(error(r#"storage.namespace("n"):set_u128("k", i128(-1))"#).contains("-1 is outside what a U128 holds"));
        assert!(error(r#"storage.namespace("n"):set_i64("k", u64.max)"#).contains("outside what an I64 holds"));
        assert!(error(r#"storage.namespace("n"):set_i128("k", i256.min)"#).contains("outside what an I128 holds"));
        assert!(error(r#"storage.namespace("n"):set_i256("k", u256.max)"#).contains("outside what an I256 holds"));
        assert!(error(r#"storage.namespace("n"):set_i256("k", "5")"#).contains("whole number"));
        assert!(error(r#"storage.namespace("n"):get_i256("text")"#).contains("the value is not an i256: 5 bytes, not 32"));
        assert!(error(r#"storage.namespace("n"):get_i128("text")"#).contains("the value is not an i128: 5 bytes, not 16"));
        assert!(error(r#"storage.namespace("n"):set_i64("k", 1.5)"#).contains("whole number"));
        assert!(error(r#"storage.namespace("n"):set_u256("k", -1)"#).contains("non-negative whole number"));
        assert!(error(r#"storage.namespace("n"):set_address("k", u256(1))"#).contains("takes an Address"));
        assert!(error(r#"address.decode("short")"#).contains("33 bytes"));
    }
}
