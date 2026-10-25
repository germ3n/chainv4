//! What happened to the account, newest first: what it sent, from this wallet's own log until
//! proven, and every transaction the node's address index says touched it, each shown once the
//! light client proved who sent what and what it did.

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

pub fn show(ui: &mut Ui, ctx: &mut Ctx) {
    let palette = *ctx.palette;
    theme::title(ui, &palette, "Activity", "Everything this account sent, and everything that touched it, as the light client proved it.");

    let note = match (ctx.chain.indexes_addresses, ctx.chain.indexes_events) {
        (Some(false), Some(false)) => Some("This node keeps no address index, nor an event index: only what this wallet sent shows. Start it with --index-addresses."),
        (Some(false), _) => Some("This node keeps no address index, so coins sent here show in the balance but not below. Start it with --index-addresses."),
        _ => None,
    };

    if let Some(note) = note {
        ui.label(RichText::new(note).color(palette.warn));
        ui.add_space(10.0);
    }

    theme::card(ui, &palette, |ui| list(ui, ctx, usize::MAX));
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
    let address = ctx.address();
    let entries = entries(ctx, &address);

    if entries.is_empty() {
        let text = match ctx.chain.caught_up() {
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

        line(ui, ctx, &address, entry);
    }
}

fn entries<'a>(ctx: &'a Ctx, address: &str) -> Vec<Entry<'a>> {
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

    for sent in ctx.settings.sent.iter().filter(|sent| sent.from == address) {
        add(&sent.id, Some(sent), None);
    }

    for mention in ctx.chain.mentions.get(address).into_iter().flatten() {
        add(&mention.tx, None, Some(mention.height));
    }

    // What was proven before, whether or not the node still lists it.
    for (tx, checked) in &ctx.settings.checked {
        if checked.sender == address || checked.for_holder(address).next().is_some() {
            add(tx, None, None);
        }
    }

    entries.sort_by_key(|entry| std::cmp::Reverse(entry.order()));

    entries
}

fn line(ui: &mut Ui, ctx: &Ctx, address: &str, entry: &Entry) {
    let palette = *ctx.palette;
    let id = ui.make_persistent_id(("activity", &entry.tx));
    let open = ui.data(|data| data.get_temp::<bool>(id)).unwrap_or(false);
    let (title, detail, amount, incoming) = summary(ctx, address, entry);
    let (status, color) = status(ctx, entry);
    // The amount and the pill keep their column on the right, however narrow the screen.
    let words = (ui.available_width() - 150.0).max(120.0);

    let response = ui
        .horizontal(|ui| {
            let mark = match incoming {
                Some(true) => palette.good,
                Some(false) => palette.accent,
                None => palette.muted,
            };
            theme::dot(ui, mark);
            ui.vertical(|ui| {
                ui.set_max_width(words);
                ui.label(theme::strong(&title, 14.5).color(palette.ink));
                ui.add(egui::Label::new(RichText::new(&detail).small().color(palette.muted)).wrap());
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.with_layout(egui::Layout::top_down(egui::Align::Max), |ui| {
                    if let Some((brief, full)) = &amount {
                        let (sign, color) = match incoming {
                            Some(true) => ("+", palette.good),
                            _ => ("−", palette.ink),
                        };
                        ui.label(theme::strong(format!("{sign}{brief}"), 14.5).color(color)).on_hover_text(full);
                    }

                    theme::pill(ui, status, color);
                });
            });
        })
        .response
        .interact(egui::Sense::click());

    if response.clicked() {
        ui.data_mut(|data| data.insert_temp(id, !open));
    }

    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    if open {
        ui.indent(id, |ui| details(ui, ctx, address, entry));
    }
}

/// A line's title, what it says under it, the amount it moved for this account, cut short and in
/// full, and whether that came in.
fn summary(ctx: &Ctx, address: &str, entry: &Entry) -> (String, String, Option<(String, String)>, Option<bool>) {
    let when = |checked: &Checked| match checked.time {
        0 => format!("block {}", format::thousands(&checked.height.to_string())),
        time => format::ago(time, ctx.now),
    };

    match (entry.checked, entry.sent) {
        (Some(checked), _) => match checked.for_holder(address).next() {
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
                let by = match checked.sender == address {
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

fn details(ui: &mut Ui, ctx: &Ctx, address: &str, entry: &Entry) {
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

        for (incoming, moved) in checked.for_holder(address) {
            let other = match incoming {
                true => moved.from.as_deref().map_or_else(|| "minted".to_string(), |from| format!("from {}", format::short(from))),
                false => moved.to.as_deref().map_or_else(|| "burned".to_string(), |to| format!("to {}", format::short(to))),
            };
            super::row(ui, &palette, if incoming { "Received" } else { "Sent" }, |ui| {
                ui.label(format!("{} {other}", ctx.amount(moved.token.as_deref(), &moved.amount)));
            });
        }

        if checked.sender == address {
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
