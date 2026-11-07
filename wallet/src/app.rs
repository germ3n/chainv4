//! The wallet: vaults of accounts, each sealed by what opens it, a node it talks to but
//! never trusts, and the light client that checks everything the node says. Each frame takes what
//! arrived, asks for what is due, and draws the page.

use {
    crate::chain::{Chain, Trust},
    crate::history::{Checked, Origin},
    crate::keys::{self, Holder},
    crate::lock::{self, Lock, Opened},
    crate::node::{Ask, Node},
    crate::reading::{Reading, Wanted},
    crate::theme::{self, DARK, LIGHT, Palette},
    crate::updates::Updates,
    crate::vault::{Book, Sealed, Vault},
    crate::views::{self, Ctx, Page},
    chainv4::structs::address::Address,
    chainv4_ui::asks::{PageAsks, Reload},
    chainv4_ui::pull::Pull,
    egui::{RichText, Ui},
    serde::{Deserialize, Serialize},
    std::collections::HashMap,
    std::time::Duration,
};

/// Seconds between asking again for the coin's details, and for what touched the account.
const STATUS_EVERY: f64 = 10.0;
const HISTORY_EVERY: f64 = 6.0;
/// The key the wallet keeps everything it remembers under.
const STORAGE_KEY: &str = "chainv4-wallet";
/// Where the Bitcoin light client's findings are kept, apart from the settings.
#[cfg(all(feature = "bitcoin-light", not(target_arch = "wasm32")))]
const BITCOIN_KEY: &str = "chainv4-bitcoin";
/// The most proven transactions kept, the newest; older ones are proven again if asked.
const MAX_CHECKED: usize = 2_000;
/// Below this width, in points, as on a phone, the sidebar gives way to a bar on top and tabs at
/// the bottom.
const NARROW: f32 = 640.0;
/// How many pages the back gesture remembers.
const VISITED: usize = 32;
/// What the titles call the network while no name is known for it: before its node answers, on a
/// chain with none, or from a node from before chains had names.
const FALLBACK_NAME: &str = "chainv4";

/// The pages left, latest last, each once, for the back gesture: a page left again moves to the
/// end, so going back never loops between two.
pub(crate) struct Visits {
    /// The pages left, each with where it was scrolled to as it was left.
    left: Vec<(Page, f32)>,
    shown: Page,
}

impl Visits {
    pub(crate) fn new(first: Page) -> Visits {
        Visits { left: Vec::new(), shown: first }
    }

    /// Notes the page shown now; the one before is left, scrolled to offset, if it changed.
    /// Whether it did: a page come to anew starts at its top.
    pub(crate) fn see(&mut self, page: Page, offset: f32) -> bool {
        if page == self.shown {
            return false;
        }

        let left = self.shown;
        self.left.retain(|(known, _)| *known != left);
        self.left.push((left, offset));
        self.shown = page;

        if self.left.len() > VISITED {
            self.left.remove(0);
        }

        true
    }

    /// The page to go back to, now shown, and where it was scrolled to; None at the first.
    pub(crate) fn back(&mut self) -> Option<(Page, f32)> {
        let (page, offset) = self.left.pop()?;
        self.shown = page;

        Some((page, offset))
    }
}
/// What this install is, chosen on its first launch. Changing it takes removing its wallets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Talks to a node. Its wallets hold their keys and sign, or watch an offline signer's
    /// accounts and send what the signer signed, side by side. An install made a watch-only
    /// sender before that was a kind of wallet reads as this.
    #[serde(alias = "sender")]
    Online,
    /// Holds its keys and never goes online: signs what a watch-only wallet asks, by QR code.
    Signer,
}

impl Role {
    pub fn name(self) -> &'static str {
        match self {
            Role::Online => "Online wallet",
            Role::Signer => "Offline signer",
        }
    }
}

/// Everything the wallet remembers between runs. The accounts are sealed; nothing else is secret.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// What this install is; None until its first launch asks.
    pub role: Option<Role>,
    /// The newest release manifest seen, as JSON, so no node takes this wallet back past it.
    #[serde(default)]
    pub newest_release: Option<String>,
    /// The node's HTTP API.
    pub node: String,
    /// The proxy every request goes through, as socks5h://host:port or http://host:port, or empty
    /// to go direct. Never used in the browser, whose own proxy settings apply.
    pub proxy: String,
    /// The genesis id trusted at each node: the one first seen there, or one pasted.
    pub pinned: HashMap<String, String>,
    /// A finalized checkpoint's block id to start the light client from at each node, pasted,
    /// rather than genesis.
    pub checkpoints: HashMap<String, String>,
    /// What the light client checked, by genesis id, encoded, to resume from.
    pub light: HashMap<String, String>,
    /// The nodes trusted to resume the light client from their own finalized checkpoint, rather
    /// than have it walk every header since it last ran: off for any node until switched on, as a
    /// dishonest one could show a false history from that point.
    pub fast_resume: HashMap<String, bool>,
    /// Whether to take everything from final blocks fetched and checked, rather than ask nodes: see
    /// crate::reading. For the install rather than for a node, as fast resume is, so that changing
    /// the node never quietly starts asking it about everything, nor for one wallet alone. Off by
    /// default, as reading blocks costs far more than asking. Written only when on.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub read_blocks: bool,
    /// The coin's symbol and decimals its node last said, by genesis id: what amounts are read by while
    /// no node is asked, as nothing the light client checks says them.
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub coins: HashMap<String, (String, u8)>,
    /// A wallet made just now, to note the height it was made at. Never kept.
    #[serde(skip)]
    pub just_made: bool,
    /// Whether to forget what the wallet open has read, once the page is drawn. Never kept.
    #[serde(skip)]
    pub forget_reading: bool,
    /// Wallets just removed, by the name of their files, whose indexes are to go too: on a device the
    /// store takes those with the wallet's files, and in a browser this does. Never kept.
    #[serde(skip)]
    pub forget_indexes: Vec<String>,
    /// The wallets on this device, each its accounts sealed by what opens it (see crate::guard).
    pub wallets: Vec<WalletEntry>,
    /// Records of biometrics no wallet uses any more, whose keys are not yet forgotten: those are
    /// forgotten when settings saying so are read back, as the app starts, so a kill in between can
    /// never leave settings on disk naming a key that went. Written only when there are any.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub retired: Vec<String>,
    /// Whether to write settings now, rather than when the app next saves: a vault was replaced or
    /// a wallet removed. Never kept.
    #[serde(skip)]
    pub persist_now: bool,
    /// The wallet last opened, by its place among them.
    pub wallet: usize,
    /// Transactions this wallet sent that are not final yet, oldest first.
    pub sent: Vec<Sent>,
    /// Transactions the light client proved, by id. What a wallet read from blocks is shown from here
    /// too, and written only to that wallet's sealed index.
    #[serde(serialize_with = "crate::history::kept_only")]
    pub checked: HashMap<String, Checked>,
    /// Minutes without input before the wallet locks itself; 0 for never.
    pub lock_after: u32,
    /// The light palette rather than the dark one.
    pub light_theme: bool,
    /// The contracts the Contracts page keeps, with an EVM one's ABI.
    pub contracts: Vec<SavedContract>,
    /// An offline signer's word for which chain is yours, by its Ethereum chain id: what MetaMask's
    /// requests for it are named by, and what any other is warned of.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signer_chain: Option<u64>,
    /// An Esplora API, such as https://mempool.space/api, that the Swaps page looks Bitcoin up
    /// at and broadcasts through: empty for none, so nothing about a swap leaves the wallet.
    pub esplora: String,
    /// The swap contract to use in place of the one the node names: empty to take the node's.
    pub swap_contract: String,
    /// EVM chains' JSON-RPC URLs, one a line, that the Swaps page reads and sends an EVM leg's
    /// locks and claims through: each is asked its chain id, and serves that chain's legs.
    pub eth_rpc: String,
    /// Whether the light client follows Bitcoin, and its test network, for swaps and the accounts'
    /// Bitcoin addresses: off by default, as a first sync takes a while and some data.
    pub bitcoin_main: bool,
    pub bitcoin_test: bool,
    /// Which test network: testnet4, or signet.
    pub bitcoin_test_network: String,
    /// A Bitcoin node to try first, such as your own: host:port, or empty for the DNS seeds'.
    pub bitcoin_peer: String,
    /// How many confirmations a swap's lock on Bitcoin needs before the other side acts on it.
    pub bitcoin_depth: u32,
    /// Whether the NFTs page fetches what a token's URI names, its metadata and picture: off by
    /// default, as each fetch tells whoever serves it that this wallet looks at that token.
    pub nft_media: bool,
    /// Tokens sites asked the wallet to show (wallet_watchAsset), each by the genesis id of the
    /// chain whose EVM it lives on.
    pub watched: Vec<(String, crate::dapp::Watched)>,
    /// The sidebar's groups folded, by heading.
    pub folded: std::collections::HashSet<String>,
    /// The WalletConnect project id sites are reached under, from cloud.reown.com: empty, and
    /// WalletConnect is off.
    pub wc_project: String,
    /// The WalletConnect relay: empty for Reown's.
    pub wc_relay: String,
    /// The IPFS gateway ipfs:// URIs are fetched through, ending in a slash.
    pub ipfs_gateway: String,
    /// The multisigs the Multisig page follows, by address: those made here, and those added.
    pub multisigs: Vec<String>,
    /// The HTTP API of the node that runs the mining pool the Mining page asks, for a pool on
    /// another node than the wallet's: empty to ask the wallet's own.
    pub pool_node: String,
    /// The mining pools' claims kept for the accounts' addresses, by the genesis of the chain
    /// they are on: SavedClaims, as the hex of its bytes. With them the Mining page claims even
    /// once a pool's node is gone.
    pub pool_claims: HashMap<String, String>,
    /// The chain's name its node last said, by genesis id: what the titles show before the node
    /// answers, as on the lock screen.
    pub names: HashMap<String, String>,
    /// How the Mining page's Mine card mines: alone, or in the port's pool.
    pub mine_solo: bool,
    /// On the GPU rather than CPU threads.
    pub mine_gpu: bool,
    /// The one GPU picked, by its place in the GPUs listed, by a build from before several could be:
    /// read until a tick replaces it, as settings of another shape would not load at all.
    pub mine_gpus: Option<usize>,
    /// The GPUs picked, by their places in the GPUs listed, or every one for None.
    pub mine_gpu_list: Option<Vec<usize>>,
    /// How many CPU threads, when not on the GPU.
    pub mine_threads: usize,
    /// The mining port as host:port, in place of the one GET /mining names: empty to take the
    /// node's.
    pub mine_port: String,
    /// The port's token, as typed, for a port that is not open. It is the port's shared secret,
    /// which its operator hands its miners, not a key of the wallet's: it proves only that this
    /// worker may mine there, so it is kept as plainly as the rest of the settings.
    pub mine_token: String,
    /// Whether a phone mines on battery too, rather than pausing off its charger.
    pub mine_on_battery: bool,
    /// Low data: ask the node a few times less often, for a metered connection or a phone's battery.
    /// See chainv4_ui::pace.
    pub low_data: bool,
    /// How many times as long as usual a request may take before a page says it got no answer:
    /// chainv4_ui::net::waits. 1 is the usual.
    pub patience: u32,
    /// How many minutes a trade or a deposit on the Trade page may wait to land before the DEX
    /// refuses it.
    pub trade_deadline_minutes: u32,
    /// The most transactions a wallet reading checked blocks keeps, the newest: see crate::reading.
    pub reading_txs: usize,
    /// What a wallet from before there could be several kept: its one vault, and the account in
    /// use. Read, to become the first wallet, and never written again.
    #[serde(skip_serializing)]
    vault: Option<Sealed>,
    #[serde(skip_serializing)]
    current: String,
}

/// A contract kept on the Contracts page.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedContract {
    /// Its address, as the chain writes it: contract:0x… or evm:0x….
    pub address: String,
    /// What to call it here.
    pub name: String,
    /// An EVM contract's ABI, as pasted; empty for a native one, whose functions the node lists.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub abi: String,
}

/// A wallet on this device.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WalletEntry {
    /// What its files are named, on a device or a desktop: see crate::store. Empty until first saved.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    pub name: String,
    /// Its seed phrase and accounts, sealed by what opens it.
    pub vault: Sealed,
    /// The account in use, by its first address.
    pub current: String,
    /// What biometrics keep for it, as the platform keeps it, tagged when the wallet needs them to
    /// open: None while it has none. See crate::guard, which reads what opens it from this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub biometric: Option<String>,
    /// Whether it is sealed with no password, so it opens with a tap, and the lock screen says
    /// anyone who reads app storage could. Never true of one that needs hardware.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unprotected: bool,
    /// The height the chain had reached when this wallet was made here, as a new wallet has nothing
    /// before it to read: u64::MAX for one made before the chain was followed, read from wherever the
    /// reading starts. None for one restored or brought in, whose history could reach back any way.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub born: Option<u64>,
}

impl Settings {
    /// The GPUs picked to mine on, by their places in the GPUs listed, or every one for None.
    #[cfg_attr(not(feature = "mining"), allow(dead_code))]
    pub fn gpus_picked(&self) -> Option<Vec<usize>> {
        self.mine_gpu_list.clone().or_else(|| self.mine_gpus.map(|which| vec![which]))
    }
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            newest_release: None,
            role: None,
            node: String::new(),
            proxy: String::new(),
            pinned: HashMap::new(),
            checkpoints: HashMap::new(),
            light: HashMap::new(),
            fast_resume: HashMap::new(),
            read_blocks: false,
            coins: HashMap::new(),
            just_made: false,
            forget_reading: false,
            forget_indexes: Vec::new(),
            wallets: Vec::new(),
            retired: Vec::new(),
            persist_now: false,
            wallet: 0,
            sent: Vec::new(),
            checked: HashMap::new(),
            lock_after: 15,
            light_theme: false,
            contracts: Vec::new(),
            signer_chain: None,
            esplora: String::new(),
            swap_contract: String::new(),
            eth_rpc: String::new(),
            bitcoin_main: false,
            bitcoin_test: false,
            bitcoin_test_network: "testnet4".to_string(),
            bitcoin_peer: String::new(),
            bitcoin_depth: 3,
            nft_media: false,
            watched: Vec::new(),
            folded: std::collections::HashSet::new(),
            wc_project: String::new(),
            wc_relay: String::new(),
            // chainv4::nft::DEFAULT_GATEWAY, which a build without NFTs has no copy of.
            ipfs_gateway: "https://ipfs.io/ipfs/".to_string(),
            multisigs: Vec::new(),
            pool_node: String::new(),
            pool_claims: HashMap::new(),
            names: HashMap::new(),
            mine_solo: false,
            mine_gpu: false,
            mine_gpus: None,
            mine_gpu_list: None,
            mine_threads: 1,
            mine_port: String::new(),
            mine_token: String::new(),
            mine_on_battery: false,
            low_data: false,
            patience: 1,
            trade_deadline_minutes: TRADE_DEADLINE_MINUTES,
            reading_txs: crate::reading::MOST_TXS,
            vault: None,
            current: String::new(),
        }
    }
}

/// How many minutes a trade may wait to land unless Settings says otherwise, and the most it may
/// say: a trade that lands a day after it was priced is a trade at a price nobody chose.
pub const TRADE_DEADLINE_MINUTES: u32 = 20;
pub const MOST_TRADE_DEADLINE_MINUTES: u32 = 24 * 60;

impl Settings {
    /// How often the wallet asks its node, by Low data.
    pub fn pace(&self) -> chainv4_ui::pace::Pace {
        chainv4_ui::pace::Pace::new(self.low_data)
    }

    /// How long a trade may wait to land, in milliseconds, within the bounds Settings offers.
    #[cfg_attr(not(feature = "dex"), allow(dead_code))]
    pub fn trade_deadline_ms(&self) -> u64 {
        u64::from(self.trade_deadline_minutes.clamp(1, MOST_TRADE_DEADLINE_MINUTES)) * 60_000
    }

    /// The most transactions a reading keeps, within the bounds Settings offers.
    pub fn reading_txs(&self) -> usize {
        self.reading_txs.clamp(crate::reading::LEAST_TXS, crate::reading::MOST_TXS_SET)
    }
}

#[cfg(test)]
impl Settings {
    /// Settings of an online wallet holding these, for a test.
    pub fn with_wallets(wallets: Vec<WalletEntry>) -> Settings {
        Settings { role: Some(Role::Online), wallets, ..Settings::default() }
    }
}

impl Settings {
    /// What the titles call the network: the name its node says, else the one kept for the genesis
    /// pinned at the node, else chainv4. An offline signer follows no chain, so it says chainv4.
    fn chain_name<'a>(&'a self, said: Option<&'a str>) -> &'a str {
        if self.role == Some(Role::Signer) {
            return FALLBACK_NAME;
        }

        said.or_else(|| self.names.get(self.pinned.get(&self.node)?).map(String::as_str)).unwrap_or(FALLBACK_NAME)
    }

    /// Settings as kept, with the vault of a wallet from before there could be several made the
    /// first wallet.
    fn migrated(mut self) -> Settings {
        if let Some(vault) = self.vault.take() {
            self.wallets.insert(0, WalletEntry {
                id: String::new(),
                name: "Wallet 1".to_string(),
                vault,
                current: std::mem::take(&mut self.current),
                biometric: None,
                unprotected: false,
                born: None,
            });
            self.wallet = 0;
        }

        self.wallet = self.wallet.min(self.wallets.len().saturating_sub(1));

        // An install from before there were roles, with wallets in it, held its keys online.
        if self.role.is_none() && !self.wallets.is_empty() {
            self.role = Some(Role::Online);
        }

        self
    }

    /// What this install is, online until it chose.
    pub fn role(&self) -> Role {
        self.role.unwrap_or(Role::Online)
    }

    /// The chain what is kept is shown for: the one followed, or, with none followed yet, the one this
    /// node is pinned to, as a wallet refusing another chain is still on that one. None before either.
    pub fn chain_shown(&self, followed: Option<chainv4::structs::hash::Hash>) -> Option<String> {
        followed.map(|genesis| genesis.to_string()).or_else(|| self.pinned.get(&self.node).cloned())
    }

    /// What became of a transaction, as proven on this chain.
    pub fn checked_on<'s>(&'s self, tx: &str, genesis: Option<&str>) -> Option<&'s Checked> {
        self.checked.get(tx).filter(|checked| checked.on(genesis))
    }

    /// The account in use in the wallet open, by its first address.
    pub fn current(&self) -> &str {
        self.wallets.get(self.wallet).map_or("", |entry| entry.current.as_str())
    }

    pub fn set_current(&mut self, id: String) {
        if let Some(entry) = self.wallets.get_mut(self.wallet) {
            entry.current = id;
        }
    }
}

/// A transaction this wallet sent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sent {
    pub id: String,
    pub from: String,
    pub to: String,
    /// What was sent, as shown: 1.5 DEV, 200 EXT.
    pub what: String,
    pub nonce: u64,
    /// When, in milliseconds since the Unix epoch.
    pub at: u64,
    /// The chain it was sent to, by its genesis id; empty for one sent before it was said.
    #[serde(default)]
    pub genesis: String,
}

/// The chain's name in GET /status, as a chain could have it: none on a chain without one, which
/// says null, nor from a node from before chains had names, which leaves it out. Nothing proven
/// rests on it: it only titles the wallet.
fn status_name(status: &serde_json::Value) -> Option<String> {
    status["name"].as_str().filter(|name| chainv4::governance::chain_name_problem(name).is_none()).map(str::to_string)
}

/// The chain's coin, as the node names it.
#[derive(Clone, Debug)]
pub struct Coin {
    pub symbol: String,
    pub decimals: u8,
}

pub struct Wallet {
    settings: Settings,
    palette: Palette,
    node: Node,
    chain: Chain,
    lock: Lock,
    /// The wallet open: its vault, what the vault holds, and its accounts, unlocked.
    vault: Option<Vault>,
    book: Book,
    holders: Vec<Holder>,
    page: Page,
    /// The pages left, for the back gesture to return to.
    visits: Visits,
    /// Where the page shown is scrolled to, and where it should be: the top of a page come to
    /// anew, or where one gone back to was left.
    scrolled: chainv4_ui::layout::Scrolled,
    coin: Coin,
    /// The chain's name, as the node's status says it, if it says one.
    name: Option<String>,
    /// The window's title as last set, to set it again only when the name changes.
    titled: String,
    /// The node's faucet, as its status describes it, on a node that runs one.
    faucet: Option<serde_json::Value>,
    /// The swap contract the node names in GET /status, if it names one.
    swap_contract: Option<String>,
    /// The DEX, and the EVM DEX's router, the node names in GET /status.
    dex: (Option<String>, Option<String>),
    /// The DEXes' fee tiers and burned share in force, as the node's status says.
    dex_settings: chainv4::params::vm::DexSettings,
    /// The EVM's spec /status names, which reads run here go by.
    #[cfg(not(target_arch = "wasm32"))]
    evm_spec: Option<chainv4::params::vm::EvmSpec>,
    /// The fork schedule, as /status says it, which the wallet's own reads run by: on the desktop
    /// and Android, as the browser runs none.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    fork_schedule: chainv4::params::forks::ForkSchedule,
    /// The engines reads run on here, over the state the node proves.
    #[cfg(not(target_arch = "wasm32"))]
    engines: chainv4_ui::rerun::Engines,
    /// Where this wallet stands against the newest release the node relays, and whether the node
    /// stops at an upgrade.
    updates: Updates,
    /// The Bitcoin light clients the settings ask for.
    #[cfg(all(feature = "bitcoin-light", not(target_arch = "wasm32")))]
    lights: crate::light::Lights,
    /// WalletConnect, while Settings names a project and the wallet is open.
    #[cfg(all(feature = "walletconnect", not(target_arch = "wasm32")))]
    walletconnect: Option<crate::walletconnect::WalletConnect>,
    wake: egui::Context,
    asked_status: Option<f64>,
    asked_history: f64,
    /// When the wallet last saw input, for locking itself.
    active: f64,
    /// The lock screen's width, and the tallest it has been at it: what places its card, so a
    /// keyboard taking the bottom of the screen does not move the field under a finger.
    lock_room: (f32, f32),
    /// Whether the unlock screen shows the node and proxy to change.
    reaching: bool,
    views: views::States,
    /// What it reads from checked blocks, with that setting on.
    reading: Reading,
    /// Whether the node just said what the coin is, for keeping with its chain once that is known.
    status_coin: bool,
    /// What each page asked, for a reload to ask again.
    asked_by_page: PageAsks<Page, Ask>,
    /// A reload under way, and the finger on the page that may start one.
    reloading: Reload<Ask>,
    pull: Pull,
    /// What reads EVM calls and logs, each once.
    reader: chainv4::abi::known::AbiReader,
    /// Where wallets and settings are kept, off the web: None if there is nowhere, and eframe's
    /// storage is used as the web's is.
    #[cfg(not(target_arch = "wasm32"))]
    store: Option<crate::store::Store>,
    /// Whether eframe's file still holds wallets, to take out once they are kept elsewhere.
    #[cfg(not(target_arch = "wasm32"))]
    eframes_to_clear: bool,
    /// What to tell the person of what storage could not read or write, until they have seen it.
    storage_trouble: Vec<String>,
}

impl Wallet {
    pub fn new(creation: &eframe::CreationContext, node: Option<String>, fallback: &str, folder: Option<std::path::PathBuf>) -> Wallet {
        let eframes = creation.storage.and_then(|storage| eframe::get_value::<Settings>(storage, STORAGE_KEY));
        #[cfg(not(target_arch = "wasm32"))]
        let eframes_to_clear = eframes.is_some();
        #[cfg(not(target_arch = "wasm32"))]
        let mut store = folder.and_then(|folder| crate::store::Store::new(&folder).map_err(|why| log::error!("{why}")).ok());
        #[cfg(target_arch = "wasm32")]
        let _ = folder;
        #[cfg_attr(target_arch = "wasm32", allow(unused_mut))]
        let mut storage_trouble = Vec::new();
        #[cfg(not(target_arch = "wasm32"))]
        let (mut settings, heal) = match &mut store {
            Some(store) => {
                let loaded = store.load();
                let heal = loaded.heal || eframes_to_clear;
                storage_trouble.clone_from(&loaded.trouble);

                (crate::store::combined(loaded, eframes).migrated(), heal)
            }
            None => (eframes.unwrap_or_default().migrated(), false),
        };
        #[cfg(target_arch = "wasm32")]
        let mut settings = eframes.unwrap_or_default().migrated();
        #[cfg(not(target_arch = "wasm32"))]
        {
            settings.persist_now |= heal;
        }
        settings.node = node
            .or_else(|| Some(settings.node.clone()).filter(|node| !node.is_empty()))
            .unwrap_or_else(|| fallback.to_string())
            .trim()
            .trim_end_matches('/')
            .to_string();
        crate::biometrics::retire_all(&creation.egui_ctx, &mut settings);
        let palette = if settings.light_theme { LIGHT } else { DARK };
        announce(&settings.node);
        theme::install_fonts(&creation.egui_ctx);
        theme::apply(&creation.egui_ctx, &palette);
        let lock = Lock::start(&settings);
        #[cfg(not(target_arch = "wasm32"))]
        let reading = crate::reading::on_this_device(store.as_ref().map(crate::store::Store::folder));
        #[cfg(target_arch = "wasm32")]
        let reading = crate::reading::on_this_device();

        Wallet {
            node: Node::new(&settings.node, client(&settings), creation.egui_ctx.clone()).reading_blocks(settings.read_blocks),
            reading,
            status_coin: false,
            settings,
            palette,
            chain: Chain::default(),
            lock,
            vault: None,
            book: Book::default(),
            holders: Vec::new(),
            page: Page::Overview,
            visits: Visits::new(Page::Overview),
            scrolled: chainv4_ui::layout::Scrolled::default(),
            coin: Coin {
                symbol: "DEV".to_string(),
                decimals: 18,
            },
            name: None,
            titled: String::new(),
            faucet: None,
            swap_contract: None,
            dex: (None, None),
            dex_settings: chainv4::params::vm::DexSettings::DEFAULT,
            #[cfg(not(target_arch = "wasm32"))]
            evm_spec: None,
            fork_schedule: Default::default(),
            #[cfg(not(target_arch = "wasm32"))]
            engines: chainv4_ui::rerun::Engines::default(),
            updates: Updates::default(),
            #[cfg(all(feature = "bitcoin-light", not(target_arch = "wasm32")))]
            lights: crate::light::Lights {
                kept: creation.storage.and_then(|storage| eframe::get_value(storage, BITCOIN_KEY)).unwrap_or_default(),
                ..Default::default()
            },
            #[cfg(all(feature = "walletconnect", not(target_arch = "wasm32")))]
            walletconnect: None,
            wake: creation.egui_ctx.clone(),
            asked_status: None,
            asked_history: 0.0,
            active: 0.0,
            lock_room: (0.0, 0.0),
            reaching: false,
            views: views::States::default(),
            asked_by_page: PageAsks::default(),
            reloading: Reload::default(),
            pull: Pull::default(),
            reader: chainv4::abi::known::AbiReader::default(),
            #[cfg(not(target_arch = "wasm32"))]
            store,
            #[cfg(not(target_arch = "wasm32"))]
            eframes_to_clear,
            storage_trouble,
        }
    }

    /// Starts afresh with a node, through the proxy set: its own chain, light client and pinned
    /// genesis.
    fn connect(&mut self, node: &str) {
        self.settings.node = node.trim().trim_end_matches('/').to_string();
        announce(&self.settings.node);
        self.node = Node::new(&self.settings.node, client(&self.settings), self.wake.clone()).reading_blocks(self.settings.read_blocks);
        self.reading.close(&mut self.chain, &mut self.settings.checked, self.active);
        self.chain = Chain::default();

        // What the node said of itself is no use once it is not being asked.
        if self.settings.read_blocks {
            self.faucet = None;
            self.swap_contract = None;
            self.dex = (None, None);
        }

        self.name = None;
        self.asked_status = None;
        self.asked_history = 0.0;
        self.asked_by_page = PageAsks::default();
        self.reloading = Reload::default();
        self.updates.connected();
    }

    /// Asks the node again for what the page shows and its status, past whatever failed before:
    /// what Reload does, and pulling a page down. What shows stays until the answers come.
    fn reload(&mut self, time: f64) {
        self.asked_status = None;
        self.asked_history = f64::NEG_INFINITY;
        self.reading.retry();
        self.chain.reload();
        self.views.reload(self.page);
        let mut asks = vec![Ask::Status];
        asks.extend_from_slice(self.asked_by_page.of(&self.page));
        asks.extend(self.node.trouble().map(|(_, failed)| failed).unwrap_or_default());

        for ask in &asks {
            self.node.retry(ask.clone());
        }

        self.reloading.start(asks, time);
    }

    /// Waits for the save just asked for, says if it failed, and only with it safely written takes
    /// the wallets out of eframe's file, which would otherwise offer them again as an older build's.
    #[cfg(not(target_arch = "wasm32"))]
    fn settled(&mut self, storage: &mut dyn eframe::Storage) {
        let Some(store) = &mut self.store else {
            return;
        };

        if store.wait() {
            if std::mem::take(&mut self.eframes_to_clear) {
                storage.remove_string(STORAGE_KEY);
            }
        } else if let Some(why) = store.failed.clone().filter(|why| !self.storage_trouble.contains(why)) {
            self.storage_trouble.push(why);
        }
    }

    /// A line across the top for what storage could not read or write, until it is dismissed.
    fn storage_said(&mut self, ui: &mut Ui) {
        if self.storage_trouble.is_empty() {
            return;
        }

        let palette = self.palette;
        let mut seen = false;

        egui::Panel::top("storage trouble").frame(egui::Frame::new().fill(palette.card).inner_margin(egui::Margin::symmetric(14, 8))).show(ui, |ui| {
            for why in &self.storage_trouble {
                ui.colored_label(palette.bad, why.as_str());
            }

            seen = theme::quiet(ui, &palette, "OK").clicked();
        });

        if seen {
            self.storage_trouble.clear();
        }
    }

    /// Seals the wallet's accounts again after a change. Those this build cannot read have no holder
    /// and go back as they came.
    fn store(&mut self) -> Result<(), String> {
        let vault = self.vault.as_ref().ok_or("the wallet is locked")?;
        self.book.accounts = keys::stored(&self.holders, &self.book);
        let sealed = vault.seal(&self.book)?;
        self.settings.wallets.get_mut(self.settings.wallet).ok_or("the wallet is gone")?.vault = sealed;

        Ok(())
    }

    /// The account in use: the one chosen, or else the first.
    fn current(&self) -> Option<&Holder> {
        let chosen = self.settings.current();

        self.holders.iter().find(|holder| holder.id() == chosen).or_else(|| self.holders.first())
    }

    /// Drops the keys, and asks for what opens it again. What it read of the chain goes first, sealed
    /// with those keys, as nothing of it is to be seen or read on while it is locked.
    fn lock(&mut self) {
        self.reading.close(&mut self.chain, &mut self.settings.checked, self.active);
        self.vault = None;
        self.book = Book::default();
        // Its sessions' keys go with the accounts.
        #[cfg(all(feature = "walletconnect", not(target_arch = "wasm32")))]
        {
            self.walletconnect = None;
        }
        self.holders.clear();
        self.views = views::States::default();
        self.lock = Lock::start(&self.settings);
        // What egui laid out of those pages, the words if they were shown, freed and so wiped.
        chainv4_ui::wipe::let_go(&self.wake);

        // Biometrics show by themselves as the app starts, not as it locks: whoever locked it, or
        // left it, may be gone.
        if let Lock::Locked { prompted, .. } = &mut self.lock {
            *prompted = Some(self.settings.wallet);
        }
    }

    /// Takes everything that arrived.
    fn take(&mut self, now: u64, time: f64) {
        for (ask, result) in self.node.answers() {
            let node = self.settings.node.clone();

            let trust = Trust {
                node: &node,
                pinned: &mut self.settings.pinned,
                checkpoints: &self.settings.checkpoints,
                saved: &self.settings.light,
                // Taking up the chain on a node's word is the opposite of what that setting is for.
                fast_resume: !self.settings.read_blocks && self.settings.fast_resume.get(&node).copied().unwrap_or(false),
            };

            if let Some(client) = &self.chain.client
                && self.reading.proof(&ask, &result, client, time)
            {
                continue;
            }

            if matches!(ask, Ask::ScanProof { .. }) {
                continue;
            }

            if self.chain.take(&ask, &result, trust, now, time) {
                continue;
            }

            match (&ask, &result) {
                (Ask::Status, Ok(value)) => {
                    self.faucet = value.get("faucet").filter(|faucet| !faucet.is_null()).cloned();
                    self.swap_contract = value["swap"]["contract"].as_str().map(str::to_string);
                    self.dex = (value["dex"]["contract"].as_str().map(str::to_string), value["dex"]["router"].as_str().map(str::to_string));
                    self.dex_settings = chainv4::params::json::dex_from_status(value);
                    self.fork_schedule = chainv4::params::json::forks_from_status(value);
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        self.evm_spec = value["evm_spec"].as_str().and_then(|spec| spec.parse().ok());
                    }
                    self.updates.status(value);
                    self.name = status_name(value);

                    if let (Some(symbol), Some(decimals)) = (value["coin"]["symbol"].as_str(), value["coin"]["decimals"].as_u64()) {
                        self.coin = Coin {
                            symbol: symbol.to_string(),
                            decimals: u8::try_from(decimals).unwrap_or(18),
                        };
                        self.status_coin = true;
                    }
                }
                (Ask::Contract(_) | Ask::Call(_) | Ask::Read(_), _) => {
                    // A read the node proved is run again here, and its outcome is this wallet's own.
                    #[cfg(not(target_arch = "wasm32"))]
                    let result = match (&ask, result) {
                        (Ask::Call(body), Ok(value)) => {
                            let rules = chainv4_ui::rerun::Rules { dex: self.dex_settings, evm_spec: self.evm_spec, forks: self.fork_schedule.clone() };
                            Ok(chainv4_ui::rerun::checked(&mut self.engines, body, value, self.chain.client.as_ref(), rules))
                        }
                        (_, result) => result,
                    };

                    self.views.answered(&ask, &result, &self.settings.contracts)
                }
                (Ask::Rpc(body), _) if self.views.unlisted_answered(body, &result, now) => {}
                #[cfg(feature = "dex")]
                (Ask::Rpc(body), _) if self.views.trade.answered_rpc(body, &result) => {}
                (Ask::Rpc(body), _) => self.views.dapp.answered(body, &result),
                #[cfg(feature = "dex")]
                (Ask::Named { .. }, _) => self.views.trade.events(&result),
                #[cfg(feature = "nft")]
                (Ask::Logs { .. }, _) => {
                    if let Some(next) = self.views.nfts.logs(&ask, &result) {
                        self.node.ask(next);
                    }
                }
                (Ask::Faucet(_), _) => self.views.faucet.answered(&ask, &result),
                (Ask::PoolClaim(_) | Ask::Pool(_) | Ask::Claims { .. } | Ask::Mining, _) => self.views.mining.answered(&ask, &result),
                (Ask::Governance | Ask::GovernanceHistory { .. } | Ask::GovernanceWeight(_), _) => self.views.governance.answered(&ask, &result),
                (Ask::Release, _) => match chainv4::release::ReleaseKeys::built_in() {
                    Ok(keys) => self.updates.answered(&result, &keys, time, &mut self.settings.newest_release, now / 1_000),
                    Err(why) => log::warn!("{why}"),
                },
                (Ask::Multisig(_), _) => {
                    #[cfg(feature = "swap")]
                    self.views.swaps.multisig_answered(&ask, &result);
                    self.views.multisig.answered(&ask, &result);
                }
                (Ask::Memberships { .. }, _) => {
                    self.views.multisig.answered(&ask, &result);
                }
                (Ask::Offer(_) | Ask::Submit(_), _) if self.views.multisig.answered(&ask, &result) => {}
                // One listing, a page at a time, for the proofs and the NFTs page both.
                (Ask::Collections(cursor), _) => {
                    self.chain.list_collections(cursor.is_some(), &result);
                    #[cfg(feature = "nft")]
                    self.views.nfts.collections(&self.chain.collection_entries);
                }
                (Ask::Offer(_) | Ask::Submit(_), _) => {
                    if let Some(mut sent) = self.views.send.answered(&ask, &result, now) {
                        sent.genesis = self.chain.genesis.map(|genesis| genesis.to_string()).unwrap_or_default();
                        self.settings.sent.push(sent);
                    }
                }
                _ => {}
            }
        }

        self.updates.take();

        #[cfg(feature = "nft")]
        for (url, result) in self.node.fetched() {
            self.views.nfts.fetched(&url, result, self.settings.ipfs_gateway.trim());
        }

        // What was proven is kept, so it shows without proving it again, with the chain it is on.
        let genesis = self.chain.genesis.map(|genesis| genesis.to_string()).unwrap_or_default();

        for (tx, proven) in std::mem::take(&mut self.chain.receipts) {
            // Its own sent transaction, looked up while reading blocks, stands until the reading
            // reaches it and takes over: included, whatever the block's finality, and not final.
            if self.settings.read_blocks {
                if self.settings.checked_on(&tx, Some(&genesis)).is_none_or(|known| known.came != Origin::Read) {
                    self.settings.checked.insert(tx, Checked { genesis: genesis.clone(), final_: false, came: Origin::Looked, ..Checked::of(&proven) });
                }

                continue;
            }

            self.settings.checked.insert(tx, Checked { genesis: genesis.clone(), ..Checked::of(&proven) });
        }

        // No node says what the coin is called while none is asked: what one last said for this chain,
        // or else plain units, as a wrong number of decimals would misstate every amount.
        if self.settings.read_blocks {
            self.coin = self.settings.coins.get(&genesis).map_or(Coin { symbol: "units".to_string(), decimals: 0 }, |(symbol, decimals)| Coin { symbol: symbol.clone(), decimals: *decimals });
        } else if !genesis.is_empty() && self.status_coin {
            self.settings.coins.insert(genesis.clone(), (self.coin.symbol.clone(), self.coin.decimals));
            self.status_coin = false;
        }

        // One kept before the chain was said is this chain's once its node mentions it.
        if !genesis.is_empty() {
            for mention in self.chain.mentions.values().flatten() {
                if let Some(checked) = self.settings.checked.get_mut(&mention.tx).filter(|checked| checked.genesis.is_empty()) {
                    checked.genesis = genesis.clone();
                }
            }
        }

        // Too many kept: another chain's go first, then this one's oldest. What a wallet read from
        // blocks has its own bound, in its index.
        if !self.settings.read_blocks && self.settings.checked.len() > MAX_CHECKED {
            let rank = |checked: &Checked| (checked.genesis == genesis, checked.height);
            let mut ranks: Vec<(bool, u64)> = self.settings.checked.values().map(rank).collect();
            ranks.sort_unstable();
            let lowest = ranks[ranks.len() - MAX_CHECKED];
            self.settings.checked.retain(|_, checked| rank(checked) >= lowest);
        }

        // Sent ones that are final are the history's now; the log keeps only what waits.
        let checked = &self.settings.checked;
        self.settings.sent.retain(|sent| !checked.get(&sent.id).is_some_and(|checked| checked.final_ && checked.on(Some(&sent.genesis))));

        // The name the node says is kept with its chain, for the titles before it answers again.
        if !genesis.is_empty()
            && let Some(name) = &self.name
            && self.settings.names.get(&genesis) != Some(name)
        {
            self.settings.names.insert(genesis, name.clone());
        }

        if let Some((genesis, state)) = self.chain.saved(false) {
            self.settings.light.insert(genesis, state);
        }
    }

    /// Keeps the Bitcoin light clients as the settings say, watching the accounts' addresses and
    /// the swaps' HTLCs: only while the wallet is open, and never on an offline signer.
    #[cfg(all(feature = "bitcoin-light", not(target_arch = "wasm32")))]
    fn follow_bitcoin(&mut self) {
        use crate::light::Net;

        if !matches!(self.lock, Lock::Open) || self.settings.role != Some(Role::Online) {
            self.lights.stop_all();

            return;
        }

        let wanted = [(Net::Main, self.settings.bitcoin_main), (Net::test(&self.settings.bitcoin_test_network), self.settings.bitcoin_test)];
        let (holders, book) = (&self.holders, &self.book);
        // And each account's deposit address to the Bitcoin gateway, for the Bridge page to prove.
        #[cfg(feature = "bridge")]
        let deposits = crate::views::bridge::deposit_outputs(&self.chain, holders);
        #[cfg(not(feature = "bridge"))]
        let deposits: Vec<Vec<u8>> = Vec::new();
        let scripts = |net: Net| {
            let mut scripts = crate::light::scripts(holders, book, net.testnet());
            scripts.extend(deposits.iter().cloned());
            scripts
        };
        self.lights.keep(wanted, &self.settings.proxy, &self.settings.bitcoin_peer, scripts, &self.wake);
    }

    /// Asks for what is due. An offline signer asks for nothing, nor an install before it says
    /// what it is: it may be about to become one.
    fn poll(&mut self, time: f64) {
        if matches!(self.settings.role, None | Some(Role::Signer)) {
            return;
        }

        let listening = self.node.listen();
        self.chain.streamed = listening.open;
        self.chain.pace = self.settings.pace();

        if listening.moved {
            self.chain.nudge();
        }

        if let Some(ask) = self.chain.next_ask(time) {
            self.node.ask(ask);
        }

        if self.settings.read_blocks {
            if self.updates.due(time) {
                self.node.ask(Ask::Release);
            }

            self.read_blocks(time);

            return;
        }

        if self.asked_status.is_none_or(|asked| time - asked >= self.settings.pace().every(STATUS_EVERY)) {
            self.asked_status = Some(time);
            self.node.ask(Ask::Status);
        }

        if self.updates.due(time) {
            self.node.ask(Ask::Release);
        }

        let addresses: Vec<Address> = self.holders.iter().flat_map(Holder::addresses).collect();

        // The multisig the Multisig page shows, for its policy to be proven too.
        self.chain.wanted_policies = self.views.multisig.selected().map(str::to_string).into_iter().collect();
        self.chain.wanted_claims = self.views.mining.wanted_claims();

        // Contract reads go at the block the light client checked last, to be proven there.
        self.node.proving = self.chain.client.as_ref().map(|client| client.tip().id().to_string());

        // A slow way to the node, as Tor, leaves a round waiting: a new one on top would only grow.
        if !self.node.proofs_waiting() {
            for ask in self.chain.proofs_due(&addresses, time) {
                self.node.ask(ask);
            }
        }

        let wanted = self.views.wanted_names();

        for ask in self.chain.names_due(&wanted, time) {
            self.node.ask(ask);
        }

        // The NFTs the account holds, proven while their page shows.
        #[cfg(feature = "nft")]
        if self.page == views::Page::Nfts {
            for ask in self.chain.nfts_due(&self.views.nfts.wanted(), time) {
                self.node.ask(ask);
            }
        }

        let Some(current) = self.current().map(|holder| holder.addresses().iter().map(Address::to_string).collect::<Vec<_>>()) else {
            return;
        };

        if !self.chain.caught_up() || time - self.asked_history < self.settings.pace().every(HISTORY_EVERY) {
            return;
        }

        self.asked_history = time;

        // The address index lists everything that touched an address; without one, the event index
        // lists at least what mentions it. With both, both: a token's transfer to an address names
        // it only in an EVM log's topics, which the event index knows and the address index does not.
        for address in &current {
            if self.chain.indexes_addresses != Some(false) {
                self.node.ask(Ask::History { address: address.clone(), cursor: None });
            }

            if self.chain.indexes_events != Some(false) {
                self.node.ask(Ask::Events { address: address.clone(), cursor: None });
            }
        }

        let mentioned = current.iter().flat_map(|address| self.chain.mentions.get(address).into_iter().flatten()).map(|mention| mention.tx.clone());
        let sent = self.settings.sent.iter().filter(|sent| current.contains(&sent.from)).map(|sent| sent.id.clone());
        let genesis = self.settings.chain_shown(self.chain.genesis);
        let unsettled: Vec<String> = sent
            .chain(mentioned)
            // Final ones are settled; the rest are proven again until they are. Final on another chain
            // says nothing of this one, though the id be the same.
            .filter(|tx| !self.settings.checked_on(tx, genesis.as_deref()).is_some_and(|checked| checked.final_))
            .take(40)
            .collect();

        for tx in unsettled {
            self.node.ask(Ask::Receipt(tx));
        }
    }

    /// With Read from checked blocks on: takes what came of the blocks asked for, asks for more, and
    /// the one lookup left, this wallet's own sent transactions. Only while it is unlocked, and only
    /// once the light client has something to check them against. A chain with no finality has no
    /// final blocks to read, which the setting's card says.
    fn read_blocks(&mut self, time: f64) {
        self.reading.keep_most_txs(self.settings.reading_txs());
        let wallet = self.settings.wallets.get(self.settings.wallet);
        let (Some(client), Some(genesis), Some(vault), Some(wallet)) = (&self.chain.client, self.chain.genesis, &self.vault, wallet) else {
            return;
        };

        if !matches!(self.lock, Lock::Open) || client.params().finality.is_none() || !self.chain.has_followed() {
            return;
        }

        if wallet.id.is_empty() {
            match crate::vault::fresh_name() {
                Ok(id) => {
                    if let Some(entry) = self.settings.wallets.get_mut(self.settings.wallet) {
                        entry.id = id;
                    }

                    self.settings.persist_now = true;
                }
                Err(why) => log::warn!("{why}"),
            }

            return;
        }

        let addresses: Vec<Address> = self.holders.iter().flat_map(Holder::addresses).collect();
        let wanted = Wanted { wallet: &wallet.id, genesis: &genesis.to_string(), vault_key: vault.key(), addresses, born: wallet.born };

        for (asked, came) in self.node.ranges() {
            self.reading.came(asked, came, client, time);
        }

        let go = self.reading.go(wanted, client, time);

        for ask in go.proofs {
            self.node.ask(ask);
        }

        if let Some(asked) = go.range {
            self.node.read_range(asked);
        }

        let hash = client.params().hash;
        self.reading.show(hash, &mut self.chain, &mut self.settings.checked);

        if time - self.asked_history >= self.settings.pace().every(HISTORY_EVERY) {
            self.asked_history = time;
            let genesis = self.settings.chain_shown(self.chain.genesis);
            let unknown: Vec<String> = self.settings.sent.iter().filter(|sent| self.settings.checked_on(&sent.id, genesis.as_deref()).is_none()).map(|sent| sent.id.clone()).take(40).collect();

            for tx in unknown {
                self.node.ask(Ask::SentReceipt(tx));
            }
        }
    }

    /// Tells the browser extension what the wallet is, for the sites it answers, and takes what
    /// they ask, once the wallet is open. Nothing outside the extension.
    fn sites(&mut self) {
        #[cfg(all(feature = "walletconnect", not(target_arch = "wasm32")))]
        self.walletconnect();

        // Natively, what was answered is WalletConnect's to take, or no one's.
        #[cfg(not(target_arch = "wasm32"))]
        self.views.dapp.outbox.clear();

        if !crate::dapp::available() || self.settings.role() != Role::Online {
            return;
        }

        let unlocked = matches!(self.lock, Lock::Open);
        let accounts: Vec<[u8; 20]> = crate::views::dapp::offered(&self.holders).into_iter().map(|(_, account)| account).collect();
        let chain = self.chain.genesis.map(|genesis| chainv4::crypto::ethereum::chain_id(&genesis));
        crate::dapp::report(&crate::dapp::state(&self.settings.node, chain, &accounts, unlocked));

        // What a site asks of the chain goes through the node, which is not asked with this on: each
        // is answered so at once, for the site to say.
        if unlocked && self.settings.read_blocks {
            for request in crate::dapp::pending() {
                if let Some(id) = request["id"].as_u64() {
                    crate::dapp::answer(id, Err((4200, format!("the wallet says {}", crate::node::NEEDS_ASKING))));
                }
            }
        } else if unlocked {
            self.views.dapp.take(crate::dapp::pending());
        }
    }

    /// WalletConnect kept running while it may: a project named, the wallet open and online, on a
    /// chain it follows. Its sites' requests go to the approval as the extension's do, the answers
    /// back, and its sessions are sealed with the accounts as they change.
    #[cfg(all(feature = "walletconnect", not(target_arch = "wasm32")))]
    fn walletconnect(&mut self) {
        use crate::walletconnect::{RELAY, Session, WalletConnect};

        let project = self.settings.wc_project.trim().to_string();
        let relay = match self.settings.wc_relay.trim() {
            "" => RELAY.to_string(),
            relay => relay.to_string(),
        };
        let proxy = chainv4_ui::net::Proxy::parse(&self.settings.proxy).ok().map(|proxy| proxy.as_str().to_string());
        let chain = self.chain.genesis.map(|genesis| chainv4::crypto::ethereum::chain_id(&genesis));
        let may = !project.is_empty() && matches!(self.lock, Lock::Open) && self.settings.role() == Role::Online;

        let (Some(chain), true) = (chain, may) else {
            self.walletconnect = None;

            return;
        };
        let wanted = (project.clone(), relay.clone(), proxy.clone());

        if self.walletconnect.as_ref().is_some_and(|running| running.started_with != wanted) {
            self.walletconnect = None;
        }

        let running = self.walletconnect.get_or_insert_with(|| {
            let sessions = self.book.walletconnect.iter().filter_map(|session| serde_json::from_value::<Session>(session.clone()).ok()).collect();

            WalletConnect::start(&project, &relay, proxy, sessions, chain, self.wake.clone())
        });
        running.follow(chain);

        for (id, answer) in self.views.dapp.outbox.drain(..) {
            running.answer(id, answer);
        }

        // What needs the node, with the node not asked, is answered so at once.
        let pending = running.pending();

        if self.settings.read_blocks {
            for request in pending.iter().filter(|request| request["method"] != "eth_requestAccounts") {
                if let Some(id) = request["id"].as_u64() {
                    running.answer(id, Err((4200, format!("the wallet says {}", crate::node::NEEDS_ASKING))));
                }
            }
        }

        self.views.dapp.take(running.pending());

        if let Some(sessions) = running.changed() {
            self.book.walletconnect = sessions.iter().filter_map(|session| serde_json::to_value(session).ok()).collect();

            if let Err(error) = self.store() {
                log::warn!("could not seal the WalletConnect sessions: {error}");
            }
        }
    }

    /// Names the window, or the browser's tab, after the chain: chainv4 testnet wallet.
    fn title(&mut self, ctx: &egui::Context) {
        let title = format!("{} wallet", self.settings.chain_name(self.name.as_deref()));

        if title == self.titled {
            return;
        }

        // The browser's tab is the page's title, which eframe leaves alone.
        #[cfg(target_arch = "wasm32")]
        if let Some(document) = web_sys::window().and_then(|window| window.document()) {
            document.set_title(&title);
        }

        ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
        self.titled = title;
    }

    fn lock_screen(&mut self, ui: &mut Ui) {
        let palette = self.palette;
        let (width, height) = (ui.available_width(), ui.available_height());

        if self.lock_room.0 != width {
            self.lock_room = (width, height);
        }

        self.lock_room.1 = self.lock_room.1.max(height);
        let tallest = self.lock_room.1;
        let mut opened = None;

        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            chainv4_ui::layout::keep_focus_in_view(ui);
            ui.vertical_centered(|ui| {
                ui.add_space((tallest * 0.16).max(24.0));
                ui.set_max_width(ui.available_width().min(380.0));
                let name = self.settings.chain_name(self.name.as_deref());
                ui.add(egui::Label::new(RichText::new(name).font(egui::FontId::new(44.0, egui::FontFamily::Name(theme::SERIF.into()))).color(palette.ink)).wrap());
                theme::caps(ui, &palette, match self.settings.role {
                    Some(Role::Signer) => "Offline signer",
                    _ => "Light wallet",
                });
                ui.add_space(26.0);
                let role = self.settings.role;
                ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                    opened = lock::show(ui, &palette, &mut self.settings, &mut self.lock);
                });

                // A role picked: an offline signer is left nothing to send requests by.
                if self.settings.role != role {
                    let node = self.settings.node.clone();
                    self.connect(&node);
                }
                ui.add_space(12.0);
                let through = match chainv4_ui::net::Proxy::parse(&self.settings.proxy) {
                    Ok(proxy) if !cfg!(target_arch = "wasm32") => format!(" · through {}", proxy.shown()),
                    _ => String::new(),
                };
                let footer = match self.settings.role {
                    Some(Role::Signer) => "Offline signer · makes no network requests".to_string(),
                    None => String::new(),
                    Some(_) => format!("Node {}{through}", self.settings.node),
                };
                ui.label(RichText::new(footer).small().color(palette.muted));

                // The node and the way to it, changed before the wallet opens: one that is gone,
                // or behind Tor, would leave it with nothing to show.
                if matches!(self.settings.role, Some(role) if role != Role::Signer) {
                    let shown = if self.reaching { "Done" } else { "Change the node or proxy" };

                    if ui.add(egui::Button::new(RichText::new(shown).small().color(palette.accent)).frame(false)).clicked() {
                        self.reaching = !self.reaching;
                    }

                    if self.reaching {
                        ui.add_space(6.0);
                        let mut connect = None;
                        ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                            theme::card(ui, &palette, |ui| {
                                connect = views::reach(ui, &palette, &mut self.settings, &mut self.views);
                            });
                        });

                        if let Some(node) = connect {
                            self.connect(&node);
                        }
                    }
                }
                ui.add_space(24.0);
            });
        });

        if let Some(Opened { vault, book, reseal }) = opened {
            // Opening checked the book already. Should it still not read, it stays locked with
            // nothing written: opened without an account, the next save would delete that account.
            match keys::open(&book) {
                Ok(holders) => {
                    // A wallet made here has nothing before now to read.
                    if std::mem::take(&mut self.settings.just_made)
                        && let Some(entry) = self.settings.wallets.get_mut(self.settings.wallet)
                    {
                        entry.born = Some(self.chain.following().map_or(u64::MAX, |(tip, _)| tip));
                    }

                    self.holders = holders;
                    self.book = book;
                    self.vault = Some(vault);
                    self.lock = Lock::Open;

                    // Keyed again at the stronger stretch as it opened: sealed so at once, or the
                    // old seal stays and opens as before.
                    if reseal && let Err(why) = self.store() {
                        self.storage_trouble.push(format!("the wallet could not be sealed with the stronger password stretch: {why}"));
                    }
                    self.page = views::home(self.settings.role());

                    if let Some(first) = self.holders.first().map(Holder::id)
                        && !self.holders.iter().any(|holder| holder.id() == self.settings.current())
                    {
                        self.settings.set_current(first);
                    }
                }
                Err(why) => {
                    self.lock = Lock::locked();

                    if let Lock::Locked { error, .. } = &mut self.lock {
                        *error = Some(why);
                    }
                }
            }
        }
    }

    fn sidebar(&mut self, ui: &mut Ui) {
        let palette = self.palette;
        chainv4_ui::layout::sidebar(
            ui,
            self,
            |app, ui| {
                // A short window, as a phone on its side, keeps its room for the pages: the foot goes
                // on one line, what the dot would say on a tap.
                if chainv4_ui::layout::is_short(ui.ctx()) {
                    app.folded_foot(ui);

                    return;
                }

                ui.add_space(18.0);
                ui.horizontal(|ui| {
                    // Three to a line in the sidebar's width, none of them wrapping its word.
                    ui.spacing_mut().item_spacing.x = 6.0;
                    ui.spacing_mut().button_padding.x = 8.0;
                    let theme_name = if app.settings.light_theme { "Dark" } else { "Light" };

                    if theme::quiet(ui, &palette, theme_name).clicked() {
                        app.settings.light_theme = !app.settings.light_theme;
                    }

                    if theme::quiet(ui, &palette, "Lock").clicked() {
                        app.lock();
                    }

                    // A signer asks nothing of any node, and a finger pulls the page down instead.
                    if app.settings.role() != Role::Signer && !chainv4_ui::layout::touch(ui.ctx()) {
                        let node = &app.node;
                        let working = app.reloading.working(ui.input(|input| input.time), |ask| node.asking(ask));

                        if theme::reload(ui, &palette, working) {
                            app.reload(ui.input(|input| input.time));
                        }
                    }
                });
                ui.add_space(10.0);

                // A signer follows no chain: it says so instead.
                match app.settings.role() {
                    Role::Signer => {
                        ui.label(RichText::new("Never online: what it signs goes by QR code.").small().color(palette.muted));
                        ui.add_space(4.0);
                        theme::caps(ui, &palette, "Offline");
                    }
                    _ => {
                        views::light_status(ui, &palette, &app.chain, app.node.down());
                        ui.add_space(4.0);
                        theme::caps(ui, &palette, "Light client");
                    }
                }
            },
            |app, ui| {
                // A short window, as a phone on its side, keeps its room for the pages.
                let gap = |ui: &Ui, tall: f32| chainv4_ui::layout::sidebar_gap(ui.ctx(), tall);
                ui.add_space(gap(ui, 26.0));
                let name = app.settings.chain_name(app.name.as_deref());
                chainv4_ui::widgets::sidebar_name(ui, &palette, name, match (app.settings.role(), app.book.watch) {
                    (Role::Signer, _) => "Offline signer",
                    (Role::Online, true) => "Watch-only",
                    (Role::Online, false) => "Light wallet",
                });
                ui.add_space(gap(ui, 24.0));

                if !app.holders.is_empty() {
                    theme::caps(ui, &palette, "Account");
                    app.account_picker(ui, ui.available_width());
                    ui.add_space(gap(ui, 18.0));
                }

                for &(group, pages) in views::groups(app.settings.role()) {
                    if pages.is_empty() {
                        continue;
                    }

                    // A group folded keeps the page showing listed, so where you are stays in view.
                    let folded = app.settings.folded.contains(group);

                    if !group.is_empty() && chainv4_ui::widgets::group_heading(ui, &palette, group, folded).clicked() {
                        match folded {
                            true => app.settings.folded.remove(group),
                            false => app.settings.folded.insert(group.to_string()),
                        };
                    }

                    for &(page, name) in pages {
                        if folded && app.page != page {
                            continue;
                        }

                        if chainv4_ui::widgets::nav_entry(ui, &palette, name, app.page == page).clicked() {
                            app.page = page;
                        }
                    }

                    ui.add_space(gap(ui, 14.0));
                }
            },
        );
    }

    /// The sidebar's foot on a short screen, on one line: the light client's state, as a dot and a
    /// word that say the rest on a tap, and the buttons at its end.
    fn folded_foot(&mut self, ui: &mut Ui) {
        let palette = self.palette;
        let time = ui.input(|input| input.time);
        let online = self.settings.role() != Role::Signer;
        let node = &self.node;
        let working = self.reloading.working(time, |ask| node.asking(ask));
        let theme_name = if self.settings.light_theme { "Dark" } else { "Light" };
        let (mut lock, mut flip, mut reload) = (false, false, false);

        ui.add_space(8.0);
        chainv4_ui::widgets::folded_foot(
            ui,
            |ui| match online {
                true => {
                    views::light_dot(ui, &palette, &self.chain, self.node.down());
                    ui.label(RichText::new(views::light_word(&palette, &self.chain, self.node.down())).small().color(palette.soft));
                }
                false => {
                    ui.label(RichText::new("Offline").small().color(palette.soft));
                }
            },
            |ui| {
                lock = theme::quiet(ui, &palette, "Lock").clicked();
                flip = theme::quiet(ui, &palette, theme_name).clicked();
                reload = online && !chainv4_ui::layout::touch(ui.ctx()) && theme::reload(ui, &palette, working);
            },
        );

        if flip {
            self.settings.light_theme = !self.settings.light_theme;
        }

        if reload {
            self.reload(time);
        }

        if lock {
            self.lock();
        }
    }

    /// Which account is in use, to pick from.
    fn account_picker(&mut self, ui: &mut Ui, width: f32) {
        let Some(current) = self.current() else {
            return;
        };
        let mut chosen = current.id();
        egui::ComboBox::from_id_salt("account").selected_text(current.name.clone()).width(width).truncate().show_ui(ui, |ui| {
            for holder in &self.holders {
                ui.selectable_value(&mut chosen, holder.id(), &holder.name);
            }
        });

        if chosen != self.settings.current() {
            self.settings.set_current(chosen);
        }
    }

    /// On a narrow screen, what the sidebar holds but the pages: the name, the light client's
    /// state, the account and the lock.
    fn top_bar(&mut self, ui: &mut Ui) {
        let palette = self.palette;
        ui.horizontal(|ui| {
            // Buttons no wider than their words, so the account keeps its room.
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.spacing_mut().button_padding.x = 8.0;
            // A long name is cut short, leaving the account and Lock their room.
            let name = self.settings.chain_name(self.name.as_deref());
            let room = ui.available_width() * 0.45;
            chainv4_ui::widgets::bar_name(ui, &palette, name, room);

            if self.settings.role() != Role::Signer {
                views::light_dot(ui, &palette, &self.chain, self.node.down());
            }
            let mut reload = false;
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::quiet(ui, &palette, "Lock").clicked() {
                    self.lock();
                }

                // A narrow window a mouse is on has no sidebar to reload from, nor a finger to
                // pull with: its Reload sits here, out of the page's way.
                if self.settings.role() != Role::Signer && !chainv4_ui::layout::touch(ui.ctx()) {
                    let node = &self.node;
                    let working = self.reloading.working(ui.input(|input| input.time), |ask| node.asking(ask));
                    reload = theme::reload(ui, &palette, working);
                }

                let width = (ui.available_width() - 8.0).clamp(90.0, 180.0);
                self.account_picker(ui, width);
            });

            if reload {
                self.reload(ui.input(|input| input.time));
            }
        });
    }

    /// Keeps the pages left, and takes Android's back gesture, or a mouse's back button: a view the
    /// page opened closes first, such as a swap, then the page before comes back. With neither, it
    /// is left for the platform, which sends the app to the background.
    fn go_back(&mut self, ui: &mut Ui) {
        // Before the page is drawn, the offset is still the page left's.
        if self.visits.see(self.page, self.scrolled.offset()) {
            self.scrolled.top();
        }

        if !ui.input(|input| input.key_pressed(egui::Key::BrowserBack)) {
            return;
        }

        let went = self.views.back(self.page)
            || self
                .visits
                .back()
                .inspect(|(page, offset)| {
                    self.page = *page;
                    self.scrolled.back_to(*offset);
                })
                .is_some();

        if went {
            ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::BrowserBack));
        }
    }

    /// On a narrow screen, the tabs along the bottom, each as wide as the next: the same place for
    /// each, on every page.
    fn tabs(&mut self, ui: &mut Ui) {
        let palette = self.palette;
        let role = self.settings.role();

        if let Some(page) = chainv4_ui::widgets::tabs(ui, &palette, views::tabs(role), &views::tab_of(self.page, role)) {
            self.page = *page;
        }
    }
}

impl eframe::App for Wallet {
    fn ui(&mut self, ui: &mut Ui, frame: &mut eframe::Frame) {
        // What changed what opens a wallet goes to disk at once, rather than when next saved.
        if std::mem::take(&mut self.settings.persist_now)
            && let Some(storage) = frame.storage_mut()
        {
            self.save(storage);
            #[cfg(not(target_arch = "wasm32"))]
            self.settled(storage);
            storage.flush();
        }

        chainv4_ui::wipe::letting_go(ui.ctx());
        let time = ui.input(|input| input.time);
        let now = now_ms();
        self.take(now, time);
        self.poll(time);
        #[cfg(all(feature = "bitcoin-light", not(target_arch = "wasm32")))]
        self.follow_bitcoin();
        self.sites();
        self.title(ui.ctx());
        chainv4_ui::layout::suit_touch(ui.ctx());
        let palette = self.palette;

        self.storage_said(ui);

        // A wallet too old for the network goes no further, locked or open.
        if self.updates.blocks() {
            egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(palette.paper).inner_margin(egui::Margin::symmetric(14, 0)))
            .show(ui, |ui| self.updates.blocking(ui, &palette, &self.settings, &self.wake));
            ui.ctx().request_repaint_after(Duration::from_millis(500));

            return;
        }

        if !matches!(self.lock, Lock::Open) {
            egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(palette.paper).inner_margin(egui::Margin::symmetric(14, 0)))
            .show(ui, |ui| {
                if chainv4_ui::layout::is_narrow(ui.available_width()) || chainv4_ui::layout::touch(ui.ctx()) {
                    chainv4_ui::layout::finger_sized(ui);
                }

                self.lock_screen(ui);
            });
            self.active = time;
            ui.ctx().request_repaint_after(Duration::from_millis(500));

            return;
        }

        if ui.input(|input| !input.events.is_empty() || input.pointer.is_moving()) {
            self.active = time;
        }

        if self.settings.lock_after > 0 && time - self.active > f64::from(self.settings.lock_after) * 60.0 {
            self.lock();
            ui.ctx().request_repaint();

            return;
        }

        // The miner is kept on every page: its payout, a phone's charger and its being in front.
        #[cfg(feature = "mining")]
        {
            let addresses = self.current().map(Holder::addresses).unwrap_or_default();
            self.views.mining.keep_mining(ui.ctx(), &self.settings, &addresses, now);
        }

        if std::mem::take(&mut self.settings.forget_reading) {
            self.reading.forget(&mut self.chain, &mut self.settings.checked);
        }

        for id in std::mem::take(&mut self.settings.forget_indexes).into_iter().filter(|id| !id.is_empty()) {
            self.reading.forget_wallet(&id);
        }

        if let Some(why) = self.reading.keeping_trouble().filter(|why| !self.storage_trouble.contains(why)) {
            self.storage_trouble.push(why);
        }

        self.storage_trouble.extend(std::mem::take(&mut self.reading.said));

        self.go_back(ui);
        let narrow = ui.available_width() < NARROW;
        let bar = chainv4_ui::widgets::bar_frame(&palette);
        // A finger needs taller buttons and fields than a mouse: on a phone, upright or on its side.
        let fingers = narrow || chainv4_ui::layout::touch(ui.ctx());
        let sized = |ui: &mut Ui| {
            if fingers {
                chainv4_ui::layout::finger_sized(ui);
            }
        };

        match narrow {
            true => {
                egui::Panel::top("bar").frame(bar.inner_margin(chainv4_ui::layout::BAR_MARGIN)).show(ui, |ui| {
                    chainv4_ui::layout::finger_row(ui);
                    self.top_bar(ui);
                });
                egui::Panel::bottom("tabs").frame(bar).show(ui, |ui| self.tabs(ui));
            }
            false => {
                egui::Panel::left("nav")
                    .exact_size(chainv4_ui::layout::SIDEBAR_WIDTH)
                    .resizable(false)
                    .frame(bar.inner_margin(egui::Margin::symmetric(chainv4_ui::layout::SIDEBAR_MARGIN, 0)))
                    .show(ui, |ui| {
                        sized(ui);
                        self.sidebar(ui);
                    });
            }
        }

        let mut connect = None;
        let mut changed = false;
        let mut leave = false;
        let mut lock = None;
        let chosen = self.settings.current().to_string();
        let current = self.holders.iter().position(|holder| holder.id() == chosen).unwrap_or(0);
        let margin = egui::Margin::symmetric(chainv4_ui::layout::page_margin(narrow), 0);
        // A signer asks nothing of any node, so there is nothing to reload or retry.
        let online = self.settings.role() != Role::Signer;
        let node = &self.node;
        let working = self.reloading.working(time, |ask| node.asking(ask));
        let trouble = self.node.trouble().filter(|_| online);
        let mut reload = false;
        egui::CentralPanel::default().frame(egui::Frame::new().fill(palette.paper).inner_margin(margin)).show(ui, |ui| {
            sized(ui);

            let area = ui.available_rect_before_wrap();
            let scroll = egui::ScrollArea::vertical().id_salt("page").auto_shrink(false);

            self.scrolled.show(ui, scroll, |ui| {
                chainv4_ui::layout::keep_focus_in_view(ui);
                ui.vertical_centered(|ui| {
                    // Never wider than there is room for: a centered column widens to its width.
                    ui.set_max_width(ui.available_width().min(760.0));
                    ui.add_space(if narrow { 18.0 } else { 34.0 });
                    ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                        self.updates.banner(ui, &palette, &self.settings, &self.wake);

                        // What failed or took too long, whichever page asked: said on every page,
                        // as a wallet's pages all rest on the node.
                        if let Some((failure, failed)) = &trouble {
                            let asking = failed.iter().all(|ask| self.node.asking(ask));

                            reload |= theme::retry(ui, &palette, &failure.sentence(), asking);
                            theme::section_gap(ui);
                        }

                        let mut ctx = Ctx {
                            palette: &palette,
                            chain: &self.chain,
                            reader: &self.reader,
                            node: &mut self.node,
                            holders: &mut self.holders,
                            current,
                            book: &mut self.book,
                            coin: &self.coin,
                            settings: &mut self.settings,
                            reading: &mut self.reading,
                            now,
                            page: &mut self.page,
                            connect: &mut connect,
                            changed: &mut changed,
                            vault: &mut self.vault,
                            leave: &mut leave,
                            lock: &mut lock,
                            wake: &self.wake,
                            faucet: self.faucet.as_ref(),
                            swap_contract: self.swap_contract.as_deref(),
                            dex: (self.dex.0.as_deref(), self.dex.1.as_deref()),
                            dex_settings: self.dex_settings,
                            node_stops: self.updates.stops(),
                            #[cfg(all(feature = "bitcoin-light", not(target_arch = "wasm32")))]
                            lights: &mut self.lights,
                            #[cfg(all(feature = "walletconnect", not(target_arch = "wasm32")))]
                            walletconnect: self.walletconnect.as_ref(),
                        };
                        views::show(ui, &mut ctx, &mut self.views);
                    });
                    ui.add_space(40.0);
                });
            });

            reload |= online && self.pull.show(ui, &palette, area, self.scrolled.offset(), self.scrolled.drag(), working);
        });

        if reload {
            self.reload(time);
        }

        // What the page asked, for a reload to ask again.
        let sent = self.node.sent();
        self.asked_by_page.note(&self.page, sent.into_iter().filter(Ask::reloads));

        // A wallet removed is locked away without sealing it again: its place is another's now.
        if leave {
            self.lock();
        } else {
            if changed && let Err(error) = self.store() {
                log::warn!("could not seal the accounts: {error}");
            }

            if let Some(why) = lock {
                self.lock();

                if let Lock::Locked { error, .. } = &mut self.lock {
                    *error = Some(why);
                }
            }
        }

        if let Some(node) = connect {
            self.connect(&node);
        }

        // The theme follows the setting, wherever it was changed.
        if self.palette.dark == self.settings.light_theme {
            self.palette = if self.settings.light_theme { LIGHT } else { DARK };
            theme::apply(ui.ctx(), &self.palette);
        }

        // What is due comes due even when nothing arrives.
        ui.ctx().request_repaint_after(self.settings.pace().wake_after(Duration::from_millis(400)));
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        if let Some((genesis, state)) = self.chain.saved(true) {
            self.settings.light.insert(genesis, state);
        }

        self.reading.save(self.active);

        #[cfg(not(target_arch = "wasm32"))]
        match &mut self.store {
            Some(store) => {
                store.save(&mut self.settings);

                if let Some(why) = store.failed.clone().filter(|why| !self.storage_trouble.contains(why)) {
                    self.storage_trouble.push(why);
                }

                if self.eframes_to_clear {
                    self.settled(storage);
                }
            }
            None => eframe::set_value(storage, STORAGE_KEY, &self.settings),
        }
        // In the browser there is no store to report a failure, and eframe only logs when the
        // site's storage is full or blocked, so the wallet could keep a change in this tab alone
        // with nothing shown. Read it back and compare, and say so when it did not take.
        #[cfg(target_arch = "wasm32")]
        {
            eframe::set_value(storage, STORAGE_KEY, &self.settings);
            storage.flush();

            let trouble = "This browser would not store the wallet: its site storage may be full or blocked. What you changed stays in this tab until that is fixed.".to_string();
            let saved = eframe::get_value::<Settings>(storage, STORAGE_KEY).and_then(|settings| serde_json::to_string(&settings).ok());
            let took = saved.is_some() && saved == serde_json::to_string(&self.settings).ok();

            match took {
                true => self.storage_trouble.retain(|why| why != &trouble),
                false if !self.storage_trouble.contains(&trouble) => self.storage_trouble.push(trouble),
                false => {}
            }
        }

        #[cfg(all(feature = "bitcoin-light", not(target_arch = "wasm32")))]
        {
            self.lights.sync();
            eframe::set_value(storage, BITCOIN_KEY, &self.lights.kept);
        }
    }

    /// The paper's color, behind everything: what shows under a phone's status bar.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        self.palette.paper.to_normalized_gamma_f32()
    }
}

/// What the wallet's requests go out through: the proxy set, or straight to the node. The browser
/// makes its own requests, through its own proxy. An offline signer has nothing to send them by,
/// so nothing it might ask can go out.
pub(crate) fn client(settings: &Settings) -> Result<chainv4_ui::net::Client, String> {
    match (settings.role, cfg!(target_arch = "wasm32")) {
        (Some(Role::Signer), _) => Err("an offline signer makes no requests".to_string()),
        (_, true) => chainv4_ui::net::Client::patient(None, settings.patience),
        (_, false) => chainv4_ui::net::Client::for_settings(&settings.proxy, settings.patience),
    }
}

/// Tells the page which node the wallet talks to, as window.chainv4Node: the browser extension's
/// page reads it, and asks the browser for leave to reach that node, which an extension needs.
fn announce(node: &str) {
    #[cfg(target_arch = "wasm32")]
    if let Some(window) = web_sys::window() {
        let _ = js_sys::Reflect::set(&window, &"chainv4Node".into(), &node.into());
    }

    #[cfg(not(target_arch = "wasm32"))]
    let _ = node;
}

fn now_ms() -> u64 {
    web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map_or(0, |since| since.as_millis() as u64)
}

#[cfg(test)]
mod tests {
    use {super::*, serde_json::json};

    #[test]
    fn every_tabs_name_fits_a_phones_share_of_the_bar() {
        // The narrowest phones in use are 320 points wide.
        const NARROWEST: f32 = 320.0;
        let ctx = egui::Context::default();
        theme::install_fonts(&ctx);
        let mut names = Vec::new();
        let mut drawn = ctx.run_ui(egui::RawInput::default(), |ui| {
            for role in [Role::Online, Role::Signer] {
                let share = NARROWEST / views::tabs(role).len() as f32;

                for &(_, name) in views::tabs(role) {
                    let galley = ui.painter().layout_no_wrap(name.to_string(), egui::FontId::new(12.0, egui::FontFamily::Name(theme::SEMIBOLD.into())), egui::Color32::WHITE);
                    names.push((name, galley.size().x, share));
                }
            }
        });
        drawn.textures_delta.clear();

        // Each tab has two points of padding on either side.
        for (name, width, share) in names {
            assert!(width + 4.0 <= share, "{name} is {width} wide, in {share}");
        }
    }

    #[test]
    fn back_goes_to_more_then_home() {
        let mut visits = Visits::new(Page::Overview);

        visits.see(Page::More, 0.0);
        visits.see(Page::Names, 120.0);

        assert_eq!(visits.back(), Some((Page::More, 120.0)));
        assert_eq!(visits.back(), Some((Page::Overview, 0.0)));
        assert_eq!(visits.back(), None);
    }

    #[test]
    fn back_returns_through_the_pages_left_each_once_and_stops_at_the_first() {
        let mut visits = Visits::new(Page::Overview);

        assert!(!visits.see(Page::Overview, 0.0), "no change");
        assert!(visits.see(Page::Contracts, 10.0), "Overview left at 10");
        visits.see(Page::Send, 20.0);
        // Back to Contracts, where it was left, then on to Send again: Contracts is left once,
        // not twice, and where it was left last.
        assert_eq!(visits.back(), Some((Page::Contracts, 20.0)));
        assert!(!visits.see(Page::Contracts, 0.0), "the page gone back to is the one shown");
        visits.see(Page::Send, 30.0);

        assert_eq!(visits.back(), Some((Page::Contracts, 30.0)));
        assert_eq!(visits.back(), Some((Page::Overview, 10.0)));
        assert_eq!(visits.back(), None, "the first page: left to the platform");
    }

    #[test]
    fn the_vault_of_a_wallet_from_before_several_becomes_the_first() {
        let sealed = Sealed {
            salt: "0x00".to_string(),
            nonce: "0x01".to_string(),
            sealed: "0x02".to_string(),
        };
        let old = json!({ "node": "http://node", "vault": sealed, "current": "ed25519:0x01", "lock_after": 5 });
        let settings = serde_json::from_value::<Settings>(old).unwrap().migrated();

        assert_eq!(settings.wallets, vec![WalletEntry {
            id: String::new(),
            name: "Wallet 1".to_string(),
            vault: sealed,
            current: "ed25519:0x01".to_string(),
            biometric: None,
            unprotected: false,
            born: None,
        }]);
        assert_eq!((settings.wallet, settings.current(), settings.lock_after), (0, "ed25519:0x01", 5));

        let kept = serde_json::to_value(&settings).unwrap();

        assert!(kept.get("vault").is_none() && kept.get("current").is_none(), "never written again");
        assert_eq!(serde_json::from_value::<Settings>(kept).unwrap().migrated().wallets, settings.wallets);
    }

    #[test]
    fn a_wallets_biometrics_are_kept_with_it_and_absent_while_off() {
        let entry = WalletEntry {
            id: String::new(),
            name: "Main".to_string(),
            vault: Sealed {
                salt: "0x00".to_string(),
                nonce: "0x01".to_string(),
                sealed: "0x02".to_string(),
                },
            current: String::new(),
            biometric: None,
            unprotected: false,
            born: None,
        };

        assert!(serde_json::to_value(&entry).unwrap().get("biometric").is_none(), "an entry from before reads the same");

        let on = WalletEntry {
            biometric: Some("android:chainv4-00:0a:0b".to_string()),
            ..entry
        };

        assert_eq!(serde_json::from_value::<WalletEntry>(serde_json::to_value(&on).unwrap()).unwrap(), on);
    }

    #[test]
    fn an_install_from_before_roles_was_online_and_a_signer_has_no_way_out() {
        let sealed = Sealed {
            salt: "0x00".to_string(),
            nonce: "0x01".to_string(),
            sealed: "0x02".to_string(),
        };
        let with_wallets = serde_json::from_value::<Settings>(json!({ "node": "http://node", "vault": sealed })).unwrap().migrated();
        let fresh = serde_json::from_value::<Settings>(json!({})).unwrap().migrated();

        assert_eq!(with_wallets.role, Some(Role::Online), "it held its keys online");
        assert_eq!(fresh.role, None, "a new install asks");
        assert_eq!(fresh.role(), Role::Online);

        let signer = Settings {
            role: Some(Role::Signer),
            proxy: chainv4_ui::net::Proxy::ORBOT.to_string(),
            ..Settings::default()
        };

        assert_eq!(client(&signer).err().as_deref(), Some("an offline signer makes no requests"), "whatever else is set");
        assert!(client(&Settings { role: Some(Role::Online), ..Settings::default() }).is_ok());

        // Watch-only was a role before it was a kind of wallet: such an install is online now.
        let sender = serde_json::from_value::<Settings>(json!({ "role": "sender" })).unwrap().migrated();

        assert_eq!(sender.role, Some(Role::Online));

        let kept = serde_json::to_value(&signer).unwrap();

        assert_eq!(kept["role"], "signer");
        assert_eq!(serde_json::from_value::<Settings>(kept).unwrap().migrated().role, Some(Role::Signer));
    }

    #[test]
    fn the_titles_name_the_chain_its_node_says_else_the_one_kept_else_chainv4() {
        assert_eq!(status_name(&json!({ "name": "chainv4 testnet", "height": 4 })).as_deref(), Some("chainv4 testnet"));
        assert_eq!(status_name(&json!({ "name": null })), None, "a chain with no name");
        assert_eq!(status_name(&json!({ "height": 4 })), None, "a node from before names");
        assert_eq!(status_name(&json!({ "name": "" })), None, "no name a chain could have");
        assert_eq!(status_name(&json!({ "name": " padded" })), None);
        assert_eq!(status_name(&json!({ "name": "x".repeat(41) })), None);
        assert_eq!(status_name(&json!({ "name": 7 })), None);

        let mut settings = Settings {
            role: Some(Role::Online),
            node: "http://node".to_string(),
            ..Settings::default()
        };

        assert_eq!(settings.chain_name(None), "chainv4", "no node answered yet");

        settings.pinned.insert("http://node".to_string(), "0xgenesis".to_string());
        settings.names.insert("0xgenesis".to_string(), "chainv4 testnet".to_string());
        settings.names.insert("0xother".to_string(), "another chain".to_string());

        assert_eq!(settings.chain_name(None), "chainv4 testnet", "kept for the genesis pinned at the node");
        assert_eq!(settings.chain_name(Some("chainv4 mainnet")), "chainv4 mainnet", "the node's word first");

        let kept = serde_json::from_value::<Settings>(serde_json::to_value(&settings).unwrap()).unwrap();

        assert_eq!(kept.chain_name(None), "chainv4 testnet", "and kept between runs");
        assert_eq!(serde_json::from_value::<Settings>(json!({ "node": "http://node" })).unwrap().names, HashMap::new(), "settings from before names");
        assert_eq!(Settings { role: Some(Role::Signer), ..settings }.chain_name(None), "chainv4", "a signer follows no chain");
    }

    #[test]
    fn settings_from_before_mining_mine_on_a_cpu_thread_in_the_pool_and_choices_are_kept() {
        let old = serde_json::from_value::<Settings>(json!({ "node": "http://node" })).unwrap();

        assert_eq!((old.mine_solo, old.mine_gpu, old.mine_gpus, old.mine_threads), (false, false, None, 1));
        assert_eq!((old.mine_port.as_str(), old.mine_token.as_str(), old.mine_on_battery), ("", "", false));

        let set = Settings {
            mine_solo: true,
            mine_gpu: true,
            mine_gpus: Some(1),
            mine_threads: 6,
            mine_port: "pool.example:4002".to_string(),
            mine_token: "the port's secret".to_string(),
            mine_on_battery: true,
            ..Settings::default()
        };
        let kept = serde_json::from_value::<Settings>(serde_json::to_value(&set).unwrap()).unwrap();

        assert_eq!((kept.mine_solo, kept.mine_gpu, kept.mine_gpus, kept.mine_threads), (true, true, Some(1), 6));
        assert_eq!(kept.gpus_picked(), Some(vec![1]), "the one a build before several could be picked");
        assert_eq!(Settings { mine_gpu_list: Some(vec![0, 2]), ..kept.clone() }.gpus_picked(), Some(vec![0, 2]), "a list once there is one");
        assert_eq!((kept.mine_port.as_str(), kept.mine_token.as_str(), kept.mine_on_battery), ("pool.example:4002", "the port's secret", true));
    }

    #[test]
    fn settings_of_a_build_that_mines_and_one_that_does_not_read_each_other() {
        // A build's settings are the same whichever features it has: what one that mines keeps
        // goes through one that does not untouched, and back.
        let set = Settings { mine_gpu: true, mine_threads: 6, mine_port: "pool.example:4002".to_string(), mine_on_battery: true, pool_node: "http://pool".to_string(), ..Settings::default() };
        let kept = serde_json::to_value(&set).unwrap();

        for name in ["mine_solo", "mine_gpu", "mine_gpus", "mine_threads", "mine_port", "mine_token", "mine_on_battery", "pool_node", "pool_claims"] {
            assert!(kept.get(name).is_some(), "{name} is kept in every build");
        }

        let read = serde_json::from_value::<Settings>(kept.clone()).unwrap();

        assert_eq!((read.mine_gpu, read.mine_threads, read.mine_port.as_str(), read.mine_on_battery), (true, 6, "pool.example:4002", true));
        assert_eq!(serde_json::to_value(&read).unwrap(), kept, "and written back as they came");

        // From a build with fields this one does not know, as settings that outlive a downgrade.
        let later = json!({ "node": "http://node", "mine_threads": 3, "something_from_later": { "a": 1 } });
        assert_eq!(serde_json::from_value::<Settings>(later).unwrap().mine_threads, 3);
    }

    #[test]
    fn settings_from_before_fast_resume_trust_no_node_for_it() {
        let old = serde_json::from_value::<Settings>(json!({ "node": "http://node" })).unwrap();

        assert!(old.fast_resume.is_empty(), "off, for every node");

        let mut set = Settings::default();
        set.fast_resume.insert("http://mine".to_string(), true);
        let kept = serde_json::from_value::<Settings>(serde_json::to_value(&set).unwrap()).unwrap();

        assert_eq!(kept.fast_resume.get("http://mine"), Some(&true));
        assert_eq!(kept.fast_resume.get("http://other"), None, "per node");
    }

    #[test]
    fn settings_from_before_proxies_go_direct_and_a_proxy_set_is_kept() {
        let old = serde_json::from_value::<Settings>(json!({ "node": "http://node" })).unwrap();

        assert_eq!(old.proxy, "");
        assert!(client(&old).unwrap().proxy().is_none());

        let set = Settings {
            proxy: chainv4_ui::net::Proxy::ORBOT.to_string(),
            ..Settings::default()
        };
        let kept = serde_json::from_value::<Settings>(serde_json::to_value(&set).unwrap()).unwrap();

        assert_eq!(kept.proxy, chainv4_ui::net::Proxy::ORBOT);
        assert_eq!(client(&kept).unwrap().proxy().map(chainv4_ui::net::Proxy::as_str), Some(chainv4_ui::net::Proxy::ORBOT));
        assert!(client(&Settings { proxy: "tor".to_string(), ..Settings::default() }).is_err(), "never direct instead");
    }

    #[test]
    fn settings_from_before_network_use_trade_and_reading_caps_keep_todays_values_and_choices_are_kept() {
        let old = serde_json::from_value::<Settings>(json!({ "node": "http://node", "proxy": chainv4_ui::net::Proxy::ORBOT })).unwrap();

        assert_eq!((old.low_data, old.patience, old.trade_deadline_minutes, old.reading_txs), (false, 1, 20, 20_000));
        assert_eq!(old.pace(), chainv4_ui::pace::Pace::default(), "every timer as before");
        assert_eq!(old.trade_deadline_ms(), 20 * 60_000);
        assert_eq!(client(&old).unwrap().whole(), chainv4_ui::net::limit(true), "and every wait");

        let set = Settings { low_data: true, patience: 4, trade_deadline_minutes: 5, reading_txs: 100_000, ..old };
        let kept = serde_json::from_value::<Settings>(serde_json::to_value(&set).unwrap()).unwrap();

        assert_eq!((kept.low_data, kept.patience, kept.trade_deadline_minutes, kept.reading_txs), (true, 4, 5, 100_000));
        assert!(kept.pace().low_data);
        assert_eq!(client(&kept).unwrap().whole(), chainv4_ui::net::waits(true, 4).0, "requests through the proxy wait four times as long");
        assert_eq!(kept.trade_deadline_ms(), 5 * 60_000);
        assert_eq!(kept.reading_txs(), 100_000);

        // What a settings file says past the bounds Settings offers is held to them.
        let wild = Settings { trade_deadline_minutes: 0, reading_txs: 3, ..Settings::default() };

        assert_eq!((wild.trade_deadline_ms(), wild.reading_txs()), (60_000, crate::reading::LEAST_TXS));
    }
}

