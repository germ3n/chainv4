use {
    super::accounts::refused,
    crate::vm::lvm::charge,
    crate::vm::{Host, MAX_KEY_LEN, MAX_NAMESPACE_LEN, MAX_VALUE_LEN, Space, cost},
    mlua::{Error, Lua, UserData, UserDataMethods},
    r#macro::document,
};

/// One part of the contract's storage with keys of its own: a namespace, from storage.namespace,
/// or the contract's own space, storage.own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LuaNamespace(pub Space);

impl UserData for LuaNamespace {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("get", |lua, this, key: mlua::String| storage_get(lua, &this.0, &key.as_bytes()));
        methods.add_method("set", |lua, this, (key, value): (mlua::String, Option<mlua::String>)| {
            let value = value.as_ref().map(|value| value.as_bytes());

            storage_set(lua, &this.0, &key.as_bytes(), value.as_deref())
        });
    }
}

fn host(lua: &Lua) -> mlua::Result<Host> {
    lua.app_data_ref::<Host>()
        .map(|host| host.clone())
        .ok_or_else(|| Error::runtime("storage is only available inside a call"))
}

fn storage_get(lua: &Lua, space: &Space, key: &[u8]) -> mlua::Result<Option<mlua::String>> {
    let host = host(lua)?;

    if key.len() > MAX_KEY_LEN {
        return Err(Error::runtime(format!("storage keys are at most {MAX_KEY_LEN} bytes")));
    }

    let named = (space.name().len() + key.len()) as u64;
    charge(lua, cost::STORAGE_READ + named / cost::STORAGE_BYTES_PER_FUEL)?;

    let Some(value) = host.storage_get(space, key).map_err(refused)? else {
        return Ok(None);
    };

    charge(lua, value.len() as u64 / cost::STORAGE_BYTES_PER_FUEL)?;
    lua.create_string(&value).map(Some)
}

fn storage_set(lua: &Lua, space: &Space, key: &[u8], value: Option<&[u8]>) -> mlua::Result<()> {
    let host = host(lua)?;
    let len = value.map_or(0, <[u8]>::len);

    if key.len() > MAX_KEY_LEN || len > MAX_VALUE_LEN {
        // The Host words the error.
        return host.storage_set(space, key, value).map_err(refused);
    }

    let moved = (space.name().len() + key.len() + len) as u64;
    charge(lua, cost::STORAGE_WRITE + moved / cost::STORAGE_BYTES_PER_FUEL)?;
    host.storage_set(space, key, value).map_err(refused)
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
        let long_key = format!("storage.set(string.rep('k', {}), 'v')", MAX_KEY_LEN + 1);
        let long_value = format!("storage.set('k', string.rep('v', {}))", MAX_VALUE_LEN + 1);

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
}
