//! Read from checked blocks: a wallet that takes what it shows from blocks it fetched and checked,
//! rather than ask a node, so what it shows rests on those blocks and the node learns less of what it
//! looks at. The reading itself is chainv4_ui::blocks; what this holds is what a wallet makes of it,
//! its index: its accounts' values as the blocks' proofs update them, and the transactions that touch
//! them.
//!
//! The index is private as addresses and history are: sealed with a key made from the wallet's vault
//! key, so it opens only while the wallet is unlocked, and reading only goes on then. It lives apart
//! from the vault's file, on a shelf of its own, so that nothing large endangers the vault or slows its
//! save. Losing it costs a rescan, never a wallet: one that will not open or read is set aside, and
//! rebuilt, and the wallet says so.
//!
//! What a value is known from is a direct proof, once, at the final block the reading started at, then
//! the entries blocks list, the highest block's standing. Blocks can fail that three ways, each of
//! which this says plainly:
//! - A block that lists nothing, or has no body, is unknown rather than unchanged. A value older than
//!   the newest such block is out of date, and is not shown; one direct proof brings it up to date.
//! - A node can leave an entry out of a block's list, which nothing can tell. The value stays proven
//!   but stale until someone presses Check now, which proves it directly.
//! - Something first looked for after the blocks that could have changed it, as a token first seen
//!   in an older block read since: what is proven of it there is no use for now. One direct proof.

use {
    crate::chain::{Chain, Proven},
    crate::history::{Checked, Origin},
    crate::chain::Landed,
    crate::node::{Ask, Failure},
    chacha20poly1305::aead::{Aead, KeyInit},
    chacha20poly1305::{ChaCha20Poly1305, Key, Nonce},
    chainv4::db::{Column, address_key},
    chainv4::light::{Assurance, CheckedItem, RangeBody, token_balance, token_balance_key},
    chainv4::registry::names::{NameRecord, record_key},
    chainv4::registry::tokens::{Token, token_key},
    chainv4::structs::account::Account,
    chainv4::structs::address::Address,
    chainv4::structs::hash::{Hash, HashType},
    chainv4::structs::hex::{from_hex, to_hex},
    chainv4::structs::tx::Tx,
    chainv4::structs::u256::u256,
    chainv4_ui::blocks::fees::{FeesSeen, used_in},
    chainv4_ui::blocks::touched::{Ours, touches},
    chainv4_ui::blocks::{Asked, Came, Followed, Place, Read, Scanner, Speaks, Trouble, column_named, proven_value, speaks_for},
    chainv4_ui::shelf::{Keeper, MemoryShelf},
    hkdf::Hkdf,
    serde::{Deserialize, Serialize},
    serde_json::Value,
    sha2::Sha256,
    std::collections::{BTreeMap, BTreeSet, HashMap},
    std::sync::{Arc, Mutex, PoisonError},
    zeroize::Zeroizing,
};

/// What sets the key an index is sealed with apart from the vault's own.
const INDEX_INFO: &[u8] = b"chainv4-wallet-index-v1";
/// Seconds between writing the index out while it changes.
const SAVE_EVERY: f64 = 30.0;
/// Seconds before asking again for a proof that did not come.
const PROOF_AGAIN: f64 = 15.0;
/// The least seconds between catching a value up after a gap: a node that has no proofs of any block
/// would otherwise be told about every account with every block.
const CATCH_UP_EVERY: f64 = 300.0;
/// The most proofs out at once.
const PROOFS_AT_ONCE: usize = 8;
/// The most transactions kept, the newest, unless Settings says otherwise: an address that busy shows
/// what it can.
pub(crate) const MOST_TXS: usize = 20_000;
/// The fewest and the most Settings may say: they are all in memory while the wallet is open, sealed
/// and written out whole each time they change.
pub(crate) const LEAST_TXS: usize = 1_000;
pub(crate) const MOST_TXS_SET: usize = 500_000;

/// What an entry in the state is, to this wallet.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Target {
    Account(String),
    Balance { token: String, holder: String },
    /// A token's record: its symbol and decimals.
    Token(String),
    /// A name's record, by name without the suffix.
    Name(String),
}

/// A block something was proven at.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct At {
    height: u64,
    block: String,
}

/// A state entry the wallet follows, and what it knows of it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Known {
    target: Target,
    column: String,
    key: String,
    /// What it holds, in hex, None for nothing, as of `at`.
    value: Option<String>,
    /// Where that was proven: nowhere yet, until a proof or an entry says.
    at: Option<At>,
    /// The height the reading had got to when it began looking for it: a value older than that says
    /// nothing, as the blocks between were passed over.
    watched_from: u64,
}

/// What a wallet keeps of what it read. All of it can be read again.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Index {
    /// Where the reading has got to, once it started.
    place: Option<Place>,
    known: BTreeMap<String, Known>,
    txs: BTreeMap<String, Checked>,
    /// The height the reading goes back to for each address, once it started looking for it.
    since: BTreeMap<String, u64>,
    /// The newest block that could not say what it changed.
    state_gap: Option<u64>,
    /// Heights whose receipts did not come: blocks whose transactions are not known.
    unread: Vec<(u64, u64)>,
    /// The height below which transactions were let go, for there being too many to keep.
    cut_below: Option<u64>,
    /// Proofs asked for and taken: to start from or catch up, and because someone pressed Check now.
    proofs: u64,
    checks: u64,
    /// Whether the last Check now found the blocks' lists had left something out, and of what.
    omitted: Vec<String>,
    /// What fees it saw paid.
    #[serde(flatten)]
    fees: FeesSeen,
}

/// What a checked block came to for this wallet: what is held until the block stands.
#[derive(Clone, Debug, PartialEq)]
pub struct Picked {
    height: u64,
    block: String,
    speaks: Speaks,
    /// Transactions that touch the wallet's addresses.
    txs: Vec<(String, Checked)>,
    /// Entries it follows, by what they are known under, and what each holds.
    entries: Vec<(String, Option<Vec<u8>>)>,
    /// Tokens and names first come across here.
    tokens: Vec<(String, String)>,
    names: Vec<String>,
    /// What the block's transactions used, by kind, and the most any paid per gas.
    hints: Vec<(String, u64)>,
    price: Option<u256>,
}

/// What the keys followed are looked up by, to find an entry among a block's many.
type Watch = HashMap<Vec<u8>, Vec<(Column, String)>>;

/// How a request for a proof went out: for what, and at what.
#[derive(Clone, Debug)]
struct Proving {
    known: String,
    height: u64,
    block: String,
    asked_at: f64,
    /// Because someone pressed Check now, and tells them what it found.
    checking: bool,
}

struct Open {
    name: String,
    /// The vault's key it was opened by, which a change of password changes, and what seals it.
    vault_key: Zeroizing<[u8; 32]>,
    key: Zeroizing<[u8; 32]>,
    genesis: String,
    index: Index,
    scanner: Option<Scanner<Picked>>,
    ours: Ours,
    watch: Watch,
    /// What was picked up from blocks going down, which do not stand yet, to go on looking for.
    tentative: Watch,
    proving: HashMap<String, Proving>,
    /// When each last took a proof, to catch it up at most so often.
    proven_at: HashMap<String, f64>,
    /// Keys to prove because someone pressed Check now.
    checking: BTreeSet<String>,
    saved_at: f64,
    dirty: bool,
    /// Whether what was shown is behind what is known, and whether it was shown catching up.
    stale_view: bool,
    shown_behind: bool,
    /// The addresses last followed, so nothing is worked out again while they stay.
    addresses: Vec<Address>,
    /// Transactions not shown yet: the whole index, the first time.
    fresh: Vec<String>,
    /// What to start from, once it can: a wallet's height, or one someone picked.
    floor: Option<u64>,
    /// The most transactions it keeps.
    most_txs: usize,
}

/// What opening an index came to, once a shelf answers.
type Opening = Arc<Mutex<Option<Result<Option<Vec<u8>>, String>>>>;

/// What goes with an index's being opened.
struct Opened {
    name: String,
    vault_key: Zeroizing<[u8; 32]>,
    genesis: String,
    inbox: Opening,
}

/// A wallet's reading: nothing while it is locked or the setting is off.
pub struct Reading {
    keeper: Keeper,
    /// Whether an index lasts: false where there is nowhere to put one.
    lasting: bool,
    opening: Option<Opened>,
    open: Option<Open>,
    /// What to tell someone of the index, until they have seen it.
    pub said: Vec<String>,
    /// The most transactions an index keeps, as Settings says.
    most_txs: usize,
}

impl Default for Reading {
    fn default() -> Reading {
        Reading { keeper: Keeper::new(Arc::new(MemoryShelf::default())), lasting: false, opening: None, open: None, said: Vec::new(), most_txs: MOST_TXS }
    }
}

/// What a frame's reading needs of the wallet.
pub struct Wanted<'a> {
    /// The wallet's name for its files, and the chain's genesis.
    pub wallet: &'a str,
    pub genesis: &'a str,
    pub vault_key: Zeroizing<[u8; 32]>,
    pub addresses: Vec<Address>,
    /// The height it was made at, for a wallet made here.
    pub born: Option<u64>,
}

/// What to ask now.
#[derive(Default)]
pub struct Go {
    pub proofs: Vec<Ask>,
    pub range: Option<Asked>,
}

/// Where a reading stands, for a page to say.
#[derive(Clone, Debug, PartialEq)]
pub struct Standing {
    /// How far up it has read.
    pub high: u64,
    /// How far back it knows without a break.
    pub known_from: u64,
    /// How far back it was asked to go, and what lies between that and `known_from`, in blocks.
    pub floor: u64,
    pub left_below: u64,
    pub catching_up: bool,
    pub downloaded: u64,
    pub read: u64,
    /// Direct proofs taken: to start from or catch up, and because of Check now.
    pub proofs: u64,
    pub checks: u64,
    /// Why it waits, or stopped.
    pub waiting: Option<String>,
    pub trouble: Option<String>,
    /// The newest block that could not say what it changed.
    pub state_gap: Option<u64>,
    /// Blocks whose transactions are not known, in all.
    pub unread: u64,
    /// The lowest height the node has, once it said it has nothing below.
    pub held_from: Option<u64>,
    /// Below this, transactions were let go for there being too many.
    pub cut_below: Option<u64>,
    /// What Check now found was left out.
    pub omitted: Vec<String>,
    /// Values that are out of date, which take a proof to show.
    pub stale: usize,
}

/// A reading whose indexes last in this folder, in the private one the wallets are kept in, or for
/// this run alone with none.
#[cfg(not(target_arch = "wasm32"))]
pub fn on_this_device(folder: Option<&std::path::Path>) -> Reading {
    match folder.map(|folder| chainv4_ui::shelf::FileShelf::new(folder.to_path_buf())) {
        Some(Ok(shelf)) => Reading::new(Keeper::new(Arc::new(shelf)), true),
        Some(Err(why)) => {
            log::warn!("{why}");

            Reading::default()
        }
        None => Reading::default(),
    }
}

/// A reading whose indexes last in this browser's IndexedDB, as localStorage has no room for them.
#[cfg(target_arch = "wasm32")]
pub fn on_this_device() -> Reading {
    Reading::new(Keeper::new(Arc::new(chainv4_ui::shelf::IndexedDbShelf::new("chainv4-wallet-indexes"))), true)
}

/// The name an index goes by on a shelf, starting with its wallet's: it goes when the wallet does.
pub fn index_name(wallet: &str, genesis: &str) -> String {
    format!("{wallet}-{genesis}")
}

/// The key an index is sealed with, from the key of the vault it belongs to, so that a sealed index
/// and a sealed vault share neither a key nor a nonce's chance of colliding.
fn index_key(vault_key: &[u8; 32]) -> Result<Zeroizing<[u8; 32]>, String> {
    let mut key = Zeroizing::new([0; 32]);

    Hkdf::<Sha256>::new(None, vault_key).expand(INDEX_INFO, &mut *key).map_err(|err| format!("could not make the index's key: {err}"))?;

    Ok(key)
}

fn seal(key: &[u8; 32], plain: &[u8]) -> Result<Vec<u8>, String> {
    let mut nonce = [0; 12];
    getrandom::fill(&mut nonce).map_err(|err| format!("no randomness for a nonce: {err}"))?;
    let sealed = ChaCha20Poly1305::new(Key::from_slice(key)).encrypt(Nonce::from_slice(&nonce), plain).map_err(|_| "could not seal the index".to_string())?;

    Ok([nonce.as_slice(), &sealed].concat())
}

fn unseal(key: &[u8; 32], bytes: &[u8]) -> Result<Vec<u8>, String> {
    let (nonce, sealed) = bytes.split_at_checked(12).ok_or("an index too short to have been sealed")?;

    ChaCha20Poly1305::new(Key::from_slice(key)).decrypt(Nonce::from_slice(nonce), sealed).map_err(|_| "an index that does not open with this wallet's key".to_string())
}

/// What an index goes by among those followed.
fn known_key(column: Column, key: &[u8]) -> String {
    format!("{}/{}", column.name(), to_hex(key))
}

fn parts_of(target: &Target) -> Option<(Column, Vec<u8>)> {
    match target {
        Target::Account(address) => Some((Column::Accounts, address_key(&address.parse().ok()?))),
        Target::Balance { token, holder } => Some((Column::Namespaces, token_balance_key(&token.parse().ok()?, &holder.parse().ok()?))),
        Target::Token(token) => Some((Column::Tokens, token_key(&token.parse().ok()?))),
        Target::Name(name) => Some((Column::Names, record_key(name))),
    }
}

/// A new entry to follow from this height, with nothing known of it yet.
fn following(target: Target, watched_from: u64) -> Option<(String, Known)> {
    let (column, key) = parts_of(&target)?;

    Some((known_key(column, &key), Known { target, column: column.name().to_string(), key: to_hex(&key), value: None, at: None, watched_from }))
}

fn add_to_watch(watch: &mut Watch, column: Column, key: Vec<u8>, known: String) {
    let listed = watch.entry(key).or_default();

    if !listed.iter().any(|(_, listed)| *listed == known) {
        listed.push((column, known));
    }
}

impl Index {
    /// Whether what is known of this is out of date, or nothing is.
    fn stale(&self, known: &Known) -> bool {
        known.at.as_ref().is_none_or(|at| at.height < known.watched_from || self.state_gap.is_some_and(|gap| at.height < gap))
    }

    /// Lets go of the oldest transactions past what is kept.
    fn trim(&mut self, most: usize) {
        if self.txs.len() <= most {
            return;
        }

        let mut heights: Vec<u64> = self.txs.values().map(|tx| tx.height).collect();
        heights.sort_unstable();
        let cut = heights[heights.len() - most];
        self.txs.retain(|_, tx| tx.height >= cut);
        self.cut_below = Some(self.cut_below.map_or(cut, |below| below.max(cut)));
    }

    /// Notes heights whose receipts are not known, joined with those next to them.
    fn note_unread(&mut self, from: u64, to: u64) {
        self.unread.push((from, to));
        self.unread.sort_unstable();
        let mut joined: Vec<(u64, u64)> = Vec::with_capacity(self.unread.len());

        for (from, to) in self.unread.drain(..) {
            match joined.last_mut() {
                Some(last) if from <= last.1.saturating_add(1) => last.1 = last.1.max(to),
                _ => joined.push((from, to)),
            }
        }

        self.unread = joined;
    }
}

/// What a block comes to for a wallet watching these, as far as it can say.
fn picked_from(item: &CheckedItem, ours: &Ours, watch: &Watch, tentative: &mut Watch, genesis: &str, hash: HashType) -> Picked {
    let (hints, price) = used_in(item);
    let mut picked = Picked {
        height: item.height(),
        block: item.block.to_string(),
        speaks: speaks_for(item),
        txs: Vec::new(),
        entries: Vec::new(),
        tokens: Vec::new(),
        names: Vec::new(),
        hints,
        price,
    };

    if let RangeBody::Whole { txs, receipts: Some(receipts), .. } = &item.body {
        let time = item.sealed.header.timestamp;
        let assurance = Assurance { block: item.block, height: item.height(), final_: true };

        for (signed, receipt) in txs.iter().zip(receipts) {
            if !touches(signed, receipt, ours) {
                continue;
            }

            let id = signed.id(hash).to_string();
            let proven = Proven { value: Landed { signed: signed.clone(), receipt: receipt.clone(), time }, assurance };
            let checked = Checked { genesis: genesis.to_string(), came: Origin::Read, ..Checked::of(&proven) };

            for token in tokens_moved(&checked, ours) {
                note_token(&token, &mut picked, tentative);
            }

            for name in names_in(&signed.tx) {
                if let Some((column, key)) = parts_of(&Target::Name(name.clone())) {
                    let known = known_key(column, &key);

                    add_to_watch(tentative, column, key, known);
                    picked.names.push(name);
                }
            }

            picked.txs.push((id, checked));
        }
    }

    for entry in item.entries() {
        let found = watch.get(entry.key.as_slice()).into_iter().chain(tentative.get(entry.key.as_slice())).flatten().find(|(column, _)| *column == entry.column);

        if let Some((_, known)) = found {
            picked.entries.push((known.clone(), entry.value.clone()));
        }
    }

    picked
}

/// Which tokens a transaction moved to or from which of the wallet's addresses, by what a token's own
/// standard emits: an EVM log's claim of a move is shown as one, and followed by nothing here, as its
/// balance is no entry this can look up.
fn tokens_moved(checked: &Checked, ours: &Ours) -> Vec<(String, String)> {
    let mut moved = Vec::new();

    for one in checked.moves.iter().filter(|one| one.standard.is_none()) {
        let Some(token) = &one.token else {
            continue;
        };

        for party in [&one.from, &one.to].into_iter().flatten() {
            if ours.is_ours(party) {
                moved.push((token.clone(), party.clone()));
            }
        }
    }

    moved
}

/// Notes a token's balance and record to look for, from this block on.
fn note_token(moved: &(String, String), picked: &mut Picked, tentative: &mut Watch) {
    let (token, holder) = moved;

    for target in [Target::Balance { token: token.clone(), holder: holder.clone() }, Target::Token(token.clone())] {
        if let Some((column, key)) = parts_of(&target) {
            let known = known_key(column, &key);

            add_to_watch(tentative, column, key, known);
        }
    }

    picked.tokens.push(moved.clone());
}

/// The names a transaction names, for a wallet that sent it: what the Names page can show of them.
fn names_in(tx: &Tx) -> Vec<String> {
    tx.what_it_runs()
        .iter()
        .filter_map(|tx| match tx {
            Tx::Name { name, .. } => Some(name.clone()),
            _ => None,
        })
        .collect()
}

impl Open {
    fn new(name: String, vault_key: Zeroizing<[u8; 32]>, key: Zeroizing<[u8; 32]>, genesis: String, index: Index, floor: Option<u64>, most_txs: usize) -> Open {
        let scanner = index.place.clone().map(Scanner::new);
        let fresh = index.txs.keys().cloned().collect();
        let mut open = Open {
            name,
            vault_key,
            key,
            genesis,
            index,
            scanner,
            ours: Ours::default(),
            watch: Watch::new(),
            tentative: Watch::new(),
            proving: HashMap::new(),
            proven_at: HashMap::new(),
            checking: BTreeSet::new(),
            saved_at: 0.0,
            dirty: false,
            stale_view: true,
            shown_behind: false,
            addresses: Vec::new(),
            fresh,
            floor,
            most_txs,
        };

        open.rewatch();

        open
    }

    /// Works out afresh what to look for among a block's entries.
    fn rewatch(&mut self) {
        self.watch.clear();

        for (name, known) in &self.index.known {
            if let (Some(column), Some(key)) = (column_named(&known.column), from_hex(&known.key)) {
                add_to_watch(&mut self.watch, column, key, name.clone());
            }
        }
    }

    /// How far the reading has got.
    fn reached(&self) -> u64 {
        self.scanner.as_ref().map_or(0, |scanner| scanner.place().high)
    }

    /// Follows these addresses: any new one takes a proof, from where the reading is now.
    fn keep_addresses(&mut self, addresses: &[Address]) {
        if self.addresses == addresses {
            return;
        }

        self.addresses = addresses.to_vec();
        let reached = self.reached();
        // The first go goes back as far as the reading does; one added later, from where it has got.
        let since = match self.index.since.is_empty() {
            true => self.scanner.as_ref().map_or(0, |scanner| scanner.place().floor),
            false => reached,
        };
        let mut changed = false;

        for address in addresses {
            let text = address.to_string();

            if !self.index.since.contains_key(&text) {
                self.index.since.insert(text.clone(), since);
                self.dirty = true;
            }

            if let Some((name, known)) = following(Target::Account(text), reached)
                && let std::collections::btree_map::Entry::Vacant(place) = self.index.known.entry(name)
            {
                place.insert(known);
                changed = true;
            }
        }

        // An account gone from the wallet stays in the index, sealed, for if it comes back.
        let before = self.ours.len();
        self.ours = Ours::of(addresses);

        if changed || before != self.ours.len() {
            self.rewatch();
            self.stale_view = true;
        }
    }

    /// The proofs due: what has no value yet, and what fell out of date, once a gap is behind the
    /// reading, and what someone pressed Check now for. At the final block, or for a check where the
    /// reading has been, so that what it finds can be told from what has not been read yet.
    fn proofs_due(&mut self, followed: &impl Followed, time: f64) -> Vec<Ask> {
        let Some(place) = self.scanner.as_ref().map(|scanner| scanner.place().clone()) else {
            return Vec::new();
        };
        let final_height = followed.final_height();
        let held = |height: u64| followed.sealed_at(height).map(|sealed| sealed.id().to_string());
        let Some(final_block) = held(final_height) else {
            return Vec::new();
        };
        let behind_gap = !place.catching_up() && self.index.state_gap.is_none_or(|gap| gap < place.high || place.high >= final_height);
        let mut asks = Vec::new();
        self.proving.retain(|_, proving| time - proving.asked_at < PROOF_AGAIN);
        let checking: Vec<String> = self.checking.iter().cloned().collect();
        let stale: Vec<String> = self.index.known.iter().filter(|(_, known)| self.index.stale(known)).map(|(name, _)| name.clone()).collect();

        for name in checking.into_iter().map(|name| (name, true)).chain(stale.into_iter().map(|name| (name, false))) {
            let (name, is_check) = name;

            if self.proving.len() >= PROOFS_AT_ONCE {
                break;
            }

            if self.proving.contains_key(&name) {
                continue;
            }

            let Some(known) = self.index.known.get(&name) else {
                self.checking.remove(&name);

                continue;
            };
            let first = known.at.is_none();

            // What has no value goes first, whatever else is going on: it shows nothing until then.
            if !is_check && !first && (!behind_gap || self.proven_at.get(&name).is_some_and(|at| time - at < CATCH_UP_EVERY)) {
                continue;
            }

            let (height, block) = match is_check {
                true if place.high <= final_height => (place.high, held(place.high).unwrap_or_else(|| final_block.clone())),
                _ => (final_height, final_block.clone()),
            };
            let Some(column) = column_named(&known.column) else {
                continue;
            };
            let key = known.key.clone();

            self.proving.insert(name.clone(), Proving { known: name.clone(), height, block: block.clone(), asked_at: time, checking: is_check });
            asks.push(Ask::ScanProof { column: column.name(), key, block });
        }

        asks
    }

    /// Takes in what a block came to, once it stands. What was first come across in it is followed
    /// from there on, going up, as the blocks after it are read knowing to look. Going down, the blocks
    /// above were passed over: what it says here is no use for now, and a direct proof is.
    fn take(&mut self, picked: Picked, going_down: bool) {
        let Picked { height, block, speaks, txs, entries, tokens, names, hints, price } = picked;
        let from = if going_down { self.reached() } else { height };

        for (token, holder) in tokens {
            for target in [Target::Balance { token: token.clone(), holder: holder.clone() }, Target::Token(token.clone())] {
                self.follow_from(target, from);
            }
        }

        for name in names {
            self.follow_from(Target::Name(name), from);
        }

        for (name, value) in entries {
            if let Some(known) = self.index.known.get_mut(&name)
                && known.at.as_ref().is_none_or(|at| height >= at.height)
            {
                known.value = value.as_deref().map(to_hex);
                known.at = Some(At { height, block: block.clone() });
            }
        }

        if !speaks.state && height > 0 {
            self.index.state_gap = Some(self.index.state_gap.map_or(height, |gap| gap.max(height)));
        }

        if !speaks.receipts {
            self.index.note_unread(height, height);
        }

        for (id, checked) in txs {
            self.fresh.push(id.clone());
            self.index.txs.insert(id, checked);
        }

        self.index.fees.take(height, &hints, price);
        self.index.trim(self.most_txs);
        self.dirty = true;
        self.stale_view = true;
    }

    fn follow_from(&mut self, target: Target, from: u64) {
        if let Some((name, known)) = following(target, from)
            && let std::collections::btree_map::Entry::Vacant(place) = self.index.known.entry(name.clone())
        {
            place.insert(known.clone());

            if let (Some(column), Some(key)) = (column_named(&known.column), from_hex(&known.key)) {
                add_to_watch(&mut self.watch, column, key, name);
            }
        }
    }

    /// Writes the index out, whatever it was changed by, or sets aside why it could not be.
    fn save(&mut self, keeper: &Keeper, time: f64) {
        if let Some(scanner) = &self.scanner {
            self.index.place = Some(scanner.place().clone());
        }

        let sealed = serde_json::to_vec(&self.index).map_err(|why| why.to_string()).and_then(|plain| seal(&self.key, &plain));

        match sealed {
            Ok(sealed) => keeper.save(&self.name, sealed),
            Err(why) => log::warn!("could not keep the index: {why}"),
        }

        self.saved_at = time;
        self.dirty = false;
    }
}

impl Reading {
    /// A reading whose indexes are kept by this keeper. Whether they last says whether this keeper's
    /// shelf is more than memory, for the page to say.
    pub fn new(keeper: Keeper, lasting: bool) -> Reading {
        Reading { keeper, lasting, opening: None, open: None, said: Vec::new(), most_txs: MOST_TXS }
    }

    /// Whether an index is open: only while its wallet is unlocked.
    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// Whether what it reads is kept for next time.
    pub fn lasts(&self) -> bool {
        self.lasting
    }

    /// Keeps at most this many transactions from now on, the newest: fewer than it holds lets the
    /// oldest go at once, and what was cut is said, as when there are too many.
    pub fn keep_most_txs(&mut self, most: usize) {
        if self.most_txs == most {
            return;
        }

        self.most_txs = most;

        if let Some(open) = &mut self.open {
            open.most_txs = most;
            let held = open.index.txs.len();
            open.index.trim(most);

            if open.index.txs.len() < held {
                open.dirty = true;
                open.stale_view = true;
            }
        }
    }

    /// How many transactions an index holds, and the most it keeps.
    pub fn txs_kept(&self) -> (usize, usize) {
        (self.open.as_ref().map_or(0, |open| open.index.txs.len()), self.most_txs)
    }

    /// Whether it needs telling what height to start from: a restored wallet, whose history could
    /// reach back any way.
    pub fn needs_floor(&self) -> bool {
        self.open.as_ref().is_some_and(|open| open.scanner.is_none() && open.floor.is_none())
    }

    /// Starts from this height, as someone picked.
    pub fn start_from(&mut self, floor: u64) {
        if let Some(open) = &mut self.open {
            open.floor = Some(floor);
        }
    }

    /// Goes further back, down to this height: what was read stands. An account added since the
    /// reading got past its history is read again from there too.
    pub fn further_back(&mut self, followed: &impl Followed, floor: u64) -> bool {
        let Some(open) = &mut self.open else {
            return false;
        };
        let Some(scanner) = &mut open.scanner else {
            open.floor = Some(floor);

            return true;
        };
        let reach_for_all = open.index.since.values().copied().max().unwrap_or(0) > scanner.place().floor;
        let moved = match reach_for_all {
            true => scanner.again_from(followed, floor),
            false => scanner.further_back(floor),
        };

        if moved {
            for since in open.index.since.values_mut() {
                *since = (*since).min(floor);
            }

            open.dirty = true;
        }

        moved
    }

    /// Asks for direct proofs of everything followed, as someone pressed Check now for: the node sees
    /// each account.
    pub fn check_now(&mut self) {
        if let Some(open) = &mut self.open {
            open.index.omitted.clear();
            open.checking = open.index.known.keys().cloned().collect();
        }
    }

    /// How many values Check now still has out.
    pub fn checks_left(&self) -> usize {
        self.open.as_ref().map_or(0, |open| open.checking.len())
    }

    /// Tries again whatever stopped the reading: what Retry and Reload do.
    pub fn retry(&mut self) {
        if let Some(scanner) = self.open.as_mut().and_then(|open| open.scanner.as_mut()) {
            scanner.retry();
        }
    }

    /// Forgets the index of the wallet open, and starts again from asking where from.
    pub fn forget(&mut self, chain: &mut Chain, checked: &mut HashMap<String, Checked>) {
        let Some(open) = self.open.take() else {
            return;
        };

        self.keeper.forget(&open.name, Box::new(|result| {
            if let Err(why) = result {
                log::warn!("could not forget an index: {why}");
            }
        }));
        clear_view(chain, checked);
    }

    /// Forgets every index of a wallet removed.
    pub fn forget_wallet(&self, wallet: &str) {
        self.keeper.forget(&format!("{wallet}-"), Box::new(|result| {
            if let Err(why) = result {
                log::warn!("could not forget a wallet's index: {why}");
            }
        }));
    }

    /// Writes out and lets go of what is open, and everything shown of it, as the wallet locks or
    /// the setting goes off.
    pub fn close(&mut self, chain: &mut Chain, checked: &mut HashMap<String, Checked>, time: f64) {
        self.opening = None;

        if let Some(mut open) = self.open.take() {
            open.save(&self.keeper, time);
            clear_view(chain, checked);
        }
    }

    /// Writes out what is open now, as the app saves: no harm if it is written again in a moment.
    pub fn save(&mut self, time: f64) {
        let keeper = self.keeper.clone();

        if let Some(open) = self.open.as_mut().filter(|open| open.dirty) {
            open.save(&keeper, time);
        }
    }

    /// Why the index could not be written, if it could not.
    pub fn keeping_trouble(&self) -> Option<String> {
        self.keeper.failed()
    }

    /// What to ask now, for this wallet on this chain: it opens the index first, on a shelf that may
    /// take a moment to answer, and starts once it knows from what height.
    pub fn go(&mut self, wanted: Wanted, followed: &impl Followed, time: f64) -> Go {
        let name = index_name(wanted.wallet, wanted.genesis);
        let keeper = self.keeper.clone();

        // Another wallet's or another chain's: put away, by whoever changed it, as this one opens.
        if self.open.as_ref().is_some_and(|open| open.name != name)
            && let Some(mut open) = self.open.take()
        {
            open.save(&keeper, time);
        }

        if self.open.is_none() {
            self.opened_by_now(&name, &wanted);
        }

        let Some(open) = &mut self.open else {
            return Go::default();
        };

        // A change of password: from here on it is sealed with the new one's key.
        if open.vault_key != wanted.vault_key {
            match index_key(&wanted.vault_key) {
                Ok(key) => {
                    open.key = key;
                    open.vault_key = wanted.vault_key.clone();
                    open.dirty = true;
                    open.saved_at = 0.0;
                }
                Err(why) => log::warn!("{why}"),
            }
        }

        open.floor = open.floor.or(wanted.born);

        if open.scanner.is_none() {
            let at = followed.final_height();
            let Some(place) = open.floor.and_then(|floor| Place::starting(followed, at, floor)) else {
                return Go::default();
            };

            open.index.place = Some(place.clone());
            open.scanner = Some(Scanner::new(place));
            open.dirty = true;
        }

        open.keep_addresses(&wanted.addresses);

        let proofs = open.proofs_due(followed, time);
        let range = open.scanner.as_mut().and_then(|scanner| scanner.next(followed, time));

        if open.dirty && time - open.saved_at >= SAVE_EVERY {
            open.save(&keeper, time);
        }

        Go { proofs, range }
    }

    /// Asks a shelf for the index, once, and takes what it says when it does.
    fn opened_by_now(&mut self, name: &str, wanted: &Wanted) {
        if self.opening.as_ref().is_none_or(|opening| opening.name != name) {
            let inbox = Opening::default();
            let answered = inbox.clone();

            self.opening = Some(Opened { name: name.to_string(), vault_key: wanted.vault_key.clone(), genesis: wanted.genesis.to_string(), inbox });
            self.keeper.read(name, Box::new(move |result| *answered.lock().unwrap_or_else(PoisonError::into_inner) = Some(result)));
        }

        let answer = self.opening.as_ref().and_then(|opening| opening.inbox.lock().unwrap_or_else(PoisonError::into_inner).take());
        let Some(answer) = answer else {
            return;
        };
        let opening = self.opening.take().expect("it answered");
        let key = match index_key(&opening.vault_key) {
            Ok(key) => key,
            Err(why) => {
                self.said.push(why);

                return;
            }
        };
        let index = answer.and_then(|bytes| {
            bytes
                .map(|bytes| {
                    let plain = unseal(&key, &bytes)?;

                    serde_json::from_slice::<Index>(&plain).map_err(|_| "an index that does not read".to_string())
                })
                .transpose()
        });
        let index = match index {
            Ok(index) => index.unwrap_or_default(),
            // A rescan costs nothing but its bytes, and what would not read stays for anyone to look
            // into, written over by nothing.
            Err(why) => {
                self.keeper.set_aside(&opening.name, Box::new(|result| {
                    if let Err(why) = result {
                        log::warn!("could not set an index aside: {why}");
                    }
                }));
                self.said.push(format!("What this wallet read of the chain could not be opened ({why}), so it was set aside and is being read again. The wallet itself is untouched."));

                Index::default()
            }
        };

        self.open = Some(Open::new(opening.name, opening.vault_key, key, opening.genesis, index, None, self.most_txs));
    }

    /// Takes what came of blocks asked for.
    pub fn came(&mut self, asked: Asked, came: Came, followed: &impl Followed, time: f64) {
        let Some(open) = &mut self.open else {
            return;
        };
        let Some(mut scanner) = open.scanner.take() else {
            return;
        };
        let hash = followed.hash();
        let (ours, watch, tentative, genesis) = (&open.ours, &open.watch, &mut open.tentative, &open.genesis);
        let going_down = scanner.going_down();
        let read = scanner.came(asked, came, followed, time, |item| picked_from(item, ours, watch, tentative, genesis, hash));

        let settled = !scanner.going_down();
        open.scanner = Some(scanner);

        for read in read {
            match read {
                Read::Picked(picked) => open.take(picked, going_down),
                // A hole that will never be read is a gap, whatever it changed.
                Read::Unread { from, to, hole } => {
                    if hole {
                        open.index.state_gap = Some(open.index.state_gap.map_or(to, |gap| gap.max(to)));
                    }

                    open.index.note_unread(from, to);
                    open.dirty = true;
                    open.stale_view = true;
                }
            }
        }

        // What was picked up on the way is either followed by now, or never stood.
        if settled {
            open.tentative.clear();
        }
    }

    /// Takes what a node answered of a proof asked for. Returns whether it was one.
    pub fn proof(&mut self, ask: &Ask, result: &Result<Value, Failure>, followed: &impl Followed, time: f64) -> bool {
        let Ask::ScanProof { column, key, block } = ask else {
            return false;
        };
        let Some(open) = &mut self.open else {
            return true;
        };
        let name = format!("{column}/{key}");
        let Some(proving) = open.proving.get(&name).filter(|proving| proving.block == *block).cloned() else {
            return true;
        };
        // Not there, as a block the node has let go of the state of, or no answer: asked again, as
        // what is out of date stays so.
        let Ok(value) = result else {
            return true;
        };

        match proven_value(column, key, block, value, proving.height, followed) {
            Ok(held) => open.proved(&proving, held, time),
            Err(why) => self.said.push(format!("This node's proof does not check ({why}), so it was not taken. What it was for stays unshown.")),
        }

        if let Some(open) = &mut self.open {
            open.proving.remove(&name);
        }

        true
    }

    /// Shows what is known: only what can be shown as it stands. A value out of date is not shown at
    /// all, as a value that cannot be proven is turned off, not labelled.
    pub fn show(&mut self, hash: HashType, chain: &mut Chain, checked: &mut HashMap<String, Checked>) {
        let Some(open) = &mut self.open else {
            return;
        };

        for id in std::mem::take(&mut open.fresh) {
            if let Some(tx) = open.index.txs.get(&id) {
                checked.insert(id, Checked { came: Origin::Read, ..tx.clone() });
            }
        }

        let behind = open.scanner.as_ref().is_some_and(|scanner| scanner.place().catching_up());

        if !std::mem::take(&mut open.stale_view) && behind == open.shown_behind {
            return;
        }

        open.shown_behind = behind;
        chain.accounts.clear();
        chain.balances.clear();
        chain.tokens.clear();
        chain.records.clear();
        chain.listed.clear();
        // Nothing here asks a node's index for anything, so nothing says it has none.
        chain.indexes_addresses = Some(false);
        chain.indexes_events = Some(false);

        for known in open.index.known.values() {
            if behind || open.index.stale(known) {
                continue;
            }

            let Some(at) = &known.at else {
                continue;
            };
            let Some(block) = from_hex(&at.block).and_then(|bytes| Hash::from_bytes(hash, &bytes)) else {
                continue;
            };
            let assurance = Assurance { block, height: at.height, final_: true };
            let bytes = known.value.as_deref().and_then(from_hex);

            match &known.target {
                Target::Account(address) => {
                    let account = match &bytes {
                        Some(bytes) => wincode::deserialize::<Account>(bytes).ok(),
                        None => Some(Account::default()),
                    };

                    if let Some(value) = account {
                        chain.accounts.insert(address.clone(), vec![Proven { value, assurance }]);
                    }
                }
                Target::Balance { token, holder } => {
                    let balance = bytes.as_deref().map_or(Some(u256::ZERO), token_balance);

                    if let Some(value) = balance {
                        chain.balances.insert((token.clone(), holder.clone()), vec![Proven { value, assurance }]);
                    }
                }
                Target::Token(token) => {
                    if let Some(value) = bytes.as_deref().and_then(|bytes| wincode::deserialize::<Token>(bytes).ok()) {
                        chain.tokens.insert(token.clone(), Proven { value, assurance });
                        chain.listed.push(token.clone());
                    }
                }
                Target::Name(name) => {
                    let record = bytes.as_deref().and_then(|bytes| wincode::deserialize::<NameRecord>(bytes).ok());

                    chain.records.insert(name.clone(), Proven { value: record, assurance });
                }
            }
        }

        chain.listed.sort();
    }

    /// Where the reading stands: None while nothing is open.
    pub fn standing(&self) -> Option<Standing> {
        let open = self.open.as_ref()?;
        let scanner = open.scanner.as_ref()?;
        let place = scanner.place();

        Some(Standing {
            high: place.high,
            known_from: place.known_from(),
            floor: place.floor,
            left_below: place.below.iter().map(|stretch| stretch.top - stretch.bottom + 1).sum(),
            catching_up: place.catching_up(),
            downloaded: place.downloaded,
            read: place.read,
            proofs: open.index.proofs,
            checks: open.index.checks,
            waiting: scanner.waiting().map(str::to_string),
            trouble: scanner.trouble().map(Trouble::sentence),
            state_gap: open.index.state_gap,
            unread: open.index.unread.iter().map(|(from, to)| to - from + 1).sum(),
            held_from: place.held_from,
            cut_below: open.index.cut_below,
            omitted: open.index.omitted.clone(),
            stale: open.index.known.values().filter(|known| open.index.stale(known)).count(),
        })
    }

    /// What to offer for this transaction, as checked blocks show: see `FeesSeen::suggest`. With
    /// nothing open, nothing was seen.
    pub fn suggest(&self, tx: &Tx, base: u256, tip: u256) -> (chainv4::structs::tx::FeeOffer, bool) {
        self.open.as_ref().map_or_else(FeesSeen::default, |open| open.index.fees.clone()).suggest(tx, base, tip)
    }
}

impl Open {
    /// Takes in a value proven directly. Lower than what the blocks read have shown, it says nothing
    /// new. Where the reading has been and a check found otherwise, the node left it off a block's
    /// list, which is the one way that comes to light.
    fn proved(&mut self, proving: &Proving, held: Option<Vec<u8>>, time: f64) {
        let hex = held.as_deref().map(to_hex);
        let reached = self.reached();
        let Some(known) = self.index.known.get_mut(&proving.known) else {
            return;
        };

        if proving.checking {
            self.checking.remove(&proving.known);
            self.index.checks += 1;

            if proving.height <= reached && known.at.is_some() && known.value != hex && known.at.as_ref().is_some_and(|at| at.height <= proving.height) {
                self.index.omitted.push(what_it_is(&known.target));
            }
        } else {
            self.index.proofs += 1;
        }

        if known.at.as_ref().is_none_or(|at| at.height <= proving.height) {
            known.value = hex;
            known.at = Some(At { height: proving.height, block: proving.block.clone() });
            known.watched_from = known.watched_from.min(proving.height);
        }

        self.proven_at.insert(proving.known.clone(), time);
        self.dirty = true;
        self.stale_view = true;
    }
}

/// What a value is, for a person.
fn what_it_is(target: &Target) -> String {
    match target {
        Target::Account(address) => format!("the account {address}"),
        Target::Balance { token, holder } => format!("the balance of {token} at {holder}"),
        Target::Token(token) => format!("the token {token}"),
        Target::Name(name) => format!("the name {name}"),
    }
}

/// Takes down everything shown of a wallet's index.
fn clear_view(chain: &mut Chain, checked: &mut HashMap<String, Checked>) {
    chain.accounts.clear();
    chain.balances.clear();
    chain.tokens.clear();
    chain.records.clear();
    chain.listed.clear();
    checked.retain(|_, tx| tx.came == Origin::Proven);
}

#[cfg(test)]
pub(crate) mod tests;
