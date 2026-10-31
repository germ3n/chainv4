//! Read from checked blocks: an explorer that takes what it shows from final blocks it fetched and
//! checked, rather than ask a node, so what it shows rests on those blocks and the node learns less of
//! what is looked at. The reading itself is chainv4_ui::blocks; this holds what the explorer makes of
//! it, its index (index.rs), and what the index answers in place of the node (answers.rs).
//!
//! Where a wallet follows a few accounts, an explorer keeps every transaction, so its index is bounded
//! in bytes: past the cap the oldest blocks go, and the height it knows from moves up to say so. It
//! starts at the final block when it is turned on, and reads back as far as someone says. It holds no
//! secrets, so it is not sealed, which the setting says: it lives in a private folder, or IndexedDB in a
//! browser. Losing it costs a rescan. What is read back from there is checked again as a node's answer
//! is, so a damaged part is set aside, what was below it goes, and it is read again if asked.
//!
//! Nothing here follows the state from a block's entries. A balance or a record is a direct proof,
//! at the final block, only when someone presses for it, and it says so, because the node sees which.
//! So what blocks leave out of their lists, or come without, cannot make anything here out of date.

mod answers;
pub mod index;

#[cfg(test)]
pub(crate) mod tests;

use {
    crate::api::{Ask, Failure},
    answers::Reach,
    chainv4::db::Column,
    chainv4::light::{BlockRange, CheckedItem, LightClient, RangeItem, check_range_item},
    chainv4::structs::hash::HashType,
    chainv4::structs::hex::to_hex,
    chainv4::structs::tx::{FeeOffer, Tx},
    chainv4::structs::u256::u256,
    chainv4_ui::blocks::fees::{FeesSeen, used_in},
    chainv4_ui::blocks::{Asked, Came, Followed, Place, Read, Scanner, Trouble, proven_value},
    chainv4_ui::shelf::{Keeper, MemoryShelf},
    index::{BUCKET, Index, bucket_of, kept_of},
    serde::{Deserialize, Serialize},
    serde_json::{Value, json},
    std::collections::{BTreeMap, BTreeSet, HashMap},
    std::sync::{Arc, Mutex, PoisonError},
};

/// The most bytes an index holds, as encoded: its blocks but for their entries. About 18 days of the
/// devnet's empty blocks on a desktop, and 4 on a phone or in a browser, whose room is another's, at
/// about 400 bytes each. It is all in memory while the app is open.
pub const MOST_BYTES: u64 = if cfg!(any(target_os = "android", target_os = "ios", target_arch = "wasm32")) { 32_000_000 } else { 128_000_000 };
/// Seconds between writing out what changed.
const SAVE_EVERY: f64 = 30.0;
/// The most single blocks held that were fetched by themselves, outside the index.
const MOST_FETCHED: usize = 32;

/// What goes with the blocks kept: where the reading got to, and what it can say of them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
struct Head {
    place: Option<Place>,
    /// What fees it saw paid.
    fees: FeesSeen,
    /// Heights the node holds nothing of among those read, so that none is taken for lost.
    unread: Vec<(u64, u64)>,
    /// The height below which blocks were let go, for there being too many to keep.
    cut_below: Option<u64>,
}

/// What a checked block comes to.
#[derive(Debug)]
struct Picked {
    item: Option<RangeItem>,
    used: Vec<(String, u64)>,
    price: Option<u256>,
}

/// A value a node proved, once it checked, and the final block it was proven at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProvenValue {
    pub value: Option<Vec<u8>>,
    pub height: u64,
    pub block: String,
}

/// What came of something asked of a shelf, once it answered.
type Inbox<T> = Arc<Mutex<Option<Result<T, String>>>>;

/// Where opening an index has got to.
struct Opening {
    genesis: String,
    hash: HashType,
    /// The head, and the buckets read so far, newest first.
    head: Option<Head>,
    /// The bucket being read, and what is expected above it to link up to.
    reading: Option<u64>,
    /// The height and parent of the lowest block read so far.
    above: Option<(u64, String)>,
    head_inbox: Inbox<Option<Vec<u8>>>,
    bucket_inbox: Inbox<Option<Vec<RangeItem>>>,
    blocks: Vec<RangeItem>,
}

struct Open {
    genesis: String,
    hash: HashType,
    head: Head,
    index: Index,
    scanner: Option<Scanner<Picked>>,
    /// What to start from, once it can: the final block, unless someone said.
    floor: Option<u64>,
    /// Buckets changed since they were written, and whether the head was.
    dirty: BTreeSet<u64>,
    head_dirty: bool,
    saved_at: f64,
    /// Blocks fetched one at a time, outside what the index holds and says it knows, for this run.
    fetched: BTreeMap<u64, RangeItem>,
    /// The one being fetched, while it is.
    fetching: Option<Asked>,
    /// Why the last one could not be.
    fetch_trouble: Option<String>,
}

/// Where a reading stands, for a page to say.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Standing {
    /// How far up it has read.
    pub high: u64,
    /// How far back it knows without a break.
    pub known_from: u64,
    /// How far back it was asked to go, and what lies between that and `known_from`, in blocks.
    pub floor: u64,
    pub left_below: u64,
    /// Newer blocks it has yet to read, from a time the app was shut.
    pub catching_up: bool,
    pub downloaded: u64,
    pub read: u64,
    /// How much it holds, and the most it may.
    pub bytes: u64,
    pub most: u64,
    pub blocks: u64,
    /// Why it waits, or stopped.
    pub waiting: Option<String>,
    pub trouble: Option<String>,
    /// Blocks that came without their receipts or with a header alone.
    pub no_receipts: u64,
    /// Heights the node holds nothing of among those read, in all.
    pub unread: u64,
    /// The lowest height the node has, once it said it has nothing below.
    pub held_from: Option<u64>,
    /// Below this, blocks were let go for there being too many.
    pub cut_below: Option<u64>,
}

/// What came of asking to read further back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Further {
    Moved,
    /// It reads from this height already.
    Already(u64),
    /// The index has no room for what is older.
    Full,
}

/// An explorer's reading: nothing while the setting is off.
pub struct Reading {
    keeper: Keeper,
    /// Whether an index lasts: false where there is nowhere to put one.
    lasting: bool,
    most: u64,
    opening: Option<Opening>,
    open: Option<Open>,
    /// What was asked to be proven, by what it is of, and at what.
    proving: HashMap<(String, String), (u64, String)>,
    proven: HashMap<(String, String), Result<ProvenValue, String>>,
    /// What to tell someone of the index, until they have seen it.
    pub said: Vec<String>,
}

impl Default for Reading {
    fn default() -> Reading {
        Reading::new(Keeper::new(Arc::new(MemoryShelf::default())), false)
    }
}

/// A reading whose index lasts in this app's folder, or for this run alone with none.
#[cfg(not(target_arch = "wasm32"))]
pub fn on_this_device(folder: Option<&std::path::Path>) -> Reading {
    match folder.map(|folder| chainv4_ui::shelf::FileShelf::new(folder.join("explorer-index"))) {
        Some(Ok(shelf)) => Reading::new(Keeper::new(Arc::new(shelf)), true),
        Some(Err(why)) => {
            log::warn!("{why}");

            Reading::default()
        }
        None => Reading::default(),
    }
}

/// A reading whose index lasts in this browser's IndexedDB, as localStorage has no room for it.
#[cfg(target_arch = "wasm32")]
pub fn on_this_device() -> Reading {
    Reading::new(Keeper::new(Arc::new(chainv4_ui::shelf::IndexedDbShelf::new("chainv4-explorer-index"))), true)
}

fn head_name(genesis: &str) -> String {
    format!("{genesis}-head")
}

/// What a bucket goes by: its end says where the number stops, so that a prefix names one alone.
fn bucket_name(genesis: &str, bucket: u64) -> String {
    format!("{genesis}-b{bucket}-")
}

fn logged(what: &'static str) -> chainv4_ui::shelf::Done<()> {
    Box::new(move |result| {
        if let Err(why) = result {
            log::warn!("could not {what}: {why}");
        }
    })
}

/// Notes heights the node holds nothing of, joined with those next to them.
fn note_unread(unread: &mut Vec<(u64, u64)>, from: u64, to: u64) {
    unread.push((from, to));
    unread.sort_unstable();
    let mut joined: Vec<(u64, u64)> = Vec::with_capacity(unread.len());

    for (from, to) in unread.drain(..) {
        match joined.last_mut() {
            Some(last) if from <= last.1.saturating_add(1) => last.1 = last.1.max(to),
            _ => joined.push((from, to)),
        }
    }

    *unread = joined;
}

impl Open {
    fn fresh(genesis: String, hash: HashType) -> Open {
        Open::with(genesis, hash, Head::default(), Index::default())
    }

    fn with(genesis: String, hash: HashType, head: Head, index: Index) -> Open {
        let scanner = head.place.clone().map(Scanner::new);

        Open { genesis, hash, head, index, scanner, floor: None, dirty: BTreeSet::new(), head_dirty: false, saved_at: 0.0, fetched: BTreeMap::new(), fetching: None, fetch_trouble: None }
    }

    /// Where it stands, for what it answers to say.
    fn reach(&self, final_height: u64) -> Reach {
        let place = self.scanner.as_ref().map(Scanner::place);
        let reading_to = place.filter(|place| place.below.iter().any(|stretch| !stretch.hole)).map(|place| place.floor);

        Reach {
            known_from: place.map_or(final_height, Place::known_from),
            high: place.map_or(final_height, |place| place.high),
            reading_to,
            hash: self.hash,
        }
    }

    /// Takes in what a block came to, once it stands.
    fn take(&mut self, picked: Picked) {
        self.head.fees.take(picked.item.as_ref().map_or(0, |item| item.sealed.header.height), &picked.used, picked.price);
        self.head_dirty = true;

        if let Some(item) = picked.item {
            self.fetched.remove(&item.sealed.header.height);
            self.dirty.insert(self.index.take(item, self.hash));
        }
    }

    /// Lets go of the oldest buckets past the cap, and moves what it says it knows up to what is left.
    fn trim(&mut self, keeper: &Keeper, most: u64) {
        let mut cut = false;

        while self.index.bytes() > most {
            let Some(bucket) = self.index.drop_oldest_bucket(self.hash) else {
                break;
            };

            cut = true;
            self.dirty.remove(&bucket);
            keeper.forget(&bucket_name(&self.genesis, bucket), logged("let go of old blocks"));
        }

        if !cut {
            return;
        }

        if let Some(lowest) = self.index.lowest()
            && let Some(item) = self.index.at(lowest)
        {
            let parent = item.sealed.header.parent.to_string();

            if let Some(scanner) = &mut self.scanner {
                scanner.let_go_below(lowest, parent);
            }

            self.head.cut_below = Some(lowest);
            self.head_dirty = true;
        }
    }

    /// Writes out what changed: the buckets first and the head last, so that a kill between them
    /// leaves a head that says less than was written, never more.
    fn save(&mut self, keeper: &Keeper, time: f64) {
        for bucket in std::mem::take(&mut self.dirty) {
            match self.index.bucket_bytes(bucket) {
                Ok(bytes) => keeper.save(&bucket_name(&self.genesis, bucket), bytes),
                Err(why) => log::warn!("{why}"),
            }
        }

        if let Some(scanner) = &self.scanner {
            self.head.place = Some(scanner.place().clone());
        }

        match serde_json::to_vec(&self.head) {
            Ok(bytes) => keeper.save(&head_name(&self.genesis), bytes),
            Err(why) => log::warn!("could not write where the reading got to: {why}"),
        }

        self.head_dirty = false;
        self.saved_at = time;
    }

    /// Takes what came of a block fetched by itself: never into the index, and never as what it knows.
    fn fetched_one(&mut self, asked: Asked, came: Came, followed: &impl Followed) {
        self.fetching = None;
        let checked = match came {
            Came::Range { range, .. } => one_block(asked, range, followed),
            Came::Bad { why, .. } => Err(format!("this node sent {why}")),
            Came::Busy { wait, .. } => Err(format!("the node is busy, and asks to wait {wait} seconds")),
            Came::Refused { message, .. } => Err(format!("the node refused: {message}")),
            Came::Away(why) => Err(format!("could not reach the node: {why}")),
        };

        match checked {
            Ok(item) => {
                while self.fetched.len() >= MOST_FETCHED {
                    self.fetched.pop_first();
                }

                self.fetch_trouble = None;
                self.fetched.insert(asked.from, item);
            }
            Err(why) => self.fetch_trouble = Some(why),
        }
    }
}

/// One block a range was asked for, checked against the header the light client holds of it.
fn one_block(asked: Asked, range: BlockRange, followed: &impl Followed) -> Result<RangeItem, String> {
    range.check_shape(asked.from, asked.entry).map_err(|why| format!("this node sent {why}"))?;

    let item = range.items.into_iter().next().ok_or_else(|| format!("this node says it holds nothing at block {}", asked.from))?;
    let sealed = followed.sealed_at(asked.from).ok_or("the light client holds no header of that block")?;
    let checked = check_range_item(followed.hash(), sealed, item).map_err(|why| format!("this node sent {why}"))?;

    kept_of(&checked).ok_or_else(|| "this node sent no block".to_string())
}

impl Reading {
    /// A reading whose index is kept by this keeper. Whether it lasts says whether this keeper's shelf
    /// is more than memory, for the page to say.
    pub fn new(keeper: Keeper, lasting: bool) -> Reading {
        Reading { keeper, lasting, most: MOST_BYTES, opening: None, open: None, proving: HashMap::new(), proven: HashMap::new(), said: Vec::new() }
    }

    /// Whether an index is open, to answer from.
    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// Whether what it reads is kept for next time.
    pub fn lasts(&self) -> bool {
        self.lasting
    }

    /// Why the index could not be written, if it could not.
    pub fn keeping_trouble(&self) -> Option<String> {
        self.keeper.failed()
    }

    /// Opens the index of this chain, a step at a time as a shelf answers, and says whether it is open.
    /// Another chain's is put away first.
    pub fn open(&mut self, genesis: &str, hash: HashType, followed: &impl Followed, time: f64) -> bool {
        let keeper = self.keeper.clone();

        if self.open.as_ref().is_some_and(|open| open.genesis != genesis)
            && let Some(mut open) = self.open.take()
        {
            open.save(&keeper, time);
        }

        if self.open.is_none() {
            self.opened_by_now(genesis, hash, followed);
        }

        self.open.is_some()
    }

    fn opened_by_now(&mut self, genesis: &str, hash: HashType, followed: &impl Followed) {
        if self.opening.as_ref().is_none_or(|opening| opening.genesis != genesis) {
            let head_inbox: Inbox<Option<Vec<u8>>> = Inbox::default();
            let answered = head_inbox.clone();

            self.opening = Some(Opening { genesis: genesis.to_string(), hash, head: None, reading: None, above: None, head_inbox, bucket_inbox: Inbox::default(), blocks: Vec::new() });
            self.keeper.read(&head_name(genesis), Box::new(move |result| *answered.lock().unwrap_or_else(PoisonError::into_inner) = Some(result)));
        }

        // A shelf may answer at once, in memory: each step is taken as far as it goes.
        while self.opening.is_some() && self.open.is_none() && self.step_opening(followed) {}
    }

    /// Takes the next step of opening, if the shelf has answered. Whether it took one.
    fn step_opening(&mut self, followed: &impl Followed) -> bool {
        let opening = self.opening.as_mut().expect("it is opening");

        if opening.head.is_none() {
            let Some(answer) = opening.head_inbox.lock().unwrap_or_else(PoisonError::into_inner).take() else {
                return false;
            };
            let head = answer.and_then(|bytes| bytes.map(|bytes| serde_json::from_slice::<Head>(&bytes).map_err(|_| "where it got to does not read".to_string())).transpose());

            match head {
                Ok(head) => {
                    let head = head.unwrap_or_default();

                    match head.place.as_ref().map(|place| bucket_of(place.high + u64::from(place.entry > 0))) {
                        Some(bucket) => {
                            opening.head = Some(head);
                            self.read_bucket(bucket);
                        }
                        None => self.finish_opening(Head::default(), Vec::new()),
                    }
                }
                Err(why) => {
                    let (genesis, hash) = (opening.genesis.clone(), opening.hash);

                    self.opening = None;
                    self.keeper.set_aside(&head_name(&genesis), Box::new(|_| {}));
                    self.keeper.forget(&format!("{genesis}-b"), logged("let go of unreadable blocks"));
                    self.said.push(format!("What this explorer read of the chain could not be opened ({why}), so it was set aside and is being read again."));
                    self.open = Some(Open::fresh(genesis, hash));
                }
            }

            return true;
        }

        let Some(answer) = opening.bucket_inbox.lock().unwrap_or_else(PoisonError::into_inner).take() else {
            return false;
        };
        let bucket = opening.reading.expect("a bucket is being read");
        let place = opening.head.as_ref().and_then(|head| head.place.clone()).expect("a head with a place");
        let unread = opening.head.as_ref().map_or_else(Vec::new, |head| head.unread.clone());
        let known_from = place.known_from();
        let top = place.high + u64::from(place.entry > 0);
        let expected = |height: u64| !place.below.iter().any(|stretch| stretch.hole && (stretch.bottom..=stretch.top).contains(&height)) && !unread.iter().any(|(from, to)| (*from..=*to).contains(&height));
        let low = (bucket * BUCKET).max(known_from);
        let high = (bucket * BUCKET + BUCKET - 1).min(top);
        let checked = answer.and_then(|items| {
            let mut items = items.unwrap_or_default();
            items.retain(|item| (low..=high).contains(&item.sealed.header.height));
            let held: BTreeSet<u64> = items.iter().map(|item| item.sealed.header.height).collect();

            match (low..=high).all(|height| !expected(height) || held.contains(&height)) {
                true => Ok(items),
                false => Err("blocks missing from among those read".to_string()),
            }
        });

        match checked {
            Ok(items) => {
                if let Some(lowest) = items.first() {
                    opening.above = Some((lowest.sealed.header.height, lowest.sealed.header.parent.to_string()));
                }

                opening.blocks.extend(items);

                match low > known_from {
                    true => self.read_bucket(bucket - 1),
                    false => {
                        let opening = self.opening.as_mut().expect("it is opening");
                        let (head, blocks) = (opening.head.take().unwrap_or_default(), std::mem::take(&mut opening.blocks));
                        self.finish_opening(head, blocks);
                        self.check_top(followed);
                    }
                }
            }
            Err(why) => self.damaged(bucket, known_from, &why, followed),
        }

        true
    }

    /// A bucket that will not read: it stays for anyone to look into, what is older goes, and it knows
    /// from what is left. With nothing left it starts again.
    fn damaged(&mut self, bucket: u64, known_from: u64, why: &str, followed: &impl Followed) {
        let opening = self.opening.as_mut().expect("it is opening");
        let genesis = opening.genesis.clone();
        let hash = opening.hash;
        let mut head = opening.head.take().unwrap_or_default();
        let blocks = std::mem::take(&mut opening.blocks);

        self.keeper.set_aside(&bucket_name(&genesis, bucket), Box::new(|_| {}));

        for older in bucket_of(known_from)..bucket {
            self.keeper.forget(&bucket_name(&genesis, older), logged("let go of old blocks"));
        }

        match blocks.iter().map(|item| item.sealed.header.height).min() {
            Some(lowest) => {
                let parent = blocks.iter().find(|item| item.sealed.header.height == lowest).map(|item| item.sealed.header.parent.to_string()).unwrap_or_default();

                if let Some(place) = &mut head.place {
                    place.let_go_below(lowest, parent);
                }

                self.said.push(format!("Part of what this explorer read of the chain could not be opened ({why}), so it was set aside. It knows from block {lowest}: read further back in Settings for what is older."));
                self.finish_opening(head, blocks);
                self.check_top(followed);
            }
            None => {
                self.opening = None;
                self.keeper.set_aside(&head_name(&genesis), Box::new(|_| {}));
                self.said.push(format!("What this explorer read of the chain could not be opened ({why}), so it was set aside and is being read again."));
                self.open = Some(Open::fresh(genesis, hash));
            }
        }
    }

    /// Asks a shelf for a bucket, read as a node's answer is, once it has come.
    fn read_bucket(&mut self, bucket: u64) {
        let opening = self.opening.as_mut().expect("it is opening");
        let name = bucket_name(&opening.genesis, bucket);
        let (inbox, hash, above) = (opening.bucket_inbox.clone(), opening.hash, opening.above.clone());

        opening.reading = Some(bucket);
        self.keeper.read(
            &name,
            Box::new(move |result| {
                let read = result.and_then(|bytes| bytes.map(|bytes| index::decode(&bytes, hash, above.as_ref())).transpose());

                *inbox.lock().unwrap_or_else(PoisonError::into_inner) = Some(read);
            }),
        );
    }

    fn finish_opening(&mut self, head: Head, blocks: Vec<RangeItem>) {
        let opening = self.opening.take().expect("it is opening");
        let mut index = Index::default();

        for item in blocks {
            index.take(item, opening.hash);
        }

        self.open = Some(Open::with(opening.genesis, opening.hash, head, index));
    }

    /// The top of what was read back must be what the light client checked, if it holds it: an index
    /// from another branch of a chain that was started over goes.
    fn check_top(&mut self, followed: &impl Followed) {
        let Some(open) = &self.open else {
            return;
        };
        let top = open.index.highest().and_then(|height| Some((open.index.at(height)?, followed.sealed_at(height)?)));

        if top.is_some_and(|(kept, sealed)| kept.sealed.id() != sealed.id()) {
            let (genesis, hash) = (open.genesis.clone(), open.hash);

            self.keeper.forget(&format!("{genesis}-"), logged("let go of another chain's blocks"));
            self.said.push("What this explorer read is not the chain the light client follows now, so it was let go and is being read again.".to_string());
            self.open = Some(Open::fresh(genesis, hash));
        }
    }

    /// What to ask now: nothing until the index is open and something follows the chain, then the
    /// blocks due. It writes out what changed every so often.
    pub fn go(&mut self, followed: &impl Followed, time: f64) -> Option<Asked> {
        let keeper = self.keeper.clone();
        let open = self.open.as_mut()?;

        if open.scanner.is_none() {
            let at = followed.final_height();

            if let Some(place) = Place::starting(followed, at, open.floor.unwrap_or(at)) {
                open.head.place = Some(place.clone());
                open.scanner = Some(Scanner::new(place));
                open.head_dirty = true;
            }
        }

        let asked = open.scanner.as_mut().and_then(|scanner| scanner.next(followed, time));

        if (open.head_dirty || !open.dirty.is_empty()) && time - open.saved_at >= SAVE_EVERY {
            open.save(&keeper, time);
        }

        asked
    }

    /// Takes what came of blocks asked for: for the reading, or a block fetched by itself.
    pub fn came(&mut self, asked: Asked, came: Came, followed: &impl Followed, time: f64) {
        let keeper = self.keeper.clone();
        let most = self.most;
        let Some(open) = &mut self.open else {
            return;
        };

        if open.fetching == Some(asked) {
            // The very block the reading asked for too is the reading's, which keeps it.
            if open.scanner.as_ref().and_then(Scanner::out) != Some(asked) {
                open.fetched_one(asked, came, followed);

                return;
            }

            open.fetching = None;
        }

        let Some(mut scanner) = open.scanner.take() else {
            return;
        };
        let read = scanner.came(asked, came, followed, time, |item: &CheckedItem| {
            let (used, price) = used_in(item);

            Picked { item: kept_of(item), used, price }
        });

        open.scanner = Some(scanner);

        for read in read {
            match read {
                Read::Picked(picked) => open.take(picked),
                Read::Unread { from, to, .. } => {
                    note_unread(&mut open.head.unread, from, to);
                    open.head_dirty = true;
                }
            }
        }

        open.trim(&keeper, most);
    }

    /// Reads further back, down to this height: what it read stands, and it says from what height it
    /// knows. Nothing, where there is no room for what is older.
    pub fn further_back(&mut self, floor: u64) -> Further {
        // With less room than this left, whatever older came would only push out the rest.
        let (most, least) = (self.most, self.most / 64);
        let Some(open) = &mut self.open else {
            return Further::Already(floor);
        };
        let Some(scanner) = &mut open.scanner else {
            open.floor = Some(floor);

            return Further::Moved;
        };
        let known = scanner.place().floor;

        if floor >= known {
            return Further::Already(known);
        }

        // The oldest went for want of room: whatever is older than that would only go the same way.
        if open.index.bytes().saturating_add(least) > most || open.head.cut_below.is_some_and(|cut| cut >= known) {
            return Further::Full;
        }

        match scanner.further_back(floor) {
            true => {
                open.head_dirty = true;

                Further::Moved
            }
            false => Further::Already(known),
        }
    }

    /// Tries again whatever stopped the reading: what Retry and Reload do.
    pub fn retry(&mut self) {
        if let Some(scanner) = self.open.as_mut().and_then(|open| open.scanner.as_mut()) {
            scanner.retry();
        }
    }

    /// Forgets the way to the node: what is out is not coming.
    pub fn line_changed(&mut self) {
        if let Some(open) = &mut self.open {
            open.fetching = None;

            if let Some(scanner) = &mut open.scanner {
                scanner.line_changed();
            }
        }
    }

    /// Forgets the index of the chain open, and starts again at the final block.
    pub fn forget(&mut self) {
        let Some(open) = self.open.take() else {
            return;
        };

        self.keeper.forget(&format!("{}-", open.genesis), logged("forget what it read"));
        self.open = Some(Open::fresh(open.genesis, open.hash));
        self.proven.clear();
        self.proving.clear();
    }

    /// Writes out and lets go of what is open, as the setting goes off or the node changes.
    pub fn close(&mut self, time: f64) {
        self.opening = None;
        self.proven.clear();
        self.proving.clear();

        if let Some(mut open) = self.open.take() {
            open.save(&self.keeper, time);
        }
    }

    /// Writes out what changed, as the app saves: no harm if it is written again in a moment.
    pub fn save(&mut self, time: f64) {
        let keeper = self.keeper.clone();

        if let Some(open) = self.open.as_mut().filter(|open| open.head_dirty || !open.dirty.is_empty()) {
            open.save(&keeper, time);
        }
    }

    /// Where the reading stands: None until it started.
    pub fn standing(&self) -> Option<Standing> {
        let open = self.open.as_ref()?;
        let scanner = open.scanner.as_ref()?;
        let place = scanner.place();

        Some(Standing {
            high: place.high,
            known_from: place.known_from(),
            floor: place.floor,
            left_below: place.below.iter().filter(|stretch| !stretch.hole).map(|stretch| stretch.top - stretch.bottom + 1).sum(),
            catching_up: place.catching_up(),
            downloaded: place.downloaded,
            read: place.read,
            bytes: open.index.bytes(),
            most: self.most,
            blocks: open.index.len() as u64,
            waiting: scanner.waiting().map(str::to_string),
            trouble: scanner.trouble().map(Trouble::sentence),
            no_receipts: open.index.no_receipts(),
            unread: open.head.unread.iter().map(|(from, to)| to - from + 1).sum(),
            held_from: place.held_from,
            cut_below: open.head.cut_below,
        })
    }

    /// Asks for a single block by itself, as someone pressed for: the node sees which. It is held for
    /// this run and never counts as what the index knows. What to ask, or why it cannot be.
    pub fn fetch_one(&mut self, height: u64, followed: &impl Followed) -> Result<Asked, String> {
        let open = self.open.as_mut().ok_or("nothing is open yet")?;

        if height > followed.final_height() {
            return Err(format!("Block {height} is not final yet, and only final blocks are read."));
        }

        if followed.sealed_at(height).is_none() {
            return Err(format!("The light client holds no header at block {height}, so that block could not be checked by itself. Read further back to it instead."));
        }

        let asked = Asked { from: height, entry: 0, count: 1 };

        open.fetching = Some(asked);
        open.fetch_trouble = None;

        Ok(asked)
    }

    /// Says why a block could not be fetched by itself, as a page found.
    pub fn say_fetching(&mut self, why: String) {
        if let Some(open) = &mut self.open {
            open.fetch_trouble = Some(why);
        }
    }

    /// Whether a block by itself is held, as fetched.
    pub fn holds_fetched(&self, height: u64) -> bool {
        self.open.as_ref().is_some_and(|open| open.fetched.contains_key(&height))
    }

    /// Whether a block by itself is on its way, and why the last one could not be had.
    pub fn fetching(&self) -> (bool, Option<&str>) {
        self.open.as_ref().map_or((false, None), |open| (open.fetching.is_some(), open.fetch_trouble.as_deref()))
    }

    /// What the index answers of this, in place of the node: None for what it has no say in.
    pub fn answer(&self, ask: &Ask, final_height: u64) -> Option<Result<Value, Failure>> {
        let open = self.open.as_ref()?;
        let reach = open.reach(final_height);
        let wanted_block = |wanted: &str| answers::block_by(&open.index, wanted, reach.hash).or_else(|| wanted.parse().ok().and_then(|height| open.fetched.get(&height)).or_else(|| open.fetched.values().find(|item| item.sealed.id().to_string() == wanted)));

        Some(match ask {
            Ask::Blocks { before, limit } => Ok(answers::blocks_answer(&open.index, *before, *limit)),
            Ask::Block(wanted) | Ask::Lookup(wanted) => answers::block_answer(wanted_block(wanted), wanted, &reach),
            Ask::Tx(id) => answers::tx_answer(&open.index, id, &reach),
            Ask::AccountTxs { address, query, cursor } => answers::account_txs(&open.index, address, query, cursor.as_deref(), &reach),
            Ask::Events { query, cursor } => answers::events_answer(&open.index, query, cursor.as_deref(), &reach),
            _ => return None,
        })
    }

    /// What the chain looks like from here, as GET /status writes it: the newest block read, and what
    /// the light client checked of finality. The coin's name and decimals are as a node last said on
    /// this device, or else plain units, and the chain's name as a node last said.
    pub fn status(&self, genesis: &str, client: &LightClient, coin: Option<&(String, u8)>, name: Option<&str>) -> Option<Value> {
        let open = self.open.as_ref()?;
        let final_height = client.final_height();
        let reach = open.reach(final_height);
        let held = open.index.at(reach.high);
        let header = held.map(|item| item.sealed.clone()).or_else(|| client.sealed_at(final_height).cloned())?;
        let (symbol, decimals) = coin.cloned().unwrap_or_else(|| ("units".to_string(), 0));
        let finality = client.params().finality.map(|finality| {
            let interval = finality.checkpoint_interval().unwrap_or(0);
            let (justified, finalized) = (client.justified(), client.finalized());

            json!({
                "type": finality.name(),
                "epoch": finalized.epoch,
                "checkpoint_interval": interval,
                "justified": { "epoch": justified.epoch, "block": justified.block.to_string(), "height": justified.epoch * interval },
                "finalized": { "epoch": finalized.epoch, "block": finalized.block.to_string(), "height": final_height },
            })
        });

        Some(json!({
            "genesis": genesis,
            "tip": header.id().to_string(),
            "height": header.header.height,
            "timestamp": header.header.timestamp,
            "difficulty": header.header.difficulty.to_string(),
            "work": client.work_at(header.header.height).map_or_else(|| "0".to_string(), |work| work.to_string()),
            "hash": client.params().hash.name(),
            "coin": { "symbol": symbol, "decimals": decimals },
            "name": name,
            "finality": finality,
            "fees": (header.header.base_price != u256::ZERO).then(|| json!({ "base_price": header.header.base_price.to_string() })),
        }))
    }

    /// What someone pressed to prove directly, at the final block, in place of what was proven of it
    /// before: what to ask, or None while nothing is followed yet.
    pub fn prove(&mut self, column: Column, key: &[u8], followed: &impl Followed) -> Option<Ask> {
        let height = followed.final_height();
        let block = followed.sealed_at(height)?.id().to_string();
        let what = (column.name().to_string(), to_hex(key));

        self.proven.remove(&what);
        self.proving.insert(what.clone(), (height, block.clone()));

        Some(Ask::DirectProof { column: what.0, key: what.1, block })
    }

    /// Says why a proof could not be asked for, as a page found.
    pub fn say_proving(&mut self, column: Column, key: &[u8], why: String) {
        self.proven.insert((column.name().to_string(), to_hex(key)), Err(why));
    }

    /// Whether a proof pressed for is on its way.
    pub fn proving(&self, column: Column, key: &[u8]) -> bool {
        self.proving.contains_key(&(column.name().to_string(), to_hex(key)))
    }

    /// What was proven of this, or why it could not be.
    pub fn proven(&self, column: Column, key: &[u8]) -> Option<&Result<ProvenValue, String>> {
        self.proven.get(&(column.name().to_string(), to_hex(key)))
    }

    /// As though a proof had come and checked, as a test has one.
    #[cfg(test)]
    pub fn has_proven(&mut self, column: Column, key: &[u8], proven: ProvenValue) {
        self.proven.insert((column.name().to_string(), to_hex(key)), Ok(proven));
    }

    /// Takes what a node answered of a proof. Whether it was one.
    pub fn take_proof(&mut self, ask: &Ask, result: &Result<Value, Failure>, followed: Option<&impl Followed>) -> bool {
        let Ask::DirectProof { column, key, block } = ask else {
            return false;
        };
        let what = (column.clone(), key.clone());
        let Some((height, asked_block)) = self.proving.get(&what).cloned().filter(|(_, asked)| asked == block) else {
            return true;
        };

        self.proving.remove(&what);
        let proven = match (result, followed) {
            (Err(failure), _) => Err(failure.sentence()),
            (Ok(_), None) => Err("the light client stopped, so nothing can be checked".to_string()),
            (Ok(value), Some(followed)) => proven_value(column, key, block, value, height, followed).map(|value| ProvenValue { value, height, block: asked_block }).map_err(|why| format!("This node's proof does not check: {why}.")),
        };

        self.proven.insert(what, proven);

        true
    }

    /// What to offer for this transaction, as checked blocks show: see `FeesSeen::suggest`.
    pub fn suggest(&self, tx: &Tx, base: u256, tip: u256) -> (FeeOffer, bool) {
        self.open.as_ref().map_or_else(FeesSeen::default, |open| open.head.fees.clone()).suggest(tx, base, tip)
    }

}
