//! EVM calldata for the CLI: Solidity's ABI for the static types, enough to deploy and call a
//! contract from the command line. Dynamic types (string, bytes, arrays) take --data, from a tool
//! such as Foundry's cast calldata.

use {
    crate::crypto::hasher::Hasher,
    crate::structs::address::Address,
    crate::structs::hash::HashType,
    crate::structs::hex::from_hex,
    crate::structs::u256::{i256, u256},
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

/// The types in a parameter list, such as "uint256,address" or "(uint256, address)".
fn types(list: &str) -> Vec<String> {
    let inner = list.trim().trim_start_matches('(').trim_end_matches(')');

    inner.split(',').map(|kind| kind.trim().to_string()).filter(|kind| !kind.is_empty()).collect()
}

/// A function's selector: the first four bytes of its signature's keccak256, such as bump(uint256)'s.
pub fn selector(signature: &str) -> Vec<u8> {
    let compact: String = signature.chars().filter(|c| !c.is_whitespace()).collect();

    Hasher::digest(HashType::Keccak256, compact.as_bytes()).to_bytes()[..4].to_vec()
}

/// Calldata for a function: its selector and its arguments, encoded.
pub fn call(signature: &str, args: &[String]) -> Result<Vec<u8>, String> {
    let open = signature.find('(').ok_or_else(|| format!("{signature} is not a signature such as bump(uint256)"))?;

    Ok([selector(signature), encode(&signature[open..], args)?].concat())
}

/// Arguments encoded for a parameter list, as a constructor's follow its code.
pub fn encode(list: &str, args: &[String]) -> Result<Vec<u8>, String> {
    let kinds = types(list);

    if kinds.len() != args.len() {
        return Err(format!("{} takes {} arguments, not {}", list.trim(), kinds.len(), args.len()));
    }

    let mut out = Vec::new();

    for (kind, arg) in kinds.iter().zip(args) {
        out.extend(word(kind, arg).map_err(|why| format!("{arg} as {kind}: {why}"))?);
    }

    Ok(out)
}

/// One static value as its 32-byte word.
fn word(kind: &str, arg: &str) -> Result<[u8; 32], String> {
    let number = |text: &str| -> Result<u256, String> {
        match text.strip_prefix("0x") {
            Some(hex) => u256::from_str_radix(hex, 16).map_err(|_| "not a hex number".to_string()),
            None => text.parse().map_err(|_| "not a number".to_string()),
        }
    };

    if let Some(bits) = kind.strip_prefix("uint") {
        let bits: u32 = if bits.is_empty() { 256 } else { bits.parse().map_err(|_| "no such type".to_string())? };
        let value = number(arg)?;

        if bits < 256 && value >> bits != u256::ZERO {
            return Err(format!("more than {bits} bits"));
        }

        return Ok(value.to_be_bytes());
    }

    if let Some(bits) = kind.strip_prefix("int") {
        let bits: u32 = if bits.is_empty() { 256 } else { bits.parse().map_err(|_| "no such type".to_string())? };
        let value: i256 = arg.parse().map_err(|_| "not a whole number".to_string())?;
        let half = i256::ONE << (bits - 1);

        if bits < 256 && (value >= half || value < -half) {
            return Err(format!("does not fit in {bits} bits"));
        }

        return Ok(value.to_be_bytes());
    }

    match kind {
        "address" => {
            let mut out = [0; 32];
            out[12..].copy_from_slice(&crate::vm::evm::evm_alias(&evm_address(arg)?));

            Ok(out)
        }
        "bool" => match arg {
            "true" => Ok(u256::ONE.to_be_bytes()),
            "false" => Ok([0; 32]),
            _ => Err("true or false".to_string()),
        },
        bytes if bytes.starts_with("bytes") && bytes.len() > 5 => {
            let size: usize = bytes[5..].parse().map_err(|_| "no such type".to_string())?;
            let data = from_hex(arg).ok_or("not hex")?;

            if size == 0 || size > 32 || data.len() != size {
                return Err(format!("{size} bytes of hex"));
            }

            let mut out = [0; 32];
            out[..size].copy_from_slice(&data);

            Ok(out)
        }
        _ => Err("only static types here: uint, int, address, bool and bytes1 to bytes32; give --data for the rest, such as cast calldata writes".to_string()),
    }
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
        assert!(encode("(string)", &["hi".into()]).is_err(), "dynamic types take --data");
        assert_eq!(evm_address("0x00000000000000000000000000000000000000ff"), Ok(Address::Evm({
            let mut bytes = [0; 20];
            bytes[19] = 0xff;
            bytes
        })));
        assert_eq!(shown(&u256::new(8).to_be_bytes()), format!("0x{:064x} (8)", 8));
    }
}
