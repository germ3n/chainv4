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

use {
    super::{Ctx, decoded},
    crate::app::Sent,
    crate::format,
    crate::history::{Checked, KeptCall, KeptLog, Move, with_evm_forms},
    crate::node::Ask,
    crate::theme,
    chainv4::structs::address::Address,
    chainv4_ui::filters::{self, Answered, COIN, Facts, Offered, TxFilter},
    egui::{RichText, Ui},
    std::collections::HashSet,
};

/// How long a sent transaction may go unseen before the history says the node may have dropped it,
/// in milliseconds.
pub const UNSEEN_AFTER: u64 = 10 * 60 * 1_000;

/// The widest a line's amount and pill get.
const AMOUNT_WIDTH: f32 = 150.0;

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
}

impl ActivityState {
    /// For a reload: asks again for what the filters leave.
    pub fn reload(&mut self) {
        self.asked = None;
        self.proved = 0;
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

    let note = match (ctx.chain.indexes_addresses, ctx.chain.indexes_events) {
        (Some(false), Some(false)) => Some("This node keeps no address index, nor an event index: only what this wallet sent shows. Start it with --index-addresses."),
        (Some(false), _) => Some("This node keeps no address index, so coins sent here show in the balance but not below. Start it with --index-addresses."),
        _ => None,
    };

    if let Some(note) = note {
        ui.add(egui::Label::new(chainv4_ui::widgets::flagged(ui, note, egui::TextStyle::Body, palette.warn)).wrap());
        ui.add_space(10.0);
    }

    let tokens = offered_tokens(ctx);
    let problem = state.filter.problem(|text| text.parse::<Address>().is_ok());
    filters::controls(ui, &palette, &mut state.filter, Offered { coin: &ctx.coin.symbol, tokens: &tokens }, problem.as_deref());
    ui.add_space(2.0);
    theme::segmented(ui, &palette, &mut state.kind, &[(Kind::All, "All"), (Kind::Waiting, "Waiting"), (Kind::Failed, "Failed")]);
    theme::segmented(ui, &palette, &mut state.oldest_first, &[(false, "Newest first"), (true, "Oldest first")]);
    ui.add(egui::TextEdit::singleline(&mut state.search).hint_text("Search: a transaction, an address, a contract, what it did").desired_width(f32::INFINITY));
    ui.add_space(8.0);

    let addresses = ctx.addresses();
    // An EVM move names its parties by the 20 bytes each goes by there. What the node is asked stays
    // about the addresses themselves, which are all it can look up.
    let parties = with_evm_forms(&addresses);
    // What was typed wrong is said above, and the list stays as it was until it reads.
    let query = (state.filter.on() > 0 && problem.is_none()).then(|| state.filter.query(ctx.now));
    let under = query.as_ref().map(|query| listed(ctx.chain, &addresses, query));

    if let Some(query) = &query {
        ask_filtered(ctx, state, &addresses, query, under.as_ref());
    }

    let mut entries = match &under {
        None => entries(ctx, &parties),
        Some(listed) => entries_under(ctx, &parties, listed, &state.filter),
    };
    entries.retain(|entry| entry.is(state.kind) && (entry.matches(&state.search) || reads_as(ctx, entry, &state.search)));

    if state.oldest_first {
        entries.reverse();
    }

    theme::card(ui, &palette, |ui| {
        if entries.is_empty() {
            let looking = under.as_ref().is_some_and(|listed| !listed.read);
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

            line(ui, ctx, &parties, entry);
        }
    });

    if let (Some(query), Some(listed)) = (&query, &under) {
        further(ui, ctx, query, listed, state.filter.on(), entries.len());
    }
}

/// Asks the node what passes the filters, and for proof of what it listed: the filter's own page,
/// so nothing goes on while it is not looked at.
fn ask_filtered(ctx: &mut Ctx, state: &mut ActivityState, addresses: &[String], query: &str, under: Option<&Listed>) {
    if state.asked.as_ref().is_none_or(|(asked, at)| asked != query || ctx.now.saturating_sub(*at) >= FILTERED_EVERY) {
        state.asked = Some((query.to_string(), ctx.now));

        for address in addresses {
            ctx.node.ask(Ask::Filtered { address: address.clone(), query: query.to_string(), cursor: None });
        }
    }

    if !ctx.chain.caught_up() || ctx.now.saturating_sub(state.proved) < PROVEN_EVERY {
        return;
    }

    state.proved = ctx.now;
    let unsettled = under.into_iter().flat_map(|listed| &listed.mentions).filter(|(tx, _)| !ctx.settings.checked.get(tx).is_some_and(|checked| checked.final_));

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

    if listed.answered.filtered && ctx.chain.indexes_addresses != Some(false) {
        for (tx, height) in &listed.mentions {
            if seen.insert(tx.clone()) {
                found.push(Entry { tx: tx.clone(), checked: ctx.settings.checked.get(tx), sent: None, height: Some(*height) });
            }
        }
    } else {
        let genesis = ctx.chain.genesis.map(|genesis| genesis.to_string());
        let here = |kept: &str| genesis.as_deref().is_none_or(|genesis| kept == genesis);

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
    let waiting = listed.mentions.iter().filter(|(tx, _)| !ctx.settings.checked.contains_key(tx)).count();
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

/// An amount moved, cut short and in full: an NFT's is which one it was.
fn moved_amount(ctx: &Ctx, moved: &Move) -> (String, String) {
    let token = moved.token.as_deref();

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

        line(ui, ctx, &addresses, entry);
    }
}

fn entries<'a>(ctx: &'a Ctx, addresses: &[String]) -> Vec<Entry<'a>> {
    let mut seen = HashSet::new();
    let mut entries = Vec::new();
    let mut add = |tx: &str, sent: Option<&'a Sent>, height: Option<u64>| {
        if seen.insert(tx.to_string()) {
            entries.push(Entry {
                tx: tx.to_string(),
                checked: ctx.settings.checked.get(tx),
                sent,
                height,
            });
        }
    };

    // The chain followed now; until it is known, everything kept.
    let genesis = ctx.chain.genesis.map(|genesis| genesis.to_string());
    let here = |kept: &str| genesis.as_deref().is_none_or(|genesis| kept == genesis);

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

fn line(ui: &mut Ui, ctx: &Ctx, addresses: &[String], entry: &Entry) {
    let palette = *ctx.palette;
    let id = ui.make_persistent_id(("activity", &entry.tx));
    let open = ui.data(|data| data.get_temp::<bool>(id)).unwrap_or(false);
    let (title, detail, amount, incoming) = summary(ctx, addresses, entry);
    let (status, color) = status(ctx, entry);
    // The amount and the pill keep their column on the right, on the line's first line, however
    // narrow the screen and however long what it says.
    let response = theme::end_row(
        ui,
        |ui| {
            let mark = match incoming {
                Some(true) => palette.good,
                Some(false) => palette.accent,
                None => palette.muted,
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
                        Some(true) => ("+", palette.good),
                        Some(false) => ("−", palette.ink),
                        // Only claimed: it is not said to have added to anything.
                        None => ("", palette.soft),
                    };
                    theme::fitted(ui, &format!("{sign}{brief}"), 14.5, 10.0, egui::FontFamily::Name(theme::SEMIBOLD.into()), color).on_hover_text(full);
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

    if open {
        ui.indent(id, |ui| details(ui, ctx, addresses, entry));
    }
}

/// A line's title, what it says under it, the amount it moved for this account, cut short and in
/// full, and whether that came in.
fn summary(ctx: &Ctx, addresses: &[String], entry: &Entry) -> (String, String, Option<(String, String)>, Option<bool>) {
    let when = |checked: &Checked| match checked.time {
        0 => format!("block {}", format::thousands(&checked.height.to_string())),
        time => format::ago(time, ctx.now),
    };

    match (entry.checked, entry.sent) {
        (Some(checked), _) => match checked.for_account(addresses).next() {
            Some((true, moved)) => {
                let from = moved.from.as_deref().map_or_else(|| "minted".to_string(), |from| format!("from {}", format::short(from)));
                let (title, sign) = moved_title(ctx, moved, "Received", "Says you received", true);

                (title, format!("{from} · {}{}", when(checked), read_as(moved)), Some(moved_amount(ctx, moved)), sign)
            }
            Some((false, moved)) => {
                let to = moved.to.as_deref().map_or_else(|| "burned".to_string(), |to| format!("to {}", format::short(to)));
                let (title, sign) = moved_title(ctx, moved, "Sent", "Says you sent", false);

                (title, format!("{to} · {}{}", when(checked), read_as(moved)), Some(moved_amount(ctx, moved)), sign)
            }
            None => {
                let by = match addresses.contains(&checked.sender) {
                    true => String::new(),
                    false => format!("by {} · ", format::short(&checked.sender)),
                };
                let (title, read_by) = did(ctx, checked);
                let read_by = read_by.map_or_else(String::new, |read_by| format!(" · {read_by}"));

                (title, format!("{by}{}{read_by}", when(checked)), None, None)
            }
        },
        (None, Some(sent)) => (
            "Sending".to_string(),
            format!("to {} · {}", format::short(&sent.to), format::ago(sent.at, ctx.now)),
            Some((sent.what.clone(), sent.what.clone())),
            Some(false),
        ),
        (None, None) => (
            "Checking…".to_string(),
            entry.height.map_or_else(String::new, |height| format!("block {}", format::thousands(&height.to_string()))),
            None,
            None,
        ),
    }
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
        (Some(_), _) => ("Proven", palette.accent),
        (None, _) if ctx.chain.refused.contains_key(&entry.tx) => ("Does not check", palette.bad),
        (None, Some(sent)) if ctx.now.saturating_sub(sent.at) > UNSEEN_AFTER => ("Not seen", palette.warn),
        (None, Some(_)) => ("Pending", palette.warn),
        (None, None) => ("Checking", palette.muted),
    }
}

fn details(ui: &mut Ui, ctx: &Ctx, addresses: &[String], entry: &Entry) {
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

        for (incoming, moved) in checked.for_account(addresses) {
            let other = match incoming {
                true => moved.from.as_deref().map_or_else(|| "minted".to_string(), |from| format!("from {}", format::short(from))),
                false => moved.to.as_deref().map_or_else(|| "burned".to_string(), |to| format!("to {}", format::short(to))),
            };
            let (title, _) = moved_title(ctx, moved, if incoming { "Received" } else { "Sent" }, if incoming { "Says you received" } else { "Says you sent" }, incoming);
            super::row(ui, &palette, &title, |ui| {
                ui.label(format!("{} {other}{}", moved_amount(ctx, moved).1, read_as(moved)));
            });

            if claimed(ctx, moved) {
                ui.label(RichText::new("Only what a contract's log claims: the registry does not list that contract, and no balance counts it.").small().color(palette.muted));
            }
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
        }
    }

    /// Runs something with a page's context, on a chain that holds these tokens, drawing it headless:
    /// what it returned, and the text it drew.
    fn with_ctx<R>(chain: &crate::chain::Chain, contracts: Vec<crate::app::SavedContract>, draw: impl FnOnce(&mut Ui, &Ctx) -> R) -> (R, String) {
        let wake = egui::Context::default();
        let reader = chainv4::abi::known::AbiReader::default();
        let mut node = crate::node::Node::new("http://127.0.0.1:1", Err("offline".to_string()), wake.clone());
        let (mut holders, mut book, mut settings, mut page) = (Vec::new(), crate::vault::Book::default(), crate::app::Settings::default(), super::super::Page::Activity);
        let (mut connect, mut changed, mut vault, mut leave, mut lock) = (None, false, None, false, None);
        let coin = crate::app::Coin { symbol: "DEV".to_string(), decimals: 18 };
        #[cfg(all(feature = "bitcoin-light", not(target_arch = "wasm32")))]
        let mut lights = crate::light::Lights::default();

        settings.contracts = contracts;

        let palette = theme::DARK;
        let ctx = Ctx {
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
        };
        let mut returned = None;
        let mut draw = Some(draw);
        let mut output = wake.run_ui(egui::RawInput::default(), |ui| {
            returned = draw.take().map(|draw| draw(ui, &ctx));
        });
        output.textures_delta.clear();

        let text = output.shapes.iter().filter_map(|clipped| match &clipped.shape {
            egui::epaint::Shape::Text(shape) => Some(format!("{}\n", shape.galley.text())),
            _ => None,
        });

        (returned.expect("drawn once"), text.collect())
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
        let ((title, detail, amount, incoming), _) = with_ctx(&crate::chain::Chain::default(), Vec::new(), |_, ctx| summary(ctx, &me, &entry(&moved)));

        assert_eq!(title, "Says you received");
        assert!(detail.contains("read as ERC-20"), "{detail}");
        assert_eq!(incoming, None, "so it gets no plus, and nothing says it came in");
        assert!(amount.unwrap().1.starts_with("1,500 "), "a plain whole number, in no decimals");

        let (_, text) = with_ctx(&crate::chain::Chain::default(), Vec::new(), |ui, ctx| details(ui, ctx, &me, &entry(&moved)));

        assert!(text.contains("no balance counts it"), "{text}");
    }

    #[test]
    fn an_evm_transfer_of_a_listed_token_counts_as_any_token_s_does() {
        let me = vec!["0xaa".to_string()];
        let moved = token_moved("0xaa", None, "1500", "ERC-20");
        let mut chain = crate::chain::Chain::default();
        chain.tokens.insert(TOKEN.to_string(), listed_token());
        let ((title, detail, amount, incoming), _) = with_ctx(&chain, Vec::new(), |_, ctx| summary(ctx, &me, &entry(&moved)));

        assert_eq!((title.as_str(), incoming), ("Received", Some(true)));
        assert!(detail.contains("read as ERC-20"), "still says what read it: {detail}");
        assert_eq!(amount.unwrap().1, "15 TKN", "in its decimals");
    }

    #[test]
    fn an_nft_moving_says_which_token_rather_than_an_amount() {
        let me = vec!["0xaa".to_string()];
        let nft = token_moved("0xaa", Some("9"), "1", "ERC-721");
        let ((_, _, amount, _), _) = with_ctx(&crate::chain::Chain::default(), Vec::new(), |_, ctx| summary(ctx, &me, &entry(&nft)));

        assert!(amount.unwrap().1.starts_with("token 9 of "));

        let multi = token_moved("0xaa", Some("4"), "50", "ERC-1155");
        let ((_, _, amount, _), _) = with_ctx(&crate::chain::Chain::default(), Vec::new(), |_, ctx| summary(ctx, &me, &entry(&multi)));

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
        let ((title, detail, _, _), _) = with_ctx(&crate::chain::Chain::default(), Vec::new(), |_, ctx| summary(ctx, &me, &entry(&called)));

        assert!(title.starts_with("Called transfer on evm:0x0505"), "{title}");
        assert!(detail.ends_with("read as ERC-20"), "{detail}");

        let (_, text) = with_ctx(&crate::chain::Chain::default(), Vec::new(), |ui, ctx| details(ui, ctx, &me, &entry(&called)));

        for said in ["Did", "transfer", "value =", "Transfer", "read as ERC-20", "Raw data"] {
            assert!(text.contains(said), "{said} in {text}");
        }

        // An ABI saved for the contract goes first, and says so differently.
        let saved = crate::app::SavedContract {
            address: TOKEN.to_string(),
            name: "mine".to_string(),
            abi: r#"[{"type": "function", "name": "transfer", "inputs": [{"name": "payee", "type": "address"}, {"name": "cents", "type": "uint256"}]}]"#.to_string(),
        };
        let ((_, detail, _, _), _) = with_ctx(&crate::chain::Chain::default(), vec![saved], |_, ctx| summary(ctx, &me, &entry(&called)));

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
}
