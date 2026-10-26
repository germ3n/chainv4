//! EVM calldata for the CLI: Solidity's ABI, through crate::abi, for every type, to deploy and call
//! a contract from the command line. Arrays and tuples are written as JSON, such as [1, 2].

use {
    crate::abi::{AbiType, encode_values, parse_value},
    crate::crypto::hasher::Hasher,
    crate::structs::address::Address,
    crate::structs::hash::HashType,
    crate::structs::hex::from_hex,
    crate::structs::u256::u256,
};

/// An address as the EVM takes it: 0x and 40 hex digits, for an EVM contract, or any chainv4
/// address, such as secp256k1:0x… or ed25519:0x….
pub fn evm_address(text: &str) -> Result<Address, String> {
    if let Some(hex) = text.strip_prefix("0x")
        && hex.len() == 40
    {
        let bytes = from_hex(text).ok_or_else(|| format!("{text} is not hex"))?;

        return Ok(Address::Evm(bytes.try_into().map_err(|_| format!("{text} is not 20 bytes"))?));
    }

    text.parse().map_err(|err| format!("{text}: {err}"))
}

/// The types in a parameter list, such as "(uint256,address)" or "uint256, string[]".
fn types(list: &str) -> Result<Vec<AbiType>, String> {
    let list = list.trim();
    let list = match list.starts_with('(') && list.ends_with(')') {
        true => list.to_string(),
        false => format!("({list})"),
    };

    match AbiType::parse(&list)? {
        AbiType::Tuple(types) => Ok(types),
        _ => Err(format!("{list} is not a parameter list")),
    }
}

/// A function's selector: the first four bytes of its signature's keccak256, such as bump(uint256)'s.
pub fn selector(signature: &str) -> Vec<u8> {
    let compact: String = signature.chars().filter(|c| !c.is_whitespace()).collect();

    Hasher::digest(HashType::Keccak256, compact.as_bytes()).to_bytes()[..4].to_vec()
}

/// Calldata for a function: its selector and its arguments, encoded.
pub fn call(signature: &str, args: &[String]) -> Result<Vec<u8>, String> {
    let open = signature.find('(').ok_or_else(|| format!("{signature} is not a signature such as bump(uint256)"))?;
    let canonical = format!("{}{}", signature[..open].trim(), AbiType::Tuple(types(&signature[open..])?));

    Ok([selector(&canonical), encode(&signature[open..], args)?].concat())
}

/// Arguments encoded for a parameter list, as a constructor's follow its code: every Solidity type,
/// arrays and tuples written as JSON, such as [1, 2].
pub fn encode(list: &str, args: &[String]) -> Result<Vec<u8>, String> {
    let kinds = types(list)?;

    if kinds.len() != args.len() {
        return Err(format!("{} takes {} arguments, not {}", list.trim(), kinds.len(), args.len()));
    }

    let values = kinds.iter().zip(args).map(|(kind, arg)| parse_value(kind, arg).map_err(|why| format!("{arg} as {kind}: {why}"))).collect::<Result<Vec<_>, _>>()?;

    encode_values(&kinds, &values)
}

/// What a call returned, for a person: its words in hex, and a single word as a number too.
pub fn shown(output: &[u8]) -> String {
    match output.len() {
        0 => "nothing".to_string(),
        32 => format!("{} ({})", crate::structs::hex::to_hex(output), u256::from_be_bytes(output.try_into().expect("32 bytes"))),
        _ => crate::structs::hex::to_hex(output),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calldata_is_solidity_abi() {
        // bump(uint256) with 3, as solc and cast write it.
        assert_eq!(crate::structs::hex::to_hex(&call("bump(uint256)", &["3".to_string()]).unwrap()), format!("0xb20eb4c4{:064x}", 3));
        assert_eq!(selector("transfer(address, uint256)"), [0xa9, 0x05, 0x9c, 0xbb], "spaces do not count");

        let encoded = encode("(address,bool,int8,bytes2)", &["0x00000000000000000000000000000000000000ff".into(), "true".into(), "-1".into(), "0xbeef".into()]).unwrap();

        assert_eq!(encoded.len(), 128);
        assert_eq!(encoded[31], 0xff);
        assert_eq!(encoded[63], 1);
        assert!(encoded[64..96].iter().all(|byte| *byte == 0xff), "-1 in two's complement");
        assert_eq!(&encoded[96..98], &[0xbe, 0xef]);
    }

    #[test]
    fn what_does_not_fit_is_refused() {
        assert!(encode("(uint8)", &["256".into()]).is_err());
        assert!(encode("(int8)", &["128".into()]).is_err());
        assert!(encode("(uint256)", &[]).is_err(), "too few");
        assert_eq!(encode("(string)", &["hi".into()]).unwrap().len(), 96, "a string: its offset, its length and its bytes");
        assert_eq!(selector("set(uint256[])"), call("set(uint256[])", &["[1, 2]".into()]).unwrap()[..4].to_vec(), "arrays as JSON");
        assert_eq!(evm_address("0x00000000000000000000000000000000000000ff"), Ok(Address::Evm({
            let mut bytes = [0; 20];
            bytes[19] = 0xff;
            bytes
        })));
        assert_eq!(shown(&u256::new(8).to_be_bytes()), format!("0x{:064x} (8)", 8));
    }
}
