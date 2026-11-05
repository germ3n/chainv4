//! Ethereum anchors as anchored sync checks them: sync committees followed from a trusted root
//! through updates, each checked, and a kept finality update's signature checked with the
//! committee of its period, on a synthetic beacon chain whose committees are made here. The chain's
//! own check against the relay's records is in vm/tests/eth_relay.rs, where the relay runs.

use {
    super::*,
    crate::{
        anchor::{
            AnchorEntry, AnchorsAnswer, chain_tag, list_root,
            sync::{AnchorVerdict, History, Sources, check_anchors, compare_histories},
        },
        crypto::hasher::Hasher,
        params::anchoring::EthNetwork,
        structs::{hash::{Hash, HashType}, tx::AnchorProof},
    },
    ethlight::{
        MAINNET,
        testing::{self, BlockRoots, Committee, State as BeaconState},
    },
};

const C: Native = Native;
/// A mainnet period well into Fulu.
const PERIOD: u64 = 1610;

fn slot(period: u64, plus: u64) -> u64 {
    period * MAINNET.slots_per_period() + plus
}

/// The time a minute after a slot, in seconds.
fn time_at(slot: u64) -> u64 {
    MAINNET.genesis_time + 12 * slot + 60
}

fn id(seed: &[u8]) -> Hash {
    Hasher::digest(HashType::default_chain(), seed)
}

/// A synthetic beacon chain followed from a bootstrap: its start, the bootstrap, two periods of
/// updates, and the committees that signed them.
struct Beacon {
    start: EthStart,
    bootstrap: Vec<u8>,
    updates: Vec<Vec<u8>>,
    committees: [Committee; 3],
}

fn header_at(at: u64, transactions: &[Vec<u8>]) -> ethlight::LightClientHeader {
    testing::header(&C, &MAINNET, at, testing::filler(0x77, at, 0), testing::execution(&C, at, transactions))
}

fn beacon() -> Beacon {
    let committees = [Committee::new(1), Committee::new(2), Committee::new(3)];
    let (bootstrap, root) = testing::bootstrap(&C, &MAINNET, slot(PERIOD, 64), &committees[0].committee);
    let learn = testing::update(&C, &MAINNET, &committees[0], 512, slot(PERIOD, 8000), slot(PERIOD, 8001), &header_at(slot(PERIOD, 7936), &[]), Some(&committees[1].committee));
    let cross = testing::update(&C, &MAINNET, &committees[1], 512, slot(PERIOD + 1, 100), slot(PERIOD + 1, 101), &header_at(slot(PERIOD + 1, 32), &[]), Some(&committees[2].committee));
    // One signed by a committee of no period: passed over, and nothing stops.
    let forged = testing::update(&C, &MAINNET, &Committee::new(9), 512, slot(PERIOD + 1, 200), slot(PERIOD + 1, 201), &header_at(slot(PERIOD + 1, 160), &[]), None);

    Beacon {
        start: EthStart { root, network: EthNetwork::Preset("mainnet".into()) },
        bootstrap: bootstrap.encode(&MAINNET),
        updates: vec![learn.encode(&MAINNET), forged.encode(&MAINNET), cross.encode(&MAINNET)],
        committees,
    }
}

#[test]
fn committees_are_followed_from_the_trusted_root_and_forged_updates_passed_over() {
    let beacon = beacon();
    let followed = EthCommittees::follow(&beacon.start, &beacon.bootstrap, &beacon.updates, time_at(slot(PERIOD + 1, 300))).unwrap();

    assert_eq!(followed.by_period.keys().copied().collect::<Vec<_>>(), [PERIOD, PERIOD + 1, PERIOD + 2]);
    assert_eq!(followed.by_period[&(PERIOD + 1)], beacon.committees[1].committee);
    assert_eq!(followed.by_period[&(PERIOD + 2)], beacon.committees[2].committee);
    assert_eq!(followed.last_period(), PERIOD + 2);

    // A start whose bootstrap is not at its root is refused whole.
    let elsewhere = EthStart { root: [1; 32], ..beacon.start.clone() };
    assert!(EthCommittees::follow(&elsewhere, &beacon.bootstrap, &beacon.updates, time_at(slot(PERIOD + 1, 300))).unwrap_err().contains("trusted block root"));
}

/// An Ethereum anchor of a checkpoint, its proof as the chain keeps it: the anchor's block an
/// ancestor of a checkpoint finalized in a period after the start, signed by that period's
/// committee.
fn anchored(beacon: &Beacon, entries: &[AnchorEntry], epoch: u64, block: &Hash, plus: u64) -> (AnchorEntry, AnchorProof) {
    let payload = crate::anchor::Payload { chain: chain_tag(&id(b"genesis")), epoch, block: block.to_bytes().try_into().unwrap(), root: list_root(entries) };
    let anchor = testing::eip1559(1, epoch, &[0xa1; 20], &payload.encode());
    let transactions = vec![testing::eip1559(1, 1, &[2; 20], b"swap"), anchor.clone()];
    let older = header_at(slot(PERIOD + 1, plus), &transactions);
    let at = slot(PERIOD + 1, plus + 4096);
    let mut state = BeaconState::new(MAINNET.fork_at_slot(at).fork, at);
    let mut roots = BlockRoots::new(at);
    roots.set(older.beacon.slot, older.beacon.hash_tree_root(&C));
    state.fields[testing::BLOCK_ROOTS as usize] = roots.root(&C);
    let checkpoint = testing::header(&C, &MAINNET, at, state.root(&C), testing::execution(&C, at, &[] as &[&[u8]]));
    let update = testing::update(&C, &MAINNET, &beacon.committees[1], 400, at + 40, at + 41, &checkpoint, None);
    let proof = EthAnchorProof {
        header: older.encode(MAINNET.fork_at_slot(older.beacon.slot).fork),
        root: checkpoint.beacon.hash_tree_root(&C),
        tx: anchor,
        index: 1,
        branch: tx::transaction_branch(&C, &transactions, 1),
        ancestry: Some(roots.branch(&C, &state, older.beacon.slot)),
        update: update.encode(&MAINNET),
    };
    let entry = AnchorEntry { epoch, block: *block, source: AnchorSource::Ethereum { root: older.beacon.hash_tree_root(&C), slot: older.beacon.slot }, enforced: true };

    (entry, AnchorProof::Ethereum(proof))
}

#[test]
fn a_kept_finality_update_is_checked_with_its_period_s_committee() {
    let beacon = beacon();
    let followed = [EthCommittees::follow(&beacon.start, &beacon.bootstrap, &beacon.updates, time_at(slot(PERIOD + 1, 8000))).unwrap()];
    let sources = Sources { ethereum: &followed, ..Sources::default() };
    let (entry, proof) = anchored(&beacon, &[], 1, &id(b"one"), 100);
    let answer = AnchorsAnswer { start: None, btc_tip: 0, eth_start: Some(beacon.start.clone()), entries: vec![entry.clone()], proofs: vec![(0, proof.clone())] };

    let history = check_anchors(&answer, &sources, &id(b"genesis"), 6).unwrap();
    assert_eq!(history.proven[&0], entry.source);
    assert_eq!(history.readable, [crate::params::anchoring::AnchorChain::Ethereum]);
    assert_eq!(crate::anchor::sync::newest_readable(&answer, &sources), Some(0));
    assert_eq!(crate::anchor::sync::newest_readable(&answer, &Sources::default()), None, "without --beacon, no Ethereum entry is read");

    // Without --beacon there is nothing to check it by: it proves nothing.
    assert!(check_anchors(&answer, &Sources::default(), &id(b"genesis"), 6).unwrap_err().contains("--beacon"));

    // Committees learned only up to the start's period cannot check a later signature.
    let early = [EthCommittees::follow(&beacon.start, &beacon.bootstrap, &[], time_at(slot(PERIOD + 1, 8000))).unwrap()];
    assert!(check_anchors(&answer, &Sources { ethereum: &early, ..Sources::default() }, &id(b"genesis"), 6).unwrap_err().contains("not learned"));

    // A forged signature, an entry naming another block, and another chain's genesis are refused.
    let AnchorProof::Ethereum(inner) = &proof else { unreachable!() };
    let mut update = ethlight::LightClientUpdate::decode(&inner.update, &MAINNET).unwrap();
    let version = MAINNET.fork_at_slot(update.signature_slot - 1).version;
    testing::resign(&C, &Committee::new(9), 512, &mut update, version, &MAINNET.genesis_validators_root);
    let forged = AnchorProof::Ethereum(EthAnchorProof { update: update.encode(&MAINNET), ..inner.clone() });
    assert!(check_anchors(&AnchorsAnswer { proofs: vec![(0, forged)], ..answer.clone() }, &sources, &id(b"genesis"), 6).unwrap_err().contains("signature"));
    let moved = AnchorEntry { source: AnchorSource::Ethereum { root: [3; 32], slot: 5 }, ..entry.clone() };
    assert!(check_anchors(&AnchorsAnswer { entries: vec![moved], ..answer.clone() }, &sources, &id(b"genesis"), 6).unwrap_err().contains("another Ethereum block"));
    assert!(check_anchors(&answer, &sources, &id(b"another"), 6).unwrap_err().contains("not of its checkpoint"));

    // A peer naming another start is not checked by these committees.
    let other_start = AnchorsAnswer { eth_start: Some(EthStart { root: [9; 32], ..beacon.start.clone() }), ..answer };
    assert!(check_anchors(&other_start, &sources, &id(b"genesis"), 6).unwrap_err().contains("--beacon"));
}

#[test]
fn checked_ethereum_anchors_decide_where_no_bitcoin_anchor_does_and_bitcoin_first_where_one_does() {
    let beacon = beacon();
    let followed = [EthCommittees::follow(&beacon.start, &beacon.bootstrap, &beacon.updates, time_at(slot(PERIOD + 1, 8000))).unwrap()];
    let sources = Sources { ethereum: &followed, ..Sources::default() };
    let shared = AnchorEntry { epoch: 1, block: id(b"shared"), source: AnchorSource::Bitcoin { block: [1; 32], height: 10 }, enforced: true };

    // Two histories part after a shared entry, each anchoring its own checkpoint in Ethereum: the
    // one in the earlier slot was first.
    let (a, a_proof) = anchored(&beacon, std::slice::from_ref(&shared), 2, &id(b"a"), 100);
    let (b, b_proof) = anchored(&beacon, std::slice::from_ref(&shared), 2, &id(b"b"), 300);
    let checked = |entry: &AnchorEntry, proof: &AnchorProof| {
        let answer = AnchorsAnswer { start: None, btc_tip: 0, eth_start: Some(beacon.start.clone()), entries: vec![shared.clone(), entry.clone()], proofs: vec![(1, proof.clone())] };

        check_anchors(&answer, &sources, &id(b"genesis"), 6).unwrap()
    };
    let (first, second) = (checked(&a, &a_proof), checked(&b, &b_proof));

    assert_eq!(compare_histories(&first, &second), AnchorVerdict::Earlier(false), "a's slot is earlier");
    assert_eq!(compare_histories(&second, &first), AnchorVerdict::Earlier(true));

    // Each then also anchors a later checkpoint in Bitcoin, b lower: Bitcoin decides, whatever
    // Ethereum says, in a build that reads it.
    let with_bitcoin = |history: &History, height: u64, seed: &[u8]| {
        let mut history = history.clone();
        let entry = AnchorEntry { epoch: 3, block: id(seed), source: AnchorSource::Bitcoin { block: [height as u8; 32], height }, enforced: true };
        history.proven.insert(2, entry.source);
        history.entries.push(entry);
        // As a peer showing Bitcoin's headers makes them read.
        history.readable.push(crate::params::anchoring::AnchorChain::Bitcoin);

        history
    };
    let verdict = compare_histories(&with_bitcoin(&first, 20, b"a 3"), &with_bitcoin(&second, 15, b"b 3"));

    #[cfg(feature = "anchoring-btc")]
    assert_eq!(verdict, AnchorVerdict::Earlier(true), "b is lower in Bitcoin");
    #[cfg(not(feature = "anchoring-btc"))]
    assert_eq!(verdict, AnchorVerdict::Earlier(false), "Ethereum alone: a");
}

/// A beacon API serving a synthetic chain, as the node reads one, its requests kept.
struct FakeBeacon {
    beacon: Beacon,
    asked: std::sync::Mutex<Vec<(u64, u64)>>,
}

impl EthBeacon for FakeBeacon {
    fn bootstrap(&self, root: &[u8; 32]) -> Result<Vec<u8>, String> {
        (*root == self.beacon.start.root).then(|| self.beacon.bootstrap.clone()).ok_or_else(|| "no such bootstrap".to_string())
    }

    fn updates(&self, start_period: u64, count: u64) -> Result<Vec<Vec<u8>>, String> {
        self.asked.lock().unwrap().push((start_period, count));
        let periods = start_period..start_period + count;

        Ok(self.beacon.updates.iter().filter(|bytes| ethlight::LightClientUpdate::decode(bytes, &MAINNET).is_ok_and(|update| periods.contains(&MAINNET.period_at_slot(update.attested_header.beacon.slot)))).cloned().collect())
    }
}

#[test]
fn committees_are_fetched_from_the_start_s_period_to_now() {
    let fake = FakeBeacon { beacon: beacon(), asked: Default::default() };
    let followed = EthCommittees::fetch(&fake, &fake.beacon.start, time_at(slot(PERIOD + 1, 300))).unwrap();

    assert_eq!(followed.last_period(), PERIOD + 2);
    assert_eq!(*fake.asked.lock().unwrap(), [(PERIOD, 2)], "from the bootstrap's period to the one now, in one request");
    assert!(EthCommittees::fetch(&fake, &EthStart { root: [3; 32], ..fake.beacon.start.clone() }, time_at(slot(PERIOD + 1, 300))).unwrap_err().contains("no such bootstrap"));
}

#[cfg(feature = "json")]
#[test]
fn a_beacon_network_is_a_preset_or_a_whole_definition_governance_carries() {
    use crate::params::{anchoring::AnchorSettings, json::eth_network_from_json};
    use serde_json::json;

    assert_eq!(eth_network_from_json(&json!("main")).unwrap(), EthNetwork::Preset("mainnet".into()));
    assert_eq!(eth_network_from_json(&json!("sepolia")).unwrap().definition().unwrap(), ethlight::SEPOLIA);
    assert!(eth_network_from_json(&json!("goerli")).unwrap_err().contains("not goerli"));

    // A definition overriding a preset's fork table, written out, is carried as its SSZ.
    let written = json!({
        "name": "sepolia",
        "genesis_validators_root": crate::structs::hex::to_hex(&ethlight::SEPOLIA.genesis_validators_root),
        "genesis_time": ethlight::SEPOLIA.genesis_time,
        "seconds_per_slot": 12,
        "slots_per_epoch": 32,
        "epochs_per_sync_committee_period": 256,
        "forks": [{ "fork": "phase0", "version": "0x90000069", "epoch": 0 }, { "fork": "capella", "version": "0x90000072", "epoch": 56_832 }],
    });
    let network = eth_network_from_json(&written).unwrap();
    let definition = network.definition().unwrap();
    assert_eq!((definition.name.as_ref(), definition.forks.len()), ("sepolia", 2));
    assert_eq!(eth_network_from_json(&crate::params::json::eth_network_json(&network)).unwrap(), network, "as 0x SSZ, back the same");
    let mut broken = written.clone();
    broken["forks"] = json!([{ "fork": "capella", "version": "0x01000000", "epoch": 0 }]);
    assert!(eth_network_from_json(&broken).unwrap_err().contains("phase0"), "checked by Network::problem");
    assert!(EthNetwork::Custom(vec![1, 2, 3]).problem().is_some());

    // The settings ask for a relay and a start while Ethereum is on.
    let on = AnchorSettings { ethereum: true, ..AnchorSettings::OFF };
    assert!(on.problem().unwrap().contains("eth_relay"));
    let set = AnchorSettings { eth_relay: Some(crate::structs::address::Address::Contract([1; 32])), eth_start: Some(EthStart { root: [2; 32], network }), ..on };
    assert_eq!(set.problem(), None);
    let parsed = crate::params::json::anchoring_from_json(crate::params::json::Fields::of(&crate::params::json::anchoring_json(&set), "anchoring").unwrap()).unwrap();
    assert_eq!(parsed, set, "through JSON and back");
}
