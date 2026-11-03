use {
    super::accounts::refused,
    super::address::LuaAddress,
    super::integers::exact_as,
    crate::structs::u256::u256,
    crate::vm::lvm::{charge, costs},
    crate::vm::{Host, VmError},
    mlua::{AnyUserData, Error, Lua, Value},
    r#macro::document,
};

fn host(lua: &Lua) -> mlua::Result<Host> {
    lua.app_data_ref::<Host>()
        .map(|host| host.clone())
        .ok_or_else(|| Error::runtime("evm is only available inside a call"))
}

fn call(lua: &Lua, (to, input, value): (AnyUserData, mlua::String, Option<Value>)) -> mlua::Result<mlua::String> {
    let to = *to.borrow::<LuaAddress>()?;
    let value = match &value {
        Some(value) => exact_as::<u256>(value)?,
        None => u256::ZERO,
    };
    let host = host(lua)?;
    let input = input.as_bytes().to_vec();
    let transfer = if value == u256::ZERO { 0 } else { costs(lua).transfer };
    charge(lua, costs(lua).call + transfer + input.len() as u64 / costs(lua).copy_bytes_per_fuel)?;
    let fuel = lua.get_fuel()?;
    let outcome = host.evm_call(&to.0, &input, value, fuel);
    lua.set_fuel(fuel.saturating_sub(outcome.fuel_used))?;

    match outcome.result {
        Ok(returned) => match returned.as_slice() {
            [crate::structs::tx::TxCallArgumentType::Bytes(output)] => lua.create_string(output),
            _ => Err(Error::runtime("the EVM returned no bytes")),
        },
        Err(err @ VmError::Storage(_)) => Err(refused(err)),
        Err(err) => Err(Error::runtime(format!("EVM call to {} failed: {err}", to.0))),
    }
}

#[cfg(feature = "evm")]
mod abi {
    use {
        super::{charge, costs, host, refused},
        crate::abi::{AbiType, AbiValue, decode_values, encode_values, parse_signature},
        crate::vm::evm::{abi_value, native_from_abi},
        crate::vm::lvm::value,
        crate::vm::{Host, VmError},
        mlua::{Error, Lua, MultiValue, Table, Value},
    };

    /// How deep tables may nest as arrays and tuples.
    const MAX_DEPTH: usize = 16;

    fn bad(err: VmError) -> Error {
        match err {
            err @ VmError::Storage(_) => refused(err),
            other => Error::runtime(other.to_string()),
        }
    }

    pub fn encode(lua: &Lua, (signature, args): (String, MultiValue)) -> mlua::Result<mlua::String> {
        let host = host(lua)?;
        charge(lua, costs(lua).abi + signature.len() as u64 / costs(lua).copy_bytes_per_fuel)?;
        let (selector, types) = parse_signature(&signature).map_err(Error::runtime)?;

        if args.len() != types.len() {
            return Err(Error::runtime(format!("{signature} takes {} values, not {}", types.len(), args.len())));
        }

        let values = types.iter().zip(args).map(|(ty, arg)| to_abi(&host, ty, arg, 0)).collect::<mlua::Result<Vec<_>>>()?;
        let encoded = encode_values(&types, &values).map_err(Error::runtime)?;
        charge(lua, encoded.len() as u64 / costs(lua).copy_bytes_per_fuel)?;

        lua.create_string([selector.map(Vec::from).unwrap_or_default(), encoded].concat())
    }

    pub fn decode(lua: &Lua, (types, data): (String, mlua::String)) -> mlua::Result<MultiValue> {
        let host = host(lua)?;
        let data = data.as_bytes().to_vec();
        charge(lua, costs(lua).abi + (types.len() + data.len()) as u64 / costs(lua).copy_bytes_per_fuel)?;
        // A bare list, uint256,bool, reads as one in parentheses.
        let text = match types.trim().contains('(') {
            true => types.clone(),
            false => format!("({types})"),
        };
        let (selector, types) = parse_signature(&text).map_err(Error::runtime)?;
        let data = match selector {
            Some(selector) => data.strip_prefix(&selector[..]).ok_or_else(|| Error::runtime(format!("the calldata is not for {text}")))?,
            None => &data[..],
        };
        let values = decode_values(&types, data).map_err(Error::runtime)?;

        types.iter().zip(values).map(|(ty, value)| to_lua(lua, &host, ty, value)).collect::<mlua::Result<Vec<_>>>().map(MultiValue::from_vec)
    }

    /// A Lua value as an ABI value of a type: tables for arrays and tuples, in order, and for the
    /// rest what a contract function takes.
    fn to_abi(host: &Host, ty: &AbiType, arg: Value, depth: usize) -> mlua::Result<AbiValue> {
        if depth > MAX_DEPTH {
            return Err(Error::runtime("values nest too deep"));
        }

        // The type of each item, in order.
        let items = |table: &Table, types: Vec<&AbiType>| -> mlua::Result<Vec<AbiValue>> {
            if table.raw_len() != types.len() {
                return Err(Error::runtime(format!("a {ty} takes {} values, not {}", types.len(), table.raw_len())));
            }

            types.into_iter().enumerate().map(|(index, item)| to_abi(host, item, table.raw_get(index + 1)?, depth + 1)).collect()
        };

        match (ty, arg) {
            (AbiType::Array(inner), Value::Table(table)) => items(&table, vec![&**inner; table.raw_len()]).map(AbiValue::Array),
            (AbiType::FixedArray(inner, len), Value::Table(table)) => items(&table, vec![&**inner; *len]).map(AbiValue::Array),
            (AbiType::Tuple(types), Value::Table(table)) => items(&table, types.iter().collect()).map(AbiValue::Tuple),
            (AbiType::Array(_) | AbiType::FixedArray(..) | AbiType::Tuple(_), other) => Err(Error::runtime(format!("a {ty} is a table, not a {}", other.type_name()))),
            (ty, arg) => {
                let native = value::from_lua(arg).map_err(bad)?;

                abi_value(ty, &native, Some(host.state())).map_err(bad)
            }
        }
    }

    /// An ABI value as Lua: arrays and tuples as tables, the rest as a contract function gets it.
    fn to_lua(lua: &Lua, host: &Host, ty: &AbiType, value: AbiValue) -> mlua::Result<Value> {
        let list = |types: &mut dyn Iterator<Item = &AbiType>, values: Vec<AbiValue>| -> mlua::Result<Value> {
            let table = lua.create_table()?;

            for (ty, value) in types.zip(values) {
                table.raw_push(to_lua(lua, host, ty, value)?)?;
            }

            Ok(Value::Table(table))
        };

        match (ty, value) {
            (AbiType::Array(inner) | AbiType::FixedArray(inner, _), AbiValue::Array(values)) => list(&mut std::iter::repeat(&**inner), values),
            (AbiType::Tuple(types), AbiValue::Tuple(values)) => list(&mut types.iter(), values),
            (ty, value) => {
                let native = native_from_abi(ty, value, host.state()).map_err(bad)?;

                value::to_lua(lua, &native).map_err(bad)
            }
        }
    }
}

#[cfg(not(feature = "evm"))]
mod abi {
    use mlua::{Error, Lua, MultiValue};

    pub fn encode(_: &Lua, _: (String, MultiValue)) -> mlua::Result<mlua::String> {
        Err(Error::runtime("this node was built without the EVM"))
    }

    pub fn decode(_: &Lua, _: (String, mlua::String)) -> mlua::Result<MultiValue> {
        Err(Error::runtime("this node was built without the EVM"))
    }
}

#[document(
    lua,
    kind = "library",
    name = "evm",
    category = "Chain",
    summary = "Calls EVM contracts, and encodes and decodes what they take and give in Solidity's ABI. The EVM sees this contract at its alias, as msg.sender; what the call changes is kept only if it succeeds and this call does.",
    note = "A call costs 1000 fuel, 1000 more when it sends value, on top of the EVM gas it burns, at the block's rate of fuel to gas. Encoding and decoding cost 50 fuel and 1 per 64 bytes. On a chain without the EVM, call raises an error. EVM contracts call this one back through the native precompile, which enters a contract already running only through a function marked reentrant; calls nest at most 8 deep across engines.",
    example = "function M.bump(counter, by)\n    local output = evm.call(counter, evm.encode(\"bump(uint256)\", by))\n    return evm.decode(\"(uint256)\", output)\nend",
    see_also = "contracts.call",
)]
fn evm_lib() {}

#[document(
    lua,
    parent = "evm",
    name = "call",
    summary = "Calls an EVM contract with calldata, or pays any account, sending value from this contract's balance, as an EVM call transaction does. Raises an error when it reverts, with Solidity's reason, or runs out of gas; catch it with pcall to carry on.",
    params = {
        address = { ty = "Address", desc = "Whom to call: an EVM contract, evm:0x…, or any account, which the EVM reaches at its 20 bytes." },
        calldata = { ty = "string", desc = "The selector and arguments, as evm.encode writes them; empty for a plain payment." },
        value = { ty = "U256 | number", desc = "What to send.", optional = true },
    },
    returns = { ty = "string", desc = "What the EVM returned, as bytes: read it with evm.decode." },
    see_also = "evm.encode, evm.decode",
)]
fn evm_call() {}

#[document(
    lua,
    parent = "evm",
    name = "encode",
    summary = "Encodes values in Solidity's ABI. A signature with a name, transfer(address,uint256), gives calldata: the function's selector, then the arguments. Types alone in parentheses, (uint256,string), give the arguments only.",
    params = {
        signature = { ty = "string", desc = "The function's signature, or its types in parentheses, as Solidity writes them; uint alone is uint256." },
        values = { ty = "...", desc = "One value for each type: any integer or whole number for an integer type, an Address for address, which goes as its alias, true or false, a string for string and bytes, and a table for an array or a tuple.", optional = true },
    },
    returns = { ty = "string", desc = "The encoded bytes." },
    example = "local data = evm.encode(\"transfer(address,uint256)\", to, u256(5))",
)]
fn evm_encode() {}

#[document(
    lua,
    parent = "evm",
    name = "decode",
    summary = "Decodes bytes in Solidity's ABI: what an EVM call returned, a list of types, or calldata, a signature with a name, whose selector it checks.",
    params = {
        types = { ty = "string", desc = "Types in parentheses, (uint256,bool), or alone, uint256,bool; or a function's signature, for calldata." },
        data = { ty = "string", desc = "The bytes." },
    },
    returns = { ty = "...", desc = "One value for each type: integers as their own types, U64 for uint64, U256 for uint256 and so on; an address as the account behind it; bytes as a string; arrays and tuples as tables." },
    example = "local count, ok = evm.decode(\"(uint256,bool)\", output)",
)]
fn evm_decode() {}

pub fn register_evm_lib(lua: &Lua) {
    let evm_table = lua
        .create_table()
        .expect("[evm] Failed to create evm table");

    evm_table
        .set(
            "call",
            lua.create_function(call)
                .expect("[evm] Failed to create call"),
        )
        .expect("[evm] Failed to set call");
    evm_table
        .set(
            "encode",
            lua.create_function(abi::encode)
                .expect("[evm] Failed to create encode"),
        )
        .expect("[evm] Failed to set encode");
    evm_table
        .set(
            "decode",
            lua.create_function(abi::decode)
                .expect("[evm] Failed to create decode"),
        )
        .expect("[evm] Failed to set decode");

    lua.globals()
        .set("evm", evm_table)
        .expect("[evm] Failed to set evm table");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outside_a_call_there_is_no_evm_to_call() {
        let lua = Lua::new();
        lua.set_fuel(u64::MAX).unwrap();
        register_evm_lib(&lua);
        lua.globals().set("counter", LuaAddress(crate::structs::address::Address::Evm([1; 20]))).unwrap();
        let err = lua.load("evm.call(counter, '')").exec().unwrap_err();

        assert!(err.to_string().contains("only available inside a call"), "{err}");
    }
}
