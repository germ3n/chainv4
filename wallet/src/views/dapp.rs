//! What sites ask, through the browser extension, shown over whatever page is open until it is
//! answered: to connect, choosing which Ethereum accounts a site sees; to send, priced on the node
//! and reviewed as any send is; to sign a message or typed data, laid out to read. Nothing is
//! signed but from a tap here, after biometrics when they are on. Settings has the switch that
//! lets sites ask at all, and the sites connected.

use {
    super::Ctx,
    crate::biometrics::{self, Check},
    crate::dapp::{self, Asked, Request, Wanted},
    crate::format,
    crate::node::Ask,
    crate::theme,
    chainv4::crypto::ethereum::{EthereumKind, EthereumTx, chain_id},
    chainv4::crypto::keypair::PrivateKey,
    chainv4::structs::address::Address,
    chainv4::structs::hex::to_hex,
    chainv4::structs::u256::u256,
    chainv4_ui::biometric::Failure,
    egui::{RichText, Ui},
    serde_json::{Value, json},
    std::collections::HashSet,
};

/// How a send's questions to the node are told apart: the JSON-RPC id is the request's times this,
/// plus which.
const KINDS: u64 = 4;
const NONCE: u64 = 0;
const GAS: u64 = 1;
const PRICE: u64 = 2;
const SEND: u64 = 3;

#[derive(Default)]
pub struct DappState {
    /// What waits, in the order it came.
    shown: Vec<Shown>,
    /// Requests answered here, or refused as unreadable, so a late copy is not shown again.
    answered: HashSet<u64>,
    /// The biometric check before signing, with the request it is for.
    confirming: Option<(u64, Check)>,
}

struct Shown {
    request: Request,
    /// For a connect: the accounts ticked.
    chosen: HashSet<[u8; 20]>,
    /// For a send: what the node said it lacked.
    nonce: Option<u64>,
    gas: Option<u64>,
    price: Option<u256>,
    asked: bool,
    sending: bool,
    trouble: Option<String>,
}

impl DappState {
    /// Takes what waits, as the extension lists it: new ones are read, and ones settled elsewhere,
    /// in another of the wallet's windows, go.
    pub fn take(&mut self, pending: Vec<Value>) {
        let ids: HashSet<u64> = pending.iter().filter_map(|json| json["id"].as_u64()).collect();
        self.shown.retain(|shown| ids.contains(&shown.request.id));

        for json in pending {
            let Some(id) = json["id"].as_u64() else { continue };

            if self.answered.contains(&id) || self.shown.iter().any(|shown| shown.request.id == id) {
                continue;
            }

            match Request::read(&json) {
                Ok(request) => self.shown.push(Shown {
                    request,
                    chosen: HashSet::new(),
                    nonce: None,
                    gas: None,
                    price: None,
                    asked: false,
                    sending: false,
                    trouble: None,
                }),
                Err((id, why)) => {
                    self.answered.insert(id);
                    dapp::answer(id, Err((-32602, why)));
                }
            }
        }
    }

    /// The node's answer to something a send asked at POST /rpc.
    pub fn answered(&mut self, body: &str, result: &Result<Value, crate::node::Failure>) {
        let Some(id) = serde_json::from_str::<Value>(body).ok().and_then(|body| body["id"].as_u64()) else {
            return;
        };
        let (request, kind) = (id / KINDS, id % KINDS);
        let Some(shown) = self.shown.iter_mut().find(|shown| shown.request.id == request) else {
            return;
        };
        let answer = match result {
            Ok(value) => match (&value["result"], value["error"]["message"].as_str()) {
                (_, Some(error)) => Err(error.to_string()),
                (result, None) => Ok(result.clone()),
            },
            Err(failure) => Err(failure.message.clone()),
        };
        let number = |value: &Value| value.as_str().and_then(|hex| u256::from_str_radix(hex.trim_start_matches("0x"), 16).ok());

        match (kind, answer) {
            (NONCE, Ok(value)) => shown.nonce = number(&value).and_then(|nonce| u64::try_from(nonce).ok()),
            (GAS, Ok(value)) => shown.gas = number(&value).and_then(|gas| u64::try_from(gas).ok()),
            (PRICE, Ok(value)) => shown.price = number(&value),
            (SEND, Ok(hash)) => {
                let id = shown.request.id;
                self.answered.insert(id);
                self.shown.retain(|shown| shown.request.id != id);
                dapp::answer(id, Ok(hash));
            }
            (SEND, Err(why)) => {
                shown.sending = false;
                shown.trouble = Some(format!("The node refused it: {why}"));
            }
            (_, Err(why)) => shown.trouble = Some(format!("The node could not price it: {why}")),
            (_, Ok(_)) => {}
        }
    }
}

/// The Ethereum accounts this wallet signs for: each its name and 20 bytes.
pub fn accounts(ctx: &Ctx) -> Vec<(String, [u8; 20])> {
    ctx.holders
        .iter()
        .flat_map(|holder| {
            holder.keys.iter().filter_map(move |key| match key.public().address() {
                Address::Secp256k1(bytes) => Some((holder.name.clone(), bytes)),
                _ => None,
            })
        })
        .collect()
}

/// The key to one of them.
fn secret(ctx: &Ctx, account: &[u8; 20]) -> Option<[u8; 32]> {
    ctx.holders.iter().flat_map(|holder| &holder.keys).find_map(|key| match (key.public().address(), key.private()) {
        (Address::Secp256k1(bytes), PrivateKey::Secp256k1(secret)) if bytes == *account => Some(*secret),
        _ => None,
    })
}

/// Draws the first request waiting, over the page; false when none waits.
pub fn show(ui: &mut Ui, ctx: &mut Ctx, state: &mut DappState) -> bool {
    let palette = *ctx.palette;

    if state.shown.is_empty() {
        if dapp::approving() {
            theme::title(ui, &palette, "Sites", "Nothing waits for you to approve. This window closes once it has answered.");

            return true;
        }

        return false;
    }

    let count = state.shown.len();
    let accounts = accounts(ctx);
    let chain = ctx.chain.genesis.map(|genesis| chain_id(&genesis));
    let mut done: Option<(u64, dapp::Answer)> = None;
    let mut ask = Vec::new();
    let confirmed = take_check(ctx, state);
    let shown = &mut state.shown[0];
    let id = shown.request.id;
    theme::title(ui, &palette, "A site asks", &match count {
        1 => shown.request.origin.clone(),
        more => format!("{} · 1 of {more}", shown.request.origin),
    });

    // Any but a connect names its account, which must be one this wallet signs for.
    if let Some(signer) = shown.request.signer()
        && !accounts.iter().any(|(_, account)| *account == signer)
    {
        shown.trouble = Some(format!("This wallet holds no key for {}.", to_hex(&signer)));
    }

    let mut approve = false;
    let mut reject = false;
    // What it asks, apart from what this frame changes of it.
    let asked = shown.request.asked.clone();

    theme::card(ui, &palette, |ui| match &asked {
        Asked::Connect => {
            theme::caps(ui, &palette, "Connect");
            ui.label(RichText::new("It would see these accounts' addresses and balances, and could ask you to send and sign. Each send and each signature waits for you here.").color(palette.soft));
            ui.add_space(8.0);

            if accounts.is_empty() {
                shown.trouble = Some("This wallet has no Ethereum account it holds a key for: add one under Accounts.".to_string());
            }

            if shown.chosen.is_empty()
                && let Some(current) = ctx.holders.get(ctx.current).and_then(|holder| accounts.iter().find(|(name, _)| *name == holder.name))
            {
                shown.chosen.insert(current.1);
            }

            for (name, account) in &accounts {
                let mut ticked = shown.chosen.contains(account);

                if ui.checkbox(&mut ticked, RichText::new(format!("{name} · {}", format::short(&to_hex(account)))).color(palette.ink)).changed() {
                    match ticked {
                        true => shown.chosen.insert(*account),
                        false => shown.chosen.remove(account),
                    };
                }
            }
        }
        Asked::Send(wanted) => send(ui, ctx, &palette, wanted, shown, chain, &mut ask),
        Asked::Message { message, .. } => {
            theme::caps(ui, &palette, "Sign a message");
            ui.label(RichText::new("Signing a message moves nothing, but a site may take it as you saying so: sign only what you mean.").color(palette.soft));
            ui.add_space(8.0);

            match std::str::from_utf8(message).ok().filter(|text| !text.chars().any(|c| c.is_control() && !c.is_whitespace())) {
                Some(text) => super::code(ui, &palette, text),
                None => super::code(ui, &palette, &to_hex(message)),
            }
        }
        Asked::Typed { data, json, .. } => {
            theme::caps(ui, &palette, &format!("Sign {}", data.primary));
            ui.label(RichText::new("Typed data can approve a token to be spent, or an order: read each field.").color(palette.soft));
            ui.add_space(8.0);

            for field in ["name", "version", "chainId", "verifyingContract"] {
                if let Some(value) = json["domain"].get(field).filter(|value| !value.is_null()) {
                    super::row(ui, &palette, field, |ui| {
                        ui.label(theme::mono(value.to_string().trim_matches('"')).color(palette.ink));
                    });
                }
            }

            let named = json["domain"]["chainId"].as_u64().or_else(|| json["domain"]["chainId"].as_str().and_then(|text| u64::from_str_radix(text.trim_start_matches("0x"), if text.starts_with("0x") { 16 } else { 10 }).ok()));

            if let (Some(named), Some(chain)) = (named, chain)
                && named != chain
            {
                ui.colored_label(palette.warn, format!("It names chain {named}, not this one, {chain}."));
            }

            ui.add_space(6.0);
            super::code(ui, &palette, &serde_json::to_string_pretty(&json["message"]).unwrap_or_default());
        }
    });

    theme::card(ui, &palette, |ui| {
        if let Some(trouble) = &shown.trouble {
            ui.colored_label(palette.bad, trouble.as_str());
            ui.add_space(6.0);
        }

        ui.horizontal(|ui| {
            let ready = match &asked {
                Asked::Connect => !shown.chosen.is_empty(),
                Asked::Send(_) => shown.trouble.is_none() && !shown.sending && prepared(shown).is_some(),
                _ => shown.trouble.is_none(),
            };
            let label = match &asked {
                Asked::Connect => "Connect",
                Asked::Send(_) if shown.sending => "Sending…",
                Asked::Send(_) => "Send",
                _ => "Sign",
            };
            approve = theme::primary(ui, &palette, label, ready && state.confirming.is_none()).clicked();
            reject = theme::quiet(ui, &palette, "Reject").clicked();
        });
    });

    // A connect gives addresses alone; anything signed waits for biometrics, when they are on.
    let signs = !matches!(asked, Asked::Connect);

    if approve && signs && !confirmed && state.confirming.is_none() && let Some((enrolled, _)) = biometrics::enrolled(ctx.wake, ctx.settings) {
        state.confirming = Some((id, Check::start(ctx.wake, &enrolled, "Approve what the site asks")));
    } else if approve || confirmed {
        match &asked {
            Asked::Connect => {
                let chosen = accounts.iter().filter(|(_, account)| shown.chosen.contains(account)).map(|(_, account)| json!(to_hex(account))).collect();
                done = Some((id, Ok(Value::Array(chosen))));
            }
            Asked::Send(wanted) => match (prepared(shown), chain, secret(ctx, &wanted.from)) {
                (Some((nonce, gas, price)), Some(chain), Some(secret)) => {
                    let tx = EthereumTx {
                        kind: EthereumKind::DynamicFee,
                        chain_id: chain,
                        nonce,
                        max_price: price,
                        tip: wanted.tip.unwrap_or(u256::ZERO).min(price),
                        gas_limit: gas,
                        to: wanted.to,
                        value: wanted.value,
                        input: wanted.input.clone(),
                        access_list: Vec::new(),
                        signature: [0; 64],
                        parity: 0,
                    }
                    .signed(&secret);
                    shown.sending = true;
                    ask.push(rpc(id, SEND, "eth_sendRawTransaction", json!([to_hex(&tx.encode())])));
                }
                _ => shown.trouble = Some("The wallet lacks the key, or the chain, to sign it.".to_string()),
            },
            Asked::Message { address, message } => match secret(ctx, address) {
                Some(secret) => done = Some((id, Ok(json!(dapp::signature(&secret, &dapp::personal_digest(message)))))),
                None => shown.trouble = Some("The key is gone from this wallet.".to_string()),
            },
            Asked::Typed { address, data, .. } => match (secret(ctx, address), data.digest()) {
                (Some(secret), Ok(digest)) => done = Some((id, Ok(json!(dapp::signature(&secret, &digest))))),
                (None, _) => shown.trouble = Some("The key is gone from this wallet.".to_string()),
                (_, Err(why)) => shown.trouble = Some(why),
            },
        }
    }

    if reject {
        state.confirming = None;
        done = Some((id, dapp::rejected()));
    }

    for asked in ask {
        ctx.node.ask(asked);
    }

    if let Some((id, answer)) = done {
        state.answered.insert(id);
        state.shown.retain(|shown| shown.request.id != id);
        dapp::answer(id, answer);
    }

    true
}

/// What the biometric check came to, for the request it was started for.
fn take_check(ctx: &mut Ctx, state: &mut DappState) -> bool {
    let Some((id, check)) = &state.confirming else {
        return false;
    };
    let Some(came) = check.take() else {
        return false;
    };
    let still = state.shown.first().is_some_and(|shown| shown.request.id == *id);
    state.confirming = None;

    match came {
        Ok(_) => still,
        Err(Failure::Canceled) => false,
        Err(Failure::Invalidated) => {
            biometrics::turn_off(ctx.wake, ctx.settings);
            *ctx.lock = Some(biometrics::CHANGED.to_string());

            false
        }
        Err(Failure::Failed(why)) => {
            if let Some(shown) = state.shown.first_mut() {
                shown.trouble = Some(why);
            }

            false
        }
    }
}

/// A send's nonce, gas and price, once the node said what the site left out.
fn prepared(shown: &Shown) -> Option<(u64, u64, u256)> {
    let Asked::Send(wanted) = &shown.request.asked else {
        return None;
    };

    Some((wanted.nonce.or(shown.nonce)?, wanted.gas.or(shown.gas)?, wanted.max_price.or(shown.price)?))
}

fn rpc(request: u64, kind: u64, method: &str, params: Value) -> Ask {
    Ask::Rpc(json!({ "jsonrpc": "2.0", "id": request * KINDS + kind, "method": method, "params": params }).to_string())
}

fn send(ui: &mut Ui, ctx: &Ctx, palette: &theme::Palette, wanted: &Wanted, shown: &mut Shown, chain: Option<u64>, ask: &mut Vec<Ask>) {
    let id = shown.request.id;

    // What the site left out, from the node, once.
    if !shown.asked {
        shown.asked = true;
        let from = to_hex(&wanted.from);

        if wanted.nonce.is_none() {
            ask.push(rpc(id, NONCE, "eth_getTransactionCount", json!([from, "pending"])));
        }

        if wanted.gas.is_none() {
            let mut call = json!({ "from": from, "value": format!("{:#x}", wanted.value), "data": to_hex(&wanted.input) });

            if let Some(to) = wanted.to {
                call["to"] = json!(to_hex(&to));
            }

            ask.push(rpc(id, GAS, "eth_estimateGas", json!([call])));
        }

        if wanted.max_price.is_none() {
            ask.push(rpc(id, PRICE, "eth_gasPrice", json!([])));
        }
    }

    if chain.is_none() {
        shown.trouble = Some("The light client has not met the chain yet: wait for it, then try again.".to_string());
    }

    theme::caps(ui, palette, if wanted.to.is_none() { "Deploy a contract" } else { "Send" });
    let coin = ctx.coin;
    theme::fitted(ui, &format::amount(&wanted.value.to_string(), coin.decimals, &coin.symbol), 30.0, 16.0, egui::FontFamily::Name(theme::SERIF.into()), palette.ink);
    ui.add_space(6.0);
    super::row(ui, palette, "From", |ui| super::address(ui, palette, &format!("secp256k1:{}", to_hex(&wanted.from))));

    if let Some(to) = wanted.to {
        let kept = ctx.settings.contracts.iter().find(|kept| kept.address.eq_ignore_ascii_case(&format!("evm:{}", to_hex(&to))));
        super::row(ui, palette, "To", |ui| {
            ui.vertical(|ui| {
                if let Some(kept) = kept {
                    ui.label(RichText::new(&kept.name).color(palette.ink));
                }

                super::address(ui, palette, &to_hex(&to));
            });
        });

        if !wanted.input.is_empty() {
            let decoded = kept
                .and_then(|kept| chainv4::abi::Abi::parse(&kept.abi).ok())
                .and_then(|abi| abi.function_for(&wanted.input).and_then(|function| function.decode_input(&wanted.input).ok().map(|values| format!("{}({})", function.name, function.show_inputs(&values)))));
            super::row(ui, palette, "Calls", |ui| match decoded {
                Some(decoded) => {
                    ui.label(theme::mono(decoded).color(palette.ink));
                }
                None => {
                    ui.label(theme::mono(format!("{} with {} bytes", to_hex(&wanted.input[..wanted.input.len().min(4)]), wanted.input.len())).color(palette.soft))
                        .on_hover_text("Keep the contract with its ABI on the Contracts page to see this by name.");
                }
            });
        }
    } else {
        super::row(ui, palette, "Code", |ui| {
            ui.label(format!("{} bytes", wanted.input.len()));
        });
    }

    match prepared(shown) {
        Some((nonce, gas, price)) => {
            let most = (u256::from(gas) * price).to_string();
            super::row(ui, palette, "Fee, at most", |ui| {
                ui.label(format::amount(&most, coin.decimals, &coin.symbol)).on_hover_text(format!("{gas} gas at up to {price} units"));
            });
            super::row(ui, palette, "Nonce", |ui| {
                ui.label(nonce.to_string());
            });
        }
        None if shown.trouble.is_none() => super::waiting(ui, palette, "Pricing it on the node…"),
        None => {}
    }

    ui.add_space(6.0);
    ui.label(RichText::new("Sent as an Ethereum transaction, from this account's Ethereum key. Nothing the site says here is proven: check the address and the amount.").small().color(palette.warn));
}

/// Settings' part: whether sites may ask, and which are connected. Only in the browser extension.
pub fn settings(ui: &mut Ui, ctx: &Ctx) {
    if !dapp::available() {
        return;
    }

    let palette = *ctx.palette;
    let sites = dapp::sites();
    let on = sites["on"].as_bool().unwrap_or(false);

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Sites");
        ui.label(
            RichText::new("Sites that speak to Ethereum wallets can ask this one to connect, through window.ethereum: your Ethereum accounts, the ones you choose, and each send and signature approved here.")
                .color(palette.soft),
        );
        ui.add_space(8.0);
        let mut wanted = on;

        if ui.checkbox(&mut wanted, "Let sites ask to connect").changed() {
            dapp::let_sites(wanted);
        }

        if sites["wanted"].as_bool() == Some(true) && !on {
            ui.label(RichText::new("Allow it in the bar above: the browser asks for every site, once.").small().color(palette.warn));
        }

        let connected = sites["connected"].as_object().cloned().unwrap_or_default();

        if connected.is_empty() {
            return;
        }

        ui.add_space(8.0);
        theme::caps(ui, &palette, "Connected");

        for (origin, site) in &connected {
            ui.horizontal(|ui| {
                let accounts = site["accounts"].as_array().map_or(0, Vec::len);
                ui.label(RichText::new(origin).color(palette.ink));
                ui.label(RichText::new(format!("{accounts} account{}", if accounts == 1 { "" } else { "s" })).small().color(palette.muted));

                if theme::quiet(ui, &palette, "Disconnect").clicked() {
                    dapp::disconnect(origin);
                }
            });
        }
    });
}
