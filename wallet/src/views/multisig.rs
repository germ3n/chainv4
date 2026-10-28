//! Native multisigs: making one, following those this wallet cares about, and taking their
//! transactions round the members. A payment is proposed here and priced by the node; the bundle
//! it makes carries the members' signatures, added here for a key this wallet holds, or on the
//! offline signer from a code, or passed as text to members using chainv4 multisig. Approvals go on
//! chain as any other transaction, through Send, so a hardware or offline account approves too.
//! Once enough members back it, any key carries it to the node: the multisig pays the fee.
//!
//! What the node says of a multisig, its members, nonce and open proposals, is shown as the node
//! says it. Nothing signed rests on it: the bundle says what it does, and the chain checks the
//! quorum when it runs.

use {
    super::scan::Scan,
    super::send::{Prepared, SendState},
    super::{Ctx, Page},
    crate::airgap::Message,
    crate::format,
    crate::node::{Ask, Failure},
    crate::theme,
    chainv4::crypto::keypair::{KeyType, Keypair, PublicKey},
    chainv4::registry::multisig::{MultisigBody, MultisigBundle, MultisigMember},
    chainv4::structs::address::{Address, multisig_address},
    chainv4::structs::hex::{from_hex, to_hex},
    chainv4::structs::tx::{FeeOffer, MemberSignature, Tx},
    chainv4::structs::u256::u256,
    egui::{RichText, Ui},
    serde_json::{Value, json},
};

pub(crate) mod bitcoin;
#[cfg(feature = "swap")]
pub(crate) mod evm;

/// How often the followed multisig is asked of the node again, in milliseconds.
const REFRESH: u64 = 10_000;

#[derive(Default)]
pub struct MultisigState {
    /// The members to make a multisig of, one a line, its threshold, and its Bitcoin network.
    members: String,
    threshold: String,
    testnet: bool,
    /// A multisig to follow, as typed.
    adding: String,
    selected: Option<String>,
    /// The node's answer about the selected multisig, by its address, and when it was asked.
    shown: Option<(String, Value)>,
    asked_at: u64,
    /// A payment to propose from the selected multisig.
    to: String,
    amount: String,
    /// A payment the node is pricing: what was asked, the body it was asked for, and the chain.
    pricing: Option<(Ask, MultisigBody, chainv4::structs::hash::Hash)>,
    /// The bundle on its way round, and whether the offline signer is asked for its signature.
    bundle: Option<MultisigBundle>,
    asking_signer: bool,
    pasted: String,
    scan: Scan,
    submitting: Option<Ask>,
    note: Option<String>,
    trouble: Option<String>,
    /// Its coins on Bitcoin, for one whose members name xpubs.
    bitcoin: bitcoin::BitcoinState,
    /// Its contract on EVM chains, for one whose members name EVM addresses.
    #[cfg(feature = "swap")]
    evm: evm::EvmState,
    /// A transaction another page asked the multisig to send, waiting for its nonce to price.
    incoming: Option<(String, Tx)>,
}

impl MultisigState {
    /// Takes a transaction another page made for a followed multisig to send, such as a swap's
    /// lock: it is priced and becomes the bundle for its members to sign, once the node says the
    /// multisig's nonce.
    pub fn propose(&mut self, prepared: Prepared) {
        let account = prepared.from.to_string();
        self.selected = Some(account.clone());
        self.asked_at = 0;
        self.bundle = None;
        self.incoming = Some((account, prepared.tx));
    }

    /// Takes calls another page made for a followed multisig's contract on an EVM chain, such as a
    /// swap's approval and lock: each becomes a bundle for its owners in turn.
    #[cfg(feature = "swap")]
    pub fn propose_evm(&mut self, account: &str, chain_id: u64, calls: Vec<chainv4::swap::evm::Call>) {
        self.selected = Some(account.to_string());
        self.asked_at = 0;
        self.evm.queue(account, chain_id, calls);
    }

    /// Takes the node's answer to something this page asked; false when it asked none of it.
    pub fn answered(&mut self, ask: &Ask, result: &Result<Value, Failure>) -> bool {
        match ask {
            Ask::Multisig(address) => {
                match result {
                    Ok(value) => self.shown = Some((address.clone(), value.clone())),
                    Err(failure) if self.selected.as_deref() == Some(address.as_str()) => {
                        self.shown = None;
                        self.trouble = Some(format!("The node knows no multisig there: {}", failure.message));
                    }
                    Err(_) => {}
                }

                true
            }
            Ask::Offer(_) if self.pricing.as_ref().is_some_and(|(asked, _, _)| asked == ask) => {
                let (_, body, genesis) = self.pricing.take().expect("checked above");

                match result.as_ref().map_err(|failure| failure.message.clone()).and_then(fee_of) {
                    Ok((fee, warning)) => {
                        self.trouble = warning.map(|why| format!("A dry run says it would fail: {why}"));
                        self.bundle = Some(MultisigBundle { genesis, body: MultisigBody { fee, ..body }, signatures: Vec::new() });
                    }
                    Err(why) => self.trouble = Some(format!("The node could not price it: {why}")),
                }

                true
            }
            Ask::Submit(_) if self.submitting.as_ref() == Some(ask) => {
                self.submitting = None;
                self.asked_at = 0;

                match result {
                    Ok(value) => {
                        self.note = Some(format!("Submitted {}: the multisig pays its fee.", format::short(value["id"].as_str().unwrap_or("?"))));
                        self.bundle = None;
                    }
                    Err(failure) => self.trouble = Some(format!("The node refused it: {}", failure.message)),
                }

                true
            }
            _ => false,
        }
    }
}

pub fn show(ui: &mut Ui, ctx: &mut Ctx, state: &mut MultisigState, send: &mut SendState) {
    let palette = *ctx.palette;
    theme::title(ui, &palette, "Multisig", "Accounts that M of N members' keys control, here and, for those set up for it, on Bitcoin and EVM chains.");

    let Some(genesis) = ctx.chain.genesis else {
        theme::card(ui, &palette, |ui| {
            ui.label(RichText::new("Waiting for the light client to start.").color(palette.muted));
        });

        return;
    };
    let mut prepared = None;

    followed(ui, ctx, state);

    if let Some(selected) = state.selected.clone() {
        if ctx.now.saturating_sub(state.asked_at) > REFRESH {
            ctx.node.ask(Ask::Multisig(selected.clone()));
            state.asked_at = ctx.now;
        }

        if let Some((_, value)) = state.shown.as_ref().filter(|(address, _)| *address == selected) {
            let value = value.clone();

            // What another page asked: priced as the multisig's next transaction.
            if let Some((account, tx)) = state.incoming.take_if(|(account, _)| *account == selected)
                && let Ok(account) = account.parse::<Address>()
            {
                price(ctx, state, account, tx, &value, genesis);
            }
            details(ui, ctx, state, &value, &mut prepared);
            propose(ui, ctx, state, &selected, &value, genesis);

            if let Some(wallet) = bitcoin::wallet_of(&value) {
                bitcoin::show(ui, ctx, &mut state.bitcoin, &selected, &wallet);
            }

            #[cfg(feature = "swap")]
            if let Some((start, now)) = evm::side_of(&value) {
                evm::show(ui, ctx, &mut state.evm, &selected, &start, now.as_ref());
            }
        }
    }

    bundle(ui, ctx, state, genesis, &mut prepared);
    create(ui, ctx, state, &mut prepared);

    if let Some(prepared) = prepared {
        send.prepare(prepared);
        *ctx.page = Page::Send;
    }
}

/// The multisigs followed, one picked, and one to add.
fn followed(ui: &mut Ui, ctx: &mut Ctx, state: &mut MultisigState) {
    let palette = *ctx.palette;

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Multisigs");

        if state.selected.as_ref().is_none_or(|selected| !ctx.settings.multisigs.contains(selected)) {
            state.selected = ctx.settings.multisigs.first().cloned();
        }

        match &state.selected {
            Some(selected) => {
                egui::ComboBox::from_id_salt("multisig").selected_text(format::short(selected)).width(ui.available_width()).show_ui(ui, |ui| {
                    for address in ctx.settings.multisigs.clone() {
                        if ui.selectable_label(state.selected.as_deref() == Some(&address), format::short(&address)).clicked() {
                            state.selected = Some(address);
                            state.asked_at = 0;
                        }
                    }
                });
            }
            None => {
                ui.label(RichText::new("None followed yet: make one below, or add one a member gave you.").color(palette.muted));
            }
        }

        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            ui.add(egui::TextEdit::singleline(&mut state.adding).hint_text("multisig:0x…").font(egui::TextStyle::Monospace).desired_width(ui.available_width() - 70.0));

            if theme::quiet(ui, &palette, "Follow").clicked() {
                match state.adding.trim().parse::<Address>() {
                    Ok(address @ Address::Multisig(_)) => {
                        follow(ctx, &address.to_string());
                        state.selected = Some(address.to_string());
                        state.asked_at = 0;
                        state.adding.clear();
                    }
                    _ => state.trouble = Some("A multisig's address is multisig:0x and 64 hex digits.".to_string()),
                }
            }
        });
    });
}

fn follow(ctx: &mut Ctx, address: &str) {
    if !ctx.settings.multisigs.iter().any(|known| known == address) {
        ctx.settings.multisigs.push(address.to_string());
    }
}

/// Which of the account's addresses is a member's key, of those the node lists.
fn member_address(ctx: &Ctx, members: &[Value]) -> Option<Address> {
    let holder = ctx.holder()?;

    holder.addresses().into_iter().find(|address| {
        holder.public(address).is_some_and(|key| members.iter().any(|member| member["key"].as_str() == Some(key.to_string().as_str())))
    })
}

/// The selected multisig: its members, balance and next nonce, and the proposals open for it.
fn details(ui: &mut Ui, ctx: &mut Ctx, state: &mut MultisigState, value: &Value, prepared: &mut Option<Prepared>) {
    let palette = *ctx.palette;
    let members = value["members"].as_array().cloned().unwrap_or_default();
    let threshold = value["threshold"].as_u64().unwrap_or(0);
    let nonce = value["nonce"].as_u64().unwrap_or(0);
    let account: Option<Address> = value["address"].as_str().and_then(|text| text.parse().ok());
    let mine = member_address(ctx, &members);
    let my_key = mine.and_then(|address| ctx.holder().and_then(|holder| holder.public(&address)));

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, &format!("{threshold} of {} members, as the node says", members.len()));
        super::row(ui, &palette, "Balance", |ui| {
            ui.label(ctx.amount(None, value["balance"].as_str().unwrap_or("0")));
        });
        super::row(ui, &palette, "Next nonce", |ui| {
            ui.label(nonce.to_string());
        });

        for member in &members {
            let key = member["key"].as_str().unwrap_or("?");
            let you = my_key.is_some_and(|mine| mine.to_string() == key);
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(format::short(key)).monospace().color(palette.ink));

                if you {
                    ui.label(RichText::new("this account").small().color(palette.good));
                }

                if member["bitcoin"].is_string() {
                    ui.label(RichText::new("Bitcoin · EVM").small().color(palette.muted));
                }
            });
        }

        let Some(account) = account else {
            return;
        };

        for proposal in value["proposals"].as_array().cloned().unwrap_or_default() {
            let Some(tx) = proposal["encoded"].as_str().and_then(from_hex).and_then(|bytes| wincode::deserialize::<Tx>(&bytes).ok()) else {
                continue;
            };
            let Some(fee) = fee_of(&proposal["fee"]).ok().map(|(fee, _)| fee) else {
                continue;
            };
            let approvals: Vec<String> = proposal["approvals"].as_array().into_iter().flatten().filter_map(|key| key.as_str().map(str::to_string)).collect();
            let approved = my_key.is_some_and(|key| approvals.contains(&key.to_string()));

            ui.add_space(8.0);
            ui.separator();
            ui.label(RichText::new(describe(ctx, &tx)).color(palette.ink));
            ui.label(RichText::new(format!("approved by {} of the {threshold} it needs", approvals.len())).small().color(palette.soft));
            ui.horizontal_wrapped(|ui| {
                if let Some(from) = mine {
                    if !approved && theme::quiet(ui, &palette, "Approve").clicked() {
                        *prepared = Some(approval(from, account, nonce, fee, tx.clone()));
                    }

                    if approved && theme::quiet(ui, &palette, "Withdraw").clicked() {
                        let id = MultisigBody { account, tx: tx.clone(), nonce, fee }.id(&ctx.chain.genesis.expect("shown only with a genesis"));
                        *prepared = Some(Prepared {
                            from,
                            tx: Tx::WithdrawMultisig { account, proposal: id },
                            what: "Withdraw an approval".to_string(),
                            about: format::short(&account.to_string()),
                            coin: u256::ZERO,
                        });
                    }
                }

                if approvals.len() as u64 >= threshold && state.submitting.is_none() && theme::primary(ui, &palette, "Submit", true).clicked() {
                    let bundle = MultisigBundle { genesis: ctx.chain.genesis.expect("shown only with a genesis"), body: MultisigBody { account, tx: tx.clone(), nonce, fee }, signatures: Vec::new() };
                    submit(ctx, state, &bundle);
                }
            });
        }
    });
}

/// Proposing a payment of the coin from the multisig, priced by the node.
fn propose(ui: &mut Ui, ctx: &mut Ctx, state: &mut MultisigState, selected: &str, value: &Value, genesis: chainv4::structs::hash::Hash) {
    let palette = *ctx.palette;
    let Ok(account) = selected.parse::<Address>() else {
        return;
    };

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Propose a payment");
        ui.add(egui::TextEdit::singleline(&mut state.to).hint_text("To: ed25519:0x…, 0x…, bc1q…").font(egui::TextStyle::Monospace).desired_width(f32::INFINITY));
        ui.add(egui::TextEdit::singleline(&mut state.amount).hint_text(format!("Amount in {}", ctx.coin.symbol)).desired_width(f32::INFINITY));
        let busy = state.pricing.is_some();

        if theme::primary(ui, &palette, if busy { "Pricing…" } else { "Propose" }, !busy).clicked() {
            let to = super::send::recipient(state.to.trim());
            let amount = format::parse_units(state.amount.trim(), ctx.coin.decimals).and_then(|raw| u256::from_str_radix(&raw, 10).ok());

            match (to, amount) {
                (Some(to), Some(amount)) if amount > u256::ZERO => {
                    price(ctx, state, account, Tx::Transfer { from: account, to, amount: chainv4::structs::amount::Amount::U256(amount) }, value, genesis);
                }
                _ => state.trouble = Some("A payment needs an address to pay and an amount above zero.".to_string()),
            }
        }
    });
}

/// Asks the node to price a transaction as the multisig's next, the bundle made once it answers.
fn price(ctx: &mut Ctx, state: &mut MultisigState, account: Address, tx: Tx, value: &Value, genesis: chainv4::structs::hash::Hash) {
    let nonce = value["nonce"].as_u64().unwrap_or(0);
    let threshold = value["threshold"].as_u64().unwrap_or(1) as usize;
    let body = MultisigBody { account, tx, nonce, fee: FeeOffer::NONE };
    let ask = Ask::Offer(json!({ "tx": to_hex(&wincode::serialize(&sized(&body, threshold)).expect("a probe always encodes")) }).to_string());
    ctx.node.ask(ask.clone());
    state.pricing = Some((ask, body, genesis));
    state.trouble = None;
}

/// A probe of the multisig's transaction, carrying as many signatures as its quorum will, so it
/// costs what the real one will: the node prices it unsigned.
fn sized(body: &MultisigBody, threshold: usize) -> chainv4::structs::tx::SignedTx {
    let key = Keypair::from_seed([1; 32]);
    let signature = MemberSignature::sign(body, &key, &chainv4::structs::hash::Hash::zero(chainv4::structs::hash::HashType::default_chain()));
    let tx = Tx::Multisig { account: body.account, tx: Box::new(body.tx.clone()), signatures: vec![signature; threshold.max(1)] };

    chainv4::structs::tx::SignedTx::sign(tx, body.nonce, FeeOffer::NONE, &key)
}

/// The bundle on its way round: what it does, who signed, and signing, approving or submitting it;
/// or, with none, reading one.
fn bundle(ui: &mut Ui, ctx: &mut Ctx, state: &mut MultisigState, genesis: chainv4::structs::hash::Hash, prepared: &mut Option<Prepared>) {
    let palette = *ctx.palette;

    let Some(mut held) = state.bundle.clone() else {
        theme::card(ui, &palette, |ui| {
            theme::caps(ui, &palette, "A bundle from another member");
            ui.label(RichText::new("Read a member's code, or paste the text chainv4 multisig wrote.").color(palette.soft));
            ui.add(egui::TextEdit::multiline(&mut state.pasted).hint_text("chainv4-multisig:0x…").font(egui::TextStyle::Monospace).desired_rows(2).desired_width(f32::INFINITY));

            if !state.pasted.trim().is_empty() {
                match MultisigBundle::from_text(&state.pasted) {
                    Some(read) => {
                        take(state, read, genesis);
                        state.pasted.clear();
                    }
                    None => {
                        ui.colored_label(palette.bad, "Not a bundle: it starts chainv4-multisig:");
                    }
                }
            }

            match state.scan.show(ui, &palette) {
                Some(Message::Multisig(read)) => take(state, *read, genesis),
                Some(other) => state.scan.refuse(&other, "a multisig's transaction"),
                None => {}
            }

            notes(ui, &palette, state);
        });

        return;
    };
    let members = state.shown.as_ref().filter(|(address, _)| *address == held.body.account.to_string()).and_then(|(_, value)| value["members"].as_array().cloned());
    let threshold = state.shown.as_ref().filter(|(address, _)| *address == held.body.account.to_string()).and_then(|(_, value)| value["threshold"].as_u64());
    let mine = members.as_deref().and_then(|members| member_address(ctx, members));

    // Follow the bundle's multisig, so its members and approvals are known.
    if state.selected.as_deref() != Some(held.body.account.to_string().as_str()) {
        follow(ctx, &held.body.account.to_string());
        state.selected = Some(held.body.account.to_string());
        state.asked_at = 0;
    }

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "On its way round the members");
        ui.label(RichText::new(describe(ctx, &held.body.tx)).color(palette.ink).size(16.0));
        super::row(ui, &palette, "From", |ui| super::address(ui, &palette, &held.body.account.to_string()));
        super::row(ui, &palette, "Nonce", |ui| {
            ui.label(held.body.nonce.to_string());
        });
        super::row(ui, &palette, "Fee, at most", |ui| {
            ui.label(ctx.amount(None, &(u256::from(held.body.fee.gas_limit) * held.body.fee.max_price).to_string()));
        });
        super::row(ui, &palette, "Signed by", |ui| {
            let keys: Vec<String> = held.signatures.iter().map(|signed| format::short(&signed.key.to_string())).collect();
            ui.label(if keys.is_empty() { "nobody yet".to_string() } else { keys.join(", ") });
        });

        let approvals = approvals_of(state, &held);
        let backed = held.signatures.len() + approvals.iter().filter(|key| !held.signatures.iter().any(|signed| signed.key.to_string() == **key)).count();
        let needed = threshold.unwrap_or(u64::MAX);
        ui.label(RichText::new(match threshold {
            Some(needed) => format!("{backed} of the {needed} it needs back it, signatures and approvals on chain together"),
            None => "Asking the node about the multisig…".to_string(),
        }).small().color(palette.soft));
        ui.add_space(10.0);

        if state.asking_signer {
            ui.label(RichText::new("On the offline signer, open Sign and read this code; then read its code back.").color(palette.soft));
            super::code(ui, &palette, &Message::Multisig(Box::new(held.clone())));

            match state.scan.show(ui, &palette) {
                Some(Message::Multisig(signed)) if signed.body == held.body => {
                    for signature in signed.signatures {
                        if !held.signatures.iter().any(|known| known.key == signature.key) {
                            held.signatures.push(signature);
                        }
                    }
                    state.asking_signer = false;
                }
                Some(Message::Multisig(_)) => state.scan.fail("A bundle for another transaction: read the one the signer signed for this.".to_string()),
                Some(other) => state.scan.refuse(&other, "a multisig's transaction"),
                None => {}
            }
        }

        ui.horizontal_wrapped(|ui| {
            if let Some(from) = mine {
                let holder = ctx.holder().expect("a member's address is the account's");
                let signed_already = holder.public(&from).is_some_and(|key| held.signatures.iter().any(|signed| signed.key == key));

                let keypair = holder.key(&from);

                if !signed_already
                    && let Some(keypair) = keypair
                    && theme::primary(ui, &palette, "Sign", true).clicked()
                {
                    held.sign(keypair);
                }

                if !signed_already && keypair.is_none() && !state.asking_signer && theme::primary(ui, &palette, "Ask the signer", true).clicked() {
                    state.asking_signer = true;
                }

                if theme::quiet(ui, &palette, "Approve on chain").clicked() {
                    *prepared = Some(approval(from, held.body.account, held.body.nonce, held.body.fee, held.body.tx.clone()));
                }
            }

            if backed as u64 >= needed && state.submitting.is_none() && theme::primary(ui, &palette, "Submit", true).clicked() {
                submit(ctx, state, &held);
            }

            if theme::quiet(ui, &palette, "Copy as text").clicked() {
                ui.ctx().copy_text(held.to_text());
                state.note = Some("Copied: send it to the other members, for chainv4 multisig sign or this page.".to_string());
            }

            if theme::quiet(ui, &palette, "Put away").clicked() {
                state.bundle = None;
                state.asking_signer = false;
            }
        });

        if mine.is_none() && members.is_some() {
            ui.label(RichText::new("This account is not one of its members: switch to one that is to sign or approve.").small().color(palette.muted));
        }

        notes(ui, &palette, state);
    });

    if state.bundle.is_some() {
        state.bundle = Some(held);
    }
}

/// The members that approved a bundle's body on chain, as the node lists them.
fn approvals_of(state: &MultisigState, bundle: &MultisigBundle) -> Vec<String> {
    let id = bundle.body.id(&bundle.genesis).to_string();
    let Some((_, value)) = state.shown.as_ref().filter(|(address, _)| *address == bundle.body.account.to_string()) else {
        return Vec::new();
    };

    value["proposals"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|proposal| proposal["id"].as_str() == Some(id.as_str()))
        .flat_map(|proposal| proposal["approvals"].as_array().cloned().unwrap_or_default())
        .filter_map(|key| key.as_str().map(str::to_string))
        .collect()
}

/// Takes a bundle read or pasted, for this chain only.
fn take(state: &mut MultisigState, read: MultisigBundle, genesis: chainv4::structs::hash::Hash) {
    match read.genesis == genesis {
        true => {
            state.bundle = Some(read);
            state.trouble = None;
        }
        false => state.trouble = Some("A bundle for another chain: its signatures hold there only.".to_string()),
    }
}

/// Carries a bundle to the node, under a key made for the moment: it pays nothing, the multisig does.
fn submit(ctx: &mut Ctx, state: &mut MultisigState, bundle: &MultisigBundle) {
    let carrier = Keypair::generate(KeyType::Ed25519).unwrap_or_else(|_| Keypair::from_seed([7; 32]));
    let signed = bundle.envelope(&carrier);
    let ask = Ask::Submit(json!({ "tx": to_hex(&wincode::serialize(&signed).expect("a signed transaction always encodes")) }).to_string());
    ctx.node.ask(ask.clone());
    state.submitting = Some(ask);
}

/// A member's approval of a multisig's transaction, for Send to sign however the account signs.
fn approval(from: Address, account: Address, nonce: u64, fee: FeeOffer, tx: Tx) -> Prepared {
    Prepared {
        from,
        tx: Tx::ApproveMultisig { account, nonce, fee, tx: Box::new(tx) },
        what: format!("Approve the multisig's transaction {nonce}"),
        about: format::short(&account.to_string()),
        coin: u256::ZERO,
    }
}

/// Making a multisig: its members, one a line, and its threshold.
fn create(ui: &mut Ui, ctx: &mut Ctx, state: &mut MultisigState, prepared: &mut Option<Prepared>) {
    let palette = *ctx.palette;

    egui::CollapsingHeader::new(RichText::new("Make a multisig").color(palette.soft)).id_salt("make-multisig").show(ui, |ui| {
        theme::card(ui, &palette, |ui| {
            ui.label(RichText::new("One member a line: a key here, and for a multisig that reaches Bitcoin and EVM chains its Bitcoin multisig xpub, with its origin, and EVM address, after commas, every member or none.").small().color(palette.muted));
            ui.add(egui::TextEdit::multiline(&mut state.members).hint_text("ed25519:0x…,[73c5da0a/48h/0h/0h/2h]xpub…,0x…").font(egui::TextStyle::Monospace).desired_rows(3).desired_width(f32::INFINITY));
            ui.checkbox(&mut state.testnet, "On Bitcoin's test networks, not Bitcoin");

            if theme::quiet(ui, &palette, "Add this account").clicked()
                && let Some(line) = ctx.holder().and_then(|holder| member_line(holder, crate::keys::seed(ctx.book).as_ref(), state.testnet))
            {
                if !state.members.is_empty() && !state.members.ends_with('\n') {
                    state.members.push('\n');
                }
                state.members.push_str(&line);
            }

            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Members needed").color(palette.soft));
                ui.add(egui::TextEdit::singleline(&mut state.threshold).hint_text("2").desired_width(48.0));
            });

            if theme::primary(ui, &palette, "Make it", true).clicked() {
                match made(ctx, state) {
                    Ok(made) => *prepared = Some(made),
                    Err(why) => state.trouble = Some(why),
                }
            }
        });
    });
}

/// The line Add this account adds: its first key, and, for an account its words make, its Bitcoin
/// multisig xpub on the network picked and its EVM address.
pub(super) fn member_line(holder: &crate::keys::Holder, seed: Option<&chainv4::crypto::mnemonic::Seed>, testnet: bool) -> Option<String> {
    let publics: Vec<PublicKey> = holder.addresses().iter().filter_map(|address| holder.public(address)).collect();
    let key = publics.iter().find(|key| !matches!(key, PublicKey::Bitcoin(_))).or(publics.first())?;
    let evm = publics.iter().find_map(|key| match key.address() {
        Address::Secp256k1(address) => Some(to_hex(&address)),
        _ => None,
    });
    let network = network(testnet);
    let xpub = seed.filter(|_| holder.number().is_some()).map(|seed| chainv4::crypto::bitcoin::multisig::xpub_text(&seed.multisig_account(network.coin()), network));

    Some(match (xpub, evm) {
        (Some(xpub), Some(evm)) => format!("{key},{xpub},{evm}"),
        (None, Some(evm)) => format!("{key},,{evm}"),
        _ => key.to_string(),
    })
}

fn network(testnet: bool) -> chainv4::crypto::bitcoin::tx::Network {
    match testnet {
        true => chainv4::crypto::bitcoin::tx::Network::Test,
        false => chainv4::crypto::bitcoin::tx::Network::Main,
    }
}

/// The transaction making the multisig, from the account's first address, and the address it will
/// be at, which the page follows at once.
fn made(ctx: &mut Ctx, state: &MultisigState) -> Result<Prepared, String> {
    let read = state.members.lines().map(str::trim).filter(|line| !line.is_empty()).map(member).collect::<Result<Vec<_>, _>>()?;
    let threshold: u32 = state.threshold.trim().parse().map_err(|_| "Members needed is a whole number.".to_string())?;

    if read.iter().any(|(_, testnet)| testnet.is_some_and(|testnet| testnet != state.testnet)) {
        return Err(format!("A member's xpub is for {}: tick the test networks box to match, or take the other xpub.", if state.testnet { "Bitcoin" } else { "Bitcoin's test networks" }));
    }

    let members = read.into_iter().map(|(member, _)| member).collect();
    let policy = chainv4::registry::multisig::MultisigPolicy { members, threshold, testnet: state.testnet };

    if let Some(problem) = policy.problem() {
        return Err(format!("Not a multisig: {problem}."));
    }

    let from = ctx.holder().and_then(|holder| holder.addresses().into_iter().find(|address| crate::keys::scheme_of(address) != Some(KeyType::Bitcoin))).ok_or("There is no account to make it from.")?;
    let nonce = ctx.chain.account(&from.to_string()).map(|proven| proven.value.nonce).ok_or("The account is not proven yet: wait for the light client.")?;
    let genesis = ctx.chain.genesis.ok_or("Waiting for the light client to start.")?;
    let address = multisig_address(&from, nonce, genesis.algorithm());
    follow(ctx, &address.to_string());

    Ok(Prepared {
        from,
        tx: Tx::CreateMultisig { members: policy.members, threshold, testnet: policy.testnet },
        what: format!("Make a multisig, {threshold} of {}", state.members.lines().filter(|line| !line.trim().is_empty()).count()),
        about: format::short(&address.to_string()),
        coin: u256::ZERO,
    })
}

/// A member as a line gives it, KEY, KEY,,EVM_ADDRESS or KEY,[origin]xpub…,EVM_ADDRESS, and whether its xpub is for
/// Bitcoin's test networks.
fn member(line: &str) -> Result<(MultisigMember, Option<bool>), String> {
    let parts: Vec<&str> = line.split(',').map(str::trim).collect();
    let key: PublicKey = parts[0].parse().map_err(|_| format!("{} is not a key", format::short(parts[0])))?;

    let evm = |evm: &str| from_hex(evm).and_then(|bytes| bytes.try_into().ok()).ok_or_else(|| format!("{} is not an EVM address", format::short(evm)));

    match parts[1..] {
        [] => Ok((MultisigMember { key, bitcoin: None, evm: None }, None)),
        ["", address] => Ok((MultisigMember { key, bitcoin: None, evm: Some(evm(address)?) }, None)),
        [xpub, address] => {
            let (bitcoin, network) = chainv4::crypto::bitcoin::multisig::parse_xpub(xpub).map_err(|why| format!("{}: {why}", format::short(xpub)))?;

            Ok((MultisigMember { key, bitcoin: Some(bitcoin), evm: Some(evm(address)?) }, Some(network == chainv4::crypto::bitcoin::tx::Network::Test)))
        }
        _ => Err("A member is a key; a key, two commas and an EVM address; or a key, a Bitcoin xpub with its origin and an EVM address, with commas between.".to_string()),
    }
}

/// What a multisig's transaction does, in a line.
fn describe(ctx: &Ctx, tx: &Tx) -> String {
    match tx {
        Tx::Transfer { to, amount, .. } => format!("Pay {} to {}", ctx.amount(None, &amount.to_u256().to_string()), format::short(&to.to_string())),
        Tx::ChangeMultisig { members, threshold, .. } => format!("Make it {threshold} of {} members", members.len()),
        other => crate::history::action(other, &chainv4::structs::receipt::CommittedOutcome::Done),
    }
}

fn notes(ui: &mut Ui, palette: &crate::theme::Palette, state: &mut MultisigState) {
    if let Some(note) = &state.note {
        ui.label(RichText::new(note).small().color(palette.good));
    }

    if let Some(trouble) = &state.trouble {
        ui.colored_label(palette.bad, trouble);
    }
}

/// A fee offer as POST /offer answers it, or as the node lists a proposal's, and why its dry run
/// says it would fail, if it does.
fn fee_of(value: &Value) -> Result<(FeeOffer, Option<String>), String> {
    let wide = |name: &str| value[name].as_str().and_then(|text| u256::from_str_radix(text, 10).ok()).ok_or_else(|| format!("no {name}"));
    let fee = FeeOffer { gas_limit: value["gas_limit"].as_u64().ok_or("no gas_limit")?, max_price: wide("max_price")?, tip: wide("tip")? };
    let warning = (value["estimate"]["status"] == "failed").then(|| value["estimate"]["error"].as_str().unwrap_or("no reason given").to_string());

    Ok((fee, warning))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn members_read_as_a_line_gives_them_and_bad_ones_say_why() {
        let key = Keypair::from_seed([3; 32]).public();
        let seed = chainv4::crypto::mnemonic::Phrase::parse("abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about").unwrap().seed("");
        let xpub = chainv4::crypto::bitcoin::multisig::xpub_text(&seed.multisig_account(1), network(true));
        let evm = format!("0x{}", "22".repeat(20));

        assert_eq!(member(&key.to_string()), Ok((MultisigMember { key, bitcoin: None, evm: None }, None)));
        assert_eq!(
            member(&format!("{key},{xpub},{evm}")).map(|(member, testnet)| (member.bitcoin, member.evm, testnet)),
            Ok((Some(seed.multisig_account(1)), Some([0x22; 20]), Some(true)))
        );
        assert_eq!(member(&format!("{key},,{evm}")), Ok((MultisigMember { key, bitcoin: None, evm: Some([0x22; 20]) }, None)), "on EVM chains alone");
        assert!(member("not a key").is_err());
        assert!(member(&format!("{key},{xpub}")).unwrap_err().contains("commas"));
        assert!(member(&format!("{key},xpub123,{evm}")).is_err(), "an xpub without its origin");
    }

    #[test]
    fn a_priced_probe_carries_the_quorum_s_signatures_and_the_body_s_nonce() {
        let account = Address::Multisig([4; 32]);
        let tx = Tx::Transfer { from: account, to: Address::Ed25519([9; 32]), amount: chainv4::structs::amount::Amount::U128(5) };
        let body = MultisigBody { account, tx, nonce: 7, fee: FeeOffer::NONE };
        let probe = sized(&body, 3);

        assert_eq!((probe.sender(), probe.nonce), (account, 7));
        assert!(matches!(&probe.tx, Tx::Multisig { signatures, .. } if signatures.len() == 3));
    }

    #[test]
    fn a_fee_reads_from_an_offer_or_a_proposal() {
        let offer = json!({ "gas_limit": 30_000, "max_price": "20", "tip": "1", "estimate": { "status": "failed", "error": "out of fuel" } });
        let (fee, warning) = fee_of(&offer).unwrap();

        assert_eq!((fee.gas_limit, fee.max_price, fee.tip), (30_000, u256::new(20), u256::new(1)));
        assert_eq!(warning.as_deref(), Some("out of fuel"));
        assert!(fee_of(&json!({ "gas_limit": 1 })).is_err());
    }
}
