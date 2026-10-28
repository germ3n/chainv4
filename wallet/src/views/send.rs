//! Sending the coin or a token, from one of the account's addresses. The wallet builds and signs
//! the transaction itself, from what it proved: that address's nonce and balance. The node only prices it, from a probe signed with no
//! fee offer, as `chainv4 send` does, and takes it in. Nothing is signed for real before the
//! review shows what it costs at most.
//!
//! Paid to a name, such as alice.dev, the wallet proves what the name resolves to before it
//! signs: the name's records, and every one above it, against the tip the light client checked.
//! The transaction carries the address, so what the review shows is what is paid.
//!
//! The Names page hands transactions of its own here, prepared, for the same review and signing.
//!
//! A watch-only sender holds no key: its probe signs nothing, which the node never checks, and
//! once priced the transaction goes to the offline signer as a code, and comes back signed as
//! another. The sender sends it only once it is the very one it asked for. An account MetaMask
//! holds goes to MetaMask instead, as EIP-712 typed data for this chain, and its signature is
//! checked the same way.

use {
    super::scan::Scan,
    super::{Ctx, Page},
    crate::airgap::{self, Message, Request, Shown},
    crate::app::Sent,
    crate::biometrics::{self, Check},
    crate::format,
    crate::node::{Ask, Failure},
    crate::theme,
    chainv4::crypto::hasher::Hasher,
    chainv4::crypto::keypair::PublicKey,
    chainv4::structs::address::Address,
    chainv4::structs::amount::Amount,
    chainv4::structs::hash::{Hash, HashType},
    chainv4::structs::hex::to_hex,
    chainv4::structs::tx::{FeeOffer, SignedTx, Tx, TxCallArgumentType},
    chainv4::structs::u256::u256,
    egui::{RichText, Ui},
    serde_json::{Value, json},
};

/// How long a sent transaction counts as waiting, holding its nonce, before the next one may take
/// that nonce again, in milliseconds.
const PENDING_FOR: u64 = super::activity::UNSEEN_AFTER;

/// What MetaMask gave, once it did: its signature, or why there is none.
type Signed = std::sync::Arc<std::sync::Mutex<Option<Result<[u8; 65], String>>>>;

#[derive(Default)]
pub struct SendState {
    /// The address it is sent from: one of the account's, the first until another is picked.
    from: Option<Address>,
    /// What to send: None for the coin, or a token's address.
    token: Option<String>,
    to: String,
    amount: String,
    /// What to add per gas for the producer, in whole units of the coin; empty for the node's
    /// choice.
    tip: String,
    step: Step,
    /// A transaction another page made, to review and send instead of the form's.
    prepared: Option<Prepared>,
    /// The biometric check before a transaction is signed here, while it is under way, and why
    /// the last one failed.
    confirming: Option<Check>,
    confirm_trouble: Option<String>,
}

/// A transaction another page made, such as registering a name: sent from one of the account's
/// addresses, shown as what, moving coin of it beside the fee.
#[derive(Clone, Debug)]
pub struct Prepared {
    pub from: Address,
    pub tx: Tx,
    pub what: String,
    /// What it is for, where a transfer shows whom it pays: the name.
    pub about: String,
    pub coin: u256,
}

/// A transaction built and not yet sent.
#[derive(Clone, Debug)]
struct Draft {
    from: Address,
    /// The key it is signed with, here or on the offline signer.
    signer: PublicKey,
    /// Whom it pays, as shown: an address, a name with the address it resolved to, or what a
    /// prepared transaction is for.
    to: String,
    tx: Tx,
    nonce: u64,
    /// The token it sends, as the registry names it, for the offline signer to show.
    token: Option<Shown>,
    /// What it sends, as shown: 1.5 DEV.
    what: String,
    /// What it moves of the coin, beside the fee: what the review checks the balance for.
    coin: u256,
}

#[derive(Debug, Default)]
enum Step {
    #[default]
    Edit,
    /// Asked the node to price the probe.
    Pricing { draft: Draft, ask: Ask },
    /// Priced; warning is why a dry run on the node says it would fail, if it does.
    Review { draft: Draft, fee: FeeOffer, warning: Option<String> },
    /// Asked of the offline signer, as the code shown, until its signed code is read back.
    Offline { draft: Draft, request: Box<Request>, message: Box<Message>, scan: Box<Scan> },
    /// Asked of MetaMask, until it signs or refuses.
    MetaMask { draft: Draft, fee: FeeOffer, genesis: Hash, signed: Signed },
    /// Submitted, signed with the fee.
    Sending { draft: Draft, ask: Ask },
    Sent { id: String },
    Failed(String),
}

impl SendState {
    /// Sets a transaction another page made to review and send, in place of the form's.
    pub fn prepare(&mut self, prepared: Prepared) {
        self.prepared = Some(prepared);
        self.step = Step::Edit;
    }

    /// The name the To field holds, if it holds one rather than an address: what to prove.
    pub fn wanted_name(&self) -> Option<String> {
        let to = self.to.trim();

        (self.prepared.is_none() && !to.is_empty() && recipient(to).is_none() && !to.contains(':')).then(|| to.to_string())
    }

    /// Takes the node's answer to a pricing or a submission this page asked, and returns the
    /// transaction it sent, once the node took it, for the history to keep.
    pub fn answered(&mut self, ask: &Ask, result: &Result<Value, Failure>, now: u64) -> Option<Sent> {
        match (&self.step, result) {
            (Step::Pricing { draft, ask: asked }, Ok(value)) if asked == ask => {
                self.step = match offer(value) {
                    Ok((fee, warning)) => Step::Review { draft: draft.clone(), fee, warning },
                    Err(why) => Step::Failed(why),
                };

                None
            }
            (Step::Sending { draft, ask: asked }, Ok(value)) if asked == ask => {
                let id = value["id"].as_str().unwrap_or_default().to_string();
                let sent = Sent {
                    id: id.clone(),
                    from: draft.from.to_string(),
                    to: draft.to.clone(),
                    what: draft.what.clone(),
                    nonce: draft.nonce,
                    at: now,
                    genesis: String::new(),
                };
                self.step = Step::Sent { id };
                self.amount.clear();
                self.prepared = None;

                Some(sent)
            }
            (Step::Pricing { ask: asked, .. } | Step::Sending { ask: asked, .. }, Err(failure)) if asked == ask => {
                self.step = Step::Failed(match failure.status {
                    0 => format!("The node could not be reached: {}", failure.message),
                    _ => format!("The node refused it: {}", failure.message),
                });

                None
            }
            _ => None,
        }
    }
}

pub fn show(ui: &mut Ui, ctx: &mut Ctx, state: &mut SendState) {
    let palette = *ctx.palette;
    theme::title(ui, &palette, "Send", "The wallet signs it from what it proved; the node only prices it and passes it on.");

    if let Step::Review { draft, fee, warning } = &state.step {
        let (draft, fee, warning) = (draft.clone(), *fee, warning.clone());
        review(ui, ctx, state, draft, fee, warning);

        return;
    }

    if let Step::Sent { id } = &state.step {
        let id = id.clone();
        sent(ui, ctx, state, &id);

        return;
    }

    if matches!(state.step, Step::Offline { .. }) {
        offline(ui, ctx, state);

        return;
    }

    if matches!(state.step, Step::MetaMask { .. }) {
        metamask(ui, ctx, state);

        return;
    }

    let busy = matches!(state.step, Step::Pricing { .. } | Step::Sending { .. });

    if state.prepared.is_some() {
        prepared(ui, ctx, state, busy);

        return;
    }

    let addresses = ctx.holder().map(|holder| holder.addresses()).unwrap_or_default();

    if !state.from.is_some_and(|from| addresses.contains(&from)) {
        state.from = addresses.first().copied();
    }

    let address = state.from.map(|from| from.to_string()).unwrap_or_default();

    theme::card(ui, &palette, |ui| {
        if addresses.len() > 1 {
            theme::caps(ui, &palette, "From");
            let before = state.from;
            egui::ComboBox::from_id_salt("from")
                .selected_text(paying(ctx, state.from.as_ref()))
                .width(ui.available_width())
                .show_ui(ui, |ui| {
                    for from in &addresses {
                        ui.selectable_value(&mut state.from, Some(*from), paying(ctx, Some(from)));
                    }
                });

            // A token held at one address may not be at another.
            if state.from != before {
                state.token = None;
            }

            ui.add_space(10.0);
        }

        theme::caps(ui, &palette, "What");
        let shown = match &state.token {
            None => ctx.coin.symbol.clone(),
            Some(token) => ctx.token(token).0,
        };
        egui::ComboBox::from_id_salt("asset").selected_text(shown).width(ui.available_width()).show_ui(ui, |ui| {
            ui.selectable_value(&mut state.token, None, format!("{} · the coin", ctx.coin.symbol));

            for token in &ctx.chain.listed {
                let (symbol, _) = ctx.token(token);
                let held = ctx.chain.balance(token, &address).map(|proven| proven.value.to_string());

                if held.as_deref().is_some_and(|held| held != "0") {
                    ui.selectable_value(&mut state.token, Some(token.clone()), format!("{symbol} · {}", format::short(token)));
                }
            }
        });

        let (balance, decimals, symbol) = available(ctx, state.token.as_deref(), &address);
        ui.label(
            RichText::new(match &balance {
                Some(raw) => format!("{} available, proven", format::amount(raw, decimals, &symbol)),
                None => "Proving what is available…".to_string(),
            })
            .small()
            .color(palette.muted),
        );

        ui.add_space(10.0);
        theme::caps(ui, &palette, "To");
        ui.add(egui::TextEdit::singleline(&mut state.to).hint_text("ed25519:0x…, an Ethereum 0x…, a bc1q…, or a name such as alice.dev").font(egui::TextStyle::Monospace).desired_width(f32::INFINITY));

        match destination(ctx, &state.to) {
            Destination::Blank | Destination::Address(_) => {}
            Destination::Named { shown, resolved: Some(resolved) } => match resolved.target() {
                Some(target) => {
                    ui.horizontal_wrapped(|ui| {
                        super::assurance(ui, &palette, &resolved.assurance);
                        ui.label(RichText::new(format!("{shown} resolves to {}", format::short(&target.to_string()))).small().color(palette.soft));
                    });
                }
                None => {
                    ui.colored_label(palette.bad, format!("{shown} resolves to nothing: nobody holds it, it expired, or it points nowhere."));
                }
            },
            Destination::Named { shown, resolved: None } => {
                ui.label(RichText::new(format!("Proving what {shown} resolves to…")).small().color(palette.muted));
            }
            Destination::Bad => {
                ui.colored_label(palette.bad, "Not an address or a name: an address starts with its kind, such as ed25519:0x and 32 bytes of hex, or is Ethereum's, 0x and 20 bytes; a name is such as alice.dev.");
            }
        }

        ui.add_space(10.0);
        theme::caps(ui, &palette, "Amount");
        let max = super::beside(
            ui,
            |ui| balance.is_some() && state.token.is_some() && theme::quiet(ui, &palette, "Max").clicked(),
            |ui, width| {
                ui.add(egui::TextEdit::singleline(&mut state.amount).hint_text("0.0").desired_width(width));
            },
        );

        if max && let Some(raw) = &balance {
            state.amount = format::units(raw, decimals).replace(',', "");
        }

        ui.add_space(10.0);
        egui::CollapsingHeader::new(RichText::new("Fee").color(palette.soft)).id_salt("fee").show(ui, |ui| {
            ui.label(RichText::new("The node offers a fee sure to cover it now. A tip per gas, in the coin, asks producers to take it sooner.").small().color(palette.muted));
            ui.add(egui::TextEdit::singleline(&mut state.tip).hint_text("tip per gas, such as 0.000000001").desired_width(f32::INFINITY));
        });

        ui.add_space(12.0);

        if let Step::Failed(why) = &state.step {
            ui.colored_label(palette.bad, why);
            ui.add_space(6.0);
        }

        ui.horizontal_wrapped(|ui| {
            if theme::primary(ui, &palette, "Review", !busy).clicked() {
                state.step = match draft(ctx, state) {
                    Ok((draft, body)) => {
                        let ask = Ask::Offer(body);
                        ctx.node.ask(ask.clone());

                        Step::Pricing { draft, ask }
                    }
                    Err(why) => Step::Failed(why),
                };
            }

            if busy {
                ui.spinner();
                ui.label(RichText::new("Asking the node for a fee offer…").color(palette.muted));
            }
        });
    });
}

/// An address to pay from, as the picker shows it: its scheme, cut short, and its coin.
fn paying(ctx: &Ctx, from: Option<&Address>) -> String {
    let Some(from) = from else {
        return String::new();
    };
    let text = from.to_string();
    let held = ctx.chain.account(&text).map_or_else(|| "…".to_string(), |proven| ctx.brief(None, &proven.value.balance.to_string()));

    // The scheme leads, so the address goes without its kind.
    let short = format::short(&text);
    let hex = short.split_once(':').map_or(short.as_str(), |(_, hex)| hex);

    format!("{} · {hex} · {held}", super::scheme(from))
}

/// What the coin a prepared transaction carries does: a name's price is burned; any other goes
/// with its call, into a pool, a swap's lock or the contract called.
fn coin_label(tx: &Tx) -> &'static str {
    match tx {
        Tx::Name { .. } => "Burned",
        _ => "Sends",
    }
}

/// A transaction another page prepared, waiting to be reviewed.
fn prepared(ui: &mut Ui, ctx: &mut Ctx, state: &mut SendState, busy: bool) {
    let palette = *ctx.palette;
    let Some(prepared) = state.prepared.clone() else {
        return;
    };

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "To send");
        ui.add_space(4.0);
        theme::fitted(ui, &prepared.what, 26.0, 14.0, egui::FontFamily::Name(theme::SERIF.into()), palette.ink);
        ui.add_space(8.0);
        super::row(ui, &palette, "From", |ui| super::address(ui, &palette, &prepared.from.to_string()));
        super::row(ui, &palette, "For", |ui| {
            ui.label(RichText::new(&prepared.about).color(palette.ink));
        });

        if prepared.coin != u256::ZERO {
            super::row(ui, &palette, coin_label(&prepared.tx), |ui| {
                ui.label(ctx.amount(None, &prepared.coin.to_string()));
            });
        }

        ui.add_space(12.0);

        if let Step::Failed(why) = &state.step {
            ui.colored_label(palette.bad, why);
            ui.add_space(6.0);
        }

        ui.horizontal_wrapped(|ui| {
            if theme::primary(ui, &palette, "Review", !busy).clicked() {
                state.step = match draft(ctx, state) {
                    Ok((draft, body)) => {
                        let ask = Ask::Offer(body);
                        ctx.node.ask(ask.clone());

                        Step::Pricing { draft, ask }
                    }
                    Err(why) => Step::Failed(why),
                };
            }

            if theme::quiet(ui, &palette, "Cancel").clicked() {
                state.prepared = None;
                state.step = Step::Edit;
                *ctx.page = Page::Names;
            }

            if busy {
                ui.spinner();
            }
        });
    });
}

/// Whom the To field names.
enum Destination {
    Blank,
    Address(Address),
    /// A name, as shown with the suffix, and what was proven of it so far.
    Named { shown: String, resolved: Option<crate::chain::Resolved> },
    Bad,
}

fn destination(ctx: &Ctx, text: &str) -> Destination {
    let text = text.trim();

    if text.is_empty() {
        return Destination::Blank;
    }

    if let Some(address) = recipient(text) {
        return Destination::Address(address);
    }

    match &ctx.chain.names {
        Some(names) if !text.contains(':') && chainv4::registry::names::name_problem(&names.strip(text)).is_none() => Destination::Named {
            shown: names.display(&names.strip(text)),
            resolved: ctx.chain.resolve(text),
        },
        _ => Destination::Bad,
    }
}

/// The address to send to, as typed: with its kind, or an Ethereum one as Ethereum writes it, 0x
/// and 40 hex digits.
pub(super) fn recipient(text: &str) -> Option<Address> {
    let text = text.trim();

    match text.len() == 42 && text.starts_with("0x") {
        true => format!("secp256k1:{text}").parse().ok(),
        false => text.parse().ok(),
    }
}

/// What can be sent, proven, with its decimals and symbol.
fn available(ctx: &Ctx, token: Option<&str>, address: &str) -> (Option<String>, u8, String) {
    match token {
        None => (
            ctx.chain.account(address).map(|proven| proven.value.balance.to_string()),
            ctx.coin.decimals,
            ctx.coin.symbol.clone(),
        ),
        Some(token) => {
            let (symbol, decimals) = ctx.token(token);

            (ctx.chain.balance(token, address).map(|proven| proven.value.to_string()), decimals, symbol)
        }
    }
}

/// The transaction the form asks for, and the body of POST /offer for its probe.
fn draft(ctx: &Ctx, state: &SendState) -> Result<(Draft, String), String> {
    let holder = ctx.holder().ok_or("there is no account to send from")?;
    let from = state.from.ok_or("there is no address to send from")?;
    let signer = holder.public(&from).ok_or("that address is not this account's")?;
    let address = from.to_string();
    let account = ctx.chain.account(&address).ok_or("the address is not proven yet: wait for the light client")?;

    if let Some(prepared) = &state.prepared {
        return finish(ctx, state, from, signer, prepared.tx.clone(), prepared.coin, None, prepared.about.clone(), prepared.what.clone(), account.value.nonce);
    }

    let (to, shown) = match destination(ctx, &state.to) {
        Destination::Address(address) => (address, address.to_string()),
        Destination::Named { shown, resolved: Some(resolved) } => {
            let target = resolved.target().ok_or(format!("{shown} resolves to nothing"))?;

            (target, format!("{shown} · {target}"))
        }
        Destination::Named { shown, resolved: None } => return Err(format!("{shown} is not proven yet: wait a moment")),
        Destination::Blank | Destination::Bad => return Err("the address to send to does not read".to_string()),
    };
    let (balance, decimals, symbol) = available(ctx, state.token.as_deref(), &address);
    let units = format::parse_units(&state.amount, decimals).ok_or(format!("the amount does not read as {symbol}, with at most {decimals} decimals"))?;
    let units = u256::from_str_radix(&units, 10).map_err(|_| "the amount is too large".to_string())?;
    let held = balance.and_then(|raw| u256::from_str_radix(&raw, 10).ok()).ok_or("the balance is not proven yet")?;

    if units == u256::ZERO {
        return Err("send more than nothing".to_string());
    }

    if units > held {
        return Err(format!("that is more than the {} available", format::amount(&held.to_string(), decimals, &symbol)));
    }

    let (tx, coin, token) = match &state.token {
        None => (Tx::Transfer { from, to, amount: amount(units) }, units, None),
        Some(token) => {
            let token: Address = token.parse().map_err(|_| "the token's address does not read".to_string())?;
            let tx = Tx::ContractCall {
                to: token,
                func: Hasher::digest(HashType::Xxh3_64, b"transfer"),
                amount: Amount::U128(0),
                data: vec![TxCallArgumentType::Address(to), TxCallArgumentType::U256(units)],
            };

            (tx, u256::ZERO, Some(Shown { symbol: symbol.clone(), decimals }))
        }
    };
    let what = format::amount(&units.to_string(), decimals, &symbol);

    finish(ctx, state, from, signer, tx, coin, token, shown, what, account.value.nonce)
}

/// The draft of a transaction, and the body of POST /offer for its probe, at the next nonce.
#[allow(clippy::too_many_arguments)]
fn finish(ctx: &Ctx, state: &SendState, from: Address, signer: PublicKey, tx: Tx, coin: u256, token: Option<Shown>, to: String, what: String, proven_nonce: u64) -> Result<(Draft, String), String> {
    let holder = ctx.holder().ok_or("there is no account to send from")?;
    let address = from.to_string();
    let nonce = next_nonce(proven_nonce, &ctx.settings.sent, &address, ctx.now);
    let probe = match holder.key(&from) {
        Some(key) => SignedTx::sign(tx.clone(), nonce, FeeOffer::NONE, key),
        None => airgap::probe(tx.clone(), nonce, signer),
    };
    let mut body = json!({ "tx": encode(&probe) });

    if !state.tip.trim().is_empty() {
        let tip = format::parse_units(&state.tip, ctx.coin.decimals).ok_or("the tip does not read as an amount of the coin")?;
        body["tip"] = tip.into();
    }

    let draft = Draft {
        from,
        signer,
        to,
        tx,
        nonce,
        token,
        what,
        coin,
    };

    Ok((draft, body.to_string()))
}

/// What the transaction costs at most, and the button that signs and sends it.
fn review(ui: &mut Ui, ctx: &mut Ctx, state: &mut SendState, draft: Draft, fee: FeeOffer, warning: Option<String>) {
    let palette = *ctx.palette;
    let most = u256::from(fee.gas_limit) * fee.max_price;
    let balance = ctx.chain.account(&draft.from.to_string()).map(|proven| proven.value.balance);
    let short = balance.is_some_and(|balance| balance < draft.coin.saturating_add(most));

    // What the check before signing came to: passed, it signs; turned down, nothing; the fingers
    // or faces enrolled changed since, the wallet locks, for its password.
    let mut confirmed = false;

    if let Some(came) = state.confirming.as_ref().and_then(Check::take) {
        state.confirming = None;

        match came {
            Ok(_) => confirmed = true,
            Err(chainv4_ui::biometric::Failure::Canceled) => {}
            Err(chainv4_ui::biometric::Failure::Invalidated) => {
                biometrics::turn_off(ctx.wake, ctx.settings);
                *ctx.lock = Some(biometrics::CHANGED.to_string());
            }
            Err(chainv4_ui::biometric::Failure::Failed(why)) => state.confirm_trouble = Some(why),
        }
    }

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Review");
        ui.add_space(4.0);
        theme::fitted(ui, &draft.what, 34.0, 16.0, egui::FontFamily::Name(theme::SERIF.into()), palette.ink);
        ui.add_space(8.0);
        super::row(ui, &palette, "From", |ui| super::address(ui, &palette, &draft.from.to_string()));
        super::row(ui, &palette, "To", |ui| super::address(ui, &palette, &draft.to));
        super::row(ui, &palette, "Fee, at most", |ui| {
            ui.label(ctx.amount(None, &most.to_string()))
                .on_hover_text(format!("{} gas at up to {} per gas; what it does not use is refunded", fee.gas_limit, ctx.amount(None, &fee.max_price.to_string())));
        });

        if fee.tip != u256::ZERO {
            super::row(ui, &palette, "Tip per gas", |ui| {
                ui.label(ctx.amount(None, &fee.tip.to_string()));
            });
        }

        super::row(ui, &palette, "Nonce", |ui| {
            ui.label(draft.nonce.to_string());
        });

        if short {
            ui.add_space(6.0);
            ui.colored_label(palette.bad, "The balance does not cover this and its fee.");
        }

        if let Some(warning) = &warning {
            ui.add_space(6.0);
            ui.colored_label(palette.bad, format!("A dry run on the node says this would fail: {warning}"));
            ui.label(RichText::new("Sent anyway, it uses its nonce and pays its fee, unless things change before it lands.").small().color(palette.muted));
        }

        ui.add_space(12.0);
        ui.horizontal_wrapped(|ui| {
            let key = ctx.holders.iter().find_map(|holder| holder.key(&draft.from));
            let by_metamask = ctx.holders.iter().any(|holder| holder.by_metamask() && holder.addresses().contains(&draft.from));
            let send = match (key.is_some(), by_metamask, warning.is_some()) {
                (true, _, false) => "Sign and send",
                (true, _, true) => "Send anyway",
                (false, true, false) => "Sign with MetaMask",
                (false, true, true) => "Sign with MetaMask anyway",
                (false, false, false) => "Sign on the signer",
                (false, false, true) => "Sign on the signer anyway",
            };

            if theme::primary(ui, &palette, send, !short && state.confirming.is_none()).clicked() || confirmed {
                match key {
                    None if by_metamask => match ctx.chain.genesis {
                        Some(genesis) => {
                            let signed = Signed::default();
                            let (inbox, wake) = (signed.clone(), ctx.wake.clone());
                            let typed = crate::metamask::typed_data(&draft.tx, draft.nonce, &fee, &genesis);
                            crate::metamask::sign(ethereum(&draft.from), typed, move |answer| {
                                *inbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(answer);
                                wake.request_repaint();
                            });
                            state.step = Step::MetaMask { draft: draft.clone(), fee, genesis, signed };
                        }
                        None => state.step = Step::Failed("The light client has not started: the chain to sign for is not known yet.".to_string()),
                    },
                    // With biometrics on, a check first; signing once it passes.
                    Some(_) if !confirmed && let Some((enrolled, _)) = biometrics::enrolled(ctx.wake, ctx.settings) => {
                        state.confirm_trouble = None;
                        state.confirming = Some(Check::start(ctx.wake, &enrolled, &format!("Send {}", draft.what)));
                    }
                    Some(key) => {
                        let signed = SignedTx::sign(draft.tx.clone(), draft.nonce, fee, key);
                        let ask = Ask::Submit(json!({ "tx": encode(&signed) }).to_string());
                        ctx.node.ask(ask.clone());
                        state.step = Step::Sending { draft: draft.clone(), ask };
                    }
                    None => {
                        let request = Request {
                            tx: draft.tx.clone(),
                            nonce: draft.nonce,
                            fee,
                            signer: draft.signer,
                            coin: Shown {
                                symbol: ctx.coin.symbol.clone(),
                                decimals: ctx.coin.decimals,
                            },
                            token: draft.token.clone(),
                        };
                        state.step = Step::Offline {
                            draft: draft.clone(),
                            message: Box::new(Message::Sign(request.clone())),
                            request: Box::new(request),
                            scan: Box::default(),
                        };
                    }
                }
            }

            if theme::quiet(ui, &palette, "Back").clicked() {
                state.step = Step::Edit;
                state.confirming = None;
            }
        });

        if state.confirming.is_some() {
            ui.label(RichText::new("Confirm it on the prompt to sign it.").small().color(palette.muted));
        }

        if let Some(why) = &state.confirm_trouble {
            ui.colored_label(palette.bad, why.as_str());
        }
    });
}

/// The request, as a code for the offline signer, and reading back what it signed: sent once it
/// is the very transaction asked for, signed by the key asked.
fn offline(ui: &mut Ui, ctx: &mut Ctx, state: &mut SendState) {
    let palette = *ctx.palette;
    let Step::Offline { draft, request, message, scan } = &mut state.step else {
        return;
    };
    let mut next = None;

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Sign it on the signer");
        ui.label(RichText::new(format!("{} to {}: on the offline signer, open Sign and read this code.", draft.what, format::short(&draft.to))).color(palette.soft));
        ui.add_space(10.0);
        super::code(ui, &palette, message);
        ui.add_space(14.0);
        theme::caps(ui, &palette, "Then read what it signed");

        match scan.show(ui, &palette) {
            Some(Message::Signed(signed)) => match airgap::check(request, &signed) {
                Ok(()) => {
                    let ask = Ask::Submit(json!({ "tx": encode(&signed) }).to_string());
                    ctx.node.ask(ask.clone());
                    next = Some(Step::Sending { draft: draft.clone(), ask });
                }
                Err(why) => scan.fail(why),
            },
            Some(other) => scan.refuse(&other, "a signed transaction"),
            None => {}
        }

        ui.add_space(10.0);

        if theme::quiet(ui, &palette, "Back").clicked() {
            next = Some(Step::Edit);
        }
    });

    if let Some(next) = next {
        state.step = next;
    }
}

/// Waiting for MetaMask, then the transaction it signed, checked and sent.
fn metamask(ui: &mut Ui, ctx: &mut Ctx, state: &mut SendState) {
    let palette = *ctx.palette;
    let Step::MetaMask { draft, fee, genesis, signed } = &state.step else {
        return;
    };
    let answer = signed.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();

    if let Some(answer) = answer {
        let hash = chainv4::crypto::eip712::transaction_digest(&draft.tx, draft.nonce, fee, genesis);
        let checked = answer
            .and_then(|signature| crate::metamask::key_of(&hash, &signature, &ethereum(&draft.from)))
            .and_then(|(key, low)| {
                let signed = SignedTx {
                    tx: draft.tx.clone(),
                    nonce: draft.nonce,
                    fee: *fee,
                    signer: PublicKey::Secp256k1(key),
                    signature: chainv4::crypto::keypair::Signature::Secp256k1Eip712(low),
                };
                signed.verify_on(genesis).map_err(|err| format!("MetaMask's signature does not hold: {err}"))?;

                Ok(signed)
            });

        state.step = match checked {
            Ok(signed) => {
                let ask = Ask::Submit(json!({ "tx": encode(&signed) }).to_string());
                ctx.node.ask(ask.clone());

                Step::Sending { draft: draft.clone(), ask }
            }
            Err(why) => Step::Failed(why),
        };

        return;
    }

    let waiting = format!("{} to {}: MetaMask shows each field to check before you sign.", draft.what, format::short(&draft.to));
    let mut back = false;

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Sign it in MetaMask");
        ui.horizontal_wrapped(|ui| {
            ui.spinner();
            ui.label(RichText::new(waiting).color(palette.soft));
        });
        ui.add_space(10.0);
        back = theme::quiet(ui, &palette, "Back").clicked();
    });

    if back {
        state.step = Step::Edit;
    }
}

/// An Ethereum address as MetaMask names an account: 0x and 40 hex digits.
fn ethereum(address: &Address) -> String {
    match address {
        Address::Secp256k1(bytes) => to_hex(bytes),
        other => other.to_string(),
    }
}

/// What became of a transaction just sent.
fn sent(ui: &mut Ui, ctx: &mut Ctx, state: &mut SendState, id: &str) {
    let palette = *ctx.palette;

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Sent");
        ui.add_space(4.0);
        let (text, color) = match ctx.settings.checked.get(id) {
            Some(checked) if !checked.done => ("It landed, and failed: it used its nonce and paid its fee.".to_string(), palette.bad),
            Some(checked) if checked.final_ => (format!("Final at block {}.", checked.height), palette.good),
            Some(checked) => (format!("Proven in block {}, not final yet.", checked.height), palette.accent),
            None => ("The node took it. Waiting for a block to hold it…".to_string(), palette.warn),
        };
        ui.horizontal_wrapped(|ui| {
            theme::dot(ui, color);
            ui.label(RichText::new(text).color(palette.ink));
        });
        ui.add_space(6.0);
        super::row(ui, &palette, "Transaction", |ui| super::address(ui, &palette, id));
        ui.add_space(12.0);
        ui.horizontal_wrapped(|ui| {
            if theme::primary(ui, &palette, "Send another", true).clicked() {
                state.step = Step::Edit;
            }


            if theme::quiet(ui, &palette, "Activity").clicked() {
                *ctx.page = Page::Activity;
                state.step = Step::Edit;
            }
        });
    });
}

/// The nonce the next transaction takes: the proven one, past any this wallet sent that is still
/// waiting. One unseen for long is taken to be dropped, so its nonce is used again.
pub fn next_nonce(proven: u64, sent: &[Sent], from: &str, now: u64) -> u64 {
    sent.iter()
        .filter(|sent| sent.from == from && sent.nonce >= proven && now.saturating_sub(sent.at) < PENDING_FOR)
        .map(|sent| sent.nonce + 1)
        .max()
        .unwrap_or(proven)
        .max(proven)
}

/// The narrowest width an amount fits.
fn amount(units: u256) -> Amount {
    match u128::try_from(units) {
        Ok(narrow) => Amount::U128(narrow),
        Err(_) => Amount::U256(units),
    }
}

fn encode(signed: &SignedTx) -> String {
    to_hex(&wincode::serialize(signed).expect("a signed transaction always encodes"))
}

/// A fee offer as POST /offer answers it, and why its dry run says the transaction would fail, if
/// it does.
fn offer(value: &Value) -> Result<(FeeOffer, Option<String>), String> {
    let wide = |name: &str| {
        value[name]
            .as_str()
            .and_then(|text| u256::from_str_radix(text, 10).ok())
            .ok_or_else(|| format!("the node sent a bad offer: no {name}"))
    };

    let fee = FeeOffer {
        gas_limit: value["gas_limit"].as_u64().ok_or("the node sent a bad offer: no gas_limit")?,
        max_price: wide("max_price")?,
        tip: wide("tip")?,
    };
    let warning = (value["estimate"]["status"] == "failed").then(|| value["estimate"]["error"].as_str().unwrap_or("no reason given").to_string());

    Ok((fee, warning))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_s_price_is_burned_and_any_other_coin_goes_with_its_call() {
        use chainv4::registry::names::NameAction;
        use chainv4::structs::address::Address;

        let name = Tx::Name { name: "alice".to_string(), action: NameAction::Register { years: 1 } };
        let swap = Tx::EvmCall { to: Address::Evm([1; 20]), value: chainv4::structs::amount::Amount::U128(40), input: Vec::new() };

        assert_eq!(coin_label(&name), "Burned");
        assert_eq!(coin_label(&swap), "Sends", "into the pool, not burned");
    }

    fn sent(nonce: u64, at: u64) -> Sent {
        Sent {
            id: format!("0x{nonce}"),
            from: "ed25519:0x01".to_string(),
            to: "ed25519:0x02".to_string(),
            what: "1 DEV".to_string(),
            nonce,
            at,
            genesis: String::new(),
        }
    }

    #[test]
    fn the_next_nonce_skips_what_is_still_waiting() {
        let now = 1_000_000;
        let waiting = [sent(3, now - 1_000), sent(4, now - 500)];

        assert_eq!(next_nonce(3, &waiting, "ed25519:0x01", now), 5, "past both that wait");
        assert_eq!(next_nonce(5, &waiting, "ed25519:0x01", now), 5, "both landed");
        assert_eq!(next_nonce(3, &waiting, "ed25519:0x09", now), 3, "another account's");
        assert_eq!(next_nonce(3, &[sent(3, now - PENDING_FOR)], "ed25519:0x01", now), 3, "unseen for long, so dropped");
    }

    #[test]
    fn offers_and_amounts_read_as_the_node_and_the_chain_take_them() {
        let (fee, warning) = offer(&json!({ "gas_limit": 21000, "max_price": "2000000000", "tip": "3" })).unwrap();

        assert_eq!((fee.gas_limit, fee.max_price, fee.tip), (21000, u256::new(2_000_000_000), u256::new(3)));
        assert_eq!(warning, None, "no dry run, as for a transfer");

        let ran = json!({ "gas_limit": 30000, "max_price": "1", "tip": "0", "estimate": { "gas_used": 24000, "fuel_used": 900, "status": "ok", "error": null } });

        assert_eq!(offer(&ran).unwrap(), (FeeOffer { gas_limit: 30000, max_price: u256::ONE, tip: u256::ZERO }, None));

        let failing = json!({ "gas_limit": 10000000, "max_price": "1", "tip": "0", "estimate": { "gas_used": 0, "fuel_used": 0, "status": "failed", "error": "insufficient balance" } });

        assert_eq!(offer(&failing).unwrap().1.as_deref(), Some("insufficient balance"));
        assert!(offer(&json!({ "gas_limit": 1, "max_price": "x", "tip": "0" })).is_err());
        assert_eq!(amount(u256::new(5)), Amount::U128(5));
        assert!(matches!(amount(u256::MAX), Amount::U256(_)));
    }

    #[test]
    fn an_ethereum_address_reads_as_ethereum_writes_it() {
        let ethereum = "0x2c7536E3605D9C16a7a3D7b1898e529396a65c23";

        assert_eq!(recipient(ethereum).map(|to| to.to_string()), Some("secp256k1:0x2c7536e3605d9c16a7a3d7b1898e529396a65c23".to_string()));
        assert_eq!(recipient(&format!(" secp256k1:{ethereum} ")), recipient(ethereum));
        assert_eq!(recipient(&format!("ed25519:0x{}", "ab".repeat(32))), Some(Address::Ed25519([0xab; 32])));
        assert_eq!(recipient("0x1234"), None);
        assert_eq!(recipient(&format!("0x{}", "ab".repeat(32))), None, "no kind, and not Ethereum's length");
    }
}
