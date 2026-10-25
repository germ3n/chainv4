use {
    crate::crypto::hasher::Hasher,
    crate::structs::hash::{Hash, HashType},
    crate::vm::cost,
    crate::vm::lvm::{charge, to_hex},
    mlua::{Lua, MetaMethod, UserData, UserDataMethods, Value},
    r#macro::document,
};

/// The algorithms a script can name, in the order the library lists them.
const ALGORITHMS: &[(&str, HashType)] = &[
    #[cfg(feature = "sha256")]
    ("sha256", HashType::Sha256),
    #[cfg(feature = "blake3")]
    ("blake3", HashType::Blake3_256),
    #[cfg(feature = "blake2s")]
    ("blake2s", HashType::Blake2s256),
    #[cfg(feature = "blake2b")]
    ("blake2b", HashType::Blake2b256),
    #[cfg(feature = "xxh3")]
    ("xxh3", HashType::Xxh3_64),
    #[cfg(feature = "keccak256")]
    ("keccak256", HashType::Keccak256),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LuaHash(pub Hash);

impl UserData for LuaHash {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("hex", |_, this, ()| Ok(to_hex(&this.0.to_bytes())));
        methods.add_method("bytes", |lua, this, ()| lua.create_string(this.0.to_bytes()));
        methods.add_method("algorithm", |_, this, ()| Ok(algorithm_name(this.0.algorithm())));
        methods.add_meta_method(MetaMethod::ToString, |_, this, ()| Ok(to_hex(&this.0.to_bytes())));
        // either side may be a number, which the VM also hands to __eq
        methods.add_meta_function(MetaMethod::Eq, |_, (a, b): (Value, Value)| {
            let (Value::UserData(a), Value::UserData(b)) = (a, b) else {
                return Ok(false);
            };

            match (a.borrow::<LuaHash>(), b.borrow::<LuaHash>()) {
                (Ok(a), Ok(b)) => Ok(*a == *b),
                _ => Ok(false),
            }
        });
    }
}

fn algorithm_name(algorithm: HashType) -> &'static str {
    ALGORITHMS
        .iter()
        .find(|(_, candidate)| *candidate == algorithm)
        .map(|(name, _)| *name)
        .expect("every compiled-in algorithm is listed")
}

#[document(
    lua,
    kind = "library",
    name = "hash",
    category = "Chain",
    summary = "Hashes strings of bytes. Each function exists only when its algorithm's cargo feature is compiled in; all are by default.",
    note = "Each call costs 20 fuel plus 1 for every 64 bytes hashed.",
    example = "local id = hash.blake3(ctx.sender:bytes() .. \"nonce\")",
)]
fn hash_lib() {}

#[document(
    lua,
    parent = "hash",
    name = "sha256",
    summary = "SHA-256.",
    params = { data = { ty = "string", desc = "The bytes to hash." } },
    returns = { ty = "Hash", desc = "The digest." },
)]
fn hash_sha256() {}

#[document(
    lua,
    parent = "hash",
    name = "blake3",
    summary = "BLAKE3, 256 bits.",
    params = { data = { ty = "string", desc = "The bytes to hash." } },
    returns = { ty = "Hash", desc = "The digest." },
)]
fn hash_blake3() {}

#[document(
    lua,
    parent = "hash",
    name = "blake2s",
    summary = "BLAKE2s-256.",
    params = { data = { ty = "string", desc = "The bytes to hash." } },
    returns = { ty = "Hash", desc = "The digest." },
)]
fn hash_blake2s() {}

#[document(
    lua,
    parent = "hash",
    name = "blake2b",
    summary = "BLAKE2b-256.",
    params = { data = { ty = "string", desc = "The bytes to hash." } },
    returns = { ty = "Hash", desc = "The digest." },
)]
fn hash_blake2b() {}

#[document(
    lua,
    parent = "hash",
    name = "xxh3",
    summary = "XXH3-64, the hash contract calls name functions by. Not cryptographic.",
    params = { data = { ty = "string", desc = "The bytes to hash." } },
    returns = { ty = "Hash", desc = "The 64-bit hash." },
)]
fn hash_xxh3() {}

#[document(
    lua,
    parent = "hash",
    name = "keccak256",
    summary = "Keccak-256, the hash Ethereum uses.",
    params = { data = { ty = "string", desc = "The bytes to hash." } },
    returns = { ty = "Hash", desc = "The digest." },
)]
fn hash_keccak256() {}

#[document(
    lua,
    kind = "class",
    name = "Hash",
    category = "Chain",
    summary = "A digest, tagged with its algorithm. Compare with ==; tostring gives the 0x hex form.",
    see_also = "hash",
)]
fn hash_class() {}

#[document(
    lua,
    parent = "Hash",
    name = "hex",
    kind = "method",
    summary = "The digest as 0x and lowercase hex.",
    returns = { ty = "string", desc = "The hex form." },
)]
fn hash_hex() {}

#[document(
    lua,
    parent = "Hash",
    name = "bytes",
    kind = "method",
    summary = "The raw digest: 32 bytes, or 8 big-endian bytes for XXH3.",
    returns = { ty = "string", desc = "The digest bytes." },
)]
fn hash_bytes() {}

#[document(
    lua,
    parent = "Hash",
    name = "algorithm",
    kind = "method",
    summary = "The name of the algorithm, the same as the hash function that made it.",
    returns = { ty = "string", desc = "For example \"blake3\"." },
)]
fn hash_algorithm() {}

pub fn register_hash_lib(lua: &Lua) {
    let hash_table = lua.create_table().expect("[hash] Failed to create hash table");

    for &(name, algorithm) in ALGORITHMS {
        hash_table
            .set(
                name,
                lua.create_function(move |lua, data: mlua::String| {
                    let data = data.as_bytes();
                    charge(lua, cost::HASH + data.len() as u64 / cost::HASH_BYTES_PER_FUEL)?;

                    Ok(LuaHash(Hasher::digest(algorithm, &data)))
                })
                .expect("[hash] Failed to create hash function"),
            )
            .expect("[hash] Failed to set hash function");
    }

    lua.globals()
        .set("hash", hash_table)
        .expect("[hash] Failed to set hash table");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> Lua {
        let lua = Lua::new();
        lua.set_fuel(u64::MAX).unwrap();
        register_hash_lib(&lua);

        lua
    }

    #[test]
    fn every_algorithm_matches_the_hasher() {
        let lua = setup();

        for &(name, algorithm) in ALGORITHMS {
            let code = format!("local h = hash.{name}('abc') return h:hex(), h:algorithm()");
            let (hex, named): (String, String) = lua.load(code).eval().unwrap();

            assert_eq!(hex, to_hex(&Hasher::digest(algorithm, b"abc").to_bytes()), "{name}");
            assert_eq!(named, name);
        }
    }

    #[test]
    fn equal_digests_compare_equal() {
        let lua = setup();
        let code = "return hash.xxh3('a') == hash.xxh3('a'), hash.xxh3('a') == hash.xxh3('b')";
        let (same, different): (bool, bool) = lua.load(code).eval().unwrap();

        assert!(same);
        assert!(!different);
    }

    #[test]
    fn longer_input_costs_more_fuel() {
        let lua = setup();
        let mut costs = Vec::new();

        for size in [0, 64 * 100] {
            lua.set_fuel(1_000_000).unwrap();
            lua.load(format!("local _ = hash.xxh3(string.rep('x', {size}))"))
                .exec()
                .unwrap();
            costs.push(1_000_000 - lua.get_fuel().unwrap());
        }

        assert!(costs[1] >= costs[0] + 100, "{costs:?}");
    }
}
