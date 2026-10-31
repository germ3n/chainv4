//! The wallet's pages, each a column of cards, and the pieces they share: how a proven value says
//! how far it is trusted, amounts in the coin or a token, and addresses to copy.

mod accounts;
mod activity;
mod batch;
#[cfg(feature = "bridge")]
pub mod bridge;
mod contracts;
mod decoded;
mod faucet;
pub mod dapp;
#[cfg(feature = "swap")]
pub mod evm;
mod governance;
#[cfg(feature = "mining")]
mod mine;
mod mining;
mod more;
pub(crate) mod multisig;
mod names;
#[cfg(feature = "nft")]
mod nfts;
mod overview;
mod pair;
mod reading;
mod receive;
pub mod scan;
pub mod send;
mod steps;
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
    crate::node::{Failure, Node},
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
    /// Several steps sent as one transaction, all or nothing.
    Batch,
    Receive,
    Activity,
    Names,
    /// Claiming what a pool owes the addresses and, in a build that mines, mining from this device,
    /// alone or in a pool.
    Mining,
    /// Native multisigs: making one, and taking their transactions round the members.
    Multisig,
    /// The chain's settings under governance: what stands, the changes open, and approving them.
    Governance,
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
    /// On a phone, the pages the tabs have no room for.
    More,
}

/// What the Mining page is called: in a build with no mining in it, as the app stores' are, nothing
/// suggests any, in a listing or a screenshot.
pub const MINING_NAME: &str = if cfg!(feature = "mining") { "Mining" } else { "Pool earnings" };

/// The pages the sidebar leads to, in its order. An offline signer follows no chain, so it has
/// nothing to show of one: its pages are for signing.
pub fn nav(role: Role) -> &'static [(Page, &'static str)] {
    match role {
        Role::Online => &[
            (Page::Overview, "Overview"),
            (Page::Send, "Send"),
            (Page::Batch, "Batch"),
            (Page::Receive, "Receive"),
            (Page::Activity, "Activity"),
            #[cfg(feature = "nft")]
            (Page::Nfts, "NFTs"),
            (Page::Names, "Names"),
            (Page::Mining, MINING_NAME),
            (Page::Multisig, "Multisig"),
            (Page::Governance, "Governance"),
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

/// A phone's tabs, along the bottom, each with what it is called there. An online wallet's are the
/// four it is mostly opened for, then More for the rest; an offline signer has no more pages than
/// fit, so it needs no More.
pub fn tabs(role: Role) -> &'static [(Page, &'static str)] {
    match role {
        Role::Online => &[(Page::Overview, "Home"), (Page::Send, "Send"), (Page::Receive, "Receive"), (Page::Activity, "Activity"), (Page::More, "More")],
        Role::Signer => nav(Role::Signer),
    }
}

/// The tab to mark while a page shows: its own, or More for a page under it.
pub fn tab_of(page: Page, role: Role) -> Page {
    match tabs(role).iter().any(|(tab, _)| *tab == page) {
        true => page,
        false => Page::More,
    }
}

/// Whether this install's role has a page.
fn has_page(role: Role, page: Page) -> bool {
    tabs(role).iter().chain(nav(role)).any(|(known, _)| *known == page)
}

/// The page a wallet opens on.
pub fn home(role: Role) -> Page {
    nav(role)[0].0
}

/// What a page works with.
pub struct Ctx<'a> {
    pub palette: &'a Palette,
    pub chain: &'a Chain,
    /// What reads EVM calls and logs, each once, however many frames show them.
    pub reader: &'a chainv4::abi::known::AbiReader,
    pub node: &'a mut Node,
    pub holders: &'a mut Vec<Holder>,
    /// The account in use, by its place among the holders.
    pub current: usize,
    /// What the wallet's vault holds besides its accounts: its seed phrases.
    pub book: &'a mut Book,
    pub coin: &'a Coin,
    pub settings: &'a mut Settings,
    /// What it reads from checked blocks, with that setting on.
    pub reading: &'a mut crate::reading::Reading,
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
    /// What wakes the app when something arrives later, as a key file picked or a pool's node's
    /// answer.
    pub wake: &'a egui::Context,
    /// The node's faucet, as GET /status describes it, on a node that runs one.
    pub faucet: Option<&'a serde_json::Value>,
    /// The swap contract the node names, if it names one.
    #[cfg_attr(not(feature = "swap"), allow(dead_code))]
    pub swap_contract: Option<&'a str>,
    /// The DEX, and the EVM DEX's router, the node names, if it names them.
    #[cfg_attr(not(feature = "dex"), allow(dead_code))]
    pub dex: (Option<&'a str>, Option<&'a str>),
    /// The upgrade that stops the node's own chainv4 at a height, as GET /status or GET /release
    /// says, if one does.
    pub node_stops: Option<chainv4::structs::version::Upgrade>,
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

/// The node and the proxy, changed on the unlock screen, as Settings changes them: the node to
/// connect to afresh, once either changes.
pub fn reach(ui: &mut Ui, palette: &Palette, settings: &mut Settings, states: &mut States) -> Option<String> {
    settings::reach(ui, palette, settings, &mut states.settings)
}

/// What each page keeps between frames.
#[derive(Default)]
pub struct States {
    reading: reading::ReadingState,
    pub send: send::SendState,
    batch: batch::BatchState,
    activity: activity::ActivityState,
    names: names::NamesState,
    pub multisig: multisig::MultisigState,
    contracts: contracts::ContractsState,
    receive: receive::ReceiveState,
    accounts: accounts::AccountsState,
    settings: settings::SettingsState,
    sign: sign::SignState,
    pub dapp: dapp::DappState,
    pub faucet: faucet::FaucetState,
    pub mining: mining::MiningState,
    pub governance: governance::GovernanceState,
    #[cfg(feature = "swap")]
    pub swaps: swaps::SwapsState,
    #[cfg(feature = "dex")]
    pub trade: trade::TradeState,
    #[cfg(feature = "nft")]
    pub nfts: nfts::NftsState,
    #[cfg(feature = "bridge")]
    bridge: bridge::BridgeState,
}

impl States {
    /// For a reload of a page: what it keeps of when it last asked, so it asks again.
    pub fn reload(&mut self, page: Page) {
        match page {
            Page::Activity => self.activity.reload(),
            Page::Mining => self.mining.reload(),
            _ => {}
        }
    }

    /// Closes a view a page opened, such as a swap, for the back gesture: false when the page has
    /// none open.
    pub fn back(&mut self, page: Page) -> bool {
        match page {
            #[cfg(feature = "swap")]
            Page::Swaps => self.swaps.back(),
            _ => false,
        }
    }
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

    /// Takes the node's answer to what Activity asked a contract the registry does not list: false
    /// for an answer to anyone else. Activity alone keeps what it says.
    pub fn unlisted_answered(&mut self, body: &str, result: &Result<serde_json::Value, crate::node::Failure>, now: u64) -> bool {
        self.activity.answered(body, result, now)
    }

    /// The names the pages want proven: one typed to send to, and one searched for.
    pub fn wanted_names(&self) -> Vec<String> {
        self.send.wanted_name().into_iter().chain(self.batch.wanted_name()).chain(self.names.wanted()).collect()
    }
}

/// Draws the page.
pub fn show(ui: &mut Ui, ctx: &mut Ctx, states: &mut States) {
    // What a site asks comes before any page.
    if ctx.settings.role() == Role::Online && dapp::show(ui, ctx, &mut states.dapp) {
        return;
    }

    if ctx.holders.is_empty() && !matches!(*ctx.page, Page::Accounts | Page::Settings | Page::More) {
        *ctx.page = Page::Accounts;
    }

    // A page this install's role has none of, as one left from before the role changed.
    if !has_page(ctx.settings.role(), *ctx.page) {
        *ctx.page = home(ctx.settings.role());
    }

    // Words and keys shown go out of sight once their page does.
    if *ctx.page != Page::Accounts {
        states.accounts.hide();
    }

    if *ctx.page != Page::Settings {
        states.settings.hide();
    }

    // What cannot be done without asking the node says so, rather than ask it.
    if ctx.settings.read_blocks && let Some(what) = reading::turned_off(*ctx.page) {
        reading::needs_asking(ui, ctx, what);

        return;
    }

    match *ctx.page {
        Page::Overview => {
            reading::note(ui, ctx, &mut states.reading);
            overview::show(ui, ctx);
            faucet::card(ui, ctx, &mut states.faucet);
        }
        Page::Send => send::show(ui, ctx, &mut states.send),
        Page::Batch => batch::show(ui, ctx, &mut states.batch, &mut states.send, &mut states.multisig),
        Page::Receive => receive::show(ui, ctx, &mut states.receive),
        Page::Activity => activity::show(ui, ctx, &mut states.activity),
        Page::Names => names::show(ui, ctx, &mut states.names, &mut states.send),
        Page::Mining => mining::show(ui, ctx, &mut states.mining, &mut states.send),
        Page::Multisig => multisig::show(ui, ctx, &mut states.multisig, &mut states.send),
        Page::Governance => governance::show(ui, ctx, &mut states.governance, &mut states.send, &mut states.multisig),
        Page::Contracts => contracts::show(ui, ctx, &mut states.contracts, &mut states.send),
        #[cfg(feature = "swap")]
        Page::Swaps => swaps::show(ui, ctx, &mut states.swaps, &mut states.send, &mut states.multisig),
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
        Page::More => more::show(ui, ctx),
    }
}

pub use chainv4_ui::widgets::beside;

/// The light client's state, in the sidebar: how far it follows, how much of that is final, and
/// where it took up the chain, if it fast-resumed.
pub fn light_status(ui: &mut Ui, palette: &Palette, chain: &Chain, down: Option<&Failure>) {
    let (color, text, detail) = following(palette, chain, down);
    let response = ui
        .horizontal(|ui| {
            theme::dot(ui, color);
            ui.label(RichText::new(text).color(palette.soft).size(13.0));
        })
        .response;
    response.on_hover_text(detail);

    if let Some(said) = fast_resumed(chain) {
        ui.add(egui::Label::new(RichText::new(said).small().color(palette.warn)).wrap());
    }
}

/// Says where the client took up the chain, if it fast-resumed: on a node's word, so never silently.
pub fn fast_resumed(chain: &Chain) -> Option<String> {
    chain.fast_resumed.map(|resumed| chainv4_ui::widgets::fast_resumed(&format::thousands(&resumed.at.to_string())))
}

/// The light client's state and the node's as a dot alone, where there is no room for words: they
/// show on a tap, and it is hollow when something is wrong.
pub fn light_dot(ui: &mut Ui, palette: &Palette, chain: &Chain, down: Option<&Failure>) {
    let (color, text, detail) = following(palette, chain, down);
    chainv4_ui::widgets::status_dot(ui, palette, color, down.is_some() || chain.trouble.is_some(), &format!("{text}. {detail}"));
}

/// The light client's state in a word or two, as the dot says it.
pub fn light_word(palette: &Palette, chain: &Chain, down: Option<&Failure>) -> String {
    following(palette, chain, down).1
}

/// The light client's state, and the node's once it stops answering, as nothing the client knows
/// goes on being news then: its color, in a word or two, and in full.
pub(crate) fn following(palette: &Palette, chain: &Chain, down: Option<&Failure>) -> (egui::Color32, String, String) {
    if let Some(failure) = down.filter(|_| chain.trouble.is_none()) {
        let shown = chain.following().map_or_else(String::new, |(tip, _)| format!(" What shows is as it stood at height {}.", format::thousands(&tip.to_string())));
        let resumed = fast_resumed(chain).map_or_else(String::new, |said| format!(" {said}."));

        return (palette.bad, "Node not answering".to_string(), format!("{}{shown}{resumed}", failure.sentence()));
    }

    let (color, text) = match (&chain.trouble, chain.following(), chain.has_followed()) {
        (Some(_), _, _) => (palette.bad, "Stopped".to_string()),
        (None, None, _) => (palette.muted, "Starting".to_string()),
        (None, Some((tip, _)), false) => (palette.warn, format!("Catching up · {}", format::thousands(&tip.to_string()))),
        (None, Some((tip, _)), true) => (palette.good, format!("Following · {}", format::thousands(&tip.to_string()))),
    };
    let detail = match (&chain.trouble, chain.following()) {
        (Some(why), _) => why.clone(),
        (None, Some((tip, final_height))) if chain.fast_resumed.is_some() => chainv4_ui::widgets::fast_resumed_story(&fast_resumed(chain).unwrap_or_default(), tip, final_height),
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
    ui.horizontal_wrapped(|ui| {
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

/// A row of a card: a label on the left, what it says on the right; on a narrow screen, the label
/// above what it says, so neither is squeezed.
pub fn row(ui: &mut Ui, palette: &Palette, label: &str, add: impl FnOnce(&mut Ui)) {
    if ui.available_width() < ROW_STACKS_BELOW {
        ui.label(RichText::new(label).small().color(palette.muted));
        ui.horizontal_wrapped(add);
        ui.add_space(2.0);

        return;
    }

    ui.horizontal_top(|ui| {
        ui.add_sized([128.0, 20.0], egui::Label::new(RichText::new(label).color(palette.muted)));
        ui.horizontal_wrapped(add);
    });
}

/// The narrowest a card's row keeps its label beside what it says.
const ROW_STACKS_BELOW: f32 = 420.0;

/// A note that something is still on its way.
pub fn waiting(ui: &mut Ui, palette: &Palette, text: &str) {
    ui.horizontal_wrapped(|ui| {
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

    /// How many parts the message is cut into. The encoder goes on past them with parts mixed of
    /// several, numbered on, so a camera that missed some finishes all the same: their numbers say
    /// nothing worth showing.
    fn count(&self) -> usize {
        self.encoder.lock().unwrap_or_else(std::sync::PoisonError::into_inner).fragment_count()
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
    ui.horizontal_wrapped(|ui| {
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

    let count = cycling.count();
    theme::caps(ui, palette, &format!("Animated code · {count} parts"));
    ui.label(
        RichText::new("Too long for one code, so it changes several times a second. Hold the other device's camera on it until it has read it: it needs about as many frames as there are parts, in any order, and it never runs out.")
            .small()
            .color(palette.muted),
    );
    ui.add_space(6.0);
    ui.vertical_centered(|ui| receive::qr(ui, &cycling.part, side));
    ui.add_space(6.0);
    ui.horizontal_wrapped(|ui| {
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

/// A wallet of two phrases drawn with no window, to see what a page does with what it holds.
#[cfg(test)]
pub(crate) mod drawn {
    use {
        super::*,
        crate::vault::{Book, Source, StoredAccount, Vault, Words},
    };

    /// A book of the test words, with an account, and the abandon words, named, with one.
    pub fn two_phrases() -> Book {
        let mut book = Book {
            phrases: vec![Words::new("test test test test test test test test test test test junk".to_string(), String::new())],
            accounts: vec![StoredAccount { name: "Main".to_string(), source: Source::Phrase { account: 0, extra: Vec::new(), from: None } }],
            ..Book::default()
        };
        let mut words = Words::new("abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about".to_string(), String::new());
        words.set_name("Savings");
        let from = book.add_phrase(words).unwrap();
        book.accounts.push(StoredAccount { name: "Saved".to_string(), source: Source::Phrase { account: 0, extra: Vec::new(), from } });

        book
    }

    /// What a drawn page works on besides its book: the settings it sees, the vault open, and why it
    /// asked to lock.
    #[derive(Default)]
    pub struct World {
        pub settings: Settings,
        pub vault: Option<Vault>,
        pub lock: Option<String>,
        pub leave: bool,
        pub reading: crate::reading::Reading,
    }

    /// A world of one wallet sealed with no password.
    fn bare(holders: &[Holder]) -> World {
        let vault = Vault::create("").unwrap();
        let mut settings = Settings::default();
        settings.wallets = vec![crate::app::WalletEntry {
            id: String::new(),
            name: "Mine".to_string(),
            vault: vault.seal(&Book::default()).unwrap(),
            current: holders.first().map(Holder::id).unwrap_or_default(),
            biometric: None,
            unprotected: true,
            born: None,
        }];

        World { settings, vault: Some(vault), ..World::default() }
    }

    /// Draws a page twice, as a screen this wide and tall shows it, with this book, on a wallet
    /// sealed with no password. How wide it came out, and whether anything was marked to be sealed.
    pub fn page(book: &mut Book, holders: &mut Vec<Holder>, size: (f32, f32), draw: impl FnMut(&mut Ui, &mut Ctx)) -> (f32, bool) {
        page_in(&mut bare(holders), book, holders, size, draw)
    }

    /// As `page`, in this world.
    pub fn page_in(world: &mut World, book: &mut Book, holders: &mut Vec<Holder>, size: (f32, f32), draw: impl FnMut(&mut Ui, &mut Ctx)) -> (f32, bool) {
        let (width, changed, _) = frames(world, book, holders, size, 2, |_, _| Vec::new(), draw);

        (width, changed)
    }

    /// Draws a page as page does, and says too each text the last frame drew and where.
    pub fn page_shown(book: &mut Book, holders: &mut Vec<Holder>, size: (f32, f32), draw: impl FnMut(&mut Ui, &mut Ctx)) -> (f32, bool, Vec<(String, egui::Rect)>) {
        frames(&mut bare(holders), book, holders, size, 2, |_, _| Vec::new(), draw)
    }

    /// As `page`: what it said, in the order it drew it.
    pub fn said(book: &mut Book, holders: &mut Vec<Holder>, size: (f32, f32), draw: impl FnMut(&mut Ui, &mut Ctx)) -> String {
        said_in(&mut bare(holders), book, holders, size, draw)
    }

    /// As `said`, in this world.
    pub fn said_in(world: &mut World, book: &mut Book, holders: &mut Vec<Holder>, size: (f32, f32), draw: impl FnMut(&mut Ui, &mut Ctx)) -> String {
        said_of(frames(world, book, holders, size, 2, |_, _| Vec::new(), draw).2)
    }

    /// As `page`, with a finger tapping this text on the way: what it said once it took the tap.
    pub fn tap(book: &mut Book, holders: &mut Vec<Holder>, size: (f32, f32), text: &str, draw: impl FnMut(&mut Ui, &mut Ctx)) -> String {
        tap_in(&mut bare(holders), book, holders, size, text, draw)
    }

    /// As `tap`, in this world.
    pub fn tap_in(world: &mut World, book: &mut Book, holders: &mut Vec<Holder>, size: (f32, f32), text: &str, draw: impl FnMut(&mut Ui, &mut Ctx)) -> String {
        said_of(frames(world, book, holders, size, 6, tapping(text), draw).2)
    }

    /// Draws what needs no page, such as the lock screen, a finger tapping a text on the way if one
    /// is named: how wide it came out, and what it said.
    pub fn screen(size: (f32, f32), tap: Option<&str>, draw: impl FnMut(&mut Ui)) -> (f32, String) {
        let egui = egui::Context::default();
        crate::theme::install_fonts(&egui);

        let (width, shown) = match tap {
            Some(text) => run(&egui, size, 6, tapping(text), draw),
            None => run(&egui, size, 2, |_, _| Vec::new(), draw),
        };

        (width, said_of(shown))
    }

    /// A finger tapping this text, as a frame's input from what the one before it drew. A box over
    /// the page draws nothing the frame it first sizes itself, so it looks for a frame longer.
    fn tapping(text: &str) -> impl Fn(usize, &[egui::epaint::ClippedShape]) -> Vec<egui::Event> + '_ {
        let at = std::cell::Cell::new(None);
        let stage = std::cell::Cell::new(0);

        move |frame: usize, shapes: &[egui::epaint::ClippedShape]| {
            let button = |pressed: bool| egui::Event::PointerButton { pos: at.get().unwrap_or_default(), button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };

            if frame == 0 {
                return Vec::new();
            }

            stage.set(stage.get() + 1);

            match stage.get() {
                1 => {
                    let drawn = shapes.iter().find_map(|clipped| match &clipped.shape {
                        egui::epaint::Shape::Text(shape) if shape.galley.text() == text => Some(shape.galley.rect.translate(shape.pos.to_vec2()).center()),
                        _ => None,
                    });

                    match drawn {
                        Some(found) => {
                            at.set(Some(found));

                            vec![egui::Event::PointerMoved(found)]
                        }
                        None if frame < 2 => {
                            stage.set(0);

                            Vec::new()
                        }
                        None => panic!("no {text} to tap"),
                    }
                }
                2 => vec![button(true)],
                3 => vec![button(false)],
                _ => Vec::new(),
            }
        }
    }

    /// What a frame drew, in order, a line each.
    fn said_of(shown: Vec<(String, egui::Rect)>) -> String {
        shown.into_iter().map(|(text, _)| format!("{text}\n")).collect()
    }

    /// Draws this many times, each frame's input from what the one before it drew: how wide it came
    /// out, and each text the last frame drew and where.
    fn run(
        egui: &egui::Context,
        size: (f32, f32),
        count: usize,
        events: impl Fn(usize, &[egui::epaint::ClippedShape]) -> Vec<egui::Event>,
        mut draw: impl FnMut(&mut Ui),
    ) -> (f32, Vec<(String, egui::Rect)>) {
        let mut width = 0.0_f32;
        let mut shapes = Vec::new();

        for frame in 0..count {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(size.0, size.1))),
                events: events(frame, &shapes),
                ..egui::RawInput::default()
            };
            let mut output = egui.run_ui(input, |ui| {
                draw(ui);
                width = ui.min_rect().width();
            });
            output.textures_delta.clear();
            shapes = output.shapes;
        }

        let shown = shapes.iter().filter_map(|clipped| match &clipped.shape {
            egui::epaint::Shape::Text(shape) => Some((shape.galley.text().to_string(), shape.galley.rect.translate(shape.pos.to_vec2()))),
            _ => None,
        });

        (width, shown.collect())
    }

    /// Draws a page this many times, each frame's input from what the one before it drew: how wide it
    /// came out, whether anything was marked to be sealed, and each text the last frame drew and where.
    fn frames(
        world: &mut World,
        book: &mut Book,
        holders: &mut Vec<Holder>,
        size: (f32, f32),
        count: usize,
        events: impl Fn(usize, &[egui::epaint::ClippedShape]) -> Vec<egui::Event>,
        mut draw: impl FnMut(&mut Ui, &mut Ctx),
    ) -> (f32, bool, Vec<(String, egui::Rect)>) {
        let egui = egui::Context::default();
        crate::theme::install_fonts(&egui);
        let palette = crate::theme::DARK;
        let (chain, coin) = (Chain::default(), Coin { symbol: "DEV".to_string(), decimals: 18 });
        let mut node = Node::new("http://127.0.0.1:1", Err("no client in a test".to_string()), egui.clone());
        let (mut page, mut connect, mut changed) = (Page::Accounts, None, false);
        #[cfg(all(feature = "bitcoin-light", not(target_arch = "wasm32")))]
        let mut lights = crate::light::Lights::default();
        let reader = chainv4::abi::known::AbiReader::default();
        let (width, shown) = run(&egui, size, count, events, |ui| {
            let mut ctx = Ctx {
                palette: &palette,
                reader: &reader,
                chain: &chain,
                node: &mut node,
                holders: &mut *holders,
                current: 0,
                book: &mut *book,
                coin: &coin,
                settings: &mut world.settings,
                reading: &mut world.reading,
                now: 0,
                page: &mut page,
                connect: &mut connect,
                changed: &mut changed,
                vault: &mut world.vault,
                leave: &mut world.leave,
                lock: &mut world.lock,
                wake: &egui,
                faucet: None,
                swap_contract: None,
                dex: (None, None),
                node_stops: None,
                #[cfg(all(feature = "bitcoin-light", not(target_arch = "wasm32")))]
                lights: &mut lights,
            };
            draw(ui, &mut ctx);
        });

        (width, changed, shown)
    }

    /// A phone upright, a phone on its side, and a desktop.
    pub const SCREENS: [(f32, f32); 3] = [(360.0, 740.0), (740.0, 360.0), (1200.0, 800.0)];
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
    fn the_tabs_and_more_lead_to_every_page_each_once() {
        let mut reached: Vec<Page> = tabs(Role::Online).iter().map(|(page, _)| *page).filter(|page| *page != Page::More).collect();
        reached.extend(more::PAGES.iter().map(|(page, _, _)| *page));
        let mut pages: Vec<Page> = nav(Role::Online).iter().map(|(page, _)| *page).collect();
        let order = |page: &Page| format!("{page:?}");
        reached.sort_by_key(order);
        pages.sort_by_key(order);

        assert_eq!(reached, pages, "the sidebar's pages, whatever the features");
        assert_eq!(tabs(Role::Online).len(), 5);
    }

    #[test]
    fn a_page_under_more_marks_more_and_a_tabs_own_page_marks_it() {
        for (page, _, _) in more::PAGES {
            assert_eq!(tab_of(*page, Role::Online), Page::More, "{page:?}");
        }

        for (page, _) in tabs(Role::Online) {
            assert_eq!(tab_of(*page, Role::Online), *page);
        }

        assert_eq!(tab_of(Page::Accounts, Role::Online), Page::More);
        assert_eq!(tab_of(Page::Accounts, Role::Signer), Page::Accounts, "a signer's five are all tabs");
    }

    #[test]
    fn a_signer_has_no_more_and_keeps_its_five_pages_as_tabs() {
        assert_eq!(tabs(Role::Signer), nav(Role::Signer));
        assert_eq!(tabs(Role::Signer).len(), 5);
        assert!(!has_page(Role::Signer, Page::More));
        assert!(!has_page(Role::Signer, Page::Send));
        assert!(has_page(Role::Online, Page::More));
        assert!(!has_page(Role::Online, Page::Sign));
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
