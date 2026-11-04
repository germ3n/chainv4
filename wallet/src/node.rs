//! Talking to a node's HTTP API, in the background on the desktop and in the browser alike, and
//! through the proxy set, if one is, and never around it. Answers wait in an inbox the app empties
//! each frame, and each wakes the UI. The node is never trusted: everything it sends that the
//! wallet shows as proven, the light client checked.

use {
    chainv4_ui::asks::Asks,
    chainv4_ui::blocks::{Asked, Came, RangeLine},
    chainv4_ui::net::{Answer, Client},
    chainv4_ui::sse::{self, News},
    serde_json::Value,
    std::collections::{HashMap, HashSet, VecDeque},
    std::sync::atomic::{AtomicBool, Ordering},
    std::sync::{Arc, Mutex},
    web_time::Instant,
};

pub use chainv4_ui::net::{Failure, NEEDS_ASKING, NEEDS_ASKING_STATUS};

/// How long after the stream is lost it is opened again, in seconds.
const STREAM_RETRY: f64 = 10.0;
/// How often a page that goes on asking for what Read from checked blocks keeps in is told again, in
/// seconds.
const TOLD_AGAIN: f64 = 3.0;

/// How the node's stream stands, kept as it arrives. No events pile up here: they only say that the
/// chain moved, and a phone in the background may not read them for hours.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Listening {
    pub open: bool,
    /// Whether the chain moved since this was last read.
    pub moved: bool,
    /// When it last said anything, in the node's seconds.
    heard: f64,
    /// When it was lost, in the node's seconds.
    lost_at: Option<f64>,
    /// The node does not stream at all, so asking again is no use.
    unsupported: bool,
}

impl Listening {
    /// Takes in what the stream said, and says whether it is worth waking the app for.
    fn hear(&mut self, news: News, now: f64) -> bool {
        match news {
            News::Open => {
                (self.open, self.heard) = (true, now);
                true
            }
            News::Alive => {
                self.heard = now;
                false
            }
            News::Event { name, .. } => {
                self.heard = now;
                self.moved |= sse::chain_moved(&name);
                sse::chain_moved(&name)
            }
            News::Closed { unsupported, .. } => {
                (self.open, self.unsupported, self.lost_at) = (false, unsupported, Some(now));
                true
            }
        }
    }

    /// Whether to open the stream now: for the first time, past a wait since it was lost, or at
    /// once if it fell silent. Not while one is on its way.
    fn due(&mut self, opened: bool, now: f64) -> bool {
        let silent = self.open && now - self.heard >= sse::SILENT;
        let waited = self.lost_at.is_some_and(|lost| now - lost >= STREAM_RETRY);
        let due = !self.unsupported && (silent || !self.open && (!opened || waited));

        if due {
            (self.open, self.lost_at) = (false, None);
        }

        due
    }

    /// How it stands, once: whether the chain moved is said only the once.
    fn said(&mut self) -> Listening {
        let said = *self;
        self.moved = false;

        said
    }
}

/// Something to ask the node, which also names what came back.
/// Proofs it has out at a time. A wallet's accounts, each with an address in every scheme, times every
/// listed token, come to hundreds of them at once, and nginx lets an address have 64 requests under
/// way, this app's and any other's on the same connection together.
const PROOFS_AT_ONCE: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Ask {
    /// The coin's symbol and decimals, for showing amounts: nothing proven rests on it.
    Status,
    /// What the light client starts from.
    Light,
    Headers { from: u64, count: u32 },
    /// Headers below the lowest the light client holds, for it to prove by their hash chain.
    Ancestors { from: u64, count: u32 },
    /// Finality from an epoch on, many epochs to an answer.
    Finality { from: u64, count: u64 },
    /// What the light client wants next, all at once: headers from a height and finality from an
    /// epoch.
    Bundle { headers: u64, epoch: u64 },
    /// How far the node has finalized, by GET /status: what says whether a client fast-resuming
    /// from this node is far enough behind to.
    Finalized,
    /// What the light client starts from at a checkpoint: the one named, or the node's finalized
    /// one, for a node met for the first time.
    Checkpoint(Option<String>),
    /// A chain-state entry with its proof, at a block.
    Proof { column: &'static str, key: String, block: String },
    /// A proof a wallet reading checked blocks asks for all the same: an account's or token's value
    /// where reading started or past a gap, or what someone pressed Check now for. It tells the node
    /// which, so nothing else asks for it: see crate::reading.
    ScanProof { column: &'static str, key: String, block: String },
    /// A transaction and its receipt, each with its Merkle path.
    Receipt(String),
    /// The same of a transaction this wallet sent, while it reads checked blocks: the one lookup left
    /// to it, shown as included but not final until the reading reaches it. The node has that
    /// transaction already, as the wallet sent it there.
    SentReceipt(String),
    /// A page of the token registry, the first or the one after a cursor, to know which tokens to
    /// prove: each page's cursor is followed to the end.
    Tokens(Option<String>),
    /// The transactions touching an address, newest first, from the node's address index, the newest
    /// page or the older one after a cursor: where the history looks.
    History { address: String, cursor: Option<String> },
    /// The transactions touching an address that pass a filter, as the filter's query says, from
    /// the newest or from where a cursor says: where the history looks once filters are on. An
    /// older node ignores the filter, and lists everything.
    Filtered { address: String, query: String, cursor: Option<String> },
    /// Events that mention an address, newest first, the newest page or the older one after a
    /// cursor: where it looks on a node with no address index but an event index, and beside one.
    Events { address: String, cursor: Option<String> },
    /// The chain's names: their suffix, prices and grace period, as the node says. Nothing proven
    /// rests on them but whether a name is in its grace period.
    Names,
    /// A page of the names the node says an address owns, the first or the one after a cursor:
    /// where the Names page looks, each one proven, every page followed to the end.
    OwnedNames { address: String, cursor: Option<String> },
    /// A contract's record, as the node says it: its engine, and a native contract's functions.
    Contract(String),
    /// A native contract's function run read-only, with the body of POST /call.
    Call(String),
    /// An EVM contract read: a call's probe dry-run at POST /offer, for what it returns.
    Read(String),
    /// A fee offer for a signed probe.
    Offer(String),
    /// A signed transaction.
    Submit(String),
    /// A request to the node's faucet, with its proof of work: the body of POST /faucet.
    Faucet(String),
    /// A contract's events of one name, newest first, from the node's event index: what the Trade
    /// page draws a pool's price from.
    #[cfg_attr(not(feature = "dex"), allow(dead_code))]
    Named { contract: String, name: String },
    /// A JSON-RPC request to POST /rpc, as its body: what a site's transaction is priced and sent
    /// by, as Ethereum's tools do.
    Rpc(String),
    /// A multisig's members, nonce and open proposals, as the node says: what the Multisig page
    /// shows and counts approvals by. Nothing it signs rests on it: a bundle says what it does.
    Multisig(String),
    /// The multisigs an address's key is a member of, from the node's member index, a page at a
    /// time, the first or the one after a cursor: what the Multisig page lists each of this
    /// wallet's multisigs by, following each page's cursor to the end. Nothing signed rests on it:
    /// each is asked of again, and a bundle says what it does.
    Memberships { address: String, cursor: Option<String> },
    /// What a mining pool owes an address, with the Merkle path a claim takes, as the pool's node
    /// says: what the Mining page claims with. Nothing rests on it but the fee: the chain checks
    /// the path against the root the pool's block committed to.
    PoolClaim(String),
    /// A mining pool's standing for an address: its work in the window, its part of the next
    /// block, its workers and the blocks it shared in.
    Pool(String),
    /// The node's mining port, as GET /mining says it: where it is reached, the protocol it speaks,
    /// and whether it takes a pool's workers, workers mining alone, and anyone. What the Mining
    /// page mines by; the node checks every nonce it is sent, so nothing rests on it.
    #[cfg_attr(not(feature = "mining"), allow(dead_code))]
    Mining,
    /// What an address has claimed from a pool, and the root the pool's block at a height
    /// committed to, as the wallet's own node says: what a claim kept on this device is checked
    /// against before it is sent, the pool's node no matter.
    Claims { pool: String, to: String, height: u64 },
    /// A page of the collection registry, the first or the one after a cursor, for the NFTs page to
    /// look through, every page followed to the end.
    #[cfg_attr(not(feature = "nft"), allow(dead_code))]
    Collections(Option<String>),
    /// Governance as the node says it: its kind, the version an approval names, what a change
    /// needs, the open proposals with their approvals, and every setting as it stands. What the
    /// Governance page shows; nothing signed rests on it but the version, which the chain checks.
    Governance,
    /// A page of governance's decisions, newest first: the newest, or those below a version.
    GovernanceHistory { before: Option<u64> },
    /// What an address's approval weighs, and the change it counts for, as the node says.
    GovernanceWeight(String),
    /// The newest release the node took, as signed, beside where the node's own chainv4 stands:
    /// what the wallet checks its own version against, with the keys built in, as the node only
    /// relays it.
    Release,
}

/// A cursor as a page's query goes on with it, for the page after the one it ended.
fn after(cursor: &Option<String>) -> String {
    cursor.as_ref().map_or_else(String::new, |cursor| format!("&cursor={cursor}"))
}

impl Ask {
    fn path(&self) -> String {
        match self {
            Ask::Status | Ask::Finalized => "/status".to_string(),
            Ask::Light => "/light".to_string(),
            Ask::Headers { from, count } | Ask::Ancestors { from, count } => format!("/headers?from={from}&count={count}"),
            Ask::Finality { from, count } => format!("/light/finality?from={from}&count={count}"),
            Ask::Bundle { headers, epoch } => format!("/light/bundle?headers={headers}&epoch={epoch}"),
            Ask::Checkpoint(Some(block)) => format!("/light/checkpoint/{block}"),
            Ask::Checkpoint(None) => "/light/checkpoint".to_string(),
            Ask::Proof { column, key, block } | Ask::ScanProof { column, key, block } => format!("/proofs/{column}/{key}?block={block}"),
            Ask::Receipt(id) | Ask::SentReceipt(id) => format!("/proofs/receipts/{id}"),
            Ask::Tokens(cursor) => format!("/tokens?limit=500{}", after(cursor)),
            Ask::Collections(cursor) => format!("/collections?limit=500{}", after(cursor)),
            Ask::History { address, cursor } => format!("/accounts/{address}/txs?limit=50{}", after(cursor)),
            Ask::Filtered { address, query, cursor } => {
                let cursor = cursor.as_ref().map_or_else(String::new, |cursor| format!("&cursor={cursor}"));

                format!("/accounts/{address}/txs?limit=50&{query}{cursor}")
            }
            Ask::Events { address, cursor } => format!("/events?address={address}&limit=50{}", after(cursor)),
            Ask::Named { contract, name } => format!("/events?contract={contract}&name={name}&limit=200"),
            Ask::Names => "/names".to_string(),
            Ask::OwnedNames { address, cursor } => format!("/accounts/{address}/names?limit=200{}", after(cursor)),
            Ask::Contract(address) => format!("/contracts/{address}"),
            Ask::Multisig(address) => format!("/multisig/{address}"),
            Ask::Memberships { address, cursor } => {
                let cursor = cursor.as_ref().map_or_else(String::new, |cursor| format!("&cursor={cursor}"));

                format!("/accounts/{address}/multisigs?limit=100{cursor}")
            }
            Ask::PoolClaim(address) => format!("/pool/claim/{address}"),
            Ask::Pool(address) => format!("/pool/{address}"),
            Ask::Mining => "/mining".to_string(),
            Ask::Claims { pool, to, height } => format!("/claims/{pool}/{to}?height={height}"),
            Ask::Release => "/release".to_string(),
            Ask::Governance => "/governance".to_string(),
            Ask::GovernanceHistory { before: Some(before) } => format!("/governance/history?before={before}&limit=20"),
            Ask::GovernanceHistory { before: None } => "/governance/history?limit=20".to_string(),
            Ask::GovernanceWeight(address) => format!("/governance/weight/{address}"),
            Ask::Call(_) => "/call".to_string(),
            Ask::Read(_) => "/offer".to_string(),
            Ask::Offer(_) => "/offer".to_string(),
            Ask::Submit(_) => "/txs".to_string(),
            Ask::Rpc(_) => "/rpc".to_string(),
            Ask::Faucet(_) => "/faucet".to_string(),
        }
    }

    fn body(&self) -> Option<&str> {
        match self {
            Ask::Offer(body) | Ask::Submit(body) | Ask::Call(body) | Ask::Read(body) | Ask::Rpc(body) | Ask::Faucet(body) => Some(body),
            _ => None,
        }
    }

    /// Whether a reload of the page that asked this asks it again: what a page reads, not what the
    /// light client follows and proves by, which it asks again itself, nor anything sent.
    pub fn reloads(&self) -> bool {
        self.body().is_none()
            && !matches!(
                self,
                Ask::Status | Ask::Light | Ask::Headers { .. } | Ask::Ancestors { .. } | Ask::Finality { .. } | Ask::Bundle { .. } | Ask::Finalized | Ask::Checkpoint(_) | Ask::Proof { .. } | Ask::ScanProof { .. } | Ask::Receipt(_) | Ask::SentReceipt(_) | Ask::Release
            )
    }

    /// Whether it takes a place among the proofs out at once: see PROOFS_AT_ONCE.
    fn is_a_proof(&self) -> bool {
        matches!(self, Ask::Proof { .. } | Ask::ScanProof { .. })
    }

    /// Whether this asks the node for what Read from checked blocks does without. Named one by one,
    /// with no catch-all, so a new request has to be decided here before it builds: what stays is
    /// what follows the chain, the few proofs that setting says it takes, sending, and a release,
    /// which is checked against the keys built in.
    pub fn needs_asking(&self) -> bool {
        match self {
            Ask::Light | Ask::Headers { .. } | Ask::Ancestors { .. } | Ask::Finality { .. } | Ask::Bundle { .. } | Ask::Checkpoint(_) | Ask::ScanProof { .. } | Ask::SentReceipt(_) | Ask::Submit(_) | Ask::Release => false,
            Ask::Status
            | Ask::Finalized
            | Ask::Proof { .. }
            | Ask::Receipt(_)
            | Ask::Tokens(_)
            | Ask::History { .. }
            | Ask::Filtered { .. }
            | Ask::Events { .. }
            | Ask::Names
            | Ask::OwnedNames { .. }
            | Ask::Contract(_)
            | Ask::Call(_)
            | Ask::Read(_)
            | Ask::Offer(_)
            | Ask::Faucet(_)
            | Ask::Named { .. }
            | Ask::Rpc(_)
            | Ask::Multisig(_)
            | Ask::Memberships { .. }
            | Ask::PoolClaim(_)
            | Ask::Pool(_)
            | Ask::Mining
            | Ask::Claims { .. }
            | Ask::Collections(_)
            | Ask::Governance
            | Ask::GovernanceHistory { .. }
            | Ask::GovernanceWeight(_) => true,
        }
    }
}

/// Where answers wait for the app, each with what it answers.
type Inbox = Arc<Mutex<Vec<(Ask, Result<Value, Failure>)>>>;

/// What was fetched from elsewhere than the node, by URL: its bytes, or why there are none.
type Fetched = Arc<Mutex<Vec<(String, Result<Vec<u8>, String>)>>>;

pub struct Node {
    base: String,
    /// Proofs out, and those waiting for room: see PROOFS_AT_ONCE.
    proofs_out: usize,
    proofs_held: VecDeque<Ask>,
    /// What every request goes out through, or why none can: a proxy set that does not build
    /// sends nothing rather than go direct.
    client: Result<Client, String>,
    inbox: Inbox,
    /// What is out, what failed, and when to ask it again.
    asks: Asks<Ask>,
    /// What the seconds these go by are counted from.
    began: Instant,
    wake: egui::Context,
    fetched: Fetched,
    fetching: HashSet<String>,
    /// The node's stream: how it stands, and the flag that ends it.
    listening: Arc<Mutex<Listening>>,
    hang_up: Option<Arc<AtomicBool>>,
    /// Whether what needs asking is kept in, with Read from checked blocks on: here, at the only way
    /// out, so that a page or code path that forgot about it still sends nothing.
    reading: bool,
    /// What was kept in, and when the page was last told.
    told: HashMap<Ask, f64>,
    /// The way a reading's requests for blocks go.
    line: RangeLine,
}

impl Node {
    pub fn new(base: &str, client: Result<Client, String>, wake: egui::Context) -> Node {
        let woken = wake.clone();

        Node {
            line: RangeLine::new(base, client.clone(), Arc::new(move || woken.request_repaint())),
            reading: false,
            told: HashMap::new(),
            base: base.trim().trim_end_matches('/').to_string(),
            proofs_out: 0,
            proofs_held: VecDeque::new(),
            client,
            inbox: Arc::default(),
            asks: Asks::default(),
            began: Instant::now(),
            wake,
            fetched: Arc::default(),
            fetching: HashSet::new(),
            listening: Arc::default(),
            hang_up: None,
        }
    }

    /// A node that keeps in what needs asking, with Read from checked blocks on, or lets it out.
    pub fn reading_blocks(mut self, on: bool) -> Node {
        self.reading = on;

        self
    }

    /// Asks for blocks: what a reading goes by. What comes of it is in `ranges`.
    pub fn read_range(&self, asked: Asked) {
        self.line.ask(asked);
    }

    /// What came of the blocks asked for, since last time.
    pub fn ranges(&self) -> Vec<(Asked, Came)> {
        self.line.came()
    }

    /// Fetches a URL that is not the node's, as an NFT's metadata or picture, through the same
    /// proxy, unless it is out already. At most `most` bytes are kept: more fails.
    #[cfg_attr(not(feature = "nft"), allow(dead_code))]
    pub fn fetch(&mut self, url: &str, most: usize) {
        if !self.fetching.insert(url.to_string()) {
            return;
        }

        // What a page fetches from elsewhere tells whoever serves it what is looked at.
        if self.reading {
            self.fetched.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push((url.to_string(), Err(NEEDS_ASKING.to_string())));

            return;
        }

        let (fetched, wake, key) = (self.fetched.clone(), self.wake.clone(), url.to_string());
        let done = move |answer: Result<Answer, String>| {
            let result = match answer {
                Ok(answer) if !answer.ok => Err(format!("{} {}", answer.status, answer.status_text)),
                Ok(answer) if answer.bytes.len() > most => Err(format!("more than {} MB", most / (1024 * 1024))),
                Ok(answer) => Ok(answer.bytes),
                Err(message) => Err(message),
            };

            fetched.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push((key, result));
            wake.request_repaint();
        };

        match &self.client {
            Ok(client) => client.fetch(url.to_string(), None, done),
            Err(why) => done(Err(format!("the proxy set does not work: {why}"))),
        }
    }

    /// Everything fetched from elsewhere since last time.
    #[cfg_attr(not(feature = "nft"), allow(dead_code))]
    pub fn fetched(&mut self) -> Vec<(String, Result<Vec<u8>, String>)> {
        let fetched = std::mem::take(&mut *self.fetched.lock().unwrap_or_else(std::sync::PoisonError::into_inner));

        for (url, _) in &fetched {
            self.fetching.remove(url);
        }

        fetched
    }

    /// The seconds since this node was connected to.
    fn now(&self) -> f64 {
        self.began.elapsed().as_secs_f64()
    }

    /// Asks, unless the same question is out already, or failed a moment ago and has not been held
    /// back long enough. Something sent, rather than read, goes whenever it is pressed for.
    pub fn ask(&mut self, ask: Ask) {
        let now = self.now();

        if self.kept_in(&ask, now) {
            return;
        }

        let sending = match ask.body() {
            Some(_) => self.asks.force(&ask),
            None => self.asks.begin(&ask, now),
        };

        if sending {
            self.send(ask);
        }
    }

    /// Asks again now, whatever failed before, and lets go of why it did: what Retry and Reload do.
    pub fn retry(&mut self, ask: Ask) {
        if self.kept_in(&ask, self.now()) {
            return;
        }

        self.asks.forget(&ask);

        if self.asks.force(&ask) {
            self.send(ask);
        }
    }

    /// Whether this is kept in, with Read from checked blocks on: it goes no further, and whoever
    /// asked is told why, as it would be of anything else that came to nothing.
    fn kept_in(&mut self, ask: &Ask, now: f64) -> bool {
        if !self.reading || !ask.needs_asking() {
            return false;
        }

        if self.told.get(ask).is_none_or(|told| now - told >= TOLD_AGAIN) {
            self.told.insert(ask.clone(), now);
            self.inbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push((ask.clone(), Err(Failure { status: NEEDS_ASKING_STATUS, message: NEEDS_ASKING.to_string() })));
            self.wake.request_repaint();
        }

        true
    }

    /// Why the node does not answer, while it does not: what became of the status asked of it every
    /// few seconds, rather than of something one page asked. With no status asked, as with Read from
    /// checked blocks on, what became of what follows the chain.
    pub fn down(&self) -> Option<&Failure> {
        if self.reading {
            return self.asks.troubles(self.now()).into_iter().map(|(_, failure)| failure).next();
        }

        self.asks.failure(&Ask::Status).filter(|failure| failure.is_trouble())
    }

    /// What went wrong on the way to the node, for what is still wanted, for a page to say: the
    /// failure that has gone on longest, and everything that failed so, to ask again.
    pub fn trouble(&self) -> Option<(Failure, Vec<Ask>)> {
        // What was sent says for itself on its own page, and is never sent again unasked.
        let troubles: Vec<_> = self.asks.troubles(self.now()).into_iter().filter(|(ask, _)| ask.body().is_none()).collect();
        let failure = troubles.first()?.1.clone();

        Some((failure, troubles.into_iter().map(|(ask, _)| ask.clone()).collect()))
    }

    /// Whether this is out.
    pub fn asking(&self, ask: &Ask) -> bool {
        self.asks.asking(ask)
    }

    /// What went out since last time.
    pub fn sent(&mut self) -> Vec<Ask> {
        self.asks.take_sent()
    }

    fn send(&mut self, ask: Ask) {
        if ask.is_a_proof() {
            if self.proofs_out >= PROOFS_AT_ONCE {
                self.proofs_held.push_back(ask);

                return;
            }

            self.proofs_out += 1;
        }

        self.send_now(ask);
    }

    /// Whether proofs wait for room to go out.
    pub fn proofs_waiting(&self) -> bool {
        !self.proofs_held.is_empty()
    }

    fn send_now(&mut self, ask: Ask) {
        debug_assert!(!(self.reading && ask.needs_asking()), "kept in before it got here");
        let url = format!("{}{}", self.base, ask.path());
        let body = ask.body().map(|body| body.as_bytes().to_vec());
        let (inbox, wake) = (self.inbox.clone(), self.wake.clone());
        let done = move |answer: Result<Answer, String>| {
            let result = match answer {
                Ok(answer) => read(&answer),
                Err(message) => Err(Failure { status: 0, message }),
            };

            inbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push((ask, result));
            wake.request_repaint();
        };

        match &self.client {
            Ok(client) => client.fetch(url, body, done),
            Err(why) => done(Err(format!("the proxy set does not work: {why}"))),
        }
    }

    /// Everything that arrived since last time.
    /// Keeps the node's stream open while it serves one, and says how it stands: whether it is
    /// open, and whether the chain moved since it was last asked.
    pub fn listen(&mut self) -> Listening {
        let now = self.now();
        let (said, due) = {
            let mut listening = self.listening.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let due = listening.due(self.hang_up.is_some(), now);

            (listening.said(), due)
        };

        if due {
            if let Some(hang_up) = self.hang_up.take() {
                hang_up.store(true, Ordering::Relaxed);
            }

            let (shared, wake, began) = (self.listening.clone(), self.wake.clone(), self.began);
            self.hang_up = Some(sse::listen(&self.client, format!("{}/stream", self.base), move |news| {
                let now = began.elapsed().as_secs_f64();

                if shared.lock().unwrap_or_else(std::sync::PoisonError::into_inner).hear(news, now) {
                    wake.request_repaint();
                }
            }));
        }

        said
    }

    pub fn answers(&mut self) -> Vec<(Ask, Result<Value, Failure>)> {
        let answers = std::mem::take(&mut *self.inbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        let now = self.now();

        for (ask, result) in &answers {
            self.asks.ended(ask, result.as_ref().err(), now);

            if ask.is_a_proof() {
                self.proofs_out = self.proofs_out.saturating_sub(1);
            }
        }

        while self.proofs_out < PROOFS_AT_ONCE
            && let Some(ask) = self.proofs_held.pop_front()
        {
            self.proofs_out += 1;
            self.send_now(ask);
        }

        answers
    }
}

impl Drop for Node {
    fn drop(&mut self) {
        if let Some(hang_up) = self.hang_up.take() {
            hang_up.store(true, Ordering::Relaxed);
        }
    }
}

/// An answer's JSON, or the API's {"error": …} as a failure.
fn read(response: &Answer) -> Result<Value, Failure> {
    let value: Option<Value> = serde_json::from_slice(&response.bytes).ok();

    match (response.ok, value) {
        (true, Some(value)) => Ok(value),
        (_, value) => Err(Failure {
            status: response.status,
            message: value
                .as_ref()
                .and_then(|value| value.get("error"))
                .and_then(Value::as_str)
                .map_or_else(|| format!("{} {}", response.status, response.status_text), str::to_string),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(name: &str) -> News {
        News::Event { name: name.to_string(), data: Value::Null }
    }

    #[test]
    fn the_stream_opens_once_and_again_only_past_a_wait_when_lost() {
        let mut listening = Listening::default();
        assert!(listening.due(false, 0.0), "the first time");
        assert!(!listening.due(true, 5.0), "on its way");

        let closed = || News::Closed { why: "gone".to_string(), unsupported: false };
        assert!(listening.hear(closed(), 5.0));
        assert!(!listening.due(true, 5.0 + STREAM_RETRY - 1.0));
        assert!(listening.due(true, 5.0 + STREAM_RETRY));
        assert!(!listening.due(true, 5.0 + STREAM_RETRY), "and it is on its way again");

        assert!(listening.hear(News::Open, 20.0));
        assert!(listening.open);
        assert!(!listening.due(true, 21.0));
    }

    #[test]
    fn a_node_with_no_stream_is_not_asked_again_but_a_silent_stream_is_reopened_at_once() {
        let mut listening = Listening::default();
        listening.hear(News::Closed { why: "404".to_string(), unsupported: true }, 0.0);
        assert!(!listening.open);
        assert!(!listening.due(true, 1e9));

        let mut listening = Listening::default();
        listening.hear(News::Open, 100.0);
        assert!(!listening.hear(News::Alive, 100.0 + sse::SILENT - 1.0), "a keep-alive wakes nobody");
        assert!(!listening.due(true, 100.0 + sse::SILENT), "as it has just said something");
        assert!(listening.due(true, 100.0 + 2.0 * sse::SILENT - 1.0), "gone quiet, with no more ever to come");
        assert!(!listening.open, "so nothing relies on it meanwhile");
    }

    #[test]
    fn only_news_of_the_chain_says_it_moved_and_that_only_once() {
        let mut listening = Listening::default();

        for name in ["queued", "peer_up", "peer_down"] {
            assert!(!listening.hear(event(name), 0.0), "{name}");
        }

        assert!(!listening.said().moved);

        for name in ["block", "reorganized", "justified", "finalized", "lagged"] {
            assert!(listening.hear(event(name), 0.0), "{name}");
            assert!(listening.said().moved, "{name}");
            assert!(!listening.said().moved, "{name}, once");
        }
    }

    #[test]
    fn with_no_way_out_the_stream_is_lost_rather_than_waited_on() {
        let mut node = unreachable();
        assert!(!node.listen().open);
        assert!(node.hang_up.is_some());
        assert!(node.listening.lock().unwrap().lost_at.is_some());
    }

    #[test]
    fn the_nodes_stream_reaches_the_wallet_and_ends_with_it() {
        use std::io::{Read, Write};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let mut node = Node::new(&format!("http://{}", listener.local_addr().unwrap()), Ok(Client::direct()), egui::Context::default());
        node.listen();

        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0; 1024];
        let read = stream.read(&mut request).unwrap();
        assert!(request[..read].starts_with(b"GET /stream "));
        stream.write_all(b"HTTP/1.1 200 OK\r\n\r\nevent: block\ndata: {}\n\n").unwrap();

        let deadline = Instant::now() + std::time::Duration::from_secs(10);
        while !node.listening.lock().unwrap().moved {
            assert!(Instant::now() < deadline, "never heard");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        let said = node.listen();
        assert!(said.open && said.moved);
        assert!(!node.listen().moved);

        let hang_up = node.hang_up.clone().unwrap();
        drop(node);
        assert!(hang_up.load(Ordering::Relaxed), "a wallet that left this node stops listening to it");
    }

    #[test]
    fn with_a_proxy_that_does_not_work_nothing_is_sent_and_the_answer_says_why() {
        let mut node = Node::new("http://127.0.0.1:1", Err("not a port: 0".to_string()), egui::Context::default());
        node.ask(Ask::Status);
        let answers = node.answers();

        assert_eq!(answers, vec![(Ask::Status, Err(Failure {
            status: 0,
            message: "the proxy set does not work: not a port: 0".to_string(),
        }))]);
        assert!(!node.asking(&Ask::Status), "answered, so asked again later");
    }

    /// A node with no way out, whose every request fails at once and says so.
    fn unreachable() -> Node {
        Node::new("http://127.0.0.1:1", Err("not a port: 0".to_string()), egui::Context::default())
    }

    #[test]
    fn what_failed_is_held_back_for_a_moment_and_retry_clears_it_and_asks_at_once() {
        let mut node = unreachable();

        node.ask(Ask::Tokens(None));
        assert_eq!(node.answers().len(), 1);

        let (failure, failed) = node.trouble().expect("the way to the node is in trouble");
        assert!(failure.sentence().starts_with("Could not reach the node"));
        assert_eq!(failed, [Ask::Tokens(None)]);

        // The page goes on asking, as pages do each frame: nothing goes out for now.
        node.ask(Ask::Tokens(None));
        assert_eq!(node.answers().len(), 0, "held back");
        assert!(!node.asking(&Ask::Tokens(None)));
        assert_eq!(node.sent(), [Ask::Tokens(None)], "the first ask alone went out");

        // Retry goes past the hold, and clears what failed until it comes back.
        node.retry(Ask::Tokens(None));
        assert_eq!(node.sent(), [Ask::Tokens(None)]);
        assert_eq!(node.answers().len(), 1);
        assert!(node.trouble().is_some(), "and failed again");
    }

    #[test]
    fn something_sent_goes_whenever_it_is_pressed_for_failed_or_not() {
        let mut node = unreachable();
        let submit = Ask::Submit("{}".to_string());

        node.ask(submit.clone());
        assert_eq!(node.answers().len(), 1);
        node.ask(submit);
        assert_eq!(node.answers().len(), 1, "no hold on what someone sends");
        assert!(node.trouble().is_none(), "nor is it what a page offers to ask again");
    }

    fn proof(key: usize) -> Ask {
        Ask::Proof { column: "accounts", key: format!("{key}"), block: "0x01".to_string() }
    }

    #[test]
    fn only_so_many_proofs_go_out_at_once_and_each_answer_lets_the_next_go() {
        let mut node = unreachable();
        let many = PROOFS_AT_ONCE * 3 + 1;

        for key in 0..many {
            node.ask(proof(key));
        }

        assert!(node.proofs_waiting());

        let mut told = 0;

        for _ in 0..3 {
            let answers = node.answers();
            assert_eq!(answers.len(), PROOFS_AT_ONCE, "no more than that at a time");
            told += answers.len();
        }

        let answers = node.answers();
        assert_eq!(told + answers.len(), many);
        assert!(!node.proofs_waiting(), "and every one went in the end");
    }

    #[test]
    fn a_proof_waiting_counts_as_asked_and_is_not_queued_twice() {
        let mut node = unreachable();

        for key in 0..PROOFS_AT_ONCE + 1 {
            node.ask(proof(key));
        }

        node.ask(proof(PROOFS_AT_ONCE));
        assert!(node.asking(&proof(PROOFS_AT_ONCE)));
        assert_eq!(node.proofs_held.len(), 1);
    }

    #[test]
    fn what_is_not_a_proof_never_waits_behind_them() {
        let mut node = unreachable();

        for key in 0..PROOFS_AT_ONCE * 2 {
            node.ask(proof(key));
        }

        node.ask(Ask::Status);

        assert_eq!(node.answers().len(), PROOFS_AT_ONCE + 1);
    }

    #[test]
    fn a_reload_asks_again_what_a_page_reads_and_leaves_the_light_client_its_own() {
        assert!(Ask::Governance.reloads() && Ask::Pool("a".to_string()).reloads());
        assert!(Ask::Filtered { address: "a".to_string(), query: "kind=transfer".to_string(), cursor: None }.reloads());
        assert!(!Ask::Status.reloads(), "always asked again by itself");
        assert!(!Ask::Headers { from: 0, count: 1 }.reloads());
        assert!(!Ask::Receipt("0x01".to_string()).reloads());
        assert!(!Ask::Submit("{}".to_string()).reloads(), "nothing sent is sent again");
    }

    /// Every request there is, one of each.
    fn every_ask() -> Vec<Ask> {
        let text = || "a".to_string();

        vec![
            Ask::Status,
            Ask::Light,
            Ask::Headers { from: 0, count: 1 },
            Ask::Ancestors { from: 0, count: 1 },
            Ask::Finality { from: 0, count: 1 },
            Ask::Bundle { headers: 0, epoch: 0 },
            Ask::Finalized,
            Ask::Checkpoint(None),
            Ask::Proof { column: "accounts", key: "0x00".to_string(), block: "0x01".to_string() },
            Ask::ScanProof { column: "accounts", key: "0x00".to_string(), block: "0x01".to_string() },
            Ask::Receipt(text()),
            Ask::SentReceipt(text()),
            Ask::Tokens(None),
            Ask::History { address: text(), cursor: None },
            Ask::Filtered { address: text(), query: text(), cursor: None },
            Ask::Events { address: text(), cursor: None },
            Ask::Names,
            Ask::OwnedNames { address: text(), cursor: None },
            Ask::Contract(text()),
            Ask::Call(text()),
            Ask::Read(text()),
            Ask::Offer(text()),
            Ask::Submit(text()),
            Ask::Faucet(text()),
            Ask::Named { contract: text(), name: text() },
            Ask::Rpc(text()),
            Ask::Multisig(text()),
            Ask::Memberships { address: text(), cursor: None },
            Ask::PoolClaim(text()),
            Ask::Pool(text()),
            Ask::Mining,
            Ask::Claims { pool: text(), to: text(), height: 0 },
            Ask::Collections(None),
            Ask::Governance,
            Ask::GovernanceHistory { before: None },
            Ask::GovernanceWeight(text()),
            Ask::Release,
        ]
    }

    /// What reaches a node from asking each of these, by the first line of each request a listener
    /// takes, each once.
    fn reached(reading: bool, asks: &[Ask]) -> Vec<String> {
        use std::io::{Read, Write};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let mut node = Node::new(&format!("http://{}", listener.local_addr().unwrap()), Ok(Client::direct()), egui::Context::default()).reading_blocks(reading);

        for ask in asks {
            node.ask(ask.clone());
        }

        let mut lines = Vec::new();
        let deadline = Instant::now() + std::time::Duration::from_secs(2);

        // Long enough for all that went out to arrive: anything sent connects within this.
        while Instant::now() < deadline {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream.set_nonblocking(false).unwrap();
                    let mut request = [0; 2048];
                    let read = stream.read(&mut request).unwrap_or(0);
                    let text = String::from_utf8_lossy(&request[..read]).into_owned();
                    lines.push(text.lines().next().unwrap_or_default().split(' ').take(2).collect::<Vec<_>>().join(" "));
                    let _ = stream.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\n{}");
                }
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(5)),
            }
        }

        lines.sort();
        lines.dedup();

        lines
    }

    #[test]
    fn with_reading_on_only_what_it_says_it_takes_ever_reaches_the_node() {
        let lines = reached(true, &every_ask());

        assert_eq!(lines, [
            "GET /headers?from=0&count=1",
            "GET /light",
            "GET /light/bundle?headers=0&epoch=0",
            "GET /light/checkpoint",
            "GET /light/finality?from=0&count=1",
            "GET /proofs/accounts/0x00?block=0x01",
            "GET /proofs/receipts/a",
            "GET /release",
            "POST /txs",
        ]);
    }

    #[test]
    fn with_reading_off_every_request_goes_out_as_it_always_did() {
        let asks = every_ask();
        let lines = reached(false, &asks);
        let mut paths: Vec<String> = asks.iter().map(|ask| format!("{} {}", if ask.body().is_some() { "POST" } else { "GET" }, ask.path())).collect();
        paths.sort();
        paths.dedup();

        assert_eq!(lines, paths);
    }

    #[test]
    fn what_is_kept_in_is_answered_at_once_saying_why_and_is_no_trouble_on_the_way() {
        let mut node = Node::new("http://127.0.0.1:1", Ok(Client::direct()), egui::Context::default()).reading_blocks(true);

        for ask in every_ask().into_iter().filter(Ask::needs_asking) {
            node.ask(ask.clone());
            node.ask(ask.clone());
            node.retry(ask.clone());

            let answers = node.answers();
            assert_eq!(answers.len(), 1, "{ask:?}, told once however often it is asked");
            assert_eq!(answers[0].1, Err(Failure { status: NEEDS_ASKING_STATUS, message: NEEDS_ASKING.to_string() }));
            assert!(!node.asking(&ask), "{ask:?} is not out");
        }

        assert_eq!(node.sent(), Vec::<Ask>::new(), "nothing went out");
        assert!(node.trouble().is_none(), "and no page offers to ask it again");
        assert!(node.down().is_none());

        node.fetch("https://elsewhere/picture.png", 1024);
        assert_eq!(node.fetched(), [("https://elsewhere/picture.png".to_string(), Err(NEEDS_ASKING.to_string()))], "nor anything fetched from elsewhere");
    }

    #[test]
    fn a_filter_goes_on_the_listing_s_path() {
        let ask = |cursor: Option<&str>| Ask::Filtered { address: "ed25519:0x01".to_string(), query: "kind=transfer&direction=in".to_string(), cursor: cursor.map(str::to_string) };

        assert_eq!(ask(None).path(), "/accounts/ed25519:0x01/txs?limit=50&kind=transfer&direction=in");
        assert_eq!(ask(Some("0xff")).path(), "/accounts/ed25519:0x01/txs?limit=50&kind=transfer&direction=in&cursor=0xff");
        assert_eq!(Ask::Finalized.path(), "/status");
    }
}
