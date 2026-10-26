//! A name, such as alice.dev: who holds it, what it resolves to, when it expires, its text
//! records, and what a year of it costs. As the node says it; the wallet proves names before it
//! pays one.

use {
    super::{Ctx, View},
    crate::api::Ask,
    crate::format,
    chainv4_ui::widgets,
    egui::{RichText, Ui},
};

pub fn show(ui: &mut Ui, ctx: &mut Ctx, name: &str) {
    let look = ctx.look;
    let store = ctx.store;

    if !store.names.contains_key(name) {
        ctx.api.ask(Ask::Name(name.to_string()));
    }

    let Some(found) = store.names.get(name).cloned() else {
        super::heading(ui, look, name, "", |_| {});
        super::waiting(ui, ctx, &Ask::Name(name.to_string()));

        return;
    };
    let color = match found.status.as_str() {
        "active" => look.palette.good,
        "grace" => look.palette.warn,
        _ => look.palette.muted,
    };

    super::heading(ui, look, &found.display, "", |ui| {
        widgets::pill(ui, &found.status, color);
    });
    ui.add_space(10.0);

    super::section(ui, look, "Name", |ui| {
        super::facts(ui, look, "name", |facts| {
            match &found.owner {
                Some(owner) => facts.row("owner", |ui| super::address(ui, ctx, owner)),
                None => facts.text("owner", "nobody: anyone may register it"),
            }

            match &found.resolves_to {
                Some(target) => facts.row("resolves to", |ui| super::address(ui, ctx, target)),
                None if found.target.is_some() => facts.text("resolves to", "nothing while it is expired"),
                None => facts.text("resolves to", "nothing"),
            }

            if let Some(expires) = found.expires {
                let when = format!("{} UTC, {}", format::utc(expires), format::ago(expires, ctx.now));
                facts.text("expires", when);
            }

            if let Some(grace_ends) = found.grace_ends {
                facts.text("grace ends", format!("{} UTC: then anyone may register it", format::utc(grace_ends)));
            }

            if let Some(price) = &found.price_per_year {
                facts.text("a year costs", format!("{}, burned", ctx.coins(price)));
            }

            // A subname's name, for its own page: the display without its first label.
            if found.name.contains('.')
                && let Some((_, parent)) = found.display.split_once('.')
            {
                let parent = parent.to_string();
                facts.row("subname of", |ui| {
                    if ui.link(RichText::new(&parent).monospace()).clicked() {
                        ctx.go(View::Name(parent.clone()));
                    }
                });
            }
        });
    });

    if !found.texts.is_empty() {
        super::section(ui, look, "Records", |ui| {
            super::facts(ui, look, "records", |facts| {
                for (key, value) in &found.texts {
                    facts.text(key, value.clone());
                }
            });
        });
    }
}
