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
    /// Whether what the node just took was a batch the Batch page built, so that page starts afresh.
    batch_sent: bool,
    /// The gas limit and most per gas, in the coin's smallest units, as typed: what a transaction
    /// offers with no node to ask, starting from what checked blocks suggest.
    fee_typed: Option<(String, String, FeeOffer, bool)>,
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
    /// Whether a batch went out since last asked.
    pub fn take_batch_sent(&mut self) -> bool {
        std::mem::take(&mut self.batch_sent)
    }

    /// Sets a transaction another page made to review and send, in place of the form's.
    pub fn prepare(&mut self, prepared: Prepared) {
        // Signed by the address it was prepared for, not whichever the form had picked.
        self.from = Some(prepared.from);
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
                let draft_tx = Some(draft.tx.clone());
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
                self.batch_sent = matches!(draft_tx, Some(Tx::Batch { .. }));
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
    theme::title(ui, &palette, "Send", match ctx.settings.read_blocks {
        true => "The wallet signs it from what it read from checked blocks, and prices it from them; the node only passes it on.",
        false => "The wallet signs it from what it proved; the node only prices it and passes it on.",
    });

    if let Step::Sent { id } = &state.step {
        let id = id.clone();
        sent(ui, ctx, state, &id);

        return;
    }

    // A signature holds on one chain alone, so with none known nothing here can be signed: the chain
    // comes from the light client, never from a guess or a genesis pinned but not yet checked.
    let Some(genesis) = ctx.chain.genesis else {
        theme::card(ui, &palette, |ui| {
            ui.colored_label(palette.warn, NO_CHAIN);
        });

        return;
    };

    if let Step::Review { draft, fee, warning } = &state.step {
        let (draft, fee, warning) = (draft.clone(), *fee, warning.clone());
        review(ui, ctx, state, draft, fee, warning, genesis);

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

    super::from::settle(ctx, &mut state.from, &|_| None);
    let several = ctx.holder().is_some_and(|holder| holder.addresses().len() > 1);
    let address = state.from.map(|from| from.to_string()).unwrap_or_default();

    theme::card(ui, &palette, |ui| {
        // A token held at one address may not be at another.
        if super::from::pick(ui, ctx, "send", &mut state.from, &|_| None) {
            state.token = None;
        }

        if several {
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
            Destination::NeedsAsking => {
                ui.colored_label(palette.bad, format!("Not an address. Paying to a name: {}, so give the address.", crate::node::NEEDS_ASKING));
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
            ui.label(RichText::new(match ctx.settings.read_blocks {
                true => "The next step suggests a fee from checked blocks. A tip per gas, in the coin, asks producers to take it sooner.",
                false => "The node offers a fee sure to cover it now. A tip per gas, in the coin, asks producers to take it sooner.",
            }).small().color(palette.muted));
            ui.add(egui::TextEdit::singleline(&mut state.tip).hint_text("tip per gas, such as 0.000000001").desired_width(f32::INFINITY));
        });

        ui.add_space(12.0);

        if let Step::Failed(why) = &state.step {
            ui.colored_label(palette.bad, why);
            ui.add_space(6.0);
        }

        ui.horizontal_wrapped(|ui| {
            if theme::primary(ui, &palette, "Review", !busy).clicked() {
                state.step = priced(ctx, state);
            }

            if busy {
                ui.spinner();
                ui.label(RichText::new("Asking the node for a fee offer…").color(palette.muted));
            }
        });
    });
}

/// What the Review button comes to: asking the node to price it, or, with Read from checked blocks on,
/// what those blocks suggest, to change on the review, as no node is asked.
fn priced(ctx: &mut Ctx, state: &mut SendState) -> Step {
    let (draft, body) = match draft(ctx, state) {
        Ok(drafted) => drafted,
        Err(why) => return Step::Failed(why),
    };

    if ctx.settings.read_blocks {
        return match offered_from_blocks(ctx, state, &draft) {
            Ok(fee) => Step::Review { draft, fee, warning: None },
            Err(why) => Step::Failed(why),
        };
    }

    let ask = Ask::Offer(body);
    ctx.node.ask(ask.clone());

    Step::Pricing { draft, ask }
}

/// A fee offer with no node to ask, and notes it for the review to change: only for what can be
/// priced from what blocks show. What the Name, Multisig or EVM pages prepare needs a node's dry run,
/// which is what setting its gas limit takes.
fn offered_from_blocks(ctx: &Ctx, state: &mut SendState, draft: &Draft) -> Result<FeeOffer, String> {
    if !matches!(draft.tx, Tx::Transfer { .. } | Tx::ContractCall { .. }) {
        return Err(format!("Pricing a transaction of this kind: {}.", crate::node::NEEDS_ASKING));
    }

    let client = ctx.chain.client.as_ref().ok_or("the light client has not started")?;
    let tip = match state.tip.trim() {
        "" => u256::ZERO,
        typed => u256::from_str_radix(&format::parse_units(typed, ctx.coin.decimals).ok_or("the tip does not read as an amount of the coin")?, 10).map_err(|_| "the tip is too large".to_string())?,
    };
    let (fee, seen) = ctx.reading.suggest(&draft.tx, client.tip().base_price, tip);

    state.fee_typed = Some((fee.gas_limit.to_string(), fee.max_price.to_string(), fee, seen));

    Ok(fee)
}

/// The offer as typed on the review, or what was suggested until it reads.
fn typed_offer(state: &SendState, fee: FeeOffer) -> Option<FeeOffer> {
    let (gas, price, _, _) = state.fee_typed.as_ref()?;

    Some(FeeOffer { gas_limit: gas.trim().parse().ok()?, max_price: u256::from_str_radix(price.trim(), 10).ok()?, tip: fee.tip })
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

        // A batch's are its steps: the count, the total, and each of them.
        match super::steps::of_tx(ctx, &prepared.tx, None) {
            Some(steps) if matches!(prepared.tx, Tx::Batch { .. }) => super::steps::listed(ui, ctx, &steps),
            laid => {
                super::row(ui, &palette, "For", |ui| {
                    ui.label(RichText::new(&prepared.about).color(palette.ink));
                });

                if let Some(steps) = laid {
                    super::steps::listed(ui, ctx, &steps);
                }

                if prepared.coin != u256::ZERO {
                    super::row(ui, &palette, coin_label(&prepared.tx), |ui| {
                        ui.label(ctx.amount(None, &prepared.coin.to_string()));
                    });
                }
            }
        }

        ui.add_space(12.0);

        if let Step::Failed(why) = &state.step {
            ui.colored_label(palette.bad, why);
            ui.add_space(6.0);
        }

        ui.horizontal_wrapped(|ui| {
            if theme::primary(ui, &palette, "Review", !busy).clicked() {
                state.step = priced(ctx, state);
            }

            if theme::quiet(ui, &palette, "Cancel").clicked() {
                // A batch goes back to its steps, still there to change.
                *ctx.page = match prepared.tx {
                    Tx::Batch { .. } => Page::Batch,
                    _ => Page::Names,
                };
                state.prepared = None;
                state.step = Step::Edit;
            }

            if busy {
                ui.spinner();
            }
        });
    });
}

/// Whom the To field names.
pub(super) enum Destination {
    Blank,
    Address(Address),
    /// A name, as shown with the suffix, and what was proven of it so far.
    Named { shown: String, resolved: Option<crate::chain::Resolved> },
    /// What may be a name, with Read from checked blocks on: what it resolves to is asked of a node.
    NeedsAsking,
    Bad,
}

pub(super) fn destination(ctx: &Ctx, text: &str) -> Destination {
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
        _ if ctx.settings.read_blocks && !text.contains(':') => Destination::NeedsAsking,
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
        Destination::Blank | Destination::Bad | Destination::NeedsAsking => return Err("the address to send to does not read".to_string()),
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

/// What Send says while the chain to sign for is not known.
const NO_CHAIN: &str = "The wallet cannot sign yet: a signature holds on one chain alone, and the light client has not said which chain this node is on. It starts once the node answers.";

/// The NFT or multi-token collection a call goes to, as the light client proved the registry names
/// it, for the offline signer to show: none for a call to anything else.
fn named_collection(ctx: &Ctx, tx: &Tx) -> Option<crate::airgap::NamedCollection> {
    let Tx::ContractCall { to, .. } = tx else {
        return None;
    };
    let collection = ctx.chain.collections.get(&to.to_string())?.value.clone();

    Some(crate::airgap::NamedCollection {
        name: collection.name.unwrap_or_default(),
        symbol: collection.symbol.unwrap_or_default(),
        multi: collection.kind == chainv4::registry::collections::CollectionKind::Multi,
    })
}

/// The draft of a transaction, and the body of POST /offer for its probe, at the next nonce.
#[allow(clippy::too_many_arguments)]
fn finish(ctx: &Ctx, state: &SendState, from: Address, signer: PublicKey, tx: Tx, coin: u256, token: Option<Shown>, to: String, what: String, proven_nonce: u64) -> Result<(Draft, String), String> {
    let holder = ctx.holder().ok_or("there is no account to send from")?;
    let genesis = ctx.chain.genesis.ok_or(NO_CHAIN)?;
    let address = from.to_string();
    let nonce = next_nonce(proven_nonce, &ctx.settings.sent, &address, ctx.now);
    let probe = match holder.key(&from) {
        Some(key) => SignedTx::sign(tx.clone(), nonce, FeeOffer::NONE, key, &genesis),
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
#[allow(clippy::too_many_arguments)]
fn review(ui: &mut Ui, ctx: &mut Ctx, state: &mut SendState, draft: Draft, fee: FeeOffer, warning: Option<String>, genesis: Hash) {
    let palette = *ctx.palette;
    // What Read from checked blocks suggested may be changed here, and the offer is what was typed.
    let fee = if ctx.settings.read_blocks { typed_offer(state, fee).unwrap_or(fee) } else { fee };
    let most = u256::from(fee.gas_limit).saturating_mul(fee.max_price);
    let balance = ctx.chain.account(&draft.from.to_string()).map(|proven| proven.value.balance);
    let short = balance.is_some_and(|balance| balance < draft.coin.saturating_add(most));

    // What the check before signing came to: passed, it signs; turned down, nothing; the fingers
    // or faces enrolled changed since, the wallet locks.
    let mut confirmed = false;

    if let Some(came) = state.confirming.as_ref().and_then(Check::take) {
        state.confirming = None;

        match came {
            Ok(_) => confirmed = true,
            Err(chainv4_ui::biometric::Failure::Canceled) => {}
            Err(chainv4_ui::biometric::Failure::Invalidated) => {
                *ctx.lock = Some(biometrics::invalidated(ctx.wake, ctx.settings));
            }
            Err(chainv4_ui::biometric::Failure::Failed(why)) => state.confirm_trouble = Some(why),
        }
    }

    let landing = super::steps::landing_of(ctx, &draft.tx, draft.from, draft.nonce);
    let steps = super::steps::of_tx(ctx, &draft.tx, landing.as_ref());
    // A batch's own review: an approval of one still says whom it approves for.
    let whole_batch = matches!(draft.tx, Tx::Batch { .. });
    // A single transaction is laid out in the same step frame, so its amount, recipient and
    // calldata all show, not only the headline the page wrote.
    let one = (!whole_batch).then(|| super::steps::of_one(ctx, &draft.tx, landing.as_ref()));

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Review");
        ui.add_space(4.0);
        theme::fitted(ui, &draft.what, 34.0, 16.0, egui::FontFamily::Name(theme::SERIF.into()), palette.ink);
        ui.add_space(8.0);

        if let Some(steps) = &steps {
            super::steps::heading(ui, ctx, steps);
        }

        super::row(ui, &palette, "From", |ui| super::address(ui, &palette, &draft.from.to_string()));

        super::row(ui, &palette, "Fee, at most", |ui| {
            ui.label(ctx.amount(None, &most.to_string()))
                .on_hover_text(format!("{} gas at up to {} per gas; what it does not use is refunded", fee.gas_limit, ctx.amount(None, &fee.max_price.to_string())));
        });

        if fee.tip != u256::ZERO {
            super::row(ui, &palette, "Tip per gas", |ui| {
                ui.label(ctx.amount(None, &fee.tip.to_string()));
            });
        }

        if ctx.settings.read_blocks {
            fee_fields(ui, ctx, state);
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
            ui.colored_label(palette.bad, format!("A dry run on the node says this would fail: {}", crate::steps::plain(warning)));
            ui.label(RichText::new("Sent anyway, it uses its nonce and pays its fee, unless things change before it lands.").small().color(palette.muted));
        }

        // The transaction itself, before the button that signs it: every step of a batch, or the
        // lone one in its own frame, with its amount, recipient and data.
        ui.add_space(8.0);

        match (&steps, &one) {
            (Some(steps), _) => super::steps::each(ui, ctx, steps),
            (None, Some(one)) if one.steps[0].allowed => super::steps::single(ui, ctx, &one.steps[0]),
            _ => super::row(ui, &palette, "To", |ui| super::address(ui, &palette, &draft.to)),
        }

        // The collection a call goes to, for the offline signer to name it.
        let collection = named_collection(ctx, &draft.tx);
        let key = ctx.holders.iter().find_map(|holder| holder.key(&draft.from));
        let by_metamask = ctx.holders.iter().any(|holder| holder.by_metamask() && holder.addresses().contains(&draft.from));
        // It would have to sign one opaque hash for all the steps, which the chain refuses anyway.
        let metamask_refuses = key.is_none() && by_metamask && steps.is_some();

        if metamask_refuses {
            ui.add_space(6.0);
            ui.colored_label(palette.bad, crate::metamask::NO_BATCH);
        }

        ui.add_space(12.0);
        ui.horizontal_wrapped(|ui| {
            let send = match (key.is_some(), by_metamask, warning.is_some()) {
                (true, _, false) => "Sign and send",
                (true, _, true) => "Send anyway",
                (false, true, false) => "Sign with MetaMask",
                (false, true, true) => "Sign with MetaMask anyway",
                (false, false, false) => "Sign on the signer",
                (false, false, true) => "Sign on the signer anyway",
            };

            if theme::primary(ui, &palette, send, !short && !metamask_refuses && state.confirming.is_none()).clicked() || confirmed {
                match (key, genesis) {
                    (None, genesis) if by_metamask => {
                        let signed = Signed::default();
                        let (inbox, wake) = (signed.clone(), ctx.wake.clone());
                        let typed = crate::metamask::typed_data(&draft.tx, draft.nonce, &fee, &genesis);
                        crate::metamask::sign(ethereum(&draft.from), typed, move |answer| {
                            *inbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(answer);
                            wake.request_repaint();
                        });
                        state.step = Step::MetaMask { draft: draft.clone(), fee, genesis, signed };
                    }
                    // With biometrics on, a check first; signing once it passes.
                    (Some(_), _) if !confirmed && let Some((enrolled, _)) = biometrics::enrolled(ctx.wake, ctx.settings) => {
                        state.confirm_trouble = None;
                        state.confirming = Some(Check::start(ctx.wake, &enrolled, &format!("Send {}", draft.what)));
                    }
                    (Some(key), genesis) => {
                        let signed = SignedTx::sign(draft.tx.clone(), draft.nonce, fee, key, &genesis);
                        let ask = Ask::Submit(json!({ "tx": encode(&signed) }).to_string());
                        ctx.node.ask(ask.clone());
                        state.step = Step::Sending { draft: draft.clone(), ask };
                    }
                    (None, genesis) => {
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
                            collection: collection.clone(),
                            genesis,
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
        let said = match draft.tx {
            Tx::Batch { .. } => format!("{}: on the offline signer, open Sign and read this code. It shows every step before it signs.", draft.what),
            _ => format!("{} to {}: on the offline signer, open Sign and read this code.", draft.what, format::short(&draft.to)),
        };
        ui.label(RichText::new(said).color(palette.soft));
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

/// The fee's fields, with no node to ask: what checked blocks suggest, to change, and what that rests
/// on. Nothing tries the transaction first.
fn fee_fields(ui: &mut Ui, ctx: &Ctx, state: &mut SendState) {
    let palette = *ctx.palette;
    let Some((gas, price, suggested, seen)) = &mut state.fee_typed else {
        return;
    };

    ui.add_space(6.0);
    theme::caps(ui, &palette, "Gas limit");
    ui.add(egui::TextEdit::singleline(gas).desired_width(f32::INFINITY));
    theme::caps(ui, &palette, "Most per gas, in the coin's smallest units");
    ui.add(egui::TextEdit::singleline(price).desired_width(f32::INFINITY));

    let reads = gas.trim().parse::<u64>().is_ok() && u256::from_str_radix(price.trim(), 10).is_ok();

    if !reads {
        ui.colored_label(palette.bad, format!("Those do not read as whole numbers: offering what was suggested, {} gas at {}.", suggested.gas_limit, suggested.max_price));
    }

    let said = match seen {
        true => "Suggested from checked blocks: what the same kind of transaction used lately and a quarter more, and twice the newest checked base price. What it does not use is refunded.",
        false => "No checked block read so far shows what this kind of transaction uses, so the gas limit is a round guess, on the high side. What it does not use is refunded.",
    };

    ui.add(egui::Label::new(RichText::new(said).small().color(palette.muted)).wrap());
    ui.add(egui::Label::new(RichText::new("Read from checked blocks is on, so no node tries this first: a call that would fail still uses its nonce and pays its fee.").small().color(palette.warn)).wrap());
}

/// What became of a transaction just sent.
fn sent(ui: &mut Ui, ctx: &mut Ctx, state: &mut SendState, id: &str) {
    let palette = *ctx.palette;

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Sent");
        ui.add_space(4.0);
        let genesis = ctx.settings.chain_shown(ctx.chain.genesis);
        let (text, color) = match ctx.settings.checked_on(id, genesis.as_deref()) {
            Some(checked) if !checked.done => ("It landed, and failed: it used its nonce and paid its fee.".to_string(), palette.bad),
            Some(checked) if checked.final_ => (format!("Final at block {}.", checked.height), palette.good),
            Some(checked) if ctx.settings.read_blocks => (format!("Included in block {}, not final: the reading has not reached it yet.", checked.height), palette.accent),
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

    /// The Send page on a screen this wide, reviewing a batch from the first address of an account,
    /// and what it drew.
    fn reviewed(book: &mut crate::vault::Book, width: f32, steps: impl Fn(Address) -> Vec<Tx>) -> Vec<(String, egui::Rect)> {
        let mut holders = crate::keys::open(book).unwrap();
        let from = holders[0].addresses()[0];
        let signer = holders[0].public(&from).unwrap();
        let tx = Tx::Batch { steps: steps(from) };
        let draft = Draft { from, signer, to: "its steps".to_string(), tx, nonce: 2, token: None, what: "A batch of 2 steps".to_string(), coin: u256::ZERO };
        let fee = FeeOffer { gas_limit: 50_000, max_price: u256::new(2_000_000_000), tip: u256::ZERO };
        let mut state = SendState { step: Step::Review { draft, fee, warning: None }, ..SendState::default() };
        let (drawn, _, shown) = crate::views::drawn::following_shown(book, &mut holders, (width, 20_000.0), |ui, ctx| show(ui, ctx, &mut state));

        assert!(drawn <= width + 1.0, "{drawn} on {width}");

        shown
    }

    fn some_steps(from: Address) -> Vec<Tx> {
        let pay = |amount: u128| Tx::Transfer { from, to: Address::Ed25519([9; 32]), amount: Amount::U128(amount) };

        vec![pay(1_000_000_000_000_000_000), pay(2_000_000_000_000_000_000)]
    }

    #[test]
    fn the_review_shows_a_batchs_count_and_total_first_and_every_step_above_the_button_that_signs() {
        for (width, _) in crate::views::drawn::SCREENS {
            let shown = reviewed(&mut crate::views::drawn::two_phrases(), width, some_steps);
            let top = |said: &str| shown.iter().find(|(text, _)| text.contains(said)).map_or_else(|| panic!("{said} in {shown:?}"), |(_, rect)| rect.top());

            assert!(top("2 STEPS") < top("Sends 3 DEV in all"));
            assert!(top("Sends 3 DEV in all") < top("Fee, at most"), "the total before anything else");
            assert!(top("Fee, at most") < top("Step 1 of 2"));
            assert!(top("Step 1 of 2") < top("Step 2 of 2"));
            assert!(top("Step 2 of 2") < top("Sign and send"));
        }
    }

    #[test]
    fn a_wallet_with_no_chain_known_says_so_and_offers_nothing_to_sign_even_with_one_pinned() {
        use crate::views::drawn::{SCREENS, World, said_in, two_phrases};

        let mut book = two_phrases();
        let mut holders = crate::keys::open(&book).unwrap();
        let from = holders[0].addresses()[0];
        let signer = holders[0].public(&from).unwrap();
        let tx = Tx::Transfer { from, to: from, amount: Amount::U128(1) };
        let draft = Draft { from, signer, to: "to someone".to_string(), tx, nonce: 2, token: None, what: "1 DEV".to_string(), coin: u256::ONE };
        let fee = FeeOffer { gas_limit: 26_250, max_price: u256::new(2_000), tip: u256::ZERO };

        for size in SCREENS {
            for step in [Step::Edit, Step::Review { draft: draft.clone(), fee, warning: None }] {
                // A genesis pinned at the node is what it said last time, and says nothing of now.
                let mut world = World::default();
                world.settings.pinned.insert(world.settings.node.clone(), Hash::zero(HashType::default_chain()).to_string());
                let mut state = SendState { step, ..SendState::default() };
                let said = said_in(&mut world, &mut book, &mut holders, size, |ui, ctx| show(ui, ctx, &mut state));

                assert!(said.contains("cannot sign yet"), "{said}");
                assert!(!said.contains("Sign and send") && !said.contains("Review"), "{said}");
            }
        }

        // Nor is a draft made, whatever asks for one.
        let mut world = World::default();
        let (mut asked, state) = (None, SendState::default());
        crate::views::drawn::page_in(&mut world, &mut book, &mut holders, SCREENS[0], |_, ctx| {
            asked = Some(finish(ctx, &state, from, signer, Tx::Transfer { from, to: from, amount: Amount::U128(1) }, u256::ONE, None, String::new(), String::new(), 0).map(|_| ()));
        });

        assert_eq!(asked, Some(Err(NO_CHAIN.to_string())));
    }

    #[test]
    fn a_probe_and_what_goes_to_the_signer_are_for_the_chain_the_light_client_follows() {
        let mut book = crate::views::drawn::two_phrases();
        let mut holders = crate::keys::open(&book).unwrap();
        let from = holders[0].addresses()[0];
        let signer = holders[0].public(&from).unwrap();
        let state = SendState::default();
        let chain = Hash::zero(HashType::default_chain());
        let mut drafted = None;

        crate::views::drawn::following_shown(&mut book, &mut holders, crate::views::drawn::SCREENS[0], |_, ctx| {
            drafted = Some(finish(ctx, &state, from, signer, Tx::Transfer { from, to: from, amount: Amount::U128(1) }, u256::ONE, None, String::new(), String::new(), 0));
        });

        let (_, body) = drafted.unwrap().unwrap();
        let body: Value = serde_json::from_str(&body).unwrap();
        let probe: SignedTx = wincode::deserialize(&chainv4::structs::hex::from_hex(body["tx"].as_str().unwrap()).unwrap()).unwrap();

        assert_eq!(probe.verify_on(&chain), Ok(()));
    }

    /// The review of a plain transfer, in a world with this setting, and what it said.
    fn reviewing_a_transfer(reading: bool, size: (f32, f32)) -> (f32, String) {
        use crate::views::drawn::{World, page_in, said_in, two_phrases};

        let mut book = two_phrases();
        let mut holders = crate::keys::open(&book).unwrap();
        let from = holders[0].addresses()[0];
        let signer = holders[0].public(&from).unwrap();
        let tx = Tx::Transfer { from, to: from, amount: Amount::U128(1) };
        let draft = Draft { from, signer, to: "to someone".to_string(), tx, nonce: 2, token: None, what: "1 DEV".to_string(), coin: u256::ONE };
        let fee = FeeOffer { gas_limit: 26_250, max_price: u256::new(2_000), tip: u256::ZERO };
        let world = || {
            let mut world = World::default();
            world.settings.read_blocks = reading;

            world
        };
        let state = || SendState { fee_typed: reading.then(|| ("26250".to_string(), "2000".to_string(), fee, true)), ..SendState::default() };
        let (mut one, mut other) = (state(), state());
        let (width, _) = page_in(&mut world(), &mut book, &mut holders, size, |ui, ctx| review(ui, ctx, &mut one, draft.clone(), fee, None, Hash::zero(HashType::default_chain())));
        let said = said_in(&mut world(), &mut book, &mut holders, size, |ui, ctx| review(ui, ctx, &mut other, draft.clone(), fee, None, Hash::zero(HashType::default_chain())));

        (width, said)
    }

    #[test]
    fn reading_blocks_the_review_offers_what_they_suggest_to_change_and_says_nothing_tried_it() {
        for size in crate::views::drawn::SCREENS {
            let (width, said) = reviewing_a_transfer(true, size);

            assert!(width <= size.0, "{width} wide, on {size:?}");
            assert!(said.contains("GAS LIMIT") && said.contains("MOST PER GAS"), "{said}");
            assert!(said.contains("Suggested from checked blocks"), "{said}");
            assert!(said.contains("no node tries this first"), "{said}");
        }

        let (_, off) = reviewing_a_transfer(false, (1200.0, 800.0));
        assert!(!off.contains("GAS LIMIT") && !off.contains("checked blocks"), "with it off, as it always was: {off}");
    }

    #[test]
    fn what_is_typed_for_a_fee_is_the_offer_and_what_does_not_read_is_not() {
        let fee = FeeOffer { gas_limit: 100, max_price: u256::new(7), tip: u256::new(1) };
        let state = |gas: &str, price: &str| SendState { fee_typed: Some((gas.to_string(), price.to_string(), fee, true)), ..SendState::default() };

        assert_eq!(typed_offer(&state(" 500 ", "9"), fee), Some(FeeOffer { gas_limit: 500, max_price: u256::new(9), tip: u256::new(1) }));
        assert_eq!(typed_offer(&state("lots", "9"), fee), None);
        assert_eq!(typed_offer(&state("500", "-1"), fee), None);
        assert_eq!(typed_offer(&SendState::default(), fee), None);
    }

    #[test]
    fn an_account_metamask_holds_is_told_it_cannot_sign_a_batch_and_is_not_asked_to() {
        use crate::vault::{Book, Source, StoredAccount};

        let key = chainv4::crypto::keypair::Keypair::generate(chainv4::crypto::keypair::KeyType::Secp256k1).unwrap().public();
        let mut book = Book { accounts: vec![StoredAccount { name: "MetaMask".to_string(), source: Source::MetaMask { key: key.to_string() } }], ..Book::default() };
        let shown = reviewed(&mut book, 360.0, some_steps);

        assert!(shown.iter().any(|(text, _)| text.contains("MetaMask would show a batch as one opaque hash")), "{shown:?}");
        assert!(shown.iter().any(|(text, _)| text == "Sign with MetaMask"));
    }

    #[test]
    fn a_prepared_batch_goes_back_to_its_steps_and_starts_the_batch_page_afresh_once_sent() {
        let mut state = SendState::default();

        assert!(!state.take_batch_sent());

        let from = Address::Ed25519([2; 32]);
        let draft = Draft { from, signer: chainv4::crypto::keypair::Keypair::from_seed([7; 32]).public(), to: "its steps".to_string(), tx: Tx::Batch { steps: Vec::new() }, nonce: 2, token: None, what: "A batch".to_string(), coin: u256::ZERO };
        let ask = Ask::Submit("{}".to_string());
        state.step = Step::Sending { draft: draft.clone(), ask: ask.clone() };

        assert!(state.answered(&ask, &Ok(json!({ "id": "0x01" })), 5).is_some());
        assert!(state.take_batch_sent());
        assert!(!state.take_batch_sent(), "once");

        state.step = Step::Sending { draft: Draft { tx: Tx::PrimaryName { name: None }, ..draft }, ask: ask.clone() };
        state.answered(&ask, &Ok(json!({ "id": "0x02" })), 5);

        assert!(!state.take_batch_sent());
    }

    #[test]
    fn a_prepared_transaction_is_signed_by_the_address_it_was_prepared_for() {
        use chainv4::structs::address::Address;

        let mut state = SendState { from: Some(Address::Ed25519([1; 32])), ..SendState::default() };
        state.prepare(Prepared { from: Address::Ed25519([2; 32]), tx: Tx::ClearVote { version: 3, change: chainv4::governance::Change::Name("x".to_string()) }, what: "Clear a vote".to_string(), about: String::new(), coin: u256::ZERO });

        assert_eq!(state.from, Some(Address::Ed25519([2; 32])), "not whichever the form had picked");
    }

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

    /// The review of one call from the first address, on a chain whose registry the light client
    /// proved lists this collection, or nothing: what it said.
    fn call_reviewed(listed: Option<chainv4::registry::collections::CollectionKind>, data: Vec<TxCallArgumentType>, size: (f32, f32)) -> String {
        let mut book = crate::views::drawn::two_phrases();
        let mut holders = crate::keys::open(&book).unwrap();
        let from = holders[0].addresses()[0];
        let signer = holders[0].public(&from).unwrap();
        let collection = Address::Contract([4; 32]);
        let mut world = crate::views::drawn::following(&holders);

        if let Some(kind) = listed {
            let assurance = chainv4::light::Assurance { block: Hash::zero(HashType::default_chain()), height: 1, final_: true };
            let entry = chainv4::registry::collections::Collection { address: collection, kind, name: Some("Art".to_string()), symbol: None };
            world.chain.collections.insert(collection.to_string(), crate::chain::Proven { value: entry, assurance });
        }

        let tx = Tx::ContractCall { to: collection, func: Hasher::digest(HashType::Xxh3_64, b"transfer"), amount: Amount::U128(0), data };
        let draft = Draft { from, signer, to: collection.to_string(), tx, nonce: 2, token: None, what: "Send NFT #42".to_string(), coin: u256::ZERO };
        let fee = FeeOffer { gas_limit: 50_000, max_price: u256::new(2_000_000_000), tip: u256::ZERO };
        let mut state = SendState { step: Step::Review { draft, fee, warning: None }, ..SendState::default() };

        crate::views::drawn::said_in(&mut world, &mut book, &mut holders, size, |ui, ctx| show(ui, ctx, &mut state))
    }

    #[test]
    fn the_review_of_an_nft_s_transfer_names_the_nft_and_never_reads_its_id_as_an_amount_on_every_screen() {
        use chainv4::registry::collections::CollectionKind;

        let to = Address::Ed25519([9; 32]);

        for size in crate::views::drawn::SCREENS {
            let nft = call_reviewed(Some(CollectionKind::Nft), vec![TxCallArgumentType::Address(to), TxCallArgumentType::U256(u256::new(42))], (size.0, 4_000.0));

            assert!(nft.contains("Send NFT #42") && nft.lines().any(|line| line == "Art") && nft.lines().any(|line| line == "Id"), "{nft}");
            assert!(!nft.contains("units") && !nft.lines().any(|line| line == "Token" || line == "Amount"), "{nft}");

            let multi = call_reviewed(Some(CollectionKind::Multi), vec![TxCallArgumentType::Address(to), TxCallArgumentType::U256(u256::new(7)), TxCallArgumentType::U256(u256::new(3))], (size.0, 4_000.0));

            assert!(multi.contains("Send 3 of #7") && multi.contains("How many"), "{multi}");

            // Nothing proven of the contract: the call as it is, said to be of one the wallet does
            // not know, and no amount.
            let unknown = call_reviewed(None, vec![TxCallArgumentType::Address(to), TxCallArgumentType::U256(u256::new(42))], (size.0, 4_000.0));

            assert!(unknown.contains("Call transfer") && unknown.contains("neither a token nor a collection"), "{unknown}");
            assert!(!unknown.contains("units of the token"), "{unknown}");
        }
    }
}
