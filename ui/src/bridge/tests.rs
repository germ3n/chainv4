use super::*;

#[test]
fn each_standing_says_where_a_transfer_is() {
    let threshold = Some(2);
    let of = |known: Known| Standing::of(Known { threshold, ..known });

    assert_eq!(of(Known { signed: 1, ..Known::default() }).word(), "signing, 1 of 2");
    assert_eq!(Standing::of(Known { signed: 1, ..Known::default() }).word(), "1 signed", "before the federation is known");
    assert_eq!(of(Known { leaving: true, signed: 2, ..Known::default() }).word(), "signed, to deliver");
    assert_eq!(of(Known { signed: 2, ..Known::default() }).word(), "signing, 2 of 2", "one coming here is done once enough signed, not delivered by anyone");
    assert_eq!(of(Known { arrived: true, ..Known::default() }).word(), "arrived");
    assert_eq!(of(Known { leaving: true, signed: 2, stuck: true, ..Known::default() }).word(), "stuck: its peer is gone");
    assert_eq!(of(Known { leaving: true, signed: 2, void_signed: 1, ..Known::default() }).word(), "voiding, 1 of 2");
    assert_eq!(of(Known { leaving: true, signed: 2, void_signed: 2, ..Known::default() }).word(), "void signed, to deliver");
    assert_eq!(of(Known { voided: true, ..Known::default() }).word(), "voided, refund to come");
    assert_eq!(of(Known { refunded: true, ..Known::default() }).word(), "refunded");
}

#[test]
fn the_furthest_along_wins_whatever_else_is_known() {
    let all = Known { leaving: true, signed: 2, threshold: Some(2), arrived: true, stuck: true, void_signed: 2, voided: true, refunded: true };

    assert_eq!(Standing::of(all), Standing::Refunded);
    assert_eq!(Standing::of(Known { refunded: false, ..all }), Standing::Voided);
    assert_eq!(Standing::of(Known { refunded: false, voided: false, ..all }), Standing::Arrived);
    assert_eq!(Standing::of(Known { refunded: false, voided: false, arrived: false, ..all }), Standing::Stuck);
    assert_eq!(Standing::of(Known { refunded: false, voided: false, arrived: false, stuck: false, ..all }), Standing::Voiding { signed: 2, threshold: Some(2) });
    assert!(Standing::Voided.refunding() && Standing::Refunded.refunding() && Standing::Voiding { signed: 1, threshold: None }.refunding());
    assert!(!Standing::Stuck.refunding() && !Standing::Arrived.refunding());
}

#[test]
fn a_standing_is_colored_by_whether_it_is_done_waiting_or_going_nowhere() {
    let palette = crate::theme::DARK;

    assert_eq!(Standing::Arrived.color(&palette), palette.good);
    assert_eq!(Standing::Stuck.color(&palette), palette.bad);
    assert_eq!(Standing::Signing { signed: 0, threshold: None }.color(&palette), palette.warn);
    assert_eq!(Standing::Refunded.color(&palette), palette.soft);
}

#[cfg(feature = "bridge")]
mod left {
    use {
        super::super::*,
        chainv4::structs::address::Address,
        chainv4::structs::hex::to_hex,
        serde_json::{Value, json},
    };

    /// A value as the node's /call writes it.
    fn typed(kind: &str, value: Value) -> Value {
        json!({ "type": kind, "value": value })
    }

    #[test]
    fn a_transfer_out_is_read_back_only_as_the_gateway_signed_it() {
        let identity = [&Address::Contract([7; 32]).to_abi()[..], b"1800000000000"].concat();
        let transfer = Transfer {
            from: identity.clone(),
            to: vec![0xee; 20],
            source: 1337,
            target: 11155111,
            nonce: 4,
            kind: AssetKind::Token,
            origin: 1337,
            asset: Address::Contract([9; 32]).to_abi().to_vec(),
            id: u256::ZERO,
            amount: u256::from(250u64),
            recipient: vec![0xb0; 20],
            name: "Dev Token".into(),
            symbol: "DVT".into(),
            decimals: 6,
            uri: String::new(),
        };
        let values = |digest: &[u8]| {
            vec![
                typed("i64", json!(11155111)),
                typed("i64", json!(1)),
                typed("i64", json!(1337)),
                typed("bytes", json!(to_hex(&transfer.asset))),
                typed("u256", json!("0")),
                typed("u256", json!("250")),
                typed("bytes", json!(to_hex(&transfer.recipient))),
                typed("string", json!("Dev Token")),
                typed("string", json!("DVT")),
                typed("i64", json!(6)),
                typed("string", json!("")),
                typed("bytes", json!(to_hex(digest))),
                typed("address", json!(Address::Ed25519([1; 32]).to_string())),
            ]
        };

        assert_eq!(left(&values(&transfer.digest()), &identity, [0xee; 20], 1337, 4), Some(transfer.clone()));
        // A node that changed any field, or the digest, is caught: the signers signed another.
        assert_eq!(left(&values(&[0; 32]), &identity, [0xee; 20], 1337, 4), None);
        assert_eq!(left(&values(&transfer.digest()), &identity, [0xee; 20], 1337, 5), None, "another nonce");
        assert_eq!(left(&values(&transfer.digest()), &identity, [0xdd; 20], 1337, 4), None, "another Gateway.sol, as one whose peer was replaced since reads");
        assert_eq!(left(&values(&transfer.digest()), &Address::Contract([7; 32]).to_abi(), [0xee; 20], 1337, 4), None, "the gateway as it was set up another time");
        assert_eq!(left(&values(&transfer.digest())[..11], &identity, [0xee; 20], 1337, 4), None, "an answer cut short");

        // Gateway.sol's delivered key differs by nonce, and by the gateway it came from: a chain
        // restarted from the identical genesis counts its nonces from 1 again.
        assert_ne!(transfer.delivered_key(), Transfer { nonce: 5, ..transfer.clone() }.delivered_key());
        assert_ne!(transfer.delivered_key(), Transfer { from: Address::Contract([7; 32]).to_abi().to_vec(), ..transfer.clone() }.delivered_key());
    }
}
