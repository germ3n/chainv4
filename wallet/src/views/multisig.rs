//! Native multisigs: making one in plain steps (create.rs), listing every one this wallet's accounts
//! are members of, as the node's member index says, beside those followed by address, changing
//! their members, and taking their transactions round the members. A multisig asks each member for a
//! key on this chain, of any scheme the chain has, and nothing else; a Bitcoin account key and an EVM
//! address are for one that is to hold coins there too. A payment is proposed here, or anything the
//! Batch page makes, with its form, and priced by the node; the bundle it makes carries the members'
//! signatures, added here for any key this wallet holds, picked one at a time, or on the offline
//! signer from a code, or passed as text to members using chainv4 multisig. Approvals go on chain as
//! any other transaction, through Send, as the account picked, so a hardware or offline account
//! approves too. Once enough members back it, any key carries it to the node: the multisig pays the
//! fee.
//!
//! What the node says of a multisig, its members, nonce and open proposals, and which multisigs an
//! account is in, is shown as the node says it. Nothing signed rests on it: the bundle says what it
//! does, and the chain checks the quorum when it runs.

use {
    super::batch::{self, BatchState},
    super::scan::Scan,
    super::send::{Prepared, SendState},
    super::{Ctx, Page},
    crate::airgap::Message,
    crate::format,
    crate::keys::Holder,
    crate::node::{Ask, Failure},
    crate::theme,
    chainv4::crypto::keypair::{KeyType, Keypair, PublicKey},
    chainv4::registry::multisig::{MultisigBody, MultisigBundle},
    chainv4::structs::address::Address,
    chainv4::structs::hex::{from_hex, to_hex},
    chainv4::structs::tx::{FeeOffer, SignedTx, Tx},
    chainv4::structs::u256::u256,
    egui::{RichText, Ui},
    serde_json::{Value, json},
    std::collections::BTreeMap,
};

pub(crate) mod bitcoin;
mod create;
#[cfg(feature = "swap")]
pub(crate) mod evm;

/// How often the followed multisig is asked of the node again, in milliseconds.
const REFRESH: u64 = 10_000;

/// How often the node is asked again which multisigs this wallet's accounts are in, in milliseconds.
const LISTED_EVERY: u64 = 60_000;

/// What to propose: a payment, the quick one, or anything the Batch page makes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Proposing {
    #[default]
    Payment,
    Steps,
}

/// Who signs a bundle and approves on chain: one of this wallet's addresses whose key is a member's,
/// or an offline signer this wallet holds no account of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Signer {
    Account(Address),
    Offline,
}

/// One of this wallet's keys among a multisig's members.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mine {
    /// Its account, by its place among the holders.
    pub holder: usize,
    pub address: Address,
    pub key: PublicKey,
    /// What it goes by: the account's name and the key's scheme.
    pub name: String,
    /// Whether this wallet holds the key, to sign with here; else the offline signer, or MetaMask,
    /// holds it.
    pub here: bool,
    /// Whether MetaMask holds it, which signs no bundle, only an approval sent from Send.
    pub by_metamask: bool,
}

/// A multisig this wallet knows of: its address, which of this wallet's accounts the node says are
/// its members, and whether it is followed by its address too.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Known {
    pub address: String,
    pub members: Vec<String>,
    pub followed: bool,
}

#[derive(Default)]
pub struct MultisigState {
    /// The steps of making a multisig or changing its members, while they are open.
    wizard: Option<create::Wizard>,
    /// A multisig to follow, as typed.
    adding: String,
    selected: Option<String>,
    /// The node's answer about the selected multisig, by its address, and when it was asked.
    shown: Option<(String, Value)>,
    asked_at: u64,
    /// The multisigs the node's member index lists for each of this wallet's addresses, by the
    /// address; when they were asked; the pages still to ask, by address and cursor; and why the
    /// node lists none, once it said it keeps no index.
    memberships: BTreeMap<String, Vec<String>>,
    listed_at: u64,
    more: Vec<(String, String)>,
    no_index: Option<String>,
    /// What is being proposed from the selected multisig: a payment, or steps.
    proposing: Proposing,
    to: String,
    amount: String,
    steps: BatchState,
    /// A transaction the node is pricing: what was asked, the body it was asked for, the chain, and
    /// whether it came from the steps, which go once it is a bundle.
    pricing: Option<(Ask, MultisigBody, chainv4::structs::hash::Hash, bool)>,
    /// The bundle on its way round, and whether the offline signer is asked for its signature.
    bundle: Option<MultisigBundle>,
    asking_signer: bool,
    /// Who signs and approves, as picked; None for whoever is next by default.
    signer: Option<Signer>,
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
    /// lock or a governance approval: it is priced and becomes the bundle for its members to sign,
    /// once the node says the multisig's nonce.
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

    /// Asks the node which multisigs each of this wallet's addresses is a member of, now and then,
    /// and the pages of an answer still to come.
    pub fn ask_memberships(&mut self, ctx: &mut Ctx) {
        for (address, cursor) in self.more.drain(..) {
            ctx.node.ask(Ask::Memberships { address, cursor: Some(cursor) });
        }

        if self.listed_at != 0 && ctx.now.saturating_sub(self.listed_at) < ctx.settings.pace().every_ms(LISTED_EVERY) {
            return;
        }

        self.listed_at = ctx.now.max(1);

        for address in names_by_address(ctx.holders).into_keys() {
            ctx.node.ask(Ask::Memberships { address, cursor: None });
        }
    }

    /// Every multisig this wallet knows of: those the node lists any of its accounts in, and those
    /// it follows by address.
    pub fn known(&self, ctx: &Ctx) -> Vec<Known> {
        known(&self.memberships, &names_by_address(ctx.holders), &ctx.settings.multisigs)
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
            Ask::Memberships { address, cursor } => {
                match result {
                    Ok(value) => {
                        let page = value["multisigs"].as_array().into_iter().flatten().filter_map(|multisig| multisig["address"].as_str().map(str::to_string));
                        let listed = self.memberships.entry(address.clone()).or_default();

                        // A first page starts the listing over; each after it adds on.
                        if cursor.is_none() {
                            listed.clear();
                        }

                        listed.extend(page);
                        self.no_index = None;

                        if let Some(next) = value["next"].as_str() {
                            self.more.push((address.clone(), next.to_string()));
                        }
                    }
                    Err(failure) if failure.status == 404 => self.no_index = Some(failure.message.clone()),
                    Err(_) => {}
                }

                true
            }
            Ask::Offer(_) if self.pricing.as_ref().is_some_and(|(asked, _, _, _)| asked == ask) => {
                let (_, body, genesis, from_steps) = self.pricing.take().expect("checked above");

                match result.as_ref().map_err(|failure| failure.message.clone()).and_then(fee_of) {
                    Ok((fee, warning)) => {
                        self.trouble = warning.map(|why| format!("A dry run says it would fail: {why}"));
                        self.bundle = Some(MultisigBundle { genesis, body: MultisigBody { fee, ..body }, signatures: Vec::new() });
                        self.signer = None;

                        if from_steps {
                            self.steps.clear();
                        }
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

/// Each of this wallet's addresses, with what it goes by: its account's name and its scheme.
fn names_by_address(holders: &[Holder]) -> BTreeMap<String, String> {
    holders
        .iter()
        .flat_map(|holder| holder.publics.iter().map(move |public| (public.address().to_string(), format!("{} · {}", holder.name, crate::keys::label(public.key_type())))))
        .collect()
}

/// The multisigs the node lists this wallet's addresses in, each with the names of those of its
/// members, then those followed by address alone: one list, each multisig once, those followed
/// marked so.
fn known(memberships: &BTreeMap<String, Vec<String>>, names: &BTreeMap<String, String>, followed: &[String]) -> Vec<Known> {
    let mut by_multisig: BTreeMap<String, Vec<String>> = BTreeMap::new();

    for (member, multisigs) in memberships {
        // An address of a wallet open before, or of an account since removed, is no one's here.
        let Some(name) = names.get(member) else {
            continue;
        };

        for multisig in multisigs {
            let listed = by_multisig.entry(multisig.clone()).or_default();

            if !listed.contains(name) {
                listed.push(name.clone());
            }
        }
    }

    let mut known: Vec<Known> = by_multisig.into_iter().map(|(address, members)| Known { followed: followed.contains(&address), address, members }).collect();

    for address in followed {
        if !known.iter().any(|known| known.address == *address) {
            known.push(Known { address: address.clone(), members: Vec::new(), followed: true });
        }
    }

    known
}

pub fn show(ui: &mut Ui, ctx: &mut Ctx, state: &mut MultisigState, send: &mut SendState) {
    let palette = *ctx.palette;
    theme::title(ui, &palette, "Multisig", "An account several people control together: it sends nothing until enough of them agree.");

    let Some(genesis) = ctx.chain.genesis else {
        theme::card(ui, &palette, |ui| {
            ui.label(RichText::new("Waiting for the light client to start: a multisig's signatures hold on one chain alone, and it has not said which.").color(palette.muted));
        });

        return;
    };
    let mut prepared = None;

    state.ask_memberships(ctx);

    // The steps take the page while they are open.
    if let Some(mut wizard) = state.wizard.take() {
        let listed = wizard.changing.as_ref().and_then(|account| state.shown.as_ref().filter(|(address, _)| address == account)).map(|(_, value)| value.clone());

        match create::show(ui, ctx, &mut wizard, listed.as_ref()) {
            create::Done::Going => state.wizard = Some(wizard),
            create::Done::Cancelled => {}
            create::Done::Make(made) => prepared = Some(made),
            create::Done::Change(account, tx) => {
                if let Some(value) = &listed {
                    price(ctx, state, account, tx, value, genesis, false);
                }
            }
        }

        if let Some(prepared) = prepared {
            send.prepare(prepared);
            *ctx.page = Page::Send;
        }

        return;
    }

    followed(ui, ctx, state);

    if let Some(selected) = state.selected.clone() {
        if ctx.now.saturating_sub(state.asked_at) > ctx.settings.pace().every_ms(REFRESH) {
            ctx.node.ask(Ask::Multisig(selected.clone()));
            state.asked_at = ctx.now;
        }

        if let Some((_, value)) = state.shown.as_ref().filter(|(address, _)| *address == selected) {
            let value = value.clone();

            // What another page asked: priced as the multisig's next transaction.
            if let Some((account, tx)) = state.incoming.take_if(|(account, _)| *account == selected)
                && let Ok(account) = account.parse::<Address>()
            {
                price(ctx, state, account, tx, &value, genesis, false);
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

    if let Some(prepared) = prepared {
        send.prepare(prepared);
        *ctx.page = Page::Send;
    }
}

/// What a multisig needs, for someone who has none yet, or every one this wallet knows of, one
/// picked, with its address whole, to copy or show as a code; making one, and following one a member
/// shared.
fn followed(ui: &mut Ui, ctx: &mut Ctx, state: &mut MultisigState) {
    let palette = *ctx.palette;
    let known = state.known(ctx);

    theme::card(ui, &palette, |ui| {
        if state.selected.as_ref().is_none_or(|selected| !known.iter().any(|known| known.address == *selected)) {
            state.selected = known.first().map(|known| known.address.clone());
        }

        match &state.selected.clone() {
            Some(selected) => {
                theme::caps(ui, &palette, "Your multisigs");

                for each in &known {
                    let who = match (each.members.is_empty(), each.followed) {
                        (true, _) => "followed: none of this wallet's accounts is a member".to_string(),
                        (false, _) => format!("{} {} a member", each.members.join(", "), if each.members.len() == 1 { "is" } else { "are" }),
                    };
                    let picked = *selected == each.address;
                    let text = RichText::new(format!("{}\n{who}", format::short(&each.address))).color(if picked { palette.ink } else { palette.soft });

                    if ui.add(egui::Button::selectable(picked, text).wrap()).clicked() && !picked {
                        state.selected = Some(each.address.clone());
                        state.asked_at = 0;
                        state.steps.clear();
                        state.signer = None;
                    }
                }

                ui.add_space(8.0);
                theme::caps(ui, &palette, "Its address");
                super::text_block(ui, &palette, selected);
                ui.horizontal_wrapped(|ui| {
                    if theme::quiet(ui, &palette, "Copy").clicked() {
                        ui.ctx().copy_text(selected.clone());
                        state.note = Some("Copied its address: send it coin there, from any account.".to_string());
                    }
                });
                let side = ui.available_width().clamp(160.0, 220.0);
                ui.vertical_centered(|ui| super::receive::qr(ui, selected, side));
                ui.add_space(4.0);
            }
            None => {
                theme::caps(ui, &palette, "What you need");
                ui.add(egui::Label::new(RichText::new("One key on this chain from each member, and how many of them must agree.").color(palette.ink)).wrap());
                ui.add_space(2.0);
                ui.add(egui::Label::new(RichText::new("You do not need a Bitcoin address, or anything from another chain. Those are optional, for a group that also wants to hold coins on Bitcoin or EVM chains.").small().color(palette.soft)).wrap());
                ui.add_space(6.0);
            }
        }

        if state.no_index.is_some() {
            ui.add(egui::Label::new(RichText::new("This node keeps no index of multisigs by member, so only those made here or followed by their address show.").small().color(palette.muted)).wrap());
        }

        // The first thing to do until there is one; after that, one thing among others.
        let make = match state.selected.is_some() {
            true => theme::quiet(ui, &palette, "Make another multisig"),
            false => theme::primary(ui, &palette, "Make a multisig", true),
        };

        if make.clicked() {
            state.wizard = Some(create::Wizard::default());
        }

        ui.add_space(10.0);
        ui.label(RichText::new("Watching one you are not a member of, or one this node does not list? Paste its address to follow it.").small().color(palette.soft));
        let followed = theme::beside(
            ui,
            |ui| theme::quiet(ui, &palette, "Follow").clicked(),
            |ui, width| {
                ui.add(egui::TextEdit::singleline(&mut state.adding).hint_text("multisig:0x…").font(egui::TextStyle::Monospace).desired_width(width));
            },
        );

        if followed {
            match state.adding.trim().parse::<Address>() {
                Ok(address @ Address::Multisig(_)) => {
                    follow(ctx, &address.to_string());
                    state.selected = Some(address.to_string());
                    state.asked_at = 0;
                    state.adding.clear();
                    state.trouble = None;
                }
                _ => state.trouble = Some("A multisig's address is multisig:0x and 64 hex digits.".to_string()),
            }
        }

        if state.selected.is_none() {
            notes(ui, &palette, state);
        }
    });
}

fn follow(ctx: &mut Ctx, address: &str) {
    if !ctx.settings.multisigs.iter().any(|known| known == address) {
        ctx.settings.multisigs.push(address.to_string());
    }
}

/// This wallet's keys among a multisig's members, as the node lists them: every account's, not the
/// one in use alone, each by its account's name and scheme.
fn wallet_members(holders: &[Holder], members: &[Value]) -> Vec<Mine> {
    let listed: Vec<&str> = members.iter().filter_map(|member| member["key"].as_str()).collect();

    holders
        .iter()
        .enumerate()
        .flat_map(|(index, holder)| {
            holder.publics.iter().filter(|public| listed.contains(&public.to_string().as_str())).map(move |public| {
                let address = public.address();

                Mine {
                    holder: index,
                    address,
                    key: *public,
                    name: format!("{} · {}", holder.name, crate::keys::label(public.key_type())),
                    here: holder.key(&address).is_some(),
                    by_metamask: holder.by_metamask(),
                }
            })
        })
        .collect()
}

/// Who signs next, by default: the first of this wallet's member keys that has neither signed nor
/// approved, one held here before one on an offline signer; once they all have, an offline signer.
pub fn default_signer(mine: &[Mine], done: &[PublicKey]) -> Signer {
    let open = |here: bool| mine.iter().find(|member| member.here == here && !member.by_metamask && !done.contains(&member.key));

    open(true).or_else(|| open(false)).map_or(Signer::Offline, |member| Signer::Account(member.address))
}

/// Who signs: the one picked, while it is still one of this wallet's members, or the default.
pub fn resolve(picked: Option<Signer>, mine: &[Mine], done: &[PublicKey]) -> Signer {
    match picked {
        Some(Signer::Offline) => Signer::Offline,
        Some(Signer::Account(address)) if mine.iter().any(|member| member.address == address) => Signer::Account(address),
        _ => default_signer(mine, done),
    }
}

/// What a signer goes by in the picker, and whether it has signed or approved already.
fn signer_name(mine: &[Mine], signer: Signer, done: &[PublicKey]) -> String {
    match signer {
        Signer::Offline => "An offline signer".to_string(),
        Signer::Account(address) => match mine.iter().find(|member| member.address == address) {
            Some(member) if done.contains(&member.key) => format!("{}, agreed already", member.name),
            Some(member) => member.name.clone(),
            None => format::short(&address.to_string()),
        },
    }
}

/// The picker of who signs and approves: each of this wallet's members, and an offline signer.
fn signer_picker(ui: &mut Ui, ctx: &Ctx, state: &mut MultisigState, mine: &[Mine], done: &[PublicKey], salt: &str) -> Signer {
    let palette = *ctx.palette;
    let chosen = resolve(state.signer, mine, done);

    ui.label(RichText::new("Agree as").small().color(palette.muted));
    egui::ComboBox::from_id_salt(("multisig signer", salt)).selected_text(signer_name(mine, chosen, done)).width(ui.available_width()).show_ui(ui, |ui| {
        for member in mine {
            let signer = Signer::Account(member.address);

            if ui.selectable_label(chosen == signer, signer_name(mine, signer, done)).clicked() {
                state.signer = Some(signer);
            }
        }

        if ui.selectable_label(chosen == Signer::Offline, signer_name(mine, Signer::Offline, done)).clicked() {
            state.signer = Some(Signer::Offline);
        }
    });

    resolve(state.signer, mine, done)
}

/// What a member's key goes by: this wallet's account and scheme for one of its own, else the key
/// cut short.
fn who(holders: &[Holder], key: &str) -> String {
    holders
        .iter()
        .find_map(|holder| holder.publics.iter().find(|public| public.to_string() == key).map(|public| format!("{} · {}", holder.name, crate::keys::label(public.key_type()))))
        .unwrap_or_else(|| format::short(key))
}

/// Approving as one of this wallet's members goes to Send as that account, which signs it however
/// the account signs: here, on the offline signer, or in MetaMask.
fn approve_as(ctx: &mut Ctx, member: &Mine, prepared: Prepared) -> Prepared {
    if let Some(holder) = ctx.holders.get(member.holder) {
        let id = holder.id();
        ctx.settings.set_current(id);
    }

    prepared
}

/// The selected multisig: its members, balance and next nonce, and the proposals open for it, to
/// approve or take back as whichever of this wallet's members is picked.
fn details(ui: &mut Ui, ctx: &mut Ctx, state: &mut MultisigState, value: &Value, prepared: &mut Option<Prepared>) {
    let palette = *ctx.palette;
    let members = value["members"].as_array().cloned().unwrap_or_default();
    let threshold = value["threshold"].as_u64().unwrap_or(0);
    let nonce = value["nonce"].as_u64().unwrap_or(0);
    let account: Option<Address> = value["address"].as_str().and_then(|text| text.parse().ok());
    let mine = wallet_members(ctx.holders, &members);

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, &format!("{} members must agree", create::of(threshold as u32, members.len())));
        super::row(ui, &palette, "Balance", |ui| {
            ui.label(ctx.amount(None, value["balance"].as_str().unwrap_or("0")));
        });

        if value["balance"].as_str().unwrap_or("0") == "0" {
            ui.add(egui::Label::new(RichText::new(format!("It pays its own fees, so it can do nothing until someone sends it some {}.", ctx.coin.symbol)).small().color(palette.warn)).wrap());
        }

        super::row(ui, &palette, "Its next transaction is", |ui| {
            ui.label(format!("number {nonce}"));
        });

        ui.add_space(4.0);
        theme::caps(ui, &palette, "Members");

        for member in &members {
            let key = member["key"].as_str().unwrap_or("?");
            let ours = mine.iter().find(|mine| mine.key.to_string() == key);
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(format::short(key)).monospace().color(palette.ink));

                if let Some(ours) = ours {
                    ui.label(RichText::new(&ours.name).small().color(palette.good));
                }

                let reach = match (member["bitcoin"].is_string(), member["evm"].is_string()) {
                    (true, true) => "also Bitcoin and EVM",
                    (true, false) => "also Bitcoin",
                    (false, true) => "also EVM",
                    (false, false) => "",
                };
                ui.label(RichText::new(reach).small().color(palette.muted));
            });
        }

        if mine.is_empty() {
            ui.add(egui::Label::new(RichText::new("None of this wallet's accounts is a member: it can watch, and an offline signer that holds a member's key can sign.").small().color(palette.muted)).wrap());
        }

        let Some(account) = account else {
            return;
        };

        ui.add_space(4.0);

        if theme::quiet(ui, &palette, "Change its members").on_hover_text("Add, remove or rotate keys, or change how many must agree").clicked() {
            state.wizard = Some(create::Wizard::changing(&account.to_string(), value));
        }

        let waiting = value["proposals"].as_array().cloned().unwrap_or_default();

        ui.add_space(8.0);
        theme::caps(ui, &palette, "Waiting for approval");

        if waiting.is_empty() {
            ui.label(RichText::new("Nothing. Anything proposed below, or by another member, shows here once a member approves it on chain.").small().color(palette.muted));
        }

        for (place, proposal) in waiting.iter().enumerate() {
            let Some(tx) = proposal["encoded"].as_str().and_then(from_hex).and_then(|bytes| wincode::deserialize::<Tx>(&bytes).ok()) else {
                continue;
            };
            let Some(fee) = fee_of(&proposal["fee"]).ok().map(|(fee, _)| fee) else {
                continue;
            };
            let approvals: Vec<String> = proposal["approvals"].as_array().into_iter().flatten().filter_map(|key| key.as_str().map(str::to_string)).collect();
            let done: Vec<PublicKey> = approvals.iter().filter_map(|key| key.parse().ok()).collect();

            ui.add_space(8.0);
            ui.separator();
            ui.label(RichText::new(describe(ctx, &tx)).color(palette.ink));

            // What it is members approve, whole, before anyone can: a batch's steps, or a lone
            // call laid out in the same frame, so its value, recipient and calldata all show.
            match super::steps::of_tx(ctx, &tx, None) {
                Some(steps) => {
                    ui.push_id(("proposal steps", approvals.len(), nonce), |ui| super::steps::listed(ui, ctx, &steps));
                }
                None => {
                    let one = super::steps::of_one(ctx, &tx, None);

                    if one.steps[0].allowed {
                        ui.push_id(("proposal step", approvals.len(), nonce), |ui| super::steps::single(ui, ctx, &one.steps[0]));
                    }
                }
            }

            let names: Vec<String> = approvals.iter().map(|key| who(ctx.holders, key)).collect();

            if !names.is_empty() {
                ui.add(egui::Label::new(RichText::new(format!("Approved by {}", names.join(", "))).small().color(palette.soft)).wrap());
            }

            let ours_approved = mine.iter().any(|member| done.contains(&member.key));
            ui.add(egui::Label::new(RichText::new(progress(approvals.len() as u64, threshold, ours_approved, !mine.is_empty())).small().color(palette.soft)).wrap());

            let chosen = match mine.is_empty() {
                true => None,
                false => ui.push_id(("approve as", place), |ui| Some(signer_picker(ui, ctx, state, &mine, &done, &format!("proposal {place}")))).inner,
            };
            ui.horizontal_wrapped(|ui| {
                if let Some(Signer::Account(address)) = chosen
                    && let Some(member) = mine.iter().find(|member| member.address == address)
                {
                    let approved = done.contains(&member.key);

                    if !approved && theme::quiet(ui, &palette, &format!("Approve as {}", member.name)).clicked() {
                        *prepared = Some(approve_as(ctx, member, approval(address, account, nonce, fee, tx.clone())));
                    }

                    if approved && theme::quiet(ui, &palette, &format!("Take back {}'s approval", member.name)).clicked() {
                        let id = MultisigBody { account, tx: tx.clone(), nonce, fee }.id(&ctx.chain.genesis.expect("shown only with a genesis"));
                        let withdrawal = Prepared {
                            from: address,
                            tx: Tx::WithdrawMultisig { account, proposal: id },
                            what: "Withdraw an approval".to_string(),
                            about: format::short(&account.to_string()),
                            coin: u256::ZERO,
                        };
                        *prepared = Some(approve_as(ctx, member, withdrawal));
                    }
                }

                if approvals.len() as u64 >= threshold && state.submitting.is_none() && theme::primary(ui, &palette, "Send it", true).clicked() {
                    let bundle = MultisigBundle { genesis: ctx.chain.genesis.expect("shown only with a genesis"), body: MultisigBody { account, tx: tx.clone(), nonce, fee }, signatures: Vec::new() };
                    submit(ctx, state, &bundle, threshold);
                }
            });

            if chosen == Some(Signer::Offline) {
                ui.add(egui::Label::new(RichText::new(OFFLINE_APPROVES).small().color(palette.muted)).wrap());
            }
        }

        notes(ui, &palette, state);
    });
}

/// What an offline signer this wallet holds no account of does about approving on chain.
const OFFLINE_APPROVES: &str = "An approval on chain is a transaction from the member's own account. To approve from an offline signer, pair its account here first, under Accounts, and pick it above; or sign the bundle on it instead.";

/// Where a proposal stands and what it needs next, in a line.
fn progress(backed: u64, needed: u64, mine: bool, member: bool) -> String {
    let more = needed.saturating_sub(backed);

    match (more, mine, member) {
        (0, _, _) => format!("{backed} of {needed} agree: it has what it needs. Anyone can send it now, and the multisig pays the fee."),
        (more, true, _) => format!("{backed} of {needed} agree, you among them. It needs {more} more of the other members."),
        (more, false, true) => format!("{backed} of {needed} agree. It needs {more} more: you have not agreed yet."),
        (more, false, false) => format!("{backed} of {needed} agree. It needs {more} more of its members."),
    }
}

/// What a bundle needs next, in a line: passing it on, or sending it.
fn next_step(backed: u64, needed: Option<u64>, signed: bool, member: bool) -> String {
    let Some(needed) = needed else {
        return "Asking the node about the multisig…".to_string();
    };
    let more = needed.saturating_sub(backed);

    match (more, signed, member) {
        (0, _, _) => format!("{backed} of {needed} agree: it can be sent. The multisig pays the fee."),
        (more, false, true) => format!("{backed} of {needed} agree. Next, sign it here, then pass it on: it needs {more} more."),
        (more, true, _) => format!("{backed} of {needed} agree, you among them. Next, sign it with another of your keys, or pass it on: copy it as text, or show its code, to the other members. It needs {more} more."),
        (more, false, false) => format!("{backed} of {needed} agree. It needs {more} more, from its members."),
    }
}

/// Proposing from the multisig: a payment of the coin, the quick one, or anything the Batch page
/// makes, with its form; priced by the node.
fn propose(ui: &mut Ui, ctx: &mut Ctx, state: &mut MultisigState, selected: &str, value: &Value, genesis: chainv4::structs::hash::Hash) {
    let palette = *ctx.palette;
    let Ok(account) = selected.parse::<Address>() else {
        return;
    };
    let busy = state.pricing.is_some();

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Propose");
        ui.add(egui::Label::new(RichText::new("Nothing is sent yet: this makes a bundle for the members to agree to.").small().color(palette.muted)).wrap());
        theme::segmented(ui, &palette, &mut state.proposing, &[(Proposing::Payment, "A payment"), (Proposing::Steps, "Anything else")]);
        ui.add_space(4.0);

        match state.proposing {
            Proposing::Payment => {
                ui.add(egui::TextEdit::singleline(&mut state.to).hint_text("To: ed25519:0x…, 0x…, bc1q…").font(egui::TextStyle::Monospace).desired_width(f32::INFINITY));
                ui.add(egui::TextEdit::singleline(&mut state.amount).hint_text(format!("Amount in {}", ctx.coin.symbol)).desired_width(f32::INFINITY));

                if theme::primary(ui, &palette, if busy { "Pricing…" } else { "Make the proposal" }, !busy).clicked() {
                    let to = super::send::recipient(state.to.trim());
                    let amount = format::parse_units(state.amount.trim(), ctx.coin.decimals).and_then(|raw| u256::from_str_radix(&raw, 10).ok());

                    match (to, amount) {
                        (Some(to), Some(amount)) if amount > u256::ZERO => {
                            price(ctx, state, account, Tx::Transfer { from: account, to, amount: chainv4::structs::amount::Amount::U256(amount) }, value, genesis, false);
                        }
                        _ => state.trouble = Some("A payment needs an address to pay and an amount above zero.".to_string()),
                    }
                }
            }
            Proposing::Steps => {
                ui.add(
                    egui::Label::new(
                        RichText::new("Anything it may send, as the Batch page makes it: token transfers, contract and EVM calls, deploys, names, burns and claims. Add each below. One step goes out as it is; several go together as a batch, all or nothing.")
                            .small()
                            .color(palette.soft),
                    )
                    .wrap(),
                );
            }
        }
    });

    if state.proposing == Proposing::Steps {
        let button = if busy { "Pricing…" } else { "Make the proposal" };

        if let Some((tx, _, _)) = batch::steps_card(ui, ctx, &mut state.steps, &account, button, true)
            && !busy
        {
            price(ctx, state, account, tx, value, genesis, true);
        }

        batch::add_form(ui, ctx, &mut state.steps, &account);
    }
}

/// Asks the node to price a transaction as the multisig's next, the bundle made once it answers.
fn price(ctx: &mut Ctx, state: &mut MultisigState, account: Address, tx: Tx, value: &Value, genesis: chainv4::structs::hash::Hash, from_steps: bool) {
    let nonce = value["nonce"].as_u64().unwrap_or(0);
    let body = MultisigBody { account, tx, nonce, fee: FeeOffer::NONE };
    let ask = Ask::Offer(json!({ "tx": to_hex(&wincode::serialize(&sized(&body, value, &genesis)).expect("a probe always encodes")) }).to_string());
    ctx.node.ask(ask.clone());
    state.pricing = Some((ask, body, genesis, from_steps));
    state.trouble = None;
}

/// A probe of the multisig's transaction, as heavy as any quorum of its members can make it, carried
/// by any key: what the node prices, unsigned. Each member's key and signature weigh by its scheme,
/// so a quorum of the largest keys costs more than one of the proposer's.
fn sized(body: &MultisigBody, value: &Value, genesis: &chainv4::structs::hash::Hash) -> SignedTx {
    let keys: Vec<PublicKey> = value["members"].as_array().into_iter().flatten().filter_map(|member| member["key"].as_str()?.parse().ok()).collect();
    let threshold = value["threshold"].as_u64().unwrap_or(1) as usize;

    MultisigBundle::probe(genesis, body, &keys, threshold)
}

/// The bundle on its way round: what it does, who signed, and signing, approving or submitting it,
/// as whichever of this wallet's members is picked, one after another; or, with none, reading one.
fn bundle(ui: &mut Ui, ctx: &mut Ctx, state: &mut MultisigState, genesis: chainv4::structs::hash::Hash, prepared: &mut Option<Prepared>) {
    let palette = *ctx.palette;

    let Some(mut held) = state.bundle.clone() else {
        egui::CollapsingHeader::new(RichText::new("Another member sent you something to sign?").color(palette.soft)).id_salt("multisig-bundle").default_open(!state.pasted.is_empty()).show(ui, |ui| {
            ui.add(egui::Label::new(RichText::new("Read the code they show you, or paste the text they sent. You see what it does before you sign.").small().color(palette.muted)).wrap());
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
        theme::section_gap(ui);

        return;
    };
    let listed = state.shown.as_ref().filter(|(address, _)| *address == held.body.account.to_string()).map(|(_, value)| value.clone());
    let members = listed.as_ref().and_then(|value| value["members"].as_array().cloned());
    let threshold = listed.as_ref().and_then(|value| value["threshold"].as_u64());
    let mine = members.as_deref().map(|members| wallet_members(ctx.holders, members)).unwrap_or_default();

    // Follow the bundle's multisig, so its members and approvals are known.
    if state.selected.as_deref() != Some(held.body.account.to_string().as_str()) {
        follow(ctx, &held.body.account.to_string());
        state.selected = Some(held.body.account.to_string());
        state.asked_at = 0;
    }

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Waiting for its members");
        ui.label(RichText::new(describe(ctx, &held.body.tx)).color(palette.ink).size(16.0));

        // The count and total first, then the rest of the bundle, then every step to sign. A lone
        // transaction is laid out in the same frame, so its value, recipient and calldata all show.
        let steps = super::steps::of_tx(ctx, &held.body.tx, None);
        let one = steps.is_none().then(|| super::steps::of_one(ctx, &held.body.tx, None));

        if let Some(steps) = &steps {
            super::steps::heading(ui, ctx, steps);
        }

        super::row(ui, &palette, "From", |ui| super::address(ui, &palette, &held.body.account.to_string()));
        super::row(ui, &palette, "Its transaction", |ui| {
            ui.label(format!("number {}", held.body.nonce));
        });
        super::row(ui, &palette, "Fee, at most", |ui| {
            ui.label(ctx.amount(None, &(u256::from(held.body.fee.gas_limit) * held.body.fee.max_price).to_string()));
        });

        let approvals = approvals_of(state, &held);
        super::row(ui, &palette, "Signed by", |ui| {
            let names: Vec<String> = held.signatures.iter().map(|signed| who(ctx.holders, &signed.key.to_string())).collect();
            ui.add(egui::Label::new(if names.is_empty() { "nobody yet".to_string() } else { names.join(", ") }).wrap());
        });

        if !approvals.is_empty() {
            super::row(ui, &palette, "Approved on chain by", |ui| {
                let names: Vec<String> = approvals.iter().map(|key| who(ctx.holders, key)).collect();
                ui.add(egui::Label::new(names.join(", ")).wrap());
            });
        }

        match (&steps, &one) {
            (Some(steps), _) => {
                ui.add_space(6.0);
                super::steps::each(ui, ctx, steps);
            }
            (None, Some(one)) if one.steps[0].allowed => {
                ui.add_space(6.0);
                super::steps::single(ui, ctx, &one.steps[0]);
            }
            _ => {}
        }

        let backed = held.signatures.len() + approvals.iter().filter(|key| !held.signatures.iter().any(|signed| signed.key.to_string() == **key)).count();
        let needed = threshold.unwrap_or(u64::MAX);
        let done: Vec<PublicKey> = held.signatures.iter().map(|signed| signed.key).chain(approvals.iter().filter_map(|key| key.parse().ok())).collect();
        let signed = mine.iter().any(|member| done.contains(&member.key));
        ui.add(egui::Label::new(RichText::new(next_step(backed as u64, threshold, signed, !mine.is_empty())).color(palette.ink)).wrap());
        ui.add(egui::Label::new(RichText::new("A member agrees by signing here, which costs nothing and goes round as a code or text, or by approving on chain from their own account, which others see without the bundle and pays a small fee. The two count together.").small().color(palette.muted)).wrap());
        ui.add_space(6.0);

        let chosen = signer_picker(ui, ctx, state, &mine, &done, "bundle");
        let member = match chosen {
            Signer::Account(address) => mine.iter().find(|member| member.address == address),
            Signer::Offline => None,
        };
        let agreed = member.is_some_and(|member| done.contains(&member.key));
        ui.add_space(6.0);

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
                    // The next of this wallet's members is the one to sign, unless one was picked.
                    state.signer = None;
                }
                Some(Message::Multisig(_)) => state.scan.fail("A bundle for another transaction: read the one the signer signed for this.".to_string()),
                Some(other) => state.scan.refuse(&other, "a multisig's transaction"),
                None => {}
            }
        }

        ui.horizontal_wrapped(|ui| {
            let keypair = member.and_then(|member| ctx.holders.get(member.holder)?.key(&member.address).cloned());

            if let (Some(member), Some(keypair)) = (member, &keypair)
                && !agreed
                && theme::primary(ui, &palette, &format!("Sign as {}", member.name), true).clicked()
            {
                held.sign(keypair);
                // On to the next of this wallet's members that has not signed.
                state.signer = None;
            }

            let by_signer = match member {
                Some(member) => !agreed && !member.here && !member.by_metamask,
                None => true,
            };

            if by_signer && !state.asking_signer && theme::primary(ui, &palette, "Ask the signer", true).clicked() {
                state.asking_signer = true;
            }

            if let Some(member) = member
                && !agreed
                && theme::quiet(ui, &palette, &format!("Approve on chain as {}", member.name)).clicked()
            {
                *prepared = Some(approve_as(ctx, member, approval(member.address, held.body.account, held.body.nonce, held.body.fee, held.body.tx.clone())));
            }

            if backed as u64 >= needed && state.submitting.is_none() && theme::primary(ui, &palette, "Send it", true).clicked() {
                submit(ctx, state, &held, needed);
            }

            if theme::quiet(ui, &palette, "Copy as text").clicked() {
                ui.ctx().copy_text(held.to_text());
                state.note = Some("Copied: send it to the other members, to paste on this page or in chainv4 multisig sign.".to_string());
            }

            if theme::quiet(ui, &palette, "Put away").clicked() {
                state.bundle = None;
                state.asking_signer = false;
            }
        });

        if chosen == Signer::Offline {
            ui.add(egui::Label::new(RichText::new(OFFLINE_APPROVES).small().color(palette.muted)).wrap());
        }

        if mine.is_empty() && members.is_some() {
            ui.add(egui::Label::new(RichText::new("None of this wallet's accounts is one of its members: an offline signer that holds a member's key can sign, or pass it on to the members.").small().color(palette.muted)).wrap());
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
            state.signer = None;
            state.trouble = None;
        }
        false => state.trouble = Some("A bundle for another chain: its signatures hold there only.".to_string()),
    }
}

/// Carries a bundle to the node, under a key made for the moment: it pays nothing, the multisig
/// does. It carries no more signatures than it needs, which is all its fee was priced for.
fn submit(ctx: &mut Ctx, state: &mut MultisigState, bundle: &MultisigBundle, threshold: u64) {
    let carrier = Keypair::generate(KeyType::Ed25519).unwrap_or_else(|_| Keypair::from_seed([7; 32]));
    let signed = bundle.trimmed(threshold as usize).envelope(&carrier);
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

/// A member's line for one of an account's keys, any scheme: the key, and, for an account its words
/// make, its Bitcoin multisig xpub on the network picked, and its EVM address. The xpub is the
/// words', whichever key here is the member's, a bitcoin: one too, which derives elsewhere.
pub(super) fn member_line_of(holder: &Holder, key: &PublicKey, seed: Option<&chainv4::crypto::mnemonic::Seed>, testnet: bool) -> String {
    let (evm, xpub) = other_chains(holder, seed, testnet);

    match (xpub, evm) {
        (Some(xpub), Some(evm)) => format!("{key},{xpub},{evm}"),
        (Some(xpub), None) => format!("{key},{xpub}"),
        (None, Some(evm)) => format!("{key},,{evm}"),
        (None, None) => key.to_string(),
    }
}

/// An account's EVM address, its Ethereum key's, and its words' multisig xpub on the network picked,
/// where it has them: what a member names for the multisig to reach EVM chains and Bitcoin.
pub(super) fn other_chains(holder: &Holder, seed: Option<&chainv4::crypto::mnemonic::Seed>, testnet: bool) -> (Option<String>, Option<String>) {
    let evm = holder.publics.iter().find_map(|key| match key.address() {
        Address::Secp256k1(address) => Some(to_hex(&address)),
        _ => None,
    });
    let network = network(testnet);
    let xpub = seed.filter(|_| holder.number().is_some()).map(|seed| chainv4::crypto::bitcoin::multisig::xpub_text(&seed.multisig_account(network.coin()), network));

    (evm, xpub)
}

fn network(testnet: bool) -> chainv4::crypto::bitcoin::tx::Network {
    match testnet {
        true => chainv4::crypto::bitcoin::tx::Network::Test,
        false => chainv4::crypto::bitcoin::tx::Network::Main,
    }
}

/// What a multisig's transaction does, in a line.
fn describe(ctx: &Ctx, tx: &Tx) -> String {
    match tx {
        Tx::Transfer { to, amount, .. } => format!("Pay {} to {}", ctx.amount(None, &amount.to_u256().to_string()), format::short(&to.to_string())),
        Tx::ChangeMultisig { members, threshold, .. } => format!("Change its members: {} must agree", create::of(*threshold, members.len())),
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
    fn a_bundle_lists_every_step_of_its_batch_for_its_members_on_any_screen() {
        use chainv4::structs::hash::{Hash, HashType};

        let account = Address::Multisig([6; 32]);
        let pay = Tx::Transfer { from: account, to: Address::Ed25519([9; 32]), amount: chainv4::structs::amount::Amount::U128(1_000_000_000_000_000_000) };
        let genesis = Hash::zero(HashType::default_chain());
        let body = MultisigBody { account, tx: Tx::Batch { steps: vec![pay; 3] }, nonce: 1, fee: FeeOffer::NONE };

        for (width, _) in crate::views::drawn::SCREENS {
            let mut book = crate::views::drawn::two_phrases();
            let mut holders = crate::keys::open(&book).unwrap();
            let mut state = MultisigState { bundle: Some(MultisigBundle { genesis, body: body.clone(), signatures: Vec::new() }), selected: Some(account.to_string()), ..MultisigState::default() };
            let (drawn, _, shown) = crate::views::drawn::page_shown(&mut book, &mut holders, (width, 20_000.0), |ui, ctx| {
                let mut prepared = None;

                bundle(ui, ctx, &mut state, genesis, &mut prepared);
            });
            let top = |said: &str| shown.iter().find(|(text, _)| text.contains(said)).map_or_else(|| panic!("{said} in {shown:?}"), |(_, rect)| rect.top());

            assert!(drawn <= width + 1.0, "{drawn} on {width}");
            assert!(top("A batch of 3 steps") < top("Sends 3 DEV in all"));
            assert!(top("Sends 3 DEV in all") < top("Step 1 of 3"));
            assert!(top("Step 1 of 3") < top("Step 2 of 3") && top("Step 2 of 3") < top("Step 3 of 3"));
            assert!(top("Step 3 of 3") < top("Copy as text"), "every step above what passes it on");
        }
    }

    /// What the node says of a multisig whose members are these keys.
    fn listed(account: Address, keys: &[PublicKey], threshold: u64) -> Value {
        json!({
            "address": account.to_string(),
            "members": keys.iter().map(|key| json!({ "key": key.to_string() })).collect::<Vec<_>>(),
            "threshold": threshold,
            "nonce": 7,
            "balance": "1000",
            "proposals": [],
        })
    }

    #[test]
    fn a_proposed_batch_lines_up_as_the_multisigs_next_transaction_with_each_step_its_own() {
        let account = Address::Multisig([4; 32]);
        let tx = Tx::Batch { steps: vec![Tx::Burn { from: account, amount: chainv4::structs::amount::Amount::U128(5) }] };
        let body = MultisigBody { account, tx, nonce: 7, fee: FeeOffer::NONE };
        let keys = [Keypair::from_seed([1; 32]).public(), Keypair::from_seed([2; 32]).public()];
        let probe = sized(&body, &listed(account, &keys, 2), &chainv4::structs::hash::Hash::zero(chainv4::structs::hash::HashType::default_chain()));

        assert_eq!((probe.sender(), probe.nonce), (account, 7));
        assert_eq!(probe.batch_problem(), None, "a batch the chain takes from it");
        assert!(matches!(&probe.tx, Tx::Multisig { tx, signatures, .. } if matches!(**tx, Tx::Batch { .. }) && signatures.len() == 2));

        let mut state = MultisigState::default();
        state.propose(Prepared { from: account, tx: body.tx.clone(), what: String::new(), about: String::new(), coin: u256::ZERO });

        assert_eq!(state.selected, Some(account.to_string()));
        assert!(state.incoming.is_some());
    }

    #[test]
    fn a_member_line_takes_its_xpub_from_the_accounts_own_words() {
        use crate::vault::{Book, Source, StoredAccount, Words};

        let abandon = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
        let mut book = Book {
            phrases: vec![Words::new("test test test test test test test test test test test junk".to_string(), String::new())],
            ..Book::default()
        };
        let from = book.add_phrase(Words::new(abandon.to_string(), String::new())).unwrap();
        book.accounts = vec![
            StoredAccount { name: "First".to_string(), source: Source::Phrase { account: 0, extra: Vec::new(), from: None } },
            StoredAccount { name: "Other".to_string(), source: Source::Phrase { account: 0, extra: Vec::new(), from } },
        ];
        let holders = crate::keys::open(&book).unwrap();
        let line = |holder: &crate::keys::Holder| member_line_of(holder, &holder.publics[0], crate::keys::seed_of(&book, holder), false);
        let xpub = |words: &Words| chainv4::crypto::bitcoin::multisig::xpub_text(&words.seed().unwrap().multisig_account(0), network(false));

        assert!(line(&holders[1]).contains(&xpub(&book.phrases[1])), "the other words' xpub, for their account");
        assert!(!line(&holders[1]).contains(&xpub(&book.phrases[0])));
        assert!(line(&holders[0]).contains(&xpub(&book.phrases[0])));
        assert_ne!(line(&holders[0]), line(&holders[1]));

        // Any of its keys is a member's, its bitcoin: one too, with the same words' xpub beside it.
        for public in &holders[0].publics {
            let line = member_line_of(&holders[0], public, crate::keys::seed_of(&book, &holders[0]), false);
            let row = create::Row::from_line(&line);

            assert_eq!(row.key, public.to_string());
            assert_eq!(row.xpub, xpub(&book.phrases[0]), "{line}");
            assert!(row.evm.starts_with("0x") && row.evm.len() == 42);
        }
    }

    #[test]
    fn a_proposal_says_where_it_stands_and_what_it_needs_next() {
        assert_eq!(progress(1, 2, false, true), "1 of 2 agree. It needs 1 more: you have not agreed yet.");
        assert_eq!(progress(1, 2, true, true), "1 of 2 agree, you among them. It needs 1 more of the other members.");
        assert_eq!(progress(0, 2, false, false), "0 of 2 agree. It needs 2 more of its members.");
        assert!(progress(2, 2, true, true).contains("Anyone can send it now"));
        assert!(progress(3, 2, false, false).contains("it has what it needs"), "more than it needs is enough");

        assert!(next_step(0, Some(2), false, true).contains("Next, sign it here"));
        assert!(next_step(1, Some(2), true, true).contains("another of your keys, or pass it on"));
        assert!(next_step(1, Some(2), false, false).contains("from its members"));
        assert!(next_step(2, Some(2), true, true).contains("it can be sent"));
        assert!(next_step(1, None, false, true).starts_with("Asking the node"));
    }

    #[test]
    fn a_priced_probe_weighs_what_its_heaviest_quorum_does_whoever_proposes() {
        use chainv4::crypto::keypair::PrivateKey;
        use chainv4::crypto::hashable::Hashable;

        let account = Address::Multisig([4; 32]);
        let genesis = chainv4::structs::hash::Hash::zero(chainv4::structs::hash::HashType::default_chain());
        let tx = Tx::Transfer { from: account, to: Address::Ed25519([9; 32]), amount: chainv4::structs::amount::Amount::U128(5) };
        let body = MultisigBody { account, tx, nonce: 7, fee: FeeOffer::NONE };
        let (ed, other, secp) = (Keypair::from_seed([1; 32]), Keypair::from_seed([2; 32]), Keypair::from_private(PrivateKey::Secp256k1([3; 32])));
        let probe = sized(&body, &listed(account, &[ed.public(), other.public(), secp.public()], 2), &genesis);
        let weight = |signed: &SignedTx| signed.hashable_bytes_with_tag().len();

        assert_eq!((probe.sender(), probe.nonce), (account, 7));
        assert!(matches!(&probe.tx, Tx::Multisig { signatures, .. } if signatures.len() == 2));

        // An Ed25519 member proposes; the secp256k1 one signs too, and any key carries it.
        let mut real = MultisigBundle { genesis, body: body.clone(), signatures: Vec::new() };
        real.sign(&ed);
        real.sign(&secp);

        for carrier in [&ed, &secp, &Keypair::generate(KeyType::Ed25519).unwrap()] {
            assert!(weight(&real.envelope(carrier)) <= weight(&probe), "the probe weighs at least what is sent");
        }

        // As it was priced: by Ed25519 keys alone, it weighs less than what is sent.
        let ed_only = sized(&body, &listed(account, &[ed.public(), other.public()], 2), &genesis);
        assert!(weight(&ed_only) < weight(&real.envelope(&secp)));
    }

    #[test]
    fn a_fee_reads_from_an_offer_or_a_proposal() {
        let offer = json!({ "gas_limit": 30_000, "max_price": "20", "tip": "1", "estimate": { "status": "failed", "error": "out of fuel" } });
        let (fee, warning) = fee_of(&offer).unwrap();

        assert_eq!((fee.gas_limit, fee.max_price, fee.tip), (30_000, u256::new(20), u256::new(1)));
        assert_eq!(warning.as_deref(), Some("out of fuel"));
        assert!(fee_of(&json!({ "gas_limit": 1 })).is_err());
    }

    #[test]
    fn every_multisig_this_wallets_accounts_are_in_lists_with_those_followed_each_once() {
        let names = BTreeMap::from([("ed25519:0x01".to_string(), "Main · Ed25519".to_string()), ("bitcoin:0x02".to_string(), "Main · Bitcoin".to_string()), ("ed25519:0x03".to_string(), "Saved · Ed25519".to_string())]);
        let memberships = BTreeMap::from([
            ("ed25519:0x01".to_string(), vec!["multisig:0xaa".to_string(), "multisig:0xbb".to_string()]),
            ("bitcoin:0x02".to_string(), vec!["multisig:0xaa".to_string()]),
            ("ed25519:0x03".to_string(), vec!["multisig:0xbb".to_string()]),
            // An address no account here has any more.
            ("ed25519:0x09".to_string(), vec!["multisig:0xcc".to_string()]),
        ]);
        let followed = ["multisig:0xbb".to_string(), "multisig:0xdd".to_string()];
        let listed = known(&memberships, &names, &followed);

        assert_eq!(listed, [
            Known { address: "multisig:0xaa".to_string(), members: vec!["Main · Bitcoin".to_string(), "Main · Ed25519".to_string()], followed: false },
            Known { address: "multisig:0xbb".to_string(), members: vec!["Main · Ed25519".to_string(), "Saved · Ed25519".to_string()], followed: true },
            Known { address: "multisig:0xdd".to_string(), members: Vec::new(), followed: true },
        ]);
        assert!(known(&BTreeMap::new(), &names, &[]).is_empty());
    }

    #[test]
    fn the_node_s_listing_is_taken_a_page_at_a_time_to_its_end_and_a_node_without_the_index_says_so() {
        let mut state = MultisigState::default();
        let first = Ask::Memberships { address: "ed25519:0x01".to_string(), cursor: None };
        let page = |addresses: &[&str], next: Option<&str>| Ok(json!({ "multisigs": addresses.iter().map(|address| json!({ "address": address })).collect::<Vec<_>>(), "next": next }));

        assert!(state.answered(&first, &page(&["multisig:0xaa"], Some("multisig:0xaa"))));
        assert_eq!(state.more, [("ed25519:0x01".to_string(), "multisig:0xaa".to_string())], "the next page is still to ask");

        let second = Ask::Memberships { address: "ed25519:0x01".to_string(), cursor: Some("multisig:0xaa".to_string()) };
        state.more.clear();
        state.answered(&second, &page(&["multisig:0xbb"], None));

        assert_eq!(state.memberships["ed25519:0x01"], ["multisig:0xaa", "multisig:0xbb"]);
        assert!(state.more.is_empty(), "nothing after the last");

        // Asked again from the start, the listing is what the node says now.
        state.answered(&first, &page(&["multisig:0xbb"], None));
        assert_eq!(state.memberships["ed25519:0x01"], ["multisig:0xbb"]);

        state.answered(&first, &Err(Failure { status: 404, message: "this node keeps no member index".to_string() }));
        assert!(state.no_index.is_some());
    }

    /// Mine, for the picker's tests: held here or not.
    fn mine(seed: u8, here: bool) -> Mine {
        let key = Keypair::from_seed([seed; 32]).public();

        Mine { holder: 0, address: key.address(), key, name: format!("Key {seed}"), here, by_metamask: false }
    }

    #[test]
    fn the_key_that_signs_is_by_default_one_that_has_not_and_any_can_be_picked() {
        let members = [mine(1, true), mine(2, false), mine(3, true)];
        let [one, two, three] = [&members[0], &members[1], &members[2]];

        assert_eq!(default_signer(&members, &[]), Signer::Account(one.address));
        assert_eq!(default_signer(&members, &[one.key]), Signer::Account(three.address), "one held here before the offline signer's");
        assert_eq!(default_signer(&members, &[one.key, three.key]), Signer::Account(two.address));
        assert_eq!(default_signer(&members, &[one.key, two.key, three.key]), Signer::Offline);
        assert_eq!(default_signer(&[], &[]), Signer::Offline, "no account here is a member");

        // What is picked stays picked while it is one of them.
        assert_eq!(resolve(Some(Signer::Account(two.address)), &members, &[]), Signer::Account(two.address));
        assert_eq!(resolve(Some(Signer::Offline), &members, &[]), Signer::Offline);
        assert_eq!(resolve(Some(Signer::Account(Address::Ed25519([9; 32]))), &members, &[]), Signer::Account(one.address));
        assert_eq!(signer_name(&members, Signer::Account(one.address), &[one.key]), "Key 1, agreed already");
    }

    #[test]
    fn a_bundle_is_signed_by_one_of_this_wallets_members_then_the_next_on_any_screen() {
        use chainv4::structs::hash::{Hash, HashType};

        let account = Address::Multisig([6; 32]);
        let genesis = Hash::zero(HashType::default_chain());
        let body = MultisigBody { account, tx: Tx::Transfer { from: account, to: Address::Ed25519([9; 32]), amount: chainv4::structs::amount::Amount::U128(1) }, nonce: 7, fee: FeeOffer::NONE };

        for (width, _) in crate::views::drawn::SCREENS {
            // Tall enough that every button is on the screen to tap.
            let size = (width, 20_000.0);
            let mut book = crate::views::drawn::two_phrases();
            let mut holders = crate::keys::open(&book).unwrap();
            // Both accounts' Ed25519 keys, and a stranger's.
            let keys = [holders[0].publics[0], holders[1].publics[0], Keypair::from_seed([5; 32]).public()];
            let mut state = MultisigState {
                bundle: Some(MultisigBundle { genesis, body: body.clone(), signatures: Vec::new() }),
                selected: Some(account.to_string()),
                shown: Some((account.to_string(), listed(account, &keys, 3))),
                ..MultisigState::default()
            };
            let (main, saved) = (holders[0].name.clone(), holders[1].name.clone());
            let mut draw = |ui: &mut Ui, ctx: &mut Ctx| {
                let mut prepared = None;

                bundle(ui, ctx, &mut state, genesis, &mut prepared);
            };
            let said = crate::views::drawn::tap(&mut book, &mut holders, size, &format!("Sign as {main} · Ed25519"), &mut draw);

            assert!(said.contains(&format!("Sign as {saved} · Ed25519")), "on to the next: {said}");
            assert!(said.contains(&format!("Signed by {main} · Ed25519")) || said.contains(&format!("{main} · Ed25519")), "{said}");

            let said = crate::views::drawn::tap(&mut book, &mut holders, size, &format!("Sign as {saved} · Ed25519"), &mut draw);
            let signed: Vec<PublicKey> = state.bundle.as_ref().unwrap().signatures.iter().map(|signed| signed.key).collect();

            assert_eq!(signed, keys[..2], "each of them, in turn");
            assert!(said.contains("Ask the signer"), "the offline signer is next: {said}");
            assert!(said.contains("2 of 3 agree"), "{said}");
        }
    }

    #[test]
    fn the_page_lists_this_wallets_multisigs_whole_and_proposes_any_step_on_any_screen() {
        let account = Address::Multisig([6; 32]);

        for size in crate::views::drawn::SCREENS {
            let mut book = crate::views::drawn::two_phrases();
            let mut holders = crate::keys::open(&book).unwrap();
            let main = holders[0].publics[0];
            let mut state = MultisigState {
                memberships: BTreeMap::from([(main.address().to_string(), vec![account.to_string()])]),
                listed_at: 1,
                shown: Some((account.to_string(), listed(account, &[main, Keypair::from_seed([5; 32]).public()], 2))),
                asked_at: u64::MAX / 2,
                proposing: Proposing::Steps,
                ..MultisigState::default()
            };
            let mut send = SendState::default();
            // Tall enough that nothing is left undrawn for being off the screen.
            let (width, _, shown) = crate::views::drawn::following_shown(&mut book, &mut holders, (size.0, 20_000.0), |ui, ctx| show(ui, ctx, &mut state, &mut send));
            let texts: Vec<&str> = shown.iter().map(|(text, _)| text.as_str()).collect();

            assert!(width <= size.0 + 1.0, "{size:?}: {width}");
            assert!(texts.iter().any(|text| text.contains(&format!("{} · Ed25519 is a member", holders[0].name))), "{texts:?}");
            assert!(texts.contains(&account.to_string().as_str()), "its address whole");
            assert!(texts.contains(&"Copy") && texts.contains(&"Add step") && texts.contains(&"Make the proposal"), "{texts:?}");
            assert_eq!(state.selected, Some(account.to_string()), "picked by itself, though never followed");
        }
    }
}
