use {super::*, crate::crypto::secp256k1, crate::structs::hex::to_hex, serde_json::json};

/// What MetaMask's eth-sig-util gives for the same data, version 4: EIP-712's own Mail example,
/// and one with arrays of structs and of arrays, a struct left out, and integers at their edges.
const VECTORS: &str = include_str!("vectors.json");

/// The key that signs them: the keccak256 of "cow", as EIP-712's example has it.
const COW: &str = "0xc85ef7d79691fe79573b1a7064c19c1a9819ebdbd1faaab1a8ec92344438aaf4";

fn signed(digest: &[u8; 32]) -> String {
    let secret: [u8; 32] = from_hex(COW).unwrap().try_into().unwrap();
    let (signature, parity) = secp256k1::sign_recoverable(&secret, digest);

    to_hex(&[signature.as_slice(), &[27 + parity]].concat())
}

#[test]
fn typed_data_hashes_and_signs_as_metamask_does() {
    let vectors: Value = serde_json::from_str(VECTORS).unwrap();

    for name in ["mail", "hard"] {
        let vector = &vectors[name];
        let data = TypedData::parse(&vector["json"]).unwrap();
        let digest = data.digest().unwrap();

        assert_eq!(data.encode_type(&data.primary).unwrap(), vector["order"].as_str().unwrap(), "{name}");
        assert_eq!(to_hex(&digest), vector["digest"].as_str().unwrap(), "{name}");
        assert_eq!(signed(&digest), vector["signature"].as_str().unwrap(), "{name}");

        // As sites pass it too: the JSON's text.
        let text = Value::String(vector["json"].to_string());

        assert_eq!(TypedData::parse(&text).unwrap().digest().unwrap(), digest, "{name}");
    }

    assert_eq!(to_hex(&TypedData::parse(&vectors["mail"]["json"]).unwrap().digest().unwrap()), "0xbe609aee343fb3c4b28e1df9e632fca64fcfaede20f02e86244efddf30957bd2", "EIP-712's own");
}

#[test]
fn data_that_does_not_fit_its_types_is_refused() {
    let data = |ty: &str, value: Value| {
        TypedData::parse(&json!({
            "types": { "EIP712Domain": [], "M": [{ "name": "v", "type": ty }] },
            "primaryType": "M",
            "domain": {},
            "message": { "v": value },
        }))
        .unwrap()
        .digest()
    };

    assert!(data("uint8", json!(255)).is_ok());
    assert!(data("uint8", json!(256)).is_err());
    assert!(data("uint8", json!(-1)).is_err());
    assert!(data("int8", json!("-128")).is_ok());
    assert!(data("int8", json!(128)).is_err());
    assert!(data("int8", json!(-129)).is_err());
    assert!(data("bytes2", json!("0x010203")).is_err());
    assert!(data("address", json!("0x1234")).is_err());
    assert!(data("uint256[2]", json!([1, 2, 3])).is_err());
    assert!(data("float", json!(1)).is_err());
    assert!(data("bool", json!("maybe")).is_err());
    assert!(TypedData::parse(&json!({ "types": {}, "primaryType": "Nope" })).is_err());

    // A domain without EIP712Domain in its types goes by the fields it has.
    let bare = TypedData::parse(&json!({ "types": { "M": [] }, "primaryType": "M", "domain": { "name": "x", "chainId": 1 }, "message": {} })).unwrap();

    assert_eq!(bare.encode_type("EIP712Domain").unwrap(), "EIP712Domain(string name,uint256 chainId)");

    // A type that holds itself is refused, not followed forever.
    let looping = json!({ "types": { "EIP712Domain": [], "Node": [{ "name": "next", "type": "Node" }] }, "primaryType": "Node", "domain": {}, "message": nest(40) });

    assert!(TypedData::parse(&looping).unwrap().digest().is_err());
}

fn nest(depth: usize) -> Value {
    match depth {
        0 => Value::Null,
        depth => json!({ "next": nest(depth - 1) }),
    }
}
