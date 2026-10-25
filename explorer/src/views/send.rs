//! Signing and sending transactions from the explorer. The key stays in this window: transactions
//! are built and signed here with chainv4's own code, and only the signed bytes go to the node.
//! The node works out the fee offer, as `chainv4 send` does, for a probe signed with none.

use {
    super::{Ctx, View},
    chainv4_ui::{layout::CAN_PICK_FILES, widgets},
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
    chainv4::structs::u256::u256,
    egui::{RichText, Ui},
    serde_json::{Value, json},
    std::sync::{Arc, Mutex},
};

/// The argument types a call can be given.
const KINDS: [&str; 12] = ["address", "amount", "u256", "u128", "u64", "i64", "i128", "number", "bool", "string", "bytes", "nil"];

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

/// What the person picked, read.
enum Picked {
    Key { name: String, bytes: Vec<u8> },
    Wasm { name: String, bytes: Vec<u8> },
    /// A Lua contract's files, by name.
    Lua(Vec<(String, Vec<u8>)>),
}

/// Where sending stands.
enum Step {
    Idle,
    /// Asked for the sender's nonce.
    Nonce { tx: Tx },
    /// Asked the node for an offer.
    Offer { tx: Tx, nonce: u64, ask: Ask },
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
    /// A key's text, pasted, as chainv4 keygen writes it.
    pasted: String,
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
}

impl Default for SendState {
    fn default() -> SendState {
        SendState {
            key: None,
            key_name: String::new(),
            pasted: String::new(),
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
                    Ok(fee) => {
                        let signed = SignedTx::sign(tx, nonce, fee, key);
                        let ask = Ask::Submit(json!({ "tx": encode(&signed) }).to_string());
                        api.ask(ask.clone());

                        Step::Submit { ask }
                    }
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
    super::section(ui, look, "Signing key", |ui| key(ui, ctx, state));

    let Some(key) = state.key.clone() else {
        return;
    };
    let (decimals, symbol) = ctx.coin();
    let field = if look.narrow { f32::INFINITY } else { 460.0 };

    super::section(ui, look, "Transaction", |ui| {
        ui.horizontal_wrapped(|ui| {
            for (kind, name) in [(Kind::Transfer, "Transfer"), (Kind::Call, "Call"), (Kind::Deploy, "Deploy"), (Kind::Burn, "Burn")] {
                ui.selectable_value(&mut state.kind, kind, name);
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
        egui::CollapsingHeader::new(RichText::new("Fee: worked out by the node unless set").color(palette.soft)).show(ui, |ui| {
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
        let busy = matches!(state.step, Step::Nonce { .. } | Step::Offer { .. } | Step::Submit { .. });

        if widgets::primary(ui, &palette, "Sign and send", !busy).clicked() {
            state.bundle = match (state.kind, &state.wasm) {
                (Kind::Deploy, None) => lua_bundle(state).ok(),
                _ => None,
            };

            match build(state, &key, decimals) {
                Ok(tx) => {
                    ctx.api.ask(Ask::Account(key.public().address().to_string()));
                    state.step = Step::Nonce { tx };
                }
                Err(err) => state.step = Step::Failed(err),
            }
        }

        ui.add_space(4.0);
        outcome(ui, ctx, state);
    });
}

/// Where sending stands, under the button.
fn outcome(ui: &mut Ui, ctx: &mut Ctx, state: &SendState) {
    let palette = ctx.look.palette;

    match &state.step {
        Step::Idle => {}
        Step::Nonce { .. } => progress(ui, "Asking for the sender's nonce…"),
        Step::Offer { .. } => progress(ui, "Asking the node for a fee offer…"),
        Step::Submit { .. } => progress(ui, "Submitting…"),
        Step::Waiting { id } => {
            let id = id.clone();
            ui.horizontal_wrapped(|ui| {
                ui.spinner();
                ui.label("Taken by the node, waiting for a block:");
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

    if CAN_PICK_FILES {
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
            drop = ui.small_button("✕").clicked();
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
                    ui.add(egui::TextEdit::singleline(&mut state.pasted).password(true).hint_text("ed25519:0x…").desired_width(width));
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
                if CAN_PICK_FILES && widgets::quiet(ui, &palette, "Load a key file…").clicked() {
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

                if ui.small_button("✕").clicked() {
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

            if ui.small_button("x").clicked() {
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
        "number" => TxCallArgumentType::Number(text.parse().map_err(|_| bad())?),
        "bool" => TxCallArgumentType::Bool(text.parse().map_err(|_| bad())?),
        "string" => TxCallArgumentType::String(text.to_string()),
        "bytes" => TxCallArgumentType::Bytes(from_hex(text).ok_or_else(bad)?),
        "nil" => TxCallArgumentType::Nil,
        other => return Err(format!("unknown argument type {other}")),
    })
}

fn offer(value: &Value) -> Result<FeeOffer, String> {
    let offer: Offer = serde_json::from_value(value.clone()).map_err(|err| format!("the node sent a bad offer: {err}"))?;
    let wide = |text: &str| u256::from_str_radix(text, 10).map_err(|_| format!("the node sent a bad amount: {text}"));

    Ok(FeeOffer {
        gas_limit: offer.gas_limit,
        max_price: wide(&offer.max_price)?,
        tip: wide(&offer.tip)?,
    })
}

fn encode(signed: &SignedTx) -> String {
    to_hex(&wincode::serialize(signed).expect("transactions always encode"))
}

/// Opens a file picker, for several files when picking Lua's; what is picked arrives in picked,
/// read.
fn pick(wake: &egui::Context, picked: Arc<Mutex<Option<Picked>>>, picking: Picking) {
    let wake = wake.clone();
    let finish = move |mut files: Vec<(String, Vec<u8>)>| {
        if files.is_empty() {
            return;
        }

        let read = match picking {
            Picking::Lua => Picked::Lua(files),
            Picking::Key | Picking::Wasm => {
                let (name, bytes) = files.swap_remove(0);

                match picking {
                    Picking::Key => Picked::Key { name, bytes },
                    _ => Picked::Wasm { name, bytes },
                }
            }
        };
        *picked.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(read);
        wake.request_repaint();
    };

    // Android has no picker an egui app can reach yet: CAN_PICK_FILES keeps its buttons away.
    #[cfg(target_os = "android")]
    drop(finish);

    #[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
    {
        let dialog = match picking {
            Picking::Lua => rfd::FileDialog::new().add_filter("Luau", &["lua", "luau"]),
            Picking::Key | Picking::Wasm => rfd::FileDialog::new(),
        };
        let paths = match picking {
            Picking::Lua => dialog.pick_files().unwrap_or_default(),
            Picking::Key | Picking::Wasm => dialog.pick_file().into_iter().collect(),
        };
        let files = paths
            .iter()
            .filter_map(|path| Some((path.file_name()?.to_string_lossy().into_owned(), std::fs::read(path).ok()?)))
            .collect();
        finish(files);
    }

    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_futures::spawn_local(async move {
        let handles = match picking {
            Picking::Lua => rfd::AsyncFileDialog::new().add_filter("Luau", &["lua", "luau"]).pick_files().await.unwrap_or_default(),
            Picking::Key | Picking::Wasm => rfd::AsyncFileDialog::new().pick_file().await.into_iter().collect(),
        };
        let mut files = Vec::new();

        for handle in handles {
            files.push((handle.file_name(), handle.read().await));
        }

        finish(files);
    });
}

fn take_picked(state: &mut SendState) {
    let picked = state.picked.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();

    match picked {
        Some(Picked::Key { name, bytes }) => match String::from_utf8(bytes).map_err(|err| err.to_string()).and_then(|text| Keypair::from_key_file(&text)) {
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
        None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let mut api = crate::api::Api::new(&node, egui::Context::default());
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
        let mut api = crate::api::Api::new("http://127.0.0.1:0", egui::Context::default());
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
        let fee = offer(&json!({ "gas_limit": 21000, "max_price": "2000000000", "tip": "3" })).unwrap();

        assert_eq!((fee.gas_limit, fee.max_price, fee.tip), (21000, u256::from(2_000_000_000u64), u256::from(3u64)));
        assert!(offer(&json!({ "gas_limit": 1, "max_price": "x", "tip": "0" })).is_err());
    }
}
