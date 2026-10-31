//! The explorer: one node's API, everything learned from it, and the page being looked at. Each
//! frame takes what arrived, asks for what is due, and draws the page.
//!
//! What the node pushes over its stream lands at once. Without a stream, or besides it, the
//! explorer polls: the status every second or so, and whatever the page in view shows.
//!
//! With Read from checked blocks on, none of that: the stream only says when to look, what a page asks
//! is answered from what was read or refused at the Api, and blocks are read as they are final.

use {
    crate::api::{Api, Ask, Failure, News, Reply},
    crate::format,
    crate::light::Light,
    crate::model::{
        BlockDetail, Called, Checkpoint, CollectionsPage, Contract, DecisionsPage, EventsPage, Forks, TxsPage, Mempool, NodeView, Page, PeerChange, Receipt, Reorganized,
        Status, Summary, TokensPage, TxView,
    },
    crate::store::{PAGE, Store},
    crate::theme::{self, DARK, LIGHT, Roles},
    chainv4_ui::asks::{PageAsks, Reload},
    chainv4_ui::pull::Pull,
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

/// The widest the page column grows on a big screen.
const COLUMN: f32 = 1400.0;

/// The most of a phone's top bar the chain's name takes, leaving the search its room.
const BAR_NAME: f32 = 140.0;

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
/// How long after the stream drops it is opened again.
const STREAM_RETRY: f64 = 10.0;

/// When each thing was last asked for.
#[derive(Default)]
struct Asked {
    status: f64,
    node: f64,
    page: f64,
    forks: f64,
    stream: f64,
    release: f64,
}

/// Lets go of why these failed, as they are being asked again: the page says so until they answer.
fn retrying(store: &mut Store, asks: impl IntoIterator<Item = Ask>) {
    for ask in asks {
        store.failures.remove(&ask);
    }
}

pub struct Explorer {
    settings: Settings,
    /// What reads EVM calls and logs, each once.
    reader: chainv4::abi::known::AbiReader,
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
    /// What was read from checked blocks, while that is on.
    reading: crate::reading::Reading,
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
    /// What each page asked, for a reload to ask again.
    asked_by_view: PageAsks<View, Ask>,
    /// A reload under way, and the finger on the page that may start one.
    reloading: Reload<Ask>,
    pull: Pull,
}

impl Explorer {
    /// The explorer, talking to node when given, or else to the node it talked to last.
    pub fn new(creation: &eframe::CreationContext, node: Option<String>, fallback: &str, folder: Option<std::path::PathBuf>) -> Explorer {
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
        let mut api = Api::new(&node, client(&settings), wake.clone()).reading_blocks(settings.read_blocks);
        api.bear_report_token(settings.report_tokens.get(&node).map(String::as_str));
        #[cfg(not(target_arch = "wasm32"))]
        let reading = crate::reading::on_this_device(folder.as_deref());
        #[cfg(target_arch = "wasm32")]
        let reading = {
            let _ = folder;

            crate::reading::on_this_device()
        };
        let mut explorer = Explorer {
            settings,
            address: node.clone(),
            api,
            wake,
            store: Store::default(),
            reader: chainv4::abi::known::AbiReader::default(),
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
            reading,
            keys: ReleaseKeys::built_in().unwrap_or_else(|why| {
                log::warn!("{why}: no update notices");

                ReleaseKeys::default()
            }),
            update: UpdateState::default(),
            notice: None,
            palette,
            scrolled: chainv4_ui::layout::Scrolled::default(),
            title: String::new(),
            asked_by_view: PageAsks::default(),
            reloading: Reload::default(),
            pull: Pull::default(),
        };
        explorer.connect(&node);

        explorer
    }

    /// Starts afresh with another node, or the same one through another proxy.
    fn connect(&mut self, node: &str) {
        let node = node.trim().trim_end_matches('/').to_string();
        self.api = Api::new(&node, client(&self.settings), self.wake.clone()).reading_blocks(self.settings.read_blocks);
        self.api.bear_report_token(self.settings.report_tokens.get(&node).map(String::as_str));
        self.store = Store::default();
        // What the setting holds goes as it does, and what it read stays for next time. The answers to
        // what was out, by the way that was, are not coming.
        self.reading.line_changed();

        if !self.settings.read_blocks {
            self.reading.close(0.0);
        }

        self.map = MapState::default();
        self.light = Light::default();
        self.asked = Asked::default();
        self.settings.node = node.clone();
        self.address = node;
        self.streaming = Streaming::Connecting;
        self.api.listen();
        self.asked_by_view = PageAsks::default();
        self.reloading = Reload::default();
    }

    /// Asks the node again for what the page shows and its status, past whatever failed before:
    /// what Reload does, and pulling a page down. What shows stays until the answers come.
    fn reload(&mut self, time: f64) {
        self.asked = Asked { release: self.asked.release, ..Asked::default() };
        self.light.reload();
        self.reading.retry();
        let failed = self.store.failures.iter().filter(|(ask, failure)| failure.is_trouble() && ask.reads()).map(|(ask, _)| ask.clone());
        let mut asks = vec![Ask::Status];
        asks.extend_from_slice(self.asked_by_view.of(&self.view));
        asks.extend(failed);

        for ask in &asks {
            self.api.retry(ask.clone());
        }

        if matches!(self.streaming, Streaming::Lost(_)) {
            self.streaming = Streaming::Connecting;
            self.api.listen();
        }

        self.reloading.start(asks, time);
    }

    fn go(&mut self, view: View) {
        if view != self.view {
            self.back.push((std::mem::replace(&mut self.view, view), self.scrolled.offset()));
            self.ahead.clear();
            self.scrolled.top();
        }
    }

    /// Takes everything that arrived. Why something failed stands until it is answered, or asked
    /// again by Retry or Reload: a page says so steadily, rather than between spells of loading,
    /// while what it goes on asking is held back by the Api.
    fn take(&mut self, now: u64, time: f64) {
        retrying(&mut self.store, self.api.retried());

        for reply in self.api.replies() {
            match reply {
                Reply::Answer { ask, result } => {
                    if self.reading.take_proof(&ask, &result, self.light.client()) {
                        continue;
                    }

                    let reading = self.settings.read_blocks;
                    let trust = crate::light::Trust {
                        node: &self.settings.node,
                        pinned: &mut self.settings.pinned,
                        checkpoints: &self.settings.checkpoints,
                        fast_resume: !reading && self.settings.fast_resume.get(&self.settings.node).copied().unwrap_or(false),
                        walks: reading,
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

                    // No node says what the coin is called with Read from checked blocks on: what one
                    // last said is what shows then.
                    if !self.settings.read_blocks {
                        self.settings.coins.insert(status.genesis.clone(), (status.coin.symbol.clone(), status.coin.decimals));
                    }

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
            Ask::AccountTxs { address, query, cursor } => {
                if let Some(page) = parse::<TxsPage>(value) {
                    // Only the filter in use: what another one found is asked again if it comes back.
                    store.account_txs.retain(|(kept, kept_query), _| *kept == address && (kept_query.is_empty() || *kept_query == query));
                    store.account_txs.entry((address, query)).or_default().take(page, cursor.is_some(), now);
                }
            }
            Ask::Governance => store.governance = parse(value),
            Ask::Decisions { before } => {
                if let Some(page) = parse::<DecisionsPage>(value) {
                    store.decisions.take(page.version, page.from, page.decisions, before);
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
            Ask::Light | Ask::Headers { .. } | Ask::Finality { .. } | Ask::Bundle { .. } | Ask::Checkpoint(_) | Ask::Proof { .. } | Ask::DirectProof { .. } | Ask::ReceiptProof(_) | Ask::Ancestors { .. } => {}
        }
    }

    fn news(&mut self, news: News, now: u64, time: f64) {
        let store = &mut self.store;
        self.light.hear(&news, time);

        match news {
            News::Open => self.streaming = Streaming::Live,
            News::Alive => {}
            News::Closed { why, unsupported } => {
                self.streaming = match unsupported {
                    true => Streaming::Unsupported,
                    false => Streaming::Lost(why),
                };
                self.asked.stream = time;
            }
            // With Read from checked blocks on, the stream is a wake-up and nothing else: the light
            // client above heard the chain moved, and nothing the node says of it is taken.
            News::Event { .. } if self.settings.read_blocks => {}
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

    /// With Read from checked blocks on: what follows the chain, the blocks due, and what waits for
    /// the index to answer. A page's own asks are turned off or answered here as they come, and nothing
    /// else is polled. A chain with no finality has no final blocks to read, which the pages say.
    fn poll_reading(&mut self, time: f64) {
        if let Some(ask) = self.light.ask(time) {
            self.api.ask(ask);
        }

        if time - self.asked.release >= RELEASE_EVERY || self.asked.release == 0.0 {
            self.asked.release = time;
            self.api.ask(Ask::Release);
        }

        if let Some(client) = self.light.client()
            && let Some(genesis) = self.settings.pinned.get(&self.settings.node).cloned()
            && self.reading.open(&genesis, client.params().hash, client, time)
        {
            for (asked, came) in self.api.ranges() {
                self.reading.came(asked, came, client, time);
            }

            if client.params().finality.is_some()
                && self.light.has_followed()
                && let Some(asked) = self.reading.go(client, time)
            {
                self.api.read_range(asked);
            }

            let (coin, name) = (self.settings.coins.get(&genesis), self.settings.names.get(&self.settings.node).map(String::as_str));
            let status = || self.reading.status(&genesis, client, coin, name);

            if time - self.asked.status >= STATUS_STREAMED || self.asked.status == 0.0 {
                self.asked.status = time;

                if let Some(status) = status() {
                    self.api.answered_here(Ask::Status, Ok(status));
                }
            }

            for ask in self.api.for_index() {
                let result = match &ask {
                    Ask::Status => status().ok_or_else(|| Failure { status: 425, message: "nothing read yet".to_string() }),
                    other => self.reading.answer(other, client.final_height()).unwrap_or_else(|| Err(Failure { status: 425, message: "nothing read yet".to_string() })),
                };

                self.api.answered_here(ask, result);
            }
        }

        for said in std::mem::take(&mut self.reading.said) {
            self.notice = Some((said, true));
        }

        if let Some(missing) = self.store.missing() {
            self.api.ask(missing);
        }

        if let Some(id) = self.send.waiting_for() {
            self.api.ask(Ask::Tx(id.to_string()));
        }

        self.keep_listening(time);
    }

    /// Opens the node's stream again once it was lost, or fell silent.
    fn keep_listening(&mut self, time: f64) {
        if matches!(self.streaming, Streaming::Lost(_)) && time - self.asked.stream >= STREAM_RETRY {
            self.streaming = Streaming::Connecting;
            self.api.listen();
        }

        if self.light.silent(time) {
            self.light.streamed = false;
            self.streaming = Streaming::Connecting;
            self.api.listen();
        }
    }

    /// Asks for whatever is due.
    fn poll(&mut self, time: f64) {
        if self.settings.read_blocks {
            self.poll_reading(time);

            return;
        }

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

        self.keep_listening(time);
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
        let up = match self.settings.read_blocks {
            true => self.api.down().is_none(),
            false => self.store.status.is_some() && !self.store.failures.contains_key(&Ask::Status),
        };

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
            (None, Some((tip, final_height))) if self.light.has_followed() && self.light.fast_resumed.is_some() => {
                (palette.good, "light client following", widgets::fast_resumed_story(&self.light.fast_resumed_words().unwrap_or_default(), tip, final_height))
            }
            (None, Some((tip, final_height))) if self.light.has_followed() => (
                palette.good,
                "light client following",
                format!("The light client checked every header to {tip}, and finality to {final_height}, trusting only the genesis it first saw at this node"),
            ),
            (None, Some((tip, _))) => (palette.warn, "light client catching up", format!("{tip} headers checked so far")),
            (None, None) => (palette.muted, "light client starting", "Fetching the chain's light parameters".to_string()),
        }
    }

    /// The whole story of the light client and the node, for where only a dot has room.
    fn story(&self) -> String {
        let (_, light_words, light_story) = self.light_state();
        let (_, words) = self.health();

        format!("{light_words}: {light_story}\nThe node at {} is {words}", self.api.base())
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
                if layout::is_short(ui.ctx()) {
                    let (light_color, _, _) = app.light_state();
                    let (color, words) = app.health();
                    let story = app.story();

                    ui.add_space(8.0);
                    widgets::folded_foot(
                        ui,
                        |ui| {
                            widgets::status_dot(ui, &palette, color, color == palette.bad, &story);
                            views::dot(ui, light_color);
                            ui.label(RichText::new(words).small().color(palette.soft));
                        },
                        |_| {},
                    );

                    return;
                }

                ui.add_space(18.0);
                let (light_color, light_words, light_story) = app.light_state();

                if let Some(said) = app.light.fast_resumed_words() {
                    ui.add(egui::Label::new(RichText::new(said).small().color(palette.warn)).wrap());
                }

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
                // A short window, as a phone on its side, keeps its room for the pages.
                ui.add_space(layout::sidebar_gap(ui.ctx(), 26.0));
                widgets::sidebar_name(ui, &palette, &name, "Explorer");
                ui.add_space(layout::sidebar_gap(ui.ctx(), 24.0));
                ui.spacing_mut().item_spacing.y = chainv4_ui::Density::COMFORTABLE.spacing.y;

                for (group, pages) in View::GROUPS {
                    let folded = app.settings.folded.contains(group);

                    if widgets::group_heading(ui, &palette, group, folded).clicked() {
                        match folded {
                            true => app.settings.folded.remove(group),
                            false => app.settings.folded.insert(group.to_string()),
                        };
                    }

                    for (view, name) in pages {
                        if !app.view.lists(view, folded) {
                            continue;
                        }

                        if widgets::nav_entry(ui, &palette, name, app.view.marks(view)).clicked() {
                            app.go(view.clone());
                        }
                    }

                    ui.add_space(layout::sidebar_gap(ui.ctx(), 18.0));
                }
            },
        );
    }

    /// On a wide screen, over the page: back and forward, the search, and where the chain is.
    fn header(&mut self, ui: &mut Ui) {
        let palette = self.palette;
        ui.horizontal(|ui| {
            if ui.add_enabled_ui(!self.back.is_empty(), |ui| widgets::quiet(ui, &palette, "‹")).inner.on_hover_text("Back").clicked() {
                self.back();
            }

            if ui.add_enabled_ui(!self.ahead.is_empty(), |ui| widgets::quiet(ui, &palette, "›")).inner.on_hover_text("Forward").clicked() {
                self.forward();
            }

            let time = ui.input(|input| input.time);
            let api = &self.api;
            let working = self.reloading.working(time, |ask| api.asking(ask));

            if widgets::reload(ui, &palette, working) {
                self.reload(time);
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
                    widgets::bar_name(ui, &palette, &name, BAR_NAME).on_hover_text(&name);
                }
                false => {
                    if widgets::quiet(ui, &palette, "‹").clicked() {
                        self.back();
                    }
                }
            }

            let (color, _) = self.health();
            let story = self.story();
            widgets::status_dot(ui, &palette, color, color == palette.bad, &story);

            // A narrow window a mouse is on has no finger to pull the page down with.
            if !layout::touch(ui.ctx()) {
                let time = ui.input(|input| input.time);
                let api = &self.api;
                let working = self.reloading.working(time, |ask| api.asking(ask));

                if widgets::reload(ui, &palette, working) {
                    self.reload(time);
                }
            }

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
        if let Some(view) = widgets::tabs(ui, &self.palette, &View::TABS, &self.view.tab()) {
            self.go(view.clone());
        }
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
            .corner_radius(ui.visuals().widgets.inactive.corner_radius)
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

        // The node not answering: said on every page, whichever it is, with a way to ask again.
        let down = match self.settings.read_blocks {
            true => self.api.down(),
            false => self.store.failures.get(&Ask::Status).filter(|failure| failure.is_trouble()).cloned(),
        };

        if let Some(failure) = down
            && widgets::retry(ui, &self.palette, &failure.sentence(), self.api.asking(&Ask::Status))
        {
            self.reload(ui.input(|input| input.time));
        }

        let time = ui.input(|input| input.time);
        let api = &self.api;
        let working = self.reloading.working(time, |ask| api.asking(ask));
        let mut pulled = false;
        let palette = self.palette;
        let off = self.settings.read_blocks.then(|| views::reading::turned_off(&self.view)).flatten();
        let mut ctx = Ctx {
            store: &self.store,
            reader: &self.reader,
            light: &mut self.light,
            reading: &mut self.reading,
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
            let area = ui.available_rect_before_wrap();
            let scroll = egui::ScrollArea::vertical().id_salt("page").auto_shrink(false);

            self.scrolled.show(ui, scroll, |ui| {
                layout::keep_focus_in_view(ui);
                ui.vertical_centered(|ui| {
                    ui.set_max_width(ui.available_width().min(COLUMN));
                    ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| match &self.view {
                        _ if off.is_some() => {
                            if let Some((title, why)) = off {
                                views::reading::needs_asking(ui, &mut ctx, title, why);
                            }
                        }
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

            pulled = self.pull.show(ui, &palette, area, self.scrolled.offset(), self.scrolled.drag(), working);
        }

        if pulled {
            self.reload(time);
        }

        // What the page asked, for a reload to ask again.
        let sent = self.api.sent();
        self.asked_by_view.note(&self.view, sent.into_iter().filter(Ask::reloads));

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

        let bar = widgets::bar_frame(&palette);
        // A finger needs taller buttons and fields than a mouse: on a phone, upright or on its side.
        let fingers = narrow || layout::touch(ui.ctx());
        let sized = |ui: &mut Ui| {
            if fingers {
                layout::finger_sized(ui);
            }
        };

        match narrow {
            true => {
                egui::Panel::top("bar").frame(bar.inner_margin(layout::BAR_MARGIN)).show(ui, |ui| {
                    layout::finger_row(ui);
                    self.bar(ui);
                });
                egui::Panel::bottom("tabs").frame(bar).show(ui, |ui| self.tabs(ui));
            }
            false => {
                egui::Panel::left("nav")
                    .exact_size(layout::SIDEBAR_WIDTH)
                    .resizable(false)
                    .frame(bar.inner_margin(egui::Margin::symmetric(layout::SIDEBAR_MARGIN, 0)))
                    .show(ui, |ui| {
                        sized(ui);
                        self.sidebar(ui, now);
                    });
                egui::Panel::top("header")
                    .frame(egui::Frame::new().fill(palette.paper).inner_margin(egui::Margin::symmetric(layout::page_margin(false), 10)))
                    .show(ui, |ui| {
                        if fingers {
                            layout::finger_row(ui);
                        }

                        self.header(ui);
                    });
            }
        }

        let margin = egui::Margin::symmetric(layout::page_margin(narrow), if narrow { 14 } else { 4 });
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

        if self.settings.read_blocks {
            self.reading.save(0.0);
        }
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
pub(crate) mod tests {
    use {
        super::*,
        chainv4::consensus::ConsensusConfig,
        chainv4::crypto::hashable::Hashable,
        chainv4::crypto::keypair::Keypair,
        chainv4::light::{BlockRange, LightClient, LightFinality, LightParams, NoProofs, RangeBody, RangeChanges, RangeItem},
        chainv4::structs::address::Address,
        chainv4::structs::block::{Block, BlockHeader, Seal, SealedHeader},
        chainv4::structs::hash::{Hash, HashType},
        chainv4::structs::receipt::receipts_root,
        chainv4::structs::u256::u256,
        crate::api::Route,
        serde_json::json,
        std::sync::{Arc, Mutex},
    };

    fn params() -> LightParams {
        LightParams {
            hash: HashType::default_chain(),
            consensus: ConsensusConfig::Authority { key: Keypair::from_seed([7; 32]).public(), slot_ms: 1_000 },
            finality: Some(LightFinality::Authority { checkpoint_interval: 8, fixed_ahead: 2 }),
            pow_window: None,
            governance: None,
        }
    }

    /// A genesis header, committing to these parameters as a chain's does.
    fn genesis() -> SealedHeader {
        let hash = HashType::default_chain();
        let zero = Hash::zero(hash);

        SealedHeader {
            header: BlockHeader {
                parent: zero,
                height: 0,
                timestamp: 0,
                producer: Address::Ed25519([0; 32]),
                difficulty: 0,
                tx_root: Block::tx_root(hash, &[]),
                proof_root: params().hash(hash),
                pool_root: Block::pool_root(hash, None),
                receipts_root: receipts_root(hash, &[]),
                state_root: zero,
                gas_used: 0,
                base_price: u256::ZERO,
            },
            seal: Seal::None,
        }
    }

    /// A light client at genesis of a chain under voting finality.
    pub fn following_client() -> LightClient {
        let genesis = genesis();
        let trusted = genesis.id();

        LightClient::new(params(), genesis, trusted).unwrap()
    }

    /// A node on a real socket that has a genesis block to give and nothing else to say, and notes
    /// the first line of every request it gets. It answers everything, as what is checked is what
    /// reached it.
    fn a_node() -> (String, Arc<Mutex<Vec<String>>>) {
        use std::io::{Read, Write};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let heard: Arc<Mutex<Vec<String>>> = Arc::default();
        let noted = heard.clone();
        let genesis_range = {
            let item = RangeItem { sealed: genesis(), body: RangeBody::Whole { txs: Vec::new(), proofs: Vec::new(), pool: None, receipts: Some(Vec::new()) }, changes: RangeChanges::None(NoProofs::NoList) };

            wincode::serialize(&BlockRange { tip: 0, items: vec![item], next: None }).unwrap()
        };

        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else {
                    break;
                };
                let (noted, genesis_range) = (noted.clone(), genesis_range.clone());

                std::thread::spawn(move || {
                    let mut got = Vec::new();
                    let mut buffer = [0; 4096];

                    while !got.windows(4).any(|end| end == b"\r\n\r\n") {
                        match stream.read(&mut buffer) {
                            Ok(read) if read > 0 => got.extend_from_slice(&buffer[..read]),
                            _ => break,
                        }
                    }

                    let line = String::from_utf8_lossy(&got).lines().next().unwrap_or_default().trim_end_matches(" HTTP/1.1").to_string();
                    let body: Vec<u8> = match line.as_str() {
                        line if line.contains("/blocks/range") => genesis_range,
                        line if line.contains("/light/bundle") => json!({ "headers": [], "steps": [], "tip": 0 }).to_string().into_bytes(),
                        line if line.contains("/headers") => json!({ "headers": [] }).to_string().into_bytes(),
                        line if line.contains("/light/finality") => json!({ "steps": [] }).to_string().into_bytes(),
                        // A stream that stays open and says nothing.
                        line if line.contains("/stream") => {
                            stream.write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n").ok();
                            noted.lock().unwrap().push(line.to_string());
                            std::thread::sleep(std::time::Duration::from_secs(120));

                            return;
                        }
                        _ => b"{}".to_vec(),
                    };

                    noted.lock().unwrap().push(line);
                    stream.write_all(format!("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n", body.len()).as_bytes()).ok();
                    stream.write_all(&body).ok();
                });
            }
        });

        (base, heard)
    }

    /// An explorer on that node, with the setting as asked, following a chain already.
    fn an_explorer(node: &str, reading: bool) -> Explorer {
        let creation = eframe::CreationContext::_new_kittest(egui::Context::default());
        let mut explorer = Explorer::new(&creation, Some(node.to_string()), node, None);
        let client = following_client();

        explorer.settings.read_blocks = reading;
        explorer.settings.pinned.insert(node.to_string(), client.sealed_at(0).unwrap().id().to_string());
        explorer.connect(node);
        explorer.light = Light::following_with(client);

        explorer
    }

    /// Runs its frames for a moment: what arrives is taken, what is due is asked, the page drawn.
    fn run_for(explorer: &mut Explorer, seconds: f64, time: &mut f64, size: (f32, f32)) {
        let context = egui::Context::default();
        let started = web_time::Instant::now();

        while started.elapsed().as_secs_f64() < seconds {
            *time += 1.0;
            let now = now_ms();
            explorer.take(now, *time);
            explorer.poll(*time);

            let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(size.0, size.1))), ..egui::RawInput::default() };
            let mut output = context.run_ui(input, |ui| explorer.page(ui, now, layout::is_narrow(size.0)));

            output.textures_delta.clear();
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Every page there is.
    fn every_view() -> Vec<View> {
        let groups = View::GROUPS.iter().flat_map(|(_, pages)| pages.iter().map(|(view, _)| view.clone()));
        let open = [
            View::Block("0".to_string()),
            View::Block("5".to_string()),
            View::Tx("0x01".to_string()),
            View::Account("ed25519:0x0101010101010101010101010101010101010101010101010101010101010101".to_string()),
            View::Name("alice.dev".to_string()),
            View::Collection("contract:0x01".to_string()),
            View::Transfer("0x01".to_string()),
            View::Release(1),
            View::Multisig("multisig:0x01".to_string()),
            View::More,
        ];

        groups.chain(open).collect()
    }

    #[test]
    fn with_the_setting_on_opening_every_page_and_searching_sends_only_what_may_leave_and_the_chain_shows_from_blocks() {
        let (node, heard) = a_node();
        let mut explorer = an_explorer(&node, true);
        let mut time = 0.0;

        for size in [(360.0, 740.0), (740.0, 360.0), (1200.0, 800.0)] {
            for view in every_view() {
                explorer.view = view;
                run_for(&mut explorer, 0.03, &mut time, size);
            }
        }

        // A search for an address, a height, an id and a name tells the node nothing.
        for search in ["ed25519:0x0202020202020202020202020202020202020202020202020202020202020202", "7", "0x0303030303030303030303030303030303030303030303030303030303030303", "bob.dev"] {
            explorer.search = search.to_string();
            explorer.find();
            run_for(&mut explorer, 0.05, &mut time, (1200.0, 800.0));
        }

        run_for(&mut explorer, 0.2, &mut time, (1200.0, 800.0));

        let lines = heard.lock().unwrap().clone();
        let may_leave = |line: &str| line.contains("/light") || line.contains("/headers") || line.contains("/blocks/range") || line.contains("/stream") || line.ends_with("/release");

        assert!(lines.iter().any(|line| line.contains("/blocks/range")), "it reads: {lines:?}");
        assert!(lines.iter().all(|line| may_leave(line)), "{:?}", lines.iter().filter(|line| !may_leave(line)).collect::<Vec<_>>());
        assert_eq!(explorer.store.status.as_ref().map(|status| status.height), Some(0), "what it shows comes from what it read");
        assert_eq!(explorer.store.status.as_ref().map(|status| status.coin.symbol.clone()), Some("units".to_string()), "no coin was ever said here");
        assert_eq!(explorer.store.top(), Some(0), "the feed comes from the index");
        assert!(explorer.reading.is_open());
    }

    /// The whole app against a node on a private chain under voting finality, keeping what it reads in
    /// a real folder: it walks, reads from the final block on, shows blocks from what it read, puts them
    /// away and goes on from there. Started by hand, as it needs a node.
    #[test]
    #[ignore = "needs a node with voting finality: CHAINV4_NODE=http://127.0.0.1:8099 cargo test --lib the_whole_app -- --ignored --nocapture"]
    fn the_whole_app_reads_a_real_node_from_its_final_block_and_goes_on_from_there_when_opened_again() {
        let node = std::env::var("CHAINV4_NODE").expect("CHAINV4_NODE names a node");
        let folder = std::env::temp_dir().join(format!("chainv4-explorer-reading-{}", std::process::id()));
        std::fs::remove_dir_all(&folder).ok();
        let open = |folder: &std::path::Path| {
            let creation = eframe::CreationContext::_new_kittest(egui::Context::default());
            let mut explorer = Explorer::new(&creation, Some(node.clone()), &node, Some(folder.to_path_buf()));

            explorer.settings.read_blocks = true;
            explorer.connect(&node);

            explorer
        };
        let settle = |explorer: &mut Explorer, time: &mut f64, done: &dyn Fn(&Explorer) -> bool| {
            let deadline = web_time::Instant::now() + Duration::from_secs(60);

            while !done(explorer) {
                assert!(web_time::Instant::now() < deadline, "{:?} {:?} {:?}", explorer.light.trouble, explorer.reading.standing(), explorer.store.status.as_ref().map(|status| status.height));
                run_for(explorer, 0.02, time, (1200.0, 800.0));
            }
        };
        let mut time = 0.0;
        let mut explorer = open(&folder);

        settle(&mut explorer, &mut time, &|explorer| explorer.reading.standing().is_some_and(|standing| standing.read >= 3) && explorer.store.top().is_some_and(|top| top > 0));

        let first = explorer.reading.standing().unwrap();
        let top = explorer.store.top().unwrap();

        assert_eq!(explorer.store.status.as_ref().map(|status| status.coin.symbol.clone()), Some("units".to_string()));
        assert!(explorer.store.bottom().unwrap() >= first.known_from, "nothing older than it knows");
        assert!(explorer.light.fast_resumed.is_none());
        println!("read to {} from {}, {} bytes down, {} kept", first.high, first.known_from, first.downloaded, first.bytes);

        // A search for what it read finds it, and for what it did not says so.
        explorer.view = View::Block(top.to_string());
        settle(&mut explorer, &mut time, &|explorer| explorer.store.detail_for(&top.to_string()).is_some());
        assert_eq!(explorer.store.detail_for(&top.to_string()).unwrap().summary.height, top);

        let older = first.known_from.saturating_sub(1).to_string();
        explorer.view = View::Block(older.clone());
        settle(&mut explorer, &mut time, &|explorer| explorer.store.failures.get(&Ask::Block(older.clone())).is_some_and(|failure| failure.status == 404));

        // Put away, it comes back from the device, checks, and goes on from where it got to.
        explorer.reading.close(time);
        std::thread::sleep(Duration::from_millis(300));
        let mut again = open(&folder);
        let mut time_again = 0.0;

        settle(&mut again, &mut time_again, &|explorer| explorer.reading.standing().is_some_and(|standing| standing.read > first.read));

        let standing = again.reading.standing().unwrap();

        assert_eq!(standing.known_from, first.known_from, "it knows from where it did");
        assert!(standing.downloaded > first.downloaded && standing.high > first.high);
        assert!(again.reading.said.is_empty() && again.notice.is_none(), "{:?} {:?}", again.reading.said, again.notice);
        println!("came back at {}, read to {}", first.high, standing.high);
        again.reading.close(time_again);
        std::thread::sleep(Duration::from_millis(300));

        let files: Vec<String> = std::fs::read_dir(folder.join("explorer-index")).unwrap().flatten().map(|file| file.file_name().to_string_lossy().into_owned()).collect();

        assert!(files.iter().any(|name| name.ends_with("-head.index")) && files.iter().any(|name| name.contains("-b0-.index")), "{files:?}");
        std::fs::remove_dir_all(&folder).ok();
    }

    #[test]
    fn with_the_setting_on_the_stream_only_wakes_it_and_nothing_it_says_is_taken() {
        let (node, _) = a_node();
        let mut explorer = an_explorer(&node, true);
        let summary = json!({ "id": "0xforged", "height": 99, "parent": "0x00", "timestamp": 1, "producer": "ed25519:0x01", "difficulty": "0", "tx_root": "0x00", "gas_used": 0, "base_price": "0", "tx_count": 0, "proof_count": 0, "main": true });

        explorer.news(News::Event { name: "block".to_string(), data: summary.clone() }, 1, 1.0);
        explorer.news(News::Event { name: "queued".to_string(), data: json!({}) }, 1, 1.0);

        assert_eq!(explorer.store.top(), None, "not taken");
        assert!(explorer.store.feed.is_empty());

        // With it off, the same is taken as it always was.
        let (node, _) = a_node();
        let mut explorer = an_explorer(&node, false);

        explorer.news(News::Event { name: "block".to_string(), data: summary }, 1, 1.0);
        assert_eq!(explorer.store.top(), Some(99));
        assert_eq!(explorer.store.feed.len(), 1);
    }

    #[test]
    fn with_the_setting_off_the_explorer_asks_the_node_and_shows_the_tip_as_it_always_did() {
        let (node, heard) = a_node();
        let mut explorer = an_explorer(&node, false);
        let mut time = 0.0;

        explorer.view = View::Account("ed25519:0x0101010101010101010101010101010101010101010101010101010101010101".to_string());
        run_for(&mut explorer, 0.3, &mut time, (1200.0, 800.0));

        let lines = heard.lock().unwrap().clone();

        for route in ["/status", "/node", "/release", "/accounts/ed25519:0x0101010101010101010101010101010101010101010101010101010101010101"] {
            assert!(lines.iter().any(|line| line.contains(route)), "{route} in {lines:?}");
        }

        assert!(!lines.iter().any(|line| line.contains("/blocks/range")), "{lines:?}");
        assert!(!explorer.reading.is_open(), "nothing here happens at all");
    }

    #[test]
    fn a_coin_a_node_said_is_kept_for_the_chain_with_the_setting_off_and_shown_with_it_on() {
        let (node, _) = a_node();
        let mut explorer = an_explorer(&node, false);
        let status = json!({
            "genesis": "0xchain", "tip": "t", "height": 1, "timestamp": 0, "difficulty": "0", "work": "1",
            "hash": "blake3", "coin": { "symbol": "DEV", "decimals": 18 }, "finality": null, "fees": null,
        });

        explorer.answered(Ask::Status, status.clone(), 0);
        assert_eq!(explorer.settings.coins.get("0xchain"), Some(&("DEV".to_string(), 18)));

        // A status answered with the setting on keeps nothing of it: nobody said so.
        explorer.settings.coins.clear();
        explorer.settings.read_blocks = true;
        explorer.answered(Ask::Status, status, 0);
        assert!(explorer.settings.coins.is_empty());
    }

    #[test]
    fn with_the_setting_on_fast_resume_is_off_even_on_a_node_trusted_for_it() {
        let (node, _) = a_node();
        let mut explorer = an_explorer(&node, true);

        explorer.settings.fast_resume.insert(node.clone(), true);
        explorer.light = Light::default();
        // What starting from the node says: no checkpoint is asked for, as it walks.
        let params = wincode::serialize(&params()).unwrap();
        let header = wincode::serialize(&genesis()).unwrap();
        let light = json!({ "params": chainv4::structs::hex::to_hex(&params), "header": chainv4::structs::hex::to_hex(&header) });

        explorer.api.answered_here(Ask::Light, Ok(light));
        explorer.take(now_ms(), 1.0);

        assert!(explorer.light.client().is_some(), "{:?}", explorer.light.trouble);
        assert_eq!(explorer.light.fast_resumed, None);
    }

    #[test]
    fn every_route_an_explorer_has_is_decided_with_the_setting_on() {
        // What is decided by Ask::route's naming every request: this states the table the guide gives.
        let goes = |ask: &Ask| ask.route() == Route::Goes;

        assert!(goes(&Ask::Light) && goes(&Ask::Submit("{}".to_string())) && goes(&Ask::Bundle { headers: 0, epoch: 0 }) && goes(&Ask::Release));
        assert_eq!(Ask::Status.route(), Route::Index);
        assert_eq!(Ask::Blocks { before: None, limit: 1 }.route(), Route::Index);
        assert_eq!(Ask::Tx("0x01".to_string()).route(), Route::Index);
        assert_eq!(Ask::Proof { column: "accounts".to_string(), key: "0x01".to_string(), block: None }.route(), Route::Off, "only one pressed for goes");
        assert_eq!(Ask::ReceiptProof("0x01".to_string()).route(), Route::Off);
        assert_eq!(Ask::Rpc("{}".to_string()).route(), Route::Off);
    }

    #[test]
    fn retrying_clears_why_it_failed_and_a_page_says_it_is_loading_again() {
        let mut store = Store::default();
        let mut api = Api::new("http://127.0.0.1:1", Err("not a port: 0".to_string()), egui::Context::default());

        api.ask(Ask::Mempool);
        for reply in api.replies() {
            if let Reply::Answer { ask, result: Err(failure) } = reply {
                store.failures.insert(ask, failure);
            }
        }

        assert!(store.failures.contains_key(&Ask::Mempool));

        // What Retry does: asked again at once, with why it failed gone by the next frame.
        api.retry(Ask::Mempool);
        retrying(&mut store, api.retried());

        assert!(!store.failures.contains_key(&Ask::Mempool));
        assert_eq!(api.replies().len(), 1, "and it went out");
    }

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
