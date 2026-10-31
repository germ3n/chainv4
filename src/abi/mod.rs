//! Solidity's ABI: how EVM contracts take arguments and give back results and logs. Every type,
//! dynamic ones included: strings, bytes, arrays and tuples. A contract's JSON ABI, as solc and
//! Foundry write it, names its functions, events and errors. The CLI, the wallet and the explorer
//! all speak to EVM contracts through this.
//!
//! Also the 20 bytes an account goes by inside the EVM, and where CREATE puts a contract, which
//! clients work out without running an EVM.

use {
    crate::crypto::hasher::Hasher,
    crate::db::address_key,
    crate::structs::address::Address,
    crate::structs::hash::HashType,
    crate::structs::hex::{from_hex, to_hex},
    crate::structs::u256::{i256, u256},
    r#macro::document,
    serde_json::Value,
    std::cell::Cell,
    std::fmt,
};

/// How deep types and values may nest: past this, input is refused rather than recursed into.
const MAX_DEPTH: usize = 16;

fn keccak(bytes: &[u8]) -> [u8; 32] {
    Hasher::digest(HashType::Keccak256, bytes).to_bytes().try_into().expect("keccak256 is 32 bytes")
}

#[document(
    summary = "The 20 bytes an address goes by inside the EVM: a secp256k1 account's or an EVM contract's own, and for any other account, the last 20 bytes of the keccak256 of its key, as stored: its alias.",
    category = "ABI",
    see_also = "evm_create_address",
)]
pub fn evm_address(address: &Address) -> [u8; 20] {
    match address {
        Address::Secp256k1(bytes) | Address::Evm(bytes) => *bytes,
        other => keccak(&address_key(other))[12..].try_into().expect("32 bytes hold 20"),
    }
}

#[document(
    summary = "Where an EVM contract a deployer creates goes, as Ethereum's CREATE puts it: the last 20 bytes of the keccak256 of the RLP of the deployer's 20 bytes and its nonce.",
    category = "ABI",
    see_also = "evm_address",
)]
pub fn evm_create_address(deployer: [u8; 20], nonce: u64) -> [u8; 20] {
    let mut body = vec![0x94];
    body.extend_from_slice(&deployer);

    match nonce {
        0 => body.push(0x80),
        low @ 1..=0x7f => body.push(low as u8),
        nonce => {
            let digits: Vec<u8> = nonce.to_be_bytes().into_iter().skip_while(|digit| *digit == 0).collect();
            body.push(0x80 + digits.len() as u8);
            body.extend(digits);
        }
    }

    let mut list = vec![0xc0 + body.len() as u8];
    list.extend(body);

    keccak(&list)[12..].try_into().expect("32 bytes hold 20")
}

#[document(summary = "A Solidity type, as an ABI names it.", category = "ABI", see_also = "AbiValue")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AbiType {
    #[document(summary = "uint8 to uint256: the width in bits.")]
    Uint(u16),
    #[document(summary = "int8 to int256.")]
    Int(u16),
    Address,
    Bool,
    #[document(summary = "bytes1 to bytes32.")]
    FixedBytes(u8),
    #[document(summary = "bytes, of any length.")]
    Bytes,
    String,
    #[document(summary = "T[], of any length.")]
    Array(Box<AbiType>),
    #[document(summary = "T[k].")]
    FixedArray(Box<AbiType>, usize),
    #[document(summary = "(T1, T2, …): a struct.")]
    Tuple(Vec<AbiType>),
}

impl AbiType {
    #[document(parent = "AbiType", summary = "Reads a type as Solidity writes it: uint256, bytes32[], (address,uint256)[2]. uint and int alone are 256 bits.")]
    pub fn parse(text: &str) -> Result<AbiType, String> {
        parse_type(text.trim(), 0)
    }

    #[document(parent = "AbiType", summary = "Whether its encoding's length depends on its value: bytes, string, T[], and anything holding one.")]
    pub fn is_dynamic(&self) -> bool {
        match self {
            AbiType::Bytes | AbiType::String | AbiType::Array(_) => true,
            AbiType::FixedArray(inner, _) => inner.is_dynamic(),
            AbiType::Tuple(types) => types.iter().any(AbiType::is_dynamic),
            _ => false,
        }
    }

    /// What a static type takes in place: 32 bytes a word.
    fn head_size(&self) -> usize {
        match self {
            AbiType::FixedArray(inner, len) if !self.is_dynamic() => inner.head_size() * len,
            AbiType::Tuple(types) if !self.is_dynamic() => types.iter().map(AbiType::head_size).sum(),
            _ => 32,
        }
    }
}

impl fmt::Display for AbiType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AbiType::Uint(bits) => write!(f, "uint{bits}"),
            AbiType::Int(bits) => write!(f, "int{bits}"),
            AbiType::Address => f.write_str("address"),
            AbiType::Bool => f.write_str("bool"),
            AbiType::FixedBytes(size) => write!(f, "bytes{size}"),
            AbiType::Bytes => f.write_str("bytes"),
            AbiType::String => f.write_str("string"),
            AbiType::Array(inner) => write!(f, "{inner}[]"),
            AbiType::FixedArray(inner, len) => write!(f, "{inner}[{len}]"),
            AbiType::Tuple(types) => write!(f, "({})", types.iter().map(ToString::to_string).collect::<Vec<_>>().join(",")),
        }
    }
}

fn parse_type(text: &str, depth: usize) -> Result<AbiType, String> {
    if depth > MAX_DEPTH {
        return Err("types nest too deep".to_string());
    }

    // Array suffixes, outermost last: T[2][] is an array of T[2].
    if let Some(open) = text.strip_suffix(']').and_then(|rest| rest.rfind('[')) {
        let (inner, len) = (&text[..open], &text[open + 1..text.len() - 1]);
        let inner = Box::new(parse_type(inner, depth + 1)?);

        return match len {
            "" => Ok(AbiType::Array(inner)),
            len => Ok(AbiType::FixedArray(inner, len.parse().map_err(|_| format!("{text}: {len} is no length"))?)),
        };
    }

    if let Some(inner) = text.strip_prefix('(').and_then(|rest| rest.strip_suffix(')')) {
        return Ok(AbiType::Tuple(split_top(inner).into_iter().filter(|part| !part.is_empty()).map(|part| parse_type(part, depth + 1)).collect::<Result<_, _>>()?));
    }

    let bits = |rest: &str| -> Result<u16, String> {
        let bits: u16 = match rest {
            "" => 256,
            rest => rest.parse().map_err(|_| format!("{text} is no type"))?,
        };

        match bits.is_multiple_of(8) && (8..=256).contains(&bits) {
            true => Ok(bits),
            false => Err(format!("{text}: a width is 8 to 256 bits, by eights")),
        }
    };

    match text {
        "address" => Ok(AbiType::Address),
        "bool" => Ok(AbiType::Bool),
        "bytes" => Ok(AbiType::Bytes),
        "string" => Ok(AbiType::String),
        _ if text.starts_with("uint") => Ok(AbiType::Uint(bits(&text[4..])?)),
        _ if text.starts_with("int") => Ok(AbiType::Int(bits(&text[3..])?)),
        _ if text.starts_with("bytes") => match text[5..].parse::<u8>() {
            Ok(size @ 1..=32) => Ok(AbiType::FixedBytes(size)),
            _ => Err(format!("{text}: bytes1 to bytes32")),
        },
        _ => Err(format!("{text} is no type this reads")),
    }
}

/// Splits a list at the commas outside any parentheses.
fn split_top(list: &str) -> Vec<&str> {
    let (mut parts, mut depth, mut start) = (Vec::new(), 0usize, 0);

    for (at, character) in list.char_indices() {
        match character {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                parts.push(list[start..at].trim());
                start = at + 1;
            }
            _ => {}
        }
    }

    parts.push(list[start..].trim());
    parts
}

#[document(summary = "A value of a Solidity type.", category = "ABI", see_also = "AbiType, encode_values, decode_values")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AbiValue {
    Uint(u256),
    Int(i256),
    Address([u8; 20]),
    Bool(bool),
    FixedBytes(Vec<u8>),
    Bytes(Vec<u8>),
    String(String),
    #[document(summary = "An array, fixed or not.")]
    Array(Vec<AbiValue>),
    Tuple(Vec<AbiValue>),
}

impl fmt::Display for AbiValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let list = |f: &mut fmt::Formatter<'_>, values: &[AbiValue], open: &str, close: &str| {
            write!(f, "{open}{}{close}", values.iter().map(ToString::to_string).collect::<Vec<_>>().join(", "))
        };

        match self {
            AbiValue::Uint(value) => write!(f, "{value}"),
            AbiValue::Int(value) => write!(f, "{value}"),
            AbiValue::Address(bytes) => f.write_str(&to_hex(bytes)),
            AbiValue::Bool(value) => write!(f, "{value}"),
            AbiValue::FixedBytes(bytes) | AbiValue::Bytes(bytes) => f.write_str(&to_hex(bytes)),
            AbiValue::String(text) => write!(f, "{text:?}"),
            AbiValue::Array(values) => list(f, values, "[", "]"),
            AbiValue::Tuple(values) => list(f, values, "(", ")"),
        }
    }
}

#[document(
    summary = "Encodes values as a function's arguments, or a constructor's: the head of each, and after them the tails of the dynamic ones, as Solidity reads them.",
    category = "ABI",
    returns = "The bytes, or why a value is not of its type.",
)]
pub fn encode_values(types: &[AbiType], values: &[AbiValue]) -> Result<Vec<u8>, String> {
    if types.len() != values.len() {
        return Err(format!("{} values for {} types", values.len(), types.len()));
    }

    let heads: usize = types.iter().map(AbiType::head_size).sum();
    let (mut head, mut tail) = (Vec::with_capacity(heads), Vec::new());

    for (ty, value) in types.iter().zip(values) {
        let encoded = encode_one(ty, value)?;

        match ty.is_dynamic() {
            true => {
                head.extend_from_slice(&u256::from((heads + tail.len()) as u64).to_be_bytes());
                tail.extend(encoded);
            }
            false => head.extend(encoded),
        }
    }

    head.extend(tail);

    Ok(head)
}

fn encode_one(ty: &AbiType, value: &AbiValue) -> Result<Vec<u8>, String> {
    let word = |value: u256| value.to_be_bytes().to_vec();
    let padded = |bytes: &[u8]| {
        let mut out = bytes.to_vec();
        out.resize(bytes.len().div_ceil(32) * 32, 0);
        out
    };

    match (ty, value) {
        (AbiType::Uint(bits), AbiValue::Uint(value)) => match *bits < 256 && *value >> u32::from(*bits) != u256::ZERO {
            true => Err(format!("{value} is wider than {bits} bits")),
            false => Ok(word(*value)),
        },
        (AbiType::Int(bits), AbiValue::Int(value)) => {
            let half = i256::ONE << (u32::from(*bits) - 1);

            match *bits < 256 && (*value >= half || *value < -half) {
                true => Err(format!("{value} does not fit {bits} bits")),
                false => Ok(value.to_be_bytes().to_vec()),
            }
        }
        (AbiType::Address, AbiValue::Address(bytes)) => Ok([vec![0; 12], bytes.to_vec()].concat()),
        (AbiType::Bool, AbiValue::Bool(value)) => Ok(word(u256::from(u8::from(*value)))),
        (AbiType::FixedBytes(size), AbiValue::FixedBytes(bytes)) if bytes.len() == usize::from(*size) => Ok(padded(bytes)),
        (AbiType::Bytes, AbiValue::Bytes(bytes)) => Ok([word(u256::from(bytes.len() as u64)), padded(bytes)].concat()),
        (AbiType::String, AbiValue::String(text)) => Ok([word(u256::from(text.len() as u64)), padded(text.as_bytes())].concat()),
        (AbiType::Array(inner), AbiValue::Array(values)) => {
            let types = vec![(**inner).clone(); values.len()];

            Ok([word(u256::from(values.len() as u64)), encode_values(&types, values)?].concat())
        }
        (AbiType::FixedArray(inner, len), AbiValue::Array(values)) if values.len() == *len => encode_values(&vec![(**inner).clone(); *len], values),
        (AbiType::Tuple(types), AbiValue::Tuple(values)) => encode_values(types, values),
        (ty, value) => Err(format!("{value} is not a {ty}")),
    }
}

#[document(
    summary = "Reads a function's signature, such as transfer(address,uint256), for its selector and its parameters' types; or a list of types alone in parentheses, such as (uint256,bool), which has no selector. Types are written as Solidity writes them; uint and int alone are 256 bits.",
    category = "ABI",
    returns = "The selector, None for a list alone, and the types; or why the text is neither.",
)]
pub fn parse_signature(text: &str) -> Result<(Option<[u8; 4]>, Vec<AbiType>), String> {
    let text = text.trim();
    let open = text.find('(').ok_or_else(|| format!("{text}: write name(types), such as transfer(address,uint256), or (types) alone"))?;
    let (name, list) = text.split_at(open);
    let types = match parse_type(list, 0)? {
        AbiType::Tuple(types) => types,
        other => return Err(format!("{list} is a {other}, not a list of types in parentheses")),
    };

    if name.is_empty() {
        return Ok((None, types));
    }

    let named = name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$') && !name.starts_with(|c: char| c.is_ascii_digit());

    if !named {
        return Err(format!("{name} is not a function name"));
    }

    let canonical = format!("{name}{}", AbiType::Tuple(types.clone()));

    Ok((Some(keccak(canonical.as_bytes())[..4].try_into().expect("4 bytes")), types))
}

#[document(
    summary = "The topic of an event's signature, such as Transfer(address,address,uint256): its keccak256, written as a selector is, so uint is uint256 and spaces go. What a log's first topic is, and what the event index finds it by.",
    category = "ABI",
    returns = "The topic, or why the text is no signature.",
)]
pub fn signature_topic(text: &str) -> Result<[u8; 32], String> {
    let (selector, types) = parse_signature(text)?;

    selector.ok_or_else(|| format!("{text}: a signature names its event, such as Transfer(address,address,uint256)"))?;

    let name = text.trim().split('(').next().unwrap_or_default();

    Ok(keccak(format!("{name}{}", AbiType::Tuple(types)).as_bytes()))
}

#[document(
    summary = "Decodes values of these types from a call's result, a log's data, or a function's arguments, checking every offset and length against the bytes there are.",
    category = "ABI",
    returns = "The values, or why the bytes are not of the types.",
)]
pub fn decode_values(types: &[AbiType], data: &[u8]) -> Result<Vec<AbiValue>, String> {
    decode_tuple(types, data, 0, &Cell::new(usize::MAX))
}

/// As decode_values, with no more than this budget to spend on what comes out: a word a value and
/// a byte a byte. Dynamic values may share their bytes, so without one, a few kilobytes of offsets
/// all pointing at one long value decode to gigabytes. Honest bytes take about what they hold.
/// Only what shows calldata to a person spends one: contracts decode without, as they always have.
fn decode_within(types: &[AbiType], data: &[u8], budget: &Cell<usize>) -> Result<Vec<AbiValue>, String> {
    decode_tuple(types, data, 0, budget)
}

/// Takes what a value costs from the budget, or says it has run out.
fn spend(budget: &Cell<usize>, cost: usize) -> Result<(), String> {
    budget.get().checked_sub(cost).map(|left| budget.set(left)).ok_or_else(|| "more values than its bytes honestly hold".to_string())
}

fn decode_tuple(types: &[AbiType], data: &[u8], depth: usize, budget: &Cell<usize>) -> Result<Vec<AbiValue>, String> {
    if depth > MAX_DEPTH {
        return Err("values nest too deep".to_string());
    }

    let mut at = 0;
    let mut values = Vec::with_capacity(types.len());

    for ty in types {
        let value = match ty.is_dynamic() {
            true => {
                let offset = read_len(data, at)?;
                decode_one(ty, data.get(offset..).ok_or("an offset past the end")?, depth, budget)?
            }
            false => decode_one(ty, data.get(at..).ok_or("ends early")?, depth, budget)?,
        };
        values.push(value);
        at += ty.head_size();
    }

    Ok(values)
}

fn word_at(data: &[u8], at: usize) -> Result<[u8; 32], String> {
    data.get(at..at.checked_add(32).ok_or("past the end")?)
        .map(|word| word.try_into().expect("32 bytes"))
        .ok_or_else(|| "ends early".to_string())
}

fn read_len(data: &[u8], at: usize) -> Result<usize, String> {
    let value = u256::from_be_bytes(word_at(data, at)?);

    usize::try_from(value).ok().filter(|len| *len <= data.len()).ok_or_else(|| "a length or offset past the end".to_string())
}

fn decode_one(ty: &AbiType, data: &[u8], depth: usize, budget: &Cell<usize>) -> Result<AbiValue, String> {
    let word = word_at(data, 0);
    let tail = |len: usize| -> Result<&[u8], String> {
        spend(budget, len)?;
        data.get(32..32 + len).ok_or_else(|| "ends early".to_string())
    };

    spend(budget, 32)?;

    match ty {
        AbiType::Uint(bits) => {
            let value = u256::from_be_bytes(word?);

            match *bits < 256 && value >> u32::from(*bits) != u256::ZERO {
                true => Err(format!("a uint{bits} wider than it")),
                false => Ok(AbiValue::Uint(value)),
            }
        }
        AbiType::Int(_) => Ok(AbiValue::Int(i256::from_be_bytes(word?))),
        AbiType::Address => {
            let word = word?;

            match word[..12].iter().all(|byte| *byte == 0) {
                true => Ok(AbiValue::Address(word[12..].try_into().expect("20 bytes"))),
                false => Err("an address with bytes before its 20".to_string()),
            }
        }
        AbiType::Bool => match u256::from_be_bytes(word?) {
            value if value <= u256::ONE => Ok(AbiValue::Bool(value == u256::ONE)),
            _ => Err("a bool that is neither 0 nor 1".to_string()),
        },
        AbiType::FixedBytes(size) => Ok(AbiValue::FixedBytes(word?[..usize::from(*size)].to_vec())),
        AbiType::Bytes => Ok(AbiValue::Bytes(tail(read_len(data, 0)?)?.to_vec())),
        AbiType::String => String::from_utf8(tail(read_len(data, 0)?)?.to_vec()).map(AbiValue::String).map_err(|_| "a string that is not UTF-8".to_string()),
        AbiType::Array(inner) => {
            let len = read_len(data, 0)?;
            // Every element takes a word at least: a length past what is there is refused before
            // anything is made of it.
            if len.saturating_mul(32) > data.len() {
                return Err("an array longer than its bytes".to_string());
            }

            spend(budget, len)?;
            decode_tuple(&vec![(**inner).clone(); len], &data[32..], depth + 1, budget).map(AbiValue::Array)
        }
        AbiType::FixedArray(inner, len) => {
            if len.saturating_mul(32) > data.len() {
                return Err("an array longer than its bytes".to_string());
            }

            spend(budget, *len)?;
            decode_tuple(&vec![(**inner).clone(); *len], data, depth + 1, budget).map(AbiValue::Array)
        }
        AbiType::Tuple(types) => decode_tuple(types, data, depth + 1, budget).map(AbiValue::Tuple),
    }
}

#[document(
    summary = "Reads a value of a type from what a person typed: a number, decimal or 0x hex; true or false; an address as 0x and 40 hex digits, or any chainv4 address, which goes as its EVM address; hex for bytes; text for a string; and JSON for arrays and tuples, such as [1, 2] or [\"0x…\", 5].",
    category = "ABI",
)]
pub fn parse_value(ty: &AbiType, text: &str) -> Result<AbiValue, String> {
    match ty {
        AbiType::Array(_) | AbiType::FixedArray(..) | AbiType::Tuple(_) => {
            let json: Value = serde_json::from_str(text).map_err(|_| format!("{ty} is written as JSON, such as [1, 2]"))?;

            from_json(ty, &json, 0)
        }
        ty => scalar(ty, text.trim()),
    }
}

fn scalar(ty: &AbiType, text: &str) -> Result<AbiValue, String> {
    let bad = |why: &str| format!("{text:?} as {ty}: {why}");

    match ty {
        AbiType::Uint(_) => {
            let value = match text.strip_prefix("0x") {
                Some(hex) => u256::from_str_radix(hex, 16),
                None => u256::from_str_radix(text, 10),
            }
            .map_err(|_| bad("not a whole number"))?;
            let value = AbiValue::Uint(value);
            encode_one(ty, &value).map(|_| value)
        }
        AbiType::Int(_) => {
            let value = AbiValue::Int(text.parse::<i256>().map_err(|_| bad("not a whole number"))?);
            encode_one(ty, &value).map(|_| value)
        }
        AbiType::Address => {
            if let Some(hex) = text.strip_prefix("0x")
                && hex.len() == 40
            {
                return from_hex(text).map(|bytes| AbiValue::Address(bytes.try_into().expect("20 bytes"))).ok_or_else(|| bad("not hex"));
            }

            text.parse::<Address>().map(|address| AbiValue::Address(evm_address(&address))).map_err(|_| bad("not an address"))
        }
        AbiType::Bool => match text {
            "true" => Ok(AbiValue::Bool(true)),
            "false" => Ok(AbiValue::Bool(false)),
            _ => Err(bad("true or false")),
        },
        AbiType::FixedBytes(size) => match from_hex(text) {
            Some(bytes) if bytes.len() == usize::from(*size) => Ok(AbiValue::FixedBytes(bytes)),
            _ => Err(bad(&format!("{size} bytes of hex"))),
        },
        AbiType::Bytes => from_hex(text).map(AbiValue::Bytes).ok_or_else(|| bad("hex")),
        AbiType::String => Ok(AbiValue::String(text.to_string())),
        _ => unreachable!("lists go through from_json"),
    }
}

fn from_json(ty: &AbiType, json: &Value, depth: usize) -> Result<AbiValue, String> {
    if depth > MAX_DEPTH {
        return Err("values nest too deep".to_string());
    }

    match (ty, json) {
        (AbiType::Array(inner), Value::Array(items)) => items.iter().map(|item| from_json(inner, item, depth + 1)).collect::<Result<_, _>>().map(AbiValue::Array),
        (AbiType::FixedArray(inner, len), Value::Array(items)) if items.len() == *len => items.iter().map(|item| from_json(inner, item, depth + 1)).collect::<Result<_, _>>().map(AbiValue::Array),
        (AbiType::Tuple(types), Value::Array(items)) if items.len() == types.len() => types.iter().zip(items).map(|(ty, item)| from_json(ty, item, depth + 1)).collect::<Result<_, _>>().map(AbiValue::Tuple),
        (AbiType::Array(_) | AbiType::FixedArray(..) | AbiType::Tuple(_), _) => Err(format!("{json} is not a {ty}")),
        (ty, Value::String(text)) => scalar(ty, text),
        (ty, Value::Number(number)) => scalar(ty, &number.to_string()),
        (ty, Value::Bool(value)) => scalar(ty, &value.to_string()),
        (ty, other) => Err(format!("{other} is not a {ty}")),
    }
}

#[document(summary = "A named parameter of a function, event, error or constructor.", category = "ABI", see_also = "Abi")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AbiParam {
    #[document(summary = "Its name, which may be empty.")]
    pub name: String,
    #[document(summary = "Its type.")]
    pub ty: AbiType,
    #[document(summary = "For an event's parameter, whether it is a topic rather than in the data.")]
    pub indexed: bool,
}

#[document(summary = "A function of a contract, as its ABI describes it.", category = "ABI", see_also = "Abi")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AbiFunction {
    pub name: String,
    pub inputs: Vec<AbiParam>,
    pub outputs: Vec<AbiParam>,
    #[document(summary = "Whether it only reads: view or pure.")]
    pub reads: bool,
    #[document(summary = "Whether it takes the coin sent with it.")]
    pub payable: bool,
}

#[document(summary = "An event or a custom error, as an ABI describes it: what a log or a revert carries.", category = "ABI", see_also = "Abi")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AbiEvent {
    pub name: String,
    pub inputs: Vec<AbiParam>,
    #[document(summary = "For an event, whether its signature is left out of its topics.")]
    pub anonymous: bool,
}

#[document(
    summary = "A contract's ABI, read from the JSON solc --abi and Foundry write: its functions, events, custom errors and constructor.",
    category = "ABI",
    see_also = "AbiType",
)]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Abi {
    pub functions: Vec<AbiFunction>,
    pub events: Vec<AbiEvent>,
    pub errors: Vec<AbiEvent>,
    #[document(summary = "The constructor's parameters; none when it has none.")]
    pub constructor: Vec<AbiParam>,
    #[document(summary = "Whether its constructor takes the coin sent with a deploy.")]
    pub payable_constructor: bool,
}

/// A parameter's type from its JSON entry: tuples from their components.
fn param_type(entry: &Value, depth: usize) -> Result<AbiType, String> {
    let ty = entry.get("type").and_then(Value::as_str).ok_or("a parameter with no type")?;

    match ty.strip_prefix("tuple") {
        Some(suffix) => {
            let components = entry.get("components").and_then(Value::as_array).ok_or("a tuple with no components")?;
            let inner = components.iter().map(|component| param_type(component, depth + 1).map(|ty| ty.to_string())).collect::<Result<Vec<_>, _>>()?;

            parse_type(&format!("({}){suffix}", inner.join(",")), depth)
        }
        None => parse_type(ty, depth),
    }
}

fn params(entry: &Value, key: &str) -> Result<Vec<AbiParam>, String> {
    entry
        .get(key)
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .map(|param| {
                    Ok(AbiParam {
                        name: param.get("name").and_then(Value::as_str).unwrap_or_default().to_string(),
                        ty: param_type(param, 0)?,
                        indexed: param.get("indexed").and_then(Value::as_bool).unwrap_or(false),
                    })
                })
                .collect()
        })
        .unwrap_or(Ok(Vec::new()))
}

impl Abi {
    #[document(parent = "Abi", summary = "Reads an ABI's JSON: a list of entries, or an object holding one under abi, as Foundry's artifacts do.")]
    pub fn parse(json: &str) -> Result<Abi, String> {
        let value: Value = serde_json::from_str(json).map_err(|err| format!("the ABI is not JSON: {err}"))?;
        let entries = match &value {
            Value::Array(entries) => entries,
            Value::Object(object) => object.get("abi").and_then(Value::as_array).ok_or("an object with no abi list")?,
            _ => return Err("an ABI is a list of entries".to_string()),
        };
        let mut abi = Abi::default();

        for entry in entries {
            let name = entry.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
            let mutability = entry.get("stateMutability").and_then(Value::as_str).unwrap_or("nonpayable");

            match entry.get("type").and_then(Value::as_str).unwrap_or("function") {
                "function" => abi.functions.push(AbiFunction {
                    name,
                    inputs: params(entry, "inputs")?,
                    outputs: params(entry, "outputs")?,
                    reads: matches!(mutability, "view" | "pure") || entry.get("constant").and_then(Value::as_bool) == Some(true),
                    payable: mutability == "payable" || entry.get("payable").and_then(Value::as_bool) == Some(true),
                }),
                "event" => abi.events.push(AbiEvent {
                    name,
                    inputs: params(entry, "inputs")?,
                    anonymous: entry.get("anonymous").and_then(Value::as_bool).unwrap_or(false),
                }),
                "error" => abi.errors.push(AbiEvent { name, inputs: params(entry, "inputs")?, anonymous: false }),
                "constructor" => {
                    abi.constructor = params(entry, "inputs")?;
                    abi.payable_constructor = mutability == "payable";
                }
                _ => {}
            }
        }

        Ok(abi)
    }

    #[document(parent = "Abi", summary = "The function calldata calls: by its first four bytes.")]
    pub fn function_for(&self, input: &[u8]) -> Option<&AbiFunction> {
        let selector = input.get(..4)?;

        self.functions.iter().find(|function| function.selector() == selector)
    }

    #[document(parent = "Abi", summary = "The event a log's first topic names.")]
    pub fn event_for(&self, topics: &[[u8; 32]]) -> Option<&AbiEvent> {
        let first = topics.first()?;

        self.events.iter().find(|event| !event.anonymous && event.topic() == *first)
    }

    #[document(
        parent = "Abi",
        summary = "What a revert returned, read: Solidity's Error(string) and Panic(uint256), and the ABI's custom errors with their values. None for anything else.",
    )]
    pub fn revert_reason(&self, data: &[u8]) -> Option<String> {
        let (selector, args) = (data.get(..4)?, &data[4..]);

        if selector == [0x08, 0xc3, 0x79, 0xa0] {
            return match decode_values(&[AbiType::String], args).ok()?.first()? {
                AbiValue::String(reason) => Some(reason.clone()),
                _ => None,
            };
        }

        if selector == [0x4e, 0x48, 0x7b, 0x71] {
            return decode_values(&[AbiType::Uint(256)], args).ok().map(|code| format!("panic {}", code[0]));
        }

        let error = self.errors.iter().find(|error| error.signature_hash()[..4] == *selector)?;
        let values = decode_values(&error.inputs.iter().map(|param| param.ty.clone()).collect::<Vec<_>>(), args).ok()?;

        Some(format!("{}({})", error.name, named(&error.inputs, &values)))
    }
}

/// Values beside their parameters' names: name = value, …
fn named(params: &[AbiParam], values: &[AbiValue]) -> String {
    params
        .iter()
        .zip(values)
        .map(|(param, value)| match param.name.is_empty() {
            true => value.to_string(),
            false => format!("{} = {value}", param.name),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn signature(name: &str, params: &[AbiParam]) -> String {
    format!("{name}({})", params.iter().map(|param| param.ty.to_string()).collect::<Vec<_>>().join(","))
}

impl AbiFunction {
    #[document(parent = "AbiFunction", summary = "Its signature, as its selector hashes it: bump(uint256).")]
    pub fn signature(&self) -> String {
        signature(&self.name, &self.inputs)
    }

    #[document(parent = "AbiFunction", summary = "Its selector: the first four bytes of its signature's keccak256.")]
    pub fn selector(&self) -> [u8; 4] {
        keccak(self.signature().as_bytes())[..4].try_into().expect("4 bytes")
    }

    #[document(parent = "AbiFunction", summary = "Calldata for it: its selector and its arguments, encoded.")]
    pub fn encode_call(&self, values: &[AbiValue]) -> Result<Vec<u8>, String> {
        let types: Vec<AbiType> = self.inputs.iter().map(|param| param.ty.clone()).collect();

        Ok([self.selector().to_vec(), encode_values(&types, values)?].concat())
    }

    #[document(parent = "AbiFunction", summary = "Its arguments, read back from calldata for it.")]
    pub fn decode_input(&self, input: &[u8]) -> Result<Vec<AbiValue>, String> {
        let types: Vec<AbiType> = self.inputs.iter().map(|param| param.ty.clone()).collect();

        decode_values(&types, input.get(4..).ok_or("calldata shorter than a selector")?)
    }

    #[document(parent = "AbiFunction", summary = "What it returned, read.")]
    pub fn decode_output(&self, output: &[u8]) -> Result<Vec<AbiValue>, String> {
        decode_values(&self.outputs.iter().map(|param| param.ty.clone()).collect::<Vec<_>>(), output)
    }

    #[document(parent = "AbiFunction", summary = "Values beside its inputs' names, for a person to read.")]
    pub fn show_inputs(&self, values: &[AbiValue]) -> String {
        named(&self.inputs, values)
    }

    #[document(parent = "AbiFunction", summary = "Values beside its outputs' names, for a person to read.")]
    pub fn show_outputs(&self, values: &[AbiValue]) -> String {
        named(&self.outputs, values)
    }
}

impl AbiEvent {
    #[document(parent = "AbiEvent", summary = "Its signature: Bumped(address,uint256).")]
    pub fn signature(&self) -> String {
        signature(&self.name, &self.inputs)
    }

    fn signature_hash(&self) -> [u8; 32] {
        keccak(self.signature().as_bytes())
    }

    #[document(parent = "AbiEvent", summary = "Its first topic: its signature's keccak256.")]
    pub fn topic(&self) -> [u8; 32] {
        self.signature_hash()
    }

    #[document(
        parent = "AbiEvent",
        summary = "A log of it, read: its indexed values from the topics after its signature, its others from the data. An indexed string, bytes, array or tuple is in its topic as its keccak256 alone, which it reads as bytes32.",
        returns = "Its values in its parameters' order, or why the log is not one of it.",
    )]
    pub fn decode_log(&self, topics: &[[u8; 32]], data: &[u8]) -> Result<Vec<AbiValue>, String> {
        self.decode_log_within(topics, data, &Cell::new(usize::MAX))
    }

    /// As decode_log, with a budget for what comes out, as decode_within has.
    fn decode_log_within(&self, topics: &[[u8; 32]], data: &[u8], budget: &Cell<usize>) -> Result<Vec<AbiValue>, String> {
        let mut indexed = topics.iter().skip(usize::from(!self.anonymous));
        let unindexed: Vec<AbiType> = self.inputs.iter().filter(|param| !param.indexed).map(|param| param.ty.clone()).collect();
        let mut data_values = decode_within(&unindexed, data, budget)?.into_iter();

        self.inputs
            .iter()
            .map(|param| match param.indexed {
                true => {
                    let topic = indexed.next().ok_or("fewer topics than indexed values")?;

                    match param.ty.is_dynamic() || matches!(param.ty, AbiType::Tuple(_) | AbiType::FixedArray(..)) {
                        true => Ok(AbiValue::FixedBytes(topic.to_vec())),
                        false => decode_one(&param.ty, topic, 0, budget),
                    }
                }
                false => data_values.next().ok_or_else(|| "fewer values than the event's".to_string()),
            })
            .collect()
    }

    #[document(parent = "AbiEvent", summary = "Values beside its inputs' names, for a person to read.")]
    pub fn show(&self, values: &[AbiValue]) -> String {
        format!("{}({})", self.name, named(&self.inputs, values))
    }
}

pub mod known;
#[cfg(feature = "secp256k1")]
pub mod multisig;
#[cfg(test)]
mod tests;
