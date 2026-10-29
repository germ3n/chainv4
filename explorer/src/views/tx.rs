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
        && !outcome.payments.is_empty()
    {
        super::section_with(
            ui,
            look,
            "Paid out",
            |ui| {
                ui.label(RichText::new(outcome.payments.len().to_string()).monospace().color(look.palette.muted));
            },
            |ui| payments(ui, ctx, &outcome.payments),
        );
    }

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
pub(crate) fn asked(ui: &mut Ui, ctx: &mut Ctx, tx: &TxView, outcome: Option<&Outcome>) {
    let look = ctx.look;
    // The trace names the function a call ran, which the transaction only names by selector.
    let called = outcome
        .and_then(|outcome| outcome.trace.hops.first())
        .filter(|hop| hop.depth == 0 && !hop.function.is_empty())
        .map(|hop| hop.function.clone());

    super::facts(ui, look, "asked", |facts| {
        facts.text("type", tx.kind());

        if let Some(means) = means(tx) {
            facts.text("means", means);
        }

        for (name, value) in tx.tx.as_object().into_iter().flatten().filter(|(name, _)| *name != "type") {
            facts.row(name, |ui| match (name.as_str(), value) {
                ("amount", Value::String(raw)) => {
                    ui.label(ctx.coins(raw)).on_hover_text(format!("{raw} units"));
                }
                // A claim's: all its miner earned from the pool, not what this one paid.
                ("earned", Value::String(raw)) => {
                    ui.label(ctx.coins(raw)).on_hover_text(format!("{raw} units, all told"));
                }
                // A claim's: the pool's block whose root it claims against.
                ("height", Value::Number(height)) if tx.kind() == "claim" => {
                    super::block(ui, ctx, &height.to_string(), &format::thousands(&height.to_string()));
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
                (_, Value::String(text)) if super::is_address(text) => super::address(ui, ctx, text),
                (_, Value::String(text)) => super::copyable(ui, text),
                (name, other) => super::json_tree(ui, ctx, name, other),
            });
        }

        // An EVM call's calldata, read by the ABI kept for whom it calls.
        if let (Some(to), Some(input)) = (tx.tx["to"].as_str(), tx.tx["input"].as_str())
            && let Some(decoded) = evm_call(ctx, to, input)
        {
            facts.row("calls", |ui| {
                ui.monospace(decoded);
            });
        }
    });
}

/// What a transaction asks, in words, for kinds whose fields alone say it badly.
fn means(tx: &TxView) -> Option<String> {
    match tx.kind() {
        "claim" => Some(format!(
            "Claim of {}'s mining pool earnings, from the pool's block {}: what it earned, less what it claimed before",
            tx.tx["to"].as_str().map_or_else(|| "?".to_string(), format::short),
            tx.tx["height"].as_u64().map_or_else(|| "?".to_string(), |height| format::thousands(&height.to_string())),
        )),
        // An approval names the version it follows: it is for the change after it.
        "approve_change" => Some(format!(
            "Approves {}, for governance's change {}",
            change(&tx.tx["change"]),
            tx.tx["version"].as_u64().map_or_else(|| "?".to_string(), |version| (version + 1).to_string()),
        )),
        "set_directory" => Some("The directory's admin sets the chain's directory: the contracts the apps use".to_string()),
        "hand_directory" => Some(match tx.tx["to"].as_str() {
            Some(to) => format!("The directory's admin hands the role to {}", format::short(to)),
            None => "The directory's admin renounces the role: nobody sets the directory again".to_string(),
        }),
        _ => None,
    }
}

/// A change under governance, as Tx::ApproveChange carries it: { what it is of: its new value }.
pub fn change(change: &Value) -> String {
    let Some((what, value)) = change.as_object().and_then(|change| change.iter().next()) else {
        return "a change".to_string();
    };

    match what.as_str() {
        "upgrade" => format!(
            "an upgrade: {}",
            super::network::stopping(value["least"].as_str().unwrap_or("?"), value["from"].as_u64().unwrap_or_default())
        ),
        "switch_finality" => format!("a switch of finality at epoch {}", value["epoch"].as_u64().map_or_else(|| "?".to_string(), |epoch| epoch.to_string())),
        "name" => format!("renaming the chain to {}", value.as_str().unwrap_or("?")),
        "consensus" => "a new consensus".to_string(),
        "bootnodes" => "new bootnodes".to_string(),
        "name_prices" => "new name prices".to_string(),
        "collections" => "new collection standards".to_string(),
        "federation" => "a new bridge federation".to_string(),
        "directory" => "a new directory".to_string(),
        "fees" => "new fee settings".to_string(),
        "reward" => "a new block reward".to_string(),
        "limits" => "new limits".to_string(),
        "finality" => "new finality settings".to_string(),
        other => format!("a change of its {}", other.replace('_', " ")),
    }
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
        "decided" => {
            facts.row("decided", |ui| {
                if ui.link(RichText::new(decided(outcome)).color(look.palette.accent)).on_hover_text("Open governance").clicked() {
                    ctx.go(super::View::Governance);
                }
            });
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

/// What a governance approval that decided its change says of it: the change's version.
pub fn decided(outcome: &Outcome) -> String {
    match outcome.version {
        Some(version) => format!("change {version} under governance: its approval was the last one needed"),
        None => "a change under governance".to_string(),
    }
}

/// Events in a list: what each contract emitted, in order. A token's Transfer reads as who sent how
/// much to whom, in its decimals.
/// The coin the transaction's contracts paid, line by line, as its receipt commits to it.
fn payments(ui: &mut Ui, ctx: &mut Ctx, payments: &[crate::model::PaymentView]) {
    let look = ctx.look;

    super::table(ui, look, "payments", &["#", "from", "to", "amount"], |ui| {
        for (index, payment) in payments.iter().enumerate() {
            ui.monospace(index.to_string());
            super::address(ui, ctx, &payment.from);
            super::address(ui, ctx, &payment.to);
            ui.label(ctx.coins(&payment.amount));
            ui.end_row();
        }
    });
}

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
    if let Some(decoded) = evm_log(ctx, contract, name, data) {
        ui.label(RichText::new(decoded).monospace());

        return;
    }

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

/// An EVM log read by its contract's ABI, kept here: Bumped(by = 0x…, count = 8).
fn evm_log(ctx: &Ctx, contract: &str, name: &str, data: &[Argument]) -> Option<String> {
    if name != "log" || !contract.starts_with("evm:") {
        return None;
    }

    let abi = ctx.abi(contract)?;
    let bytes: Vec<Vec<u8>> = data.iter().map(|value| chainv4::structs::hex::from_hex(value.value.as_str()?)).collect::<Option<_>>()?;
    let (log_data, topics) = bytes.split_last()?;
    let topics: Vec<[u8; 32]> = topics.iter().map(|topic| topic.as_slice().try_into().ok()).collect::<Option<_>>()?;
    let event = abi.event_for(&topics)?;

    Some(event.show(&event.decode_log(&topics, log_data).ok()?))
}

/// An EVM call's calldata, read by the ABI kept for whom it calls: bump(by = 3).
pub fn evm_call(ctx: &Ctx, to: &str, input: &str) -> Option<String> {
    let abi = ctx.abi(to)?;
    let input = chainv4::structs::hex::from_hex(input)?;
    let function = abi.function_for(&input)?;
    let values = function.decode_input(&input).ok()?;

    Some(format!("{}({})", function.name, function.show_inputs(&values)))
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

#[cfg(test)]
mod tests {
    use {super::*, serde_json::json};

    /// A governance approval of this change, as decision 4.
    fn approval(change: Value) -> TxView {
        serde_json::from_value(json!({
            "id": "0x01", "sender": "ed25519:0x02", "nonce": 0, "signer": "0x03", "signature": "0x04",
            "fee": { "gas_limit": 0, "max_price": "0", "tip": "0" },
            "tx": { "type": "approve_change", "version": 4, "change": change },
        }))
        .unwrap()
    }

    #[test]
    fn an_upgrade_under_governance_reads_as_the_nodes_it_stops() {
        assert_eq!(change(&json!({ "upgrade": { "least": "0.2.0", "from": 150_000 } })), "an upgrade: nodes older than 0.2.0 stop at block 150,000");
        assert_eq!(change(&json!({ "switch_finality": { "epoch": 9, "finality": {} } })), "a switch of finality at epoch 9");
        assert_eq!(change(&json!({ "fees": {} })), "new fee settings");
        assert_eq!(change(&json!({ "something_new": 1 })), "a change of its something new");
        assert_eq!(change(&Value::Null), "a change");
    }

    #[test]
    fn a_rename_under_governance_reads_as_the_chains_new_name() {
        assert_eq!(change(&json!({ "name": "chainv4 mainnet" })), "renaming the chain to chainv4 mainnet");
        assert_eq!(
            means(&approval(json!({ "name": "chainv4 mainnet" }))).as_deref(),
            Some("Approves renaming the chain to chainv4 mainnet, for governance's change 5")
        );
    }

    #[test]
    fn the_directory_admin_s_transactions_say_what_they_do() {
        let of = |tx: Value| -> TxView {
            serde_json::from_value(json!({
                "id": "0x01", "sender": "ed25519:0x02", "nonce": 0, "signer": "0x03", "signature": "0x04",
                "fee": { "gas_limit": 0, "max_price": "0", "tip": "0" },
                "tx": tx,
            }))
            .unwrap()
        };

        assert!(means(&of(json!({ "type": "set_directory", "directory": {} }))).is_some_and(|said| said.contains("sets the chain's directory")));
        assert!(means(&of(json!({ "type": "hand_directory", "to": null }))).is_some_and(|said| said.contains("renounces")));
        assert!(means(&of(json!({ "type": "hand_directory", "to": "ed25519:0x0505050505050505050505050505050505050505050505050505050505050505" }))).is_some_and(|said| said.contains("hands the role to")));
    }

    #[test]
    fn a_receipt_that_decided_a_change_names_its_version() {
        let receipt: crate::model::Receipt = serde_json::from_value(json!({
            "id": "0xdd", "block": "0xaa", "height": 3, "index": 0, "confirmations": 2, "final": true,
            "status": "decided", "version": 5, "fuel_used": 0, "gas_used": 0, "fee": "0", "events": [],
        }))
        .unwrap();

        assert_eq!((receipt.outcome.status.as_str(), receipt.outcome.version), ("decided", Some(5)));
        assert_eq!(decided(&receipt.outcome), "change 5 under governance: its approval was the last one needed");

        let counted: Outcome = serde_json::from_value(json!({ "status": "done", "fuel_used": 0, "gas_used": 0, "fee": "0", "events": [] })).unwrap();
        assert_eq!(counted.version, None, "an approval that only counted decided nothing");
    }
}
