//! The wallet's pages, each a column of cards, and the pieces they share: how a proven value says
//! how far it is trusted, amounts in the coin or a token, and addresses to copy.

mod accounts;
mod activity;
#[cfg(feature = "bridge")]
pub mod bridge;
mod contracts;
mod faucet;
pub mod dapp;
#[cfg(feature = "swap")]
pub mod evm;
pub(crate) mod multisig;
mod names;
#[cfg(feature = "nft")]
mod nfts;
mod overview;
mod pair;
mod receive;
pub mod scan;
pub mod send;
mod settings;
mod sign;
#[cfg(feature = "swap")]
mod swaps;
#[cfg(feature = "dex")]
mod trade;

use {
    crate::app::{Coin, Role, Settings},
    crate::chain::Chain,
    crate::format,
    crate::keys::{self, Holder},
    crate::node::Node,
    crate::theme::{self, Palette},
    crate::vault::{Book, Vault},
    chainv4::light::Assurance,
    chainv4::structs::address::Address,
    egui::{RichText, Ui},
};

/// Which page shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Overview,
    Send,
    Receive,
    Activity,
    Names,
    /// Native multisigs: making one, and taking their transactions round the members.
    Multisig,
    Contracts,
    /// Atomic swaps between this chain, Bitcoin and EVM chains.
    #[cfg(feature = "swap")]
    Swaps,
    /// Trading on the DEXes: swaps of tokens, and pools to add to.
    #[cfg(feature = "dex")]
    Trade,
    /// What the accounts hold of the chain's NFT and multi-token collections.
    #[cfg(feature = "nft")]
    Nfts,
    /// The bridges: to and from EVM chains, and bitcoin in and out as wBTC.
    #[cfg(feature = "bridge")]
    Bridge,
    Accounts,
    Settings,
    /// An offline signer's: a request to sign, read off the sender's screen.
    Sign,
    /// An offline signer's: an account's public keys, for a sender to watch.
    Pair,
}

/// The pages the sidebar leads to, in its order. An offline signer follows no chain, so it has
/// nothing to show of one: its pages are for signing.
pub fn nav(role: Role) -> &'static [(Page, &'static str)] {
    match role {
        Role::Online => &[
            (Page::Overview, "Overview"),
            (Page::Send, "Send"),
            (Page::Receive, "Receive"),
            (Page::Activity, "Activity"),
            #[cfg(feature = "nft")]
            (Page::Nfts, "NFTs"),
            (Page::Names, "Names"),
            (Page::Multisig, "Multisig"),
            (Page::Contracts, "Contracts"),
            #[cfg(feature = "swap")]
            (Page::Swaps, "Swaps"),
            #[cfg(feature = "dex")]
            (Page::Trade, "Trade"),
            #[cfg(feature = "bridge")]
            (Page::Bridge, "Bridge"),
            (Page::Accounts, "Accounts"),
            (Page::Settings, "Settings"),
        ],
        Role::Signer => &[
            (Page::Sign, "Sign"),
            (Page::Pair, "Pair"),
            (Page::Receive, "Receive"),
            (Page::Accounts, "Accounts"),
            (Page::Settings, "Settings"),
        ],
    }
}

/// The page a wallet opens on.
pub fn home(role: Role) -> Page {
    nav(role)[0].0
}

/// What a page works with.
pub struct Ctx<'a> {
    pub palette: &'a Palette,
    pub chain: &'a Chain,
    pub node: &'a mut Node,
    pub holders: &'a mut Vec<Holder>,
    /// The account in use, by its place among the holders.
    pub current: usize,
    /// What the wallet's vault holds besides its accounts: the seed phrase.
    pub book: &'a mut Book,
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
    /// Whether to lock once the page is drawn, without sealing again: the wallet open was removed.
    pub leave: &'a mut bool,
    /// Why to lock once the page is drawn, sealing what changed first, for the lock screen to say:
    /// as when biometrics found the fingers enrolled changed, so the password is needed again.
    pub lock: &'a mut Option<String>,
    /// What wakes the app when something arrives later, as a key file picked.
    #[cfg_attr(target_os = "android", allow(dead_code))]
    pub wake: &'a egui::Context,
    /// The node's faucet, as GET /status describes it, on a node that runs one.
    pub faucet: Option<&'a serde_json::Value>,
    /// The swap contract the node names, if it names one.
    #[cfg_attr(not(feature = "swap"), allow(dead_code))]
    pub swap_contract: Option<&'a str>,
    /// The DEX, and the EVM DEX's router, the node names, if it names them.
    #[cfg_attr(not(feature = "dex"), allow(dead_code))]
    pub dex: (Option<&'a str>, Option<&'a str>),
    /// The Bitcoin light clients running, and what they found.
    #[cfg(all(feature = "bitcoin-light", not(target_arch = "wasm32")))]
    pub lights: &'a mut crate::light::Lights,
}

impl Ctx<'_> {
    pub fn holder(&self) -> Option<&Holder> {
        self.holders.get(self.current)
    }

    /// The account in use's addresses, as text.
    pub fn addresses(&self) -> Vec<String> {
        self.holder().map(|holder| holder.addresses().iter().map(Address::to_string).collect()).unwrap_or_default()
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

    /// An amount cut to four decimals, as a row has room for, with its symbol.
    pub fn brief(&self, token: Option<&str>, raw: &str) -> String {
        let (symbol, decimals) = match token {
            None => (self.coin.symbol.clone(), self.coin.decimals),
            Some(token) => self.token(token),
        };

        format!("{} {symbol}", format::compact(raw, decimals, 4))
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
    names: names::NamesState,
    pub multisig: multisig::MultisigState,
    contracts: contracts::ContractsState,
    receive: receive::ReceiveState,
    accounts: accounts::AccountsState,
    settings: settings::SettingsState,
    sign: sign::SignState,
    pub dapp: dapp::DappState,
    pub faucet: faucet::FaucetState,
    #[cfg(feature = "swap")]
    swaps: swaps::SwapsState,
    #[cfg(feature = "dex")]
    pub trade: trade::TradeState,
    #[cfg(feature = "nft")]
    pub nfts: nfts::NftsState,
    #[cfg(feature = "bridge")]
    bridge: bridge::BridgeState,
}

impl States {
    /// Takes the node's answer to something the Contracts page asked.
    pub fn answered(&mut self, ask: &crate::node::Ask, result: &Result<serde_json::Value, crate::node::Failure>, kept: &[crate::app::SavedContract]) {
        #[cfg(feature = "swap")]
        if self.swaps.answered(ask, result) {
            return;
        }

        #[cfg(feature = "dex")]
        if self.trade.answered(ask, result) {
            return;
        }

        #[cfg(feature = "nft")]
        if self.nfts.answered(ask, result) {
            return;
        }

        #[cfg(feature = "bridge")]
        if self.bridge.answered(ask, result) {
            return;
        }

        self.contracts.answered(ask, result, kept);
    }

    /// The names the pages want proven: one typed to send to, and one searched for.
    pub fn wanted_names(&self) -> Vec<String> {
        self.send.wanted_name().into_iter().chain(self.names.wanted()).collect()
    }
}

/// Draws the page.
pub fn show(ui: &mut Ui, ctx: &mut Ctx, states: &mut States) {
    // What a site asks comes before any page.
    if ctx.settings.role() == Role::Online && dapp::show(ui, ctx, &mut states.dapp) {
        return;
    }

    if ctx.holders.is_empty() && !matches!(*ctx.page, Page::Accounts | Page::Settings) {
        *ctx.page = Page::Accounts;
    }

    // A page this install's role has none of, as one left from before the role changed.
    if !nav(ctx.settings.role()).iter().any(|(page, _)| page == ctx.page) {
        *ctx.page = home(ctx.settings.role());
    }

    // Words and keys shown go out of sight once their page does.
    if *ctx.page != Page::Accounts {
        states.accounts.hide();
    }

    if *ctx.page != Page::Settings {
        states.settings.hide();
    }

    match *ctx.page {
        Page::Overview => {
            overview::show(ui, ctx);
            faucet::card(ui, ctx, &mut states.faucet);
        }
        Page::Send => send::show(ui, ctx, &mut states.send),
        Page::Receive => receive::show(ui, ctx, &mut states.receive),
        Page::Activity => activity::show(ui, ctx),
        Page::Names => names::show(ui, ctx, &mut states.names, &mut states.send),
        Page::Multisig => multisig::show(ui, ctx, &mut states.multisig, &mut states.send),
        Page::Contracts => contracts::show(ui, ctx, &mut states.contracts, &mut states.send),
        #[cfg(feature = "swap")]
        Page::Swaps => swaps::show(ui, ctx, &mut states.swaps, &mut states.send),
        #[cfg(feature = "dex")]
        Page::Trade => trade::show(ui, ctx, &mut states.trade, &mut states.send),
        #[cfg(feature = "nft")]
        Page::Nfts => nfts::show(ui, ctx, &mut states.nfts, &mut states.send),
        #[cfg(feature = "bridge")]
        Page::Bridge => bridge::show(ui, ctx, &mut states.bridge, &mut states.send),
        Page::Accounts => accounts::show(ui, ctx, &mut states.accounts),
        Page::Settings => settings::show(ui, ctx, &mut states.settings),
        Page::Sign => sign::show(ui, ctx, &mut states.sign),
        Page::Pair => pair::show(ui, ctx),
    }
}

pub use chainv4_ui::widgets::beside;

/// The light client's state, in the sidebar: how far it follows, and how much of that is final.
pub fn light_status(ui: &mut Ui, palette: &Palette, chain: &Chain) {
    let (color, text, detail) = following(palette, chain);
    let response = ui
        .horizontal(|ui| {
            theme::dot(ui, color);
            ui.label(RichText::new(text).color(palette.soft).size(13.0));
        })
        .response;
    response.on_hover_text(detail);
}

/// The light client's state as a dot alone, where there is no room for words: they show on a tap.
pub fn light_dot(ui: &mut Ui, palette: &Palette, chain: &Chain) {
    let (color, text, detail) = following(palette, chain);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::click());
    ui.painter().circle_filled(rect.center(), 4.5, color);
    response.on_hover_text(format!("{text}. {detail}"));
}

/// The light client's state: its color, in a word or two, and in full.
fn following(palette: &Palette, chain: &Chain) -> (egui::Color32, String, String) {
    let (color, text) = match (&chain.trouble, chain.following(), chain.caught_up()) {
        (Some(_), _, _) => (palette.bad, "Stopped".to_string()),
        (None, None, _) => (palette.muted, "Starting".to_string()),
        (None, Some((tip, _)), false) => (palette.warn, format!("Catching up · {}", format::thousands(&tip.to_string()))),
        (None, Some((tip, _)), true) => (palette.good, format!("Following · {}", format::thousands(&tip.to_string()))),
    };
    let detail = match (&chain.trouble, chain.following()) {
        (Some(why), _) => why.clone(),
        (None, Some((tip, final_height))) => format!(
            "Every header to height {tip} checked by consensus, and finality to height {final_height} by the voters it proved. Nothing the node says is shown unless it checks against them."
        ),
        (None, None) => "Asking the node where the chain starts.".to_string(),
    };

    (color, text, detail)
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

/// The scheme an address belongs to, as a page names it.
pub fn scheme(address: &Address) -> &'static str {
    keys::scheme_of(address).map_or("Contract", keys::label)
}

/// An address in the mono face, cut short, with the whole one on hover. Clicking or tapping it
/// copies it whole.
pub fn address(ui: &mut Ui, palette: &Palette, address: &str) {
    ui.horizontal(|ui| {
        let label = egui::Label::new(theme::mono(format::short(address)).color(palette.soft));
        chainv4_ui::widgets::copy_on_click(ui, palette, label, address).on_hover_text(format!("{address}\nClick to copy"));
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

/// How long each part of a message shows, as the display goes through them: about three a
/// second, so a phone's camera catches enough within a few seconds.
const PART_SECONDS: f64 = 0.33;

/// An animated code: its fountain encoder, the part showing, and whether it is paused there.
#[derive(Clone)]
struct Cycling {
    encoder: std::sync::Arc<std::sync::Mutex<ur::Encoder<'static>>>,
    part: String,
    paused: bool,
    /// When the part showing came up.
    since: f64,
}

impl Cycling {
    fn next(&mut self, now: f64) {
        let mut encoder = self.encoder.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        self.part = encoder.next_part().map(|part| part.to_ascii_uppercase()).unwrap_or_default();
        self.since = now;
    }

    /// Which part shows, and how many parts the message is cut into.
    fn counted(&self) -> (usize, usize) {
        let encoder = self.encoder.lock().unwrap_or_else(std::sync::PoisonError::into_inner);

        (encoder.current_index(), encoder.fragment_count())
    }
}

/// A message for the other device: its QR code, as wide as there is room for up to what a camera
/// reads comfortably off a screen; or for one too long for a code, fountain-coded parts, one after
/// another, of which the other device's camera needs any enough, in any order; and its whole text
/// to copy, for a file or where there is no camera.
pub fn code(ui: &mut Ui, palette: &Palette, message: &crate::airgap::Message) {
    let text = message.to_text();
    let bytes = message.bytes();
    let side = ui.available_width().clamp(200.0, 420.0);

    match bytes.len() <= crate::airgap::FRAGMENT {
        true => {
            ui.vertical_centered(|ui| receive::qr(ui, &text, side));
        }
        false => animated(ui, palette, message.ur_type(), &bytes, &text, side),
    }

    ui.add_space(8.0);
    ui.horizontal(|ui| {
        if theme::quiet(ui, palette, "Copy as text").on_hover_text("For a file, or where there is no camera").clicked() {
            ui.ctx().copy_text(text.clone());
        }

        ui.label(RichText::new(format!("{} characters", text.len())).small().color(palette.muted));
    });
}

fn animated(ui: &mut Ui, palette: &Palette, kind: &'static str, bytes: &[u8], text: &str, side: f32) {
    let id = egui::Id::unique(("airgap code shown", text));
    let now = ui.input(|input| input.time);
    let mut cycling = match ui.data(|data| data.get_temp::<Cycling>(id)) {
        Some(cycling) => cycling,
        None => {
            let encoder = ur::Encoder::new(bytes, crate::airgap::FRAGMENT, kind).expect("a message is never empty");
            let mut cycling = Cycling {
                encoder: std::sync::Arc::new(std::sync::Mutex::new(encoder)),
                part: String::new(),
                paused: false,
                since: now,
            };
            cycling.next(now);

            cycling
        }
    };

    if !cycling.paused && now - cycling.since >= PART_SECONDS {
        cycling.next(now);
    }

    let (index, count) = cycling.counted();
    theme::caps(ui, palette, &format!("Part {index} · {count} make it"));
    ui.label(
        RichText::new("Too long for one code: it shows parts one after another. Hold the other device's camera on it; any enough of them, in any order, make the message.")
            .small()
            .color(palette.muted),
    );
    ui.add_space(6.0);
    ui.vertical_centered(|ui| receive::qr(ui, &cycling.part, side));
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui.button(if cycling.paused { "Play" } else { "Pause" }).clicked() {
            cycling.paused = !cycling.paused;
            cycling.since = now;
        }

        // Stepping by hand pauses it there.
        if ui.button("Next part").clicked() {
            cycling.next(now);
            cycling.paused = true;
        }
    });

    if !cycling.paused {
        ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64((PART_SECONDS - (now - cycling.since)).max(0.02)));
    }

    ui.data_mut(|data| data.insert_temp(id, cycling));
}

/// Text to read, as it is: a message to sign, or typed data's fields.
pub fn text_block(ui: &mut Ui, palette: &Palette, text: &str) {
    egui::Frame::new().fill(palette.row).corner_radius(8.0).inner_margin(10).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.add(egui::Label::new(theme::mono(text).color(palette.ink)).wrap());
    });
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::airgap::{Message, Pairing},
    };

    /// Draws the message's code at each of these times, and says what showed.
    fn shown(ctx: &egui::Context, message: &Message, times: &[f64]) -> Vec<String> {
        let id = egui::Id::unique(("airgap code shown", message.to_text().as_str()));

        times
            .iter()
            .map(|time| {
                let mut drawn = ctx.run_ui(egui::RawInput { time: Some(*time), ..Default::default() }, |ui| code(ui, &crate::theme::DARK, message));
                drawn.textures_delta.clear();

                ctx.data(|data| data.get_temp::<Cycling>(id)).map(|cycling| cycling.part).unwrap_or_default()
            })
            .collect()
    }

    fn pairing(keys: usize) -> Message {
        let key = chainv4::crypto::keypair::Keypair::from_seed([7; 32]).public();

        Message::Pair(Pairing { name: "Main".to_string(), keys: vec![key; keys], origin: None })
    }

    #[test]
    fn a_long_message_animates_fountain_parts_about_three_a_second_until_paused() {
        let ctx = egui::Context::default();
        let long = pairing(40);

        assert!(long.bytes().len() > crate::airgap::FRAGMENT * 3);

        // A new part each third of a second, and past the first round, mixed ones that go on.
        let times: Vec<f64> = (0..12).map(|step| f64::from(step) * 0.34).collect();
        let parts = shown(&ctx, &long, &times);

        assert!(parts.iter().all(|part| part.starts_with("UR:CHAINV4-PAIRING/")), "{parts:?}");
        assert_eq!(parts.iter().collect::<std::collections::BTreeSet<_>>().len(), parts.len(), "never the same part twice in a row");

        // They read back into the message, from any enough of them.
        let mut assembly = crate::hardware::Assembly::default();
        let read = parts.iter().skip(2).find_map(|part| assembly.add(part).unwrap());

        assert_eq!(read.map(|(kind, bytes)| Message::from_ur(&kind, &bytes)), Some(Ok(long.clone())));

        // Paused, it stays; a message that fits one code never animates.
        let id = egui::Id::unique(("airgap code shown", long.to_text().as_str()));
        ctx.data_mut(|data| {
            let mut cycling = data.get_temp::<Cycling>(id).unwrap();
            cycling.paused = true;
            data.insert_temp(id, cycling);
        });
        let held = shown(&ctx, &long, &[5.0, 6.0, 9.0]);

        assert!(held.windows(2).all(|pair| pair[0] == pair[1]));
        assert_eq!(shown(&ctx, &pairing(1), &[10.0, 11.0]), ["", ""], "one code, whole");
    }
}
