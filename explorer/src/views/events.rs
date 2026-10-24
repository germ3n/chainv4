//! Events from the node's event index: a contract's, of one name or all, or those that mention an
//! address, newest first, a page at a time. A node keeps the index only when started with
//! --index-events, and only from the height it was turned on at.

use {
    super::{Ctx, tx},
    crate::api::{Ask, EventQuery},
    crate::format,
    egui::Ui,
};

/// How long the newest page stands before it is asked for again, in milliseconds.
const REFRESH: u64 = 3_000;

/// The events a query finds, with a button for older ones. The contract column shows only when
/// the query is not already about one contract.
pub fn list(ui: &mut Ui, ctx: &mut Ctx, query: &EventQuery) {
    let store = ctx.store;
    let newest = Ask::Events {
        query: query.clone(),
        cursor: None,
    };
    let found = store.events.get(query);
    let failed = store.failures.get(&newest);

    if failed.is_none() && found.is_none_or(|found| ctx.now.saturating_sub(found.refreshed) > REFRESH) {
        ctx.api.ask(newest.clone());
    }

    let found = match (found, failed) {
        (_, Some(failure)) if failure.status == 404 => {
            ui.weak("This node keeps no event index. Start it with --index-events to search events.");

            return;
        }
        (Some(found), _) => found,
        (None, _) => {
            super::waiting(ui, ctx, &newest);

            return;
        }
    };

    if found.events.is_empty() {
        ui.weak(format!("None since height {}, where this node's index starts.", found.indexed_from));

        return;
    }

    let by_contract = query.contract.is_some();

    egui::Grid::new(("events", query.contract.clone(), query.name.clone(), query.address.clone()))
        // The data goes last, so it gets the rest of the width.
        .num_columns(if by_contract { 4 } else { 5 })
        .striped(true)
        .show(ui, |ui| {
            for event in &found.events {
                super::block(ui, ctx, &event.block, &format::thousands(&event.height.to_string()));
                super::tx(ui, ctx, &event.tx);

                if !by_contract {
                    super::address(ui, ctx, &event.contract);
                }

                ui.strong(&event.name);
                tx::event_data(ui, ctx, &event.contract, &event.name, &event.data);
                ui.end_row();
            }
        });

    ui.horizontal(|ui| {
        if let Some(next) = &found.next {
            let older = Ask::Events {
                query: query.clone(),
                cursor: Some(next.clone()),
            };

            if ctx.api.asking(&older) {
                ui.spinner();
            } else if ui.button("Older events").clicked() {
                ctx.api.ask(older);
            }
        }

        ui.weak(format!("{} shown, indexed from height {}", found.events.len(), found.indexed_from));
    });
}
