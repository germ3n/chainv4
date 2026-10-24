//! The explorer's pages, and the small pieces they share: links to blocks, transactions and
//! accounts, amounts in the right units, and a view of any JSON.

use {
    crate::api::{Ask, Api},
    crate::format,
    crate::model::Argument,
    crate::store::Store,
    egui::{Color32, RichText, Ui},
    serde::{Deserialize, Serialize},
    serde_json::{Value, json},
    std::collections::HashMap,
};

pub mod account;
pub mod block;
pub mod events;
pub mod finality;
pub mod hops;
pub mod map;
pub mod mempool;
pub mod network;
pub mod overview;
pub mod send;
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
    Network,
    Mempool,
    Finality,
    Send,
}

impl View {
    /// The pages the tab bar lists.
    pub const TABS: [(View, &'static str); 6] = [
        (View::Overview, "Overview"),
        (View::Map, "Block map"),
        (View::Network, "Network"),
        (View::Mempool, "Mempool"),
        (View::Finality, "Finality"),
        (View::Send, "Send"),
    ];
}

/// What the explorer remembers between runs.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The node's HTTP API.
    pub node: String,
    /// Decimals set by hand per token contract, by chain (its genesis id), then contract.
    pub decimals: HashMap<String, HashMap<String, u8>>,
}

pub const GOOD: Color32 = Color32::from_rgb(90, 190, 120);
pub const BAD: Color32 = Color32::from_rgb(230, 90, 80);
pub const MAIN: Color32 = Color32::from_rgb(80, 140, 225);
pub const SIDE: Color32 = Color32::from_rgb(230, 150, 60);
pub const FINAL: Color32 = Color32::from_rgb(70, 170, 110);
pub const JUSTIFIED: Color32 = Color32::from_rgb(80, 200, 210);

/// What a page works with.
pub struct Ctx<'a> {
    pub store: &'a Store,
    pub api: &'a mut Api,
    /// Where to go next, when a page says.
    pub go: &'a mut Option<View>,
    pub settings: &'a mut Settings,
    /// The time, in milliseconds since the Unix epoch.
    pub now: u64,
}

impl Ctx<'_> {
    pub fn go(&mut self, view: View) {
        *self.go = Some(view);
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

    /// A token's decimals and symbol when the contract is one, asking for what is not known yet.
    pub fn token(&mut self, contract: &str) -> Option<(u8, String)> {
        match self.store.contracts.get(contract) {
            None => {
                self.api.ask(Ask::Contract(contract.to_string()));

                return None;
            }
            Some(Some(record)) if record.is_token() => {}
            Some(_) => return None,
        }

        let token = self.store.tokens.get(contract).cloned().unwrap_or_default();

        for (function, known) in [("decimals", token.decimals.is_some()), ("symbol", token.symbol.is_some()), ("name", token.name.is_some())] {
            if !known {
                self.api.ask(Ask::Call {
                    contract: contract.to_string(),
                    function: function.to_string(),
                    args: "[]".to_string(),
                });
            }
        }

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

/// An address that opens its account page. Text that is not an address shows as it is.
pub fn address(ui: &mut Ui, ctx: &mut Ctx, address: &str) {
    if !address.contains(':') {
        ui.label(address);

        return;
    }

    let response = ui.link(RichText::new(format::short(address)).monospace()).on_hover_text(address);

    if response.clicked() {
        ctx.go(View::Account(address.to_string()));
    }

    copy_menu(&response, address);
}

/// A block id or height that opens the block's page.
pub fn block(ui: &mut Ui, ctx: &mut Ctx, block: &str, label: &str) {
    let response = ui.link(RichText::new(label).monospace()).on_hover_text(block);

    if response.clicked() {
        ctx.go(View::Block(block.to_string()));
    }

    copy_menu(&response, block);
}

/// A transaction id that opens its page.
pub fn tx(ui: &mut Ui, ctx: &mut Ctx, id: &str) {
    let response = ui.link(RichText::new(format::short(id)).monospace()).on_hover_text(id);

    if response.clicked() {
        ctx.go(View::Tx(id.to_string()));
    }

    copy_menu(&response, id);
}

/// Text that copies itself when clicked.
pub fn copyable(ui: &mut Ui, text: &str) {
    let response = ui
        .add(egui::Label::new(RichText::new(text).monospace()).sense(egui::Sense::click()))
        .on_hover_text("Click to copy");

    if response.clicked() {
        ui.ctx().copy_text(text.to_string());
    }
}

fn copy_menu(response: &egui::Response, text: &str) {
    response.context_menu(|ui| {
        if ui.button("Copy").clicked() {
            ui.ctx().copy_text(text.to_string());
            ui.close();
        }
    });
}

/// A transaction's status, colored.
pub fn status(ui: &mut Ui, status: &str) {
    let color = match status {
        "failed" => BAD,
        _ => GOOD,
    };

    ui.colored_label(color, status);
}

/// A colored dot, as fonts may lack one.
pub fn dot(ui: &mut Ui, color: Color32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.5, color);

    response
}

/// A small colored tag.
pub fn badge(ui: &mut Ui, text: &str, color: Color32) {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.25))
        .stroke(egui::Stroke::new(1.0, color))
        .corner_radius(4.0)
        .inner_margin(egui::Margin::symmetric(6, 1))
        .show(ui, |ui| {
            ui.colored_label(color, RichText::new(text).small());
        });
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
            ui.horizontal(|ui| {
                ui.weak(format!("{name}:"));

                match text.contains(':') && text.contains("0x") {
                    true => address(ui, ctx, text),
                    false => copyable(ui, text),
                }
            });
        }
        other => {
            ui.horizontal(|ui| {
                ui.weak(format!("{name}:"));
                ui.monospace(other.to_string());
            });
        }
    }
}

/// A two-column grid of labels and values.
pub fn facts(ui: &mut Ui, id: &str, add: impl FnOnce(&mut Ui)) {
    egui::Grid::new(id).num_columns(2).striped(true).spacing([16.0, 4.0]).show(ui, add);
}

/// A label in a facts grid.
pub fn fact(ui: &mut Ui, name: &str) {
    ui.weak(name);
}

/// What to show while something loads, or why it did not.
pub fn waiting(ui: &mut Ui, ctx: &Ctx, ask: &Ask) {
    match ctx.store.failures.get(ask) {
        Some(failure) if failure.status == 404 => {
            ui.weak(format!("Not found: {}", failure.message));
        }
        Some(failure) => {
            ui.colored_label(BAD, &failure.message);
        }
        None => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.weak("Loading…");
            });
        }
    }
}

/// A heading with room under it.
pub fn title(ui: &mut Ui, text: &str) {
    ui.add_space(4.0);
    ui.heading(text);
    ui.add_space(4.0);
}
