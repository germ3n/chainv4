//! Signing and sending transactions from the explorer. The key stays in this window: transactions
//! are built and signed here with chainv4's own code, and only the signed bytes go to the node.
//! The node works out the fee offer, as `chainv4 send` does, for a probe signed with none.

use {
    super::{BAD, Ctx, GOOD, View},
    crate::api::{Ask, Failure},
    crate::format,
    crate::model::{Account, Offer},
    chainv4::crypto::hasher::Hasher,
    chainv4::crypto::keypair::Keypair,
    chainv4::structs::address::Address,
    chainv4::structs::amount::Amount,
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

/// A file the person picked, read.
enum Picked {
    Key { name: String, bytes: Vec<u8> },
    Code { name: String, bytes: Vec<u8> },
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
    Landed { id: String },
    Failed(String),
}

pub struct SendState {
    key: Option<Keypair>,
    key_name: String,
    picked: Arc<Mutex<Option<Picked>>>,
    kind: Kind,
    to: String,
    amount: String,
    function: String,
    args: Vec<(String, String)>,
    lua: String,
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
            picked: Arc::default(),
            kind: Kind::Transfer,
            to: String::new(),
            amount: String::new(),
            function: String::new(),
            args: Vec::new(),
            lua: "local M = {}\n\nfunction M.hello()\n    return \"hi\"\nend\n\nreturn M\n".to_string(),
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
            (Step::Waiting { id }, Ask::Tx(asked), Ok(_)) if *asked == id => Step::Landed { id },
            (step, _, _) => step,
        };
    }
}

pub fn show(ui: &mut Ui, ctx: &mut Ctx, state: &mut SendState) {
    take_picked(state);
    super::title(ui, "Send a transaction");
    key(ui, ctx, state);

    let Some(key) = state.key.clone() else {
        return;
    };

    ui.separator();
    ui.horizontal(|ui| {
        for (kind, name) in [(Kind::Transfer, "Transfer"), (Kind::Call, "Call a contract"), (Kind::Deploy, "Deploy"), (Kind::Burn, "Burn")] {
            ui.selectable_value(&mut state.kind, kind, name);
        }
    });
    ui.add_space(4.0);

    let (decimals, symbol) = ctx.coin();

    super::facts(ui, "send", |ui| {
        if matches!(state.kind, Kind::Transfer | Kind::Call) {
            super::fact(ui, if state.kind == Kind::Call { "contract" } else { "to" });
            ui.add(egui::TextEdit::singleline(&mut state.to).hint_text("ed25519:0x… or contract:0x…").desired_width(420.0));
            ui.end_row();
        }

        if state.kind == Kind::Call {
            super::fact(ui, "function");
            ui.text_edit_singleline(&mut state.function);
            ui.end_row();
            super::fact(ui, "arguments");
            arguments(ui, &mut state.args);
            ui.end_row();
        }

        if state.kind != Kind::Deploy {
            super::fact(ui, if state.kind == Kind::Call { "value" } else { "amount" });
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut state.amount).hint_text("0").desired_width(160.0));
                ui.label(&symbol);
            });
            ui.end_row();
        }

        if state.kind == Kind::Deploy {
            super::fact(ui, "code");
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    if ui.button("Load a WebAssembly module…").clicked() {
                        pick(ui.ctx(), state.picked.clone(), false);
                    }

                    if let Some((name, bytes)) = &state.wasm {
                        ui.label(format!("{name}, {} bytes", format::thousands(&bytes.len().to_string())));

                        if ui.small_button("x").clicked() {
                            state.wasm = None;
                        }
                    }
                });

                if state.wasm.is_none() {
                    ui.weak("or Luau source:");
                    ui.add(egui::TextEdit::multiline(&mut state.lua).code_editor().desired_rows(10).desired_width(f32::INFINITY));
                }
            });
            ui.end_row();
        }
    });

    egui::CollapsingHeader::new("Fee (worked out by the node unless set)").show(ui, |ui| {
        super::facts(ui, "fee", |ui| {
            for (name, value, hint) in [
                ("gas limit", &mut state.gas_limit, "auto"),
                ("max price per gas", &mut state.max_price, "auto, in units"),
                ("tip per gas", &mut state.tip, "0, in units"),
            ] {
                super::fact(ui, name);
                ui.add(egui::TextEdit::singleline(value).hint_text(hint).desired_width(200.0));
                ui.end_row();
            }
        });
    });

    ui.add_space(6.0);
    let busy = matches!(state.step, Step::Nonce { .. } | Step::Offer { .. } | Step::Submit { .. });

    if ui.add_enabled(!busy, egui::Button::new(RichText::new("Sign and send").strong())).clicked() {
        match build(state, &key, decimals) {
            Ok(tx) => {
                ctx.api.ask(Ask::Account(key.public().address().to_string()));
                state.step = Step::Nonce { tx };
            }
            Err(err) => state.step = Step::Failed(err),
        }
    }

    match &state.step {
        Step::Idle => {}
        Step::Nonce { .. } => progress(ui, "Asking for the sender's nonce…"),
        Step::Offer { .. } => progress(ui, "Asking the node for a fee offer…"),
        Step::Submit { .. } => progress(ui, "Submitting…"),
        Step::Waiting { id } => {
            let id = id.clone();
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Taken by the node, waiting for a block:");
                super::tx(ui, ctx, &id);
            });
        }
        Step::Landed { id } => {
            let id = id.clone();
            ui.horizontal(|ui| {
                ui.colored_label(GOOD, "In a block:");
                super::tx(ui, ctx, &id);

                if ui.button("Open").clicked() {
                    ctx.go(View::Tx(id.clone()));
                }
            });
        }
        Step::Failed(err) => {
            ui.colored_label(BAD, err);
        }
    }
}

fn key(ui: &mut Ui, ctx: &mut Ctx, state: &mut SendState) {
    ui.horizontal(|ui| {
        if ui.button("Load a key file…").clicked() {
            pick(ui.ctx(), state.picked.clone(), true);
        }

        if ui.button("Use the development key").on_hover_text("Its seed is in chainv4's source: only for dev chains").clicked() {
            state.key = Some(Keypair::development());
            state.key_name = "the development key".to_string();
        }

        if state.key.is_some() && ui.button("Forget the key").clicked() {
            state.key = None;
            state.key_name.clear();
        }
    });

    match &state.key {
        Some(key) => {
            let address = key.public().address().to_string();
            ui.horizontal(|ui| {
                ui.label(format!("Signing with {} as", state.key_name));
                super::address(ui, ctx, &address);

                match ctx.store.accounts.get(&address) {
                    Some(account) => {
                        ui.weak(format!("holding {}, next nonce {}", ctx.coins(&account.balance), account.nonce));
                    }
                    None => ctx.api.ask(Ask::Account(address.clone())),
                }
            });
            ui.weak("The key stays in this window. Only signed transactions leave it.");
        }
        None => {
            ui.weak("Load a key file written by chainv4 keygen. The key stays in this window, and only signed transactions leave it.");
        }
    }
}

fn arguments(ui: &mut Ui, args: &mut Vec<(String, String)>) {
    ui.vertical(|ui| {
        let mut remove = None;

        for (index, (kind, value)) in args.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt(("send_kind", index)).selected_text(kind.as_str()).show_ui(ui, |ui| {
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
            args.remove(index);
        }

        if ui.small_button("+ argument").clicked() {
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
                None => Code::Lua(state.lua.clone()),
            },
        }),
    }
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

/// Opens a file picker; what is picked arrives in picked, read.
fn pick(wake: &egui::Context, picked: Arc<Mutex<Option<Picked>>>, key: bool) {
    let wake = wake.clone();
    let finish = move |name: String, bytes: Vec<u8>| {
        *picked.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(match key {
            true => Picked::Key { name, bytes },
            false => Picked::Code { name, bytes },
        });
        wake.request_repaint();
    };

    #[cfg(not(target_arch = "wasm32"))]
    if let Some(path) = rfd::FileDialog::new().pick_file()
        && let Ok(bytes) = std::fs::read(&path)
    {
        finish(path.file_name().map_or_else(String::new, |name| name.to_string_lossy().into_owned()), bytes);
    }

    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_futures::spawn_local(async move {
        if let Some(file) = rfd::AsyncFileDialog::new().pick_file().await {
            let bytes = file.read().await;
            finish(file.file_name(), bytes);
        }
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
        Some(Picked::Code { name, bytes }) => state.wasm = Some((name, bytes)),
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
    fn an_offer_from_the_node_becomes_a_fee_offer() {
        let fee = offer(&json!({ "gas_limit": 21000, "max_price": "2000000000", "tip": "3" })).unwrap();

        assert_eq!((fee.gas_limit, fee.max_price, fee.tip), (21000, u256::from(2_000_000_000u64), u256::from(3u64)));
        assert!(offer(&json!({ "gas_limit": 1, "max_price": "x", "tip": "0" })).is_err());
    }
}
