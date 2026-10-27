//! Bech32, as BIP173 writes Bitcoin's native segwit addresses, version 0 only: the node's
//! src/structs/bech32.rs, for a bitcoin address's text.

use alloc::{string::String, vec, vec::Vec};

const ALPHABET: &[u8; 32] = b"qpzry9x8gf2tvdw0s3jn54khce6mua7l";

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

fn expand(hrp: &str) -> Vec<u8> {
    let bytes = hrp.as_bytes();

    bytes.iter().map(|byte| byte >> 5).chain([0]).chain(bytes.iter().map(|byte| byte & 31)).collect()
}

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

/// A version 0 witness program under a human-readable part: bc1q… for bc and 20 bytes.
pub fn encode_segwit(hrp: &str, program: &[u8]) -> String {
    let mut data = vec![0];
    data.extend(regroup(program, 8, 5, true).unwrap_or_default());
    let check = polymod(expand(hrp).into_iter().chain(data.iter().copied()).chain([0; 6])) ^ 1;
    let mut out = String::from(hrp);
    out.push('1');

    out.extend(data.iter().map(|value| ALPHABET[usize::from(*value)] as char));
    out.extend((0..6).map(|place| ALPHABET[((check >> (5 * (5 - place))) & 31) as usize] as char));

    out
}

/// Reads a version 0 segwit address: its human-readable part, in lowercase, and its program.
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

    (*version == 0 && matches!(program.len(), 20 | 32)).then(|| (String::from(hrp), program))
}
