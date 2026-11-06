//! What happened to the account, at all its addresses together, newest first: what it sent, from
//! this wallet's own log until proven, and every transaction the node's address index says touched
//! one of them, each shown once the light client proved who sent what and what it did. Only the
//! chain the wallet follows now: a new chain of the same keys has its own heights. Searched, sorted
//! either way, and filtered: by kind, direction, counterparty, token and date on the node, so all
//! the history counts and not only what is loaded, each line still proven before it says so. The
//! node only says where to look, under a filter as without one. An older node ignores filters and
//! says nothing of them, which is how it is told apart: then what it listed is tested here, once
//! proven, and the page says only what was read was searched.
//! An EVM call and its logs read by name, and an ERC-20's or an NFT's transfer counts as a token's
//! does, though from a contract the registry does not list, it is only shown as what the log claims.
//! Such a claim's amount reads in what the contract says of itself, labelled as unverified: see
//! `unlisted`, which is this page's alone.

mod unlisted;

use {
    super::{Ctx, decoded},
    crate::app::{Role, Sent},
    crate::format,
    crate::history::{Checked, KeptCall, KeptLog, Move, Origin, with_evm_forms},
    crate::node::Ask,
    crate::theme,
    chainv4::structs::address::Address,
    chainv4_ui::filters::{self, Answered, COIN, Facts, Offered, TxFilter},
    egui::{RichText, Ui},
    std::collections::HashSet,
    unlisted::{Said, UNVERIFIED, Unlisted},
};

/// How long a sent transaction may go unseen before the history says the node may have dropped it,
/// in milliseconds.
pub const UNSEEN_AFTER: u64 = 10 * 60 * 1_000;

/// The widest a line's amount and pill get.
const AMOUNT_WIDTH: f32 = 150.0;
/// How large an amount in what a contract says of itself shows.
const NAMED_SIZE: f32 = 12.0;

/// How often the node is asked again for what passes the filters, in milliseconds: each ask has it
/// read its index, and what is there changes only as blocks come.
const FILTERED_EVERY: u64 = 15_000;
/// How often what it listed is asked to be proven, in milliseconds, and how many at a time.
const PROVEN_EVERY: u64 = 6_000;
const PROVEN_AT_ONCE: usize = 40;

/// Which lines show, of what the filters leave.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    #[default]
    All,
    /// Not proven yet.
    Waiting,
    Failed,
}

#[derive(Default)]
pub struct ActivityState {
    kind: Kind,
    search: String,
    oldest_first: bool,
    filter: TxFilter,
    /// The filter's query last asked of the node, and when; and when its lines were last asked to
    /// be proven.
    asked: Option<(String, u64)>,
    proved: u64,
    /// What contracts the registry does not list said of themselves. Here, and so out of reach of
    /// every other page: nothing someone signs or sends, and no balance, can come to rest on it.
    unlisted: Unlisted,
}

impl ActivityState {
    /// For a reload: asks again for what the filters leave.
    pub fn reload(&mut self) {
        self.asked = None;
        self.proved = 0;
    }

    /// Takes the node's answer to what an unlisted contract was asked: false for anyone else's.
    pub fn answered(&mut self, body: &str, result: &Result<serde_json::Value, crate::node::Failure>, now: u64) -> bool {
        self.unlisted.answered(body, result, now)
    }
}

/// How the node's answers under the filters stand, over all the account's addresses.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Listed {
    /// Each transaction it listed, with where it landed.
    mentions: Vec<(String, u64)>,
    /// Where each address's next page starts.
    next: Vec<(String, String)>,
    answered: Answered,
    /// Whether every address has been answered for.
    read: bool,
}

/// What the node said under this query, for these addresses, put together.
fn listed(chain: &crate::chain::Chain, addresses: &[String], query: &str) -> Listed {
    let mut listed = Listed { read: true, answered: Answered { filtered: true, ..Answered::default() }, ..Listed::default() };

    for address in addresses {
        let Some(page) = chain.filtered.get(&(address.clone(), query.to_string())) else {
            listed.read = false;
            continue;
        };

        listed.mentions.extend(page.mentions.iter().map(|mention| (mention.tx.clone(), mention.height)));
        listed.next.extend(page.next.clone().map(|next| (address.clone(), next)));
        listed.answered.filtered &= page.answered.filtered;
        listed.answered.limited |= page.answered.limited;
        listed.answered.scanned += page.answered.scanned;
    }

    listed
}

/// What can be said of a proven transaction, for testing it here against the filters.
fn facts<'a>(checked: &'a Checked, addresses: &[String]) -> Facts<'a> {
    let moved = checked.moves.iter();

    Facts {
        kinds: checked.kinds.iter().map(String::as_str).collect(),
        sent: addresses.contains(&checked.sender),
        took_part: checked.for_account(addresses).any(|(incoming, _)| incoming),
        parties: std::iter::once(checked.sender.as_str()).chain(moved.clone().flat_map(|moved| [moved.from.as_deref(), moved.to.as_deref()]).flatten()).collect(),
        tokens: moved.map(|moved| moved.token.as_deref().unwrap_or(COIN)).collect(),
        time: (checked.time > 0).then_some(checked.time),
    }
}

/// The tokens the filter offers by name: those the chain lists, as proven.
fn offered_tokens(ctx: &Ctx) -> Vec<(String, String)> {
    ctx.chain.listed.iter().filter(|token| ctx.chain.tokens.contains_key(*token)).map(|token| (token.clone(), ctx.token(token).0)).collect()
}

pub fn show(ui: &mut Ui, ctx: &mut Ctx, state: &mut ActivityState) {
    let palette = *ctx.palette;
    theme::title(ui, &palette, "Activity", "Everything this account sent, and everything that touched its addresses, as the light client proved it.");

    let reading = ctx.settings.read_blocks;
    let note = match (ctx.chain.indexes_addresses, ctx.chain.indexes_events) {
        // Nothing asks a node's indexes, with this on: what shows came from the blocks read.
        _ if reading => None,
        (Some(false), Some(false)) => Some("This node keeps no address index, nor an event index: only what this wallet sent shows. Start it with --index-addresses."),
        (Some(false), _) => Some("This node keeps no address index, so coins sent here show in the balance but not below. Start it with --index-addresses."),
        _ => None,
    };

    if let Some(note) = note {
        ui.add(egui::Label::new(chainv4_ui::widgets::flagged(ui, note, egui::TextStyle::Body, palette.warn)).wrap());
        ui.add_space(10.0);
    }

    if reading {
        let known = ctx.reading.standing().map_or_else(|| "Reading checked blocks has not started yet: nothing shows until it has.".to_string(), |standing| format!("From checked blocks only: this wallet knows from block {}, and shows nothing older. Settings reads further back.", format::thousands(&standing.known_from.to_string())));

        ui.add(egui::Label::new(RichText::new(known).small().color(palette.soft)).wrap());
        ui.add_space(10.0);
    }

    let tokens = offered_tokens(ctx);
    let problem = state.filter.problem(|text| text.parse::<Address>().is_ok());
    filters::controls(ui, &palette, &mut state.filter, Offered { coin: &ctx.coin.symbol, tokens: &tokens }, problem.as_deref());
    ui.add_space(2.0);
    // Which and in what order, on one line where it fits.
    ui.horizontal_wrapped(|ui| {
        theme::segmented(ui, &palette, &mut state.kind, &[(Kind::All, "All"), (Kind::Waiting, "Waiting"), (Kind::Failed, "Failed")]);
        ui.add_space(6.0);
        theme::segmented(ui, &palette, &mut state.oldest_first, &[(false, "Newest first"), (true, "Oldest first")]);
    });
    ui.add(egui::TextEdit::singleline(&mut state.search).hint_text("Search: a transaction, an address, a contract, what it did").desired_width(f32::INFINITY));
    ui.add_space(8.0);

    let addresses = ctx.addresses();
    // An EVM move names its parties by the 20 bytes each goes by there. What the node is asked stays
    // about the addresses themselves, which are all it can look up.
    let parties = with_evm_forms(&addresses);
    // What was typed wrong is said above, and the list stays as it was until it reads.
    let query = (state.filter.on() > 0 && problem.is_none()).then(|| state.filter.query(ctx.now));
    let under = query.as_ref().map(|query| listed(ctx.chain, &addresses, query));

    if let Some(query) = query.as_ref().filter(|_| !reading) {
        ask_filtered(ctx, state, &addresses, query, under.as_ref());
    }

    let mut entries = match &under {
        None => entries(ctx, &parties),
        Some(listed) => entries_under(ctx, &parties, listed, &state.filter),
    };
    entries.retain(|entry| entry.is(state.kind) && (entry.matches(&state.search) || reads_as(ctx, entry, &state.search)));

    // Reading blocks, what was proven before is older than what it knows, or was asked of a node: it
    // shows no history but what it read, and what this wallet sent and looked up.
    if reading {
        entries.retain(|entry| entry.sent.is_some() || entry.checked.is_some_and(|checked| checked.came != Origin::Proven));
    }

    if state.oldest_first {
        entries.reverse();
    }

    let genesis = ctx.chain.genesis.map(|genesis| genesis.to_string());
    let said = genesis.as_deref().map_or_else(Said::default, |genesis| Said::on(&state.unlisted, genesis));
    let shown = entries.len();
    let mut wanted = Vec::new();

    theme::card(ui, &palette, |ui| {
        if entries.is_empty() {
            let looking = !reading && under.as_ref().is_some_and(|listed| !listed.read);
            ui.label(RichText::new(match (state.kind, state.search.trim().is_empty(), ctx.chain.has_followed(), query.is_some()) {
                _ if looking => "Asking the node…",
                (_, _, _, true) => "Nothing matches.",
                (Kind::All, true, true, _) => "Nothing yet.",
                (Kind::All, true, false, _) => "Looking once the light client catches up…",
                _ => "Nothing matches.",
            }).color(palette.muted));

            return;
        }

        for (index, entry) in entries.iter().enumerate() {
            if index > 0 {
                theme::rule(ui, &palette);
            }

            wanted.extend(line(ui, ctx, &parties, entry, said));
        }
    });

    // Only what these lines show is asked about, and only by an online wallet that knows its chain:
    // an offline signer has no such page, and nothing to send a question by if it had.
    if ctx.settings.role == Some(Role::Online)
        && !reading
        && let Some(genesis) = &genesis
        && let Some(ask) = state.unlisted.next(genesis, &wanted, ctx.now)
    {
        ctx.node.ask(ask);
    }

    if let (Some(query), Some(listed)) = (&query, &under)
        && !reading
    {
        further(ui, ctx, query, listed, state.filter.on(), shown);
    }

    // Reading checked blocks asks no node for its lists, so there is no page of one to go on with:
    // what it shows is what it read, and Settings reads further back, as the note above says.
    if under.is_none() && !reading {
        older(ui, ctx, &addresses);
    }
}

/// Below the whole history: asking the node for the page after the one each of its lists ended for
/// each address, as it lists the newest first, a page at a time. Each line still waits for its proof.
fn older(ui: &mut Ui, ctx: &mut Ctx, addresses: &[String]) {
    let asks = ctx.chain.older_pages(addresses);

    if asks.is_empty() {
        return;
    }

    let palette = *ctx.palette;
    ui.horizontal(|ui| match asks.iter().any(|ask| ctx.node.asking(ask)) {
        true => {
            ui.spinner();
        }
        false => {
            if theme::quiet(ui, &palette, "Older transactions").clicked() {
                for ask in asks {
                    ctx.node.retry(ask);
                }
            }
        }
    });
    theme::section_gap(ui);
}

/// Asks the node what passes the filters, and for proof of what it listed: the filter's own page,
/// so nothing goes on while it is not looked at.
fn ask_filtered(ctx: &mut Ctx, state: &mut ActivityState, addresses: &[String], query: &str, under: Option<&Listed>) {
    if state.asked.as_ref().is_none_or(|(asked, at)| asked != query || ctx.now.saturating_sub(*at) >= ctx.settings.pace().every_ms(FILTERED_EVERY)) {
        state.asked = Some((query.to_string(), ctx.now));

        for address in addresses {
            ctx.node.ask(Ask::Filtered { address: address.clone(), query: query.to_string(), cursor: None });
        }
    }

    if !ctx.chain.caught_up() || ctx.now.saturating_sub(state.proved) < ctx.settings.pace().every_ms(PROVEN_EVERY) {
        return;
    }

    state.proved = ctx.now;
    let genesis = ctx.settings.chain_shown(ctx.chain.genesis);
    let unsettled = under.into_iter().flat_map(|listed| &listed.mentions).filter(|(tx, _)| !ctx.settings.checked_on(tx, genesis.as_deref()).is_some_and(|checked| checked.final_));

    for (tx, _) in unsettled.take(PROVEN_AT_ONCE) {
        ctx.node.ask(Ask::Receipt(tx.clone()));
    }
}

/// What the filters leave. On a node that filtered, what it listed, each line waiting for its proof
/// as any other. On an older one, which listed everything, or one with no index to list from, what
/// passes the same test here among what is proven: a line not proven yet cannot be said to pass.
fn entries_under<'a>(ctx: &'a Ctx, addresses: &[String], listed: &Listed, filter: &TxFilter) -> Vec<Entry<'a>> {
    let mut seen = HashSet::new();
    let mut found = Vec::new();
    let genesis = ctx.settings.chain_shown(ctx.chain.genesis);

    if !ctx.settings.read_blocks && listed.answered.filtered && ctx.chain.indexes_addresses != Some(false) {
        for (tx, height) in &listed.mentions {
            if seen.insert(tx.clone()) {
                found.push(Entry { tx: tx.clone(), checked: ctx.settings.checked_on(tx, genesis.as_deref()), sent: None, height: Some(*height) });
            }
        }
    } else {
        let here = |kept: &str| shows(genesis.as_deref(), kept);

        for (tx, checked) in ctx.settings.checked.iter().filter(|(_, checked)| here(&checked.genesis)) {
            let ours = addresses.contains(&checked.sender) || checked.for_account(addresses).next().is_some() || listed.mentions.iter().any(|(listed, _)| listed == tx);

            if ours && filter.passes(&facts(checked, addresses), ctx.now) && seen.insert(tx.clone()) {
                found.push(Entry { tx: tx.clone(), checked: Some(checked), sent: None, height: None });
            }
        }
    }

    found.sort_by_key(|entry| std::cmp::Reverse(entry.order()));

    found
}

/// Below a filtered list: what it came to, why the node's word is only where to look, and asking
/// the node to go on from where it stopped, whether its page was full or it reached its bound.
fn further(ui: &mut Ui, ctx: &mut Ctx, query: &str, listed: &Listed, on: usize, shown: usize) {
    let palette = *ctx.palette;
    let older = !listed.answered.filtered && listed.read;
    let genesis = ctx.settings.chain_shown(ctx.chain.genesis);
    let waiting = listed.mentions.iter().filter(|(tx, _)| ctx.settings.checked_on(tx, genesis.as_deref()).is_none()).count();
    let mut note = filters::said(shown, !listed.next.is_empty(), &listed.answered, on).unwrap_or_default();

    if older && waiting > 0 {
        note.push_str(&format!(" {waiting} still being proven before they can be tested."));
    }

    if !older {
        note.push_str(" The node chooses what it lists: each line is proven before it says so, but a node could leave one out.");
    }

    ui.add(egui::Label::new(RichText::new(note).small().color(palette.muted)).wrap());

    if !listed.next.is_empty() {
        let asks: Vec<Ask> = listed.next.iter().map(|(address, next)| Ask::Filtered { address: address.clone(), query: query.to_string(), cursor: Some(next.clone()) }).collect();

        ui.horizontal(|ui| {
            match asks.iter().any(|ask| ctx.node.asking(ask)) {
                true => {
                    ui.spinner();
                }
                false => {
                    if theme::quiet(ui, &palette, "Look further back").clicked() {
                        for ask in asks {
                            ctx.node.retry(ask);
                        }
                    }
                }
            }
        });
    }

    theme::section_gap(ui);
}

/// A kept call, read by the ABI saved for its contract, or one built in.
fn read_call(ctx: &Ctx, call: &KeptCall) -> Option<std::rc::Rc<chainv4::abi::known::ReadCall>> {
    ctx.reader.call_hex(decoded::kept(&ctx.settings.contracts, &call.to), &call.input)
}

/// A kept log, read by the ABI saved for its contract, or one built in.
fn read_log(ctx: &Ctx, log: &KeptLog) -> Option<std::rc::Rc<chainv4::abi::known::ReadLog>> {
    let topics: Vec<&str> = log.topics.iter().map(String::as_str).collect();

    ctx.reader.log(decoded::kept(&ctx.settings.contracts, &log.contract), &topics, &log.data)
}

/// A listed token's symbol and decimals, for its amounts: only if the registry gives both.
fn units<'a>(ctx: &'a Ctx, token: &str) -> decoded::TokenUnits<'a> {
    let proven = ctx.chain.tokens.get(token)?;

    Some((proven.value.symbol.as_deref()?, proven.value.decimals?))
}

/// Whether what its EVM call and logs read as holds the search, as what is kept of them is bytes.
fn reads_as(ctx: &Ctx, entry: &Entry, search: &str) -> bool {
    let search = search.trim().to_lowercase();
    let Some(checked) = entry.checked else {
        return false;
    };

    checked.call.as_ref().and_then(|call| read_call(ctx, call)).is_some_and(|read| read.shown().to_lowercase().contains(&search))
        || checked.logs.iter().filter_map(|log| read_log(ctx, log)).any(|read| read.shown().to_lowercase().contains(&search))
}

/// What a transaction did, in a few words, by what its call reads as, and which ABI read it.
fn did(ctx: &Ctx, checked: &Checked) -> (String, Option<String>) {
    match checked.call.as_ref().and_then(|call| read_call(ctx, call).map(|read| (call, read))) {
        Some((call, read)) => (format!("Called {} on {}", read.name, format::short(&call.to)), Some(read.by.label())),
        None => (checked.action.clone(), None),
    }
}

/// An amount moved, cut short and in full: an NFT's is which one it was. A claim of an amount is in
/// what its contract says of itself, once it has said, and otherwise a plain whole number.
fn moved_amount(ctx: &Ctx, moved: &Move, said: Said) -> (String, String) {
    let token = moved.token.as_deref();

    if moved.id.is_none() && claimed(ctx, moved) && let Some(named) = token.and_then(|token| said.amount(token, &moved.amount)) {
        return named;
    }

    match (&moved.id, moved.amount.as_str()) {
        (Some(id), "1") => {
            let (symbol, _) = ctx.token(token.unwrap_or_default());

            (format!("{symbol} {}", format::short(id)), format!("token {id} of {symbol}"))
        }
        (Some(id), amount) => {
            let (symbol, _) = ctx.token(token.unwrap_or_default());

            (format!("{amount} {symbol} {}", format::short(id)), format!("{amount} of token {id} of {symbol}"))
        }
        (None, _) => (ctx.brief(token, &moved.amount), ctx.amount(token, &moved.amount)),
    }
}

/// Whether a move is only what a contract's log claims: the registry does not list the contract.
fn claimed(ctx: &Ctx, moved: &Move) -> bool {
    moved.claimed(moved.token.as_ref().is_some_and(|token| ctx.chain.tokens.contains_key(token)))
}

/// The contract to ask about for a move: one whose amount is only a claim, unless the node lists it
/// and only its proof is on its way, which will say all there is to say.
fn to_ask_of<'a>(ctx: &Ctx, moved: &'a Move) -> Option<&'a String> {
    moved.token.as_ref().filter(|token| moved.id.is_none() && claimed(ctx, moved) && !ctx.chain.listed.contains(token))
}

/// One line of the history.
struct Entry<'a> {
    tx: String,
    checked: Option<&'a Checked>,
    sent: Option<&'a Sent>,
    /// Where the node said it landed, before it is proven.
    height: Option<u64>,
}

impl Entry<'_> {
    /// Whether it is of a kind: not proven yet, or failed.
    fn is(&self, kind: Kind) -> bool {
        match (kind, self.checked) {
            (Kind::All, _) => true,
            (Kind::Waiting, checked) => checked.is_none(),
            (Kind::Failed, Some(checked)) => !checked.done,
            (Kind::Failed, None) => false,
        }
    }

    /// Whether what it says holds the search, its case aside: its id, who sent it and to whom,
    /// what it did, what it moved where, and its events.
    fn matches(&self, search: &str) -> bool {
        let search = search.trim().to_lowercase();

        if search.is_empty() {
            return true;
        }

        let mut said = vec![self.tx.clone()];

        if let Some(checked) = self.checked {
            said.extend([checked.sender.clone(), checked.action.clone()]);
            said.extend(checked.moves.iter().flat_map(|moved| [moved.from.clone(), moved.to.clone(), moved.token.clone()]).flatten());
            said.extend(checked.events.iter().cloned());
            said.extend(checked.logs.iter().flat_map(|log| log.topics.iter().cloned().chain([log.contract.clone()])));
            said.extend(checked.call.iter().map(|call| call.to.clone()));
        }

        if let Some(sent) = self.sent {
            said.extend([sent.to.clone(), sent.what.clone()]);
        }

        said.iter().any(|text| text.to_lowercase().contains(&search))
    }

    /// Newest first: what is still waiting, then by height.
    fn order(&self) -> (u64, u64) {
        match (self.checked, self.height, self.sent) {
            (Some(checked), _, _) => (checked.height, 0),
            (None, Some(height), _) => (height, 0),
            (None, None, sent) => (u64::MAX, sent.map_or(0, |sent| sent.at)),
        }
    }
}

/// The account's history, at most this many lines.
pub fn list(ui: &mut Ui, ctx: &Ctx, limit: usize) {
    let palette = *ctx.palette;
    let addresses = with_evm_forms(&ctx.addresses());
    let entries = entries(ctx, &addresses);

    if entries.is_empty() {
        let text = match ctx.chain.has_followed() {
            true => "Nothing yet.",
            false => "Looking once the light client catches up…",
        };
        ui.label(RichText::new(text).color(palette.muted));

        return;
    }

    for (index, entry) in entries.iter().take(limit).enumerate() {
        if index > 0 {
            theme::rule(ui, &palette);
        }

        // These lines sit under the balances, so they say nothing a contract says of itself.
        line(ui, ctx, &addresses, entry, Said::default());
    }
}

/// Whether something kept shows as this chain's. With no chain known yet, only what was kept before
/// chains were said: another chain's history is not this wallet's, whatever is keeping it from one.
fn shows(genesis: Option<&str>, kept: &str) -> bool {
    genesis.map_or(kept.is_empty(), |genesis| kept == genesis)
}

fn entries<'a>(ctx: &'a Ctx, addresses: &[String]) -> Vec<Entry<'a>> {
    let mut seen = HashSet::new();
    let mut entries = Vec::new();
    // The chain followed now, or that this node is pinned to.
    let genesis = ctx.settings.chain_shown(ctx.chain.genesis);
    let mut add = |tx: &str, sent: Option<&'a Sent>, height: Option<u64>| {
        if seen.insert(tx.to_string()) {
            entries.push(Entry {
                tx: tx.to_string(),
                checked: ctx.settings.checked_on(tx, genesis.as_deref()),
                sent,
                height,
            });
        }
    };

    let here = |kept: &str| shows(genesis.as_deref(), kept);

    for sent in ctx.settings.sent.iter().filter(|sent| addresses.contains(&sent.from) && here(&sent.genesis)) {
        add(&sent.id, Some(sent), None);
    }

    for mention in addresses.iter().flat_map(|address| ctx.chain.mentions.get(address).into_iter().flatten()) {
        add(&mention.tx, None, Some(mention.height));
    }

    // What was proven before, whether or not the node still lists it. A move between two of the
    // account's own addresses moved nothing for it, but shows: one of them sent it.
    for (tx, checked) in ctx.settings.checked.iter().filter(|(_, checked)| here(&checked.genesis)) {
        if addresses.contains(&checked.sender) || checked.for_account(addresses).next().is_some() {
            add(tx, None, None);
        }
    }

    entries.sort_by_key(|entry| std::cmp::Reverse(entry.order()));

    entries
}

/// Draws a line, and says which contracts it would name, of those it shows on screen.
fn line(ui: &mut Ui, ctx: &Ctx, addresses: &[String], entry: &Entry, said: Said) -> Vec<String> {
    let palette = *ctx.palette;
    let id = ui.make_persistent_id(("activity", &entry.tx));
    let open = ui.data(|data| data.get_temp::<bool>(id)).unwrap_or(false);
    let (title, detail, amount, incoming) = summary(ctx, addresses, entry, said);
    let (status, color) = status(ctx, entry);
    // The amount and the pill keep their column on the right, on the line's first line, however
    // narrow the screen and however long what it says.
    let response = theme::end_row(
        ui,
        |ui| {
            let mark = match incoming {
                Way::In => palette.good,
                Way::Out => palette.accent,
                Way::Between => palette.soft,
                Way::Unsaid => palette.muted,
            };
            theme::dot(ui, mark);
            ui.vertical(|ui| {
                ui.label(theme::strong(&title, 14.5).color(palette.ink));
                ui.add(egui::Label::new(RichText::new(&detail).small().color(palette.muted)).wrap());
            });
        },
        |ui| {
            ui.with_layout(egui::Layout::top_down(egui::Align::Max), |ui| {
                ui.set_max_width(AMOUNT_WIDTH);

                if let Some((brief, full)) = &amount {
                    let (sign, color) = match incoming {
                        Way::In => ("+", palette.good),
                        Way::Out => ("−", palette.ink),
                        // From one of its addresses to another: neither more nor less in all.
                        Way::Between => ("", palette.ink),
                        // Only claimed: it is not said to have added to anything.
                        Way::Unsaid => ("", palette.soft),
                    };
                    let (text, family) = (format!("{sign}{brief}"), egui::FontFamily::Name(theme::SEMIBOLD.into()));

                    // What is fitted gets cut short past a point, which must never be what says it is
                    // unverified. What a contract says of itself was cut short already, so all of it
                    // shows, on as many lines as it takes.
                    match brief.ends_with(UNVERIFIED) {
                        true => ui.add(egui::Label::new(RichText::new(text).font(egui::FontId::new(NAMED_SIZE, family)).color(color)).wrap()),
                        false => theme::fitted(ui, &text, 14.5, 10.0, family, color),
                    }
                    .on_hover_text(full);
                }

                theme::pill(ui, status, color);
            });
        },
    )
    .response
    .interact(egui::Sense::click());

    if response.clicked() {
        ui.data_mut(|data| data.insert_temp(id, !open));
    }

    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    let moves = || entry.checked.into_iter().flat_map(|checked| checked.for_account(addresses).map(|(_, moved)| moved).chain(checked.between(addresses)));
    let mut wanted: Vec<String> = match ui.is_rect_visible(response.rect) {
        true => moves().take(1).filter_map(|moved| to_ask_of(ctx, moved)).cloned().collect(),
        false => Vec::new(),
    };

    if open {
        let shown = ui.indent(id, |ui| details(ui, ctx, addresses, entry, said)).response.rect;

        if ui.is_rect_visible(shown) {
            wanted.extend(moves().filter_map(|moved| to_ask_of(ctx, moved)).cloned());
        }
    }

    wanted
}

/// Which way a line's amount went for the account.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Way {
    In,
    Out,
    /// From one of its addresses to another.
    Between,
    /// Neither said: only a contract's claim, or no amount at all.
    Unsaid,
}

impl Way {
    fn of(incoming: Option<bool>) -> Way {
        match incoming {
            Some(true) => Way::In,
            Some(false) => Way::Out,
            None => Way::Unsaid,
        }
    }
}

/// Which of the account's addresses a party is, by its scheme: "your Bitcoin". An EVM alias of one
/// is that address's. None for anyone else's.
fn yours(ctx: &Ctx, party: &str) -> Option<String> {
    let mine = ctx.holder()?.addresses();
    let found = mine.iter().find(|address| address.to_string() == party || chainv4::structs::hex::to_hex(&chainv4::abi::evm_address(address)) == party)?;

    Some(format!("your {}", super::scheme(found)))
}

/// A party in a line: which of the account's addresses it is, or the address, short.
fn party(ctx: &Ctx, address: &str) -> String {
    yours(ctx, address).unwrap_or_else(|| format::short(address))
}

/// A line's title, what it says under it, the amount it moved for this account, cut short and in
/// full, and which way that went.
fn summary(ctx: &Ctx, addresses: &[String], entry: &Entry, said: Said) -> (String, String, Option<(String, String)>, Way) {
    let when = |checked: &Checked| match checked.time {
        0 => format!("block {}", format::thousands(&checked.height.to_string())),
        time => format::ago(time, ctx.now),
    };

    match (entry.checked, entry.sent) {
        // Its steps, each with its own amount and party, show when it is opened.
        (Some(checked), _) if !checked.steps.is_empty() => (checked.action.clone(), format!("{}{}", by_whom(ctx, checked), when(checked)), None, Way::Unsaid),
        (Some(checked), _) => match (checked.for_account(addresses).next(), checked.between(addresses).next()) {
            (Some((true, moved)), _) => {
                let from = moved.from.as_deref().map_or_else(|| "minted".to_string(), |from| format!("from {}", format::short(from)));
                let at = moved.to.as_deref().and_then(|to| yours(ctx, to)).map_or_else(String::new, |at| format!(" · to {at}"));
                let (title, sign) = moved_title(ctx, moved, "Received", "Says you received", true);

                (title, format!("{from}{at} · {}{}", when(checked), read_as(moved)), Some(moved_amount(ctx, moved, said)), Way::of(sign))
            }
            (Some((false, moved)), _) => {
                let to = moved.to.as_deref().map_or_else(|| "burned".to_string(), |to| format!("to {}", format::short(to)));
                let at = moved.from.as_deref().and_then(|from| yours(ctx, from)).map_or_else(String::new, |at| format!(" · from {at}"));
                let (title, sign) = moved_title(ctx, moved, "Sent", "Says you sent", false);

                (title, format!("{to}{at} · {}{}", when(checked), read_as(moved)), Some(moved_amount(ctx, moved, said)), Way::of(sign))
            }
            // Between the account's own addresses: what moved and where, not a bare transfer.
            (None, Some(moved)) => {
                let (from, to) = (party(ctx, moved.from.as_deref().unwrap_or_default()), party(ctx, moved.to.as_deref().unwrap_or_default()));
                let (title, sign) = moved_title(ctx, moved, "Between your addresses", "Says it moved between your addresses", false);
                let way = if sign.is_some() { Way::Between } else { Way::Unsaid };

                (title, format!("from {from} to {to} · {}{}", when(checked), read_as(moved)), Some(moved_amount(ctx, moved, said)), way)
            }
            (None, None) => {
                let (title, read_by) = did(ctx, checked);
                let read_by = read_by.map_or_else(String::new, |read_by| format!(" · {read_by}"));

                (title, format!("{}{}{read_by}", by_whom(ctx, checked), when(checked)), None, Way::Unsaid)
            }
        },
        (None, Some(sent)) => (
            "Sending".to_string(),
            format!("to {} · from {} · {}", format::short(&sent.to), party(ctx, &sent.from), format::ago(sent.at, ctx.now)),
            Some((sent.what.clone(), sent.what.clone())),
            Way::Out,
        ),
        (None, None) => (
            "Checking…".to_string(),
            entry.height.map_or_else(String::new, |height| format!("block {}", format::thousands(&height.to_string()))),
            None,
            Way::Unsaid,
        ),
    }
}

/// Who sent a line's transaction, before the rest of what it says: which of the account's addresses,
/// or another's address, short.
fn by_whom(ctx: &Ctx, checked: &Checked) -> String {
    format!("by {} · ", party(ctx, &checked.sender))
}

/// What a line says of a move, and which way it went: a claim's says neither that it happened nor
/// that it added to anything.
fn moved_title(ctx: &Ctx, moved: &Move, real: &str, claim: &str, incoming: bool) -> (String, Option<bool>) {
    match claimed(ctx, moved) {
        true => (claim.to_string(), None),
        false => (real.to_string(), Some(incoming)),
    }
}

/// Which standard's shape a move's log fit, after a separator: nothing for a native one.
fn read_as(moved: &Move) -> String {
    moved.standard.as_deref().map_or_else(String::new, |standard| format!(" · read as {standard}"))
}

fn status(ctx: &Ctx, entry: &Entry) -> (&'static str, egui::Color32) {
    let palette = ctx.palette;

    match (entry.checked, entry.sent) {
        (Some(checked), _) if !checked.done => ("Failed", palette.bad),
        (Some(checked), _) if checked.final_ => ("Final", palette.good),
        (Some(checked), _) if checked.came == Origin::Looked => ("Included, not final", palette.accent),
        (Some(_), _) => ("Proven", palette.accent),
        (None, _) if ctx.chain.refused.contains_key(&entry.tx) => ("Does not check", palette.bad),
        (None, Some(sent)) if ctx.now.saturating_sub(sent.at) > UNSEEN_AFTER => ("Not seen", palette.warn),
        (None, Some(_)) => ("Pending", palette.warn),
        (None, None) => ("Checking", palette.muted),
    }
}

fn details(ui: &mut Ui, ctx: &Ctx, addresses: &[String], entry: &Entry, said: Said) {
    let palette = *ctx.palette;
    ui.add_space(4.0);
    super::row(ui, &palette, "Transaction", |ui| super::address(ui, &palette, &entry.tx));

    if let Some(why) = ctx.chain.refused.get(&entry.tx) {
        super::row(ui, &palette, "Proof", |ui| {
            ui.colored_label(palette.bad, why);
        });
    }

    if let Some(checked) = entry.checked {
        super::row(ui, &palette, "Block", |ui| {
            let when = match checked.time {
                0 => String::new(),
                time => format!(" · {} UTC", format::utc(time)),
            };
            ui.label(format!("{}{when}", format::thousands(&checked.height.to_string())));
        });
        super::row(ui, &palette, "Sender", |ui| super::address(ui, &palette, &checked.sender));
        super::row(ui, &palette, "Did", |ui| {
            ui.label(&checked.action);
        });

        // What its calldata reads as, under what is sure of it, and its bytes.
        if let Some(call) = &checked.call {
            if let Some(read) = read_call(ctx, call) {
                decoded::call(ui, &palette, units(ctx, &call.to), &read);
            }

            decoded::raw(ui, &palette, "call", &call.input);
        }

        super::steps::ran(ui, ctx, checked);

        for (incoming, moved) in checked.for_account(addresses) {
            let other = match incoming {
                true => moved.from.as_deref().map_or_else(|| "minted".to_string(), |from| format!("from {}", format::short(from))),
                false => moved.to.as_deref().map_or_else(|| "burned".to_string(), |to| format!("to {}", format::short(to))),
            };
            let (title, _) = moved_title(ctx, moved, if incoming { "Received" } else { "Sent" }, if incoming { "Says you received" } else { "Says you sent" }, incoming);
            let (brief, full) = moved_amount(ctx, moved, said);
            super::row(ui, &palette, &title, |ui| {
                ui.label(format!("{full} {other}{}", read_as(moved)));
            });

            if claimed(ctx, moved) {
                ui.label(RichText::new("Only what a contract's log claims: the registry does not list that contract, and no balance counts it.").small().color(palette.muted));
            }

            if brief.ends_with(UNVERIFIED) {
                ui.label(RichText::new("Its name and decimals are only what the contract says of itself, through the node. Nothing proves them, and any contract can say any name.").small().color(palette.muted));
            }
        }

        for moved in checked.between(addresses) {
            let (_, full) = moved_amount(ctx, moved, said);
            let (from, to) = (party(ctx, moved.from.as_deref().unwrap_or_default()), party(ctx, moved.to.as_deref().unwrap_or_default()));
            super::row(ui, &palette, "Moved", |ui| {
                ui.label(format!("{full} from {from} to {to}{}", read_as(moved)));
            });
        }

        if addresses.contains(&checked.sender) {
            super::row(ui, &palette, "Fee", |ui| {
                ui.label(format!("{} · nonce {}", ctx.amount(None, &checked.fee), checked.nonce));
            });
        }

        for (position, event) in checked.events.iter().enumerate() {
            let log = checked.logs.iter().find(|log| log.position as usize == position);

            match log.and_then(|log| read_log(ctx, log).map(|read| (log, read))) {
                Some((log, read)) => decoded::log(ui, &palette, units(ctx, &log.contract), &read),
                None => super::row(ui, &palette, "Event", |ui| {
                    ui.label(theme::mono(event).color(palette.soft));
                }),
            }

            if let Some(log) = log {
                decoded::raw(ui, &palette, &format!("log {position}"), &[log.topics.join("\n"), log.data.clone()].join("\n"));
            }
        }
    } else if let Some(sent) = entry.sent {
        super::row(ui, &palette, "Nonce", |ui| {
            ui.label(sent.nonce.to_string());
        });
        let note = match ctx.now.saturating_sub(sent.at) > UNSEEN_AFTER {
            true => "No block holds it yet. The node may have dropped it: once a later transaction from this account lands, its nonce is free again.",
            false => "Waiting for a block to hold it, and for the light client to prove that.",
        };
        ui.label(RichText::new(note).small().color(palette.muted));
    }

    ui.add_space(4.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checked(sender: &str, to: &str, done: bool) -> Checked {
        Checked {
            height: 7,
            time: 0,
            final_: true,
            done,
            sender: sender.to_string(),
            nonce: 0,
            action: "Called swap on contract:0x6c1d".to_string(),
            kinds: vec!["contract_call".to_string()],
            moves: vec![Move { token: None, from: Some(sender.to_string()), to: Some(to.to_string()), amount: "40".to_string(), id: None, standard: None }],
            fee: "0".to_string(),
            events: vec!["Swap contract:0x0000 contract:0x5119".to_string()],
            genesis: "0x9a7f".to_string(),
            vote: None,
            call: None,
            logs: Vec::new(),
            steps: Vec::new(),
            came: Default::default(),
        }
    }

    /// Runs something with a page's context, on a chain that holds these tokens, drawing it headless:
    /// what it returned, and the text it drew.
    fn with_ctx<R>(chain: &crate::chain::Chain, contracts: Vec<crate::app::SavedContract>, draw: impl FnOnce(&mut Ui, &mut Ctx) -> R) -> (R, String) {
        with_ctx_on((1_200.0, 4_000.0), chain, contracts, draw)
    }

    /// As with_ctx, on a screen of this size.
    fn with_ctx_on<R>(size: (f32, f32), chain: &crate::chain::Chain, contracts: Vec<crate::app::SavedContract>, draw: impl FnOnce(&mut Ui, &mut Ctx) -> R) -> (R, String) {
        let (returned, galleys) = drawing(size, chain, contracts, draw);

        (returned, galleys.iter().map(|galley| format!("{}\n", galley.text())).collect())
    }

    /// As with_ctx, with what it drew as it was laid out.
    fn drawing<R>(size: (f32, f32), chain: &crate::chain::Chain, contracts: Vec<crate::app::SavedContract>, draw: impl FnOnce(&mut Ui, &mut Ctx) -> R) -> (R, Vec<std::sync::Arc<egui::Galley>>) {
        let wake = egui::Context::default();
        theme::install_fonts(&wake);
        let reader = chainv4::abi::known::AbiReader::default();
        let mut node = crate::node::Node::new("http://127.0.0.1:1", Err("offline".to_string()), wake.clone());
        let (mut holders, mut book, mut settings, mut page) = (Vec::new(), crate::vault::Book::default(), crate::app::Settings::default(), super::super::Page::Activity);
        let (mut connect, mut changed, mut vault, mut leave, mut lock) = (None, false, None, false, None);
        let coin = crate::app::Coin { symbol: "DEV".to_string(), decimals: 18 };
        #[cfg(all(feature = "bitcoin-light", not(target_arch = "wasm32")))]
        let mut lights = crate::light::Lights::default();

        settings.contracts = contracts;

        let palette = theme::DARK;
        let mut reading = crate::reading::Reading::default();
        let mut ctx = Ctx {
            reading: &mut reading,
            palette: &palette,
            chain,
            reader: &reader,
            node: &mut node,
            holders: &mut holders,
            current: 0,
            book: &mut book,
            coin: &coin,
            settings: &mut settings,
            now: 2_000,
            page: &mut page,
            connect: &mut connect,
            changed: &mut changed,
            vault: &mut vault,
            leave: &mut leave,
            lock: &mut lock,
            wake: &wake,
            faucet: None,
            swap_contract: None,
            dex: (None, None),
            node_stops: None,
            #[cfg(all(feature = "bitcoin-light", not(target_arch = "wasm32")))]
            lights: &mut lights,
            #[cfg(all(feature = "walletconnect", not(target_arch = "wasm32")))]
            walletconnect: None,
            dex_fee_bps: chainv4::params::vm::DexSettings::DEFAULT.fee_bps,
        };
        let mut returned = None;
        let mut draw = Some(draw);
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(size.0, size.1))),
            ..egui::RawInput::default()
        };
        let mut output = wake.run_ui(input, |ui| {
            returned = draw.take().map(|draw| draw(ui, &mut ctx));
        });
        output.textures_delta.clear();

        let galleys = output.shapes.iter().filter_map(|clipped| match &clipped.shape {
            egui::epaint::Shape::Text(shape) => Some(shape.galley.clone()),
            _ => None,
        });

        (returned.expect("drawn once"), galleys.collect())
    }

    const TOKEN: &str = "evm:0x0505050505050505050505050505050505050505";

    /// A transaction whose log says a token moved from them to me, or how an NFT did.
    fn token_moved(me: &str, id: Option<&str>, amount: &str, standard: &str) -> Checked {
        Checked {
            moves: vec![Move {
                token: Some(TOKEN.to_string()),
                from: Some("0xbb".to_string()),
                to: Some(me.to_string()),
                amount: amount.to_string(),
                id: id.map(str::to_string),
                standard: Some(standard.to_string()),
            }],
            ..checked("ed25519:0xbb", "ed25519:0xcc", true)
        }
    }

    fn entry(checked: &Checked) -> Entry<'_> {
        Entry { tx: "0x01ab".to_string(), checked: Some(checked), sent: None, height: Some(7) }
    }

    fn listed_token() -> crate::chain::Proven<chainv4::registry::tokens::Token> {
        crate::chain::Proven {
            value: chainv4::registry::tokens::Token { address: TOKEN.parse().unwrap(), name: None, symbol: Some("TKN".to_string()), decimals: Some(2) },
            assurance: chainv4::light::Assurance { block: chainv4::structs::hash::Hash::zero(chainv4::structs::hash::HashType::default_chain()), height: 1, final_: true },
        }
    }

    #[test]
    fn an_evm_transfer_from_a_contract_nothing_lists_is_only_shown_as_what_it_claims() {
        let me = vec!["0xaa".to_string()];
        let moved = token_moved("0xaa", None, "1500", "ERC-20");
        let ((title, detail, amount, incoming), _) = with_ctx(&crate::chain::Chain::default(), Vec::new(), |_, ctx| summary(ctx, &me, &entry(&moved), Said::default()));

        assert_eq!(title, "Says you received");
        assert!(detail.contains("read as ERC-20"), "{detail}");
        assert_eq!(incoming, Way::Unsaid, "so it gets no plus, and nothing says it came in");
        assert!(amount.unwrap().1.starts_with("1,500 "), "a plain whole number, in no decimals");

        let (_, text) = with_ctx(&crate::chain::Chain::default(), Vec::new(), |ui, ctx| details(ui, ctx, &me, &entry(&moved), Said::default()));

        assert!(text.contains("no balance counts it"), "{text}");
    }

    #[test]
    fn an_evm_transfer_of_a_listed_token_counts_as_any_token_s_does() {
        let me = vec!["0xaa".to_string()];
        let moved = token_moved("0xaa", None, "1500", "ERC-20");
        let mut chain = crate::chain::Chain::default();
        chain.tokens.insert(TOKEN.to_string(), listed_token());
        let ((title, detail, amount, incoming), _) = with_ctx(&chain, Vec::new(), |_, ctx| summary(ctx, &me, &entry(&moved), Said::default()));

        assert_eq!((title.as_str(), incoming), ("Received", Way::In));
        assert!(detail.contains("read as ERC-20"), "still says what read it: {detail}");
        assert_eq!(amount.unwrap().1, "15 TKN", "in its decimals");
    }

    #[test]
    fn an_nft_moving_says_which_token_rather_than_an_amount() {
        let me = vec!["0xaa".to_string()];
        let nft = token_moved("0xaa", Some("9"), "1", "ERC-721");
        let ((_, _, amount, _), _) = with_ctx(&crate::chain::Chain::default(), Vec::new(), |_, ctx| summary(ctx, &me, &entry(&nft), Said::default()));

        assert!(amount.unwrap().1.starts_with("token 9 of "));

        let multi = token_moved("0xaa", Some("4"), "50", "ERC-1155");
        let ((_, _, amount, _), _) = with_ctx(&crate::chain::Chain::default(), Vec::new(), |_, ctx| summary(ctx, &me, &entry(&multi), Said::default()));

        assert!(amount.unwrap().1.starts_with("50 of token 4 of "));
    }

    #[test]
    fn a_call_and_a_log_read_by_name_say_which_abi_read_them_and_keep_their_bytes() {
        use chainv4::structs::hex::to_hex;

        let me = vec!["ed25519:0xaa".to_string()];
        let (selector, types) = chainv4::abi::parse_signature("transfer(address,uint256)").unwrap();
        let args = chainv4::abi::encode_values(&types, &[chainv4::abi::AbiValue::Address([0xab; 20]), chainv4::abi::AbiValue::Uint(chainv4::structs::u256::u256::new(5))]).unwrap();
        let input = to_hex(&[selector.unwrap().to_vec(), args].concat());
        let topic = |address: u8| to_hex(&[[0u8; 12].as_slice(), &[address; 20]].concat());
        let transfer = to_hex(&chainv4::abi::signature_topic("Transfer(address,address,uint256)").unwrap());
        let called = Checked {
            moves: Vec::new(),
            call: Some(KeptCall { to: TOKEN.to_string(), input: input.clone() }),
            events: vec!["log 0xddf2…b3ef · 3 topics, 32 bytes".to_string()],
            logs: vec![KeptLog {
                position: 0,
                contract: TOKEN.to_string(),
                topics: vec![transfer, topic(1), topic(2)],
                data: to_hex(&chainv4::structs::u256::u256::new(7).to_be_bytes()),
            }],
            ..checked("ed25519:0xaa", "ed25519:0xcc", true)
        };
        let ((title, detail, _, _), _) = with_ctx(&crate::chain::Chain::default(), Vec::new(), |_, ctx| summary(ctx, &me, &entry(&called), Said::default()));

        assert!(title.starts_with("Called transfer on evm:0x0505"), "{title}");
        assert!(detail.ends_with("read as ERC-20"), "{detail}");

        let (_, text) = with_ctx(&crate::chain::Chain::default(), Vec::new(), |ui, ctx| details(ui, ctx, &me, &entry(&called), Said::default()));

        for said in ["Did", "transfer", "value =", "Transfer", "read as ERC-20", "Raw data"] {
            assert!(text.contains(said), "{said} in {text}");
        }

        // An ABI saved for the contract goes first, and says so differently.
        let saved = crate::app::SavedContract {
            address: TOKEN.to_string(),
            name: "mine".to_string(),
            abi: r#"[{"type": "function", "name": "transfer", "inputs": [{"name": "payee", "type": "address"}, {"name": "cents", "type": "uint256"}]}]"#.to_string(),
        };
        let ((_, detail, _, _), _) = with_ctx(&crate::chain::Chain::default(), vec![saved], |_, ctx| summary(ctx, &me, &entry(&called), Said::default()));

        assert!(detail.ends_with("read by the ABI kept for this contract"), "{detail}");

        // The search finds what it reads as, though only its bytes are kept.
        let ((found, missed), _) = with_ctx(&crate::chain::Chain::default(), Vec::new(), |_, ctx| (reads_as(ctx, &entry(&called), "Transfer(from"), reads_as(ctx, &entry(&called), "swapExact")));

        assert!(found && !missed);
    }

    #[test]
    fn lines_are_told_apart_by_kind_and_found_by_what_they_say() {
        let (out, back, failed) = (checked("ed25519:0xaa", "contract:0x6c1d", true), checked("ed25519:0xbb", "ed25519:0xaa", true), checked("ed25519:0xaa", "ed25519:0xcc", false));
        fn line(checked: &Checked) -> Entry<'_> {
            Entry { tx: "0x01ab".to_string(), checked: Some(checked), sent: None, height: Some(7) }
        }

        let waiting = Entry { tx: "0x02cd".to_string(), checked: None, sent: None, height: None };

        assert!(line(&failed).is(Kind::Failed) && !line(&out).is(Kind::Failed));
        assert!(waiting.is(Kind::Waiting) && !line(&out).is(Kind::Waiting));
        assert!(line(&out).is(Kind::All) && line(&back).is(Kind::All));

        // By its id, a party, what it did, or an event, its case aside.
        for search in ["01AB", "0x6c1d", "called swap", "5119", ""] {
            assert!(line(&out).matches(search), "{search}");
        }

        assert!(!line(&out).matches("0xdead"));
    }

    #[test]
    fn what_was_proven_is_tested_against_the_filters_by_what_it_says() {
        use filters::{Direction, When};

        let me = vec!["ed25519:0xaa".to_string()];
        let (out, back) = (checked("ed25519:0xaa", "ed25519:0xbb", true), checked("ed25519:0xbb", "ed25519:0xaa", true));
        let passes = |checked: &Checked, filter: TxFilter| filter.passes(&facts(checked, &me), 0);

        assert!(passes(&out, TxFilter { direction: Some(Direction::Out), ..TxFilter::default() }));
        assert!(!passes(&back, TxFilter { direction: Some(Direction::Out), ..TxFilter::default() }));
        assert!(passes(&back, TxFilter { direction: Some(Direction::In), ..TxFilter::default() }));
        assert!(!passes(&out, TxFilter { direction: Some(Direction::In), ..TxFilter::default() }));
        assert!(passes(&out, TxFilter { kind: Some("contract_call".to_string()), ..TxFilter::default() }));
        assert!(!passes(&out, TxFilter { kind: Some("transfer".to_string()), ..TxFilter::default() }));
        assert!(passes(&out, TxFilter { counterparty: "ed25519:0xbb".to_string(), ..TxFilter::default() }));
        assert!(!passes(&out, TxFilter { counterparty: "ed25519:0xcc".to_string(), ..TxFilter::default() }));
        assert!(passes(&out, TxFilter { token: Some(COIN.to_string()), ..TxFilter::default() }));
        assert!(!passes(&out, TxFilter { token: Some("contract:0x01".to_string()), ..TxFilter::default() }));
        assert!(passes(&out, TxFilter { when: When::Day, ..TxFilter::default() }), "a time not known passes");

        // Kept from before it said its kind: it passes, as nothing says otherwise.
        let older = Checked { kinds: Vec::new(), ..out };

        assert!(passes(&older, TxFilter { kind: Some("transfer".to_string()), ..TxFilter::default() }));
    }

    #[test]
    fn the_nodes_answers_are_put_together_over_an_accounts_addresses() {
        use crate::chain::{Chain, FilteredTxs, Mention};

        let (first, second) = ("ed25519:0xaa".to_string(), "secp256k1:0xbb".to_string());
        let addresses = [first.clone(), second.clone()];
        let mention = |tx: &str, height: u64| Mention { tx: tx.to_string(), height };
        let mut chain = Chain::default();
        let nothing = listed(&chain, &addresses, "kind=transfer");

        assert!(!nothing.read, "nothing yet");

        chain.filtered.insert((first.clone(), "kind=transfer".to_string()), FilteredTxs {
            mentions: vec![mention("0x01", 9)],
            next: Some("0xff".to_string()),
            answered: Answered { filtered: true, scanned: 1_000, limited: true },
            pages: 1,
        });

        assert!(!listed(&chain, &addresses, "kind=transfer").read, "one address of two");

        chain.filtered.insert((second.clone(), "kind=transfer".to_string()), FilteredTxs {
            mentions: vec![mention("0x02", 7)],
            next: None,
            answered: Answered { filtered: true, scanned: 4, limited: false },
            pages: 1,
        });
        let both = listed(&chain, &addresses, "kind=transfer");

        assert!(both.read);
        assert_eq!(both.mentions, [("0x01".to_string(), 9), ("0x02".to_string(), 7)]);
        assert_eq!(both.next, [(first.clone(), "0xff".to_string())]);
        assert_eq!(both.answered, Answered { filtered: true, scanned: 1_004, limited: true });
        assert!(!listed(&chain, &addresses, "kind=deploy").read, "another filter's answers are not these");

        // One of them an answer from an older node: the list is not filtered.
        chain.filtered.get_mut(&(second, "kind=transfer".to_string())).unwrap().answered.filtered = false;

        assert!(!listed(&chain, &addresses, "kind=transfer").answered.filtered);
    }

    /// What a contract answered, taken as the page takes it.
    fn answered(symbol: &str, decimals: u64) -> Unlisted {
        let mut unlisted = Unlisted::default();
        unlisted::tests::answering(&mut unlisted, unlisted::tests::GENESIS, TOKEN, &unlisted::tests::says_symbol(symbol), &unlisted::tests::says_number(decimals), 0);

        unlisted
    }

    /// The account the tests' lines are of, and the form an EVM move names it by.
    fn watching() -> (crate::keys::Holder, String) {
        let key = chainv4::crypto::keypair::Keypair::from_seed([7; 32]).public();
        let holder = crate::keys::Holder::watched("Watched".to_string(), vec![key], None);
        let evm = with_evm_forms(&[holder.addresses()[0].to_string()]).pop().unwrap();

        (holder, evm)
    }

    /// What Activity lists for this address, with one line of each chain kept under one id each and the
    /// same id mentioned by the node followed.
    fn listed_with_old_rows(followed: bool, pinned: Option<&str>) -> Vec<(String, Option<u64>)> {
        let (mut chain, genesis) = following();
        let (me, other) = ("ed25519:me", "ed25519:other");
        chain.mentions.insert(me.to_string(), vec![crate::chain::Mention { tx: "0xshared".to_string(), height: 7 }]);

        if !followed {
            chain.genesis = None;
            chain.mentions.clear();
        }

        let row = |height, genesis: &str| Checked { height, final_: true, genesis: genesis.to_string(), ..checked(me, other, true) };

        with_ctx(&chain, Vec::new(), |_, ctx| {
            ctx.settings.checked.insert("0xshared".to_string(), row(9000, "0xoldchain"));
            ctx.settings.checked.insert("0xoldonly".to_string(), row(9001, "0xoldchain"));
            ctx.settings.checked.insert("0xnewonly".to_string(), row(3, &genesis));

            if let Some(pinned) = pinned {
                ctx.settings.pinned.insert(ctx.settings.node.clone(), pinned.to_string());
            }

            let mut listed: Vec<_> = entries(ctx, &[me.to_string()]).iter().map(|entry| (entry.tx.clone(), entry.checked.map(|checked| checked.height))).collect();
            listed.sort();

            listed
        })
        .0
    }

    #[test]
    fn a_row_from_another_chain_does_not_stand_in_for_the_same_id_here_nor_show_by_itself() {
        // The same id on the chain followed has nothing proven here yet, rather than the old chain's
        // height and time. What was only on the old chain does not show, and this one's does.
        assert_eq!(listed_with_old_rows(true, None), [("0xnewonly".to_string(), Some(3)), ("0xshared".to_string(), None)]);
    }

    #[test]
    fn with_no_chain_followed_it_shows_only_what_the_node_is_pinned_to_and_otherwise_nothing_of_any_chain() {
        // Refusing another chain, or before an answer: what it trusts here, and not every chain kept.
        let old = listed_with_old_rows(false, Some("0xoldchain"));
        assert_eq!(old, [("0xoldonly".to_string(), Some(9001)), ("0xshared".to_string(), Some(9000))]);

        // Its pin forgotten, or a node never reached.
        assert_eq!(listed_with_old_rows(false, None), []);
    }

    #[test]
    fn a_row_kept_before_chains_were_said_still_goes_with_this_one() {
        assert!(Checked { genesis: String::new(), ..checked("a", "b", true) }.on(Some("0xnew")));
        assert!(!Checked { genesis: "0xold".to_string(), ..checked("a", "b", true) }.on(Some("0xnew")));
        assert!(!Checked { genesis: "0xold".to_string(), ..checked("a", "b", true) }.on(None));
        assert!(shows(None, ""), "before any chain, what was kept before chains were said");
        assert!(!shows(Some("0xnew"), ""), "listed by itself only once a node mentions it");
    }

    /// A chain followed from this genesis, and the same to keep a line under.
    fn following() -> (crate::chain::Chain, String) {
        let genesis = chainv4::structs::hash::Hash::zero(chainv4::structs::hash::HashType::default_chain());
        let mut chain = crate::chain::Chain::default();
        chain.genesis = Some(genesis);

        (chain, genesis.to_string())
    }

    #[test]
    fn a_claim_reads_in_what_its_contract_says_labelled_and_is_still_only_a_claim() {
        let me = vec!["0xaa".to_string()];
        let moved = token_moved("0xaa", None, "1500000", "ERC-20");
        let names = answered("USDC", 6);
        let said = Said::on(&names, unlisted::tests::GENESIS);
        let chain = crate::chain::Chain::default();
        let ((title, _, amount, incoming), text) = with_ctx(&chain, Vec::new(), |ui, ctx| {
            line(ui, ctx, &me, &entry(&moved), said);

            summary(ctx, &me, &entry(&moved), said)
        });

        assert_eq!(amount.unwrap(), ("1.5 \"USDC\" (unverified)".to_string(), "1.5 \"USDC\" (unverified)".to_string()));
        assert_eq!((title.as_str(), incoming), ("Says you received", Way::Unsaid), "named or not, it does not say it came in");
        assert!(text.contains("1.5 \"USDC\" (unverified)\n"), "drawn whole: {text}");
        assert!(!text.contains('+'), "and with no plus: {text}");

        let (_, text) = with_ctx(&chain, Vec::new(), |ui, ctx| details(ui, ctx, &me, &entry(&moved), said));

        assert!(text.contains("1.5 \"USDC\" (unverified) from 0xbb"), "{text}");
        assert!(text.contains("no balance counts it") && text.contains("what the contract says of itself"), "{text}");
    }

    #[test]
    fn a_contract_that_calls_itself_the_coin_or_a_listed_token_gets_no_look_of_theirs() {
        let me = vec!["0xaa".to_string()];
        let moved = token_moved("0xaa", None, "1500", "ERC-20");

        // DEV is the chain's coin here, and TKN a listed token's symbol, on another contract.
        for symbol in ["DEV", "TKN"] {
            let names = answered(symbol, 2);
            let mut chain = crate::chain::Chain::default();
            chain.tokens.insert("evm:0x0606060606060606060606060606060606060606".to_string(), listed_token());
            let ((title, _, amount, incoming), _) = with_ctx(&chain, Vec::new(), |_, ctx| summary(ctx, &me, &entry(&moved), Said::on(&names, unlisted::tests::GENESIS)));

            assert_eq!(amount.unwrap().0, format!("15 \"{symbol}\" (unverified)"));
            assert_eq!((title.as_str(), incoming), ("Says you received", Way::Unsaid));
        }
    }

    #[test]
    fn a_listed_token_keeps_its_proven_name_whatever_was_heard_before_it_was_listed() {
        let me = vec!["0xaa".to_string()];
        let moved = token_moved("0xaa", None, "1500", "ERC-20");
        let names = answered("FAKE", 6);
        let said = Said::on(&names, unlisted::tests::GENESIS);
        let mut chain = crate::chain::Chain::default();
        chain.tokens.insert(TOKEN.to_string(), listed_token());
        let ((title, _, amount, incoming), _) = with_ctx(&chain, Vec::new(), |_, ctx| summary(ctx, &me, &entry(&moved), said));

        assert_eq!(amount.unwrap(), ("15 TKN".to_string(), "15 TKN".to_string()));
        assert_eq!((title.as_str(), incoming), ("Received", Way::In));

        let (wanted, text) = with_ctx(&chain, Vec::new(), |ui, ctx| line(ui, ctx, &me, &entry(&moved), said));

        assert!(wanted.is_empty(), "nothing to ask of a listed token");
        assert!(!text.contains("unverified") && !text.contains("FAKE"), "{text}");
    }

    #[test]
    fn until_a_contract_has_said_both_and_when_it_never_will_the_plain_whole_number_stays() {
        let me = vec!["0xaa".to_string()];
        let moved = token_moved("0xaa", None, "1500", "ERC-20");
        let chain = crate::chain::Chain::default();
        let (plain, _) = with_ctx(&chain, Vec::new(), |_, ctx| moved_amount(ctx, &moved.moves[0], Said::default()));

        assert!(plain.1.starts_with("1,500 evm:0x"), "{}", plain.1);

        for names in [Unlisted::default(), answered("USDC", 77), answered("", 6)] {
            let said = Said::on(&names, unlisted::tests::GENESIS);
            let (shown, text) = with_ctx(&chain, Vec::new(), |ui, ctx| {
                line(ui, ctx, &me, &entry(&moved), said);

                moved_amount(ctx, &moved.moves[0], said)
            });

            assert_eq!(shown, plain);
            assert!(!text.contains("unverified"), "{text}");
        }
    }

    #[test]
    fn only_a_claim_of_an_amount_is_asked_about() {
        let me = vec!["0xaa".to_string()];
        let chain = crate::chain::Chain::default();
        let names = answered("BAYC", 0);
        let wanted = |checked: &Checked| with_ctx(&chain, Vec::new(), |ui, ctx| line(ui, ctx, &me, &entry(checked), Said::on(&names, unlisted::tests::GENESIS)));

        assert_eq!(wanted(&token_moved("0xaa", None, "1500", "ERC-20")).0, [TOKEN], "an ERC-20's");

        // An NFT has an id rather than an amount, and stays as it was.
        let (asked, text) = wanted(&token_moved("0xaa", Some("9"), "1", "ERC-721"));

        assert!(asked.is_empty());
        assert!(!text.contains("unverified") && text.contains("evm:0x050505…0505"), "{text}");
        assert!(wanted(&token_moved("0xaa", Some("4"), "50", "ERC-1155")).0.is_empty());

        // A native token's move is no log's claim, and its contract answers no eth_call.
        let native = Checked {
            moves: vec![Move { token: Some("contract:0x05".to_string()), from: Some("0xbb".to_string()), to: Some("0xaa".to_string()), amount: "5".to_string(), id: None, standard: None }],
            ..checked("ed25519:0xbb", "ed25519:0xcc", true)
        };

        assert!(wanted(&native).0.is_empty());
        assert!(wanted(&checked("ed25519:0xbb", "0xaa", true)).0.is_empty(), "nor the coin");

        // The node lists it and its proof is on the way: that will say all there is to say.
        let mut listing = crate::chain::Chain::default();
        listing.listed.push(TOKEN.to_string());
        let moved = token_moved("0xaa", None, "1500", "ERC-20");
        let (asked, _) = with_ctx(&listing, Vec::new(), |ui, ctx| line(ui, ctx, &me, &entry(&moved), Said::default()));

        assert!(asked.is_empty());
    }

    #[test]
    fn only_lines_on_screen_are_asked_about() {
        let me = vec!["0xaa".to_string()];
        let chain = crate::chain::Chain::default();
        let all: Vec<Checked> = (0..200).map(|_| token_moved("0xaa", None, "1500", "ERC-20")).collect();
        let lines = |ui: &mut Ui, ctx: &Ctx| all.iter().enumerate().flat_map(|(at, checked)| line(ui, ctx, &me, &Entry { tx: format!("0x{at}"), ..entry(checked) }, Said::default())).collect::<Vec<_>>();
        let (wanted, _) = with_ctx_on((360.0, 640.0), &chain, Vec::new(), |ui, ctx| lines(ui, ctx));

        assert!(!wanted.is_empty() && wanted.len() < 40, "as many as fit on a phone: {}", wanted.len());

        // And in a list scrolled past those at the top.
        let (scrolled, _) = with_ctx_on((360.0, 640.0), &chain, Vec::new(), |ui, ctx| egui::ScrollArea::vertical().vertical_scroll_offset(4_000.0).show(ui, |ui| lines(ui, ctx)).inner);

        assert!(!scrolled.is_empty() && scrolled.len() < 40, "{}", scrolled.len());
    }

    #[test]
    fn however_a_contract_writes_its_name_it_shows_whole_with_its_label_on_every_screen() {
        let me = vec!["0xaa".to_string()];
        let moved = token_moved("0xaa", None, "123456789123456789123456", "ERC-20");
        let chain = crate::chain::Chain::default();

        for size in crate::views::drawn::SCREENS {
            for symbol in ["WWWWWWWWWWWWWWWW", "﷽﷽﷽﷽﷽﷽﷽﷽﷽﷽﷽﷽﷽﷽﷽﷽", "\u{202e}\u{202e}\u{202e}\u{202e}\u{202e}\u{202e}\u{202e}\u{202e}"] {
                let names = answered(symbol, 6);
                let (width, galleys) = drawing(size, &chain, Vec::new(), |ui, ctx| {
                    line(ui, ctx, &me, &entry(&moved), Said::on(&names, unlisted::tests::GENESIS));

                    ui.min_rect().width()
                });
                let label = galleys.iter().find(|galley| galley.text().ends_with(UNVERIFIED)).unwrap_or_else(|| panic!("no label for {symbol} at {size:?}"));

                assert!(!label.elided, "cut short: {}", label.text());
                assert!(label.text().contains('"') && label.size().x <= AMOUNT_WIDTH + 0.5, "{} is {} wide", label.text(), label.size().x);
                assert!(width <= size.0 + 0.5, "{width} on a screen {} wide", size.0);
            }
        }
    }

    /// The Activity page drawn with a line of an unlisted contract's, for an install of this role on
    /// a chain known or not: what it sent.
    fn asked_by_a(role: Option<Role>, known: bool) -> Vec<Ask> {
        let (holder, evm) = watching();
        let (mut chain, genesis) = following();
        let moved = Checked { genesis, ..token_moved(&evm, None, "1500", "ERC-20") };
        let mut state = ActivityState::default();

        if !known {
            chain.genesis = None;
        }

        with_ctx(&chain, Vec::new(), |ui, ctx| {
            ctx.settings.role = role;
            ctx.holders.push(holder);
            ctx.settings.checked.insert("0x01ab".to_string(), moved);
            show(ui, ctx, &mut state);

            ctx.node.sent()
        })
        .0
    }

    #[test]
    fn an_online_wallet_asks_and_a_signer_asks_nothing() {
        let sent = asked_by_a(Some(Role::Online), true);

        assert!(matches!(sent.as_slice(), [Ask::Rpc(body)] if body.contains("eth_call") && body.contains("0x95d89b41")), "{sent:?}");

        // A signer has no such page, and nothing to send by, and asks nothing even so drawn.
        assert_eq!(asked_by_a(Some(Role::Signer), true), []);
        assert!(!crate::views::nav(Role::Signer).iter().chain(crate::views::tabs(Role::Signer)).any(|(page, _)| *page == crate::views::Page::Activity));

        // Nor does an install yet to say what it is, nor a wallet yet to know which chain it follows.
        assert_eq!(asked_by_a(None, true), []);
        assert_eq!(asked_by_a(Some(Role::Online), false), []);
    }

    #[test]
    fn a_failed_lookup_says_nothing_on_the_page_and_is_not_asked_again_soon() {
        let (holder, evm) = watching();
        let (chain, genesis) = following();
        let moved = Checked { genesis, ..token_moved(&evm, None, "1500", "ERC-20") };
        let mut state = ActivityState::default();
        let (_, text) = with_ctx(&chain, Vec::new(), |ui, ctx| {
            ctx.settings.role = Some(Role::Online);
            ctx.holders.push(holder);
            ctx.settings.checked.insert("0x01ab".to_string(), moved);
            show(ui, ctx, &mut state);

            // This node has no way out: its one question fails, and goes to the page that asked.
            let answers = ctx.node.answers();
            assert_eq!(answers.len(), 1);

            for (ask, result) in &answers {
                let Ask::Rpc(body) = ask else {
                    panic!("{ask:?}");
                };
                assert!(state.answered(body, result, ctx.now));
            }

            assert!(ctx.node.trouble().is_none(), "no page offers to ask it again");
            assert!(ctx.node.down().is_none(), "and nothing says the node is down for it");
            ctx.node.sent();

            for _ in 0..50 {
                ctx.now += 400;
                show(ui, ctx, &mut state);
            }

            assert_eq!(ctx.node.sent(), []);
        });

        assert!(!text.contains("unverified") && !text.contains("offline"), "{text}");
    }

    #[test]
    fn what_a_contract_said_reaches_no_other_screen_no_balance_and_nothing_kept() {
        let names = answered("USDC", 6);
        let said = Said::on(&names, unlisted::tests::GENESIS);
        let me = vec!["0xaa".to_string()];
        let moved = token_moved("0xaa", None, "1500000", "ERC-20");
        let chain = crate::chain::Chain::default();

        with_ctx(&chain, Vec::new(), |ui, ctx| {
            let kept = serde_json::to_value(&*ctx.settings).unwrap();

            assert!(said.amount(TOKEN, "1500000").is_some(), "it is there to see");
            line(ui, ctx, &me, &entry(&moved), said);

            // What every other page has for naming a token and showing its amounts, and the chain's
            // proven tokens, which balances are read from.
            assert_eq!(ctx.token(TOKEN), (format::short(TOKEN), 0));
            assert_eq!(ctx.amount(Some(TOKEN), "1500000"), format!("1,500,000 {}", format::short(TOKEN)));
            assert_eq!(ctx.brief(Some(TOKEN), "1500000"), format!("{} {}", format::compact("1500000", 0, 4), format::short(TOKEN)));
            assert!(ctx.chain.tokens.is_empty());
            assert_eq!(serde_json::to_value(&*ctx.settings).unwrap(), kept, "and nothing is stored");
        });

        // The Overview's lines, under its balances, have nothing to read it by, and ask nothing.
        let (holder, evm) = watching();
        let (chain, genesis) = following();
        let claim = Checked { genesis, ..token_moved(&evm, None, "1500000", "ERC-20") };
        let (sent, text) = with_ctx(&chain, Vec::new(), |ui, ctx| {
            ctx.settings.role = Some(Role::Online);
            ctx.holders.push(holder);
            ctx.settings.checked.insert("0x01ab".to_string(), claim);
            list(ui, ctx, 5);

            ctx.node.sent()
        });

        assert!(text.contains("Says you received") && !text.contains("unverified"), "{text}");
        assert_eq!(sent, []);

        // The rest cannot name it, as the module is private to this page. Should that ever change,
        // the pages someone signs or sends on, and the one with balances, must still not mention it.
        for (page, source) in [
            ("sign", include_str!("sign.rs")),
            ("evm", include_str!("evm.rs")),
            ("dapp", include_str!("dapp.rs")),
            ("contracts", include_str!("contracts.rs")),
            ("decoded", include_str!("decoded.rs")),
            ("send", include_str!("send.rs")),
            ("overview", include_str!("overview.rs")),
        ] {
            for word in ["unlisted::", "Unlisted", UNVERIFIED] {
                assert!(!source.contains(word), "{page} mentions {word}");
            }
        }
    }

    /// A transfer of 40 of the coin, or of a listed token, from one address to another.
    fn moved_between(from: &Address, to: &Address, token: Option<&str>) -> Checked {
        let mut moved = checked(&from.to_string(), &to.to_string(), true);
        moved.action = "Transfer".to_string();
        moved.kinds = vec!["transfer".to_string()];
        moved.moves[0].token = token.map(str::to_string);
        moved.fee = "21000".to_string();

        moved
    }

    #[test]
    fn a_transfer_between_the_account_s_own_addresses_shows_what_moved_and_where_for_every_pair_of_schemes() {
        let mut book = crate::views::drawn::two_phrases();
        let mut holders = crate::keys::open(&book).unwrap();
        let mine = holders[0].addresses();
        let addresses = with_evm_forms(&mine.iter().map(Address::to_string).collect::<Vec<_>>());
        let token = "contract:0x0303030303030303030303030303030303030303030303030303030303030303";
        let mut world = crate::views::drawn::following(&holders);
        world.chain.listed.push(token.to_string());
        world.chain.tokens.insert(token.to_string(), crate::chain::Proven {
            value: chainv4::registry::tokens::Token { address: token.parse().unwrap(), symbol: Some("GOLD".to_string()), name: None, decimals: Some(0) },
            assurance: chainv4::light::Assurance { block: chainv4::structs::hash::Hash::zero(chainv4::structs::hash::HashType::default_chain()), height: 1, final_: true },
        });

        for from in &mine {
            for to in mine.iter().filter(|to| *to != from) {
                for (asset, shown) in [(None, "DEV"), (Some(token), "GOLD")] {
                    let moved = moved_between(from, to, asset);
                    let mut summed = None;
                    crate::views::drawn::page_in(&mut world, &mut book, &mut holders, (360.0, 740.0), |_, ctx| summed = Some(summary(ctx, &addresses, &entry(&moved), Said::default())));
                    let (title, detail, amount, way) = summed.unwrap();
                    let (from_said, to_said) = (super::super::scheme(from), super::super::scheme(to));

                    assert_eq!((title.as_str(), way), ("Between your addresses", Way::Between), "{from_said} to {to_said}");
                    assert!(detail.starts_with(&format!("from your {from_said} to your {to_said}")), "{detail}");
                    assert!(amount.as_ref().is_some_and(|(brief, full)| brief.contains(shown) && full.contains(shown)), "{amount:?} of {shown}");
                }
            }
        }
    }

    #[test]
    fn a_transfer_between_the_account_s_addresses_draws_its_amount_and_fee_on_every_screen() {
        let mut book = crate::views::drawn::two_phrases();
        let mut holders = crate::keys::open(&book).unwrap();
        let mine = holders[0].addresses();
        let addresses = with_evm_forms(&mine.iter().map(Address::to_string).collect::<Vec<_>>());
        let mut moved = moved_between(&mine[0], &mine[3], None);
        moved.moves[0].amount = "1500000000000000000".to_string();

        for size in crate::views::drawn::SCREENS {
            let mut world = crate::views::drawn::following(&holders);
            let said = crate::views::drawn::said_in(&mut world, &mut book, &mut holders, size, |ui, ctx| {
                line(ui, ctx, &addresses, &entry(&moved), Said::default());
                details(ui, ctx, &addresses, &entry(&moved), Said::default());
            });

            assert!(said.contains("Between your addresses"), "{size:?}: {said}");
            assert!(said.lines().any(|line| line.contains("1.5 DEV")), "the amount on {size:?}: {said}");
            assert!(said.contains("from your Ed25519 to your Bitcoin"), "{said}");
            assert!(said.contains("Fee"), "and the fee: {said}");
        }
    }

    #[test]
    fn each_line_says_which_of_the_account_s_addresses_it_is_for() {
        let mut book = crate::views::drawn::two_phrases();
        let mut holders = crate::keys::open(&book).unwrap();
        let mine = holders[0].addresses();
        let addresses = with_evm_forms(&mine.iter().map(Address::to_string).collect::<Vec<_>>());
        let stranger = "ed25519:0x0909090909090909090909090909090909090909090909090909090909090909";
        let came = checked(stranger, &mine[2].to_string(), true);
        let left = checked(&mine[1].to_string(), stranger, true);
        let mut world = crate::views::drawn::following(&holders);
        let mut said = Vec::new();
        crate::views::drawn::page_in(&mut world, &mut book, &mut holders, (360.0, 740.0), |_, ctx| {
            said = vec![summary(ctx, &addresses, &entry(&came), Said::default()).1, summary(ctx, &addresses, &entry(&left), Said::default()).1];
        });

        assert!(said[0].contains("to your Ethereum"), "{}", said[0]);
        assert!(said[1].contains("from your EdBlake"), "{}", said[1]);
    }

    /// A chain whose node listed the newest page of this address's history, with more behind it.
    fn listed_a_page_of(address: &str) -> crate::chain::Chain {
        let (mut chain, _) = following();
        let (mut pinned, checkpoints, saved) = (std::collections::HashMap::new(), std::collections::HashMap::new(), std::collections::HashMap::new());
        let trust = crate::chain::Trust { node: "http://node", pinned: &mut pinned, checkpoints: &checkpoints, saved: &saved, fast_resume: false };
        let page = serde_json::json!({ "txs": [{ "id": "0x09", "height": 9 }], "next": "0xaa" });

        assert!(chain.take(&Ask::History { address: address.to_string(), cursor: None }, &Ok(page), trust, 0, 0.0));

        chain
    }

    #[test]
    fn older_transactions_shows_while_the_node_has_more_and_asks_for_the_next_page() {
        use crate::views::drawn;

        let (holder, _) = watching();
        let address = holder.addresses()[0].to_string();
        let mut world = drawn::World { chain: listed_a_page_of(&address), ..drawn::World::default() };
        world.settings.role = Some(Role::Online);
        let (mut book, mut holders) = (crate::vault::Book::default(), vec![holder]);

        for size in drawn::SCREENS {
            let said = drawn::said_in(&mut world, &mut book, &mut holders, size, |ui, ctx| show(ui, ctx, &mut ActivityState::default()));

            assert!(said.contains("Older transactions"), "{size:?}: {said}");
        }

        // Pressed, the next page is out, and the button waits for it.
        let tapped = drawn::tap_in(&mut world, &mut book, &mut holders, (360.0, 740.0), "Older transactions", |ui, ctx| show(ui, ctx, &mut ActivityState::default()));

        assert!(!tapped.contains("Older transactions"), "asking: {tapped}");
        assert_eq!(world.chain.older_pages(std::slice::from_ref(&address)), [Ask::History { address: address.clone(), cursor: Some("0xaa".to_string()) }]);

        // Reading checked blocks, no node's list is followed: Settings reads further back instead.
        world.settings.read_blocks = true;
        let said = drawn::said_in(&mut world, &mut book, &mut holders, (360.0, 740.0), |ui, ctx| show(ui, ctx, &mut ActivityState::default()));

        assert!(!said.contains("Older transactions") && said.contains("Reading checked blocks"), "{said}");

        // Read to its end, there is nothing older to offer.
        world.settings.read_blocks = false;
        world.chain = crate::chain::Chain::default();
        let said = drawn::said_in(&mut world, &mut book, &mut holders, (360.0, 740.0), |ui, ctx| show(ui, ctx, &mut ActivityState::default()));

        assert!(!said.contains("Older transactions"), "{said}");
    }
}
