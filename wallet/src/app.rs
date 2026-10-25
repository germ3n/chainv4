//! The wallet: a vault of accounts sealed with a password, a node it talks to but never trusts, and
//! the light client that checks everything the node says. Each frame takes what arrived, asks for
//! what is due, and draws the page.

use {
    crate::chain::{Chain, Trust},
    crate::history::Checked,
    crate::node::{Ask, Node},
    crate::theme::{self, DARK, LIGHT, Palette},
    crate::vault::{Sealed, Stored, Vault, key_of},
    crate::views::{self, Ctx, Page},
    chainv4::crypto::keypair::Keypair,
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
    /// The accounts, sealed with the password.
    pub vault: Option<Sealed>,
    /// Transactions this wallet sent that are not final yet, oldest first.
    pub sent: Vec<Sent>,
    /// Transactions the light client proved, by id.
    pub checked: HashMap<String, Checked>,
    /// The account in use, by address.
    pub current: String,
    /// Minutes without input before the wallet locks itself; 0 for never.
    pub lock_after: u32,
    /// The light palette rather than the dark one.
    pub light_theme: bool,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            node: String::new(),
            pinned: HashMap::new(),
            light: HashMap::new(),
            vault: None,
            sent: Vec::new(),
            checked: HashMap::new(),
            current: String::new(),
            lock_after: 15,
            light_theme: false,
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

/// An unlocked account.
pub struct Holder {
    pub name: String,
    pub key: Keypair,
    pub address: Address,
}

/// The chain's coin, as the node names it.
#[derive(Clone, Debug)]
pub struct Coin {
    pub symbol: String,
    pub decimals: u8,
}

/// Whether the vault is open.
enum Lock {
    /// No vault yet: choosing a password.
    Setup { first: String, second: String, error: Option<String> },
    Locked { password: String, error: Option<String> },
    Open,
}

pub struct Wallet {
    settings: Settings,
    palette: Palette,
    node: Node,
    chain: Chain,
    lock: Lock,
    vault: Option<Vault>,
    holders: Vec<Holder>,
    page: Page,
    coin: Coin,
    wake: egui::Context,
    asked_status: Option<f64>,
    asked_history: f64,
    /// When the wallet last saw input, for locking itself.
    active: f64,
    views: views::States,
}

impl Wallet {
    pub fn new(creation: &eframe::CreationContext, node: Option<String>, fallback: &str) -> Wallet {
        let mut settings: Settings = creation.storage.and_then(|storage| eframe::get_value(storage, STORAGE_KEY)).unwrap_or_default();
        settings.node = node
            .or_else(|| Some(settings.node.clone()).filter(|node| !node.is_empty()))
            .unwrap_or_else(|| fallback.to_string())
            .trim()
            .trim_end_matches('/')
            .to_string();
        let palette = if settings.light_theme { LIGHT } else { DARK };
        theme::install_fonts(&creation.egui_ctx);
        theme::apply(&creation.egui_ctx, &palette);
        let lock = match settings.vault {
            Some(_) => Lock::Locked { password: String::new(), error: None },
            None => Lock::Setup {
                first: String::new(),
                second: String::new(),
                error: None,
            },
        };

        Wallet {
            node: Node::new(&settings.node, creation.egui_ctx.clone()),
            settings,
            palette,
            chain: Chain::default(),
            lock,
            vault: None,
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

    /// Seals the accounts again after a change.
    fn store(&mut self) -> Result<(), String> {
        let vault = self.vault.as_ref().ok_or("the wallet is locked")?;
        let stored: Vec<Stored> = self
            .holders
            .iter()
            .map(|holder| Stored {
                name: holder.name.clone(),
                key: holder.key.to_key_file(),
            })
            .collect();
        self.settings.vault = Some(vault.seal(&stored)?);

        Ok(())
    }

    /// The account in use: the one chosen, or else the first.
    fn current(&self) -> Option<&Holder> {
        self.holders
            .iter()
            .find(|holder| holder.address.to_string() == self.settings.current)
            .or_else(|| self.holders.first())
    }

    /// Drops the keys, and asks for the password again.
    fn lock(&mut self) {
        self.vault = None;
        self.holders.clear();
        self.views = views::States::default();
        self.lock = Lock::Locked { password: String::new(), error: None };
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

        let addresses: Vec<Address> = self.holders.iter().map(|holder| holder.address).collect();

        for ask in self.chain.proofs_due(&addresses, time) {
            self.node.ask(ask);
        }

        let Some(current) = self.current().map(|holder| holder.address.to_string()) else {
            return;
        };

        if !self.chain.caught_up() || time - self.asked_history < HISTORY_EVERY {
            return;
        }

        self.asked_history = time;

        // The address index lists everything that touched it; without one, the event index lists
        // at least what mentions it.
        match (self.chain.indexes_addresses, self.chain.indexes_events) {
            (Some(false), Some(false)) => {}
            (Some(false), _) => self.node.ask(Ask::Events(current.clone())),
            _ => self.node.ask(Ask::History(current.clone())),
        }

        let mentioned = self.chain.mentions.get(&current).into_iter().flatten().map(|mention| mention.tx.clone());
        let sent = self.settings.sent.iter().filter(|sent| sent.from == current).map(|sent| sent.id.clone());
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
        let mut opened: Option<Result<(Vault, Vec<Stored>), String>> = None;

        ui.vertical_centered(|ui| {
            ui.add_space((ui.available_height() * 0.2).max(24.0));
            ui.set_max_width(380.0);
            ui.label(RichText::new("chainv4").font(egui::FontId::new(44.0, egui::FontFamily::Name(theme::SERIF.into()))).color(palette.ink));
            theme::caps(ui, &palette, "Light wallet");
            ui.add_space(26.0);

            theme::card(ui, &palette, |ui| match &mut self.lock {
                Lock::Setup { first, second, error } => {
                    theme::caps(ui, &palette, "Choose a password");
                    ui.label(RichText::new("It seals your keys on this device. Nothing opens them without it, so keep it safe.").color(palette.muted));
                    ui.add_space(8.0);
                    ui.add(egui::TextEdit::singleline(first).password(true).hint_text("password").desired_width(f32::INFINITY));
                    let enter = ui
                        .add(egui::TextEdit::singleline(second).password(true).hint_text("the same again").desired_width(f32::INFINITY))
                        .lost_focus()
                        && ui.input(|input| input.key_pressed(egui::Key::Enter));

                    if let Some(error) = error {
                        ui.colored_label(palette.bad, error.as_str());
                    }

                    ui.add_space(6.0);

                    if theme::primary(ui, &palette, "Create wallet", !first.is_empty()).clicked() || enter {
                        opened = Some(match (first.len() >= 8, first == second) {
                            (false, _) => Err("use at least 8 characters".to_string()),
                            (_, false) => Err("the two do not match".to_string()),
                            (true, true) => Vault::create(first).map(|vault| (vault, Vec::new())),
                        });
                    }
                }
                Lock::Locked { password, error } => {
                    theme::caps(ui, &palette, "Unlock");
                    let field = ui.add(egui::TextEdit::singleline(password).password(true).hint_text("password").desired_width(f32::INFINITY));
                    let enter = field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));

                    if let Some(error) = error {
                        ui.colored_label(palette.bad, error.as_str());
                    }

                    ui.add_space(6.0);

                    if theme::primary(ui, &palette, "Unlock", !password.is_empty()).clicked() || enter {
                        opened = Some(match &self.settings.vault {
                            Some(sealed) => Vault::open(sealed, password),
                            None => Err("there is no vault".to_string()),
                        });
                    }
                }
                Lock::Open => {}
            });

            ui.add_space(12.0);
            ui.label(RichText::new(format!("Node {}", self.settings.node)).small().color(palette.muted));
        });

        match opened {
            Some(Ok((vault, stored))) => {
                self.holders = stored
                    .iter()
                    .filter_map(|stored| {
                        let key = key_of(stored).ok()?;

                        Some(Holder {
                            name: stored.name.clone(),
                            address: key.public().address(),
                            key,
                        })
                    })
                    .collect();
                self.vault = Some(vault);
                self.lock = Lock::Open;
                self.page = Page::Overview;

                if self.settings.vault.is_none() {
                    let _ = self.store();
                    self.page = Page::Accounts;
                }
            }
            Some(Err(error)) => match &mut self.lock {
                Lock::Setup { error: shown, .. } | Lock::Locked { error: shown, .. } => *shown = Some(error),
                Lock::Open => {}
            },
            None => {}
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

        if let Some(current) = self.current() {
            let (address, name) = (current.address.to_string(), current.name.clone());
            theme::caps(ui, &palette, "Account");
            egui::ComboBox::from_id_salt("account").selected_text(name).width(ui.available_width()).show_ui(ui, |ui| {
                for holder in &self.holders {
                    let text = format!("{}  ·  {}", holder.name, crate::format::short(&holder.address.to_string()));
                    ui.selectable_value(&mut self.settings.current, holder.address.to_string(), text);
                }
            });

            if self.settings.current.is_empty() {
                self.settings.current = address;
            }

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
                    self.palette = if self.settings.light_theme { LIGHT } else { DARK };
                    theme::apply(ui.ctx(), &self.palette);
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
}

impl eframe::App for Wallet {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let time = ui.input(|input| input.time);
        let now = now_ms();
        self.take(now, time);
        self.poll(time);
        let palette = self.palette;

        if !matches!(self.lock, Lock::Open) {
            egui::CentralPanel::default().frame(egui::Frame::new().fill(palette.paper)).show(ui, |ui| self.lock_screen(ui));
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

        egui::Panel::left("nav")
            .exact_size(236.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(palette.nav).inner_margin(egui::Margin::symmetric(16, 0)).stroke(egui::Stroke::new(1.0, palette.line)))
            .show(ui, |ui| self.sidebar(ui));

        let mut connect = None;
        let mut changed = false;
        let current = self
            .holders
            .iter()
            .position(|holder| holder.address.to_string() == self.settings.current)
            .unwrap_or(0);
        egui::CentralPanel::default().frame(egui::Frame::new().fill(palette.paper)).show(ui, |ui| {
            egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.set_max_width(760.0);
                    ui.add_space(34.0);
                    ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                        let mut ctx = Ctx {
                            palette: &palette,
                            chain: &self.chain,
                            node: &mut self.node,
                            holders: &mut self.holders,
                            current,
                            coin: &self.coin,
                            settings: &mut self.settings,
                            now,
                            page: &mut self.page,
                            connect: &mut connect,
                            changed: &mut changed,
                            vault: &mut self.vault,
                            wake: &self.wake,
                        };
                        views::show(ui, &mut ctx, &mut self.views);
                    });
                    ui.add_space(40.0);
                });
            });
        });

        if changed && let Err(error) = self.store() {
            log::warn!("could not seal the accounts: {error}");
        }

        if let Some(node) = connect {
            self.connect(&node);
        }

        // What is due comes due even when nothing arrives.
        ui.ctx().request_repaint_after(Duration::from_millis(400));
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, STORAGE_KEY, &self.settings);
    }
}

fn now_ms() -> u64 {
    web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map_or(0, |since| since.as_millis() as u64)
}
