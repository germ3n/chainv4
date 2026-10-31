//! Block ranges off a real chain: what a node keeps and sends, and what a client refuses of it. The
//! parts that need no chain are in light/range.rs, and the route is in the API's tests.

use {
    super::*,
    crate::db::{Database, StateProof, history_list_key},
    crate::light::{BlockRange, NoProofs, ProvenEntry, RangeBody, RangeChanges, RangeCursor, RangeError, RangeItem, check_range_item, decode_block_range},
    crate::structs::block::BodyRoot,
    crate::structs::vote::Checkpoint,
};

/// Keeps a value, and takes it out again: what makes a block delete an entry.
const KEEPER: &str = r#"
local M = {}

function M.keep(key, value)
    storage.set(key, value)
end

function M.drop(key)
    storage.set(key, nil)
end

return M
"#;

const ALL: RangeLimits = RangeLimits { blocks: 1_000, bytes: u64::MAX, entries: u32::MAX, time: std::time::Duration::from_secs(600) };

fn range_of(db: &Arc<MemoryDb>, chain: &Chain, from: u64, entry: u32, count: u64, limits: &RangeLimits) -> BlockRange {
    stored_block_range(db.clone(), chain.hash(), from, entry, count, limits).unwrap().unwrap()
}

fn header_at(chain: &Chain, height: u64) -> SealedHeader {
    chain.headers_from(height, 1).unwrap().pop().unwrap()
}

fn item_at(db: &Arc<MemoryDb>, chain: &Chain, height: u64) -> RangeItem {
    range_of(db, chain, height, 0, 1, &ALL).items.into_iter().next().unwrap()
}

fn check(chain: &Chain, item: RangeItem) -> Result<crate::light::CheckedItem, RangeError> {
    check_range_item(chain.hash(), &header_at(chain, item.sealed.header.height), item)
}

fn call(contract: Address, function: &str, data: Vec<TxCallArgumentType>, nonce: u64) -> SignedTx {
    let tx = Tx::ContractCall { to: contract, func: Hash::Xxh3_64(selector(function)), amount: Amount::U128(0), data };

    SignedTx::sign(tx, nonce, FeeOffer::NONE, &authority())
}

fn text(value: &str) -> TxCallArgumentType {
    TxCallArgumentType::String(value.to_string())
}

/// A chain with transfers, a deploy, a value kept and that value dropped. Returns the chain's
/// database and chain, and the contract's address.
fn busy_chain() -> (Arc<MemoryDb>, Chain, Address) {
    let db = Arc::new(MemoryDb::default());
    let mut chain = open(&db);
    let me = authority().public().address();
    let (bob, carol) = (Keypair::from_seed([1; 32]).public().address(), Keypair::from_seed([2; 32]).public().address());

    // Block 1, two transfers; 2, a deploy; 3, a value kept; 4, nothing; 5, the value dropped.
    let pays = vec![transfer(&chain, &authority(), &bob, 5), SignedTx::sign(Tx::Transfer { from: me, to: carol, amount: Amount::U128(7) }, 1, FeeOffer::NONE, &authority())];
    advance(&mut chain, pays);
    let contract = contract_address(&me, 2, chain.hash());
    let deploy = SignedTx::sign(Tx::Deploy { code: Code::Lua(KEEPER.to_string()) }, 2, FeeOffer::NONE, &authority());
    assert_eq!(advance(&mut chain, vec![deploy])[0].result, Ok(Effect::Deployed(contract)));
    assert!(advance(&mut chain, vec![call(contract, "keep", vec![text("kept"), text("value")], 3)])[0].result.is_ok());
    advance(&mut chain, Vec::new());
    assert!(advance(&mut chain, vec![call(contract, "drop", vec![text("kept")], 4)])[0].result.is_ok());

    (db, chain, contract)
}

fn entries(item: &RangeItem) -> &[ProvenEntry] {
    match &item.changes {
        RangeChanges::Proven { entries, .. } => entries,
        RangeChanges::None(why) => panic!("no proofs: {why:?}"),
    }
}

fn entries_mut(item: &mut RangeItem) -> &mut Vec<ProvenEntry> {
    match &mut item.changes {
        RangeChanges::Proven { entries, .. } => entries,
        RangeChanges::None(why) => panic!("no proofs: {why:?}"),
    }
}

/// A body's parts, to tamper with.
struct Parts<'a> {
    txs: &'a mut Vec<SignedTx>,
    proofs: &'a mut Vec<Justification>,
    pool: &'a mut Option<PoolCommitment>,
    receipts: &'a mut Vec<CommittedReceipt>,
}

fn parts(item: &mut RangeItem) -> Parts<'_> {
    match &mut item.body {
        RangeBody::Whole { txs, proofs, pool, receipts: Some(receipts) } => Parts { txs, proofs, pool, receipts },
        other => panic!("not whole: {other:?}"),
    }
}

#[test]
fn a_chains_blocks_go_through_the_encoding_and_every_one_checks() {
    let (db, chain, contract) = busy_chain();
    let bytes = wincode::serialize(&range_of(&db, &chain, 0, 0, 100, &ALL)).unwrap();
    let range = decode_block_range(&bytes).unwrap();

    range.check_shape(0, 0).unwrap();
    assert_eq!((range.tip, range.next, range.items.len() as u64), (5, None, 6));

    let checked: Vec<_> = range.items.into_iter().map(|item| check(&chain, item).unwrap()).collect();

    for (height, item) in checked.iter().enumerate() {
        assert_eq!((item.height(), item.block), (height as u64, header_at(&chain, height as u64).id()));
    }

    // Genesis ran nothing, so it has a body of nothing and no list.
    assert_eq!(checked[0].body, RangeBody::Whole { txs: Vec::new(), proofs: Vec::new(), pool: None, receipts: Some(Vec::new()) });
    assert_eq!(checked[0].changes, RangeChanges::None(NoProofs::NoList));

    // What each transaction did is there, in order, for the transaction beside it.
    let RangeBody::Whole { txs, receipts: Some(receipts), .. } = &checked[1].body else { panic!("whole") };
    assert_eq!(txs.len(), 2);
    assert_eq!(receipts.iter().map(|receipt| receipt.tx).collect::<Vec<_>>(), txs.iter().map(|tx| tx.id(chain.hash())).collect::<Vec<_>>());

    // A client picks out its own entry, and reads what it held once the block had run.
    let bob = Keypair::from_seed([1; 32]).public().address();
    let key = crate::db::address_key(&bob);
    let held = checked[1].entries().iter().find(|entry| entry.column == Column::Accounts && entry.key == key).expect("the transfer changed bob's account");
    let account: crate::structs::account::Account = wincode::deserialize(held.value.as_ref().unwrap()).unwrap();
    assert_eq!(account.balance, u256::new(5));

    // The sender's too, and nothing outside the state tree's columns, nor the tree's own nodes.
    assert!(checked[1].entries().iter().any(|entry| entry.key == crate::db::address_key(&authority().public().address())));
    assert!(checked.iter().flat_map(|item| item.entries()).all(|entry| crate::db::STATE_COLUMNS.contains(&entry.column)));

    // The deploy wrote its contract, and the call its storage.
    assert!(checked[2].entries().iter().any(|entry| entry.column == Column::Contracts && entry.key == crate::db::address_key(&contract)));
    assert!(checked[3].entries().iter().any(|entry| entry.column == Column::Storage && entry.value.as_deref() == Some(&b"value"[..])));
}

#[test]
fn a_block_is_shown_with_the_entries_it_changed_as_they_stood_then_not_as_they_stand_now() {
    let db = Arc::new(MemoryDb::default());
    let mut chain = open(&db);
    let bob = Keypair::from_seed([1; 32]).public().address();

    for amount in [5, 6, 7] {
        let tx = transfer(&chain, &authority(), &bob, amount);
        advance(&mut chain, vec![tx]);
    }

    let key = crate::db::address_key(&bob);
    let balance_at = |height: u64| {
        let item = check(&chain, item_at(&db, &chain, height)).unwrap();
        let held = item.entries().iter().find(|entry| entry.key == key).unwrap().value.clone().unwrap();

        wincode::deserialize::<crate::structs::account::Account>(&held).unwrap().balance
    };

    assert_eq!((balance_at(1), balance_at(2), balance_at(3)), (u256::new(5), u256::new(11), u256::new(18)));
}

#[test]
fn a_deleted_entry_is_proven_absent_and_cannot_be_shown_as_still_there() {
    let (db, chain, contract) = busy_chain();
    let key = crate::db::storage_key(&contract, b"kept");

    let kept = item_at(&db, &chain, 3);
    let held = entries(&kept).iter().find(|entry| entry.key == key).expect("the block that kept it lists it").clone();
    assert_eq!(held.value.as_deref(), Some(&b"value"[..]));

    let dropped = item_at(&db, &chain, 5);
    let at = entries(&dropped).iter().position(|entry| entry.key == key).expect("the block that dropped it lists it");
    assert_eq!(entries(&dropped)[at].value, None, "it holds nothing now");
    check(&chain, dropped.clone()).unwrap();

    // The node cannot say it is still there, with the value or with the proof from when it was.
    let mut lying = dropped.clone();
    entries_mut(&mut lying)[at].value = Some(b"value".to_vec());
    assert!(matches!(check(&chain, lying), Err(RangeError::BadEntry { .. })));

    let mut lying = dropped;
    entries_mut(&mut lying)[at] = held;
    assert!(matches!(check(&chain, lying), Err(RangeError::BadEntry { .. })));

    // Nor that an entry another block kept is gone.
    let mut lying = kept;
    let at = entries(&lying).iter().position(|entry| entry.key == key).unwrap();
    entries_mut(&mut lying)[at].value = None;
    assert!(matches!(check(&chain, lying), Err(RangeError::BadEntry { .. })));
}

#[test]
fn every_root_and_proof_tampered_with_is_refused() {
    let (db, chain, _) = busy_chain();
    let good = item_at(&db, &chain, 1);
    check(&chain, good.clone()).unwrap();
    let hash = chain.hash();
    let refused = |change: &dyn Fn(&mut RangeItem)| {
        let mut item = good.clone();
        change(&mut item);

        check(&chain, item).err()
    };

    // The header, each root in it and the seal.
    let header = |change: &dyn Fn(&mut BlockHeader)| refused(&|item| change(&mut item.sealed.header));
    let other = Hash::from_bytes(hash, &[9; 32]).unwrap();
    assert_eq!(header(&|header| header.tx_root = other), Some(RangeError::WrongHeader));
    assert_eq!(header(&|header| header.proof_root = other), Some(RangeError::WrongHeader));
    assert_eq!(header(&|header| header.pool_root = other), Some(RangeError::WrongHeader));
    assert_eq!(header(&|header| header.receipts_root = other), Some(RangeError::WrongHeader));
    assert_eq!(header(&|header| header.state_root = other), Some(RangeError::WrongHeader));
    assert_eq!(refused(&|item| item.sealed.seal = Seal::None), Some(RangeError::WrongHeader));

    // Each root against the body.
    assert_eq!(refused(&|item| { parts(item).txs.pop(); }), Some(RangeError::Root(BodyRoot::Tx)), "a transaction dropped");
    assert_eq!(refused(&|item| parts(item).txs.reverse()), Some(RangeError::Root(BodyRoot::Tx)), "transactions reordered");
    assert_eq!(refused(&|item| parts(item).txs[0].nonce += 1), Some(RangeError::Root(BodyRoot::Tx)), "a transaction changed");
    let checkpoint = Checkpoint { epoch: 0, block: other };
    assert_eq!(
        refused(&|item| parts(item).proofs.push(Justification { source: checkpoint, target: checkpoint, votes: Vec::new() })),
        Some(RangeError::Root(BodyRoot::Proof)),
        "a finality proof added",
    );
    assert_eq!(
        refused(&|item| *parts(item).pool = Some(PoolCommitment { parts: 1, root: other, count: 0 })),
        Some(RangeError::Root(BodyRoot::Pool)),
        "a pool's commitment added",
    );
    assert_eq!(refused(&|item| { parts(item).receipts.pop(); }), Some(RangeError::ReceiptsRoot), "a receipt dropped");
    assert_eq!(refused(&|item| parts(item).receipts.reverse()), Some(RangeError::ReceiptsRoot), "receipts reordered");
    assert_eq!(refused(&|item| parts(item).receipts[0].gas_used += 1), Some(RangeError::ReceiptsRoot), "a receipt changed");
    assert_eq!(refused(&|item| parts(item).receipts[0].outcome = CommittedOutcome::Failed), Some(RangeError::ReceiptsRoot), "what it did changed");

    // Each entry, in each thing a proof says.
    for at in 0..entries(&good).len() {
        let bad = |change: &dyn Fn(&mut ProvenEntry)| refused(&|item| change(&mut entries_mut(item)[at]));

        assert!(matches!(bad(&|entry| entry.value.as_mut().unwrap().push(0)), Some(RangeError::BadEntry { index, .. }) if index as usize == at), "another value");
        assert!(matches!(bad(&|entry| entry.value = None), Some(RangeError::BadEntry { .. })), "nothing there");
        assert!(matches!(bad(&|entry| entry.key.push(0)), Some(RangeError::BadEntry { .. })), "another key");
        assert!(matches!(bad(&|entry| entry.column = Column::Names), Some(RangeError::BadEntry { .. })), "another column");
        assert!(matches!(bad(&|entry| entry.proof.leaf = None), Some(RangeError::BadEntry { .. })), "no leaf");
        assert!(matches!(bad(&|entry| entry.proof = StateProof { siblings: vec![other; 300], leaf: None }), Some(RangeError::BadEntry { .. })), "too long");

        if !entries(&good)[at].proof.siblings.is_empty() {
            assert!(matches!(bad(&|entry| entry.proof.siblings[0] = other), Some(RangeError::BadEntry { .. })), "a sibling changed");
            assert!(matches!(bad(&|entry| { entry.proof.siblings.pop(); }), Some(RangeError::BadEntry { .. })), "a sibling dropped");
        }
    }

    // Another block's entries do not check here.
    let mut swapped = good.clone();
    *entries_mut(&mut swapped) = entries(&item_at(&db, &chain, 3)).to_vec();
    assert!(matches!(check(&chain, swapped), Err(RangeError::BadEntry { .. }) | Err(RangeError::BadCounts)));

    // Nor is a body or header from another block taken for this one.
    let mut other_header = good.clone();
    other_header.sealed = header_at(&chain, 2);
    assert_eq!(check_range_item(hash, &header_at(&chain, 1), other_header), Err(RangeError::WrongHeader));
}

#[test]
fn receipts_each_for_another_transaction_are_refused_even_under_a_root_that_fits() {
    let (db, chain, _) = busy_chain();
    let hash = chain.hash();
    let good = item_at(&db, &chain, 1);
    // What a header that commits to them would need, as no honest producer's does.
    let under_own_root = |item: &RangeItem| {
        let mut sealed = item.sealed.clone();
        let RangeBody::Whole { receipts: Some(receipts), .. } = &item.body else { panic!("whole") };
        sealed.header.receipts_root = receipts_root(hash, receipts);

        sealed
    };

    let mut swapped = good.clone();
    parts(&mut swapped).receipts.swap(0, 1);
    let sealed = under_own_root(&swapped);
    swapped.sealed = sealed.clone();
    assert_eq!(check_range_item(hash, &sealed, swapped), Err(RangeError::ReceiptOutOfPlace { index: 0 }));

    let mut short = good;
    parts(&mut short).receipts.pop();
    let sealed = under_own_root(&short);
    short.sealed = sealed.clone();
    assert_eq!(check_range_item(hash, &sealed, short), Err(RangeError::ReceiptOutOfPlace { index: 1 }));
}

#[test]
fn a_pools_block_is_checked_by_its_pool_root_as_a_snapshot_node_now_checks_it_too() {
    let db = Arc::new(MemoryDb::default());
    let mut chain = Chain::open(db.clone(), &pow_genesis()).unwrap();
    let commitment = PoolCommitment { parts: 500_000_000, root: Hash::zero(chain.hash()), count: 0 };
    let block = sealed(&chain, chain.build_declaring(Vec::new(), Vec::new(), Address::Ed25519([7; 32]), 1_000, None, None, Some(commitment)).unwrap());
    extended(chain.import(&block, LATER));

    let item = item_at(&db, &chain, 1);
    assert!(matches!(&check(&chain, item.clone()).unwrap().body, RangeBody::Whole { pool: Some(pool), .. } if *pool == commitment));

    let mut changed = item.clone();
    *parts(&mut changed).pool = Some(PoolCommitment { parts: 1, ..commitment });
    assert_eq!(check(&chain, changed), Err(RangeError::Root(BodyRoot::Pool)));
    let mut none = item;
    *parts(&mut none).pool = None;
    assert_eq!(check(&chain, none), Err(RangeError::Root(BodyRoot::Pool)));

    // What fits checks: a block with the wrong commitment was not the pivot's.
    let header = block.sealed_header();
    assert!(block.fits(&header, chain.hash()));
    let mut other = block.clone();
    other.pool = Some(PoolCommitment { parts: 1, ..commitment });
    assert!(!other.fits(&header, chain.hash()), "the commitment is checked, not only the header");
    other.pool = None;
    assert!(!other.fits(&header, chain.hash()));
}

/// Asks for a whole range a response at a time, each checked, and returns what came.
fn walked(db: &Arc<MemoryDb>, chain: &Chain, limits: &RangeLimits) -> (Vec<(u64, RangeBody, Vec<ProvenEntry>)>, usize) {
    let (mut got, mut answers): (Vec<(u64, RangeBody, Vec<ProvenEntry>)>, usize) = (Vec::new(), 0);
    let mut at = RangeCursor { height: 0, entry: 0 };

    loop {
        let range = range_of(db, chain, at.height, at.entry, limits.blocks, limits);
        range.check_shape(at.height, at.entry).unwrap();
        assert!(!range.items.is_empty(), "each answer gets further");
        answers += 1;
        assert!(answers < 10_000, "it ends");

        for item in &range.items {
            let checked = check(chain, item.clone()).unwrap();
            let proven = checked.entries().to_vec();

            match got.last_mut() {
                Some((height, _, entries)) if *height == checked.height() => entries.extend(proven),
                _ => got.push((checked.height(), checked.body.clone(), proven)),
            }
        }

        match range.next {
            Some(next) => at = next,
            None => return (got, answers),
        }
    }
}

#[test]
fn every_bound_stops_an_answer_where_it_says_to_go_on_from_and_nothing_is_lost() {
    let (db, chain, _) = busy_chain();
    let (all, one) = walked(&db, &chain, &ALL);
    assert_eq!((all.len(), one), (6, 1));

    // A block at a time; an entry or two at a time; bytes so few each answer gives the least it
    // may; some in between; and no time at all.
    for limits in [
        RangeLimits { blocks: 1, ..ALL },
        RangeLimits { entries: 1, ..ALL },
        RangeLimits { entries: 2, ..ALL },
        RangeLimits { bytes: 1, ..ALL },
        RangeLimits { bytes: 2_000, ..ALL },
        RangeLimits { time: std::time::Duration::ZERO, ..ALL },
    ] {
        let (got, answers) = walked(&db, &chain, &limits);

        assert_eq!(got, all, "{limits:?}: everything arrives, in order, whatever the bounds");
        assert!(answers > 1, "{limits:?}");
    }

    // Blocks: as many as asked, and as the limit says, and where to go on.
    let range = range_of(&db, &chain, 1, 0, 2, &ALL);
    assert_eq!((range.items.len(), range.next), (2, Some(RangeCursor { height: 3, entry: 0 })));
    let range = range_of(&db, &chain, 1, 0, 100, &RangeLimits { blocks: 3, ..ALL });
    assert_eq!((range.items.len(), range.next), (3, Some(RangeCursor { height: 4, entry: 0 })));
    assert_eq!(range_of(&db, &chain, 1, 0, 0, &ALL).items.len(), 1, "at least one");

    // Past the tip, nothing, and nothing to go on to.
    let range = range_of(&db, &chain, 6, 0, 100, &ALL);
    assert_eq!((range.tip, range.items.len(), range.next), (5, 0, None));
    assert_eq!(range_of(&db, &chain, u64::MAX, 0, u64::MAX, &ALL).items.len(), 0);

    // Entries: stopped inside a block, it says where, and going on leaves its body out.
    let range = range_of(&db, &chain, 1, 0, 100, &RangeLimits { entries: 1, ..ALL });
    assert_eq!((range.items.len(), range.next), (1, Some(RangeCursor { height: 1, entry: 1 })));
    assert_eq!(entries(&range.items[0]).len(), 1);
    assert!(matches!(range.items[0].body, RangeBody::Whole { .. }));
    let more = range_of(&db, &chain, 1, 1, 100, &RangeLimits { entries: 1, ..ALL });
    assert_eq!(more.items[0].body, RangeBody::Continued);
    assert!(matches!(more.items[0].changes, RangeChanges::Proven { from: 1, .. }));

    // A list asked past its end, as after a reorg.
    assert_eq!(stored_block_range(db.clone(), chain.hash(), 1, 1_000, 1, &ALL).unwrap(), Err(RangeProblem::NoSuchEntry));
}

#[test]
fn an_answer_stays_within_its_bytes_but_for_what_the_first_block_must_give() {
    let (db, chain, _) = busy_chain();
    let size = |range: &BlockRange| wincode::serialized_size(range).unwrap();
    let whole = range_of(&db, &chain, 1, 0, 100, &ALL);
    let first_alone = range_of(&db, &chain, 1, 0, 1, &ALL);

    for limit in (500..size(&whole)).step_by(300) {
        let range = range_of(&db, &chain, 1, 0, 100, &RangeLimits { bytes: limit, ..ALL });

        range.check_shape(1, 0).unwrap();
        // One entry's the least the first block gives, and it may be that which passes the limit.
        assert!(size(&range) <= limit.max(size(&least_of_first(&first_alone))), "{limit}: {}", size(&range));
    }

    // Too few for any of it, and the first block comes anyway, with one entry.
    let range = range_of(&db, &chain, 1, 0, 100, &RangeLimits { bytes: 1, ..ALL });
    assert_eq!(range.items.len(), 1);
    assert_eq!(entries(&range.items[0]).len(), 1);
    check(&chain, range.items[0].clone()).unwrap();
}

/// The first block of a range as the least an answer may hold of it: the body, and one entry.
fn least_of_first(range: &BlockRange) -> BlockRange {
    let mut least = range.clone();
    entries_mut(&mut least.items[0]).truncate(1);
    least.next = Some(RangeCursor { height: 0, entry: 0 });

    least
}

#[test]
fn state_pruned_is_said_of_each_block_and_the_block_still_comes() {
    let db = Arc::new(MemoryDb::default());
    let mut chain = open(&db);
    chain.keep_state_history(Some(3));
    let to = stranger().public().address();

    for amount in 1..=8u128 {
        let tx = transfer(&chain, &authority(), &to, amount);
        advance(&mut chain, vec![tx]);
    }

    assert_eq!(chain.history_from().unwrap(), 5);
    let range = range_of(&db, &chain, 3, 0, 100, &ALL);
    range.check_shape(3, 0).unwrap();
    let checked: Vec<_> = range.items.into_iter().map(|item| check(&chain, item).unwrap()).collect();
    let heights: Vec<u64> = checked.iter().map(|item| item.height()).collect();
    assert_eq!(heights, [3, 4, 5, 6, 7, 8]);

    for item in &checked {
        assert!(matches!(&item.body, RangeBody::Whole { txs, receipts: Some(receipts), .. } if txs.len() == 1 && receipts.len() == 1), "the block is given all the same");
    }

    assert_eq!(checked[0].changes, RangeChanges::None(NoProofs::StatePruned { kept_from: 5 }));
    assert_eq!(checked[1].changes, RangeChanges::None(NoProofs::StatePruned { kept_from: 5 }));
    // The lowest height's state can still be read, but its list went as the height below did.
    assert_eq!(checked[2].changes, RangeChanges::None(NoProofs::NoList));
    assert!(checked[3..].iter().all(|item| !item.entries().is_empty()));
}

#[test]
fn an_archive_serves_every_height_it_ran() {
    let db = Arc::new(MemoryDb::default());
    let mut chain = open(&db);
    chain.keep_state_history(None);
    let to = stranger().public().address();

    for amount in 1..=12u128 {
        let tx = transfer(&chain, &authority(), &to, amount);
        advance(&mut chain, vec![tx]);
    }

    let range = range_of(&db, &chain, 1, 0, 100, &ALL);
    assert_eq!(range.items.len(), 12);
    assert!(range.items.into_iter().all(|item| !check(&chain, item).unwrap().entries().is_empty()));
}

#[test]
fn a_database_that_lacks_a_blocks_list_says_so_of_that_block_and_serves_the_rest() {
    let (db, chain, _) = busy_chain();
    let reopened = open(&db);

    // What a database from before these lists would hold: state read at a height, and no list.
    let state = State::new(db.clone());
    state.delete(Column::History, history_list_key(3));
    db.commit(state.take_changes()).unwrap();

    let range = range_of(&db, &reopened, 2, 0, 100, &ALL);
    range.check_shape(2, 0).unwrap();
    assert_eq!(range.items[1].changes, RangeChanges::None(NoProofs::NoList));
    check(&chain, range.items[1].clone()).expect("the block still checks");
    assert!(matches!(&range.items[1].body, RangeBody::Whole { txs, receipts: Some(_), .. } if txs.len() == 1));
    assert!(!entries(&range.items[0]).is_empty());
    assert!(!entries(&range.items[2]).is_empty() || entries(&range.items[3]).len() > 1);
}

#[test]
fn a_database_from_before_history_is_served_from_where_its_history_begins() {
    let (db, chain, _) = busy_chain();
    let tip = chain.tip().height;
    drop(chain);

    // It held neither the record of where history begins nor a single list.
    let state = State::new(db.clone());
    state.delete(Column::Meta, HISTORY_FROM_KEY.to_vec());

    for height in 1..=tip {
        state.delete(Column::History, history_list_key(height));
    }

    db.commit(state.take_changes()).unwrap();
    let mut chain = open(&db);
    assert_eq!(chain.history_from().unwrap(), tip, "it begins at the tip it opened on");

    let range = range_of(&db, &chain, 1, 0, 100, &ALL);
    range.check_shape(1, 0).unwrap();

    for item in &range.items[..tip as usize - 1] {
        assert_eq!(item.changes, RangeChanges::None(NoProofs::StatePruned { kept_from: tip }));
        assert!(matches!(item.body, RangeBody::Whole { .. }), "the block is given all the same");
    }

    assert_eq!(range.items[tip as usize - 1].changes, RangeChanges::None(NoProofs::NoList));
    assert!(range.items.iter().cloned().all(|item| check(&chain, item).is_ok()));

    // The first block it runs from there on has its list, and its proofs.
    let tx = transfer(&chain, &authority(), &stranger().public().address(), 1);
    advance(&mut chain, vec![tx]);
    assert!(!entries(&item_at(&db, &chain, tip + 1)).is_empty());
}

#[test]
fn what_a_block_changed_needs_nothing_written_past_what_a_node_keeps_already() {
    let (db, chain, _) = busy_chain();
    // A database as it stood before this existed reads the same: there is nothing new in it. Its
    // lists are history's, which name the state tree's nodes too, left out here.
    let list: HistoryList = read_value(&*db, Column::History, &history_list_key(1)).unwrap().unwrap();
    let proven = entries(&item_at(&db, &chain, 1)).len();
    let of_state = list.keys.iter().filter(|key| crate::db::STATE_COLUMNS.contains(&key.column)).count();

    assert_eq!(proven, of_state);
    assert!(list.keys.len() > of_state, "the tree's nodes are in the list, and left out");
}

#[test]
fn a_reorg_takes_a_blocks_list_with_it_and_the_new_branch_is_served() {
    let db = Arc::new(MemoryDb::default());
    let mut chain = Chain::open(db.clone(), &Genesis { finality: None, ..pow_genesis() }).unwrap();
    let genesis = chain.block_at(0).unwrap().unwrap();
    let (a, b) = (Address::Ed25519([1; 32]), Address::Ed25519([2; 32]));
    let a1 = mine_on(&chain, &genesis, Vec::new(), a);
    extended(chain.import(&a1, LATER));
    let answered_before = item_at(&db, &chain, 1);
    assert!(entries(&answered_before).iter().any(|entry| entry.key == crate::db::address_key(&a)));

    let b1 = mine_on(&chain, &genesis, Vec::new(), b);
    assert!(matches!(chain.import(&b1, LATER), Ok(Imported::Side)));
    let b2 = mine_on(&chain, &b1, Vec::new(), b);
    assert!(matches!(chain.import(&b2, LATER), Ok(Imported::Reorganized { .. })));

    let item = item_at(&db, &chain, 1);
    let checked = check(&chain, item).unwrap();
    assert_eq!(checked.block, b1.id());
    assert!(checked.entries().iter().any(|entry| entry.key == crate::db::address_key(&b)));
    assert!(!checked.entries().iter().any(|entry| entry.key == crate::db::address_key(&a)), "the miner of the block taken off is not in this one's list");

    // A client that checked the old branch's item finds it no longer fits.
    assert_eq!(check(&chain, answered_before).err(), Some(RangeError::WrongHeader));
}

#[test]
fn a_list_that_leaves_an_entry_out_still_checks_as_no_client_can_tell() {
    let (db, chain, _) = busy_chain();
    let mut item = item_at(&db, &chain, 1);
    let bob = crate::db::address_key(&Keypair::from_seed([1; 32]).public().address());
    let before = entries(&item).len();

    // The state root commits to the state, not to which entries a block changed: a node may leave
    // bob's account out, and its counts along with it. This test holds the limit in place: should
    // it ever fail, the limit is gone, and the docs that state it want changing.
    entries_mut(&mut item).retain(|entry| entry.key != bob);
    let RangeChanges::Proven { listed, .. } = &mut item.changes else { panic!("proven") };
    *listed -= 1;

    let checked = check(&chain, item).expect("every entry left checks, and nothing says one is missing");

    assert_eq!(checked.entries().len(), before - 1);
    assert!(!checked.entries().iter().any(|entry| entry.key == bob));

    // The same goes for a node that claims it holds no proofs at all.
    let mut item = item_at(&db, &chain, 1);
    item.changes = RangeChanges::None(NoProofs::NoList);
    assert!(check(&chain, item).is_ok());

    // Receipts, being under a root, cannot be left out so: only entries can.
    let mut item = item_at(&db, &chain, 1);
    parts(&mut item).receipts.pop();
    assert_eq!(check(&chain, item), Err(RangeError::ReceiptsRoot));
}

#[test]
fn any_bytes_at_all_are_read_without_panicking_or_passing_for_what_they_are_not() {
    let (db, chain, _) = busy_chain();
    let range = range_of(&db, &chain, 1, 0, 4, &ALL);
    let bytes = wincode::serialize(&range).unwrap();
    assert_eq!(decode_block_range(&bytes).unwrap(), range);

    for end in (0..bytes.len()).step_by(7) {
        assert_eq!(decode_block_range(&bytes[..end]), Err(RangeError::Undecodable), "cut at {end}");
    }

    // Every byte, one value at a time. Whatever decodes is checked: it may be refused or, where the
    // byte was in what nothing commits to, the counts or the cursor, let by; but it must not panic,
    // and what checks must be true of the chain.
    let mut changed_and_decoded = 0;
    let headers: Vec<SealedHeader> = (1..=4).map(|height| header_at(&chain, height)).collect();

    for at in 0..bytes.len() {
        for flip in [0x01, 0x80, 0xff] {
            let mut bent = bytes.clone();
            bent[at] ^= flip;

            let Ok(decoded) = decode_block_range(&bent) else {
                continue;
            };
            changed_and_decoded += 1;
            let _ = decoded.check_shape(1, 0);

            for item in decoded.items {
                let height = item.sealed.header.height;
                let Some(header) = height.checked_sub(1).and_then(|at| headers.get(at as usize)) else {
                    continue;
                };

                if let Ok(checked) = check_range_item(chain.hash(), header, item) {
                    let original = &range.items[height as usize - 1];

                    assert_eq!(&checked.sealed, header);
                    // A node may always hold back, and saying so is all it can do: what it does
                    // give must be the block's.
                    if let (RangeBody::Whole { txs, proofs, pool, receipts }, RangeBody::Whole { txs: real_txs, proofs: real_proofs, pool: real_pool, receipts: real_receipts }) = (&checked.body, &original.body) {
                        assert_eq!((txs, proofs, pool), (real_txs, real_proofs, real_pool), "byte {at}: a body that differs passed");
                        assert!(receipts.is_none() || receipts == real_receipts, "byte {at}: receipts that differ passed");
                    }

                    // A key changed may pass, where it too is absent from a small tree: that is
                    // no lie, as nothing checks but what the state at that height holds.
                    for entry in checked.entries() {
                        assert_eq!(state_at(&db, &chain, height).unwrap().get(entry.column, &entry.key).unwrap(), entry.value, "byte {at}: an entry the state does not hold passed");
                    }
                }
            }
        }
    }

    assert!(changed_and_decoded > 0, "some did decode, or this tested little");

    // Bytes that say a list runs on past what the bytes hold, at sizes that would take all memory.
    for claim in [u64::MAX, u64::MAX / 2, 1 << 40, 1 << 33] {
        let mut lying = bytes.clone();
        lying[8..16].copy_from_slice(&claim.to_le_bytes());
        assert_eq!(decode_block_range(&lying), Err(RangeError::Undecodable), "{claim}");
    }
}

/// What a block costs to send with its proofs and without, and to prove: what the docs quote. It
/// runs on RocksDB over a state of many accounts, whose depth is what a proof's size goes by, and is
/// ignored for how long it takes and what it writes. Set RANGE_MEASURE_DIR to where it may write, and
/// run it in release with --ignored --nocapture.
#[test]
#[ignore = "a measurement, not a check: see above"]
fn what_a_block_costs_with_its_proofs() {
    use crate::db::RocksDb;

    let dir = std::path::PathBuf::from(std::env::var("RANGE_MEASURE_DIR").expect("RANGE_MEASURE_DIR is where it may write")).join(format!("range-{}", std::process::id()));
    // As many as genesis's encoding holds.
    let accounts: u64 = 50_000;
    let me = authority().public().address();
    let mut allocations = vec![Allocation { address: me, amount: dev_funds() }];
    allocations.extend((0..accounts).map(|n| {
        let mut bytes = [1; 32];
        bytes[..8].copy_from_slice(&n.to_le_bytes());

        Allocation { address: Address::Ed25519(bytes), amount: u256::new(1) }
    }));
    // A reward, as a chain of value pays one, so an empty block changes what one does.
    let reward = Reward { initial: u256::new(1_000), halving_interval: None };
    let genesis = Genesis { allocations, reward, ..genesis() };
    let database = Arc::new(RocksDb::open(&dir).unwrap().unsynced());
    let db: Arc<dyn Database> = database.clone();
    let mut chain = Chain::open(db.clone(), &genesis).unwrap();
    let hash = chain.hash();
    println!("state of {accounts} accounts");

    let report = |name: &str, chain: &Chain, height: u64| {
        // Twice, as a first read fills the cache a second does not.
        let snapshot = db.snapshot();
        let started = Instant::now();
        let range = stored_block_range(snapshot.clone(), hash, height, 0, 1, &ALL).unwrap().unwrap();
        let first = started.elapsed();
        let started = Instant::now();
        let again = stored_block_range(snapshot.clone(), hash, height, 0, 1, &ALL).unwrap().unwrap();
        let second = started.elapsed();
        assert_eq!(range, again);
        let item = range.items.into_iter().next().unwrap();
        let with_proofs = wincode::serialized_size(&item).unwrap();
        let without = wincode::serialized_size(&RangeItem { changes: RangeChanges::None(NoProofs::NoList), ..item.clone() }).unwrap();
        let block_alone = wincode::serialized_size(&chain.block_at(height).unwrap().unwrap()).unwrap();
        let count = entries(&item).len() as u64;
        let depth = entries(&item).iter().map(|entry| entry.proof.siblings.len() as u64).sum::<u64>() / count.max(1);
        let listed: HistoryList = read_value(&*db, Column::History, &history_list_key(height)).unwrap().unwrap();
        let of_state: Vec<&HistoryKey> = listed.keys.iter().filter(|key| crate::db::STATE_COLUMNS.contains(&key.column)).collect();
        let state_list_bytes: u64 = of_state.iter().map(|key| wincode::serialized_size(*key).unwrap()).sum();
        let all_list_bytes = wincode::serialized_size(&listed).unwrap();
        let started = Instant::now();
        let header = header_at(chain, height);
        let checked = check_range_item(hash, &header, item).unwrap();
        let checking = started.elapsed();

        println!(
            "{name}: block {block_alone} B, item without proofs {without} B, with {with_proofs} B ({count} entries, {} B each, depth {depth}); built {first:?} then {second:?}, {:?} an entry; checked {checking:?}; list of state keys {state_list_bytes} B, history's list {all_list_bytes} B ({} keys)",
            (with_proofs - without) / count.max(1),
            second / count.max(1) as u32,
            listed.keys.len(),
        );
        drop(checked);
    };

    advance(&mut chain, Vec::new());
    report("an empty block", &chain, chain.tip().height);

    let mut nonce = 0;

    for size in [100u64, 2_000, 10_000] {
        let txs: Vec<SignedTx> = (0..size)
            .map(|n| {
                let to = Keypair::from_seed({
                    let mut seed = [3; 32];
                    seed[..8].copy_from_slice(&(nonce + n).to_le_bytes());
                    seed
                })
                .public()
                .address();

                SignedTx::sign(Tx::Transfer { from: me, to, amount: Amount::U128(1) }, nonce + n, FeeOffer::NONE, &authority())
            })
            .collect();
        nonce += size;
        advance(&mut chain, txs);
        report(&format!("{size} transfers to new accounts"), &chain, chain.tip().height);
    }

    let contract = contract_address(&me, nonce, hash);
    let deploy = SignedTx::sign(Tx::Deploy { code: Code::Lua(KEEPER.to_string()) }, nonce, FeeOffer::NONE, &authority());
    assert_eq!(advance(&mut chain, vec![deploy])[0].result, Ok(Effect::Deployed(contract)));
    report("a deploy", &chain, chain.tip().height);
    nonce += 1;

    for size in [100u64, 1_000] {
        let txs: Vec<SignedTx> = (0..size).map(|n| call(contract, "keep", vec![text(&format!("key-{}", nonce + n)), text("a value of a few dozen bytes kept by a contract")], nonce + n)).collect();
        nonce += size;
        assert!(advance(&mut chain, txs).iter().all(|receipt| receipt.result.is_ok()));
        report(&format!("{size} contract calls, each writing a key"), &chain, chain.tip().height);
    }

    // What an answer costs within the limits it is served by, from the largest block on.
    let (mut at, mut answers, mut biggest) = (RangeCursor { height: 3, entry: 0 }, 0, 0);
    let started = Instant::now();

    while at.height <= 4 {
        let range = stored_block_range(db.snapshot(), hash, at.height, at.entry, 1, &RangeLimits::SERVED).unwrap().unwrap();
        biggest = biggest.max(wincode::serialized_size(&range).unwrap());
        answers += 1;

        match range.next {
            Some(next) => at = next,
            None => break,
        }
    }

    println!("the blocks of 2,000 and 10,000 transfers, as served: {answers} answers, {:?} an answer, the largest {biggest} B", started.elapsed() / answers);
    drop(chain);
    drop(db);
    drop(database);
    std::fs::remove_dir_all(&dir).unwrap();
}
