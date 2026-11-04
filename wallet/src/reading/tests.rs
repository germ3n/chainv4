use {
    super::*,
    chainv4::crypto::keypair::Keypair,
    chainv4::db::{MemoryDb, State, StateProof, StateTree},
    chainv4_ui::blocks::fees::{GUESS_CALL_GAS, HINT_BLOCKS},
    chainv4::light::{BlockRange, NoProofs, ProvenEntry, RangeChanges, RangeCursor, RangeItem},
    chainv4::structs::amount::Amount,
    chainv4::structs::block::{Block, BlockHeader, Seal, SealedHeader},
    chainv4::structs::event::Event,
    chainv4::structs::receipt::{CommittedOutcome, CommittedReceipt, receipts_root},
    chainv4::structs::tx::{FeeOffer, SignedTx, TxCallArgumentType},
    chainv4_ui::net::Answer,
    chainv4_ui::shelf::Shelf,
    serde_json::json,
};

const HASH: HashType = HashType::Blake3_256;

/// What a proof a node would give is the proof of: a key at a height.
type ProofOf = (u64, Column, Vec<u8>);

/// One block's making: what ran in it, what it wrote, and how a node comes to send it.
#[derive(Default)]
struct Making {
    txs: Vec<SignedTx>,
    receipts: Vec<CommittedReceipt>,
    /// What it writes: each entry's new value, None to delete it.
    writes: Vec<(Column, Vec<u8>, Option<Vec<u8>>)>,
    /// What the node leaves off the block's list, undetectably.
    left_out: Vec<Vec<u8>>,
    /// Why the node proves no entries, if it does not.
    no_proofs: Option<NoProofs>,
    no_receipts: bool,
    header_only: bool,
    base_price: u64,
}

/// A chain of real headers and proofs, in the range's real encoding: what a node sends, and a light
/// client's view of it, its genesis allocating what the first argument says.
struct Fixture {
    items: Vec<RangeItem>,
    final_height: u64,
    lowest: u64,
    /// The proofs a node would give of each key it may be asked about, by height: value and proof.
    proofs: BTreeMap<ProofOf, (Option<Vec<u8>>, StateProof)>,
    tracked: Vec<(Column, Vec<u8>)>,
    state: State,
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
    fn new(genesis: &[(Column, Vec<u8>, Vec<u8>)], tracked: Vec<(Column, Vec<u8>)>) -> Fixture {
        let state = State::new(Arc::new(MemoryDb::default()));

        for (column, key, value) in genesis {
            state.put(*column, key.clone(), value.clone());
        }

        let mut fixture = Fixture { items: Vec::new(), final_height: 0, lowest: 0, proofs: BTreeMap::new(), tracked, state };

        fixture.push(Making::default());
        fixture
    }

    /// Adds a block.
    fn push(&mut self, making: Making) {
        let height = self.items.len() as u64;

        for (column, key, value) in &making.writes {
            match value {
                Some(value) => self.state.put(*column, key.clone(), value.clone()),
                None => self.state.delete(*column, key.clone()),
            }
        }

        let tree = StateTree::new(&self.state, HASH);
        let header = BlockHeader {
            parent: self.items.last().map_or_else(|| Hash::zero(HASH), |item| item.sealed.id()),
            height,
            timestamp: height * 1_000,
            producer: Address::Ed25519([0; 32]),
            difficulty: 0,
            tx_root: Block::tx_root(HASH, &making.txs),
            proof_root: Block::proof_root(HASH, &[]),
            pool_root: Block::pool_root(HASH, None),
            receipts_root: receipts_root(HASH, &making.receipts),
            state_root: tree.follow().unwrap(),
            gas_used: 0,
            base_price: u256::from(making.base_price),
        };

        for (column, key) in &self.tracked {
            let value = self.state.get(*column, key).unwrap();
            self.proofs.insert((height, *column, key.clone()), (value, tree.prove(*column, key).unwrap()));
        }

        let listed: Vec<ProvenEntry> = making
            .writes
            .iter()
            .filter(|(_, key, _)| !making.left_out.contains(key))
            .map(|(column, key, value)| ProvenEntry { column: *column, key: key.clone(), value: value.clone(), proof: tree.prove(*column, key).unwrap() })
            .collect();
        let changes = match (height, making.no_proofs) {
            (0, _) => RangeChanges::None(NoProofs::NoList),
            (_, Some(why)) => RangeChanges::None(why),
            (_, None) => RangeChanges::Proven { from: 0, listed: listed.len() as u32, entries: listed },
        };
        let body = match making.header_only {
            true => RangeBody::HeaderOnly,
            false => RangeBody::Whole { txs: making.txs, proofs: Vec::new(), pool: None, receipts: (!making.no_receipts).then_some(making.receipts) },
        };

        self.items.push(RangeItem { sealed: SealedHeader { header, seal: Seal::None }, body, changes });
    }

    /// Adds blocks that do nothing, up to this height, each final.
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

    /// What a node answers, as bytes come over the wire.
    fn answer(&self, asked: Asked) -> Came {
        answered(&self.range(asked))
    }

    /// What a node answers of a proof, as GET /proofs/{column}/{key} writes it.
    fn proof_of(&self, ask: &Ask) -> Result<Value, Failure> {
        let Ask::ScanProof { column, key, block } = ask else {
            panic!("only proofs are asked of here: {ask:?}");
        };
        let column = column_named(column).unwrap();
        let height = self.items.iter().position(|item| item.sealed.id().to_string() == *block).expect("a block on the chain") as u64;
        let (value, proof) = self.proofs.get(&(height, column, from_hex(key).unwrap())).expect("a key this tracks");

        Ok(json!({ "block": block, "value": value.as_deref().map(to_hex), "proof": to_hex(&wincode::serialize(proof).unwrap()) }))
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

fn account_entry(address: &Address, balance: u64, nonce: u64) -> (Column, Vec<u8>, Option<Vec<u8>>) {
    let account = Account { balance: u256::from(balance), nonce };

    (Column::Accounts, address_key(address), Some(wincode::serialize(&account).unwrap()))
}

/// What transactions here are signed for: reading a block checks its roots, and no signature.
fn chain() -> Hash {
    Hash::zero(HASH)
}

fn pay(from: u8, to: Address, amount: u64, nonce: u64) -> (SignedTx, CommittedReceipt) {
    let signed = SignedTx::sign(Tx::Transfer { from: key(from).public().address(), to, amount: Amount::U128(u128::from(amount)) }, nonce, FeeOffer::NONE, &key(from), &chain());
    let receipt = CommittedReceipt::new(signed.id(HASH), 0, 21_000, u256::from(21_000u64 * 7), CommittedOutcome::Done, Vec::new(), Vec::new());

    (signed, receipt)
}

fn wanted(vault: u8, addresses: Vec<Address>, born: Option<u64>) -> Wanted<'static> {
    Wanted { wallet: "wallet", genesis: "0xaa", vault_key: Zeroizing::new([vault; 32]), addresses, born }
}

/// Answers everything a reading asks of this node until it has nothing more to ask.
fn read_all(reading: &mut Reading, fixture: &Fixture, addresses: &[Address], born: Option<u64>) -> usize {
    let mut asks = 0;

    for time in 0.. {
        let go = reading.go(wanted(7, addresses.to_vec(), born), fixture, f64::from(time) * 40.0);
        let idle = go.proofs.is_empty() && go.range.is_none();

        for ask in go.proofs {
            reading.proof(&ask, &fixture.proof_of(&ask), fixture, f64::from(time) * 40.0);
        }

        if let Some(asked) = go.range {
            asks += 1;
            reading.came(asked, fixture.answer(asked), fixture, f64::from(time) * 40.0);
        }

        assert!(time < 2_000, "it never settles");

        if idle {
            break;
        }
    }

    asks
}

/// A reading with all it could say to say: a gap behind it and a value out of date, blocks without
/// their receipts, something Check now found left out, a node that sent what does not check.
pub(crate) fn a_reading_with_everything_to_say() -> Reading {
    let mut fixture = alices_chain();
    let (_, mut reading) = shelf();
    read_all(&mut reading, &fixture, &[alice()], Some(9));

    fixture.push(Making { writes: vec![account_entry(&alice(), 1, 2)], left_out: vec![address_key(&alice())], ..Making::default() });
    fixture.final_height = 11;
    read_all(&mut reading, &fixture, &[alice()], Some(9));
    reading.check_now();
    read_all(&mut reading, &fixture, &[alice()], Some(9));

    fixture.push(Making { no_receipts: true, no_proofs: Some(NoProofs::StatePruned { kept_from: 50 }), ..Making::default() });
    fixture.push(Making::default());
    fixture.final_height = 13;
    let go = reading.go(wanted(7, vec![alice()], Some(9)), &fixture, 1_000.0);
    reading.came(go.range.unwrap(), Came::Bad { why: chainv4::light::RangeError::ReceiptsRoot, bytes: 10 }, &fixture, 1_000.0);
    reading.retry();
    let go = reading.go(wanted(7, vec![alice()], Some(9)), &fixture, 2_000.0);
    let asked = go.range.unwrap();
    let one = Asked { count: 1, ..asked };
    reading.came(asked, fixture.answer(one), &fixture, 2_000.0);
    let go = reading.go(wanted(7, vec![alice()], Some(9)), &fixture, 3_000.0);
    reading.came(go.range.unwrap(), Came::Bad { why: chainv4::light::RangeError::ReceiptsRoot, bytes: 10 }, &fixture, 3_000.0);

    let standing = reading.standing().unwrap();
    assert!(standing.trouble.is_some() && standing.unread > 0 && !standing.omitted.is_empty() && standing.stale > 0, "{standing:?}");

    reading
}

/// Lets the shelf take what was written, which a memory shelf does at once.
fn shelf() -> (Arc<MemoryShelf>, Reading) {
    let shelf = Arc::new(MemoryShelf::default());
    let reading = Reading::new(Keeper::new(shelf.clone()), true);

    (shelf, reading)
}

/// What is shown of the index, through the same maps the pages read.
fn shown(reading: &mut Reading) -> (Chain, HashMap<String, Checked>) {
    let (mut chain, mut checked) = (Chain::default(), HashMap::new());

    reading.show(HASH, &mut chain, &mut checked);

    (chain, checked)
}

fn balance(chain: &Chain, address: &Address) -> Option<u256> {
    chain.account(&address.to_string()).map(|proven| proven.value.balance)
}

fn tracked_alice() -> Vec<(Column, Vec<u8>)> {
    vec![(Column::Accounts, address_key(&alice())), (Column::Accounts, address_key(&bob()))]
}

/// A chain where alice starts with 100, and blocks 3 and 8 change her balance.
fn alices_chain() -> Fixture {
    let mut fixture = Fixture::new(&[(Column::Accounts, address_key(&alice()), wincode::serialize(&Account { balance: u256::from(100u64), ..Account::default() }).unwrap())], tracked_alice());

    fixture.empty_to(2);
    let (signed, receipt) = pay(2, alice(), 50, 0);
    fixture.push(Making { txs: vec![signed], receipts: vec![receipt], writes: vec![account_entry(&alice(), 150, 0)], ..Making::default() });
    fixture.empty_to(7);
    let (signed, receipt) = pay(1, bob(), 30, 0);
    fixture.push(Making { txs: vec![signed], receipts: vec![receipt], writes: vec![account_entry(&alice(), 120, 1), account_entry(&bob(), 30, 0)], ..Making::default() });
    fixture.empty_to(10);

    fixture
}

#[test]
fn a_reading_shows_a_balance_from_its_first_proof_and_what_blocks_say_of_it_from_then_on() {
    let mut fixture = alices_chain();
    fixture.final_height = 5;
    let (_, mut reading) = shelf();

    read_all(&mut reading, &fixture, &[alice()], Some(0));
    let (chain, checked) = shown(&mut reading);

    assert_eq!(balance(&chain, &alice()), Some(u256::from(150u64)), "proven at block 5, all there is of it");
    assert_eq!(checked.len(), 1, "going back to block 0, the one transaction that touched her");
    assert!(checked.values().all(|tx| tx.came == Origin::Read && tx.final_ && tx.height == 3));

    let standing = reading.standing().unwrap();
    assert_eq!((standing.high, standing.known_from, standing.floor, standing.left_below), (5, 0, 0, 0));
    assert_eq!(standing.proofs, 1, "one direct proof, to start from");
    assert_eq!(standing.read, 6);
    assert!(standing.downloaded > 0);

    // Block 8 becomes final: the blocks say what changed, with nothing asked directly.
    fixture.final_height = 10;
    read_all(&mut reading, &fixture, &[alice()], Some(0));
    let (chain, checked) = shown(&mut reading);

    assert_eq!(balance(&chain, &alice()), Some(u256::from(120u64)));
    assert_eq!(chain.account(&alice().to_string()).unwrap().value.nonce, 1, "and the nonce it sends with");
    assert_eq!(chain.account(&alice().to_string()).unwrap().assurance.height, 8);
    assert_eq!(checked.len(), 1, "only what is new shows anew");
    assert_eq!(reading.standing().unwrap().proofs, 1, "still the one");
}

#[test]
fn a_smaller_cap_on_transactions_lets_the_oldest_go_at_once_and_holds_as_reading_goes_on() {
    let mut fixture = alices_chain();
    fixture.final_height = 10;
    let (_, mut reading) = shelf();

    read_all(&mut reading, &fixture, &[alice()], Some(0));

    assert_eq!(reading.txs_kept(), (2, crate::reading::MOST_TXS), "both of hers, under the usual cap");
    assert_eq!(reading.standing().unwrap().cut_below, None);

    reading.keep_most_txs(1);

    assert_eq!(reading.txs_kept(), (1, 1), "the newest alone");
    assert_eq!(reading.standing().unwrap().cut_below, Some(8), "and it says from where");

    // A cap set before the index opens holds from its first block read.
    let (_, mut capped) = shelf();
    capped.keep_most_txs(1);
    read_all(&mut capped, &fixture, &[alice()], Some(0));

    assert_eq!(capped.txs_kept(), (1, 1));
}

#[test]
fn a_new_wallet_reads_from_where_it_was_made_and_a_restored_one_is_asked() {
    let fixture = alices_chain();
    let (_, mut new) = shelf();

    new.go(wanted(7, vec![alice()], Some(9)), &fixture, 0.0);
    new.go(wanted(7, vec![alice()], Some(9)), &fixture, 0.0);
    assert!(!new.needs_floor());
    assert_eq!(new.standing().unwrap().floor, 9, "its creation height");

    // Made before the chain was followed, it reads from wherever the reading starts.
    let (_, mut newest) = shelf();
    newest.go(wanted(7, vec![alice()], Some(u64::MAX)), &fixture, 0.0);
    newest.go(wanted(7, vec![alice()], Some(u64::MAX)), &fixture, 0.0);
    assert_eq!(newest.standing().unwrap().floor, 10);

    let (_, mut restored) = shelf();
    let go = restored.go(wanted(7, vec![alice()], None), &fixture, 0.0);
    assert!(go.proofs.is_empty() && go.range.is_none());
    assert!(restored.needs_floor(), "it asks");
    assert!(restored.standing().is_none(), "and shows nothing until told");

    restored.start_from(4);
    read_all(&mut restored, &fixture, &[alice()], None);
    assert!(!restored.needs_floor());
    let standing = restored.standing().unwrap();
    assert_eq!((standing.floor, standing.known_from, standing.high), (4, 4, 10));
    let (chain, checked) = shown(&mut restored);
    assert_eq!(checked.len(), 1, "what was at block 8, and nothing of block 3");
    assert_eq!(balance(&chain, &alice()), Some(u256::from(120u64)));
}

#[test]
fn a_start_at_height_zero_still_takes_its_first_proof_as_genesis_is_in_no_block() {
    let mut fixture = Fixture::new(&[(Column::Accounts, address_key(&alice()), wincode::serialize(&Account { balance: u256::from(100u64), ..Account::default() }).unwrap())], tracked_alice());
    fixture.empty_to(0);
    let (_, mut reading) = shelf();

    read_all(&mut reading, &fixture, &[alice()], Some(0));
    let (chain, _) = shown(&mut reading);

    assert_eq!(balance(&chain, &alice()), Some(u256::from(100u64)));
    assert_eq!(reading.standing().unwrap().proofs, 1);
    assert_eq!(reading.standing().unwrap().state_gap, None, "genesis lists nothing, and that is no gap");
}

#[test]
fn an_entry_a_block_deleted_reads_as_nothing_there() {
    let mut fixture = alices_chain();
    fixture.empty_to(3);
    let (_, mut reading) = shelf();
    read_all(&mut reading, &fixture, &[alice()], Some(0));
    assert_eq!(balance(&shown(&mut reading).0, &alice()), Some(u256::from(150u64)));

    fixture.items.truncate(4);
    fixture.push(Making { writes: vec![(Column::Accounts, address_key(&alice()), None)], ..Making::default() });
    fixture.final_height = 4;
    read_all(&mut reading, &fixture, &[alice()], Some(0));

    let (chain, _) = shown(&mut reading);
    assert_eq!(balance(&chain, &alice()), Some(u256::ZERO), "proven absent, not left as it was");
    assert_eq!(chain.account(&alice().to_string()).unwrap().assurance.height, 4);
}

#[test]
fn a_block_without_proofs_counts_as_unknown_and_takes_one_direct_proof_saying_the_node_saw_it() {
    for why in [NoProofs::NoList, NoProofs::StatePruned { kept_from: 50 }] {
        let mut fixture = alices_chain();
        let (_, mut reading) = shelf();

        read_all(&mut reading, &fixture, &[alice()], Some(9));
        assert_eq!(reading.standing().unwrap().proofs, 1);

        // A change to her account, in a block that lists nothing.
        fixture.push(Making { writes: vec![account_entry(&alice(), 999, 2)], no_proofs: Some(why), ..Making::default() });
        fixture.final_height = 11;
        let go = reading.go(wanted(7, vec![alice()], Some(9)), &fixture, 1_000.0);
        reading.came(go.range.unwrap(), fixture.answer(Asked { from: 11, entry: 0, count: 1 }), &fixture, 1_000.0);

        let standing = reading.standing().unwrap();
        assert_eq!(standing.state_gap, Some(11));
        assert_eq!(standing.stale, 1, "unknown, not unchanged");
        let (mut chain, mut checked) = (Chain::default(), HashMap::new());
        chain.accounts.insert(alice().to_string(), Vec::new());
        reading.show(HASH, &mut chain, &mut checked);
        assert_eq!(balance(&chain, &alice()), None, "and what showed goes");

        let go = reading.go(wanted(7, vec![alice()], Some(9)), &fixture, 1_001.0);
        assert_eq!(go.proofs.len(), 1, "one direct proof to catch up");
        reading.proof(&go.proofs[0], &fixture.proof_of(&go.proofs[0]), &fixture, 1_001.0);

        let standing = reading.standing().unwrap();
        assert_eq!((standing.proofs, standing.stale), (2, 0));
        assert_eq!(balance(&shown(&mut reading).0, &alice()), Some(u256::from(999u64)));
        assert!(reading.go(wanted(7, vec![alice()], Some(9)), &fixture, 1_002.0).proofs.is_empty(), "once");
    }
}

#[test]
fn a_node_with_no_proofs_of_anything_is_not_told_of_every_account_with_every_block() {
    let mut fixture = alices_chain();
    let (_, mut reading) = shelf();

    read_all(&mut reading, &fixture, &[alice()], Some(9));

    let mut proofs = 0;

    for height in 11..=40u32 {
        fixture.push(Making { no_proofs: Some(NoProofs::NoList), ..Making::default() });
        fixture.final_height = u64::from(height);
        let time = 1_000.0 + f64::from(height);
        let go = reading.go(wanted(7, vec![alice()], Some(9)), &fixture, time);

        for ask in &go.proofs {
            proofs += 1;
            reading.proof(ask, &fixture.proof_of(ask), &fixture, time);
        }

        if let Some(asked) = go.range {
            reading.came(asked, fixture.answer(asked), &fixture, time);
        }
    }

    assert!(proofs <= 1, "at most once in {CATCH_UP_EVERY} seconds: {proofs}");
}

#[test]
fn blocks_without_their_bodies_or_receipts_say_so_rather_than_speak_for_them() {
    let mut fixture = alices_chain();
    let (_, mut reading) = shelf();
    read_all(&mut reading, &fixture, &[alice()], Some(9));

    let (signed, receipt) = pay(2, alice(), 5, 1);
    fixture.push(Making { txs: vec![signed.clone()], receipts: vec![receipt.clone()], no_receipts: true, writes: vec![account_entry(&alice(), 125, 1)], ..Making::default() });
    fixture.push(Making { txs: vec![signed], receipts: vec![receipt], header_only: true, no_proofs: Some(NoProofs::NoList), ..Making::default() });
    fixture.final_height = 12;
    read_all(&mut reading, &fixture, &[alice()], Some(9));

    let standing = reading.standing().unwrap();
    assert_eq!(standing.unread, 2, "a body without receipts, and a header alone");
    let (_, checked) = shown(&mut reading);
    assert!(checked.values().all(|tx| tx.height < 11), "neither claims to know what they held");
}

#[test]
fn an_entry_left_off_a_blocks_list_stays_stale_until_check_now_proves_it_and_finds_it_out() {
    let mut fixture = alices_chain();
    let (_, mut reading) = shelf();
    read_all(&mut reading, &fixture, &[alice()], Some(9));

    // The block changed her balance, and the node's list leaves that out: every check passes.
    fixture.push(Making { writes: vec![account_entry(&alice(), 1, 2)], left_out: vec![address_key(&alice())], ..Making::default() });
    fixture.final_height = 11;
    read_all(&mut reading, &fixture, &[alice()], Some(9));

    let (chain, _) = shown(&mut reading);
    assert_eq!(balance(&chain, &alice()), Some(u256::from(120u64)), "stale, and nothing can tell");
    assert_eq!(reading.standing().unwrap().stale, 0, "as far as it knows, in date");
    assert!(reading.standing().unwrap().omitted.is_empty());

    reading.check_now();
    assert!(reading.checks_left() > 0);
    read_all(&mut reading, &fixture, &[alice()], Some(9));

    let (chain, _) = shown(&mut reading);
    let standing = reading.standing().unwrap();
    assert_eq!(balance(&chain, &alice()), Some(u256::from(1u64)), "Check now brought it up to date");
    assert_eq!(standing.checks, 1);
    assert_eq!(standing.proofs, 1, "counted apart from those it takes by itself");
    assert_eq!(standing.omitted, [format!("the account {}", alice())], "and says what the node left out");
    assert_eq!(reading.checks_left(), 0);

    // A check that finds nothing says nothing.
    reading.check_now();
    read_all(&mut reading, &fixture, &[alice()], Some(9));
    assert!(reading.standing().unwrap().omitted.is_empty());
}

/// What is kept of an index, past what a test expects to move.
fn kept(reading: &Reading) -> (Index, Place) {
    let open = reading.open.as_ref().unwrap();

    (open.index.clone(), open.scanner.as_ref().unwrap().place().clone())
}

#[test]
fn tampered_truncated_and_hostile_answers_leave_the_index_as_it_was_and_say_why() {
    let mut fixture = alices_chain();
    fixture.final_height = 5;
    let (_, mut reading) = shelf();
    read_all(&mut reading, &fixture, &[alice()], Some(0));
    fixture.final_height = 10;

    let hostile = |change: &dyn Fn(&mut BlockRange)| {
        let (_, mut fresh) = shelf();
        let mut before = alices_chain();
        before.final_height = 5;
        read_all(&mut fresh, &before, &[alice()], Some(0));
        before.final_height = 10;
        let go = fresh.go(wanted(7, vec![alice()], Some(0)), &before, 1_000.0);
        let asked = go.range.unwrap();
        let mut range = before.range(asked);
        change(&mut range);
        let (index, place) = kept(&fresh);

        fresh.came(asked, answered(&range), &before, 1_000.0);

        let (after, place_after) = kept(&fresh);
        assert_eq!(index, after, "the index is as it was");
        assert_eq!(place.high, place_after.high);
        assert_eq!(place.read, place_after.read);
        assert_eq!(balance(&{ let mut chain = Chain::default(); fresh.show(HASH, &mut chain, &mut HashMap::new()); chain }, &alice()), Some(u256::from(150u64)));

        let said = fresh.standing().unwrap().trouble.expect("it says why");

        assert!(fresh.go(wanted(7, vec![alice()], Some(0)), &before, 2_000.0).range.is_none(), "and goes no further");
        fresh.retry();
        assert!(fresh.go(wanted(7, vec![alice()], Some(0)), &before, 2_001.0).range.is_some(), "until told to try again");

        said
    };

    // Block 8 changes her balance: a false value for it fails its proof.
    let tampered = hostile(&|range| {
        let item = range.items.iter_mut().find(|item| item.sealed.header.height == 8).unwrap();

        if let RangeChanges::Proven { entries, .. } = &mut item.changes {
            entries[0].value = Some(wincode::serialize(&Account { balance: u256::from(1_000_000u64), ..Account::default() }).unwrap());
        }
    });
    assert!(tampered.contains("proof does not check"), "{tampered}");

    let dropped = hostile(&|range| {
        let item = range.items.iter_mut().find(|item| item.sealed.header.height == 8).unwrap();

        if let RangeBody::Whole { txs, receipts, .. } = &mut item.body {
            txs.clear();
            *receipts = Some(Vec::new());
        }
    });
    assert!(dropped.contains("tx_root"), "a transaction cannot be left out: {dropped}");

    let header = hostile(&|range| range.items[0].sealed.header.state_root = Hash::zero(HASH));
    assert!(header.contains("not the one checked"), "{header}");

    let skipped = hostile(&|range| {
        range.items.remove(1);
    });
    assert!(skipped.contains("out of place"), "{skipped}");

    // And bytes that are no range at all.
    let (index, _) = kept(&reading);
    let go = reading.go(wanted(7, vec![alice()], Some(0)), &fixture, 1_000.0);
    let asked = go.range.unwrap();
    let whole = wincode::serialize(&fixture.range(asked)).unwrap();
    let cut = Came::of(Ok(Answer { ok: true, status: 200, status_text: String::new(), bytes: whole[..whole.len() / 2].to_vec(), retry_after: None }));

    reading.came(asked, cut, &fixture, 1_000.0);
    assert_eq!(kept(&reading).0, index);
    assert!(reading.standing().unwrap().trouble.unwrap().contains("does not decode"));
}

#[test]
fn a_stretch_of_older_blocks_that_does_not_link_up_leaves_nothing_of_itself() {
    let mut other = Fixture::new(&[], tracked_alice());
    let (signed, receipt) = pay(2, alice(), 1_000_000, 0);
    other.push(Making { txs: vec![signed], receipts: vec![receipt], writes: vec![account_entry(&alice(), 1_000_000, 0)], ..Making::default() });
    other.empty_to(10);
    let fixture = alices_chain();
    let (_, mut reading) = shelf();

    let go = reading.go(wanted(7, vec![alice()], Some(0)), &fixture, 0.0);
    let (index, _) = {
        for ask in &go.proofs {
            reading.proof(ask, &fixture.proof_of(ask), &fixture, 0.0);
        }

        kept(&reading)
    };

    // Asked for the top of the older blocks, it is sent another chain's, each checking by its own
    // header: a history made up, seen only by the top not being the block it should be.
    reading.came(go.range.unwrap(), other.answer(go.range.unwrap()), &fixture, 0.0);

    assert_eq!(kept(&reading).0, index, "no transaction of it is taken");
    assert!(reading.standing().unwrap().trouble.unwrap().contains("do not link up"));
    assert_eq!(reading.standing().unwrap().known_from, 11, "and it says it knows nothing of them");
    assert!(shown(&mut reading).1.is_empty());
}

#[test]
fn a_proof_that_does_not_check_or_is_of_another_block_is_not_taken() {
    let fixture = alices_chain();
    let (_, mut reading) = shelf();
    let go = reading.go(wanted(7, vec![alice()], Some(10)), &fixture, 0.0);
    let ask = go.proofs.into_iter().next().unwrap();
    let mut forged = fixture.proof_of(&ask).unwrap();
    forged["value"] = json!(to_hex(&wincode::serialize(&Account { balance: u256::from(77u64), ..Account::default() }).unwrap()));

    reading.proof(&ask, &Ok(forged), &fixture, 0.0);
    assert_eq!(balance(&shown(&mut reading).0, &alice()), None);
    assert!(reading.said.iter().any(|said| said.contains("does not check")), "{:?}", reading.said);

    let mut elsewhere = fixture.proof_of(&ask).unwrap();
    elsewhere["block"] = json!(fixture.items[3].sealed.id().to_string());
    reading.said.clear();

    let go = reading.go(wanted(7, vec![alice()], Some(10)), &fixture, 100.0);
    let ask = go.proofs.into_iter().next().unwrap();
    reading.proof(&ask, &Ok(elsewhere), &fixture, 100.0);
    assert!(reading.said.iter().any(|said| said.contains("another block")), "{:?}", reading.said);
    assert_eq!(reading.standing().unwrap().proofs, 0);

    // A node that says it has no such proof: it asks again later, with nothing taken.
    let go = reading.go(wanted(7, vec![alice()], Some(10)), &fixture, 200.0);
    let ask = go.proofs.into_iter().next().unwrap();
    reading.proof(&ask, &Err(Failure { status: 404, message: "pruned".to_string() }), &fixture, 200.0);
    assert!(!reading.go(wanted(7, vec![alice()], Some(10)), &fixture, 300.0).proofs.is_empty());
}

#[test]
fn a_busy_node_is_waited_for_and_read_on_from_where_it_was() {
    let fixture = alices_chain();
    let (_, mut reading) = shelf();
    let go = reading.go(wanted(7, vec![alice()], Some(10)), &fixture, 0.0);
    let asked = go.range.unwrap();
    let busy = |status: u16, retry_after| Came::of(Ok(Answer { ok: false, status, status_text: String::new(), bytes: b"{\"error\":\"too many ranges\"}".to_vec(), retry_after }));

    reading.came(asked, busy(429, Some(2)), &fixture, 0.0);
    assert!(reading.standing().unwrap().waiting.unwrap().contains("busy"));
    assert_eq!(reading.go(wanted(7, vec![alice()], Some(10)), &fixture, 1.0).range, None);

    let go = reading.go(wanted(7, vec![alice()], Some(10)), &fixture, 2.0);
    assert_eq!(go.range, Some(asked));
    reading.came(asked, busy(503, None), &fixture, 2.0);
    assert_eq!(reading.standing().unwrap().trouble, None, "it is no trouble");

    let go = reading.go(wanted(7, vec![alice()], Some(10)), &fixture, 10.0);
    reading.came(go.range.unwrap(), fixture.answer(go.range.unwrap()), &fixture, 10.0);
    assert_eq!(reading.standing().unwrap().read, 1);
}

#[test]
fn an_older_node_without_the_route_leaves_the_wallet_saying_so_and_asking_nothing_else() {
    let fixture = alices_chain();
    let (_, mut reading) = shelf();
    let go = reading.go(wanted(7, vec![alice()], Some(10)), &fixture, 0.0);

    for ask in &go.proofs {
        reading.proof(ask, &fixture.proof_of(ask), &fixture, 0.0);
    }

    let refused = Came::of(Ok(Answer { ok: false, status: 400, status_text: String::new(), bytes: b"{\"error\":\"not a block: range\"}".to_vec(), retry_after: None }));
    reading.came(go.range.unwrap(), refused, &fixture, 0.0);

    let said = reading.standing().unwrap().trouble.unwrap();
    assert!(said.contains("does not serve checked blocks") && said.contains("Nothing is asked of the node instead"), "{said}");

    let go = reading.go(wanted(7, vec![alice()], Some(10)), &fixture, 100.0);
    assert!(go.range.is_none() && go.proofs.is_empty());
}

#[test]
fn a_hole_the_light_client_left_hides_balances_until_it_is_read_and_a_node_without_it_makes_a_gap() {
    let mut fixture = alices_chain();
    fixture.final_height = 2;
    let (_, mut reading) = shelf();
    read_all(&mut reading, &fixture, &[alice()], Some(0));
    let (mut chain, mut checked) = (Chain::default(), HashMap::new());
    reading.show(HASH, &mut chain, &mut checked);
    assert_eq!(balance(&chain, &alice()), Some(u256::from(100u64)));

    // The wallet was shut while the chain went on, and the light client took it up at block 9.
    fixture.empty_to(10);
    fixture.lowest = 9;
    let go = reading.go(wanted(7, vec![alice()], Some(0)), &fixture, 1_000.0);
    let asked = go.range.unwrap();
    assert_eq!((asked.from, asked.count), (3, 6), "what lies between");
    assert!(reading.standing().unwrap().catching_up);
    reading.show(HASH, &mut chain, &mut checked);
    assert_eq!(balance(&chain, &alice()), None, "what showed goes: it is out of date until this is read");

    reading.came(asked, fixture.answer(asked), &fixture, 1_000.0);
    read_all(&mut reading, &fixture, &[alice()], Some(0));
    reading.show(HASH, &mut chain, &mut checked);

    assert_eq!(balance(&chain, &alice()), Some(u256::from(120u64)));
    assert_eq!(checked.len(), 2, "found on the way");
    assert_eq!(reading.standing().unwrap().known_from, 0);
    assert_eq!(reading.standing().unwrap().proofs, 1, "and no new proof");

    // On a node that holds nothing there, the hole can only be a gap: one proof, and it says so.
    let mut fixture = alices_chain();
    fixture.final_height = 2;
    let (_, mut reading) = shelf();
    read_all(&mut reading, &fixture, &[alice()], Some(0));
    fixture.empty_to(10);
    fixture.lowest = 9;
    let mut asks = 0;

    for time in 0..40 {
        let time = 1_000.0 + f64::from(time) * 400.0;
        let go = reading.go(wanted(7, vec![alice()], Some(0)), &fixture, time);

        for ask in &go.proofs {
            reading.proof(ask, &fixture.proof_of(ask), &fixture, time);
        }

        if let Some(asked) = go.range {
            asks += 1;
            let came = match asked.from < 9 {
                true => Came::Refused { status: 404, message: "no such block".to_string(), bytes: 0 },
                false => fixture.answer(asked),
            };
            reading.came(asked, came, &fixture, time);
        }
    }

    assert!(asks < 20);
    let standing = reading.standing().unwrap();
    assert_eq!((standing.catching_up, standing.high, standing.stale), (false, 10, 0));
    assert!(standing.state_gap.is_some_and(|gap| gap >= 3));
    assert!(standing.unread > 0);
    assert_eq!(standing.proofs, 2, "the one to start, and the one that caught up");
    assert_eq!(balance(&shown(&mut reading).0, &alice()), Some(u256::from(120u64)));
}

#[test]
fn a_reading_put_away_goes_on_from_where_it_was_and_reads_nothing_twice() {
    let mut fixture = alices_chain();
    fixture.final_height = 5;
    let (shelf, mut reading) = shelf();
    read_all(&mut reading, &fixture, &[alice()], Some(0));
    let (mut chain, mut checked) = (Chain::default(), HashMap::new());
    reading.show(HASH, &mut chain, &mut checked);
    reading.close(&mut chain, &mut checked, 100.0);

    assert!(checked.is_empty(), "nothing read shows once it is put away");
    assert_eq!(balance(&chain, &alice()), None);
    assert!(!reading.is_open());

    // A new run: the same shelf, and more final.
    let mut again = Reading::new(Keeper::new(shelf.clone()), true);
    fixture.final_height = 10;
    let asks = read_all(&mut again, &fixture, &[alice()], Some(0));
    let standing = again.standing().unwrap();

    assert_eq!(asks, 1, "the five new blocks in one ask, and nothing older");
    assert_eq!((standing.high, standing.known_from, standing.read, standing.proofs), (10, 0, 11, 1));

    let (chain, checked) = shown(&mut again);
    assert_eq!(balance(&chain, &alice()), Some(u256::from(120u64)));
    assert_eq!(checked.len(), 2, "what was read before shows again, and what is new");
}

#[test]
fn a_reading_stopped_while_going_down_starts_that_stretch_again_and_keeps_what_stood() {
    let mut fixture = alices_chain();
    fixture.empty_to(350);
    let (shelf, mut reading) = shelf();
    let go = reading.go(wanted(7, vec![alice()], Some(0)), &fixture, 0.0);
    reading.proof(&go.proofs[0], &fixture.proof_of(&go.proofs[0]), &fixture, 0.0);
    reading.came(go.range.unwrap(), fixture.answer(go.range.unwrap()), &fixture, 0.0);

    let go = reading.go(wanted(7, vec![alice()], Some(0)), &fixture, 1.0);
    reading.came(go.range.unwrap(), fixture.answer(go.range.unwrap()), &fixture, 1.0);
    let known = reading.standing().unwrap().known_from;
    assert_eq!(known, 151, "two stretches stood");

    reading.save(2.0);
    let mut again = Reading::new(Keeper::new(shelf.clone()), true);
    read_all(&mut again, &fixture, &[alice()], Some(0));
    let standing = again.standing().unwrap();

    assert_eq!((standing.known_from, standing.left_below, standing.read), (0, 0, 351));
    assert_eq!(shown(&mut again).1.len(), 2);
}

#[test]
fn reading_further_back_finds_older_history_and_leaves_what_it_knew_alone() {
    let fixture = alices_chain();
    let (_, mut reading) = shelf();

    read_all(&mut reading, &fixture, &[alice()], Some(6));
    let (chain, checked) = shown(&mut reading);
    assert_eq!(balance(&chain, &alice()), Some(u256::from(120u64)));
    assert_eq!(checked.len(), 1, "block 8 alone");
    assert_eq!(reading.standing().unwrap().known_from, 6);

    assert!(reading.further_back(&fixture, 1));
    assert!(!reading.further_back(&fixture, 2), "that much it does already");
    let asks = read_all(&mut reading, &fixture, &[alice()], Some(6));
    let standing = reading.standing().unwrap();

    assert_eq!(asks, 1, "only what lies below");
    assert_eq!((standing.known_from, standing.read), (1, 10));
    let (chain, checked) = shown(&mut reading);
    assert_eq!(checked.len(), 1, "what is newly shown: block 3");
    assert_eq!(checked.values().next().unwrap().height, 3);
    assert_eq!(balance(&chain, &alice()), Some(u256::from(120u64)), "what block 3 says of her is older than what it knows");
    assert_eq!(standing.proofs, 1, "and no new proof");
}

#[test]
fn an_account_added_later_takes_a_proof_and_reading_back_for_it_reads_it_all_again() {
    let fixture = alices_chain();
    let (_, mut reading) = shelf();

    read_all(&mut reading, &fixture, &[alice()], Some(6));
    read_all(&mut reading, &fixture, &[alice(), bob()], Some(6));

    let (chain, _) = shown(&mut reading);
    assert_eq!(balance(&chain, &bob()), Some(u256::from(30u64)));
    assert_eq!(reading.standing().unwrap().proofs, 2, "one each");

    assert!(reading.further_back(&fixture, 5), "its history lies in what was passed over");
    read_all(&mut reading, &fixture, &[alice(), bob()], Some(6));
    let (_, checked) = shown(&mut reading);

    assert!(checked.values().any(|tx| tx.height == 8), "block 8 paid it, found now");
}

#[test]
fn a_token_first_seen_going_up_takes_its_balance_from_the_block_and_one_proof_of_its_record() {
    let token_record = wincode::serialize(&Token { address: token(), name: None, symbol: Some("TKN".to_string()), decimals: Some(2) }).unwrap();
    let balance_key = token_balance_key(&token(), &alice());
    let mut tracked = tracked_alice();
    tracked.push((Column::Tokens, token_key(&token())));
    tracked.push((Column::Namespaces, balance_key.clone()));
    let mut fixture = Fixture::new(&[(Column::Tokens, token_key(&token()), token_record)], tracked);
    fixture.empty_to(2);
    let (_, mut reading) = shelf();
    read_all(&mut reading, &fixture, &[alice()], Some(2));
    let start_proofs = reading.standing().unwrap().proofs;

    let signed = SignedTx::sign(Tx::ContractCall { to: token(), func: Hash::zero(HashType::Xxh3_64), amount: Amount::U128(0), data: vec![TxCallArgumentType::Address(alice()), TxCallArgumentType::U256(u256::from(5u64))] }, 0, FeeOffer::NONE, &key(2), &chain());
    let event = Event { contract: token(), name: "Transfer".to_string(), data: vec![TxCallArgumentType::Address(bob()), TxCallArgumentType::Address(alice()), TxCallArgumentType::U256(u256::from(5u64))] };
    let receipt = CommittedReceipt::new(signed.id(HASH), 0, 90_000, u256::ZERO, CommittedOutcome::Done, vec![event], Vec::new());
    fixture.push(Making { txs: vec![signed.clone()], receipts: vec![receipt.clone()], writes: vec![(Column::Namespaces, balance_key.clone(), Some(b"5".to_vec()))], ..Making::default() });
    fixture.final_height = 3;
    read_all(&mut reading, &fixture, &[alice()], Some(2));

    let (chain, checked) = shown(&mut reading);
    let text = (token().to_string(), alice().to_string());

    assert_eq!(chain.balances.get(&text).unwrap()[0].value, u256::from(5u64), "from the block");
    assert_eq!(chain.listed, [token().to_string()]);
    assert_eq!(chain.tokens[&token().to_string()].value.symbol.as_deref(), Some("TKN"), "from one proof of its record");
    assert_eq!(reading.standing().unwrap().proofs, start_proofs + 1);
    assert_eq!(checked.values().filter(|tx| tx.height == 3).count(), 1);

    // Later entries come on their own, for it is followed now.
    fixture.push(Making { txs: vec![signed], receipts: vec![receipt], writes: vec![(Column::Namespaces, balance_key.clone(), Some(b"8".to_vec()))], ..Making::default() });
    fixture.final_height = 4;
    read_all(&mut reading, &fixture, &[alice()], Some(2));

    assert_eq!(shown(&mut reading).0.balances.get(&text).unwrap()[0].value, u256::from(8u64));
    assert_eq!(reading.standing().unwrap().proofs, start_proofs + 1);
}

#[test]
fn a_token_first_seen_among_many_blocks_of_one_answer_takes_no_proof_of_its_balance() {
    let balance_key = token_balance_key(&token(), &alice());
    let mut tracked = tracked_alice();
    tracked.push((Column::Tokens, token_key(&token())));
    tracked.push((Column::Namespaces, balance_key.clone()));
    let mut fixture = Fixture::new(&[], tracked);
    fixture.empty_to(1);
    let (_, mut reading) = shelf();
    read_all(&mut reading, &fixture, &[alice()], Some(1));
    let signed = SignedTx::sign(Tx::ContractCall { to: token(), func: Hash::zero(HashType::Xxh3_64), amount: Amount::U128(0), data: Vec::new() }, 0, FeeOffer::NONE, &key(2), &chain());
    let event = Event { contract: token(), name: "Transfer".to_string(), data: vec![TxCallArgumentType::Address(bob()), TxCallArgumentType::Address(alice()), TxCallArgumentType::U256(u256::from(5u64))] };
    let receipt = CommittedReceipt::new(signed.id(HASH), 0, 90_000, u256::ZERO, CommittedOutcome::Done, vec![event], Vec::new());
    fixture.push(Making { txs: vec![signed], receipts: vec![receipt], writes: vec![(Column::Namespaces, balance_key.clone(), Some(b"5".to_vec()))], ..Making::default() });
    // It changes again in the same answer: what picked it up goes on looking for it.
    fixture.push(Making::default());
    fixture.push(Making { writes: vec![(Column::Namespaces, balance_key.clone(), Some(b"3".to_vec()))], ..Making::default() });
    fixture.empty_to(8);

    let go = reading.go(wanted(7, vec![alice()], Some(1)), &fixture, 100.0);
    let asked = go.range.unwrap();
    assert_eq!(asked.count, 7, "all in one");
    reading.came(asked, fixture.answer(asked), &fixture, 100.0);

    let open = reading.open.as_ref().unwrap();
    let name = known_key(Column::Namespaces, &balance_key);
    assert_eq!(open.index.known[&name].at.as_ref().unwrap().height, 4);
    assert!(!open.index.stale(&open.index.known[&name]), "in date, so no proof tells the node");
    assert_eq!(shown(&mut reading).0.balances[&(token().to_string(), alice().to_string())][0].value, u256::from(3u64));
}

#[test]
fn a_token_first_seen_going_down_takes_a_proof_as_what_it_read_there_is_old() {
    let balance_key = token_balance_key(&token(), &alice());
    let mut tracked = tracked_alice();
    tracked.push((Column::Tokens, token_key(&token())));
    tracked.push((Column::Namespaces, balance_key.clone()));
    let mut fixture = Fixture::new(&[], tracked);
    fixture.empty_to(2);
    let signed = SignedTx::sign(Tx::ContractCall { to: token(), func: Hash::zero(HashType::Xxh3_64), amount: Amount::U128(0), data: Vec::new() }, 0, FeeOffer::NONE, &key(2), &chain());
    let event = Event { contract: token(), name: "Transfer".to_string(), data: vec![TxCallArgumentType::Address(bob()), TxCallArgumentType::Address(alice()), TxCallArgumentType::U256(u256::from(5u64))] };
    let receipt = CommittedReceipt::new(signed.id(HASH), 0, 90_000, u256::ZERO, CommittedOutcome::Done, vec![event], Vec::new());
    fixture.push(Making { txs: vec![signed], receipts: vec![receipt], writes: vec![(Column::Namespaces, balance_key.clone(), Some(b"5".to_vec()))], ..Making::default() });
    // It changes again, naming nobody, in a block that is read before this one is.
    fixture.push(Making { writes: vec![(Column::Namespaces, balance_key.clone(), Some(b"3".to_vec()))], ..Making::default() });
    fixture.empty_to(6);
    let (_, mut reading) = shelf();
    read_all(&mut reading, &fixture, &[alice()], Some(0));

    let (chain, _) = shown(&mut reading);
    let text = (token().to_string(), alice().to_string());
    assert_eq!(chain.balances.get(&text).unwrap()[0].value, u256::from(3u64), "as it stands, not as block 3 left it");
}

#[test]
fn an_index_is_sealed_no_address_reads_in_it_and_nothing_goes_on_while_locked() {
    let fixture = alices_chain();
    let (shelf, mut reading) = shelf();
    read_all(&mut reading, &fixture, &[alice()], Some(0));
    let (mut chain, mut checked) = shown(&mut reading);

    reading.close(&mut chain, &mut checked, 1.0);

    let held = shelf.held();
    assert_eq!(held.len(), 1);
    assert_eq!(held[0].0, "wallet-0xaa", "named for its wallet and chain, and nothing else");

    let bytes = &held[0].1;
    let text = String::from_utf8_lossy(bytes);
    let alices = alice().to_string();

    for hidden in [alices.as_str(), &alices[alices.len() - 16..], "balance", "Transfer"] {
        assert!(!text.contains(hidden), "{hidden} reads in the index");
    }

    let raw = alice().bytes().to_vec();
    assert!(!bytes.windows(raw.len()).any(|window| window == raw), "nor the address's bytes");

    assert!(!reading.is_open(), "put away as the wallet locks");
    assert!(reading.standing().is_none());
    assert!(!reading.needs_floor());
    assert!(checked.is_empty() && chain.accounts.is_empty(), "and none of it shows");

    // Another key does not open it: another wallet, or a password that does not open this one.
    let mut other = Reading::new(Keeper::new(shelf.clone()), true);
    other.go(wanted(8, vec![alice()], Some(0)), &fixture, 0.0);
    other.go(wanted(8, vec![alice()], Some(0)), &fixture, 0.0);
    assert!(other.said.iter().any(|said| said.contains("set aside")), "{:?}", other.said);
}

#[test]
fn an_index_sealed_with_a_new_password_opens_with_it() {
    let fixture = alices_chain();
    let (shelf, mut reading) = shelf();
    read_all(&mut reading, &fixture, &[alice()], Some(0));

    // The password changes with the wallet open.
    reading.go(Wanted { vault_key: Zeroizing::new([9; 32]), ..wanted(7, vec![alice()], Some(0)) }, &fixture, 1_000.0);
    reading.save(1_000.0);

    let mut again = Reading::new(Keeper::new(shelf.clone()), true);
    let opened = |reading: &mut Reading| {
        reading.go(Wanted { vault_key: Zeroizing::new([9; 32]), ..wanted(7, vec![alice()], Some(0)) }, &fixture, 0.0);
        reading.go(Wanted { vault_key: Zeroizing::new([9; 32]), ..wanted(7, vec![alice()], Some(0)) }, &fixture, 0.0);
    };
    opened(&mut again);

    assert!(again.said.is_empty(), "{:?}", again.said);
    assert_eq!(again.standing().unwrap().high, 10);
}

#[test]
fn a_damaged_index_is_set_aside_read_again_and_says_so() {
    let fixture = alices_chain();
    let (shelf, mut reading) = shelf();
    read_all(&mut reading, &fixture, &[alice()], Some(0));
    reading.save(1_000.0);

    for damage in [vec![1, 2, 3], vec![0; 100]] {
        shelf.write("wallet-0xaa", damage, Box::new(|result| result.unwrap()));
        let mut again = Reading::new(Keeper::new(shelf.clone()), true);

        read_all(&mut again, &fixture, &[alice()], Some(0));

        assert!(again.said.iter().any(|said| said.contains("set aside") && said.contains("wallet itself is untouched")), "{:?}", again.said);
        assert!(shelf.held().iter().any(|(name, _)| name.contains("unreadable")), "kept, not written over");
        assert_eq!(balance(&shown(&mut again).0, &alice()), Some(u256::from(120u64)), "and built again");
        again.save(1_000.0);
    }

    // What opens but is not an index goes the same way.
    let key = index_key(&[7; 32]).unwrap();
    shelf.write("wallet-0xaa", seal(&key, b"not an index").unwrap(), Box::new(|result| result.unwrap()));
    let mut again = Reading::new(Keeper::new(shelf.clone()), true);
    again.go(wanted(7, vec![alice()], Some(0)), &fixture, 0.0);
    again.go(wanted(7, vec![alice()], Some(0)), &fixture, 0.0);
    assert!(again.said.iter().any(|said| said.contains("does not read")), "{:?}", again.said);
}

#[test]
fn an_index_beside_a_wallets_files_is_never_taken_for_one_and_a_damaged_one_touches_no_wallet() {
    use crate::app::{Settings, WalletEntry};
    use crate::vault::{Book, Vault};

    let dir = std::env::temp_dir().join(format!("chainv4-reading-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).unwrap();
    let mut store = crate::store::Store::new(&dir).unwrap();
    let vault = Vault::create("").unwrap();
    let mut settings = Settings::with_wallets(vec![WalletEntry { id: "aaaa".to_string(), name: "Mine".to_string(), vault: vault.seal(&Book::default()).unwrap(), current: String::new(), biometric: None, unprotected: true, born: None }]);
    store.save(&mut settings);
    assert!(store.wait());
    let folder = store.folder().to_path_buf();
    std::fs::write(folder.join("aaaa-0xaa.index"), b"damaged").unwrap();

    let fixture = alices_chain();
    let mut reading = on_this_device(Some(&folder));
    let waited = |reading: &mut Reading| {
        for _ in 0..200 {
            let go = reading.go(Wanted { wallet: "aaaa", ..wanted(7, vec![alice()], Some(0)) }, &fixture, 0.0);

            if reading.is_open() {
                return go;
            }

            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        panic!("it never opened");
    };

    waited(&mut reading);
    assert!(reading.said.iter().any(|said| said.contains("set aside")), "{:?}", reading.said);
    let loaded = crate::store::Store::new(&dir).unwrap().load();
    assert_eq!(loaded.wallets.len(), 1, "the wallet is there");
    assert!(loaded.trouble.is_empty(), "{:?}", loaded.trouble);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn every_kind_of_touching_a_wallet_counts_and_nothing_else_does() {
    let ours = Ours::of(&[alice()]);
    let receipt = |events, payments| CommittedReceipt::new(Hash::zero(HASH), 0, 0, u256::ZERO, CommittedOutcome::Done, events, payments);
    let signed = |tx, seed| SignedTx::sign(tx, 0, FeeOffer::NONE, &key(seed), &chain());
    let carol = key(3).public().address();
    let plain = Tx::Transfer { from: bob(), to: carol, amount: Amount::U128(1) };

    assert!(!touches(&signed(plain.clone(), 2), &receipt(Vec::new(), Vec::new()), &ours), "nothing of hers");
    assert!(touches(&signed(plain.clone(), 1), &receipt(Vec::new(), Vec::new()), &ours), "she sent it");
    assert!(touches(&signed(Tx::Transfer { from: bob(), to: alice(), amount: Amount::U128(1) }, 2), &receipt(Vec::new(), Vec::new()), &ours), "it pays her");
    assert!(touches(&signed(Tx::ContractCall { to: token(), func: Hash::zero(HashType::Xxh3_64), amount: Amount::U128(0), data: vec![TxCallArgumentType::Address(alice())] }, 2), &receipt(Vec::new(), Vec::new()), &ours), "named in a call");
    assert!(touches(&signed(plain.clone(), 2), &receipt(Vec::new(), vec![chainv4::structs::receipt::Payment { from: token(), to: alice(), amount: u256::from(1u64) }]), &ours), "a contract paid her");

    let native = Event { contract: token(), name: "Transfer".to_string(), data: vec![TxCallArgumentType::Address(bob()), TxCallArgumentType::Address(alice()), TxCallArgumentType::U256(u256::from(5u64))] };
    assert!(touches(&signed(plain.clone(), 2), &receipt(vec![native], Vec::new()), &ours), "a token's event names her");

    let evm = chainv4::abi::evm_address(&alice());
    let mut topic = [0; 32];
    topic[12..].copy_from_slice(&evm);
    let log = |topics: &[[u8; 32]]| {
        let mut data: Vec<TxCallArgumentType> = topics.iter().map(|topic| TxCallArgumentType::Bytes(topic.to_vec())).collect();
        data.push(TxCallArgumentType::Bytes(Vec::new()));

        Event { contract: Address::Evm([5; 20]), name: chainv4::structs::event::EVM_LOG.to_string(), data }
    };
    assert!(touches(&signed(plain.clone(), 2), &receipt(vec![log(&[[1; 32], topic])], Vec::new()), &ours), "an EVM log's topic");
    assert!(!touches(&signed(plain.clone(), 2), &receipt(vec![log(&[[1; 32], [2; 32]])], Vec::new()), &ours), "another's");
    assert!(ours.is_ours(&alice().to_string()) && !ours.is_ours(&bob().to_string()) && !ours.is_ours("nonsense"));
}

#[test]
fn fees_are_suggested_from_what_checked_blocks_show_and_rest_on_nothing_asked() {
    let fixture = alices_chain();
    let (_, mut reading) = shelf();
    read_all(&mut reading, &fixture, &[alice()], Some(0));

    let transfer = Tx::Transfer { from: alice(), to: bob(), amount: Amount::U128(1) };
    let (offer, seen) = reading.suggest(&transfer, u256::from(3u64), u256::from(1u64));

    assert!(seen, "transfers were read");
    assert_eq!(offer.gas_limit, 21_000 + 5_250, "what it used and a quarter more");
    assert_eq!(offer.max_price, u256::from(7u64 + 1), "what was seen paid, 7, being above twice the base; and the tip");
    assert_eq!(offer.tip, u256::from(1u64));

    let (offer, _) = reading.suggest(&transfer, u256::from(50u64), u256::ZERO);
    assert_eq!(offer.max_price, u256::from(100u64), "twice the checked base price");

    // Nothing of its kind read: a round guess, and it says so.
    let call = Tx::ContractCall { to: token(), func: Hash::zero(HashType::Xxh3_64), amount: Amount::U128(0), data: Vec::new() };
    let (offer, seen) = reading.suggest(&call, u256::from(5u64), u256::ZERO);
    assert!(!seen);
    assert_eq!(offer.gas_limit, GUESS_CALL_GAS);

    // A chain without fees offers nothing.
    let free = Fixture::new(&[], tracked_alice());
    let (_, mut reading) = shelf();
    read_all(&mut reading, &free, &[alice()], Some(0));
    assert_eq!(reading.suggest(&transfer, u256::ZERO, u256::ZERO).0, FeeOffer::NONE);

    // A figure gone old gives way to a lower one; a fresh higher one always replaces it.
    let mut index = Index::default();
    index.fees.take(10, &[("x".to_string(), 100)], None);
    index.fees.take(20, &[("x".to_string(), 50)], None);
    assert_eq!(index.fees.hinted("x"), Some(100));
    index.fees.take(10 + HINT_BLOCKS + 1, &[("x".to_string(), 50)], None);
    assert_eq!(index.fees.hinted("x"), Some(50));
    index.fees.take(10 + HINT_BLOCKS + 2, &[("x".to_string(), 70)], None);
    assert_eq!(index.fees.hinted("x"), Some(70));
}

#[test]
fn what_is_read_never_reaches_the_settings_file_and_the_reading_takes_over_from_a_lookup() {
    let fixture = alices_chain();
    let (_, mut reading) = shelf();
    read_all(&mut reading, &fixture, &[alice()], Some(0));
    let mut settings = crate::app::Settings::default();
    let mut chain = Chain::default();
    let tx = pay(1, bob(), 30, 0).0.id(HASH).to_string();

    // Looked up first, it counts as not final.
    let looked = Checked { final_: false, came: Origin::Looked, ..shown(&mut reading).1.remove(&tx).unwrap() };
    settings.checked.insert(tx.clone(), looked);
    let (old_id, old) = ("0xolder".to_string(), Checked { came: Origin::Proven, ..settings.checked[&tx].clone() });
    settings.checked.insert(old_id.clone(), old);

    let written = |settings: &crate::app::Settings| serde_json::to_value(settings).unwrap()["checked"].clone();
    assert_eq!(written(&settings).as_object().unwrap().keys().collect::<Vec<_>>(), [&old_id], "what was looked up stays out");

    // The reading reaches it: what it found stands.
    reading.open.as_mut().unwrap().fresh.push(tx.clone());
    reading.show(HASH, &mut chain, &mut settings.checked);
    assert_eq!((settings.checked[&tx].came, settings.checked[&tx].final_), (Origin::Read, true));
    assert_eq!(written(&settings).as_object().unwrap().keys().collect::<Vec<_>>(), [&old_id], "and what was read stays out");

    // As the wallet locks, it all goes from memory but what was kept before.
    reading.close(&mut chain, &mut settings.checked, 0.0);
    assert_eq!(settings.checked.keys().collect::<Vec<_>>(), [&old_id]);
}

#[test]
fn settings_with_the_setting_off_are_written_as_they_always_were() {
    let settings = crate::app::Settings::default();
    let written = serde_json::to_value(&settings).unwrap();

    for field in ["read_blocks", "coins", "just_made", "forget_reading", "forget_indexes"] {
        assert!(written.get(field).is_none(), "{field}");
    }

    let mut on = crate::app::Settings::default();
    on.read_blocks = true;
    let kept: crate::app::Settings = serde_json::from_value(serde_json::to_value(&on).unwrap()).unwrap();
    assert!(kept.read_blocks);
    assert!(!serde_json::from_value::<crate::app::Settings>(json!({ "node": "http://node" })).unwrap().read_blocks, "off, for settings from before it");
}

/// A node on a socket, answering each request by what this says, in turn: each answer's whole text.
fn serving(answers: Vec<Vec<u8>>) -> (String, std::thread::JoinHandle<Vec<String>>) {
    use std::io::{Read, Write};

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let served = std::thread::spawn(move || {
        let mut heard = Vec::new();

        for answer in answers {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 2048];
            let read = stream.read(&mut request).unwrap();
            heard.push(String::from_utf8_lossy(&request[..read]).lines().next().unwrap_or_default().to_string());
            stream.write_all(&answer).unwrap();
        }

        heard
    });

    (base, served)
}

#[test]
fn a_range_comes_over_a_real_connection_waits_out_a_busy_node_and_is_read() {
    let mut fixture = alices_chain();
    fixture.final_height = 10;
    let (_, mut reading) = shelf();
    let go = reading.go(wanted(7, vec![alice()], Some(9)), &fixture, 0.0);

    for ask in &go.proofs {
        reading.proof(ask, &fixture.proof_of(ask), &fixture, 0.0);
    }

    let asked = go.range.unwrap();
    let bytes = wincode::serialize(&fixture.range(asked)).unwrap();
    let mut body = format!("HTTP/1.1 200 OK\r\ncontent-type: application/octet-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n", bytes.len()).into_bytes();
    body.extend_from_slice(&bytes);
    let busy = b"HTTP/1.1 429 Too Many Requests\r\nretry-after: 3\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}".to_vec();
    let (base, served) = serving(vec![busy, body]);
    let node = crate::node::Node::new(&base, Ok(chainv4_ui::net::Client::direct()), egui::Context::default());

    let wait_for = |node: &crate::node::Node| {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);

        loop {
            if let Some(found) = node.ranges().into_iter().next() {
                return found;
            }

            assert!(std::time::Instant::now() < deadline, "no answer");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    };

    node.read_range(asked);
    let (answered_for, came) = wait_for(&node);
    assert_eq!(answered_for, asked);
    assert!(matches!(came, Came::Busy { wait: 3, .. }), "{came:?}");
    reading.came(asked, came, &fixture, 0.0);
    assert_eq!(reading.go(wanted(7, vec![alice()], Some(9)), &fixture, 2.9).range, None, "it said 3");

    let again = reading.go(wanted(7, vec![alice()], Some(9)), &fixture, 3.0).range.unwrap();
    node.read_range(again);
    let (answered_for, came) = wait_for(&node);
    reading.came(answered_for, came, &fixture, 3.0);

    assert_eq!(reading.standing().unwrap().read, 2);
    assert!(reading.standing().unwrap().trouble.is_none());
    assert_eq!(served.join().unwrap()[1], format!("GET {} HTTP/1.1", asked.path()));
}

/// Asks a node, as the wallet does, and takes the answers, until told to stop: what a real chain comes
/// to. It keeps count of everything that went out.
#[test]
#[ignore = "needs a node with voting finality: CHAINV4_NODE=http://127.0.0.1:8080 CHAINV4_ACCOUNT=ed25519:0x… cargo test -- --ignored"]
fn a_reading_of_a_real_node_shows_what_its_own_accounts_page_says() {
    let (base, account) = (std::env::var("CHAINV4_NODE").unwrap(), std::env::var("CHAINV4_ACCOUNT").unwrap());
    let address: Address = account.parse().unwrap();
    let mut node = crate::node::Node::new(&base, Ok(chainv4_ui::net::Client::direct()), egui::Context::default()).reading_blocks(true);
    let (mut chain, mut checked) = (Chain::default(), HashMap::new());
    let mut pinned = HashMap::new();
    let (saved, checkpoints) = (HashMap::new(), HashMap::new());
    let dir = std::env::temp_dir().join(format!("chainv4-reading-live-{}", std::process::id()));
    let mut reading = on_this_device(Some(&dir));
    let began = std::time::Instant::now();
    let mut asked = 0;

    while began.elapsed().as_secs() < 120 {
        let time = began.elapsed().as_secs_f64();

        for (ask, result) in node.answers() {
            if let Some(client) = &chain.client
                && reading.proof(&ask, &result, client, time)
            {
                continue;
            }

            let trust = crate::chain::Trust { node: &base, pinned: &mut pinned, checkpoints: &checkpoints, saved: &saved, fast_resume: false };
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
            chain.take(&ask, &result, trust, now, time);
        }

        assert_eq!(chain.trouble, None);

        if let Some(ask) = chain.next_ask(time) {
            node.ask(ask);
        }

        if let (Some(client), Some(genesis), true) = (&chain.client, chain.genesis, chain.has_followed()) {
            for (asked, came) in node.ranges() {
                reading.came(asked, came, client, time);
            }

            let wanted = Wanted { wallet: "live", genesis: &genesis.to_string(), vault_key: Zeroizing::new([3; 32]), addresses: vec![address], born: Some(0) };
            let go = reading.go(wanted, client, time);

            for ask in go.proofs {
                node.ask(ask);
            }

            if let Some(range) = go.range {
                asked += 1;
                node.read_range(range);
            }

            reading.show(client.params().hash, &mut chain, &mut checked);

            if let Some(standing) = reading.standing() {
                assert_eq!(standing.trouble, None);

                let client = chain.client.as_ref().unwrap();

                if standing.known_from == 0 && standing.left_below == 0 && standing.high > 8 && standing.high == client.final_height() && !checked.is_empty() && chain.account(&account).is_some() {
                    break;
                }
            }
        }

        std::thread::sleep(std::time::Duration::from_millis(20));
    }

    let standing = reading.standing().expect("it started");
    let client = chain.client.as_ref().unwrap();
    eprintln!("{standing:#?}\nranges asked: {asked}, transactions read: {}", checked.len());
    assert_eq!(standing.left_below, 0, "it read back to genesis");
    assert_eq!(standing.known_from, 0);
    assert!(standing.high >= 8 && standing.high <= client.final_height());
    assert!(standing.downloaded > 0);
    assert_eq!(standing.proofs, 1, "one direct proof, for the one account");
    assert!(!checked.is_empty(), "it found its transactions in the blocks");
    assert!(checked.values().all(|tx| tx.came == Origin::Read && tx.final_));

    for ask in node.sent() {
        assert!(!ask.needs_asking(), "{ask:?} left");
    }

    // What the node would have said, asked at the block the reading says it stands on, to compare.
    let proven = chain.account(&account).expect("a value").clone();
    let at_block = chainv4_ui::net::Client::direct();
    let (sender, receiver) = std::sync::mpsc::channel();
    at_block.fetch(format!("{base}/accounts/{account}?block={}", proven.assurance.block), None, move |answer| sender.send(answer).unwrap());
    let answer = receiver.recv().unwrap().unwrap();
    let said: Value = serde_json::from_slice(&answer.bytes).unwrap();
    eprintln!("the reading says {} at block {}; the node says {said}", proven.value.balance, proven.assurance.height);
    assert_eq!(said["balance"].as_str(), Some(proven.value.balance.to_string().as_str()));
    assert_eq!(said["nonce"].as_u64(), Some(proven.value.nonce));

    let mut sealed = Vec::new();

    reading.close(&mut chain, &mut checked, 0.0);

    for _ in 0..200 {
        if let Ok(found) = std::fs::read(dir.join(format!("live-{}.index", client_genesis(&pinned, &base)))) {
            sealed = found;

            break;
        }

        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    assert!(!sealed.is_empty(), "it was written out");
    assert!(!String::from_utf8_lossy(&sealed).contains(&account), "and nothing reads in it");
    std::fs::remove_dir_all(&dir).ok();
}

fn client_genesis(pinned: &HashMap<String, String>, base: &str) -> String {
    pinned[base].clone()
}
