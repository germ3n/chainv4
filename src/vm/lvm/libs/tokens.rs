use {
    crate::vm::lvm::{charge, costs},
    crate::vm::{Host},
    mlua::{Error, Lua},
    r#macro::document,
};

fn refresh(lua: &Lua, (): ()) -> mlua::Result<()> {
    let host = lua
        .app_data_ref::<Host>()
        .map(|host| host.clone())
        .ok_or_else(|| Error::runtime("tokens is only available inside a call"))?;
    host.refresh_token();

    charge(lua, costs(lua).token_refresh)
}

#[document(
    lua,
    kind = "library",
    name = "tokens",
    category = "Chain",
    summary = "What a token tells the chain's token registry about itself. The registry keeps what a token's name, symbol and decimals answered at its deploy or last upgrade; a token whose answers change asks it to ask again.",
    note = "Asking costs 100 fuel, and the questions, run once the transaction succeeds, cost what they burn, out of the transaction's fuel.",
    example = "function M.rename(name)\n    -- only the owner, then:\n    storage.set(\"name\", name)\n    tokens.refresh()\nend",
)]
fn tokens_lib() {}

#[document(
    lua,
    parent = "tokens",
    name = "refresh",
    summary = "Asks the token registry to ask this contract's name(), symbol() and decimals() again, as a deploy does, once the transaction succeeds. An answer that does not fit fails the transaction. Kept only if this call, and every call above it, succeeds; does nothing for a contract that is not a token. Under contracts.delegate, it is the delegating contract that is asked again.",
    returns = { ty = "nil", desc = "" },
)]
fn tokens_refresh() {}

#[document(
    lua,
    kind = "library",
    name = "collections",
    category = "Chain",
    summary = "What an NFT or multi-token collection tells the chain's collection registry about itself. The registry keeps what its name and symbol answered at its deploy or last upgrade; a collection whose answers change asks it to ask again.",
    note = "The same request as tokens.refresh, under a name that reads right in a collection: one asks again for both registries. Asking costs 100 fuel, and the questions, run once the transaction succeeds, cost what they burn, out of the transaction's fuel.",
    example = "function M.rename(name)\n    -- only the owner, then:\n    storage.set(\"name\", name)\n    collections.refresh()\nend",
)]
fn collections_lib() {}

#[document(
    lua,
    parent = "collections",
    name = "refresh",
    summary = "Asks the registries to ask this contract's name() and symbol() again, as a deploy does, once the transaction succeeds: tokens.refresh by another name. An answer that does not fit fails the transaction. Kept only if this call, and every call above it, succeeds; does nothing for a contract that is neither a token nor a collection.",
    returns = { ty = "nil", desc = "" },
)]
fn collections_refresh() {}

pub fn register_tokens_lib(lua: &Lua) {
    let tokens_table = lua
        .create_table()
        .expect("[tokens] Failed to create tokens table");

    tokens_table
        .set(
            "refresh",
            lua.create_function(refresh)
                .expect("[tokens] Failed to create refresh"),
        )
        .expect("[tokens] Failed to set refresh");

    lua.globals()
        .set("tokens", tokens_table)
        .expect("[tokens] Failed to set tokens table");

    let collections_table = lua
        .create_table()
        .expect("[collections] Failed to create collections table");

    collections_table
        .set(
            "refresh",
            lua.create_function(refresh)
                .expect("[collections] Failed to create refresh"),
        )
        .expect("[collections] Failed to set refresh");

    lua.globals()
        .set("collections", collections_table)
        .expect("[collections] Failed to set collections table");
}
