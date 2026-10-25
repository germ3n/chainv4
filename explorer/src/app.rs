//! The explorer: one node's API, everything learned from it, and the page being looked at. Each
//! frame takes what arrived, asks for what is due, and draws the page.
//!
//! What the node pushes over its stream lands at once. Without a stream, or besides it, the
//! explorer polls: the status every second or so, and whatever the page in view shows.

use {
    crate::api::{Api, Ask, News, Reply},
    crate::format,
    crate::light::Light,
    crate::model::{
        BlockDetail, Called, Checkpoint, Contract, EventsPage, Forks, TxsPage, Mempool, NodeView, Page, PeerChange, Receipt, Reorganized,
        Status, Summary, TokensPage, TxView,
    },
    crate::store::{PAGE, Store},
    crate::theme::{self, DARK, LIGHT, Roles},
    crate::views::{
        self, Ctx, Look, Settings, View, account::AccountState, hops::HopState, map::MapState, network::Streaming, send::SendState,
        settings::SettingsState, tokens::TokensState,
    },
    chainv4_ui::{Palette, layout, widgets},
    egui::{RichText, Ui},
    serde_json::Value,
    std::time::Duration,
};

/// How wide the sidebar is, and the widest the page column grows on a big screen.
const SIDEBAR: f32 = 212.0;
const COLUMN: f32 = 1400.0;

/// Seconds between asking the same thing again.
const STATUS_POLLED: f64 = 1.0;
const STATUS_STREAMED: f64 = 3.0;
const NODE_EVERY: f64 = 2.0;
const PAGE_EVERY: f64 = 2.5;
const FORKS_EVERY: f64 = 5.0;
/// How long a failure stands before what failed is asked again.
const RETRY_AFTER: f64 = 3.0;
/// How long after the stream drops it is opened again.
const STREAM_RETRY: f64 = 10.0;

/// When each thing was last asked for.
#[derive(Default)]
struct Asked {
    status: f64,
    node: f64,
    page: f64,
    forks: f64,
    failures: f64,
    stream: f64,
}

pub struct Explorer {
    settings: Settings,
    /// The node address being typed.
    address: String,
    wake: egui::Context,
    api: Api,
    store: Store,
    view: View,
    back: Vec<View>,
    ahead: Vec<View>,
    search: String,
    streaming: Streaming,
    asked: Asked,
    map: MapState,
    hops: HopState,
    account: AccountState,
    tokens: TokensState,
    send: SendState,
    settings_page: SettingsState,
    /// The light client, which checks what the node says about balances.
    light: Light,
    /// Something to tell the person, and whether it is bad news.
    notice: Option<(String, bool)>,
    palette: Palette,
    /// Whether the page changed, so the next one starts at its top.
    moved: bool,
}

impl Explorer {
    /// The explorer, talking to node when given, or else to the node it talked to last.
    pub fn new(creation: &eframe::CreationContext, node: Option<String>, fallback: &str) -> Explorer {
        let settings: Settings = creation
            .storage
            .and_then(|storage| eframe::get_value(storage, eframe::APP_KEY))
            .unwrap_or_default();
        let palette = if settings.light_theme { LIGHT } else { DARK };
        theme::install_fonts(&creation.egui_ctx);
        theme::apply(&creation.egui_ctx, &palette);
        let node = node
            .or_else(|| Some(settings.node.clone()).filter(|node| !node.is_empty()))
            .unwrap_or_else(|| fallback.to_string());
        let wake = creation.egui_ctx.clone();
        let api = Api::new(&node, client(&settings), wake.clone());
        let mut explorer = Explorer {
            settings,
            address: node.clone(),
            api,
            wake,
            store: Store::default(),
            view: View::Overview,
            back: Vec::new(),
            ahead: Vec::new(),
            search: String::new(),
            streaming: Streaming::Off,
            asked: Asked::default(),
            map: MapState::default(),
            hops: HopState::default(),
            account: AccountState::default(),
            tokens: TokensState::default(),
            send: SendState::default(),
            settings_page: SettingsState::default(),
            light: Light::default(),
            notice: None,
            palette,
            moved: false,
        };
        explorer.connect(&node);

        explorer
    }

    /// Starts afresh with another node, or the same one through another proxy.
    fn connect(&mut self, node: &str) {
        let node = node.trim().trim_end_matches('/').to_string();
        self.api = Api::new(&node, client(&self.settings), self.wake.clone());
        self.store = Store::default();
        self.map = MapState::default();
        self.light = Light::default();
        self.asked = Asked::default();
        self.settings.node = node.clone();
        self.address = node;
        self.streaming = Streaming::Connecting;
        self.api.listen();
    }

    fn go(&mut self, view: View) {
        if view != self.view {
            self.back.push(std::mem::replace(&mut self.view, view));
            self.ahead.clear();
            self.moved = true;
        }
    }

    /// Takes everything that arrived.
    fn take(&mut self, now: u64, time: f64) {
        for reply in self.api.replies() {
            match reply {
                Reply::Answer { ask, result } => {
                    if self.light.answered(&ask, &result, &mut self.settings.pinned, &self.settings.node, now, time) {
                        continue;
                    }

                    self.send.answered(&ask, &result, &mut self.api);

                    match result {
                        Ok(value) => {
                            self.store.failures.remove(&ask);
                            self.answered(ask, value, now);
                        }
                        Err(failure) => {
                            match &ask {
                                // Not a block: perhaps a transaction.
                                Ask::Lookup(id) if failure.status == 404 => self.go(View::Tx(id.clone())),
                                Ask::Contract(address) if failure.status == 404 => {
                                    self.store.contracts.insert(address.clone(), None);
                                }
                                _ => {}
                            }

                            self.store.failures.insert(ask, failure);
                        }
                    }
                }
                Reply::Stream(news) => self.news(news, now, time),
            }
        }
    }

    fn answered(&mut self, ask: Ask, value: Value, now: u64) {
        let store = &mut self.store;

        match ask {
            Ask::Status => {
                if let Some(status) = parse::<Status>(value) {
                    if store.status.as_ref().is_some_and(|known| known.genesis != status.genesis) {
                        // Another chain behind the same address: start over.
                        *store = Store::default();
                    }

                    store.status = Some(status);
                }
            }
            Ask::Node => store.node = parse::<NodeView>(value),
            Ask::Mempool => store.mempool = parse::<Mempool>(value),
            Ask::Forks => {
                if let Some(forks) = parse::<Forks>(value) {
                    store.forks(forks);
                }
            }
            Ask::Blocks { .. } => {
                if let Some(page) = parse::<Page>(value)
                    && let Some(before) = store.page(page)
                {
                    self.api.ask(Ask::Blocks {
                        before: Some(before),
                        limit: PAGE,
                    });
                }
            }
            Ask::Block(_) => {
                if let Some(detail) = parse::<BlockDetail>(value) {
                    store.detail(detail);
                }
            }
            Ask::Lookup(id) => {
                if let Some(detail) = parse::<BlockDetail>(value) {
                    store.detail(detail);
                    self.go(View::Block(id));
                }
            }
            Ask::Tx(id) => {
                if let Some(receipt) = parse::<Receipt>(value) {
                    store.receipts.insert(id, receipt);
                }
            }
            Ask::Account(address) => {
                if let Some(account) = parse(value) {
                    store.accounts.insert(address, account);
                }
            }
            Ask::Contract(address) => {
                let contract = parse::<Contract>(value);

                // The registry has the last word on whether it is a token: an upgrade may have
                // taken it off.
                match contract.as_ref().and_then(|contract| contract.token.clone()) {
                    Some(token) => store.tokens.insert(address.clone(), token),
                    None => store.tokens.remove(&address),
                };
                store.contracts.insert(address, contract);
            }
            Ask::Tokens { symbol, cursor } => {
                if let Some(page) = parse::<TokensPage>(value) {
                    for token in &page.tokens {
                        store.tokens.insert(token.address.clone(), token.clone());
                    }

                    store.token_lists.entry(symbol).or_default().take(page, cursor.is_some(), now);
                }
            }
            Ask::Validators => store.validators = Some(value),
            Ask::Signers => store.signers = Some(value),
            Ask::Evidence => store.evidence = Some(value),
            Ask::Call { ref contract, ref function, ref args } => {
                if function == "balance_of"
                    && let Some(called) = parse::<Called>(value.clone())
                    && called.status == "returned"
                    && let Some(first) = called.values.first()
                {
                    let holder = serde_json::from_str::<Value>(args)
                        .ok()
                        .and_then(|args| args.get(0)?.get("value")?.as_str().map(str::to_string));

                    if let Some(holder) = holder {
                        let raw = first.text();
                        let raw = raw.split('.').next().unwrap_or_default().to_string();
                        store.holdings.insert((contract.clone(), holder), raw);
                    }
                }

                store.calls.insert(ask.clone(), value);
            }
            Ask::Events { query, cursor } => {
                if let Some(page) = parse::<EventsPage>(value) {
                    store.events.entry(query).or_default().take(page, cursor.is_some(), now);
                }
            }
            Ask::AccountTxs { address, cursor } => {
                if let Some(page) = parse::<TxsPage>(value) {
                    store.account_txs.entry(address).or_default().take(page, cursor.is_some(), now);
                }
            }
            Ask::Offer(_) | Ask::Submit(_) => {}
            // The light client's own, which it took already.
            Ask::Light | Ask::Headers { .. } | Ask::LightUpdate(_) | Ask::Justifications(_) | Ask::Proof { .. } => {}
        }
    }

    fn news(&mut self, news: News, now: u64, time: f64) {
        let store = &mut self.store;

        match news {
            News::Open => self.streaming = Streaming::Live,
            News::Closed { why, unsupported } => {
                self.streaming = match unsupported {
                    true => Streaming::Unsupported,
                    false => Streaming::Lost(why),
                };
                self.asked.stream = time;
            }
            News::Event { name, data } => match name.as_str() {
                "block" => {
                    if let Some(block) = parse::<Summary>(data) {
                        let text = format!("{} · {} txs · {}", block.height, block.tx_count, format::short(&block.id));

                        match block.main {
                            Some(true) => store.main_block(block),
                            _ => store.side_block(block),
                        }

                        store.live(now, "block", text);
                    }
                }
                "reorganized" => {
                    if let Some(reorg) = parse::<Reorganized>(data) {
                        let text = format!("took off {}, put on {}", reorg.undone.len(), reorg.applied.len());
                        store.reorganized(reorg);
                        store.live(now, "reorg", text);
                    }
                }
                "justified" | "finalized" => {
                    if let Some(checkpoint) = parse::<Checkpoint>(data) {
                        let text = format!("epoch {} · {}", checkpoint.epoch, format::short(&checkpoint.block));
                        store.checkpoint(name == "finalized", checkpoint);
                        store.live(now, &name, text);
                    }
                }
                "queued" => {
                    if let Some(tx) = parse::<TxView>(data) {
                        let text = format!("{} from {}", tx.kind(), format::short(&tx.sender));
                        store.queued(tx);
                        store.live(now, "tx", text);
                    }
                }
                "peer_up" | "peer_down" => {
                    if let Some(change) = parse::<PeerChange>(data) {
                        let text = format::short(&change.id);
                        store.peer(change, name == "peer_up", now);
                        store.live(now, &name.replace('_', " "), text);
                    }
                }
                "lagged" => {
                    store.live(now, "lagged", "missed some events; reading the chain again".to_string());
                    self.asked = Asked::default();
                }
                _ => {}
            },
        }
    }

    /// Asks for whatever is due.
    fn poll(&mut self, time: f64) {
        let live = self.streaming == Streaming::Live;
        let due = |last: &mut f64, every: f64| {
            let due = time - *last >= every || *last == 0.0;

            if due {
                *last = time;
            }

            due
        };

        if due(&mut self.asked.status, if live { STATUS_STREAMED } else { STATUS_POLLED }) {
            self.api.ask(Ask::Status);
        }

        if let Some(missing) = self.store.missing() {
            self.api.ask(missing);
        }

        if due(&mut self.asked.node, NODE_EVERY) {
            self.api.ask(Ask::Node);
        }

        if due(&mut self.asked.failures, RETRY_AFTER) {
            self.store.failures.retain(|ask, _| matches!(ask, Ask::Offer(_) | Ask::Submit(_) | Ask::Call { .. }));
        }

        if matches!(self.view, View::Map | View::Overview) && due(&mut self.asked.forks, FORKS_EVERY) {
            self.api.ask(Ask::Forks);
        }

        if let Some(ask) = self.light.ask(time) {
            self.api.ask(ask);
        }

        let page = match &self.view {
            View::Mempool | View::Tx(_) => vec![Ask::Mempool],
            View::Finality => vec![Ask::Validators, Ask::Signers, Ask::Evidence],
            View::Account(address) => [Ask::Account(address.clone())].into_iter().chain(self.light.prove(address)).collect(),
            _ => Vec::new(),
        };

        if !page.is_empty() && due(&mut self.asked.page, PAGE_EVERY) {
            for ask in page {
                self.api.ask(ask);
            }
        }

        if let Some(id) = self.send.waiting_for() {
            self.api.ask(Ask::Tx(id.to_string()));
        }

        if matches!(self.streaming, Streaming::Lost(_)) && time - self.asked.stream >= STREAM_RETRY {
            self.streaming = Streaming::Connecting;
            self.api.listen();
        }
    }

    fn look(&self, narrow: bool) -> Look {
        Look {
            palette: self.palette,
            roles: Roles::of(&self.palette),
            narrow,
        }
    }

    fn back(&mut self) {
        if let Some(view) = self.back.pop() {
            self.ahead.push(std::mem::replace(&mut self.view, view));
            self.moved = true;
        }
    }

    fn forward(&mut self) {
        if let Some(view) = self.ahead.pop() {
            self.back.push(std::mem::replace(&mut self.view, view));
            self.moved = true;
        }
    }

    /// Whether the node answers, and the stream and the light client's state, as a color and words.
    fn health(&self) -> (egui::Color32, String) {
        let palette = self.palette;
        let up = self.store.status.is_some() && !self.store.failures.contains_key(&Ask::Status);

        match (up, &self.streaming) {
            (false, _) => (palette.bad, "not answering".to_string()),
            (true, Streaming::Live) => (palette.good, "live".to_string()),
            (true, Streaming::Connecting) => (palette.warn, "connecting".to_string()),
            (true, _) => (palette.warn, "polling".to_string()),
        }
    }

    /// The light client's state: a color, a few words, and the whole story.
    fn light_state(&self) -> (egui::Color32, &'static str, String) {
        let palette = self.palette;

        match (&self.light.trouble, self.light.following()) {
            (Some(trouble), _) => (palette.bad, "light client stopped", trouble.clone()),
            (None, Some((tip, final_height))) if self.light.caught_up() => (
                palette.good,
                "light client following",
                format!("The light client checked every header to {tip}, and finality to {final_height}, trusting only the genesis it first saw at this node"),
            ),
            (None, Some((tip, _))) => (palette.warn, "light client catching up", format!("{tip} headers checked so far")),
            (None, None) => (palette.muted, "light client starting", "Fetching the chain's light parameters".to_string()),
        }
    }

    /// On a wide screen: the name, the pages in groups, and the node's state at the bottom.
    fn sidebar(&mut self, ui: &mut Ui, now: u64) {
        let palette = self.palette;
        ui.add_space(16.0);
        ui.horizontal(|ui| {
            ui.label(widgets::strong("chainv4", 17.0).color(palette.ink));
            widgets::caps(ui, &palette, "explorer");
        });
        ui.add_space(14.0);

        for (group, pages) in View::GROUPS {
            widgets::caps(ui, &palette, group);
            ui.add_space(2.0);

            for (view, name) in pages {
                let selected = self.view == *view;
                let text = RichText::new(*name).color(if selected { palette.ink } else { palette.soft });
                let button = egui::Button::new(text)
                    .fill(if selected { palette.row } else { egui::Color32::TRANSPARENT })
                    .stroke(egui::Stroke::NONE)
                    .min_size(egui::vec2(ui.available_width(), 28.0));
                let response = ui.add(button);

                if selected {
                    let rect = response.rect;
                    ui.painter().rect_filled(
                        egui::Rect::from_min_size(rect.left_top() + egui::vec2(0.0, 6.0), egui::vec2(3.0, rect.height() - 12.0)),
                        2.0,
                        palette.accent,
                    );
                }

                if response.clicked() {
                    self.go(view.clone());
                }
            }

            ui.add_space(10.0);
        }

        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            ui.add_space(14.0);
            let (light_color, light_words, light_story) = self.light_state();
            ui.horizontal(|ui| {
                views::dot(ui, light_color);
                ui.label(RichText::new(light_words).small().color(palette.soft));
            })
            .response
            .on_hover_text(light_story);

            let (color, words) = self.health();
            ui.horizontal(|ui| {
                views::dot(ui, color);
                ui.label(RichText::new(words).small().color(palette.soft));

                if let Some(status) = &self.store.status {
                    ui.label(RichText::new(format!("· {}", format::ago(status.timestamp, now))).small().color(palette.muted));
                }
            });
            ui.add(egui::Label::new(RichText::new(self.api.base()).monospace().small().color(palette.muted)).truncate());
            widgets::caps(ui, &palette, "Node");
        });
    }

    /// On a wide screen, over the page: back and forward, the search, and where the chain is.
    fn header(&mut self, ui: &mut Ui) {
        let palette = self.palette;
        ui.horizontal(|ui| {
            if ui.add_enabled(!self.back.is_empty(), egui::Button::new("‹").min_size(egui::vec2(28.0, 26.0))).on_hover_text("Back").clicked() {
                self.back();
            }

            if ui.add_enabled(!self.ahead.is_empty(), egui::Button::new("›").min_size(egui::vec2(28.0, 26.0))).on_hover_text("Forward").clicked() {
                self.forward();
            }

            let height = self.store.status.as_ref().map(|status| format!("height {}", format::thousands(&status.height.to_string())));
            let room = ui.available_width() - if height.is_some() { 150.0 } else { 0.0 };
            self.search_field(ui, room.min(640.0));

            if let Some(height) = height {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(height).monospace().color(palette.soft));
                });
            }
        });
    }

    /// On a phone, over the page: back or the name, the node's state as a dot, and the search.
    fn bar(&mut self, ui: &mut Ui) {
        let palette = self.palette;
        ui.horizontal(|ui| {
            match self.back.is_empty() {
                true => {
                    ui.label(widgets::strong("chainv4", 16.0).color(palette.ink));
                }
                false => {
                    if ui.add(egui::Button::new(RichText::new("‹").size(18.0)).min_size(egui::vec2(34.0, 30.0))).clicked() {
                        self.back();
                    }
                }
            }

            let (color, words) = self.health();
            views::dot(ui, color).on_hover_text(words);
            let room = ui.available_width();
            self.search_field(ui, room);
        });
    }

    fn search_field(&mut self, ui: &mut Ui, width: f32) {
        let search = ui.add(
            egui::TextEdit::singleline(&mut self.search)
                .hint_text("Search a height, block or transaction id, or address")
                .desired_width(width),
        );

        if search.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
            self.find();
        }
    }

    /// On a phone, the pages, as tabs along the bottom.
    fn tabs(&mut self, ui: &mut Ui) {
        let palette = self.palette;
        let width = ui.available_width() / View::TABS.len() as f32;
        let current = self.view.tab();
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.horizontal(|ui| {
            for (view, name) in View::TABS {
                let selected = current == view;
                let text = widgets::strong(name, 12.0).color(if selected { palette.ink } else { palette.muted });
                let button = egui::Button::new(text).fill(egui::Color32::TRANSPARENT).stroke(egui::Stroke::NONE);
                let response = ui.add_sized([width, 46.0], button);

                if selected {
                    let rect = response.rect;
                    ui.painter().rect_filled(
                        egui::Rect::from_center_size(egui::pos2(rect.center().x, rect.top() + 1.5), egui::vec2(28.0, 3.0)),
                        2.0,
                        palette.accent,
                    );
                }

                if response.clicked() {
                    self.go(view);
                }
            }
        });
    }

    /// Something to tell the person, over the page.
    fn notice(&mut self, ui: &mut Ui) {
        let Some((text, bad)) = self.notice.clone() else {
            return;
        };
        let palette = self.palette;
        let color = if bad { palette.bad } else { palette.good };

        egui::Frame::new()
            .fill(color.gamma_multiply(0.12))
            .stroke(egui::Stroke::new(1.0, color.gamma_multiply(0.5)))
            .corner_radius(6.0)
            .inner_margin(egui::Margin::symmetric(10, 6))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add(egui::Label::new(RichText::new(text).color(color)).wrap());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("Dismiss").clicked() {
                            self.notice = None;
                        }
                    });
                });
            });
        ui.add_space(8.0);
    }

    /// Goes where the search box says.
    fn find(&mut self) {
        let wanted = self.search.trim().to_string();

        if wanted.is_empty() {
            return;
        }

        if wanted.chars().all(|digit| digit.is_ascii_digit() || digit == ',') {
            self.go(View::Block(wanted.replace(',', "")));
        } else if wanted.contains(':') {
            self.go(View::Account(wanted));
        } else if wanted.starts_with("0x") {
            self.api.ask(Ask::Lookup(wanted));
        } else {
            self.notice = Some((format!("{wanted} is not a height, an id or an address"), true));

            return;
        }

        self.notice = None;
        self.search.clear();
    }

    fn page(&mut self, ui: &mut Ui, now: u64, narrow: bool) {
        let mut go = None;
        let mut connect = None;
        let look = self.look(narrow);
        self.notice(ui);
        let mut ctx = Ctx {
            store: &self.store,
            light: &mut self.light,
            api: &mut self.api,
            go: &mut go,
            settings: &mut self.settings,
            now,
            look,
        };

        // The map fills the page itself, and pans and zooms in it rather than scrolling.
        if self.view == View::Map {
            views::map::show(ui, &mut ctx, &mut self.map);
        } else {
            let mut scroll = egui::ScrollArea::vertical().id_salt("page").auto_shrink(false);

            if std::mem::take(&mut self.moved) {
                scroll = scroll.vertical_scroll_offset(0.0);
            }

            scroll.show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.set_max_width(ui.available_width().min(COLUMN));
                    ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| match &self.view {
                        View::Overview => views::overview::show(ui, &mut ctx),
                        View::Map => {}
                        View::Block(block) => views::block::show(ui, &mut ctx, block),
                        View::Tx(id) => views::tx::show(ui, &mut ctx, id, &mut self.hops),
                        View::Account(address) => views::account::show(ui, &mut ctx, address, &mut self.account),
                        View::Network => views::network::show(ui, &mut ctx, &self.streaming),
                        View::Mempool => views::mempool::show(ui, &mut ctx),
                        View::Tokens => views::tokens::show(ui, &mut ctx, &mut self.tokens),
                        View::Finality => views::finality::show(ui, &mut ctx),
                        View::Send => views::send::show(ui, &mut ctx, &mut self.send),
                        View::Settings => views::settings::show(ui, &mut ctx, &mut self.address, &mut self.settings_page, &self.streaming, &mut connect),
                        View::More => views::more::show(ui, &mut ctx),
                    });
                    ui.add_space(24.0);
                });
            });
        }

        if let Some(view) = go {
            self.go(view);
        }

        if let Some(node) = connect {
            self.connect(&node);
        }
    }
}

impl eframe::App for Explorer {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let time = ui.input(|input| input.time);
        let now = now_ms();
        self.take(now, time);
        self.poll(time);
        layout::suit_touch(ui.ctx());
        let palette = self.palette;
        let narrow = layout::is_narrow(ui.available_width());
        let bar = egui::Frame::new().fill(palette.nav).stroke(egui::Stroke::new(1.0, palette.line));

        match narrow {
            true => {
                egui::Panel::top("bar").frame(bar.inner_margin(egui::Margin::symmetric(10, 8))).show(ui, |ui| self.bar(ui));
                egui::Panel::bottom("tabs").frame(bar).show(ui, |ui| self.tabs(ui));
            }
            false => {
                egui::Panel::left("nav")
                    .exact_size(SIDEBAR)
                    .resizable(false)
                    .frame(bar.inner_margin(egui::Margin::symmetric(12, 0)))
                    .show(ui, |ui| self.sidebar(ui, now));
                egui::Panel::top("header")
                    .frame(egui::Frame::new().fill(palette.paper).inner_margin(egui::Margin::symmetric(16, 10)))
                    .show(ui, |ui| self.header(ui));
            }
        }

        let margin = if narrow { egui::Margin::symmetric(10, 10) } else { egui::Margin::symmetric(16, 4) };
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(palette.paper).inner_margin(margin))
            .show(ui, |ui| self.page(ui, now, narrow));

        // The theme follows the setting, wherever it was changed.
        if self.palette.dark == self.settings.light_theme {
            self.palette = if self.settings.light_theme { LIGHT } else { DARK };
            theme::apply(ui.ctx(), &self.palette);
        }

        // Polls come due even when nothing arrives.
        ui.ctx().request_repaint_after(Duration::from_millis(500));
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, eframe::APP_KEY, &self.settings);
    }

    /// The paper's color, behind everything: what shows under a phone's status bar.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        self.palette.paper.to_normalized_gamma_f32()
    }
}

fn parse<T: serde::de::DeserializeOwned>(value: Value) -> Option<T> {
    match serde_json::from_value(value) {
        Ok(parsed) => Some(parsed),
        Err(err) => {
            log::warn!("the node sent something unexpected: {err}");

            None
        }
    }
}

/// The time, in milliseconds since the Unix epoch.
/// What the explorer's requests go out through: the proxy set, or straight to the node. The
/// browser makes its own requests, through its own proxy.
fn client(settings: &Settings) -> Result<chainv4_ui::net::Client, String> {
    match cfg!(target_arch = "wasm32") {
        true => Ok(chainv4_ui::net::Client::direct()),
        false => chainv4_ui::net::Client::for_setting(&settings.proxy),
    }
}

fn now_ms() -> u64 {
    web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}
