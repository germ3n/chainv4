//! Anchoring: the chain's finalized checkpoints written into Bitcoin, and in time Ethereum, so a
//! long-range fork made from old validator keys loses to history Bitcoin's work already holds.
//! These are the settings genesis's anchoring or a Change::Anchoring sets; what the chain does
//! with them is in crate::anchor, and docs/anchoring.md tells the whole story.

use {
    crate::crypto::hashable::Hashable,
    crate::structs::{address::Address, hash::Hash, tx::AnchorProof, u256::{U256Le, u256}},
    r#macro::document,
    wincode::{SchemaRead, SchemaWrite},
};

#[document(
    summary = "Which Bitcoin network a relay follows, as btc_relay's start names it: main checks every rule of Bitcoin's headers, regtest every rule but the retarget, test only that each header meets the difficulty it names. Only main makes an anchor worth more than the votes behind it: the others' headers cost nothing to forge.",
    category = "Anchoring",
    see_also = "BtcStart, AnchorSettings",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, SchemaWrite, SchemaRead)]
#[wincode(tag_encoding = "u8")]
pub enum BtcNetworkRules {
    #[document(summary = "Bitcoin itself: proof of work, the retarget every 2016 blocks and the median time past.")]
    #[wincode(tag = 0)]
    Main,
    #[document(summary = "Regtest: one difficulty throughout.")]
    #[wincode(tag = 1)]
    Regtest,
    #[document(summary = "Testnet4 or signet, checked only for the work each header names: for testing alone.")]
    #[wincode(tag = 2)]
    Test,
}

impl Hashable for BtcNetworkRules {
    const TAG_LEN: usize = 1;
}

impl BtcNetworkRules {
    #[document(parent = "BtcNetworkRules", summary = "Every network, in tag order.")]
    pub const ALL: [BtcNetworkRules; 3] = [BtcNetworkRules::Main, BtcNetworkRules::Regtest, BtcNetworkRules::Test];

    #[document(parent = "BtcNetworkRules", summary = "Its name, as btc_relay's start and the API write it: main, regtest or test.")]
    pub fn name(self) -> &'static str {
        match self {
            BtcNetworkRules::Main => "main",
            BtcNetworkRules::Regtest => "regtest",
            BtcNetworkRules::Test => "test",
        }
    }

    #[document(parent = "BtcNetworkRules", summary = "The network by its name.")]
    pub fn from_name(name: &str) -> Option<BtcNetworkRules> {
        BtcNetworkRules::ALL.into_iter().find(|network| network.name() == name)
    }
}

#[document(
    summary = "Where the Bitcoin headers anchoring trusts begin: the header the relay was started at, its height and its network. A node checking anchors on its own, as anchored sync does, checks every header from it, so it is what the relay was started at and nothing a fork could move without a vote from before the fork.",
    category = "Anchoring",
    see_also = "AnchorSettings, BtcNetworkRules",
)]
#[derive(Clone, Debug, PartialEq, Eq, Hash, SchemaWrite, SchemaRead)]
pub struct BtcStart {
    #[document(summary = "The header, 80 bytes as Bitcoin writes it.")]
    pub header: Vec<u8>,
    #[document(summary = "Its height. On main, a difficulty period's first, divisible by 2016.")]
    pub height: u64,
    #[document(summary = "The network the relay follows.")]
    pub network: BtcNetworkRules,
}

impl Hashable for BtcStart {}

#[document(
    summary = "Anchoring's settings, as genesis's anchoring or a Change::Anchoring sets them, each part switched on its own. Unset, everything is off. Anchors recorded while enforce was on stay enforced once it is off: a vote stops new protection, never the protection anchors already give.",
    category = "Anchoring",
    see_also = "Change::Anchoring, Genesis::anchoring, Tx::Anchor, BtcStart",
)]
#[derive(Clone, Debug, PartialEq, Eq, Hash, SchemaWrite, SchemaRead)]
pub struct AnchorSettings {
    #[document(summary = "Whether the chain takes Bitcoin anchors: Tx::Anchor fails while it is off.")]
    pub bitcoin: bool,
    #[document(summary = "Whether anchorers write to Ethereum too. The chain does not count an Ethereum anchor yet: that waits for a light client of Ethereum's finality.")]
    pub ethereum: bool,
    #[document(summary = "Whether the anchors recorded from now on bind nodes: no block replaces an enforced anchored checkpoint, and anchored sync goes by them.")]
    pub enforce: bool,
    #[document(summary = "The Bitcoin relay contract proofs are checked against, laid out as contracts/examples/btc_relay keeps its headers.")]
    pub relay: Option<Address>,
    #[document(summary = "Where the relay started, which its records must hold, and anchored sync checks headers from.")]
    pub btc_start: Option<BtcStart>,
    #[document(summary = "How deep a Bitcoin block must be in the relay before an anchor in it counts: 1 is the tip.")]
    pub depth: u64,
    #[document(summary = "Epochs between anchors that earn the reward: anchors in between are recorded and enforced, unpaid.")]
    pub every: u64,
    #[document(summary = "What the first proof of a rewarded anchor earns, in the coin's smallest unit: minted, or from the reward pool on a chain that pools fees, at most what the pool holds.")]
    #[wincode(with = "U256Le")]
    pub reward: u256,
}

impl Hashable for AnchorSettings {}

#[document(
    summary = "How deep an anchor's Bitcoin block must be unless the settings say otherwise: 6, as Bitcoin's wallets wait.",
    category = "Anchoring",
)]
pub const DEFAULT_ANCHOR_DEPTH: u64 = 6;

#[document(
    summary = "Epochs between rewarded anchors unless the settings say otherwise: 90, an hour of five-second blocks at eight a checkpoint.",
    category = "Anchoring",
)]
pub const DEFAULT_ANCHOR_EVERY: u64 = 90;

impl AnchorSettings {
    #[document(parent = "AnchorSettings", summary = "Everything off: what a chain without anchoring settings runs under.")]
    pub const OFF: AnchorSettings = AnchorSettings {
        bitcoin: false,
        ethereum: false,
        enforce: false,
        relay: None,
        btc_start: None,
        depth: DEFAULT_ANCHOR_DEPTH,
        every: DEFAULT_ANCHOR_EVERY,
        reward: u256::ZERO,
    };

    #[document(parent = "AnchorSettings", summary = "Why no chain could run under them: Bitcoin on without a relay or a start, a start that is no header, a depth of 0 or past a day of blocks, or no epochs between rewards.")]
    pub fn problem(&self) -> Option<&'static str> {
        if self.bitcoin && (self.relay.is_none() || self.btc_start.is_none()) {
            return Some("anchoring.bitcoin needs anchoring.relay and anchoring.btc_start");
        }

        if self.relay.is_some_and(|relay| !matches!(relay, Address::Contract(_))) {
            return Some("anchoring.relay is a native contract");
        }

        if self.btc_start.as_ref().is_some_and(|start| start.header.len() != 80) {
            return Some("anchoring.btc_start.header is 80 bytes");
        }

        if self.btc_start.as_ref().is_some_and(|start| start.network == BtcNetworkRules::Main && !start.height.is_multiple_of(2016)) {
            return Some("on main, anchoring.btc_start.height is a difficulty period's first, divisible by 2016");
        }

        if self.depth == 0 || self.depth > 144 {
            return Some("anchoring.depth is 1 to 144 blocks");
        }

        (self.every == 0).then_some("anchoring.every is at least one epoch")
    }
}

impl Default for AnchorSettings {
    fn default() -> AnchorSettings {
        AnchorSettings::OFF
    }
}

#[document(
    summary = "Which chain an anchor sits in: Bitcoin or Ethereum.",
    category = "Anchoring",
    see_also = "AnchorSource, AnchorEntry",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AnchorChain {
    #[document(summary = "Bitcoin, by its proof of work.")]
    Bitcoin,
    #[document(summary = "Ethereum's beacon chain, by its finality.")]
    Ethereum,
}

impl AnchorChain {
    #[document(parent = "AnchorChain", summary = "Its name, as the API writes it in an entry's chain: bitcoin or ethereum.")]
    pub fn name(self) -> &'static str {
        match self {
            AnchorChain::Bitcoin => "bitcoin",
            AnchorChain::Ethereum => "ethereum",
        }
    }
}

#[document(
    summary = "Where an anchored checkpoint sits in the chain that holds it: a Bitcoin block and its height, or an Ethereum beacon block's root and its slot. What anchored sync compares two histories by where they part.",
    category = "Anchoring",
    see_also = "AnchorEntry, AnchorChain, compare_histories",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
#[wincode(tag_encoding = "u8")]
pub enum AnchorSource {
    #[document(
        summary = "In Bitcoin.",
        params = { block = "The Bitcoin block holding the anchor, as the relay keys it: its header's SHA-256 twice, unreversed.", height = "Its height." },
    )]
    #[wincode(tag = 0)]
    Bitcoin { block: [u8; 32], height: u64 },
    #[document(
        summary = "In Ethereum.",
        params = { root = "The beacon block root of the block whose execution payload holds the anchor.", slot = "Its slot." },
    )]
    #[wincode(tag = 1)]
    Ethereum { root: [u8; 32], slot: u64 },
}

impl Hashable for AnchorSource {
    const TAG_LEN: usize = 1;
}

impl AnchorSource {
    #[document(parent = "AnchorSource", summary = "The chain it is in.")]
    pub fn chain(&self) -> AnchorChain {
        match self {
            AnchorSource::Bitcoin { .. } => AnchorChain::Bitcoin,
            AnchorSource::Ethereum { .. } => AnchorChain::Ethereum,
        }
    }

    #[document(parent = "AnchorSource", summary = "How far along its chain it sits: a Bitcoin height, or an Ethereum slot. Only two of one chain compare.")]
    pub fn position(&self) -> u64 {
        match self {
            AnchorSource::Bitcoin { height, .. } => *height,
            AnchorSource::Ethereum { slot, .. } => *slot,
        }
    }

    #[document(parent = "AnchorSource", summary = "Where it sits, in words: Bitcoin block 840000, or Ethereum slot 9000000.")]
    pub fn describe(&self) -> String {
        match self {
            AnchorSource::Bitcoin { height, .. } => format!("Bitcoin block {height}"),
            AnchorSource::Ethereum { slot, .. } => format!("Ethereum slot {slot}"),
        }
    }
}

#[document(
    summary = "One anchored checkpoint, as the chain records it: which, where the chain that holds it holds it, and whether the settings enforced it when it was recorded.",
    category = "Anchoring",
    see_also = "Anchors, Tx::Anchor, AnchorSource",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct AnchorEntry {
    #[document(summary = "The checkpoint's epoch.")]
    pub epoch: u64,
    #[document(summary = "The checkpoint's block.")]
    pub block: Hash,
    #[document(summary = "Where the anchor sits: a Bitcoin block, or an Ethereum one.")]
    pub source: AnchorSource,
    #[document(summary = "Whether nodes refuse to replace it: the settings' enforce when it was recorded.")]
    pub enforced: bool,
}

impl Hashable for AnchorEntry {}

#[document(
    summary = "Where the Ethereum headers anchoring trusts begin: the beacon block root the Ethereum relay was started at, and its network by name. What a peer's answer names so a node checking its Ethereum anchors knows where to start; the chain takes no Ethereum anchors yet, so nothing sets it.",
    category = "Anchoring",
    see_also = "AnchorsAnswer, BtcStart",
)]
#[derive(Clone, Debug, PartialEq, Eq, Hash, SchemaWrite, SchemaRead)]
pub struct EthStart {
    #[document(summary = "The trusted beacon block root the relay was started at.")]
    pub root: [u8; 32],
    #[document(summary = "Its network, as the relay was started with: mainnet, sepolia, hoodi or one defined.")]
    pub network: String,
}

impl Hashable for EthStart {}

#[document(
    summary = "A peer's anchor list, as SyncResponse::Anchors carries it: where its relays' headers start and how far its Bitcoin relay reaches, every entry in the order its chain recorded them, and proofs of those asked for, the newest's always.",
    category = "Anchoring",
    see_also = "SyncRequest::Anchors, History",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct AnchorsAnswer {
    #[document(summary = "Where the peer's Bitcoin relay started, as its anchoring settings say; none on a chain that takes no Bitcoin anchors.")]
    pub start: Option<BtcStart>,
    #[document(summary = "The height of its Bitcoin relay's tip.")]
    pub btc_tip: u64,
    #[document(summary = "Where the peer's Ethereum relay started; none until the chain takes Ethereum anchors.")]
    pub eth_start: Option<EthStart>,
    #[document(summary = "Every entry, oldest first.")]
    pub entries: Vec<AnchorEntry>,
    #[document(summary = "Proofs by place in the list: the newest entry's, and any asked for.")]
    pub proofs: Vec<(u64, AnchorProof)>,
}
