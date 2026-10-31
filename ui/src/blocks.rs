//! Reading blocks a node sends, for an app that means to rest on what it checked rather than on what
//! the node says: the requests to `GET /blocks/range`, what to make of each answer, and where the
//! reading has got to. What an app wants of each block is its own: it picks that out, and this hands
//! it back only once what it was picked from has checked. Plain data but for `RangeLine`, so all of
//! it is tested without a network.
//!
//! Only final blocks are read, so nothing read is ever undone. The reading goes two ways from where
//! it started. Up: the blocks made since, each against the header the light client checked at its
//! height. Down: the older ones, a stretch at a time, as a light client holds few headers below the
//! finalized block it saved, and holding a header for every block down to a height months back is
//! more than a phone has room for. The answers carry their own headers, and a header's id is named
//! by the one above it, so a stretch is proven by its top block's id: nothing of it is handed back
//! until the whole stretch has come and links up to that id, and the next stretch's top is the
//! parent of this one's lowest. Going down costs no headers beyond those in the blocks, and what is
//! newest is known first.
//!
//! Where the light client took up the chain above where the reading got to, as it does on restoring
//! what it saved once the reading fell behind, what lies between is read in the same way, as a hole:
//! before anything else, as what the reading knows of the chain's state is out of date until then.
//!
//! A node may have 2 ranges under way for one client, and over an onion everyone counts as the same
//! client, so it answers 429 and 503 often. Those are waited out here. An answer that does not check
//! stops the reading, says why, and leaves nothing of itself behind.

pub mod fees;
mod line;
#[cfg(test)]
mod tests;
pub mod touched;

pub use line::{RangeLine, Wake};

use {
    crate::net::Answer,
    chainv4::db::{Column, STATE_COLUMNS, StateProof},
    chainv4::light::{BlockRange, CheckedItem, LightClient, RangeBody, RangeChanges, RangeCursor, RangeError, check_range_item, decode_block_range},
    chainv4::structs::block::SealedHeader,
    chainv4::structs::hash::HashType,
    chainv4::structs::hex::from_hex,
    serde::{Deserialize, Serialize},
    serde_json::Value,
};

/// The most blocks asked for at a time: the most a node gives at once.
pub const MOST_BLOCKS: u64 = 100;
/// Seconds before asking again after a node that could not be reached, at first and at most.
const AWAY_FIRST: f64 = 3.0;
const AWAY_MOST: f64 = 30.0;
/// The longest a node's Retry-After is taken at, in seconds: a node cannot stop a reading for good
/// by asking it to wait for a year.
const LONGEST_WAIT: u64 = 60;
/// The wait a busy node is taken to want when it does not say, or a browser will not let it be read.
const BUSY_WAIT: u64 = 1;
/// Seconds before asking again from a node that says its tip is below a block already final.
const BEHIND_WAIT: f64 = 5.0;

/// What the blocks read are checked against: a light client, which followed them.
pub trait Followed {
    /// The chain's hash.
    fn hash(&self) -> HashType;
    /// A header it holds, by height.
    fn sealed_at(&self, height: u64) -> Option<&SealedHeader>;
    /// The height up to which blocks are final.
    fn final_height(&self) -> u64;
    /// The lowest height it holds a header at.
    fn lowest(&self) -> u64;
}

impl Followed for LightClient {
    fn hash(&self) -> HashType {
        self.params().hash
    }

    fn sealed_at(&self, height: u64) -> Option<&SealedHeader> {
        LightClient::sealed_at(self, height)
    }

    fn final_height(&self) -> u64 {
        LightClient::final_height(self)
    }

    fn lowest(&self) -> u64 {
        LightClient::lowest(self)
    }
}

/// One request: from= and entry= of `GET /blocks/range`, and how many blocks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Asked {
    pub from: u64,
    pub entry: u32,
    pub count: u64,
}

impl Asked {
    pub fn path(&self) -> String {
        match self.entry {
            0 => format!("/blocks/range?from={}&count={}", self.from, self.count),
            entry => format!("/blocks/range?from={}&entry={entry}&count={}", self.from, self.count),
        }
    }
}

/// What came of a request, read as far as it can be without the light client: safe to work out on
/// the thread the answer arrived on, as a large range takes a moment to read.
#[derive(Debug, PartialEq)]
pub enum Came {
    /// A range that reads, as big as it was. Nothing in it is checked yet.
    Range { range: BlockRange, bytes: u64 },
    /// Bytes that do not read as a range.
    Bad { why: RangeError, bytes: u64 },
    /// The node asks to wait, as 429 and 503 do, this many seconds.
    Busy { wait: u64, bytes: u64 },
    /// The node said no, as it will again, and what it said.
    Refused { status: u16, message: String, bytes: u64 },
    /// No answer, or one that says the trouble is the node's for now: why.
    Away(String),
}

impl Came {
    /// Reads what a request came to.
    pub fn of(answer: Result<Answer, String>) -> Came {
        let answer = match answer {
            Ok(answer) => answer,
            Err(why) => return Came::Away(why),
        };
        let bytes = answer.bytes.len() as u64;

        match answer.status {
            200..=299 => match decode_block_range(&answer.bytes) {
                Ok(range) => Came::Range { range, bytes },
                Err(why) => Came::Bad { why, bytes },
            },
            429 | 503 => Came::Busy { wait: answer.retry_after.unwrap_or(BUSY_WAIT).clamp(1, LONGEST_WAIT), bytes },
            500.. => Came::Away(format!("{} {}", answer.status, answer.status_text)),
            status => {
                let said = serde_json::from_slice::<serde_json::Value>(&answer.bytes).ok().and_then(|value| value["error"].as_str().map(str::to_string));

                Came::Refused { status, message: said.unwrap_or_else(|| format!("{status} {}", answer.status_text)), bytes }
            }
        }
    }
}

/// What a block says of what it changed, and what its receipts are: a block that cannot say must
/// count as unknown, never as having changed nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Speaks {
    /// Whether it lists the state entries it changed.
    pub state: bool,
    /// Whether every one of its receipts is there.
    pub receipts: bool,
}

/// What a checked block can say. The part of a block that goes on from an earlier one says what that
/// one said, each of its entries proven.
pub fn speaks_for(item: &CheckedItem) -> Speaks {
    Speaks {
        state: matches!(item.changes, RangeChanges::Proven { .. }),
        receipts: match &item.body {
            RangeBody::Whole { receipts, .. } => receipts.is_some(),
            RangeBody::HeaderOnly => false,
            RangeBody::Continued => true,
        },
    }
}

/// Heights not yet read, below what was. Its top block's id is known, and proves everything below.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stretch {
    pub bottom: u64,
    pub top: u64,
    /// The top block's id.
    pub anchor: String,
    /// Whether it lies among blocks newer than the start: what the reading knows of the state is out
    /// of date until it is read.
    pub hole: bool,
}

/// Where a reading has got to: what an app keeps, to go on from there.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Place {
    /// Every block from the start up to this has been read, holes aside.
    pub high: u64,
    /// How many entries of the next block came already.
    pub entry: u32,
    /// What is left to read below, the ones to do first last.
    pub below: Vec<Stretch>,
    /// The height it was asked to read from.
    pub floor: u64,
    /// The id of the block just below the lowest read, to go further back from.
    pub floor_parent: Option<String>,
    /// The lowest height this node has anything of, once it said it has nothing below.
    pub held_from: Option<u64>,
    /// Bytes downloaded, all told, kept or not.
    pub downloaded: u64,
    /// Blocks read and kept.
    pub read: u64,
}

impl Place {
    /// A reading that starts at this final block, which what it holds of the state was proven at, and
    /// goes back to this height. None while the block's header is not held.
    pub fn starting(followed: &impl Followed, at: u64, floor: u64) -> Option<Place> {
        let anchor = followed.sealed_at(at)?.id().to_string();
        let floor = floor.min(at);

        Some(Place { high: at, floor, below: vec![Stretch { bottom: floor, top: at, anchor, hole: false }], ..Place::default() })
    }

    /// How far back it knows without a break: below this it says nothing.
    pub fn known_from(&self) -> u64 {
        self.below.iter().filter(|stretch| !stretch.hole).map(|stretch| stretch.top + 1).max().unwrap_or(self.floor).max(self.held_from.unwrap_or(0))
    }

    /// Whether a stretch of newer blocks is still to read: what it holds of the state is behind.
    pub fn catching_up(&self) -> bool {
        self.below.iter().any(|stretch| stretch.hole)
    }

    /// Whether it read everything it was asked to, up to this height.
    pub fn done(&self, final_height: u64) -> bool {
        self.below.is_empty() && self.high >= final_height
    }

    /// Reads further back, down to this height, on from what was read: what it read stands. Whether
    /// that changes anything.
    pub fn further_back(&mut self, floor: u64) -> bool {
        if floor >= self.floor {
            return false;
        }

        match self.below.iter_mut().find(|stretch| !stretch.hole) {
            Some(stretch) => stretch.bottom = floor,
            None => match &self.floor_parent {
                Some(anchor) if self.floor > 0 => {
                    self.below.insert(0, Stretch { bottom: floor, top: self.floor - 1, anchor: anchor.clone(), hole: false });
                }
                _ => return false,
            },
        }

        self.floor = floor;
        self.held_from = None;

        true
    }

    /// Lets go of everything below this height, as an app with too much to keep does, oldest first:
    /// it knows from here now, whatever it had left to read below. `parent` is the id of the block
    /// just below this one, to read further back from. Whether that changes anything.
    pub fn let_go_below(&mut self, height: u64, parent: String) -> bool {
        if height <= self.floor && self.below.iter().all(|stretch| stretch.hole || stretch.bottom >= height) {
            return false;
        }

        self.below.retain(|stretch| stretch.top >= height);

        for stretch in self.below.iter_mut().filter(|stretch| stretch.bottom < height) {
            stretch.bottom = height;
        }

        self.floor = height;
        self.floor_parent = Some(parent);
        self.held_from = None;

        true
    }

    /// Reads again everything from this height on up to where it has got to: for what the reading
    /// passed over before it was looking for it, as an account added since. What it read stands.
    pub fn again_from(&mut self, followed: &impl Followed, floor: u64) -> bool {
        let Some(anchor) = followed.sealed_at(self.high).map(|sealed| sealed.id().to_string()) else {
            return false;
        };

        self.below.retain(|stretch| stretch.hole);
        self.below.insert(0, Stretch { bottom: floor.min(self.high), top: self.high, anchor, hole: false });
        self.floor = floor.min(self.high);
        self.held_from = None;

        true
    }
}

/// What stops a reading until someone says to try again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Trouble {
    /// The node has no such route: older than it, or something in front of it turns it away.
    NoRoute,
    /// An answer that does not check: what was wrong with it.
    Bad(String),
    /// The node refused, and goes on refusing.
    Refused(String),
}

impl Trouble {
    /// What to say of it, in sentences.
    pub fn sentence(&self) -> String {
        match self {
            Trouble::NoRoute => "This node does not serve checked blocks (GET /blocks/range): it is older than that route, or something in front of it turns the route away. Nothing is asked of the node instead. Use a node that serves them, or turn this setting off.".to_string(),
            Trouble::Bad(why) => format!("This node sent {why}. Nothing from that answer was kept. Reading stopped: try again, or use another node."),
            Trouble::Refused(why) => format!("This node refused to give blocks: {why}. Reading stopped: try again, or use another node."),
        }
    }
}

/// What a reading hands back, once it stands on what checked.
#[derive(Clone, Debug, PartialEq)]
pub enum Read<P> {
    /// What was picked from a block, or the part of one that came in one answer. Blocks going down
    /// come by the stretch, in order within it, and not in order across stretches.
    Picked(P),
    /// Heights the node holds nothing of, and which will not be read. Whether they were a hole, which
    /// says what is known of the state above them may be out of date.
    Unread { from: u64, to: u64, hole: bool },
}

/// How a block's id and parent came, to link them up.
#[derive(Clone, Debug)]
struct Link {
    height: u64,
    id: String,
    parent: String,
}

/// A stretch being read: what was picked waits here until the top's id says it stands.
#[derive(Debug)]
struct Run<P> {
    stretch: Stretch,
    /// The first height of the part of it being read now.
    from: u64,
    /// Where to ask next: the next block, and how far through its entries.
    at: RangeCursor,
    links: Vec<Link>,
    picked: Vec<P>,
}

/// A reading under way.
#[derive(Debug)]
pub struct Scanner<P> {
    place: Place,
    run: Option<Run<P>>,
    /// What is out.
    out: Option<Asked>,
    /// When it may ask again.
    wait_until: f64,
    /// How many times in a row it had to wait.
    waits: u32,
    /// Why it waits, for a page to say.
    waiting: Option<String>,
    trouble: Option<Trouble>,
}

impl<P> Scanner<P> {
    /// Goes on from this place.
    pub fn new(place: Place) -> Scanner<P> {
        Scanner { place, run: None, out: None, wait_until: 0.0, waits: 0, waiting: None, trouble: None }
    }

    pub fn place(&self) -> &Place {
        &self.place
    }

    /// Reads further back: see `Place::further_back`. A stretch under way that was given a new
    /// bottom goes on to it next time round.
    pub fn further_back(&mut self, floor: u64) -> bool {
        self.place.further_back(floor)
    }

    /// Lets go of what is below this height: see `Place::let_go_below`. A stretch under way below it
    /// is dropped, and whatever it asked comes to nothing.
    pub fn let_go_below(&mut self, height: u64, parent: String) -> bool {
        if self.run.as_ref().is_some_and(|run| !run.stretch.hole && run.stretch.top < height) {
            self.run = None;
            self.out = None;
        }

        self.place.let_go_below(height, parent)
    }

    /// Reads again from this height: see `Place::again_from`.
    pub fn again_from(&mut self, followed: &impl Followed, floor: u64) -> bool {
        self.run = None;
        self.out = None;

        self.place.again_from(followed, floor)
    }

    /// Why it stopped, until `retry`.
    pub fn trouble(&self) -> Option<&Trouble> {
        self.trouble.as_ref()
    }

    /// Why it is waiting, while it is.
    pub fn waiting(&self) -> Option<&str> {
        self.waiting.as_deref()
    }

    /// Whether it is in the middle of a stretch going down, so what was picked waits to stand.
    pub fn going_down(&self) -> bool {
        self.run.is_some()
    }

    /// Whether a request is out.
    pub fn asking(&self) -> bool {
        self.out.is_some()
    }

    /// What is out.
    pub fn out(&self) -> Option<Asked> {
        self.out
    }

    /// Asks again whatever stopped it, at once: what someone pressed.
    pub fn retry(&mut self) {
        self.trouble = None;
        self.wait_until = 0.0;
        self.waits = 0;
        self.waiting = None;
    }

    /// Forgets what is out: the way to the node changed, so its answer is not coming. A stretch under
    /// way starts again, as what it had waiting came from elsewhere.
    pub fn line_changed(&mut self) {
        self.out = None;
        self.run = None;
        self.waiting = None;
        self.wait_until = 0.0;
    }

    /// What to ask now, if anything: nothing while something is out, while it waits, or once it has
    /// stopped. It asks one thing at a time, as a node asks of its clients.
    pub fn next(&mut self, followed: &impl Followed, time: f64) -> Option<Asked> {
        if self.trouble.is_some() || self.out.is_some() || time < self.wait_until {
            return None;
        }

        self.waiting = None;
        let final_height = followed.final_height();

        // The light client took up the chain above where this got to, so what lies between is read
        // down from there.
        if self.run.is_none() && self.place.high < final_height && self.place.high + 1 < followed.lowest() {
            let lowest = followed.lowest();
            let anchor = followed.sealed_at(lowest)?.header.parent.to_string();

            self.place.below.push(Stretch { bottom: self.place.high + 1, top: lowest - 1, anchor, hole: true });
            self.place.high = lowest - 1;
            self.place.entry = 0;
        }

        let asked = if let Some(run) = &self.run {
            Asked { from: run.at.height, entry: run.at.entry, count: run.stretch.top - run.at.height + 1 }
        } else if let Some(place) = self.place.below.iter().rposition(|stretch| stretch.hole) {
            self.begin_run(place)
        } else if self.place.high < final_height {
            Asked { from: self.place.high + 1, entry: self.place.entry, count: (final_height - self.place.high).min(MOST_BLOCKS) }
        } else {
            let place = self.place.below.len().checked_sub(1)?;

            self.begin_run(place)
        };
        self.out = Some(asked);

        Some(asked)
    }

    /// Starts on a stretch's next part, from its top down.
    fn begin_run(&mut self, place: usize) -> Asked {
        let stretch = self.place.below[place].clone();
        let from = stretch.bottom.max(stretch.top.saturating_sub(MOST_BLOCKS - 1));
        let asked = Asked { from, entry: 0, count: stretch.top - from + 1 };

        self.run = Some(Run { stretch, from, at: RangeCursor { height: from, entry: 0 }, links: Vec::new(), picked: Vec::new() });

        asked
    }

    /// Takes what came of the request out. Whatever does not check stops the reading, and nothing of
    /// it is handed back: `pick` runs only on what checked, and what it returned for a stretch going
    /// down comes back only once the stretch has shown it stands.
    pub fn came(&mut self, asked: Asked, came: Came, followed: &impl Followed, time: f64, pick: impl FnMut(&CheckedItem) -> P) -> Vec<Read<P>> {
        // An answer to what is no longer asked, as from before the node changed.
        if self.out != Some(asked) {
            return Vec::new();
        }

        self.out = None;

        match came {
            Came::Range { range, bytes } => {
                self.place.downloaded += bytes;
                self.waits = 0;

                match self.take_range(asked, range, followed, time, pick) {
                    Ok(read) => read,
                    Err(why) => {
                        self.stop(Trouble::Bad(why));

                        Vec::new()
                    }
                }
            }
            Came::Bad { why, bytes } => {
                self.place.downloaded += bytes;
                self.stop(Trouble::Bad(why.to_string()));

                Vec::new()
            }
            Came::Busy { wait, bytes } => {
                self.place.downloaded += bytes;
                self.waits += 1;
                // However little it asks for, the longer this goes on the longer it waits.
                let waited = (wait as f64).max(2f64.powi(self.waits.min(5) as i32 - 1));
                self.wait_until = time + waited;
                self.waiting = Some(format!("The node is busy, and asks to wait: asking again in {waited:.0} seconds."));

                Vec::new()
            }
            Came::Away(why) => {
                self.waits += 1;
                let waited = (AWAY_FIRST * 2f64.powi(self.waits.min(16) as i32 - 1)).min(AWAY_MOST);
                self.wait_until = time + waited;
                self.waiting = Some(format!("Could not reach the node: {why}. Asking again in {waited:.0} seconds."));

                Vec::new()
            }
            Came::Refused { status, message, bytes } => {
                self.place.downloaded += bytes;

                self.refused(asked, status, message)
            }
        }
    }

    /// The node said no. An older node reads /blocks/range as a block named range, and says 400; a
    /// newer one says that with entry=0 only of what this never sends.
    fn refused(&mut self, asked: Asked, status: u16, message: String) -> Vec<Read<P>> {
        let lower = self.run.as_ref().map(|run| asked.from < run.stretch.top && run.links.is_empty());

        match (status, lower) {
            (400, _) if asked.entry == 0 => self.stop(Trouble::NoRoute),
            (400, _) => self.stop(Trouble::Bad("a block whose entries it gave part of, and now says are not there".to_string())),
            // Nothing held that low: higher up a stretch may still be. Halving finds where it starts.
            (404, Some(true)) => {
                let run = self.run.as_mut().expect("going down");
                let from = asked.from + (run.stretch.top - asked.from).div_ceil(2);

                run.from = from;
                run.at = RangeCursor { height: from, entry: 0 };
            }
            (404, Some(false)) => {
                let run = self.run.take().expect("going down");

                self.place.below.retain(|stretch| stretch.top != run.stretch.top || stretch.anchor != run.stretch.anchor);

                if !run.stretch.hole {
                    self.place.held_from = Some(run.stretch.top + 1);
                    self.place.floor = run.stretch.top + 1;
                }

                return vec![Read::Unread { from: run.stretch.bottom, to: run.stretch.top, hole: run.stretch.hole }];
            }
            (404, None) => self.stop(Trouble::NoRoute),
            _ => self.stop(Trouble::Refused(message)),
        }

        Vec::new()
    }

    /// Stops, dropping whatever a stretch had waiting: it never showed it stands.
    fn stop(&mut self, trouble: Trouble) {
        self.run = None;
        self.trouble = Some(trouble);
    }

    fn take_range(&mut self, asked: Asked, range: BlockRange, followed: &impl Followed, time: f64, mut pick: impl FnMut(&CheckedItem) -> P) -> Result<Vec<Read<P>>, String> {
        range.check_shape(asked.from, asked.entry).map_err(|why| why.to_string())?;

        // A block whose list says there is more, with nothing more given, comes round for ever.
        if range.next == Some(RangeCursor { height: asked.from, entry: asked.entry }) {
            return Err(RangeError::OutOfPlace.to_string());
        }

        let hash = followed.hash();
        let (next, tip) = (range.next, range.tip);
        let last_asked = asked.from + asked.count - 1;
        let Some(last) = range.items.last().map(|item| item.sealed.header.height) else {
            self.wait_until = time + BEHIND_WAIT;
            self.waiting = Some(format!("The node says its tip is block {tip}, behind what is final. Asking again in {BEHIND_WAIT:.0} seconds."));

            return Ok(Vec::new());
        };

        if last > last_asked {
            return Err("more blocks than were asked for".to_string());
        }

        let going_down = self.run.is_some();
        let mut checked = Vec::with_capacity(range.items.len());

        for item in range.items {
            let sealed = match going_down {
                // Going down, each is its own to check by, until the stretch's top says it stands.
                true => item.sealed.clone(),
                false => followed.sealed_at(item.sealed.header.height).ok_or_else(|| RangeError::WrongHeader.to_string())?.clone(),
            };

            checked.push(check_range_item(hash, &sealed, item).map_err(|why| why.to_string())?);
        }

        if !going_down {
            let picked = checked.iter().map(|item| Read::Picked(pick(item))).collect();
            let unfinished = next.filter(|next| next.height == last);
            let whole = checked.iter().filter(|item| unfinished.is_none() || item.height() < last).count() as u64;

            match unfinished {
                Some(next) => (self.place.high, self.place.entry) = (last - 1, next.entry),
                None => (self.place.high, self.place.entry) = (last, 0),
            }

            self.place.read += whole;

            return Ok(picked);
        }

        let run = self.run.as_mut().expect("going down");

        for item in &checked {
            let (height, id) = (item.height(), item.block.to_string());

            match run.links.last() {
                // The part of a block that goes on from the last answer must be that block.
                Some(link) if link.height == height && link.id != id => return Err(RangeError::WrongHeader.to_string()),
                Some(link) if link.height == height => {}
                _ => run.links.push(Link { height, id, parent: item.sealed.header.parent.to_string() }),
            }

            run.picked.push(pick(item));
        }

        match next.filter(|next| next.height <= run.stretch.top) {
            Some(next) => {
                run.at = next;

                Ok(Vec::new())
            }
            None if last < run.stretch.top => Err(format!("a tip below block {}, which it should hold", run.stretch.top)),
            None => self.finish_run(),
        }
    }

    /// The whole part of a stretch came: whether it links up to the top's id.
    fn finish_run(&mut self) -> Result<Vec<Read<P>>, String> {
        let run = self.run.take().expect("going down");
        let starts = run.links.first().is_some_and(|first| first.height == run.from);
        let links = run.links.windows(2).all(|pair| pair[1].height == pair[0].height + 1 && pair[1].parent == pair[0].id);
        let ends = run.links.last().is_some_and(|last| last.height == run.stretch.top && last.id == run.stretch.anchor);

        if !(starts && links && ends) {
            return Err("blocks that do not link up to the chain the light client checked".to_string());
        }

        let first = run.links.first().expect("it starts");
        // A stretch whose bottom moved while it ran still ends with its top coming down.
        let place = self.place.below.iter().position(|stretch| stretch.top == run.stretch.top && stretch.anchor == run.stretch.anchor);

        if let Some(place) = place {
            let stretch = &mut self.place.below[place];

            if run.from <= stretch.bottom {
                self.place.below.remove(place);
            } else {
                stretch.top = run.from - 1;
                stretch.anchor.clone_from(&first.parent);
            }
        }

        if !run.stretch.hole {
            self.place.floor_parent = Some(first.parent.clone());
        }

        self.place.read += run.links.len() as u64;

        Ok(run.picked.into_iter().map(Read::Picked).collect())
    }
}

/// A state column, by the name the API gives it.
pub fn column_named(name: &str) -> Option<Column> {
    STATE_COLUMNS.into_iter().find(|column| column.name() == name)
}

/// What a proof a node answered says is held, as GET /proofs/{column}/{key} writes it, once it checks
/// against the header of the block asked for, which is final, as nothing else is asked of it.
pub fn proven_value(column: &str, key: &str, block: &str, value: &Value, height: u64, followed: &impl Followed) -> Result<Option<Vec<u8>>, String> {
    let column = column_named(column).ok_or("a column it never asks for")?;
    let raw_key = from_hex(key).ok_or("a key that is not hex")?;
    let sealed = followed.sealed_at(height).ok_or("a block it holds no header of")?;

    if value["block"].as_str() != Some(block) || sealed.id().to_string() != block || height > followed.final_height() {
        return Err("a proof at another block than the one asked for".to_string());
    }

    let held = value["value"].as_str().map(|text| from_hex(text).ok_or("a value that is not hex")).transpose()?;
    let proof: StateProof = from_hex(value["proof"].as_str().ok_or("no proof")?).and_then(|bytes| wincode::deserialize(&bytes).ok()).ok_or("a proof that does not read")?;

    match proof.verify(&sealed.header.state_root, column, &raw_key, held.as_deref()) {
        true => Ok(held),
        false => Err("it does not check against the block's state root".to_string()),
    }
}
