//! The chain's token registry: every contract that exports what makes a token on this chain, by
//! symbol, with the name, symbol and decimals each gave when it was deployed or last upgraded.

use {
    super::Ctx,
    crate::api::Ask,
    crate::store::TokenList,
    egui::Ui,
};

/// How long the first page of a search stands before it is asked for again, in milliseconds.
const REFRESH: u64 = 10_000;

#[derive(Default)]
pub struct TokensState {
    /// The symbol being typed, which the list searches for as it changes.
    typing: String,
}

pub fn show(ui: &mut Ui, ctx: &mut Ctx, state: &mut TokensState) {
    super::title(ui, "Tokens");

    match ctx.store.status.as_ref().map(|status| status.token_functions.clone()) {
        Some(functions) if functions.is_empty() => {
            ui.weak("This chain keeps no token registry: its genesis names no functions that make a token.");

            return;
        }
        Some(functions) => {
            ui.weak(format!(
                "Every contract that exports {} is a token. Name, symbol and decimals are its own, as it answered when deployed or last upgraded; symbols are not unique.",
                functions.join(", ")
            ));
        }
        None => {}
    }

    ui.horizontal(|ui| {
        ui.weak("Symbol");
        ui.add(egui::TextEdit::singleline(&mut state.typing).hint_text("any").desired_width(160.0));
    });
    ui.add_space(4.0);
    list(ui, ctx, state.typing.trim());
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

    if stale && !ctx.store.failures.contains_key(&first) {
        ctx.api.ask(first);
    }

    ctx.store.token_lists.get(symbol)
}

fn list(ui: &mut Ui, ctx: &mut Ctx, symbol: &str) {
    let Some(found) = first_page(ctx, symbol).cloned() else {
        super::waiting(ui, ctx, &Ask::Tokens { symbol: symbol.to_string(), cursor: None });

        return;
    };

    if found.tokens.is_empty() {
        ui.weak(match symbol {
            "" => "No tokens yet.".to_string(),
            symbol => format!("No symbol starts with {symbol}."),
        });

        return;
    }

    egui::Grid::new(("tokens", symbol.to_string())).num_columns(4).striped(true).show(ui, |ui| {
        for heading in ["symbol", "decimals", "contract", "name"] {
            ui.strong(heading);
        }

        ui.end_row();

        for token in &found.tokens {
            match &token.symbol {
                Some(symbol) => ui.strong(symbol),
                None => ui.weak("none"),
            };
            ui.label(token.decimals.map_or("—".to_string(), |decimals| decimals.to_string()));
            super::address(ui, ctx, &token.address);
            ui.label(token.name.as_deref().unwrap_or("—"));
            ui.end_row();
        }
    });

    ui.horizontal(|ui| {
        if let Some(next) = &found.next {
            let more = Ask::Tokens {
                symbol: symbol.to_string(),
                cursor: Some(next.clone()),
            };

            if ctx.api.asking(&more) {
                ui.spinner();
            } else if ui.button("More tokens").clicked() {
                ctx.api.ask(more);
            }
        }

        ui.weak(format!("{} shown", found.tokens.len()));
    });
}
