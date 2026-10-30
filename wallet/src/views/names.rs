//! The account's names, such as alice.dev: finding one and registering it, what each it owns
//! points at, its records and subnames, renewing them, giving them away, and which one an address
//! goes by. Everything shown is proven: each name's records, and every one above it, against the
//! tip the light client checked. Only the list of names to look at, and their prices, come from
//! the node as it says them.
//!
//! What a name transaction does is signed and sent on the Send page, as any other.

use {
    super::send::{Prepared, SendState, recipient},
    super::{Ctx, Page},
    crate::chain::Resolved,
    crate::format,
    crate::theme,
    chainv4::registry::names::{NameAction, NameStatus, NamesConfig, name_problem, price},
    chainv4::structs::address::Address,
    chainv4::structs::tx::Tx,
    chainv4::structs::u256::u256,
    egui::{RichText, Ui},
};

#[derive(Default)]
pub struct NamesState {
    /// The address whose names show: one of the account's, the first until another is picked.
    from: Option<Address>,
    /// A name to find, and register if nobody holds it.
    search: String,
    years: String,
    /// The name whose actions are open, and what they are filled in with.
    open: Option<String>,
    renew: String,
    target: String,
    key: String,
    value: String,
    label: String,
    owner: String,
    give_to: String,
    trouble: Option<String>,
}

impl NamesState {
    /// The names this page wants proven besides the account's own: the one searched for.
    pub fn wanted(&self) -> Option<String> {
        let search = self.search.trim();

        (!search.is_empty()).then(|| search.to_string())
    }
}

pub fn show(ui: &mut Ui, ctx: &mut Ctx, state: &mut NamesState, send: &mut SendState) {
    let palette = *ctx.palette;
    theme::title(ui, &palette, "Names", "Names such as alice.dev point at an address. Each is proven before it shows.");

    let Some(names) = ctx.chain.names.clone() else {
        theme::card(ui, &palette, |ui| {
            let text = match ctx.chain.has_names {
                Some(false) => "This chain has no names.",
                _ => "Asking the node about the chain's names…",
            };
            ui.label(RichText::new(text).color(palette.muted));
        });

        return;
    };
    let addresses = ctx.holder().map(|holder| holder.addresses()).unwrap_or_default();

    if !state.from.is_some_and(|from| addresses.contains(&from)) {
        state.from = addresses.first().copied();
    }

    let Some(from) = state.from else {
        return;
    };

    if addresses.len() > 1 {
        theme::card(ui, &palette, |ui| {
            theme::caps(ui, &palette, "Address");
            egui::ComboBox::from_id_salt("names-from")
                .selected_text(format!("{} · {}", super::scheme(&from), format::short(&from.to_string())))
                .width(ui.available_width())
                .show_ui(ui, |ui| {
                    for address in &addresses {
                        ui.selectable_value(&mut state.from, Some(*address), format!("{} · {}", super::scheme(address), format::short(&address.to_string())));
                    }
                });
        });
    }

    let mut prepared = None;

    primary(ui, ctx, &from, &mut prepared);
    find(ui, ctx, state, &names, &from, &mut prepared);
    owned(ui, ctx, state, &names, &from, &mut prepared);

    if let Some(prepared) = prepared {
        send.prepare(prepared);
        *ctx.page = Page::Send;
    }
}

/// The name the address goes by, and clearing it.
fn primary(ui: &mut Ui, ctx: &mut Ctx, from: &Address, prepared: &mut Option<Prepared>) {
    let palette = *ctx.palette;
    let address = from.to_string();

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Goes by");
        ui.add_space(4.0);

        match ctx.chain.primary(&address) {
            Some(name) => {
                theme::end_row(
                    ui,
                    |ui| {
                        ui.label(RichText::new(&name).color(palette.ink).size(18.0));
                    },
                    |ui| {
                        if theme::quiet(ui, &palette, "Clear").clicked() {
                            *prepared = Some(Prepared {
                                from: *from,
                                tx: Tx::PrimaryName { name: None },
                                what: "Clear the primary name".to_string(),
                                about: name.clone(),
                                coin: u256::ZERO,
                            });
                        }
                    },
                );
            }
            None => {
                ui.label(RichText::new("No name: make one of this address's names its primary name below, and others see it by that name.").color(palette.muted));
            }
        }
    });
}

/// Finding a name: who holds it, or what registering it costs.
fn find(ui: &mut Ui, ctx: &mut Ctx, state: &mut NamesState, names: &NamesConfig, from: &Address, prepared: &mut Option<Prepared>) {
    let palette = *ctx.palette;

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Find or register");
        ui.add(egui::TextEdit::singleline(&mut state.search).hint_text(format!("such as alice.{}", names.suffix)).desired_width(f32::INFINITY));

        let search = state.search.trim().to_string();

        if search.is_empty() {
            return;
        }

        let name = names.strip(&search);

        if let Some(problem) = name_problem(&name) {
            ui.colored_label(palette.bad, problem);

            return;
        }

        let shown = names.display(&name);

        match ctx.chain.resolve(&name) {
            None => {
                ui.label(RichText::new(format!("Proving {shown}…")).small().color(palette.muted));
            }
            Some(Resolved { found: Some(found), assurance }) => {
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    super::assurance(ui, &palette, &assurance);
                    ui.label(RichText::new(format!("{shown} is {}", status(found.status))).color(palette.soft));
                });
                super::row(ui, &palette, "Owner", |ui| super::address(ui, &palette, &found.record.owner.to_string()));
                super::row(ui, &palette, "Points at", |ui| match found.record.target {
                    Some(target) => super::address(ui, &palette, &target.to_string()),
                    None => {
                        ui.label(RichText::new("nothing").color(palette.muted));
                    }
                });
                super::row(ui, &palette, "Expires", |ui| {
                    ui.label(format::utc(found.expires));
                });

                for text in &found.record.texts {
                    super::row(ui, &palette, &text.key, |ui| {
                        ui.label(RichText::new(&text.value).color(palette.ink));
                    });
                }
            }
            Some(Resolved { found: None, assurance }) if !name.contains('.') => {
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    super::assurance(ui, &palette, &assurance);
                    ui.label(RichText::new(format!("{shown} is available")).color(palette.good));
                });
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("Years").color(palette.soft));
                    ui.add(egui::TextEdit::singleline(&mut state.years).hint_text("1").desired_width(48.0));
                });
                let years = years_of(&state.years);
                let cost = years.and_then(|years| price(&names.prices, &name, years));

                match (years, cost) {
                    (Some(years), Some(cost)) => {
                        ui.label(
                            RichText::new(format!("{} a year, {} for {years}, burned. Prices as the node says them: the chain burns what it charges.", ctx.amount(None, &(cost / u256::from(years)).to_string()), ctx.amount(None, &cost.to_string())))
                                .small()
                                .color(palette.muted),
                        );

                        if theme::primary(ui, &palette, "Register", true).clicked() {
                            *prepared = Some(Prepared {
                                from: *from,
                                tx: Tx::Name { name: name.clone(), action: NameAction::Register { years } },
                                what: format!("Register {shown}"),
                                about: format!("{years} year{}", if years == 1 { "" } else { "s" }),
                                coin: cost,
                            });
                            state.search.clear();
                        }
                    }
                    _ => {
                        ui.colored_label(palette.bad, format!("A name is held 1 to {} years.", names.max_years));
                    }
                }
            }
            Some(Resolved { found: None, .. }) => {
                ui.label(RichText::new(format!("Nobody holds {shown}: a subname is made by its name's owner, for free.")).color(palette.muted));
            }
        }
    });
}

/// The names the address owns, each with what can be done with it.
fn owned(ui: &mut Ui, ctx: &mut Ctx, state: &mut NamesState, names: &NamesConfig, from: &Address, prepared: &mut Option<Prepared>) {
    let palette = *ctx.palette;
    let address = from.to_string();
    let listed = ctx.chain.owned.get(&address).cloned().unwrap_or_default();

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Owned");

        if listed.is_empty() {
            ui.label(RichText::new("This address owns no names yet.").color(palette.muted));
        }

        for name in &listed {
            let shown = names.display(name);
            let Some(resolved) = ctx.chain.resolve(name) else {
                ui.label(RichText::new(format!("Proving {shown}…")).small().color(palette.muted));
                continue;
            };
            // The node lists a name under its owner until someone else registers it; proven, only
            // those still this address's show.
            let Some(found) = resolved.found.as_ref().filter(|found| found.record.owner == *from) else {
                continue;
            };

            ui.add_space(6.0);
            ui.separator();
            let open = state.open.as_deref() == Some(name.as_str());
            let said = format!("{} · expires {}", status(found.status), format::utc(found.expires));
            theme::end_row(
                ui,
                |ui| {
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            super::assurance(ui, &palette, &resolved.assurance);
                            ui.label(RichText::new(&shown).color(palette.ink).size(16.0));
                        });
                        ui.add(egui::Label::new(RichText::new(&said).small().color(palette.muted)).wrap());
                    });
                },
                |ui| {
                    if theme::quiet(ui, &palette, if open { "Close" } else { "Manage" }).clicked() {
                        state.open = (!open).then(|| name.clone());
                        state.trouble = None;
                    }
                },
            );
            super::row(ui, &palette, "Points at", |ui| match found.record.target {
                Some(target) => super::address(ui, &palette, &target.to_string()),
                None => {
                    ui.label(RichText::new("nothing").color(palette.muted));
                }
            });

            if open {
                *prepared = prepared.take().or_else(|| manage(ui, ctx, state, names, from, name, found));
            }
        }
    });
}

/// What can be done with one name: each a transaction to send.
fn manage(ui: &mut Ui, ctx: &mut Ctx, state: &mut NamesState, names: &NamesConfig, from: &Address, name: &str, found: &chainv4::registry::names::FoundName) -> Option<Prepared> {
    let palette = *ctx.palette;
    let shown = names.display(name);
    let top = !name.contains('.');
    let prepare = |action: NameAction, what: String, coin: u256| Prepared {
        from: *from,
        tx: Tx::Name { name: name.to_string(), action },
        what,
        about: shown.clone(),
        coin,
    };
    let mut chosen = None;

    ui.add_space(6.0);

    if top {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Renew for").color(palette.soft));
            ui.add(egui::TextEdit::singleline(&mut state.renew).hint_text("1").desired_width(48.0));
            ui.label(RichText::new("years").color(palette.soft));
            let years = years_of(&state.renew);
            let cost = years.and_then(|years| price(&names.prices, name, years));

            if let (Some(years), Some(cost)) = (years, cost)
                && theme::quiet(ui, &palette, &format!("Renew · {}", ctx.brief(None, &cost.to_string()))).clicked()
            {
                chosen = Some(prepare(NameAction::Renew { years }, format!("Renew {shown}"), cost));
            }
        });
    }

    if found.record.target == Some(*from) && ctx.chain.primary(&from.to_string()).as_deref() != Some(shown.as_str()) && theme::quiet(ui, &palette, "Make it this address's name").clicked() {
        chosen = Some(Prepared {
            from: *from,
            tx: Tx::PrimaryName { name: Some(name.to_string()) },
            what: format!("Go by {shown}"),
            about: shown.clone(),
            coin: u256::ZERO,
        });
    }

    let field = |ui: &mut Ui, text: &mut String, hint: &str| {
        ui.add(egui::TextEdit::singleline(text).hint_text(hint).desired_width(220.0));
    };

    ui.horizontal_wrapped(|ui| {
        field(ui, &mut state.target, "point at: ed25519:0x…, or empty for this address");

        if theme::quiet(ui, &palette, "Point").clicked() {
            match state.target.trim() {
                "" => chosen = Some(prepare(NameAction::Target { target: Some(*from) }, format!("Point {shown} here"), u256::ZERO)),
                text => match recipient(text) {
                    Some(target) => chosen = Some(prepare(NameAction::Target { target: Some(target) }, format!("Point {shown} at {}", format::short(&target.to_string())), u256::ZERO)),
                    None => state.trouble = Some("That is not an address.".to_string()),
                },
            }
        }
    });

    ui.horizontal_wrapped(|ui| {
        field(ui, &mut state.key, "text record: url, avatar, description…");
        ui.add(egui::TextEdit::singleline(&mut state.value).hint_text("its value; empty clears it").desired_width(220.0));

        if theme::quiet(ui, &palette, "Set").clicked() {
            let (key, value) = (state.key.trim().to_string(), state.value.trim().to_string());
            let action = NameAction::Text { key: key.clone(), value: (!value.is_empty()).then_some(value) };

            match action.problem() {
                Some(problem) => state.trouble = Some(problem),
                None => chosen = Some(prepare(action, format!("Set {key} on {shown}"), u256::ZERO)),
            }
        }
    });

    ui.horizontal_wrapped(|ui| {
        field(ui, &mut state.label, "subname: pay, for pay.alice…");
        ui.add(egui::TextEdit::singleline(&mut state.owner).hint_text("its owner; empty for this address").desired_width(220.0));

        if theme::quiet(ui, &palette, "Make").clicked() {
            let owner = match state.owner.trim() {
                "" => Some(*from),
                text => recipient(text),
            };
            let label = state.label.trim().to_string();

            match owner {
                Some(owner) => {
                    let action = NameAction::Subname { label: label.clone(), owner };

                    match action.problem() {
                        Some(problem) => state.trouble = Some(problem),
                        None => chosen = Some(prepare(action, format!("Make {label}.{shown}"), u256::ZERO)),
                    }
                }
                None => state.trouble = Some("The owner is not an address.".to_string()),
            }
        }
    });

    ui.horizontal_wrapped(|ui| {
        field(ui, &mut state.give_to, "give it to: ed25519:0x…");

        if theme::quiet(ui, &palette, "Give").clicked() {
            match recipient(&state.give_to) {
                Some(to) => chosen = Some(prepare(NameAction::Transfer { to }, format!("Give {shown} to {}", format::short(&to.to_string())), u256::ZERO)),
                None => state.trouble = Some("That is not an address.".to_string()),
            }
        }
    });

    if let Some(why) = &state.trouble {
        ui.colored_label(palette.bad, why.as_str());
    }

    chosen
}

fn status(status: NameStatus) -> &'static str {
    match status {
        NameStatus::Active => "active",
        NameStatus::Grace => "expired, in its grace period: only its owner may renew it",
        NameStatus::Available => "available",
    }
}

/// Whole years, as typed: 1 when nothing is.
fn years_of(text: &str) -> Option<u32> {
    match text.trim() {
        "" => Some(1),
        text => text.parse().ok().filter(|years| *years > 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn years_are_whole_and_one_unless_typed() {
        assert_eq!(years_of(""), Some(1));
        assert_eq!(years_of(" 3 "), Some(3));
        assert_eq!(years_of("0"), None);
        assert_eq!(years_of("two"), None);
    }
}
