//! What happened to the account, at all its addresses together, newest first: what it sent, from
//! this wallet's own log until proven, and every transaction the node's address index says touched
//! one of them, each shown once the light client proved who sent what and what it did. Only the
//! chain the wallet follows now: a new chain of the same keys has its own heights. Filtered by
//! kind, searched, and sorted either way.

use {
    super::Ctx,
    crate::app::Sent,
    crate::format,
    crate::history::Checked,
    crate::theme,
    egui::{RichText, Ui},
    std::collections::HashSet,
};

/// How long a sent transaction may go unseen before the history says the node may have dropped it,
/// in milliseconds.
pub const UNSEEN_AFTER: u64 = 10 * 60 * 1_000;

/// The widest a line's amount and pill get.
const AMOUNT_WIDTH: f32 = 150.0;

/// Which lines show.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    #[default]
    All,
    Sent,
    Received,
    /// Not proven yet.
    Waiting,
    Failed,
}

#[derive(Default)]
pub struct ActivityState {
    kind: Kind,
    search: String,
    oldest_first: bool,
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

    ui.horizontal_wrapped(|ui| {
        for (kind, name) in [(Kind::All, "All"), (Kind::Sent, "Sent"), (Kind::Received, "Received"), (Kind::Waiting, "Waiting"), (Kind::Failed, "Failed")] {
            ui.selectable_value(&mut state.kind, kind, name);
        }
    });
    theme::segmented(ui, &palette, &mut state.oldest_first, &[(false, "Newest first"), (true, "Oldest first")]);
    ui.add(egui::TextEdit::singleline(&mut state.search).hint_text("Search: a transaction, an address, a contract, what it did").desired_width(f32::INFINITY));
    ui.add_space(8.0);

    let addresses = ctx.addresses();
    let mut entries = entries(ctx, &addresses);
    entries.retain(|entry| entry.is(state.kind, &addresses) && entry.matches(&state.search));

    if state.oldest_first {
        entries.reverse();
    }

    theme::card(ui, &palette, |ui| {
        if entries.is_empty() {
            ui.label(RichText::new(match (state.kind, state.search.trim().is_empty(), ctx.chain.has_followed()) {
                (Kind::All, true, true) => "Nothing yet.",
                (Kind::All, true, false) => "Looking once the light client catches up…",
                _ => "Nothing matches.",
            }).color(palette.muted));

            return;
        }

        for (index, entry) in entries.iter().enumerate() {
            if index > 0 {
                theme::rule(ui, &palette);
            }

            line(ui, ctx, &addresses, entry);
        }
    });
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
    /// Whether it is of a kind: sent by the account, bringing it something, not proven yet, or
    /// failed.
    fn is(&self, kind: Kind, addresses: &[String]) -> bool {
        match (kind, self.checked) {
            (Kind::All, _) => true,
            (Kind::Sent, Some(checked)) => addresses.contains(&checked.sender),
            (Kind::Sent, None) => self.sent.is_some(),
            (Kind::Received, Some(checked)) => checked.for_account(addresses).any(|(incoming, _)| incoming),
            (Kind::Waiting, checked) => checked.is_none(),
            (Kind::Failed, Some(checked)) => !checked.done,
            (Kind::Received | Kind::Failed, None) => false,
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
    let addresses = ctx.addresses();
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
                        _ => ("−", palette.ink),
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
                let token = moved.token.as_deref();
                let from = moved.from.as_deref().map_or_else(|| "minted".to_string(), |from| format!("from {}", format::short(from)));
                let amount = (ctx.brief(token, &moved.amount), ctx.amount(token, &moved.amount));

                ("Received".to_string(), format!("{from} · {}", when(checked)), Some(amount), Some(true))
            }
            Some((false, moved)) => {
                let token = moved.token.as_deref();
                let to = moved.to.as_deref().map_or_else(|| "burned".to_string(), |to| format!("to {}", format::short(to)));
                let amount = (ctx.brief(token, &moved.amount), ctx.amount(token, &moved.amount));

                ("Sent".to_string(), format!("{to} · {}", when(checked)), Some(amount), Some(false))
            }
            None => {
                let by = match addresses.contains(&checked.sender) {
                    true => String::new(),
                    false => format!("by {} · ", format::short(&checked.sender)),
                };

                (checked.action.clone(), format!("{by}{}", when(checked)), None, None)
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

        for (incoming, moved) in checked.for_account(addresses) {
            let other = match incoming {
                true => moved.from.as_deref().map_or_else(|| "minted".to_string(), |from| format!("from {}", format::short(from))),
                false => moved.to.as_deref().map_or_else(|| "burned".to_string(), |to| format!("to {}", format::short(to))),
            };
            super::row(ui, &palette, if incoming { "Received" } else { "Sent" }, |ui| {
                ui.label(format!("{} {other}", ctx.amount(moved.token.as_deref(), &moved.amount)));
            });
        }

        if addresses.contains(&checked.sender) {
            super::row(ui, &palette, "Fee", |ui| {
                ui.label(format!("{} · nonce {}", ctx.amount(None, &checked.fee), checked.nonce));
            });
        }

        for event in &checked.events {
            super::row(ui, &palette, "Event", |ui| {
                ui.label(theme::mono(event).color(palette.soft));
            });
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
    use {super::*, crate::history::Move};

    fn checked(sender: &str, to: &str, done: bool) -> Checked {
        Checked {
            height: 7,
            time: 0,
            final_: true,
            done,
            sender: sender.to_string(),
            nonce: 0,
            action: "Called swap on contract:0x6c1d".to_string(),
            moves: vec![Move { token: None, from: Some(sender.to_string()), to: Some(to.to_string()), amount: "40".to_string() }],
            fee: "0".to_string(),
            events: vec!["Swap contract:0x0000 contract:0x5119".to_string()],
            genesis: "0x9a7f".to_string(),
            vote: None,
        }
    }

    #[test]
    fn lines_are_told_apart_by_kind_and_found_by_what_they_say() {
        let me = vec!["ed25519:0xaa".to_string()];
        let (out, back, failed) = (checked("ed25519:0xaa", "contract:0x6c1d", true), checked("ed25519:0xbb", "ed25519:0xaa", true), checked("ed25519:0xaa", "ed25519:0xcc", false));
        fn line(checked: &Checked) -> Entry<'_> {
            Entry { tx: "0x01ab".to_string(), checked: Some(checked), sent: None, height: Some(7) }
        }

        let waiting = Entry { tx: "0x02cd".to_string(), checked: None, sent: None, height: None };

        assert!(line(&out).is(Kind::Sent, &me) && !line(&out).is(Kind::Received, &me));
        assert!(line(&back).is(Kind::Received, &me) && !line(&back).is(Kind::Sent, &me));
        assert!(line(&failed).is(Kind::Failed, &me) && !line(&out).is(Kind::Failed, &me));
        assert!(waiting.is(Kind::Waiting, &me) && !line(&out).is(Kind::Waiting, &me));
        assert!([Kind::Sent, Kind::Received, Kind::Waiting, Kind::Failed, Kind::All].iter().any(|kind| line(&out).is(*kind, &me)));

        // By its id, a party, what it did, or an event, its case aside.
        for search in ["01AB", "0x6c1d", "called swap", "5119", ""] {
            assert!(line(&out).matches(search), "{search}");
        }

        assert!(!line(&out).matches("0xdead"));
    }
}
