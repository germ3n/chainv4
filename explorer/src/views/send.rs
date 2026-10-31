//! Signing and sending transactions from the explorer. The key stays in this window: transactions
//! are built and signed here with chainv4's own code, and only the signed bytes go to the node.
//! The node works out the fee offer, as `chainv4 send` does, for a probe signed with none.

use {
    super::{Ctx, View},
    chainv4_ui::{
        files::{self, Pick},
        widgets,
    },
    crate::api::{Ask, Failure},
    crate::format,
    crate::model::{Account, Offer},
    chainv4::crypto::hasher::Hasher,
    chainv4::crypto::keypair::Keypair,
    chainv4::structs::address::Address,
    chainv4::structs::amount::Amount,
    chainv4::structs::bundle::{Bundle, bundle},
    chainv4::structs::code::Code,
    chainv4::structs::hash::HashType,
    chainv4::structs::hex::{from_hex, to_hex},
    chainv4::structs::tx::{FeeOffer, SignedTx, Tx, TxCallArgumentType},
    chainv4::structs::u256::{i256, u256},
    egui::{RichText, Ui},
    serde_json::{Value, json},
    std::sync::{Arc, Mutex},
};

/// The argument types a call can be given.
const KINDS: [&str; 13] = ["address", "amount", "u256", "u128", "u64", "i64", "i128", "i256", "number", "bool", "string", "bytes", "nil"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Transfer,
    Call,
    Deploy,
    Burn,
}

/// What a file picker is for.
#[derive(Clone, Copy)]
enum Picking {
    Key,
    Wasm,
    Lua,
}

/// What the person picked, read, or why it could not be.
enum Picked {
    Key { name: String, bytes: Vec<u8> },
    Wasm { name: String, bytes: Vec<u8> },
    /// A Lua contract's files, by name.
    Lua(Vec<(String, Vec<u8>)>),
    Failed(String),
}

/// Where sending stands.
enum Step {
    Idle,
    /// Asked for the sender's nonce.
    Nonce { tx: Tx },
    /// With Read from checked blocks on: pressed to send, so a direct proof of the sender's account
    /// was asked for, for its nonce.
    Proving { tx: Tx },
    /// With Read from checked blocks on: priced from checked blocks, to be seen before it goes, as
    /// nothing tries it first. Whether a block showed the gas, and the block the nonce is as of.
    Review { tx: Tx, nonce: u64, fee: FeeOffer, seen: bool, at: u64 },
    /// Asked the node for an offer.
    Offer { tx: Tx, nonce: u64, ask: Ask },
    /// Priced, but a dry run on the node says it would fail: sent only if asked again.
    Confirm { tx: Tx, nonce: u64, fee: FeeOffer, warning: String },
    /// Submitted.
    Submit { ask: Ask },
    /// Taken by the node: waiting for a block.
    Waiting { id: String },
    /// In a block, and why it failed if it did.
    Landed { id: String, failed: Option<String> },
    Failed(String),
}

pub struct SendState {
    key: Option<Keypair>,
    key_name: String,
    /// A key's text, pasted, as chainv4 keygen writes it: wiped as it goes.
    pasted: chainv4_ui::secret::Typed,
    picked: Arc<Mutex<Option<Picked>>>,
    kind: Kind,
    to: String,
    amount: String,
    function: String,
    args: Vec<(String, String)>,
    /// The Lua contract's entry, by name, and its text, as the editor holds it.
    lua_name: String,
    lua: String,
    /// The files the entry may --@include, by name.
    included: Vec<(String, String)>,
    /// The Lua contract last sent, put together: where its errors' lines came from.
    bundle: Option<Bundle>,
    wasm: Option<(String, Vec<u8>)>,
    gas_limit: String,
    max_price: String,
    tip: String,
    step: Step,
    /// The next nonce after what was sent from here, by sender: what a proof at a final block has not
    /// reached yet.
    sent_nonce: Option<(String, u64)>,
}

impl Default for SendState {
    fn default() -> SendState {
        SendState {
            key: None,
            key_name: String::new(),
            pasted: chainv4_ui::secret::Typed::default(),
            picked: Arc::default(),
            kind: Kind::Transfer,
            to: String::new(),
            amount: String::new(),
            function: String::new(),
            args: Vec::new(),
            lua_name: "contract.lua".to_string(),
            lua: "local M = {}\n\nfunction M.hello()\n    return \"hi\"\nend\n\nreturn M\n".to_string(),
            included: Vec::new(),
            bundle: None,
            wasm: None,
            gas_limit: String::new(),
            max_price: String::new(),
            tip: String::new(),
            step: Step::Idle,
            sent_nonce: None,
        }
    }
}

impl SendState {
    /// The transaction waiting for a block, which the app checks on.
    pub fn waiting_for(&self) -> Option<&str> {
        match &self.step {
            Step::Waiting { id } => Some(id),
            _ => None,
        }
    }

    /// Takes an answer meant for sending, moving on to the next step.
    pub fn answered(&mut self, ask: &Ask, result: &Result<Value, Failure>, api: &mut crate::api::Api) {
        let Some(key) = &self.key else {
            return;
        };
        let sender = key.public().address().to_string();

        // What a nonce sent from here goes on from, for a final block to catch up with.
        if let (Step::Submit { ask: asked }, Ok(_), Ask::Submit(body)) = (&self.step, result, ask)
            && asked == ask
        {
            let sent = serde_json::from_str::<Value>(body).ok().and_then(|body| from_hex(body["tx"].as_str()?)).and_then(|bytes| wincode::deserialize::<SignedTx>(&bytes).ok());

            if let Some(sent) = sent {
                self.sent_nonce = Some((sender.clone(), sent.nonce + 1));
            }
        }

        let step = std::mem::replace(&mut self.step, Step::Idle);

        self.step = match (step, ask, result) {
            (Step::Nonce { tx }, Ask::Account(address), Ok(value)) if *address == sender => {
                match serde_json::from_value::<Account>(value.clone()) {
                    Ok(account) => {
                        let probe = SignedTx::sign(tx.clone(), account.nonce, FeeOffer::NONE, key);
                        let mut body = json!({ "tx": encode(&probe) });

                        if let Ok(gas_limit) = self.gas_limit.trim().parse::<u64>() {
                            body["gas_limit"] = gas_limit.into();
                        }

                        for (name, text) in [("max_price", &self.max_price), ("tip", &self.tip)] {
                            if !text.trim().is_empty() {
                                body[name] = text.trim().replace(',', "").into();
                            }
                        }

                        let ask = Ask::Offer(body.to_string());
                        api.ask(ask.clone());

                        Step::Offer {
                            tx,
                            nonce: account.nonce,
                            ask,
                        }
                    }
                    Err(err) => Step::Failed(format!("the node sent no nonce: {err}")),
                }
            }
            (Step::Nonce { .. }, Ask::Account(address), Err(failure)) if *address == sender => Step::Failed(failure.message.clone()),
            (Step::Offer { tx, nonce, ask: asked }, ask, result) if asked == *ask => match result {
                Ok(value) => match offer(value) {
                    Ok((fee, None)) => submit(api, key, tx, nonce, fee),
                    Ok((fee, Some(warning))) => Step::Confirm { tx, nonce, fee, warning },
                    Err(err) => Step::Failed(err),
                },
                Err(failure) => Step::Failed(failure.message.clone()),
            },
            (Step::Submit { ask: asked }, ask, result) if asked == *ask => match result {
                Ok(value) => match value.get("id").and_then(Value::as_str) {
                    Some(id) => Step::Waiting { id: id.to_string() },
                    None => Step::Failed("the node sent no transaction id".to_string()),
                },
                Err(failure) => Step::Failed(failure.message.clone()),
            },
            (Step::Waiting { id }, Ask::Tx(asked), Ok(value)) if *asked == id => {
                // A deploy's error names lines of the source as sent: put back in the files they came from.
                let failed = (value["status"] == "failed").then(|| {
                    let error = value["error"].as_str().unwrap_or("?");

                    self.bundle.as_ref().map_or_else(|| error.to_string(), |bundle| bundle.explain(error))
                });

                Step::Landed { id, failed }
            }
            (step, _, _) => step,
        };
    }
}

pub fn show(ui: &mut Ui, ctx: &mut Ctx, state: &mut SendState) {
    let look = ctx.look;
    let palette = look.palette;
    take_picked(state);
    super::heading(ui, look, "Send", "Sign a transaction here and send it to the node: the key never leaves this app.", |_| {});
    let reading = ctx.settings.read_blocks;

    if reading {
        priced(ctx, state);
    }

    super::section(ui, look, "Signing key", |ui| key(ui, ctx, state));

    let Some(key) = state.key.clone() else {
        return;
    };
    let (decimals, symbol) = ctx.coin();
    let field = if look.narrow { f32::INFINITY } else { 460.0 };

    super::section(ui, look, "Transaction", |ui| {
        ui.horizontal_wrapped(|ui| {
            for (kind, name) in [(Kind::Transfer, "Transfer"), (Kind::Call, "Call"), (Kind::Deploy, "Deploy"), (Kind::Burn, "Burn")] {
                // What a deploy uses cannot be known without trying it on the node.
                if kind != Kind::Deploy || !reading {
                    ui.selectable_value(&mut state.kind, kind, name);
                }
            }
        });
        ui.add_space(6.0);

        super::facts(ui, look, "send", |facts| {
            if matches!(state.kind, Kind::Transfer | Kind::Call) {
                facts.row(if state.kind == Kind::Call { "contract" } else { "to" }, |ui| {
                    ui.add(egui::TextEdit::singleline(&mut state.to).hint_text("ed25519:0x… or contract:0x…").desired_width(field));
                });
            }

            if state.kind == Kind::Call {
                facts.row("function", |ui| {
                    ui.add(egui::TextEdit::singleline(&mut state.function).hint_text("transfer").desired_width(field.min(260.0)));
                });
                facts.row("arguments", |ui| arguments(ui, &palette, &mut state.args));
            }

            if state.kind != Kind::Deploy {
                facts.row(if state.kind == Kind::Call { "value" } else { "amount" }, |ui| {
                    ui.add(egui::TextEdit::singleline(&mut state.amount).hint_text("0").desired_width(160.0));
                    ui.label(&symbol);
                });
            }
        });

        if state.kind == Kind::Deploy {
            code(ui, look, state);
        }

        ui.add_space(4.0);
        let fee_heading = match reading {
            true => "Fee: suggested from checked blocks unless set",
            false => "Fee: worked out by the node unless set",
        };

        egui::CollapsingHeader::new(RichText::new(fee_heading).color(palette.soft)).show(ui, |ui| {
            super::facts(ui, look, "fee", |facts| {
                for (name, value, hint) in [
                    ("gas limit", &mut state.gas_limit, "auto"),
                    ("max price per gas", &mut state.max_price, "auto, in units"),
                    ("tip per gas", &mut state.tip, "0, in units"),
                ] {
                    facts.row(name, |ui| {
                        ui.add(egui::TextEdit::singleline(value).hint_text(hint).desired_width(200.0));
                    });
                }
            });
        });

        ui.add_space(8.0);
        let busy = matches!(state.step, Step::Nonce { .. } | Step::Proving { .. } | Step::Offer { .. } | Step::Submit { .. });

        if widgets::primary(ui, &palette, "Sign and send", !busy).clicked() {
            state.bundle = match (state.kind, &state.wasm) {
                (Kind::Deploy, None) => lua_bundle(state).ok(),
                _ => None,
            };

            match (build(state, &key, decimals), reading) {
                (Ok(tx), true) => state.step = prove_nonce(ctx, &key, tx),
                (Ok(tx), false) => {
                    ctx.api.ask(Ask::Account(key.public().address().to_string()));
                    state.step = Step::Nonce { tx };
                }
                (Err(err), _) => state.step = Step::Failed(err),
            }
        }

        ui.add_space(4.0);
        confirm(ui, ctx, state, &key);
        review(ui, ctx, state, &key);
        outcome(ui, ctx, state);
    });
}

/// Asks for a direct proof of the sender's account at the final block, for its nonce: what Read from
/// checked blocks does in place of asking the node, and says so before it goes.
fn prove_nonce(ctx: &mut Ctx, key: &Keypair, tx: Tx) -> Step {
    let account = chainv4::db::address_key(&key.public().address());

    match ctx.light.client().and_then(|client| ctx.reading.prove(chainv4::db::Column::Accounts, &account, client)) {
        Some(ask) => {
            ctx.api.retry(ask);

            Step::Proving { tx }
        }
        None => Step::Failed("The light client has not reached a final block yet, so the nonce cannot be proven.".to_string()),
    }
}

/// Moves on once the nonce is proven: the fee as checked blocks suggest it, unless set, for review.
fn priced(ctx: &mut Ctx, state: &mut SendState) {
    let (Step::Proving { .. }, Some(key)) = (&state.step, &state.key) else {
        return;
    };
    let sender = key.public().address();
    let account = chainv4::db::address_key(&sender);

    if ctx.reading.proving(chainv4::db::Column::Accounts, &account) {
        return;
    }

    let Step::Proving { tx } = std::mem::replace(&mut state.step, Step::Idle) else {
        return;
    };

    state.step = match ctx.reading.proven(chainv4::db::Column::Accounts, &account).cloned() {
        Some(Ok(proven)) => {
            let held: Option<chainv4::structs::account::Account> = match &proven.value {
                Some(bytes) => wincode::deserialize(bytes).ok(),
                None => Some(chainv4::structs::account::Account::default()),
            };

            match (held, ctx.light.client()) {
                (Some(held), Some(client)) => {
                    let sent = state.sent_nonce.as_ref().filter(|(from, _)| *from == sender.to_string()).map_or(0, |(_, nonce)| *nonce);
                    let tip = typed_units(&state.tip).unwrap_or(u256::ZERO);
                    let (fee, seen) = ctx.reading.suggest(&tx, client.tip().base_price, tip);
                    let fee = FeeOffer {
                        gas_limit: state.gas_limit.trim().parse().unwrap_or(fee.gas_limit),
                        max_price: typed_units(&state.max_price).unwrap_or(fee.max_price),
                        tip: fee.tip,
                    };

                    Step::Review { tx, nonce: held.nonce.max(sent), fee, seen, at: proven.height }
                }
                _ => Step::Failed("An account that does not read.".to_string()),
            }
        }
        Some(Err(why)) => Step::Failed(why),
        None => Step::Failed("The node gave no proof of the sender's account.".to_string()),
    };
}

/// Units typed for a fee, as written, if they read.
fn typed_units(text: &str) -> Option<u256> {
    u256::from_str_radix(&text.trim().replace(',', ""), 10).ok().filter(|_| !text.trim().is_empty())
}

/// What goes out with Read from checked blocks on, to be seen first, as no node tries it.
fn review(ui: &mut Ui, ctx: &mut Ctx, state: &mut SendState, key: &Keypair) {
    let palette = ctx.look.palette;
    let Step::Review { nonce, fee, seen, at, .. } = &state.step else {
        return;
    };
    let gas = match seen {
        true => "what the same kind of transaction used lately in checked blocks, a quarter more",
        false => "a round guess, as no checked block shows what this kind of transaction uses",
    };

    ui.add(egui::Label::new(RichText::new(format!(
        "Nonce {nonce}, proven at final block {}: the node saw this account, and a transaction since that block is not counted unless sent from here. Up to {} gas, {gas}, at up to {} units. Nothing tries it first, so a call that would fail still costs its fee.",
        format::thousands(&at.to_string()),
        format::thousands(&fee.gas_limit.to_string()),
        format::thousands(&fee.max_price.to_string()),
    )).color(palette.soft)).wrap());
    ui.horizontal_wrapped(|ui| {
        if widgets::primary(ui, &palette, "Send", true).clicked()
            && let Step::Review { tx, nonce, fee, .. } = std::mem::replace(&mut state.step, Step::Idle)
        {
            state.step = submit(ctx.api, key, tx, nonce, fee);
        }

        if widgets::quiet(ui, &palette, "Cancel").clicked() {
            state.step = Step::Idle;
        }
    });
}

/// Signs a transaction with its fee and submits it.
fn submit(api: &mut crate::api::Api, key: &Keypair, tx: Tx, nonce: u64, fee: FeeOffer) -> Step {
    let signed = SignedTx::sign(tx, nonce, fee, key);
    let ask = Ask::Submit(json!({ "tx": encode(&signed) }).to_string());
    api.ask(ask.clone());

    Step::Submit { ask }
}

/// A transaction a dry run says would fail: the warning, and whether to send it all the same.
fn confirm(ui: &mut Ui, ctx: &mut Ctx, state: &mut SendState, key: &Keypair) {
    let palette = ctx.look.palette;
    let Step::Confirm { warning, .. } = &state.step else {
        return;
    };

    ui.colored_label(palette.bad, format!("A dry run on the node says this would fail: {warning}"));
    ui.label(RichText::new("Sent anyway, it uses its nonce and pays its fee, unless things change before it lands.").small().color(palette.muted));
    ui.horizontal(|ui| {
        if widgets::primary(ui, &palette, "Send anyway", true).clicked()
            && let Step::Confirm { tx, nonce, fee, .. } = std::mem::replace(&mut state.step, Step::Idle)
        {
            state.step = submit(ctx.api, key, tx, nonce, fee);
        }

        if widgets::quiet(ui, &palette, "Cancel").clicked() {
            state.step = Step::Idle;
        }
    });
}

/// Where sending stands, under the button.
fn outcome(ui: &mut Ui, ctx: &mut Ctx, state: &SendState) {
    let palette = ctx.look.palette;

    match &state.step {
        Step::Idle | Step::Confirm { .. } | Step::Review { .. } => {}
        Step::Nonce { .. } => progress(ui, "Asking for the sender's nonce…"),
        Step::Proving { .. } => progress(ui, "Proving the sender's account at the final block: the node sees which…"),
        Step::Offer { .. } => progress(ui, "Asking the node for a fee offer…"),
        Step::Submit { .. } => progress(ui, "Submitting…"),
        Step::Waiting { id } => {
            let id = id.clone();
            ui.horizontal_wrapped(|ui| {
                ui.spinner();
                ui.label(match ctx.settings.read_blocks {
                    true => "Taken by the node, waiting for a final block read here:",
                    false => "Taken by the node, waiting for a block:",
                });
                super::tx(ui, ctx, &id);
            });
        }
        Step::Landed { id, failed } => {
            let (id, failed) = (id.clone(), failed.clone());
            ui.horizontal_wrapped(|ui| {
                match failed {
                    None => widgets::pill(ui, "in a block", palette.good),
                    Some(_) => widgets::pill(ui, "in a block, but it failed", palette.bad),
                };
                super::tx(ui, ctx, &id);

                if widgets::quiet(ui, &palette, "Open").clicked() {
                    ctx.go(View::Tx(id.clone()));
                }
            });

            if let Some(why) = failed {
                ui.colored_label(palette.bad, why);
            }
        }
        Step::Failed(err) => {
            ui.colored_label(palette.bad, err);
        }
    }
}

/// A deploy's code: a WebAssembly module, or Luau, typed or opened from its files.
fn code(ui: &mut Ui, look: super::Look, state: &mut SendState) {
    let palette = look.palette;
    ui.add_space(4.0);
    widgets::caps(ui, &palette, "Code");

    if files::can_pick(ui.ctx()) {
        ui.horizontal_wrapped(|ui| {
            if widgets::quiet(ui, &palette, "Load a WebAssembly module…").clicked() {
                pick(ui.ctx(), state.picked.clone(), Picking::Wasm);
            }

            if widgets::quiet(ui, &palette, "Open Luau files…")
                .on_hover_text("A contract split across files: open them all, and --@include \"name.lua\" lines take each by name")
                .clicked()
            {
                pick(ui.ctx(), state.picked.clone(), Picking::Lua);
            }
        });
    }

    if let Some((name, bytes)) = &state.wasm {
        let mut drop = false;
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{name}, {} bytes", format::thousands(&bytes.len().to_string()))).monospace());
            drop = ui.small_button("×").clicked();
        });

        if drop {
            state.wasm = None;
        }

        return;
    }

    ui.label(RichText::new(format!("Luau source, {}:", state.lua_name)).color(palette.muted));
    ui.add(egui::TextEdit::multiline(&mut state.lua).code_editor().desired_rows(if look.narrow { 8 } else { 12 }).desired_width(f32::INFINITY));
    included(ui, state);
}

fn key(ui: &mut Ui, ctx: &mut Ctx, state: &mut SendState) {
    let look = ctx.look;
    let palette = look.palette;

    match state.key.clone() {
        Some(key) => {
            let address = key.public().address().to_string();
            ui.horizontal_wrapped(|ui| {
                ui.label(format!("Signing with {} as", state.key_name));
                super::address(ui, ctx, &address);
            });

            match ctx.store.accounts.get(&address) {
                _ if ctx.settings.read_blocks => super::note(ui, look, "Sending takes a direct proof of this account, for its nonce: the node sees which. Nothing is asked until you send."),
                Some(account) => super::note(ui, look, format!("Holding {}, next nonce {}.", ctx.coins(&account.balance), account.nonce)),
                None => ctx.api.ask(Ask::Account(address.clone())),
            }

            if widgets::quiet(ui, &palette, "Forget the key").clicked() {
                state.key = None;
                state.key_name.clear();
            }
        }
        None => {
            super::note(ui, look, "A key as chainv4 keygen writes it. It stays in this app, and only signed transactions leave it.");
            ui.add_space(4.0);
            let pasted = widgets::beside(
                ui,
                |ui| widgets::quiet(ui, &palette, "Use it").clicked(),
                |ui, width| {
                    chainv4_ui::secret::secret(ui, egui::TextEdit::singleline(&mut *state.pasted).password(true).hint_text("ed25519:0x…").desired_width(width));
                },
            );

            if pasted {
                match Keypair::from_key_file(state.pasted.trim()) {
                    Ok(key) => {
                        state.key = Some(key);
                        state.key_name = "the key pasted".to_string();
                        state.pasted.clear();
                    }
                    Err(err) => state.step = Step::Failed(err),
                }
            }

            ui.horizontal_wrapped(|ui| {
                if files::can_pick(ui.ctx()) && widgets::quiet(ui, &palette, "Load a key file…").clicked() {
                    pick(ui.ctx(), state.picked.clone(), Picking::Key);
                }

                if widgets::quiet(ui, &palette, "Use the development key").on_hover_text("Its seed is in chainv4's source: only for dev chains").clicked() {
                    state.key = Some(Keypair::development());
                    state.key_name = "the development key".to_string();
                }
            });

            if let Step::Failed(err) = &state.step {
                ui.colored_label(palette.bad, err);
            }
        }
    }
}

fn arguments(ui: &mut Ui, palette: &chainv4_ui::Palette, args: &mut Vec<(String, String)>) {
    ui.vertical(|ui| {
        let mut remove = None;

        for (index, (kind, value)) in args.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt(("send_kind", index)).width(90.0).selected_text(kind.as_str()).show_ui(ui, |ui| {
                    for option in KINDS {
                        ui.selectable_value(kind, option.to_string(), option);
                    }
                });

                if ui.small_button("×").clicked() {
                    remove = Some(index);
                }

                ui.add(egui::TextEdit::singleline(value).desired_width(ui.available_width().min(320.0)));
            });
        }

        if let Some(index) = remove {
            args.remove(index);
        }

        if widgets::quiet(ui, palette, "+ argument").clicked() {
            args.push(("u256".to_string(), String::new()));
        }
    });
}

fn progress(ui: &mut Ui, text: &str) {
    ui.horizontal(|ui| {
        ui.spinner();
        ui.label(text);
    });
}

/// The transaction the form describes.
fn build(state: &SendState, key: &Keypair, decimals: u8) -> Result<Tx, String> {
    let sender = key.public().address();
    let amount = || -> Result<Amount, String> {
        match state.amount.trim() {
            "" => Ok(Amount::U128(0)),
            text => format::parse_units(text, decimals)
                .and_then(|raw| u256::from_str_radix(&raw, 10).ok())
                .map(Amount::U256)
                .ok_or_else(|| format!("{text} is not an amount with at most {decimals} decimals")),
        }
    };
    let to = || -> Result<Address, String> { state.to.trim().parse().map_err(|err| format!("{err}: {}", state.to.trim())) };

    match state.kind {
        Kind::Transfer => Ok(Tx::Transfer {
            from: sender,
            to: to()?,
            amount: amount()?,
        }),
        Kind::Burn => Ok(Tx::Burn {
            from: sender,
            amount: amount()?,
        }),
        Kind::Call => {
            if state.function.trim().is_empty() {
                return Err("name the function to call".to_string());
            }

            Ok(Tx::ContractCall {
                to: to()?,
                func: Hasher::digest(HashType::Xxh3_64, state.function.trim().as_bytes()),
                amount: amount()?,
                data: state.args.iter().map(|(kind, value)| argument(kind, value)).collect::<Result<_, _>>()?,
            })
        }
        Kind::Deploy => Ok(Tx::Deploy {
            code: match &state.wasm {
                Some((_, bytes)) => Code::Wasm(bytes.clone()),
                None => Code::Lua(lua_bundle(state)?.source),
            },
        }),
    }
}

/// The files the entry may include, each with a button to drop it, and what putting them together
/// gives: how big, or which include does not resolve.
fn included(ui: &mut Ui, state: &mut SendState) {
    if state.included.is_empty() {
        return;
    }

    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("with").color(ui.visuals().weak_text_color()));
        let mut dropped = None;

        for (index, (name, _)) in state.included.iter().enumerate() {
            ui.label(name);

            if ui.small_button("×").clicked() {
                dropped = Some(index);
            }
        }

        if let Some(index) = dropped {
            state.included.remove(index);
        }
    });

    match lua_bundle(state) {
        Ok(bundle) if bundle.files.len() > 1 => {
            ui.label(
                RichText::new(format!("Sends {} with {} put in: {} bytes", bundle.files[0], bundle.files[1..].join(", "), format::thousands(&bundle.source.len().to_string())))
                    .small()
                    .color(ui.visuals().weak_text_color()),
            );
        }
        Ok(_) => {
            ui.label(RichText::new("It includes none of them yet: a --@include \"name.lua\" line puts one in.").small().color(ui.visuals().weak_text_color()));
        }
        Err(why) => {
            ui.colored_label(ui.visuals().error_fg_color, why);
        }
    }
}

/// The Lua contract, put together: the editor's source, every --@include in it taken from the
/// files opened, by path or else by name, as a browser gives no folders.
fn lua_bundle(state: &SendState) -> Result<Bundle, String> {
    bundle(&state.lua_name, &state.lua, |path| lookup(&state.included, path)).map_err(|err| err.to_string())
}

/// A file opened, by the path an include names: that path, or its file name when one file has it.
fn lookup(files: &[(String, String)], path: &str) -> Result<String, String> {
    let name = path.rsplit('/').next().unwrap_or(path);
    let mut named = files.iter().filter(|(file, _)| file == name);
    let found = match files.iter().find(|(file, _)| file == path) {
        Some(file) => Some(file),
        None => match (named.next(), named.next()) {
            (Some(file), None) => Some(file),
            _ => None,
        },
    };

    found.map(|(_, text)| text.clone()).ok_or_else(|| format!("no file named {name} is open: open it with the others"))
}

/// Which of the files opened is the contract: the one that puts in the most of the others, the
/// first opened when none does.
fn entry(files: &[(String, String)]) -> usize {
    let pulls = |index: usize| {
        let (name, text) = &files[index];

        bundle(name, text, |path| lookup(files, path)).map_or(0, |bundle| bundle.files.len())
    };

    (0..files.len()).rev().max_by_key(|index| pulls(*index)).unwrap_or(0)
}

/// A call argument from its type and text. Amounts are whole units, like every other number.
fn argument(kind: &str, text: &str) -> Result<TxCallArgumentType, String> {
    let text = text.trim();
    let bad = || format!("not a {kind}: {text}");
    let wide = || u256::from_str_radix(text, 10).map_err(|_| bad());

    Ok(match kind {
        "address" => TxCallArgumentType::Address(text.parse().map_err(|_| bad())?),
        "amount" => TxCallArgumentType::Amount(Amount::U256(wide()?)),
        "u256" => TxCallArgumentType::U256(wide()?),
        "u128" => TxCallArgumentType::U128(text.parse().map_err(|_| bad())?),
        "u64" => TxCallArgumentType::U64(text.parse().map_err(|_| bad())?),
        "i64" => TxCallArgumentType::I64(text.parse().map_err(|_| bad())?),
        "i128" => TxCallArgumentType::I128(text.parse().map_err(|_| bad())?),
        "i256" => TxCallArgumentType::I256(i256::from_str_radix(text, 10).map_err(|_| bad())?),
        "number" => TxCallArgumentType::Number(text.parse().map_err(|_| bad())?),
        "bool" => TxCallArgumentType::Bool(text.parse().map_err(|_| bad())?),
        "string" => TxCallArgumentType::String(text.to_string()),
        "bytes" => TxCallArgumentType::Bytes(from_hex(text).ok_or_else(bad)?),
        "nil" => TxCallArgumentType::Nil,
        other => return Err(format!("unknown argument type {other}")),
    })
}

/// A fee offer as POST /offer answers it, and why its dry run says the transaction would fail, if
/// it does.
fn offer(value: &Value) -> Result<(FeeOffer, Option<String>), String> {
    let offer: Offer = serde_json::from_value(value.clone()).map_err(|err| format!("the node sent a bad offer: {err}"))?;
    let wide = |text: &str| u256::from_str_radix(text, 10).map_err(|_| format!("the node sent a bad amount: {text}"));
    let fee = FeeOffer {
        gas_limit: offer.gas_limit,
        max_price: wide(&offer.max_price)?,
        tip: wide(&offer.tip)?,
    };
    let warning = offer
        .estimate
        .filter(|estimate| estimate.status == "failed")
        .map(|estimate| estimate.error.unwrap_or_else(|| "no reason given".to_string()));

    Ok((fee, warning))
}

fn encode(signed: &SignedTx) -> String {
    to_hex(&wincode::serialize(signed).expect("transactions always encode"))
}

/// Opens a file picker, for several files when picking Lua's; what is picked arrives in picked,
/// read.
fn pick(ctx: &egui::Context, picked: Arc<Mutex<Option<Picked>>>, picking: Picking) {
    let ask = match picking {
        Picking::Lua => Pick {
            many: true,
            kind: Some(("Luau", &["lua", "luau"])),
            ..Pick::default()
        },
        Picking::Key | Picking::Wasm => Pick::default(),
    };

    files::pick(ctx, ask, move |files| {
        let read = match files {
            Err(why) => Picked::Failed(why),
            Ok(files) if files.is_empty() => return,
            Ok(files) => {
                let mut files: Vec<(String, Vec<u8>)> = files.into_iter().map(|file| (file.name, file.bytes)).collect();

                match picking {
                    Picking::Lua => Picked::Lua(files),
                    Picking::Key | Picking::Wasm => {
                        let (name, bytes) = files.swap_remove(0);

                        match picking {
                            Picking::Key => Picked::Key { name, bytes },
                            _ => Picked::Wasm { name, bytes },
                        }
                    }
                }
            }
        };
        *picked.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(read);
    });
}

fn take_picked(state: &mut SendState) {
    let picked = state.picked.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();

    match picked {
        // The file's bytes, and its text, are wiped once read.
        Some(Picked::Key { name, bytes }) => match String::from_utf8(bytes).map(zeroize::Zeroizing::new).map_err(|err| err.to_string()).and_then(|text| Keypair::from_key_file(&text)) {
            Ok(key) => {
                state.key = Some(key);
                state.key_name = name;
            }
            Err(err) => state.step = Step::Failed(format!("{name}: {err}")),
        },
        Some(Picked::Wasm { name, bytes }) => state.wasm = Some((name, bytes)),
        Some(Picked::Lua(files)) => {
            let read: Result<Vec<(String, String)>, String> = files
                .into_iter()
                .map(|(name, bytes)| String::from_utf8(bytes).map(|text| (name.clone(), text)).map_err(|_| format!("{name} is not text")))
                .collect();

            match read {
                Ok(mut files) => {
                    let (name, text) = files.remove(entry(&files));
                    state.lua_name = name;
                    state.lua = text;
                    state.included = files;
                    state.wasm = None;
                }
                Err(why) => state.step = Step::Failed(why),
            }
        }
        Some(Picked::Failed(why)) => state.step = Step::Failed(why),
        None => {}
    }
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::light::Light,
        crate::reading::{ProvenValue, Reading},
        crate::reading::tests::{SCREENS, drawn_in},
        crate::store::Store,
        chainv4::db::{Column, address_key},
    };

    /// A state with a key, about to send a transfer, as Sign and send left it with the proof asked for.
    fn proving(key: &Keypair) -> SendState {
        let tx = Tx::Transfer { from: key.public().address(), to: key.public().address(), amount: Amount::U128(5) };

        SendState { key: Some(key.clone()), key_name: "a key".to_string(), step: Step::Proving { tx }, ..SendState::default() }
    }

    fn proven_nonce(reading: &mut Reading, key: &Keypair, nonce: u64) {
        let account = chainv4::structs::account::Account { nonce, balance: u256::from(900u64) };

        reading.has_proven(Column::Accounts, &address_key(&key.public().address()), ProvenValue { value: Some(wincode::serialize(&account).unwrap()), height: 40, block: "0x40".to_string() });
    }

    #[test]
    fn with_the_setting_on_it_goes_on_a_proven_nonce_and_a_fee_from_blocks_says_so_and_fits_a_phone_either_way() {
        let key = Keypair::from_seed([4; 32]);

        for size in SCREENS {
            let mut reading = Reading::default();
            let client = crate::app::tests::following_client();
            let genesis = client.sealed_at(0).unwrap().id().to_string();

            assert!(reading.open(&genesis, client.params().hash, &client, 0.0));
            proven_nonce(&mut reading, &key, 3);

            let mut state = proving(&key);
            let mut light = Light::following_with(client);
            let (width, _, _) = drawn_in(&Store::default(), &mut reading, &mut light, size, true, |ui, ctx| show(ui, ctx, &mut state));
            let (_, said, asked) = drawn_in(&Store::default(), &mut reading, &mut light, (size.0, 8_000.0), true, |ui, ctx| show(ui, ctx, &mut state));

            assert!(width <= size.0, "{width} wide, on {size:?}");
            assert!(said.contains("Nonce 3, proven at final block 40") && said.contains("the node saw this account") && said.contains("Nothing tries it first") && said.contains("still costs its fee"), "{said}");
            assert!(matches!(state.step, Step::Review { nonce: 3, at: 40, .. }));
            assert!(!asked.iter().any(|ask| matches!(ask, Ask::Account(_) | Ask::Offer(_))), "no node is asked: {asked:?}");
        }
    }

    #[test]
    fn a_nonce_sent_from_here_is_counted_though_no_final_block_shows_it_yet() {
        let key = Keypair::from_seed([4; 32]);
        let mut reading = Reading::default();
        let client = crate::app::tests::following_client();
        let genesis = client.sealed_at(0).unwrap().id().to_string();

        assert!(reading.open(&genesis, client.params().hash, &client, 0.0));
        proven_nonce(&mut reading, &key, 3);

        let mut state = proving(&key);
        state.sent_nonce = Some((key.public().address().to_string(), 7));
        let mut light = Light::following_with(client);
        drawn_in(&Store::default(), &mut reading, &mut light, (1200.0, 800.0), true, |ui, ctx| show(ui, ctx, &mut state));

        assert!(matches!(state.step, Step::Review { nonce: 7, .. }));
    }

    #[test]
    fn with_the_setting_on_a_deploy_is_not_offered_as_it_needs_a_dry_run() {
        let key = Keypair::from_seed([4; 32]);
        let mut state = SendState { key: Some(key), ..SendState::default() };
        let (_, said, _) = drawn_in(&Store::default(), &mut Reading::default(), &mut Light::default(), (1200.0, 800.0), true, |ui, ctx| show(ui, ctx, &mut state));

        assert!(!said.contains("Deploy") && said.contains("suggested from checked blocks"), "{said}");

        let mut state = SendState { key: Some(Keypair::from_seed([4; 32])), ..SendState::default() };
        let (_, said, asked) = drawn_in(&Store::default(), &mut Reading::default(), &mut Light::default(), (1200.0, 800.0), false, |ui, ctx| show(ui, ctx, &mut state));

        assert!(said.contains("Deploy") && said.contains("worked out by the node"), "{said}");
        assert!(asked.iter().any(|ask| matches!(ask, Ask::Account(_))), "with it off, as it always did: {asked:?}");
    }

    #[test]
    fn forms_become_the_transactions_they_describe() {
        let key = Keypair::development();
        let mut state = SendState {
            to: "contract:0x0101010101010101010101010101010101010101010101010101010101010101".to_string(),
            amount: "1.5".to_string(),
            function: "transfer".to_string(),
            args: vec![("u256".to_string(), "7".to_string()), ("bool".to_string(), "true".to_string())],
            kind: Kind::Call,
            ..SendState::default()
        };

        let Tx::ContractCall { func, amount, data, .. } = build(&state, &key, 18).unwrap() else {
            panic!("not a call");
        };

        assert_eq!(func, Hasher::digest(HashType::Xxh3_64, b"transfer"));
        assert_eq!(amount.to_u256(), u256::from(15u64) * u256::from(10u64).pow(17));
        assert_eq!(data, [TxCallArgumentType::U256(u256::from(7u64)), TxCallArgumentType::Bool(true)]);

        state.kind = Kind::Transfer;
        state.amount = "1.0000000000000000001".to_string();

        assert!(build(&state, &key, 18).unwrap_err().contains("at most 18 decimals"));

        state.args = vec![("u64".to_string(), "-1".to_string())];
        state.kind = Kind::Call;
        state.amount.clear();

        assert!(build(&state, &key, 18).unwrap_err().contains("not a u64"));
    }

    /// Sends a transfer with the development key to the node in CHAINV4_NODE, through the same
    /// steps the panel takes, and waits until it is in a block.
    #[test]
    #[ignore = "needs a dev node: CHAINV4_NODE=http://127.0.0.1:8080 cargo test -- --ignored"]
    fn a_transfer_goes_through_a_real_node() {
        let node = std::env::var("CHAINV4_NODE").expect("CHAINV4_NODE names a dev node");
        let mut api = crate::api::Api::new(&node, Ok(chainv4_ui::net::Client::direct()), egui::Context::default());
        let key = Keypair::development();
        let mut state = SendState {
            key: Some(key.clone()),
            to: Keypair::generate(key.public().key_type()).unwrap().public().address().to_string(),
            amount: "0.25".to_string(),
            ..SendState::default()
        };
        state.step = Step::Nonce {
            tx: build(&state, &key, 18).unwrap(),
        };
        api.ask(Ask::Account(key.public().address().to_string()));
        let started = std::time::Instant::now();

        while !matches!(state.step, Step::Landed { .. }) {
            assert!(started.elapsed() < std::time::Duration::from_secs(30), "it never landed");

            if let Step::Failed(err) = &state.step {
                panic!("{err}");
            }

            if let Some(id) = state.waiting_for() {
                api.ask(Ask::Tx(id.to_string()));
            }

            for reply in api.replies() {
                if let crate::api::Reply::Answer { ask, result } = reply {
                    state.answered(&ask, &result, &mut api);
                }
            }

            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }

    #[test]
    fn a_contract_opened_as_files_is_put_together_before_it_is_signed() {
        let files = vec![
            ("token.lua".to_string(), "local M = {}\n--@include \"lib/balances.lua\"\nreturn M\n".to_string()),
            ("balances.lua".to_string(), "local balances = 1\n".to_string()),
        ];
        let reversed: Vec<_> = files.iter().rev().cloned().collect();

        assert_eq!(entry(&files), 0, "the file that puts the other in");
        assert_eq!(entry(&reversed), 1, "whichever was opened first");

        let key = Keypair::development();
        let mut state = SendState {
            kind: Kind::Deploy,
            key: Some(key.clone()),
            lua_name: files[0].0.clone(),
            lua: files[0].1.clone(),
            included: files[1..].to_vec(),
            ..SendState::default()
        };
        let Ok(Tx::Deploy { code: Code::Lua(source) }) = build(&state, &key, 18) else {
            panic!("not a Lua deploy");
        };

        assert!(source.contains("-- included lib/balances.lua\nlocal balances = 1\n"), "found by its file name: {source}");

        // A deploy that lands and fails says where, in the files it came from.
        let mut api = crate::api::Api::new("http://127.0.0.1:0", Ok(chainv4_ui::net::Client::direct()), egui::Context::default());
        state.bundle = lua_bundle(&state).ok();
        state.step = Step::Waiting { id: "0x01".to_string() };
        state.answered(&Ask::Tx("0x01".to_string()), &Ok(json!({ "status": "failed", "error": "compile error: syntax error: 3: oops" })), &mut api);

        assert!(matches!(&state.step, Step::Landed { failed: Some(why), .. } if why == "compile error: syntax error: lib/balances.lua:1: oops"));

        state.included.clear();

        assert_eq!(build(&state, &key, 18).unwrap_err(), "token.lua:2: cannot include lib/balances.lua: no file named balances.lua is open: open it with the others");
        assert!(lookup(&[("a.lua".to_string(), String::new()), ("lib/a.lua".to_string(), "x".to_string())], "lib/a.lua").is_ok_and(|text| text == "x"), "a path first");
    }

    #[test]
    fn an_offer_from_the_node_becomes_a_fee_offer() {
        let (fee, warning) = offer(&json!({ "gas_limit": 21000, "max_price": "2000000000", "tip": "3" })).unwrap();

        assert_eq!((fee.gas_limit, fee.max_price, fee.tip), (21000, u256::from(2_000_000_000u64), u256::from(3u64)));
        assert_eq!(warning, None);
        assert!(offer(&json!({ "gas_limit": 1, "max_price": "x", "tip": "0" })).is_err());

        let ok = json!({ "gas_limit": 30000, "max_price": "1", "tip": "0", "estimate": { "gas_used": 24000, "fuel_used": 9, "status": "ok", "error": null } });
        let failing = json!({ "gas_limit": 10000000, "max_price": "1", "tip": "0", "estimate": { "gas_used": 0, "fuel_used": 0, "status": "failed", "error": "aborted: not enough" } });

        assert_eq!(offer(&ok).unwrap().1, None);
        assert_eq!(offer(&failing).unwrap().1.as_deref(), Some("aborted: not enough"));
    }
}
