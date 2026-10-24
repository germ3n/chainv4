//! One transaction: where it landed, what it was, what it cost and did, the events it emitted, and
//! where its contracts went.

use {
    super::{BAD, Ctx, GOOD, hops},
    crate::api::Ask,
    crate::format,
    crate::model::{Argument, EventView, Outcome, TxView},
    egui::{RichText, Ui},
    serde_json::Value,
};

pub fn show(ui: &mut Ui, ctx: &mut Ctx, id: &str, state: &mut hops::HopState) {
    let store = ctx.store;
    let receipt = store.receipts.get(id);

    if receipt.is_none() && !store.failures.contains_key(&Ask::Tx(id.to_string())) {
        ctx.api.ask(Ask::Tx(id.to_string()));
    }

    if let Some(receipt) = receipt
        && !store.details.contains_key(&receipt.block)
    {
        ctx.api.ask(Ask::Block(receipt.block.clone()));
    }

    super::title(ui, "Transaction");
    super::copyable(ui, id);

    let landed = store.tx(id);
    let waiting = store.waiting(id);
    let Some(tx) = landed.map(|(_, tx)| tx).or(waiting) else {
        match store.failures.get(&Ask::Tx(id.to_string())) {
            Some(failure) if failure.status == 404 => {
                ui.weak("Not on the main chain, at least not yet, and not in the mempool as last listed.");

                if ui.button("Look in the mempool").clicked() {
                    ctx.api.ask(Ask::Mempool);
                }
            }
            _ => super::waiting(ui, ctx, &Ask::Tx(id.to_string())),
        }

        return;
    };
    let outcome = receipt.map(|receipt| &receipt.outcome).or(tx.receipt.as_ref());

    super::facts(ui, "tx", |ui| {
        super::fact(ui, "status");

        match (receipt, waiting) {
            (Some(receipt), _) => {
                ui.horizontal(|ui| {
                    super::status(ui, &receipt.outcome.status);
                    ui.label(format!("{} confirmations", receipt.confirmations));

                    match receipt.finalized {
                        true => super::badge(ui, "final", GOOD),
                        false => super::badge(ui, "not final yet", super::SIDE),
                    }
                });
            }
            (None, Some(_)) => {
                ui.weak("waiting in the mempool");
            }
            (None, None) => {
                ui.weak("in a block off the main chain");
            }
        }

        ui.end_row();

        if let Some(receipt) = receipt {
            super::fact(ui, "block");
            ui.horizontal(|ui| {
                super::block(ui, ctx, &receipt.block, &format::thousands(&receipt.height.to_string()));
                ui.weak(format!("transaction {} in it", receipt.index));
            });
            ui.end_row();
        }

        if let Some((detail, _)) = landed {
            super::fact(ui, "time");
            ui.label(format!("{} UTC ({})", format::utc(detail.summary.timestamp), format::ago(detail.summary.timestamp, ctx.now)));
            ui.end_row();
        }

        super::fact(ui, "type");
        ui.monospace(tx.kind());
        ui.end_row();
        super::fact(ui, "sender");
        super::address(ui, ctx, &tx.sender);
        ui.end_row();
        super::fact(ui, "nonce");
        ui.label(tx.nonce.to_string());
        ui.end_row();
        super::fact(ui, "offer");
        ui.label(format!(
            "up to {} gas at up to {} units, tip {}",
            format::thousands(&tx.fee.gas_limit.to_string()),
            format::thousands(&tx.fee.max_price),
            format::thousands(&tx.fee.tip)
        ));
        ui.end_row();

        if let Some(outcome) = outcome {
            super::fact(ui, "used");
            ui.label(format!(
                "{} gas, {} fuel",
                format::thousands(&outcome.gas_used.to_string()),
                format::thousands(&outcome.fuel_used.to_string())
            ));
            ui.end_row();
            super::fact(ui, "fee paid");
            ui.label(ctx.coins(&outcome.fee));
            ui.end_row();
            result(ui, ctx, outcome);
        }
    });

    ui.add_space(6.0);
    ui.strong("Hops");
    hops::show(ui, ctx, tx, outcome, state);

    if let Some(outcome) = outcome
        && !outcome.events.is_empty()
    {
        ui.add_space(6.0);
        ui.strong(format!("Events ({})", outcome.events.len()));
        events(ui, ctx, &outcome.events);
    }

    ui.add_space(6.0);
    egui::CollapsingHeader::new("What it asked").default_open(true).show(ui, |ui| asked(ui, ctx, tx, outcome));
    signature(ui, tx);
}

/// The transaction's own fields: amounts in the coin, a call's function by name and its
/// arguments in a row, Luau source as code.
fn asked(ui: &mut Ui, ctx: &mut Ctx, tx: &TxView, outcome: Option<&Outcome>) {
    // The trace names the function a call ran, which the transaction only names by selector.
    let called = outcome
        .and_then(|outcome| outcome.trace.hops.first())
        .filter(|hop| hop.depth == 0 && !hop.function.is_empty())
        .map(|hop| hop.function.clone());

    super::facts(ui, "asked", |ui| {
        for (name, value) in tx.tx.as_object().into_iter().flatten().filter(|(name, _)| *name != "type") {
            super::fact(ui, name);

            match (name.as_str(), value) {
                ("amount", Value::String(raw)) => {
                    ui.label(ctx.coins(raw)).on_hover_text(format!("{raw} units"));
                }
                ("func", Value::String(selector)) => {
                    match &called {
                        Some(function) => ui.monospace(function).on_hover_text(selector),
                        None => ui.monospace(selector),
                    };
                }
                ("data", Value::Array(args)) => {
                    ui.horizontal_wrapped(|ui| {
                        if args.is_empty() {
                            ui.weak("none");
                        }

                        for arg in args.iter().filter_map(|arg| serde_json::from_value::<Argument>(arg.clone()).ok()) {
                            super::argument(ui, ctx, &arg);
                        }
                    });
                }
                ("source", Value::String(source)) => {
                    egui::CollapsingHeader::new(format!("{} lines of Luau", source.lines().count())).show(ui, |ui| {
                        let mut source = source.as_str();
                        ui.add(egui::TextEdit::multiline(&mut source).code_editor().desired_width(f32::INFINITY));
                    });
                }
                (_, Value::String(text)) if text.contains(':') && text.contains("0x") => super::address(ui, ctx, text),
                (_, Value::String(text)) => super::copyable(ui, text),
                (name, other) => super::json_tree(ui, ctx, name, other),
            }

            ui.end_row();
        }
    });
}

fn result(ui: &mut Ui, ctx: &mut Ctx, outcome: &Outcome) {
    match outcome.status.as_str() {
        "failed" => {
            super::fact(ui, "error");
            ui.colored_label(BAD, outcome.error.as_deref().unwrap_or("?"));
            ui.end_row();
        }
        "deployed" | "upgraded" => {
            super::fact(ui, "contract");

            if let Some(contract) = &outcome.contract {
                super::address(ui, ctx, contract);
            }

            ui.end_row();
        }
        "returned" if !outcome.values.is_empty() => {
            super::fact(ui, "returned");
            ui.horizontal_wrapped(|ui| {
                for value in &outcome.values {
                    super::argument(ui, ctx, value);
                }
            });
            ui.end_row();
        }
        _ => {}
    }
}

/// Events in a table. A token's Transfer reads as who sent how much to whom, in its decimals.
pub fn events(ui: &mut Ui, ctx: &mut Ctx, events: &[EventView]) {
    egui::Grid::new("events").num_columns(4).striped(true).show(ui, |ui| {
        for (index, event) in events.iter().enumerate() {
            ui.label(index.to_string());
            super::address(ui, ctx, &event.contract);
            ui.strong(&event.name);
            event_data(ui, ctx, &event.contract, &event.name, &event.data);
            ui.end_row();
        }
    });
}

/// An event's data in a row. A token's Transfer reads as who sent how much to whom, and its
/// Approval as who let whom spend how much, in the token's units.
pub fn event_data(ui: &mut Ui, ctx: &mut Ctx, contract: &str, name: &str, data: &[Argument]) {
    ui.horizontal_wrapped(|ui| match token_movement(ctx, contract, name, data) {
        Some((from, link, to, amount)) => {
            super::address(ui, ctx, &from);
            ui.weak(link);
            super::address(ui, ctx, &to);
            ui.label(RichText::new(amount).strong());
        }
        None => {
            for value in data {
                super::argument(ui, ctx, value);
            }
        }
    });
}

/// A token standard Transfer or Approval: who, how they relate, whom, and the amount in the
/// token's units.
fn token_movement(ctx: &mut Ctx, contract: &str, name: &str, data: &[Argument]) -> Option<(String, &'static str, String, String)> {
    let [from, to, amount] = data else {
        return None;
    };
    let link = match name {
        "Transfer" => "to",
        "Approval" => "lets spend",
        _ => return None,
    };

    if from.kind != "address" || to.kind != "address" {
        return None;
    }

    let (decimals, symbol) = ctx.token(contract)?;

    Some((from.text(), link, to.text(), format::amount(&amount.text(), decimals, &symbol)))
}

fn signature(ui: &mut Ui, tx: &TxView) {
    egui::CollapsingHeader::new("Signature").show(ui, |ui| {
        super::facts(ui, "signature", |ui| {
            super::fact(ui, "signer");
            super::copyable(ui, &tx.signer);
            ui.end_row();
            super::fact(ui, "signature");
            super::copyable(ui, &tx.signature);
            ui.end_row();
        });
    });
}

