use {
    super::*,
    crate::api::{Api, EventQuery, Reply},
    crate::light::Light,
    crate::model::{BlockDetail, EventsPage, Page, Receipt, TxsPage},
    crate::store::Store,
    crate::views::{Ctx, Look, Settings},
    chainv4::crypto::keypair::Keypair,
    chainv4::db::{MemoryDb, State, StateTree, address_key},
    chainv4::light::{NoProofs, ProvenEntry, RangeBody, RangeChanges, RangeCursor},
    chainv4::structs::account::Account,
    chainv4::structs::address::Address,
    chainv4::structs::amount::Amount,
    chainv4::structs::block::{Block, BlockHeader, Seal, SealedHeader},
    chainv4::structs::event::{EVM_LOG, Event},
    chainv4::structs::hash::Hash,
    chainv4::structs::receipt::{CommittedOutcome, CommittedReceipt, receipts_root},
    chainv4::structs::tx::{FeeOffer, SignedTx, TxCallArgumentType},
    chainv4_ui::net::Answer,
    chainv4_ui::shelf::{MemoryShelf, Shelf},
    egui::Ui,
};

const HASH: HashType = HashType::Blake3_256;

/// A phone upright, a phone on its side, and a desktop.
pub const SCREENS: [(f32, f32); 3] = [(360.0, 740.0), (740.0, 360.0), (1200.0, 800.0)];

/// One block's making.
#[derive(Default)]
struct Making {
    txs: Vec<(SignedTx, CommittedReceipt)>,
    /// Entries it lists, proven, as a node sends them.
    writes: Vec<(Column, Vec<u8>, Vec<u8>)>,
    no_receipts: bool,
    header_only: bool,
}

/// A chain of real headers and bodies, in the range's real encoding: what a node sends, and a light
/// client's view of it.
struct Fixture {
    items: Vec<RangeItem>,
    final_height: u64,
    lowest: u64,
    state: State,
    root: Hash,
}

impl Followed for Fixture {
    fn hash(&self) -> HashType {
        HASH
    }

    fn sealed_at(&self, height: u64) -> Option<&SealedHeader> {
        (height >= self.lowest).then(|| self.items.get(height as usize).map(|item| &item.sealed)).flatten()
    }

    fn final_height(&self) -> u64 {
        self.final_height
    }

    fn lowest(&self) -> u64 {
        self.lowest
    }
}

impl Fixture {
    fn new() -> Fixture {
        let mut fixture = Fixture { items: Vec::new(), final_height: 0, lowest: 0, state: State::new(Arc::new(MemoryDb::default())), root: Hash::zero(HASH) };

        fixture.push(Making::default());
        fixture
    }

    fn push(&mut self, making: Making) {
        let height = self.items.len() as u64;

        for (column, key, value) in &making.writes {
            self.state.put(*column, key.clone(), value.clone());
        }

        let tree = StateTree::new(&self.state, HASH);

        // A tree walked only as it changes: thousands of blocks here write nothing.
        if !making.writes.is_empty() || height == 0 {
            self.root = tree.follow().unwrap();
        }

        let (txs, receipts): (Vec<SignedTx>, Vec<CommittedReceipt>) = making.txs.into_iter().unzip();
        let header = BlockHeader {
            parent: self.items.last().map_or_else(|| Hash::zero(HASH), |item| item.sealed.id()),
            height,
            timestamp: height * 5_000,
            producer: Address::Ed25519([3; 32]),
            difficulty: 0,
            tx_root: Block::tx_root(HASH, &txs),
            proof_root: Block::proof_root(HASH, &[]),
            pool_root: Block::pool_root(HASH, None),
            receipts_root: receipts_root(HASH, &receipts),
            state_root: self.root,
            gas_used: 0,
            base_price: u256::from(7u64),
        };
        let entries: Vec<ProvenEntry> = making.writes.iter().map(|(column, key, value)| ProvenEntry { column: *column, key: key.clone(), value: Some(value.clone()), proof: tree.prove(*column, key).unwrap() }).collect();
        let body = match making.header_only {
            true => RangeBody::HeaderOnly,
            false => RangeBody::Whole { txs, proofs: Vec::new(), pool: None, receipts: (!making.no_receipts).then_some(receipts) },
        };
        let changes = match height {
            0 => RangeChanges::None(NoProofs::NoList),
            _ => RangeChanges::Proven { from: 0, listed: entries.len() as u32, entries },
        };

        self.items.push(RangeItem { sealed: SealedHeader { header, seal: Seal::None }, body, changes });
    }

    fn empty_to(&mut self, height: u64) {
        while (self.items.len() as u64) <= height {
            self.push(Making::default());
        }

        self.final_height = height;
    }

    fn range(&self, asked: Asked) -> BlockRange {
        let tip = self.items.len() as u64 - 1;
        let items: Vec<RangeItem> = (asked.from..(asked.from + asked.count).min(tip + 1)).map(|height| self.items[height as usize].clone()).collect();
        let next = items.last().map(|item| item.sealed.header.height + 1).filter(|after| *after <= tip).map(|height| RangeCursor { height, entry: 0 });

        BlockRange { tip, items, next }
    }

    fn answer(&self, asked: Asked) -> Came {
        answered(&self.range(asked))
    }

    fn id(&self, height: u64) -> String {
        self.items[height as usize].sealed.id().to_string()
    }

    /// Everything final read: what asks has nothing left to ask.
    fn read_all(&self, reading: &mut Reading) {
        self.read_with(reading, |asked, fixture| fixture.answer(asked));
    }

    fn read_with(&self, reading: &mut Reading, answer: impl Fn(Asked, &Fixture) -> Came) {
        let mut rounds = 0;

        while let Some(asked) = reading.go(self, 0.0) {
            reading.came(asked, answer(asked, self), self, 0.0);
            rounds += 1;
            assert!(rounds < 100_000, "it never stopped asking");
        }
    }
}

fn answered(range: &BlockRange) -> Came {
    Came::of(Ok(Answer { ok: true, status: 200, status_text: "OK".to_string(), bytes: wincode::serialize(range).unwrap(), retry_after: None }))
}

fn key(seed: u8) -> Keypair {
    Keypair::from_seed([seed; 32])
}

fn alice() -> Address {
    key(1).public().address()
}

fn bob() -> Address {
    key(2).public().address()
}

fn token() -> Address {
    Address::Contract([9; 32])
}

const GENESIS: &str = "0xgenesis";

/// A reading opened on a shelf a test can see into.
fn on_shelf(shelf: &Arc<MemoryShelf>, fixture: &Fixture) -> Reading {
    let mut reading = Reading::new(Keeper::new(shelf.clone()), true);

    assert!(reading.open(GENESIS, HASH, fixture, 0.0), "a shelf in memory answers at once");

    reading
}

fn opened(fixture: &Fixture) -> Reading {
    on_shelf(&Arc::new(MemoryShelf::default()), fixture)
}

/// What transactions here are signed for: reading a block checks its roots, and no signature.
fn chain() -> Hash {
    Hash::zero(HASH)
}

fn transfer(from: u8, to: Address, amount: u64, nonce: u64) -> (SignedTx, CommittedReceipt) {
    let signed = SignedTx::sign(Tx::Transfer { from: key(from).public().address(), to, amount: Amount::U128(u128::from(amount)) }, nonce, FeeOffer::NONE, &key(from), &chain());
    let receipt = CommittedReceipt::new(signed.id(HASH), 0, 21_000, u256::from(21_000u64 * 9), CommittedOutcome::Done, Vec::new(), Vec::new());

    (signed, receipt)
}

fn call_with_events(from: u8, nonce: u64, events: Vec<Event>, outcome: CommittedOutcome) -> (SignedTx, CommittedReceipt) {
    let tx = Tx::ContractCall { to: token(), func: Hash::zero(HashType::Xxh3_64), amount: Amount::U128(0), data: vec![TxCallArgumentType::Address(bob())] };
    let signed = SignedTx::sign(tx, nonce, FeeOffer::NONE, &key(from), &chain());
    let receipt = CommittedReceipt::new(signed.id(HASH), 5, 90_000, u256::from(90_000u64 * 9), outcome, events, Vec::new());

    (signed, receipt)
}

fn mention(name: &str, of: Address) -> Event {
    Event { contract: token(), name: name.to_string(), data: vec![TxCallArgumentType::Address(of)] }
}

fn evm_log(contract: [u8; 20], topics: Vec<[u8; 32]>) -> Event {
    Event { contract: Address::Evm(contract), name: EVM_LOG.to_string(), data: topics.iter().map(|topic| TxCallArgumentType::Bytes(topic.to_vec())).chain([TxCallArgumentType::Bytes(vec![1])]).collect() }
}

fn padded(address: [u8; 20]) -> [u8; 32] {
    let mut topic = [0; 32];
    topic[12..].copy_from_slice(&address);

    topic
}

/// A chain with something in it: a transfer at 5, a call with events at 6, a call that failed at 7,
/// and an EVM log naming alice in its topics at 8, then empty blocks, all final up to 12.
fn busy_chain() -> Fixture {
    let mut fixture = Fixture::new();

    fixture.empty_to(4);
    fixture.push(Making { txs: vec![transfer(1, bob(), 100, 0)], writes: vec![(Column::Accounts, address_key(&alice()), wincode::serialize(&Account { nonce: 1, balance: u256::from(900u64) }).unwrap())], ..Making::default() });
    fixture.push(Making { txs: vec![call_with_events(1, 1, vec![mention("Paid", bob()), mention("Other", alice())], CommittedOutcome::Returned(vec![TxCallArgumentType::Address(bob())]))], ..Making::default() });
    fixture.push(Making { txs: vec![call_with_events(2, 0, Vec::new(), CommittedOutcome::Failed)], ..Making::default() });
    fixture.push(Making { txs: vec![call_with_events(2, 1, vec![evm_log([5; 20], vec![[7; 32], padded(chainv4::abi::evm_address(&alice()))])], CommittedOutcome::Done)], ..Making::default() });
    fixture.empty_to(12);

    fixture
}

fn from_ok<T: serde::de::DeserializeOwned>(answer: Option<Result<Value, Failure>>) -> T {
    serde_json::from_value(answer.expect("the index has a say").expect("it answers")).expect("it reads as a node's answer does")
}

fn refused(answer: Option<Result<Value, Failure>>) -> Failure {
    answer.expect("the index has a say").expect_err("it does not answer")
}

fn events_of(reading: &Reading, query: EventQuery) -> EventsPage {
    from_ok(reading.answer(&Ask::Events { query, cursor: None }, 12))
}

fn txs_of(reading: &Reading, address: &Address, query: &str) -> TxsPage {
    from_ok(reading.answer(&Ask::AccountTxs { address: address.to_string(), query: query.to_string(), cursor: None }, 12))
}

/// Starts at height `from` and reads everything final.
fn read_busy_chain_from(from: u64) -> (Fixture, Reading) {
    let fixture = busy_chain();
    let mut reading = opened(&fixture);

    reading.further_back(from);
    fixture.read_all(&mut reading);

    (fixture, reading)
}

#[test]
fn a_reading_starts_at_the_final_block_and_goes_up_as_more_is_final_with_no_history_read() {
    let mut fixture = busy_chain();
    let mut reading = opened(&fixture);
    let asked = reading.go(&fixture, 0.0).unwrap();

    assert_eq!(asked, Asked { from: 12, entry: 0, count: 1 }, "the final block, and nothing older");
    reading.came(asked, fixture.answer(asked), &fixture, 0.0);
    assert_eq!(reading.go(&fixture, 0.0), None, "no history asked for, none read");

    let standing = reading.standing().unwrap();
    assert_eq!((standing.high, standing.known_from, standing.blocks), (12, 12, 1));
    assert!(standing.downloaded > 0);

    fixture.empty_to(15);
    fixture.read_all(&mut reading);

    let standing = reading.standing().unwrap();
    assert_eq!((standing.high, standing.known_from, standing.blocks), (15, 12, 4));
}

#[test]
fn what_was_read_is_answered_end_to_end_in_the_form_a_node_writes_it() {
    let (fixture, reading) = read_busy_chain_from(0);
    let standing = reading.standing().unwrap();
    assert_eq!((standing.known_from, standing.high), (0, 12));

    // The feed and pages of blocks, newest first.
    let page: Page = from_ok(reading.answer(&Ask::Blocks { before: None, limit: 3 }, 12));
    assert_eq!(page.blocks.iter().map(|block| block.height).collect::<Vec<_>>(), [12, 11, 10]);
    let page: Page = from_ok(reading.answer(&Ask::Blocks { before: Some(2), limit: 100 }, 12));
    assert_eq!(page.blocks.iter().map(|block| block.height).collect::<Vec<_>>(), [1, 0]);

    // A block by height and by id, with what its transactions did.
    let by_height: BlockDetail = from_ok(reading.answer(&Ask::Block("5".to_string()), 12));
    let by_id: BlockDetail = from_ok(reading.answer(&Ask::Lookup(fixture.id(5)), 12));
    assert_eq!(by_height.summary, by_id.summary);
    assert_eq!((by_height.summary.height, by_height.summary.tx_count, by_height.summary.main), (5, 1, Some(true)));
    assert_eq!(by_height.txs[0].kind(), "transfer");
    assert_eq!(by_height.txs[0].receipt.as_ref().unwrap().status, "done");
    assert_eq!(by_height.txs[0].sender, alice().to_string());

    // A transaction by its id: final, where it landed, and what it did.
    let id = by_height.txs[0].id.clone();
    let receipt: Receipt = from_ok(reading.answer(&Ask::Tx(id), 12));
    assert_eq!((receipt.height, receipt.index, receipt.confirmations, receipt.finalized), (5, 0, 7, true));
    assert_eq!(receipt.outcome.fee, (21_000u64 * 9).to_string());

    // A failed one says that it failed, and that a block does not say why.
    let failed: BlockDetail = from_ok(reading.answer(&Ask::Block("7".to_string()), 12));
    let outcome = failed.txs[0].receipt.as_ref().unwrap();
    assert_eq!(outcome.status, "failed");
    assert!(outcome.error.as_deref().unwrap().contains("not why"), "{:?}", outcome.error);
    assert!(outcome.trace.hops.is_empty(), "a block does not commit to them");
}

#[test]
fn what_touched_an_address_comes_from_what_was_read_with_its_filters() {
    let (_, reading) = read_busy_chain_from(0);
    let of_alice = txs_of(&reading, &alice(), "");

    assert_eq!(of_alice.items.iter().map(|found| found.height).collect::<Vec<_>>(), [6, 5], "newest first");
    assert_eq!(of_alice.indexed_from, 0);
    assert_eq!(of_alice.applied, None);

    let of_bob = txs_of(&reading, &bob(), "");
    assert_eq!(of_bob.items.iter().map(|found| found.height).collect::<Vec<_>>(), [8, 7, 6, 5], "sent, or named as an argument or a payee");

    let filtered = txs_of(&reading, &bob(), "kind=transfer");
    assert_eq!(filtered.items.iter().map(|found| found.height).collect::<Vec<_>>(), [5]);
    assert!(filtered.applied.is_some(), "the page knows it was filtered");

    assert_eq!(txs_of(&reading, &bob(), "direction=out").items.iter().map(|found| found.height).collect::<Vec<_>>(), [8, 7]);
    assert_eq!(txs_of(&reading, &bob(), "direction=in").items.iter().map(|found| found.height).collect::<Vec<_>>(), [6, 5]);
    assert_eq!(txs_of(&reading, &alice(), &format!("counterparty={}", bob())).items.iter().map(|found| found.height).collect::<Vec<_>>(), [5]);
    assert_eq!(txs_of(&reading, &bob(), &format!("token={}", token())).items.iter().map(|found| found.height).collect::<Vec<_>>(), [8, 7, 6]);
    assert_eq!(txs_of(&reading, &bob(), "from=30000&until=35000").items.iter().map(|found| found.height).collect::<Vec<_>>(), [7, 6]);
    assert_eq!(txs_of(&reading, &Address::Ed25519([200; 32]), "").items.len(), 0, "nobody in what was read");
}

#[test]
fn a_page_of_transactions_goes_on_from_its_cursor_with_nothing_twice() {
    let mut fixture = Fixture::new();

    for nonce in 0..120 {
        fixture.push(Making { txs: vec![transfer(1, bob(), 1, nonce)], ..Making::default() });
    }

    fixture.final_height = 120;
    let mut reading = opened(&fixture);
    reading.further_back(0);
    fixture.read_all(&mut reading);

    let mut seen = Vec::new();
    let mut cursor = None;

    loop {
        let page: TxsPage = from_ok(reading.answer(&Ask::AccountTxs { address: alice().to_string(), query: String::new(), cursor: cursor.clone() }, 120));
        seen.extend(page.items.iter().map(|found| found.height));

        match page.next {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }

    assert_eq!(seen, (1..=120).rev().collect::<Vec<u64>>());
}

#[test]
fn events_are_found_by_contract_name_address_and_topic_among_what_was_read() {
    let (_, reading) = read_busy_chain_from(0);
    let names = |page: EventsPage| page.items.iter().map(|event| event.name.clone()).collect::<Vec<_>>();

    assert_eq!(names(events_of(&reading, EventQuery::contract(&token().to_string(), None))), ["Other", "Paid"], "newest first, the later in a receipt before the earlier");
    assert_eq!(names(events_of(&reading, EventQuery::contract(&token().to_string(), Some("Paid")))), ["Paid"]);
    assert_eq!(names(events_of(&reading, EventQuery::mentioning(&bob().to_string()))), ["Paid"]);
    assert_eq!(names(events_of(&reading, EventQuery::mentioning(&alice().to_string()))), [EVM_LOG, "Other"], "in what an event holds, and in an EVM log's topics");
    assert_eq!(names(events_of(&reading, EventQuery::topic(&to_hex(&[7; 32]), None))), [EVM_LOG]);
    assert_eq!(names(events_of(&reading, EventQuery::topic(&to_hex(&[7; 32]), Some(&token().to_string())))), Vec::<String>::new(), "another contract's");

    let page = events_of(&reading, EventQuery::contract(&token().to_string(), None).limited(1));
    let first = page.items[0].clone();
    let next: EventsPage = from_ok(reading.answer(&Ask::Events { query: EventQuery::contract(&token().to_string(), None).limited(1), cursor: page.next.clone() }, 12));

    assert_eq!(page.indexed_from, 0);
    assert!(page.topics);
    assert_eq!((first.name.as_str(), next.items[0].name.as_str()), ("Other", "Paid"), "a page at a time");
}

#[test]
fn what_is_outside_what_was_read_says_so_and_what_is_above_it_is_not_taken_for_older() {
    let (fixture, reading) = read_busy_chain_from(9);
    let standing = reading.standing().unwrap();
    assert_eq!(standing.known_from, 9);

    let below = refused(reading.answer(&Ask::Block("5".to_string()), 12));
    assert_eq!(below.status, 404);
    assert!(below.message.contains("knows from block 9"), "{}", below.message);

    let above = refused(reading.answer(&Ask::Block("99".to_string()), 12));
    assert!(above.message.contains("has read up to block 12") && above.message.contains("final"), "{}", above.message);

    let by_id = refused(reading.answer(&Ask::Block(fixture.id(5)), 12));
    assert!(by_id.message.contains("from 9 to 12"), "{}", by_id.message);

    let (signed, _) = transfer(1, bob(), 100, 0);
    let tx = refused(reading.answer(&Ask::Tx(signed.id(HASH).to_string()), 12));
    assert_eq!(tx.status, 404);
    assert!(tx.message.contains("9 to 12"), "{}", tx.message);

    assert_eq!(txs_of(&reading, &alice(), "").items.len(), 0, "what touched it below is not known");
    assert_eq!(txs_of(&reading, &alice(), "").indexed_from, 9, "and the page says from where");
    assert_eq!(events_of(&reading, EventQuery::mentioning(&bob().to_string())).items.len(), 0);
}

#[test]
fn reading_further_back_leaves_what_was_read_and_says_from_what_height_it_knows_as_it_goes() {
    let fixture = busy_chain();
    let mut reading = opened(&fixture);

    fixture.read_all(&mut reading);
    assert_eq!(reading.standing().unwrap().known_from, 12);
    assert_eq!(reading.further_back(12), Further::Already(12));
    assert_eq!(reading.further_back(100), Further::Already(12));
    assert_eq!(reading.further_back(4), Further::Moved);
    assert_eq!(reading.standing().unwrap().known_from, 12, "nothing older counts until it stands");
    assert_eq!(reading.standing().unwrap().left_below, 8);

    let asked = reading.go(&fixture, 0.0).unwrap();
    assert_eq!(asked, Asked { from: 4, entry: 0, count: 8 }, "a stretch at once, going down");
    reading.came(asked, fixture.answer(asked), &fixture, 0.0);
    fixture.read_all(&mut reading);

    let standing = reading.standing().unwrap();
    assert_eq!((standing.known_from, standing.high, standing.blocks), (4, 12, 9));
    assert!(reading.answer(&Ask::Block("5".to_string()), 12).unwrap().is_ok());
    assert!(reading.answer(&Ask::Block("3".to_string()), 12).unwrap().is_err());
}

#[test]
fn hostile_answers_leave_the_index_as_it_was_and_stop_the_reading_saying_why() {
    let fixture = busy_chain();
    let mut reading = opened(&fixture);

    fixture.read_all(&mut reading);
    let before = reading.standing().unwrap();
    assert_eq!(reading.further_back(0), Further::Moved);

    // A transaction changed in a block going down, whose header stays what links up.
    let mut forged = fixture.range(Asked { from: 0, entry: 0, count: 12 });
    let (signed, receipt) = transfer(2, alice(), 1_000_000, 9);

    if let RangeBody::Whole { txs, receipts, .. } = &mut forged.items[5].body {
        txs[0] = signed;
        receipts.as_mut().unwrap()[0] = receipt;
    }

    let asked = reading.go(&fixture, 0.0).unwrap();
    reading.came(asked, answered(&forged), &fixture, 0.0);

    let standing = reading.standing().unwrap();
    assert_eq!((standing.blocks, standing.known_from, standing.high), (before.blocks, before.known_from, before.high), "nothing of it stands");
    assert!(standing.trouble.as_deref().unwrap().contains("do not fit"), "{:?}", standing.trouble);
    assert!(reading.answer(&Ask::Block("5".to_string()), 12).unwrap().is_err());
    assert_eq!(reading.go(&fixture, 0.0), None, "it asks nothing until told");

    // Blocks that check by themselves, from another chain: a stretch that does not link up.
    reading.retry();
    let mut other = Fixture::new();
    other.push(Making { txs: vec![transfer(2, alice(), 1, 0)], ..Making::default() });
    other.empty_to(12);
    let asked = reading.go(&fixture, 0.0).unwrap();
    reading.came(asked, other.answer(asked), &fixture, 0.0);

    assert!(reading.standing().unwrap().trouble.as_deref().unwrap().contains("link up"), "{:?}", reading.standing().unwrap().trouble);
    assert_eq!(reading.standing().unwrap().blocks, before.blocks);

    // And bytes that do not read, going up.
    let mut fixture = busy_chain();
    let mut reading = opened(&fixture);
    fixture.read_all(&mut reading);
    fixture.empty_to(13);
    let asked = reading.go(&fixture, 0.0).unwrap();
    let garbage = Came::of(Ok(Answer { ok: true, status: 200, status_text: "OK".to_string(), bytes: vec![1, 2, 3], retry_after: None }));
    let blocks = reading.standing().unwrap().blocks;

    reading.came(asked, garbage, &fixture, 0.0);
    assert_eq!(reading.standing().unwrap().blocks, blocks);
    assert!(reading.standing().unwrap().trouble.is_some());
}

#[test]
fn a_node_without_the_route_stops_the_reading_and_nothing_else_is_asked_in_its_place() {
    let fixture = busy_chain();
    let mut reading = opened(&fixture);
    let asked = reading.go(&fixture, 0.0).unwrap();

    reading.came(asked, Came::Refused { status: 400, message: "not a block".to_string(), bytes: 0 }, &fixture, 0.0);

    assert!(reading.standing().unwrap().trouble.as_deref().unwrap().contains("does not serve checked blocks"));
    assert_eq!(reading.go(&fixture, 0.0), None);
}

/// A chain of so many empty blocks, every one final, with a transfer at the given heights.
fn long_chain(height: u64, with_txs: &[u64]) -> Fixture {
    let mut fixture = Fixture::new();

    while (fixture.items.len() as u64) <= height {
        let at = fixture.items.len() as u64;
        let txs = if with_txs.contains(&at) { vec![transfer(1, bob(), at, at)] } else { Vec::new() };

        fixture.push(Making { txs, ..Making::default() });
    }

    fixture.final_height = height;

    fixture
}

#[test]
fn past_its_cap_the_oldest_go_a_bucket_at_a_time_and_the_height_it_knows_from_moves_up_to_say_so() {
    let mut fixture = long_chain(3 * BUCKET, &[10, 2 * BUCKET + 10]);
    let shelf = Arc::new(MemoryShelf::default());
    let most = weight_of_blocks(&fixture, BUCKET * 5 / 2);
    let mut reading = on_shelf(&shelf, &fixture);
    reading.most = most;
    reading.further_back(0);

    // Going down it stops once it fills, and says so.
    fixture.final_height = BUCKET + 100;
    fixture.read_all(&mut reading);
    let standing = reading.standing().unwrap();
    assert_eq!((standing.known_from, standing.left_below), (0, 0));
    assert!(standing.bytes <= most);

    let (signed, _) = transfer(1, bob(), 10, 10);
    let old = signed.id(HASH).to_string();
    assert!(reading.answer(&Ask::Tx(old.clone()), BUCKET).unwrap().is_ok());
    assert_eq!(txs_of(&reading, &alice(), "").items.len(), 1);

    reading.save(0.0);
    assert!(shelf.held().iter().any(|(name, _)| name == &bucket_name(GENESIS, 0)));

    // Going up, the oldest are let go.
    fixture.final_height = 3 * BUCKET;
    fixture.read_all(&mut reading);

    let standing = reading.standing().unwrap();
    assert!(standing.bytes <= most, "{} of at most {most}", standing.bytes);
    assert_eq!(standing.known_from % BUCKET, 0, "a bucket at a time: {}", standing.known_from);
    assert!(standing.known_from >= BUCKET, "{}", standing.known_from);
    assert_eq!(standing.cut_below, Some(standing.known_from));
    assert_eq!(standing.high, 3 * BUCKET, "what is newest stays");
    assert!(standing.blocks < 3 * BUCKET);

    // What went is said to be outside, and found no more, however it is looked for.
    assert!(reading.answer(&Ask::Block("10".to_string()), BUCKET).unwrap().unwrap_err().message.contains(&format!("knows from block {}", standing.known_from)));
    assert!(reading.answer(&Ask::Tx(old), 3 * BUCKET).unwrap().is_err());
    assert_eq!(txs_of(&reading, &alice(), "").items.iter().map(|found| found.height).collect::<Vec<_>>(), [2 * BUCKET + 10], "the newer one stays");
    assert_eq!(txs_of(&reading, &alice(), "").indexed_from, standing.known_from);

    // Its file goes too.
    assert!(!shelf.held().iter().any(|(name, _)| name == &bucket_name(GENESIS, 0)), "written over nothing");
}

/// What so many of a fixture's blocks weigh, as kept.
fn weight_of_blocks(fixture: &Fixture, count: u64) -> u64 {
    fixture.items.iter().take(count as usize).map(index::weight).sum()
}

#[test]
fn with_no_room_it_will_not_read_further_back_and_says_so_and_never_eats_what_it_just_read() {
    let fixture = long_chain(3 * BUCKET, &[]);
    let mut reading = opened(&fixture);
    reading.most = weight_of_blocks(&fixture, 3 * BUCKET / 2);
    fixture.read_all(&mut reading);
    assert_eq!(reading.further_back(2 * BUCKET), Further::Moved);
    fixture.read_all(&mut reading);

    assert_eq!(reading.standing().unwrap().known_from, 2 * BUCKET);

    // Told to go on to genesis, it stops when full: what was newest stays.
    assert_eq!(reading.further_back(0), Further::Moved);
    fixture.read_all(&mut reading);
    let standing = reading.standing().unwrap();

    assert!(standing.known_from > 0 && standing.left_below == 0, "{standing:?}");
    assert_eq!(standing.high, 3 * BUCKET);
    assert!(reading.answer(&Ask::Block((3 * BUCKET).to_string()), 3 * BUCKET).unwrap().is_ok());
    assert_eq!(standing.cut_below, Some(standing.known_from));
    assert_eq!(reading.further_back(0), Further::Full, "and it says there is no room for more");
}

#[test]
fn a_smaller_cap_lets_the_oldest_blocks_go_at_once_and_says_from_where_it_knows() {
    let fixture = long_chain(3 * BUCKET, &[]);
    let mut reading = opened(&fixture);
    reading.further_back(0);
    fixture.read_all(&mut reading);
    let (held, most) = reading.bytes_kept();

    assert_eq!((most, reading.standing().unwrap().known_from), (MOST_BYTES, 0), "all of it, under the usual cap");

    reading.keep_most(weight_of_blocks(&fixture, 3 * BUCKET / 2));
    let standing = reading.standing().unwrap();

    assert!(reading.bytes_kept().0 <= weight_of_blocks(&fixture, 3 * BUCKET / 2) && reading.bytes_kept().0 < held);
    assert!(standing.known_from > 0 && standing.cut_below == Some(standing.known_from), "{standing:?}");
    assert_eq!(standing.high, 3 * BUCKET, "the newest stay");

    // Raised again, nothing comes back by itself, and nothing more goes.
    reading.keep_most(MOST_BYTES);

    assert_eq!(reading.standing().unwrap().known_from, standing.known_from);
}

#[test]
fn what_a_block_keeps_is_what_the_pages_need_and_what_it_weighs_is_measured() {
    let mut fixture = Fixture::new();

    // What a node sends of an empty block: its producer's account changed, with a proof.
    for at in 0..2_000u64 {
        let account = Account { nonce: at, balance: u256::from(at) };
        let mut writes = vec![(Column::Accounts, address_key(&Address::Ed25519([3; 32])), wincode::serialize(&account).unwrap())];

        // A state like a chain's, for proofs of a chain's length.
        writes.push((Column::Accounts, address_key(&Address::Ed25519([(at % 251) as u8; 32])), wincode::serialize(&account).unwrap()));
        fixture.push(Making { writes, ..Making::default() });
    }

    let sent: u64 = fixture.items[1_900..2_000].iter().map(|item| wincode::serialized_size(item).unwrap()).sum::<u64>() / 100;
    let kept: u64 = fixture.items[1_900..2_000].iter().map(|item| RangeItem { sealed: item.sealed.clone(), body: item.body.clone(), changes: RangeChanges::None(NoProofs::NoList) }).map(|item| index::weight(&item)).sum::<u64>() / 100;

    // Seals here are empty: a real one adds its signature.
    let sealed = kept + 65;
    let days = |most: u64| most / (sealed * 17_280);

    println!("an empty block: {sent} bytes sent with its proofs, {kept} kept, {sealed} with a signature; {} days in 128 MB, {} in 32 MB", days(128_000_000), days(32_000_000));
    assert!(sent > 2 * kept, "the proofs are what weigh: {sent} sent, {kept} kept");
    assert!(sealed <= 400, "{sealed}");
    assert!(days(128_000_000) >= 18 && days(32_000_000) >= 4, "as the setting says");
}

#[test]
fn a_block_kept_answers_as_it_came_and_its_entries_are_gone() {
    let (_, reading) = read_busy_chain_from(0);
    let item = reading.open.as_ref().unwrap().index.at(5).unwrap();

    assert_eq!(item.changes, RangeChanges::None(NoProofs::NoList));
    assert!(matches!(&item.body, RangeBody::Whole { txs, receipts: Some(_), .. } if txs.len() == 1));
}

#[test]
fn a_reading_put_away_comes_back_and_goes_on_from_where_it_was_asking_for_nothing_again() {
    let mut fixture = long_chain(BUCKET + 50, &[BUCKET - 5, BUCKET + 5]);
    let shelf = Arc::new(MemoryShelf::default());
    let mut reading = on_shelf(&shelf, &fixture);

    reading.further_back(BUCKET - 10);
    fixture.read_all(&mut reading);
    let before = reading.standing().unwrap();
    let page = txs_of(&reading, &alice(), "");
    reading.close(0.0);

    let mut again = on_shelf(&shelf, &fixture);
    let standing = again.standing().unwrap();

    assert_eq!((standing.high, standing.known_from, standing.blocks, standing.bytes, standing.downloaded), (before.high, before.known_from, before.blocks, before.bytes, before.downloaded));
    assert_eq!(again.go(&fixture, 0.0), None, "nothing to read again");
    assert_eq!(txs_of(&again, &alice(), "").items.len(), page.items.len());
    assert!(again.answer(&Ask::Block((BUCKET - 5).to_string()), BUCKET + 50).unwrap().is_ok(), "across buckets");

    fixture.empty_to(BUCKET + 60);
    let asked = again.go(&fixture, 0.0).unwrap();

    assert_eq!(asked.from, BUCKET + 51, "on from where it was");
}

#[test]
fn a_part_kept_that_is_damaged_is_set_aside_and_it_knows_from_what_is_left_and_can_read_back() {
    let fixture = long_chain(BUCKET + 50, &[]);
    let shelf = Arc::new(MemoryShelf::default());
    let mut reading = on_shelf(&shelf, &fixture);

    reading.further_back(BUCKET - 10);
    fixture.read_all(&mut reading);
    reading.close(0.0);

    // The older of the two buckets, flipped.
    let name = bucket_name(GENESIS, 0);
    let (_, mut bytes) = shelf.held().into_iter().find(|(held, _)| *held == name).unwrap();
    let middle = bytes.len() / 2;
    bytes[middle] ^= 0xff;
    shelf.write(&name, bytes, Box::new(|_| {}));

    let mut again = on_shelf(&shelf, &fixture);
    let standing = again.standing().unwrap();

    assert_eq!(standing.known_from, BUCKET, "what is older than the damage goes");
    assert_eq!(standing.high, BUCKET + 50, "and what is newer stands");
    assert!(again.said[0].contains("set aside") && again.said[0].contains(&format!("knows from block {BUCKET}")), "{:?}", again.said);
    assert!(shelf.held().iter().any(|(held, _)| held.starts_with(&format!("{name}.unreadable"))), "it stays for anyone to look into");
    assert!(again.answer(&Ask::Block("1020".to_string()), BUCKET + 50).unwrap().is_err());

    assert_eq!(again.further_back(BUCKET - 10), Further::Moved);
    fixture.read_all(&mut again);
    assert_eq!(again.standing().unwrap().known_from, BUCKET - 10, "read back again from a block's own parent");
}

#[test]
fn an_index_with_its_newest_part_damaged_is_set_aside_and_rebuilt() {
    let fixture = long_chain(100, &[]);
    let shelf = Arc::new(MemoryShelf::default());
    let mut reading = on_shelf(&shelf, &fixture);

    reading.further_back(50);
    fixture.read_all(&mut reading);
    reading.close(0.0);
    shelf.write(&bucket_name(GENESIS, 0), vec![1, 2, 3], Box::new(|_| {}));

    let mut again = on_shelf(&shelf, &fixture);

    assert!(again.said[0].contains("being read again"), "{:?}", again.said);
    assert_eq!(again.standing(), None, "it starts over");
    fixture.read_all(&mut again);
    assert_eq!(again.standing().unwrap().known_from, 100, "at the final block");

    // A head that does not read goes the same way.
    again.close(0.0);
    shelf.write(&head_name(GENESIS), b"not json".to_vec(), Box::new(|_| {}));
    let again = on_shelf(&shelf, &fixture);

    assert!(again.said[0].contains("being read again"), "{:?}", again.said);
    assert_eq!(again.standing(), None);
}

#[test]
fn blocks_kept_with_a_forged_body_do_not_come_back_as_read() {
    let fixture = long_chain(20, &[15]);
    let shelf = Arc::new(MemoryShelf::default());
    let mut reading = on_shelf(&shelf, &fixture);

    reading.further_back(10);
    fixture.read_all(&mut reading);
    reading.close(0.0);

    // Someone with the device changes what a block says it held: it is read as a node's answer is.
    let name = bucket_name(GENESIS, 0);
    let (_, bytes) = shelf.held().into_iter().find(|(held, _)| *held == name).unwrap();
    let mut range: BlockRange = chainv4::light::decode_block_range(&bytes).unwrap();
    let forged = range.items.iter_mut().find(|item| item.sealed.header.height == 15).unwrap();

    if let RangeBody::Whole { txs, .. } = &mut forged.body {
        txs[0] = transfer(2, alice(), 1_000_000, 9).0;
    }

    shelf.write(&name, wincode::serialize(&range).unwrap(), Box::new(|_| {}));
    let again = on_shelf(&shelf, &fixture);

    assert!(again.said[0].contains("do not fit") || again.said[0].contains("set aside"), "{:?}", again.said);
    assert_eq!(again.standing(), None);
}

#[test]
fn an_index_of_another_chain_at_the_same_name_is_let_go() {
    let fixture = long_chain(20, &[]);
    let shelf = Arc::new(MemoryShelf::default());
    let mut reading = on_shelf(&shelf, &fixture);

    fixture.read_all(&mut reading);
    reading.close(0.0);

    let mut other = Fixture::new();
    other.push(Making { txs: vec![transfer(2, bob(), 1, 0)], ..Making::default() });
    other.empty_to(20);

    let again = on_shelf(&shelf, &other);

    assert!(again.said[0].contains("not the chain the light client follows"), "{:?}", again.said);
    assert_eq!(again.standing(), None);
}

#[test]
fn a_shelf_that_answers_later_opens_a_step_at_a_time_with_nothing_shown_until_then() {
    /// A read waiting to be answered.
    type Pending = (String, chainv4_ui::shelf::Done<Option<Vec<u8>>>);

    /// A shelf that holds its answers until the test lets them go, as a browser's does.
    #[derive(Default)]
    struct Later {
        held: MemoryShelf,
        reads: Mutex<Vec<Pending>>,
    }

    impl Shelf for Later {
        fn read(&self, name: &str, done: chainv4_ui::shelf::Done<Option<Vec<u8>>>) {
            self.reads.lock().unwrap().push((name.to_string(), done));
        }

        fn write(&self, name: &str, bytes: Vec<u8>, done: chainv4_ui::shelf::Done<()>) {
            self.held.write(name, bytes, done);
        }

        fn set_aside(&self, name: &str, done: chainv4_ui::shelf::Done<String>) {
            self.held.set_aside(name, done);
        }

        fn forget(&self, prefix: &str, done: chainv4_ui::shelf::Done<()>) {
            self.held.forget(prefix, done);
        }
    }

    let fixture = long_chain(20, &[]);
    let later = Arc::new(Later::default());
    let mut reading = Reading::new(Keeper::new(later.clone()), true);

    assert!(!reading.open(GENESIS, HASH, &fixture, 0.0));
    assert!(!reading.is_open() && reading.answer(&Ask::Blocks { before: None, limit: 1 }, 20).is_none());
    assert_eq!(later.reads.lock().unwrap().len(), 1, "asked once");
    assert!(!reading.open(GENESIS, HASH, &fixture, 0.0));
    assert_eq!(later.reads.lock().unwrap().len(), 1, "still once");

    let (name, done) = later.reads.lock().unwrap().remove(0);
    assert_eq!(name, head_name(GENESIS));
    done(Ok(None));

    assert!(reading.open(GENESIS, HASH, &fixture, 0.0));
}

#[test]
fn a_block_fetched_by_itself_is_checked_shown_for_now_and_never_counts_as_read() {
    let fixture = busy_chain();
    let mut reading = opened(&fixture);

    fixture.read_all(&mut reading);

    let asked = reading.fetch_one(5, &fixture).unwrap();
    assert_eq!(asked, Asked { from: 5, entry: 0, count: 1 });
    assert_eq!(reading.fetching(), (true, None));

    // Forged, it is refused and nothing shows.
    let mut forged = fixture.range(asked);
    if let RangeBody::Whole { txs, .. } = &mut forged.items[0].body {
        txs[0] = transfer(2, alice(), 1_000_000, 9).0;
    }
    reading.came(asked, answered(&forged), &fixture, 0.0);
    assert!(!reading.fetching().0);
    assert!(reading.fetching().1.unwrap().contains("do not fit"), "{:?}", reading.fetching());
    assert!(reading.answer(&Ask::Block("5".to_string()), 12).unwrap().is_err());

    let asked = reading.fetch_one(5, &fixture).unwrap();
    reading.came(asked, fixture.answer(asked), &fixture, 0.0);

    let detail: BlockDetail = from_ok(reading.answer(&Ask::Block("5".to_string()), 12));
    assert_eq!(detail.summary.height, 5);
    assert!(reading.holds_fetched(5));

    let standing = reading.standing().unwrap();
    assert_eq!((standing.known_from, standing.blocks), (12, 1), "it says it knows from where it did");
    assert_eq!(txs_of(&reading, &alice(), "").items.len(), 0, "and what it did is not among what was read");

    // Not final, or with no header to check it by, it says so and nothing goes.
    assert!(reading.fetch_one(99, &fixture).unwrap_err().contains("not final"));
    let mut far = busy_chain();
    far.lowest = 8;
    assert!(reading.fetch_one(5, &far).unwrap_err().contains("no header"));
}

#[test]
fn a_direct_proof_is_asked_when_pressed_at_the_final_block_and_a_false_one_is_refused() {
    let mut fixture = Fixture::new();
    let account = Account { nonce: 3, balance: u256::from(500u64) };
    let alice_key = address_key(&alice());

    fixture.push(Making { writes: vec![(Column::Accounts, alice_key.clone(), wincode::serialize(&account).unwrap())], ..Making::default() });
    fixture.empty_to(4);
    let mut reading = opened(&fixture);
    let ask = reading.prove(Column::Accounts, &alice_key, &fixture).unwrap();

    let Ask::DirectProof { column, key: asked_key, block } = &ask else {
        panic!("{ask:?}");
    };
    assert_eq!((column.as_str(), block.as_str()), ("accounts", fixture.id(4).as_str()), "at the final block");
    assert_eq!(asked_key, &to_hex(&alice_key));
    assert!(reading.proving(Column::Accounts, &alice_key));

    let tree = StateTree::new(&fixture.state, HASH);
    let bytes = wincode::serialize(&account).unwrap();
    let proof = to_hex(&wincode::serialize(&tree.prove(Column::Accounts, &alice_key).unwrap()).unwrap());
    let good = json!({ "block": block, "value": to_hex(&bytes), "proof": proof });
    let false_one = json!({ "block": block, "value": to_hex(&[0; 40]), "proof": proof });

    assert!(reading.take_proof(&ask, &Ok(false_one), Some(&fixture)));
    assert!(!reading.proving(Column::Accounts, &alice_key));
    assert!(reading.proven(Column::Accounts, &alice_key).unwrap().as_ref().unwrap_err().contains("does not check"));

    let ask = reading.prove(Column::Accounts, &alice_key, &fixture).unwrap();
    assert!(reading.proven(Column::Accounts, &alice_key).is_none(), "what it said goes as it is asked again");
    assert!(reading.take_proof(&ask, &Ok(good), Some(&fixture)));

    let proven = reading.proven(Column::Accounts, &alice_key).unwrap().clone().unwrap();
    assert_eq!((proven.height, proven.block, proven.value), (4, fixture.id(4), Some(bytes)), "with the block it was proven at");

    // No one else's answer is taken as one, and nor is one nobody asked for.
    assert!(!reading.take_proof(&Ask::Status, &Ok(json!({})), Some(&fixture)));
    assert!(reading.take_proof(&Ask::DirectProof { column: "accounts".to_string(), key: to_hex(&[9]), block: fixture.id(4) }, &Ok(json!({})), Some(&fixture)));
    assert_eq!(reading.proven(Column::Accounts, &[9]), None);
}

#[test]
fn fees_are_suggested_from_what_blocks_show_or_a_guess_that_says_it_is_one() {
    let fixture = busy_chain();
    let mut reading = opened(&fixture);

    reading.further_back(0);
    fixture.read_all(&mut reading);

    let (signed, _) = transfer(1, bob(), 1, 9);
    let (offer, seen) = reading.suggest(&signed.tx, u256::from(10u64), u256::ZERO);

    assert!(seen);
    assert_eq!(offer.gas_limit, 21_000 + 21_000u64.div_ceil(4), "what it used, and a quarter more");
    assert_eq!(offer.max_price, u256::from(20u64), "twice the header's base price, over the 9 seen paid");

    let (unknown, seen) = reading.suggest(&Tx::Burn { from: alice(), amount: Amount::U128(1) }, u256::from(10u64), u256::ZERO);
    assert!(!seen, "a guess says it is one");
    assert_eq!(unknown.gas_limit, chainv4_ui::blocks::fees::GUESS_CALL_GAS);
}

#[test]
fn forgetting_what_was_read_lets_it_all_go_and_starts_again_at_the_final_block() {
    let fixture = busy_chain();
    let shelf = Arc::new(MemoryShelf::default());
    let mut reading = on_shelf(&shelf, &fixture);

    reading.further_back(0);
    fixture.read_all(&mut reading);
    reading.save(0.0);
    assert!(!shelf.held().is_empty());

    reading.forget();

    assert!(shelf.held().is_empty(), "nothing of it stays");
    assert_eq!(reading.standing(), None);
    assert!(reading.answer(&Ask::Block("5".to_string()), 12).unwrap().is_err());

    fixture.read_all(&mut reading);
    assert_eq!(reading.standing().unwrap().known_from, 12);
}

#[test]
fn a_block_without_its_receipts_or_with_its_header_alone_says_so_and_is_kept_as_it_came() {
    let mut fixture = Fixture::new();

    fixture.push(Making { txs: vec![transfer(1, bob(), 1, 0)], no_receipts: true, ..Making::default() });
    fixture.push(Making { header_only: true, ..Making::default() });
    fixture.empty_to(3);

    let mut reading = opened(&fixture);
    reading.further_back(0);
    fixture.read_all(&mut reading);

    assert_eq!(reading.standing().unwrap().no_receipts, 2);

    let block: BlockDetail = from_ok(reading.answer(&Ask::Block("1".to_string()), 3));
    assert!(block.txs[0].receipt.is_none());

    let id = block.txs[0].id.clone();
    let tx = refused(reading.answer(&Ask::Tx(id), 3));
    assert!(tx.message.contains("without its receipts"), "{}", tx.message);

    let pruned: BlockDetail = from_ok(reading.answer(&Ask::Block("2".to_string()), 3));
    assert!(pruned.summary.pruned);
}

#[test]
fn the_status_it_answers_comes_from_what_was_checked_with_the_coin_as_last_said_or_plain_units() {
    let client = crate::app::tests::following_client();
    let genesis = client.sealed_at(0).unwrap().id().to_string();
    let mut reading = Reading::default();

    assert!(reading.open(&genesis, client.params().hash, &client, 0.0));

    let status: crate::model::Status = serde_json::from_value(reading.status(&genesis, &client, None, None).unwrap()).unwrap();
    assert_eq!((status.height, status.tip.as_str(), status.coin.symbol.as_str(), status.coin.decimals), (0, genesis.as_str(), "units", 0));
    assert_eq!(status.finality.as_ref().map(|finality| finality.kind.as_str()), Some("authority"));

    let coin = ("DEV".to_string(), 18);
    let status: crate::model::Status = serde_json::from_value(reading.status(&genesis, &client, Some(&coin), Some("devnet")).unwrap()).unwrap();
    assert_eq!((status.coin.symbol.as_str(), status.coin.decimals, status.chain_name()), ("DEV", 18, "devnet"));
}

/// Runs against a node on a private chain under voting finality, with a transaction landed: follows it
/// with the light client, reads its blocks, and holds what the index answers against what the node says
/// of the same, with what it cost. Started by hand, as it needs a node.
#[test]
#[ignore = "needs a node with voting finality: CHAINV4_NODE=http://127.0.0.1:8099 cargo test --lib a_real_node -- --ignored --nocapture"]
fn a_real_nodes_blocks_read_here_say_what_the_node_says_of_them() {
    use {crate::light::Trust, chainv4_ui::net::Client, std::collections::HashMap, std::time::{Duration, Instant}};

    let node = std::env::var("CHAINV4_NODE").expect("CHAINV4_NODE names a node");
    let mut api = Api::new(&node, Ok(Client::direct()), egui::Context::default()).reading_blocks(true);
    let mut light = Light::default();
    let mut pinned = HashMap::new();
    let mut reading = Reading::default();
    let deadline = Instant::now() + Duration::from_secs(120);
    let started = Instant::now();

    // Follows the chain, and reads every block from genesis.
    let mut told_to_go_back = false;

    loop {
        assert!(Instant::now() < deadline, "never read it: {:?} {:?}", light.trouble, reading.standing());
        let time = started.elapsed().as_secs_f64();

        if let Some(ask) = light.ask(time) {
            api.ask(ask);
        }

        for reply in api.replies() {
            if let Reply::Answer { ask, result } = reply {
                let trust = Trust { node: &node, pinned: &mut pinned, checkpoints: &HashMap::new(), fast_resume: false, walks: true };

                light.answered(&ask, &result, trust, u64::MAX / 2, time);
            }
        }

        assert_eq!(light.trouble, None);

        if let Some(client) = light.client()
            && light.has_followed()
            && client.final_height() >= 16
        {
            let genesis = pinned.get(&node).unwrap().clone();

            if reading.open(&genesis, client.params().hash, client, time) {
                for (asked, came) in api.ranges() {
                    reading.came(asked, came, client, time);
                }

                if let Some(asked) = reading.go(client, time) {
                    api.read_range(asked);
                }

                if !told_to_go_back && reading.standing().is_some() {
                    told_to_go_back = true;
                    assert_eq!(reading.further_back(0), Further::Moved);
                }

                let standing = reading.standing();

                assert_eq!(standing.as_ref().and_then(|standing| standing.trouble.clone()), None);

                if told_to_go_back && standing.is_some_and(|standing| standing.known_from == 0 && standing.left_below == 0 && standing.read > 16) {
                    break;
                }
            }
        }

        std::thread::sleep(Duration::from_millis(5));
    }

    let standing = reading.standing().unwrap();
    let per_block = standing.downloaded / standing.read;
    let kept = standing.bytes / standing.blocks;

    println!("read {} blocks: {} bytes downloaded, {per_block} a block; {} kept, {kept} a block", standing.read, standing.downloaded, standing.bytes);

    // Against what the node itself says, on another line to it.
    let client = Client::direct();
    let ask_node = |path: String| -> Value {
        let (sender, receiver) = std::sync::mpsc::channel();

        client.fetch(format!("{node}{path}"), None, move |answer| sender.send(answer).unwrap());

        serde_json::from_slice(&receiver.recv_timeout(Duration::from_secs(10)).unwrap().unwrap().bytes).unwrap()
    };
    let mut landed = None;

    for height in [0, 1, 2, standing.high / 2, standing.high] {
        let theirs: BlockDetail = serde_json::from_value(ask_node(format!("/blocks/{height}"))).unwrap();
        let ours: BlockDetail = from_ok(reading.answer(&Ask::Block(height.to_string()), standing.high));

        assert_eq!(ours.summary.id, theirs.summary.id);
        assert_eq!((ours.summary.tx_root.clone(), ours.summary.state_root.clone(), ours.summary.producer.clone()), (theirs.summary.tx_root.clone(), theirs.summary.state_root.clone(), theirs.summary.producer.clone()));
        assert_eq!((ours.summary.tx_count, ours.summary.proof_count), (theirs.summary.tx_count, theirs.summary.proof_count));
        assert_eq!(ours.seal, theirs.seal);
        assert_eq!(ours.proofs, theirs.proofs);
        assert_eq!(ours.txs.iter().map(|tx| tx.id.clone()).collect::<Vec<_>>(), theirs.txs.iter().map(|tx| tx.id.clone()).collect::<Vec<_>>());
    }

    for height in 0..=standing.high {
        let ours: BlockDetail = from_ok(reading.answer(&Ask::Block(height.to_string()), standing.high));

        if let Some(tx) = ours.txs.first() {
            landed = Some((height, tx.clone()));
        }
    }

    match landed {
        Some((height, tx)) => {
            let theirs: Receipt = serde_json::from_value(ask_node(format!("/txs/{}", tx.id))).unwrap();
            let ours: Receipt = from_ok(reading.answer(&Ask::Tx(tx.id.clone()), standing.high));

            assert_eq!((ours.block, ours.height, ours.index, ours.outcome.status, ours.outcome.fee, ours.outcome.gas_used, ours.finalized), (theirs.block, theirs.height, theirs.index, theirs.outcome.status, theirs.outcome.fee, theirs.outcome.gas_used, true));

            let (sender, target) = (tx.sender.clone(), tx.target().map(str::to_string));
            let of_sender: TxsPage = from_ok(reading.answer(&Ask::AccountTxs { address: sender.clone(), query: String::new(), cursor: None }, standing.high));

            assert!(of_sender.items.iter().any(|found| found.height == height), "{sender}");

            if let Some(target) = target {
                let of_target: TxsPage = from_ok(reading.answer(&Ask::AccountTxs { address: target, query: String::new(), cursor: None }, standing.high));

                assert!(of_target.items.iter().any(|found| found.tx.id == tx.id));
            }

            println!("a transaction at block {height} reads as the node says it");
        }
        None => println!("no transaction on this chain to hold against the node's"),
    }

    // And a direct proof of the development key's account, checked, at the final block.
    let client_now = light.client().unwrap();
    let account = address_key(&Keypair::development().public().address());
    let ask = reading.prove(Column::Accounts, &account, client_now).unwrap();
    let Ask::DirectProof { column, key: proof_key, block } = &ask else {
        panic!("{ask:?}");
    };
    let proof = ask_node(format!("/proofs/{column}/{proof_key}?block={block}"));

    assert!(reading.take_proof(&ask, &Ok(proof), Some(client_now)));

    let proven = reading.proven(Column::Accounts, &account).unwrap().clone().unwrap();
    let held: Account = wincode::deserialize(proven.value.as_ref().unwrap()).unwrap();

    println!("the development key holds {} with nonce {} at final block {}", held.balance, held.nonce, proven.height);
    assert_eq!(proven.height, client_now.final_height());
}

/// Where a reading stands with all it could have to say.
pub fn a_reading_with_everything_to_say() -> Standing {
    Standing {
        high: 17_000,
        known_from: 1_024,
        floor: 0,
        left_below: 1_024,
        catching_up: true,
        downloaded: 34_000_000,
        read: 16_000,
        bytes: 6_000_000,
        most: 32_000_000,
        blocks: 16_000,
        waiting: Some("The node is busy, and asks to wait: asking again in 8 seconds.".to_string()),
        trouble: Some(Trouble::Bad("blocks that do not link up to the chain the light client checked".to_string()).sentence()),
        no_receipts: 3,
        unread: 12,
        held_from: Some(1_024),
        cut_below: Some(1_024),
    }
}

/// A page drawn twice, as a screen this size shows it: how wide it came out, what it said, and what it
/// asked of the node or the index, as it is on a world with this reading and store.
pub fn drawn_in(store: &Store, reading: &mut Reading, light: &mut Light, size: (f32, f32), on: bool, mut draw: impl FnMut(&mut Ui, &mut Ctx)) -> (f32, String, Vec<Ask>) {
    let egui = egui::Context::default();
    crate::theme::install_fonts(&egui);
    let palette = crate::theme::DARK;
    let look = Look { palette, roles: crate::theme::Roles::of(&palette), narrow: chainv4_ui::layout::is_narrow(size.0) };
    let reader = chainv4::abi::known::AbiReader::default();
    let mut api = Api::new("http://127.0.0.1:1", Err("no client in a test".to_string()), egui.clone()).reading_blocks(on);
    let mut settings = Settings { read_blocks: on, node: "http://here".to_string(), ..Settings::default() };
    let mut go = None;
    let (mut width, mut said) = (0.0_f32, String::new());

    for _ in 0..2 {
        let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(size.0, size.1))), ..egui::RawInput::default() };
        let mut output = egui.run_ui(input, |ui| {
            let mut ctx = Ctx { store, reader: &reader, light: &mut *light, reading: &mut *reading, api: &mut api, go: &mut go, settings: &mut settings, now: 2_000_000, look };

            draw(ui, &mut ctx);
            width = ui.min_rect().width();
        });

        output.textures_delta.clear();
        said = output.shapes.iter().filter_map(|clipped| match &clipped.shape {
            egui::epaint::Shape::Text(shape) => Some(shape.galley.text().to_string()),
            _ => None,
        }).collect::<Vec<_>>().join("\n");
    }

    let mut asked: Vec<Ask> = api.for_index();
    asked.extend(api.replies().into_iter().filter_map(|reply| match reply {
        Reply::Answer { ask, .. } => Some(ask),
        Reply::Stream(_) => None,
    }));

    (width, said, asked)
}

/// As `drawn_in`, on an empty world: how wide, and what it said.
pub fn drawn(size: (f32, f32), on: bool, draw: impl FnMut(&mut Ui, &mut Ctx)) -> (f32, String) {
    let (width, said, _) = drawn_in(&Store::default(), &mut Reading::default(), &mut Light::default(), size, on, draw);

    (width, said)
}

/// What the store holds once the index has answered, as it does in the app, of a chain with something
/// in it.
fn store_of(reading: &Reading, fixture: &Fixture, ids: &[String]) -> Store {
    let status = json!({
        "genesis": GENESIS, "tip": fixture.id(12), "height": 12, "timestamp": 60_000, "difficulty": "0", "work": "0", "hash": "blake3",
        "coin": { "symbol": "DEV", "decimals": 18 }, "finality": null, "fees": null,
    });
    let mut store = Store { status: serde_json::from_value(status).ok(), ..Store::default() };

    let page: Page = from_ok(reading.answer(&Ask::Blocks { before: None, limit: 100 }, 12));
    store.page(page);

    for height in 5..=8 {
        let detail: BlockDetail = from_ok(reading.answer(&Ask::Block(height.to_string()), 12));
        store.detail(detail);
    }

    for id in ids {
        let receipt: Receipt = from_ok(reading.answer(&Ask::Tx(id.clone()), 12));
        store.receipts.insert(id.clone(), receipt);
    }

    store
}

#[test]
fn every_page_that_works_from_blocks_fits_a_phone_either_way_says_what_it_should_and_asks_nothing_turned_off() {
    let (fixture, mut reading) = read_busy_chain_from(0);
    let (signed, _) = transfer(1, bob(), 100, 0);
    let id = signed.id(HASH).to_string();
    let failed: BlockDetail = from_ok(reading.answer(&Ask::Block("7".to_string()), 12));
    let store = store_of(&reading, &fixture, &[id.clone(), failed.txs[0].id.clone()]);
    let address = alice().to_string();

    for size in SCREENS {
        let mut light = Light::default();
        let mut page = |draw: &mut dyn FnMut(&mut Ui, &mut Ctx)| -> (String, Vec<Ask>) {
            let (width, _, _) = drawn_in(&store, &mut reading, &mut light, size, true, |ui, ctx| draw(ui, ctx));
            // What is below a screen is not drawn, so what it says is read off a very tall one.
            let (_, said, asked) = drawn_in(&store, &mut reading, &mut light, (size.0, 8_000.0), true, |ui, ctx| draw(ui, ctx));

            assert!(width <= size.0, "{width} wide, on {size:?}: {said}");

            (said, asked)
        };

        let (said, _) = page(&mut |ui, ctx| crate::views::overview::show(ui, ctx));
        assert!(said.contains("READING CHECKED BLOCKS") && said.contains("knowing from block 0"), "{said}");
        assert!(!said.contains("PEERS") && !said.contains("MEMPOOL") && !said.contains("LIVE"), "what only a node says: {said}");

        let (said, _) = page(&mut |ui, ctx| crate::views::block::show(ui, ctx, "5"));
        assert!(said.contains("final, checked here") && said.contains("transfer"), "{said}");

        let (said, _) = page(&mut |ui, ctx| crate::views::tx::show(ui, ctx, &id, &mut Default::default()));
        assert!(said.contains("final") && said.contains("does not commit to the hops"), "{said}");

        let (said, _) = page(&mut |ui, ctx| crate::views::tx::show(ui, ctx, &failed.txs[0].id, &mut Default::default()));
        assert!(said.contains("commits to that it failed") || said.contains("failed"), "{said}");

        // Opening an address tells nobody: not its balance, nor its names, nor what it holds.
        let (said, asked) = page(&mut |ui, ctx| crate::views::account::show(ui, ctx, &address, &mut Default::default()));
        assert!(said.contains("Prove it directly") && said.contains("the node sees"), "{said}");
        assert!(asked.iter().all(|ask| ask.route() == crate::api::Route::Index), "{asked:?}");

        let (said, asked) = page(&mut |ui, ctx| crate::views::names::show(ui, ctx, &mut Default::default()));
        assert!(said.contains("needs asking the node") && asked.is_empty(), "{said} {asked:?}");

        let (said, asked) = page(&mut |ui, ctx| crate::views::name::show(ui, ctx, "alice.dev"));
        assert!(said.contains("Prove it directly") && asked.is_empty(), "{said} {asked:?}");

        let (said, asked) = page(&mut |ui, ctx| crate::views::tokens::show(ui, ctx, &mut Default::default()));
        assert!(said.contains("needs asking the node") && asked.is_empty(), "{said} {asked:?}");

        let (said, asked) = page(&mut |ui, ctx| crate::views::rich::show(ui, ctx));
        assert!(said.contains("Prove it directly") && said.contains("needs asking the node") && asked.is_empty(), "{said} {asked:?}");

        let (said, asked) = page(&mut |ui, ctx| crate::views::send::show(ui, ctx, &mut Default::default()));
        assert!(said.contains("SIGNING KEY") && asked.is_empty(), "{said} {asked:?}");

        let (said, asked) = page(&mut |ui, ctx| crate::views::tools::show(ui, ctx, &mut Default::default()));
        assert!(said.contains("Decode") && asked.is_empty(), "{said} {asked:?}");

        page(&mut |ui, ctx| crate::views::map::show(ui, ctx, &mut Default::default()));
    }
}

#[test]
fn what_was_proven_by_a_press_shows_with_its_block_and_fits_a_phone_either_way() {
    let fixture = busy_chain();
    let address = alice().to_string();
    let store = Store::default();
    let at = |value: Option<Vec<u8>>| ProvenValue { value, height: 1_234, block: "0xblock".to_string() };
    let token = chainv4::registry::tokens::Token { address: self::token(), name: Some("Example".to_string()), symbol: Some("EXT".to_string()), decimals: Some(6) };
    let record = chainv4::registry::names::NameRecord { owner: bob(), target: Some(alice()), expires: 9_000_000, root: 1, texts: vec![chainv4::registry::names::TextRecord { key: "url".to_string(), value: "https://example.org".to_string() }] };
    let supply = wincode::serialize(&chainv4::registry::supply::Supply { minted: u256::from(1_000u64), burned: u256::from(1u64) }).unwrap();

    for size in SCREENS {
        let mut reading = opened(&fixture);

        reading.has_proven(Column::Accounts, &address_key(&alice()), at(Some(wincode::serialize(&Account { nonce: 6, balance: u256::from(700u64) }).unwrap())));
        reading.has_proven(Column::Tokens, &chainv4::registry::tokens::token_key(&self::token()), at(Some(wincode::serialize(&token).unwrap())));
        reading.has_proven(Column::Names, &chainv4::registry::names::record_key("alice"), at(Some(wincode::serialize(&record).unwrap())));
        reading.has_proven(Column::Validators, chainv4::registry::supply::SUPPLY_KEY, at(Some(supply.clone())));

        let mut page = |draw: &mut dyn FnMut(&mut Ui, &mut Ctx)| -> String {
            let (width, _, _) = drawn_in(&store, &mut reading, &mut Light::default(), size, true, |ui, ctx| draw(ui, ctx));
            let (_, said, _) = drawn_in(&store, &mut reading, &mut Light::default(), (size.0, 8_000.0), true, |ui, ctx| draw(ui, ctx));

            assert!(width <= size.0, "{width} wide, on {size:?}: {said}");

            said
        };

        let said = page(&mut |ui, ctx| crate::views::account::show(ui, ctx, &address, &mut Default::default()));
        assert!(said.contains("700") && said.contains("nonce 6") && said.contains("at final block 1,234") && said.contains("Prove it again"), "{said}");

        let said = page(&mut |ui, ctx| crate::views::name::show(ui, ctx, "alice.dev"));
        assert!(said.contains("owner") && said.contains("example.org") && said.contains("at final block 1,234"), "{said}");

        let mut typed = crate::views::tokens::TokensState::with_typed(&self::token().to_string());
        let said = page(&mut |ui, ctx| crate::views::tokens::show(ui, ctx, &mut typed));
        assert!(said.contains("Example") && said.contains("EXT") && said.contains("at final block 1,234"), "{said}");

        let said = page(&mut |ui, ctx| crate::views::rich::show(ui, ctx));
        assert!(said.contains("minted") && said.contains("burned") && said.contains("at final block 1,234"), "{said}");
    }
}

#[test]
fn the_finality_page_says_what_the_light_client_checked_and_settings_fit_a_phone_either_way() {
    let status = json!({
        "genesis": GENESIS, "tip": "0x01", "height": 12, "timestamp": 60_000, "difficulty": "0", "work": "0", "hash": "blake3",
        "coin": { "symbol": "units", "decimals": 0 }, "fees": null,
        "finality": { "type": "ffg", "epoch": 1, "checkpoint_interval": 8, "justified": { "epoch": 2, "block": "0x02", "height": 16 }, "finalized": { "epoch": 1, "block": "0x01", "height": 8 } },
    });
    let store = Store { status: serde_json::from_value(status).ok(), ..Store::default() };
    assert!(store.status.is_some());

    for size in SCREENS {
        let (width, _, asked) = drawn_in(&store, &mut Reading::default(), &mut Light::default(), size, true, crate::views::finality::show);
        let (_, said, _) = drawn_in(&store, &mut Reading::default(), &mut Light::default(), (size.0, 8_000.0), true, crate::views::finality::show);

        assert!(width <= size.0, "{width} wide, on {size:?}");
        assert!(said.contains("JUSTIFIED") && said.contains("needs asking the node"), "{said}");
        assert!(asked.is_empty(), "{asked:?}");

        let streaming = crate::views::network::Streaming::Live;
        let (mut address, mut state, mut connect) = ("http://here".to_string(), crate::views::settings::SettingsState::default(), None);
        let (width, _, asked) = drawn_in(&store, &mut Reading::default(), &mut Light::default(), size, true, |ui, ctx| crate::views::settings::show(ui, ctx, &mut address, &mut state, &streaming, &mut connect));

        assert!(width <= size.0, "{width} wide, on {size:?}");
        assert!(asked.is_empty(), "{asked:?}");
    }
}

#[test]
fn a_block_and_a_transaction_outside_what_was_read_say_so_with_the_way_further_back_and_fit() {
    let fixture = busy_chain();
    let mut reading = opened(&fixture);
    fixture.read_all(&mut reading);

    let mut store = Store::default();
    store.failures.insert(Ask::Block("5".to_string()), refused(reading.answer(&Ask::Block("5".to_string()), 12)));
    store.failures.insert(Ask::Tx("0x01".to_string()), Failure { status: 404, message: "Not in the blocks read, 12 to 12: it may be older, or not final yet".to_string() });

    for size in SCREENS {
        let (width, _, _) = drawn_in(&store, &mut reading, &mut Light::default(), size, true, |ui, ctx| crate::views::block::show(ui, ctx, "5"));
        let (_, said, _) = drawn_in(&store, &mut reading, &mut Light::default(), (size.0, 8_000.0), true, |ui, ctx| crate::views::block::show(ui, ctx, "5"));

        assert!(width <= size.0, "{width} wide, on {size:?}");
        assert!(said.contains("knows from block 12") && said.contains("Read further back") && said.contains("Fetch this one block") && said.contains("the node sees which"), "{said}");

        let (width, _, _) = drawn_in(&store, &mut reading, &mut Light::default(), size, true, |ui, ctx| crate::views::tx::show(ui, ctx, "0x01", &mut Default::default()));
        let (_, said, _) = drawn_in(&store, &mut reading, &mut Light::default(), (size.0, 8_000.0), true, |ui, ctx| crate::views::tx::show(ui, ctx, "0x01", &mut Default::default()));

        assert!(width <= size.0, "{width} wide, on {size:?}");
        assert!(said.contains("Not in the blocks read") && said.contains("Read further back") && !said.contains("mempool"), "{said}");
    }
}

#[test]
fn with_the_setting_off_the_pages_ask_as_they_always_did_and_say_nothing_of_it() {
    let store = Store::default();
    let address = alice().to_string();
    let (_, said, asked) = drawn_in(&store, &mut Reading::default(), &mut Light::default(), (1200.0, 800.0), false, |ui, ctx| crate::views::account::show(ui, ctx, &address, &mut Default::default()));

    assert!(asked.contains(&Ask::Account(address.clone())), "it asks the node, as it always did: {asked:?}");
    assert!(!said.contains("Prove it directly") && !said.contains("needs asking"), "{said}");

    let (_, said, _) = drawn_in(&store, &mut Reading::default(), &mut Light::default(), (1200.0, 800.0), false, |ui, ctx| crate::views::names::show(ui, ctx, &mut Default::default()));
    assert!(!said.contains("needs asking"), "{said}");
}

