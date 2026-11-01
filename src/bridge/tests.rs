use super::*;

fn transfer() -> Transfer {
    Transfer {
        from: vec![1; 33],
        to: vec![2; 20],
        source: 0x1234_5678,
        target: 11155111,
        nonce: 7,
        kind: AssetKind::Token,
        origin: 0x1234_5678,
        asset: vec![3; 33],
        id: u256::ZERO,
        amount: u256::from(5_000u64),
        recipient: vec![4; 20],
        name: "Example Token".to_string(),
        symbol: "EXT".to_string(),
        decimals: 18,
        uri: String::new(),
    }
}

#[test]
fn every_field_changes_the_digest() {
    let base = transfer().digest();
    let changed = [
        Transfer { from: vec![9; 33], ..transfer() },
        Transfer { to: vec![9; 20], ..transfer() },
        Transfer { source: 1, ..transfer() },
        Transfer { target: 1, ..transfer() },
        Transfer { nonce: 8, ..transfer() },
        Transfer { kind: AssetKind::Coin, ..transfer() },
        Transfer { origin: 1, ..transfer() },
        Transfer { asset: Vec::new(), ..transfer() },
        Transfer { id: u256::from(1u64), ..transfer() },
        Transfer { amount: u256::from(5_001u64), ..transfer() },
        Transfer { recipient: vec![5; 20], ..transfer() },
        Transfer { name: "Other".to_string(), ..transfer() },
        Transfer { symbol: "OTH".to_string(), ..transfer() },
        Transfer { decimals: 6, ..transfer() },
        Transfer { uri: "ipfs://x".to_string(), ..transfer() },
    ];

    for (index, other) in changed.iter().enumerate() {
        assert_ne!(other.digest(), base, "field {index}");
    }
}

#[test]
fn a_signature_recovers_its_signer_as_ecrecover_would() {
    let secret = [7; 32];
    let signature = transfer().sign(&secret);

    assert!(matches!(signature[64], 27 | 28));
    let rs: [u8; 64] = signature[..64].try_into().unwrap();

    assert_eq!(crate::crypto::secp256k1::recover_strict(&transfer().digest(), &rs, signature[64] - 27), crate::crypto::secp256k1::public_key(&secret));
    assert_eq!(transfer().attest_args(&signature).len(), 14);
    assert_eq!(AssetKind::from_number(3), Some(AssetKind::Multi));
    assert_eq!(AssetKind::from_number(4), None);
}

#[cfg(feature = "bitcoin")]
mod planning {
    use {super::super::*, crate::crypto::bitcoin::tx::p2wpkh};

    const CAP: u64 = 50_000;
    const FLOOR: u64 = 2;
    const CHANGE: [u8; 34] = [0; 34];

    fn queued(id: u64, sats: u64, most_fee: u64) -> NamedWithdrawal {
        NamedWithdrawal { id, script: p2wpkh(&[id as u8; 20]), sats, most_fee, state: "queued".to_string(), anchor: None }
    }

    fn held(n: u8, sats: u64) -> (String, SpentOutput) {
        (format!("{}:0", crate::structs::hex::to_hex(&[n; 32]).trim_start_matches("0x")), SpentOutput { sats, version: 0, state: "unspent".to_string(), threshold: 2, script_len: 180 })
    }

    fn plan(withdrawals: &[NamedWithdrawal], outputs: &[(String, SpentOutput)], fee: ReleaseFee) -> ReleasePlan {
        plan_btc_release(withdrawals, outputs, &CHANGE, CAP, FLOOR, fee).unwrap()
    }

    #[test]
    fn signers_with_the_same_state_and_rate_work_out_the_same_release() {
        let withdrawals = [queued(1, 20_000, 2_000), queued(2, 30_000, 3_000)];
        let outputs = [held(1, 40_000), held(2, 60_000), held(3, 70_000)];
        let first = plan(&withdrawals, &outputs, ReleaseFee::Rate(5));

        assert_eq!(first, plan(&withdrawals, &outputs, ReleaseFee::Rate(5)));
        let tx = first.tx.unwrap();
        assert_eq!(first.ids, [1, 2]);
        assert_eq!(tx.inputs.iter().map(|input| input.txid[0]).collect::<Vec<_>>(), [1, 2], "what it took first, as far as it needs");
        // What it spends past what they burned is change, so the fee is theirs.
        assert_eq!(tx.outputs[2].value, 100_000 - 50_000);
        assert_eq!(tx.outputs[0].value + tx.outputs[1].value, 50_000 - first.fee);
        assert_eq!(first.fee % 5, 0, "at this rate");
        assert!(first.waiting.is_empty());
    }

    #[test]
    fn a_rate_under_the_floor_pays_the_floor() {
        let chosen = plan(&[queued(1, 20_000, 2_000)], &[held(1, 20_000)], ReleaseFee::Rate(0));
        let raw = chosen.tx.unwrap().base();

        assert_eq!(chosen.fee, btc_release_vbytes(raw.len(), &[(2, 180)]) * FLOOR);
    }

    #[test]
    fn a_withdrawal_whose_limit_cannot_cover_its_share_does_not_hold_up_the_rest_and_says_why() {
        let withdrawals = [queued(1, 20_000, 1), queued(2, 30_000, 3_000), queued(3, 30_000, 2)];
        let chosen = plan(&withdrawals, &[held(1, 100_000)], ReleaseFee::Rate(5));

        assert_eq!(chosen.ids, [2], "the oldest and the third wait, and the second goes");
        assert_eq!(chosen.waiting.iter().map(|(id, _)| *id).collect::<Vec<_>>(), [1, 3]);
        assert!(chosen.waiting[0].1.contains("agreed to lose 1"), "{:?}", chosen.waiting);

        // Fifty that cannot go do not keep the fifty-first out either.
        let mut full: Vec<NamedWithdrawal> = (1..=50).map(|id| queued(id, 20_000, 1)).collect();
        full.push(queued(51, 20_000, 2_000));
        assert_eq!(plan(&full, &[held(1, 100_000)], ReleaseFee::Rate(5)).ids, [51]);
    }

    #[test]
    fn nothing_goes_if_no_limit_covers_the_fee_and_all_of_it_is_said() {
        let chosen = plan(&[queued(1, 20_000, 1), queued(2, 20_000, 1)], &[held(1, 100_000)], ReleaseFee::Rate(5));

        assert_eq!((chosen.tx, chosen.ids.len(), chosen.waiting.len()), (None, 0, 2));
    }

    #[test]
    fn past_the_cap_at_this_rate_a_release_pays_fewer_and_the_rest_go_in_the_next() {
        // 90 satoshis a virtual byte: fine for a few, not for all of them.
        let withdrawals: Vec<NamedWithdrawal> = (1..=20).map(|id| queued(id, 200_000, 100_000)).collect();
        let chosen = plan_btc_release(&withdrawals, &[held(1, 10_000_000)], &CHANGE, CAP, FLOOR, ReleaseFee::Rate(90)).unwrap();

        assert!(chosen.tx.is_some() && chosen.fee <= CAP, "{}", chosen.fee);
        assert!((2..20).contains(&chosen.ids.len()), "{}", chosen.ids.len());
        assert_eq!(chosen.ids[0], 1, "oldest first");
        assert!(chosen.waiting.is_empty(), "those left are next, and waiting for nothing");

        // Past the cap for even one: it stays queued, and says so.
        let alone = plan_btc_release(&withdrawals[..1], &[held(1, 10_000_000)], &CHANGE, CAP, FLOOR, ReleaseFee::Rate(5_000)).unwrap();
        assert_eq!(alone.tx, None);
        assert!(alone.waiting[0].1.contains("until fees fall"), "{:?}", alone.waiting);
    }

    #[test]
    fn a_withdrawal_past_what_the_gateway_holds_does_not_hold_up_a_smaller_one() {
        let chosen = plan(&[queued(1, 500_000, 50_000), queued(2, 20_000, 2_000)], &[held(1, 30_000)], ReleaseFee::Rate(5));

        assert_eq!(chosen.ids, [2]);
        assert_eq!(chosen.waiting[0].0, 1);
        assert!(chosen.waiting[0].1.contains("too little"), "{:?}", chosen.waiting);
    }

    #[test]
    fn a_withdrawal_from_a_cancelled_release_spends_its_anchor_or_waits_for_it() {
        let outputs = [held(1, 100_000), held(2, 50_000)];
        let anchored = NamedWithdrawal { anchor: Some(outputs[1].0.clone()), ..queued(1, 20_000, 2_000) };
        let chosen = plan(std::slice::from_ref(&anchored), &outputs, ReleaseFee::Rate(5));

        assert_eq!(chosen.tx.unwrap().inputs.iter().map(|input| input.txid[0]).collect::<Vec<_>>(), [2], "that one, though another came first");

        // Its anchor is in another release: only the others go.
        let elsewhere = NamedWithdrawal { anchor: Some(held(9, 1).0), ..queued(1, 20_000, 2_000) };
        let chosen = plan(&[elsewhere, queued(2, 20_000, 2_000)], &outputs, ReleaseFee::Rate(5));
        assert_eq!(chosen.ids, [2]);
        assert!(chosen.waiting[0].1.contains("in another release"), "{:?}", chosen.waiting);
    }

    #[test]
    fn a_fee_told_is_kept_to_or_refused_rather_than_changed() {
        let one = [queued(1, 20_000, 2_000)];
        let outputs = [held(1, 20_000)];

        assert_eq!(plan(&one, &outputs, ReleaseFee::Fixed(1_000)).fee, 1_000);
        assert!(plan_btc_release(&one, &outputs, &CHANGE, CAP, FLOOR, ReleaseFee::Fixed(1)).unwrap_err().contains("under the floor"));
        assert!(plan_btc_release(&one, &outputs, &CHANGE, CAP, FLOOR, ReleaseFee::Fixed(CAP + 1)).unwrap_err().contains("past the gateway's cap"));
    }

    #[test]
    fn what_a_signer_signs_for_follows_its_operator_then_the_network_then_a_fixed_limit() {
        assert_eq!(btc_most_fee_rate(Some(7), Some(1_000.0), 2), 7, "the operator's word first");
        assert_eq!(btc_most_fee_rate(None, Some(40.2), 2), 41 * BTC_RATE_TIMES_FASTEST);
        assert_eq!(btc_most_fee_rate(None, Some(1.0), 2), BTC_RATE_ALWAYS_SIGNED, "a quiet network refuses no normal fee");
        assert_eq!(btc_most_fee_rate(None, Some(1.0), 40), 40, "the floor is always allowed");
        assert_eq!(btc_most_fee_rate(None, None, 2), BTC_RATE_BLIND, "Esplora out of reach");
        assert_eq!(btc_most_fee_rate(None, None, 500), 500);
    }
}
