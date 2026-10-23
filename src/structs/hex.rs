//! Hex the way the chain prints it: lowercase, with a 0x prefix.

/// Lowercase hex with a 0x prefix.
pub fn to_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(2 + bytes.len() * 2);
    out.push_str("0x");

    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }

    out
}

/// Hex with or without a 0x prefix, in either case.
pub fn from_hex(text: &str) -> Option<Vec<u8>> {
    let text = text.strip_prefix("0x").unwrap_or(text);

    if !text.len().is_multiple_of(2) {
        return None;
    }

    (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(text.get(index..index + 2)?, 16).ok())
        .collect()
}
