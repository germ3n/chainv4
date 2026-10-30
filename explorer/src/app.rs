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
        BlockDetail, Called, Checkpoint, CollectionsPage, Contract, DecisionsPage, EventsPage, Forks, TxsPage, Mempool, NodeView, Page, PeerChange, Receipt, Reorganized,
        Status, Summary, TokensPage, TxView,
    },
    crate::store::{PAGE, Store},
    crate::theme::{self, DARK, LIGHT, Roles},
    crate::views::{
        self, Ctx, Look, Settings, View, account::AccountState, hops::HopState, map::MapState, network::Streaming, send::SendState,
        settings::SettingsState, tokens::TokensState, dex::DexState, swaps::SwapsState, bridge::BridgeState, collections::CollectionsState,
        update::UpdateState, governance::GovernanceState,
    },
    chainv4::release::{ReleaseError, ReleaseKeys},
    chainv4_ui::{Palette, layout, widgets},
    egui::{RichText, Ui},
    serde_json::Value,
    std::time::Duration,
};

/// How wide the sidebar is, and the widest the page column grows on a big screen.
const SIDEBAR: f32 = 212.0;
const COLUMN: f32 = 1400.0;
/// Below this height, in points, as a phone on its side, the sidebar's foot folds to one line.
const SHORT: f32 = 560.0;

/// Seconds between asking the same thing again.
const STATUS_POLLED: f64 = 1.0;
const STATUS_STREAMED: f64 = 3.0;
const NODE_EVERY: f64 = 2.0;
const PAGE_EVERY: f64 = 2.5;
const FORKS_EVERY: f64 = 5.0;
/// The node's release: on connecting, then about every hour, and a minute after it failed to
/// answer.
const RELEASE_EVERY: f64 = 3600.0;
const RELEASE_RETRY: f64 = 60.0;
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
    release: f64,
}

pub struct Explorer {
    settings: Settings,
    /// The node address being typed.
    address: String,
    wake: egui::Context,
    api: Api,
    store: Store,
    view: View,
    /// The pages behind and ahead, each with where it was scrolled to as it was left.
    back: Vec<(View, f32)>,
    ahead: Vec<(View, f32)>,
    search: String,
    streaming: Streaming,
    asked: Asked,
    map: MapState,
    hops: HopState,
    account: AccountState,
    tokens: TokensState,
    collections: CollectionsState,
    dex: DexState,
    swaps: SwapsState,
    bridge: BridgeState,
    send: SendState,
    faucet: views::faucet::FaucetPage,
    pool: views::pool::PoolPage,
    governance: GovernanceState,
    tools: views::tools::ToolsState,
    multisig: views::multisig::MultisigState,
    names: views::names::NamesState,
    settings_page: SettingsState,
    /// The light client, which checks what the node says about balances.
    light: Light,
    /// The release keys built in, which the release a node relays must be signed by.
    keys: ReleaseKeys,
    /// A newer explorer, if one is out, and putting it in place.
    update: UpdateState,
    /// Something to tell the person, and whether it is bad news.
    notice: Option<(String, bool)>,
    palette: Palette,
    /// Where the page shown is scrolled to, and where it should be: the top of a new page, or
    /// where one returned to was left.
    scrolled: chainv4_ui::layout::Scrolled,
    /// The window's title as last set, which follows the chain's name.
    title: String,
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
        let mut api = Api::new(&node, client(&settings), wake.clone());
        api.bear_report_token(settings.report_tokens.get(&node).map(String::as_str));
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
            collections: CollectionsState::default(),
            dex: DexState::default(),
            swaps: SwapsState::default(),
            bridge: BridgeState::default(),
            send: SendState::default(),
            faucet: views::faucet::FaucetPage::default(),
            pool: views::pool::PoolPage::default(),
            governance: GovernanceState::default(),
            tools: views::tools::ToolsState::default(),
            multisig: views::multisig::MultisigState::default(),
            names: views::names::NamesState::default(),
            settings_page: SettingsState::default(),
            light: Light::default(),
            keys: ReleaseKeys::built_in().unwrap_or_else(|why| {
                log::warn!("{why}: no update notices");

                ReleaseKeys::default()
            }),
            update: UpdateState::default(),
            notice: None,
            palette,
            scrolled: chainv4_ui::layout::Scrolled::default(),
            title: String::new(),
        };
        explorer.connect(&node);

        explorer
    }

    /// Starts afresh with another node, or the same one through another proxy.
    fn connect(&mut self, node: &str) {
        let node = node.trim().trim_end_matches('/').to_string();
        self.api = Api::new(&node, client(&self.settings), self.wake.clone());
        self.api.bear_report_token(self.settings.report_tokens.get(&node).map(String::as_str));
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
            self.back.push((std::mem::replace(&mut self.view, view), self.scrolled.offset()));
            self.ahead.clear();
            self.scrolled.top();
        }
    }

    /// Takes everything that arrived.
    fn take(&mut self, now: u64, time: f64) {
        for reply in self.api.replies() {
            match reply {
                Reply::Answer { ask, result } => {
                    let trust = crate::light::Trust {
                        node: &self.settings.node,
                        pinned: &mut self.settings.pinned,
                        checkpoints: &self.settings.checkpoints,
                    };

                    if self.light.answered(&ask, &result, trust, now, time) {
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
                                // The node runs no pool, or no longer does.
                                Ask::Pool if failure.status == 404 => self.store.pool = None,
                                Ask::PoolMiner(address) if failure.status == 404 => {
                                    self.store.pool_miners.remove(address);
                                }
                                // An older node, which has no releases to relay.
                                Ask::Release if failure.status == 404 => {
                                    self.store.release = None;
                                    self.update.found = None;
                                }
                                Ask::Release => self.asked.release = time - RELEASE_EVERY + RELEASE_RETRY,
                                // A chain without governance, or one whose node no longer has it.
                                Ask::Governance if failure.status == 404 => {
                                    self.store.governance = None;
                                    self.store.decisions = Default::default();
                                }
                                Ask::GovernanceWeight(address) => {
                                    self.store.governance_weights.remove(address);
                                }
                                // Not a multisig, or no longer one.
                                Ask::Multisig(address) if failure.status == 404 => {
                                    self.store.multisigs.remove(address);
                                }
                                // A chain without names, or a node from before these.
                                Ask::NamesList { after: None } if failure.status == 404 => self.store.name_list = Default::default(),
                                Ask::Rich if failure.status == 404 => self.store.rich = None,
                                Ask::Supply if failure.status == 404 => self.store.supply = None,
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
                        self.asked.release = 0.0;
                    }

                    // Kept per node, so the next launch names the chain before the node answers.
                    match status.name.as_deref().map(str::trim).filter(|name| !name.is_empty()) {
                        Some(name) => self.settings.names.insert(self.settings.node.clone(), name.to_string()),
                        None => self.settings.names.remove(&self.settings.node),
                    };
                    store.status = Some(status);
                }
            }
            Ask::Node => {
                if let Some(node) = parse::<NodeView>(value) {
                    store.node_report(node, now);
                }
            }
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
            Ask::Rpc(_) | Ask::Faucet(_) => {
                store.calls.insert(ask.clone(), value);
            }
            Ask::Pool => store.pool = parse(value),
            Ask::Release => {
                if let Some(release) = parse::<crate::model::NodeRelease>(value) {
                    self.update.found = match crate::release::update(&release.manifest, &self.keys, crate::release::ours()) {
                        Ok(found) => found,
                        // A build without release keys shows no update notices at all.
                        Err(ReleaseError::NoKeys) => None,
                        Err(why) => {
                            log::warn!("the release the node relays does not count: {why}");

                            None
                        }
                    };
                    store.release = Some(release);
                }
            }
            Ask::PoolMiner(address) => {
                if let Some(books) = parse(value) {
                    store.pool_miners.insert(address, books);
                }
            }
            Ask::Name(name) => {
                if let Some(found) = parse::<crate::model::NameView>(value) {
                    store.names.insert(name, found);
                }
            }
            Ask::AccountNames(address) => {
                if let Some(owned) = parse::<crate::model::OwnedNames>(value) {
                    store.owned_names.insert(address, owned.names);
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
            Ask::Collections => {
                if let Some(page) = parse::<CollectionsPage>(value) {
                    store.collections = Some((page.collections, now));
                }
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
                // A token's balance_of takes a holder alone; a multi-token collection's takes an id too.
                let one_argument = serde_json::from_str::<Value>(args).ok().and_then(|args| args.as_array().map(Vec::len)) == Some(1);

                if function == "balance_of"
                    && one_argument
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
            Ask::Governance => store.governance = parse(value),
            Ask::Decisions { before } => {
                if let Some(page) = parse::<DecisionsPage>(value) {
                    store.decisions.take(page, before);
                }
            }
            Ask::GovernanceWeight(address) => {
                if let Some(found) = parse(value) {
                    store.governance_weights.insert(address, found);
                }
            }
            Ask::Multisig(address) => {
                if let Some(found) = parse(value) {
                    store.multisigs.insert(address, found);
                }
            }
            Ask::NamesList { after } => {
                if let Some(page) = parse(value) {
                    store.name_list.take(page, after.is_some());
                }
            }
            Ask::Rich => store.rich = parse(value),
            Ask::Supply => store.supply = parse(value),
            Ask::Offer(_) | Ask::Submit(_) => {}
            // The light client's own, which it took already.
            Ask::Light | Ask::Headers { .. } | Ask::Finality(_) | Ask::Checkpoint(_) | Ask::Proof { .. } | Ask::ReceiptProof(_) | Ask::Ancestors { .. } => {}
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

        if due(&mut self.asked.release, RELEASE_EVERY) {
            self.api.ask(Ask::Release);
        }

        // The pool's, governance's, the multisig's, the names' and the rich list's pages ask again
        // on their own, so why it failed stands till then: a node without a pool, or a chain
        // without governance or names, says so steadily, rather than between spells of loading.
        if due(&mut self.asked.failures, RETRY_AFTER) {
            self.store
                .failures
                .retain(|ask, _| {
                    matches!(
                        ask,
                        Ask::Offer(_)
                            | Ask::Submit(_)
                            | Ask::Call { .. }
                            | Ask::Rpc(_)
                            | Ask::Faucet(_)
                            | Ask::Pool
                            | Ask::PoolMiner(_)
                            | Ask::Governance
                            | Ask::Decisions { .. }
                            | Ask::GovernanceWeight(_)
                            | Ask::Multisig(_)
                            | Ask::NamesList { .. }
                            | Ask::Rich
                            | Ask::Supply
                    )
                });
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
            View::Account(address) => [Ask::Account(address.clone()), Ask::AccountNames(address.clone())].into_iter().chain(self.light.prove(address)).collect(),
            View::Name(name) => vec![Ask::Name(name.clone())],
            View::Pool => [Ask::Pool].into_iter().chain(self.pool.miner().map(|miner| Ask::PoolMiner(miner.to_string()))).collect(),
            // The newest decisions, and the light client's proof of each one read.
            View::Governance => {
                let light = &mut self.light;
                let proofs: Vec<Ask> = self.store.decisions.items.values().filter_map(|decision| light.prove_receipt(&decision.tx, &decision.block, decision.height)).collect();

                [Ask::Governance, Ask::Decisions { before: None }].into_iter().chain(proofs).collect()
            }
            View::Multisig(address) if !address.is_empty() => vec![Ask::Multisig(address.clone())],
            // The first page of the directory, refreshed while it is all that was read.
            View::Names => vec![Ask::NamesList { after: None }],
            // The supply, and its proof at the light client's tip once it caught up.
            View::Rich => {
                let proof = views::rich::supply_proof(&self.store, &mut self.light);

                [Ask::Rich, Ask::Supply].into_iter().chain(proof).collect()
            }
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

    /// Back to the page before, scrolled to where it was left.
    fn back(&mut self) {
        if let Some((view, offset)) = self.back.pop() {
            self.ahead.push((std::mem::replace(&mut self.view, view), self.scrolled.offset()));
            self.scrolled.back_to(offset);
        }
    }

    fn forward(&mut self) {
        if let Some((view, offset)) = self.ahead.pop() {
            self.back.push((std::mem::replace(&mut self.view, view), self.scrolled.offset()));
            self.scrolled.back_to(offset);
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

    /// The chain's name: as the node says it, else as it said it last time, else chainv4.
    fn chain_name(&self) -> &str {
        shown_name(self.store.status.as_ref(), self.settings.names.get(&self.settings.node).map(String::as_str))
    }

    /// Titles the window, or the browser's tab, after the chain, when its name changed.
    fn retitle(&mut self, ctx: &egui::Context) {
        let title = window_title(self.chain_name());

        if title == self.title {
            return;
        }

        // The web runner ignores the viewport's title, so the page's own is set there.
        #[cfg(target_arch = "wasm32")]
        if let Some(document) = web_sys::window().and_then(|window| window.document()) {
            document.set_title(&title);
        }

        ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
        self.title = title;
    }

    /// The light client's state: a color, a few words, and the whole story.
    fn light_state(&self) -> (egui::Color32, &'static str, String) {
        let palette = self.palette;

        match (&self.light.trouble, self.light.following()) {
            (Some(trouble), _) => (palette.bad, "light client stopped", trouble.clone()),
            (None, Some((tip, final_height))) if self.light.has_followed() => (
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
        let name = self.chain_name().to_string();
        layout::sidebar(
            ui,
            self,
            |app, ui| {
                // A short window, as a phone on its side, keeps its room for the pages: the state
                // goes on one line, with the rest on a tap or a hover.
                if ui.ctx().content_rect().height() < SHORT {
                    let (light_color, light_words, light_story) = app.light_state();
                    let (color, words) = app.health();
                    let story = format!("{light_words}: {light_story}\nThe node at {} is {words}", app.api.base());

                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        views::dot(ui, light_color);
                        views::dot(ui, color);
                        ui.add(egui::Label::new(RichText::new(words).small().color(palette.soft)).truncate());
                    })
                    .response
                    .on_hover_text(story);

                    return;
                }

                ui.add_space(14.0);
                let (light_color, light_words, light_story) = app.light_state();
                ui.horizontal(|ui| {
                    views::dot(ui, light_color);
                    ui.label(RichText::new(light_words).small().color(palette.soft));
                })
                .response
                .on_hover_text(light_story);

                let (color, words) = app.health();
                ui.horizontal(|ui| {
                    views::dot(ui, color);
                    ui.label(RichText::new(words).small().color(palette.soft));

                    if let Some(status) = &app.store.status {
                        ui.label(RichText::new(format!("· {}", format::ago(status.timestamp, now))).small().color(palette.muted));
                    }
                });
                ui.add(egui::Label::new(RichText::new(app.api.base()).monospace().small().color(palette.muted)).truncate());
                widgets::caps(ui, &palette, "Node");
            },
            |app, ui| {
                ui.add_space(16.0);
                ui.horizontal(|ui| {
                    // A long name is cut short, leaving room for the word after it.
                    let room = ui.available_width() - 70.0;
                    ui.scope(|ui| {
                        ui.set_max_width(room);
                        ui.add(egui::Label::new(widgets::strong(&name, 17.0).color(palette.ink)).truncate())
                    })
                    .inner
                    .on_hover_text(&name);
                    widgets::caps(ui, &palette, "explorer");
                });
                ui.add_space(14.0);

                for (group, pages) in View::GROUPS {
                    widgets::caps(ui, &palette, group);
                    ui.add_space(2.0);

                    for (view, name) in pages {
                        let selected = app.view.marks(view);
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
                            app.go(view.clone());
                        }
                    }

                    ui.add_space(10.0);
                }
            },
        );
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
        let name = self.chain_name().to_string();
        ui.horizontal(|ui| {
            match self.back.is_empty() {
                // A long name is cut short, leaving the search its room.
                true => {
                    ui.scope(|ui| {
                        ui.set_max_width(140.0);
                        ui.add(egui::Label::new(widgets::strong(&name, 16.0).color(palette.ink)).truncate())
                    })
                    .inner
                    .on_hover_text(&name);
                }
                false => {
                    if ui.add(egui::Button::new(RichText::new("‹").size(18.0)).min_size(egui::vec2(40.0, 30.0))).clicked() {
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
                widgets::end_row(
                    ui,
                    |ui| {
                        ui.add(egui::Label::new(RichText::new(text).color(color)).wrap());
                    },
                    |ui| {
                        if ui.small_button("Dismiss").clicked() {
                            self.notice = None;
                        }
                    },
                );
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
        } else if views::is_address(&wanted) || wanted.contains(':') {
            self.go(View::Account(wanted));
        } else if wanted.starts_with("0x") {
            self.api.ask(Ask::Lookup(wanted));
        } else if wanted.contains('.') {
            self.go(View::Name(wanted.to_ascii_lowercase()));
        } else {
            self.notice = Some((format!("{wanted} is not a height, an id, an address or a name such as alice.dev"), true));

            return;
        }

        self.notice = None;
        self.search.clear();
    }

    fn page(&mut self, ui: &mut Ui, now: u64, narrow: bool) {
        let mut go = None;
        let mut connect = None;
        let look = self.look(narrow);

        if views::update::notice(ui, look, &mut self.update) {
            self.update.start(client(&self.settings), self.wake.clone());
        }

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
            let scroll = egui::ScrollArea::vertical().id_salt("page").auto_shrink(false);

            self.scrolled.show(ui, scroll, |ui| {
                layout::keep_focus_in_view(ui);
                ui.vertical_centered(|ui| {
                    ui.set_max_width(ui.available_width().min(COLUMN));
                    ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| match &self.view {
                        View::Overview => views::overview::show(ui, &mut ctx),
                        View::Map => {}
                        View::Block(block) => views::block::show(ui, &mut ctx, block),
                        View::Tx(id) => views::tx::show(ui, &mut ctx, id, &mut self.hops),
                        View::Account(address) => views::account::show(ui, &mut ctx, address, &mut self.account),
                        View::Name(name) => views::name::show(ui, &mut ctx, name),
                        View::Network => views::network::show(ui, &mut ctx, &self.streaming),
                        View::Mempool => views::mempool::show(ui, &mut ctx),
                        View::Tokens => views::tokens::show(ui, &mut ctx, &mut self.tokens),
                        View::Collections => views::collections::list(ui, &mut ctx),
                        View::Collection(address) => views::collections::show(ui, &mut ctx, &mut self.collections, address),
                        View::Dex => views::dex::show(ui, &mut ctx, &mut self.dex),
                        View::Swaps => views::swaps::show(ui, &mut ctx, &mut self.swaps),
                        View::Bridges => views::bridge::show(ui, &mut ctx, &mut self.bridge),
                        View::Transfer(digest) => views::bridge::transfer(ui, &mut ctx, &mut self.bridge, digest),
                        View::Release(id) => views::bridge::release(ui, &mut ctx, &mut self.bridge, *id),
                        View::Finality => views::finality::show(ui, &mut ctx),
                        View::Governance => views::governance::show(ui, &mut ctx, &mut self.governance),
                        View::Send => views::send::show(ui, &mut ctx, &mut self.send),
                        View::Faucet => views::faucet::show(ui, &mut ctx, &mut self.faucet),
                        View::Pool => views::pool::show(ui, &mut ctx, &mut self.pool),
                        View::Settings => views::settings::show(ui, &mut ctx, &mut self.address, &mut self.settings_page, &self.streaming, &mut connect),
                        View::More => views::more::show(ui, &mut ctx),
                        View::Tools => views::tools::show(ui, &mut ctx, &mut self.tools),
                        View::Multisig(address) => views::multisig::show(ui, &mut ctx, &mut self.multisig, address),
                        View::Names => views::names::show(ui, &mut ctx, &mut self.names),
                        View::Rich => views::rich::show(ui, &mut ctx),
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
        self.retitle(ui.ctx());
        self.poll(time);
        layout::suit_touch(ui.ctx());

        // Android's back gesture, and a mouse's back button: the page before, while there is one.
        // At the first, it is left for the platform, which sends the app to the background.
        if !self.back.is_empty() && ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::BrowserBack)) {
            self.back();
        }

        let palette = self.palette;
        let narrow = layout::is_narrow(ui.available_width());

        // An explorer the network no longer works with shows nothing but how to update it.
        if self.update.blocks() {
            egui::CentralPanel::default()
                .frame(egui::Frame::new().fill(palette.paper).inner_margin(egui::Margin::symmetric(16, 16)))
                .show(ui, |ui| {
                    if views::update::blocking(ui, self.look(narrow), &self.update) {
                        self.update.start(client(&self.settings), self.wake.clone());
                    }
                });
            ui.ctx().request_repaint_after(Duration::from_millis(500));

            return;
        }

        let bar = egui::Frame::new().fill(palette.nav).stroke(egui::Stroke::new(1.0, palette.line));
        // A finger needs taller buttons and fields than a mouse: on a phone, upright or on its side.
        let fingers = narrow || layout::touch(ui.ctx());
        let sized = |ui: &mut Ui| {
            if fingers {
                layout::finger_sized(ui);
            }
        };

        match narrow {
            true => {
                egui::Panel::top("bar").frame(bar.inner_margin(egui::Margin::symmetric(10, 8))).show(ui, |ui| {
                    layout::finger_row(ui);
                    self.bar(ui);
                });
                egui::Panel::bottom("tabs").frame(bar).show(ui, |ui| self.tabs(ui));
            }
            false => {
                egui::Panel::left("nav")
                    .exact_size(SIDEBAR)
                    .resizable(false)
                    .frame(bar.inner_margin(egui::Margin::symmetric(12, 0)))
                    .show(ui, |ui| {
                        sized(ui);
                        self.sidebar(ui, now);
                    });
                egui::Panel::top("header")
                    .frame(egui::Frame::new().fill(palette.paper).inner_margin(egui::Margin::symmetric(16, 10)))
                    .show(ui, |ui| {
                        if fingers {
                            layout::finger_row(ui);
                        }

                        self.header(ui);
                    });
            }
        }

        let margin = if narrow { egui::Margin::symmetric(10, 10) } else { egui::Margin::symmetric(16, 4) };
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(palette.paper).inner_margin(margin))
            .show(ui, |ui| {
                sized(ui);
                self.page(ui, now, narrow);
            });

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

/// The chain's name to show: the status's, else the one remembered, else chainv4. A status that
/// names none wins over what was remembered: the chain has none now.
fn shown_name<'a>(status: Option<&'a Status>, remembered: Option<&'a str>) -> &'a str {
    match status {
        Some(status) => status.chain_name(),
        None => crate::model::chain_name(remembered),
    }
}

/// The window's title on a chain of this name.
fn window_title(name: &str) -> String {
    format!("{name} explorer")
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

#[cfg(test)]
mod tests {
    use {super::*, serde_json::json};

    #[test]
    fn the_header_names_the_chain_as_the_node_says_else_as_remembered_else_chainv4() {
        let status = |name: Value| -> Status {
            serde_json::from_value(json!({
                "genesis": "g", "tip": "t", "height": 1, "timestamp": 0, "difficulty": "0", "work": "1",
                "hash": "blake3", "coin": { "symbol": "DEV", "decimals": 18 }, "finality": null, "fees": null, "name": name,
            }))
            .unwrap()
        };
        let named = status(json!("chainv4 mainnet"));
        let unnamed = status(Value::Null);

        assert_eq!(shown_name(None, None), "chainv4", "nothing known yet");
        assert_eq!(shown_name(None, Some("chainv4 testnet")), "chainv4 testnet", "before the node answers");
        assert_eq!(shown_name(Some(&named), Some("chainv4 testnet")), "chainv4 mainnet", "renamed since");
        assert_eq!(shown_name(Some(&unnamed), Some("chainv4 testnet")), "chainv4", "the node names none now");
        assert_eq!(window_title(shown_name(Some(&named), None)), "chainv4 mainnet explorer");
        assert_eq!(window_title(shown_name(None, None)), "chainv4 explorer");
    }
}
