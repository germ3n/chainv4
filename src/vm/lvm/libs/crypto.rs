use {
    super::address::LuaAddress,
    super::hash::LuaHash,
    crate::vm::lvm::{charge, costs},
    crate::vm::{Host, crypto},
    mlua::{Error, Lua, ObjectLike, Value},
    r#macro::document,
};

fn host(lua: &Lua) -> mlua::Result<Host> {
    lua.app_data_ref::<Host>()
        .map(|host| host.clone())
        .ok_or_else(|| Error::runtime("crypto's proofs are only available inside a call"))
}

/// Bytes a script hands over: a string, or a Hash, for roots and digests.
fn bytes(value: &Value, what: &str) -> mlua::Result<Vec<u8>> {
    match value {
        Value::String(text) => Ok(text.as_bytes().to_vec()),
        Value::UserData(data) => Ok(data.borrow::<LuaHash>().map_err(|_| Error::runtime(format!("{what} is a string of bytes or a Hash")))?.0.to_bytes()),
        _ => Err(Error::runtime(format!("{what} is a string of bytes or a Hash"))),
    }
}

/// A whole number a script hands over, as a plain number or one of the integer types.
fn whole(value: &Value, what: &str) -> mlua::Result<u64> {
    let refused = || Error::runtime(format!("{what} is a whole number"));

    match value {
        Value::Integer(number) => u64::try_from(*number).map_err(|_| refused()),
        Value::Number(number) if number.fract() == 0.0 && *number >= 0.0 => Ok(*number as u64),
        Value::UserData(data) => data.call_method::<f64>("tonumber", ()).ok().filter(|number| number.fract() == 0.0 && *number >= 0.0).map(|number| number as u64).ok_or_else(refused),
        _ => Err(refused()),
    }
}

fn verify(lua: &Lua, (scheme, key, message, signature): (String, mlua::String, mlua::String, mlua::String)) -> mlua::Result<bool> {
    let message = message.as_bytes();
    charge(lua, costs(lua).verify + message.len() as u64 / costs(lua).hash_bytes_per_fuel)?;

    crypto::check_signature(&scheme, &key.as_bytes(), &message, &signature.as_bytes()).map_err(Error::runtime)
}

#[cfg(feature = "secp256k1")]
fn recover(lua: &Lua, (digest, signature): (Value, mlua::String)) -> mlua::Result<Option<mlua::String>> {
    charge(lua, costs(lua).recover)?;
    let digest: [u8; 32] = bytes(&digest, "the digest")?.try_into().map_err(|_| Error::runtime("the digest is 32 bytes"))?;

    crypto::recover_key(&digest, &signature.as_bytes()).map(|key| lua.create_string(key)).transpose()
}

fn address(lua: &Lua, (scheme, key): (String, mlua::String)) -> mlua::Result<LuaAddress> {
    charge(lua, costs(lua).key_address)?;

    crypto::key_address(&scheme, &key.as_bytes()).map(LuaAddress).map_err(Error::runtime)
}

fn digest(lua: &Lua, name: &'static str, data: mlua::String) -> mlua::Result<mlua::String> {
    let data = data.as_bytes();
    charge(lua, costs(lua).hash + data.len() as u64 / costs(lua).hash_bytes_per_fuel)?;

    lua.create_string(crypto::digest(name, &data).ok_or_else(|| Error::runtime(format!("{name} is not in this build")))?)
}

fn check_state(lua: &Lua, (root, column, key, value, proof): (Value, String, mlua::String, Option<mlua::String>, mlua::String)) -> mlua::Result<bool> {
    let proof = proof.as_bytes();
    charge(lua, costs(lua).check_proof + costs(lua).proof_sibling * (proof.len() as u64 / 32))?;
    let hash = host(lua)?.chain_hash();
    let value = value.map(|value| value.as_bytes().to_vec());

    Ok(crypto::check_state(hash, &bytes(&root, "the root")?, &column, &key.as_bytes(), value.as_deref(), &proof))
}

fn check_merkle(lua: &Lua, (root, data, index, count, path): (Value, mlua::String, Value, Value, mlua::String)) -> mlua::Result<bool> {
    let path = path.as_bytes();
    charge(lua, costs(lua).check_proof + costs(lua).proof_sibling * (path.len() as u64 / 32))?;
    let hash = host(lua)?.chain_hash();

    Ok(crypto::check_merkle(hash, &bytes(&root, "the root")?, &data.as_bytes(), whole(&index, "the index")?, whole(&count, "the count")?, &path))
}

#[document(
    lua,
    kind = "library",
    name = "crypto",
    category = "Chain",
    summary = "Signatures of every scheme the chain has, secp256k1 recovery as Ethereum's ecrecover does, the address a key controls, Bitcoin's hashes, and chainv4's own proofs: what a contract checks of another chain, a signer, or a header, without trusting whoever sends it.",
    note = "verify and recover cost 3,000 fuel, as the EVM's ecrecover does, verify 1 more for every 64 bytes of message; the hashes cost what the hash library's do; a proof 200, and 60 for every 32 bytes of it.",
    example = "local key = crypto.recover(digest, signature)\nassert(key and key == storage.get(\"signer\"), \"not the signer\")",
    see_also = "hash",
)]
fn crypto_lib() {}

#[document(
    lua,
    parent = "crypto",
    name = "verify",
    summary = "Whether a signature over a message is a key's, by its scheme's own rules, as chainv4's keys sign: strict Ed25519 and EdBlake, low-s ECDSA over the keccak256 for secp256k1, over the double SHA-256 for Bitcoin.",
    params = {
        scheme = { ty = "string", desc = "ed25519, edblake, secp256k1 or bitcoin, those the chain has." },
        key = { ty = "string", desc = "The key's bytes: 32, or 33 compressed for secp256k1 and Bitcoin." },
        message = { ty = "string", desc = "The bytes signed." },
        signature = { ty = "string", desc = "64 bytes." },
    },
    returns = { ty = "boolean", desc = "Whether it holds." },
    panics = "Errors for a scheme the chain lacks, or a key of the wrong length.",
)]
fn crypto_verify() {}

#[document(
    lua,
    parent = "crypto",
    name = "recover",
    summary = "The secp256k1 key that signed a 32-byte digest, as Ethereum's ecrecover finds it, compressed.",
    params = {
        digest = { ty = "string | Hash", desc = "The digest signed, 32 bytes." },
        signature = { ty = "string", desc = "r, s and v, 65 bytes, v 0, 1, 27 or 28." },
    },
    returns = { ty = "string | nil", desc = "The key, 33 bytes, or nil for a signature that recovers none, high-s ones included." },
)]
fn crypto_recover() {}

#[document(
    lua,
    parent = "crypto",
    name = "address",
    summary = "The account a key controls, as the chain writes it.",
    params = {
        scheme = { ty = "string", desc = "ed25519, edblake, secp256k1 or bitcoin." },
        key = { ty = "string", desc = "The key's bytes." },
    },
    returns = { ty = "Address", desc = "Such as ed25519:0x…, secp256k1:0x… (Ethereum's) or bc1q…." },
)]
fn crypto_address() {}

#[document(lua, parent = "crypto", name = "ripemd160", summary = "RIPEMD-160.", params = { data = { ty = "string", desc = "The bytes to hash." } }, returns = { ty = "string", desc = "20 bytes." })]
fn crypto_ripemd160() {}

#[document(lua, parent = "crypto", name = "hash160", summary = "Bitcoin's HASH160: RIPEMD-160 of SHA-256, what its addresses are made of.", params = { data = { ty = "string", desc = "The bytes to hash." } }, returns = { ty = "string", desc = "20 bytes." })]
fn crypto_hash160() {}

#[document(lua, parent = "crypto", name = "sha256d", summary = "SHA-256 twice, as Bitcoin makes block and transaction ids, in its byte order: reverse it for the hex explorers show.", params = { data = { ty = "string", desc = "The bytes to hash." } }, returns = { ty = "string", desc = "32 bytes." })]
fn crypto_sha256d() {}

#[document(
    lua,
    parent = "crypto",
    name = "check_state",
    summary = "Whether a state proof shows a chain-state entry holds a value under a state root, or, given nil, holds nothing: what a light client checks values by.",
    params = {
        root = { ty = "string | Hash", desc = "The state root, from a header the contract trusts." },
        column = { ty = "string", desc = "The column, such as accounts or namespaces." },
        key = { ty = "string", desc = "The entry's key." },
        value = { ty = "string | nil", desc = "What it holds, or nil for nothing.", optional = true },
        proof = { ty = "string", desc = "The proof, as GET /proofs encodes it." },
    },
    returns = { ty = "boolean", desc = "Whether it shows that." },
)]
fn crypto_check_state() {}

#[document(
    lua,
    parent = "crypto",
    name = "check_merkle",
    summary = "Whether data is the index-th of count leaves under a Merkle root, by its audit path: how a header's tx_root and receipts_root hold transactions and receipts.",
    params = {
        root = { ty = "string | Hash", desc = "The root." },
        data = { ty = "string", desc = "The leaf's bytes." },
        index = { ty = "integer", desc = "Its place, from 0." },
        count = { ty = "integer", desc = "How many leaves." },
        path = { ty = "string", desc = "The siblings, 32 bytes each, from the leaf up." },
    },
    returns = { ty = "boolean", desc = "Whether it is so." },
)]
fn crypto_check_merkle() {}

pub fn register_crypto_lib(lua: &Lua) {
    let table = lua.create_table().expect("[crypto] Failed to create crypto table");
    let set = |name: &str, function: mlua::Function| table.set(name, function).expect("[crypto] Failed to set a function");

    set("verify", lua.create_function(verify).expect("[crypto] Failed to create verify"));
    #[cfg(feature = "secp256k1")]
    set("recover", lua.create_function(recover).expect("[crypto] Failed to create recover"));
    set("address", lua.create_function(address).expect("[crypto] Failed to create address"));

    for name in ["ripemd160", "hash160", "sha256d"] {
        set(name, lua.create_function(move |lua, data: mlua::String| digest(lua, name, data)).expect("[crypto] Failed to create a hash"));
    }

    set("check_state", lua.create_function(check_state).expect("[crypto] Failed to create check_state"));
    set("check_merkle", lua.create_function(check_merkle).expect("[crypto] Failed to create check_merkle"));

    lua.globals().set("crypto", table).expect("[crypto] Failed to set crypto table");
}
