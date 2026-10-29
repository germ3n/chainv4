//! Governance: the chain's settings that its council, or finality's own voters, change on chain.
//! What a change needs and who may approve, the changes open and their approvals, approving one or
//! withdrawing an approval, proposing a change of any kind, what stands and what was decided last.
//!
//! What the node says of governance is shown as its word: GET /governance, /governance/history and
//! /governance/weight/{address}. Nothing signed rests on it but the version an approval names,
//! which the chain checks: an approval for a version that moved on fails and changes nothing. Each
//! change is read with chainv4's own change_from_json, which checks it as the CLI's approve-change
//! does, before it is signed. Approvals are signed and sent on the Send page, as any other
//! transaction, or proposed to a multisig's members on its page; once in a block, the light client
//! proves their receipts, whose outcome says which change an approval decided.

use {
    super::multisig::MultisigState,
    super::send::{Prepared, SendState},
    super::{Ctx, Page},
    crate::app::Coin,
    crate::format,
    crate::history::Vote,
    crate::node::{Ask, Failure},
    crate::theme,
    chainv4::consensus::ConsensusConfig,
    chainv4::governance::Change,
    chainv4::params::fees::PricingConfig,
    chainv4::params::finality::FinalityConfig,
    chainv4::params::json::{change_from_json, change_json},
    chainv4::structs::address::Address,
    chainv4::structs::tx::Tx,
    chainv4::structs::u256::u256,
    egui::{RichText, Ui},
    serde_json::{Value, json},
    std::collections::HashMap,
};

/// How often the node is asked again, in milliseconds: governance moves at most once a block.
const REFRESH: u64 = 15_000;

/// What the API answers on a chain without governance.
const NOT_FOUND: u16 = 404;

/// The kinds of change, in the order the propose form lists them.
const KINDS: [&str; 13] = ["name", "bootnodes", "upgrade", "name_prices", "consensus", "fees", "reward", "limits", "finality", "switch_finality", "collections", "federation", "directory"];

/// The settings in force, in the order the API lists them.
const IN_FORCE: [&str; 12] = ["consensus", "name", "bootnodes", "name_prices", "collections", "federation", "directory", "fees", "reward", "limits", "finality", "upgrade"];

/// GET /governance, read.
#[derive(Clone, Debug, PartialEq)]
pub struct Listing {
    /// council or electorate.
    pub kind: String,
    pub epoch: u64,
    /// Decisions so far: what an approval names.
    pub version: u64,
    pub approval_epochs: u64,
    /// What a change needs, as a decimal.
    pub needed: String,
    /// The council's accounts; None for finality's voters.
    pub members: Option<Vec<String>>,
    pub proposals: Vec<Proposal>,
    pub in_force: Vec<Setting>,
    /// The kind of finality the tip's epoch runs: what an electorate's weight is counted in.
    pub finality: Option<String>,
    /// A switch of finality's kind ahead: its epoch and kind.
    pub switch: Option<(u64, String)>,
}

/// An open change and its approvals.
#[derive(Clone, Debug, PartialEq)]
pub struct Proposal {
    /// The change's JSON, without what the listing adds to it.
    pub json: Value,
    /// The change, or why it does not read.
    pub change: Result<Change, String>,
    pub weight: String,
    pub needed: String,
    pub approvals: Vec<Approval>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Approval {
    pub account: String,
    pub weight: String,
    pub epoch: u64,
    pub counts_until: u64,
}

/// A setting as it stands: genesis's, or the change that last decided it.
#[derive(Clone, Debug, PartialEq)]
pub struct Setting {
    pub kind: String,
    pub value: Value,
    pub decided: Option<u64>,
}

/// GET /governance/weight/{address}, read.
#[derive(Clone, Debug, PartialEq)]
pub struct Weight {
    pub weight: u256,
    /// The change this address's counting approval is for.
    pub approved: Option<Change>,
}

impl Weight {
    /// Whether the address may approve at all.
    pub fn may_vote(&self) -> bool {
        self.weight > u256::ZERO
    }
}

/// A decision, from GET /governance/history.
#[derive(Clone, Debug, PartialEq)]
pub struct Decision {
    pub version: u64,
    pub tx: String,
    pub height: u64,
    pub timestamp: u64,
    pub final_: bool,
    pub sender: Option<String>,
    pub change: Option<Result<Change, String>>,
}

/// An address that could approve: one of an account's, or a multisig followed.
#[derive(Clone, Debug, PartialEq)]
struct Voter {
    address: Address,
    /// The account it is of, as named, or Multisig.
    account: String,
    /// Whether this page can send for it: the account in use's, or a followed multisig's.
    here: bool,
}

/// A transaction this page built, waiting to be confirmed before Send or the multisig's page.
#[derive(Clone, Debug, PartialEq)]
struct Pending {
    from: Address,
    tx: Tx,
    what: String,
    /// The change in words.
    words: String,
    /// The change's JSON, pretty.
    json: Option<String>,
    /// What it does to the address's approval.
    note: String,
}

/// The propose form's fields.
#[derive(Clone, Debug, Default, PartialEq)]
struct Form {
    name: String,
    /// One a line.
    bootnodes: String,
    least: String,
    from: String,
    /// One price a line, in the coin.
    prices: String,
    /// Any other kind's value, as JSON.
    json: String,
}

#[derive(Default)]
pub struct GovernanceState {
    /// The node the answers came from: they go once the wallet talks to another.
    node: String,
    listing: Option<Result<Listing, Failure>>,
    history: Option<Result<Vec<Decision>, String>>,
    weights: HashMap<String, Result<Weight, String>>,
    asked: Option<u64>,
    /// The kind proposed, and the form, filled from what stands for that kind once picked.
    kind: String,
    filled: Option<String>,
    form: Form,
    /// The address to propose from.
    proposer: Option<Address>,
    pending: Option<Pending>,
    trouble: Option<String>,
}

impl GovernanceState {
    /// Takes the node's answer to something this page asked.
    pub fn answered(&mut self, ask: &Ask, result: &Result<Value, Failure>) {
        let unreadable = |why: String| Failure { status: 0, message: format!("its answer does not read: {why}") };

        match ask {
            Ask::Governance => self.listing = Some(result.clone().and_then(|value| listing(&value).map_err(unreadable))),
            Ask::GovernanceHistory => self.history = Some(result.as_ref().map(decisions).map_err(|failure| failure.message.clone())),
            Ask::GovernanceWeight(address) => {
                self.weights.insert(address.clone(), result.as_ref().map_err(|failure| failure.message.clone()).and_then(weight));
            }
            _ => {}
        }
    }

    /// Asks what is due: everything again each REFRESH, and at once for a voter not asked about.
    fn poll(&mut self, ctx: &mut Ctx, voters: &[Voter]) {
        if self.node != ctx.settings.node {
            *self = GovernanceState { node: ctx.settings.node.clone(), kind: std::mem::take(&mut self.kind), ..GovernanceState::default() };
        }

        let due = self.asked.is_none_or(|at| ctx.now.saturating_sub(at) > REFRESH);
        let governed = !matches!(&self.listing, Some(Err(failure)) if failure.status == NOT_FOUND);

        if due {
            ctx.node.ask(Ask::Governance);
            self.asked = Some(ctx.now);
        }

        if !governed {
            return;
        }

        if due {
            ctx.node.ask(Ask::GovernanceHistory);
        }

        for voter in voters {
            let address = voter.address.to_string();

            if due || !self.weights.contains_key(&address) {
                ctx.node.ask(Ask::GovernanceWeight(address));
            }
        }
    }
}

pub fn show(ui: &mut Ui, ctx: &mut Ctx, state: &mut GovernanceState, send: &mut SendState, multisig: &mut MultisigState) {
    let palette = *ctx.palette;
    theme::title(ui, &palette, "Governance", "Changes to the chain's settings, approved on chain by its council or by finality's voters.");

    let voters = voters(ctx);
    state.poll(ctx, &voters);

    let listing = match &state.listing {
        None => {
            theme::card(ui, &palette, |ui| super::waiting(ui, &palette, "Asking the node about the chain's governance…"));

            return;
        }
        Some(Err(failure)) if failure.status == NOT_FOUND => {
            theme::card(ui, &palette, |ui| {
                ui.label(RichText::new("This chain has no governance.").color(palette.ink));
                ui.label(RichText::new("Its settings stay as its genesis set them: nobody can approve a change to them.").small().color(palette.muted));
            });

            return;
        }
        Some(Err(failure)) => {
            let message = failure.message.clone();
            theme::card(ui, &palette, |ui| {
                ui.colored_label(palette.bad, format!("The node did not answer: {message}"));
            });

            return;
        }
        Some(Ok(listing)) => listing.clone(),
    };

    if let Some(pending) = state.pending.clone() {
        match confirm(ui, ctx, &pending) {
            Some(true) => {
                state.pending = None;
                // What the vote changes shows once back.
                state.asked = None;
                let prepared = Prepared { from: pending.from, tx: pending.tx, what: pending.what, about: pending.words, coin: u256::ZERO };

                match prepared.from {
                    Address::Multisig(_) => {
                        multisig.propose(prepared);
                        *ctx.page = Page::Multisig;
                    }
                    _ => {
                        send.prepare(prepared);
                        *ctx.page = Page::Send;
                    }
                }
            }
            Some(false) => state.pending = None,
            None => {}
        }

        return;
    }

    let mut chosen = None;

    header(ui, ctx, state, &listing, &voters);
    proposals(ui, ctx, state, &listing, &voters, &mut chosen);
    propose(ui, ctx, state, &listing, &voters, &mut chosen);
    votes(ui, ctx);
    in_force(ui, ctx, &listing);
    recent(ui, ctx, state);

    if chosen.is_some() {
        state.pending = chosen;
    }
}

/// The account in use's addresses and the multisigs followed, which this page sends for, and every
/// other account's, which it only shows.
fn voters(ctx: &Ctx) -> Vec<Voter> {
    let mut voters: Vec<Voter> = ctx
        .holders
        .iter()
        .enumerate()
        .flat_map(|(index, holder)| holder.addresses().into_iter().map(move |address| Voter { address, account: holder.name.clone(), here: index == ctx.current }))
        .collect();

    voters.extend(ctx.settings.multisigs.iter().filter_map(|account| account.parse().ok()).map(|address| Voter { address, account: "Multisig".to_string(), here: true }));

    voters
}

/// The addresses this page may approve with: its own to send for, which may approve.
fn acting(voters: &[Voter], weights: &HashMap<String, Result<Weight, String>>) -> Vec<Address> {
    voters
        .iter()
        .filter(|voter| voter.here && weights.get(&voter.address.to_string()).is_some_and(|weight| weight.as_ref().is_ok_and(Weight::may_vote)))
        .map(|voter| voter.address)
        .collect()
}

/// What the governance is, what a change needs, and what each account weighs.
fn header(ui: &mut Ui, ctx: &mut Ctx, state: &GovernanceState, listing: &Listing, voters: &[Voter]) {
    let palette = *ctx.palette;
    let coin = ctx.coin.clone();

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, &format!("Who decides · {}", listing.kind));
        ui.add_space(4.0);

        let who = match &listing.members {
            Some(members) => format!("A council of {} account{}", members.len(), plural(members.len() as u64)),
            None => match listing.finality.as_deref() {
                Some("ffg") => "Finality's voters: the FFG validators, by the stake they hold".to_string(),
                Some("authority") => "Finality's voters: the authority signers' accounts".to_string(),
                _ => "Finality's voters".to_string(),
            },
        };
        ui.label(RichText::new(who).color(palette.ink).size(16.0));

        if let Some(members) = &listing.members {
            for member in members {
                ui.horizontal_wrapped(|ui| {
                    super::address(ui, &palette, member);

                    if voters.iter().any(|voter| voter.address.to_string() == *member) {
                        theme::pill(ui, "yours", palette.accent);
                    }
                });
            }
        }

        ui.add_space(4.0);
        super::row(ui, &palette, "A change needs", |ui| {
            let needed = match &listing.members {
                Some(members) => format!("{} of the {} members' approvals", listing.needed, members.len()),
                None => weighs(listing, &listing.needed, &coin),
            };
            ui.label(RichText::new(needed).color(palette.ink));
        });
        super::row(ui, &palette, "Approvals count", |ui| {
            ui.label(format!("{} epoch{} from the one they were made in", listing.approval_epochs, plural(listing.approval_epochs)));
        });
        super::row(ui, &palette, "Version", |ui| {
            ui.label(format!("{} · changes decided so far; an approval names it, and lapses once another change is decided", listing.version));
        });
        super::row(ui, &palette, "Epoch", |ui| {
            ui.label(listing.epoch.to_string());
        });

        if let Some((epoch, kind)) = &listing.switch {
            super::row(ui, &palette, "Finality", |ui| {
                ui.label(RichText::new(format!("switches to {kind} from epoch {epoch}")).color(palette.warn));
            });
        }

        ui.add_space(8.0);
        theme::caps(ui, &palette, "Your accounts");

        if voters.is_empty() {
            ui.label(RichText::new("No accounts yet.").color(palette.muted));
        }

        for voter in voters {
            let address = voter.address.to_string();
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(format!("{} · {}", voter.account, super::scheme(&voter.address))).color(palette.soft));
                super::address(ui, &palette, &address);

                match state.weights.get(&address) {
                    None => {
                        ui.spinner();
                    }
                    Some(Err(why)) => {
                        ui.colored_label(palette.bad, why.as_str());
                    }
                    Some(Ok(weight)) if weight.may_vote() => {
                        theme::pill(ui, "may vote", palette.good);
                        ui.label(RichText::new(weighs(listing, &weight.weight.to_string(), &coin)).small().color(palette.soft));

                        if !voter.here {
                            ui.label(RichText::new("switch to this account to vote with it").small().color(palette.muted));
                        }
                    }
                    Some(Ok(_)) => {
                        ui.label(RichText::new("may not vote").small().color(palette.muted));
                    }
                }
            });

            if let Some(Ok(Weight { approved: Some(change), .. })) = state.weights.get(&address) {
                ui.label(RichText::new(format!("Its approval counts for: {}", describe(change, &coin))).small().color(palette.muted));
            }
        }
    });
}

/// The changes open, each with its approvals, and approving or withdrawing.
fn proposals(ui: &mut Ui, ctx: &mut Ctx, state: &GovernanceState, listing: &Listing, voters: &[Voter], chosen: &mut Option<Pending>) {
    let palette = *ctx.palette;
    let coin = ctx.coin.clone();
    let acting = acting(voters, &state.weights);
    let ours: Vec<String> = voters.iter().map(|voter| voter.address.to_string()).collect();

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Open changes");

        if listing.proposals.is_empty() {
            ui.label(RichText::new("No change has approvals that still count. Propose one below.").color(palette.muted));
        }

        for (index, proposal) in listing.proposals.iter().enumerate() {
            ui.add_space(6.0);
            ui.separator();

            match &proposal.change {
                Ok(change) => {
                    ui.label(RichText::new(describe(change, &coin)).color(palette.ink).size(15.0));
                }
                Err(why) => {
                    ui.colored_label(palette.bad, format!("A change this wallet does not read: {why}"));
                }
            }

            let fraction = match (proposal.weight.parse::<f64>(), proposal.needed.parse::<f64>()) {
                (Ok(weight), Ok(needed)) if needed > 0.0 => (weight / needed).clamp(0.0, 1.0) as f32,
                _ => 0.0,
            };
            ui.add(egui::ProgressBar::new(fraction).text(format!("{} of {}", weighs(listing, &proposal.weight, &coin), weighs(listing, &proposal.needed, &coin))));

            for approval in &proposal.approvals {
                ui.horizontal_wrapped(|ui| {
                    super::address(ui, &palette, &approval.account);

                    if ours.contains(&approval.account) {
                        theme::pill(ui, "yours", palette.accent);
                    }

                    ui.label(RichText::new(format!("{} · approved in epoch {}, counts until epoch {}", weighs(listing, &approval.weight, &coin), approval.epoch, approval.counts_until)).small().color(palette.muted));
                });
            }

            egui::CollapsingHeader::new(RichText::new("JSON").small().color(palette.muted)).id_salt(("governance proposal", index)).show(ui, |ui| {
                super::text_block(ui, &palette, &pretty(&proposal.json));
            });

            let Ok(change) = &proposal.change else {
                continue;
            };

            ui.horizontal_wrapped(|ui| {
                for from in &acting {
                    let counted = state.weights.get(&from.to_string()).and_then(|weight| weight.as_ref().ok()).and_then(|weight| weight.approved.as_ref());
                    let short = format::short(&from.to_string());
                    let by = if acting.len() > 1 { format!(" as {short}") } else { String::new() };

                    match counted == Some(change) {
                        true => {
                            if theme::quiet(ui, &palette, &format!("Withdraw{by}")).clicked() {
                                *chosen = Some(withdrawal(*from, listing.version, change, &coin));
                            }
                        }
                        false => {
                            if theme::primary(ui, &palette, &format!("Approve{by}"), true).clicked() {
                                *chosen = Some(approval(*from, listing, change.clone(), counted.is_some(), &coin));
                            }
                        }
                    }
                }
            });
        }

        if acting.is_empty() && !listing.proposals.is_empty() {
            ui.add_space(4.0);
            ui.label(RichText::new("None of this account's addresses, nor a multisig it follows, may approve: only the council's members, or finality's voters, do.").small().color(palette.muted));
        }
    });
}

/// Proposing a change: picking its kind, filling it in, and approving it.
fn propose(ui: &mut Ui, ctx: &mut Ctx, state: &mut GovernanceState, listing: &Listing, voters: &[Voter], chosen: &mut Option<Pending>) {
    let palette = *ctx.palette;
    let coin = ctx.coin.clone();
    let acting = acting(voters, &state.weights);
    let tip = ctx.chain.following().map(|(tip, _)| tip);

    if state.kind.is_empty() {
        state.kind = KINDS[0].to_string();
    }

    if state.filled.as_deref() != Some(state.kind.as_str()) {
        fill(&mut state.form, &state.kind, listing, coin.decimals);
        state.filled = Some(state.kind.clone());
        state.trouble = None;
    }

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Propose a change");
        ui.label(RichText::new("A change is proposed by approving it: others then approve the same change, and it is decided once its approvals weigh what it needs.").small().color(palette.muted));
        ui.add_space(4.0);

        egui::ComboBox::from_id_salt("governance-kind").selected_text(label(&state.kind)).width(ui.available_width()).show_ui(ui, |ui| {
            for kind in KINDS {
                ui.selectable_value(&mut state.kind, kind.to_string(), label(kind));
            }
        });
        ui.add_space(6.0);

        let form = &mut state.form;
        let line = |ui: &mut Ui, text: &mut String, hint: &str| {
            ui.add(egui::TextEdit::singleline(text).hint_text(hint).desired_width(f32::INFINITY));
        };
        let lines = |ui: &mut Ui, text: &mut String, hint: &str, rows: usize, mono: bool| {
            let edit = egui::TextEdit::multiline(text).hint_text(hint).desired_rows(rows).desired_width(f32::INFINITY);
            ui.add(if mono { edit.font(egui::TextStyle::Monospace) } else { edit });
        };

        match state.kind.as_str() {
            "name" => line(ui, &mut form.name, "the chain's new name, such as chainv4 mainnet"),
            "bootnodes" => {
                ui.label(RichText::new("One libp2p address a line, at most 32; none leaves the chain without bootnodes.").small().color(palette.muted));
                lines(ui, &mut form.bootnodes, "/dns4/boot.example/udp/4001/quic-v1", 4, true);
            }
            "upgrade" => {
                line(ui, &mut form.least, "least version that runs on, such as 0.2.0");
                line(ui, &mut form.from, "from block, ahead of the tip");
                ui.label(RichText::new(format!("Nodes older than the version stop before that block and say to update.{}", tip.map(|tip| format!(" The tip is block {}.", format::thousands(&tip.to_string()))).unwrap_or_default())).small().color(palette.muted));
            }
            "name_prices" => {
                ui.label(RichText::new(format!("What a year of a name costs, in {}, one price a line: names of 3 characters first, then 4, and so on; the last line prices every longer name.", coin.symbol)).small().color(palette.muted));
                lines(ui, &mut form.prices, "100\n20\n5", 5, true);
            }
            kind => {
                ui.label(RichText::new(format!("The new {} as JSON, filled in with what stands now: edit what should change.", label(kind).to_lowercase())).small().color(palette.muted));
                lines(ui, &mut form.json, "{ … }", 10, true);
            }
        }

        let parsed = proposed(&state.kind, &state.form, coin.decimals, tip);

        if let Err(why) = &parsed {
            ui.colored_label(palette.bad, why.as_str());
        }

        if let Ok(change) = &parsed {
            ui.label(RichText::new(describe(change, &coin)).color(palette.soft));
        }

        ui.add_space(6.0);

        match acting.as_slice() {
            [] => {
                ui.label(RichText::new("None of this account's addresses, nor a multisig it follows, may approve, so it cannot propose.").small().color(palette.muted));
            }
            [only] => state.proposer = Some(*only),
            _ => {
                if !state.proposer.is_some_and(|proposer| acting.contains(&proposer)) {
                    state.proposer = acting.first().copied();
                }

                let shown = |address: &Address| format!("{} · {}", super::scheme(address), format::short(&address.to_string()));
                egui::ComboBox::from_id_salt("governance-from").selected_text(state.proposer.as_ref().map(shown).unwrap_or_default()).width(ui.available_width()).show_ui(ui, |ui| {
                    for address in &acting {
                        ui.selectable_value(&mut state.proposer, Some(*address), shown(address));
                    }
                });
            }
        }

        if let (Some(from), Ok(change)) = (state.proposer.filter(|proposer| acting.contains(proposer)), parsed) {
            let counted = state.weights.get(&from.to_string()).and_then(|weight| weight.as_ref().ok()).is_some_and(|weight| weight.approved.is_some());

            if theme::primary(ui, &palette, "Review", true).clicked() {
                *chosen = Some(approval(from, listing, change, counted, &coin));
            }
        }

        if let Some(why) = &state.trouble {
            ui.colored_label(palette.bad, why.as_str());
        }
    });
}

/// The confirm screen: the change in words and as JSON, and what approving does. Some(true) to go
/// on, Some(false) to go back.
fn confirm(ui: &mut Ui, ctx: &mut Ctx, pending: &Pending) -> Option<bool> {
    let palette = *ctx.palette;

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, &pending.what);
        ui.add_space(4.0);
        ui.label(RichText::new(&pending.words).color(palette.ink).size(17.0));
        ui.add_space(6.0);
        super::row(ui, &palette, "From", |ui| super::address(ui, &palette, &pending.from.to_string()));

        if let Some(json) = &pending.json {
            ui.add_space(4.0);
            super::text_block(ui, &palette, json);
        }

        ui.add_space(6.0);
        ui.label(RichText::new(&pending.note).small().color(palette.soft));
        ui.add_space(10.0);

        let next = match pending.from {
            Address::Multisig(_) => "Continue to the multisig",
            _ => "Continue to Send",
        };

        ui.horizontal_wrapped(|ui| {
            let go = theme::primary(ui, &palette, next, true).clicked();
            let back = theme::quiet(ui, &palette, "Back").clicked();

            match (go, back) {
                (true, _) => Some(true),
                (_, true) => Some(false),
                _ => None,
            }
        })
        .inner
    })
}

/// The account's votes, as the light client proved them: which change each decided, if one did.
fn votes(ui: &mut Ui, ctx: &mut Ctx) {
    let palette = *ctx.palette;
    let genesis = ctx.chain.genesis.map(|genesis| genesis.to_string()).unwrap_or_default();
    let ours: Vec<String> = ctx.holders.iter().flat_map(|holder| holder.addresses()).map(|address| address.to_string()).collect();
    let mut votes: Vec<_> = ctx.settings.checked.values().filter(|checked| checked.vote.is_some() && checked.genesis == genesis && ours.contains(&checked.sender)).collect();

    if votes.is_empty() {
        return;
    }

    votes.sort_by_key(|checked| std::cmp::Reverse(checked.height));

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Your votes");

        for checked in votes.into_iter().take(6) {
            ui.horizontal_wrapped(|ui| {
                match checked.final_ {
                    true => theme::pill(ui, "Final", palette.good),
                    false => theme::pill(ui, "Proven", palette.accent),
                }
                .on_hover_text(format!("Its receipt is proven at block {}", checked.height));

                if let Some(Vote::Approved { decided: Some(version), .. }) = &checked.vote {
                    theme::pill(ui, &format!("Decided change {version}"), palette.good);
                }

                if !checked.done {
                    theme::pill(ui, "Failed", palette.bad);
                }

                ui.label(RichText::new(&checked.action).color(palette.soft));
            });
        }
    });
}

/// Every setting governance changes, as it stands.
fn in_force(ui: &mut Ui, ctx: &mut Ctx, listing: &Listing) {
    let palette = *ctx.palette;
    let coin = ctx.coin.clone();

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "In force");

        for setting in &listing.in_force {
            super::row(ui, &palette, label(&setting.kind), |ui| {
                let words = setting_change(setting).map_or_else(|_| setting.value.to_string(), |change| describe(&change, &coin));
                ui.label(RichText::new(words).color(palette.ink));
                ui.label(RichText::new(since(setting.decided)).small().color(palette.muted));
            });
        }
    });
}

/// The latest decisions.
fn recent(ui: &mut Ui, ctx: &mut Ctx, state: &GovernanceState) {
    let palette = *ctx.palette;
    let coin = ctx.coin.clone();

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Recent decisions");

        match &state.history {
            None => super::waiting(ui, &palette, "Asking the node…"),
            Some(Err(why)) => {
                ui.colored_label(palette.bad, format!("The node did not answer: {why}"));
            }
            Some(Ok(decisions)) if decisions.is_empty() => {
                ui.label(RichText::new("Nothing decided yet: every setting is genesis's.").color(palette.muted));
            }
            Some(Ok(decisions)) => {
                for decision in decisions {
                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        if decision.final_ {
                            theme::pill(ui, "Final", palette.good);
                        }

                        let words = match &decision.change {
                            Some(Ok(change)) => describe(change, &coin),
                            Some(Err(why)) => format!("a change this wallet does not read: {why}"),
                            None => "a change the node did not say".to_string(),
                        };
                        ui.label(RichText::new(format!("Change {} · {words}", decision.version)).color(palette.ink)).on_hover_text(format!("Decided by {}", decision.tx));
                    });
                    ui.label(
                        RichText::new(format!(
                            "Block {} · {}{}",
                            format::thousands(&decision.height.to_string()),
                            format::ago(decision.timestamp, ctx.now),
                            decision.sender.as_deref().map(|sender| format!(" · by {}", format::short(sender))).unwrap_or_default()
                        ))
                        .small()
                        .color(palette.muted),
                    );
                }
            }
        }
    });
}

/// GET /governance's answer, read.
fn listing(value: &Value) -> Result<Listing, String> {
    let whole = |name: &str| value[name].as_u64().ok_or(format!("it says no {name}"));
    let in_force = value["in_force"].as_object().ok_or("it says nothing of what is in force")?;
    let mut settings: Vec<Setting> = in_force.iter().map(|(kind, setting)| Setting { kind: kind.clone(), value: setting["value"].clone(), decided: setting["decided"].as_u64() }).collect();
    let rank = |kind: &str| IN_FORCE.iter().position(|known| *known == kind).unwrap_or(IN_FORCE.len());
    settings.sort_by_key(|setting| rank(&setting.kind));

    Ok(Listing {
        kind: value["kind"].as_str().ok_or("it names no kind")?.to_string(),
        epoch: whole("epoch")?,
        version: whole("version")?,
        approval_epochs: whole("approval_epochs")?,
        needed: value["needed"].as_str().ok_or("it says no needed")?.to_string(),
        members: value["members"].as_array().map(|members| members.iter().filter_map(Value::as_str).map(str::to_string).collect()),
        proposals: value["proposals"].as_array().map(|proposals| proposals.iter().map(proposal).collect::<Result<Vec<_>, _>>()).transpose()?.unwrap_or_default(),
        in_force: settings,
        finality: value["finality"].as_str().map(str::to_string),
        switch: value["switch"]["epoch"].as_u64().zip(value["switch"]["type"].as_str().map(str::to_string)),
    })
}

/// A proposal of the listing: the change's JSON, which the listing adds its weight, what it needs
/// and its approvals to, read as the change.
fn proposal(value: &Value) -> Result<Proposal, String> {
    let mut fields = value.as_object().ok_or("a proposal is not an object")?.clone();
    let text = |value: Option<Value>| value.and_then(|value| value.as_str().map(str::to_string)).unwrap_or_else(|| "0".to_string());
    let weight = text(fields.remove("weight"));
    let needed = text(fields.remove("needed"));
    let approvals = fields
        .remove("approvals")
        .and_then(|approvals| approvals.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .map(|approval| Approval {
            account: approval["account"].as_str().unwrap_or("?").to_string(),
            weight: approval["weight"].as_str().unwrap_or("0").to_string(),
            epoch: approval["epoch"].as_u64().unwrap_or(0),
            counts_until: approval["counts_until"].as_u64().unwrap_or(0),
        })
        .collect();
    let json = Value::Object(fields);

    Ok(Proposal { change: change_from_json(&json), json, weight, needed, approvals })
}

/// GET /governance/weight/{address}'s answer, read.
fn weight(value: &Value) -> Result<Weight, String> {
    let weight = value["weight"].as_str().and_then(|text| u256::from_str_radix(text, 10).ok()).ok_or("it says no weight")?;
    let approved = match &value["approved"] {
        Value::Null => None,
        change => Some(change_from_json(change)?),
    };

    Ok(Weight { weight, approved })
}

/// GET /governance/history's decisions, read.
fn decisions(value: &Value) -> Vec<Decision> {
    value["decisions"]
        .as_array()
        .map(|decisions| {
            decisions
                .iter()
                .filter_map(|decision| {
                    Some(Decision {
                        version: decision["version"].as_u64()?,
                        tx: decision["tx"].as_str()?.to_string(),
                        height: decision["height"].as_u64().unwrap_or(0),
                        timestamp: decision["timestamp"].as_u64().unwrap_or(0),
                        final_: decision["final"].as_bool().unwrap_or(false),
                        sender: decision["sender"].as_str().map(str::to_string),
                        change: decision.get("change").filter(|change| !change.is_null()).map(change_from_json),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A setting in force as the change that would set it.
fn setting_change(setting: &Setting) -> Result<Change, String> {
    change_from_json(&json!({ setting.kind.clone(): setting.value }))
}

/// Where a setting in force came from.
fn since(decided: Option<u64>) -> String {
    match decided {
        Some(version) => format!("decided in change {version}"),
        None => "genesis".to_string(),
    }
}

/// Approving a change from an address, at the listing's version.
fn approval(from: Address, listing: &Listing, change: Change, moves: bool, coin: &Coin) -> Pending {
    let moving = match moves {
        true => " This address's approval counts for another change now: approving this one moves it here, as an address approves one change at a time.",
        false => " An address approves one change at a time: approving another change later moves this approval there.",
    };

    Pending {
        from,
        words: describe(&change, coin),
        json: Some(pretty(&change_json(&change))),
        note: format!(
            "Approves at version {}: should another change be decided first, the version moves on and this approval no longer counts.{moving} It counts for {} epoch{}, and decides the change once the approvals weigh {}.",
            listing.version,
            listing.approval_epochs,
            plural(listing.approval_epochs),
            weighs(listing, &listing.needed, coin)
        ),
        tx: Tx::ApproveChange { version: listing.version, change },
        what: "Approve a change".to_string(),
    }
}

/// Withdrawing an address's approval of a change, at the listing's version.
fn withdrawal(from: Address, version: u64, change: &Change, coin: &Coin) -> Pending {
    Pending {
        from,
        tx: Tx::WithdrawChange { version },
        what: "Withdraw an approval".to_string(),
        words: describe(change, coin),
        json: Some(pretty(&change_json(change))),
        note: "Takes this address's approval off this change; the others' approvals stay. Approving another change instead would move the approval there.".to_string(),
    }
}

/// The change the propose form asks for, through change_from_json, so it is checked as the CLI's
/// approve-change checks it. tip is the light client's: an upgrade must start ahead of it.
fn proposed(kind: &str, form: &Form, decimals: u8, tip: Option<u64>) -> Result<Change, String> {
    let lines = |text: &str| text.lines().map(str::trim).filter(|line| !line.is_empty()).map(str::to_string).collect::<Vec<_>>();
    let value = match kind {
        "name" => json!({ "name": form.name.trim() }),
        "bootnodes" => json!({ "bootnodes": lines(&form.bootnodes) }),
        "upgrade" => {
            let from: u64 = form.from.trim().replace([',', '_'], "").parse().map_err(|_| "The block it starts from is a whole height.".to_string())?;

            if let Some(tip) = tip.filter(|tip| from <= *tip) {
                return Err(format!("It must start ahead of the tip, block {tip}."));
            }

            json!({ "upgrade": { "least": form.least.trim(), "from": from } })
        }
        "name_prices" => {
            let prices = lines(&form.prices)
                .iter()
                .enumerate()
                .map(|(index, line)| format::parse_units(line, decimals).ok_or(format!("Line {}: {line} is not an amount.", index + 1)))
                .collect::<Result<Vec<_>, _>>()?;

            json!({ "name_prices": prices })
        }
        kind => {
            let value: Value = serde_json::from_str(&form.json).map_err(|why| format!("Not JSON: {why}"))?;

            json!({ kind: value })
        }
    };

    change_from_json(&value)
}

/// The form filled in with what stands for a kind, so editing starts from it.
fn fill(form: &mut Form, kind: &str, listing: &Listing, decimals: u8) {
    let standing = |kind: &str| listing.in_force.iter().find(|setting| setting.kind == kind).map(|setting| setting.value.clone());
    let strings = |value: Option<Value>| value.and_then(|value| value.as_array().cloned()).unwrap_or_default().iter().filter_map(|item| item.as_str().map(str::to_string)).collect::<Vec<_>>();

    match kind {
        "name" => form.name = standing("name").and_then(|name| name.as_str().map(str::to_string)).unwrap_or_default(),
        "bootnodes" => form.bootnodes = strings(standing("bootnodes")).join("\n"),
        "upgrade" => {
            if let Some(upgrade) = standing("upgrade") {
                form.least = upgrade["least"].as_str().unwrap_or_default().to_string();
            }

            form.from.clear();
        }
        "name_prices" => form.prices = strings(standing("name_prices")).iter().map(|raw| format::units(raw, decimals).replace(',', "")).collect::<Vec<_>>().join("\n"),
        "switch_finality" => form.json = pretty(&json!({ "epoch": listing.epoch + 2, "finality": standing("finality").unwrap_or(Value::Null) })),
        kind => form.json = standing(kind).map_or_else(|| template(kind), |value| pretty(&value)),
    }
}

/// What a JSON kind starts from on a chain where none stands.
fn template(kind: &str) -> String {
    match kind {
        "collections" => pretty(&json!({ "nft": [], "multi": [] })),
        "directory" => pretty(&json!({ "gateway": null, "btc_gateway": null, "evm_gateways": [], "swap": null, "dex": null, "dex_router": null, "nft_factory": null })),
        "federation" => pretty(&json!({ "signers": [], "threshold": 1, "chain_depth": 0, "evm_depth": 0, "bitcoin_depth": 0 })),
        _ => "{}".to_string(),
    }
}

/// A kind of change, as the page names it.
fn label(kind: &str) -> &str {
    match kind {
        "name" => "The chain's name",
        "bootnodes" => "Bootnodes",
        "upgrade" => "Upgrade",
        "name_prices" => "Name prices",
        "consensus" => "Consensus",
        "fees" => "Fees",
        "reward" => "Block reward",
        "limits" => "Limits",
        "finality" => "Finality",
        "switch_finality" => "Finality's kind",
        "collections" => "Collections",
        "federation" => "Federation",
        "directory" => "Directory",
        other => other,
    }
}

/// A weight as the governance counts it: a council's in members, an electorate's in stake under
/// FFG and in signers under authority finality.
fn weighs(listing: &Listing, raw: &str, coin: &Coin) -> String {
    let count = raw.parse::<u64>().ok();

    match (listing.members.is_some(), listing.finality.as_deref()) {
        (true, _) => format!("{raw} vote{}", count.map_or("s", plural)),
        (false, Some("ffg")) => format!("{} of stake", format::amount(raw, coin.decimals, &coin.symbol)),
        (false, _) => format!("{raw} signer{}", count.map_or("s", plural)),
    }
}

/// A change in words.
pub fn describe(change: &Change, coin: &Coin) -> String {
    let amount = |raw: &u256| format::amount(&raw.to_string(), coin.decimals, &coin.symbol);
    let blocks = |count: u64| format!("{} block{}", format::thousands(&count.to_string()), plural(count));
    let finality = |config: &FinalityConfig| match config {
        FinalityConfig::Ffg(ffg) => format!(
            "FFG, votes {} deep, bonds of at least {}, at most {} validators",
            blocks(ffg.vote_depth),
            amount(&ffg.min_bond),
            ffg.max_validators
        ),
        FinalityConfig::Authority(authority) => format!(
            "authority, {} of {} signers to finalize and {} to change them",
            authority.threshold,
            authority.signers.len(),
            authority.change_threshold
        ),
        FinalityConfig::Depth(depth) => format!("final once {} deep", blocks(depth.depth)),
    };

    match change {
        Change::Name(name) => format!("Rename the chain to {name}"),
        Change::Bootnodes(bootnodes) if bootnodes.is_empty() => "Leave the chain without bootnodes".to_string(),
        Change::Bootnodes(bootnodes) => format!("Set {} bootnode{}: {}", bootnodes.len(), plural(bootnodes.len() as u64), bootnodes.join(", ")),
        Change::NamePrices(prices) => {
            let each: Vec<String> = prices
                .iter()
                .enumerate()
                .map(|(index, price)| format!("{}{} characters {}", index + 3, if index + 1 == prices.len() { " or more" } else { "" }, amount(price)))
                .collect();

            format!("Price a year of a name by its length: {}", each.join(", "))
        }
        Change::Upgrade(upgrade) => format!("Upgrade: nodes older than {} stop at block {}", upgrade.least, format::thousands(&upgrade.from.to_string())),
        Change::Consensus(consensus) => match consensus {
            ConsensusConfig::Authority { key, slot_ms } => format!("Consensus: one key, {}, makes a block every {}", format::short(&key.to_string()), millis(*slot_ms)),
            ConsensusConfig::Rotating { slot_ms } => format!("Consensus: finality's signers take turns, a block every {}", millis(*slot_ms)),
            ConsensusConfig::Staked { slot_ms, schedule } => format!("Consensus: bonded validators by stake, picked by {}, a block every {}", schedule.name(), millis(*slot_ms)),
            ConsensusConfig::ProofOfWork(pow) => format!("Consensus: proof of work on {}, a block every {}", pow.hash.name(), millis(pow.target_block_ms)),
        },
        Change::Fees(fees) => {
            let pricing = match &fees.pricing {
                PricingConfig::BaseFee(base) => format!("a base price from {}, at least {}", amount(&base.initial), amount(&base.min)),
                PricingConfig::Bid(bid) => format!("bids of at least {} a gas", amount(&bid.min_price)),
            };

            format!(
                "Fees: {} gas a block, {}‰ of each fee burned, {} gas schedule, {pricing}",
                format::thousands(&fees.block_gas_limit.to_string()),
                fees.burn_permille,
                fees.schedule.name()
            )
        }
        Change::Reward(reward) => match reward.halving_interval {
            Some(interval) => format!("Block reward: {}, halving every {}", amount(&reward.initial), blocks(interval)),
            None => format!("Block reward: {}, never halving", amount(&reward.initial)),
        },
        Change::Limits(limits) => format!(
            "Limits: {} fuel and {} of memory a transaction, {}",
            format::thousands(&limits.tx_limits.fuel.to_string()),
            bytes(limits.tx_limits.memory as u64),
            limits.max_block_bytes.map_or_else(|| "blocks of any size".to_string(), |most| format!("blocks of at most {}", bytes(most)))
        ),
        Change::Finality(config) => format!("Finality: {}", finality(config)),
        Change::SwitchFinality { epoch, config } => format!("Switch finality to {} from epoch {epoch}: {}", config.name(), finality(config)),
        Change::Collections(functions) => format!(
            "Collections: {} NFT function{} and {} multi-token function{} make one",
            functions.nft.len(),
            plural(functions.nft.len() as u64),
            functions.multi.len(),
            plural(functions.multi.len() as u64)
        ),
        Change::Federation(federation) => format!(
            "Federation: {} of {} signers, waiting {} here, {} on EVM chains and {} on Bitcoin",
            federation.threshold,
            federation.signers.len(),
            blocks(federation.chain_depth),
            blocks(federation.evm_depth),
            blocks(u64::from(federation.bitcoin_depth))
        ),
        Change::Directory(directory) => {
            let named = [
                (directory.gateway.is_some(), "the gateway".to_string()),
                (directory.btc_gateway.is_some(), "the Bitcoin gateway".to_string()),
                (!directory.evm_gateways.is_empty(), format!("{} EVM gateway{}", directory.evm_gateways.len(), plural(directory.evm_gateways.len() as u64))),
                (directory.swap.is_some(), "the swap contract".to_string()),
                (directory.dex.is_some(), "the DEX".to_string()),
                (directory.dex_router.is_some(), "the DEX's EVM router".to_string()),
                (directory.nft_factory.is_some(), "the NFT factory".to_string()),
            ];
            let named: Vec<String> = named.into_iter().filter(|(set, _)| *set).map(|(_, what)| what).collect();

            match named.is_empty() {
                true => "Directory: announce nothing".to_string(),
                false => format!("Directory: announce {}", named.join(", ")),
            }
        }
    }
}

fn plural(count: u64) -> &'static str {
    if count == 1 { "" } else { "s" }
}

/// Milliseconds, as seconds where they are whole.
fn millis(ms: u64) -> String {
    match ms % 1000 {
        0 => format!("{}s", ms / 1000),
        _ => format!("{ms} ms"),
    }
}

/// Bytes, in the biggest binary unit they are whole in.
fn bytes(count: u64) -> String {
    match count {
        0 => "0 bytes".to_string(),
        count if count % (1 << 20) == 0 => format!("{} MiB", count >> 20),
        count if count % (1 << 10) == 0 => format!("{} KiB", count >> 10),
        count => format!("{count} bytes"),
    }
}

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}


#[cfg(test)]
mod tests {
    use {
        super::*,
        chainv4::consensus::{PowConfig, Schedule},
        chainv4::params::fees::{BaseFee, Bid, FeeConfig, ScheduleConfig, Weighted},
        chainv4::params::finality::{DepthConfig, FfgConfig},
        chainv4::params::{BlockLimits, Limits, Reward},
        chainv4::registry::collections::CollectionFunctions,
        chainv4::registry::directory::{Directory, EvmGateway},
        chainv4::registry::federation::Federation,
        chainv4::structs::hash::HashType,
        chainv4::structs::version::{Upgrade, Version},
    };

    fn coin() -> Coin {
        Coin { symbol: "DEV".to_string(), decimals: 18 }
    }

    fn member(byte: u8) -> Address {
        Address::Ed25519([byte; 32])
    }

    /// GET /governance on a council of two, one deciding, with the chain renamed and a change of
    /// bootnodes open, as the node's own test serves it.
    fn council() -> Value {
        json!({
            "kind": "council",
            "epoch": 3,
            "version": 2,
            "approval_epochs": 16,
            "needed": "1",
            "members": [member(1).to_string(), member(2).to_string()],
            "proposals": [{
                "bootnodes": ["/dns4/boot.example/udp/4001/quic-v1"],
                "weight": "1",
                "needed": "1",
                "approvals": [{ "account": member(2).to_string(), "weight": "1", "epoch": 3, "counts_until": 18 }],
            }, {
                "name": "",
                "weight": "0",
                "needed": "1",
                "approvals": [],
            }],
            "in_force": {
                "reward": { "value": { "initial": "5000000000000000000", "halving_interval": null }, "decided": null },
                "name": { "value": "second", "decided": 1 },
                "bootnodes": { "value": [], "decided": 2 },
                "limits": { "value": { "tx_limits": { "fuel": 10000000, "memory": 67108864 }, "max_block_bytes": null }, "decided": null },
                "consensus": { "value": { "type": "rotating", "slot_ms": 2000 }, "decided": null },
            },
            "active": null,
            "decided": { "type": "rotating", "slot_ms": 2000 },
            "bootnodes": [],
            "finality": "authority",
            "switch": { "epoch": 9, "type": "ffg" },
        })
    }

    #[test]
    fn the_listing_reads_into_the_model_with_each_proposal_stripped_to_its_change() {
        let listing = listing(&council()).expect("reads");

        assert_eq!((listing.kind.as_str(), listing.epoch, listing.version, listing.approval_epochs, listing.needed.as_str()), ("council", 3, 2, 16, "1"));
        assert_eq!(listing.members, Some(vec![member(1).to_string(), member(2).to_string()]));
        assert_eq!(listing.switch, Some((9, "ffg".to_string())));

        let open = &listing.proposals[0];
        assert_eq!(open.json, json!({ "bootnodes": ["/dns4/boot.example/udp/4001/quic-v1"] }), "weight, needed and approvals are the listing's, not the change's");
        assert_eq!(open.change, Ok(Change::Bootnodes(vec!["/dns4/boot.example/udp/4001/quic-v1".to_string()])));
        assert_eq!((open.weight.as_str(), open.needed.as_str()), ("1", "1"));
        assert_eq!(open.approvals, vec![Approval { account: member(2).to_string(), weight: "1".to_string(), epoch: 3, counts_until: 18 }]);

        // A change that does not read says why, as change_from_json does, and the rest still show.
        assert!(listing.proposals[1].change.as_ref().is_err_and(|why| why.starts_with("name:")), "{:?}", listing.proposals[1].change);

        // What stands, in the API's order, each genesis's or decided by a change.
        let kinds: Vec<&str> = listing.in_force.iter().map(|setting| setting.kind.as_str()).collect();
        assert_eq!(kinds, ["consensus", "name", "bootnodes", "reward", "limits"]);
        assert_eq!(since(listing.in_force[1].decided), "decided in change 1");
        assert_eq!(since(listing.in_force[0].decided), "genesis");
        assert_eq!(setting_change(&listing.in_force[1]), Ok(Change::Name("second".to_string())));
        assert_eq!(setting_change(&listing.in_force[0]), Ok(Change::Consensus(ConsensusConfig::Rotating { slot_ms: 2000 })));
    }

    #[test]
    fn an_electorate_has_no_members_and_weighs_in_stake_under_ffg_or_signers_under_authority() {
        let mut value = council();
        value["kind"] = "electorate".into();
        value["members"] = Value::Null;
        value["switch"] = Value::Null;
        value["finality"] = "ffg".into();
        value["needed"] = "1500000000000000000".into();
        let mut listing = listing(&value).unwrap();

        assert_eq!((listing.members.clone(), listing.switch.clone()), (None, None));
        assert_eq!(weighs(&listing, &listing.needed, &coin()), "1.5 DEV of stake");

        listing.finality = Some("authority".to_string());
        assert_eq!(weighs(&listing, "1", &coin()), "1 signer");

        listing.members = Some(Vec::new());
        assert_eq!(weighs(&listing, "2", &coin()), "2 votes");
        assert!(super::listing(&json!({ "kind": "council" })).is_err(), "an answer missing its parts does not read");
    }

    #[test]
    fn weights_say_which_accounts_may_vote_and_what_their_approval_counts_for() {
        let voting = weight(&json!({ "account": member(1).to_string(), "weight": "1", "needed": "1", "version": 2, "approved": { "name": "third" } })).unwrap();
        let barred = weight(&json!({ "account": member(3).to_string(), "weight": "0", "needed": "1", "version": 2, "approved": null })).unwrap();

        assert!(voting.may_vote());
        assert_eq!(voting.approved, Some(Change::Name("third".to_string())));
        assert!(!barred.may_vote());
        assert_eq!(barred.approved, None);
        assert!(weight(&json!({ "weight": 1 })).is_err());

        // Only the addresses this page sends for, that may vote, act: another account's waits for
        // the wallet to switch to it, and one the node did not answer for, or barred, never acts.
        let multisig = Address::Multisig([9; 32]);
        let voters = [
            Voter { address: member(1), account: "Main".to_string(), here: true },
            Voter { address: member(3), account: "Main".to_string(), here: true },
            Voter { address: member(2), account: "Cold".to_string(), here: false },
            Voter { address: multisig, account: "Multisig".to_string(), here: true },
            Voter { address: member(4), account: "Main".to_string(), here: true },
        ];
        let weights = HashMap::from([
            (member(1).to_string(), Ok(voting.clone())),
            (member(2).to_string(), Ok(voting.clone())),
            (member(3).to_string(), Ok(barred)),
            (multisig.to_string(), Ok(voting)),
            (member(4).to_string(), Err("not found".to_string())),
        ]);

        assert_eq!(acting(&voters, &weights), [member(1), multisig]);
    }

    #[test]
    fn approving_and_withdrawing_build_their_transactions_at_the_current_version() {
        let listing = listing(&council()).unwrap();
        let change = Change::Bootnodes(vec!["/dns4/boot.example/udp/4001/quic-v1".to_string()]);
        let approve = approval(member(1), &listing, change.clone(), true, &coin());

        assert_eq!(approve.tx, Tx::ApproveChange { version: 2, change: change.clone() });
        assert_eq!(approve.from, member(1));
        assert_eq!(approve.words, "Set 1 bootnode: /dns4/boot.example/udp/4001/quic-v1");
        assert!(approve.note.contains("moves it here"), "moving an approval is approving another change: {}", approve.note);
        assert!(approve.note.contains("version 2"));
        assert_eq!(approve.json.as_deref().map(|json| serde_json::from_str::<Value>(json).unwrap()), Some(change_json(&change)));
        assert!(approval(member(1), &listing, change.clone(), false, &coin()).note.contains("approving another change later moves this approval there"));

        let withdraw = withdrawal(member(1), listing.version, &change, &coin());
        assert_eq!(withdraw.tx, Tx::WithdrawChange { version: 2 });
        assert_eq!(withdraw.what, "Withdraw an approval");
    }

    #[test]
    fn the_propose_forms_simple_fields_make_their_change_and_refuse_bad_input() {
        let form = |edit: fn(&mut Form)| {
            let mut form = Form::default();
            edit(&mut form);
            form
        };

        assert_eq!(proposed("name", &form(|form| form.name = " chainv4 mainnet ".to_string()), 18, None), Ok(Change::Name("chainv4 mainnet".to_string())));
        assert!(proposed("name", &form(|form| form.name = String::new()), 18, None).is_err());
        assert!(proposed("name", &form(|form| form.name = "x".repeat(41)), 18, None).is_err());

        let two = form(|form| form.bootnodes = "/ip4/203.0.113.7/udp/4001/quic-v1\n\n  /dns4/boot.example/udp/4001/quic-v1 \n".to_string());
        assert_eq!(
            proposed("bootnodes", &two, 18, None),
            Ok(Change::Bootnodes(vec!["/ip4/203.0.113.7/udp/4001/quic-v1".to_string(), "/dns4/boot.example/udp/4001/quic-v1".to_string()]))
        );
        assert_eq!(proposed("bootnodes", &Form::default(), 18, None), Ok(Change::Bootnodes(Vec::new())), "none leaves the chain without");
        assert!(proposed("bootnodes", &form(|form| form.bootnodes = "not an address".to_string()), 18, None).is_err());

        let upgrade = form(|form| {
            form.least = "0.2.0".to_string();
            form.from = "150,000".to_string();
        });
        assert_eq!(proposed("upgrade", &upgrade, 18, Some(100)), Ok(Change::Upgrade(Upgrade { least: Version::new(0, 2, 0), from: 150_000 })));
        assert!(proposed("upgrade", &upgrade, 18, Some(150_000)).is_err_and(|why| why.contains("ahead of the tip")));
        assert!(proposed("upgrade", &form(|form| form.from = "soon".to_string()), 18, None).is_err());
        assert!(proposed("upgrade", &form(|form| {
            form.least = "two".to_string();
            form.from = "9".to_string();
        }), 18, None)
        .is_err_and(|why| why.starts_with("upgrade.least")));

        let prices = form(|form| form.prices = "100\n20\n0.5\n".to_string());
        assert_eq!(
            proposed("name_prices", &prices, 18, None),
            Ok(Change::NamePrices(vec![u256::new(100_000_000_000_000_000_000), u256::new(20_000_000_000_000_000_000), u256::new(500_000_000_000_000_000)]))
        );
        assert!(proposed("name_prices", &form(|form| form.prices = "100\nfree".to_string()), 18, None).is_err_and(|why| why.contains("Line 2")));
        assert!(proposed("name_prices", &Form::default(), 18, None).is_err(), "a price for every length");

        // Every other kind is JSON, its errors change_from_json's.
        assert_eq!(proposed("reward", &form(|form| form.json = r#"{ "initial": "7", "halving_interval": 100 }"#.to_string()), 18, None), Ok(Change::Reward(Reward { initial: u256::new(7), halving_interval: Some(100) })));
        assert!(proposed("reward", &form(|form| form.json = "{ initial".to_string()), 18, None).is_err_and(|why| why.starts_with("Not JSON")));
        assert!(proposed("reward", &form(|form| form.json = r#"{ "initial": "7" }"#.to_string()), 18, None).is_err());
    }

    #[test]
    fn the_form_starts_from_what_stands() {
        let listing = listing(&council()).unwrap();
        let mut form = Form::default();

        fill(&mut form, "name", &listing, 18);
        assert_eq!(form.name, "second");

        fill(&mut form, "reward", &listing, 18);
        assert_eq!(proposed("reward", &form, 18, None), Ok(Change::Reward(Reward { initial: u256::new(5_000_000_000_000_000_000), halving_interval: None })), "unedited, it is what stands");

        fill(&mut form, "consensus", &listing, 18);
        assert_eq!(proposed("consensus", &form, 18, None), Ok(Change::Consensus(ConsensusConfig::Rotating { slot_ms: 2000 })));

        let mut priced = council();
        priced["in_force"]["name_prices"] = json!({ "value": ["100000000000000000000", "1500000000000000000"], "decided": null });
        fill(&mut form, "name_prices", &listing_of(&priced), 18);
        assert_eq!(form.prices, "100\n1.5");

        fill(&mut form, "directory", &listing, 18);
        assert_eq!(proposed("directory", &form, 18, None).map(|change| describe(&change, &coin())), Ok("Directory: announce nothing".to_string()), "none stands: a template");
    }

    /// A compressed secp256k1 key, as a federation's signers are.
    fn compressed(byte: u8) -> [u8; 33] {
        use chainv4::crypto::keypair::{Keypair, PrivateKey, PublicKey};

        match Keypair::from_private(PrivateKey::Bitcoin([byte; 32])).public() {
            PublicKey::Bitcoin(bytes) => bytes,
            _ => unreachable!("a bitcoin key"),
        }
    }

    fn listing_of(value: &Value) -> Listing {
        listing(value).unwrap()
    }

    #[test]
    fn every_kind_of_change_reads_in_words() {
        let ffg = FfgConfig {
            checkpoint_interval: 8,
            vote_depth: 3,
            min_bond: u256::new(500_000_000_000_000_000_000),
            activation_delay: 2,
            withdrawal_delay: 64,
            reporter_divisor: 16,
            max_validators: 1_000,
            vote_reward: u256::new(1),
            max_proofs_per_block: 4,
            validators: Vec::new(),
            leak: None,
        };
        let changes = [
            (Change::Name("chainv4 mainnet".to_string()), "Rename the chain to chainv4 mainnet"),
            (Change::Bootnodes(Vec::new()), "Leave the chain without bootnodes"),
            (Change::NamePrices(vec![u256::new(100_000_000_000_000_000_000), u256::new(5_000_000_000_000_000_000)]), "Price a year of a name by its length: 3 characters 100 DEV, 4 or more characters 5 DEV"),
            (Change::Upgrade(Upgrade { least: Version::new(0, 2, 0), from: 150_000 }), "Upgrade: nodes older than 0.2.0 stop at block 150,000"),
            (Change::Consensus(ConsensusConfig::Rotating { slot_ms: 2000 }), "Consensus: finality's signers take turns, a block every 2s"),
            (Change::Consensus(ConsensusConfig::Staked { slot_ms: 1500, schedule: Schedule::Draw }), "Consensus: bonded validators by stake, picked by draw, a block every 1500 ms"),
            (
                Change::Consensus(ConsensusConfig::ProofOfWork(PowConfig { hash: HashType::Sha256, target_block_ms: 10_000, initial_difficulty: 1, min_difficulty: 1 })),
                "Consensus: proof of work on sha256, a block every 10s",
            ),
            (
                Change::Consensus(ConsensusConfig::Authority { key: chainv4::crypto::keypair::Keypair::development().public(), slot_ms: 1000 }),
                "Consensus: one key, ",
            ),
            (
                Change::Fees(FeeConfig {
                    block_gas_limit: 50_000_000,
                    burn_permille: 500,
                    schedule: ScheduleConfig::Weighted(Weighted { per_tx: 1_000, per_byte: 10, per_code_byte: 20, fuel_per_gas: 1 }),
                    pricing: PricingConfig::Bid(Bid { min_price: u256::new(7) }),
                }),
                "Fees: 50,000,000 gas a block, 500‰ of each fee burned, weighted gas schedule, bids of at least 0.000000000000000007 DEV a gas",
            ),
            (
                Change::Fees(FeeConfig {
                    block_gas_limit: 1_000,
                    burn_permille: 0,
                    schedule: ScheduleConfig::Weighted(Weighted { per_tx: 1, per_byte: 1, per_code_byte: 1, fuel_per_gas: 1 }),
                    pricing: PricingConfig::BaseFee(BaseFee { initial: u256::new(10), min: u256::new(1), elasticity: 2, change_denominator: 8 }),
                }),
                "a base price from",
            ),
            (Change::Reward(Reward { initial: u256::new(25_000_000_000_000_000_000), halving_interval: Some(210_000) }), "Block reward: 25 DEV, halving every 210,000 blocks"),
            (Change::Reward(Reward { initial: u256::new(1_000_000_000_000_000_000), halving_interval: None }), "Block reward: 1 DEV, never halving"),
            (
                Change::Limits(BlockLimits { tx_limits: Limits { fuel: 10_000_000, memory: 64 << 20 }, max_block_bytes: Some(2 << 20) }),
                "Limits: 10,000,000 fuel and 64 MiB of memory a transaction, blocks of at most 2 MiB",
            ),
            (Change::Finality(FinalityConfig::Depth(DepthConfig { depth: 6 })), "Finality: final once 6 blocks deep"),
            (Change::Finality(FinalityConfig::Ffg(ffg.clone())), "Finality: FFG, votes 3 blocks deep, bonds of at least 500 DEV, at most 1000 validators"),
            (Change::SwitchFinality { epoch: 40, config: FinalityConfig::Ffg(ffg) }, "Switch finality to ffg from epoch 40: FFG, votes 3 blocks deep"),
            (Change::Collections(CollectionFunctions { nft: vec!["owner_of".to_string()], multi: Vec::new() }), "Collections: 1 NFT function and 0 multi-token functions make one"),
            (
                Change::Federation(Federation { signers: vec![compressed(2), compressed(3), compressed(4)], threshold: 2, chain_depth: 10, evm_depth: 12, bitcoin_depth: 1 }),
                "Federation: 2 of 3 signers, waiting 10 blocks here, 12 blocks on EVM chains and 1 block on Bitcoin",
            ),
            (
                Change::Directory(Directory {
                    gateway: Some(Address::Contract([1; 32])),
                    btc_gateway: None,
                    evm_gateways: vec![EvmGateway { chain_id: 1, gateway: [3; 20] }],
                    swap: None,
                    dex: Some(Address::Contract([2; 32])),
                    dex_router: None,
                    nft_factory: None,
                }),
                "Directory: announce the gateway, 1 EVM gateway, the DEX",
            ),
        ];
        let mut kinds = std::collections::BTreeSet::new();

        for (change, said) in changes {
            let words = describe(&change, &coin());

            assert!(words.starts_with(said) || words.contains(said), "{words:?} does not say {said:?}");
            // What the page describes is what change_from_json reads back from its JSON.
            assert_eq!(change_from_json(&change_json(&change)), Ok(change.clone()), "{}", change.name());
            kinds.insert(change.name());
        }

        assert_eq!(kinds.len(), KINDS.len(), "every kind the form lists is described");
        assert!(KINDS.iter().all(|kind| kinds.contains(kind)));
    }

    #[test]
    fn the_history_reads_newest_first_with_each_change() {
        let history = json!({
            "version": 2,
            "from": 1,
            "decisions": [
                { "version": 2, "tx": "0x02", "block": "0xb2", "height": 2, "timestamp": 2000, "confirmations": 2, "final": false, "sender": member(1).to_string(), "kind": "bootnodes", "change": { "bootnodes": [] } },
                { "version": 1, "tx": "0x01", "block": "0xb1", "height": 1, "timestamp": 1000, "confirmations": 3, "final": true, "sender": member(1).to_string(), "kind": "name", "change": { "name": "second" } },
                { "version": 0, "tx": "0x00", "height": 0, "sender": null },
            ],
        });
        let decisions = decisions(&history);

        assert_eq!(decisions.len(), 3);
        assert_eq!((decisions[0].version, decisions[0].tx.as_str(), decisions[0].final_), (2, "0x02", false));
        assert_eq!(decisions[1].change, Some(Ok(Change::Name("second".to_string()))));
        assert_eq!((decisions[2].change.clone(), decisions[2].sender.clone()), (None, None), "a decision the node did not say the change of");
    }

    #[test]
    fn an_answer_lands_in_its_place_and_a_chain_without_governance_says_so() {
        let mut state = GovernanceState::default();
        state.answered(&Ask::Governance, &Err(Failure { status: NOT_FOUND, message: "this chain has no governance".to_string() }));

        assert!(matches!(&state.listing, Some(Err(failure)) if failure.status == NOT_FOUND));

        state.answered(&Ask::Governance, &Ok(council()));
        state.answered(&Ask::GovernanceWeight(member(1).to_string()), &Ok(json!({ "weight": "1", "version": 2, "approved": null })));
        state.answered(&Ask::GovernanceHistory, &Ok(json!({ "decisions": [] })));

        assert_eq!(state.listing.as_ref().and_then(|listing| listing.as_ref().ok()).map(|listing| listing.version), Some(2));
        assert!(state.weights[&member(1).to_string()].as_ref().is_ok_and(Weight::may_vote));
        assert_eq!(state.history, Some(Ok(Vec::new())));

        state.answered(&Ask::Governance, &Ok(json!({ "kind": "council" })));
        assert!(matches!(&state.listing, Some(Err(failure)) if failure.message.starts_with("its answer does not read")));
    }
}
