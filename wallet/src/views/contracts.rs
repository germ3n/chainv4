//! Contracts: keeping the ones the account talks to, reading them, calling them and deploying new
//! ones, of every engine. A native contract, Lua or WebAssembly, lists its functions itself, as the
//! node says, and takes arguments as type:value. An EVM contract is described by its ABI, pasted
//! once and kept here, which gives each function typed fields and reads what comes back.
//!
//! Reads run on the node, as a dry run that sends nothing: POST /call for a native contract, and
//! POST /offer's dry run for an EVM one. Calls and deploys go to the Send page, prepared, for its
//! review and signing, as names do.

use {
    super::send::{Prepared, SendState, next_nonce},
    super::{Ctx, Page},
    crate::airgap,
    crate::app::SavedContract,
    crate::format,
    crate::node::{Ask, Failure},
    crate::theme,
    chainv4::abi::{Abi, AbiFunction, AbiValue, evm_address, evm_create_address, parse_value},
    chainv4::crypto::hasher::Hasher,
    chainv4::structs::address::{Address, contract_address},
    chainv4::structs::amount::Amount,
    chainv4::structs::code::Code,
    chainv4::structs::hash::HashType,
    chainv4::structs::hex::{from_hex, to_hex},
    chainv4::structs::tx::{FeeOffer, SignedTx, Tx, TxCallArgumentType},
    chainv4::structs::u256::u256,
    chainv4_ui::files::{self, Pick},
    egui::{RichText, Ui},
    serde_json::{Value, json},
    std::collections::HashMap,
    std::sync::{Arc, Mutex},
};

/// What a native contract's record says, as the node said it.
#[derive(Clone, Debug)]
struct Record {
    engine: String,
    functions: Vec<String>,
}

/// Which engine a deploy is for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Engine {
    #[default]
    Lua,
    Wasm,
    Evm,
}

/// A file picked for a deploy, once it arrives.
type Picked = Arc<Mutex<Option<Result<(String, Vec<u8>), String>>>>;

#[derive(Default)]
pub struct ContractsState {
    /// The address reads and calls go from: one of the account's.
    from: Option<Address>,
    /// The contract open, by address.
    open: Option<String>,
    add_address: String,
    add_name: String,
    add_abi: String,
    /// Each function's fields, by contract and function: one per argument, as typed.
    fields: HashMap<(String, String), Vec<String>>,
    /// What each function sends with it, by contract and function.
    values: HashMap<(String, String), String>,
    /// What each read came to, by contract and function.
    results: HashMap<(String, String), Result<String, String>>,
    /// A native function's call as an EVM tool sends it, once asked for: the precompile's
    /// address and the calldata, in hex.
    calldata: HashMap<(String, String), (String, String)>,
    /// Which function each read out is for.
    asked: HashMap<Ask, (String, String)>,
    /// Native contracts' records, by address: None while asked, or for no contract.
    records: HashMap<String, Option<Record>>,
    engine: Engine,
    /// A Lua source, or an EVM contract's init code in hex, as pasted or picked.
    code: String,
    /// A WebAssembly module picked.
    wasm: Option<(String, Vec<u8>)>,
    deploy_abi: String,
    deploy_name: String,
    constructor: Vec<String>,
    picked: Picked,
    trouble: Option<String>,
}

impl ContractsState {
    /// Takes the node's answer to a read, or to the record of a contract this page asked about.
    pub fn answered(&mut self, ask: &Ask, result: &Result<Value, Failure>, abis: &[SavedContract]) {
        if let Ask::Contract(address) = ask {
            let record = result.as_ref().ok().map(|value| Record {
                engine: value["engine"].as_str().unwrap_or_default().to_string(),
                functions: value["functions"].as_array().into_iter().flatten().filter_map(|name| name.as_str().map(str::to_string)).collect(),
            });
            self.records.insert(address.clone(), record);

            return;
        }

        let Some(key) = self.asked.remove(ask) else {
            return;
        };
        let shown = match (ask, result) {
            (_, Err(failure)) => Err(failure.message.clone()),
            (Ask::Call(_), Ok(value)) => match value["status"].as_str() {
                Some("returned") => Ok(format!("{}\n{}", native_values(&value["values"]), checked_words(value))),
                _ => Err(format!("{}\n{}", value["error"].as_str().unwrap_or("it failed"), checked_words(value))),
            },
            (Ask::Read(_), Ok(value)) => evm_result(&key, &value["estimate"], abis),
            _ => return,
        };
        self.results.insert(key, shown);
    }
}

/// Whose answer a read's is: run here over state the node proved, at which block; or the node's,
/// and why.
pub fn checked_words(value: &Value) -> String {
    let checked = &value["checked"];

    match (checked["here"].as_bool(), checked["differed"].as_bool()) {
        (Some(true), Some(true)) => format!("Run here over state proven at block {}: the node answered otherwise, and is not to be trusted.", checked["height"]),
        (Some(true), _) => format!("Run here over state proven at block {}.", checked["height"]),
        (Some(false), _) => format!("The node's answer: {}.", checked["why"].as_str().unwrap_or("it could not be run here")),
        _ => "The node's answer: the browser runs no contracts of its own.".to_string(),
    }
}

/// A native read's values, for a person: each with its type.
fn native_values(values: &Value) -> String {
    let shown: Vec<String> = values
        .as_array()
        .into_iter()
        .flatten()
        .map(|value| format!("{} {}", value["type"].as_str().unwrap_or("?"), value["value"].to_string().trim_matches('"')))
        .collect();

    match shown.is_empty() {
        true => "nothing".to_string(),
        false => shown.join(", "),
    }
}

/// An EVM read's dry run, read by the function's ABI: its outputs, or why it reverted.
fn evm_result((address, signature): &(String, String), estimate: &Value, abis: &[SavedContract]) -> Result<String, String> {
    let abi = abis.iter().find(|saved| saved.address == *address).and_then(|saved| Abi::parse(&saved.abi).ok()).unwrap_or_default();
    let function = abi.functions.iter().find(|function| function.signature() == *signature);

    if estimate["status"] == "failed" {
        let error = estimate["error"].as_str().unwrap_or("it failed");
        // A custom error comes back as its bytes, which the ABI may name.
        let named = error
            .strip_prefix("reverted: ")
            .and_then(from_hex)
            .and_then(|bytes| abi.revert_reason(&bytes))
            .map(|reason| format!("reverted: {reason}"));

        return Err(named.unwrap_or_else(|| error.to_string()));
    }

    let output = estimate["returned"][0]["value"].as_str().and_then(from_hex).unwrap_or_default();

    match function.map(|function| function.decode_output(&output).map(|values| function.show_outputs(&values))) {
        Some(Ok(shown)) if !shown.is_empty() => Ok(shown),
        Some(Ok(_)) => Ok("nothing".to_string()),
        Some(Err(why)) => Err(format!("{}: {why}", to_hex(&output))),
        None => Ok(to_hex(&output)),
    }
}

pub fn show(ui: &mut Ui, ctx: &mut Ctx, state: &mut ContractsState, send: &mut SendState) {
    let palette = *ctx.palette;
    theme::title(ui, &palette, "Contracts", "Read, call and deploy contracts: Lua and WebAssembly ones by their functions, EVM ones by their ABI.");

    let addresses = ctx.holder().map(|holder| holder.addresses()).unwrap_or_default();

    if !state.from.is_some_and(|from| addresses.contains(&from)) {
        state.from = addresses.first().copied();
    }

    let Some(from) = state.from else {
        return;
    };

    if addresses.len() > 1 {
        theme::card(ui, &palette, |ui| {
            theme::caps(ui, &palette, "From");
            egui::ComboBox::from_id_salt("contracts-from")
                .selected_text(super::from::label(ctx, &from))
                .width(ui.available_width())
                .show_ui(ui, |ui| {
                    for address in &addresses {
                        ui.selectable_value(&mut state.from, Some(*address), super::from::label(ctx, address));
                    }
                });
        });
    }

    let mut prepared = None;

    saved(ui, ctx, state);

    if let Some(open) = state.open.clone()
        && let Some(contract) = ctx.settings.contracts.iter().find(|saved| saved.address == open).cloned()
    {
        prepared = opened(ui, ctx, state, &from, &contract);
    }

    add(ui, ctx, state);
    prepared = prepared.or_else(|| deploy(ui, ctx, state, &from));

    if let Some(prepared) = prepared {
        send.prepare(prepared);
        *ctx.page = Page::Send;
    }
}

/// The contracts kept here.
fn saved(ui: &mut Ui, ctx: &mut Ctx, state: &mut ContractsState) {
    let palette = *ctx.palette;
    let mut remove = None;

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Kept");

        if ctx.settings.contracts.is_empty() {
            ui.label(RichText::new("None yet: add one below, or deploy one.").color(palette.muted));
        }

        for (index, contract) in ctx.settings.contracts.iter().enumerate() {
            ui.horizontal_wrapped(|ui| {
                let kind = match contract.address.starts_with("evm:") {
                    true => "EVM",
                    false => "native",
                };
                let open = state.open.as_deref() == Some(contract.address.as_str());

                if ui.selectable_label(open, RichText::new(&contract.name).color(palette.ink)).clicked() {
                    state.open = (!open).then(|| contract.address.clone());
                }

                theme::pill(ui, kind, palette.soft);
                super::address(ui, &palette, &contract.address);

                if theme::quiet(ui, &palette, "Forget").clicked() {
                    remove = Some(index);
                }
            });
        }
    });

    if let Some(index) = remove {
        let gone = ctx.settings.contracts.remove(index);

        if state.open.as_deref() == Some(gone.address.as_str()) {
            state.open = None;
        }
    }
}

/// Keeping a contract by its address, with an EVM one's ABI.
fn add(ui: &mut Ui, ctx: &mut Ctx, state: &mut ContractsState) {
    let palette = *ctx.palette;

    egui::CollapsingHeader::new(RichText::new("Add a contract").color(palette.soft)).id_salt("contracts-add").show(ui, |ui| {
        ui.add(egui::TextEdit::singleline(&mut state.add_address).hint_text("contract:0x… or evm:0x… (or an EVM 0x…)").desired_width(f32::INFINITY));
        ui.add(egui::TextEdit::singleline(&mut state.add_name).hint_text("a name for it here").desired_width(f32::INFINITY));
        let evm = parse_contract(&state.add_address).is_some_and(|address| matches!(address, Address::Evm(_)));

        if evm {
            ui.add(egui::TextEdit::multiline(&mut state.add_abi).hint_text(chainv4_ui::widgets::flagged(ui, "its ABI's JSON, as solc --abi or Foundry writes it", egui::TextStyle::Body, palette.muted)).desired_rows(4).desired_width(f32::INFINITY));
        }

        if theme::primary(ui, &palette, "Keep it", true).clicked() {
            match keep(ctx, &state.add_address, &state.add_name, if evm { &state.add_abi } else { "" }) {
                Ok(address) => {
                    state.open = Some(address);
                    state.add_address.clear();
                    state.add_name.clear();
                    state.add_abi.clear();
                    state.trouble = None;
                }
                Err(why) => state.trouble = Some(why),
            }
        }

        if let Some(why) = &state.trouble {
            ui.colored_label(palette.bad, why.as_str());
        }
    });
}

/// A contract's address as typed: with its kind, or an EVM one as 0x and 40 hex digits.
fn parse_contract(text: &str) -> Option<Address> {
    let text = text.trim();

    match text.len() == 42 && text.starts_with("0x") {
        true => format!("evm:{text}").parse().ok(),
        false => text.parse().ok().filter(|address| matches!(address, Address::Contract(_) | Address::Evm(_))),
    }
}

/// Keeps a contract, or replaces the one kept at its address.
fn keep(ctx: &mut Ctx, address: &str, name: &str, abi: &str) -> Result<String, String> {
    let address = parse_contract(address).ok_or("not a contract's address: contract:0x…, evm:0x…, or an EVM 0x…")?.to_string();

    if !abi.trim().is_empty() {
        Abi::parse(abi)?;
    }

    let name = match name.trim() {
        "" => format::short(&address),
        name => name.to_string(),
    };
    ctx.settings.contracts.retain(|kept| kept.address != address);
    ctx.settings.contracts.push(SavedContract { address: address.clone(), name, abi: abi.trim().to_string() });

    Ok(address)
}

/// The contract open: each of its functions, to read or call.
fn opened(ui: &mut Ui, ctx: &mut Ctx, state: &mut ContractsState, from: &Address, contract: &SavedContract) -> Option<Prepared> {
    let palette = *ctx.palette;
    let mut prepared = None;
    let Ok(address) = contract.address.parse::<Address>() else {
        return None;
    };

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, &contract.name);
        super::address(ui, &palette, &contract.address);
        ui.add_space(6.0);

        prepared = match address {
            Address::Evm(_) => evm_functions(ui, ctx, state, from, contract, address),
            _ => native_functions(ui, ctx, state, from, contract, address),
        };
    });

    prepared
}

fn native_functions(ui: &mut Ui, ctx: &mut Ctx, state: &mut ContractsState, from: &Address, contract: &SavedContract, address: Address) -> Option<Prepared> {
    let palette = *ctx.palette;

    if !state.records.contains_key(&contract.address) {
        ctx.node.ask(Ask::Contract(contract.address.clone()));
        ui.label(RichText::new("Asking the node for its functions…").color(palette.muted));

        return None;
    }

    let Some(record) = state.records.get(&contract.address).cloned().flatten() else {
        ui.colored_label(palette.bad, "No contract lives there, as the node says.");

        return None;
    };
    let mut prepared = None;

    ui.label(RichText::new(format!("A {} contract. Arguments are type:value, one a line, such as i64:5, string:hi or address:ed25519:0x….", record.engine)).small().color(palette.muted));

    for function in &record.functions {
        let key = (contract.address.clone(), function.clone());
        ui.add_space(6.0);
        ui.separator();
        ui.label(RichText::new(function).monospace().color(palette.ink));
        let fields = state.fields.entry(key.clone()).or_insert_with(|| vec![String::new()]);
        ui.add(egui::TextEdit::multiline(&mut fields[0]).hint_text("arguments, one a line").desired_rows(1).desired_width(f32::INFINITY));
        let value = state.values.entry(key.clone()).or_default();
        ui.add(egui::TextEdit::singleline(value).hint_text(format!("{} to send with it, optional", ctx.coin.symbol)).desired_width(180.0));
        let args: Result<Vec<TxCallArgumentType>, String> = fields[0].lines().map(str::trim).filter(|line| !line.is_empty()).map(TxCallArgumentType::parse_typed).collect();
        let sent = coin(ctx, value);

        ui.horizontal_wrapped(|ui| {
            if theme::quiet(ui, &palette, "Read").clicked() {
                let args_json: Vec<Value> = fields[0]
                    .lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .map(|line| match line.split_once(':') {
                        Some((kind, value)) => json!({ "type": kind, "value": value }),
                        None => json!({ "type": line }),
                    })
                    .collect();
                let ask = Ask::Call(json!({ "contract": contract.address, "function": function, "args": args_json, "sender": from.to_string() }).to_string());
                state.asked.insert(ask.clone(), key.clone());
                state.results.remove(&key);
                ctx.node.ask(ask);
            }

            // The same call for an EVM tool, MetaMask or cast: sent to the native precompile.
            if theme::quiet(ui, &palette, "EVM calldata").clicked() {
                let encoded = args.clone().and_then(|args| chainv4::abi::call_native_input(&address, function, &args));

                match encoded {
                    Ok((precompile, input)) => {
                        state.calldata.insert(key.clone(), (to_hex(&precompile), to_hex(&input)));
                        state.results.remove(&key);
                    }
                    Err(why) => {
                        state.calldata.remove(&key);
                        state.results.insert(key.clone(), Err(why));
                    }
                }
            }

            if theme::quiet(ui, &palette, "Send").clicked() {
                match (&args, &sent) {
                    (Ok(args), Ok(sent)) => {
                        prepared = Some(Prepared {
                            from: *from,
                            tx: Tx::ContractCall { to: address, func: Hasher::digest(HashType::Xxh3_64, function.as_bytes()), amount: amount(*sent), data: args.clone() },
                            what: format!("Call {function} on {}", contract.name),
                            about: contract.address.clone(),
                            coin: *sent,
                        });
                    }
                    (Err(why), _) | (_, Err(why)) => {
                        state.results.insert(key.clone(), Err(why.clone()));
                    }
                }
            }
        });

        result(ui, &palette, state.results.get(&key));

        if let Some((to, input)) = state.calldata.get(&key) {
            calldata(ui, &palette, to, input);
        }
    }

    prepared
}

/// A native call as an EVM tool sends it: to the native precompile, with this calldata, from any
/// account, which the contract then sees as its sender; the coin to send goes as the value.
fn calldata(ui: &mut Ui, palette: &theme::Palette, to: &str, input: &str) {
    ui.label(RichText::new("For EVM tools, MetaMask or cast: send this to the native precompile, with what to send as the value. The contract sees the sending account as its sender.").small().color(palette.muted));
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("To").small().color(palette.soft));
        chainv4_ui::widgets::copy_on_click(ui, palette, egui::Label::new(theme::mono(to).color(palette.ink)), to);
    });
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("Data").small().color(palette.soft));
        let shown = match input.len() > 74 { true => format!("{}…{}", &input[..42], &input[input.len() - 16..]), false => input.to_string() };
        chainv4_ui::widgets::copy_on_click(ui, palette, egui::Label::new(theme::mono(shown.as_str()).color(palette.ink)), input).on_hover_text("Click to copy all of it");
    });
}

fn evm_functions(ui: &mut Ui, ctx: &mut Ctx, state: &mut ContractsState, from: &Address, contract: &SavedContract, address: Address) -> Option<Prepared> {
    let palette = *ctx.palette;
    let abi = match Abi::parse(&contract.abi) {
        Ok(abi) => abi,
        Err(_) => {
            ui.label(RichText::new("No ABI kept for it: add it again with its ABI to read and call its functions.").color(palette.muted));

            return None;
        }
    };
    let mut prepared = None;

    // Reads first, as a block explorer lays them out.
    let mut functions: Vec<&AbiFunction> = abi.functions.iter().collect();
    functions.sort_by_key(|function| !function.reads);

    for function in functions {
        let signature = function.signature();
        let key = (contract.address.clone(), signature.clone());
        ui.add_space(6.0);
        ui.separator();
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(&signature).monospace().color(palette.ink));
            theme::pill(ui, if function.reads { "read" } else if function.payable { "payable" } else { "write" }, palette.soft);
        });
        let fields = state.fields.entry(key.clone()).or_insert_with(|| vec![String::new(); function.inputs.len()]);
        fields.resize(function.inputs.len(), String::new());

        for (param, field) in function.inputs.iter().zip(fields.iter_mut()) {
            ui.add(egui::TextEdit::singleline(field).hint_text(format!("{} {}", param.ty, param.name)).desired_width(f32::INFINITY));
        }

        let value = state.values.entry(key.clone()).or_default();

        if function.payable {
            ui.add(egui::TextEdit::singleline(value).hint_text(format!("{} to send with it", ctx.coin.symbol)).desired_width(180.0));
        }

        let input = function
            .inputs
            .iter()
            .zip(fields.iter())
            .map(|(param, field)| parse_value(&param.ty, field))
            .collect::<Result<Vec<AbiValue>, String>>()
            .and_then(|values| function.encode_call(&values));
        let sent = coin(ctx, value);

        ui.horizontal_wrapped(|ui| {
            let read = theme::quiet(ui, &palette, if function.reads { "Read" } else { "Try it" }).clicked();
            let write = !function.reads && theme::quiet(ui, &palette, "Send").clicked();

            match (&input, &sent) {
                (Ok(input), Ok(sent)) if read => {
                    let tx = Tx::EvmCall { to: address, value: amount(*sent), input: input.clone() };
                    let Some(body) = probe(ctx, from, tx) else {
                        state.results.insert(key.clone(), Err("This cannot be tried yet: the light client has not said which chain this node is on.".to_string()));

                        return;
                    };
                    let ask = Ask::Read(body);
                    state.asked.insert(ask.clone(), key.clone());
                    state.results.remove(&key);
                    ctx.node.ask(ask);
                }
                (Ok(input), Ok(sent)) if write => {
                    prepared = Some(Prepared {
                        from: *from,
                        tx: Tx::EvmCall { to: address, value: amount(*sent), input: input.clone() },
                        what: format!("Call {} on {}", function.name, contract.name),
                        about: contract.address.clone(),
                        coin: *sent,
                    });
                }
                (Err(why), _) | (_, Err(why)) if read || write => {
                    state.results.insert(key.clone(), Err(why.clone()));
                }
                _ => {}
            }
        });

        result(ui, &palette, state.results.get(&key));
    }

    prepared
}

/// The body of POST /offer for a probe of a transaction from an address: signed by its key, or,
/// for a watch-only one, signed by nothing, which a dry run never checks. None until the chain to
/// sign for is known.
fn probe(ctx: &Ctx, from: &Address, tx: Tx) -> Option<String> {
    let holder = ctx.holder()?;
    let genesis = ctx.chain.genesis?;
    let probe = match holder.key(from) {
        Some(key) => SignedTx::sign(tx, 0, FeeOffer::NONE, key, &genesis),
        None => airgap::probe(tx, 0, holder.public(from)?),
    };

    Some(json!({ "tx": to_hex(&wincode::serialize(&probe).expect("a signed transaction always encodes")) }).to_string())
}

fn result(ui: &mut Ui, palette: &theme::Palette, shown: Option<&Result<String, String>>) {
    match shown {
        Some(Ok(text)) => {
            ui.label(RichText::new(format!("→ {text}")).monospace().color(palette.good));
        }
        Some(Err(why)) => {
            ui.colored_label(palette.bad, why.as_str());
        }
        None => {}
    }
}

/// An amount of the coin, as typed in whole units: nothing for none.
fn coin(ctx: &Ctx, text: &str) -> Result<u256, String> {
    match text.trim() {
        "" => Ok(u256::ZERO),
        text => format::parse_units(text, ctx.coin.decimals)
            .and_then(|units| u256::from_str_radix(&units, 10).ok())
            .ok_or_else(|| format!("{text} is not an amount of {}", ctx.coin.symbol)),
    }
}

fn amount(units: u256) -> Amount {
    match u128::try_from(units) {
        Ok(narrow) => Amount::U128(narrow),
        Err(_) => Amount::U256(units),
    }
}

/// Deploying a contract: Lua source, a WebAssembly module, or EVM init code with its
/// constructor's arguments. It is kept here under the address it will land at.
fn deploy(ui: &mut Ui, ctx: &mut Ctx, state: &mut ContractsState, from: &Address) -> Option<Prepared> {
    let palette = *ctx.palette;
    let mut prepared = None;

    if let Some(picked) = state.picked.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take() {
        match picked {
            Ok((name, bytes)) => match state.engine {
                Engine::Wasm => state.wasm = Some((name, bytes)),
                Engine::Lua => state.code = String::from_utf8_lossy(&bytes).into_owned(),
                Engine::Evm => state.code = String::from_utf8_lossy(&bytes).trim().to_string(),
            },
            Err(why) => state.trouble = Some(why),
        }
    }

    egui::CollapsingHeader::new(RichText::new("Deploy a contract").color(palette.soft)).id_salt("contracts-deploy").show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut state.engine, Engine::Lua, "Lua");
            ui.selectable_value(&mut state.engine, Engine::Wasm, "WebAssembly");
            ui.selectable_value(&mut state.engine, Engine::Evm, "EVM");
        });

        let (hint, kind): (&str, (&'static str, &'static [&'static str])) = match state.engine {
            Engine::Lua => ("the contract's Lua source, one file", ("Lua", &["lua", "luau"])),
            Engine::Wasm => ("", ("WebAssembly", &["wasm"])),
            Engine::Evm => ("its init code in hex, as solc --bin writes it", ("EVM bytecode", &["bin", "hex"])),
        };

        match state.engine {
            Engine::Wasm => {
                let text = state.wasm.as_ref().map_or("No module picked.".to_string(), |(name, bytes)| format!("{name}, {} bytes", bytes.len()));
                ui.label(RichText::new(text).color(palette.soft));
            }
            _ => {
                ui.add(egui::TextEdit::multiline(&mut state.code).hint_text(chainv4_ui::widgets::flagged(ui, hint, egui::TextStyle::Body, palette.muted)).font(egui::TextStyle::Monospace).desired_rows(4).desired_width(f32::INFINITY));
            }
        }

        if files::can_pick(ui.ctx()) && theme::quiet(ui, &palette, "Pick a file…").clicked() {
            let picked = state.picked.clone();
            files::pick(ui.ctx(), Pick { kind: Some(kind), ..Pick::default() }, move |files| {
                let read = match files {
                    Ok(mut files) if !files.is_empty() => {
                        let file = files.swap_remove(0);

                        Ok((file.name, file.bytes))
                    }
                    Ok(_) => return,
                    Err(why) => Err(why),
                };
                *picked.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(read);
            });
        }

        let mut abi = None;

        if state.engine == Engine::Evm {
            ui.add(egui::TextEdit::multiline(&mut state.deploy_abi).hint_text("its ABI's JSON, for its constructor and to keep it with").desired_rows(3).desired_width(f32::INFINITY));
            abi = Abi::parse(&state.deploy_abi).ok();

            if let Some(abi) = &abi {
                state.constructor.resize(abi.constructor.len(), String::new());

                for (param, field) in abi.constructor.iter().zip(state.constructor.iter_mut()) {
                    ui.add(egui::TextEdit::singleline(field).hint_text(format!("constructor: {} {}", param.ty, param.name)).desired_width(f32::INFINITY));
                }
            }
        }

        ui.add(egui::TextEdit::singleline(&mut state.deploy_name).hint_text("a name to keep it under here").desired_width(f32::INFINITY));

        if theme::primary(ui, &palette, "Deploy", true).clicked() {
            match deploy_tx(ctx, state, from, abi.as_ref()) {
                Ok(deploy) => prepared = Some(deploy),
                Err(why) => state.trouble = Some(why),
            }
        }

        if let Some(why) = &state.trouble {
            ui.colored_label(palette.bad, why.as_str());
        }
    });

    prepared
}

/// The deploy the form asks for, and the contract kept where it will land: at the address its
/// sender's next nonce gives it.
fn deploy_tx(ctx: &mut Ctx, state: &ContractsState, from: &Address, abi: Option<&Abi>) -> Result<Prepared, String> {
    let code = match state.engine {
        Engine::Lua if state.code.trim().is_empty() => return Err("give the contract's source".to_string()),
        Engine::Lua => Code::Lua(state.code.clone()),
        Engine::Wasm => Code::Wasm(state.wasm.as_ref().ok_or("pick the module")?.1.clone()),
        Engine::Evm => {
            let mut init = from_hex(state.code.trim()).ok_or("the init code is not hex")?;

            if let Some(abi) = abi {
                let values = abi.constructor.iter().zip(&state.constructor).map(|(param, field)| parse_value(&param.ty, field)).collect::<Result<Vec<_>, _>>()?;
                init.extend(chainv4::abi::encode_values(&abi.constructor.iter().map(|param| param.ty.clone()).collect::<Vec<_>>(), &values)?);
            }

            Code::Evm(init)
        }
    };
    let proven = ctx.chain.account(&from.to_string()).ok_or("the address is not proven yet: wait for the light client")?;
    let nonce = next_nonce(proven.value.nonce, &ctx.settings.sent, &from.to_string(), ctx.now);
    let hash = ctx.chain.client.as_ref().map(|client| client.params().hash).ok_or("the light client has not started")?;
    let lands = match &code {
        Code::Evm(_) => Address::Evm(evm_create_address(evm_address(from), nonce)),
        _ => contract_address(from, nonce, hash),
    };
    let name = match state.deploy_name.trim() {
        "" => format!("deployed at nonce {nonce}"),
        name => name.to_string(),
    };
    let what = format!("Deploy {name}");
    keep(ctx, &lands.to_string(), &name, if matches!(code, Code::Evm(_)) { state.deploy_abi.trim() } else { "" })?;

    Ok(Prepared {
        from: *from,
        tx: Tx::Deploy { code },
        what,
        about: format!("lands at {lands}"),
        coin: u256::ZERO,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_read_says_whether_it_ran_here_over_proven_state_or_is_the_nodes_word() {
        assert!(checked_words(&json!({ "checked": { "here": true, "height": 12, "differed": false } })).contains("proven at block 12"));
        assert!(checked_words(&json!({ "checked": { "here": true, "height": 12, "differed": true } })).contains("not to be trusted"));
        assert!(checked_words(&json!({ "checked": { "here": false, "why": "an older node" } })).contains("an older node"));
        assert!(checked_words(&json!({})).contains("browser"));
    }

    const COUNTER_ABI: &str = include_str!("../../../contracts/evm/Counter.abi");

    fn kept(address: &str) -> Vec<SavedContract> {
        vec![SavedContract { address: address.to_string(), name: "Counter".to_string(), abi: COUNTER_ABI.to_string() }]
    }

    #[test]
    fn a_native_function_s_call_reads_as_the_calldata_an_evm_tool_sends() {
        use chainv4::abi::{AbiType, AbiValue, decode_values};

        let counter = Address::Contract([7; 32]);
        // As typed on the page: one argument a line.
        let args: Vec<TxCallArgumentType> = ["u64:5", "string:hi", "address:ed25519:0x0101010101010101010101010101010101010101010101010101010101010101"]
            .into_iter()
            .map(TxCallArgumentType::parse_typed)
            .collect::<Result<_, _>>()
            .unwrap();
        let (to, input) = chainv4::abi::call_native_input(&counter, "add", &args).unwrap();

        assert_eq!(to_hex(&to), "0x0000000000000000000000000000000000004e43", "the native precompile");
        let call = [AbiType::FixedBytes(32), AbiType::String, AbiType::String, AbiType::Bytes];
        let fields = decode_values(&call, &input[4..]).unwrap();
        assert_eq!(fields[0], AbiValue::FixedBytes(vec![7; 32]));
        assert_eq!(fields[1], AbiValue::String("add".to_string()));
        assert_eq!(fields[2], AbiValue::String("u64,string,address".to_string()), "each argument's native type, named");
        let AbiValue::Bytes(packed) = &fields[3] else { panic!("{:?}", fields[3]) };
        let values = decode_values(&[AbiType::Uint(64), AbiType::String, AbiType::Address], packed).unwrap();
        assert_eq!(values[1], AbiValue::String("hi".to_string()));
        assert_eq!(values[2], AbiValue::Address(chainv4::abi::evm_address(&Address::Ed25519([1; 32]))), "an account as its alias inside the EVM");

        // An EVM contract has no native functions to call this way.
        assert!(chainv4::abi::call_native_input(&Address::Evm([1; 20]), "add", &[]).is_err());
    }

    #[test]
    fn contract_addresses_read_with_their_kind_or_as_ethereum_writes_them() {
        let evm = format!("0x{}", "ab".repeat(20));

        assert_eq!(parse_contract(&evm), Some(Address::Evm([0xab; 20])));
        assert_eq!(parse_contract(&format!("contract:0x{}", "01".repeat(32))), Some(Address::Contract([1; 32])));
        assert_eq!(parse_contract(&format!("ed25519:0x{}", "01".repeat(32))), None, "an account is no contract");
        assert_eq!(parse_contract("0x12"), None);
    }

    #[test]
    fn reads_come_back_named_by_the_abi_or_by_their_types() {
        let address = format!("evm:0x{}", "ab".repeat(20));
        let key = (address.clone(), "count()".to_string());
        let returned = json!({ "status": "ok", "returned": [{ "type": "bytes", "value": to_hex(&u256::from(8u64).to_be_bytes()) }] });

        assert_eq!(evm_result(&key, &returned, &kept(&address)), Ok("8".to_string()));

        // A revert with Solidity's Error(string) reads as its reason; one the ABI does not know, as it came.
        let reverted = json!({ "status": "failed", "error": "reverted: nothing to add" });

        assert_eq!(evm_result(&key, &reverted, &kept(&address)), Err("reverted: nothing to add".to_string()));

        let values = json!([{ "type": "i64", "value": "5" }, { "type": "string", "value": "hi" }]);

        assert_eq!(native_values(&values), "i64 5, string hi");
        assert_eq!(native_values(&json!([])), "nothing");
    }
}
