//! Find events: what the node's event index holds by a signature or a topic, of one contract or of
//! any, and by an address, in an event's values or an EVM log's topics. Lists the same way the
//! account page does, and says the same of a node too old to do it.

use {
    crate::api::EventQuery,
    crate::views::{self, Ctx, events},
    egui::Ui,
};

#[derive(Default)]
pub struct FindState {
    pub topic: events::TopicFilter,
    /// The contract, or the address, as typed.
    pub contract: String,
    pub address: String,
}

impl FindState {
    /// What the fields ask for, an address first, as a topic and an address do not go together.
    pub fn query(&self) -> Option<EventQuery> {
        let (contract, address) = (self.contract.trim(), self.address.trim());

        match (&self.topic.topic, contract.is_empty(), address.is_empty()) {
            (_, _, false) => Some(EventQuery::mentioning(address)),
            (Some(topic), true, _) => Some(EventQuery::topic(topic, None)),
            (Some(topic), false, _) => Some(EventQuery::topic(topic, Some(contract))),
            (None, false, _) => Some(EventQuery::contract(contract, None)),
            (None, true, _) => None,
        }
    }
}

pub fn show(ui: &mut Ui, ctx: &mut Ctx, state: &mut FindState) {
    let look = ctx.look;
    let width = super::width(ctx);

    views::section(ui, look, "Find events", |ui| {
        views::note(ui, look, "EVM logs by their event's signature, such as Transfer(address,address,uint256), a built-in event's name, or a topic in any place; of one contract or of any. Or everything that names an address: in an event's values, or an EVM log's topics.");
        views::facts(ui, look, "find events", |facts| {
            facts.row("signature or topic", |ui| state.topic.field(ui, ctx.reader, width));
            facts.row("contract", |ui| {
                ui.add(egui::TextEdit::singleline(&mut state.contract).hint_text("evm:0x…, or any").desired_width(width));
            });
            facts.row("address", |ui| {
                ui.add(egui::TextEdit::singleline(&mut state.address).hint_text("instead: everything that names it").desired_width(width));
            });
        });
        state.topic.said(ui, ctx);
    });

    if let Some(query) = state.query() {
        views::section(ui, look, "Found", |ui| events::list(ui, ctx, &query));
    }
}
