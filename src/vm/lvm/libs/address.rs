use {
    crate::crypto::verifiable::Verifiable,
    crate::structs::address::Address,
    crate::vm::lvm::{from_hex, to_hex},
    mlua::{Error, Lua, MetaMethod, Table, UserData, UserDataMethods, Value},
    r#macro::document,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LuaAddress(pub Address);

impl UserData for LuaAddress {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("hex", |_, this, ()| Ok(to_hex(this.0.bytes())));
        methods.add_method("bytes", |lua, this, ()| lua.create_string(this.0.bytes()));
        methods.add_method("is_valid", |_, this, ()| Ok(this.0.verify().is_ok()));
        methods.add_method("is_contract", |_, this, ()| {
            Ok(matches!(this.0, Address::Contract(_)))
        });
        methods.add_method("kind", |_, this, ()| Ok(this.0.kind()));
        methods.add_method("text", |_, this, ()| Ok(this.0.to_string()));
        methods.add_method("encode", |lua, this, ()| lua.create_string(this.0.to_abi()));
        methods.add_meta_method(MetaMethod::ToString, |_, this, ()| Ok(to_hex(this.0.bytes())));
        // either side may be a number, which the VM also hands to __eq
        methods.add_meta_function(MetaMethod::Eq, |_, (a, b): (Value, Value)| {
            let (Value::UserData(a), Value::UserData(b)) = (a, b) else {
                return Ok(false);
            };

            match (a.borrow::<LuaAddress>(), b.borrow::<LuaAddress>()) {
                (Ok(a), Ok(b)) => Ok(*a == *b),
                _ => Ok(false),
            }
        });
    }
}

/// An address from its raw bytes: 32 are an Ed25519 key's, 20 an Ethereum one's.
fn from_bytes(bytes: &[u8]) -> mlua::Result<LuaAddress> {
    match bytes.len() {
        32 => Ok(LuaAddress(Address::Ed25519(bytes.try_into().expect("32 bytes")))),
        20 => Ok(LuaAddress(Address::Secp256k1(bytes.try_into().expect("20 bytes")))),
        _ => Err(Error::runtime("an address is 32 bytes, or 20 for an Ethereum one")),
    }
}

fn parse_text(text: &str) -> mlua::Result<LuaAddress> {
    text.parse()
        .map(LuaAddress)
        .map_err(|err: crate::structs::address::ParseAddressError| Error::runtime(format!("{err}: {text}")))
}

fn decode(bytes: &[u8]) -> mlua::Result<LuaAddress> {
    Address::from_abi(bytes)
        .map(LuaAddress)
        .ok_or_else(|| Error::runtime("not an encoded address: 33 bytes, its kind's tag then its bytes"))
}

fn parse_hex(text: &str) -> mlua::Result<LuaAddress> {
    let bytes = from_hex(text).ok_or_else(|| Error::runtime(format!("not hex: {text}")))?;

    from_bytes(&bytes)
}

#[document(
    lua,
    kind = "library",
    name = "address",
    category = "Chain",
    summary = "Makes addresses. address(hex) is short for address.from_hex(hex).",
    example = "local treasury = address(\"0x3b6a27bcceb6a42d62a3a8d02a6f0d73653215771de243a63ac048a18b59da29\")",
)]
fn address_lib() {}

#[document(
    lua,
    parent = "address",
    name = "from_hex",
    summary = "Reads an address from hex, with or without a 0x prefix: 64 digits are an Ed25519 key's, 40 an Ethereum (secp256k1) one's.",
    params = {
        hex = { ty = "string", desc = "The address in hex." },
    },
    returns = { ty = "Address", desc = "The address. It is not checked; see Address:is_valid." },
    panics = "Errors when the text is not hex, or neither 32 nor 20 bytes long.",
    see_also = "address.from_bytes, Address:is_valid",
)]
fn address_from_hex() {}

#[document(
    lua,
    parent = "address",
    name = "parse",
    summary = "Reads an address of any kind from its full text form, as Address:text writes it and the CLI and API show it. The way to store a contract's address and get it back.",
    params = {
        text = { ty = "string", desc = "Such as ed25519:0x…, secp256k1:0x…, contract:0x… or a bitcoin address's bc1q…." },
    },
    returns = { ty = "Address", desc = "The address." },
    panics = "Errors when the kind is unknown or the hex is the wrong length for it: 32 bytes, or 20 for secp256k1; or a bc1q… address's checksum does not hold.",
    see_also = "Address:text",
)]
fn address_parse() {}

#[document(
    lua,
    parent = "address",
    name = "from_bytes",
    summary = "Makes an address from its raw bytes: 32 are an Ed25519 key's, 20 an Ethereum (secp256k1) one's.",
    params = {
        bytes = { ty = "string", desc = "Exactly 32 bytes, or 20." },
    },
    returns = { ty = "Address", desc = "The address." },
    panics = "Errors when the string is neither 32 nor 20 bytes long.",
    see_also = "Address:bytes",
)]
fn address_from_bytes() {}

#[document(
    lua,
    kind = "class",
    name = "Address",
    category = "Chain",
    summary = "An account on the chain. Compare with ==; tostring gives the 0x hex form.",
    see_also = "address.from_hex, ctx.sender",
)]
fn address_class() {}

#[document(
    lua,
    parent = "Address",
    name = "hex",
    kind = "method",
    summary = "The address as 0x followed by lowercase hex digits: 64, or 40 for secp256k1 and bitcoin.",
    returns = { ty = "string", desc = "The hex form." },
)]
fn address_hex() {}

#[document(
    lua,
    parent = "Address",
    name = "bytes",
    kind = "method",
    summary = "The address's raw bytes: 32, or 20 for secp256k1 and bitcoin.",
    returns = { ty = "string", desc = "The bytes." },
)]
fn address_bytes() {}

#[document(
    lua,
    parent = "Address",
    name = "is_contract",
    kind = "method",
    summary = "Whether the address belongs to a deployed contract rather than a key.",
    returns = { ty = "boolean", desc = "True for a contract." },
    see_also = "ctx.contract",
)]
fn address_is_contract() {}

#[document(
    lua,
    parent = "Address",
    name = "kind",
    kind = "method",
    summary = "What kind of address it is: ed25519, edblake, secp256k1, bitcoin, contract or evm.",
    returns = { ty = "string", desc = "The kind." },
)]
fn address_kind() {}

#[document(
    lua,
    parent = "Address",
    name = "text",
    kind = "method",
    summary = "The full text form: the kind, a colon and the hex, such as contract:0x…, or for a bitcoin address bc1q…, as Bitcoin writes it. Unlike hex, it keeps the kind, so address.parse gets the same address back.",
    returns = { ty = "string", desc = "The text form." },
    see_also = "address.parse",
)]
fn address_text() {}

#[document(
    lua,
    parent = "Address",
    name = "is_valid",
    kind = "method",
    summary = "Whether the address is a usable public key: a point on the curve that is not a weak small-order point. Funds sent to an invalid address can never be spent. A secp256k1 or bitcoin address is a hash of its key, so nothing can be checked, and it is always valid; so is a contract's.",
    returns = { ty = "boolean", desc = "True when the key is valid." },
)]
fn address_is_valid() {}

#[document(
    lua,
    parent = "Address",
    name = "encode",
    kind = "method",
    summary = "The 33 bytes Rust contracts keep an address in: its kind's tag, its bytes, and zeros after a kind shorter than 32. The key or value of a layout's address kind, so a Lua version reaches what a Rust version kept under addresses.",
    returns = { ty = "string", desc = "The 33 bytes." },
    see_also = "address.decode, Namespace:get_address",
)]
fn address_encode() {}

#[document(
    lua,
    parent = "address",
    name = "decode",
    summary = "Reads the 33 bytes Address:encode writes, and Rust contracts keep addresses in.",
    params = { bytes = { ty = "string", desc = "Exactly 33 bytes." } },
    returns = { ty = "Address", desc = "The address." },
    panics = "Errors for anything that is not an address in that form.",
    see_also = "Address:encode",
)]
fn address_decode() {}

pub fn register_address_lib(lua: &Lua) {
    let address_table = lua
        .create_table()
        .expect("[address] Failed to create address table");
    address_table
        .set(
            "from_hex",
            lua.create_function(|_, text: String| parse_hex(&text))
                .expect("[address] Failed to create from_hex"),
        )
        .expect("[address] Failed to set from_hex");
    address_table
        .set(
            "parse",
            lua.create_function(|_, text: String| parse_text(&text))
                .expect("[address] Failed to create parse"),
        )
        .expect("[address] Failed to set parse");
    address_table
        .set(
            "from_bytes",
            lua.create_function(|_, bytes: mlua::String| from_bytes(&bytes.as_bytes()))
                .expect("[address] Failed to create from_bytes"),
        )
        .expect("[address] Failed to set from_bytes");
    address_table
        .set(
            "decode",
            lua.create_function(|_, bytes: mlua::String| decode(&bytes.as_bytes()))
                .expect("[address] Failed to create decode"),
        )
        .expect("[address] Failed to set decode");

    let meta = lua
        .create_table()
        .expect("[address] Failed to create metatable");
    meta.set(
        "__call",
        lua.create_function(|_, (_, text): (Table, String)| parse_hex(&text))
            .expect("[address] Failed to create __call"),
    )
    .expect("[address] Failed to set __call");
    address_table.set_metatable(Some(meta));
    lua.globals()
        .set("address", address_table)
        .expect("[address] Failed to set address table");
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;

    fn setup() -> Lua {
        let lua = Lua::new();
        lua.set_fuel(u64::MAX).unwrap();
        register_address_lib(&lua);

        lua
    }

    fn real_key() -> String {
        to_hex(&SigningKey::from_bytes(&[1; 32]).verifying_key().to_bytes())
    }

    #[test]
    fn hex_round_trips_with_or_without_prefix() {
        let lua = setup();
        let key = real_key();
        let code = format!(
            "local a = address('{key}') return a:hex(), tostring(a), a == address.from_hex('{}'), #a:bytes()",
            &key[2..]
        );
        let (hex, text, same, len): (String, String, bool, u32) = lua.load(code).eval().unwrap();

        assert_eq!(hex, key);
        assert_eq!(text, key);
        assert!(same);
        assert_eq!(len, 32);
    }

    #[test]
    fn bytes_round_trip() {
        let lua = setup();
        let code = format!("local a = address('{}') return address.from_bytes(a:bytes()) == a", real_key());

        assert!(lua.load(code).eval::<bool>().unwrap());
    }

    #[test]
    fn validity_is_the_curve_check() {
        let lua = setup();
        let identity = format!("0x01{}", "00".repeat(31));
        let code = format!(
            "return address('{}'):is_valid(), address('{identity}'):is_valid()",
            real_key()
        );
        let (real, weak): (bool, bool) = lua.load(code).eval().unwrap();

        assert!(real);
        assert!(!weak);
    }

    #[test]
    fn bad_input_is_rejected() {
        let lua = setup();

        assert!(lua.load("address('0x1234')").exec().unwrap_err().to_string().contains("32 bytes, or 20"));
        assert!(lua.load("address('nothex')").exec().unwrap_err().to_string().contains("not hex"));
        assert!(lua.load("address.from_bytes('short')").exec().is_err());
    }

    #[test]
    fn the_text_form_keeps_the_kind() {
        let lua = setup();
        let text = format!("contract:0x{}", "ab".repeat(32));
        let code = format!("local a = address.parse('{text}') return a:text(), a:kind(), a:is_contract(), a == address.parse(a:text())");
        let (round, kind, contract, same): (String, String, bool, bool) = lua.load(code).eval().unwrap();

        assert_eq!((round, kind.as_str(), contract, same), (text, "contract", true, true));
        assert!(lua.load("address.parse('rsa:0x00')").exec().is_err());
    }

    #[test]
    fn twenty_bytes_are_an_ethereum_address() {
        let lua = setup();
        let hex = "0x2c7536e3605d9c16a7a3d7b1898e529396a65c23";
        let code = format!(
            "local a = address('{hex}') return a:text(), a:kind(), a:hex(), #a:bytes(), a == address.from_bytes(a:bytes()), a == address.parse(a:text()), a:is_valid()"
        );
        let (text, kind, round, len, bytes, parsed, valid): (String, String, String, u32, bool, bool, bool) = lua.load(code).eval().unwrap();

        assert_eq!((text, kind.as_str(), round.as_str(), len), (format!("secp256k1:{hex}"), "secp256k1", hex, 20));
        assert!(bytes && parsed && valid);
    }
    #[test]
    fn a_bitcoin_address_reads_and_writes_as_bitcoin_does() {
        let lua = setup();
        let text = "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4";
        let code = format!("local a = address.parse('{text}') return a:text(), a:kind(), a:hex(), a == address.decode(a:encode()), a:is_valid()");
        let (round, kind, hex, decoded, valid): (String, String, String, bool, bool) = lua.load(code).eval().unwrap();

        assert_eq!((round.as_str(), kind.as_str(), hex.as_str()), (text, "bitcoin", "0x751e76e8199196d454941c45d1b3a323f1433bd6"));
        assert!(decoded && valid);
        assert!(lua.load("address.parse('bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t5')").exec().is_err(), "a typo fails the checksum");
    }
}
