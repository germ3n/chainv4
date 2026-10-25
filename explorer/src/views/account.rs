//! An address: its balance and nonce, and for a contract, its code, its functions and a way to
//! call them read-only. A token shows its name, symbol and decimals as the chain's registry holds
//! them, and its decimals can be set by hand; every account shows what it holds of the registered
//! tokens: the registry's first page, by symbol, and any other the explorer has met. With the
//! node's address index, it lists every transaction that touched the address.

use {
    super::{Ctx, TxLine, events, tokens},
    crate::api::{Ask, EventQuery},
    crate::format,
    crate::model::{Called, RegisteredToken},
    chainv4_ui::widgets,
    egui::{RichText, Ui},
    serde_json::json,
};

/// The argument types a read-only call can be given, as the API names them.
const KINDS: [&str; 12] = ["address", "amount", "u256", "u128", "u64", "i64", "i128", "number", "bool", "string", "bytes", "nil"];

#[derive(Default)]
pub struct AccountState {
    /// The address these are for: another one starts afresh.
    address: String,
    /// The event name being typed, and the one the list shows.
    typing: String,
    named: String,
    function: String,
    args: Vec<(String, String)>,
    /// The call last made.
    asked: Option<Ask>,
}

pub fn show(ui: &mut Ui, ctx: &mut Ctx, address: &str, state: &mut AccountState) {
    if state.address != address {
        *state = AccountState {
            address: address.to_string(),
            ..AccountState::default()
        };
    }

    let look = ctx.look;
    let store = ctx.store;

    if !store.accounts.contains_key(address) {
        ctx.api.ask(Ask::Account(address.to_string()));
    }

    if address.starts_with("contract:") && !store.contracts.contains_key(address) {
        ctx.api.ask(Ask::Contract(address.to_string()));
    }

    let contract = store.contracts.get(address).cloned().flatten();
    let kind = address.split(':').next().unwrap_or_default();
    let title = match &contract {
        Some(contract) if contract.token.is_some() => "Token contract",
        Some(_) => "Contract",
        None => "Account",
    };

    super::heading(ui, look, title, "", |ui| {
        widgets::pill(ui, kind, look.palette.soft);
    });
    super::copyable(ui, address);
    ui.add_space(10.0);

    super::section(ui, look, "Balance", |ui| match store.accounts.get(address) {
        Some(account) => {
            widgets::fitted(ui, &ctx.coins(&account.balance), 24.0, 14.0, egui::FontFamily::Monospace, look.palette.ink)
                .on_hover_text(format!("{} units", account.balance));
            ui.label(RichText::new(format!("nonce {}: transactions sent so far, and the nonce the next one takes", account.nonce)).small().color(look.palette.muted));
            ui.add_space(6.0);

            match ctx.light.proven.get(address) {
                None => super::note(ui, look, "Not proven yet: the light client is still checking the chain."),
                Some(Err(why)) => {
                    ui.colored_label(look.palette.bad, format!("The node's proof does not check: {why}"));
                }
                Some(Ok(proven)) => {
                    // The newest final, and the newest at the tip when that is newer.
                    let newest = |final_: bool| proven.iter().filter(|proven| proven.assurance.final_ == final_).max_by_key(|proven| proven.assurance.height);
                    let shown: Vec<_> = [newest(true), newest(false)]
                        .into_iter()
                        .flatten()
                        .filter(|shown| shown.assurance.final_ || newest(true).is_none_or(|final_| shown.assurance.height > final_.assurance.height))
                        .collect();

                    for proven in shown {
                        let (word, color) = match proven.assurance.final_ {
                            true => ("final", look.roles.final_),
                            false => ("proven, not final yet", look.roles.side),
                        };
                        ui.horizontal_wrapped(|ui| {
                            widgets::pill(ui, word, color);
                            ui.label(RichText::new(ctx.coins(&proven.balance)).monospace());
                            ui.label(
                                RichText::new(format!("nonce {}, at block {}", proven.nonce, format::thousands(&proven.assurance.height.to_string())))
                                    .small()
                                    .color(look.palette.muted),
                            );
                        })
                        .response
                        .on_hover_text("What the light client proved, checking the node's proofs against headers it checked itself, trusting only the genesis it first saw at this node");
                    }
                }
            }
        }
        None => super::waiting(ui, ctx, &Ask::Account(address.to_string())),
    });

    if let Some(contract) = &contract {
        super::section(ui, look, "Contract", |ui| {
            super::facts(ui, look, "contract", |facts| {
                facts.row("deployer", |ui| super::address(ui, ctx, &contract.deployer));
                facts.text("engine", contract.engine.clone());
                facts.text("code", format!("{} bytes", format::thousands(&contract.code_size.to_string())));
                facts.text("upgradable", if contract.upgradable { "yes, it exports check_perms" } else { "no" });
                facts.row("functions", |ui| {
                    for function in &contract.functions {
                        let text = RichText::new(function).monospace();

                        match contract.reentrant.contains(function) {
                            true => ui.label(text.color(look.roles.justified)).on_hover_text("reentrant"),
                            false => ui.label(text),
                        };
                    }
                });

                if !contract.layout.is_empty() {
                    facts.row("layout", |ui| {
                        ui.vertical(|ui| {
                            let retired = || RichText::new("retired").color(look.roles.side);

                            for declared in &contract.layout {
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new(&declared.name).monospace());

                                    match (declared.shape.as_str(), &declared.kind) {
                                        ("map", Some(kind)) => ui.label(RichText::new(format!("map of {kind}")).color(look.palette.muted)),
                                        ("record", _) => ui.label(RichText::new("record").color(look.palette.muted)),
                                        _ => ui.label(retired()),
                                    };
                                });

                                for field in &declared.keys {
                                    ui.horizontal(|ui| {
                                        ui.add_space(16.0);
                                        ui.label(RichText::new(&field.key).monospace());

                                        match &field.kind {
                                            Some(kind) => ui.label(RichText::new(kind).color(look.palette.muted)),
                                            None => ui.label(retired()),
                                        };
                                    });
                                }
                            }
                        });
                    });
                }
            });
        });

        if contract.token.is_some() {
            token(ui, ctx, address);
        }

        super::section(ui, look, "Call a function read-only", |ui| read(ui, ctx, address, &contract.functions, state));
        let named = state.named.clone();
        super::section_with(
            ui,
            look,
            "Its events",
            |ui| {
                let field = ui.add(egui::TextEdit::singleline(&mut state.typing).hint_text("any name").desired_width(120.0));

                if field.lost_focus() {
                    state.named = state.typing.trim().to_string();
                }
            },
            |ui| events::list(ui, ctx, &EventQuery::contract(address, Some(&named))),
        );
    }

    holdings(ui, ctx, address);
    super::section(ui, look, "Transactions", |ui| transactions(ui, ctx, address));
    super::section(ui, look, "Events that mention it", |ui| events::list(ui, ctx, &EventQuery::mentioning(address)));
}

/// How long the newest page of transactions stands before it is asked for again, in milliseconds.
const REFRESH: u64 = 3_000;

/// The transactions that touched an address, newest first, from the node's address index: those
/// it sent, those that paid it or named it, a page at a time.
fn transactions(ui: &mut Ui, ctx: &mut Ctx, address: &str) {
    let store = ctx.store;
    let newest = Ask::AccountTxs {
        address: address.to_string(),
        cursor: None,
    };
    let found = store.account_txs.get(address);
    let failed = store.failures.get(&newest);

    if failed.is_none() && found.is_none_or(|found| ctx.now.saturating_sub(found.refreshed) > REFRESH) {
        ctx.api.ask(newest.clone());
    }

    let look = ctx.look;
    let found = match (found, failed) {
        (_, Some(failure)) if failure.status == 404 => {
            super::note(ui, look, "This node keeps no address index. Start it with --index-addresses to list an address's transactions.");

            return;
        }
        (Some(found), _) => found,
        (None, _) => {
            super::waiting(ui, ctx, &newest);

            return;
        }
    };
    let still = found
        .catching_up_to
        .filter(|to| *to < found.indexed_from)
        .map(|to| format!(" It is still indexing older blocks, down to height {}.", format::thousands(&to.to_string())))
        .unwrap_or_default();

    if found.items.is_empty() {
        super::note(
            ui,
            look,
            format!("None since height {}, where this node's index starts.{still}", format::thousands(&found.indexed_from.to_string())),
        );

        return;
    }

    let lines: Vec<TxLine> = found
        .items
        .iter()
        .map(|found| TxLine {
            tx: &found.tx,
            index: None,
            block: Some((found.block.as_str(), found.height)),
        })
        .collect();
    super::tx_list(ui, ctx, &format!("account_txs {address}"), &lines);

    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        if let Some(next) = &found.next {
            let older = Ask::AccountTxs {
                address: address.to_string(),
                cursor: Some(next.clone()),
            };

            if ctx.api.asking(&older) {
                ui.spinner();
            } else if widgets::quiet(ui, &look.palette, "Older transactions").clicked() {
                ctx.api.ask(older);
            }
        }

        ui.label(
            RichText::new(format!("{} shown, indexed from height {}.{still}", found.items.len(), format::thousands(&found.indexed_from.to_string())))
                .small()
                .color(look.palette.muted),
        );
    });
}

/// A token's name, symbol and decimals as the chain's registry holds them, and decimals set by
/// hand.
fn token(ui: &mut Ui, ctx: &mut Ctx, contract: &str) {
    let Some(token) = ctx.store.tokens.get(contract).cloned() else {
        return;
    };
    let look = ctx.look;
    let set = ctx.decimals_set(contract);

    super::section(ui, look, "Token", |ui| {
        super::facts(ui, look, "token", |facts| {
            facts.text("name", token.name.as_deref().unwrap_or("not given"));
            facts.text("symbol", token.symbol.as_deref().unwrap_or("not given"));
            facts.row("decimals", |ui| {
                ui.label(token.decimals.map_or("not given".to_string(), |decimals| format!("{decimals}, as the token says")));
                let mut chosen = set.or(token.decimals).unwrap_or(18);
                let mut by_hand = set.is_some();

                if ui.checkbox(&mut by_hand, "set by hand").changed() {
                    ctx.set_decimals(contract, by_hand.then_some(chosen));
                }

                if by_hand && ui.add(egui::DragValue::new(&mut chosen).range(0..=77)).changed() {
                    ctx.set_decimals(contract, Some(chosen));
                }
            });
        });
    });
}

/// What the address holds of the registered tokens: the registry's first page, by symbol, and any
/// other token met. Tokens it holds none of are left out.
fn holdings(ui: &mut Ui, ctx: &mut Ctx, holder: &str) {
    tokens::first_page(ctx, "");
    let look = ctx.look;
    let mut known: Vec<RegisteredToken> = ctx.store.tokens.values().cloned().collect();
    known.sort_by_key(|token| (token.symbol.as_ref().map(|symbol| symbol.to_ascii_lowercase()), token.address.clone()));

    if known.is_empty() {
        return;
    }

    let checked = known.len();
    let held: Vec<(String, Option<String>)> = known
        .into_iter()
        .map(|token| {
            let raw = ctx.holding(&token.address, holder);

            (token.address, raw)
        })
        .filter(|(_, raw)| raw.as_deref() != Some("0"))
        .collect();

    super::section(ui, look, "Token holdings", |ui| {
        if held.is_empty() {
            super::note(ui, look, format!("It holds none of the {checked} registered tokens checked."));

            return;
        }

        for (token, raw) in held {
            ui.horizontal(|ui| {
                let amount = match (raw, ctx.token(&token)) {
                    (Some(raw), Some((decimals, symbol))) => format::amount(&raw, decimals, &symbol),
                    (Some(raw), None) => format!("{raw} units"),
                    (None, _) => "…".to_string(),
                };
                ui.label(RichText::new(amount).monospace().color(look.palette.ink));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| super::address(ui, ctx, &token));
            });
            widgets::rule(ui, &look.palette);
        }
    });
}

/// Calls one of the contract's functions read-only, as the API's POST /call does.
fn read(ui: &mut Ui, ctx: &mut Ctx, contract: &str, functions: &[String], state: &mut AccountState) {
    let look = ctx.look;
    super::note(ui, look, "It runs on a copy of the chain's state that is thrown away: nothing it writes or sends is kept.");
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        egui::ComboBox::from_id_salt("function")
            .selected_text(if state.function.is_empty() { "function" } else { &state.function })
            .show_ui(ui, |ui| {
                for function in functions {
                    ui.selectable_value(&mut state.function, function.clone(), function);
                }
            });

        if widgets::quiet(ui, &look.palette, "+ argument").clicked() {
            state.args.push(("address".to_string(), String::new()));
        }
    });

    let mut remove = None;

    for (index, (kind, value)) in state.args.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt(("kind", index)).width(90.0).selected_text(kind.as_str()).show_ui(ui, |ui| {
                for option in KINDS {
                    ui.selectable_value(kind, option.to_string(), option);
                }
            });

            if ui.small_button("×").clicked() {
                remove = Some(index);
            }

            ui.add(egui::TextEdit::singleline(value).desired_width(ui.available_width()));
        });
    }

    if let Some(index) = remove {
        state.args.remove(index);
    }

    ui.add_space(4.0);

    if widgets::primary(ui, &look.palette, "Call", !state.function.is_empty()).clicked() {
        let args = json!(state.args.iter().map(|(kind, value)| json!({ "type": kind, "value": value })).collect::<Vec<_>>());
        let ask = Ask::Call {
            contract: contract.to_string(),
            function: state.function.clone(),
            args: args.to_string(),
        };
        ctx.api.ask(ask.clone());
        state.asked = Some(ask);
    }

    let Some(ask) = &state.asked else {
        return;
    };

    if let Some(failure) = ctx.store.failures.get(ask) {
        ui.colored_label(look.palette.bad, &failure.message);
    } else if let Some(called) = ctx.store.calls.get(ask).and_then(|value| serde_json::from_value::<Called>(value.clone()).ok()) {
        match called.status.as_str() {
            "returned" => {
                ui.horizontal_wrapped(|ui| {
                    widgets::pill(ui, "returned", look.palette.good);

                    for value in &called.values {
                        super::argument(ui, ctx, value);
                    }
                });
            }
            _ => {
                ui.colored_label(look.palette.bad, called.error.as_deref().unwrap_or("failed"));
            }
        }
    } else if ctx.api.asking(ask) {
        ui.spinner();
    }
}
