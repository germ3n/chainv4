//! Bech32, as BIP173 writes Bitcoin's native segwit addresses: a human-readable part, such as bc,
//! a 1, then the witness version and program in a 32-letter alphabet with a six-letter checksum
//! that catches any four mistyped letters. Only version 0 is read and written, whose checksum is
//! plain bech32 (BIP350's bech32m is for version 1 and up, Taproot's).

const ALPHABET: &[u8; 32] = b"qpzry9x8gf2tvdw0s3jn54khce6mua7l";

/// The checksum's polynomial over the 5-bit values.
fn polymod(values: impl IntoIterator<Item = u8>) -> u32 {
    const GENERATOR: [u32; 5] = [0x3b6a_57b2, 0x2650_8e6d, 0x1ea1_19fa, 0x3d42_33dd, 0x2a14_62b3];
    let mut check = 1u32;

    for value in values {
        let top = check >> 25;
        check = ((check & 0x1ff_ffff) << 5) ^ u32::from(value);

        for (bit, generator) in GENERATOR.iter().enumerate() {
            if (top >> bit) & 1 == 1 {
                check ^= generator;
            }
        }
    }

    check
}

/// The human-readable part as the checksum covers it: each letter's high bits, a zero, its low bits.
fn expand(hrp: &str) -> Vec<u8> {
    let bytes = hrp.as_bytes();

    bytes.iter().map(|byte| byte >> 5).chain([0]).chain(bytes.iter().map(|byte| byte & 31)).collect()
}

/// Regroups bits: 8-bit bytes into 5-bit values, padded, or back, where leftover bits must be zero
/// padding of fewer than five.
fn regroup(data: &[u8], from: u32, to: u32, pad: bool) -> Option<Vec<u8>> {
    let mut acc = 0u32;
    let mut bits = 0;
    let mut out = Vec::new();
    let max = (1 << to) - 1;

    for value in data {
        acc = (acc << from) | u32::from(*value);
        bits += from;

        while bits >= to {
            bits -= to;
            out.push(((acc >> bits) & max) as u8);
        }
    }

    match pad {
        true if bits > 0 => out.push(((acc << (to - bits)) & max) as u8),
        true => {}
        false if bits >= from || (acc << (to - bits)) & max != 0 => return None,
        false => {}
    }

    Some(out)
}

/// A version 0 witness program under a human-readable part, such as bc and a key's 20-byte
/// HASH160: bc1q…, in lowercase.
pub fn encode_segwit(hrp: &str, program: &[u8]) -> String {
    let mut data = vec![0];
    data.extend(regroup(program, 8, 5, true).expect("padding always fits"));
    let check = polymod(expand(hrp).into_iter().chain(data.iter().copied()).chain([0; 6])) ^ 1;
    let mut out = format!("{hrp}1");

    out.extend(data.iter().map(|value| ALPHABET[usize::from(*value)] as char));
    out.extend((0..6).map(|place| ALPHABET[((check >> (5 * (5 - place))) & 31) as usize] as char));

    out
}

/// Reads a version 0 segwit address: its human-readable part, in lowercase, and its program of 20
/// or 32 bytes. None for anything else: mixed case, a bad checksum, another version, or a program
/// of another length.
pub fn decode_segwit(text: &str) -> Option<(String, Vec<u8>)> {
    if text.len() > 90 || (text.bytes().any(|byte| byte.is_ascii_lowercase()) && text.bytes().any(|byte| byte.is_ascii_uppercase())) {
        return None;
    }

    let text = text.to_ascii_lowercase();
    let (hrp, data) = text.rsplit_once('1')?;

    if hrp.is_empty() || data.len() < 6 || !hrp.bytes().all(|byte| (33..=126).contains(&byte)) {
        return None;
    }

    let values = data.bytes().map(|byte| ALPHABET.iter().position(|letter| *letter == byte).map(|at| at as u8)).collect::<Option<Vec<u8>>>()?;

    if polymod(expand(hrp).into_iter().chain(values.iter().copied())) != 1 {
        return None;
    }

    let (version, program) = values[..values.len() - 6].split_first()?;
    let program = regroup(program, 5, 8, false)?;

    (*version == 0 && matches!(program.len(), 20 | 32)).then(|| (hrp.to_string(), program))
}

#[cfg(test)]
mod tests {
    use {super::*, crate::structs::hex::from_hex};

    #[test]
    fn bip173s_examples_read_and_write() {
        // BIP173's P2WPKH and P2WSH examples, on mainnet and testnet.
        let cases = [
            ("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4", "0x751e76e8199196d454941c45d1b3a323f1433bd6"),
            ("tb1qrp33g0q5c5txsp9arysrx4k6zdkfs4nce4xj0gdcccefvpysxf3q0sl5k7", "0x1863143c14c5166804bd19203356da136c985678cd4d27a1b8c6329604903262"),
        ];

        for (text, program) in cases {
            let (hrp, read) = decode_segwit(text).unwrap();

            assert_eq!(read, from_hex(program).unwrap(), "{text}");
            assert_eq!(encode_segwit(&hrp, &read), text);
            assert_eq!(decode_segwit(&text.to_ascii_uppercase()).unwrap().1, read, "uppercase is the same address");
        }
    }

    #[test]
    fn bip173s_invalid_addresses_are_refused() {
        for text in [
            "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t5",     // a bad checksum
            "BC1QW508D6QEJXTDG4Y5R3ZARVARY0C5XW7KV8F3t4",     // mixed case
            "bc1zw508d6qejxtdg4y5r3zarvaryvqyzf3du",           // version 2
            "bc1rw5uspcuh",                                     // a program too short
            "bc1qr508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4",     // a letter changed
            "tb1qrp33g0q5c5txsp9arysrx4k6zdkfs4nce4xj0gdcccefvpysxf3pjxtptv", // non-zero padding
            "bc1gmk9yu",                                        // nothing after the version
            "bc1p0xlxvlhemja6c4dqv22uapctqupfhlxm9h8z3k2e72q4k9hcz7vqzk5jj0", // Taproot's v1, bech32m
        ] {
            assert_eq!(decode_segwit(text), None, "{text}");
        }
    }
}
