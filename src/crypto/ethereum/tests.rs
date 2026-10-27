use super::*;
use crate::structs::hex::{from_hex, to_hex};

/// EIP-155's own example: a legacy transaction for chain 1, signed by the key of 32 bytes of 0x46.
const EIP155_RAW: &str = "0xf86c098504a817c800825208943535353535353535353535353535353535353535880de0b6b3a76400008025a028ef61340bd939bc2195fe537567866003e1a15d3c71ff63e1590620aa636276a067cbe9d8997f761aecb703304b3800ccf555c9f3dc64214b297fb1966a3b6d83";

fn unsigned(kind: EthereumKind) -> EthereumTx {
    EthereumTx {
        kind,
        chain_id: 4_242,
        nonce: 7,
        max_price: u256::from(2_000_000_000u64),
        tip: match kind {
            EthereumKind::DynamicFee => u256::from(1_000u64),
            _ => u256::from(2_000_000_000u64),
        },
        gas_limit: 90_000,
        to: Some([0x35; 20]),
        value: u256::from(10u64).pow(18),
        input: vec![0xb2, 0x0e, 0xb4, 0xc4],
        access_list: match kind {
            EthereumKind::Legacy => Vec::new(),
            _ => vec![AccessItem { address: [0x11; 20], slots: vec![[0; 32], [0xff; 32]] }],
        },
        signature: [0; 64],
        parity: 0,
    }
}

#[test]
fn eip155s_example_reads_hashes_and_recovers_its_sender() {
    let raw = from_hex(EIP155_RAW).unwrap();
    let tx = decode(&raw).unwrap();

    assert_eq!((tx.kind, tx.chain_id, tx.nonce, tx.gas_limit), (EthereumKind::Legacy, 1, 9, 21_000));
    assert_eq!(tx.max_price, u256::from(20_000_000_000u64));
    assert_eq!(tx.to, Some([0x35; 20]));
    assert_eq!(tx.value, u256::from(10u64).pow(18));
    assert_eq!(to_hex(&tx.signing_hash()), "0xdaf5a779ae972f972197303d7b574746c7ef83eadac0f2791ad23db92e4c8e53", "as EIP-155 gives it");
    assert_eq!(tx.encode(), raw, "written back byte for byte");

    let key = tx.signer().expect("its signature recovers a key");

    assert_eq!(to_hex(&crate::crypto::secp256k1::address(&key).unwrap()), "0x9d8a62f656a8d1615c1294fd71e9cfb3e4855a4f", "the key of 0x46…46");
    assert_eq!(key, crate::crypto::secp256k1::public_key(&[0x46; 32]).unwrap());
    assert_eq!(to_hex(&tx_hash(&raw)), to_hex(&keccak(&raw)));
}

#[test]
fn every_type_signs_reads_back_and_recovers_its_key() {
    let secret = [0x46; 32];

    for kind in [EthereumKind::Legacy, EthereumKind::AccessList, EthereumKind::DynamicFee] {
        let signed = unsigned(kind).signed(&secret);
        let raw = signed.encode();
        let read = decode(&raw).unwrap();

        assert_eq!(read, signed, "{kind:?}");
        assert_eq!(read.signer(), crate::crypto::secp256k1::public_key(&secret), "{kind:?}");

        if kind != EthereumKind::Legacy {
            assert_eq!(raw[0], kind.number(), "typed: its type first");
        }

        // Anything changed, the signature recovers another key, or none.
        let mut changed = read.clone();
        changed.nonce += 1;

        assert_ne!(changed.signer(), read.signer(), "{kind:?}");
    }

    // A create has no to.
    let create = EthereumTx { to: None, ..unsigned(EthereumKind::DynamicFee) }.signed(&secret);

    assert_eq!(decode(&create.encode()).unwrap().to, None);
}

#[test]
fn what_ethereum_refuses_is_refused() {
    let secret = [0x46; 32];
    let signed = unsigned(EthereumKind::DynamicFee).signed(&secret);

    // A high s signs the same, reshaped: Ethereum refuses it, so it recovers nothing.
    let order = u256::from_str_radix("fffffffffffffffffffffffffffffffebaaedce6af48a03bbfd25e8cd0364141", 16).unwrap();
    let s = u256::from_be_bytes(signed.signature[32..].try_into().unwrap());
    let mut high = signed.clone();
    high.signature[32..].copy_from_slice(&(order - s).to_be_bytes());
    high.parity ^= 1;

    assert_eq!(high.signer(), None);

    // Pre-EIP-155 legacy transactions are replayable on any chain.
    let raw = from_hex(EIP155_RAW).unwrap();
    let mut old = decode(&raw).unwrap();
    old.chain_id = 0;
    let mut bytes = old.encode();
    // v = 35 + parity for chain 0 is still EIP-155's; write 27 instead.
    let at = bytes.len() - 67;
    bytes[at] = 27;

    assert!(decode(&bytes).unwrap_err().contains("EIP-155"));

    // What is no transaction, or not of a type this chain takes.
    assert!(decode(&[]).is_err());
    assert!(decode(&[3, 0xc0]).is_err(), "blob transactions are not taken");
    assert!(decode(&[2, 0xc0]).is_err(), "too few fields");
    assert!(decode(&[0xc1, 0x80, 0x00]).is_err(), "trailing bytes");
    assert!(decode(&vec![0xc0; MAX_RAW + 1]).is_err());

    // Encodings RLP never writes: a leading zero, a low byte as a string, a short length written long.
    assert!(item(&[0x82, 0x00, 0x01]).is_ok(), "bytes may start with zero");
    assert!(number(&item(&[0x82, 0x00, 0x01]).unwrap().0, "n").is_err(), "but a number may not");
    assert!(item(&[0x81, 0x05]).is_err());
    assert!(item(&[0xb8, 0x02, 0x01, 0x02]).is_err());
    assert_eq!(item(&[0x80]).unwrap().0, Item::Bytes(&[]));
}

#[test]
#[cfg(feature = "secp256k1")]
fn unsigned_transactions_read_as_a_wallet_hands_them_to_a_hardware_wallet() {
    for kind in [EthereumKind::Legacy, EthereumKind::AccessList, EthereumKind::DynamicFee] {
        let tx = unsigned(kind);
        let bytes = tx.fields(false);
        let read = decode_unsigned(&bytes).unwrap();

        assert_eq!(read, EthereumTx { signature: [0; 64], parity: 0, ..tx.clone() }, "{kind:?}");
        assert_eq!(read.signing_hash(), keccak(&bytes), "{kind:?}: what is shown is what is signed");
    }

    // EIP-155's own: its signing data, and what it signs.
    let signed = decode(&from_hex(EIP155_RAW).unwrap()).unwrap();
    let data = signed.fields(false);

    assert_eq!(decode_unsigned(&data).unwrap().signing_hash(), signed.signing_hash());

    // A signed one, one without a chain id, and one written another way than RLP writes it.
    assert!(decode_unsigned(&from_hex(EIP155_RAW).unwrap()).is_err());

    let mut old = Vec::new();
    let mut body = Vec::new();

    for field in [&[9u8][..], &[1], &[0x52, 0x08], &[0x35; 20], &[1], &[]] {
        put_bytes(&mut body, field);
    }

    put_list(&mut old, &body);

    assert!(decode_unsigned(&old).unwrap_err().contains("EIP-155"));

    let mut padded = data.clone();
    padded.push(0);

    assert!(decode_unsigned(&padded).is_err());
    assert!(decode_unsigned(&[]).is_err());
}
