//! Events from the node's event index: a contract's, of one name or all, those that mention an
//! address, or EVM logs of a signature or topic, newest first, a page at a time. A node keeps the
//! index only when started with --index-events, and may still be indexing older blocks, or keep
//! only the latest ones. One older than topics ignores what it does not know, so what it answers is
//! not shown as if it were filtered.

use {
    super::{Ctx, tx},
    crate::api::{Ask, EventQuery, Failure},
    crate::format,
    crate::store::EventList,
    chainv4::abi::known::{AbiReader, KnownAbi, known_event},
    chainv4::abi::signature_topic,
    chainv4::structs::hex::{from_hex, to_hex},
    chainv4_ui::widgets,
    egui::{RichText, Ui},
};

/// How long the newest page stands before it is asked for again, in milliseconds.
const REFRESH: u64 = 3_000;

/// What to say of a node that cannot find by topic.
const TOO_OLD: &str = "This node is too old to find events by topic: it needs upgrading to do that. What it answered would not be filtered, so it is not shown.";

/// What to say of a node whose listing of an address leaves EVM logs out.
const NO_LOGS: &str = "This node is too old to find EVM logs by the addresses in their topics, so an ERC-20's or an NFT's transfers to or from here may not be listed.";

/// What a filter's field reads: a topic in hex, a signature, or the name of an event one of the ABIs
/// built in has. The topic in hex, and when that was worked out, the signature it stands for; or
/// None for an empty field.
pub fn read_topic(text: &str, known: &[KnownAbi]) -> Result<Option<(String, Option<String>)>, String> {
    let text = text.trim();

    if text.is_empty() {
        return Ok(None);
    }

    if text.starts_with("0x") {
        return from_hex(text)
            .filter(|bytes| bytes.len() == 32)
            .map(|bytes| Some((to_hex(&bytes), None)))
            .ok_or_else(|| format!("{text} is not a topic: 0x and 64 hex digits"));
    }

    if text.contains('(') {
        return signature_topic(text).map(|topic| Some((to_hex(&topic), Some(text.split_whitespace().collect()))));
    }

    known_event(known, text)
        .map(|event| Some((to_hex(&event.topic()), Some(event.signature()))))
        .ok_or_else(|| format!("no ABI built in has an event {text}: write its signature, such as {text}(address,uint256), or its topic"))
}

/// A field to filter EVM logs by a signature, a known event's name or a topic.
#[derive(Clone, Default)]
pub struct TopicFilter {
    typing: String,
    /// The topic the list shows, in hex, and the signature it was worked out from.
    pub topic: Option<String>,
    signature: Option<String>,
    trouble: Option<String>,
}

impl TopicFilter {
    /// The field: what it holds counts once it is left.
    pub fn field(&mut self, ui: &mut Ui, reader: &AbiReader, width: f32) {
        let field = ui.add(egui::TextEdit::singleline(&mut self.typing).hint_text("a signature, event or topic").desired_width(width));

        if field.lost_focus() {
            self.read(reader.known());
        }
    }

    fn read(&mut self, known: &[KnownAbi]) {
        match read_topic(&self.typing, known) {
            Ok(found) => {
                let (topic, signature) = found.unzip();
                (self.topic, self.signature, self.trouble) = (topic, signature.flatten(), None);
            }
            Err(why) => self.trouble = Some(why),
        }
    }

    /// What it was read as, or why it could not be.
    pub fn said(&self, ui: &mut Ui, ctx: &Ctx) {
        let look = ctx.look;

        if let Some(trouble) = &self.trouble {
            ui.add(egui::Label::new(RichText::new(trouble).color(look.palette.bad)).wrap());
        } else if let (Some(signature), Some(topic)) = (&self.signature, &self.topic) {
            ui.add(egui::Label::new(RichText::new(format!("{signature} · {topic}")).small().monospace().color(look.palette.muted)).wrap());
        }
    }
}

/// Whether a node has shown it cannot do what a query with a topic asks: it refused it, as an older
/// node refuses a topic alone, or answered without saying it knows topics, as one does a contract's
/// with one, listing all that contract's events.
pub fn too_old(query: &EventQuery, found: Option<&EventList>, failed: Option<&Failure>) -> bool {
    query.topic.is_some() && (failed.is_some_and(|failure| failure.status == 400) || found.is_some_and(|found| found.refreshed != 0 && !found.topics))
}

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

    if too_old(query, found, failed) {
        ui.add(egui::Label::new(RichText::new(TOO_OLD).color(look.palette.warn)).wrap());

        return;
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

    if query.address.is_some() && !found.topics {
        ui.add(egui::Label::new(RichText::new(NO_LOGS).small().color(look.palette.muted)).wrap());
    }

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
                        ui.label(widgets::strong(tx::event_name(ctx, &event.contract, &event.name, &event.data), 13.5).color(look.palette.ink));

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
            super::table(ui, look, ("events", query.contract.clone(), query.name.clone(), query.address.clone(), query.topic.clone()), &headings, |ui| {
                for event in &found.items {
                    super::block_link(ui, ctx, &event.block, event.height);
                    super::tx(ui, ctx, &event.tx);

                    if !by_contract {
                        super::address(ui, ctx, &event.contract);
                    }

                    ui.label(widgets::strong(tx::event_name(ctx, &event.contract, &event.name, &event.data), 13.0));
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

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::model::EventsPage,
        crate::store::Store,
        chainv4::abi::known::known_abis,
        serde_json::json,
    };

    const TRANSFER: &str = "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef";

    fn page(topics: bool) -> EventsPage {
        serde_json::from_value(json!({
            "events": [{
                "contract": "evm:0x05", "name": "Unfiltered", "data": [],
                "tx": "0x01", "block": "0x02", "height": 1, "index": 0, "position": 0,
            }],
            "next": null, "indexed_from": 0, "topics": topics,
        }))
        .unwrap()
    }

    fn stored(query: &EventQuery, topics: bool) -> Store {
        let mut store = Store::default();
        store.events.entry(query.clone()).or_default().take(page(topics), false, 1_999);

        store
    }

    fn drawn(store: &Store, query: &EventQuery) -> String {
        crate::views::render(store, &mut crate::light::Light::default(), 1_200.0, |ui, ctx| list(ui, ctx, query)).0
    }

    #[test]
    fn a_filter_reads_a_topic_a_signature_or_a_known_event_s_name() {
        let known = known_abis();

        assert_eq!(read_topic("  ", &known), Ok(None));
        assert_eq!(read_topic(TRANSFER, &known), Ok(Some((TRANSFER.to_string(), None))));
        assert_eq!(
            read_topic("Transfer(address, address, uint256)", &known),
            Ok(Some((TRANSFER.to_string(), Some("Transfer(address,address,uint256)".to_string())))),
        );
        assert_eq!(read_topic("Transfer", &known), Ok(Some((TRANSFER.to_string(), Some("Transfer(address,address,uint256)".to_string())))));
        assert!(read_topic("0x01", &known).unwrap_err().contains("not a topic"));
        assert!(read_topic("Nothing", &known).unwrap_err().contains("no ABI built in"));
        assert!(read_topic("Nothing(bad)", &known).is_err());
    }

    #[test]
    fn a_node_that_ignored_a_topic_is_told_apart_from_one_that_found_nothing() {
        let query = EventQuery::topic(TRANSFER, Some("evm:0x05"));
        let refused = Failure { status: 400, message: "ask for a contract's events".to_string() };
        let list = |topics: bool| {
            let mut list = EventList::default();
            list.take(page(topics), false, 1);

            list
        };

        assert!(too_old(&query, Some(&list(false)), None), "it answered without saying it knows topics");
        assert!(!too_old(&query, Some(&list(true)), None));
        assert!(too_old(&query, None, Some(&refused)), "a topic alone it refuses");
        assert!(!too_old(&query, None, None), "not asked yet");
        assert!(!too_old(&query, None, Some(&Failure { status: 404, message: "no index".to_string() })), "that is having no index");
        assert!(!too_old(&EventQuery::contract("evm:0x05", None), Some(&list(false)), None), "and without a topic nothing is missing");
    }

    #[test]
    fn what_an_older_node_answers_to_a_topic_is_not_shown_as_filtered() {
        let query = EventQuery::topic(TRANSFER, Some("evm:0x05"));
        let text = drawn(&stored(&query, false), &query);

        assert!(text.contains("too old to find events by topic"), "{text}");
        assert!(!text.contains("Unfiltered"), "every event of the contract, which is not what was asked: {text}");

        let text = drawn(&stored(&query, true), &query);

        assert!(text.contains("Unfiltered") && !text.contains("too old"), "{text}");
    }

    #[test]
    fn an_older_node_s_listing_of_an_address_says_what_it_leaves_out() {
        let query = EventQuery::mentioning("secp256k1:0x05");

        assert!(drawn(&stored(&query, false), &query).contains("too old to find EVM logs"));
        assert!(!drawn(&stored(&query, true), &query).contains("too old"));
        assert!(!drawn(&stored(&EventQuery::contract("evm:0x05", None), false), &EventQuery::contract("evm:0x05", None)).contains("too old"), "a contract's own listing misses nothing");
    }
}
