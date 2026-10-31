use {
    super::*,
    chainv4::db::{Column, MemoryDb, State, StateTree},
    chainv4::light::{BlockRange, NoProofs, ProvenEntry, RangeItem},
    chainv4::structs::address::Address,
    chainv4::structs::block::{Block, BlockHeader, Seal},
    chainv4::structs::hash::Hash,
    chainv4::structs::receipt::receipts_root,
    chainv4::structs::u256::u256,
    std::sync::Arc,
};

const HASH: HashType = HashType::Blake3_256;

/// A chain of real headers, each block's entries proven against its state root as a node proves
/// them, and sent in the range's real encoding.
struct Chain {
    items: Vec<RangeItem>,
    final_height: u64,
    lowest: u64,
}

impl Followed for Chain {
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

impl Chain {
    /// Blocks up to this height, each writing a key of its own and more if asked.
    fn up_to(top: u64, keys_in: impl Fn(u64) -> u32) -> Chain {
        let state = State::new(Arc::new(MemoryDb::default()));
        let mut items: Vec<RangeItem> = Vec::new();

        for height in 0..=top {
            let mut keys = Vec::new();

            if height > 0 {
                for key in 0..keys_in(height) {
                    let key = format!("{height}/{key}").into_bytes();
                    state.put(Column::Accounts, key.clone(), height.to_le_bytes().to_vec());
                    keys.push(key);
                }
            }

            let tree = StateTree::new(&state, HASH);
            let header = BlockHeader {
                parent: items.last().map_or_else(|| Hash::zero(HASH), |item: &RangeItem| item.sealed.id()),
                height,
                timestamp: height * 1_000,
                producer: Address::Ed25519([0; 32]),
                difficulty: 0,
                tx_root: Block::tx_root(HASH, &[]),
                proof_root: Block::proof_root(HASH, &[]),
                pool_root: Block::pool_root(HASH, None),
                receipts_root: receipts_root(HASH, &[]),
                state_root: tree.follow().unwrap(),
                gas_used: 0,
                base_price: u256::ZERO,
            };
            let entries: Vec<ProvenEntry> = keys.into_iter().map(|key| ProvenEntry { column: Column::Accounts, proof: tree.prove(Column::Accounts, &key).unwrap(), value: Some(height.to_le_bytes().to_vec()), key }).collect();
            let changes = match height {
                0 => RangeChanges::None(NoProofs::NoList),
                _ => RangeChanges::Proven { from: 0, listed: entries.len() as u32, entries },
            };

            items.push(RangeItem { sealed: SealedHeader { header, seal: Seal::None }, body: RangeBody::Whole { txs: Vec::new(), proofs: Vec::new(), pool: None, receipts: Some(Vec::new()) }, changes });
        }

        Chain { final_height: top, lowest: 0, items }
    }

    /// What a node answers: `per_answer` entries at most, cutting a block's list as a node does when
    /// it passes its limits, and the blocks from there on while none is cut.
    fn range(&self, asked: Asked, per_answer: usize) -> BlockRange {
        let tip = self.items.len() as u64 - 1;
        let mut items = Vec::new();
        let (mut left, mut next) = (per_answer, None);

        for height in asked.from..(asked.from + asked.count).min(tip + 1) {
            let mut item = self.items[height as usize].clone();
            let skip = if height == asked.from { asked.entry as usize } else { 0 };

            if skip > 0 {
                item.body = RangeBody::Continued;
            }

            if let RangeChanges::Proven { entries, listed, .. } = &item.changes {
                let (listed, taken): (u32, Vec<ProvenEntry>) = (*listed, entries.iter().skip(skip).take(left).cloned().collect());

                left -= taken.len();
                let reached = (skip + taken.len()) as u32;
                item.changes = RangeChanges::Proven { from: skip as u32, listed, entries: taken };
                items.push(item);

                if reached < listed {
                    next = Some(RangeCursor { height, entry: reached });

                    break;
                }

                continue;
            }

            items.push(item);
        }

        let next = next.or_else(|| {
            let after = items.last().map(|item: &RangeItem| item.sealed.header.height + 1)?;

            (after <= tip).then_some(RangeCursor { height: after, entry: 0 })
        });

        BlockRange { tip, items, next }
    }

    /// As it comes over the wire.
    fn answer(&self, asked: Asked, per_answer: usize) -> Came {
        Came::of(Ok(Answer { ok: true, status: 200, status_text: "OK".to_string(), bytes: wincode::serialize(&self.range(asked, per_answer)).unwrap(), retry_after: None }))
    }
}

/// What a reading picks here: the height, and the keys it was given.
type Picked = (u64, Vec<Vec<u8>>);

fn pick(item: &CheckedItem) -> Picked {
    (item.height(), item.entries().iter().map(|entry| entry.key.clone()).collect())
}

/// Reads until there is nothing more to ask, and says what was handed back, in order.
fn read_all(scanner: &mut Scanner<Picked>, chain: &Chain, per_answer: usize) -> Vec<Picked> {
    let mut picked = Vec::new();
    let mut asks = 0;

    while let Some(asked) = scanner.next(chain, 0.0) {
        asks += 1;
        assert!(asks < 10_000, "it goes round for ever");

        for read in scanner.came(asked, chain.answer(asked, per_answer), chain, 0.0, pick) {
            if let Read::Picked(found) = read {
                picked.push(found);
            }
        }
    }

    picked
}

fn heights(picked: &[Picked]) -> Vec<u64> {
    let mut heights: Vec<u64> = picked.iter().map(|(height, _)| *height).collect();
    heights.sort_unstable();
    heights.dedup();

    heights
}

fn started(chain: &Chain, at: u64, floor: u64) -> Scanner<Picked> {
    Scanner::new(Place::starting(chain, at, floor).unwrap())
}

#[test]
fn a_reading_goes_down_to_where_it_was_asked_and_up_to_what_is_final_and_reads_each_block_once() {
    let mut chain = Chain::up_to(320, |_| 2);
    chain.final_height = 250;
    let mut scanner = started(&chain, 200, 50);

    let read = read_all(&mut scanner, &chain, 4096);

    assert_eq!(heights(&read), (50..=250).collect::<Vec<_>>());
    assert_eq!(read.iter().filter(|(height, _)| *height == 200).count(), 1, "the block it started at, once");
    assert!(read.iter().all(|(_, keys)| keys.len() == 2));
    assert_eq!((scanner.place().high, scanner.place().known_from(), scanner.place().read), (250, 50, 201));
    assert!(scanner.place().done(250));
    assert!(scanner.place().downloaded > 0);

    // More becomes final: up it goes from where it was.
    chain.final_height = 320;
    let more = read_all(&mut scanner, &chain, 4096);
    assert_eq!(heights(&more), (251..=320).collect::<Vec<_>>());
}

#[test]
fn nothing_above_what_is_final_is_asked_for() {
    let mut chain = Chain::up_to(30, |_| 1);
    chain.final_height = 10;
    let mut scanner = started(&chain, 10, 10);

    read_all(&mut scanner, &chain, 4096);
    assert_eq!(scanner.next(&chain, 0.0), None);

    chain.final_height = 12;
    let asked = scanner.next(&chain, 0.0).unwrap();
    assert_eq!((asked.from, asked.count), (11, 2));
    assert_eq!(asked.path(), "/blocks/range?from=11&count=2");
}

#[test]
fn a_block_cut_across_answers_is_read_whole_going_either_way() {
    let chain = Chain::up_to(40, |height| if height % 10 == 0 { 25 } else { 1 });

    // Going down, from block 30.
    let mut down = started(&chain, 30, 5);
    let read = read_all(&mut down, &chain, 7);
    let at_20: usize = read.iter().filter(|(height, _)| *height == 20).map(|(_, keys)| keys.len()).sum();
    assert_eq!(at_20, 25);
    assert!(read.iter().filter(|(height, _)| *height == 20).count() >= 4, "over several answers");
    assert_eq!(heights(&read), (5..=40).collect::<Vec<_>>());
    assert_eq!(down.place().read, 36);

    // Going up, cut in the middle and resumed from what was kept.
    let mut up = started(&chain, 15, 15);
    up.place.below.clear();
    let mut seen = 0;

    while let Some(asked) = up.next(&chain, 0.0) {
        for read in up.came(asked, chain.answer(asked, 7), &chain, 0.0, pick) {
            if let Read::Picked((20, keys)) = read {
                seen += keys.len();
            }
        }

        if up.place().entry > 0 {
            assert_eq!((up.place().high + 1) % 10, 0, "it has not finished a long block while it is going through it");
            let kept = serde_json::to_string(up.place()).unwrap();
            up = Scanner::new(serde_json::from_str(&kept).unwrap());
            assert!(up.next(&chain, 0.0).unwrap().path().contains("entry="), "a restart goes on inside the block");
            up.line_changed();
        }
    }

    assert_eq!(seen, 25);
    assert_eq!(up.place().high, 40);
}

#[test]
fn a_reading_resumed_from_what_it_kept_reads_nothing_twice() {
    let chain = Chain::up_to(400, |_| 1);
    let mut first = started(&chain, 300, 10);
    let mut read = Vec::new();

    for _ in 0..3 {
        let asked = first.next(&chain, 0.0).unwrap();

        for found in first.came(asked, chain.answer(asked, 4096), &chain, 0.0, pick) {
            if let Read::Picked(found) = found {
                read.push(found);
            }
        }
    }

    let kept = serde_json::to_string(first.place()).unwrap();
    let mut second: Scanner<Picked> = Scanner::new(serde_json::from_str(&kept).unwrap());
    read.extend(read_all(&mut second, &chain, 4096));

    let mut seen = read.iter().map(|(height, _)| *height).collect::<Vec<_>>();
    seen.sort_unstable();

    assert_eq!(seen, (10..=400).collect::<Vec<_>>());
}

#[test]
fn a_reading_goes_further_back_from_where_it_stopped_and_reads_nothing_above_again() {
    let chain = Chain::up_to(300, |_| 1);
    let mut scanner = started(&chain, 300, 200);

    read_all(&mut scanner, &chain, 4096);
    assert_eq!(scanner.place().known_from(), 200);

    assert!(scanner.further_back(150));
    assert!(!scanner.further_back(151), "it is going further than that already");
    assert_eq!(scanner.place().known_from(), 200, "until it has read them");

    let read = read_all(&mut scanner, &chain, 4096);
    assert_eq!(heights(&read), (150..=199).collect::<Vec<_>>());
    assert_eq!(scanner.place().known_from(), 150);
    assert_eq!(scanner.place().floor, 150);
}

#[test]
fn letting_go_of_what_is_old_moves_the_height_it_knows_from_up_and_it_reads_back_from_there() {
    let chain = Chain::up_to(300, |_| 1);
    let mut scanner = started(&chain, 300, 200);

    read_all(&mut scanner, &chain, 4096);
    assert_eq!(scanner.place().known_from(), 200);

    let parent = chain.items[249].sealed.id().to_string();
    assert!(scanner.let_go_below(250, parent.clone()));
    assert_eq!(scanner.place().known_from(), 250);
    assert_eq!(scanner.place().floor_parent, Some(parent));
    assert!(!scanner.let_go_below(250, String::new()), "nothing more to let go of");

    // What it had left to read below goes with it, and nothing it asked counts.
    assert!(scanner.further_back(100));
    let asked = scanner.next(&chain, 0.0).unwrap();
    assert!(asked.from < 250);
    assert!(scanner.let_go_below(280, chain.items[279].sealed.id().to_string()));
    assert_eq!(scanner.place().known_from(), 280);
    assert!(scanner.place().below.is_empty());
    assert!(!scanner.going_down() && !scanner.asking());

    // Older blocks read back from the block it knows from, not from anywhere it let go of.
    assert!(scanner.further_back(270));
    let read = read_all(&mut scanner, &chain, 4096);
    assert_eq!(heights(&read), (270..=279).collect::<Vec<_>>());
    assert_eq!(scanner.place().known_from(), 270);
}

#[test]
fn a_stretch_given_a_lower_bottom_while_it_runs_goes_on_to_it() {
    let chain = Chain::up_to(300, |_| 1);
    let mut scanner = started(&chain, 300, 250);
    let asked = scanner.next(&chain, 0.0).unwrap();

    assert!(scanner.further_back(100));

    let mut read = scanner.came(asked, chain.answer(asked, 4096), &chain, 0.0, pick).len();

    while let Some(asked) = scanner.next(&chain, 0.0) {
        read += scanner.came(asked, chain.answer(asked, 4096), &chain, 0.0, pick).len();
    }

    assert_eq!(read, 201, "from 100 up to 300");
    assert_eq!(scanner.place().known_from(), 100);
}

#[test]
fn a_light_client_that_took_up_the_chain_above_the_reading_leaves_a_hole_read_before_anything_newer() {
    let mut chain = Chain::up_to(600, |_| 1);
    chain.final_height = 100;
    let mut scanner = started(&chain, 100, 100);

    read_all(&mut scanner, &chain, 4096);
    chain.lowest = 500;
    chain.final_height = 600;

    let asked = scanner.next(&chain, 0.0).unwrap();

    assert_eq!((asked.from, asked.count), (400, 100), "the top of the hole first");
    assert_eq!(scanner.place().high, 499);
    assert!(scanner.place().catching_up());

    let mut first = scanner.came(asked, chain.answer(asked, 4096), &chain, 0.0, pick).len();

    while let Some(asked) = scanner.next(&chain, 0.0) {
        if scanner.place().catching_up() {
            assert!(asked.from < 500, "nothing newer while it is behind");
        }

        first += scanner.came(asked, chain.answer(asked, 4096), &chain, 0.0, pick).len();
    }

    assert_eq!(first, 500);
    assert!(!scanner.place().catching_up());
    assert_eq!(scanner.place().high, 600);
    assert_eq!(scanner.place().known_from(), 100, "a hole is no floor");
}

#[test]
fn what_will_not_check_going_up_stops_the_reading_and_hands_back_nothing() {
    let chain = Chain::up_to(10, |_| 2);
    let wrong = |change: &dyn Fn(&mut BlockRange)| {
        let mut scanner = started(&chain, 2, 2);
        scanner.place.below.clear();
        let asked = scanner.next(&chain, 0.0).unwrap();
        let mut range = chain.range(asked, 4096);
        change(&mut range);
        let bytes = wincode::serialize(&range).unwrap();
        let picked = scanner.came(asked, Came::of(Ok(Answer { ok: true, status: 200, status_text: String::new(), bytes, retry_after: None })), &chain, 0.0, pick);

        assert!(picked.is_empty(), "nothing of it comes back");
        assert_eq!(scanner.place().high, 2, "and the place stands");
        assert_eq!(scanner.place().read, 0);
        assert!(scanner.next(&chain, 0.0).is_none(), "stopped");

        let said = scanner.trouble().expect("it says why").sentence();

        scanner.retry();
        assert!(scanner.next(&chain, 0.0).is_some(), "until told to try again");

        said
    };

    let tampered = wrong(&|range| {
        if let RangeChanges::Proven { entries, .. } = &mut range.items[1].changes {
            entries[0].value = Some(b"not what it held".to_vec());
        }
    });
    assert!(tampered.contains("proof does not check"), "{tampered}");

    let forged = wrong(&|range| range.items[3].sealed.header.timestamp += 1);
    assert!(forged.contains("not the one checked"), "{forged}");

    let skipped = wrong(&|range| {
        range.items.remove(2);
    });
    assert!(skipped.contains("out of place"), "{skipped}");

    let empty_body = wrong(&|range| range.items[2].body = RangeBody::Whole { txs: Vec::new(), proofs: vec![], pool: Some(chainv4::structs::block::PoolCommitment { parts: 1, root: Hash::zero(HASH), count: 0 }), receipts: Some(Vec::new()) });
    assert!(empty_body.contains("pool commitment"), "{empty_body}");

    // Asked for 8, and given past what is final: nothing past it is taken.
    let mut lower = Chain::up_to(10, |_| 1);
    lower.final_height = 5;
    let mut scanner = started(&lower, 2, 2);
    scanner.place.below.clear();
    let asked = scanner.next(&lower, 0.0).unwrap();
    let overlong = Asked { count: asked.count + 3, ..asked };

    assert!(scanner.came(asked, lower.answer(overlong, 4096), &lower, 0.0, pick).is_empty());
    assert!(scanner.trouble().unwrap().sentence().contains("more blocks than were asked for"));
}

#[test]
fn bytes_that_are_truncated_or_nonsense_stop_the_reading_and_say_so() {
    let chain = Chain::up_to(10, |_| 2);

    for bytes in [wincode::serialize(&chain.range(Asked { from: 3, entry: 0, count: 8 }, 4096)).unwrap()[..200].to_vec(), vec![0xff; 64], Vec::new()] {
        let mut scanner = started(&chain, 2, 2);
        scanner.place.below.clear();
        let asked = scanner.next(&chain, 0.0).unwrap();
        let size = bytes.len() as u64;
        let came = Came::of(Ok(Answer { ok: true, status: 200, status_text: String::new(), bytes, retry_after: None }));

        assert!(matches!(came, Came::Bad { .. }));
        assert!(scanner.came(asked, came, &chain, 0.0, pick).is_empty());
        assert_eq!(scanner.place().read, 0);
        assert_eq!(scanner.place().downloaded, size, "what it cost still counts");
        assert!(scanner.trouble().unwrap().sentence().contains("does not decode"));
    }
}

#[test]
fn a_stretch_of_blocks_of_another_chain_going_down_never_comes_back() {
    let (chain, other) = (Chain::up_to(120, |_| 1), Chain::up_to(120, |_| 3));
    let mut scanner = started(&chain, 120, 50);
    let asked = scanner.next(&chain, 0.0).unwrap();

    // Each block checks by its own header, and only the stretch's top can tell.
    assert!(scanner.came(asked, other.answer(asked, 4096), &chain, 0.0, pick).is_empty());
    assert!(scanner.trouble().unwrap().sentence().contains("do not link up"));
    assert_eq!(scanner.place().known_from(), 121);
    assert_eq!(scanner.place().read, 0);

    // A forged block in the middle breaks the links the same way.
    let mut scanner = started(&chain, 120, 50);
    let asked = scanner.next(&chain, 0.0).unwrap();
    let mut range = chain.range(asked, 4096);
    range.items[40] = other.items[(asked.from + 40) as usize].clone();
    let came = Came::Range { range, bytes: 1 };

    assert!(scanner.came(asked, came, &chain, 0.0, pick).is_empty());
    assert!(scanner.trouble().is_some());
}

#[test]
fn a_cursor_that_goes_nowhere_is_refused_rather_than_followed_round() {
    let chain = Chain::up_to(10, |_| 1);
    let mut scanner = started(&chain, 2, 2);
    scanner.place.below.clear();
    scanner.place.entry = 1;
    let asked = scanner.next(&chain, 0.0).unwrap();
    // A block whose list has more, each answer giving none of it, as check_shape lets by.
    let mut item = chain.items[3].clone();
    item.body = RangeBody::Continued;
    item.changes = RangeChanges::Proven { from: 1, listed: 9, entries: Vec::new() };
    let came = Came::Range { range: BlockRange { tip: 10, items: vec![item], next: Some(RangeCursor { height: 3, entry: 1 }) }, bytes: 1 };

    assert!(scanner.came(asked, came, &chain, 0.0, pick).is_empty());
    assert!(scanner.trouble().is_some());
}

#[test]
fn a_busy_node_is_waited_for_as_long_as_it_says_and_longer_as_it_goes_on() {
    let chain = Chain::up_to(10, |_| 1);
    let mut scanner = started(&chain, 10, 10);
    let busy = |retry_after: Option<u64>, status: u16| Came::of(Ok(Answer { ok: false, status, status_text: "Busy".to_string(), bytes: b"{\"error\":\"busy\"}".to_vec(), retry_after }));

    let asked = scanner.next(&chain, 100.0).unwrap();
    assert_eq!(scanner.next(&chain, 100.0), None, "one at a time");
    assert!(scanner.asking());

    assert!(scanner.came(asked, busy(Some(5), 429), &chain, 100.0, pick).is_empty());
    assert!(scanner.waiting().unwrap().contains("busy"));
    assert_eq!(scanner.next(&chain, 104.0), None, "it said 5");
    assert_eq!(scanner.next(&chain, 105.0), Some(asked), "and the same again");
    assert_eq!(scanner.waiting(), None);

    // It says 1, as nodes do, but the fifth time running it waits longer.
    let mut time = 105.0;
    let mut last = 0.0;
    let mut asked = asked;

    for _ in 0..5 {
        scanner.came(asked, busy(None, 503), &chain, time, pick);
        let mut waited = 1.0;

        while scanner.next(&chain, time + waited).is_none() {
            waited += 1.0;
        }

        asked = Asked { ..asked };
        time += waited;
        assert!(waited >= last, "{waited} after {last}");
        last = waited;
    }

    assert_eq!(last, 16.0);
    assert!(scanner.trouble().is_none(), "busy is no trouble");

    // An answer lets go of all of it.
    scanner.came(asked, chain.answer(asked, 4096), &chain, time, pick);
    let next = scanner.next(&chain, time);
    assert!(next.is_none(), "done: it was the only block");
    assert_eq!(scanner.place().known_from(), 10);

    // What it says is taken only so far: no node stops a reading for good.
    assert!(matches!(busy(Some(u64::MAX), 429), Came::Busy { wait: LONGEST_WAIT, .. }));
}

#[test]
fn a_node_out_of_reach_is_tried_again_ever_more_slowly_and_nothing_is_lost() {
    let chain = Chain::up_to(10, |_| 1);
    let mut scanner = started(&chain, 10, 10);
    let asked = scanner.next(&chain, 0.0).unwrap();

    scanner.came(asked, Came::of(Err("connection refused".to_string())), &chain, 0.0, pick);
    assert!(scanner.waiting().unwrap().contains("connection refused"));
    assert_eq!(scanner.next(&chain, AWAY_FIRST - 1.0), None);
    assert_eq!(scanner.next(&chain, AWAY_FIRST), Some(asked));

    let trouble = Came::of(Ok(Answer { ok: false, status: 500, status_text: "Oops".to_string(), bytes: Vec::new(), retry_after: None }));
    assert!(matches!(trouble, Came::Away(_)));
    scanner.came(asked, trouble, &chain, 10.0, pick);
    assert_eq!(scanner.next(&chain, 10.0 + 2.0 * AWAY_FIRST - 1.0), None);
    assert!(scanner.next(&chain, 10.0 + 2.0 * AWAY_FIRST).is_some());
    assert!(scanner.trouble().is_none());
}

#[test]
fn an_older_node_that_has_no_such_route_stops_the_reading_and_says_so() {
    let chain = Chain::up_to(10, |_| 1);
    let refused = |status: u16, error: &str| Came::of(Ok(Answer { ok: false, status, status_text: String::new(), bytes: serde_json::json!({ "error": error }).to_string().into_bytes(), retry_after: None }));

    let mut scanner = started(&chain, 5, 5);
    scanner.place.below.clear();
    let asked = scanner.next(&chain, 0.0).unwrap();
    assert_eq!(asked.entry, 0);
    assert!(scanner.came(asked, refused(400, "not a block id: range"), &chain, 0.0, pick).is_empty());
    assert_eq!(scanner.trouble(), Some(&Trouble::NoRoute));
    assert!(scanner.trouble().unwrap().sentence().contains("Nothing is asked of the node instead"));
    assert_eq!(scanner.next(&chain, 1e9), None, "no falling back on anything");

    // Below a block's list, a 400 is a node going back on what it said.
    let mut scanner = started(&chain, 5, 5);
    scanner.place.below.clear();
    scanner.place.entry = 1;
    let asked = scanner.next(&chain, 0.0).unwrap();
    scanner.came(asked, refused(400, "no such entry"), &chain, 0.0, pick);
    assert!(matches!(scanner.trouble(), Some(Trouble::Bad(_))));

    // Going up, with no such height, nothing can go on either.
    let mut scanner = started(&chain, 5, 5);
    scanner.place.below.clear();
    let asked = scanner.next(&chain, 0.0).unwrap();
    scanner.came(asked, refused(404, "no such block"), &chain, 0.0, pick);
    assert_eq!(scanner.trouble(), Some(&Trouble::NoRoute));

    let mut scanner = started(&chain, 5, 5);
    scanner.place.below.clear();
    let asked = scanner.next(&chain, 0.0).unwrap();
    scanner.came(asked, refused(403, "not from here"), &chain, 0.0, pick);
    assert!(scanner.trouble().unwrap().sentence().contains("not from here"));
}

#[test]
fn a_node_that_holds_nothing_below_a_height_is_found_out_and_the_reading_stops_there_saying_so() {
    let chain = Chain::up_to(400, |_| 1);
    // It holds nothing below 350: so this answers as a node that started at a checkpoint there.
    let answer = |asked: Asked| match asked.from < 350 {
        true => Came::Refused { status: 404, message: "no such block".to_string(), bytes: 0 },
        false => chain.answer(asked, 4096),
    };
    let mut scanner = started(&chain, 400, 100);
    let mut unread = None;
    let mut read = 0;

    while let Some(asked) = scanner.next(&chain, 0.0) {
        for found in scanner.came(asked, answer(asked), &chain, 0.0, pick) {
            match found {
                Read::Picked(_) => read += 1,
                Read::Unread { from, to, hole } => {
                    assert!(!hole);
                    unread = Some((from, to));
                }
            }
        }
    }

    assert!(read >= 1, "as much as it held of each stretch it found the start of");
    assert_eq!(scanner.trouble(), None);
    let held = scanner.place().held_from.expect("it says from where");
    assert!((350..=400).contains(&held), "{held}");
    assert_eq!(scanner.place().known_from(), held);
    assert_eq!(unread, Some((100, held - 1)));
    assert!(scanner.place().done(400));
}

#[test]
fn a_gap_is_a_block_that_cannot_say_whatever_the_reason() {
    let chain = Chain::up_to(3, |_| 1);
    let check = |body: RangeBody, changes: RangeChanges| {
        let sealed = chain.items[1].sealed.clone();

        speaks_for(&CheckedItem { block: sealed.id(), sealed, body, changes })
    };
    let whole = |receipts| RangeBody::Whole { txs: Vec::new(), proofs: Vec::new(), pool: None, receipts };
    let listed = || RangeChanges::Proven { from: 0, listed: 0, entries: Vec::new() };

    assert_eq!(check(whole(Some(Vec::new())), listed()), Speaks { state: true, receipts: true });
    assert_eq!(check(whole(Some(Vec::new())), RangeChanges::None(NoProofs::StatePruned { kept_from: 9 })), Speaks { state: false, receipts: true });
    assert_eq!(check(whole(Some(Vec::new())), RangeChanges::None(NoProofs::NoList)), Speaks { state: false, receipts: true });
    assert_eq!(check(whole(None), listed()), Speaks { state: true, receipts: false }, "a body whose receipts are not there");
    assert_eq!(check(RangeBody::HeaderOnly, RangeChanges::None(NoProofs::NoList)), Speaks { state: false, receipts: false });
    assert_eq!(check(RangeBody::Continued, listed()), Speaks { state: true, receipts: true }, "it goes on from what said");
}

#[test]
fn a_wrong_answer_from_before_the_way_to_the_node_changed_is_not_taken() {
    let chain = Chain::up_to(10, |_| 1);
    let mut scanner = started(&chain, 2, 2);
    scanner.place.below.clear();
    let asked = scanner.next(&chain, 0.0).unwrap();

    scanner.line_changed();
    assert!(scanner.came(asked, chain.answer(asked, 4096), &chain, 0.0, pick).is_empty());
    assert_eq!(scanner.place().high, 2);
    assert!(scanner.trouble().is_none());
}
