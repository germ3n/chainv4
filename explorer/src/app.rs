//! The explorer: one node's API, everything learned from it, and the page being looked at. Each
//! frame takes what arrived, asks for what is due, and draws the page.
//!
//! What the node pushes over its stream lands at once. Without a stream, or besides it, the
//! explorer polls: the status every second or so, and whatever the page in view shows.

use {
    crate::api::{Api, Ask, News, Reply},
    crate::format,
    crate::model::{
        BlockDetail, Called, Checkpoint, Contract, Forks, Mempool, NodeView, Page, PeerChange, Receipt, Reorganized, Status,
        Summary, TxView,
    },
    crate::store::{PAGE, Store},
    crate::views::{
        self, Ctx, Settings, View, account::AccountState, hops::HopState, map::MapState, network::Streaming, send::SendState,
    },
    egui::{RichText, Ui},
    serde_json::Value,
    std::time::Duration,
};

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
    send: SendState,
    /// Something to tell the person, and whether it is bad news.
    notice: Option<(String, bool)>,
}

impl Explorer {
    /// The explorer, talking to node when given, or else to the node it talked to last.
    pub fn new(creation: &eframe::CreationContext, node: Option<String>, fallback: &str) -> Explorer {
        let settings: Settings = creation
            .storage
            .and_then(|storage| eframe::get_value(storage, eframe::APP_KEY))
            .unwrap_or_default();
        let node = node
            .or_else(|| Some(settings.node.clone()).filter(|node| !node.is_empty()))
            .unwrap_or_else(|| fallback.to_string());
        let wake = creation.egui_ctx.clone();
        let mut explorer = Explorer {
            settings,
            address: node.clone(),
            api: Api::new(&node, wake.clone()),
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
            send: SendState::default(),
            notice: None,
        };
        explorer.connect(&node);

        explorer
    }

    /// Starts afresh with another node.
    fn connect(&mut self, node: &str) {
        let node = node.trim().trim_end_matches('/').to_string();
        self.api = Api::new(&node, self.wake.clone());
        self.store = Store::default();
        self.map = MapState::default();
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
        }
    }

    /// Takes everything that arrived.
    fn take(&mut self, now: u64, time: f64) {
        for reply in self.api.replies() {
            match reply {
                Reply::Answer { ask, result } => {
                    self.send.answered(&ask, &result, &mut self.api);

                    match result {
                        Ok(value) => {
                            self.store.failures.remove(&ask);
                            self.answered(ask, value);
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

    fn answered(&mut self, ask: Ask, value: Value) {
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
                store.contracts.insert(address, parse::<Contract>(value));
            }
            Ask::Validators => store.validators = Some(value),
            Ask::Signers => store.signers = Some(value),
            Ask::Evidence => store.evidence = Some(value),
            Ask::Call { ref contract, ref function, ref args } => {
                if let Some(called) = parse::<Called>(value.clone())
                    && called.status == "returned"
                    && let Some(first) = called.values.first()
                {
                    let token = store.tokens.entry(contract.clone()).or_default();

                    match function.as_str() {
                        "decimals" => token.decimals = first.text().parse::<f64>().ok().map(|decimals| decimals as u8),
                        "symbol" => token.symbol = Some(first.text()),
                        "name" => token.name = Some(first.text()),
                        "balance_of" => {
                            let holder = serde_json::from_str::<Value>(args)
                                .ok()
                                .and_then(|args| args.get(0)?.get("value")?.as_str().map(str::to_string));

                            if let Some(holder) = holder {
                                let raw = first.text();
                                let raw = raw.split('.').next().unwrap_or_default().to_string();
                                store.holdings.insert((contract.clone(), holder), raw);
                            }
                        }
                        _ => {}
                    }
                }

                store.calls.insert(ask.clone(), value);
            }
            Ask::Offer(_) | Ask::Submit(_) => {}
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

        let page = match &self.view {
            View::Mempool | View::Tx(_) => vec![Ask::Mempool],
            View::Finality => vec![Ask::Validators, Ask::Signers, Ask::Evidence],
            View::Account(address) => vec![Ask::Account(address.clone())],
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

    fn top_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.strong("chainv4");

            let up = self.store.status.is_some() && !self.store.failures.contains_key(&Ask::Status);
            views::dot(ui, if up { views::GOOD } else { views::BAD })
                .on_hover_text(if up { "the node answers" } else { "the node does not answer" });
            let field = ui.add(egui::TextEdit::singleline(&mut self.address).desired_width(220.0).hint_text("http://127.0.0.1:8080"));

            if (field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter))) || ui.button("Connect").clicked() {
                let address = self.address.clone();
                self.connect(&address);
            }

            match &self.streaming {
                Streaming::Live => views::badge(ui, "live", views::GOOD),
                Streaming::Connecting => views::badge(ui, "connecting", views::SIDE),
                _ => views::badge(ui, "polling", views::SIDE),
            }

            ui.separator();

            if ui.add_enabled(!self.back.is_empty(), egui::Button::new("<")).clicked()
                && let Some(view) = self.back.pop()
            {
                self.ahead.push(std::mem::replace(&mut self.view, view));
            }

            if ui.add_enabled(!self.ahead.is_empty(), egui::Button::new(">")).clicked()
                && let Some(view) = self.ahead.pop()
            {
                self.back.push(std::mem::replace(&mut self.view, view));
            }

            for (view, name) in View::TABS {
                if ui.selectable_label(self.view == view, name).clicked() {
                    self.go(view);
                }
            }

            ui.separator();
            let search = ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .hint_text("height, block or transaction id, address")
                    .desired_width(ui.available_width().max(160.0)),
            );

            if search.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                self.find();
            }
        });

        if let Some((text, bad)) = self.notice.clone() {
            ui.horizontal(|ui| {
                ui.colored_label(if bad { views::BAD } else { views::GOOD }, text);

                if ui.small_button("x").clicked() {
                    self.notice = None;
                }
            });
        }
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
    }

    fn page(&mut self, ui: &mut Ui, now: u64) {
        let mut go = None;
        let mut ctx = Ctx {
            store: &self.store,
            api: &mut self.api,
            go: &mut go,
            settings: &mut self.settings,
            now,
        };

        egui::ScrollArea::vertical().id_salt("page").auto_shrink(false).show(ui, |ui| match &self.view {
            View::Overview => views::overview::show(ui, &mut ctx),
            View::Map => views::map::show(ui, &mut ctx, &mut self.map),
            View::Block(block) => views::block::show(ui, &mut ctx, block),
            View::Tx(id) => views::tx::show(ui, &mut ctx, id, &mut self.hops),
            View::Account(address) => views::account::show(ui, &mut ctx, address, &mut self.account),
            View::Network => views::network::show(ui, &mut ctx, &self.streaming),
            View::Mempool => views::mempool::show(ui, &mut ctx),
            View::Finality => views::finality::show(ui, &mut ctx),
            View::Send => views::send::show(ui, &mut ctx, &mut self.send),
        });

        if let Some(view) = go {
            self.go(view);
        }
    }
}

impl eframe::App for Explorer {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let time = ui.input(|input| input.time);
        let now = now_ms();
        self.take(now, time);
        self.poll(time);

        egui::Panel::top("top").show(ui, |ui| {
            ui.add_space(4.0);
            self.top_bar(ui);
            ui.add_space(2.0);
        });
        egui::Panel::bottom("bottom").show(ui, |ui| {
            ui.horizontal(|ui| {
                match &self.store.status {
                    Some(status) => {
                        ui.weak(format!(
                            "height {} · {} · {} · genesis {}",
                            format::thousands(&status.height.to_string()),
                            format::ago(status.timestamp, now),
                            status.hash,
                            format::short(&status.genesis)
                        ));
                    }
                    None => {
                        ui.weak(format!("waiting for {}", self.api.base()));
                    }
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.weak(RichText::new(format!("{} blocks loaded", self.store.blocks.len())).small());
                });
            });
        });
        egui::CentralPanel::default().show(ui, |ui| self.page(ui, now));

        // Polls come due even when nothing arrives.
        ui.ctx().request_repaint_after(Duration::from_millis(500));
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, eframe::APP_KEY, &self.settings);
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
fn now_ms() -> u64 {
    web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}
