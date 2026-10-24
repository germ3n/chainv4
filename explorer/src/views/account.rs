//! An address: its balance and nonce, and for a contract, its code, its functions and a way to
//! call them read-only. A token shows its name, symbol and decimals, whose decimals can be set by
//! hand; every account shows what it holds of the tokens the explorer has met.

use {
    super::{BAD, Ctx, GOOD},
    crate::api::Ask,
    crate::format,
    crate::model::Called,
    egui::{RichText, Ui},
    serde_json::json,
};

/// The argument types a read-only call can be given, as the API names them.
const KINDS: [&str; 12] = ["address", "amount", "u256", "u128", "u64", "i64", "i128", "number", "bool", "string", "bytes", "nil"];

#[derive(Default)]
pub struct AccountState {
    /// The address these are for: another one starts afresh.
    address: String,
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

    let store = ctx.store;

    if !store.accounts.contains_key(address) {
        ctx.api.ask(Ask::Account(address.to_string()));
    }

    if address.starts_with("contract:") && !store.contracts.contains_key(address) {
        ctx.api.ask(Ask::Contract(address.to_string()));
    }

    super::title(ui, if address.starts_with("contract:") { "Contract" } else { "Account" });
    super::copyable(ui, address);

    match store.accounts.get(address) {
        Some(account) => super::facts(ui, "account", |ui| {
            super::fact(ui, "balance");
            ui.label(RichText::new(ctx.coins(&account.balance)).strong()).on_hover_text(format!("{} units", account.balance));
            ui.end_row();
            super::fact(ui, "nonce");
            ui.label(account.nonce.to_string()).on_hover_text("Transactions sent so far, and the nonce the next one takes");
            ui.end_row();
        }),
        None => super::waiting(ui, ctx, &Ask::Account(address.to_string())),
    }

    if let Some(Some(contract)) = store.contracts.get(address) {
        ui.add_space(6.0);
        super::facts(ui, "contract", |ui| {
            super::fact(ui, "deployer");
            super::address(ui, ctx, &contract.deployer);
            ui.end_row();
            super::fact(ui, "engine");
            ui.label(&contract.engine);
            ui.end_row();
            super::fact(ui, "code");
            ui.label(format!("{} bytes", format::thousands(&contract.code_size.to_string())));
            ui.end_row();
            super::fact(ui, "upgradable");
            ui.label(if contract.upgradable { "yes, it exports check_perms" } else { "no" });
            ui.end_row();
            super::fact(ui, "functions");
            ui.horizontal_wrapped(|ui| {
                for function in &contract.functions {
                    let reentrant = contract.reentrant.contains(function);
                    let text = RichText::new(function).monospace();

                    match reentrant {
                        true => ui.label(text.color(super::JUSTIFIED)).on_hover_text("reentrant"),
                        false => ui.label(text),
                    };
                }
            });
            ui.end_row();
        });

        if contract.is_token() {
            token(ui, ctx, address);
        }

        ui.add_space(6.0);
        read(ui, ctx, address, &contract.functions, state);
    }

    holdings(ui, ctx, address);
}

/// A token's own name, symbol and decimals, and decimals set by hand.
fn token(ui: &mut Ui, ctx: &mut Ctx, contract: &str) {
    ctx.token(contract);
    let token = ctx.store.tokens.get(contract).cloned().unwrap_or_default();
    let set = ctx.decimals_set(contract);

    ui.add_space(6.0);
    ui.strong("Token");
    super::facts(ui, "token", |ui| {
        super::fact(ui, "name");
        ui.label(token.name.as_deref().unwrap_or("…"));
        ui.end_row();
        super::fact(ui, "symbol");
        ui.label(token.symbol.as_deref().unwrap_or("…"));
        ui.end_row();
        super::fact(ui, "decimals");
        ui.horizontal(|ui| {
            ui.label(token.decimals.map_or("…".to_string(), |decimals| format!("{decimals}, as the token says")));
            let mut chosen = set.or(token.decimals).unwrap_or(18);
            let mut by_hand = set.is_some();

            if ui.checkbox(&mut by_hand, "set by hand").changed() {
                ctx.set_decimals(contract, by_hand.then_some(chosen));
            }

            if by_hand && ui.add(egui::DragValue::new(&mut chosen).range(0..=77)).changed() {
                ctx.set_decimals(contract, Some(chosen));
            }
        });
        ui.end_row();
    });
}

/// What the address holds of every token met so far.
fn holdings(ui: &mut Ui, ctx: &mut Ctx, holder: &str) {
    let tokens: Vec<String> = ctx
        .store
        .contracts
        .iter()
        .filter(|(_, contract)| contract.as_ref().is_some_and(|contract| contract.is_token()))
        .map(|(address, _)| address.clone())
        .collect();

    if tokens.is_empty() {
        return;
    }

    ui.add_space(6.0);
    ui.strong("Token holdings");
    egui::Grid::new("holdings").num_columns(2).striped(true).show(ui, |ui| {
        for token in tokens {
            super::address(ui, ctx, &token);

            match (ctx.holding(&token, holder), ctx.token(&token)) {
                (Some(raw), Some((decimals, symbol))) => ui.label(format::amount(&raw, decimals, &symbol)),
                (Some(raw), None) => ui.label(format!("{raw} units")),
                (None, _) => ui.weak("…"),
            };

            ui.end_row();
        }
    });
}

/// Calls one of the contract's functions read-only, as the API's POST /call does.
fn read(ui: &mut Ui, ctx: &mut Ctx, contract: &str, functions: &[String], state: &mut AccountState) {
    egui::CollapsingHeader::new("Call a function read-only").show(ui, |ui| {
        ui.weak("It runs on a copy of the chain's state that is thrown away: nothing it writes or sends is kept.");
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("function")
                .selected_text(if state.function.is_empty() { "function" } else { &state.function })
                .show_ui(ui, |ui| {
                    for function in functions {
                        ui.selectable_value(&mut state.function, function.clone(), function);
                    }
                });

            if ui.button("+ argument").clicked() {
                state.args.push(("address".to_string(), String::new()));
            }
        });

        let mut remove = None;

        for (index, (kind, value)) in state.args.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt(("kind", index)).selected_text(kind.as_str()).show_ui(ui, |ui| {
                    for option in KINDS {
                        ui.selectable_value(kind, option.to_string(), option);
                    }
                });
                ui.text_edit_singleline(value);

                if ui.small_button("x").clicked() {
                    remove = Some(index);
                }
            });
        }

        if let Some(index) = remove {
            state.args.remove(index);
        }

        if ui.add_enabled(!state.function.is_empty(), egui::Button::new("Call")).clicked() {
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
            ui.colored_label(BAD, &failure.message);
        } else if let Some(called) = ctx.store.calls.get(ask).and_then(|value| serde_json::from_value::<Called>(value.clone()).ok()) {
            match called.status.as_str() {
                "returned" => {
                    ui.horizontal_wrapped(|ui| {
                        ui.colored_label(GOOD, "returned");

                        for value in &called.values {
                            super::argument(ui, ctx, value);
                        }
                    });
                }
                _ => {
                    ui.colored_label(BAD, called.error.as_deref().unwrap_or("failed"));
                }
            }
        } else if ctx.api.asking(ask) {
            ui.spinner();
        }
    });
}
