//! The explorer's pages, and the pieces they share: sections, facts that stack on a phone, tables
//! that become rows, links to blocks, transactions and accounts, amounts in the right units, and a
//! view of any JSON.

use {
    crate::api::{Api, Ask},
    crate::format,
    crate::model::{Argument, TxView},
    crate::store::Store,
    crate::theme::Roles,
    chainv4_ui::{Palette, widgets},
    egui::{Color32, RichText, Ui},
    serde::{Deserialize, Serialize},
    serde_json::{Value, json},
    std::collections::HashMap,
};

pub mod account;
pub mod block;
pub mod chart;
pub mod events;
pub mod finality;
pub mod hops;
pub mod map;
pub mod mempool;
pub mod more;
pub mod network;
pub mod overview;
pub mod send;
pub mod name;
pub mod settings;
pub mod tokens;
pub mod tx;

/// A page of the explorer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum View {
    Overview,
    Map,
    /// A block by height or id.
    Block(String),
    Tx(String),
    Account(String),
    /// A name, such as alice.dev.
    Name(String),
    Network,
    Mempool,
    Tokens,
    Finality,
    Send,
    Settings,
    /// On a phone, the pages the tabs have no room for.
    More,
}

impl View {
    /// The sidebar's pages, in groups under a heading.
    pub const GROUPS: [(&'static str, &'static [(View, &'static str)]); 3] = [
        ("Chain", &[(View::Overview, "Overview"), (View::Map, "Block map"), (View::Mempool, "Mempool"), (View::Tokens, "Tokens")]),
        ("Network", &[(View::Network, "Network"), (View::Finality, "Finality")]),
        ("Tools", &[(View::Send, "Send"), (View::Settings, "Settings")]),
    ];

    /// A phone's tabs, along the bottom.
    pub const TABS: [(View, &'static str); 5] = [
        (View::Overview, "Home"),
        (View::Map, "Blocks"),
        (View::Mempool, "Mempool"),
        (View::Tokens, "Tokens"),
        (View::More, "More"),
    ];

    /// The pages More lists, with what each is for.
    pub const MORE: [(View, &'static str, &'static str); 4] = [
        (View::Network, "Network", "The node, its peers, and what it heard"),
        (View::Finality, "Finality", "Checkpoints, voters and caught cheats"),
        (View::Send, "Send", "Sign and send a transaction"),
        (View::Settings, "Settings", "The node, the theme, the light client"),
    ];

    /// Which tab a page belongs under on a phone: a page of its own, or More for the rest.
    pub fn tab(&self) -> View {
        match self {
            View::Overview | View::Block(_) | View::Tx(_) | View::Account(_) | View::Name(_) => View::Overview,
            View::Map => View::Map,
            View::Mempool => View::Mempool,
            View::Tokens => View::Tokens,
            View::Network | View::Finality | View::Send | View::Settings | View::More => View::More,
        }
    }
}

/// What the explorer remembers between runs.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The node's HTTP API.
    pub node: String,
    /// The proxy every request goes through, as socks5h://host:port or http://host:port, or empty
    /// to go direct. Never used in the browser, whose own proxy settings apply.
    pub proxy: String,
    /// Decimals set by hand per token contract, by chain (its genesis id), then contract.
    pub decimals: HashMap<String, HashMap<String, u8>>,
    /// EVM contracts' ABIs, as pasted, by chain (its genesis id), then contract.
    #[serde(default)]
    pub abis: HashMap<String, HashMap<String, String>>,
    /// The genesis id the light client first saw at each node, which it trusts there from then on.
    pub pinned: HashMap<String, String>,
    /// A finalized checkpoint's block id to start the light client from at each node, pasted,
    /// rather than genesis.
    pub checkpoints: HashMap<String, String>,
    /// The light palette rather than the dark one.
    pub light_theme: bool,
}

/// The look a page is drawn in: its colors, and whether the screen is a phone's.
#[derive(Clone, Copy, Debug)]
pub struct Look {
    pub palette: Palette,
    pub roles: Roles,
    pub narrow: bool,
}

/// What a page works with.
pub struct Ctx<'a> {
    pub store: &'a Store,
    /// The light client, and the balances it proved.
    pub light: &'a mut crate::light::Light,
    pub api: &'a mut Api,
    /// Where to go next, when a page says.
    pub go: &'a mut Option<View>,
    pub settings: &'a mut Settings,
    /// The time, in milliseconds since the Unix epoch.
    pub now: u64,
    pub look: Look,
}

impl Ctx<'_> {
    pub fn go(&mut self, view: View) {
        *self.go = Some(view);
    }

    pub fn palette(&self) -> Palette {
        self.look.palette
    }

    /// The chain's coin: its decimals and symbol, 18 and nothing before the status comes.
    pub fn coin(&self) -> (u8, String) {
        self.store
            .status
            .as_ref()
            .map_or((18, String::new()), |status| (status.coin.decimals, status.coin.symbol.clone()))
    }

    /// An amount of the chain's coin.
    pub fn coins(&self, raw: &str) -> String {
        let (decimals, symbol) = self.coin();

        format::amount(raw, decimals, &symbol)
    }

    /// The decimals set by hand for a token on this chain.
    pub fn decimals_set(&self, contract: &str) -> Option<u8> {
        let genesis = &self.store.status.as_ref()?.genesis;

        self.settings.decimals.get(genesis)?.get(contract).copied()
    }

    /// The ABI kept for an EVM contract on this chain, read.
    pub fn abi(&self, contract: &str) -> Option<chainv4::abi::Abi> {
        let genesis = &self.store.status.as_ref()?.genesis;

        chainv4::abi::Abi::parse(self.settings.abis.get(genesis)?.get(contract)?).ok()
    }

    /// The ABI kept for an EVM contract, as pasted.
    pub fn abi_text(&self, contract: &str) -> Option<String> {
        let genesis = &self.store.status.as_ref()?.genesis;

        self.settings.abis.get(genesis)?.get(contract).cloned()
    }

    /// Keeps, or with None forgets, an EVM contract's ABI on this chain.
    pub fn set_abi(&mut self, contract: &str, abi: Option<String>) {
        let Some(genesis) = self.store.status.as_ref().map(|status| status.genesis.clone()) else {
            return;
        };
        let chain = self.settings.abis.entry(genesis).or_default();

        match abi {
            Some(abi) => chain.insert(contract.to_string(), abi),
            None => chain.remove(contract),
        };
    }

    /// Sets, or with None clears, the decimals for a token on this chain.
    pub fn set_decimals(&mut self, contract: &str, decimals: Option<u8>) {
        let Some(genesis) = self.store.status.as_ref().map(|status| status.genesis.clone()) else {
            return;
        };
        let chain = self.settings.decimals.entry(genesis).or_default();

        match decimals {
            Some(decimals) => chain.insert(contract.to_string(), decimals),
            None => chain.remove(contract),
        };
    }

    /// A token's decimals and symbol, as the chain's registry holds them or as set by hand, when
    /// the contract is one; asks for the contract when it is not known yet. None for a contract
    /// that is no token, or one whose decimals neither it nor anyone gave.
    pub fn token(&mut self, contract: &str) -> Option<(u8, String)> {
        let Some(token) = self.store.tokens.get(contract).cloned() else {
            if !self.store.contracts.contains_key(contract) {
                self.api.ask(Ask::Contract(contract.to_string()));
            }

            return None;
        };
        let decimals = self.decimals_set(contract).or(token.decimals)?;

        Some((decimals, token.symbol.unwrap_or_else(|| "?".to_string())))
    }

    /// What a holder has of a token, asking when it is not known yet.
    pub fn holding(&mut self, token: &str, holder: &str) -> Option<String> {
        let key = (token.to_string(), holder.to_string());

        match self.store.holdings.get(&key) {
            Some(raw) => Some(raw.clone()),
            None => {
                self.api.ask(balance_of(token, holder));

                None
            }
        }
    }
}

/// The read-only call for a holder's balance of a token.
pub fn balance_of(token: &str, holder: &str) -> Ask {
    Ask::Call {
        contract: token.to_string(),
        function: "balance_of".to_string(),
        args: json!([{ "type": "address", "value": holder }]).to_string(),
    }
}

// Headings and sections.

/// A page's heading: its title, pills beside it, and what it is about under it.
pub fn heading(ui: &mut Ui, look: Look, title: &str, about: &str, pills: impl FnOnce(&mut Ui)) {
    ui.horizontal_wrapped(|ui| {
        ui.label(widgets::strong(title, if look.narrow { 19.0 } else { 22.0 }).color(look.palette.ink));
        pills(ui);
    });

    if !about.is_empty() {
        ui.label(RichText::new(about).color(look.palette.muted));
    }

    ui.add_space(10.0);
}

/// A card with a heading in capitals: one part of a page.
pub fn section<R>(ui: &mut Ui, look: Look, title: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    section_with(ui, look, title, |_| {}, add)
}

/// A section with something on the right of its heading: a link, a filter, a count.
pub fn section_with<R>(ui: &mut Ui, look: Look, title: &str, right: impl FnOnce(&mut Ui), add: impl FnOnce(&mut Ui) -> R) -> R {
    let inner = widgets::card(ui, &look.palette, |ui| {
        ui.horizontal(|ui| {
            widgets::caps(ui, &look.palette, title);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), right);
        });
        ui.add_space(4.0);
        add(ui)
    });
    ui.add_space(10.0);

    inner
}

/// A note in a section, muted.
pub fn note(ui: &mut Ui, look: Look, text: impl Into<String>) {
    ui.add(egui::Label::new(RichText::new(text.into()).color(look.palette.muted)).wrap());
}

// Facts: a label beside each value on a wide screen, above it on a phone.

/// Rows of facts: a label and a value each.
pub struct Facts<'u> {
    ui: &'u mut Ui,
    look: Look,
}

impl Facts<'_> {
    /// One fact. The value may be several widgets: they wrap.
    pub fn row(&mut self, name: &str, value: impl FnOnce(&mut Ui)) {
        let label = RichText::new(name).color(self.look.palette.muted);

        match self.look.narrow {
            true => {
                self.ui.label(label.small());
                self.ui.horizontal_wrapped(value);
                self.ui.add_space(4.0);
            }
            false => {
                self.ui.label(label);
                self.ui.horizontal_wrapped(value);
                self.ui.end_row();
            }
        }
    }

    /// A fact that is plain text.
    pub fn text(&mut self, name: &str, text: impl Into<String>) {
        let text = text.into();
        self.row(name, |ui| {
            ui.label(text);
        });
    }
}

pub fn facts(ui: &mut Ui, look: Look, id: &str, add: impl FnOnce(&mut Facts)) {
    match look.narrow {
        true => {
            ui.vertical(|ui| add(&mut Facts { ui, look }));
        }
        false => {
            egui::Grid::new(id)
                .num_columns(2)
                .spacing([18.0, 6.0])
                .min_col_width(110.0)
                .show(ui, |ui| add(&mut Facts { ui, look }));
        }
    }
}

// Tables, and the rows they become on a phone.

/// How an area inside a page scrolls: by its bar and the wheel, never by a drag, which belongs to
/// the page.
pub const NO_DRAG: egui::containers::scroll_area::ScrollSource = egui::containers::scroll_area::ScrollSource {
    scroll_bar: true,
    drag: egui::containers::scroll_area::DragScroll::Never,
    mouse_wheel: true,
};

/// A table with headings in capitals, striped. For a wide screen; a phone gets rows. One wider than
/// the page scrolls across by its bar, never by a drag, so a finger dragging over it still scrolls
/// the page.
pub fn table(ui: &mut Ui, look: Look, id: impl std::hash::Hash + std::fmt::Debug, headings: &[&str], rows: impl FnOnce(&mut Ui)) {
    egui::ScrollArea::horizontal().id_salt(("table", ui.scope_id(), &id)).scroll_source(NO_DRAG).show(ui, |ui| {
        egui::Grid::new(id).striped(true).spacing([14.0, 6.0]).num_columns(headings.len()).show(ui, |ui| {
            for heading in headings {
                widgets::caps(ui, &look.palette, heading);
            }

            ui.end_row();
            rows(ui);
        });
    });
}

/// One row of a list on a phone: what it shows, and a line under it.
pub fn list_row(ui: &mut Ui, look: Look, add: impl FnOnce(&mut Ui)) {
    ui.add_space(3.0);
    ui.vertical(add);
    ui.add_space(3.0);
    widgets::rule(ui, &look.palette);
}

/// How full something is: what it says, and a thin bar under it.
pub fn meter(ui: &mut Ui, look: Look, fraction: f32, text: &str) {
    ui.vertical(|ui| {
        ui.label(RichText::new(text).monospace());
        let width = ui.available_width().min(320.0);
        let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 5.0), egui::Sense::hover());
        ui.painter().rect_filled(rect, 3.0, look.palette.row);
        let filled = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width() * fraction.clamp(0.0, 1.0), rect.height()));
        ui.painter().rect_filled(filled, 3.0, look.palette.accent);
    });
}

/// A tile with a number in it: what the overview leads with.
pub fn stat(ui: &mut Ui, look: Look, width: f32, label: &str, value: &str, note: &str) {
    let (margin, size) = if look.narrow { (egui::Margin::symmetric(10, 7), 17.0) } else { (egui::Margin::symmetric(12, 10), 21.0) };

    egui::Frame::new()
        .fill(look.palette.card)
        .stroke(egui::Stroke::new(1.0, look.palette.line))
        .corner_radius(ui.visuals().window_corner_radius)
        .inner_margin(margin)
        .show(ui, |ui| {
            ui.set_width(width - 2.0 * f32::from(margin.left));
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.vertical(|ui| {
                widgets::caps(ui, &look.palette, label);
                widgets::fitted(ui, value, size, 12.0, egui::FontFamily::Monospace, look.palette.ink);
                ui.add(egui::Label::new(RichText::new(note).small().color(look.palette.muted)).truncate());
            });
        });
}

/// Tiles across, as many to a row as fit, each the same width.
pub fn stats(ui: &mut Ui, look: Look, tiles: &[(String, String, String)]) {
    let gap = 8.0;
    let per_row = match look.narrow {
        true => 2,
        false => chainv4_ui::layout::fit(ui.available_width(), 170.0, gap),
    };
    let width = ((ui.available_width() - gap * (per_row - 1) as f32) / per_row as f32).floor();

    for row in tiles.chunks(per_row) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;

            for (label, value, note) in row {
                stat(ui, look, width, label, value, note);
            }
        });
        ui.add_space(gap - ui.spacing().item_spacing.y);
    }

    ui.add_space(6.0);
}

/// A status as a pill: done, failed, deployed and the like.
pub fn status(ui: &mut Ui, look: Look, status: &str) {
    let color = match status {
        "failed" => look.palette.bad,
        _ => look.palette.good,
    };

    widgets::pill(ui, status, color);
}

/// A colored dot, as fonts may lack one.
pub fn dot(ui: &mut Ui, color: Color32) -> egui::Response {
    widgets::dot(ui, color)
}

/// What to show while something loads, or why it did not.
pub fn waiting(ui: &mut Ui, ctx: &Ctx, ask: &Ask) {
    let palette = ctx.palette();

    match ctx.store.failures.get(ask) {
        Some(failure) if failure.status == 404 => {
            ui.label(RichText::new(format!("Not found: {}", failure.message)).color(palette.muted));
        }
        Some(failure) => {
            ui.colored_label(palette.bad, &failure.message);
        }
        None => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new("Loading…").color(palette.muted));
            });
        }
    }
}

// Links.

/// A link's text: monospace, in the accent. The look sets every text's color, links' included, so
/// links say they are links themselves.
fn link_text(ctx: &Ctx, text: impl Into<String>) -> RichText {
    RichText::new(text.into()).monospace().color(ctx.look.palette.accent)
}

/// An address that opens its account page. Text that is not an address shows as it is.
pub fn address(ui: &mut Ui, ctx: &mut Ctx, address: &str) {
    if !address.contains(':') {
        ui.label(address);

        return;
    }

    let response = ui.link(link_text(ctx, format::short(address))).on_hover_text(address);

    if response.clicked() {
        ctx.go(View::Account(address.to_string()));
    }

    copy_menu(&response, address);
}

/// A block id or height that opens the block's page.
pub fn block(ui: &mut Ui, ctx: &mut Ctx, block: &str, label: &str) {
    let response = ui.link(link_text(ctx, label)).on_hover_text(block);

    if response.clicked() {
        ctx.go(View::Block(block.to_string()));
    }

    copy_menu(&response, block);
}

/// A transaction id that opens its page.
pub fn tx(ui: &mut Ui, ctx: &mut Ctx, id: &str) {
    let response = ui.link(link_text(ctx, format::short(id))).on_hover_text(id);

    if response.clicked() {
        ctx.go(View::Tx(id.to_string()));
    }

    copy_menu(&response, id);
}

/// Text in full, in the monospace face, that copies itself when clicked or tapped, and says so. Long
/// text wraps, so an id fits a phone.
pub fn copyable(ui: &mut Ui, text: &str) {
    let label = egui::Label::new(RichText::new(text).monospace()).wrap();
    let palette = palette_of(ui);
    widgets::copy_on_click(ui, &palette, label, text).on_hover_text("Click to copy");
}

/// The palette the explorer is dressed in, read back from egui's style: the copy feedback needs
/// only its good color.
fn palette_of(ui: &Ui) -> Palette {
    match ui.visuals().dark_mode {
        true => crate::theme::DARK,
        false => crate::theme::LIGHT,
    }
}

/// A link's menu, on a right click or a long press: copying what it links to in full.
fn copy_menu(response: &egui::Response, text: &str) {
    response.context_menu(|ui| {
        if ui.button("Copy").clicked() {
            ui.ctx().copy_text(text.to_string());
            ui.close();
        }
    });
}

// Transactions in a list, as a block, an account and the mempool show them.

/// One transaction in a list, and what the list knows of where it landed.
pub struct TxLine<'a> {
    pub tx: &'a TxView,
    /// Its place in its block, when the list is a block's.
    pub index: Option<usize>,
    /// Its block's id and height, when the list spans blocks.
    pub block: Option<(&'a str, u64)>,
}

/// Transactions: a table on a wide screen, two lines each on a phone. With nothing landed yet, the
/// status and fee columns say what the mempool knows instead: the nonce and offer.
pub fn tx_list(ui: &mut Ui, ctx: &mut Ctx, id: &str, lines: &[TxLine]) {
    let look = ctx.look;
    let waiting = lines.iter().all(|line| line.tx.receipt.is_none() && line.block.is_none() && line.index.is_none());

    if look.narrow {
        for line in lines {
            list_row(ui, look, |ui| tx_rows(ui, ctx, line, waiting));
        }

        return;
    }

    let first = match (lines.first().map(|line| (line.index.is_some(), line.block.is_some())), waiting) {
        (_, true) => "nonce",
        (Some((true, _)), _) => "#",
        _ => "block",
    };
    let last = if waiting { ["gas limit", "max price · tip"] } else { ["status", "fee"] };
    let headings = [first, "id", "type", "from", "to", last[0], last[1]];

    table(ui, look, ("txs", id), &headings, |ui| {
        for line in lines {
            let tx = line.tx;

            match (line.index, line.block) {
                _ if waiting => {
                    ui.monospace(tx.nonce.to_string());
                }
                (Some(index), _) => {
                    ui.monospace(index.to_string());
                }
                (None, Some((block, height))) => block_link(ui, ctx, block, height),
                (None, None) => {
                    ui.label("—");
                }
            }

            self::tx(ui, ctx, &tx.id);
            ui.label(RichText::new(tx.kind()).monospace().color(look.palette.soft));
            address(ui, ctx, &tx.sender);

            match tx.target() {
                Some(target) => address(ui, ctx, target),
                None => {
                    ui.label(RichText::new("—").color(look.palette.muted));
                }
            }

            match (&tx.receipt, waiting) {
                (_, true) => {
                    ui.monospace(format::thousands(&tx.fee.gas_limit.to_string()));
                    ui.monospace(format!("{} · {}", format::thousands(&tx.fee.max_price), format::thousands(&tx.fee.tip)));
                }
                (Some(receipt), false) => {
                    status(ui, look, &receipt.status);
                    ui.monospace(ctx.coins(&receipt.fee));
                }
                (None, false) => {
                    ui.label(RichText::new("—").color(look.palette.muted));
                    ui.label(RichText::new("—").color(look.palette.muted));
                }
            }

            ui.end_row();
        }
    });
}

/// A transaction as a phone lists it: what it is and how it went, then who to whom, then where.
fn tx_rows(ui: &mut Ui, ctx: &mut Ctx, line: &TxLine, waiting: bool) {
    let look = ctx.look;
    let tx = line.tx;

    ui.horizontal(|ui| {
        ui.label(widgets::strong(tx.kind(), 13.5).color(look.palette.ink));
        self::tx(ui, ctx, &tx.id);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| match &tx.receipt {
            Some(receipt) => status(ui, look, &receipt.status),
            None if waiting => {
                widgets::pill(ui, "waiting", look.palette.warn);
            }
            None => {}
        });
    });
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        address(ui, ctx, &tx.sender);

        if let Some(target) = tx.target() {
            ui.label(RichText::new("→").color(look.palette.muted));
            address(ui, ctx, target);
        }
    });
    ui.horizontal_wrapped(|ui| {
        let muted = |text: String| RichText::new(text).small().color(look.palette.muted);

        match (line.index, line.block) {
            (Some(index), _) => {
                ui.label(muted(format!("#{index}")));
            }
            (None, Some((block, height))) => {
                ui.label(muted("block".to_string()));
                block_link(ui, ctx, block, height);
            }
            (None, None) => {
                ui.label(muted(format!("nonce {}", tx.nonce)));
            }
        }

        if let Some(receipt) = &tx.receipt {
            ui.label(muted(format!("fee {}", ctx.coins(&receipt.fee))));
        }
    });
}

/// A block by its height, opening its page.
pub fn block_link(ui: &mut Ui, ctx: &mut Ctx, id: &str, height: u64) {
    block(ui, ctx, id, &format::thousands(&height.to_string()));
}

/// A call argument or event value, with addresses as links.
pub fn argument(ui: &mut Ui, ctx: &mut Ctx, argument: &Argument) {
    let text = argument.text();

    match argument.kind.as_str() {
        "address" => address(ui, ctx, &text),
        kind => {
            ui.label(RichText::new(&text).monospace()).on_hover_text(kind);
        }
    }
}

/// Any JSON, as a tree to open.
pub fn json_tree(ui: &mut Ui, ctx: &mut Ctx, name: &str, value: &Value) {
    let muted = ctx.palette().muted;

    match value {
        Value::Object(map) => {
            egui::CollapsingHeader::new(name).id_salt(ui.next_auto_id()).default_open(map.len() <= 8).show(ui, |ui| {
                for (key, value) in map {
                    json_tree(ui, ctx, key, value);
                }
            });
        }
        Value::Array(items) => {
            egui::CollapsingHeader::new(format!("{name} [{}]", items.len()))
                .id_salt(ui.next_auto_id())
                .default_open(items.len() <= 4)
                .show(ui, |ui| {
                    for (index, item) in items.iter().enumerate() {
                        json_tree(ui, ctx, &index.to_string(), item);
                    }
                });
        }
        Value::String(text) => {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(format!("{name}:")).color(muted));

                match text.contains(':') && text.contains("0x") {
                    true => address(ui, ctx, text),
                    false => copyable(ui, text),
                }
            });
        }
        other => {
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{name}:")).color(muted));
                ui.monospace(other.to_string());
            });
        }
    }
}
