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

#[test]
fn a_void_and_a_refund_are_signed_over_their_own_domains_and_every_field_changes_them() {
    let base = transfer();
    let (void, refund) = (base.void_digest(), base.refund_digest(&[6; 33]));

    // None passes for another, nor for a signer set.
    let signers = signers_digest([2; 20], 11155111, 7, &[[4; 20]], 18);
    let all = [base.digest(), void, refund, signers];

    for (at, one) in all.iter().enumerate() {
        for other in &all[at + 1..] {
            assert_ne!(one, other);
        }
    }

    // Each ABI encoding starts with where its domain is, a place none shares, so one's bytes could be
    // no other's whatever the fields say; and its domain, which is its own.
    let head = |types: &str| crate::abi::parse_signature(types).unwrap().1.len() * 32;
    let places = [head("(string,bytes32,bytes32)"), head("(string,bytes,bytes,uint64,uint64,uint64,bytes32)"), head("(string,bytes,bytes,uint64,uint64,uint64,bytes32,bytes)"), head("(string,address,uint256,uint64,address[],uint256)")];

    assert_eq!(places, [0x60, 0xe0, 0x100, 0xc0]);
    assert_eq!((DOMAIN, VOID_DOMAIN, REFUND_DOMAIN), ("chainv4-gateway", "chainv4-gateway-void", "chainv4-gateway-refund"));

    let changed = [
        Transfer { from: vec![9; 33], ..transfer() },
        Transfer { to: vec![9; 20], ..transfer() },
        Transfer { source: 1, ..transfer() },
        Transfer { target: 1, ..transfer() },
        Transfer { nonce: 8, ..transfer() },
        Transfer { amount: u256::from(5_001u64), ..transfer() },
        Transfer { recipient: vec![5; 20], ..transfer() },
    ];

    for (index, other) in changed.iter().enumerate() {
        assert_ne!(other.void_digest(), void, "field {index}");
        assert_ne!(other.refund_digest(&[6; 33]), refund, "field {index}");
    }

    assert_ne!(base.refund_digest(&[7; 33]), refund, "whom it pays is signed");
    assert_eq!(base.attest_void_args(&[1; 65]).len(), 5);
    assert_eq!(base.attest_refund_args(&[6; 33], &[1; 65]).len(), 6);
}

#[cfg(feature = "bitcoin")]
mod stuck_releases {
    use super::super::*;

    fn standing(refusal: Option<ReleaseRefusal>) -> ReleaseStanding {
        ReleaseStanding { refusal, blind: false, signed_by_me: false, signed_enough: false, past_limit: Some(false) }
    }

    fn wrong() -> Option<ReleaseRefusal> {
        Some(ReleaseRefusal::Wrong("it pays someone else".to_string()))
    }

    fn overpriced() -> Option<ReleaseRefusal> {
        Some(ReleaseRefusal::Overpriced("past the limit".to_string()))
    }

    fn voted(step: ReleaseStep) -> bool {
        matches!(step, ReleaseStep::Vote(_))
    }

    #[test]
    fn a_release_refused_for_good_takes_a_vote_at_once_and_one_that_stands_a_signature() {
        assert_eq!(btc_release_step(&standing(None)), ReleaseStep::Sign);
        assert_eq!(btc_release_step(&standing(wrong())), ReleaseStep::Vote("it pays someone else".to_string()));
        assert_eq!(btc_release_step(&standing(overpriced())), ReleaseStep::Vote("past the limit".to_string()));
        assert_eq!(btc_release_step(&ReleaseStanding { signed_by_me: true, ..standing(None) }), ReleaseStep::Sign, "for any input it has not");
    }

    #[test]
    fn overpriced_by_a_limit_with_no_fee_rate_behind_it_takes_no_vote() {
        let blind = ReleaseStanding { blind: true, ..standing(overpriced()) };

        assert!(matches!(btc_release_step(&blind), ReleaseStep::Leave(_)), "that may pass once Esplora answers");
        // What is wrong whatever the fee is still wrong with no fee to go by.
        assert!(voted(btc_release_step(&ReleaseStanding { blind: true, ..standing(wrong()) })));
    }

    #[test]
    fn a_signer_that_signed_a_release_votes_it_down_only_past_the_limit_or_when_it_can_never_be_mined() {
        let signed = ReleaseStanding { signed_by_me: true, ..standing(overpriced()) };

        assert!(matches!(btc_release_step(&signed), ReleaseStep::Leave(_)));
        assert!(voted(btc_release_step(&ReleaseStanding { past_limit: Some(true), ..signed.clone() })), "else one left alone by the others could never go");
        let spent = ReleaseStanding { refusal: Some(ReleaseRefusal::Dead("spent".to_string())), ..signed };
        assert!(voted(btc_release_step(&spent)));
    }

    #[test]
    fn past_the_limit_a_release_short_of_its_signatures_takes_a_vote_rather_than_a_signature() {
        assert!(voted(btc_release_step(&ReleaseStanding { past_limit: Some(true), ..standing(None) })));
        assert_eq!(btc_release_step(&ReleaseStanding { past_limit: None, ..standing(None) }), ReleaseStep::Sign, "an age unread says nothing");
        assert!(voted(btc_release_step(&ReleaseStanding { past_limit: None, ..standing(wrong()) })), "and stops no other reason");
    }

    #[test]
    fn a_release_with_all_its_signatures_takes_no_vote_as_the_gateway_would_refuse_it_unless_it_can_never_be_mined() {
        let held = |refusal, past_limit| ReleaseStanding { signed_enough: true, past_limit, ..standing(refusal) };

        assert_eq!(btc_release_step(&held(None, Some(true))), ReleaseStep::Nothing, "one more would only cost");
        assert!(matches!(btc_release_step(&held(overpriced(), Some(true))), ReleaseStep::Leave(_)));
        assert!(matches!(btc_release_step(&held(wrong(), Some(false))), ReleaseStep::Leave(_)));
        // The gateway takes a vote on one Bitcoin can never mine, however many signed.
        assert!(voted(btc_release_step(&held(Some(ReleaseRefusal::Dead("spent".to_string())), Some(false)))));
    }

    #[test]
    fn an_input_shown_spent_is_told_apart_from_a_release_that_is_wrong_or_only_dear() {
        let mut facts = ReleaseFacts {
            raw: vec![1, 2, 3],
            txid: vec![0; 32],
            version: 0,
            withdrawals: Vec::new(),
            inputs: vec![("aa:0".to_string(), SpentOutput { sats: 1_000, version: 0, state: "spent".to_string(), threshold: 2, script_len: 100 })],
            change_script: Vec::new(),
            fee_cap: 50_000,
            fee_floor: 2,
            most_rate: 100,
            repay: ReleaseRepay::default(),
        };

        assert!(matches!(check_btc_release(&facts), Err(ReleaseRefusal::Dead(_))));
        facts.inputs[0].1.state = "pending".to_string();
        assert!(matches!(check_btc_release(&facts), Err(ReleaseRefusal::Wrong(_))));
    }

    #[test]
    fn a_cancelled_rate_is_proposed_again_only_well_under_it_or_past_the_limit() {
        const LIMIT: u64 = BTC_CANCEL_AFTER_MS;

        assert!(!btc_may_propose_again(100, 100, Some(60_000), LIMIT), "the same rate a minute later");
        assert!(!btc_may_propose_again(200, 100, Some(60_000), LIMIT), "or more");
        assert!(!btc_may_propose_again(76, 100, Some(60_000), LIMIT), "or a little less, as estimates wander");
        assert!(btc_may_propose_again(75, 100, Some(60_000), LIMIT), "a quarter less is another question");
        assert!(btc_may_propose_again(100, 100, Some(LIMIT), LIMIT), "once past the limit, as often as that at most");
        assert!(btc_may_propose_again(500, 100, Some(LIMIT + 1), LIMIT), "one cancelled for its age holds up nothing");
        assert!(!btc_may_propose_again(100, 100, None, LIMIT), "an age unread errs towards not going round");
        assert!(btc_may_propose_again(10, 100, None, LIMIT));
    }

    #[test]
    fn the_warning_says_which_what_it_pays_against_what_the_network_asks_and_what_it_needs() {
        let warning = btc_stuck_warning(7, "abcd", 7 * 3_600_000, 3, Some(40));

        for part in ["release 7", "txid abcd", "after 7 hours", "pays 3 satoshis", "the network asks 40", "cannot be cancelled", "child-pays-for-parent"] {
            assert!(warning.contains(part), "{part}: {warning}");
        }

        assert!(btc_stuck_warning(7, "abcd", 0, 3, None).contains("gives no rate"));
    }
}

#[cfg(feature = "bitcoin")]
mod planning {
    use {super::super::*, crate::crypto::bitcoin::tx::{p2wpkh, p2wsh}};

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
        plan_btc_release(withdrawals, outputs, &CHANGE, CAP, FLOOR, fee, &ReleaseRepay::default()).unwrap()
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
        let chosen = plan_btc_release(&withdrawals, &[held(1, 10_000_000)], &CHANGE, CAP, FLOOR, ReleaseFee::Rate(90), &ReleaseRepay::default()).unwrap();

        assert!(chosen.tx.is_some() && chosen.fee <= CAP, "{}", chosen.fee);
        assert!((2..20).contains(&chosen.ids.len()), "{}", chosen.ids.len());
        assert_eq!(chosen.ids[0], 1, "oldest first");
        assert!(chosen.waiting.is_empty(), "those left are next, and waiting for nothing");

        // Past the cap for even one: it stays queued, and says so.
        let alone = plan_btc_release(&withdrawals[..1], &[held(1, 10_000_000)], &CHANGE, CAP, FLOOR, ReleaseFee::Rate(5_000), &ReleaseRepay::default()).unwrap();
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
        assert!(plan_btc_release(&one, &outputs, &CHANGE, CAP, FLOOR, ReleaseFee::Fixed(1), &ReleaseRepay::default()).unwrap_err().contains("under the floor"));
        assert!(plan_btc_release(&one, &outputs, &CHANGE, CAP, FLOOR, ReleaseFee::Fixed(CAP + 1), &ReleaseRepay::default()).unwrap_err().contains("past the gateway's cap"));
    }

    #[test]
    fn change_under_dust_is_topped_up_by_the_withdrawals_only_when_the_gateway_holds_no_more() {
        // Just what was burned, and nothing more held: the withdrawals top change up to dust with the fee.
        let alone = plan(&[queued(1, 20_000, 2_000)], &[held(1, 20_000)], ReleaseFee::Fixed(1_000));
        let tx = alone.tx.unwrap();
        assert_eq!((tx.outputs[1].value, alone.fee, alone.cost), (BTC_DUST, 1_000, 1_000 + BTC_DUST));
        assert_eq!(tx.outputs[0].value, 20_000 - 1_000 - BTC_DUST);

        // With more held, it is taken rather than topping up.
        let more = plan(&[queued(1, 20_000, 2_000)], &[held(1, 20_000), held(2, 5_000)], ReleaseFee::Fixed(1_000));
        assert_eq!((more.tx.unwrap().outputs[1].value, more.cost), (5_000, 1_000));
        assert!(plan(&[queued(1, 20_000, 2_000)], &[held(1, 19_999)], ReleaseFee::Fixed(1_000)).tx.is_none(), "less than burned");
    }

    /// A release as the gateway would hold it once proposed: its withdrawals proposed, its inputs
    /// pending.
    fn proposed(plan: &ReleasePlan, withdrawals: &[NamedWithdrawal], outputs: &[(String, SpentOutput)]) -> ReleaseFacts {
        let tx = plan.tx.clone().unwrap();
        let raw = tx.base();
        let inputs = tx
            .inputs
            .iter()
            .map(|input| {
                let key = format!("{}:{}", crate::structs::hex::to_hex(&input.txid).trim_start_matches("0x"), input.vout);
                let (_, held) = outputs.iter().find(|(outpoint, _)| *outpoint == key).unwrap().clone();

                (key, SpentOutput { state: "pending".to_string(), ..held })
            })
            .collect();
        let withdrawals = withdrawals.iter().filter(|withdrawal| plan.ids.contains(&withdrawal.id)).map(|withdrawal| NamedWithdrawal { state: "proposed".to_string(), ..withdrawal.clone() }).collect();

        ReleaseFacts { txid: crate::crypto::bitcoin::sha256d(&raw).to_vec(), raw, version: 0, withdrawals, inputs, change_script: FED.to_vec(), fee_cap: CAP, fee_floor: FLOOR, most_rate: 1_000, repay: ReleaseRepay::default() }
    }

    /// The federation's bare multisig, and its change output.
    const FED: [u8; 3] = [0x51, 0x51, 0xae];

    fn fed_change() -> Vec<u8> {
        p2wsh(&FED)
    }

    fn signed_release(most_fee: u64) -> (ReleaseFacts, Vec<(String, SpentOutput)>) {
        let withdrawals = [queued(1, 20_000, most_fee), queued(2, 30_000, most_fee)];
        let outputs = [held(1, 100_000), held(2, 50_000)];
        let plan = plan_btc_release(&withdrawals, &outputs, &fed_change(), CAP, FLOOR, ReleaseFee::Rate(5), &ReleaseRepay::default()).unwrap();

        (proposed(&plan, &withdrawals, &outputs), outputs.to_vec())
    }

    #[test]
    fn a_replacement_pays_the_same_withdrawals_more_evenly_keeps_its_change_and_passes_every_version() {
        let (facts, _) = signed_release(5_000);
        let (fee, vbytes) = btc_fee_and_vbytes(&facts).unwrap();
        let bump = plan_btc_replacement(&facts, fee, 20).unwrap();
        let (old, _) = crate::crypto::bitcoin::tx::Transaction::read(&facts.raw).unwrap();

        // The same inputs and scripts, the same size, the change as it was; the fee at the rate, off
        // the withdrawals evenly.
        assert_eq!(bump.tx.inputs, old.inputs);
        assert_eq!(bump.tx.base().len(), facts.raw.len());
        assert_eq!(bump.tx.outputs.iter().map(|output| &output.script).collect::<Vec<_>>(), old.outputs.iter().map(|output| &output.script).collect::<Vec<_>>());
        assert_eq!(bump.tx.outputs[2].value, old.outputs[2].value);
        assert_eq!((bump.fee, bump.rate), (vbytes * 20, 20));
        assert_eq!(bump.tx.outputs[0].value, 20_000 - btc_fee_share(bump.fee, 2, 0));
        assert_eq!(bump.tx.outputs[1].value, 30_000 - btc_fee_share(bump.fee, 2, 1));
        assert!(bump.held_back.is_empty());

        // The signers take it: it passes every open version by Bitcoin's incremental rate.
        let next = ReleaseFacts { raw: bump.tx.base(), txid: crate::crypto::bitcoin::sha256d(&bump.tx.base()).to_vec(), ..facts.clone() };
        assert_eq!(check_btc_replacement(&facts, &next, fee), Ok(()));
        assert!(check_btc_replacement(&facts, &next, bump.fee).unwrap_err().to_string().contains("not past every version"));

        // A rate under what it pays still pays past it, by the increment.
        assert_eq!(plan_btc_replacement(&facts, fee, 1).unwrap().fee, fee + vbytes * BTC_LEAST_BUMP_RATE);
    }

    #[test]
    fn a_replacement_goes_as_high_as_the_tightest_limit_lets_it_and_says_which_held_it_back() {
        let withdrawals = [queued(1, 20_000, 600), queued(2, 30_000, 5_000)];
        let outputs = [held(1, 100_000)];
        let plan = plan_btc_release(&withdrawals, &outputs, &fed_change(), CAP, FLOOR, ReleaseFee::Rate(2), &ReleaseRepay::default()).unwrap();
        let facts = proposed(&plan, &withdrawals, &outputs);
        let bump = plan_btc_replacement(&facts, plan.fee, 40).unwrap();

        // Split evenly, so the first's limit holds both: no more than twice it, and it says so.
        assert_eq!(bump.held_back, [1]);
        assert!(bump.fee <= 1_200 && bump.fee > plan.fee, "{}", bump.fee);
        assert!(btc_fee_share(bump.fee, 2, 0) <= 600);

        // With no room at all for even the increment, none goes, naming who holds it.
        let tight = ReleaseFacts { withdrawals: facts.withdrawals.iter().map(|withdrawal| NamedWithdrawal { most_fee: btc_fee_share(plan.cost, 2, 0), ..withdrawal.clone() }).collect(), ..facts.clone() };
        assert!(plan_btc_replacement(&tight, plan.fee, 40).unwrap_err().contains("withdrawal 1, 2 agreed to lose too little"));

        // Nor past the cap.
        let capped = ReleaseFacts { fee_cap: plan.fee + 10, ..facts.clone() };
        assert!(plan_btc_replacement(&capped, plan.fee, 40).unwrap_err().contains("past the cap"));
    }

    #[test]
    fn a_replacement_that_changes_what_it_pays_is_refused_by_the_signers() {
        let (facts, _) = signed_release(5_000);
        let (fee, _) = btc_fee_and_vbytes(&facts).unwrap();
        let bump = plan_btc_replacement(&facts, fee, 20).unwrap();
        let refused = |change: &dyn Fn(&mut crate::crypto::bitcoin::tx::Transaction), why: &str| {
            let mut tx = bump.tx.clone();
            change(&mut tx);
            let next = ReleaseFacts { raw: tx.base(), txid: crate::crypto::bitcoin::sha256d(&tx.base()).to_vec(), ..facts.clone() };
            let answer = check_btc_replacement(&facts, &next, fee).unwrap_err().to_string();
            assert!(answer.contains(why), "{why}: {answer}");
        };

        refused(&|tx| tx.outputs.swap(0, 1), "as asked");
        refused(&|tx| (tx.outputs[0].value, tx.outputs[2].value) = (tx.outputs[0].value + 100, tx.outputs[2].value - 100), "change is not all");
        refused(&|tx| tx.inputs[0].vout = 7, "does not spend what it replaces");
        refused(&|tx| tx.inputs[0].sequence = 0xffff_ffff, "delay");
    }

    #[test]
    fn a_child_spends_the_change_pays_for_both_at_the_rate_and_is_charged_within_the_limits() {
        let (facts, outputs) = signed_release(5_000);
        let (fee, vbytes) = btc_fee_and_vbytes(&facts).unwrap();
        let child = plan_btc_child(&facts, &outputs, &fed_change(), 20).unwrap();
        let (change, spent) = btc_change_of(&facts).unwrap();

        // It spends the change alone, which pays it, and the two together come to the rate.
        assert_eq!(child.tx.inputs.len(), 1);
        assert_eq!(format!("{}:{}", crate::structs::hex::to_hex(&child.tx.inputs[0].txid).trim_start_matches("0x"), child.tx.inputs[0].vout), change);
        assert_eq!(child.tx.outputs[0].value, spent.sats - child.fee);
        let child_vbytes = btc_release_vbytes(child.tx.base().len(), &[(spent.threshold, spent.script_len)]);
        assert_eq!(child.fee, 20 * (vbytes + child_vbytes) - fee);
        assert_eq!(child.rate, 20);

        // The signers take it, and refuse it past the rate they sign for, with what it pays for.
        let facts_of = |tx: &crate::crypto::bitcoin::tx::Transaction, most_rate: u64| ReleaseFacts {
            raw: tx.base(),
            txid: crate::crypto::bitcoin::sha256d(&tx.base()).to_vec(),
            version: 0,
            withdrawals: Vec::new(),
            inputs: vec![(change.clone(), SpentOutput { state: "pending".to_string(), ..spent.clone() })],
            change_script: FED.to_vec(),
            fee_cap: CAP,
            fee_floor: FLOOR,
            most_rate,
            repay: ReleaseRepay::default(),
        };
        assert_eq!(check_btc_child(&facts, &facts_of(&child.tx, 20), None), Ok(()));
        assert!(matches!(check_btc_child(&facts, &facts_of(&child.tx, 19), None), Err(ReleaseRefusal::Overpriced(_))));

        // Its replacement pays past it by the increment, from the same change.
        let again = plan_btc_child_replacement(&facts, &facts_of(&child.tx, 1_000), child.fee, 30).unwrap();
        assert_eq!(again.tx.inputs, child.tx.inputs);
        assert!(again.fee >= child.fee + child_vbytes && again.rate > child.rate, "{again:?}");
        assert!(check_btc_child(&facts, &facts_of(&again.tx, 1_000), Some(child.fee)).is_ok());
        assert!(check_btc_child(&facts, &facts_of(&child.tx, 1_000), Some(child.fee)).unwrap_err().to_string().contains("not past every version"));

        // Within what each withdrawal agreed to lose, with its share of the release's own.
        let (tight, outputs) = signed_release(btc_fee_share(fee, 2, 0) + 200);
        let held = plan_btc_child(&tight, &outputs, &fed_change(), 200).unwrap();
        assert_eq!(held.held_back, [1, 2]);
        assert!(held.fee <= 400 && held.fee >= 230, "{}", held.fee);
    }

    #[test]
    fn a_child_spends_more_than_the_change_only_when_the_change_cannot_pay_it() {
        let withdrawals = [queued(1, 20_000, 10_000)];
        let outputs = [held(1, 20_000), held(2, 50_000)];
        // Just what was burned, held alone: the change is dust, too little to pay a child.
        let plan = plan_btc_release(&withdrawals, &outputs[..1], &fed_change(), CAP, FLOOR, ReleaseFee::Rate(2), &ReleaseRepay::default()).unwrap();
        let facts = proposed(&plan, &withdrawals, &outputs);
        let child = plan_btc_child(&facts, &outputs[1..], &fed_change(), 10).unwrap();

        assert_eq!(child.tx.inputs.len(), 2, "the change, and what pays the rest");
        assert_eq!(child.tx.outputs[0].value, BTC_DUST + 50_000 - child.fee);
        assert!(plan_btc_child(&facts, &[], &fed_change(), 10).unwrap_err().contains("too little"));
    }

    #[test]
    fn what_a_release_repays_is_a_share_of_its_fee_capped_per_withdrawal_within_each_limit() {
        // A fifth of a fee of 3,000 is 600, under the 1,000 owed: 300 each, but for the one with room for 50.
        assert_eq!(btc_repays(1_000, 3_000, 20, 500, &[2_000, 50]), (600, vec![300, 50]));
        // Never more than is owed, nor than one withdrawal repays at most, and none from one at its limit.
        assert_eq!(btc_repays(100, 3_000, 20, 500, &[2_000, 2_000]), (100, vec![50, 50]));
        assert_eq!(btc_repays(5_000, 10_000, 20, 500, &[2_000, 2_000]), (2_000, vec![500, 500]));
        assert_eq!(btc_repays(1_000, 3_000, 20, 500, &[0]), (600, vec![0]));
        assert_eq!(btc_repays(0, 3_000, 20, 500, &[2_000]), (0, vec![0]), "nothing owed");
        assert_eq!(btc_repays(1_000, 3_000, 0, 500, &[2_000]), (0, vec![0]), "a policy that never repays");
    }

    #[test]
    fn a_release_repays_what_is_owed_to_the_federation_s_change_and_a_signer_refuses_a_wrong_repayment() {
        let withdrawals = [queued(1, 20_000, 5_000), queued(2, 30_000, 5_000)];
        let outputs = [held(1, 100_000)];
        let repay = ReleaseRepay { owed: 10_000, pct: 20, most: 500, ..ReleaseRepay::default() };
        let plan = plan_btc_release(&withdrawals, &outputs, &fed_change(), CAP, FLOOR, ReleaseFee::Fixed(2_000), &repay).unwrap();
        let tx = plan.tx.clone().unwrap();

        // A fifth of the fee, 400, 200 each, off their outputs and onto the change.
        assert_eq!((plan.repay.budget, plan.repay.shares.clone()), (400, vec![200, 200]));
        assert_eq!(tx.outputs[0].value, 20_000 - 1_000 - 200);
        assert_eq!(tx.outputs[2].value, 100_000 - 50_000 + 400);

        // The signers take it as the gateway records it, with what is owed now.
        let facts = ReleaseFacts { repay: ReleaseRepay { owed: 10_000, ..plan.repay.clone() }, ..proposed(&plan, &withdrawals, &outputs) };
        assert_eq!(check_btc_release(&facts), Ok(()));

        let refused = |change: &dyn Fn(&mut ReleaseFacts), why: &str| {
            let mut changed = facts.clone();
            change(&mut changed);
            let answer = check_btc_release(&changed).unwrap_err().to_string();
            assert!(answer.contains(why), "{why}: {answer}");
        };
        refused(&|facts| facts.repay.owed = 399, "past what children cost");
        refused(&|facts| facts.repay.budget = 401, "percent of its fee");
        refused(&|facts| facts.repay.most = 199, "repays at most");
        refused(&|facts| facts.repay.shares = vec![300, 100], "past its share of the budget");
        refused(&|facts| facts.repay.shares = vec![200, 100], "with what they repay");
        refused(&|facts| facts.repay.shares = vec![200, 200, 1], "account of what it repays");

        // Nothing owed, nothing repaid: the release is as it was.
        let plain = plan_btc_release(&withdrawals, &outputs, &fed_change(), CAP, FLOOR, ReleaseFee::Fixed(2_000), &ReleaseRepay { owed: 0, ..repay.clone() }).unwrap();
        assert_eq!(plain.tx.unwrap().outputs[2].value, 50_000);

        // A replacement keeps what it repays, the bump off the withdrawals alone, and a child's room counts it.
        let (fee, _) = btc_fee_and_vbytes(&facts).unwrap();
        let bump = plan_btc_replacement(&facts, fee, 20).unwrap();
        assert_eq!(bump.tx.outputs[2].value, tx.outputs[2].value);
        assert_eq!(bump.tx.outputs[0].value, 20_000 - btc_fee_share(bump.fee, 2, 0) - 200);
        let next = ReleaseFacts { raw: bump.tx.base(), txid: crate::crypto::bitcoin::sha256d(&bump.tx.base()).to_vec(), ..facts.clone() };
        assert_eq!(check_btc_replacement(&facts, &next, fee), Ok(()));
        let dropping = ReleaseFacts { repay: ReleaseRepay { shares: vec![200, 100], ..facts.repay.clone() }, ..next.clone() };
        assert!(check_btc_replacement(&facts, &dropping, fee).is_err());
    }

    #[test]
    fn a_release_is_stuck_once_old_enough_and_well_under_what_the_network_asks() {
        let hour = 3_600_000;

        assert!(btc_is_stuck(Some(hour), 7, Some(10), hour, 80), "under four fifths of it, an hour on");
        assert!(!btc_is_stuck(Some(hour), 8, Some(10), hour, 80), "at four fifths it waits");
        assert!(!btc_is_stuck(Some(hour - 1), 1, Some(10), hour, 80), "too soon");
        assert!(!btc_is_stuck(None, 1, Some(10), hour, 80), "an age unread says nothing");
        assert!(!btc_is_stuck(Some(hour), 1, None, hour, 80), "nor does a network that asks nothing");
        assert!(btc_is_stuck(Some(hour), 99, Some(100), hour, 100), "a hundred percent bumps whatever is under it");

        // Replace-by-fee first; a child once that was refused or tried; and a child is replaced.
        assert_eq!(btc_bump_kind(false, false, false), BumpKind::Replace);
        assert_eq!(btc_bump_kind(false, true, false), BumpKind::Child);
        assert_eq!(btc_bump_kind(false, false, true), BumpKind::Child);
        assert_eq!(btc_bump_kind(true, false, false), BumpKind::ReplaceChild);
    }

    #[test]
    fn signers_take_turns_a_block_apart_in_their_order() {
        assert_eq!([0, 1, 2].map(|place| btc_turn_delay(place, 5_000)), [0, 5_000, 10_000]);
        assert_eq!(btc_turn_delay(3, 50), 3 * BTC_LEAST_TURN_MS, "a chain faster than a transaction reaches its mempool");
        assert_eq!(btc_block_ms((110, 1_050_000), (100, 1_000_000)), 5_000);
        assert_eq!(btc_block_ms((100, 1_000_000), (100, 1_000_000)), 5_000, "one block says nothing");
        assert_eq!(btc_block_ms((101, 1_000_000), (100, 1_000_000)), 1, "at least a millisecond");
        assert_eq!(btc_block_ms((101, u64::MAX), (100, 0)), 600_000, "at most ten minutes");
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

#[cfg(feature = "bitcoin")]
#[test]
fn a_withdrawal_pays_only_an_address_on_the_bitcoin_the_relay_follows() {
    let program = [0xb0; 20];
    let script = [&[0x00, 20][..], &program].concat();

    for (network, hrp) in [(BtcNetwork::Main, "bc"), (BtcNetwork::Test, "tb"), (BtcNetwork::Regtest, "bcrt")] {
        assert_eq!(BtcNetwork::from_name(network.name()), Some(network));
        assert_eq!(network.hrp(), hrp);

        // Its own address reads back as the script, written as it writes them.
        let own = network.address(&script);
        assert!(own.starts_with(&format!("{hrp}1q")), "{own}");
        assert_eq!(network.output_script(&own), Ok(script.clone()));
        assert_eq!(network.output_script(&format!("  {}  ", own.to_uppercase())), Ok(script.clone()));

        // Every other network's is refused, saying whose it is and whose it takes.
        for other in [BtcNetwork::Main, BtcNetwork::Test, BtcNetwork::Regtest].into_iter().filter(|other| *other != network) {
            let why = network.output_script(&other.address(&script)).unwrap_err();
            assert!(why.contains(&format!("Bitcoin's {} network", other.name())) && why.contains(&format!("{hrp}1…")), "{why}");
        }
    }

    // A script given as hex is taken as it is, on any network; a prefix no Bitcoin has is refused.
    assert_eq!(BtcNetwork::Regtest.output_script(&crate::structs::hex::to_hex(&script)), Ok(script));
    assert!(BtcNetwork::Test.output_script("0xzz").is_err());
    let litecoin = crate::structs::bech32::encode_segwit("ltc", &program);
    assert!(BtcNetwork::Main.output_script(&litecoin).unwrap_err().contains("no Bitcoin network's"));
    assert_eq!(BtcNetwork::from_name("signet"), None);
}
