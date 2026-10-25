//! One transaction: where it landed, what it was, what it cost and did, the events it emitted, and
//! where its contracts went.

use {
    super::{Ctx, Facts, Look, hops},
    crate::api::Ask,
    crate::format,
    crate::model::{Argument, EventView, Outcome, TxView},
    chainv4_ui::widgets,
    egui::{RichText, Ui},
    serde_json::Value,
};

pub fn show(ui: &mut Ui, ctx: &mut Ctx, id: &str, state: &mut hops::HopState) {
    let look = ctx.look;
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

    let landed = store.tx(id);
    let waiting = store.waiting(id);
    let Some(tx) = landed.map(|(_, tx)| tx).or(waiting) else {
        super::heading(ui, look, "Transaction", "", |_| {});
        super::copyable(ui, id);
        ui.add_space(8.0);

        match store.failures.get(&Ask::Tx(id.to_string())) {
            Some(failure) if failure.status == 404 => {
                super::note(ui, look, "Not on the main chain, at least not yet, and not in the mempool as last listed.");

                if widgets::quiet(ui, &look.palette, "Look in the mempool").clicked() {
                    ctx.api.ask(Ask::Mempool);
                }
            }
            _ => super::waiting(ui, ctx, &Ask::Tx(id.to_string())),
        }

        return;
    };
    let outcome = receipt.map(|receipt| &receipt.outcome).or(tx.receipt.as_ref());

    super::heading(ui, look, &format!("Transaction · {}", tx.kind()), "", |ui| match (receipt, waiting) {
        (Some(receipt), _) => {
            super::status(ui, look, &receipt.outcome.status);

            match receipt.finalized {
                true => widgets::pill(ui, "final", look.roles.final_),
                false => widgets::pill(ui, "not final yet", look.roles.side),
            };
        }
        (None, Some(_)) => {
            widgets::pill(ui, "waiting in the mempool", look.palette.warn);
        }
        (None, None) => {
            widgets::pill(ui, "off the main chain", look.roles.side);
        }
    });
    super::copyable(ui, id);
    ui.add_space(10.0);

    super::section(ui, look, "Summary", |ui| {
        super::facts(ui, look, "tx", |facts| {
            if let Some(receipt) = receipt {
                facts.row("block", |ui| {
                    super::block_link(ui, ctx, &receipt.block, receipt.height);
                    ui.label(RichText::new(format!("transaction {} in it · {} confirmations", receipt.index, receipt.confirmations)).color(look.palette.muted));
                });
            }

            if let Some((detail, _)) = landed {
                facts.text("time", format!("{} UTC · {}", format::utc(detail.summary.timestamp), format::ago(detail.summary.timestamp, ctx.now)));
            }

            facts.row("sender", |ui| super::address(ui, ctx, &tx.sender));
            facts.text("nonce", tx.nonce.to_string());
            facts.text(
                "offer",
                format!(
                    "up to {} gas at up to {} units, tip {}",
                    format::thousands(&tx.fee.gas_limit.to_string()),
                    format::thousands(&tx.fee.max_price),
                    format::thousands(&tx.fee.tip)
                ),
            );

            if let Some(outcome) = outcome {
                facts.text(
                    "used",
                    format!("{} gas, {} fuel", format::thousands(&outcome.gas_used.to_string()), format::thousands(&outcome.fuel_used.to_string())),
                );
                facts.text("fee paid", ctx.coins(&outcome.fee));
                result(facts, ctx, look, outcome);
            }
        });
    });

    super::section(ui, look, "Hops", |ui| hops::show(ui, ctx, tx, outcome, state));

    if let Some(outcome) = outcome
        && !outcome.events.is_empty()
    {
        super::section_with(
            ui,
            look,
            "Events",
            |ui| {
                ui.label(RichText::new(outcome.events.len().to_string()).monospace().color(look.palette.muted));
            },
            |ui| events(ui, ctx, &outcome.events),
        );
    }

    super::section(ui, look, "What it asked", |ui| asked(ui, ctx, tx, outcome));
    super::section(ui, look, "Signature", |ui| signature(ui, look, tx));
}

/// The transaction's own fields: amounts in the coin, a call's function by name and its
/// arguments in a row, Luau source as code.
fn asked(ui: &mut Ui, ctx: &mut Ctx, tx: &TxView, outcome: Option<&Outcome>) {
    let look = ctx.look;
    // The trace names the function a call ran, which the transaction only names by selector.
    let called = outcome
        .and_then(|outcome| outcome.trace.hops.first())
        .filter(|hop| hop.depth == 0 && !hop.function.is_empty())
        .map(|hop| hop.function.clone());

    super::facts(ui, look, "asked", |facts| {
        facts.text("type", tx.kind());

        for (name, value) in tx.tx.as_object().into_iter().flatten().filter(|(name, _)| *name != "type") {
            facts.row(name, |ui| match (name.as_str(), value) {
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
                    if args.is_empty() {
                        ui.label(RichText::new("none").color(look.palette.muted));
                    }

                    for arg in args.iter().filter_map(|arg| serde_json::from_value::<Argument>(arg.clone()).ok()) {
                        super::argument(ui, ctx, &arg);
                    }
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
            });
        }
    });
}

fn result(facts: &mut Facts, ctx: &mut Ctx, look: Look, outcome: &Outcome) {
    match outcome.status.as_str() {
        "failed" => {
            facts.row("error", |ui| {
                ui.colored_label(look.palette.bad, outcome.error.as_deref().unwrap_or("?"));
            });
        }
        "deployed" | "upgraded" => {
            if let Some(contract) = &outcome.contract {
                facts.row("contract", |ui| super::address(ui, ctx, contract));
            }
        }
        "returned" if !outcome.values.is_empty() => {
            facts.row("returned", |ui| {
                for value in &outcome.values {
                    super::argument(ui, ctx, value);
                }
            });
        }
        _ => {}
    }
}

/// Events in a list: what each contract emitted, in order. A token's Transfer reads as who sent how
/// much to whom, in its decimals.
pub fn events(ui: &mut Ui, ctx: &mut Ctx, events: &[EventView]) {
    let look = ctx.look;

    match look.narrow {
        true => {
            for event in events {
                super::list_row(ui, look, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(widgets::strong(&event.name, 13.5).color(look.palette.ink));
                        super::address(ui, ctx, &event.contract);
                    });
                    event_data(ui, ctx, &event.contract, &event.name, &event.data);
                });
            }
        }
        false => {
            super::table(ui, look, "events", &["#", "contract", "event", "data"], |ui| {
                for (index, event) in events.iter().enumerate() {
                    ui.monospace(index.to_string());
                    super::address(ui, ctx, &event.contract);
                    ui.label(widgets::strong(&event.name, 13.0));
                    event_data(ui, ctx, &event.contract, &event.name, &event.data);
                    ui.end_row();
                }
            });
        }
    }
}

/// An event's data in a row. A token's Transfer reads as who sent how much to whom, and its
/// Approval as who let whom spend how much, in the token's units.
pub fn event_data(ui: &mut Ui, ctx: &mut Ctx, contract: &str, name: &str, data: &[Argument]) {
    ui.horizontal_wrapped(|ui| match token_movement(ctx, contract, name, data) {
        Some((from, link, to, amount)) => {
            super::address(ui, ctx, &from);
            ui.label(RichText::new(link).color(ctx.look.palette.muted));
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

fn signature(ui: &mut Ui, look: Look, tx: &TxView) {
    super::facts(ui, look, "signature", |facts| {
        facts.row("signer", |ui| super::copyable(ui, &tx.signer));
        facts.row("signature", |ui| super::copyable(ui, &tx.signature));
    });
}
