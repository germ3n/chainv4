//! One transaction: where it landed, what it was, what it cost and did, the events it emitted, and
//! where its contracts went. An EVM call's calldata and logs read by name, by chainv4::abi::known.

use {
    super::{Ctx, Facts, Look, decoded, hops},
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

    if receipt.is_none() {
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
            Some(failure) if failure.status == 404 && ctx.settings.read_blocks => {
                let why = failure.message.clone();

                super::reading::outside(ui, ctx, &why, None);
            }
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

    // A block commits to what a transaction did, not to where its contracts went on the way.
    match ctx.settings.read_blocks {
        true => super::section(ui, look, "Hops", |ui| super::note(ui, look, "A block does not commit to the hops a transaction's contracts made, so they are not shown while this reads from checked blocks.")),
        false => super::section(ui, look, "Hops", |ui| hops::show(ui, ctx, tx, outcome, state)),
    }

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

        // A batch's steps have rows of their own, below.
        let steps = tx.steps();
        let listed_below = |name: &str| steps.is_some() && matches!((tx.kind(), name), ("batch", "steps") | ("multisig" | "approve_multisig", "tx"));

        for (name, value) in tx.tx.as_object().into_iter().flatten().filter(|(name, _)| *name != "type" && !listed_below(name)) {
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

        // An EVM call's calldata, read, a multisig's too: the input row above stays as it is.
        let call = [&tx.tx, &tx.tx["tx"]].into_iter().find_map(|tx| Some((tx["to"].as_str()?, tx["input"].as_str()?)));

        if let Some((to, input)) = call
            && let Some(read) = decoded::call_of(ctx, to, input)
        {
            facts.row("calls", |ui| decoded::call(ui, ctx, Some(to), &read));
        }
    });

    if let Some(steps) = tx.steps() {
        ui.add_space(8.0);
        ui.label(RichText::new(format!("{} step{}, all or nothing", steps.len(), if steps.len() == 1 { "" } else { "s" })).color(look.palette.soft));

        for (index, step) in steps.iter().enumerate() {
            ui.push_id(("step", index), |ui| one_step(ui, ctx, tx, step, index, steps.len(), outcome));
        }
    }
}

/// One step of a batch: its number, what came of it, and its own fields as any transaction's show.
fn one_step(ui: &mut Ui, ctx: &mut Ctx, tx: &TxView, step: &Value, index: usize, count: usize, outcome: Option<&Outcome>) {
    let look = ctx.look;
    let view = TxView { tx: step.clone(), receipt: None, ..tx.clone() };

    super::list_row(ui, look, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(format!("{} of {count}", index + 1)).monospace().color(look.palette.muted));

            if let Some(outcome) = outcome {
                let (word, _) = outcome.of_step(index);
                let color = match word {
                    "failed" => look.palette.bad,
                    "undone" => look.palette.warn,
                    "not run" => look.palette.muted,
                    _ => look.palette.good,
                };
                widgets::pill(ui, word, color);
            }
        });

        if let Some((_, Some(done))) = outcome.map(|outcome| outcome.of_step(index)) {
            super::facts(ui, look, "step result", |facts| {
                if let Some(contract) = &done.contract {
                    facts.row("contract", |ui| super::address(ui, ctx, contract));
                }

                if !done.values.is_empty() {
                    facts.row("returned", |ui| {
                        for value in &done.values {
                            super::argument(ui, ctx, value);
                        }
                    });
                }

                if done.events + done.payments > 0 {
                    facts.text("emitted", format!("{} events, {} payments, in the lists above in order", done.events, done.payments));
                }
            });
        }

        // Why it failed is on the whole transaction's error, and stays beside the step it names.
        if let Some(error) = outcome.filter(|outcome| outcome.failed_step == Some(index as u32)).and_then(|outcome| outcome.error.as_deref()) {
            ui.add(egui::Label::new(RichText::new(error).color(look.palette.bad)).wrap());
        }

        asked(ui, ctx, &view, None);
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
        // An approval names its kind's version: it is for that kind's change after it.
        "approve_change" => Some(format!(
            "Approves {}, for {} change {}",
            change(&tx.tx["change"]),
            super::governance::kinds(&crate::model::change_kind(&tx.tx["change"]).unwrap_or_else(|| "governance".to_string())),
            next(&tx.tx["version"]),
        )),
        "withdraw_change" => Some(format!(
            "Withdraws its approval of {} change {}: it backs no change of that kind",
            super::governance::kinds(tx.tx["kind"].as_str().unwrap_or("governance")),
            next(&tx.tx["version"]),
        )),
        "set_directory" => Some("The directory's admin sets the chain's directory: the contracts the apps use".to_string()),
        "hand_directory" => Some(match tx.tx["to"].as_str() {
            Some(to) => format!("The directory's admin hands the role to {}", format::short(to)),
            None => "The directory's admin renounces the role: nobody sets the directory again".to_string(),
        }),
        _ => None,
    }
}

/// The change after a version: what an approval or withdrawal naming it is for.
fn next(version: &Value) -> String {
    version.as_u64().map_or_else(|| "?".to_string(), |version| (version + 1).to_string())
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
            if let Some(step) = outcome.failed_step {
                facts.text("failed at", format!("step {}, and every step was undone", step + 1));
            }

            facts.row("error", |ui| {
                ui.colored_label(look.palette.bad, outcome.error.as_deref().unwrap_or("?"));
            });
        }
        "batch" => facts.text("did", format!("all {} steps", outcome.steps.len())),
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

/// What a governance approval that decided its change says of it: the chain's count of decisions,
/// and its kind's version.
pub fn decided(outcome: &Outcome) -> String {
    match (outcome.version, &outcome.kind, outcome.kind_version) {
        (Some(decision), Some(kind), Some(version)) => format!("change {decision}, {} version {version}: its approval was the last one needed", super::governance::kinds(kind)),
        (Some(decision), _, _) => format!("change {decision} under governance: its approval was the last one needed"),
        (None, _, _) => "a change under governance".to_string(),
    }
}

/// Events in a list: what each contract emitted, in order. A token's Transfer reads as who sent how
/// much to whom, in its decimals.
/// The coin the transaction's contracts paid, line by line, as its receipt commits to it.
fn payments(ui: &mut Ui, ctx: &mut Ctx, payments: &[crate::model::PaymentView]) {
    let look = ctx.look;

    if look.narrow {
        for (index, payment) in payments.iter().enumerate() {
            super::list_row(ui, look, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("#{index}")).monospace().color(look.palette.muted));
                    ui.label(RichText::new(ctx.coins(&payment.amount)).strong());
                });
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    super::address(ui, ctx, &payment.from);
                    ui.label(RichText::new("→").color(look.palette.muted));
                    super::address(ui, ctx, &payment.to);
                });
            });
        }

        return;
    }

    super::table(ui, look, "payments", &["#", "from", "to", "amount"], |ui| {
        for (index, payment) in payments.iter().enumerate() {
            ui.monospace(index.to_string());
            super::address(ui, ctx, &payment.from);
            super::address(ui, ctx, &payment.to);
            ui.label(ctx.coins(&payment.amount));
            super::row_end(ui);
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
                        ui.label(widgets::strong(event_name(ctx, &event.contract, &event.name, &event.data), 13.5).color(look.palette.ink));
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
                    ui.label(widgets::strong(event_name(ctx, &event.contract, &event.name, &event.data), 13.0));
                    event_data(ui, ctx, &event.contract, &event.name, &event.data);
                    super::row_end(ui);
                }
            });
        }
    }
}

/// What to call an event: its name, and for an EVM log something reads, its event's.
pub fn event_name(ctx: &Ctx, contract: &str, name: &str, data: &[Argument]) -> String {
    decoded::log_of(ctx, contract, name, data).map_or_else(|| name.to_string(), |read| read.name.clone())
}

/// An event's data in a row. A token's Transfer reads as who sent how much to whom, and its
/// Approval as who let whom spend how much, in the token's units. An EVM log reads by its ABI, an
/// ERC-20's and an NFT's Transfer the same way, and says which ABI that was.
pub fn event_data(ui: &mut Ui, ctx: &mut Ctx, contract: &str, name: &str, data: &[Argument]) {
    if let Some(read) = decoded::log_of(ctx, contract, name, data) {
        decoded::log(ui, ctx, contract, &read, data);

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
    fn an_evm_call_reads_by_name_and_its_raw_input_stays_beside_it() {
        let (selector, types) = chainv4::abi::parse_signature("transfer(address,uint256)").unwrap();
        let args = chainv4::abi::encode_values(&types, &[chainv4::abi::AbiValue::Address([0xab; 20]), chainv4::abi::AbiValue::Uint(chainv4::structs::u256::u256::new(5))]).unwrap();
        let input = chainv4::structs::hex::to_hex(&[selector.unwrap().to_vec(), args].concat());
        let view = |tx: Value| -> TxView {
            serde_json::from_value(json!({
                "id": "0x01", "sender": "ed25519:0x02", "nonce": 0, "signer": "0x03", "signature": "0x04",
                "fee": { "gas_limit": 0, "max_price": "0", "tip": "0" },
                "tx": tx,
            }))
            .unwrap()
        };
        let call = json!({ "type": "evm_call", "to": "evm:0x0505050505050505050505050505050505050505", "value": "0", "input": input });
        let by_multisig = json!({ "type": "multisig", "account": "multisig:0x06", "tx": call.clone(), "signatures": [] });

        for tx in [view(call), view(by_multisig)] {
            let (text, _) = crate::views::render(&crate::store::Store::default(), &mut crate::light::Light::default(), 1_200.0, |ui, ctx| asked(ui, ctx, &tx, None));

            for said in ["calls", "transfer", "value =", "read as ERC-20"] {
                assert!(text.contains(said), "{said} in {text}");
            }
        }

        // What reads as nothing shows as it did.
        let unread = view(json!({ "type": "evm_call", "to": "evm:0x05", "value": "0", "input": "0xdeadbeef" }));
        let (text, _) = crate::views::render(&crate::store::Store::default(), &mut crate::light::Light::default(), 1_200.0, |ui, ctx| asked(ui, ctx, &unread, None));

        assert!(text.contains("0xdeadbeef") && !text.contains("calls"), "{text}");
    }

    /// A batch as the node writes it, with its outcome, and what the tx page's lists draw of it.
    fn batch_view(status: Value) -> (TxView, Outcome) {
        let pay = |to: &str| json!({ "type": "transfer", "from": "ed25519:0x02", "to": to, "amount": "5" });
        let tx: TxView = serde_json::from_value(json!({
            "id": "0x01", "sender": "ed25519:0x02", "nonce": 0, "signer": "0x03", "signature": "0x04",
            "fee": { "gas_limit": 0, "max_price": "0", "tip": "0" },
            "tx": { "type": "batch", "steps": [pay("ed25519:0x0a"), { "type": "contract_call", "to": "contract:0x0b", "func": "0x01", "amount": "0", "data": [] }, pay("ed25519:0x0c")] },
        }))
        .unwrap();
        let outcome: Outcome = serde_json::from_value(status).unwrap();

        (tx, outcome)
    }

    #[test]
    fn a_batch_shows_each_step_and_what_came_of_it_on_any_screen() {
        let (tx, outcome) = batch_view(json!({
            "status": "batch", "fuel_used": 0, "gas_used": 0, "fee": "0", "events": [], "payments": [],
            "steps": [
                { "status": "done", "events": 0, "payments": 0 },
                { "status": "returned", "values": [{ "type": "u64", "value": "7" }], "events": 2, "payments": 1 },
                { "status": "deployed", "contract": "contract:0x0d", "events": 0, "payments": 0 },
            ],
        }));

        for width in [360.0, 740.0, 1_200.0] {
            let (text, _) = crate::views::render(&crate::store::Store::default(), &mut crate::light::Light::default(), width, |ui, ctx| asked(ui, ctx, &tx, Some(&outcome)));

            for said in ["3 steps, all or nothing", "1 of 3", "2 of 3", "3 of 3", "done", "returned", "deployed", "2 events, 1 payments", "transfer", "contract_call"] {
                assert!(text.contains(said), "{said} in {text}");
            }
        }
    }

    #[test]
    fn a_failed_batch_says_which_step_stopped_it_why_and_that_the_rest_went() {
        let (tx, outcome) = batch_view(json!({
            "status": "failed", "failed_step": 1, "error": "step 2 failed, undoing every step: out of fuel", "fuel_used": 0, "gas_used": 0, "fee": "0", "events": [],
        }));
        let (text, _) = crate::views::render(&crate::store::Store::default(), &mut crate::light::Light::default(), 360.0, |ui, ctx| asked(ui, ctx, &tx, Some(&outcome)));

        for said in ["undone", "failed", "not run", "step 2 failed, undoing every step: out of fuel"] {
            assert!(text.contains(said), "{said} in {text}");
        }

        assert_eq!((outcome.of_step(0).0, outcome.of_step(1).0, outcome.of_step(2).0), ("undone", "failed", "not run"));
    }

    #[test]
    fn a_multisigs_batch_and_an_approval_of_one_show_their_steps_and_hops_too() {
        let (batch, _) = batch_view(json!({ "status": "batch", "fuel_used": 0, "gas_used": 0, "fee": "0", "events": [] }));
        let carried = |kind: &str| TxView { tx: json!({ "type": kind, "account": "multisig:0x06", "tx": batch.tx.clone(), "signatures": [] }), ..batch.clone() };

        for kind in ["multisig", "approve_multisig"] {
            let tx = carried(kind);

            assert_eq!(tx.steps().map(<[Value]>::len), Some(3));

            let (text, _) = crate::views::render(&crate::store::Store::default(), &mut crate::light::Light::default(), 1_200.0, |ui, ctx| asked(ui, ctx, &tx, None));

            assert!(text.contains("3 steps, all or nothing") && text.contains("2 of 3"), "{text}");
        }

        // With no contract to trace, a step's hop each, in order.
        let hops = hops::of(&batch, None);

        assert_eq!(hops.iter().map(|hop| hop.kind.as_str()).collect::<Vec<_>>(), ["transfer", "call", "transfer"]);
        assert_eq!(hops::of(&carried("multisig"), None).len(), 3);

        let failed: Outcome = serde_json::from_value(json!({ "status": "failed", "failed_step": 2, "error": "no", "fuel_used": 0, "gas_used": 0, "fee": "0", "events": [] })).unwrap();
        let hops = hops::of(&batch, Some(&failed));

        assert_eq!(hops.iter().map(|hop| hop.error.is_some()).collect::<Vec<_>>(), [false, false, true]);
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
            Some("Approves renaming the chain to chainv4 mainnet, for the name's change 5")
        );
        assert_eq!(
            means(&approval(json!({ "switch_finality": { "epoch": 9, "finality": {} } }))).as_deref(),
            Some("Approves a switch of finality at epoch 9, for the consensus's change 5"),
            "finality's changes are on the consensus's ballot"
        );
        assert_eq!(means(&approval(json!({ "name_prices": [] }))).as_deref(), Some("Approves new name prices, for the name prices' change 5"));
    }

    #[test]
    fn a_withdrawal_says_whose_kind_it_takes_back() {
        let withdrawal: TxView = serde_json::from_value(json!({
            "id": "0x01", "sender": "ed25519:0x02", "nonce": 0, "signer": "0x03", "signature": "0x04",
            "fee": { "gas_limit": 0, "max_price": "0", "tip": "0" },
            "tx": { "type": "withdraw_change", "kind": "fees", "version": 2 },
        }))
        .unwrap();

        assert_eq!(means(&withdrawal).as_deref(), Some("Withdraws its approval of the fees' change 3: it backs no change of that kind"));
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
            "status": "decided", "version": 5, "kind": "upgrade", "kind_version": 2, "fuel_used": 0, "gas_used": 0, "fee": "0", "events": [],
        }))
        .unwrap();

        assert_eq!((receipt.outcome.status.as_str(), receipt.outcome.version, receipt.outcome.kind.as_deref(), receipt.outcome.kind_version), ("decided", Some(5), Some("upgrade"), Some(2)));
        assert_eq!(decided(&receipt.outcome), "change 5, the upgrade's version 2: its approval was the last one needed");

        let unkinded = Outcome { kind: None, kind_version: None, ..receipt.outcome.clone() };
        assert_eq!(decided(&unkinded), "change 5 under governance: its approval was the last one needed", "a receipt that names no kind");

        let counted: Outcome = serde_json::from_value(json!({ "status": "done", "fuel_used": 0, "gas_used": 0, "fee": "0", "events": [] })).unwrap();
        assert_eq!(counted.version, None, "an approval that only counted decided nothing");
    }
}
