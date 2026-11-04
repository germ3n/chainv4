//! The chain's collection registry (docs/nft-standard.md): every contract that exports the NFT
//! standard, one owner a token, or the multi-token one, amounts of each id. A collection's page
//! looks a token up, lists what an address holds, and shows what moved, from the event index:
//! Transfer events for an NFT collection, TransferSingle for a multi-token one. Token URIs show as
//! text: the explorer fetches nothing from where they point.

use {
    super::{Ctx, View},
    crate::api::{Ask, EventQuery},
    crate::model::{Called, FoundEvent, RegisteredCollection},
    chainv4_ui::widgets,
    egui::{RichText, Ui},
    serde_json::{Value, json},
};

/// How long the registry, a read or a page of events stands before it is asked again, in
/// milliseconds.
const REFRESH: u64 = 15_000;
/// The most transfers a collection's page lists.
const EVENTS: u32 = 200;
/// The most ids asked of a collection for one holder: tokens_of's page.
const PAGE: u64 = 100;

#[derive(Default)]
pub struct CollectionsState {
    /// The id typed to look up.
    id: String,
    /// The holder typed, whose tokens to list.
    holder: String,
}

/// The registry, asking again once it is old.
pub fn registry(ctx: &mut Ctx) -> Option<Vec<RegisteredCollection>> {
    let stale = ctx.store.collections.as_ref().is_none_or(|(_, when)| ctx.now.saturating_sub(*when) > ctx.settings.pace().every_ms(REFRESH));

    if stale {
        ctx.api.ask(Ask::Collections(None));
    }

    ctx.store.collections.as_ref().map(|(collections, _)| collections.clone())
}

/// What a read-only call returned, as text, once it answered: Err with the contract's refusal.
fn called(ctx: &mut Ctx, contract: &str, function: &str, args: Value) -> Option<Result<Vec<String>, String>> {
    let asked = Ask::Call { contract: contract.to_string(), function: function.to_string(), args: args.to_string() };

    if !ctx.store.calls.contains_key(&asked) && !ctx.api.asking(&asked) && !ctx.store.failures.contains_key(&asked) {
        ctx.api.ask(asked.clone());
    }

    let called: Called = serde_json::from_value(ctx.store.calls.get(&asked)?.clone()).ok()?;

    Some(match called.status.as_str() {
        "returned" => Ok(called.values.iter().map(|value| value.text()).collect()),
        _ => Err(called.error.unwrap_or_else(|| "the call failed".to_string())),
    })
}

/// The ids a holder holds of a collection, as tokens_of says, once it answered.
pub fn held(ctx: &mut Ctx, collection: &str, holder: &str) -> Option<Vec<String>> {
    let args = json!([
        { "type": "address", "value": holder },
        { "type": "u256", "value": "0" },
        { "type": "u256", "value": PAGE.to_string() },
    ]);

    called(ctx, collection, "tokens_of", args)?.ok()
}

fn id_arg(id: &str) -> Value {
    json!({ "type": "u256", "value": id })
}

pub fn list(ui: &mut Ui, ctx: &mut Ctx) {
    let look = ctx.look;
    super::heading(ui, look, "NFTs", "The chain's collections: NFTs, one owner a token, and multi-token ones, amounts of each id.", |_| {});

    if ctx.store.status.as_ref().is_some_and(|status| status.collections.is_none()) {
        super::note(ui, look, "This chain keeps no collection registry: neither its genesis nor its governance names what makes one.");

        return;
    }

    let Some(collections) = registry(ctx) else {
        super::waiting(ui, ctx, &Ask::Collections(None));

        return;
    };

    super::note(ui, look, "Names and symbols are each collection's own, as it answered when deployed or last upgraded; symbols are not unique.");
    ui.add_space(8.0);
    super::section(ui, look, "Registered", |ui| {
        if collections.is_empty() {
            super::note(ui, look, "No collections yet.");

            return;
        }

        let none = |text: &str| RichText::new(text.to_string()).color(look.palette.muted);

        match look.narrow {
            true => {
                for collection in &collections {
                    super::list_row(ui, look, |ui| {
                        ui.horizontal(|ui| {
                            open_link(ui, ctx, collection);
                            ui.label(none(kind(collection)));
                        });
                        super::address(ui, ctx, &collection.address);
                    });
                }
            }
            false => {
                super::table(ui, look, "collections", &["collection", "kind", "symbol", "contract"], |ui| {
                    for collection in &collections {
                        open_link(ui, ctx, collection);
                        ui.label(none(kind(collection)));
                        ui.label(collection.symbol.as_deref().unwrap_or("—"));
                        super::address(ui, ctx, &collection.address);
                        super::row_end(ui);
                    }
                });
            }
        }
    });
}

fn kind(collection: &RegisteredCollection) -> &'static str {
    match collection.multi() {
        true => "multi-token",
        false => "NFT",
    }
}

/// A collection's name, opening its page.
pub fn open_link(ui: &mut Ui, ctx: &mut Ctx, collection: &RegisteredCollection) {
    let title = collection.title().unwrap_or_else(|| crate::format::short(&collection.address));

    if ui.link(widgets::strong(&title, 13.5)).on_hover_text(&collection.address).clicked() {
        ctx.go(View::Collection(collection.address.clone()));
    }
}

pub fn show(ui: &mut Ui, ctx: &mut Ctx, state: &mut CollectionsState, address: &str) {
    let look = ctx.look;
    let found = registry(ctx).and_then(|collections| collections.into_iter().find(|collection| collection.address == address));
    let Some(collection) = found else {
        super::heading(ui, look, "Collection", address, |_| {});

        match ctx.store.collections.is_some() {
            true => super::note(ui, look, "The collection registry does not list this contract."),
            false => super::waiting(ui, ctx, &Ask::Collections(None)),
        }

        return;
    };
    let title = collection.title().unwrap_or_else(|| "Collection".to_string());
    super::heading(ui, look, &title, kind(&collection), |_| {});

    super::section(ui, look, "Collection", |ui| {
        super::facts(ui, look, "collection facts", |facts| {
            facts.text("Kind", match collection.multi() {
                true => "Multi-token: holders hold amounts of each id",
                false => "NFT: each token has one owner",
            });
            facts.text("Name", collection.name.clone().unwrap_or_else(|| "—".to_string()));
            facts.text("Symbol", collection.symbol.clone().unwrap_or_else(|| "—".to_string()));
            facts.row("Contract", |ui| super::address(ui, ctx, &collection.address));

            if !collection.multi()
                && let Some(Ok(supply)) = called(ctx, &collection.address, "total_supply", json!([]))
                && let Some(supply) = supply.first()
            {
                facts.text("Tokens", crate::format::thousands(supply));
            }
        });
    });

    ui.add_space(10.0);
    super::section(ui, look, "Look up a token", |ui| {
        ui.add(egui::TextEdit::singleline(&mut state.id).hint_text("its id, such as 7").desired_width(if look.narrow { f32::INFINITY } else { 200.0 }));
        let id = state.id.trim().to_string();

        if id.is_empty() {
            return;
        }

        if id.parse::<u128>().is_err() && chainv4::structs::u256::u256::from_str_radix(&id, 10).is_err() {
            super::note(ui, look, "An id is a whole number.");

            return;
        }

        token(ui, ctx, &collection, &id);
    });

    ui.add_space(10.0);
    super::section(ui, look, "Held by", |ui| {
        ui.add(egui::TextEdit::singleline(&mut state.holder).hint_text("an address, such as ed25519:0x…").desired_width(f32::INFINITY));
        let holder = state.holder.trim().to_string();

        if holder.is_empty() {
            return;
        }

        if !super::is_address(&holder) {
            super::note(ui, look, "That is not an address.");

            return;
        }

        holdings(ui, ctx, &collection, &holder);
    });

    ui.add_space(10.0);
    super::section(ui, look, "Transfers", |ui| transfers(ui, ctx, &collection));
}

/// One token: its owner, or how many exist, and its URI.
fn token(ui: &mut Ui, ctx: &mut Ctx, collection: &RegisteredCollection, id: &str) {
    let look = ctx.look;
    let first = |answer: Option<Result<Vec<String>, String>>| answer.map(|answer| answer.map(|values| values.into_iter().next().unwrap_or_default()));
    let held = match collection.multi() {
        false => first(called(ctx, &collection.address, "owner_of", json!([id_arg(id)]))),
        true => first(called(ctx, &collection.address, "total_supply", json!([id_arg(id)]))),
    };
    let uri = first(called(ctx, &collection.address, "token_uri", json!([id_arg(id)])));

    super::facts(ui, look, "token facts", |facts| match held {
        None => facts.row("Token", |ui| {
            ui.spinner();
        }),
        Some(Err(why)) => facts.text("Token", why),
        Some(Ok(held)) => {
            match collection.multi() {
                false => facts.row("Owner", |ui| super::address(ui, ctx, &held)),
                true => facts.text("In existence", crate::format::thousands(&held)),
            }

            if let Some(Ok(uri)) = &uri {
                facts.row("Token URI", |ui| {
                    super::copyable(ui, uri);
                });
            }
        }
    });
}

/// What a holder holds of a collection: its ids, and of a multi-token one, how many of each.
fn holdings(ui: &mut Ui, ctx: &mut Ctx, collection: &RegisteredCollection, holder: &str) {
    let look = ctx.look;
    let Some(ids) = held(ctx, &collection.address, holder) else {
        ui.spinner();

        return;
    };

    if ids.is_empty() {
        super::note(ui, look, "Nothing of this collection.");

        return;
    }

    super::table(ui, look, ("held", holder.to_string()), if collection.multi() { &["id", "amount"] } else { &["id"] }, |ui| {
        for id in &ids {
            ui.monospace(id);

            if collection.multi() {
                let args = json!([{ "type": "address", "value": holder }, id_arg(id)]);

                match called(ctx, &collection.address, "balance_of", args) {
                    Some(Ok(amount)) => ui.monospace(amount.first().cloned().unwrap_or_default()),
                    Some(Err(why)) => ui.label(why),
                    None => ui.spinner(),
                };
            }

            super::row_end(ui);
        }
    });

    if ids.len() as u64 >= PAGE {
        super::note(ui, look, format!("The first {PAGE} shown."));
    }
}

/// A transfer, as a collection's events say: who from and to, which id, how many.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Moved {
    pub from: String,
    pub to: String,
    pub id: String,
    pub amount: String,
    pub height: u64,
    pub block: String,
    pub tx: String,
}

/// A collection's event as a transfer: Transfer(from, to, id) for an NFT collection,
/// TransferSingle(operator, from, to, id, amount) for a multi-token one.
pub fn moved(event: &FoundEvent, multi: bool) -> Option<Moved> {
    let text = |index: usize| event.data.get(index).map(|argument| argument.text());
    let (from, to, id, amount) = match (multi, event.name.as_str()) {
        (false, "Transfer") => (text(0)?, text(1)?, text(2)?, "1".to_string()),
        (true, "TransferSingle") => (text(1)?, text(2)?, text(3)?, text(4)?),
        _ => return None,
    };

    Some(Moved { from, to, id, amount, height: event.height, block: event.block.clone(), tx: event.tx.clone() })
}

fn transfers(ui: &mut Ui, ctx: &mut Ctx, collection: &RegisteredCollection) {
    let look = ctx.look;
    let name = if collection.multi() { "TransferSingle" } else { "Transfer" };
    let query = EventQuery::contract(&collection.address, Some(name)).limited(EVENTS);
    let newest = Ask::Events { query: query.clone(), cursor: None };
    let found = ctx.store.events.get(&query);

    if found.is_none_or(|found| ctx.now.saturating_sub(found.refreshed) > ctx.settings.pace().every_ms(REFRESH)) {
        ctx.api.ask(newest.clone());
    }

    let Some(events) = ctx.store.events.get(&query).map(|found| found.items.clone()) else {
        match ctx.store.failures.get(&newest) {
            Some(failure) if failure.status == 404 => super::note(ui, look, "The node keeps no event index: start it with --index-events to see what moved."),
            _ => super::waiting(ui, ctx, &newest),
        }

        return;
    };
    let moves: Vec<Moved> = events.iter().filter_map(|event| moved(event, collection.multi())).collect();

    if moves.is_empty() {
        super::note(ui, look, "Nothing moved yet.");

        return;
    }

    let zero = chainv4::structs::address::Address::Ed25519([0; 32]).to_string();
    let party = |ui: &mut Ui, ctx: &mut Ctx, text: &str, what: &str| match text == zero {
        true => {
            ui.label(RichText::new(what).color(look.palette.muted));
        }
        false => super::address(ui, ctx, text),
    };

    if look.narrow {
        for moved in &moves {
            super::list_row(ui, look, |ui| {
                ui.horizontal(|ui| {
                    ui.label(widgets::strong(format!("#{}", moved.id), 13.5).color(look.palette.ink));

                    if collection.multi() {
                        ui.label(RichText::new(format!("× {}", moved.amount)).monospace());
                    }

                    super::tx(ui, ctx, &moved.tx);
                });
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    party(ui, ctx, &moved.from, "minted");
                    ui.label(RichText::new("→").color(look.palette.muted));
                    party(ui, ctx, &moved.to, "burned");
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("block").small().color(look.palette.muted));
                    super::block_link(ui, ctx, &moved.block, moved.height);
                });
            });
        }

        return;
    }

    super::table(ui, look, ("transfers", &collection.address), &["block", "from", "to", "id", "amount", "transaction"], |ui| {
        for moved in &moves {
            super::block_link(ui, ctx, &moved.block, moved.height);
            party(ui, ctx, &moved.from, "minted");
            party(ui, ctx, &moved.to, "burned");
            ui.monospace(&moved.id);
            ui.monospace(&moved.amount);
            super::tx(ui, ctx, &moved.tx);
            super::row_end(ui);
        }
    });
}

#[cfg(test)]
mod tests;
