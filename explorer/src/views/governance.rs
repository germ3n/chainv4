//! Governance: who may approve a change, what one needs and how long an approval counts; the
//! changes being approved, by kind, each in words with its approvals; every setting in force and
//! the change that last set it; and every decision, newest first, each proven by the light client:
//! its receipt decided that change, of its kind, and its transaction approved it.
//!
//! Each kind of change has a ballot and a version of its own, which an approval names; the chain
//! also counts every decision, of any kind, without gaps, and history lists them by that count.

use {
    super::{Ctx, Look, View},
    crate::api::Ask,
    crate::format,
    crate::light::{Checked, ProvenReceipt},
    crate::model::{Decision, GovernanceView, Proposal},
    chainv4::governance::ChangeKind,
    chainv4::structs::receipt::CommittedOutcome,
    chainv4::structs::tx::Tx,
    chainv4_ui::widgets,
    egui::{Color32, RichText, Ui},
    serde_json::Value,
};

const ABOUT: &str = "What the chain's governance may change, who approves, and every change it decided.";

/// The settings governance may change, in the order the page lists them.
const KINDS: [&str; 12] = ["consensus", "name", "bootnodes", "name_prices", "collections", "federation", "directory", "fees", "reward", "limits", "finality", "upgrade"];

/// The most council members whose names the page looks up.
const MOST_NAMED: usize = 64;

#[derive(Default)]
pub struct GovernanceState {
    /// The address typed into the weight lookup.
    typed: String,
    /// The account looked up, as the node writes it.
    looked_up: Option<String>,
    trouble: Option<String>,
    /// The decision a setting in force links to, to mark and scroll to once it is listed.
    focus: Option<u64>,
}

/// The chain's coin: its decimals and symbol, for amounts in changes.
type Coin = (u8, String);

pub fn show(ui: &mut Ui, ctx: &mut Ctx, state: &mut GovernanceState) {
    let look = ctx.look;
    let store = ctx.store;
    let Some(view) = &store.governance else {
        super::heading(ui, look, "Governance", ABOUT, |_| {});

        match store.failures.get(&Ask::Governance) {
            Some(failure) if failure.status == 404 => none(ui, look),
            Some(_) => super::waiting(ui, ctx, &Ask::Governance),
            None => {
                ctx.api.ask(Ask::Governance);
                super::waiting(ui, ctx, &Ask::Governance);
            }
        }

        return;
    };
    let coin = ctx.coin();

    super::heading(ui, look, "Governance", &kind_about(view), |ui| {
        widgets::pill(ui, &view.kind, look.roles.main);
        widgets::pill(ui, &format!("epoch {}", view.epoch), look.palette.soft);
    });

    super::stats(
        ui,
        look,
        &[
            ("decided".to_string(), format::thousands(&view.version.to_string()), "changes since genesis, of every kind".to_string()),
            ("needed".to_string(), weight(view, &coin, &view.needed), "for a change to pass".to_string()),
            ("approvals count".to_string(), epochs(view.approval_epochs), "from the epoch each was made in".to_string()),
            ("open".to_string(), format::thousands(&view.proposals.len().to_string()), "changes being approved".to_string()),
        ],
    );

    ui.add(egui::Label::new(RichText::new(format!("By kind: {}", by_kind(view, false))).small().color(look.palette.muted)).wrap());
    ui.add_space(6.0);

    super::section(ui, look, "Who approves", |ui| approvers(ui, ctx, view, state, &coin));
    super::section_with(
        ui,
        look,
        "Open proposals",
        |ui| {
            ui.label(RichText::new(view.proposals.len().to_string()).monospace().color(look.palette.muted));
        },
        |ui| proposals(ui, ctx, view, &coin),
    );
    super::section(ui, look, "In force", |ui| in_force(ui, ctx, view, state, &coin));
    super::section_with(
        ui,
        look,
        "Decisions",
        |ui| {
            ui.label(RichText::new(store.decisions.version.to_string()).monospace().color(look.palette.muted));
        },
        |ui| decisions(ui, ctx, state, &coin),
    );
}

/// A chain without governance: what that means.
fn none(ui: &mut Ui, look: Look) {
    super::section(ui, look, "No governance here", |ui| {
        super::note(
            ui,
            look,
            "This chain has no governance: its genesis names none, so its consensus, fees, reward, limits and every other setting stay as genesis set them. See the governance guide.",
        );
    });
}

/// What the kind of governance is, in a line.
fn kind_about(view: &GovernanceView) -> String {
    match view.kind.as_str() {
        "council" => format!("A council genesis names approves each change. An approval counts for {}.", epochs(view.approval_epochs)),
        "electorate" => format!("Finality's voters approve each change. An approval counts for {}.", epochs(view.approval_epochs)),
        other => format!("{other} governance. An approval counts for {}.", epochs(view.approval_epochs)),
    }
}

/// Who may approve, what a change needs, the version, finality and any switch of it ahead, and a
/// lookup of what an account's approval weighs.
fn approvers(ui: &mut Ui, ctx: &mut Ctx, view: &GovernanceView, state: &mut GovernanceState, coin: &Coin) {
    let look = ctx.look;
    let store = ctx.store;

    super::facts(ui, look, "approvers", |facts| {
        facts.text("kind", &view.kind);

        match &view.members {
            Some(members) => {
                for member in members.iter().take(MOST_NAMED) {
                    if !store.accounts.contains_key(member) && !store.failures.contains_key(&Ask::Account(member.clone())) {
                        ctx.api.ask(Ask::Account(member.clone()));
                    }
                }

                facts.row(&format!("members · {}", members.len()), |ui| {
                    ui.vertical(|ui| {
                        for member in members {
                            ui.horizontal_wrapped(|ui| {
                                super::address(ui, ctx, member);

                                if let Some(name) = store.accounts.get(member).and_then(|account| account.name.as_deref()) {
                                    ui.label(RichText::new(name).color(look.palette.soft));
                                }
                            });
                        }
                    });
                });
            }
            None => facts.row("approvers", |ui| {
                ui.add(egui::Label::new(electorate(&view.finality)).wrap());

                if ui.link(RichText::new("Finality").color(look.palette.accent)).clicked() {
                    ctx.go(View::Finality);
                }
            }),
        }

        facts.text("a change needs", needed_text(view, coin));
        facts.text("approvals count", format!("{} from the epoch each was made in", epochs(view.approval_epochs)));
        facts.text("decided", format!("{} changes, of every kind", format::thousands(&view.version.to_string())));
        facts.text("versions", format!("{}; an approval names its kind's", by_kind(view, true)));

        if !view.finality.is_empty() {
            facts.text("finality", &view.finality);
        }

        // A consensus decided for epochs a checkpoint has yet to fix.
        if !view.decided.is_null() && view.decided != view.active {
            facts.text("consensus ahead", format!("{}, decided, from the epoch a checkpoint fixes it for", consensus(&view.decided)));
        }

        if let Some(switch) = &view.switch {
            facts.text("switching", format!("to {} finality from epoch {}", switch.kind, format::thousands(&switch.epoch.to_string())));
        }
    });

    ui.add_space(8.0);
    lookup(ui, ctx, view, state, coin);
}

/// Who approves under an electorate, by the finality it runs.
fn electorate(finality: &str) -> &'static str {
    match finality {
        "ffg" => "Finality's validators approve, each weighing the stake it holds in the epoch's set; two thirds of it decide.",
        "authority" => "Finality's signers approve, each weighing one; the decided set's change threshold decides.",
        "depth" => "Depth finality has no voters, so no change can be decided until finality has some.",
        _ => "Finality's voters approve.",
    }
}

/// Each kind's version, compactly: name 2 · upgrade 1. Every kind, or only those decided at all.
pub fn by_kind(view: &GovernanceView, every: bool) -> String {
    let known = ChangeKind::ALL.into_iter().map(|kind| (kind.name().to_string(), view.versions.get(kind.name()).copied().unwrap_or_default()));
    let others = view.versions.iter().filter(|(kind, _)| !ChangeKind::ALL.iter().any(|known| known.name() == kind.as_str())).map(|(kind, version)| (kind.clone(), *version));
    let shown: Vec<String> = known
        .chain(others)
        .filter(|(_, version)| every || *version > 0)
        .map(|(kind, version)| format!("{} {}", kind.replace('_', " "), format::thousands(&version.to_string())))
        .collect();

    match shown.is_empty() {
        true => "none decided yet".to_string(),
        false => shown.join(" · "),
    }
}

/// A kind, possessive, for a person: the name's, the consensus's, the bootnodes', the name prices'.
pub fn kinds(kind: &str) -> String {
    let spaced = kind.replace('_', " ");

    match spaced.ends_with('s') && spaced != "consensus" {
        true => format!("the {spaced}'"),
        false => format!("the {spaced}'s"),
    }
}

/// What a change needs, in words.
fn needed_text(view: &GovernanceView, coin: &Coin) -> String {
    match view.kind.as_str() {
        _ if unreachable(&view.needed) => "more than anyone can approve: no change can pass now".to_string(),
        "council" => format!("{} of the council's {} members", view.needed, view.members.as_ref().map_or(0, Vec::len)),
        _ if view.weighs_stake() => format!("{} of stake, two thirds of the epoch's", weight(view, coin, &view.needed)),
        _ => format!("{} signers", view.needed),
    }
}

/// Whether what a change needs is more than any approvals could weigh: what the node says when
/// nobody votes.
fn unreachable(needed: &str) -> bool {
    needed.len() > 60
}

/// What approvals weigh, in the coin under an electorate of stake, as a count elsewhere.
fn weight(view: &GovernanceView, coin: &Coin, raw: &str) -> String {
    match (unreachable(raw), view.weighs_stake()) {
        (true, _) => "out of reach".to_string(),
        (false, true) => format::amount(raw, coin.0, &coin.1),
        (false, false) => format::thousands(raw),
    }
}

/// So many epochs.
fn epochs(count: u64) -> String {
    match count {
        1 => "1 epoch".to_string(),
        count => format!("{} epochs", format::thousands(&count.to_string())),
    }
}

/// What an account's approval weighs now, and what it approved.
fn lookup(ui: &mut Ui, ctx: &mut Ctx, view: &GovernanceView, state: &mut GovernanceState, coin: &Coin) {
    let look = ctx.look;
    let palette = look.palette;
    let typed = !state.typed.trim().is_empty();
    let pressed = widgets::beside(
        ui,
        |ui| widgets::primary(ui, &palette, "Weigh", typed).clicked(),
        |ui, width| {
            ui.add(egui::TextEdit::singleline(&mut state.typed).hint_text("What an account's approval weighs: ed25519:0x…, or an Ethereum 0x…").desired_width(width));
        },
    );

    if pressed {
        match super::pool::payout_address(state.typed.trim()) {
            Ok(address) => {
                ctx.api.ask(Ask::GovernanceWeight(address.clone()));
                state.looked_up = Some(address);
                state.trouble = None;
            }
            Err(why) => state.trouble = Some(why),
        }
    }

    if let Some(trouble) = &state.trouble {
        ui.colored_label(palette.bad, trouble.as_str());
    }

    let Some(account) = state.looked_up.clone() else {
        return;
    };
    let ask = Ask::GovernanceWeight(account.clone());
    let Some(found) = ctx.store.governance_weights.get(&account) else {
        super::waiting(ui, ctx, &ask);

        return;
    };

    super::facts(ui, look, "weight", |facts| {
        facts.row("account", |ui| super::address(ui, ctx, &found.account));
        facts.text(
            "weighs",
            match found.weight.as_str() {
                "0" => "nothing: it may not approve".to_string(),
                raw => format!("{} of the {} a change needs", weight(view, coin, raw), weight(view, coin, &found.needed)),
            },
        );
        match found.approved.is_empty() {
            true => facts.text("approves", "nothing now"),
            false => facts.row(&format!("approves · {}", found.approved.len()), |ui| {
                ui.vertical(|ui| {
                    for approved in &found.approved {
                        ui.add(egui::Label::new(approved_text(approved, coin)).wrap());
                    }
                });
            }),
        }
    });
}

/// A change an account approved, in words, with the kind's change it would be.
pub fn approved_text(approved: &crate::model::ApprovedChange, coin: &Coin) -> String {
    format!("{}, for {} change {}", describe(&approved.change(), coin), kinds(&approved.kind), approved.version + 1)
}

/// The changes being approved: each in words, how far its approvals got, and who approved.
fn proposals(ui: &mut Ui, ctx: &mut Ctx, view: &GovernanceView, coin: &Coin) {
    let look = ctx.look;

    if view.proposals.is_empty() {
        super::note(ui, look, "Nothing is being approved. An approval names its kind's version: how many changes of that kind were decided.");

        return;
    }

    for (kind, version, group) in grouped(view) {
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(widgets::strong(label(&kind), 13.0).color(look.palette.soft));
            ui.label(RichText::new(kind_heading(&kind, version)).small().color(look.palette.muted));
        });

        for (index, proposal) in group {
            super::list_row(ui, look, |ui| proposal_row(ui, ctx, view, proposal, index, coin));
        }
    }
}

/// A kind's open proposals: the kind, its version, and each proposal with its place in the list.
type Group<'a> = (String, u64, Vec<(usize, &'a Proposal)>);

/// The open proposals by kind, in the order the node lists them, each kind with its version.
fn grouped(view: &GovernanceView) -> Vec<Group<'_>> {
    let mut groups: Vec<Group<'_>> = Vec::new();

    for (index, proposal) in view.proposals.iter().enumerate() {
        let kind = proposal.kind_name();

        match groups.iter_mut().find(|(known, _, _)| *known == kind) {
            Some((_, _, group)) => group.push((index, proposal)),
            None => groups.push((kind, proposal.version, vec![(index, proposal)])),
        }
    }

    groups
}

/// What a kind's open proposals are for: version 2, so its change 3.
pub fn kind_heading(kind: &str, version: u64) -> String {
    format!("version {}: approving {} change {}", format::thousands(&version.to_string()), kinds(kind), format::thousands(&(version + 1).to_string()))
}

fn proposal_row(ui: &mut Ui, ctx: &mut Ctx, view: &GovernanceView, proposal: &Proposal, index: usize, coin: &Coin) {
    let look = ctx.look;
    let change = proposal.change();

    ui.add(egui::Label::new(widgets::strong(describe(&change, coin), 14.0).color(look.palette.ink)).wrap());
    super::meter(
        ui,
        look,
        fraction(&proposal.weight, &proposal.needed),
        &format!("{} of {} needed", weight(view, coin, &proposal.weight), weight(view, coin, &proposal.needed)),
    );
    ui.add_space(4.0);

    match look.narrow {
        true => {
            for approval in &proposal.approvals {
                ui.horizontal_wrapped(|ui| {
                    super::address(ui, ctx, &approval.account);
                    ui.label(RichText::new(format!("{} · counts until epoch {}", weight(view, coin, &approval.weight), approval.counts_until)).small().color(look.palette.muted));
                });
            }
        }
        false => {
            super::table(ui, look, ("approvals", index), &["account", "weight", "made in", "counts until"], |ui| {
                for approval in &proposal.approvals {
                    super::address(ui, ctx, &approval.account);
                    ui.monospace(weight(view, coin, &approval.weight));
                    ui.monospace(format!("epoch {}", approval.epoch));
                    ui.monospace(format!("epoch {}", approval.counts_until));
                    ui.end_row();
                }
            });
        }
    }

    egui::CollapsingHeader::new(RichText::new("JSON").color(look.palette.soft)).id_salt(("proposal", index)).show(ui, |ui| {
        super::json_tree(ui, ctx, "change", &change);
    });
}

/// One decimal amount's fraction of another: 0 when the whole is.
fn fraction(part: &str, whole: &str) -> f32 {
    super::pool::fraction(part, whole) as f32
}

/// Every setting in force, summed up, and genesis or the change that last set it.
fn in_force(ui: &mut Ui, ctx: &mut Ctx, view: &GovernanceView, state: &mut GovernanceState, coin: &Coin) {
    let look = ctx.look;
    let settings = ordered(view);

    if settings.is_empty() {
        super::note(ui, look, "The node lists no settings.");

        return;
    }

    let mut set_by = |ui: &mut Ui, decided: Option<u64>| match decided {
        Some(version) => {
            if ui.link(RichText::new(format!("change {version}")).monospace().color(look.palette.accent)).on_hover_text("Show it among the decisions").clicked() {
                state.focus = Some(version);
            }
        }
        None => {
            ui.label(RichText::new("genesis").color(look.palette.muted));
        }
    };

    match look.narrow {
        true => {
            for (kind, setting) in &settings {
                super::list_row(ui, look, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(widgets::strong(label(kind), 13.5).color(look.palette.ink));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| set_by(ui, setting.decided));
                    });
                    ui.add(egui::Label::new(RichText::new(summary(kind, &setting.value, coin)).color(look.palette.soft)).wrap());
                });
            }
        }
        false => {
            super::table(ui, look, "in force", &["setting", "in force", "set by"], |ui| {
                for (kind, setting) in &settings {
                    ui.label(widgets::strong(label(kind), 13.0));
                    ui.add(egui::Label::new(summary(kind, &setting.value, coin)).wrap()).on_hover_text(setting.value.to_string());
                    set_by(ui, setting.decided);
                    ui.end_row();
                }
            });
        }
    }
}

/// The settings in force in the page's order, any kind it does not know of after.
fn ordered(view: &GovernanceView) -> Vec<(String, crate::model::InForce)> {
    let known = KINDS.iter().filter_map(|kind| view.in_force.get(*kind).map(|setting| (kind.to_string(), setting.clone())));
    let others = view.in_force.iter().filter(|(kind, _)| !KINDS.contains(&kind.as_str())).map(|(kind, setting)| (kind.clone(), setting.clone()));

    known.chain(others).collect()
}

/// How a decision stands against the light client.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Its receipt says it decided this change, and its transaction approved it, in a block the
    /// light client checked; final there or not yet.
    Proven { final_: bool },
    /// Asked for, or about to be.
    Checking,
    /// In a block at this height the light client has yet to reach.
    Waiting(u64),
    /// The node's proof does not agree with what it lists: why.
    Wrong(String),
    /// Nothing can be checked: why. What the node lists is all there is.
    Unchecked(String),
}

/// How a decision stands, from what the light client made of its receipt.
pub fn verdict(decision: &Decision, checked: Option<&Checked>, trouble: Option<&str>, hash: chainv4::structs::hash::HashType) -> Verdict {
    if let Some(trouble) = trouble {
        return Verdict::Unchecked(format!("the light client stopped: {trouble}"));
    }

    match checked {
        None => Verdict::Checking,
        Some(Checked::Waiting(height)) => Verdict::Waiting(*height),
        Some(Checked::Refused(why)) => Verdict::Wrong(why.clone()),
        Some(Checked::Proven(proven)) => match agrees(decision, proven, hash) {
            Ok(()) => Verdict::Proven { final_: proven.assurance.final_ },
            Err(why) => Verdict::Wrong(why),
        },
    }
}

/// The change a transaction approves: its own, or the one a multisig's transaction carries.
fn approved(tx: &Tx) -> Option<(u64, &chainv4::governance::Change)> {
    match tx {
        Tx::ApproveChange { version, change } => Some((*version, change)),
        Tx::ApproveMultisig { tx, .. } | Tx::Multisig { tx, .. } => approved(tx),
        _ => None,
    }
}

/// Whether what the light client proved is what the node lists: the receipt decided this change of
/// the chain's count, of the kind listed, in the block listed, and its transaction approved this
/// change, of that kind, from the kind's version before, sent by whom the node says.
fn agrees(decision: &Decision, proven: &ProvenReceipt, hash: chainv4::structs::hash::HashType) -> Result<(), String> {
    let block = proven.assurance.block.to_string();

    if block != decision.block {
        return Err(format!("the node lists it in block {} but proves it in block {block}", format::short(&decision.block)));
    }

    let CommittedOutcome::Decided { decision: number, kind, version: kind_version } = proven.receipt.outcome else {
        return Err(format!("its receipt does not say it decided change {}", decision.version));
    };

    if number != decision.version {
        return Err(format!("its receipt does not say it decided change {}", decision.version));
    }

    // The node lists a change by its own name, finality's under the consensus's kind.
    if let Some(listed) = &decision.kind
        && listed.parse::<ChangeKind>() != Ok(kind)
    {
        return Err(format!("the node lists a change of {}, but its receipt decided one of {}", listed.replace('_', " "), kind.name().replace('_', " ")));
    }

    if proven.signed.id(hash).to_string() != decision.tx {
        return Err("the node proved another transaction".to_string());
    }

    let Some((version, change)) = approved(&proven.signed.tx) else {
        return Err("its transaction approves no change".to_string());
    };

    if change.kind() != kind {
        return Err(format!("its transaction approves a change of {}, but its receipt decided one of {}", change.kind().name().replace('_', " "), kind.name().replace('_', " ")));
    }

    if version + 1 != kind_version {
        return Err(format!("its transaction approves {} change after version {version}, not after {}", kinds(kind.name()), kind_version.saturating_sub(1)));
    }

    let proven_change = chainv4::params::json::change_json(change);

    if let Some(listed) = &decision.change
        && *listed != proven_change
    {
        return Err(format!("its transaction approves {}, not the change the node lists", change.name()));
    }

    if let Some(sender) = &decision.sender
        && *sender != proven.signed.sender().to_string()
    {
        return Err(format!("its transaction was sent by {}, not {}", proven.signed.sender(), format::short(sender)));
    }

    Ok(())
}

/// A decision as the page shows it: what it changed, and how it stands.
struct Shown {
    decision: Decision,
    change: Option<Value>,
    verdict: Verdict,
}

/// The decisions, newest first: each proven or not, any gap among them, and older ones a page at a
/// time.
fn decisions(ui: &mut Ui, ctx: &mut Ctx, state: &mut GovernanceState, coin: &Coin) {
    let look = ctx.look;
    let store = ctx.store;
    let listing = &store.decisions;
    let ask = Ask::Decisions { before: None };

    if listing.bottom.is_none() {
        super::waiting(ui, ctx, &ask);

        return;
    }

    if listing.version == 0 {
        super::note(ui, look, "Governance has decided nothing yet: every setting is genesis's.");

        return;
    }

    let hash = ctx.light.hash();
    let trouble = ctx.light.trouble.clone();
    let shown: Vec<Shown> = listing
        .newest_first()
        .map(|decision| {
            let checked = ctx.light.receipts.get(&decision.tx);
            let proven_change = match checked {
                Some(Checked::Proven(proven)) => approved(&proven.signed.tx).map(|(_, change)| chainv4::params::json::change_json(change)),
                _ => None,
            };
            let verdict = match hash {
                Some(hash) => verdict(decision, checked, trouble.as_deref(), hash),
                None => Verdict::Unchecked("the light client has not started".to_string()),
            };

            Shown {
                decision: decision.clone(),
                change: proven_change.filter(|_| matches!(verdict, Verdict::Proven { .. })).or_else(|| decision.change.clone()),
                verdict,
            }
        })
        .collect();

    // A setting's link to a decision not read yet reads older pages till it is.
    if let Some(focus) = state.focus
        && !listing.items.contains_key(&focus)
    {
        match listing.next() {
            Some(before) if focus < before => ctx.api.ask(Ask::Decisions { before: Some(before) }),
            _ => state.focus = None,
        }
    }

    if listing.from > 1 {
        super::note(ui, look, format!("This node keeps decisions from change {} on: it synced from a snapshot, and lists none before it.", listing.from));
    }

    for (from, to) in listing.gaps() {
        ui.colored_label(look.palette.warn, gap(from, to));
    }

    let proven = shown.iter().filter(|shown| matches!(shown.verdict, Verdict::Proven { .. })).count();
    super::note(ui, look, format!("{proven} of the {} listed proven by the light client, against blocks it checked.", shown.len()));
    ui.add_space(4.0);

    for item in &shown {
        let focused = state.focus == Some(item.decision.version);
        let frame = egui::Frame::new().fill(if focused { look.palette.row } else { Color32::TRANSPARENT }).corner_radius(4.0);
        let response = frame.show(ui, |ui| super::list_row(ui, look, |ui| decision_row(ui, ctx, item, coin))).response;

        if focused {
            response.scroll_to_me(Some(egui::Align::Center));
            state.focus = None;
        }
    }

    if let Some(before) = listing.next() {
        let older = Ask::Decisions { before: Some(before) };

        ui.add_space(6.0);

        match ctx.api.asking(&older) {
            true => {
                ui.spinner();
            }
            false => {
                if widgets::quiet(ui, &look.palette, &format!("Older, below change {before}")).clicked() {
                    ctx.api.ask(older);
                }
            }
        }
    }
}

/// A run of versions the node did not list, in words.
pub fn gap(from: u64, to: u64) -> String {
    match from == to {
        true => format!("Change {from} is missing: the node did not list it, so what it changed is unknown here."),
        false => format!("Changes {from} to {to} are missing: the node did not list them, so what they changed is unknown here."),
    }
}

fn decision_row(ui: &mut Ui, ctx: &mut Ctx, item: &Shown, coin: &Coin) {
    let look = ctx.look;
    let decision = &item.decision;
    let what = match &item.change {
        Some(change) => describe(change, coin),
        None => "A change the node could not read".to_string(),
    };

    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(format!("#{}", decision.version)).monospace().color(look.palette.muted));
        ui.label(widgets::strong(what, 13.5).color(look.palette.ink));
    });
    ui.horizontal_wrapped(|ui| {
        let (text, color, about) = verdict_pill(look, &item.verdict);
        widgets::pill(ui, text, color).on_hover_text(about);

        match decision.finalized {
            true => widgets::pill(ui, "final", look.roles.final_),
            false => widgets::pill(ui, &format!("{} confirmations", format::thousands(&decision.confirmations.to_string())), look.roles.side),
        };
    });

    if let Verdict::Wrong(why) = &item.verdict {
        ui.colored_label(look.palette.bad, why);
    }

    ui.horizontal_wrapped(|ui| {
        let muted = |text: &str| RichText::new(text).small().color(look.palette.muted);
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.label(muted("tx"));
        super::tx(ui, ctx, &decision.tx);
        ui.label(muted("in block"));
        super::block_link(ui, ctx, &decision.block, decision.height);

        if let Some(sender) = &decision.sender {
            ui.label(muted("from"));
            super::address(ui, ctx, sender);
        }

        ui.label(muted(&format!("· {} UTC · {}", format::utc(decision.timestamp), format::ago(decision.timestamp, ctx.now))));
    });

    if let Some(change) = &item.change {
        egui::CollapsingHeader::new(RichText::new("JSON").color(look.palette.soft)).id_salt(("decision", decision.version)).show(ui, |ui| {
            super::json_tree(ui, ctx, "change", change);
        });
    }
}

/// A verdict as a pill: its word, its color, and what it means.
fn verdict_pill(look: Look, verdict: &Verdict) -> (&'static str, Color32, String) {
    match verdict {
        Verdict::Proven { final_: true } => ("proven", look.palette.good, "The light client proved its receipt decided this change, and its transaction approved it, in a final block it checked".to_string()),
        Verdict::Proven { final_: false } => ("proven", look.palette.good, "The light client proved its receipt decided this change, and its transaction approved it, in a block it checked that is not final yet".to_string()),
        Verdict::Checking => ("checking", look.palette.soft, "The light client is asking the node to prove it".to_string()),
        Verdict::Waiting(height) => ("waiting", look.palette.soft, format!("In block {}, which the light client has yet to reach", format::thousands(&height.to_string()))),
        Verdict::Wrong(why) => ("does not check", look.palette.bad, why.clone()),
        Verdict::Unchecked(why) => ("reported by the node", look.palette.muted, format!("Not checked: {why}")),
    }
}

// Changes in words.

/// A setting's name, for a person.
pub fn label(kind: &str) -> String {
    match kind {
        "name_prices" => "Name prices".to_string(),
        "reward" => "Block reward".to_string(),
        "switch_finality" => "Finality's kind".to_string(),
        other => {
            let spaced = other.replace('_', " ");
            let mut chars = spaced.chars();

            chars.next().map_or_else(String::new, |first| first.to_uppercase().chain(chars).collect())
        }
    }
}

/// A change, as Tx::ApproveChange's JSON carries it, { kind: value }, in words: what it does.
pub fn describe(change: &Value, coin: &Coin) -> String {
    let Some((kind, value)) = change.as_object().filter(|change| change.len() == 1).and_then(|change| change.iter().next()) else {
        return "A change the explorer cannot read".to_string();
    };
    let said = summary(kind, value, coin);

    match kind.as_str() {
        "consensus" => format!("Change consensus to {said}"),
        "name" => format!("Rename the chain to {said}"),
        "bootnodes" if value.as_array().is_some_and(Vec::is_empty) => "Remove every bootnode".to_string(),
        "bootnodes" => format!("Set the bootnodes to {said}"),
        "name_prices" => format!("Price names at {said}"),
        "collections" => format!("Make collections by {said}"),
        "federation" => format!("Hand the bridges to {said}"),
        "directory" => format!("Announce {said}"),
        "fees" => format!("Set the fees: {said}"),
        "reward" => format!("Set the block reward to {said}"),
        "limits" => format!("Set the limits: {said}"),
        "finality" => format!("Retune finality: {said}"),
        "switch_finality" => format!("Switch finality to {said}"),
        "upgrade" => capitalized(&said),
        other => format!("Change the chain's {}", other.replace('_', " ")),
    }
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();

    chars.next().map_or_else(String::new, |first| first.to_uppercase().chain(chars).collect())
}

/// A setting's value, in words: what it is.
pub fn summary(kind: &str, value: &Value, coin: &Coin) -> String {
    let text = |name: &str| value[name].as_str().map_or_else(|| value[name].to_string(), str::to_string);
    let number = |name: &str| value[name].as_u64().map_or_else(|| "?".to_string(), |number| format::thousands(&number.to_string()));
    let coins = |raw: &str| format::amount(raw, coin.0, &coin.1);

    match kind {
        "consensus" => consensus(value),
        "name" => value.as_str().unwrap_or("?").to_string(),
        "bootnodes" => match value.as_array().map(Vec::as_slice) {
            Some([]) => "none".to_string(),
            Some([one]) => one.as_str().unwrap_or("?").to_string(),
            Some(many) => format!("{} addresses", many.len()),
            None => "?".to_string(),
        },
        "name_prices" => {
            let prices: Vec<String> = value.as_array().into_iter().flatten().filter_map(Value::as_str).map(coins).collect();

            match prices.len() {
                0 => "nothing".to_string(),
                1..=4 => format!("{} a year, by length", prices.join(", ")),
                _ => format!("{}, … a year, by length", prices[..3].join(", ")),
            }
        }
        "collections" => {
            let count = |name: &str| value[name].as_array().map_or(0, Vec::len);

            format!("{} NFT functions and {} multi-token ones", count("nft"), count("multi"))
        }
        "federation" => format!(
            "{} of {} signers, chain depth {}, EVM {}, Bitcoin {}",
            number("threshold"),
            value["signers"].as_array().map_or(0, Vec::len),
            number("chain_depth"),
            number("evm_depth"),
            number("bitcoin_depth"),
        ),
        "directory" => {
            let names = [("gateway", "the gateway"), ("btc_gateway", "the Bitcoin gateway"), ("swap", "the swap contract"), ("dex", "the DEX"), ("dex_router", "the DEX router"), ("nft_factory", "the NFT factory")];
            let mut announced: Vec<String> = names.iter().filter(|(key, _)| value[*key].is_string()).map(|(_, name)| name.to_string()).collect();
            let evm = value["evm_gateways"].as_array().map_or(0, Vec::len);

            if evm > 0 {
                announced.push(format!("{evm} EVM gateway{}", if evm == 1 { "" } else { "s" }));
            }

            match announced.is_empty() {
                true => "no contracts".to_string(),
                false => announced.join(", "),
            }
        }
        "fees" => {
            let pricing = &value["pricing"];
            let pricing = match pricing["type"].as_str() {
                Some("base_fee") => format!("a base price of at least {}", format::thousands(pricing["min"].as_str().unwrap_or("?"))),
                Some("bid") => format!("bids of at least {}", format::thousands(pricing["min_price"].as_str().unwrap_or("?"))),
                _ => "its own pricing".to_string(),
            };
            let burned = value["burn_permille"].as_u64().map_or_else(|| "?".to_string(), |permille| format!("{}%", permille as f64 / 10.0));

            format!("{} gas a block, {burned} burned, {pricing}", number("block_gas_limit"))
        }
        "reward" => {
            let halving = match value["halving_interval"].as_u64() {
                Some(interval) => format!("halving every {} blocks", format::thousands(&interval.to_string())),
                None => "never halving".to_string(),
            };

            format!("{} a block, {halving}", coins(&text("initial")))
        }
        "limits" => {
            let tx = &value["tx_limits"];
            let fuel = tx["fuel"].as_u64().map_or_else(|| "?".to_string(), |fuel| format::thousands(&fuel.to_string()));
            let memory = tx["memory"].as_u64().map_or_else(|| "?".to_string(), bytes);
            let block = value["max_block_bytes"].as_u64().map_or_else(|| "blocks bound by gas alone".to_string(), |most| format!("{} a block", bytes(most)));

            format!("{fuel} fuel and {memory} of memory a transaction, {block}")
        }
        "finality" => finality(value),
        "switch_finality" => format!("{} from epoch {}", finality(&value["finality"]), number("epoch")),
        "upgrade" => super::network::stopping(value["least"].as_str().unwrap_or("?"), value["from"].as_u64().unwrap_or_default()),
        _ => value.to_string(),
    }
}

/// A consensus, in words.
fn consensus(value: &Value) -> String {
    let every = |name: &str| value[name].as_u64().map_or_else(|| "?".to_string(), every);

    match value["type"].as_str() {
        Some("pow") => format!("proof of work by {}, a block every {}", value["hash"].as_str().unwrap_or("?"), every("target_block_ms")),
        Some("authority") => format!("one authority, {}, a block every {}", format::short(value["key"].as_str().unwrap_or("?")), every("slot_ms")),
        Some("rotating") => format!("signers in turn, a block every {}", every("slot_ms")),
        Some("staked") => format!("validators by stake, {}, a block every {}", value["schedule"].as_str().unwrap_or("?"), every("slot_ms")),
        Some(other) => format!("{other} consensus"),
        None => "a consensus".to_string(),
    }
}

/// Finality's settings, in words.
fn finality(value: &Value) -> String {
    let number = |name: &str| value[name].as_u64().map_or_else(|| "?".to_string(), |number| format::thousands(&number.to_string()));

    match value["type"].as_str() {
        Some("ffg") => format!("FFG, checkpoints every {} blocks, at most {} validators", number("checkpoint_interval"), number("max_validators")),
        Some("authority") => format!(
            "authority, {} of {} signers, checkpoints every {} blocks",
            number("threshold"),
            value["signers"].as_array().map_or(0, Vec::len),
            number("checkpoint_interval"),
        ),
        Some("depth") => format!("depth, final {} blocks deep", number("depth")),
        Some(other) => format!("{other} finality"),
        None => "finality".to_string(),
    }
}

/// Milliseconds as a block time: 2s, or 500 ms.
fn every(ms: u64) -> String {
    match ms.is_multiple_of(1_000) {
        true => format!("{}s", format::thousands(&(ms / 1_000).to_string())),
        false => format!("{} ms", format::thousands(&ms.to_string())),
    }
}

/// Bytes, in MiB or KiB when they divide evenly.
fn bytes(count: u64) -> String {
    const KIB: u64 = 1_024;
    const MIB: u64 = 1_024 * 1_024;

    match count {
        0 => "0 bytes".to_string(),
        count if count.is_multiple_of(MIB) => format!("{} MiB", format::thousands(&(count / MIB).to_string())),
        count if count.is_multiple_of(KIB) => format!("{} KiB", format::thousands(&(count / KIB).to_string())),
        count => format!("{} bytes", format::thousands(&count.to_string())),
    }
}

#[cfg(test)]
mod tests;
