//! Any typed data, as a site hands it to eth_signTypedData_v4: its types, its primary type, its
//! domain and its message, as JSON. What MetaMask signs for a site, hashed as its eth-sig-util does
//! version 4: structs by their hash, arrays of anything, and a struct left out as zero.

use {
    super::super::secp256k1::keccak,
    crate::structs::hex::from_hex,
    crate::structs::u256::u256,
    serde_json::{Map, Value},
    std::collections::BTreeSet,
};

/// How deep structs and arrays may nest: past this, the data is refused rather than recursed into.
const MAX_DEPTH: usize = 32;

/// The domain's fields, in the order EIP-712 lists them, for data whose types leave it out.
const DOMAIN_FIELDS: [(&str, &str); 5] = [("name", "string"), ("version", "string"), ("chainId", "uint256"), ("verifyingContract", "address"), ("salt", "bytes32")];

/// Typed data, read: what eth_signTypedData_v4 is given.
#[derive(Clone, Debug)]
pub struct TypedData {
    types: Map<String, Value>,
    pub primary: String,
    pub domain: Value,
    pub message: Value,
}

impl TypedData {
    /// Reads typed data from its JSON, as an object or as the text of one, as sites pass it.
    pub fn parse(json: &Value) -> Result<TypedData, String> {
        let json = match json {
            Value::String(text) => serde_json::from_str(text).map_err(|err| format!("typed data is JSON: {err}"))?,
            other => other.clone(),
        };
        let mut types = json["types"].as_object().cloned().ok_or("typed data has types")?;
        let primary = json["primaryType"].as_str().ok_or("typed data has a primaryType")?.to_string();
        let domain = json.get("domain").cloned().unwrap_or(Value::Object(Map::new()));

        // Without EIP712Domain among its types, the domain's own fields say what it is.
        if !types.contains_key("EIP712Domain") {
            let fields = DOMAIN_FIELDS
                .iter()
                .filter(|(name, _)| domain.get(name).is_some_and(|value| !value.is_null()))
                .map(|(name, ty)| serde_json::json!({ "name": name, "type": ty }))
                .collect();
            types.insert("EIP712Domain".to_string(), Value::Array(fields));
        }

        if !types.contains_key(&primary) {
            return Err(format!("the primaryType {primary} is not among the types"));
        }

        Ok(TypedData {
            types,
            primary,
            domain,
            message: json.get("message").cloned().unwrap_or(Value::Null),
        })
    }

    /// What is signed: the keccak256 of 0x19 0x01, the domain's hash and the message's; with no
    /// message's for data whose primary type is the domain.
    pub fn digest(&self) -> Result<[u8; 32], String> {
        let mut bytes = vec![0x19, 0x01];
        bytes.extend(self.hash_struct("EIP712Domain", &self.domain, 0)?);

        if self.primary != "EIP712Domain" {
            bytes.extend(self.hash_struct(&self.primary, &self.message, 0)?);
        }

        Ok(keccak(&bytes))
    }

    /// A struct type's fields: each its name and type.
    fn fields(&self, name: &str) -> Result<Vec<(String, String)>, String> {
        let fields = self.types.get(name).and_then(Value::as_array).ok_or_else(|| format!("no type {name}"))?;

        fields
            .iter()
            .map(|field| match (field["name"].as_str(), field["type"].as_str()) {
                (Some(name), Some(ty)) => Ok((name.to_string(), ty.to_string())),
                _ => Err(format!("a field of {name} has a name and a type")),
            })
            .collect()
    }

    /// A type as encodeType writes it: itself, then every struct it reaches, sorted by name.
    pub fn encode_type(&self, name: &str) -> Result<String, String> {
        let mut reached = BTreeSet::new();
        self.reach(name, &mut reached, 0)?;
        reached.remove(name);
        let mut out = self.signature(name)?;

        for other in reached {
            out.push_str(&self.signature(&other)?);
        }

        Ok(out)
    }

    fn signature(&self, name: &str) -> Result<String, String> {
        let fields = self.fields(name)?.iter().map(|(field, ty)| format!("{ty} {field}")).collect::<Vec<_>>().join(",");

        Ok(format!("{name}({fields})"))
    }

    fn reach(&self, name: &str, reached: &mut BTreeSet<String>, depth: usize) -> Result<(), String> {
        if depth > MAX_DEPTH {
            return Err("types nest too deep".to_string());
        }

        if !reached.insert(name.to_string()) {
            return Ok(());
        }

        for (_, ty) in self.fields(name)? {
            let base = base_type(&ty);

            if self.types.contains_key(base) {
                self.reach(base, reached, depth + 1)?;
            }
        }

        Ok(())
    }

    fn hash_struct(&self, name: &str, value: &Value, depth: usize) -> Result<[u8; 32], String> {
        if depth > MAX_DEPTH {
            return Err("data nests too deep".to_string());
        }

        let mut bytes = keccak(self.encode_type(name)?.as_bytes()).to_vec();

        for (field, ty) in self.fields(name)? {
            bytes.extend(self.encode_field(&ty, value.get(&field).unwrap_or(&Value::Null), depth + 1).map_err(|err| format!("{name}.{field}: {err}"))?);
        }

        Ok(keccak(&bytes))
    }

    /// One field's 32 bytes: a struct's hash, or zero for one left out; an array's keccak of its
    /// items' encodings; string and bytes hashed; the rest as the ABI encodes them.
    fn encode_field(&self, ty: &str, value: &Value, depth: usize) -> Result<[u8; 32], String> {
        if depth > MAX_DEPTH {
            return Err("data nests too deep".to_string());
        }

        if let Some(inner) = ty.strip_suffix(']').and_then(|rest| rest.rfind('[').map(|open| &rest[..open])) {
            let items = value.as_array().ok_or_else(|| format!("{ty} is an array"))?;
            let length = &ty[inner.len() + 1..ty.len() - 1];

            if !length.is_empty() && length.parse::<usize>().ok() != Some(items.len()) {
                return Err(format!("{ty} holds {length} items, not {}", items.len()));
            }

            let mut bytes = Vec::with_capacity(items.len() * 32);

            for item in items {
                bytes.extend(self.encode_field(inner, item, depth + 1)?);
            }

            return Ok(keccak(&bytes));
        }

        if self.types.contains_key(ty) {
            return match value {
                Value::Null => Ok([0; 32]),
                value => self.hash_struct(ty, value, depth),
            };
        }

        atomic(ty, value)
    }
}

/// A type without its array suffixes: Person for Person[][2].
fn base_type(ty: &str) -> &str {
    ty.find('[').map_or(ty, |open| &ty[..open])
}

/// An atomic or dynamic value's 32 bytes.
fn atomic(ty: &str, value: &Value) -> Result<[u8; 32], String> {
    let bad = || format!("{value} is not a {ty}");

    match ty {
        "string" => Ok(keccak(value.as_str().ok_or_else(bad)?.as_bytes())),
        "bytes" => Ok(keccak(&bytes(value).ok_or_else(bad)?)),
        "bool" => {
            let flag = match value {
                Value::Bool(flag) => *flag,
                Value::String(text) if text == "true" || text == "false" => text == "true",
                _ => return Err(bad()),
            };
            let mut word = [0; 32];
            word[31] = u8::from(flag);

            Ok(word)
        }
        "address" => {
            let address = bytes(value).filter(|address| address.len() == 20).ok_or_else(bad)?;
            let mut word = [0; 32];
            word[12..].copy_from_slice(&address);

            Ok(word)
        }
        _ if ty.starts_with("bytes") => {
            let size: usize = ty[5..].parse().map_err(|_| format!("no type {ty}"))?;

            if !(1..=32).contains(&size) {
                return Err(format!("no type {ty}"));
            }

            let given = bytes(value).filter(|given| given.len() <= size).ok_or_else(bad)?;
            let mut word = [0; 32];
            word[..given.len()].copy_from_slice(&given);

            Ok(word)
        }
        _ if ty.starts_with("uint") || ty.starts_with("int") => {
            let signed = ty.starts_with("int");
            let bits: u32 = match &ty[if signed { 3 } else { 4 }..] {
                "" => 256,
                bits => bits.parse().map_err(|_| format!("no type {ty}"))?,
            };

            if bits == 0 || bits > 256 || !bits.is_multiple_of(8) {
                return Err(format!("no type {ty}"));
            }

            let (negative, magnitude) = integer(value).ok_or_else(bad)?;
            let too_big = || format!("{value} does not fit a {ty}");

            match signed {
                false if negative && magnitude != u256::ZERO => Err(too_big()),
                false if bits < 256 && magnitude >= (u256::ONE << bits) => Err(too_big()),
                false => Ok(magnitude.to_be_bytes()),
                true => {
                    // The most either way: 2^(bits-1) below zero, one less above.
                    let limit = u256::ONE << (bits - 1);

                    if magnitude > limit || (!negative && magnitude == limit) {
                        return Err(too_big());
                    }

                    let number = magnitude.as_i256();

                    Ok(if negative { number.wrapping_neg() } else { number }.to_be_bytes())
                }
            }
        }
        _ => Err(format!("no type {ty}")),
    }
}

/// Hex bytes, as 0x and pairs of digits.
fn bytes(value: &Value) -> Option<Vec<u8>> {
    let text = value.as_str()?;

    match text {
        "0x" => Some(Vec::new()),
        text if text.starts_with("0x") && text.len() % 2 == 0 => from_hex(text),
        // An odd count of digits, as some sites write a number's bytes.
        text if text.starts_with("0x") => from_hex(&format!("0x0{}", &text[2..])),
        _ => None,
    }
}

/// A number written as JSON's own, as decimal text, or as 0x hex, negative ones with a -: whether
/// it is below zero, and how far from zero.
fn integer(value: &Value) -> Option<(bool, u256)> {
    match value {
        Value::Number(number) => match (number.as_u64(), number.as_i64()) {
            (Some(number), _) => Some((false, u256::from(number))),
            (None, Some(number)) => Some((number < 0, u256::from(number.unsigned_abs()))),
            _ => None,
        },
        Value::String(text) => {
            let text = text.trim();
            let (negative, digits) = match text.strip_prefix('-') {
                Some(rest) => (true, rest),
                None => (false, text),
            };
            let magnitude = match digits.strip_prefix("0x") {
                Some(hex) => u256::from_str_radix(hex, 16).ok()?,
                None => u256::from_str_radix(digits, 10).ok()?,
            };

            Some((negative, magnitude))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests;
