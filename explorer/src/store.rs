//! What the explorer knows of the chain, put together from everything the node answered and
//! streamed. The main chain is kept as block ids by height; any block that once was on it and no
//! longer is, or never was, is a side block.

use {
    crate::api::{Ask, EventQuery, Failure},
    chainv4_ui::filters::Answered,
    crate::model::{
        Account, BlockDetail, Checkpoint, Contract, Decision, Forks, FoundEvent, FoundTx, GovernanceView, GovernanceWeight, IndexPage,
        ListedName, Mempool, MultisigView, NamesPage, NodeView, Page, Peer, PeerChange, Placed, Receipt, RegisteredCollection, RegisteredToken,
        Reorganized, RichList, Status, Summary, SupplyView, TokensPage, TxView,
    },
    serde_json::Value,
    std::collections::{BTreeMap, HashMap, VecDeque},
};

/// The most main-chain blocks kept; older ones are let go.
const MAX_MAIN: usize = 20_000;
/// The most stream events kept for the live feed.
const MAX_FEED: usize = 200;
/// The most waiting transactions kept when the stream adds them.
const MAX_WAITING: usize = 1_000;
/// How many node reports' hashrates the mining chart keeps: ten minutes of them, at one every two
/// seconds.
const MAX_HASHRATES: usize = 300;
/// How many blocks a page asks for at most, as the API allows.
pub const PAGE: u64 = 100;

/// The registered tokens a search found so far, by symbol.
#[derive(Clone, Debug, Default)]
pub struct TokenList {
    pub tokens: Vec<RegisteredToken>,
    /// Where the next page starts, or None when there is none.
    pub next: Option<String>,
    /// How many pages were read.
    pub pages: usize,
    /// When the first page last came, in milliseconds since the Unix epoch.
    pub refreshed: u64,
}

impl TokenList {
    /// Takes a page: the first, which stands for the list while that is all that was read, or a
    /// later one, which goes after what is held.
    pub fn take(&mut self, page: TokensPage, later: bool, now: u64) {
        if later {
            let held: Vec<String> = self.tokens.iter().map(|token| token.address.clone()).collect();
            self.tokens.extend(page.tokens.into_iter().filter(|token| !held.contains(&token.address)));
            self.next = page.next;
            self.pages += 1;

            return;
        }

        self.refreshed = now;

        if self.pages <= 1 {
            self.tokens = page.tokens;
            self.next = page.next;
            self.pages = 1;
        }
    }
}

/// The name directory read so far, sorted by name, a page at a time.
#[derive(Clone, Debug, Default)]
pub struct NameList {
    pub names: Vec<ListedName>,
    /// The name to read the next page after, or None at the end.
    pub next: Option<String>,
    /// How many pages were read; 0 before the first.
    pub pages: usize,
}

impl NameList {
    /// Takes a page: the first, which stands for the list while that is all that was read, or a
    /// later one, which goes after what is held, any name already held left out.
    pub fn take(&mut self, page: NamesPage, later: bool) {
        if later {
            let last = self.names.last().map(|name| name.name.clone());
            self.names.extend(page.names.into_iter().filter(|name| last.as_ref().is_none_or(|last| name.name > *last)));
            self.next = page.next;
            self.pages += 1;

            return;
        }

        if self.pages <= 1 {
            self.names = page.names;
            self.next = page.next;
            self.pages = 1;
        }
    }
}

/// What one of a node's indexes listed so far, newest first: the events a query found, or the
/// transactions that touched an address.
#[derive(Clone, Debug)]
pub struct Listing<T> {
    pub items: Vec<T>,
    /// Where the next older page starts, or None when there is none.
    pub next: Option<String>,
    /// The lowest height the node's index holds.
    pub indexed_from: u64,
    /// The height the node is still moving that to, if it is.
    pub catching_up_to: Option<u64>,
    /// Whether the node's event index knows EVM logs' topics, as an older node's does not.
    pub topics: bool,
    /// When the newest page last came, in milliseconds since the Unix epoch.
    pub refreshed: u64,
    /// What the node said of the page read last, under filters.
    pub answered: Answered,
    /// How many pages were read.
    pages: u32,
}

pub type EventList = Listing<FoundEvent>;

impl<T> Default for Listing<T> {
    fn default() -> Listing<T> {
        Listing {
            items: Vec::new(),
            next: None,
            indexed_from: 0,
            catching_up_to: None,
            topics: false,
            refreshed: 0,
            answered: Answered::default(),
            pages: 0,
        }
    }
}

impl<T: Placed> Listing<T> {
    /// Takes a page: the newest, which puts any entries newer than those held in front, or an
    /// older one, which goes after them. A list read to its end starts over from the newest page
    /// once the node has indexed further back, so the older entries can be paged to.
    pub fn take(&mut self, page: IndexPage<T>, older: bool, now: u64) {
        let further_back = !older && self.next.is_none() && page.indexed_from < self.indexed_from;
        self.indexed_from = page.indexed_from;
        self.catching_up_to = page.catching_up_to;
        let answered = Answered { filtered: page.applied.is_some(), scanned: page.scanned, limited: page.limited };

        // The page that said how far the listing has got: the older one that went on, or the
        // newest while that is all that was read.
        if older || self.answered == Answered::default() || self.pages <= 1 {
            self.answered = answered;
        }

        self.pages += u32::from(older || self.pages == 0);
        self.topics = page.topics;

        if older {
            let last = self.items.last().map(Placed::place);
            self.items.extend(page.items.into_iter().filter(|item| last.is_none_or(|last| item.place() < last)));
            self.next = page.next;

            return;
        }

        self.refreshed = now;

        match self.items.first().map(Placed::place) {
            _ if further_back => {
                self.items = page.items;
                self.next = page.next;
            }
            None => {
                self.items = page.items;
                self.next = page.next;
            }
            Some(newest) => {
                let newer: Vec<T> = page.items.into_iter().filter(|item| item.place() > newest).collect();
                self.items.splice(0..0, newer);
            }
        }
    }
}

/// Governance's decisions read so far, by version, and how far down the pages read reach.
pub type Decisions = chainv4_ui::ballots::Decided<Decision>;

/// Governance's rejections read so far, by the chain's count of them, and how far down the pages
/// read reach.
pub type Rejections = chainv4_ui::ballots::Decided<crate::model::Rejection>;

impl chainv4_ui::ballots::Counted for crate::model::Rejection {
    fn version(&self) -> u64 {
        self.rejection
    }
}

impl chainv4_ui::ballots::Counted for Decision {
    fn version(&self) -> u64 {
        self.version
    }
}

/// One line of the live feed.
#[derive(Clone, Debug)]
pub struct Live {
    /// When it arrived, in milliseconds since the Unix epoch.
    pub at: u64,
    pub name: String,
    pub text: String,
}

#[derive(Default)]
pub struct Store {
    pub status: Option<Status>,
    pub node: Option<NodeView>,
    pub mempool: Option<Mempool>,
    pub tips: Vec<String>,
    /// Every block seen, on the main chain or not.
    pub blocks: HashMap<String, Summary>,
    /// The main chain as far as it is loaded: block ids by height.
    pub main: BTreeMap<u64, String>,
    pub details: HashMap<String, BlockDetail>,
    pub receipts: HashMap<String, Receipt>,
    pub accounts: HashMap<String, Account>,
    /// Contracts by address; None for an address that holds none.
    pub contracts: HashMap<String, Option<Contract>>,
    /// Registered tokens met so far, by address, from the registry or their contract's page.
    pub tokens: HashMap<String, RegisteredToken>,
    /// Pages of the token registry, by the symbol searched for.
    pub token_lists: HashMap<String, TokenList>,
    /// The collection registry, and when it came, in milliseconds since the Unix epoch.
    pub collections: Option<(Vec<RegisteredCollection>, u64)>,
    /// Token balances by token and holder, in whole units.
    pub holdings: HashMap<(String, String), String>,
    pub validators: Option<Value>,
    pub signers: Option<Value>,
    pub evidence: Option<Value>,
    /// GET /anchors: anchoring's settings and the newest anchors.
    pub anchors: Option<Value>,
    pub feed: VecDeque<Live>,
    /// The node's total hashrate each time it reported, oldest first, with when: what the mining
    /// chart draws.
    pub hashrates: VecDeque<(u64, f64)>,
    /// Why the last asking of something failed, until it succeeds.
    pub failures: HashMap<Ask, Failure>,
    /// What read-only calls answered.
    pub calls: HashMap<Ask, Value>,
    /// Indexed events, by what was asked.
    pub events: HashMap<EventQuery, EventList>,
    /// The transactions the address index found touching an address, by the address and the
    /// filter's query, "" for none.
    pub account_txs: HashMap<(String, String), Listing<FoundTx>>,
    /// Names looked up, by what was searched for.
    pub names: HashMap<String, crate::model::NameView>,
    /// The names each address owns.
    pub owned_names: HashMap<String, Vec<crate::model::OwnedName>>,
    /// The multisigs each address's key is a member of, as the node's member index says, by
    /// address; none for a node that keeps no such index.
    pub memberships: HashMap<String, Vec<crate::model::Membership>>,
    /// The mining pool the node runs, as it last answered; None before then, and for a node that
    /// runs none.
    pub pool: Option<crate::model::PoolView>,
    /// The pool's books for one miner each, by its payout address.
    pub pool_miners: HashMap<String, crate::model::PoolView>,
    /// The node's release, as it last answered; None before then, and for an older node, which
    /// has none to give.
    pub release: Option<crate::model::NodeRelease>,
    /// Governance, as the node last answered; None before then, and for a chain without it.
    pub governance: Option<GovernanceView>,
    /// Governance's decisions read so far.
    pub decisions: Decisions,
    /// Governance's rejections read so far.
    pub rejections: Rejections,
    /// Each voter's votes, newest first, by its address, from its first page of vote_change
    /// transactions: what the Governance page's turnout is counted from.
    pub votes_cast: HashMap<String, Vec<FoundTx>>,
    /// What accounts' votes weigh, by the address looked up.
    pub governance_weights: HashMap<String, GovernanceWeight>,
    /// Multisigs looked up, by address.
    pub multisigs: HashMap<String, MultisigView>,
    /// The name directory, as far as it was read.
    pub name_list: NameList,
    /// The richest accounts, as the node last reported them.
    pub rich: Option<RichList>,
    /// The coin's supply, as the node last reported it.
    pub supply: Option<SupplyView>,
}

impl Store {
    /// The highest main-chain block loaded.
    pub fn top(&self) -> Option<u64> {
        self.main.keys().next_back().copied()
    }

    /// The lowest main-chain block loaded.
    pub fn bottom(&self) -> Option<u64> {
        self.main.keys().next().copied()
    }

    pub fn on_main(&self, summary: &Summary) -> bool {
        self.main.get(&summary.height) == Some(&summary.id)
    }

    /// The finalized and justified heights, 0 before anything is.
    pub fn finality_heights(&self) -> (u64, u64) {
        let height = |checkpoint: &Checkpoint| {
            checkpoint
                .height
                .or_else(|| self.blocks.get(&checkpoint.block).map(|block| block.height))
                .unwrap_or(0)
        };

        match self.status.as_ref().and_then(|status| status.finality.as_ref()) {
            Some(finality) => (height(&finality.finalized), height(&finality.justified)),
            None => (0, 0),
        }
    }

    /// What to ask so the loaded main chain reaches the tip the status names, if it does not.
    pub fn missing(&self) -> Option<Ask> {
        let status = self.status.as_ref()?;

        if self.main.get(&status.height) == Some(&status.tip) {
            return None;
        }

        let behind = self.top().map_or(PAGE, |top| status.height.saturating_sub(top) + 4);

        Some(Ask::Blocks {
            before: None,
            limit: behind.clamp(1, PAGE),
        })
    }

    /// What to ask for the page of main-chain blocks below the lowest loaded.
    pub fn older(&self) -> Option<Ask> {
        let bottom = self.bottom()?;

        (bottom > 0).then_some(Ask::Blocks {
            before: Some(bottom),
            limit: PAGE,
        })
    }

    /// Takes a page of main-chain blocks, newest first. Returns where to ask from next when the
    /// page did not reach blocks already loaded and agreeing, so the main chain has no hole and
    /// a deep reorg is followed all the way down.
    pub fn page(&mut self, page: Page) -> Option<u64> {
        let lowest = page.blocks.last()?.clone();
        let agrees = self.main.get(&lowest.height) == Some(&lowest.id);
        let nothing_below = self.main.range(..lowest.height).next().is_none();

        for block in page.blocks {
            self.main_block(block);
        }

        self.prune();

        (!agrees && !nothing_below && lowest.height > 0).then_some(lowest.height)
    }

    /// A block on the main chain at its height, putting whatever was there before off it.
    pub fn main_block(&mut self, mut block: Summary) {
        block.main = Some(true);

        if let Some(old) = self.main.insert(block.height, block.id.clone())
            && old != block.id
            && let Some(old) = self.blocks.get_mut(&old)
        {
            old.main = Some(false);
        }

        self.blocks.insert(block.id.clone(), block);
    }

    /// A block off the main chain.
    pub fn side_block(&mut self, mut block: Summary) {
        if self.on_main(&block) {
            return;
        }

        block.main = Some(false);
        self.blocks.insert(block.id.clone(), block);
    }

    pub fn reorganized(&mut self, reorg: Reorganized) {
        for id in &reorg.undone {
            if let Some(block) = self.blocks.get_mut(id) {
                block.main = Some(false);

                if self.main.get(&block.height) == Some(id) {
                    self.main.remove(&block.height);
                }
            }
        }

        for block in reorg.applied {
            self.main_block(block);
        }
    }

    pub fn forks(&mut self, forks: Forks) {
        for block in forks.blocks {
            self.side_block(block);
        }

        self.tips = forks.tips;
    }

    pub fn detail(&mut self, detail: BlockDetail) {
        let summary = detail.summary.clone();

        match summary.main {
            Some(true) => self.main_block(summary),
            Some(false) => self.side_block(summary),
            None => {
                self.blocks.entry(summary.id.clone()).or_insert(summary);
            }
        }

        self.details.insert(detail.summary.id.clone(), detail);
    }

    /// The block page for a height or an id, if loaded.
    pub fn detail_for(&self, block: &str) -> Option<&BlockDetail> {
        match block.parse::<u64>() {
            Ok(height) => self.main.get(&height).and_then(|id| self.details.get(id)),
            Err(_) => self.details.get(block),
        }
    }

    /// Where a transaction is: the block page holding it, by its receipt or by looking through
    /// the pages loaded.
    pub fn tx(&self, id: &str) -> Option<(&BlockDetail, &TxView)> {
        let in_block = |detail: &'_ BlockDetail| detail.txs.iter().any(|tx| tx.id == id);
        let detail = match self.receipts.get(id) {
            Some(receipt) => self.details.get(&receipt.block),
            None => self.details.values().find(|detail| in_block(detail)),
        }?;

        detail.txs.iter().find(|tx| tx.id == id).map(|tx| (detail, tx))
    }

    /// A transaction still waiting in the mempool, as last listed.
    pub fn waiting(&self, id: &str) -> Option<&TxView> {
        self.mempool.as_ref()?.txs.iter().find(|tx| tx.id == id)
    }

    /// A checkpoint the stream says was justified or finalized.
    pub fn checkpoint(&mut self, finalized: bool, mut checkpoint: Checkpoint) {
        checkpoint.height = checkpoint
            .height
            .or_else(|| self.blocks.get(&checkpoint.block).map(|block| block.height));

        if let Some(finality) = self.status.as_mut().and_then(|status| status.finality.as_mut()) {
            match finalized {
                true => finality.finalized = checkpoint,
                false => finality.justified = checkpoint,
            }
        }
    }

    /// A transaction the stream says joined the mempool.
    pub fn queued(&mut self, tx: TxView) {
        if let Some(mempool) = &mut self.mempool
            && !mempool.txs.iter().any(|waiting| waiting.id == tx.id)
        {
            mempool.count += 1;

            if mempool.txs.len() < MAX_WAITING {
                mempool.txs.push(tx);
            }
        }

        if let Some(node) = &mut self.node {
            node.mempool.count += 1;
        }
    }

    /// A transaction that left the node's mempool other than by landing: off the list of what
    /// waits, and out of its count.
    pub fn left(&mut self, id: &str) {
        if let Some(mempool) = &mut self.mempool
            && let Some(at) = mempool.txs.iter().position(|waiting| waiting.id == id)
        {
            mempool.txs.remove(at);
            mempool.count = mempool.count.saturating_sub(1);
        }

        if let Some(node) = &mut self.node {
            node.mempool.count = node.mempool.count.saturating_sub(1);
        }
    }

    /// A node report: the latest, and its hashrate kept for the chart.
    pub fn node_report(&mut self, node: NodeView, now: u64) {
        match &node.mining {
            // Two reports in one frame are one sample: the chart needs time between them.
            Some(mining) if self.hashrates.back().is_some_and(|(at, _)| *at == now) => {
                if let Some(last) = self.hashrates.back_mut() {
                    last.1 = mining.rate;
                }
            }
            Some(mining) => {
                if self.hashrates.len() == MAX_HASHRATES {
                    self.hashrates.pop_front();
                }

                self.hashrates.push_back((now, mining.rate));
            }
            None => self.hashrates.clear(),
        }

        self.node = Some(node);
    }

    /// A peer the stream says came or went.
    pub fn peer(&mut self, change: PeerChange, up: bool, now: u64) {
        let Some(node) = &mut self.node else {
            return;
        };

        node.peers.retain(|peer| peer.id != change.id);

        if up {
            node.peers.push(Peer {
                id: change.id,
                address: change.address.unwrap_or_default(),
                since: now,
                heard: Default::default(),
                dropped: 0,
                chain: None,
            });
        }
    }

    pub fn live(&mut self, at: u64, name: &str, text: String) {
        self.feed.push_front(Live {
            at,
            name: name.to_string(),
            text,
        });
        self.feed.truncate(MAX_FEED);
    }

    /// The loaded main chain's last blocks, oldest first.
    pub fn recent(&self, count: usize) -> Vec<&Summary> {
        let mut recent: Vec<&Summary> = self.main.values().rev().take(count).filter_map(|id| self.blocks.get(id)).collect();
        recent.reverse();

        recent
    }

    /// Lets go of the oldest main-chain blocks past MAX_MAIN.
    fn prune(&mut self) {
        while self.main.len() > MAX_MAIN {
            if let Some((_, id)) = self.main.pop_first() {
                self.blocks.remove(&id);
                self.details.remove(&id);
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::model::EventsPage;

    fn mining_node(rate: Option<f64>) -> NodeView {
        serde_json::from_value(serde_json::json!({
            "uptime_ms": 1, "listening": [], "producing": true, "validating": false,
            "mempool": { "count": 0, "capacity": 1 }, "syncing": null, "counters": {}, "peers": [],
            "mining": rate.map(|rate| serde_json::json!({ "rate": rate, "devices": [], "listening": null, "workers": [] })),
        }))
        .unwrap()
    }

    #[test]
    fn the_hashrate_is_kept_for_the_chart_a_sample_a_report() {
        let mut store = Store::default();

        for at in 0..(MAX_HASHRATES as u64 + 5) {
            store.node_report(mining_node(Some(at as f64)), at * 2_000);
        }

        assert_eq!(store.hashrates.len(), MAX_HASHRATES, "the oldest go");
        assert_eq!(store.hashrates.front(), Some(&(5 * 2_000, 5.0)));

        let last = store.hashrates.back().unwrap().0;
        store.node_report(mining_node(Some(1e9)), last);

        assert_eq!((store.hashrates.len(), store.hashrates.back()), (MAX_HASHRATES, Some(&(last, 1e9))), "one frame, one sample");

        store.node_report(mining_node(None), last + 2_000);

        assert!(store.hashrates.is_empty(), "a node that stopped mining starts the chart over");
        assert!(store.node.is_some());
    }

    pub fn block(height: u64, id: &str, parent: &str) -> Summary {
        Summary {
            id: id.to_string(),
            height,
            parent: parent.to_string(),
            timestamp: height * 2_000,
            producer: "ed25519:0x01".to_string(),
            difficulty: "0".to_string(),
            tx_root: "0x00".to_string(),
            receipts_root: "0x00".to_string(),
            state_root: "0x00".to_string(),
            gas_used: 0,
            base_price: "0".to_string(),
            reward: "0".to_string(),
            tx_count: 0,
            proof_count: 0,
            pruned: false,
            main: None,
            pool_root: "0x00".to_string(),
            pool: None,
        }
    }

    fn page(blocks: &[(u64, &str, &str)]) -> Page {
        Page {
            blocks: blocks.iter().map(|&(height, id, parent)| block(height, id, parent)).collect(),
        }
    }

    #[test]
    fn pages_follow_the_tip_and_reorgs_all_the_way_down() {
        let mut store = Store::default();

        assert_eq!(store.page(page(&[(3, "c", "b"), (2, "b", "a"), (1, "a", "g")])), None);
        assert_eq!(store.top(), Some(3));

        // The tip moved on by two, and the page overlaps where it agrees.
        assert_eq!(store.page(page(&[(5, "e", "d"), (4, "d", "c"), (3, "c", "b")])), None);

        // A reorg deeper than the page: the lowest block disagrees, so ask below it.
        assert_eq!(store.page(page(&[(5, "e2", "d2"), (4, "d2", "c2"), (3, "c2", "b")])), Some(3));
        assert_eq!(store.blocks["c"].main, Some(false));
        assert_eq!(store.page(page(&[(2, "b", "a")])), None);
        assert_eq!(store.main.values().cloned().collect::<Vec<_>>(), ["a", "b", "c2", "d2", "e2"]);

        // A gap: the page does not reach what is loaded.
        assert_eq!(store.page(page(&[(9, "i", "h"), (8, "h", "g2")])), Some(8));
    }

    #[test]
    fn what_is_missing_follows_the_status() {
        let mut store = Store::default();
        store.page(page(&[(3, "c", "b")]));
        store.status = Some(serde_json::from_value(serde_json::json!({
            "genesis": "g", "tip": "f", "height": 6, "timestamp": 0, "difficulty": "0", "work": "6",
            "hash": "blake3", "coin": { "symbol": "DEV", "decimals": 18 }, "finality": null, "fees": null
        })).unwrap());

        assert_eq!(store.missing(), Some(Ask::Blocks { before: None, limit: 7 }));

        store.main_block(block(6, "f", "e"));

        assert_eq!(store.missing(), None);
        assert_eq!(store.older(), Some(Ask::Blocks { before: Some(3), limit: PAGE }));
    }

    #[test]
    fn reorgs_and_side_blocks_from_the_stream() {
        let mut store = Store::default();
        store.page(page(&[(2, "b", "a"), (1, "a", "g")]));
        store.side_block(block(2, "x", "a"));

        assert_eq!(store.blocks["x"].main, Some(false));

        // A side block that is on the main chain stays there.
        store.side_block(block(2, "b", "a"));

        assert_eq!(store.blocks["b"].main, Some(true));

        store.reorganized(Reorganized {
            undone: vec!["b".to_string()],
            applied: vec![block(2, "x", "a"), block(3, "y", "x")],
        });

        assert_eq!(store.main.values().cloned().collect::<Vec<_>>(), ["a", "x", "y"]);
        assert_eq!(store.blocks["b"].main, Some(false));
        assert_eq!(store.recent(2).iter().map(|block| block.id.as_str()).collect::<Vec<_>>(), ["x", "y"]);
    }

    fn found(height: u64, index: u32) -> FoundEvent {
        FoundEvent {
            contract: "contract:0x01".to_string(),
            name: "Ping".to_string(),
            data: Vec::new(),
            tx: format!("0x{height}{index}"),
            block: format!("0x{height}"),
            height,
            index,
            position: 0,
        }
    }

    fn events_page(events: Vec<FoundEvent>, next: Option<&str>) -> EventsPage {
        EventsPage {
            items: events,
            next: next.map(str::to_string),
            indexed_from: 1,
            catching_up_to: None,
            applied: None,
            scanned: 0,
            limited: false,
            topics: false,
        }
    }

    #[test]
    fn a_token_search_refreshes_its_first_page_until_more_are_read() {
        let token = |address: &str, symbol: &str| RegisteredToken {
            address: address.to_string(),
            symbol: Some(symbol.to_string()),
            ..RegisteredToken::default()
        };
        let page = |tokens: Vec<RegisteredToken>, next: Option<&str>| TokensPage {
            tokens,
            next: next.map(str::to_string),
        };
        let addresses = |list: &TokenList| list.tokens.iter().map(|token| token.address.clone()).collect::<Vec<_>>();
        let mut list = TokenList::default();
        list.take(page(vec![token("a", "ABC")], Some("c1")), false, 10);
        list.take(page(vec![token("b", "ABD"), token("c", "XYZ")], Some("c1")), false, 20);

        assert_eq!(addresses(&list), ["b", "c"], "a first page alone is the list");

        list.take(page(vec![token("d", "ZZZ"), token("c", "XYZ")], None), true, 30);
        list.take(page(vec![token("e", "AAA")], Some("c2")), false, 40);

        assert_eq!(addresses(&list), ["b", "c", "d"], "once more were read, the list stays");
        assert_eq!((list.next, list.refreshed), (None, 40));
    }

    #[test]
    fn the_name_directory_refreshes_its_first_page_until_more_are_read() {
        let name = |name: &str| ListedName {
            name: name.to_string(),
            display: name.to_string(),
            status: "active".to_string(),
            owner: None,
            target: None,
            expires: None,
        };
        let page = |names: &[&str], next: Option<&str>| NamesPage {
            names: names.iter().map(|named| name(named)).collect(),
            next: next.map(str::to_string),
        };
        let names = |list: &NameList| list.names.iter().map(|name| name.name.clone()).collect::<Vec<_>>();
        let mut list = NameList::default();
        list.take(page(&["a.dev", "b.dev"], Some("b.dev")), false);
        list.take(page(&["a.dev", "aa.dev"], Some("aa.dev")), false);

        assert_eq!(names(&list), ["a.dev", "aa.dev"], "a first page alone is the list");

        list.take(page(&["aa.dev", "c.dev"], None), true);
        list.take(page(&["0.dev"], Some("0.dev")), false);

        assert_eq!(names(&list), ["a.dev", "aa.dev", "c.dev"], "once more were read, the list stays, each name once");
        assert_eq!((list.next, list.pages), (None, 2));
    }

    #[test]
    fn a_listing_says_what_the_node_said_of_how_far_it_got() {
        let page = |applied: Option<Vec<String>>, limited: bool| EventsPage {
            applied,
            limited,
            scanned: if limited { 1_000 } else { 3 },
            ..events_page(vec![found(5, 0)], Some("c1"))
        };
        let mut list = EventList::default();

        // An older node's answer says nothing of filters: it was not filtered.
        list.take(page(None, false), false, 10);
        assert_eq!(list.answered, Answered { filtered: false, scanned: 3, limited: false });

        let mut list = EventList::default();
        list.take(page(Some(vec!["kind".to_string()]), true), false, 10);
        assert_eq!(list.answered, Answered { filtered: true, scanned: 1_000, limited: true });

        // The newest page again changes nothing once an older one went on from it.
        list.take(EventsPage { limited: false, ..page(Some(vec!["kind".to_string()]), false) }, true, 20);
        assert!(!list.answered.limited);
        list.take(page(Some(vec!["kind".to_string()]), true), false, 30);
        assert!(!list.answered.limited);
    }

    #[test]
    fn event_pages_add_newer_events_in_front_and_older_ones_behind() {
        let mut list = EventList::default();
        list.take(events_page(vec![found(5, 0), found(4, 1), found(4, 0)], Some("c1")), false, 10);
        list.take(events_page(vec![found(3, 0), found(2, 0)], None), true, 20);

        assert_eq!(list.next, None);
        assert_eq!(list.refreshed, 10, "only the newest page refreshes");

        // The newest page again, with two new events: only those are added, in front.
        list.take(events_page(vec![found(7, 0), found(6, 0), found(5, 0), found(4, 1)], Some("c2")), false, 30);

        assert_eq!(list.items.iter().map(|event| event.height).collect::<Vec<_>>(), [7, 6, 5, 4, 4, 3, 2]);
        assert_eq!(list.next, None, "the older pages already read stay read");
        assert_eq!(list.refreshed, 30);

        // Once the node has indexed further back, a list read to its end starts over.
        let further = EventsPage {
            indexed_from: 0,
            ..events_page(vec![found(7, 0), found(6, 0)], Some("c3"))
        };
        list.take(further, false, 40);

        assert_eq!(list.items.iter().map(|event| event.height).collect::<Vec<_>>(), [7, 6]);
        assert_eq!(list.next.as_deref(), Some("c3"));
    }
}
