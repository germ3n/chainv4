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
/// The most proven transactions kept, the newest; older ones are proven again if asked.
const MAX_CHECKED: usize = 2_000;
/// Below this width, in points, as on a phone, the sidebar gives way to a bar on top and tabs at
/// the bottom.
const NARROW: f32 = 640.0;

/// Everything the wallet remembers between runs. The accounts are sealed; nothing else is secret.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The node's HTTP API.
    pub node: String,
    /// The genesis id trusted at each node: the one first seen there, or one pasted.
    pub pinned: HashMap<String, String>,
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
    /// What a wallet from before there could be several kept: its one vault, and the account in
    /// use. Read, to become the first wallet, and never written again.
    #[serde(skip_serializing)]
    vault: Option<Sealed>,
    #[serde(skip_serializing)]
    current: String,
}

/// A wallet on this device.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WalletEntry {
    pub name: String,
    /// Its seed phrase and accounts, sealed with its password.
    pub vault: Sealed,
    /// The account in use, by its first address.
    pub current: String,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            node: String::new(),
            pinned: HashMap::new(),
            light: HashMap::new(),
            wallets: Vec::new(),
            wallet: 0,
            sent: Vec::new(),
            checked: HashMap::new(),
            lock_after: 15,
            light_theme: false,
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
            });
            self.wallet = 0;
        }

        self.wallet = self.wallet.min(self.wallets.len().saturating_sub(1));

        self
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
        theme::install_fonts(&creation.egui_ctx);
        theme::apply(&creation.egui_ctx, &palette);
        let lock = Lock::start(&settings);

        Wallet {
            node: Node::new(&settings.node, creation.egui_ctx.clone()),
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
            wake: creation.egui_ctx.clone(),
            asked_status: None,
            asked_history: 0.0,
            active: 0.0,
            lock_room: (0.0, 0.0),
            views: views::States::default(),
        }
    }

    /// Starts afresh with a node: its own chain, light client and pinned genesis.
    fn connect(&mut self, node: &str) {
        self.settings.node = node.trim().trim_end_matches('/').to_string();
        self.node = Node::new(&self.settings.node, self.wake.clone());
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
    }

    /// Takes everything that arrived.
    fn take(&mut self, now: u64, time: f64) {
        for (ask, result) in self.node.answers() {
            let node = self.settings.node.clone();

            let trust = Trust {
                node: &node,
                pinned: &mut self.settings.pinned,
                saved: &self.settings.light,
            };

            if self.chain.take(&ask, &result, trust, now, time) {
                continue;
            }

            match (&ask, &result) {
                (Ask::Status, Ok(value)) => {
                    if let (Some(symbol), Some(decimals)) = (value["coin"]["symbol"].as_str(), value["coin"]["decimals"].as_u64()) {
                        self.coin = Coin {
                            symbol: symbol.to_string(),
                            decimals: u8::try_from(decimals).unwrap_or(18),
                        };
                    }
                }
                (Ask::Offer(_) | Ask::Submit(_), _) => {
                    if let Some(sent) = self.views.send.answered(&ask, &result, now) {
                        self.settings.sent.push(sent);
                    }
                }
                _ => {}
            }
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

    /// Asks for what is due.
    fn poll(&mut self, time: f64) {
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
                theme::caps(ui, &palette, "Light wallet");
                ui.add_space(26.0);
                ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                    opened = lock::show(ui, &palette, &mut self.settings, &mut self.lock);
                });
                ui.add_space(12.0);
                ui.label(RichText::new(format!("Node {}", self.settings.node)).small().color(palette.muted));
                ui.add_space(24.0);
            });
        });

        if let Some(Opened { vault, book }) = opened {
            self.holders = keys::open(&book);
            self.book = book;
            self.vault = Some(vault);
            self.lock = Lock::Open;
            self.page = Page::Overview;

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
                theme::caps(ui, &palette, "Light wallet");
            });
        });
        ui.add_space(24.0);

        if !self.holders.is_empty() {
            theme::caps(ui, &palette, "Account");
            self.account_picker(ui, ui.available_width());
            ui.add_space(18.0);
        }

        for (page, name) in Page::NAV {
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
            views::light_status(ui, &palette, &self.chain);
            ui.add_space(4.0);
            theme::caps(ui, &palette, "Light client");
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
            views::light_dot(ui, &palette, &self.chain);
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
        let width = ui.available_width() / Page::NAV.len() as f32;
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.spacing_mut().button_padding = egui::vec2(2.0, 4.0);
        ui.horizontal(|ui| {
            for (page, name) in Page::NAV {
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
                            wake: &self.wake,
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
        } else if changed && let Err(error) = self.store() {
            log::warn!("could not seal the accounts: {error}");
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
    }

    /// The paper's color, behind everything: what shows under a phone's status bar.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        self.palette.paper.to_normalized_gamma_f32()
    }
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
        }]);
        assert_eq!((settings.wallet, settings.current(), settings.lock_after), (0, "ed25519:0x01", 5));

        let kept = serde_json::to_value(&settings).unwrap();

        assert!(kept.get("vault").is_none() && kept.get("current").is_none(), "never written again");
        assert_eq!(serde_json::from_value::<Settings>(kept).unwrap().migrated().wallets, settings.wallets);
    }
}
