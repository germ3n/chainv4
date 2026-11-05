//! Bitcoin anchors: the payload in its OP_RETURN, the header chain anchored sync checks, proofs
//! against the relay's records, and histories compared by Bitcoin.

use {
    super::*,
    crate::{
        anchor::{
            Anchors, Payload, ProveError, ProveRules, Proven, chain_tag, list_root, next_root, prove,
            sync::{AnchorVerdict, History, Sources, check_anchors, compare_histories},
        },
        crypto::{bitcoin::{merkle_path, merkle_root}, hasher::Hasher},
        db::MemoryDb,
        params::anchoring::{AnchorEntry, AnchorSettings, AnchorsAnswer},
        structs::{hash::{Hash, HashType}, tx::AnchorProof},
    },
    std::sync::Arc,
};

const REGTEST_BITS: u32 = 0x207f_ffff;

fn id(seed: &[u8]) -> Hash {
    Hasher::digest(HashType::default_chain(), seed)
}

fn genesis() -> Hash {
    id(b"genesis")
}

/// A coinbase without its witness: one input spending nothing.
fn coinbase(height: u64) -> Vec<u8> {
    let mut out = 1u32.to_le_bytes().to_vec();
    out.push(1);
    out.extend([0u8; 32]);
    out.extend([0xff; 4]);
    out.push(9);
    out.push(8);
    out.extend(height.to_le_bytes());
    out.extend([0xff; 4]);
    out.push(1);
    out.extend(50u64.to_le_bytes());
    out.push(1);
    out.push(0x51);
    out.extend([0u8; 4]);

    out
}

/// A transaction carrying an anchor, without witnesses.
fn anchor_tx(payload: &Payload, salt: u8) -> Vec<u8> {
    Transaction {
        version: 2,
        inputs: vec![crate::crypto::bitcoin::tx::TxIn { txid: [salt; 32], vout: 0, script_sig: Vec::new(), sequence: 0xffff_ffff, witness: Vec::new() }],
        outputs: vec![crate::crypto::bitcoin::tx::TxOut { value: 0, script: payload.script() }],
        lock_time: 0,
    }
    .base()
}

/// A Bitcoin block holding a coinbase and an anchor's transaction: its Merkle root, and the proof
/// of the anchor once the block's hash is known.
fn block_of(height: u64, raw: &[u8]) -> ([u8; 32], BtcAnchorProof) {
    let coinbase = coinbase(height);
    let ids = [sha256d(&coinbase), sha256d(raw)];
    let proof = BtcAnchorProof { block: [0; 32], raw: raw.to_vec(), index: 1, path: merkle_path(&ids, 1), coinbase: coinbase.clone(), coinbase_path: merkle_path(&ids, 0) };

    (merkle_root(&ids), proof)
}

/// A regtest header on a parent with a Merkle root, mined.
fn mined(prev: [u8; 32], merkle: [u8; 32], time: u32) -> [u8; 80] {
    let target = btclight::target(REGTEST_BITS).expect("a target");

    for nonce in 0u32.. {
        let mut header = [0u8; 80];
        header[..4].copy_from_slice(&4u32.to_le_bytes());
        header[4..36].copy_from_slice(&prev);
        header[36..68].copy_from_slice(&merkle);
        header[68..72].copy_from_slice(&time.to_le_bytes());
        header[72..76].copy_from_slice(&REGTEST_BITS.to_le_bytes());
        header[76..].copy_from_slice(&nonce.to_le_bytes());

        if crate::structs::u256::u256::from_le_bytes(sha256d(&header)) <= target {
            return header;
        }
    }

    unreachable!("some nonce meets regtest's difficulty")
}

fn start() -> (BtcStart, [u8; 32]) {
    let header = mined([0; 32], [1; 32], 1_000);

    (BtcStart { header: header.to_vec(), height: 0, network: BtcNetworkRules::Regtest }, sha256d(&header))
}

#[test]
fn a_payload_is_80_bytes_in_an_op_return_and_is_found_in_its_transaction() {
    let payload = Payload { chain: [1, 2, 3, 4], epoch: 77, block: [5; 32], root: [6; 32] };

    assert_eq!(Payload::decode(&payload.encode()), Some(payload));
    assert_eq!(payload.script()[..3], [0x6a, 0x4c, 80]);
    assert_eq!(payload.script().len(), 83, "an OP_RETURN Bitcoin's nodes relay by default");
    assert_eq!(Payload::find(&anchor_tx(&payload, 1)), Ok(payload));

    let mut other = payload.encode();
    other[0] = b'x';
    assert_eq!(Payload::decode(&other), None, "not cv4a");
    assert!(Payload::find(&coinbase(3)).is_err(), "a transaction without one");
}

#[test]
fn the_list_root_folds_each_entry_onto_the_one_before() {
    let entry = |epoch| AnchorEntry { epoch, block: id(&[epoch as u8]), source: AnchorSource::Bitcoin { block: [epoch as u8; 32], height: epoch }, enforced: true };
    let (first, second) = (entry(1), entry(2));

    assert_eq!(list_root(&[]), [0; 32]);
    assert_eq!(list_root(std::slice::from_ref(&first)), next_root(&[0; 32], &first));
    assert_eq!(list_root(&[first.clone(), second.clone()]), next_root(&next_root(&[0; 32], &first), &second));
    assert_ne!(list_root(&[first.clone(), second.clone()]), list_root(&[second, first]), "order counts");
}

#[test]
fn the_header_chain_follows_the_most_work_and_refuses_headers_without_it() {
    let (start, first) = start();
    let mut chain = HeaderChain::new(&start).expect("a start");
    let a1 = mined(first, [2; 32], 1_001);
    let a2 = mined(sha256d(&a1), [3; 32], 1_002);

    assert_eq!(chain.add(&a1, 2_000), Ok(true));
    assert_eq!(chain.add(&a1, 2_000), Ok(false), "known");
    assert_eq!(chain.add(&a2, 2_000), Ok(true));
    assert_eq!(chain.tip().1, 2);
    assert_eq!(chain.depth(&sha256d(&a1)), 2);

    // A branch from the start, longer: the main chain moves to it.
    let b1 = mined(first, [4; 32], 1_001);
    let b2 = mined(sha256d(&b1), [5; 32], 1_002);
    let b3 = mined(sha256d(&b2), [6; 32], 1_003);

    for header in [b1, b2, b3] {
        chain.add(&header, 2_000).expect("checks");
    }

    assert_eq!(chain.tip().0, sha256d(&b3));
    assert_eq!(chain.depth(&sha256d(&a1)), 0, "off the main chain now");
    assert_eq!(chain.main_at(1), Some(sha256d(&b1)));

    let mut easy = mined(sha256d(&b3), [7; 32], 1_004);
    easy[72..76].copy_from_slice(&0x1d00_ffffu32.to_le_bytes());
    assert!(chain.add(&easy, 2_000).is_err(), "a header not meeting the difficulty it names, or another than regtest's");
    assert!(chain.add(&mined([9; 32], [7; 32], 1_004), 2_000).is_err(), "building on nothing held");
    assert!(chain.add(&mined(sha256d(&b3), [7; 32], 999), 2_000).is_err(), "a time before the median");
    assert!(chain.add(&mined(sha256d(&b3), [7; 32], 20_000), 2_000).is_err(), "two hours ahead of the clock");
}

/// A relay's records as contracts/examples/btc_relay keeps them, written into a state.
struct MockRelay {
    address: Address,
}

impl MockRelay {
    fn kept(&self, state: &State, header: &[u8; 80], height: u64) {
        let mut kept = header[4..68].to_vec();
        kept.extend(height.to_le_bytes());
        kept.extend([0u8; 32]);
        kept.extend(&header[72..76]);
        kept.extend(&header[68..72]);
        kept.extend(&header[68..72]);
        kept.push(1);
        kept.extend(&header[68..72]);
        kept.extend(&header[..4]);
        kept.extend(&header[76..]);
        let hash = sha256d(header);
        state.put(Column::Namespaces, namespace_key(&self.address, b"headers", &hash), kept);
        state.put(Column::Namespaces, namespace_key(&self.address, b"main", &height.to_be_bytes()), hash.to_vec());
        state.put(Column::Namespaces, namespace_key(&self.address, &[], b"tip"), hash.to_vec());
    }
}

/// A chain's state with a relay started, a checkpoint at epoch 3, and a Bitcoin block holding its
/// anchor under 5 more: the anchor's proof, and the settings.
fn proving() -> (State, AnchorSettings, BtcAnchorProof, MockRelay, Vec<[u8; 80]>) {
    let state = State::new(Arc::new(MemoryDb::default()));
    let relay = MockRelay { address: Address::Contract([7; 32]) };
    let (start, first) = start();
    state.put(Column::Namespaces, namespace_key(&relay.address, &[], b"network"), b"regtest".to_vec());
    relay.kept(&state, start.header.as_slice().try_into().expect("80"), 0);
    let checkpoint = id(b"checkpoint 3");
    state.put_value(Column::Heights, 24u64.to_be_bytes().to_vec(), &checkpoint);
    let payload = Payload { chain: chain_tag(&genesis()), epoch: 3, block: checkpoint.to_bytes().try_into().expect("32"), root: [0; 32] };
    let (merkle, mut proof) = block_of(1, &anchor_tx(&payload, 1));
    let mut headers = vec![mined(first, merkle, 1_001)];

    for (at, time) in (2..=6).zip(1_002..) {
        headers.push(mined(sha256d(headers.last().expect("one")), [at as u8; 32], time));
    }

    for (height, header) in (1..).zip(&headers) {
        relay.kept(&state, header, height);
    }

    proof.block = sha256d(&headers[0]);
    let settings = AnchorSettings { bitcoin: true, ethereum: false, enforce: true, relay: Some(relay.address), eth_relay: None, eth_start: None, btc_start: Some(start), depth: 6, every: 10, reward: crate::structs::u256::u256::new(5) };

    (state, settings, proof, relay, headers)
}

fn rules(settings: &AnchorSettings) -> ProveRules<'_> {
    ProveRules { settings, genesis: genesis(), checkpoint_interval: Some(8), pooled: false, time: 0 }
}

#[test]
fn an_anchor_deep_enough_in_the_relay_is_recorded_enforced_and_paid() {
    let (state, settings, proof, relay, _) = proving();
    let sender = Address::Ed25519([3; 32]);

    assert_eq!(relay.address, settings.relay.expect("set"));
    assert_eq!(Relay::new(&state, relay.address).raw_header(&proof.block).expect("reads").map(|header| sha256d(&header)), Some(proof.block), "the header written whole again");

    let proven = prove(&state, &rules(&settings), &sender, &AnchorProof::Bitcoin(proof.clone())).expect("proves");
    let anchors = Anchors::new(&state);

    assert_eq!(proven, Proven { index: 0, reward: crate::structs::u256::u256::new(5) });
    assert_eq!(anchors.count().expect("reads"), 1);
    assert_eq!(anchors.enforced_height().expect("reads"), Some(24));
    assert_eq!(anchors.by_epoch(3).expect("reads"), Some(0));
    assert_eq!(anchors.proof(0).expect("reads"), Some(AnchorProof::Bitcoin(proof.clone())));
    let entry = anchors.entry(0).expect("reads").expect("held");
    assert_eq!((entry.epoch, entry.source, entry.enforced), (3, AnchorSource::Bitcoin { block: proof.block, height: 1 }, true));
    assert_eq!(anchors.root().expect("reads"), list_root(&[entry]));
    assert_eq!(crate::registry::accounts::Accounts::new(&state).get(&sender).expect("reads").balance, crate::structs::u256::u256::new(5));

    let again = prove(&state, &rules(&settings), &sender, &AnchorProof::Bitcoin(proof.clone()));
    assert!(matches!(again, Err(ProveError::Refused(why)) if why.contains("anchored already")));
}

#[test]
fn an_anchor_is_refused_unless_everything_it_says_holds() {
    let refused = |change: &dyn Fn(&mut AnchorSettings, &mut BtcAnchorProof, &State)| {
        let (state, mut settings, mut proof, _, _) = proving();
        change(&mut settings, &mut proof, &state);

        match prove(&state, &rules(&settings), &Address::Ed25519([3; 32]), &AnchorProof::Bitcoin(proof.clone())) {
            Err(ProveError::Refused(why)) => why,
            other => panic!("{other:?}"),
        }
    };

    assert!(refused(&|settings, _, _| settings.bitcoin = false).contains("off"));
    assert!(refused(&|settings, _, _| settings.depth = 7).contains("deep"));
    assert!(refused(&|_, proof, _| proof.index = 0).contains("Merkle"));
    assert!(refused(&|_, proof, _| proof.coinbase_path.clear()).contains("paths"));
    assert!(refused(&|_, proof, _| proof.block = [9; 32]).contains("no such Bitcoin block"));
    assert!(refused(&|settings, _, _| settings.btc_start.as_mut().expect("set").height = 1).contains("did not start"));
    assert!(refused(&|_, _, state| state.put_value(Column::Heights, 24u64.to_be_bytes().to_vec(), &id(b"another"))).contains("not this branch's checkpoint"));

    // Another chain's tag, or a root the list does not hold now.
    let (state, settings, _, relay, headers) = proving();
    let checkpoint: [u8; 32] = id(b"checkpoint 3").to_bytes().try_into().expect("32");
    let foreign = |payload: Payload| {
        let (merkle, mut proof) = block_of(7, &anchor_tx(&payload, 2));
        let header = mined(sha256d(headers.last().expect("one")), merkle, 1_010);
        relay.kept(&state, &header, 7);
        proof.block = sha256d(&header);

        proof
    };
    let settings = AnchorSettings { depth: 1, ..settings };
    let proof = foreign(Payload { chain: [0; 4], epoch: 3, block: checkpoint, root: [0; 32] });
    assert!(matches!(prove(&state, &rules(&settings), &Address::Ed25519([3; 32]), &AnchorProof::Bitcoin(proof.clone())), Err(ProveError::Refused(why)) if why.contains("another chain")));
    let proof = foreign(Payload { chain: chain_tag(&genesis()), epoch: 3, block: checkpoint, root: [4; 32] });
    assert!(matches!(prove(&state, &rules(&settings), &Address::Ed25519([3; 32]), &AnchorProof::Bitcoin(proof.clone())), Err(ProveError::Refused(why)) if why.contains("root")));
}

#[test]
fn rewards_are_paid_every_so_many_epochs_and_never_beyond_the_pool() {
    let (state, settings, proof, relay, headers) = proving();
    let sender = Address::Ed25519([3; 32]);
    prove(&state, &rules(&settings), &sender, &AnchorProof::Bitcoin(proof.clone())).expect("proves");

    // Epoch 5's anchor, under the every of 10: recorded, unpaid.
    let checkpoint = id(b"checkpoint 5");
    state.put_value(Column::Heights, 40u64.to_be_bytes().to_vec(), &checkpoint);
    let root = Anchors::new(&state).root().expect("reads");
    let payload = Payload { chain: chain_tag(&genesis()), epoch: 5, block: checkpoint.to_bytes().try_into().expect("32"), root };
    let (merkle, mut next) = block_of(7, &anchor_tx(&payload, 3));
    let header = mined(sha256d(headers.last().expect("one")), merkle, 1_010);
    relay.kept(&state, &header, 7);
    next.block = sha256d(&header);
    let settings = AnchorSettings { depth: 1, enforce: false, ..settings };

    assert_eq!(prove(&state, &rules(&settings), &sender, &AnchorProof::Bitcoin(next)).expect("proves"), Proven { index: 1, reward: crate::structs::u256::u256::ZERO });
    assert_eq!(Anchors::new(&state).enforced_height().expect("reads"), Some(24), "the unenforced one moves nothing");

    // From an empty pool, nothing.
    let (state, settings, proof, _, _) = proving();
    let pooled = ProveRules { pooled: true, ..rules(&settings) };
    assert_eq!(prove(&state, &pooled, &sender, &AnchorProof::Bitcoin(proof)).expect("proves").reward, crate::structs::u256::u256::ZERO);
}

/// A history's entries and the Bitcoin blocks they sit in, built on a shared chain of headers.
struct Histories {
    headers: HeaderChain,
    tip: [u8; 32],
    height: u64,
    time: u32,
}

impl Histories {
    fn sources(&self) -> Sources<'_> {
        Sources { bitcoin: Some(&self.headers), ..Sources::default() }
    }

    fn new() -> (Histories, BtcStart) {
        let (start, first) = start();

        (Histories { headers: HeaderChain::new(&start).expect("a start"), tip: first, height: 0, time: 1_000 }, start)
    }

    /// Mines a block holding the anchor of the next entry of a list, and buries it under depth.
    fn anchor(&mut self, entries: &mut Vec<AnchorEntry>, proofs: &mut Vec<(u64, AnchorProof)>, epoch: u64, seed: &[u8], depth: u64) {
        let block = id(seed);
        let payload = Payload { chain: chain_tag(&genesis()), epoch, block: block.to_bytes().try_into().expect("32"), root: list_root(entries) };
        let (merkle, mut proof) = block_of(self.height + 1, &anchor_tx(&payload, epoch as u8));
        self.time += 1;
        let header = mined(self.tip, merkle, self.time);
        self.headers.add(&header, 1_000_000).expect("checks");
        (self.tip, self.height) = (sha256d(&header), self.height + 1);
        proof.block = self.tip;
        proofs.push((entries.len() as u64, AnchorProof::Bitcoin(proof)));
        entries.push(AnchorEntry { epoch, block, source: AnchorSource::Bitcoin { block: self.tip, height: self.height }, enforced: true });

        for _ in 1..depth {
            self.time += 1;
            let header = mined(self.tip, [0; 32], self.time);
            self.headers.add(&header, 1_000_000).expect("checks");
            (self.tip, self.height) = (sha256d(&header), self.height + 1);
        }
    }
}

fn checked(histories: &Histories, start: &BtcStart, entries: &[AnchorEntry], proofs: &[(u64, AnchorProof)], shown: &[u64]) -> History {
    let answer = AnchorsAnswer { start: Some(start.clone()), btc_tip: histories.height, eth_start: None, entries: entries.to_vec(), proofs: proofs.iter().filter(|(index, _)| shown.contains(index)).cloned().collect() };

    check_anchors(&answer, &histories.sources(), &genesis(), 2).expect("checks")
}

#[test]
fn the_history_bitcoin_saw_first_wins_where_two_part() {
    let (mut btc, start) = Histories::new();
    let (mut honest, mut honest_proofs) = (Vec::new(), Vec::new());
    btc.anchor(&mut honest, &mut honest_proofs, 10, b"shared 10", 2);
    // The fork: its own checkpoint at epoch 20, anchored later than the real chain's.
    let (mut forked, mut forked_proofs) = (honest.clone(), honest_proofs.clone());
    btc.anchor(&mut honest, &mut honest_proofs, 20, b"real 20", 2);
    btc.anchor(&mut honest, &mut honest_proofs, 30, b"real 30", 2);
    btc.anchor(&mut forked, &mut forked_proofs, 20, b"forked 20", 2);
    btc.anchor(&mut forked, &mut forked_proofs, 40, b"forked 40", 2);

    // Each shows only its newest proof first: where they part, both proofs are needed.
    let real = checked(&btc, &start, &honest, &honest_proofs, &[2]);
    let fork = checked(&btc, &start, &forked, &forked_proofs, &[2]);
    assert_eq!(compare_histories(&real, &fork), AnchorVerdict::NeedProofs(1, 1));

    let real = checked(&btc, &start, &honest, &honest_proofs, &[1, 2]);
    let fork = checked(&btc, &start, &forked, &forked_proofs, &[1, 2]);
    assert_eq!(compare_histories(&real, &fork), AnchorVerdict::Earlier(false), "the real one was first");
    assert_eq!(compare_histories(&fork, &real), AnchorVerdict::Earlier(true));
    assert_eq!(real.newest().block, id(b"real 30"));

    // The same history, shown by a peer that is further behind: the longer wins, nothing asked.
    let behind = checked(&btc, &start, &honest[..2], &honest_proofs, &[1]);
    assert_eq!(compare_histories(&behind, &real), AnchorVerdict::Extends(true));
    assert_eq!(compare_histories(&real, &behind), AnchorVerdict::Extends(false));
}

#[test]
fn a_list_that_does_not_check_against_bitcoin_is_refused() {
    let (mut btc, start) = Histories::new();
    let (mut entries, mut proofs) = (Vec::new(), Vec::new());
    btc.anchor(&mut entries, &mut proofs, 10, b"first", 2);
    btc.anchor(&mut entries, &mut proofs, 20, b"second", 2);
    let answer = |entries: &[AnchorEntry], proofs: &[(u64, AnchorProof)]| AnchorsAnswer { start: Some(start.clone()), btc_tip: btc.height, eth_start: None, entries: entries.to_vec(), proofs: proofs.to_vec() };
    let sources = btc.sources();

    assert!(check_anchors(&answer(&entries, &proofs[1..]), &sources, &genesis(), 2).is_ok());
    // An entry before the newest that its root does not hold: copied, changed or made up.
    let mut changed = entries.clone();
    changed[0].block = id(b"made up");
    assert!(check_anchors(&answer(&changed, &proofs[1..]), &sources, &genesis(), 2).unwrap_err().contains("root"));
    // An entry that says a lower Bitcoin height than its block's.
    let mut lower = entries.clone();
    if let AnchorSource::Bitcoin { height, .. } = &mut lower[0].source {
        *height = 0;
    }
    assert!(check_anchors(&answer(&lower, &proofs[..1]), &sources, &genesis(), 2).is_err());
    // Not deep enough, and no proof of the newest at all.
    assert!(check_anchors(&answer(&entries, &proofs[1..]), &sources, &genesis(), 3).unwrap_err().contains("deep"));
    assert!(check_anchors(&answer(&entries, &proofs[..1]), &sources, &genesis(), 2).unwrap_err().contains("newest"));
    // Another chain's anchors.
    assert!(check_anchors(&answer(&entries, &proofs[1..]), &sources, &id(b"another genesis"), 2).is_err());
}

#[test]
fn two_histories_anchored_in_one_bitcoin_block_tie() {
    let source = AnchorSource::Bitcoin { block: [1; 32], height: 5 };
    let entry = |seed: &[u8]| AnchorEntry { epoch: 1, block: id(seed), source, enforced: true };
    let history = |seed: &[u8]| History { entries: vec![entry(seed)], proven: [(0, source)].into(), readable: vec![crate::params::anchoring::AnchorChain::Bitcoin] };

    assert_eq!(compare_histories(&history(b"one"), &history(b"other")), AnchorVerdict::Tie(0));
}
