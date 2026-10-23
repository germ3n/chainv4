use {
    crate::db::{Column, DbError, State, storage_key},
    crate::structs::address::Address,
    crate::vm::lvm::charge,
    mlua::{Error, Lua},
    r#macro::document,
    std::sync::{Arc, Mutex},
};

/// Fuel for one read or write, plus one more for every 16 bytes moved. Writes cost more because
/// every node keeps what they store.
const READ_COST: u64 = 100;
const WRITE_COST: u64 = 500;
const BYTES_PER_FUEL: u64 = 16;
pub const MAX_KEY_LEN: usize = 256;
pub const MAX_VALUE_LEN: usize = 64 * 1024;

/// What a running call's storage reads and writes go through. Set as app data for the call.
pub struct StorageAccess {
    state: Arc<State>,
    contract: Address,
    failure: Mutex<Option<DbError>>,
}

impl StorageAccess {
    pub fn new(state: Arc<State>, contract: Address) -> StorageAccess {
        StorageAccess {
            state,
            contract,
            failure: Mutex::new(None),
        }
    }

    /// A database failure seen during the call. Recorded here as well as raised, so a script
    /// that catches the error with pcall cannot hide it from the node.
    pub fn failure(&self) -> Option<DbError> {
        self.failure.lock().expect("never poisoned").clone()
    }

    fn fail(&self, err: DbError) -> Error {
        *self.failure.lock().expect("never poisoned") = Some(err.clone());

        Error::external(err)
    }
}

fn check_key(key: &[u8]) -> mlua::Result<()> {
    if key.len() > MAX_KEY_LEN {
        return Err(Error::runtime(format!("storage keys are at most {MAX_KEY_LEN} bytes")));
    }

    Ok(())
}

fn storage_get(lua: &Lua, key: &[u8]) -> mlua::Result<Option<mlua::String>> {
    let access = lua
        .app_data_ref::<StorageAccess>()
        .ok_or_else(|| Error::runtime("storage is only available inside a call"))?;
    check_key(key)?;
    charge(lua, READ_COST + key.len() as u64 / BYTES_PER_FUEL)?;
    let value = access
        .state
        .get(Column::Storage, &storage_key(&access.contract, key))
        .map_err(|err| access.fail(err))?;
    let Some(value) = value else {
        return Ok(None);
    };
    charge(lua, value.len() as u64 / BYTES_PER_FUEL)?;

    lua.create_string(&value).map(Some)
}

fn storage_set(lua: &Lua, key: &[u8], value: Option<&[u8]>) -> mlua::Result<()> {
    let access = lua
        .app_data_ref::<StorageAccess>()
        .ok_or_else(|| Error::runtime("storage is only available inside a call"))?;
    check_key(key)?;
    let len = value.map_or(0, <[u8]>::len);

    if len > MAX_VALUE_LEN {
        return Err(Error::runtime(format!("storage values are at most {MAX_VALUE_LEN} bytes")));
    }

    charge(lua, WRITE_COST + (key.len() + len) as u64 / BYTES_PER_FUEL)?;
    let key = storage_key(&access.contract, key);

    match value {
        Some(value) => access.state.put(Column::Storage, key, value.to_vec()),
        None => access.state.delete(Column::Storage, key),
    }

    Ok(())
}

#[document(
    lua,
    kind = "library",
    name = "storage",
    category = "Chain",
    summary = "The contract's own key-value storage, kept between calls. Every contract has its own; keys and values are strings of bytes. A call that fails leaves storage as it was.",
    note = "A read costs 100 fuel and a write 500, plus 1 for every 16 bytes of key and value. Keys are at most 256 bytes and values at most 64 KiB. Store numbers as strings, for example tostring(balance) and u256(text).",
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
    see_also = "storage.get",
)]
fn storage_set_doc() {}

pub fn register_storage_lib(lua: &Lua) {
    let storage_table = lua
        .create_table()
        .expect("[storage] Failed to create storage table");
    storage_table
        .set(
            "get",
            lua.create_function(|lua, key: mlua::String| storage_get(lua, &key.as_bytes()))
                .expect("[storage] Failed to create get"),
        )
        .expect("[storage] Failed to set get");
    storage_table
        .set(
            "set",
            lua.create_function(|lua, (key, value): (mlua::String, Option<mlua::String>)| {
                let value = value.as_ref().map(|value| value.as_bytes());

                storage_set(lua, &key.as_bytes(), value.as_deref())
            })
            .expect("[storage] Failed to create set"),
        )
        .expect("[storage] Failed to set set");
    lua.globals()
        .set("storage", storage_table)
        .expect("[storage] Failed to set storage table");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::MemoryDb;

    fn setup(contract: Address) -> (Lua, Arc<State>) {
        let lua = Lua::new();
        lua.set_fuel(u64::MAX).unwrap();
        register_storage_lib(&lua);
        let state = Arc::new(State::new(Arc::new(MemoryDb::default())));
        lua.set_app_data(StorageAccess::new(state.clone(), contract));

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
