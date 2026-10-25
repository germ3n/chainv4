//! The wallet's pages, each a column of cards, and the pieces they share: how a proven value says
//! how far it is trusted, amounts in the coin or a token, and addresses to copy.

mod accounts;
mod activity;
mod overview;
mod receive;
pub mod send;
mod settings;

use {
    crate::app::{Coin, Holder, Settings},
    crate::chain::Chain,
    crate::format,
    crate::node::Node,
    crate::theme::{self, Palette},
    crate::vault::Vault,
    chainv4::light::Assurance,
    egui::{RichText, Ui},
};

/// Which page shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Overview,
    Send,
    Receive,
    Activity,
    Accounts,
    Settings,
}

impl Page {
    /// The pages the sidebar leads to, in its order.
    pub const NAV: [(Page, &'static str); 6] = [
        (Page::Overview, "Overview"),
        (Page::Send, "Send"),
        (Page::Receive, "Receive"),
        (Page::Activity, "Activity"),
        (Page::Accounts, "Accounts"),
        (Page::Settings, "Settings"),
    ];
}

/// What a page works with.
pub struct Ctx<'a> {
    pub palette: &'a Palette,
    pub chain: &'a Chain,
    pub node: &'a mut Node,
    pub holders: &'a mut Vec<Holder>,
    /// The account in use, by its place among the holders.
    pub current: usize,
    pub coin: &'a Coin,
    pub settings: &'a mut Settings,
    /// Milliseconds since the Unix epoch.
    pub now: u64,
    pub page: &'a mut Page,
    /// A node to talk to instead, once the page is drawn.
    pub connect: &'a mut Option<String>,
    /// Whether the accounts changed, so the vault seals them again.
    pub changed: &'a mut bool,
    pub vault: &'a mut Option<Vault>,
    pub wake: &'a egui::Context,
}

impl Ctx<'_> {
    pub fn holder(&self) -> Option<&Holder> {
        self.holders.get(self.current)
    }

    /// The account in use, by its address text.
    pub fn address(&self) -> String {
        self.holder().map(|holder| holder.address.to_string()).unwrap_or_default()
    }

    /// A token's symbol and decimals, as its proven registry entry says: its short address and no
    /// decimals until then.
    pub fn token(&self, token: &str) -> (String, u8) {
        match self.chain.tokens.get(token) {
            Some(proven) => (
                proven.value.symbol.clone().unwrap_or_else(|| format::short(token)),
                proven.value.decimals.unwrap_or(0),
            ),
            None => (format::short(token), 0),
        }
    }

    /// An amount of the coin, or of a token, with its symbol.
    pub fn amount(&self, token: Option<&str>, raw: &str) -> String {
        match token {
            None => format::amount(raw, self.coin.decimals, &self.coin.symbol),
            Some(token) => {
                let (symbol, decimals) = self.token(token);

                format::amount(raw, decimals, &symbol)
            }
        }
    }
}

/// What each page keeps between frames.
#[derive(Default)]
pub struct States {
    pub send: send::SendState,
    accounts: accounts::AccountsState,
    settings: settings::SettingsState,
}

/// Draws the page.
pub fn show(ui: &mut Ui, ctx: &mut Ctx, states: &mut States) {
    if ctx.holders.is_empty() && !matches!(*ctx.page, Page::Accounts | Page::Settings) {
        *ctx.page = Page::Accounts;
    }

    match *ctx.page {
        Page::Overview => overview::show(ui, ctx),
        Page::Send => send::show(ui, ctx, &mut states.send),
        Page::Receive => receive::show(ui, ctx),
        Page::Activity => activity::show(ui, ctx),
        Page::Accounts => accounts::show(ui, ctx, &mut states.accounts),
        Page::Settings => settings::show(ui, ctx, &mut states.settings),
    }
}

/// The light client's state, in the sidebar: how far it follows, and how much of that is final.
pub fn light_status(ui: &mut Ui, palette: &Palette, chain: &Chain) {
    let (color, text) = match (&chain.trouble, chain.following(), chain.caught_up()) {
        (Some(_), _, _) => (palette.bad, "Stopped".to_string()),
        (None, None, _) => (palette.muted, "Starting".to_string()),
        (None, Some((tip, _)), false) => (palette.warn, format!("Catching up · {}", format::thousands(&tip.to_string()))),
        (None, Some((tip, _)), true) => (palette.good, format!("Following · {}", format::thousands(&tip.to_string()))),
    };
    let response = ui
        .horizontal(|ui| {
            theme::dot(ui, color);
            ui.label(RichText::new(text).color(palette.soft).size(13.0));
        })
        .response;
    let detail = match (&chain.trouble, chain.following()) {
        (Some(why), _) => why.clone(),
        (None, Some((tip, final_height))) => format!(
            "Every header to height {tip} checked by consensus, and finality to height {final_height} by the voters it proved. Nothing the node says is shown unless it checks against them."
        ),
        (None, None) => "Asking the node where the chain starts.".to_string(),
    };
    response.on_hover_text(detail);
}

/// How far a proven value is trusted, as a pill.
pub fn assurance(ui: &mut Ui, palette: &Palette, assurance: &Assurance) {
    let (text, color) = match assurance.final_ {
        true => ("Final", palette.good),
        false => ("Proven", palette.accent),
    };
    let detail = match assurance.final_ {
        true => format!("Proven at block {}, which is final: it cannot be undone.", assurance.height),
        false => format!("Proven at block {}, which is not final yet: a reorg could still undo it.", assurance.height),
    };

    theme::pill(ui, text, color).on_hover_text(detail);
}

/// An address in the mono face, cut short, with the whole one on hover and a copy button.
pub fn address(ui: &mut Ui, palette: &Palette, address: &str) {
    ui.horizontal(|ui| {
        ui.label(theme::mono(format::short(address)).color(palette.soft)).on_hover_text(address);
        copy(ui, palette, address);
    });
}

/// A small button that copies text.
pub fn copy(ui: &mut Ui, palette: &Palette, text: &str) {
    let button = egui::Button::new(RichText::new("Copy").size(11.5).color(palette.muted)).fill(egui::Color32::TRANSPARENT).stroke(egui::Stroke::NONE);

    if ui.add(button).on_hover_text("Copy to the clipboard").clicked() {
        ui.ctx().copy_text(text.to_string());
    }
}

/// A row of a card: a label on the left, what it says on the right.
pub fn row(ui: &mut Ui, palette: &Palette, label: &str, add: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.add_sized([128.0, 20.0], egui::Label::new(RichText::new(label).color(palette.muted)));
        add(ui);
    });
}

/// A note that something is still on its way.
pub fn waiting(ui: &mut Ui, palette: &Palette, text: &str) {
    ui.horizontal(|ui| {
        ui.spinner();
        ui.label(RichText::new(text).color(palette.muted));
    });
}
