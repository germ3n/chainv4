//! An address: its balance and nonce, and for a contract, its code, its functions and a way to
//! call them read-only. A token shows its name, symbol and decimals as the chain's registry holds
//! them, and its decimals can be set by hand; every account shows what it holds of the registered
//! tokens: the registry's first page, by symbol, and any other the explorer has met. With the
//! node's address index, it lists every transaction that touched the address.

use {
    super::{Ctx, TxLine, View, events, tokens},
    crate::api::{Ask, EventQuery},
    crate::format,
    crate::model::{Called, FoundTx, RegisteredToken},
    chainv4::structs::address::Address,
    chainv4_ui::filters::{self, COIN, Facts, Offered, TxFilter},
    chainv4_ui::widgets,
    egui::{RichText, Ui},
    serde_json::json,
};

/// The argument types a read-only call can be given, as the API names them.
const KINDS: [&str; 13] = ["address", "amount", "u256", "u128", "u64", "i64", "i128", "i256", "number", "bool", "string", "bytes", "nil"];

#[derive(Default)]
pub struct AccountState {
    /// The address these are for: another one starts afresh.
    address: String,
    /// The event name being typed, and the one the list shows.
    typing: String,
    named: String,
    /// What an EVM contract's logs are filtered by, as they all have one name.
    topic: events::TopicFilter,
    function: String,
    args: Vec<(String, String)>,
    /// The call last made.
    asked: Option<Ask>,
    /// An EVM contract's ABI as it is being pasted.
    abi_typing: String,
    /// An EVM read function's fields, and the read last asked of it, by its signature.
    evm_fields: std::collections::HashMap<String, Vec<String>>,
    evm_asked: std::collections::HashMap<String, Ask>,
    /// What its transactions are narrowed to.
    filter: TxFilter,
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
    let reading = ctx.settings.read_blocks;

    if reading {
        reading_page(ui, ctx, address, state);

        return;
    }

    if !store.accounts.contains_key(address) {
        ctx.api.ask(Ask::Account(address.to_string()));
    }

    if (address.starts_with("contract:") || address.starts_with("evm:")) && !store.contracts.contains_key(address) {
        ctx.api.ask(Ask::Contract(address.to_string()));
    }

    let contract = store.contracts.get(address).cloned().flatten();
    let kind = address.split(':').next().unwrap_or_default();
    let title = match &contract {
        Some(contract) if contract.token.is_some() => "Token contract",
        Some(contract) if contract.collection.is_some() => "Collection contract",
        Some(contract) if contract.engine == "evm" => "EVM contract",
        Some(_) => "Contract",
        None => "Account",
    };

    super::heading(ui, look, title, "", |ui| {
        widgets::pill(ui, kind, look.palette.soft);
    });
    super::copyable(ui, address);

    // An address read from EVM calldata or a log goes by its 20 bytes, which name a contract at
    // evm:0x… and a secp256k1 account at the same bytes; another account's alias says nothing back.
    if let Some(bytes) = address.strip_prefix("evm:")
        && store.failures.get(&Ask::Contract(address.to_string())).is_some_and(|failure| failure.status == 404)
    {
        let account = format!("secp256k1:{bytes}");

        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("No contract here. These 20 bytes may be the account").small().color(look.palette.muted));
            super::address(ui, ctx, &account);
        });
    }

    // The name it goes by, as the node says: it resolves to it now.
    if let Some(name) = store.accounts.get(address).and_then(|account| account.name.clone()) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("goes by").small().color(look.palette.muted));

            if ui.link(RichText::new(&name).strong()).clicked() {
                ctx.go(View::Name(name.clone()));
            }
        });
    }

    // An Ethereum account EIP-7702 delegated: what runs when it is called.
    if let Some(to) = store.accounts.get(address).and_then(|account| account.delegated_to.clone()) {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("runs the code of").small().color(look.palette.muted));
            super::address(ui, ctx, &to);
            ui.label(RichText::new("as its own, by EIP-7702, until it is cleared").small().color(look.palette.muted));
        });
    }

        // A multisig's members, threshold and proposals have a page of their own.
    if address.starts_with("multisig:") && ui.link(RichText::new("Its members, threshold and open proposals ›").color(look.palette.accent)).clicked() {
        ctx.go(View::Multisig(address.to_string()));
    }

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

    if let Some(owned) = store.owned_names.get(address).filter(|owned| !owned.is_empty()) {
        super::section(ui, look, "Names", |ui| {
            for owned in owned {
                ui.horizontal(|ui| {
                    if ui.link(RichText::new(&owned.display).monospace()).clicked() {
                        ctx.go(View::Name(owned.display.clone()));
                    }

                    ui.label(RichText::new(format!("{} · expires {} UTC", owned.status, format::utc(owned.expires))).small().color(look.palette.muted));
                });
            }
        });
    }

    // The multisigs its key is a member of, as the node's member index says.
    if let Some(memberships) = store.memberships.get(address).filter(|memberships| !memberships.is_empty()) {
        super::section(ui, look, "Member of", |ui| {
            for membership in memberships {
                ui.horizontal_wrapped(|ui| {
                    if ui.link(RichText::new(format::short(&membership.address)).monospace()).on_hover_text(&membership.address).clicked() {
                        ctx.go(View::Multisig(membership.address.clone()));
                    }

                    ui.label(RichText::new(format!("{} of {} must agree", membership.threshold, membership.members)).small().color(look.palette.muted));
                });
            }
        });
    }

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
                                        ("map", Some(kind)) => {
                                            let values = crate::model::kind_text(kind, declared.unit.as_deref());
                                            let shown = match &declared.key_kind {
                                                Some(key) => format!("map {} → {values}", crate::model::kind_text(key, declared.key_unit.as_deref())),
                                                None => format!("map of {values}"),
                                            };

                                            ui.label(RichText::new(shown).color(look.palette.muted))
                                        }
                                        ("record", _) => ui.label(RichText::new("record").color(look.palette.muted)),
                                        _ => ui.label(retired()),
                                    };
                                });

                                for field in &declared.keys {
                                    ui.horizontal(|ui| {
                                        ui.add_space(16.0);
                                        ui.label(RichText::new(&field.key).monospace());

                                        match &field.kind {
                                            Some(kind) => ui.label(RichText::new(crate::model::kind_text(kind, field.unit.as_deref())).color(look.palette.muted)),
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

        if let Some(collection) = &contract.collection {
            super::section(ui, look, "Collection", |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(match collection.multi() {
                        true => "A multi-token collection:",
                        false => "An NFT collection:",
                    }).color(look.palette.soft));
                    super::collections::open_link(ui, ctx, collection);
                });
            });
        }

        match contract.engine.as_str() {
            "evm" => evm(ui, ctx, address, state),
            _ => super::section(ui, look, "Call a function read-only", |ui| read(ui, ctx, address, &contract.functions, state)),
        }
        let named = state.named.clone();
        let evm = contract.engine == "evm";
        let reader = ctx.reader;
        // What the field held as this frame began: it changes once it is left.
        let topic = state.topic.clone();
        super::section_with(
            ui,
            look,
            "Its events",
            |ui| match evm {
                true => state.topic.field(ui, reader, if look.narrow { 140.0 } else { 180.0 }),
                false => {
                    let field = ui.add(egui::TextEdit::singleline(&mut state.typing).hint_text("any name").desired_width(120.0));

                    if field.lost_focus() {
                        state.named = state.typing.trim().to_string();
                    }
                }
            },
            |ui| {
                topic.said(ui, ctx);

                let query = match &topic.topic {
                    Some(topic) if evm => EventQuery::topic(topic, Some(address)),
                    _ => EventQuery::contract(address, Some(&named)),
                };

                events::list(ui, ctx, &query);
            },
        );
    }

    holdings(ui, ctx, address);
    nfts(ui, ctx, address);
    super::section(ui, look, "Transactions", |ui| transactions(ui, ctx, address, &mut state.filter));
    super::section(ui, look, "Events that mention it", |ui| events::list(ui, ctx, &EventQuery::mentioning(address)));
}

/// With Read from checked blocks on: nothing is asked of a node by opening an address. What touched it
/// comes from what was read, its balance and nonce from a direct proof someone pressed for, with the
/// block it was proven at, and what only a node can say is off and says so.
fn reading_page(ui: &mut Ui, ctx: &mut Ctx, address: &str, state: &mut AccountState) {
    let look = ctx.look;
    let kind = address.split(':').next().unwrap_or_default();

    super::heading(ui, look, "Account", "", |ui| {
        widgets::pill(ui, kind, look.palette.soft);
    });
    super::copyable(ui, address);
    ui.add_space(10.0);

    super::section(ui, look, "Balance", |ui| {
        let Ok(parsed) = address.parse::<Address>() else {
            ui.colored_label(look.palette.bad, format!("{address} is not an address"));

            return;
        };
        let key = chainv4::db::address_key(&parsed);
        let Some(proven) = super::reading::direct_proof(ui, ctx, "this account", chainv4::db::Column::Accounts, &key) else {
            return;
        };
        let account: Option<chainv4::structs::account::Account> = match &proven.value {
            Some(bytes) => wincode::deserialize(bytes).ok(),
            None => Some(chainv4::structs::account::Account::default()),
        };

        match account {
            Some(account) => {
                widgets::fitted(ui, &ctx.coins(&account.balance.to_string()), 24.0, 14.0, egui::FontFamily::Monospace, look.palette.ink).on_hover_text(format!("{} units", account.balance));
                ui.label(RichText::new(format!("nonce {}: transactions sent so far, and the nonce the next one takes", account.nonce)).small().color(look.palette.muted));
                super::reading::proven_at(ui, ctx, &proven);
            }
            None => {
                ui.colored_label(look.palette.bad, "An account that does not read");
            }
        }
    });

    super::section(ui, look, "Transactions", |ui| transactions(ui, ctx, address, &mut state.filter));
    super::section(ui, look, "Events that mention it", |ui| events::list(ui, ctx, &EventQuery::mentioning(address)));

    if address.starts_with("contract:") || address.starts_with("evm:") {
        super::section(ui, look, "Its events", |ui| events::list(ui, ctx, &EventQuery::contract(address, None)));
    }

    super::reading::turned_off_card(ui, ctx, "A contract's code and functions, what an account holds of tokens and NFTs, and its names come from the node");
}

/// An EVM contract's ABI, kept here once pasted, and its read functions, run with eth_call: what
/// they return, read by the ABI.
fn evm(ui: &mut Ui, ctx: &mut Ctx, address: &str, state: &mut AccountState) {
    let look = ctx.look;
    let abi = ctx.abi(address);

    super::section(ui, look, "ABI", |ui| match &abi {
        Some(abi) => {
            ui.horizontal_wrapped(|ui| {
                ui.label(format!("{} functions and {} events, kept on this device; its logs and calls read by them.", abi.functions.len(), abi.events.len()));

                if widgets::quiet(ui, &look.palette, "Forget it").clicked() {
                    state.abi_typing = ctx.abi_text(address).map(str::to_string).unwrap_or_default();
                    ctx.set_abi(address, None);
                }
            });
        }
        None => {
            super::note(ui, look, "Paste its ABI, as solc --abi or Foundry writes it, to read its functions and logs.");
            ui.add(egui::TextEdit::multiline(&mut state.abi_typing).desired_rows(3).desired_width(f32::INFINITY).font(egui::TextStyle::Monospace));

            match chainv4::abi::Abi::parse(&state.abi_typing) {
                Ok(_) if widgets::primary(ui, &look.palette, "Keep it", true).clicked() => {
                    ctx.set_abi(address, Some(state.abi_typing.trim().to_string()));
                }
                Err(why) if !state.abi_typing.trim().is_empty() => {
                    ui.colored_label(look.palette.bad, why);
                }
                _ => {}
            }
        }
    });

    let Some(abi) = abi else {
        return;
    };
    let reads: Vec<_> = abi.functions.iter().filter(|function| function.reads).collect();

    if reads.is_empty() {
        return;
    }

    super::section(ui, look, "Read its functions", |ui| {
        super::note(ui, look, "Run with eth_call on the node's latest state: nothing is sent.");

        for function in reads {
            let signature = function.signature();
            ui.add_space(4.0);
            ui.label(RichText::new(&signature).monospace());
            let fields = state.evm_fields.entry(signature.clone()).or_insert_with(|| vec![String::new(); function.inputs.len()]);

            for (param, field) in function.inputs.iter().zip(fields.iter_mut()) {
                ui.add(egui::TextEdit::singleline(field).hint_text(format!("{} {}", param.ty, param.name)).desired_width(f32::INFINITY));
            }

            let input = function
                .inputs
                .iter()
                .zip(fields.iter())
                .map(|(param, field)| chainv4::abi::parse_value(&param.ty, field))
                .collect::<Result<Vec<_>, _>>()
                .and_then(|values| function.encode_call(&values));

            if widgets::quiet(ui, &look.palette, "Read").clicked() {
                match &input {
                    Ok(input) => {
                        let to = address.strip_prefix("evm:").unwrap_or(address);
                        let body = json!({ "jsonrpc": "2.0", "id": 1, "method": "eth_call", "params": [{ "to": to, "data": chainv4::structs::hex::to_hex(input) }, "latest"] });
                        let ask = Ask::Rpc(body.to_string());
                        ctx.api.ask(ask.clone());
                        state.evm_asked.insert(signature.clone(), ask);
                    }
                    Err(why) => {
                        ui.colored_label(look.palette.bad, why);
                    }
                }
            }

            let Some(ask) = state.evm_asked.get(&signature) else {
                continue;
            };

            match (ctx.store.failures.get(ask), ctx.store.calls.get(ask)) {
                (Some(failure), _) => {
                    ui.colored_label(look.palette.bad, &failure.message);
                }
                (None, Some(answer)) => match (answer["result"].as_str().and_then(chainv4::structs::hex::from_hex), answer["error"]["message"].as_str()) {
                    (Some(output), _) => {
                        let shown = function.decode_output(&output).map(|values| function.show_outputs(&values)).unwrap_or_else(|_| chainv4::structs::hex::to_hex(&output));
                        ui.horizontal_wrapped(|ui| {
                            widgets::pill(ui, "returned", look.palette.good);
                            ui.monospace(shown);
                        });
                    }
                    (None, Some(message)) => {
                        ui.colored_label(look.palette.bad, message);
                    }
                    (None, None) => {
                        ui.colored_label(look.palette.bad, "the node does not answer JSON-RPC: it needs the ethereum feature");
                    }
                },
                (None, None) if ctx.api.asking(ask) => {
                    ui.spinner();
                }
                _ => {}
            }
        }
    });
}

/// How long the newest page of transactions stands before it is asked for again, in milliseconds;
/// under filters longer, as each ask has the node read through its index.
const REFRESH: u64 = 3_000;
const FILTERED_REFRESH: u64 = 15_000;

/// What can be said of a transaction a node listed, to test it here against the filters, on a node
/// too old to.
fn facts<'a>(found: &'a FoundTx, address: &str, time: Option<u64>) -> Facts<'a> {
    let tx = &found.tx;
    let inner = tx.tx.get("tx").filter(|_| tx.kind() == "multisig").unwrap_or(&tx.tx);
    let named = |field: &str| inner.get(field).and_then(serde_json::Value::as_str);
    let outcome = tx.receipt.as_ref();
    let events = outcome.into_iter().flat_map(|outcome| &outcome.events).map(|event| event.contract.as_str());
    let payments = outcome.is_some_and(|outcome| !outcome.payments.is_empty());
    // A batch passes for what any of its steps does, as a node's own filter takes it.
    let steps = tx.steps().filter(|_| tx.kind() != "approve_multisig").unwrap_or_default();
    let kind_of = |step: &'a serde_json::Value| step.get("type").and_then(serde_json::Value::as_str);
    let moves_coin = matches!(inner.get("type").and_then(serde_json::Value::as_str), Some("transfer" | "mint" | "burn" | "claim")) || payments || steps.iter().any(|step| matches!(kind_of(step), Some("transfer" | "burn" | "claim")));
    let called = named("to").filter(|_| matches!(inner.get("type").and_then(serde_json::Value::as_str), Some("contract_call" | "evm_call")));
    let called_in_steps = steps.iter().filter(|step| matches!(kind_of(step), Some("contract_call" | "evm_call"))).filter_map(|step| step.get("to").and_then(serde_json::Value::as_str));
    let named_in_steps = steps.iter().flat_map(|step| ["to", "from", "contract"].map(|field| step.get(field).and_then(serde_json::Value::as_str))).flatten();

    Facts {
        kinds: [Some(tx.kind()), inner.get("type").and_then(serde_json::Value::as_str)].into_iter().flatten().chain(steps.iter().filter_map(kind_of)).collect(),
        sent: tx.sender == address,
        took_part: tx.sender != address,
        parties: [Some(tx.sender.as_str()), named("to"), named("from"), named("contract"), named("account")].into_iter().flatten().chain(named_in_steps).collect(),
        tokens: moves_coin.then_some(COIN).into_iter().chain(called).chain(called_in_steps).chain(events).collect(),
        time,
    }
}

/// The transactions that touched an address, newest first, from the node's address index: those
/// it sent, those that paid it or named it, a page at a time, narrowed as the filters say. The node
/// does that, so it counts for all the history and not only what was read; an older node ignores
/// filters, which its answer shows, and what it sent is tested here, as far as was read.
fn transactions(ui: &mut Ui, ctx: &mut Ctx, address: &str, filter: &mut TxFilter) {
    let store = ctx.store;
    let look = ctx.look;
    let tokens: Vec<(String, String)> = store.tokens.values().filter_map(|token| Some((token.address.clone(), token.symbol.clone()?))).collect();
    let coin = store.status.as_ref().map_or("Coin", |status| status.coin.symbol.as_str());
    let problem = filter.problem(|text| text.parse::<Address>().is_ok());
    filters::controls(ui, &look.palette, filter, Offered { coin, tokens: &tokens }, problem.as_deref());
    ui.add_space(4.0);

    let on = if problem.is_none() { filter.on() } else { 0 };
    let query = if on > 0 { filter.query(ctx.now) } else { String::new() };
    let refresh = ctx.settings.pace().every_ms(if on > 0 { FILTERED_REFRESH } else { REFRESH });
    let newest = Ask::AccountTxs {
        address: address.to_string(),
        query: query.clone(),
        cursor: None,
    };
    let found = store.account_txs.get(&(address.to_string(), query.clone()));

    if found.is_none_or(|found| ctx.now.saturating_sub(found.refreshed) > refresh) {
        ctx.api.ask(newest.clone());
    }

    let failed = store.failures.get(&newest);
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

    if let Some(failure) = failed
        && widgets::retry(ui, &look.palette, &failure.sentence(), ctx.api.asking(&newest))
    {
        ctx.api.retry(newest.clone());
    }

    let still = found
        .catching_up_to
        .filter(|to| *to < found.indexed_from)
        .map(|to| format!(" It is still indexing older blocks, down to height {}.", format::thousands(&to.to_string())))
        .unwrap_or_default();
    // A node too old to filter sent everything: what passes is found here, of what was read.
    let filtered_here = on > 0 && !found.answered.filtered;
    let shown: Vec<&FoundTx> = found
        .items
        .iter()
        .filter(|found| !filtered_here || filter.passes(&facts(found, address, found.time.or_else(|| store.blocks.get(&found.block).map(|block| block.timestamp))), ctx.now))
        .collect();
    let said = filters::said(shown.len(), found.next.is_some(), &found.answered, on);

    if shown.is_empty() && said.is_none() {
        super::note(
            ui,
            look,
            match ctx.settings.read_blocks {
                true => format!("None in the blocks read, which go back to height {}.{still} Read further back in Settings to look at older ones.", format::thousands(&found.indexed_from.to_string())),
                false => format!("None since height {}, where this node's index starts.{still}", format::thousands(&found.indexed_from.to_string())),
            },
        );

        return;
    }

    let lines: Vec<TxLine> = shown
        .iter()
        .map(|found| TxLine {
            tx: &found.tx,
            index: None,
            block: Some((found.block.as_str(), found.height)),
        })
        .collect();
    super::tx_list(ui, ctx, &format!("account_txs {address} {query}"), &lines);

    ui.add_space(4.0);

    if let Some(said) = &said {
        super::note(ui, look, said.clone());
    }

    ui.horizontal_wrapped(|ui| {
        if let Some(next) = &found.next {
            let older = Ask::AccountTxs {
                address: address.to_string(),
                query: query.clone(),
                cursor: Some(next.clone()),
            };

            if ctx.api.asking(&older) {
                ui.spinner();
            } else if widgets::quiet(ui, &look.palette, if on > 0 { "Look further back" } else { "Older transactions" }).clicked() {
                ctx.api.retry(older);
            }
        }

        ui.label(
            RichText::new(format!("{} shown, indexed from height {}.{still}", shown.len(), format::thousands(&found.indexed_from.to_string())))
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
            let amount = match (raw, ctx.token(&token)) {
                (Some(raw), Some((decimals, symbol))) => format::amount(&raw, decimals, &symbol),
                (Some(raw), None) => format!("{raw} units"),
                (None, _) => "…".to_string(),
            };
            let amount = RichText::new(amount).monospace().color(look.palette.ink);

            // On a phone, a long amount and the token beside it would run into each other: the
            // token goes under it.
            match look.narrow {
                true => super::list_row(ui, look, |ui| {
                    ui.label(amount);
                    ui.horizontal(|ui| super::address(ui, ctx, &token));
                }),
                false => {
                    ui.horizontal(|ui| {
                        ui.label(amount);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| super::address(ui, ctx, &token));
                    });
                    widgets::rule(ui, &look.palette);
                }
            }
        }
    });
}

/// What it holds of the chain's collections: each collection's ids, as tokens_of says, the first
/// page of each. Shown only once something is held.
fn nfts(ui: &mut Ui, ctx: &mut Ctx, address: &str) {
    let look = ctx.look;
    let Some(collections) = super::collections::registry(ctx) else {
        return;
    };
    let held: Vec<_> = collections.iter().filter_map(|collection| Some((collection, super::collections::held(ctx, &collection.address, address)?))).filter(|(_, ids)| !ids.is_empty()).collect();

    if held.is_empty() {
        return;
    }

    super::section(ui, look, "NFTs", |ui| {
        for (collection, ids) in held {
            ui.horizontal_wrapped(|ui| {
                super::collections::open_link(ui, ctx, collection);
                ui.label(RichText::new(match collection.multi() {
                    true => format!("ids {}", ids.join(", ")),
                    false => ids.iter().map(|id| format!("#{id}")).collect::<Vec<_>>().join(" "),
                }).monospace().color(look.palette.ink));
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

#[cfg(test)]
mod tests {
    use {super::*, filters::Direction, serde_json::json};

    /// A transaction as a node lists it.
    fn listed(tx: serde_json::Value, events: serde_json::Value, payments: serde_json::Value) -> FoundTx {
        serde_json::from_value(json!({
            "id": "0x01", "sender": "ed25519:0xaa", "nonce": 0, "signer": "ed25519:0xaa", "signature": "0x", "tx": tx,
            "fee": { "gas_limit": 1, "max_price": "0", "tip": "0" },
            "receipt": { "status": "done", "fuel_used": 0, "gas_used": 0, "fee": "0", "events": events, "payments": payments, "trace": { "hops": [], "dropped": 0 } },
            "block": "0xb1", "height": 7, "index": 0,
        }))
        .unwrap()
    }

    #[test]
    fn what_an_older_node_sent_unfiltered_is_tested_here() {
        let transfer = listed(json!({ "type": "transfer", "from": "ed25519:0xaa", "to": "ed25519:0xbb", "amount": "5" }), json!([]), json!([]));
        let call = listed(
            json!({ "type": "contract_call", "to": "contract:0xcc", "function": "0x01", "amount": "0", "data": [] }),
            json!([{ "contract": "contract:0xdd", "name": "Transfer", "data": [] }]),
            json!([]),
        );
        let passes = |found: &FoundTx, filter: TxFilter, address: &str| filter.passes(&facts(found, address, Some(1_000)), 1_000);

        assert!(passes(&transfer, TxFilter { kind: Some("transfer".to_string()), ..TxFilter::default() }, "ed25519:0xaa"));
        assert!(!passes(&call, TxFilter { kind: Some("transfer".to_string()), ..TxFilter::default() }, "ed25519:0xaa"));
        assert!(passes(&transfer, TxFilter { direction: Some(Direction::Out), ..TxFilter::default() }, "ed25519:0xaa"));
        assert!(passes(&transfer, TxFilter { direction: Some(Direction::In), ..TxFilter::default() }, "ed25519:0xbb"));
        assert!(!passes(&transfer, TxFilter { direction: Some(Direction::In), ..TxFilter::default() }, "ed25519:0xaa"));
        assert!(passes(&transfer, TxFilter { counterparty: "ed25519:0xbb".to_string(), ..TxFilter::default() }, "ed25519:0xaa"));
        assert!(passes(&transfer, TxFilter { token: Some(COIN.to_string()), ..TxFilter::default() }, "ed25519:0xaa"));
        assert!(!passes(&call, TxFilter { token: Some(COIN.to_string()), ..TxFilter::default() }, "ed25519:0xaa"));
        assert!(passes(&call, TxFilter { token: Some("contract:0xcc".to_string()), ..TxFilter::default() }, "ed25519:0xaa"), "the one it called");
        assert!(passes(&call, TxFilter { token: Some("contract:0xdd".to_string()), ..TxFilter::default() }, "ed25519:0xaa"), "and one that emitted");
    }

    #[test]
    fn a_multisig_counts_as_what_it_sent_and_as_itself() {
        let sent = listed(json!({ "type": "multisig", "tx": { "type": "transfer", "from": "multisig:0xaa", "to": "ed25519:0xbb", "amount": "5" } }), json!([]), json!([]));
        let passes = |kind: &str| TxFilter { kind: Some(kind.to_string()), ..TxFilter::default() }.passes(&facts(&sent, "multisig:0xaa", None), 0);

        assert!(passes("multisig") && passes("transfer") && !passes("deploy"));
    }
}
