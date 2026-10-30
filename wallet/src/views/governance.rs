//! Governance: the chain's settings that its council, or finality's own voters, change on chain.
//! What a change needs and who may approve, the changes open and their approvals, found by search,
//! kind and order, approving one or withdrawing an approval, proposing a change of any kind by a
//! form of its fields (form.rs), what stands, and every decision, a page at a time.
//!
//! What the node says of governance is shown as its word: GET /governance, /governance/history and
//! /governance/weight/{address}. Nothing signed rests on it but the version an approval names, its
//! change's kind's, which the chain checks: an approval for a version that moved on fails and
//! changes nothing. Each kind of change has a ballot and a version of its own, so an account backs
//! one change of each kind at a time. Each change is read with chainv4's own change_from_json,
//! which checks it as the CLI's approve-change does, before it is signed; a partial one is laid
//! over what stands first, with params::json::over, as the CLI does. Approvals are signed and sent
//! on the Send page, as any other transaction, or proposed to a multisig's members on its page; once
//! in a block, the light client proves their receipts, whose outcome says which change an approval
//! decided.

mod form;
mod schema;

use {
    super::multisig::MultisigState,
    super::send::{Prepared, SendState},
    super::{Ctx, Page},
    crate::app::Coin,
    crate::format,
    crate::history::{Vote, kind_words},
    crate::node::{Ask, Failure},
    crate::theme,
    chainv4_ui::ballots::{self, Counted, Decided, Finder, Item, PER_PAGE},
    chainv4::consensus::ConsensusConfig,
    chainv4::governance::{Change, ChangeKind},
    chainv4::params::fees::PricingConfig,
    chainv4::params::finality::FinalityConfig,
    chainv4::params::json::{change_from_json, change_json, over},
    form::{Built, Diff, Draft, Env},
    chainv4::structs::address::Address,
    chainv4::structs::tx::Tx,
    chainv4::structs::u256::u256,
    egui::{RichText, Ui},
    serde_json::{Value, json},
    std::collections::{BTreeMap, HashMap},
};

/// How often the node is asked again, in milliseconds: governance moves at most once a block.
const REFRESH: u64 = 15_000;

/// What the API answers on a chain without governance.
const NOT_FOUND: u16 = 404;

/// The kinds of change, in the order the propose form lists them.
const KINDS: [&str; 13] = ["name", "bootnodes", "upgrade", "name_prices", "consensus", "fees", "reward", "limits", "finality", "switch_finality", "collections", "federation", "directory"];

/// How tall the kinds' list opens: all of them on most phones, without a scroll to find.
const KIND_LIST_HEIGHT: f32 = 620.0;

/// The kinds the propose form takes in part: a JSON object naming only some fields, laid over what
/// stands, as the CLI's approve-change takes them.
const PARTIAL: [&str; 6] = ["fees", "reward", "limits", "finality", "federation", "directory"];

/// The settings in force, in the order the API lists them.
const IN_FORCE: [&str; 12] = ["consensus", "name", "bootnodes", "name_prices", "collections", "federation", "directory", "fees", "reward", "limits", "finality", "upgrade"];

/// GET /governance, read.
#[derive(Clone, Debug, PartialEq)]
pub struct Listing {
    /// council or electorate.
    pub kind: String,
    pub epoch: u64,
    /// Decisions so far, of every kind: what the history counts.
    pub version: u64,
    /// Each kind's own version: how many changes of it were decided. What an approval names.
    pub versions: BTreeMap<ChangeKind, u64>,
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
    /// Its kind, as the node names it, and that kind's version its approvals are at.
    pub kind: String,
    pub version: Option<u64>,
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

impl Counted for Decision {
    fn version(&self) -> u64 {
        self.version
    }
}

impl Listing {
    /// The version an approval of a change of this kind names.
    pub fn version_of(&self, kind: ChangeKind) -> u64 {
        self.versions.get(&kind).copied().unwrap_or_default()
    }
}

/// GET /governance/weight/{address}, read.
#[derive(Clone, Debug, PartialEq)]
pub struct Weight {
    pub weight: u256,
    /// The address's approvals that count, one at most a kind, in kind order.
    pub approved: Vec<Approved>,
}

/// An address's approval that counts: the change, its kind, and the kind's version it names.
#[derive(Clone, Debug, PartialEq)]
pub struct Approved {
    pub kind: ChangeKind,
    pub version: u64,
    pub change: Change,
}

impl Weight {
    /// Whether the address may approve at all.
    pub fn may_vote(&self) -> bool {
        self.weight > u256::ZERO
    }

    /// Its approval of a change of this kind, if one counts.
    pub fn approved_in(&self, kind: ChangeKind) -> Option<&Approved> {
        self.approved.iter().find(|approved| approved.kind == kind)
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
    /// What it moves against what stands, field by field.
    changes: Vec<Diff>,
    /// The change's JSON, pretty.
    json: Option<String>,
    /// What it does to the address's approval.
    note: String,
}

/// The propose form: a field for each setting, or, under Advanced, the kind's JSON.
#[derive(Clone, Debug, Default, PartialEq)]
struct Form {
    draft: Draft,
    /// Whether Advanced is open, and what is typed there.
    advanced: bool,
    json: String,
}

#[derive(Default)]
pub struct GovernanceState {
    /// The node the answers came from: they go once the wallet talks to another.
    node: String,
    listing: Option<Result<Listing, Failure>>,
    /// The decisions read so far, what is asked of them and of the open changes, whether a page is on
    /// its way, and why the last one did not come.
    decided: Decided<Decision>,
    past: Finder,
    open: Finder,
    reading: bool,
    past_trouble: Option<String>,
    weights: HashMap<String, Result<Weight, String>>,
    asked: Option<u64>,
    /// The kind proposed, what stood for it when the form was filled, and the form.
    kind: String,
    filled: Option<(String, Option<Value>)>,
    form: Form,
    /// The address to propose from.
    proposer: Option<Address>,
    /// Whether the review open came from the form, and whether the page goes back to the form next
    /// it shows, rather than to its top: Back from the review it led to.
    reviewing: bool,
    back_to_form: bool,
    pending: Option<Pending>,
    trouble: Option<String>,
}

impl GovernanceState {
    /// Takes the node's answer to something this page asked.
    pub fn answered(&mut self, ask: &Ask, result: &Result<Value, Failure>) {
        let unreadable = |why: String| Failure { status: 0, message: format!("its answer does not read: {why}") };

        match ask {
            Ask::Governance => self.listing = Some(result.clone().and_then(|value| listing(&value).map_err(unreadable))),
            Ask::GovernanceHistory { before } => {
                self.reading = false;

                match result {
                    Ok(value) => {
                        self.decided.take(value["version"].as_u64().unwrap_or_default(), value["from"].as_u64().unwrap_or(1), decisions(value), *before);
                        self.past_trouble = None;
                    }
                    Err(failure) => self.past_trouble = Some(failure.message.clone()),
                }
            }
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
            ctx.node.ask(Ask::GovernanceHistory { before: None });
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
        // The way to the node in trouble: said at the top of every page, with Retry.
        Some(Err(failure)) if failure.is_trouble() => {
            theme::card(ui, &palette, |ui| super::waiting(ui, &palette, "Waiting for the node to answer…"));

            return;
        }
        // An answer that says something of what was asked: said here, as nowhere else says it.
        Some(Err(failure)) => {
            let sentence = failure.sentence();
            let asking = ctx.node.asking(&Ask::Governance);

            if theme::retry(ui, &palette, &sentence, asking) {
                ctx.node.retry(Ask::Governance);
            }

            return;
        }
        Some(Ok(listing)) => listing.clone(),
    };

    if let Some(pending) = state.pending.clone() {
        match confirm(ui, ctx, &pending) {
            Some(true) => {
                state.pending = None;
                state.reviewing = false;
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
            Some(false) => {
                state.back_to_form = std::mem::take(&mut state.reviewing);
                state.pending = None;
            }
            None => {}
        }

        return;
    }

    let mut chosen = None;

    header(ui, ctx, state, &listing, &voters, &mut chosen);
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
fn header(ui: &mut Ui, ctx: &mut Ctx, state: &GovernanceState, listing: &Listing, voters: &[Voter], chosen: &mut Option<Pending>) {
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
        super::row(ui, &palette, "Decisions", |ui| {
            ui.label(format!("{} so far, of every kind", listing.version));
        });
        super::row(ui, &palette, "Versions", |ui| {
            ui.vertical(|ui| {
                ui.label(versions_line(listing));
                ui.label(RichText::new("Each kind of change has a ballot and a version of its own: an approval names its kind's, and lapses only once a change of that kind is decided.").small().color(palette.muted));
            });
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
                        // Whole, on the next line when it does not fit: never "may" here and "not vote" there.
                        ui.add(egui::Label::new(RichText::new("may not vote").small().color(palette.muted)).extend());
                    }
                }
            });

            let Some(Ok(weight)) = state.weights.get(&address) else {
                continue;
            };

            // Its approvals, one a kind, each withdrawn on its own.
            for approved in &weight.approved {
                theme::end_row(
                    ui,
                    |ui| {
                        ui.add(egui::Label::new(RichText::new(format!("Approves, in {} v{}: {}", kind_label(approved.kind).to_lowercase(), approved.version, describe(&approved.change, &coin))).small().color(palette.muted)).wrap());
                    },
                    |ui| {
                        if voter.here && weight.may_vote() && theme::quiet(ui, &palette, "Withdraw").clicked() {
                            *chosen = Some(withdrawal(voter.address, approved, &coin));
                        }
                    },
                );
            }
        }
    });
}

/// The changes open, found by search, kind and order, each with its approvals, and approving or
/// withdrawing.
fn proposals(ui: &mut Ui, ctx: &mut Ctx, state: &mut GovernanceState, listing: &Listing, voters: &[Voter], chosen: &mut Option<Pending>) {
    let palette = *ctx.palette;
    let coin = ctx.coin.clone();
    let acting = acting(voters, &state.weights);
    let ours: Vec<String> = voters.iter().map(|voter| voter.address.to_string()).collect();
    let items: Vec<Item> = listing.proposals.iter().map(|proposal| open_item(proposal, &coin)).collect();

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Open changes");

        if listing.proposals.is_empty() {
            ui.label(RichText::new("No change has approvals that still count. Propose one below.").color(palette.muted));
        }

        ballots::controls(ui, &palette, &mut state.open, "Search: kind, address or value", &items, &ballots::OPEN_ORDERS, &kind_name);

        let found = ballots::select(&items, &state.open);

        if found.is_empty() && !items.is_empty() {
            ui.label(RichText::new("No open change matches.").color(palette.muted));
        }

        for index in found[ballots::window(found.len(), state.open.page, PER_PAGE)].iter().copied() {
            let proposal = &listing.proposals[index];
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

            ui.label(RichText::new(ballot(proposal)).small().color(palette.muted));

            let fraction = ballots::share(&proposal.weight, &proposal.needed).clamp(0.0, 1.0) as f32;
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

            // The kind's version the ballot is at, as the proposal or the listing says.
            let version = proposal.version.unwrap_or_else(|| listing.version_of(change.kind()));

            ui.horizontal_wrapped(|ui| {
                for from in &acting {
                    // Its approval of this change's kind, which approving moves, or withdrawing takes back.
                    let counted = state.weights.get(&from.to_string()).and_then(|weight| weight.as_ref().ok()).and_then(|weight| weight.approved_in(change.kind()));
                    let short = format::short(&from.to_string());
                    let by = if acting.len() > 1 { format!(" as {short}") } else { String::new() };

                    match counted.filter(|counted| counted.change == *change) {
                        Some(counted) => {
                            if theme::quiet(ui, &palette, &format!("Withdraw{by}")).clicked() {
                                *chosen = Some(withdrawal(*from, counted, &coin));
                            }
                        }
                        None => {
                            if theme::primary(ui, &palette, &format!("Approve{by}"), true).clicked() {
                                *chosen = Some(approval(*from, listing, change.clone(), version, counted.is_some(), &coin));
                            }
                        }
                    }
                }
            });
        }

        if found.len() > PER_PAGE {
            let said = ballots::summary(found.len(), state.open.page, PER_PAGE, state.open.narrows(), items.len(), items.len());
            ballots::pager(ui, &palette, &mut state.open, found.len(), false, false, &said);
        }

        if acting.is_empty() && !listing.proposals.is_empty() {
            ui.add_space(4.0);
            ui.label(RichText::new("None of this account's addresses, nor a multisig it follows, may approve: only the council's members, or finality's voters, do.").small().color(palette.muted));
        }
    });
}

/// What the page's search and kinds know of an open change.
fn open_item(proposal: &Proposal, coin: &Coin) -> Item {
    let words = match &proposal.change {
        Ok(change) => describe(change, coin),
        Err(why) => why.clone(),
    };
    let accounts: Vec<&str> = proposal.approvals.iter().map(|approval| approval.account.as_str()).collect();
    let kind = kind_of(&proposal.kind, proposal.change.as_ref().ok());

    Item {
        text: format!("{} {words} {} {}", kind_name(&kind), proposal.json, accounts.join(" ")),
        kind,
        place: proposal.approvals.iter().map(|approval| approval.epoch).max().unwrap_or_default(),
        support: ballots::share(&proposal.weight, &proposal.needed),
        closes: proposal.approvals.iter().map(|approval| approval.counts_until).max(),
    }
}

/// What the page's search and kinds know of a decision.
fn decided_item(decision: &Decision, coin: &Coin) -> Item {
    let change = decision.change.as_ref().and_then(|change| change.as_ref().ok());
    let words = decision_words(decision, coin);
    let kind = kind_of("", change);
    let json = change.map_or_else(String::new, |change| change_json(change).to_string());

    Item {
        text: format!("{} {words} {json} {} {} block {}", kind_name(&kind), decision.sender.as_deref().unwrap_or_default(), decision.tx, decision.height),
        kind,
        place: decision.version,
        support: 0.0,
        closes: None,
    }
}

/// A change's kind, by the name its ballot has: as the node lists it, or from the change.
fn kind_of(listed: &str, change: Option<&Change>) -> String {
    match (listed.parse::<ChangeKind>(), change) {
        (Ok(kind), _) => kind.name().to_string(),
        (_, Some(change)) => change.kind().name().to_string(),
        _ => listed.to_string(),
    }
}

/// What a kind of ballot is called, in a filter.
fn kind_name(kind: &str) -> String {
    match kind.parse::<ChangeKind>() {
        Ok(ChangeKind::Consensus) => "Consensus".to_string(),
        Ok(kind) => kind_label(kind).trim_start_matches("The chain's ").to_string(),
        Err(_) if kind.is_empty() => "Unknown".to_string(),
        Err(_) => kind.replace('_', " "),
    }
}

/// Proposing a change: picking its kind, filling in its fields, and reviewing what moves.
fn propose(ui: &mut Ui, ctx: &mut Ctx, state: &mut GovernanceState, listing: &Listing, voters: &[Voter], chosen: &mut Option<Pending>) {
    let palette = *ctx.palette;
    let coin = ctx.coin.clone();
    let acting = acting(voters, &state.weights);
    let tip = ctx.chain.following().map(|(tip, _)| tip);
    let env = env_of(listing, &coin, tip);

    if state.kind.is_empty() {
        state.kind = KINDS[0].to_string();
    }

    let stands = standing_for(listing, &state.kind);

    // Filled once a kind is picked, and again should what stands move while nothing is typed.
    let stale = match &state.filled {
        Some((kind, filled)) if *kind == state.kind => *filled != stands && state.form.draft == Draft::of(kind, filled.as_ref(), &env) && !state.form.advanced,
        _ => true,
    };

    if stale {
        state.form = Form { draft: Draft::of(&state.kind, stands.as_ref(), &env), ..Form::default() };
        state.filled = Some((state.kind.clone(), stands.clone()));
        state.trouble = None;
    }

    theme::card(ui, &palette, |ui| {
        if std::mem::take(&mut state.back_to_form) {
            ui.scroll_to_cursor(Some(egui::Align::TOP));
        }

        theme::caps(ui, &palette, "Propose a change");
        ui.label(RichText::new("A change is proposed by approving it: others then approve the same change, and it is decided once its approvals weigh what it needs. Change only the fields that should move: the rest keep what stands.").small().color(palette.muted));
        ui.add_space(4.0);

        egui::ComboBox::from_id_salt("governance-kind").selected_text(label(&state.kind)).width(ui.available_width()).height(KIND_LIST_HEIGHT).show_ui(ui, |ui| {
            for kind in KINDS {
                ui.selectable_value(&mut state.kind, kind.to_string(), label(kind));
            }
        });
        ui.add_space(6.0);

        let built = match state.form.advanced {
            false => form::build(&state.kind, &state.form.draft, stands.as_ref(), &env),
            true => proposed(&state.kind, &state.form.json, stands.as_ref(), &env),
        };

        if !state.form.advanced {
            form::draw(ui, &palette, &state.kind, &mut state.form.draft, stands.as_ref(), &env, &built);
        }

        for why in form::general(&built, &state.kind, &state.form.draft) {
            ui.add(egui::Label::new(RichText::new(why).color(palette.bad)).wrap());
        }

        if state.kind == "upgrade" && !state.form.advanced && let Some(tip) = tip {
            ui.label(RichText::new(format!("The tip is block {}.", format::thousands(&tip.to_string()))).small().color(palette.muted));
        }

        if let Some(change) = &built.change {
            match built.changes.is_empty() {
                true => {
                    ui.label(RichText::new("Nothing differs from what stands yet.").color(palette.muted));
                }
                false => {
                    ui.add_space(4.0);
                    theme::caps(ui, &palette, &format!("{} to change", built.changes.len()));
                    changes(ui, &palette, &built.changes);
                    ui.label(RichText::new(format!("Approves at {} v{}", kind_label(change.kind()).to_lowercase(), listing.version_of(change.kind()))).small().color(palette.muted));
                }
            }
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

        if let (Some(from), Some(change)) = (state.proposer.filter(|proposer| acting.contains(proposer)), built.change.clone()) {
            let counted = state.weights.get(&from.to_string()).and_then(|weight| weight.as_ref().ok()).is_some_and(|weight| weight.approved_in(change.kind()).is_some());

            if theme::primary(ui, &palette, "Review", !built.changes.is_empty()).clicked() {
                let version = listing.version_of(change.kind());
                *chosen = Some(approval(from, listing, change, version, counted, &coin));
                state.reviewing = true;
            }
        }

        if let Some(why) = &state.trouble {
            ui.colored_label(palette.bad, why.as_str());
        }

        ui.add_space(6.0);
        advanced(ui, &palette, state, &built, stands.as_ref());
    });
}

/// The switch to the kind's JSON, for what the form does not cover, and the editor while it is on,
/// filled with what the form says by then, or what stands.
fn advanced(ui: &mut Ui, palette: &theme::Palette, state: &mut GovernanceState, built: &Built, stands: Option<&Value>) {
    let was = state.form.advanced;
    let header = egui::CollapsingHeader::new(RichText::new("Advanced: the change as JSON").small().color(palette.muted)).id_salt(("governance-advanced", &state.kind)).default_open(was).show(ui, |ui| {
        let partial = match PARTIAL.contains(&state.kind.as_str()) {
            true => " Fields left out keep what stands, so an object naming only the fields to change will do.",
            false => "",
        };
        ui.add(egui::Label::new(RichText::new(format!("For what the form above does not cover. While this is open, this is what is proposed.{partial}")).small().color(palette.muted)).wrap());

        // The first frame open: from the form.
        if !state.form.advanced {
            let start = built.change.as_ref().map_or_else(|| stands.cloned().unwrap_or_else(|| schema::defaults(&state.kind)), |change| change_value(change, &state.kind));
            state.form.json = pretty(&start);
        }

        ui.add(egui::TextEdit::multiline(&mut state.form.json).hint_text("{ … }").desired_rows(10).desired_width(f32::INFINITY).font(egui::TextStyle::Monospace));
    });
    state.form.advanced = header.body_returned.is_some();
}

/// A change's value, under its kind's name.
fn change_value(change: &Change, kind: &str) -> Value {
    change_json(change)[kind].clone()
}

/// What moves, field by field: this field, old → new.
fn changes(ui: &mut Ui, palette: &theme::Palette, changes: &[Diff]) {
    for change in changes {
        ui.add(egui::Label::new(RichText::new(&change.label).small().color(palette.soft)).wrap());
        ui.add(egui::Label::new(theme::arrowed(ui, &format!("{} → {}", change.old, change.new), egui::TextStyle::Body, palette.ink)).wrap());
        ui.add_space(2.0);
    }
}

/// The review: what moves, field by field, the change in words and as JSON, and what approving does.
/// Some(true) to go on, Some(false) to go back.
fn confirm(ui: &mut Ui, ctx: &mut Ctx, pending: &Pending) -> Option<bool> {
    let palette = *ctx.palette;

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, &pending.what);
        ui.add_space(4.0);
        ui.add(egui::Label::new(RichText::new(&pending.words).color(palette.ink).size(17.0)).wrap());
        ui.add_space(6.0);

        if !pending.changes.is_empty() {
            theme::caps(ui, &palette, "What moves");
            changes(ui, &palette, &pending.changes);
            ui.add_space(4.0);
        }

        super::row(ui, &palette, "From", |ui| super::address(ui, &palette, &pending.from.to_string()));

        if let Some(json) = &pending.json {
            ui.add_space(4.0);
            egui::CollapsingHeader::new(RichText::new("The whole change as JSON").small().color(palette.muted)).id_salt("governance-review-json").show(ui, |ui| {
                super::text_block(ui, &palette, json);
            });
        }

        ui.add_space(6.0);
        ui.add(egui::Label::new(RichText::new(&pending.note).small().color(palette.soft)).wrap());
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

                if let Some(Vote::Approved { decided: Some(decision), kind, kind_version, .. }) = &checked.vote {
                    let made = kind_version.map(|made| format!(" · {} v{made}", kind_words(*kind))).unwrap_or_default();
                    theme::pill(ui, &format!("Decided change {decision}{made}"), palette.good);
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

/// Every decision, a page at a time, found by search, kind and order: older pages are read from the
/// node as a page comes to need them.
fn recent(ui: &mut Ui, ctx: &mut Ctx, state: &mut GovernanceState) {
    let palette = *ctx.palette;
    let coin = ctx.coin.clone();
    let read: Vec<Decision> = state.decided.newest_first().cloned().collect();
    let items: Vec<Item> = read.iter().map(|decision| decided_item(decision, &coin)).collect();

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Decisions");

        if state.decided.bottom.is_none() {
            match &state.past_trouble {
                Some(why) => ui.colored_label(palette.bad, format!("The node did not answer: {why}")),
                None => {
                    super::waiting(ui, &palette, "Asking the node…");

                    return;
                }
            };

            return;
        }

        if state.decided.version == 0 {
            ui.label(RichText::new("Nothing decided yet: every setting is genesis's.").color(palette.muted));

            return;
        }

        if state.decided.from > 1 {
            ui.add(egui::Label::new(RichText::new(format!("This node keeps decisions from change {} on: it synced from a snapshot.", state.decided.from)).small().color(palette.muted)).wrap());
        }

        ballots::controls(ui, &palette, &mut state.past, "Search: kind, address, value or block", &items, &ballots::DECIDED_ORDERS, &kind_name);

        let found = ballots::select(&items, &state.past);
        let more = state.decided.next();

        // Reads on as a page needs it; a failure waits for the page's next asking.
        if ballots::wants_older(found.len(), &state.past, PER_PAGE, more.is_some()) && !state.reading && state.past_trouble.is_none() {
            ctx.node.ask(Ask::GovernanceHistory { before: more });
            state.reading = true;
        }

        if found.is_empty() {
            ui.label(RichText::new(if more.is_some() { "None found yet." } else { "No decision matches." }).color(palette.muted));
        }

        for index in found[ballots::window(found.len(), state.past.page, PER_PAGE)].iter().copied() {
            let decision = &read[index];
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                if decision.final_ {
                    theme::pill(ui, "Final", palette.good);
                }

                ui.label(RichText::new(format!("Change {} · {}", decision.version, decision_words(decision, &coin))).color(palette.ink)).on_hover_text(format!("Decided by {}", decision.tx));
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

        let said = ballots::summary(found.len(), state.past.page, PER_PAGE, state.past.narrows(), read.len(), state.decided.kept());
        ballots::pager(ui, &palette, &mut state.past, found.len(), more.is_some(), state.reading, &said);

        if let Some(why) = &state.past_trouble {
            ui.colored_label(palette.bad, format!("The node did not answer: {why}"));
        }
    });
}

/// What a decision changed, in words.
fn decision_words(decision: &Decision, coin: &Coin) -> String {
    match &decision.change {
        Some(Ok(change)) => describe(change, coin),
        Some(Err(why)) => format!("a change this wallet does not read: {why}"),
        None => "a change the node did not say".to_string(),
    }
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
        versions: versions(value)?,
        approval_epochs: whole("approval_epochs")?,
        needed: value["needed"].as_str().ok_or("it says no needed")?.to_string(),
        members: value["members"].as_array().map(|members| members.iter().filter_map(Value::as_str).map(str::to_string).collect()),
        proposals: value["proposals"].as_array().map(|proposals| proposals.iter().map(proposal).collect::<Result<Vec<_>, _>>()).transpose()?.unwrap_or_default(),
        in_force: settings,
        finality: value["finality"].as_str().map(str::to_string),
        switch: value["switch"]["epoch"].as_u64().zip(value["switch"]["type"].as_str().map(str::to_string)),
    })
}

/// GET /governance's versions, one for every kind the wallet knows.
fn versions(value: &Value) -> Result<BTreeMap<ChangeKind, u64>, String> {
    let versions = value["versions"].as_object().ok_or("it says no versions")?;

    ChangeKind::ALL
        .into_iter()
        .map(|kind| versions.get(kind.name()).and_then(Value::as_u64).map(|version| (kind, version)).ok_or(format!("it says no version for {}", kind.name())))
        .collect()
}

/// A proposal of the listing: the change's JSON, which the listing adds its kind, that kind's
/// version, its weight, what it needs and its approvals to, read as the change.
fn proposal(value: &Value) -> Result<Proposal, String> {
    let mut fields = value.as_object().ok_or("a proposal is not an object")?.clone();
    let text = |value: Option<Value>| value.and_then(|value| value.as_str().map(str::to_string)).unwrap_or_else(|| "0".to_string());
    let kind = fields.remove("kind").and_then(|kind| kind.as_str().map(str::to_string)).unwrap_or_default();
    let version = fields.remove("version").and_then(|version| version.as_u64());
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

    Ok(Proposal { change: change_from_json(&json), json, kind, version, weight, needed, approvals })
}

/// GET /governance/weight/{address}'s answer, read.
fn weight(value: &Value) -> Result<Weight, String> {
    let weight = value["weight"].as_str().and_then(|text| u256::from_str_radix(text, 10).ok()).ok_or("it says no weight")?;
    let approved = value["approved"]
        .as_array()
        .map(|approved| approved.iter().map(approved_change).collect::<Result<Vec<_>, _>>())
        .transpose()?
        .unwrap_or_default();

    Ok(Weight { weight, approved })
}

/// One of an address's approvals: the change's JSON with its kind and the kind's version beside it.
fn approved_change(value: &Value) -> Result<Approved, String> {
    let mut fields = value.as_object().ok_or("an approval is not an object")?.clone();
    let kind = fields.remove("kind");
    let version = fields.remove("version").and_then(|version| version.as_u64()).ok_or("an approval names no version")?;
    let change = change_from_json(&Value::Object(fields))?;
    // The kind the node names, or the change's own should it name none.
    let kind = match kind.as_ref().and_then(Value::as_str) {
        Some(kind) => kind.parse()?,
        None => change.kind(),
    };

    Ok(Approved { kind, version, change })
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

/// Approving a change from an address, at its kind's version. moves says whether the address's
/// approval of another change of that kind counts now.
fn approval(from: Address, listing: &Listing, change: Change, version: u64, moves: bool, coin: &Coin) -> Pending {
    let kind = kind_words(change.kind());
    let moving = match moves {
        true => format!(" This address's approval of a {kind} change counts for another one now: approving this one moves it here, as an address backs one change of each kind at a time. Its approvals of other kinds stay."),
        false => format!(" An address backs one change of each kind at a time: approving another {kind} change later moves this approval there, and its approvals of other kinds stay."),
    };

    Pending {
        from,
        words: describe(&change, coin),
        changes: form::diff(change.name(), standing_for(listing, change.name()).as_ref(), &change_value(&change, change.name()), &env_of(listing, coin, None)),
        json: Some(pretty(&change_json(&change))),
        note: format!(
            "Approves at {kind} version {version}: should another {kind} change be decided first, the version moves on and this approval no longer counts. A decision of another kind leaves it standing.{moving} It counts for {} epoch{}, and decides the change once the approvals weigh {}.",
            listing.approval_epochs,
            plural(listing.approval_epochs),
            weighs(listing, &listing.needed, coin)
        ),
        tx: Tx::ApproveChange { version, change },
        what: "Approve a change".to_string(),
    }
}

/// Withdrawing an address's approval of a change of a kind, at the kind's version it named.
fn withdrawal(from: Address, approved: &Approved, coin: &Coin) -> Pending {
    let kind = kind_words(approved.kind);

    Pending {
        from,
        tx: Tx::WithdrawChange { kind: approved.kind, version: approved.version },
        what: "Withdraw an approval".to_string(),
        words: describe(&approved.change, coin),
        changes: Vec::new(),
        json: Some(pretty(&change_json(&approved.change))),
        note: format!("Takes this address's approval off this {kind} change; its approvals of other kinds, and the others' approvals, stay. Approving another {kind} change instead would move the approval there."),
    }
}

/// What stands for a setting, by its name in force.
fn standing<'a>(listing: &'a Listing, kind: &str) -> Option<&'a Value> {
    listing.in_force.iter().find(|setting| setting.kind == kind).map(|setting| &setting.value)
}

/// What stands for a kind of change, as its fields are laid out: a switch of finality's kind over
/// the finality decided last.
fn standing_for(listing: &Listing, kind: &str) -> Option<Value> {
    match kind {
        "switch_finality" => standing(listing, "finality").map(|finality| json!({ "epoch": null, "finality": finality })),
        kind => standing(listing, kind).cloned(),
    }
}

/// What the form takes from the listing and the wallet, beside what stands.
fn env_of<'a>(listing: &'a Listing, coin: &'a Coin, tip: Option<u64>) -> Env<'a> {
    Env {
        decimals: coin.decimals,
        symbol: &coin.symbol,
        epoch: listing.epoch,
        tip,
        finality: standing(listing, "finality").and_then(|finality| finality["type"].as_str()).or(listing.finality.as_deref()),
        consensus: standing(listing, "consensus").and_then(|consensus| consensus["type"].as_str()),
        switching: listing.switch.is_some(),
    }
}

/// The change Advanced's JSON asks for, through change_from_json, so it is checked as the CLI's
/// approve-change checks it, with what else the form checks. A partial object of a PARTIAL kind is
/// laid over what stands, so the fields left out keep it.
fn proposed(kind: &str, typed: &str, standing: Option<&Value>, env: &Env) -> Built {
    let mut built = Built::default();
    let given: Value = match serde_json::from_str(typed) {
        Ok(given) => given,
        Err(why) => {
            built.errors.insert(form::GENERAL.to_string(), format!("Not JSON: {why}"));

            return built;
        }
    };
    let value = match standing {
        Some(standing) if PARTIAL.contains(&kind) && given.is_object() => over(standing, &given),
        _ => given,
    };

    form::judge(kind, &value, env, &mut built);

    if built.change.is_some() {
        built.changes = form::diff(kind, standing, &value, env);
    }

    // Under Advanced there are no fields to say it under.
    built.errors = built.errors.into_values().enumerate().map(|(index, why)| (format!("{}{index}", form::GENERAL), why)).collect();

    built
}

/// Each kind's version, in a line.
fn versions_line(listing: &Listing) -> String {
    listing.versions.iter().map(|(kind, version)| format!("{} v{version}", kind_words(*kind))).collect::<Vec<_>>().join(" · ")
}

/// Which ballot a proposal is on: its kind and the kind's version.
fn ballot(proposal: &Proposal) -> String {
    let kind = proposal.kind.parse::<ChangeKind>().map_or_else(|_| proposal.kind.replace('_', " "), |kind| kind_label(kind).to_lowercase());

    match proposal.version {
        Some(version) => format!("{kind} · version {version}"),
        None => kind,
    }
}

/// A kind of ballot, as the page names it.
fn kind_label(kind: ChangeKind) -> &'static str {
    match kind {
        ChangeKind::Consensus => "Consensus and finality",
        kind => label(kind.name()),
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
                "Fees: {} gas a block, {}% of each fee burned, {} gas schedule, {pricing}",
                format::thousands(&fees.block_gas_limit.to_string()),
                // In percent, as the explorer shows it: 500‰ is 50%, 25‰ is 2.5%.
                f64::from(fees.burn_permille) / 10.0,
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

    /// GET /governance on a council of two, one deciding, with the chain renamed and its bootnodes
    /// emptied, two decisions that made name v1 and bootnodes v1, and a change of bootnodes open,
    /// as the node's own test serves it.
    fn council() -> Value {
        json!({
            "kind": "council",
            "epoch": 3,
            "version": 2,
            "versions": {
                "consensus": 0, "bootnodes": 1, "name_prices": 0, "collections": 0, "federation": 0, "directory": 0,
                "fees": 0, "reward": 0, "limits": 0, "upgrade": 0, "name": 1,
            },
            "approval_epochs": 16,
            "needed": "1",
            "members": [member(1).to_string(), member(2).to_string()],
            "proposals": [{
                "bootnodes": ["/dns4/boot.example/udp/4001/quic-v1"],
                "kind": "bootnodes",
                "version": 1,
                "weight": "1",
                "needed": "1",
                "approvals": [{ "account": member(2).to_string(), "weight": "1", "epoch": 3, "counts_until": 18 }],
            }, {
                "name": "",
                "kind": "name",
                "version": 1,
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

        // Each kind's version, beside the count of every decision.
        assert_eq!(listing.versions.len(), ChangeKind::ALL.len());
        assert_eq!((listing.version_of(ChangeKind::Bootnodes), listing.version_of(ChangeKind::Name), listing.version_of(ChangeKind::Fees)), (1, 1, 0));
        assert!(versions_line(&listing).starts_with("consensus v0 · bootnodes v1 · name prices v0"), "{}", versions_line(&listing));

        let mut short = council();
        short["versions"].as_object_mut().unwrap().remove("fees");
        assert!(super::listing(&short).is_err_and(|why| why == "it says no version for fees"), "every kind's version, or approvals could name none");

        let open = &listing.proposals[0];
        assert_eq!(open.json, json!({ "bootnodes": ["/dns4/boot.example/udp/4001/quic-v1"] }), "kind, version, weight, needed and approvals are the listing's, not the change's");
        assert_eq!((open.kind.as_str(), open.version), ("bootnodes", Some(1)), "the version of its own kind, not the decisions'");
        assert_eq!(ballot(open), "bootnodes · version 1");
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
        let reward = json!({ "initial": "7", "halving_interval": null });
        let voting = weight(&json!({
            "account": member(1).to_string(),
            "weight": "1",
            "needed": "1",
            "approved": [{ "name": "third", "kind": "name", "version": 1 }, { "reward": reward, "kind": "reward", "version": 0 }],
        }))
        .unwrap();
        let barred = weight(&json!({ "account": member(3).to_string(), "weight": "0", "needed": "1", "approved": [] })).unwrap();

        // One approval a kind: a rename and a reward at once.
        assert!(voting.may_vote());
        assert_eq!(voting.approved_in(ChangeKind::Name), Some(&Approved { kind: ChangeKind::Name, version: 1, change: Change::Name("third".to_string()) }));
        assert_eq!(voting.approved_in(ChangeKind::Reward).map(|approved| (approved.version, &approved.change)), Some((0, &Change::Reward(Reward { initial: u256::new(7), halving_interval: None }))));
        assert_eq!(voting.approved_in(ChangeKind::Fees), None);
        assert!(!barred.may_vote());
        assert_eq!(barred.approved, Vec::new());
        assert!(weight(&json!({ "weight": 1 })).is_err());
        assert!(weight(&json!({ "weight": "1", "approved": [{ "name": "third", "kind": "name" }] })).is_err_and(|why| why.contains("no version")));
        assert!(weight(&json!({ "weight": "1", "approved": [{ "name": "third", "kind": "weather", "version": 1 }] })).is_err());
        assert_eq!(
            weight(&json!({ "weight": "1", "approved": [{ "finality": change_json(&Change::Finality(FinalityConfig::Depth(DepthConfig { depth: 6 })))["finality"], "version": 2 }] })).unwrap().approved[0].kind,
            ChangeKind::Consensus,
            "a kind not named is the change's own"
        );

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
    fn approving_and_withdrawing_build_their_transactions_at_their_kind_s_version() {
        let listing = listing(&council()).unwrap();
        let change = Change::Bootnodes(vec!["/dns4/boot.example/udp/4001/quic-v1".to_string()]);
        let version = listing.version_of(change.kind());
        let approve = approval(member(1), &listing, change.clone(), version, true, &coin());

        // Bootnodes v1, not the two decisions of every kind.
        assert_eq!(approve.tx, Tx::ApproveChange { version: 1, change: change.clone() });
        assert_eq!(approve.from, member(1));
        assert_eq!(approve.words, "Set 1 bootnode: /dns4/boot.example/udp/4001/quic-v1");
        assert!(approve.note.contains("moves it here"), "moving an approval is approving another change: {}", approve.note);
        assert!(approve.note.contains("approval of a bootnodes change counts for another one now"), "{}", approve.note);
        assert!(approve.note.contains("Its approvals of other kinds stay"), "{}", approve.note);
        assert!(approve.note.starts_with("Approves at bootnodes version 1: should another bootnodes change be decided first"), "{}", approve.note);
        assert!(approve.note.contains("A decision of another kind leaves it standing"), "{}", approve.note);
        assert_eq!(approve.json.as_deref().map(|json| serde_json::from_str::<Value>(json).unwrap()), Some(change_json(&change)));
        assert!(approval(member(1), &listing, change.clone(), version, false, &coin()).note.contains("approving another bootnodes change later moves this approval there"));

        // A finality change is on the consensus's ballot.
        let depth = Change::Finality(FinalityConfig::Depth(DepthConfig { depth: 6 }));
        assert!(approval(member(1), &listing, depth.clone(), listing.version_of(depth.kind()), false, &coin()).note.starts_with("Approves at consensus version 0"));

        // Withdrawing names the kind and the version its approval was at, one per kind.
        let weight = weight(&json!({
            "weight": "1",
            "approved": [{ "bootnodes": ["/dns4/boot.example/udp/4001/quic-v1"], "kind": "bootnodes", "version": 1 }, { "name": "third", "kind": "name", "version": 1 }],
        }))
        .unwrap();
        let withdrawals: Vec<Tx> = weight.approved.iter().map(|approved| withdrawal(member(1), approved, &coin()).tx).collect();

        assert_eq!(withdrawals, [Tx::WithdrawChange { kind: ChangeKind::Bootnodes, version: 1 }, Tx::WithdrawChange { kind: ChangeKind::Name, version: 1 }]);

        let withdraw = withdrawal(member(1), &weight.approved[1], &coin());
        assert_eq!((withdraw.what.as_str(), withdraw.words.as_str()), ("Withdraw an approval", "Rename the chain to third"));
        assert!(withdraw.note.contains("off this name change; its approvals of other kinds"), "{}", withdraw.note);
    }

    /// What Advanced's JSON says, its first error if it says none.
    fn raw(kind: &str, typed: &str, standing: Option<&Value>) -> Result<Change, String> {
        let (coin, listing) = (coin(), listing(&council()).unwrap());
        let built = proposed(kind, typed, standing, &Env { switching: false, ..env_of(&listing, &coin, None) });

        built.change.ok_or_else(|| built.errors.into_values().next().unwrap_or_default())
    }

    #[test]
    fn advanced_json_makes_its_change_and_refuses_bad_input() {
        assert_eq!(raw("name", r#""chainv4 mainnet""#, None), Ok(Change::Name("chainv4 mainnet".to_string())));
        assert!(raw("name", r#""""#, None).is_err());
        assert_eq!(raw("bootnodes", "[]", None), Ok(Change::Bootnodes(Vec::new())), "none leaves the chain without");
        assert_eq!(raw("reward", r#"{ "initial": "7", "halving_interval": 100 }"#, None), Ok(Change::Reward(Reward { initial: u256::new(7), halving_interval: Some(100) })));
        assert!(raw("reward", "{ initial", None).is_err_and(|why| why.starts_with("Not JSON")));
        assert!(raw("reward", r#"{ "initial": "7" }"#, None).is_err());
    }

    #[test]
    fn a_partial_object_of_fees_the_reward_limits_finality_the_federation_or_the_directory_keeps_what_it_leaves_out() {
        let fees = FeeConfig {
            block_gas_limit: 50_000_000,
            burn_permille: 500,
            schedule: ScheduleConfig::Weighted(Weighted { per_tx: 1_000, per_byte: 10, per_code_byte: 20, fuel_per_gas: 1 }),
            pricing: PricingConfig::Bid(Bid { min_price: u256::new(7) }),
        };
        let mut value = council();
        value["in_force"]["fees"] = json!({ "value": change_json(&Change::Fees(fees.clone()))["fees"], "decided": null });
        let listing = listing_of(&value);

        // One field of fees: the rest stand.
        assert_eq!(raw("fees", r#"{ "burn_permille": 300 }"#, standing(&listing, "fees")), Ok(Change::Fees(FeeConfig { burn_permille: 300, ..fees })));
        assert!(raw("fees", r#"{ "burn_permille": 300 }"#, None).is_err(), "with nothing standing, a part is not a change");

        // The reward's halving alone, and limits' fuel inside tx_limits, field by field.
        assert_eq!(
            raw("reward", r#"{ "halving_interval": 100 }"#, standing(&listing, "reward")),
            Ok(Change::Reward(Reward { initial: u256::new(5_000_000_000_000_000_000), halving_interval: Some(100) }))
        );
        assert_eq!(
            raw("limits", r#"{ "tx_limits": { "fuel": 5000000 } }"#, standing(&listing, "limits")),
            Ok(Change::Limits(BlockLimits { tx_limits: Limits { fuel: 5_000_000, memory: 67_108_864 }, max_block_bytes: None }))
        );

        // The federation's threshold alone, finality's depth alone, and the directory's DEX taken
        // off while its gateway stands.
        let directory = Directory {
            gateway: Some(Address::Contract([1; 32])),
            btc_gateway: None,
            evm_gateways: vec![EvmGateway { chain_id: 1, gateway: [3; 20] }],
            swap: None,
            dex: Some(Address::Contract([2; 32])),
            dex_router: None,
            nft_factory: None,
        };
        value["in_force"]["finality"] = json!({ "value": change_json(&Change::Finality(FinalityConfig::Depth(DepthConfig { depth: 6 })))["finality"], "decided": null });
        value["in_force"]["directory"] = json!({ "value": change_json(&Change::Directory(directory.clone()))["directory"], "decided": 2 });
        let federation = Federation { signers: vec![compressed(2), compressed(3), compressed(4)], threshold: 1, chain_depth: 10, evm_depth: 12, bitcoin_depth: 1 };
        value["in_force"]["federation"] = json!({ "value": change_json(&Change::Federation(federation.clone()))["federation"], "decided": null });
        let listing = listing_of(&value);

        assert_eq!(raw("federation", r#"{ "threshold": 2 }"#, standing(&listing, "federation")), Ok(Change::Federation(Federation { threshold: 2, ..federation })));
        assert_eq!(raw("finality", r#"{ "depth": 9 }"#, standing(&listing, "finality")), Ok(Change::Finality(FinalityConfig::Depth(DepthConfig { depth: 9 }))));
        assert_eq!(raw("directory", r#"{ "dex": null }"#, standing(&listing, "directory")), Ok(Change::Directory(Directory { dex: None, ..directory })));

        // An empty object is what stands, whole.
        assert_eq!(raw("reward", "{}", standing(&listing, "reward")), setting_change(listing.in_force.iter().find(|setting| setting.kind == "reward").unwrap()));

        // The consensus is not laid over: a part of it is not one.
        assert!(raw("consensus", r#"{ "slot_ms": 1000 }"#, standing(&listing, "consensus")).is_err());
        assert_eq!(PARTIAL.map(|kind| kind.parse::<ChangeKind>().is_ok()), [true; 6], "each a kind the form lists");
        assert!(PARTIAL.iter().all(|kind| KINDS.contains(kind)));
    }

    #[test]
    fn approving_says_what_moves_field_by_field() {
        let mut value = council();
        value["in_force"]["fees"] = json!({ "value": { "block_gas_limit": 50_000_000, "burn_permille": 500, "schedule": { "type": "weighted", "per_tx": 1000, "per_byte": 10, "per_code_byte": 20, "fuel_per_gas": 1 }, "pricing": { "type": "bid", "min_price": "7" } }, "decided": null });
        let listing = listing_of(&value);
        let name = approval(member(1), &listing, Change::Name("third".to_string()), 1, false, &coin());

        assert_eq!(name.changes, [Diff { label: "The chain's name".to_string(), old: "second".to_string(), new: "third".to_string() }]);

        let reward = approval(member(1), &listing, Change::Reward(Reward { initial: u256::new(7_000_000_000_000_000_000), halving_interval: None }), 0, false, &coin());
        assert_eq!(reward.changes, [Diff { label: "What a block mints".to_string(), old: "5 DEV".to_string(), new: "7 DEV".to_string() }], "what stayed is not listed");

        let withdrawn = weight(&json!({ "weight": "1", "approved": [{ "name": "third", "kind": "name", "version": 1 }] })).unwrap();
        assert!(withdrawal(member(1), &withdrawn.approved[0], &coin()).changes.is_empty(), "a withdrawal moves no setting");
    }

    #[test]
    fn open_changes_and_decisions_are_found_by_kind_address_and_value() {
        let listing = listing_of(&council());
        let items: Vec<Item> = listing.proposals.iter().map(|proposal| open_item(proposal, &coin())).collect();
        let find = |search: &str| ballots::select(&items, &Finder { search: search.to_string(), ..Finder::default() });

        assert_eq!(items[0].kind, "bootnodes");
        assert_eq!((items[0].place, items[0].closes, items[0].support), (3, Some(18), 1.0));
        assert_eq!(find("boot.example"), [0], "a value");
        assert_eq!(find(&member(2).to_string()[..24]), [0], "an address, or a part of one");
        assert_eq!(find("bootnodes"), [0], "a kind");
        assert_eq!(kind_name("name"), "name");
        assert_eq!(kind_name("consensus"), "Consensus");
        assert_eq!(kind_name("name_prices"), "Name prices");

        let history = decisions(&json!({ "decisions": [
            { "version": 2, "tx": "0x02", "height": 77, "timestamp": 2000, "final": false, "sender": member(1).to_string(), "kind": "bootnodes", "change": { "bootnodes": [] } },
            { "version": 1, "tx": "0x01", "height": 7, "timestamp": 1000, "final": true, "sender": member(3).to_string(), "kind": "finality", "change": { "finality": { "type": "depth", "depth": 6 } } },
        ] }));
        let items: Vec<Item> = history.iter().map(|decision| decided_item(decision, &coin())).collect();
        let find = |search: &str| ballots::select(&items, &Finder { search: search.to_string(), ..Finder::default() });

        assert_eq!(items[1].kind, "consensus", "finality's tuning is on the consensus's ballot");
        assert_eq!(find("without bootnodes"), [0], "its words");
        assert_eq!(find("block 77"), [0]);
        assert_eq!(find(&member(3).to_string()), [1]);
        assert_eq!(ballots::select(&items, &Finder { kind: Some("consensus".to_string()), ..Finder::default() }), [1]);
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
                "Fees: 50,000,000 gas a block, 50% of each fee burned, weighted gas schedule, bids of at least 0.000000000000000007 DEV a gas",
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
        state.answered(&Ask::GovernanceWeight(member(1).to_string()), &Ok(json!({ "weight": "1", "needed": "1", "approved": [] })));
        state.answered(&Ask::GovernanceHistory { before: None }, &Ok(json!({ "version": 2, "from": 1, "decisions": [{ "version": 2, "tx": "0x02", "height": 2 }, { "version": 1, "tx": "0x01", "height": 1 }] })));

        assert_eq!(state.listing.as_ref().and_then(|listing| listing.as_ref().ok()).map(|listing| listing.version), Some(2));
        assert!(state.weights[&member(1).to_string()].as_ref().is_ok_and(Weight::may_vote));
        assert_eq!((state.decided.version, state.decided.next(), state.decided.items.len()), (2, None, 2));

        // An older page goes under what was read, and a failure says why and keeps it.
        state.answered(&Ask::GovernanceHistory { before: None }, &Ok(json!({ "version": 9, "from": 1, "decisions": [{ "version": 9, "tx": "0x09", "height": 9 }, { "version": 8, "tx": "0x08", "height": 8 }] })));
        assert_eq!((state.decided.version, state.decided.next()), (9, Some(8)), "so many since that it starts over");
        state.answered(&Ask::GovernanceHistory { before: Some(8) }, &Ok(json!({ "version": 9, "from": 1, "decisions": [{ "version": 7, "tx": "0x07", "height": 7 }] })));
        assert_eq!(state.decided.next(), Some(7));
        state.answered(&Ask::GovernanceHistory { before: Some(7) }, &Err(Failure { status: 0, message: "gone".to_string() }));
        assert_eq!((state.decided.items.len(), state.past_trouble.as_deref()), (3, Some("gone")));

        state.answered(&Ask::Governance, &Ok(json!({ "kind": "council" })));
        assert!(matches!(&state.listing, Some(Err(failure)) if failure.message.starts_with("its answer does not read")));
    }
}
