//! What the explorer knows of the chain, put together from everything the node answered and
//! streamed. The main chain is kept as block ids by height; any block that once was on it and no
//! longer is, or never was, is a side block.

use {
    crate::api::{Ask, Failure},
    crate::model::{
        Account, BlockDetail, Checkpoint, Contract, Forks, Mempool, NodeView, Page, Peer, PeerChange, Receipt, Reorganized,
        Status, Summary, TxView,
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
/// How many blocks a page asks for at most, as the API allows.
pub const PAGE: u64 = 100;

/// A token's own account of itself, from read-only calls.
#[derive(Clone, Debug, Default)]
pub struct Token {
    pub name: Option<String>,
    pub symbol: Option<String>,
    pub decimals: Option<u8>,
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
    pub tokens: HashMap<String, Token>,
    /// Token balances by token and holder, in whole units.
    pub holdings: HashMap<(String, String), String>,
    pub validators: Option<Value>,
    pub signers: Option<Value>,
    pub evidence: Option<Value>,
    pub feed: VecDeque<Live>,
    /// Why the last asking of something failed, until it succeeds.
    pub failures: HashMap<Ask, Failure>,
    /// What read-only calls answered.
    pub calls: HashMap<Ask, Value>,
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

    pub fn block(height: u64, id: &str, parent: &str) -> Summary {
        Summary {
            id: id.to_string(),
            height,
            parent: parent.to_string(),
            timestamp: height * 2_000,
            producer: "ed25519:0x01".to_string(),
            difficulty: "0".to_string(),
            tx_root: "0x00".to_string(),
            gas_used: 0,
            base_price: "0".to_string(),
            tx_count: 0,
            proof_count: 0,
            main: None,
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
}
