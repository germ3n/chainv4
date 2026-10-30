//! The name directory: every name on the chain, sorted by name, a page at a time, each with its
//! status, owner, what it resolves to and when it expires; and a search that opens a name's page.
//! As the node says it.

use {
    super::{Ctx, View},
    crate::api::Ask,
    crate::format,
    crate::model::ListedName,
    chainv4_ui::widgets,
    egui::{RichText, Ui},
};

const ABOUT: &str = "Every name on the chain, such as alice.dev, by name: who owns it, what it resolves to, and when it expires.";

#[derive(Default)]
pub struct NamesState {
    /// The name typed into the search.
    typed: String,
}

/// A name as typed, as its page looks it up: trimmed and in lowercase; None for nothing typed.
pub fn wanted(typed: &str) -> Option<String> {
    let name = typed.trim().to_ascii_lowercase();

    (!name.is_empty()).then_some(name)
}

pub fn show(ui: &mut Ui, ctx: &mut Ctx, state: &mut NamesState) {
    let look = ctx.look;
    let store = ctx.store;
    let first = Ask::NamesList { after: None };
    super::heading(ui, look, "Names", ABOUT, |_| {});

    super::section(ui, look, "Find a name", |ui| {
        let palette = look.palette;
        let typed = wanted(&state.typed);
        let pressed = widgets::beside(
            ui,
            |ui| widgets::primary(ui, &palette, "Open", typed.is_some()).clicked(),
            |ui, width| {
                let field = ui.add(egui::TextEdit::singleline(&mut state.typed).hint_text("A name, such as alice.dev").desired_width(width));

                if field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                    ctx.go(View::Name(typed.clone().unwrap_or_default()));
                }
            },
        );

        if pressed && let Some(name) = typed {
            ctx.go(View::Name(name));
        }
    });

    let listing = &store.name_list;

    if listing.pages == 0 {
        match store.failures.get(&first) {
            Some(failure) if failure.status == 404 => super::section(ui, look, "No names here", |ui| {
                super::note(ui, look, "This chain keeps no names, or its node is from before the name directory.");
            }),
            Some(_) => super::waiting(ui, ctx, &first),
            None => {
                ctx.api.ask(first.clone());
                super::waiting(ui, ctx, &first);
            }
        }

        return;
    }

    super::section_with(
        ui,
        look,
        "Directory",
        |ui| {
            ui.label(RichText::new(format!("{} read", listing.names.len())).monospace().color(look.palette.muted));
        },
        |ui| {
            if listing.names.is_empty() {
                super::note(ui, look, "Nobody has registered a name yet.");

                return;
            }

            directory(ui, ctx, &listing.names);

            if let Some(next) = listing.next.clone() {
                ui.add_space(6.0);
                let ask = Ask::NamesList { after: Some(next) };

                match ctx.api.asking(&ask) {
                    true => {
                        ui.spinner();
                    }
                    false => {
                        if widgets::quiet(ui, &look.palette, "More").clicked() {
                            ctx.api.retry(ask.clone());
                        }

                        if let Some(failure) = store.failures.get(&ask)
                            && widgets::retry(ui, &look.palette, &failure.sentence(), ctx.api.asking(&ask))
                        {
                            ctx.api.retry(ask.clone());
                        }
                    }
                }
            }
        },
    );
}

/// A name's status as a pill.
fn status(ui: &mut Ui, palette: &chainv4_ui::Palette, status: &str) {
    let color = match status {
        "active" => palette.good,
        "grace" => palette.warn,
        _ => palette.muted,
    };

    widgets::pill(ui, status, color);
}

/// When a name expires, as a date and how far off.
fn expires(ctx: &Ctx, name: &ListedName) -> String {
    match name.expires {
        Some(expires) => format!("{} UTC, {}", format::utc(expires), format::ago(expires, ctx.now)),
        None => "—".to_string(),
    }
}

/// A link that opens a name's page.
fn name_link(ui: &mut Ui, ctx: &mut Ctx, name: &ListedName) {
    if ui.link(RichText::new(&name.display).monospace().color(ctx.look.palette.accent)).clicked() {
        ctx.go(View::Name(name.name.clone()));
    }
}

/// The names read: a table on a wide screen, rows on a phone.
fn directory(ui: &mut Ui, ctx: &mut Ctx, names: &[ListedName]) {
    let look = ctx.look;

    if look.narrow {
        for name in names {
            super::list_row(ui, look, |ui| {
                widgets::end_row(ui, |ui| name_link(ui, ctx, name), |ui| status(ui, &look.palette, &name.status));

                if let Some(owner) = &name.owner {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new("owner").small().color(look.palette.muted));
                        super::address(ui, ctx, owner);
                    });
                }

                ui.label(RichText::new(format!("expires {}", expires(ctx, name))).small().color(look.palette.muted));
            });
        }

        return;
    }

    super::table(ui, look, "names", &["name", "status", "owner", "resolves to", "expires"], |ui| {
        for name in names {
            name_link(ui, ctx, name);
            status(ui, &look.palette, &name.status);

            for party in [&name.owner, &name.target] {
                match party {
                    Some(party) => super::address(ui, ctx, party),
                    None => {
                        ui.label(RichText::new("—").color(look.palette.muted));
                    }
                }
            }

            ui.label(expires(ctx, name));
            super::row_end(ui);
        }
    });
}

#[cfg(test)]
mod tests {
    use {super::*, crate::api::Failure, crate::light::Light, crate::store::Store, serde_json::json};

    fn store() -> Store {
        let mut store = Store::default();
        store.name_list.take(
            serde_json::from_value(json!({
                "names": [
                    { "name": "alice.dev", "display": "alice.dev", "status": "active", "owner": "ed25519:0x0101010101010101010101010101010101010101010101010101010101010101", "target": null, "expires": 1_900_000_000_000_u64 },
                    { "name": "bob.dev", "display": "bob.dev", "status": "grace", "owner": null, "target": null, "expires": null },
                ],
                "next": "bob.dev",
            }))
            .unwrap(),
            false,
        );

        store
    }

    #[test]
    fn a_typed_name_is_looked_up_in_lowercase() {
        assert_eq!(wanted("  Alice.DEV "), Some("alice.dev".to_string()));
        assert_eq!(wanted("   "), None);
    }

    #[test]
    fn the_directory_lists_names_on_any_screen_and_pages_on() {
        for width in [1_200.0, 380.0] {
            let (text, _) = crate::views::render(&store(), &mut Light::default(), width, |ui, ctx| show(ui, ctx, &mut NamesState::default()));

            for wanted in ["Names", "alice.dev", "bob.dev", "active", "grace", "2 read", "More"] {
                assert!(text.contains(wanted), "{wanted} at {width} wide, in {text}");
            }
        }
    }

    #[test]
    fn a_chain_without_names_says_so_and_one_not_read_yet_asks() {
        let (_, asked) = crate::views::render(&Store::default(), &mut Light::default(), 1_200.0, |ui, ctx| show(ui, ctx, &mut NamesState::default()));
        assert_eq!(asked, [Ask::NamesList { after: None }]);

        let mut store = Store::default();
        store.failures.insert(Ask::NamesList { after: None }, Failure { status: 404, message: "this chain has no names".to_string() });
        let (text, asked) = crate::views::render(&store, &mut Light::default(), 380.0, |ui, ctx| show(ui, ctx, &mut NamesState::default()));

        assert!(text.contains("This chain keeps no names"), "{text}");
        assert!(asked.is_empty());
    }
}
