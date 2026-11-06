//! The explorer's pages, and the pieces they share: sections, facts that stack on a phone, tables
//! that become rows, links to blocks, transactions and accounts, amounts in the right units, and a
//! view of any JSON.

use {
    crate::api::{Api, Ask, Failure},
    crate::format,
    crate::model::{Argument, TxView},
    crate::store::Store,
    crate::theme::Roles,
    chainv4_ui::{Palette, widgets},
    egui::{Color32, RichText, Ui},
    serde::{Deserialize, Serialize},
    serde_json::{Value, json},
    std::collections::{HashMap, HashSet},
};

pub mod account;
pub mod block;
pub mod bridge;
pub mod chart;
pub mod collections;
pub mod decoded;
pub mod dex;
pub mod events;
pub mod faucet;
pub mod finality;
pub mod governance;
pub mod hops;
pub mod map;
pub mod mempool;
pub mod more;
pub mod multisig;
pub mod names;
pub mod network;
pub mod overview;
pub mod pool;
pub mod reading;
pub mod rich;
pub mod send;
pub mod name;
pub mod settings;
pub mod swaps;
pub mod tokens;
pub mod tools;
pub mod tx;
pub mod update;

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
    /// The mining pool the node runs: its miners, their shares and earnings, and its blocks.
    Pool,
    Mempool,
    Tokens,
    /// The NFT and multi-token collections the registry lists.
    Collections,
    /// One collection: its tokens, who holds them, and what moved them.
    Collection(String),
    /// The DEXes: their pools, prices, trades and volume.
    Dex,
    /// Atomic swaps through the swap contract: locks, claims and refunds, and their finality.
    Swaps,
    /// The bridges: the federation, transfers with EVM chains, wrapped assets, and Bitcoin's.
    Bridges,
    /// One transfer through the gateway, by its digest.
    Transfer(String),
    /// One release of the Bitcoin gateway, by its number.
    Release(u64),
    Finality,
    /// Governance: who approves, the changes being approved, what is in force, and every decision.
    Governance,
    Send,
    /// The node's faucet, on a dev or test chain.
    Faucet,
    Settings,
    /// On a phone, the pages the tabs have no room for.
    More,
    /// Decoding, converting, hashing and checking proofs, offline or against the light client.
    Tools,
    /// A multisig: its members, threshold, nonce and open proposals. Empty for the lookup alone.
    Multisig(String),
    /// The name directory.
    Names,
    /// The richest accounts, and the coin's supply.
    Rich,
}

impl View {
    /// The sidebar's pages, in groups under a heading.
    pub const GROUPS: [(&'static str, &'static [(View, &'static str)]); 4] = [
        (
            "Chain",
            &[
                (View::Overview, "Overview"),
                (View::Map, "Block map"),
                (View::Mempool, "Mempool"),
                (View::Tokens, "Tokens"),
                (View::Collections, "NFTs"),
                (View::Multisig(String::new()), "Multisig"),
                (View::Names, "Names"),
                (View::Rich, "Rich list & supply"),
            ],
        ),
        ("Markets", &[(View::Dex, "DEX"), (View::Swaps, "Swaps"), (View::Bridges, "Bridges")]),
        ("Network", &[(View::Network, "Network"), (View::Finality, "Finality"), (View::Governance, "Governance"), (View::Pool, "Mining pool")]),
        ("Tools", &[(View::Tools, "Tools"), (View::Send, "Send"), (View::Faucet, "Faucet"), (View::Settings, "Settings")]),
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
    pub const MORE: [(View, &'static str, &'static str); 15] = [
        (View::Collections, "NFTs", "Collections, their tokens and holders"),
        (View::Multisig(String::new()), "Multisig", "A multisig's members, threshold and open proposals"),
        (View::Names, "Names", "Every name, its owner and when it expires"),
        (View::Rich, "Rich list & supply", "The richest accounts, and the coin's proven supply"),
        (View::Dex, "DEX", "Pools, prices, trades and volume"),
        (View::Swaps, "Swaps", "Atomic swaps and their finality"),
        (View::Bridges, "Bridges", "What crossed to other chains, and wrapped assets"),
        (View::Network, "Network", "The node, its peers, and what it heard"),
        (View::Finality, "Finality", "Checkpoints, voters and caught cheats"),
        (View::Governance, "Governance", "Who approves changes, what is in force, and every decision"),
        (View::Pool, "Mining pool", "The node's pool: miners, shares and payouts"),
        (View::Tools, "Tools", "Decode, convert, hash, verify, and check proofs"),
        (View::Send, "Send", "Sign and send a transaction"),
        (View::Faucet, "Faucet", "Coins for a dev or test chain"),
        (View::Settings, "Settings", "The node, the theme, the light client"),
    ];

    /// Whether the sidebar's entry for this page is the one to mark: the page itself, or for the
    /// multisig lookup, any multisig.
    pub fn marks(&self, entry: &View) -> bool {
        self == entry || matches!((self, entry), (View::Multisig(_), View::Multisig(_)))
    }

    /// Whether the sidebar lists this entry, on this page: all of them, or of a group folded only the
    /// page open now, so the sidebar still says where you are.
    pub fn lists(&self, entry: &View, folded: bool) -> bool {
        !folded || self.marks(entry)
    }

    /// Which tab a page belongs under on a phone: a page of its own, or More for the rest.
    pub fn tab(&self) -> View {
        match self {
            View::Overview | View::Block(_) | View::Tx(_) | View::Account(_) | View::Name(_) => View::Overview,
            View::Map => View::Map,
            View::Mempool => View::Mempool,
            View::Tokens => View::Tokens,
            View::Collections
            | View::Collection(_)
            | View::Dex
            | View::Swaps
            | View::Bridges
            | View::Transfer(_)
            | View::Release(_)
            | View::Network
            | View::Finality
            | View::Governance
            | View::Pool
            | View::Send
            | View::Faucet
            | View::Settings
            | View::More
            | View::Tools
            | View::Multisig(_)
            | View::Names
            | View::Rich => View::More,
        }
    }
}

/// What the explorer remembers between runs.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The node's HTTP API.
    pub node: String,
    /// The newest release manifest seen, as JSON, so no node takes this explorer back past it.
    pub newest_release: Option<String>,
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
    /// What the light client checked, by genesis id, so a launch carries on from there rather than
    /// walk every header again: LightState, hex.
    #[serde(default)]
    pub light_saved: HashMap<String, String>,
    /// A finalized checkpoint's block id to start the light client from at each node, pasted,
    /// rather than genesis.
    pub checkpoints: HashMap<String, String>,
    /// The report token of each node that has one, by its address: GET /node shares everything
    /// with its bearer.
    pub report_tokens: HashMap<String, String>,
    /// The nodes trusted to start the light client at their own finalized checkpoint each launch,
    /// rather than it walking the chain from genesis.
    pub fast_resume: HashMap<String, bool>,
    /// The light palette rather than the dark one.
    pub light_theme: bool,
    /// The chain's name as each node last gave it, for the header before the node answers.
    pub names: HashMap<String, String>,
    /// The sidebar's groups folded, by name.
    pub folded: HashSet<String>,
    /// Whether to take everything from final blocks fetched and checked here, rather than ask a node.
    /// For the install, rather than for a node: changing the node never quietly starts asking it about
    /// everything. Off by default, as reading blocks costs far more than asking.
    pub read_blocks: bool,
    /// The coin's symbol and decimals as a node last said them, by chain (its genesis id): what shows
    /// with Read from checked blocks on, when no node is asked and nothing checked says them.
    pub coins: HashMap<String, (String, u8)>,
    /// Low data: ask the node a few times less often, for a metered connection or a phone's battery.
    /// See chainv4_ui::pace.
    pub low_data: bool,
    /// How many times as long as usual a request may take before a page says it got no answer:
    /// chainv4_ui::net::waits. 0, from settings kept before there was this, is the usual, as 1 is.
    pub patience: u32,
    /// The most bytes Read from checked blocks keeps, or None for crate::reading::MOST_BYTES.
    pub reading_bytes: Option<u64>,
}

impl Settings {
    /// How often the explorer asks its node, by Low data.
    pub fn pace(&self) -> chainv4_ui::pace::Pace {
        chainv4_ui::pace::Pace::new(self.low_data)
    }

    /// The most bytes Read from checked blocks keeps, within the bounds Settings offers.
    pub fn reading_bytes(&self) -> u64 {
        self.reading_bytes.unwrap_or(crate::reading::MOST_BYTES).clamp(crate::reading::LEAST_BYTES, crate::reading::MOST_BYTES_SET)
    }
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
    /// What reads EVM calls and logs, each once, however many rows and frames show them.
    pub reader: &'a chainv4::abi::known::AbiReader,
    /// The light client, and the balances it proved.
    pub light: &'a mut crate::light::Light,
    /// What was read from checked blocks, while that is on.
    pub reading: &'a mut crate::reading::Reading,
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

    /// The chain's coin: its decimals and symbol, 18 and nothing before the status comes. With Read
    /// from checked blocks on, plain units until then, as a wrong number of decimals misstates a proof.
    pub fn coin(&self) -> (u8, String) {
        let unknown = match self.settings.read_blocks {
            true => (0, "units".to_string()),
            false => (18, String::new()),
        };

        self.store.status.as_ref().map_or(unknown, |status| (status.coin.decimals, status.coin.symbol.clone()))
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

    /// The ABI kept for an EVM contract on this chain, read once.
    pub fn abi(&self, contract: &str) -> Option<std::rc::Rc<chainv4::abi::Abi>> {
        self.reader.abi(self.abi_text(contract)?)
    }

    /// The ABI kept for an EVM contract, as pasted.
    pub fn abi_text(&self, contract: &str) -> Option<&str> {
        let genesis = &self.store.status.as_ref()?.genesis;

        self.settings.abis.get(genesis)?.get(contract).map(String::as_str)
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
    widgets::title_with(ui, &look.palette, title, about, pills);
}

/// A card with a heading in capitals: one part of a page.
pub fn section<R>(ui: &mut Ui, look: Look, title: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    section_with(ui, look, title, |_| {}, add)
}

/// A section with something on the right of its heading: a link, a filter, a count.
pub fn section_with<R>(ui: &mut Ui, look: Look, title: &str, right: impl FnOnce(&mut Ui), add: impl FnOnce(&mut Ui) -> R) -> R {
    widgets::page_card(ui, &look.palette, |ui| {
        ui.horizontal(|ui| {
            widgets::caps(ui, &look.palette, title);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), right);
        });
        ui.add_space(4.0);
        add(ui)
    })
}

/// A note in a section, muted, with a command's flags in the mono face.
pub fn note(ui: &mut Ui, look: Look, text: impl Into<String>) {
    widgets::flagged_label(ui, &text.into(), egui::TextStyle::Body, look.palette.muted);
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
            // Each cell from the top of its row, the name too: a value of many lines, wrapped
            // straight into the grid's cell, would start partway down and run over the rows below.
            false => {
                self.ui.vertical(|ui| ui.label(label));
                self.ui.vertical(|ui| ui.horizontal_wrapped(value));
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
                .striped(false)
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

/// Ends a row of a table, with the look's hairline under it, across the card.
pub fn row_end(ui: &mut Ui) {
    let (drawn, room) = (ui.min_rect(), ui.max_rect());
    let y = drawn.bottom() + ui.spacing().item_spacing.y.max(TABLE_ROW_GAP) / 2.0;
    let color = ui.visuals().widgets.noninteractive.bg_stroke.color;
    ui.painter().hline(drawn.left()..=room.right().max(drawn.right()), y, egui::Stroke::new(1.0, color));
    ui.end_row();
}

/// The gap between a table's rows, which their hairlines run down the middle of.
const TABLE_ROW_GAP: f32 = 8.0;

/// A table with headings in capitals, a hairline under each row. For a wide screen; a phone gets
/// rows. One wider than the page scrolls across by its bar, never by a drag, so a finger dragging
/// over it still scrolls the page.
pub fn table(ui: &mut Ui, look: Look, id: impl std::hash::Hash + std::fmt::Debug, headings: &[&str], rows: impl FnOnce(&mut Ui)) {
    egui::ScrollArea::horizontal().id_salt(("table", ui.scope_id(), &id)).scroll_source(NO_DRAG).show(ui, |ui| {
        egui::Grid::new(id).striped(false).spacing([14.0, TABLE_ROW_GAP]).num_columns(headings.len()).show(ui, |ui| {
            for heading in headings {
                widgets::caps(ui, &look.palette, heading);
            }

            row_end(ui);
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
            // The frame's stroke is outside its margin: a row of tiles fills its width, not past it.
            ui.set_width(width - 2.0 * f32::from(margin.left) - 2.0);
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.vertical(|ui| {
                widgets::caps(ui, &look.palette, label);
                widgets::fitted(ui, value, size, 10.0, egui::FontFamily::Monospace, look.palette.ink);
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

/// What to show while something loads, or why it did not, with a way to ask again.
pub fn waiting(ui: &mut Ui, ctx: &mut Ctx, ask: &Ask) {
    let palette = ctx.palette();

    match ctx.store.failures.get(ask) {
        Some(failure) if failure.status == crate::api::NEEDS_ASKING_STATUS => {
            if widgets::needs_asking(ui, &palette, &failure.sentence()) {
                ctx.go(View::Settings);
            }
        }
        // What was asked of what was read before there was anything to answer it.
        Some(failure) if failure.status == 425 => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new("Loading…").color(palette.muted));
            });
        }
        Some(failure) if failure.status == 404 && ctx.settings.read_blocks => {
            let why = failure.message.clone();

            reading::outside(ui, ctx, &why, None);
        }
        Some(failure) if failure.status == 404 => {
            ui.label(RichText::new(format!("Not found: {}", failure.message)).color(palette.muted));
        }
        // The node itself not answering: said at the top of every page, with Retry.
        Some(failure) if failure.is_trouble() && ctx.store.failures.get(&Ask::Status).is_some_and(Failure::is_trouble) => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new("Waiting for the node to answer…").color(palette.muted));
            });
        }
        Some(failure) => {
            if widgets::retry(ui, &palette, &failure.sentence(), ctx.api.asking(ask)) {
                ctx.api.retry(ask.clone());
            }
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

/// Whether text is an address of any kind, as the node writes them: kind:0x… or bc1q….
pub fn is_address(text: &str) -> bool {
    text.parse::<chainv4::structs::address::Address>().is_ok()
}

/// An address that opens its account page. Text that is not an address shows as it is.
pub fn address(ui: &mut Ui, ctx: &mut Ctx, address: &str) {
    if !is_address(address) {
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
    copyable_label(ui, egui::Label::new(RichText::new(text).monospace()).wrap(), text);
}

/// Text cut short, such as a peer id, that copies in full when clicked or tapped, and says so. It
/// is kept whole on one line, however narrow its table column: never a letter or two a line.
pub fn copyable_as(ui: &mut Ui, shown: RichText, full: &str) {
    copyable_label(ui, egui::Label::new(shown).extend(), full);
}

/// A label that copies some text in full when clicked or tapped, and says so.
pub fn copyable_label(ui: &mut Ui, label: egui::Label, full: &str) {
    let palette = palette_of(ui);
    widgets::copy_on_click(ui, &palette, label, full).on_hover_text(format!("{full}\nClick to copy"));
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

            row_end(ui);
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

                match is_address(text) {
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

/// A page drawn once, headless, at a screen width, for tests: the text it drew, and what it asked
/// the node.
#[cfg(test)]
pub(crate) fn render(store: &Store, light: &mut crate::light::Light, width: f32, draw: impl FnMut(&mut Ui, &mut Ctx)) -> (String, Vec<Ask>) {
    let (shown, asked) = render_frames(store, light, width, 1, draw);

    (shown.into_iter().map(|(text, _)| text + "\n").collect(), asked)
}

/// A page drawn some frames, headless, at a screen width, as one settles once what it drew last is
/// known, for tests: each text the last frame drew and where, and what it asked the node.
#[cfg(test)]
pub(crate) fn render_frames(store: &Store, light: &mut crate::light::Light, width: f32, frames: usize, mut draw: impl FnMut(&mut Ui, &mut Ctx)) -> (Vec<(String, egui::Rect)>, Vec<Ask>) {
    let mut api = Api::new("http://127.0.0.1:1", Err("offline".to_string()), egui::Context::default());
    let mut settings = Settings::default();
    let mut go = None;
    let mut reading = crate::reading::Reading::default();
    let palette = crate::theme::DARK;
    let look = Look { palette, roles: crate::theme::Roles::of(&palette), narrow: chainv4_ui::layout::is_narrow(width) };
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 2_000.0))),
        ..egui::RawInput::default()
    };
    let context = egui::Context::default();
    let mut shown = Vec::new();
    let reader = chainv4::abi::known::AbiReader::default();

    for _ in 0..frames {
        let mut output = context.run_ui(input.clone(), |ui| {
            let mut ctx = Ctx { store, reader: &reader, light: &mut *light, reading: &mut reading, api: &mut api, go: &mut go, settings: &mut settings, now: 2_000, look };
            draw(ui, &mut ctx);
        });
        output.textures_delta.clear();
        shown = output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::epaint::Shape::Text(shape) => Some((shape.galley.text().to_string(), shape.galley.rect.translate(shape.pos.to_vec2()))),
                _ => None,
            })
            .collect();
    }

    // The offline API answers at once: what was asked is in the inbox.
    let asked = api
        .replies()
        .into_iter()
        .filter_map(|reply| match reply {
            crate::api::Reply::Answer { ask, .. } => Some(ask),
            crate::api::Reply::Stream(_) => None,
        })
        .collect();

    (shown, asked)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folded_group_lists_only_the_page_open_and_settings_remember_which_are() {
        assert!(View::Overview.lists(&View::Dex, false));
        assert!(!View::Overview.lists(&View::Dex, true));
        assert!(View::Dex.lists(&View::Dex, true), "where you are stays in sight");
        assert!(View::Multisig("multisig:0x01".to_string()).lists(&View::Multisig(String::new()), true), "any multisig is that page");

        let settings = Settings { folded: HashSet::from(["Markets".to_string()]), ..Settings::default() };
        let again: Settings = serde_json::from_str(&serde_json::to_string(&settings).unwrap()).unwrap();
        assert!(again.folded.contains("Markets"));

        let older: Settings = serde_json::from_str(r#"{ "node": "http://node" }"#).unwrap();
        assert!(older.folded.is_empty(), "settings from before there were folds open every group");
    }

    #[test]
    fn a_table_rules_a_hairline_under_each_row_in_its_look() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx, &crate::theme::DARK);
        let palette = crate::theme::DARK;
        let look = Look { palette, roles: crate::theme::Roles::of(&palette), narrow: false };
        let mut hairlines = 0;

        for _ in 0..2 {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                table(ui, look, "test", &["what", "how much"], |ui| {
                    for row in 0..3 {
                        ui.label("a row");
                        ui.monospace(row.to_string());
                        row_end(ui);
                    }
                });
            });
            output.textures_delta.clear();
            hairlines = output.shapes.iter().filter(|clipped| matches!(&clipped.shape, egui::epaint::Shape::LineSegment { stroke, .. } if stroke.color == palette.line)).count();
        }

        assert_eq!(hairlines, 4, "one under the headings, and one under each row");
    }

    #[test]
    fn notes_set_flags_in_the_mono_face() {
        let ctx = egui::Context::default();
        let palette = crate::theme::DARK;
        let look = Look { palette, roles: crate::theme::Roles::of(&palette), narrow: false };
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| note(ui, look, "Start it with --index-events to search."));
        output.textures_delta.clear();
        let families: Vec<egui::FontFamily> = output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::epaint::Shape::Text(shape) => Some(shape.galley.job.sections.iter().map(|section| section.format.font_id.family.clone()).collect::<Vec<_>>()),
                _ => None,
            })
            .flatten()
            .collect();

        assert_eq!(families, [egui::FontFamily::Proportional, egui::FontFamily::Monospace, egui::FontFamily::Proportional]);
    }

    #[test]
    fn a_fact_of_many_lines_on_a_wide_screen_keeps_the_next_fact_below_it() {
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(340.0, 1_200.0));
        let palette = crate::theme::DARK;
        let look = Look { palette, roles: crate::theme::Roles::of(&palette), narrow: false };
        let (mut last, mut next) = (egui::Rect::NOTHING, egui::Rect::NOTHING);

        for _ in 0..4 {
            let mut output = ctx.run_ui(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |ui| {
                facts(ui, look, "test", |facts| {
                    facts.text("up for", "32m");
                    facts.row("listening", |ui| {
                        ui.vertical(|ui| {
                            for host in 0..8 {
                                last = ui.add(egui::Label::new(RichText::new(format!("/ip4/127.0.0.{host}/udp/50231/quic-v1/p2p/12D3Ko…n6Xb")).monospace()).wrap()).rect;
                            }
                        });
                    });
                    facts.row("chain's name", |ui| {
                        next = ui.label("chainv4 testnet").rect;
                    });
                });
            });
            output.textures_delta.clear();
        }

        assert!(next.top() >= last.bottom(), "the next fact under the last line: {next:?} {last:?}");
    }
}
