//! Events from the node's event index: a contract's, of one name or all, or those that mention an
//! address, newest first, a page at a time. A node keeps the index only when started with
//! --index-events, and may still be indexing older blocks, or keep only the latest ones.

use {
    super::{Ctx, tx},
    crate::api::{Ask, EventQuery},
    crate::format,
    chainv4_ui::widgets,
    egui::{RichText, Ui},
};

/// How long the newest page stands before it is asked for again, in milliseconds.
const REFRESH: u64 = 3_000;

/// The events a query finds, with a button for older ones. The contract column shows only when
/// the query is not already about one contract.
pub fn list(ui: &mut Ui, ctx: &mut Ctx, query: &EventQuery) {
    let look = ctx.look;
    let store = ctx.store;
    let newest = Ask::Events {
        query: query.clone(),
        cursor: None,
    };
    let found = store.events.get(query);
    let failed = store.failures.get(&newest);

    if found.is_none_or(|found| ctx.now.saturating_sub(found.refreshed) > REFRESH) {
        ctx.api.ask(newest.clone());
    }

    let found = match (found, failed) {
        (_, Some(failure)) if failure.status == 404 => {
            super::note(ui, look, "This node keeps no event index. Start it with --index-events to search events.");

            return;
        }
        (Some(found), _) => found,
        (None, _) => {
            super::waiting(ui, ctx, &newest);

            return;
        }
    };

    // Still indexing older blocks, rather than dropping the oldest from a window.
    let still = found
        .catching_up_to
        .filter(|to| *to < found.indexed_from)
        .map(|to| format!(" It is still indexing older blocks, down to height {}.", format::thousands(&to.to_string())))
        .unwrap_or_default();

    if found.items.is_empty() {
        super::note(
            ui,
            look,
            format!("None since height {}, where this node's index starts.{still}", format::thousands(&found.indexed_from.to_string())),
        );

        return;
    }

    let by_contract = query.contract.is_some();

    match look.narrow {
        true => {
            for event in &found.items {
                super::list_row(ui, look, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(widgets::strong(&event.name, 13.5).color(look.palette.ink));

                        if !by_contract {
                            super::address(ui, ctx, &event.contract);
                        }
                    });
                    tx::event_data(ui, ctx, &event.contract, &event.name, &event.data);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("block").small().color(look.palette.muted));
                        super::block_link(ui, ctx, &event.block, event.height);
                        super::tx(ui, ctx, &event.tx);
                    });
                });
            }
        }
        false => {
            let mut headings = vec!["block", "transaction"];

            if !by_contract {
                headings.push("contract");
            }

            // The data goes last, so it gets the rest of the width.
            headings.extend(["event", "data"]);
            super::table(ui, look, ("events", query.contract.clone(), query.name.clone(), query.address.clone()), &headings, |ui| {
                for event in &found.items {
                    super::block_link(ui, ctx, &event.block, event.height);
                    super::tx(ui, ctx, &event.tx);

                    if !by_contract {
                        super::address(ui, ctx, &event.contract);
                    }

                    ui.label(widgets::strong(&event.name, 13.0));
                    tx::event_data(ui, ctx, &event.contract, &event.name, &event.data);
                    super::row_end(ui);
                }
            });
        }
    }

    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        if let Some(next) = &found.next {
            let older = Ask::Events {
                query: query.clone(),
                cursor: Some(next.clone()),
            };

            if ctx.api.asking(&older) {
                ui.spinner();
            } else if widgets::quiet(ui, &look.palette, "Older events").clicked() {
                ctx.api.ask(older);
            }
        }

        ui.label(
            RichText::new(format!("{} shown, indexed from height {}.{still}", found.items.len(), format::thousands(&found.indexed_from.to_string())))
                .small()
                .color(look.palette.muted),
        );
    });
}
