//! NFTs for the apps: where a collection's token_uri points, read the way wallets and marketplaces
//! read ERC-721 metadata. A URI is https, ipfs:// through a gateway the user picks, or data:,
//! carried in the URI itself; the metadata it names is JSON with a name, a description, an image
//! and attributes, and the image is another such URI.

use {
    r#macro::document,
    serde_json::Value,
};

#[document(summary = "The IPFS gateway the apps offer until the user picks another: ipfs://CID/path is fetched as this, then CID/path.", category = "NFTs")]
pub const DEFAULT_GATEWAY: &str = "https://ipfs.io/ipfs/";

#[document(summary = "The most bytes the apps read of metadata or an image: what a phone holds comfortably, and well past a reasonable picture.", category = "NFTs")]
pub const MOST_BYTES: usize = 8 * 1024 * 1024;

#[document(
    summary = "Where a token_uri, or an image URI in its metadata, leads.",
    category = "NFTs",
    see_also = "locate",
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Located {
    #[document(summary = "An https or http URL to fetch: the URI itself, or an ipfs:// one through the gateway.")]
    Fetch(String),
    #[document(summary = "A data: URI's own bytes, and their media type, such as image/svg+xml or application/json.")]
    Inline { media: String, bytes: Vec<u8> },
}

#[document(
    summary = "Where a URI leads, for an app to fetch or read in place.",
    params = {
        uri = "The URI: https://, http://, ipfs://CID/path, ipfs://ipfs/CID/path, or data:[type][;base64],…",
        gateway = "The IPFS gateway, ending in a slash, such as DEFAULT_GATEWAY.",
    },
    returns = "Where it leads, or None for a scheme the apps do not follow, such as ar:// or javascript:, or a data: URI that does not decode.",
    category = "NFTs",
)]
pub fn locate(uri: &str, gateway: &str) -> Option<Located> {
    let uri = uri.trim();
    let lower = uri.to_ascii_lowercase();

    if lower.starts_with("https://") || lower.starts_with("http://") {
        return Some(Located::Fetch(uri.to_string()));
    }

    if lower.starts_with("ipfs://") {
        let path = uri["ipfs://".len()..].trim_start_matches("ipfs/");

        return (!path.is_empty()).then(|| Located::Fetch(format!("{gateway}{path}")));
    }

    if lower.starts_with("data:") {
        let (head, data) = uri["data:".len()..].split_once(',')?;
        let (media, base64) = match head.strip_suffix(";base64") {
            Some(media) => (media, true),
            None => (head, false),
        };
        let bytes = match base64 {
            true => decode_base64(data)?,
            false => decode_percent(data)?,
        };
        let media = match media.split(';').next().unwrap_or("").trim() {
            "" => "text/plain".to_string(),
            media => media.to_ascii_lowercase(),
        };

        return Some(Located::Inline { media, bytes });
    }

    None
}

#[document(
    summary = "A token's metadata, as ERC-721's metadata JSON lays it out and marketplaces read it.",
    category = "NFTs",
)]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Metadata {
    #[document(summary = "The token's name.")]
    pub name: Option<String>,
    #[document(summary = "What it is.")]
    pub description: Option<String>,
    #[document(summary = "Its picture's URI: https, ipfs:// or data:. image_url and image_data, an inline SVG, stand in for it.")]
    pub image: Option<String>,
    #[document(summary = "Its traits, as trait_type and value, in the order given.")]
    pub attributes: Vec<(String, String)>,
}

impl Metadata {
    #[document(
        parent = "Metadata",
        summary = "Reads metadata JSON, taking what it understands and leaving the rest: text that is not a string is written as JSON, and an SVG in image_data becomes a data: URI.",
        returns = "The metadata, or None for bytes that are not a JSON object.",
    )]
    pub fn read(bytes: &[u8]) -> Option<Metadata> {
        let value: Value = serde_json::from_slice(bytes).ok()?;
        let object = value.as_object()?;
        let text = |key: &str| object.get(key).and_then(Value::as_str).map(str::to_string).filter(|text| !text.trim().is_empty());
        let shown = |value: &Value| match value {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        };
        let image = text("image").or_else(|| text("image_url")).or_else(|| {
            text("image_data").map(|svg| format!("data:image/svg+xml;base64,{}", encode_base64(svg.as_bytes())))
        });
        let attributes = object
            .get("attributes")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|attribute| {
                let value = attribute.get("value")?;
                let trait_type = attribute.get("trait_type").map_or_else(String::new, shown);

                Some((trait_type, shown(value)))
            })
            .collect();

        Some(Metadata { name: text("name"), description: text("description"), image, attributes })
    }
}

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Base64, standard or URL-safe, padded or not, ignoring whitespace.
fn decode_base64(text: &str) -> Option<Vec<u8>> {
    let mut bits = 0u32;
    let mut held = 0;
    let mut out = Vec::with_capacity(text.len() * 3 / 4);

    for byte in text.bytes().filter(|byte| !byte.is_ascii_whitespace()) {
        let value = match byte {
            b'=' => break,
            b'-' => 62,
            b'_' => 63,
            byte => ALPHABET.iter().position(|letter| *letter == byte)? as u32,
        };
        bits = bits << 6 | value;
        held += 6;

        if held >= 8 {
            held -= 8;
            out.push((bits >> held) as u8);
            bits &= (1 << held) - 1;
        }
    }

    Some(out)
}

fn encode_base64(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);

    for chunk in bytes.chunks(3) {
        let joined = chunk.iter().enumerate().fold(0u32, |joined, (index, byte)| joined | u32::from(*byte) << (16 - 8 * index));

        for index in 0..4 {
            match index <= chunk.len() {
                true => out.push(ALPHABET[(joined >> (18 - 6 * index) & 63) as usize] as char),
                false => out.push('='),
            }
        }
    }

    out
}

/// %XX escapes, as a data: URI without base64 carries its bytes.
fn decode_percent(text: &str) -> Option<Vec<u8>> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        match bytes[index] {
            b'%' => {
                let hex = std::str::from_utf8(bytes.get(index + 1..index + 3)?).ok()?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                index += 3;
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }

    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uris_lead_where_wallets_take_them() {
        let gateway = "https://gw.example/ipfs/";

        assert_eq!(locate("https://example.com/1.json", gateway), Some(Located::Fetch("https://example.com/1.json".to_string())));
        assert_eq!(locate("ipfs://bafy/1.json", gateway), Some(Located::Fetch("https://gw.example/ipfs/bafy/1.json".to_string())));
        assert_eq!(locate("ipfs://ipfs/bafy", gateway), Some(Located::Fetch("https://gw.example/ipfs/bafy".to_string())));
        assert_eq!(
            locate("data:application/json;base64,eyJuYW1lIjoiQSJ9", gateway),
            Some(Located::Inline { media: "application/json".to_string(), bytes: br#"{"name":"A"}"#.to_vec() })
        );
        assert_eq!(
            locate("data:image/svg+xml;charset=utf-8,%3Csvg%2F%3E", gateway),
            Some(Located::Inline { media: "image/svg+xml".to_string(), bytes: b"<svg/>".to_vec() })
        );
        assert_eq!(locate("data:,hi", gateway), Some(Located::Inline { media: "text/plain".to_string(), bytes: b"hi".to_vec() }));

        for refused in ["ar://abc", "javascript:alert(1)", "ipfs://", "data:no-comma", "data:,%zz", "data:;base64,!!", "1"] {
            assert_eq!(locate(refused, gateway), None, "{refused}");
        }
    }

    #[test]
    fn base64_round_trips_at_every_length() {
        for length in 0..10 {
            let bytes: Vec<u8> = (0..length).map(|index| (index * 37 + 5) as u8).collect();

            assert_eq!(decode_base64(&encode_base64(&bytes)), Some(bytes));
        }

        assert_eq!(encode_base64(b"hi"), "aGk=");
        assert_eq!(decode_base64("aGk"), Some(b"hi".to_vec()), "unpadded");
    }

    #[test]
    fn metadata_is_read_as_marketplaces_read_it() {
        let read = Metadata::read(
            br#"{"name":"Ape #1","description":"An ape","image":"ipfs://bafy/1.png",
                 "attributes":[{"trait_type":"Fur","value":"Gold"},{"trait_type":"Level","value":5},{"value":"bare"}],"extra":true}"#,
        )
        .unwrap();

        assert_eq!(read.name.as_deref(), Some("Ape #1"));
        assert_eq!(read.image.as_deref(), Some("ipfs://bafy/1.png"));
        assert_eq!(read.attributes, [("Fur".to_string(), "Gold".to_string()), ("Level".to_string(), "5".to_string()), (String::new(), "bare".to_string())]);

        let svg = Metadata::read(br#"{"image_data":"<svg/>"}"#).unwrap();

        assert_eq!(
            locate(svg.image.as_deref().unwrap(), DEFAULT_GATEWAY),
            Some(Located::Inline { media: "image/svg+xml".to_string(), bytes: b"<svg/>".to_vec() })
        );
        assert_eq!(Metadata::read(b"[1]"), None);
        assert_eq!(Metadata::read(b"{}"), Some(Metadata::default()));
    }
}
