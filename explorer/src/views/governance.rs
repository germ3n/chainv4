//! Governance: who may vote on a change, what one needs, its quorum and how long a vote counts;
//! each voter's turnout; the changes being voted on, found by search, kind and order, each a card
//! with its ring toward what it needs, a dot a voter and what it would move; every setting in force
//! and the change that last set it; the decisions and rejections along the chain; and every
//! decision and rejection, found likewise, a page at a time, each proven by the light client: its
//! receipt decided or rejected that change, of its kind, and its transaction voted on it.
//!
//! Each kind of change has a ballot and a version of its own, which a vote names; the chain also
//! counts every decision, of any kind, without gaps, and every rejection likewise, and history
//! lists each by its count.

use {
    super::{Ctx, Look, View},
    crate::api::Ask,
    crate::format,
    crate::light::{Checked, ProvenReceipt},
    crate::model::{Decision, GovernanceView, Proposal, Rejection, change_kind},
    chainv4::governance::ChangeKind,
    chainv4::structs::receipt::CommittedOutcome,
    chainv4::structs::tx::Tx,
    chainv4_ui::ballots::{self, Finder, Item, PER_PAGE},
    chainv4_ui::votes::{self, Choice, Dot, Mark, Turnout},
    chainv4_ui::widgets,
    egui::{Color32, RichText, Ui},
    serde_json::Value,
};

const ABOUT: &str = "What the chain's governance may change, who votes, and every change it decided or rejected.";

/// The settings governance may change, in the order the page lists them.
const KINDS: [&str; 18] = ["consensus", "name", "bootnodes", "names", "collections", "token_functions", "federation", "directory", "fees", "reward", "limits", "finality", "upgrade", "governance", "evm_spec", "dex", "anchoring", "locks"];

/// What the turnout asks of each voter's transactions: its votes alone.
pub const VOTES_QUERY: &str = "kind=vote_change";

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
    /// What is asked of the open proposals, of the decisions and of the rejections.
    open: Finder,
    past: Finder,
    rejected: Finder,
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
            ("rejected".to_string(), format::thousands(&view.rejections.to_string()), "changes that could no longer pass".to_string()),
            ("needed".to_string(), weight(view, &coin, &view.needed), "Yes, for a change to pass".to_string()),
            ("quorum".to_string(), quorum(view), "must vote at all, Abstain included".to_string()),
            ("votes count".to_string(), epochs(view.approval_epochs), "from the epoch each was cast in".to_string()),
            ("open".to_string(), format::thousands(&view.proposals.len().to_string()), "changes being voted on".to_string()),
        ],
    );

    ui.add(egui::Label::new(RichText::new(format!("By kind: {}", by_kind(view, false))).small().color(look.palette.muted)).wrap());
    ui.add_space(6.0);

    super::section(ui, look, "Who votes", |ui| approvers(ui, ctx, view, state, &coin));
    super::section(ui, look, "Turnout", |ui| turnout(ui, ctx, view));
    super::section_with(
        ui,
        look,
        "Open proposals",
        |ui| {
            ui.label(RichText::new(view.proposals.len().to_string()).monospace().color(look.palette.muted));
        },
        |ui| proposals(ui, ctx, view, state, &coin),
    );
    super::section(ui, look, "In force", |ui| in_force(ui, ctx, view, state, &coin));
    super::section_with(
        ui,
        look,
        "Locks",
        |ui| {
            ui.label(RichText::new(view.locks.len().to_string()).monospace().color(look.palette.muted));
        },
        |ui| locks(ui, ctx, view),
    );
    super::section(ui, look, "Along the chain", |ui| history(ui, ctx, &coin));
    super::section_with(
        ui,
        look,
        "Decisions",
        |ui| {
            ui.label(RichText::new(store.decisions.version.to_string()).monospace().color(look.palette.muted));
        },
        |ui| decisions(ui, ctx, state, &coin),
    );
    super::section_with(
        ui,
        look,
        "Rejections",
        |ui| {
            ui.label(RichText::new(store.rejections.version.to_string()).monospace().color(look.palette.muted));
        },
        |ui| rejections(ui, ctx, state, &coin),
    );
}

/// The quorum, as a share: none, or so much of every voter's weight.
fn quorum(view: &GovernanceView) -> String {
    match view.quorum_permille {
        0 => "none".to_string(),
        permille => format!("{}%", permille as f64 / 10.0),
    }
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
        "council" => format!("A council votes Yes, No or Abstain on each change. A vote counts for {}.", epochs(view.approval_epochs)),
        "electorate" => format!("Finality's voters vote Yes, No or Abstain on each change. A vote counts for {}.", epochs(view.approval_epochs)),
        other => format!("{other} governance. A vote counts for {}.", epochs(view.approval_epochs)),
    }
}

/// Who may vote, what a change needs, the quorum, the version, finality and any switch of it
/// ahead, and a lookup of what an account's vote weighs.
fn approvers(ui: &mut Ui, ctx: &mut Ctx, view: &GovernanceView, state: &mut GovernanceState, coin: &Coin) {
    let look = ctx.look;
    let store = ctx.store;

    super::facts(ui, look, "voters", |facts| {
        facts.text("kind", &view.kind);

        match &view.members {
            Some(members) => {
                for member in members.iter().take(MOST_NAMED) {
                    if !store.accounts.contains_key(member) {
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
            None => facts.row("voters", |ui| {
                ui.add(egui::Label::new(electorate(&view.finality)).wrap());

                if ui.link(RichText::new("Finality").color(look.palette.accent)).clicked() {
                    ctx.go(View::Finality);
                }
            }),
        }

        facts.text("a change needs", needed_text(view, coin));
        facts.text("quorum", match view.quorum_permille {
            0 => "none: Yes alone decides".to_string(),
            permille => format!("{}% of every voter's weight must vote, Abstain included, before Yes decides", permille as f64 / 10.0),
        });
        facts.text("rejected", "once what has not voted No can no longer reach what a change needs");
        facts.text("votes count", format!("{} from the epoch each was cast in; a new vote replaces the old", epochs(view.approval_epochs)));
        facts.text("decided", format!("{} changes, of every kind", format::thousands(&view.version.to_string())));
        facts.text("versions", format!("{}; a vote names its kind's", by_kind(view, true)));

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
        "ffg" => "Finality's validators vote, each weighing the stake it holds in the epoch's set; two thirds of it voting Yes decide.",
        "authority" => "Finality's signers vote, each weighing one; the decided set's change threshold voting Yes decides.",
        "depth" => "Depth finality has no voters, so no change can be decided until finality has some.",
        _ => "Finality's voters vote.",
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
        _ if unreachable(&view.needed) => "more than anyone can vote: no change can pass now".to_string(),
        "council" => format!("{} of the council's {} members voting Yes", view.needed, view.members.as_ref().map_or(0, Vec::len)),
        _ if view.weighs_stake() => format!("{} of stake, two thirds of the epoch's", weight(view, coin, &view.needed)),
        _ => format!("{} signers", view.needed),
    }
}

/// Whether what a change needs is more than any approvals could weigh: what the node says when
/// nobody votes.
fn unreachable(needed: &str) -> bool {
    needed.len() > 60
}

/// What votes weigh, in the coin under an electorate of stake, as a count elsewhere.
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

/// What an account's vote weighs now, and how it voted.
fn lookup(ui: &mut Ui, ctx: &mut Ctx, view: &GovernanceView, state: &mut GovernanceState, coin: &Coin) {
    let look = ctx.look;
    let palette = look.palette;
    let typed = !state.typed.trim().is_empty();
    let pressed = widgets::beside(
        ui,
        |ui| widgets::primary(ui, &palette, "Weigh", typed).clicked(),
        |ui, width| {
            ui.add(egui::TextEdit::singleline(&mut state.typed).hint_text("What an account's vote weighs: ed25519:0x…, or an Ethereum 0x…").desired_width(width));
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
                "0" => "nothing: it may not vote".to_string(),
                raw => format!("{} of the {} a change needs", weight(view, coin, raw), weight(view, coin, &found.needed)),
            },
        );
        match found.voted.is_empty() {
            true => facts.text("voted", "on nothing that still counts"),
            false => facts.row(&format!("voted · {}", found.voted.len()), |ui| {
                ui.vertical(|ui| {
                    for voted in &found.voted {
                        ui.add(egui::Label::new(voted_text(voted, coin)).wrap());
                    }
                });
            }),
        }
    });
}

/// A vote an account cast, in words, with the kind's change it would be.
pub fn voted_text(voted: &crate::model::VotedChange, coin: &Coin) -> String {
    let vote = Choice::of(&voted.vote).map_or("?", Choice::word);

    format!("{vote} on {}, for {} change {}", lowered(&describe(&voted.change, coin)), kinds(&voted.kind), voted.version + 1)
}

/// A sentence's first letter in lower case, to go on after other words.
fn lowered(text: &str) -> String {
    let mut chars = text.chars();

    chars.next().map_or_else(String::new, |first| first.to_lowercase().chain(chars).collect())
}

/// The changes being voted on, found by search, kind and order: each a card, its ring toward what
/// it needs, its quorum, a dot a voter, what it would move and who voted which way.
fn proposals(ui: &mut Ui, ctx: &mut Ctx, view: &GovernanceView, state: &mut GovernanceState, coin: &Coin) {
    let look = ctx.look;

    if view.proposals.is_empty() {
        super::note(ui, look, "Nothing is being voted on. A vote names its kind's version: how many changes of that kind were decided.");

        return;
    }

    let items: Vec<Item> = view.proposals.iter().map(|proposal| open_item(proposal, coin)).collect();
    ballots::controls(ui, &look.palette, &mut state.open, "Search: kind, address or value", &items, &ballots::OPEN_ORDERS, &label);

    let found = ballots::select(&items, &state.open);

    if found.is_empty() {
        super::note(ui, look, "No open proposal matches.");
    }

    for index in found[ballots::window(found.len(), state.open.page, PER_PAGE)].iter().copied() {
        let proposal = &view.proposals[index];

        super::list_row(ui, look, |ui| proposal_row(ui, ctx, view, proposal, index, coin));
    }

    if found.len() > PER_PAGE {
        let said = ballots::summary(found.len(), state.open.page, PER_PAGE, state.open.narrows(), items.len(), items.len());
        ballots::pager(ui, &look.palette, &mut state.open, found.len(), false, false, &said);
    }
}

/// What the page's search and kinds know of an open proposal.
pub fn open_item(proposal: &Proposal, coin: &Coin) -> Item {
    let kind = proposal.kind_name();
    let change = proposal.change();
    let accounts: Vec<String> = proposal.votes.iter().map(|vote| format!("{} {}", vote.account, vote.vote)).collect();

    Item {
        text: format!("{} {} {change} {}", label(&kind), describe(&change, coin), accounts.join(" ")),
        kind,
        place: proposal.votes.iter().map(|vote| vote.epoch).max().unwrap_or_default(),
        support: proposal.tally().support(),
        closes: proposal.closes.or_else(|| proposal.votes.iter().map(|vote| vote.counts_until).min()),
    }
}

/// What the page's search and kinds know of a decision.
pub fn decided_item(decision: &Decision, coin: &Coin) -> Item {
    let kind = decision.kind.as_deref().and_then(|kind| kind.parse::<ChangeKind>().ok()).map(|kind| kind.name().to_string()).or_else(|| decision.change.as_ref().and_then(change_kind)).unwrap_or_default();
    let (words, json) = decision.change.as_ref().map_or_else(Default::default, |change| (describe(change, coin), change.to_string()));

    Item {
        text: format!("{} {words} {json} {} {} block {}", label(&kind), decision.sender.as_deref().unwrap_or_default(), decision.tx, decision.height),
        kind,
        place: decision.version,
        support: 0.0,
        closes: None,
    }
}

/// What the page's search and kinds know of a rejection.
pub fn rejected_item(rejection: &Rejection, coin: &Coin) -> Item {
    let decision = Decision {
        version: rejection.rejection,
        of: rejection.of.clone(),
        kind_version: rejection.kind_version,
        tx: rejection.tx.clone(),
        block: rejection.block.clone(),
        height: rejection.height,
        timestamp: rejection.timestamp,
        confirmations: rejection.confirmations,
        finalized: rejection.finalized,
        sender: rejection.sender.clone(),
        kind: rejection.kind.clone(),
        change: rejection.change.clone(),
    };

    decided_item(&decision, coin)
}

/// What a kind's open proposals are for: version 2, so its change 3.
pub fn kind_heading(kind: &str, version: u64) -> String {
    format!("version {}: voting on {} change {}", format::thousands(&version.to_string()), kinds(kind), format::thousands(&(version + 1).to_string()))
}

/// Where a change's setting stands in force: the field its JSON names it by, or finality's for a
/// switch of its kind.
fn standing_of(view: &GovernanceView, change: &Value) -> (Value, Value) {
    let Some((field, value)) = change.as_object().and_then(|change| change.iter().next()) else {
        return (Value::Null, Value::Null);
    };

    match field.as_str() {
        "switch_finality" => (view.in_force.get("finality").map_or(Value::Null, |setting| setting.value.clone()), value["finality"].clone()),
        field => (view.in_force.get(field).map_or(Value::Null, |setting| setting.value.clone()), value.clone()),
    }
}

/// A proposal as its card shows it: what the explorer knows of it and of its voters.
pub fn card_of(view: &GovernanceView, proposal: &Proposal, coin: &Coin, names: &dyn Fn(&str) -> Option<String>, tip: Option<u64>) -> votes::Card {
    let kind = proposal.kind_name();
    let change = proposal.change();
    let tally = proposal.tally();
    let say = |raw: f64| weight(view, coin, &format!("{raw:.0}"));
    let raw = |name: &str| proposal.tally[name].as_str().map_or_else(|| "0".to_string(), str::to_string);
    let (before, after) = standing_of(view, &change);
    let block_ms = view.active["slot_ms"].as_u64().or_else(|| view.active["target_block_ms"].as_u64());
    let voters = view
        .voters
        .iter()
        .map(|voter| Dot {
            label: names(&voter.account).unwrap_or_else(|| voter.account.clone()),
            vote: proposal.vote_of(&voter.account),
            about: format!("{} weighs {}", voter.account, weight(view, coin, &voter.weight)),
        })
        .collect();

    votes::Card {
        title: label(&kind),
        subtitle: kind_heading(&kind, proposal.version),
        what: describe(&change, coin),
        weights: [weight(view, coin, &raw("yes")), weight(view, coin, &raw("no")), weight(view, coin, &raw("abstain"))],
        standing: tally.standing(&|more| match (more.fract() == 0.0, view.weighs_stake()) {
            (true, false) => format::thousands(&format!("{more:.0}")),
            _ => say(more),
        }),
        tally,
        closing: proposal.closes.zip(tip).map(|(closes, tip)| votes::closing(closes, tip, view.checkpoint_interval, block_ms)),
        voters,
        fields: votes::fields(&before, &after),
    }
}

fn proposal_row(ui: &mut Ui, ctx: &mut Ctx, view: &GovernanceView, proposal: &Proposal, index: usize, coin: &Coin) {
    let look = ctx.look;
    let store = ctx.store;
    let names = |account: &str| store.accounts.get(account).and_then(|found| found.name.clone());
    let card = card_of(view, proposal, coin, &names, store.status.as_ref().map(|status| status.height));
    let change = proposal.change();

    votes::card(ui, &look.palette, &card, look.narrow, |ui| {
        if card.fields.iter().any(votes::Field::changed) {
            egui::CollapsingHeader::new(RichText::new("Every field").color(look.palette.soft)).id_salt(("fields", index)).show(ui, |ui| {
                votes::diff(ui, &look.palette, &card.fields, true);
            });
        }

        ui.add_space(4.0);

        match look.narrow {
            true => {
                for vote in &proposal.votes {
                    ui.horizontal_wrapped(|ui| {
                        super::address(ui, ctx, &vote.account);
                        ui.label(RichText::new(format!("{} · {} · counts until epoch {}", vote_word(&vote.vote), weight(view, coin, &vote.weight), vote.counts_until)).small().color(look.palette.muted));
                    });
                }
            }
            false => {
                super::table(ui, look, ("votes", index), &["account", "vote", "weight", "cast in", "counts until"], |ui| {
                    for vote in &proposal.votes {
                        super::address(ui, ctx, &vote.account);
                        ui.label(RichText::new(vote_word(&vote.vote)).color(Choice::of(&vote.vote).map_or(look.palette.muted, |choice| choice.color(&look.palette))));
                        ui.monospace(weight(view, coin, &vote.weight));
                        ui.monospace(format!("epoch {}", vote.epoch));
                        ui.monospace(format!("epoch {}", vote.counts_until));
                        super::row_end(ui);
                    }
                });
            }
        }

        egui::CollapsingHeader::new(RichText::new("JSON").color(look.palette.soft)).id_salt(("proposal", index)).show(ui, |ui| {
            super::json_tree(ui, ctx, "change", &change);
        });
    });
}

/// A vote's word: Yes, No or Abstain.
fn vote_word(vote: &str) -> &'static str {
    Choice::of(vote).map_or("?", Choice::word)
}

/// Each voter: its name, what share of the changes put to a vote it voted on, its last vote, and,
/// where votes weigh stake, its share of it as a bar. Turnout is counted from each voter's own
/// votes, its newest page of them, against every change any voter's pages name.
fn turnout(ui: &mut Ui, ctx: &mut Ctx, view: &GovernanceView) {
    let look = ctx.look;
    let store = ctx.store;

    if view.voters.is_empty() {
        super::note(ui, look, "Nobody may vote now.");

        return;
    }

    for voter in view.voters.iter().take(MOST_NAMED) {
        if !store.accounts.contains_key(&voter.account) {
            ctx.api.ask(Ask::Account(voter.account.clone()));
        }
    }

    let names = |account: &str| store.accounts.get(account).and_then(|found| found.name.clone());
    let tiles = turnouts(view, &store.votes_cast, &names, ctx.now);
    votes::turnout(ui, &look.palette, &tiles, view.weighs_stake());

    if view.voters.len() > MOST_NAMED {
        super::note(ui, look, format!("The first {MOST_NAMED} of {} voters.", view.voters.len()));
    }

    super::note(ui, look, "Turnout counts each voter's newest votes against every change any voter's votes name; votes a multisig cast for its members count for the multisig.");
}

/// What the turnout asks: each voter's votes, while not read yet.
pub fn turnout_asks(store: &crate::store::Store) -> Vec<Ask> {
    let Some(view) = &store.governance else {
        return Vec::new();
    };

    view.voters
        .iter()
        .take(MOST_NAMED)
        .filter(|voter| !store.votes_cast.contains_key(&voter.account))
        .map(|voter| Ask::AccountTxs { address: voter.account.clone(), query: VOTES_QUERY.to_string(), cursor: None })
        .collect()
}

/// The change a vote was on, as one key: its kind's version and the change itself.
fn vote_key(tx: &crate::model::FoundTx) -> Option<String> {
    (tx.tx.kind() == "vote_change").then(|| format!("{} {}", tx.tx.tx["version"], tx.tx.tx["change"]))
}

/// Each voter's turnout, from the votes read: the share of every change any voter voted on that
/// it voted on too, its last vote, and its share of the electorate's weight.
pub fn turnouts(view: &GovernanceView, cast: &std::collections::HashMap<String, Vec<crate::model::FoundTx>>, names: &dyn Fn(&str) -> Option<String>, now: u64) -> Vec<Turnout> {
    let every: std::collections::BTreeSet<String> = cast.values().flatten().filter_map(vote_key).collect();
    let total = votes::number(&view.total).max(view.voters.iter().map(|voter| votes::number(&voter.weight)).sum());

    view.voters
        .iter()
        .take(MOST_NAMED)
        .map(|voter| {
            let theirs = cast.get(&voter.account);
            let voted: std::collections::BTreeSet<String> = theirs.into_iter().flatten().filter_map(vote_key).collect();
            let last = theirs.and_then(|txs| txs.iter().filter(|tx| tx.tx.kind() == "vote_change").max_by_key(|tx| (tx.height, tx.index))).map(|tx| {
                let kind = tx.tx.tx["kind"].as_str().map(str::to_string).or_else(|| change_kind(&tx.tx.tx["change"])).unwrap_or_default();
                let when = tx.time.map_or_else(|| format!("block {}", format::thousands(&tx.height.to_string())), |time| format::ago(time, now));

                format!("last: {} on {}, {when}", vote_word(tx.tx.tx["vote"].as_str().unwrap_or_default()), label(&kind).to_lowercase())
            });

            Turnout {
                label: names(&voter.account).unwrap_or_else(|| format::short(&voter.account)),
                share: votes::fraction(votes::number(&voter.weight), total),
                participation: theirs.map(|_| votes::fraction(voted.len() as f64, every.len() as f64)),
                last,
            }
        })
        .collect()
}

/// The decisions and rejections read, along the chain: each at its height, colored by kind, with
/// its kind's version, and the decisions so far as a line.
fn history(ui: &mut Ui, ctx: &mut Ctx, coin: &Coin) {
    let look = ctx.look;
    let store = ctx.store;
    let marks = marks(&store.decisions, &store.rejections, coin);

    if marks.is_empty() {
        super::note(ui, look, "Nothing decided or rejected yet.");

        return;
    }

    let mut kinds: Vec<String> = marks.iter().map(|mark| mark.kind.clone()).collect();
    kinds.sort();
    kinds.dedup();
    votes::timeline(ui, &look.palette, &marks, 76.0);
    votes::legend(ui, &look.palette, &kinds, &label);

    let points = votes::cumulative(&marks);

    if points.len() > 1 {
        ui.add_space(6.0);
        super::chart::chart(
            ui,
            look,
            &super::chart::Series {
                name: "Decisions so far, by block".to_string(),
                points,
                kind: super::chart::Kind::Line,
                color: look.roles.main,
                limit: None,
                unit: |count| format!("{} decided", count as u64),
                across: super::chart::block,
            },
            90.0,
        );
    }

    super::note(ui, look, "Of the decisions and rejections read so far: older pages below add theirs.");
}

/// The marks the timeline draws: a decision each and a rejection each, by its kind's name.
pub fn marks(decisions: &crate::store::Decisions, rejections: &crate::store::Rejections, coin: &Coin) -> Vec<Mark> {
    let kind_of = |of: &Option<String>, kind: &Option<String>, change: &Option<Value>| of.clone().or_else(|| kind.clone()).or_else(|| change.as_ref().and_then(change_kind)).unwrap_or_default();
    let decided = decisions.items.values().map(|decision| {
        let kind = kind_of(&decision.of, &decision.kind, &decision.change);
        let version = decision.kind_version.map_or_else(String::new, |version| format!(" v{version}"));

        Mark {
            height: decision.height,
            label: format!("{}{version}", label(&kind)),
            about: format!("Change {}: {}", decision.version, decision.change.as_ref().map_or_else(|| "a change the node could not read".to_string(), |change| describe(change, coin))),
            kind,
            rejected: false,
        }
    });
    let rejected = rejections.items.values().map(|rejection| {
        let kind = kind_of(&rejection.of, &rejection.kind, &rejection.change);

        Mark {
            height: rejection.height,
            label: format!("{} rejected", label(&kind)),
            about: format!("Rejection {}: {}", rejection.rejection, rejection.change.as_ref().map_or_else(|| "a change the node could not read".to_string(), |change| describe(change, coin))),
            kind,
            rejected: true,
        }
    });

    decided.chain(rejected).collect()
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

    // What each setting is takes what the name and who set it leave: a grid's cell keeps the width it
    // was first drawn at, which for text that wraps is next to nothing.
    let words = (ui.available_width() - IN_FORCE_BESIDE).max(IN_FORCE_LEAST);

    match look.narrow {
        true => {
            for (kind, setting) in &settings {
                super::list_row(ui, look, |ui| {
                    widgets::end_row(
                        ui,
                        |ui| {
                            ui.label(widgets::strong(label(kind), 13.5).color(look.palette.ink));

                            if locked_whole(view, kind) {
                                widgets::pill(ui, "locked", look.palette.soft);
                            }
                        },
                        |ui| set_by(ui, setting.decided),
                    );
                    ui.add(egui::Label::new(RichText::new(summary(kind, &setting.value, coin)).color(look.palette.soft)).wrap());
                });
            }
        }
        false => {
            super::table(ui, look, "in force", &["setting", "in force", "set by"], |ui| {
                for (kind, setting) in &settings {
                    // Each cell from the top of its row, however many lines what the setting is takes.
                    ui.vertical(|ui| {
                        ui.label(widgets::strong(label(kind), 13.0));

                        if locked_whole(view, kind) {
                            widgets::pill(ui, "locked", look.palette.soft);
                        }
                    });
                    ui.vertical(|ui| {
                        ui.set_width(words);
                        ui.add(egui::Label::new(summary(kind, &setting.value, coin)).wrap()).on_hover_text(setting.value.to_string());
                    });
                    ui.vertical(|ui| set_by(ui, setting.decided));
                    super::row_end(ui);
                }
            });
        }
    }
}

/// The settings no vote may change, each by its path, with what the lock holds.
fn locks(ui: &mut Ui, ctx: &mut Ctx, view: &GovernanceView) {
    let look = ctx.look;

    if view.locks.is_empty() {
        super::note(ui, look, "Nothing is locked: governance may change every setting above.");

        return;
    }

    super::note(ui, look, "No vote changes these, and nothing unlocks one: a change whose value at a locked path differs from what stands is refused.");

    for lock in &view.locks {
        super::list_row(ui, look, |ui| {
            widgets::end_row(ui, |ui| ui.label(RichText::new(lock).monospace().color(look.palette.ink)), |ui| {
                ui.label(RichText::new(lock_words(lock)).small().color(look.palette.muted));
            });
        });
    }
}

/// What a lock holds, in words: a whole setting, or one of its fields.
pub fn lock_words(lock: &str) -> String {
    match lock.split_once('.') {
        None if lock == "locks" => "the locks themselves: no more may be added".to_string(),
        None => format!("all of {}", lowered(&label(lock))),
        Some((kind, field)) => format!("{} of {}", field.replace('.', " "), lowered(&label(kind))),
    }
}

/// Whether a lock holds all of a setting.
fn locked_whole(view: &GovernanceView, kind: &str) -> bool {
    chainv4::governance::locks::whole(&view.locks, kind).is_some()
}

/// What the In force table's other columns take of its width, gaps included.
const IN_FORCE_BESIDE: f32 = 240.0;
/// The narrowest it sets what a setting is: past that, it scrolls across.
const IN_FORCE_LEAST: f32 = 220.0;

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

/// What the node lists of a decision or a rejection, for the light client's proof to be held to.
struct Listed<'a> {
    /// The chain's count of decisions or of rejections it made.
    number: u64,
    rejected: bool,
    tx: &'a str,
    block: &'a str,
    kind: Option<&'a str>,
    change: Option<&'a Value>,
    sender: Option<&'a str>,
}

impl<'a> From<&'a Decision> for Listed<'a> {
    fn from(decision: &'a Decision) -> Listed<'a> {
        Listed { number: decision.version, rejected: false, tx: &decision.tx, block: &decision.block, kind: decision.kind.as_deref(), change: decision.change.as_ref(), sender: decision.sender.as_deref() }
    }
}

impl<'a> From<&'a Rejection> for Listed<'a> {
    fn from(rejection: &'a Rejection) -> Listed<'a> {
        Listed { number: rejection.rejection, rejected: true, tx: &rejection.tx, block: &rejection.block, kind: rejection.kind.as_deref(), change: rejection.change.as_ref(), sender: rejection.sender.as_deref() }
    }
}

/// How a decision stands, from what the light client made of its receipt.
pub fn verdict(decision: &Decision, checked: Option<&Checked>, trouble: Option<&str>, hash: chainv4::structs::hash::HashType) -> Verdict {
    judged(&Listed::from(decision), checked, trouble, hash)
}

/// How a rejection stands, from what the light client made of its receipt.
pub fn rejection_verdict(rejection: &Rejection, checked: Option<&Checked>, trouble: Option<&str>, hash: chainv4::structs::hash::HashType) -> Verdict {
    judged(&Listed::from(rejection), checked, trouble, hash)
}

fn judged(listed: &Listed, checked: Option<&Checked>, trouble: Option<&str>, hash: chainv4::structs::hash::HashType) -> Verdict {
    if let Some(trouble) = trouble {
        return Verdict::Unchecked(format!("the light client stopped: {trouble}"));
    }

    match checked {
        None => Verdict::Checking,
        Some(Checked::Waiting(height)) => Verdict::Waiting(*height),
        Some(Checked::Refused(why)) => Verdict::Wrong(why.clone()),
        Some(Checked::Proven(proven)) => match agrees(listed, proven, hash) {
            Ok(()) => Verdict::Proven { final_: proven.assurance.final_ },
            Err(why) => Verdict::Wrong(why),
        },
    }
}

/// The change a transaction votes on: its own, or the one a multisig's transaction carries.
fn approved(tx: &Tx) -> Option<(u64, &chainv4::governance::Change)> {
    match tx {
        Tx::VoteChange { version, change, .. } => Some((*version, change)),
        Tx::ApproveMultisig { tx, .. } | Tx::Multisig { tx, .. } => approved(tx),
        _ => None,
    }
}

/// Whether what the light client proved is what the node lists: the receipt decided, or rejected,
/// this change of the chain's count, of the kind listed, in the block listed, and its transaction
/// voted on this change, of that kind, at the kind's version, sent by whom the node says.
fn agrees(listed: &Listed, proven: &ProvenReceipt, hash: chainv4::structs::hash::HashType) -> Result<(), String> {
    let block = proven.assurance.block.to_string();
    let (did, what) = match listed.rejected {
        false => ("decided", "change"),
        true => ("rejected", "rejection"),
    };

    if block != listed.block {
        return Err(format!("the node lists it in block {} but proves it in block {block}", format::short(listed.block)));
    }

    // A decision makes the kind's next version; a rejection leaves it where the vote found it.
    let (number, kind, made) = match (listed.rejected, &proven.receipt.outcome) {
        (false, CommittedOutcome::Decided { decision, kind, version }) => (*decision, *kind, version.saturating_sub(1)),
        (true, CommittedOutcome::Rejected { rejection, kind, version }) => (*rejection, *kind, *version),
        _ => return Err(format!("its receipt does not say it {did} {what} {}", listed.number)),
    };

    if number != listed.number {
        return Err(format!("its receipt does not say it {did} {what} {}", listed.number));
    }

    // The node lists a change by its own name, finality's under the consensus's kind.
    if let Some(named) = listed.kind
        && named.parse::<ChangeKind>() != Ok(kind)
    {
        return Err(format!("the node lists a change of {}, but its receipt {did} one of {}", named.replace('_', " "), kind.name().replace('_', " ")));
    }

    if proven.signed.id(hash).to_string() != listed.tx {
        return Err("the node proved another transaction".to_string());
    }

    let Some((version, change)) = approved(&proven.signed.tx) else {
        return Err("its transaction votes on no change".to_string());
    };

    if change.kind() != kind {
        return Err(format!("its transaction votes on a change of {}, but its receipt {did} one of {}", change.kind().name().replace('_', " "), kind.name().replace('_', " ")));
    }

    if version != made {
        return Err(format!("its transaction votes on {} change after version {version}, not after {made}", kinds(kind.name())));
    }

    let proven_change = chainv4::params::json::change_json(change);

    if let Some(named) = listed.change
        && *named != proven_change
    {
        return Err(format!("its transaction votes on {}, not the change the node lists", change.name()));
    }

    if let Some(sender) = listed.sender
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

    // A setting's link to a decision not read yet reads older pages till it is, whatever was being
    // searched for: then the page it is on shows.
    if let Some(focus) = state.focus {
        state.past = Finder::default();

        if listing.items.contains_key(&focus) {
            state.past.page = shown.iter().position(|item| item.decision.version == focus).unwrap_or(0) / PER_PAGE;
        } else {
            match listing.next() {
                Some(before) if focus < before => ctx.api.ask(Ask::Decisions { before: Some(before) }),
                _ => state.focus = None,
            }
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

    let items: Vec<Item> = shown.iter().map(|item| decided_item(&item.decision, coin)).collect();
    ballots::controls(ui, &look.palette, &mut state.past, "Search: kind, address, value or block", &items, &ballots::DECIDED_ORDERS, &label);

    let found = ballots::select(&items, &state.past);
    let older = listing.next().map(|before| Ask::Decisions { before: Some(before) });
    let reading = older.as_ref().is_some_and(|older| ctx.api.asking(older));

    // Reads on as the page shown needs it; a failure waits for someone to ask again.
    if let Some(older) = older.filter(|older| !store.failures.contains_key(older))
        && ballots::wants_older(found.len(), &state.past, PER_PAGE, true)
    {
        ctx.api.ask(older);
    }

    if found.is_empty() {
        super::note(ui, look, if listing.next().is_some() { "None found yet." } else { "No decision matches." });
    }

    for index in found[ballots::window(found.len(), state.past.page, PER_PAGE)].iter().copied() {
        let item = &shown[index];
        let focused = state.focus == Some(item.decision.version);
        let frame = egui::Frame::new().fill(if focused { look.palette.row } else { Color32::TRANSPARENT }).corner_radius(4.0);
        let response = frame.show(ui, |ui| super::list_row(ui, look, |ui| decision_row(ui, ctx, item, coin))).response;

        if focused {
            response.scroll_to_me(Some(egui::Align::Center));
            state.focus = None;
        }
    }

    let said = ballots::summary(found.len(), state.past.page, PER_PAGE, state.past.narrows(), shown.len(), listing.kept());
    ballots::pager(ui, &look.palette, &mut state.past, found.len(), listing.next().is_some(), reading, &said);
}

/// A rejection as the page shows it: what it would have changed, and how it stands.
struct ShownRejection {
    rejection: Rejection,
    change: Option<Value>,
    verdict: Verdict,
}

/// The rejections, newest first: each proven or not, and older ones a page at a time.
fn rejections(ui: &mut Ui, ctx: &mut Ctx, state: &mut GovernanceState, coin: &Coin) {
    let look = ctx.look;
    let store = ctx.store;
    let listing = &store.rejections;
    let ask = Ask::Rejections { before: None };

    if listing.bottom.is_none() {
        super::waiting(ui, ctx, &ask);

        return;
    }

    if listing.version == 0 {
        super::note(ui, look, "Governance has rejected nothing yet: a change is rejected once what has not voted No can no longer reach what it needs.");

        return;
    }

    let hash = ctx.light.hash();
    let trouble = ctx.light.trouble.clone();
    let shown: Vec<ShownRejection> = listing
        .newest_first()
        .map(|rejection| {
            let checked = ctx.light.receipts.get(&rejection.tx);
            let proven_change = match checked {
                Some(Checked::Proven(proven)) => approved(&proven.signed.tx).map(|(_, change)| chainv4::params::json::change_json(change)),
                _ => None,
            };
            let verdict = match hash {
                Some(hash) => rejection_verdict(rejection, checked, trouble.as_deref(), hash),
                None => Verdict::Unchecked("the light client has not started".to_string()),
            };

            ShownRejection {
                rejection: rejection.clone(),
                change: proven_change.filter(|_| matches!(verdict, Verdict::Proven { .. })).or_else(|| rejection.change.clone()),
                verdict,
            }
        })
        .collect();

    for (from, to) in listing.gaps() {
        ui.colored_label(look.palette.warn, rejection_gap(from, to));
    }

    let proven = shown.iter().filter(|shown| matches!(shown.verdict, Verdict::Proven { .. })).count();
    super::note(ui, look, format!("{proven} of the {} listed proven by the light client, against blocks it checked.", shown.len()));
    ui.add_space(4.0);

    let items: Vec<Item> = shown.iter().map(|item| rejected_item(&item.rejection, coin)).collect();
    ballots::controls(ui, &look.palette, &mut state.rejected, "Search: kind, address, value or block", &items, &ballots::DECIDED_ORDERS, &label);

    let found = ballots::select(&items, &state.rejected);
    let older = listing.next().map(|before| Ask::Rejections { before: Some(before) });
    let reading = older.as_ref().is_some_and(|older| ctx.api.asking(older));

    if let Some(older) = older.filter(|older| !store.failures.contains_key(older))
        && ballots::wants_older(found.len(), &state.rejected, PER_PAGE, true)
    {
        ctx.api.ask(older);
    }

    if found.is_empty() {
        super::note(ui, look, if listing.next().is_some() { "None found yet." } else { "No rejection matches." });
    }

    for index in found[ballots::window(found.len(), state.rejected.page, PER_PAGE)].iter().copied() {
        let item = &shown[index];
        super::list_row(ui, look, |ui| rejection_row(ui, ctx, item, coin));
    }

    let said = ballots::summary(found.len(), state.rejected.page, PER_PAGE, state.rejected.narrows(), shown.len(), listing.kept());
    ballots::pager(ui, &look.palette, &mut state.rejected, found.len(), listing.next().is_some(), reading, &said);
}

/// A run of rejections the node did not list, in words.
pub fn rejection_gap(from: u64, to: u64) -> String {
    match from == to {
        true => format!("Rejection {from} is missing: the node did not list it."),
        false => format!("Rejections {from} to {to} are missing: the node did not list them."),
    }
}

fn rejection_row(ui: &mut Ui, ctx: &mut Ctx, item: &ShownRejection, coin: &Coin) {
    let look = ctx.look;
    let rejection = &item.rejection;
    let what = match &item.change {
        Some(change) => describe(change, coin),
        None => "A change the node could not read".to_string(),
    };

    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(format!("#{}", rejection.rejection)).monospace().color(look.palette.muted));
        ui.label(widgets::strong(what, 13.5).color(look.palette.ink));
    });
    ui.horizontal_wrapped(|ui| {
        widgets::pill(ui, "rejected", look.palette.bad);
        let (text, color, about) = verdict_pill(look, &item.verdict);
        widgets::pill(ui, text, color).on_hover_text(about.replace("decided", "rejected"));

        if let Some(version) = rejection.kind_version {
            ui.label(RichText::new(format!("at {} version {version}", kinds(rejection.of.as_deref().unwrap_or("governance")))).small().color(look.palette.muted));
        }
    });

    if let Verdict::Wrong(why) = &item.verdict {
        ui.colored_label(look.palette.bad, why);
    }

    ui.horizontal_wrapped(|ui| {
        let muted = |text: &str| RichText::new(text).small().color(look.palette.muted);
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.label(muted("tx"));
        super::tx(ui, ctx, &rejection.tx);
        ui.label(muted("in block"));
        super::block_link(ui, ctx, &rejection.block, rejection.height);

        if let Some(sender) = &rejection.sender {
            ui.label(muted("from"));
            super::address(ui, ctx, sender);
        }

        ui.label(muted(&format!("· {} UTC · {}", format::utc(rejection.timestamp), format::ago(rejection.timestamp, ctx.now))));
    });
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
        Verdict::Proven { final_: true } => ("proven", look.palette.good, "The light client proved its receipt decided this change, and its transaction voted on it, in a final block it checked".to_string()),
        Verdict::Proven { final_: false } => ("proven", look.palette.good, "The light client proved its receipt decided this change, and its transaction voted on it, in a block it checked that is not final yet".to_string()),
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
        "names" => "Names".to_string(),
        "token_functions" => "Tokens".to_string(),
        "evm_spec" => "EVM spec".to_string(),
        "dex" => "DEX fee".to_string(),
        "reward" => "Block reward".to_string(),
        "switch_finality" => "Finality's kind".to_string(),
        "governance" => "Governance".to_string(),
        other => {
            let spaced = other.replace('_', " ");
            let mut chars = spaced.chars();

            chars.next().map_or_else(String::new, |first| first.to_uppercase().chain(chars).collect())
        }
    }
}

/// A change, as Tx::VoteChange's JSON carries it, { kind: value }, in words: what it does.
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
        "names" => format!("Price names at {said}"),
        "token_functions" => format!("Make tokens by {said}"),
        "evm_spec" => format!("Run the EVM by {said}"),
        "dex" => format!("Charge {said} a DEX trade"),
        "collections" => format!("Make collections by {said}"),
        "federation" => format!("Hand the bridges to {said}"),
        "directory" => format!("Announce {said}"),
        "fees" => format!("Set the fees: {said}"),
        "reward" => format!("Set the block reward to {said}"),
        "limits" => format!("Set the limits: {said}"),
        "finality" => format!("Retune finality: {said}"),
        "switch_finality" => format!("Switch finality to {said}"),
        "upgrade" => capitalized(&said),
        "governance" => format!("Set governance's own settings: {said}"),
        "locks" => format!("Lock for good: {said}"),
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
        "names" => {
            let prices: Vec<String> = value["prices"].as_array().into_iter().flatten().filter_map(Value::as_str).map(coins).collect();
            let days = value["grace_ms"].as_u64().unwrap_or_default() / 86_400_000;
            let prices = match prices.len() {
                0 => "nothing".to_string(),
                1..=4 => format!("{} a year, by length", prices.join(", ")),
                _ => format!("{}, … a year, by length", prices[..3].join(", ")),
            };

            format!("{prices}, {days} days of grace, at most {} years ahead", number("max_years"))
        }
        "token_functions" => match value.as_array().map(Vec::len) {
            Some(0) => "no token registry".to_string(),
            Some(count) => format!("{count} functions"),
            None => "?".to_string(),
        },
        "evm_spec" => format!("{} from block {}", text("spec"), number("from")),
        "dex" => format!("{}%", value["fee_bps"].as_u64().map_or_else(|| "?".to_string(), |bps| (bps as f64 / 100.0).to_string())),
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
        "governance" => {
            let quorum = match value["quorum_permille"].as_u64().unwrap_or_default() {
                0 => "no quorum".to_string(),
                permille => format!("a quorum of {}%", permille as f64 / 10.0),
            };
            let epochs = epochs(value["approval_epochs"].as_u64().unwrap_or_default());

            match value["type"].as_str() {
                Some("council") => format!("a council of {} members, {} voting Yes deciding, votes counting {epochs}, {quorum}", value["members"].as_array().map_or(0, Vec::len), number("threshold")),
                _ => format!("finality's voters, votes counting {epochs}, {quorum}"),
            }
        }
        "locks" => match value.as_array().map(|paths| paths.iter().filter_map(Value::as_str).collect::<Vec<_>>()) {
            Some(paths) if paths.is_empty() => "nothing".to_string(),
            Some(paths) => paths.join(", "),
            None => "?".to_string(),
        },
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
