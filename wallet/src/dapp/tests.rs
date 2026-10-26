use {super::*, serde_json::json};

/// The keccak256 of "cow", which EIP-712's example signs with.
const COW: &str = "0xc85ef7d79691fe79573b1a7064c19c1a9819ebdbd1faaab1a8ec92344438aaf4";
/// Its address.
const COW_ADDRESS: &str = "0xcd2a3d9f938e13cd947ec05abc7fe734df8dd826";

fn cow() -> [u8; 32] {
    from_hex(COW).unwrap().try_into().unwrap()
}

fn read(method: &str, params: Value) -> Result<Request, Unread> {
    Request::read(&json!({ "id": 7, "origin": "https://app.example", "method": method, "params": params }))
}

#[test]
fn messages_and_typed_data_sign_as_metamask_signs_them() {
    // What MetaMask's eth-sig-util gives for the same key and message.
    let request = read("personal_sign", json!(["0x5369676e20696e20746f20636861696e7634", COW_ADDRESS])).unwrap();
    let Asked::Message { address, message } = &request.asked else { panic!("a message") };

    assert_eq!(to_hex(address), COW_ADDRESS);
    assert_eq!(message, b"Sign in to chainv4");
    assert_eq!(
        signature(&cow(), &personal_digest(message)),
        "0x0026da8ec6c2acddd6f533c20b2926819ac72f6d305e97c4367c71290d5925917a5ec56d275caf7c7d0775507e84a03f079076d8524acdf3b63d480efdfb22fe1b"
    );

    // The address first, as a few sites send it; and plain text rather than hex.
    let swapped = read("personal_sign", json!([COW_ADDRESS, "Sign in to chainv4"])).unwrap();

    assert!(matches!(&swapped.asked, Asked::Message { message: same, .. } if same == message));

    // EIP-712's own example, as text, as sites pass it.
    let mail = json!({
        "types": {
            "EIP712Domain": [{ "name": "name", "type": "string" }, { "name": "version", "type": "string" }, { "name": "chainId", "type": "uint256" }, { "name": "verifyingContract", "type": "address" }],
            "Person": [{ "name": "name", "type": "string" }, { "name": "wallet", "type": "address" }],
            "Mail": [{ "name": "from", "type": "Person" }, { "name": "to", "type": "Person" }, { "name": "contents", "type": "string" }]
        },
        "primaryType": "Mail",
        "domain": { "name": "Ether Mail", "version": "1", "chainId": 1, "verifyingContract": "0xCcCCccccCCCCcCCCCCCcCcCccCcCCCcCcccccccC" },
        "message": { "from": { "name": "Cow", "wallet": "0xCD2a3d9F938E13CD947Ec05AbC7FE734Df8DD826" }, "to": { "name": "Bob", "wallet": "0xbBbBBBBbbBBBbbbBbbBbbbbBBbBbbbbBbBbbBBbB" }, "contents": "Hello, Bob!" }
    });
    let typed = read("eth_signTypedData_v4", json!([COW_ADDRESS, mail.to_string()])).unwrap();
    let Asked::Typed { data, json, .. } = &typed.asked else { panic!("typed data") };

    assert_eq!(json["domain"]["name"], "Ether Mail");
    assert_eq!(
        signature(&cow(), &data.digest().unwrap()),
        "0x4355c47d63924e8a72e509b65029052eb6c299d53a04e167c5775fd466751c9d07299936d304c153f6443dfa05f40ff007d72911b6f72307f996231605b915621c"
    );
}

#[test]
fn transactions_are_read_as_sites_write_them() {
    let request = read("eth_sendTransaction", json!([{ "from": COW_ADDRESS, "to": "0x2222222222222222222222222222222222222222", "value": "0xde0b6b3a7640000", "data": "0xa9059cbb", "gas": "0x5208", "maxFeePerGas": "0x3b9aca00" }])).unwrap();
    let Asked::Send(wanted) = &request.asked else { panic!("a send") };

    assert_eq!(request.signer().map(|signer| to_hex(&signer)).as_deref(), Some(COW_ADDRESS));
    assert_eq!(wanted.to, Some([0x22; 20]));
    assert_eq!(wanted.value, u256::new(1_000_000_000_000_000_000));
    assert_eq!(wanted.input, vec![0xa9, 0x05, 0x9c, 0xbb]);
    assert_eq!((wanted.gas, wanted.max_price, wanted.tip, wanted.nonce), (Some(21_000), Some(u256::new(1_000_000_000)), None, None));

    // gasPrice for an older site, input for a newer one, nothing left to work out.
    let older = read("eth_sendTransaction", json!([{ "from": COW_ADDRESS, "to": "0x2222222222222222222222222222222222222222", "gasPrice": "0x10", "input": "0x01" }])).unwrap();
    let Asked::Send(older) = &older.asked else { panic!("a send") };

    assert_eq!((older.max_price, older.value, older.input.as_slice()), (Some(u256::new(16)), u256::ZERO, &[1u8][..]));

    for bad in [json!([{}]), json!([{ "from": COW_ADDRESS }]), json!([{ "from": COW_ADDRESS, "to": "0x12" }]), json!([{ "from": COW_ADDRESS, "to": COW_ADDRESS, "value": 5 }])] {
        assert_eq!(read("eth_sendTransaction", bad.clone()).unwrap_err().0, 7, "{bad}");
    }

    assert!(read("eth_sign", json!([])).is_err());
    assert!(matches!(read("eth_requestAccounts", json!([])).unwrap().asked, Asked::Connect));
}

#[test]
fn the_wallet_says_its_accounts_only_while_unlocked() {
    let open = state("http://node", Some(0x22cc), &[[0xab; 20]], true);

    assert_eq!(open, json!({ "node": "http://node", "chainId": "0x22cc", "accounts": [format!("0x{}", "ab".repeat(20))], "unlocked": true }));
    assert_eq!(state("http://node", None, &[], false), json!({ "node": "http://node", "unlocked": false }), "a locked wallet leaves the ones it said");
}
