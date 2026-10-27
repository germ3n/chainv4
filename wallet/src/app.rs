//! The wallet: vaults of accounts, each sealed with a password of its own, a node it talks to but
//! never trusts, and the light client that checks everything the node says. Each frame takes what
//! arrived, asks for what is due, and draws the page.

use {
    crate::chain::{Chain, Trust},
    crate::history::Checked,
    crate::keys::{self, Holder},
    crate::lock::{self, Lock, Opened},
    crate::node::{Ask, Node},
    crate::theme::{self, DARK, LIGHT, Palette},
    crate::vault::{Book, Sealed, StoredAccount, Vault},
    crate::views::{self, Ctx, Page},
    chainv4::structs::address::Address,
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
    /// The wallets on this device, each its accounts sealed with a password of its own.
    pub wallets: Vec<WalletEntry>,
    /// The wallet last opened, by its place among them.
    pub wallet: usize,
    /// Transactions this wallet sent that are not final yet, oldest first.
    pub sent: Vec<Sent>,
    /// Transactions the light client proved, by id.
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
    /// The IPFS gateway ipfs:// URIs are fetched through, ending in a slash.
    pub ipfs_gateway: String,
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
    pub name: String,
    /// Its seed phrase and accounts, sealed with its password.
    pub vault: Sealed,
    /// The account in use, by its first address.
    pub current: String,
    /// Its vault's key, kept behind biometrics, as the platform keeps it: None while they are off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub biometric: Option<String>,
    /// Whether it is sealed with no password, so it opens with a tap, and the lock screen says
    /// anyone who reads app storage could.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unprotected: bool,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            role: None,
            node: String::new(),
            proxy: String::new(),
            pinned: HashMap::new(),
            checkpoints: HashMap::new(),
            light: HashMap::new(),
            wallets: Vec::new(),
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
            // chainv4::nft::DEFAULT_GATEWAY, which a build without NFTs has no copy of.
            ipfs_gateway: "https://ipfs.io/ipfs/".to_string(),
            vault: None,
            current: String::new(),
        }
    }
}

impl Settings {
    /// Settings as kept, with the vault of a wallet from before there could be several made the
    /// first wallet.
    fn migrated(mut self) -> Settings {
        if let Some(vault) = self.vault.take() {
            self.wallets.insert(0, WalletEntry {
                name: "Wallet 1".to_string(),
                vault,
                current: std::mem::take(&mut self.current),
                biometric: None,
                unprotected: false,
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
    coin: Coin,
    /// The node's faucet, as its status describes it, on a node that runs one.
    faucet: Option<serde_json::Value>,
    /// The swap contract the node names in GET /status, if it names one.
    swap_contract: Option<String>,
    /// The DEX, and the EVM DEX's router, the node names in GET /status.
    dex: (Option<String>, Option<String>),
    /// The Bitcoin light clients the settings ask for.
    #[cfg(all(feature = "bitcoin-light", not(target_arch = "wasm32")))]
    lights: crate::light::Lights,
    wake: egui::Context,
    asked_status: Option<f64>,
    asked_history: f64,
    /// When the wallet last saw input, for locking itself.
    active: f64,
    /// The lock screen's width, and the tallest it has been at it: what places its card, so a
    /// keyboard taking the bottom of the screen does not move the field under a finger.
    lock_room: (f32, f32),
    views: views::States,
}

impl Wallet {
    pub fn new(creation: &eframe::CreationContext, node: Option<String>, fallback: &str) -> Wallet {
        let mut settings = creation
            .storage
            .and_then(|storage| eframe::get_value::<Settings>(storage, STORAGE_KEY))
            .unwrap_or_default()
            .migrated();
        settings.node = node
            .or_else(|| Some(settings.node.clone()).filter(|node| !node.is_empty()))
            .unwrap_or_else(|| fallback.to_string())
            .trim()
            .trim_end_matches('/')
            .to_string();
        let palette = if settings.light_theme { LIGHT } else { DARK };
        announce(&settings.node);
        theme::install_fonts(&creation.egui_ctx);
        theme::apply(&creation.egui_ctx, &palette);
        let lock = Lock::start(&settings);

        Wallet {
            node: Node::new(&settings.node, client(&settings), creation.egui_ctx.clone()),
            settings,
            palette,
            chain: Chain::default(),
            lock,
            vault: None,
            book: Book::default(),
            holders: Vec::new(),
            page: Page::Overview,
            coin: Coin {
                symbol: "DEV".to_string(),
                decimals: 18,
            },
            faucet: None,
            swap_contract: None,
            dex: (None, None),
            #[cfg(all(feature = "bitcoin-light", not(target_arch = "wasm32")))]
            lights: crate::light::Lights {
                kept: creation.storage.and_then(|storage| eframe::get_value(storage, BITCOIN_KEY)).unwrap_or_default(),
                ..Default::default()
            },
            wake: creation.egui_ctx.clone(),
            asked_status: None,
            asked_history: 0.0,
            active: 0.0,
            lock_room: (0.0, 0.0),
            views: views::States::default(),
        }
    }

    /// Starts afresh with a node, through the proxy set: its own chain, light client and pinned
    /// genesis.
    fn connect(&mut self, node: &str) {
        self.settings.node = node.trim().trim_end_matches('/').to_string();
        announce(&self.settings.node);
        self.node = Node::new(&self.settings.node, client(&self.settings), self.wake.clone());
        self.chain = Chain::default();
        self.asked_status = None;
        self.asked_history = 0.0;
    }

    /// Seals the wallet's accounts again after a change.
    fn store(&mut self) -> Result<(), String> {
        let vault = self.vault.as_ref().ok_or("the wallet is locked")?;
        self.book.accounts = self
            .holders
            .iter()
            .map(|holder| StoredAccount {
                name: holder.name.clone(),
                source: holder.source.clone(),
            })
            .collect();
        let sealed = vault.seal(&self.book)?;
        self.settings.wallets.get_mut(self.settings.wallet).ok_or("the wallet is gone")?.vault = sealed;

        Ok(())
    }

    /// The account in use: the one chosen, or else the first.
    fn current(&self) -> Option<&Holder> {
        let chosen = self.settings.current();

        self.holders.iter().find(|holder| holder.id() == chosen).or_else(|| self.holders.first())
    }

    /// Drops the keys, and asks for the password again.
    fn lock(&mut self) {
        self.vault = None;
        self.book = Book::default();
        self.holders.clear();
        self.views = views::States::default();
        self.lock = Lock::start(&self.settings);

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
            };

            if self.chain.take(&ask, &result, trust, now, time) {
                continue;
            }

            match (&ask, &result) {
                (Ask::Status, Ok(value)) => {
                    self.faucet = value.get("faucet").filter(|faucet| !faucet.is_null()).cloned();
                    self.swap_contract = value["swap"]["contract"].as_str().map(str::to_string);
                    self.dex = (value["dex"]["contract"].as_str().map(str::to_string), value["dex"]["router"].as_str().map(str::to_string));

                    if let (Some(symbol), Some(decimals)) = (value["coin"]["symbol"].as_str(), value["coin"]["decimals"].as_u64()) {
                        self.coin = Coin {
                            symbol: symbol.to_string(),
                            decimals: u8::try_from(decimals).unwrap_or(18),
                        };
                    }
                }
                (Ask::Contract(_) | Ask::Call(_) | Ask::Read(_), _) => self.views.answered(&ask, &result, &self.settings.contracts),
                #[cfg(feature = "dex")]
                (Ask::Rpc(body), _) if self.views.trade.answered_rpc(body, &result) => {}
                (Ask::Rpc(body), _) => self.views.dapp.answered(body, &result),
                #[cfg(feature = "dex")]
                (Ask::Named { .. }, _) => self.views.trade.events(&result),
                (Ask::Faucet(_), _) => self.views.faucet.answered(&ask, &result),
                #[cfg(feature = "nft")]
                (Ask::Collections, _) => self.views.nfts.collections(&result),
                (Ask::Offer(_) | Ask::Submit(_), _) => {
                    if let Some(sent) = self.views.send.answered(&ask, &result, now) {
                        self.settings.sent.push(sent);
                    }
                }
                _ => {}
            }
        }

        #[cfg(feature = "nft")]
        for (url, result) in self.node.fetched() {
            self.views.nfts.fetched(&url, result, self.settings.ipfs_gateway.trim());
        }

        // What was proven is kept, so it shows without proving it again.
        for (tx, proven) in std::mem::take(&mut self.chain.receipts) {
            self.settings.checked.insert(tx, Checked::of(&proven));
        }

        if self.settings.checked.len() > MAX_CHECKED {
            let mut heights: Vec<u64> = self.settings.checked.values().map(|checked| checked.height).collect();
            heights.sort_unstable();
            let lowest = heights[heights.len() - MAX_CHECKED];
            self.settings.checked.retain(|_, checked| checked.height >= lowest);
        }

        // Sent ones that are final are the history's now; the log keeps only what waits.
        let checked = &self.settings.checked;
        self.settings.sent.retain(|sent| !checked.get(&sent.id).is_some_and(|checked| checked.final_));

        if let Some((genesis, state)) = self.chain.saved() {
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
        let scripts = |net: Net| crate::light::scripts(holders, book, net.testnet());
        self.lights.keep(wanted, &self.settings.proxy, &self.settings.bitcoin_peer, scripts, &self.wake);
    }

    /// Asks for what is due. An offline signer asks for nothing, nor an install before it says
    /// what it is: it may be about to become one.
    fn poll(&mut self, time: f64) {
        if matches!(self.settings.role, None | Some(Role::Signer)) {
            return;
        }

        if let Some(ask) = self.chain.next_ask(time) {
            self.node.ask(ask);
        }

        if self.asked_status.is_none_or(|asked| time - asked >= STATUS_EVERY) {
            self.asked_status = Some(time);
            self.node.ask(Ask::Status);
        }

        let addresses: Vec<Address> = self.holders.iter().flat_map(Holder::addresses).collect();

        for ask in self.chain.proofs_due(&addresses, time) {
            self.node.ask(ask);
        }

        #[cfg_attr(not(any(feature = "swap", feature = "dex")), allow(unused_mut))]
        let mut wanted = self.views.wanted_names();

        // The swap name, proven, for new offers to find the default swap contract by.
        #[cfg(feature = "swap")]
        if self.page == views::Page::Swaps {
            wanted.extend(views::swap_name(&self.chain));
        }

        // The DEX's name, proven, for the Trade page to find it by.
        #[cfg(feature = "dex")]
        if self.page == views::Page::Trade {
            wanted.extend(views::dex_name(&self.chain));
        }

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

        if !self.chain.caught_up() || time - self.asked_history < HISTORY_EVERY {
            return;
        }

        self.asked_history = time;

        // The address index lists everything that touched an address; without one, the event index
        // lists at least what mentions it.
        for address in &current {
            match (self.chain.indexes_addresses, self.chain.indexes_events) {
                (Some(false), Some(false)) => {}
                (Some(false), _) => self.node.ask(Ask::Events(address.clone())),
                _ => self.node.ask(Ask::History(address.clone())),
            }
        }

        let mentioned = current.iter().flat_map(|address| self.chain.mentions.get(address).into_iter().flatten()).map(|mention| mention.tx.clone());
        let sent = self.settings.sent.iter().filter(|sent| current.contains(&sent.from)).map(|sent| sent.id.clone());
        let unsettled: Vec<String> = sent
            .chain(mentioned)
            // Final ones are settled; the rest are proven again until they are.
            .filter(|tx| !self.settings.checked.get(tx).is_some_and(|checked| checked.final_))
            .take(40)
            .collect();

        for tx in unsettled {
            self.node.ask(Ask::Receipt(tx));
        }
    }

    /// Tells the browser extension what the wallet is, for the sites it answers, and takes what
    /// they ask, once the wallet is open. Nothing outside the extension.
    fn sites(&mut self) {
        if !crate::dapp::available() || self.settings.role() != Role::Online {
            return;
        }

        let unlocked = matches!(self.lock, Lock::Open);
        let accounts: Vec<[u8; 20]> = crate::views::dapp::offered(&self.holders).into_iter().map(|(_, account)| account).collect();
        let chain = self.chain.genesis.map(|genesis| chainv4::crypto::ethereum::chain_id(&genesis));
        crate::dapp::report(&crate::dapp::state(&self.settings.node, chain, &accounts, unlocked));

        if unlocked {
            self.views.dapp.take(crate::dapp::pending());
        }
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
            ui.vertical_centered(|ui| {
                ui.add_space((tallest * 0.16).max(24.0));
                ui.set_max_width(ui.available_width().min(380.0));
                ui.label(RichText::new("chainv4").font(egui::FontId::new(44.0, egui::FontFamily::Name(theme::SERIF.into()))).color(palette.ink));
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
                ui.add_space(24.0);
            });
        });

        if let Some(Opened { vault, book }) = opened {
            self.holders = keys::open(&book);
            self.book = book;
            self.vault = Some(vault);
            self.lock = Lock::Open;
            self.page = views::home(self.settings.role());

            if let Some(first) = self.holders.first().map(Holder::id)
                && !self.holders.iter().any(|holder| holder.id() == self.settings.current())
            {
                self.settings.set_current(first);
            }
        }
    }

    fn sidebar(&mut self, ui: &mut Ui) {
        let palette = self.palette;
        ui.add_space(26.0);
        ui.horizontal(|ui| {
            ui.add_space(6.0);
            ui.vertical(|ui| {
                ui.label(RichText::new("chainv4").font(egui::FontId::new(26.0, egui::FontFamily::Name(theme::SERIF.into()))).color(palette.ink));
                theme::caps(ui, &palette, match (self.settings.role(), self.book.watch) {
                    (Role::Signer, _) => "Offline signer",
                    (Role::Online, true) => "Watch-only",
                    (Role::Online, false) => "Light wallet",
                });
            });
        });
        ui.add_space(24.0);

        if !self.holders.is_empty() {
            theme::caps(ui, &palette, "Account");
            self.account_picker(ui, ui.available_width());
            ui.add_space(18.0);
        }

        for &(page, name) in views::nav(self.settings.role()) {
            let selected = self.page == page;
            let text = theme::strong(name, 14.0).color(if selected { palette.ink } else { palette.soft });
            let button = egui::Button::new(text)
                .fill(if selected { palette.row } else { egui::Color32::TRANSPARENT })
                .stroke(egui::Stroke::NONE)
                .corner_radius(8.0)
                .min_size(egui::vec2(ui.available_width(), 34.0));
            let response = ui.add(button);

            if selected {
                let rect = response.rect;
                ui.painter()
                    .rect_filled(egui::Rect::from_min_size(rect.left_top() + egui::vec2(0.0, 8.0), egui::vec2(3.0, rect.height() - 16.0)), 2.0, palette.accent);
            }

            if response.clicked() {
                self.page = page;
            }
        }

        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            ui.add_space(18.0);
            ui.horizontal(|ui| {
                let theme_name = if self.settings.light_theme { "Dark" } else { "Light" };

                if theme::quiet(ui, &palette, theme_name).clicked() {
                    self.settings.light_theme = !self.settings.light_theme;
                }

                if theme::quiet(ui, &palette, "Lock").clicked() {
                    self.lock();
                }
            });
            ui.add_space(10.0);

            // A signer follows no chain: it says so instead.
            match self.settings.role() {
                Role::Signer => {
                    ui.label(RichText::new("Never online: what it signs goes by QR code.").small().color(palette.muted));
                    ui.add_space(4.0);
                    theme::caps(ui, &palette, "Offline");
                }
                _ => {
                    views::light_status(ui, &palette, &self.chain);
                    ui.add_space(4.0);
                    theme::caps(ui, &palette, "Light client");
                }
            }
        });
    }

    /// Which account is in use, to pick from.
    fn account_picker(&mut self, ui: &mut Ui, width: f32) {
        let Some(current) = self.current() else {
            return;
        };
        let mut chosen = current.id();
        egui::ComboBox::from_id_salt("account").selected_text(current.name.clone()).width(width).show_ui(ui, |ui| {
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
            ui.label(RichText::new("chainv4").font(egui::FontId::new(21.0, egui::FontFamily::Name(theme::SERIF.into()))).color(palette.ink));

            if self.settings.role() != Role::Signer {
                views::light_dot(ui, &palette, &self.chain);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::quiet(ui, &palette, "Lock").clicked() {
                    self.lock();
                }

                let width = (ui.available_width() - 8.0).clamp(90.0, 180.0);
                self.account_picker(ui, width);
            });
        });
    }

    /// On a narrow screen, the pages, as tabs along the bottom.
    fn tabs(&mut self, ui: &mut Ui) {
        let palette = self.palette;
        let nav = views::nav(self.settings.role());
        let width = ui.available_width() / nav.len() as f32;
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.spacing_mut().button_padding = egui::vec2(2.0, 4.0);
        ui.horizontal(|ui| {
            for &(page, name) in nav {
                let selected = self.page == page;
                let name = if page == Page::Overview { "Home" } else { name };
                let text = theme::strong(name, 12.0).color(if selected { palette.ink } else { palette.muted });
                let button = egui::Button::new(text).fill(egui::Color32::TRANSPARENT).stroke(egui::Stroke::NONE);
                let response = ui.add_sized([width, 46.0], button);

                if selected {
                    let rect = response.rect;
                    ui.painter()
                        .rect_filled(egui::Rect::from_center_size(egui::pos2(rect.center().x, rect.top() + 1.5), egui::vec2(28.0, 3.0)), 2.0, palette.accent);
                }

                if response.clicked() {
                    self.page = page;
                }
            }
        });
    }
}

impl eframe::App for Wallet {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let time = ui.input(|input| input.time);
        let now = now_ms();
        self.take(now, time);
        self.poll(time);
        #[cfg(all(feature = "bitcoin-light", not(target_arch = "wasm32")))]
        self.follow_bitcoin();
        self.sites();
        chainv4_ui::layout::suit_touch(ui.ctx());
        let palette = self.palette;

        if !matches!(self.lock, Lock::Open) {
            egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(palette.paper).inner_margin(egui::Margin::symmetric(14, 0)))
            .show(ui, |ui| self.lock_screen(ui));
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

        let narrow = ui.available_width() < NARROW;
        let bar = egui::Frame::new().fill(palette.nav).stroke(egui::Stroke::new(1.0, palette.line));

        match narrow {
            true => {
                egui::Panel::top("bar").frame(bar.inner_margin(egui::Margin::symmetric(14, 10))).show(ui, |ui| self.top_bar(ui));
                egui::Panel::bottom("tabs").frame(bar).show(ui, |ui| self.tabs(ui));
            }
            false => {
                egui::Panel::left("nav")
                    .exact_size(236.0)
                    .resizable(false)
                    .frame(bar.inner_margin(egui::Margin::symmetric(16, 0)))
                    .show(ui, |ui| self.sidebar(ui));
            }
        }

        let mut connect = None;
        let mut changed = false;
        let mut leave = false;
        let mut lock = None;
        let chosen = self.settings.current().to_string();
        let current = self.holders.iter().position(|holder| holder.id() == chosen).unwrap_or(0);
        let margin = if narrow { egui::Margin::symmetric(14, 0) } else { egui::Margin::ZERO };
        egui::CentralPanel::default().frame(egui::Frame::new().fill(palette.paper).inner_margin(margin)).show(ui, |ui| {
            egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    // Never wider than there is room for: a centered column widens to its width.
                    ui.set_max_width(ui.available_width().min(760.0));
                    ui.add_space(if narrow { 18.0 } else { 34.0 });
                    ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                        let mut ctx = Ctx {
                            palette: &palette,
                            chain: &self.chain,
                            node: &mut self.node,
                            holders: &mut self.holders,
                            current,
                            book: &mut self.book,
                            coin: &self.coin,
                            settings: &mut self.settings,
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
                            #[cfg(all(feature = "bitcoin-light", not(target_arch = "wasm32")))]
                            lights: &mut self.lights,
                        };
                        views::show(ui, &mut ctx, &mut self.views);
                    });
                    ui.add_space(40.0);
                });
            });
        });

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
        ui.ctx().request_repaint_after(Duration::from_millis(400));
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, STORAGE_KEY, &self.settings);

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
        (_, true) => Ok(chainv4_ui::net::Client::direct()),
        (_, false) => chainv4_ui::net::Client::for_setting(&settings.proxy),
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
    fn the_vault_of_a_wallet_from_before_several_becomes_the_first() {
        let sealed = Sealed {
            salt: "0x00".to_string(),
            nonce: "0x01".to_string(),
            sealed: "0x02".to_string(),
        };
        let old = json!({ "node": "http://node", "vault": sealed, "current": "ed25519:0x01", "lock_after": 5 });
        let settings = serde_json::from_value::<Settings>(old).unwrap().migrated();

        assert_eq!(settings.wallets, vec![WalletEntry {
            name: "Wallet 1".to_string(),
            vault: sealed,
            current: "ed25519:0x01".to_string(),
            biometric: None,
            unprotected: false,
        }]);
        assert_eq!((settings.wallet, settings.current(), settings.lock_after), (0, "ed25519:0x01", 5));

        let kept = serde_json::to_value(&settings).unwrap();

        assert!(kept.get("vault").is_none() && kept.get("current").is_none(), "never written again");
        assert_eq!(serde_json::from_value::<Settings>(kept).unwrap().migrated().wallets, settings.wallets);
    }

    #[test]
    fn a_wallets_biometrics_are_kept_with_it_and_absent_while_off() {
        let entry = WalletEntry {
            name: "Main".to_string(),
            vault: Sealed {
                salt: "0x00".to_string(),
                nonce: "0x01".to_string(),
                sealed: "0x02".to_string(),
            },
            current: String::new(),
            biometric: None,
            unprotected: false,
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
}
