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

/// A name as its record is kept: without the chain's ending, as alice for alice.dev, and pay.alice
/// for pay.alice.dev or for pay.alice typed without it. The ending is the node's word in /status;
/// from a node that does not say, the last label is taken for it, as an ending is always one label.
pub fn record_name(name: &str, suffix: Option<&str>) -> String {
    let lower = name.trim().to_ascii_lowercase();
    let kept = match suffix {
        Some(suffix) => lower.strip_suffix(&format!(".{}", suffix.to_ascii_lowercase())),
        None => lower.rsplit_once('.').map(|(kept, _)| kept),
    };

    kept.map_or_else(|| lower.clone(), str::to_string)
}

/// With Read from checked blocks on: a name's record by a direct proof someone pressed for. What a year
/// costs and what it comes to on a name that expired are worked out by the node.
fn reading(ui: &mut Ui, ctx: &mut Ctx, name: &str) {
    let look = ctx.look;
    let suffix = ctx.store.status.as_ref().and_then(|status| status.names_suffix.clone());
    let kept = record_name(name, suffix.as_deref());
    let key = chainv4::registry::names::record_key(&kept);

    super::heading(ui, look, name, "", |_| {});
    super::section(ui, look, "Name", |ui| {
        // What the proof is of, so a suffix the node gave wrongly shows rather than misleads.
        super::note(ui, look, format!("The record kept as {kept}."));
        let Some(proven) = super::reading::direct_proof(ui, ctx, "this name's record", chainv4::db::Column::Names, &key) else {
            return;
        };

        super::reading::proven_at(ui, ctx, &proven);

        match proven.value.as_deref().map(wincode::deserialize::<chainv4::registry::names::NameRecord>) {
            None => super::note(ui, look, "Nobody holds it."),
            Some(Err(_)) => {
                ui.colored_label(look.palette.bad, "A record that does not read");
            }
            Some(Ok(record)) => super::facts(ui, look, "name", |facts| {
                facts.row("owner", |ui| super::address(ui, ctx, &record.owner.to_string()));

                match record.target {
                    Some(target) => facts.row("points at", |ui| super::address(ui, ctx, &target.to_string())),
                    None => facts.text("points at", "nothing"),
                }

                if record.expires > 0 {
                    facts.text("expires", format!("{} UTC, {}", format::utc(record.expires), format::ago(record.expires, ctx.now)));
                }

                for text in &record.texts {
                    facts.text(&text.key, text.value.clone());
                }
            }),
        }
    });
}

pub fn show(ui: &mut Ui, ctx: &mut Ctx, name: &str) {
    let look = ctx.look;
    let store = ctx.store;

    if ctx.settings.read_blocks {
        reading(ui, ctx, name);

        return;
    }

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

#[cfg(test)]
mod tests {
    use super::record_name;

    #[test]
    fn a_record_is_found_by_the_chain_s_ending_alone() {
        assert_eq!(record_name("Alice.dev", Some("dev")), "alice");
        assert_eq!(record_name("pay.alice.dev", Some("dev")), "pay.alice");
        assert_eq!(record_name("pay.alice", Some("dev")), "pay.alice", "typed without the ending");
        assert_eq!(record_name("alice", Some("dev")), "alice");
        assert_eq!(record_name("alice.dev", None), "alice", "a node that does not say: the last label");
        assert_eq!(record_name("alice", None), "alice");
    }
}
