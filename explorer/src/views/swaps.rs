//! Atomic swaps through the chain's swap contract, contracts/lua/swap.lua: every lock, claim and
//! refund, newest first, from the event index, each lock with whether it is final yet, which is when
//! the other side may safely act on it. A swap is found by its hash, as the contract holds it, or by
//! its code, ur:chainv4-swap, which lays out both legs, the other chain's too.

use {
    super::Ctx,
    crate::api::{Ask, EventQuery},
    crate::format,
    crate::model::{Called, FoundEvent},
    chainv4::structs::u256::u256,
    chainv4::swap::{Leg, Lock, LockState, Settled, Side, Terms, chain_settled, evm},
    chainv4_ui::widgets,
    egui::{RichText, Ui},
    serde_json::json,
    std::collections::HashMap,
};

/// How long an answer stands before it is asked for again, in milliseconds.
const REFRESH: u64 = 10_000;
/// How many of the contract's events a page takes.
const EVENTS: u32 = 300;

#[derive(Default)]
pub struct SwapsState {
    /// A swap's hash, or its code, as typed.
    find: String,
    asked: HashMap<Ask, u64>,
}

/// A swap as the contract's events tell it.
#[derive(Clone, Debug, Default)]
struct Seen {
    state: &'static str,
    recipient: String,
    token: String,
    amount: String,
    /// The NFT it holds, from its LockedItem event: nft or multi, and the id.
    item: Option<(String, String)>,
    deadline: u64,
    /// Where it was locked, and the transaction that locked it.
    height: u64,
    block: String,
    locked_in: String,
}

pub fn show(ui: &mut Ui, ctx: &mut Ctx, state: &mut SwapsState) {
    let look = ctx.look;
    super::heading(ui, look, "Swaps", "Coins or tokens here for coins on another chain, with no one in between: each side locks under one secret's hash, and claiming one lock shows the secret for the other.", |_| {});
    let Some(contract) = contract(ctx) else {
        super::note(ui, look, "This chain names no swap contract: its directory names none, and neither does the node.");

        return;
    };

    super::section(ui, look, "Find a swap", |ui| {
        ui.add(egui::TextEdit::singleline(&mut state.find).hint_text("Its hash, 0x…, or its code, ur:chainv4-swap/…").desired_width(f32::INFINITY));
        let find = state.find.trim().to_string();

        if find.to_ascii_lowercase().starts_with("ur:") {
            match Terms::from_text(&find) {
                Ok(terms) => legs(ui, ctx, state, &terms),
                Err(why) => {
                    ui.colored_label(look.palette.bad, why);
                }
            }
        } else if find.len() == 66 && find.starts_with("0x") {
            lookup(ui, ctx, state, &contract, &find.to_ascii_lowercase());
        }
    });

    ui.add_space(10.0);
    super::section(ui, look, "Recent swaps", |ui| recent(ui, ctx, &contract));
}

/// The swap contract: the one the chain's directory names, or the one the node names.
fn contract(ctx: &Ctx) -> Option<String> {
    let status = ctx.store.status.as_ref()?;

    status.announced("swap").or_else(|| status.swap.as_ref()?.get("contract")?.as_str().map(str::to_string))
}

fn ask(ctx: &mut Ctx, state: &mut SwapsState, asked: Ask) {
    let stale = state.asked.get(&asked).is_none_or(|when| ctx.now.saturating_sub(*when) > REFRESH);

    if stale && !ctx.api.asking(&asked) {
        state.asked.insert(asked.clone(), ctx.now);
        ctx.api.ask(asked);
    }
}

/// A lock as a swap contract's get shows it, once it answered.
fn get(ctx: &mut Ctx, state: &mut SwapsState, contract: &str, hash: &str) -> Option<Result<Lock, String>> {
    let asked = Ask::Call { contract: contract.to_string(), function: "get".to_string(), args: json!([{ "type": "string", "value": hash }]).to_string() };
    ask(ctx, state, asked.clone());
    let called: Called = serde_json::from_value(ctx.store.calls.get(&asked)?.clone()).ok()?;

    let lock = match called.status.as_str() {
        "returned" => Lock::from_chain(&called.values.iter().map(|value| value.text()).collect::<Vec<_>>()),
        _ => Err(called.error.unwrap_or_else(|| "the contract would not say".to_string())),
    };

    // Which NFT it holds, from contracts that say: those from before NFTs have no item.
    let item = Ask::Call { contract: contract.to_string(), function: "item".to_string(), args: json!([{ "type": "string", "value": hash }]).to_string() };
    ask(ctx, state, item.clone());
    let item = ctx.store.calls.get(&item).and_then(|value| serde_json::from_value::<Called>(value.clone()).ok()).filter(|called| called.status == "returned");

    Some(match (lock, item) {
        (Ok(lock), Some(item)) => lock.clone().with_chain_item(&item.values.iter().map(|value| value.text()).collect::<Vec<_>>()).or(Ok(lock)),
        (lock, _) => lock,
    })
}

/// Whether a lock made at a height is final yet, as this chain reckons it.
fn finality(ctx: &Ctx, height: u64) -> Settled {
    let Some(status) = ctx.store.status.as_ref() else {
        return Settled::Unknown("the node's status is not in yet".to_string());
    };
    let finalized = status.finality.as_ref().map(|finality| finality.finalized.height.unwrap_or(0));
    let lock = Lock { state: LockState::Locked, recipient: None, token: None, amount: u256::ZERO, deadline: 0, secret: None, height: Some(height), item: None };

    chain_settled(&lock, status.height, finalized)
}

fn settled_text(ui: &mut Ui, ctx: &Ctx, settled: &Settled) {
    let palette = ctx.look.palette;
    let color = match settled {
        Settled::Final => palette.good,
        Settled::Waiting(_) => palette.warn,
        Settled::Unknown(_) => palette.muted,
    };
    ui.colored_label(color, settled.describe());
}

/// What is locked: an NFT of a collection, or an amount of an id of one, when it says which.
fn locked_item(ctx: &Ctx, token: &str, amount: &str, item: Option<(&str, &str)>) -> String {
    let collection = ctx
        .store
        .collections
        .as_ref()
        .and_then(|(collections, _)| collections.iter().find(|collection| collection.address == token).and_then(|collection| collection.title()))
        .unwrap_or_else(|| format::short(token));

    match item {
        Some(("nft", id)) => format!("NFT #{id} of {collection}"),
        Some((_, id)) => format!("{amount} of #{id} of {collection}"),
        None => locked(ctx, token, amount),
    }
}

/// What is locked, in the coin or a token: the zero key's address is the coin.
fn locked(ctx: &Ctx, token: &str, amount: &str) -> String {
    let zero = chainv4::structs::address::Address::Ed25519([0; 32]).to_string();

    match ctx.store.status.as_ref() {
        Some(status) if token == zero || token.is_empty() => format::amount(amount, status.coin.decimals, &status.coin.symbol),
        _ => match ctx.store.tokens.get(token) {
            Some(registered) => format::amount(amount, registered.decimals.unwrap_or(0), registered.symbol.as_deref().unwrap_or("units")),
            None => format!("{amount} units of {}", format::short(token)),
        },
    }
}

fn lookup(ui: &mut Ui, ctx: &mut Ctx, state: &mut SwapsState, contract: &str, hash: &str) {
    let look = ctx.look;

    match get(ctx, state, contract, hash) {
        None => {
            ui.spinner();
        }
        Some(Err(why)) => {
            ui.colored_label(look.palette.bad, why);
        }
        Some(Ok(lock)) if lock.state == LockState::None => super::note(ui, look, "Nothing is locked under that hash in this swap contract."),
        Some(Ok(lock)) => lock_facts(ui, ctx, &lock, "swap lookup"),
    }
}

fn lock_facts(ui: &mut Ui, ctx: &mut Ctx, lock: &Lock, id: &str) {
    let look = ctx.look;
    let token = lock.token.as_ref().map(|token| match token {
        chainv4::swap::Party::Chain(address) => address.to_string(),
        other => format!("{other:?}"),
    });
    let item = lock.item.map(|item| (item.kind(), item.id.to_string()));
    let what = locked_item(ctx, token.as_deref().unwrap_or_default(), &lock.amount.to_string(), item.as_ref().map(|(kind, id)| (*kind, id.as_str())));
    let recipient = match lock.recipient {
        Some(chainv4::swap::Party::Chain(address)) => address.to_string(),
        _ => "—".to_string(),
    };
    let settled = lock.height.map(|height| finality(ctx, height));

    super::facts(ui, look, id, |facts| {
        facts.row("state", |ui| {
            ui.label(widgets::strong(state_name(lock.state), 13.5));
        });
        facts.row("locks", |ui| {
            ui.label(what.clone());
        });
        facts.row("for", |ui| {
            ui.monospace(format::short(&recipient));
        });
        facts.row("deadline", |ui| {
            ui.label(format!("{} ({})", format::utc(lock.deadline), format::ago(lock.deadline, ctx.now)));
        });

        if let Some(settled) = &settled {
            facts.row("finality", |ui| settled_text(ui, ctx, settled));
        }

        if let Some(secret) = lock.secret {
            facts.row("secret", |ui| {
                ui.monospace(chainv4::structs::hex::to_hex(&secret));
            });
        }
    });
}

fn state_name(state: LockState) -> &'static str {
    match state {
        LockState::None => "none",
        LockState::Locked => "locked",
        LockState::Claimed => "claimed",
        LockState::Refunded => "refunded",
    }
}

/// A swap's two legs, from its code: each chain, what it locks, who pays and who claims, until
/// when; this chain's leg as its contract shows it.
fn legs(ui: &mut Ui, ctx: &mut Ctx, state: &mut SwapsState, terms: &Terms) {
    let look = ctx.look;
    ui.add_space(6.0);
    ui.label(RichText::new(format!("Hash {}", chainv4::structs::hex::to_hex(&terms.hash))).monospace().small().color(look.palette.muted));

    if !terms.agreed() {
        super::note(ui, look, "An offer: the other side's part is not in yet.");
    }

    for (side, leg) in terms.legs() {
        let title = match side {
            Side::First => format!("The starter's lock, on {}", leg.chain_name()),
            Side::Second => format!("The other's lock, on {}", leg.chain_name()),
        };
        ui.add_space(6.0);
        ui.label(widgets::strong(&title, 14.0).color(look.palette.ink));
        let deadline = terms.deadline(side);
        let place = match leg {
            Leg::Chain(chain) => format!("swap contract {}", format::short(&chain.contract.to_string())),
            Leg::Bitcoin(bitcoin) => terms.htlc().map_or("an HTLC, once both keys are in".to_string(), |htlc| format!("HTLC {}", htlc.address(bitcoin.network()))),
            Leg::Evm(on) => format!("Swap.sol {} on chain {}", evm::show_address(&on.contract), on.chain_id),
        };

        super::facts(ui, look, &format!("swap leg {side:?}"), |facts| {
            facts.row("locks", |ui| {
                ui.label(leg.what());
            });
            facts.row("in", |ui| {
                ui.monospace(place.clone());
            });
            facts.row("ends", |ui| {
                ui.label(format!("{} ({})", format::utc(deadline), format::ago(deadline, ctx.now)));
            });
        });

        // This chain's side, as its contract shows it.
        if let Leg::Chain(chain) = leg {
            match get(ctx, state, &chain.contract.to_string(), &chainv4::structs::hex::to_hex(&terms.hash)) {
                Some(Ok(lock)) if lock.state != LockState::None => {
                    let stands = leg.stands(&lock, deadline).map_or_else(|why| format!("not as the terms say: {why}"), |()| "as the terms say".to_string());
                    ui.label(RichText::new(format!("{}, {stands}", state_name(lock.state))).color(look.palette.soft));
                    lock_facts(ui, ctx, &lock, &format!("swap leg lock {side:?}"));
                }
                Some(Ok(_)) => super::note(ui, look, "Nothing locked here yet."),
                Some(Err(why)) => {
                    ui.colored_label(look.palette.bad, why);
                }
                None => {
                    ui.spinner();
                }
            }
        }
    }
}

/// The contract's swaps, newest first, by their events: what was locked, and what came of it.
fn recent(ui: &mut Ui, ctx: &mut Ctx, contract: &str) {
    let look = ctx.look;
    let query = EventQuery::contract(contract, None).limited(EVENTS);
    let newest = Ask::Events { query: query.clone(), cursor: None };
    let found = ctx.store.events.get(&query);

    if found.is_none_or(|found| ctx.now.saturating_sub(found.refreshed) > REFRESH) && !ctx.api.asking(&newest) && !ctx.store.failures.contains_key(&newest) {
        ctx.api.ask(newest.clone());
    }

    let Some(found) = ctx.store.events.get(&query).cloned() else {
        match ctx.store.failures.get(&newest) {
            Some(failure) if failure.status == 404 => super::note(ui, look, "This node keeps no event index: start it with --index-events to list swaps."),
            _ => super::waiting(ui, ctx, &newest),
        }

        return;
    };
    let swaps = swaps(&found.items);

    if swaps.is_empty() {
        super::note(ui, look, "No swaps yet.");

        return;
    }

    super::table(ui, look, ("swaps", contract.to_string()), &["hash", "state", "locks", "for", "deadline", "locked in", "finality"], |ui| {
        for (hash, seen) in swaps.iter().rev().take(50) {
            ui.monospace(format::short(hash));
            ui.label(widgets::strong(seen.state, 13.0));
            ui.label(locked_item(ctx, &seen.token, &seen.amount, seen.item.as_ref().map(|(kind, id)| (kind.as_str(), id.as_str()))));
            super::address(ui, ctx, &seen.recipient);
            ui.label(format::ago(seen.deadline, ctx.now));
            ui.horizontal(|ui| {
                super::block_link(ui, ctx, &seen.block, seen.height);
                super::tx(ui, ctx, &seen.locked_in);
            });

            match seen.state {
                "locked" => settled_text(ui, ctx, &finality(ctx, seen.height)),
                _ => {
                    ui.label(RichText::new("settled").color(look.palette.muted));
                }
            }

            ui.end_row();
        }
    });
}

/// Swaps by hash, from the contract's events, newest last: Locked(hash, locker, recipient, token,
/// amount, deadline), Claimed(hash, secret), Refunded(hash).
fn swaps(events: &[FoundEvent]) -> Vec<(String, Seen)> {
    let text = |event: &FoundEvent, index: usize| event.data.get(index).map(|value| value.text()).unwrap_or_default();
    let mut by_hash: HashMap<String, Seen> = HashMap::new();

    // Oldest first, so a claim or refund lands on its lock.
    for event in events.iter().rev() {
        let hash = text(event, 0).to_ascii_lowercase();

        match event.name.as_str() {
            "Locked" => {
                by_hash.insert(hash, Seen {
                    state: "locked",
                    recipient: text(event, 2),
                    token: text(event, 3),
                    amount: text(event, 4),
                    item: None,
                    deadline: text(event, 5).parse().unwrap_or(0),
                    height: event.height,
                    block: event.block.clone(),
                    locked_in: event.tx.clone(),
                });
            }
            "LockedItem" => {
                if let Some(seen) = by_hash.get_mut(&hash) {
                    seen.item = Some((text(event, 1), text(event, 2)));
                }
            }
            "Claimed" => {
                if let Some(seen) = by_hash.get_mut(&hash) {
                    seen.state = "claimed";
                }
            }
            "Refunded" => {
                if let Some(seen) = by_hash.get_mut(&hash) {
                    seen.state = "refunded";
                }
            }
            _ => {}
        }
    }

    // Oldest first, by where each was locked.
    let mut list: Vec<(String, Seen)> = by_hash.into_iter().collect();
    list.sort_by(|(one, first), (two, second)| (first.height, one).cmp(&(second.height, two)));

    list
}

#[cfg(test)]
mod tests;
