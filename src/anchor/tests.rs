//! What every build that takes anchors shares: the list's root over entries of either chain,
//! Ethereum anchors refused until they are counted, and histories compared by the chains holding
//! them, Bitcoin first and Ethereum filling the gaps. Bitcoin's own proofs and headers are tested in
//! bitcoin/tests.rs.

use {
    super::{
        sync::{AnchorVerdict, History, Sources, check_anchors, compare_histories},
        *,
    },
    crate::{
        crypto::hasher::Hasher,
        db::MemoryDb,
        params::anchoring::{AnchorSource, AnchorsAnswer},
        structs::{
            hash::HashType,
            tx::{BtcAnchorProof, EthAnchorProof},
        },
    },
    std::sync::Arc,
};

fn id(seed: &[u8]) -> Hash {
    Hasher::digest(HashType::default_chain(), seed)
}

fn bitcoin(height: u64) -> AnchorSource {
    AnchorSource::Bitcoin { block: [height as u8; 32], height }
}

fn ethereum(slot: u64) -> AnchorSource {
    AnchorSource::Ethereum { root: [slot as u8; 32], slot }
}

fn entry(epoch: u64, seed: &[u8], source: AnchorSource) -> AnchorEntry {
    AnchorEntry { epoch, block: id(seed), source, enforced: true }
}

/// A history whose entries at the places given are proven where they say.
fn history(entries: &[AnchorEntry], proven: &[u64]) -> History {
    History { entries: entries.to_vec(), proven: proven.iter().map(|index| (*index, entries[*index as usize].source)).collect() }
}

fn an_ethereum_proof() -> EthAnchorProof {
    EthAnchorProof { header: vec![0; 244], root: [1; 32], tx: vec![2; 120], index: 3, branch: vec![[4; 32]; 21], ancestry: None }
}

#[test]
fn the_list_root_folds_entries_of_either_chain() {
    let (first, second) = (entry(1, b"one", bitcoin(10)), entry(2, b"two", ethereum(500)));
    let in_bitcoin = AnchorEntry { source: AnchorSource::Bitcoin { block: [244; 32], height: 500 }, ..second.clone() };

    assert_eq!(list_root(&[first.clone(), second.clone()]), next_root(&next_root(&[0; 32], &first), &second));
    assert_ne!(next_root(&[0; 32], &second), next_root(&[0; 32], &in_bitcoin), "the chain holding it counts");
    assert_eq!(second.source.chain(), crate::params::anchoring::AnchorChain::Ethereum);
    assert_eq!((second.source.position(), second.source.describe()), (500, "Ethereum slot 500".to_string()));
    assert_eq!(first.source.describe(), "Bitcoin block 10");
}

#[test]
fn proofs_and_entries_encode_with_their_chain_s_tag() {
    let ethereum = AnchorProof::Ethereum(an_ethereum_proof());
    let bitcoin = AnchorProof::Bitcoin(BtcAnchorProof { block: [0; 32], raw: Vec::new(), index: 0, path: Vec::new(), coinbase: Vec::new(), coinbase_path: Vec::new() });

    assert_eq!(wincode::serialize(&bitcoin).unwrap()[0], 0);
    assert_eq!(wincode::serialize(&ethereum).unwrap()[0], 1);
    assert_eq!(wincode::deserialize::<AnchorProof>(&wincode::serialize(&ethereum).unwrap()).unwrap(), ethereum);
    assert_eq!(wincode::serialize(&AnchorSource::Ethereum { root: [0; 32], slot: 0 }).unwrap()[0], 1);
    assert!(ethereum.within_limits());

    let too_long = AnchorProof::Ethereum(EthAnchorProof { ancestry: Some(vec![[0; 32]; crate::structs::tx::MAX_ANCHOR_PATH + 1]), ..an_ethereum_proof() });
    assert!(!too_long.within_limits());
}

#[test]
fn an_ethereum_anchor_is_not_counted_yet() {
    let state = State::new(Arc::new(MemoryDb::default()));
    let settings = AnchorSettings { ethereum: true, enforce: true, ..AnchorSettings::OFF };
    let rules = ProveRules { settings: &settings, genesis: id(b"genesis"), checkpoint_interval: Some(8), pooled: false };

    match prove(&state, &rules, &Address::Ed25519([3; 32]), &AnchorProof::Ethereum(an_ethereum_proof())) {
        Err(ProveError::Refused(why)) => assert_eq!(why, "Ethereum anchors are not counted yet"),
        other => panic!("{other:?}"),
    }

    assert_eq!(Anchors::new(&state).count().unwrap(), 0, "nothing recorded");

    let answer = AnchorsAnswer { start: None, btc_tip: 0, eth_start: None, entries: vec![entry(1, b"one", ethereum(100))], proofs: vec![(0, AnchorProof::Ethereum(an_ethereum_proof()))] };
    assert!(check_anchors(&answer, &Sources::default(), &id(b"genesis"), 6).unwrap_err().contains("not counted yet"));

    // A proof in another chain than its entry says.
    let answer = AnchorsAnswer { entries: vec![entry(1, b"one", bitcoin(100))], ..answer };
    assert!(check_anchors(&answer, &Sources::default(), &id(b"genesis"), 6).unwrap_err().contains("another chain"));
}

#[test]
fn the_same_history_is_one_extending_the_other_whatever_chains_hold_it() {
    let entries = [entry(1, b"one", bitcoin(10)), entry(2, b"two", ethereum(500)), entry(3, b"three", bitcoin(12))];
    let (short, long) = (history(&entries[..2], &[1]), history(&entries, &[2]));

    assert_eq!(compare_histories(&short, &long), AnchorVerdict::Extends(true));
    assert_eq!(compare_histories(&long, &short), AnchorVerdict::Extends(false));

    // The same checkpoint shown by another chain's anchor is the same history.
    let elsewhere = [entries[0].clone(), AnchorEntry { source: bitcoin(11), ..entries[1].clone() }];
    assert_eq!(compare_histories(&history(&elsewhere, &[1]), &long), AnchorVerdict::Extends(true));
}

#[test]
fn neither_chain_decides_where_no_chain_holds_an_anchor_of_both_after_they_part() {
    let shared = entry(1, b"shared", bitcoin(10));
    let a = history(&[shared.clone(), entry(2, b"a", bitcoin(11))], &[1]);
    let b = history(&[shared, entry(2, b"b", ethereum(90))], &[1]);

    assert_eq!(compare_histories(&a, &b), AnchorVerdict::Undecided(1));
}

#[cfg(feature = "anchoring-btc")]
#[test]
fn bitcoin_decides_where_both_have_bitcoin_anchors_after_they_part() {
    let shared = entry(1, b"shared", bitcoin(10));
    // a anchored its fork in Ethereum later, then in Bitcoin earlier; b the other way about.
    let a = [shared.clone(), entry(2, b"a 2", ethereum(100)), entry(3, b"a 3", bitcoin(11))];
    let b = [shared, entry(2, b"b 2", bitcoin(12)), entry(3, b"b 3", ethereum(50))];

    assert_eq!(compare_histories(&history(&a, &[2]), &history(&b, &[1])), AnchorVerdict::Earlier(false), "a's Bitcoin anchor is lower, whatever Ethereum says");
    assert_eq!(compare_histories(&history(&b, &[1]), &history(&a, &[2])), AnchorVerdict::Earlier(true));

    // Each side's first Bitcoin entry after the parting place, at places of its own.
    assert_eq!(compare_histories(&history(&a, &[1]), &history(&b, &[2])), AnchorVerdict::NeedProofs(2, 1));
    assert_eq!(compare_histories(&history(&a, &[2]), &history(&b, &[2])), AnchorVerdict::NeedProofs(2, 1), "one side proven is not enough");

    // Two in the same Bitcoin block: a tie.
    let c = [a[0].clone(), entry(2, b"c 2", bitcoin(11))];
    assert_eq!(compare_histories(&history(&a, &[2]), &history(&c, &[1])), AnchorVerdict::Tie(1));
}

#[cfg(feature = "anchoring-eth")]
#[test]
fn ethereum_fills_the_gap_where_one_has_no_bitcoin_anchor_after_they_part() {
    let shared = entry(1, b"shared", bitcoin(10));
    let a = [shared.clone(), entry(2, b"a 2", ethereum(100)), entry(3, b"a 3", ethereum(120))];
    let b = [shared, entry(2, b"b 2", bitcoin(12)), entry(3, b"b 3", ethereum(90))];

    assert_eq!(compare_histories(&history(&a, &[1]), &history(&b, &[2])), AnchorVerdict::Earlier(true), "b's Ethereum anchor is in an earlier slot");
    assert_eq!(compare_histories(&history(&a, &[2]), &history(&b, &[2])), AnchorVerdict::NeedProofs(1, 2));

    // Two in the same slot: a tie.
    let c = [a[0].clone(), entry(2, b"c 2", ethereum(100))];
    assert_eq!(compare_histories(&history(&a, &[1]), &history(&c, &[1])), AnchorVerdict::Tie(1));
}

#[cfg(all(feature = "anchoring-btc", not(feature = "anchoring-eth")))]
#[test]
fn a_build_reading_only_bitcoin_goes_by_bitcoin_alone() {
    let shared = entry(1, b"shared", bitcoin(10));
    let a = [shared.clone(), entry(2, b"a 2", ethereum(100))];
    let b = [shared, entry(2, b"b 2", ethereum(90))];

    assert_eq!(compare_histories(&history(&a, &[1]), &history(&b, &[1])), AnchorVerdict::Undecided(1));
}

/// Both histories hold Bitcoin and Ethereum anchors after they part, which disagree: a is lower in
/// Bitcoin, b earlier in Ethereum. Bitcoin decides when the build reads it; Ethereum alone when not.
#[test]
fn mixed_histories_go_by_bitcoin_first_and_by_ethereum_in_a_build_without_it() {
    let shared = entry(1, b"shared", ethereum(10));
    let a = [shared.clone(), entry(2, b"a 2", bitcoin(11)), entry(3, b"a 3", ethereum(100))];
    let b = [shared, entry(2, b"b 2", bitcoin(12)), entry(3, b"b 3", ethereum(90))];
    let verdict = compare_histories(&history(&a, &[1, 2]), &history(&b, &[1, 2]));

    #[cfg(feature = "anchoring-btc")]
    assert_eq!(verdict, AnchorVerdict::Earlier(false), "Bitcoin decides: a");
    #[cfg(all(feature = "anchoring-eth", not(feature = "anchoring-btc")))]
    assert_eq!(verdict, AnchorVerdict::Earlier(true), "Ethereum alone decides: b");
    #[cfg(not(any(feature = "anchoring-btc", feature = "anchoring-eth")))]
    assert_eq!(verdict, AnchorVerdict::Undecided(1));
}

#[cfg(not(feature = "anchoring-btc"))]
#[test]
fn a_build_reading_no_bitcoin_takes_no_bitcoin_anchor() {
    let state = State::new(Arc::new(MemoryDb::default()));
    let settings = AnchorSettings { bitcoin: true, ..AnchorSettings::OFF };
    let rules = ProveRules { settings: &settings, genesis: id(b"genesis"), checkpoint_interval: Some(8), pooled: false };
    let proof = AnchorProof::Bitcoin(BtcAnchorProof { block: [0; 32], raw: Vec::new(), index: 0, path: Vec::new(), coinbase: Vec::new(), coinbase_path: Vec::new() });

    assert!(matches!(prove(&state, &rules, &Address::Ed25519([3; 32]), &proof), Err(ProveError::Refused(why)) if why.contains("takes no Bitcoin anchors")));
}
