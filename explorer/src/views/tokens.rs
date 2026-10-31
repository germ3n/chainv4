//! The chain's token registry: every contract that exports what makes a token on this chain, by
//! symbol, with the name, symbol and decimals each gave when it was deployed or last upgraded.

use {
    super::Ctx,
    crate::api::Ask,
    crate::store::TokenList,
    chainv4_ui::widgets,
    egui::{RichText, Ui},
};

/// How long the first page of a search stands before it is asked for again, in milliseconds.
const REFRESH: u64 = 10_000;

#[derive(Default)]
pub struct TokensState {
    /// The symbol being typed, which the list searches for as it changes.
    typing: String,
}

#[cfg(test)]
impl TokensState {
    /// As it is with this typed.
    pub fn with_typed(typed: &str) -> TokensState {
        TokensState { typing: typed.to_string() }
    }
}

pub fn show(ui: &mut Ui, ctx: &mut Ctx, state: &mut TokensState) {
    let look = ctx.look;
    super::heading(ui, look, "Tokens", "The chain's token registry, by symbol.", |_| {});

    if ctx.settings.read_blocks {
        reading(ui, ctx, state);

        return;
    }

    match ctx.store.status.as_ref().map(|status| status.token_functions.clone()) {
        Some(functions) if functions.is_empty() => {
            super::note(ui, look, "This chain keeps no token registry: its genesis names no functions that make a token.");

            return;
        }
        Some(functions) => {
            super::note(
                ui,
                look,
                format!(
                    "Every contract that exports {} is a token. Name, symbol and decimals are its own, as it answered when deployed or last upgraded; symbols are not unique.",
                    functions.join(", ")
                ),
            );
        }
        None => {}
    }

    ui.add_space(6.0);
    ui.add(egui::TextEdit::singleline(&mut state.typing).hint_text("Symbol starts with…").desired_width(if look.narrow { f32::INFINITY } else { 260.0 }));
    ui.add_space(8.0);
    super::section(ui, look, "Registered", |ui| list(ui, ctx, state.typing.trim()));
}

/// With Read from checked blocks on: the list is the node's, and one token's record can be proven by
/// its address, as someone presses for.
fn reading(ui: &mut Ui, ctx: &mut Ctx, state: &mut TokensState) {
    let look = ctx.look;

    super::section(ui, look, "A token's record", |ui| {
        ui.add(egui::TextEdit::singleline(&mut state.typing).hint_text("A token contract's address").desired_width(if look.narrow { f32::INFINITY } else { 460.0 }));
        ui.add_space(4.0);

        let Ok(address) = state.typing.trim().parse::<chainv4::structs::address::Address>() else {
            return;
        };
        let key = chainv4::registry::tokens::token_key(&address);
        let Some(proven) = super::reading::direct_proof(ui, ctx, "this token's record", chainv4::db::Column::Tokens, &key) else {
            return;
        };

        super::reading::proven_at(ui, ctx, &proven);

        match proven.value.as_deref().map(wincode::deserialize::<chainv4::registry::tokens::Token>) {
            None => super::note(ui, look, "The registry holds no such token."),
            Some(Err(_)) => {
                ui.colored_label(look.palette.bad, "A record that does not read");
            }
            Some(Ok(token)) => super::facts(ui, look, "token record", |facts| {
                facts.text("name", token.name.as_deref().unwrap_or("not given"));
                facts.text("symbol", token.symbol.as_deref().unwrap_or("not given"));
                facts.text("decimals", token.decimals.map_or("not given".to_string(), |decimals| decimals.to_string()));
            }),
        }
    });
    super::reading::turned_off_card(ui, ctx, "The registry's list, and search by symbol, come from the node");
}

/// The first page of tokens whose symbol starts with this, or of every token for "", asking
/// again once it is old.
pub fn first_page<'a>(ctx: &'a mut Ctx, symbol: &str) -> Option<&'a TokenList> {
    let first = Ask::Tokens {
        symbol: symbol.to_string(),
        cursor: None,
    };
    let stale = ctx
        .store
        .token_lists
        .get(symbol)
        .is_none_or(|found| ctx.now.saturating_sub(found.refreshed) > REFRESH);

    if stale {
        ctx.api.ask(first);
    }

    ctx.store.token_lists.get(symbol)
}

fn list(ui: &mut Ui, ctx: &mut Ctx, symbol: &str) {
    let look = ctx.look;
    let Some(found) = first_page(ctx, symbol).cloned() else {
        super::waiting(ui, ctx, &Ask::Tokens { symbol: symbol.to_string(), cursor: None });

        return;
    };

    if found.tokens.is_empty() {
        super::note(ui, look, match symbol {
            "" => "No tokens yet.".to_string(),
            symbol => format!("No symbol starts with {symbol}."),
        });

        return;
    }

    let none = |text: &str| RichText::new(text.to_string()).color(look.palette.muted);

    match look.narrow {
        true => {
            for token in &found.tokens {
                super::list_row(ui, look, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(widgets::strong(token.symbol.as_deref().unwrap_or("none"), 14.0).color(look.palette.ink));
                        ui.label(RichText::new(token.name.as_deref().unwrap_or("—")).color(look.palette.soft));
                    });
                    ui.horizontal(|ui| {
                        super::address(ui, ctx, &token.address);
                        ui.label(none(&token.decimals.map_or("no decimals".to_string(), |decimals| format!("{decimals} decimals"))).small());
                    });
                });
            }
        }
        false => {
            super::table(ui, look, ("tokens", symbol.to_string()), &["symbol", "name", "decimals", "contract"], |ui| {
                for token in &found.tokens {
                    match &token.symbol {
                        Some(symbol) => ui.label(widgets::strong(symbol, 13.5)),
                        None => ui.label(none("none")),
                    };
                    ui.label(token.name.as_deref().unwrap_or("—"));
                    ui.monospace(token.decimals.map_or("—".to_string(), |decimals| decimals.to_string()));
                    super::address(ui, ctx, &token.address);
                    super::row_end(ui);
                }
            });
        }
    }

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if let Some(next) = &found.next {
            let more = Ask::Tokens {
                symbol: symbol.to_string(),
                cursor: Some(next.clone()),
            };

            if ctx.api.asking(&more) {
                ui.spinner();
            } else if widgets::quiet(ui, &look.palette, "More tokens").clicked() {
                ctx.api.ask(more);
            }
        }

        ui.label(none(&format!("{} shown", found.tokens.len())));
    });
}
