//! Read from checked blocks, on the pages: the setting itself, with everything it costs and cannot do
//! said plainly; where the reading stands; reading further back; what a page says of something that
//! was not read; what every page turned off by it says; and the press that takes a direct proof, which
//! says that the node sees what it is of.

use {
    super::{Ctx, View},
    crate::api::NEEDS_ASKING,
    crate::format,
    crate::reading::{Further, ProvenValue, Standing},
    chainv4::db::Column,
    chainv4_ui::widgets,
    egui::{RichText, Ui},
};

/// Why a page needs the node, and what to call it: None for a page that works from blocks, or wants
/// no chain, or has a part it keeps and says itself what it left. Every page is named, with no
/// catch-all, so that a new one has to be decided here before it builds.
pub fn turned_off(view: &View) -> Option<(&'static str, &'static str)> {
    match view {
        View::Mempool => Some(("Mempool", "What waits for a block is known only to the node")),
        View::Network => Some(("Network", "A node's peers and what it heard are that node's word")),
        View::Collections | View::Collection(_) => Some(("NFTs", "Collections and their tokens are listed by the node, and read by running contracts on it")),
        View::Dex => Some(("DEX", "A pool's prices and reserves are read by running its contract on the node")),
        View::Swaps => Some(("Swaps", "A swap's locks are read by running the contract on the node")),
        View::Bridges | View::Transfer(_) | View::Release(_) => Some(("Bridges", "A bridge's gateways are read by running them on the node")),
        View::Governance => Some(("Governance", "What stands, what is open and what was decided are listed by the node")),
        View::Pool => Some(("Mining pool", "What a pool owes is known only to its node")),
        View::Multisig(_) => Some(("Multisig", "A multisig's members and proposals come from the node")),
        View::Faucet => Some(("Faucet", "A faucet is its node's, and takes the address it pays")),
        View::Overview
        | View::Map
        | View::Block(_)
        | View::Tx(_)
        | View::Account(_)
        | View::Name(_)
        | View::Tokens
        | View::Finality
        | View::Send
        | View::Settings
        | View::More
        | View::Tools
        | View::Names
        | View::Rich => None,
    }
}

/// What a page turned off says: the same plain words on each, and where to turn it back on.
pub fn needs_asking(ui: &mut Ui, ctx: &mut Ctx, title: &str, why: &str) {
    let look = ctx.look;
    super::heading(ui, look, title, "Read from checked blocks is on.", |_| {});
    turned_off_card(ui, ctx, why);
}

/// The card that says so, for a part of a page.
pub fn turned_off_card(ui: &mut Ui, ctx: &mut Ctx, why: &str) {
    if widgets::needs_asking(ui, &ctx.look.palette, &format!("{why}: {NEEDS_ASKING}.")) {
        ctx.go(View::Settings);
    }
}

/// What the page keeps between frames.
#[derive(Default)]
pub struct ReadingState {
    /// A height being typed, to read further back to.
    height: String,
    trouble: Option<String>,
    /// Asked once whether to forget what was read.
    forgetting: bool,
    /// A smaller cap on what it keeps, waiting to be asked whether to let the rest go.
    cap_pending: Option<u64>,
}

/// The caps on what it keeps that Settings offers: crate::reading::MOST_BYTES is the usual, 32 MB on
/// a phone or in a browser and 128 MB on a desktop.
const BYTES_OFFERED: [(u64, &str); 5] = [(16_000_000, "16 MB"), (32_000_000, "32 MB"), (128_000_000, "128 MB"), (512_000_000, "512 MB"), (2_000_000_000, "2 GB")];

/// How much it keeps: one of BYTES_OFFERED.
fn cap(ui: &mut Ui, ctx: &mut Ctx, state: &mut ReadingState) {
    let palette = ctx.look.palette;
    let (held, _) = ctx.reading.bytes_kept();
    let mut set = ctx.settings.reading_bytes();

    if widgets::reading_cap(ui, &palette, &mut set, &BYTES_OFFERED, held, &mut state.cap_pending) {
        ctx.settings.reading_bytes = Some(set);
    }

    let usual = widgets::bytes_said(crate::reading::MOST_BYTES);
    ui.add(egui::Label::new(RichText::new(format!("of blocks, {usual} as usual on this device. Past it the oldest go, and it says from what height it knows. All of it is in memory while the explorer is open.")).small().color(palette.muted)).wrap());
}

/// Where the checks hold against whoever serves a page.
const APPS_THAT_HOLD: &str = "the desktop and phone apps";

/// What only the explorer has to say of its limits.
const EXPLORER_LIMITS: &[&str] = &[
    "It follows no state from a block's entries, so neither a gap nor something left out can leave it showing a value out of date. A balance or a record shows only from a direct proof you press for, with the block it was proven at. The node sees what each is of.",
    "Only final blocks are read, so the newest few are not shown, nor is anything waiting in the mempool.",
    "A block commits to what a transaction did, not to why it failed, nor to the hops its contracts made: neither is shown.",
    "It keeps each block but for its state entries, about 400 bytes an empty block, up to the cap set below: as usual 128 MB on a desktop, 32 MB on a phone or in a browser, about 18 or 4 days of the devnet's empty blocks. Past the cap the oldest blocks go, and it says from what height it knows now.",
    "What it read is kept on this device, private to this app, and not sealed, as the explorer has no vault: it holds no secrets, but whoever has this device can see what height it reads from. What you search for is kept nowhere. Losing it costs a rescan.",
    "Off while this is on: the mempool, the network, tokens' lists, NFTs, the DEX, swaps, bridges, multisigs, governance, the mining pool, the rich list and the faucet. The coin's name and decimals are as a node last said them on this device, or amounts show as plain units.",
    "Fast resume is off while this is on, and a node met for the first time gives no checkpoint: the light client walks every header, each time the app opens, unless you pasted a checkpoint.",
];

/// The setting, in Settings: everything it does, costs and cannot do, then where it stands.
pub fn card(ui: &mut Ui, ctx: &mut Ctx, state: &mut ReadingState, connect: &mut Option<String>) {
    let look = ctx.look;
    let palette = look.palette;
    let node = ctx.settings.node.clone();

    super::section(ui, look, "Read from checked blocks", |ui| {
        let mut on = ctx.settings.read_blocks;

        if widgets::read_blocks_switch(ui, &palette, &mut on, "explorer") {
            ctx.settings.read_blocks = on;
            *connect = Some(node.clone());
        }

        if let Some(warning) = widgets::page_warning(widgets::page_origin().as_deref(), &node, "explorer", APPS_THAT_HOLD) {
            ui.add_space(4.0);
            ui.add(egui::Label::new(RichText::new(warning).small().color(palette.warn)).wrap());
        }

        ui.add_space(4.0);
        egui::CollapsingHeader::new(RichText::new("What it cannot do").color(palette.soft)).id_salt("reading limits").show(ui, |ui| {
            for limit in widgets::READ_BLOCKS_LIMITS.iter().chain(EXPLORER_LIMITS) {
                ui.add(egui::Label::new(RichText::new(*limit).small().color(palette.muted)).wrap());
                ui.add_space(4.0);
            }
        });
        ui.add_space(4.0);
        cap(ui, ctx, state);

        if ctx.settings.read_blocks {
            ui.add_space(6.0);
            standing(ui, ctx, state);
        }
    });
}

fn thousands(height: u64) -> String {
    format::thousands(&height.to_string())
}

/// Where the reading stands, in full: what Settings shows.
fn standing(ui: &mut Ui, ctx: &mut Ctx, state: &mut ReadingState) {
    let look = ctx.look;
    let palette = look.palette;
    let Some(client) = ctx.light.client() else {
        ui.label(RichText::new("Waiting for the light client to start.").small().color(palette.muted));

        return;
    };

    if client.params().finality.is_none() {
        ui.colored_label(palette.bad, NO_FINALITY);

        return;
    }

    let final_height = client.final_height();
    let Some(standing) = ctx.reading.standing() else {
        ui.label(RichText::new(match ctx.reading.is_open() {
            true => "Waiting for the light client to catch up: then it starts at the final block.",
            false => "Opening what it read before.",
        }).small().color(palette.muted));

        return;
    };

    rows(ui, ctx, state, &standing, final_height);
}

const NO_FINALITY: &str = "This chain has no finality, so it has no final blocks to read, and nothing can be shown from them. Turn this setting off to use it.";

/// How far it has read, what that cost, what it has to say, and what can be done about it.
fn rows(ui: &mut Ui, ctx: &mut Ctx, state: &mut ReadingState, standing: &Standing, final_height: u64) {
    let look = ctx.look;
    let palette = look.palette;

    super::facts(ui, look, "reading", |facts| {
        facts.text("read to", format!("block {} of {} final", thousands(standing.high), thousands(final_height)));
        facts.text("knows from", format!("block {}", thousands(standing.known_from)));

        if standing.left_below > 0 {
            facts.text("still to read", format!("{} blocks, back to {}", thousands(standing.left_below), thousands(standing.floor)));
        }

        facts.text("downloaded", format!("{}, {} blocks", widgets::bytes_said(standing.downloaded), thousands(standing.read)));
        facts.text("keeps", format!("{} of at most {}, {} blocks", widgets::bytes_said(standing.bytes), widgets::bytes_said(standing.most), thousands(standing.blocks)));
    });

    said_of(ui, ctx, standing);

    if !ctx.reading.lasts() {
        ui.colored_label(palette.warn, "There is nowhere on this device to keep what it read, so it reads again on each start.");
    }

    if let Some(why) = ctx.reading.keeping_trouble() {
        ui.add(egui::Label::new(RichText::new(format!("What it read could not be kept: {why}")).small().color(palette.warn)).wrap());
    }

    ui.add_space(6.0);
    further_back(ui, ctx, state, standing);
    ui.add_space(6.0);

    match state.forgetting {
        false => {
            if widgets::quiet(ui, &palette, "Forget what it read").on_hover_text("Start again at the final block").clicked() {
                state.forgetting = true;
            }
        }
        true => {
            ui.add(egui::Label::new(RichText::new("What it read goes, and it starts again at the final block, which costs its download again for anything read further back.").small().color(palette.warn)).wrap());
            ui.horizontal_wrapped(|ui| {
                if widgets::quiet(ui, &palette, "Yes, forget it").clicked() {
                    state.forgetting = false;
                    ctx.reading.forget();
                }

                if widgets::quiet(ui, &palette, "Keep it").clicked() {
                    state.forgetting = false;
                }
            });
        }
    }
}

/// Reading further back: what it costs, and the height to go to.
fn further_back(ui: &mut Ui, ctx: &mut Ctx, state: &mut ReadingState, standing: &Standing) {
    let palette = ctx.look.palette;

    ui.add(egui::Label::new(RichText::new(format!(
        "It knows from block {}, and shows nothing older as known. To read further back, give a height: each block costs its download, about 2 KB an empty one and about 34 MB a day of the devnet's blocks, and it keeps about 400 bytes of an empty one. Or read no history at all, and pay nothing.",
        thousands(standing.known_from)
    )).small().color(palette.muted)).wrap());
    let further = widgets::beside(
        ui,
        |ui| widgets::quiet(ui, &palette, "Read further back").clicked(),
        |ui, width| {
            ui.add(egui::TextEdit::singleline(&mut state.height).hint_text("a height, such as 0").desired_width(width));
        },
    );

    if further {
        match state.height.trim().replace(',', "").parse::<u64>().map(|floor| ctx.reading.further_back(floor)) {
            Ok(Further::Moved) => {
                state.height.clear();
                state.trouble = None;
            }
            Ok(Further::Already(floor)) => state.trouble = Some(format!("It reads from block {} already.", thousands(floor))),
            Ok(Further::Full) => state.trouble = Some(format!("What it keeps is full, at {}: anything older would only push out the rest. Forget what it read first, to start again from the final block.", widgets::bytes_said(standing.bytes))),
            Err(_) => state.trouble = Some("That is not a height.".to_string()),
        }
    }

    if let Some(trouble) = &state.trouble {
        ui.add(egui::Label::new(RichText::new(trouble).color(palette.bad)).wrap());
    }
}

/// What the reading has to say for itself that needs saying wherever it is shown.
fn said_of(ui: &mut Ui, ctx: &mut Ctx, standing: &Standing) {
    let palette = ctx.look.palette;
    let line = |ui: &mut Ui, text: String, color: egui::Color32| {
        ui.add(egui::Label::new(RichText::new(text).small().color(color)).wrap());
    };

    if let Some(why) = &standing.trouble {
        line(ui, why.clone(), palette.bad);

        if widgets::quiet(ui, &palette, "Try again").clicked() {
            ctx.reading.retry();
        }
    }

    if let Some(why) = &standing.waiting {
        line(ui, why.clone(), palette.warn);
    }

    if standing.catching_up {
        line(ui, "Blocks came while the app was shut: those are being read.".to_string(), palette.warn);
    }

    if standing.no_receipts > 0 {
        line(ui, format!("{} blocks came without their receipts, so what their transactions did is not known.", thousands(standing.no_receipts)), palette.warn);
    }

    if standing.unread > 0 {
        line(ui, format!("This node holds nothing of {} blocks among those read.", thousands(standing.unread)), palette.warn);
    }

    if let Some(held) = standing.held_from {
        line(ui, format!("This node holds nothing below block {}, so nothing older is known.", thousands(held)), palette.warn);
    }

    if let Some(cut) = standing.cut_below {
        line(ui, format!("Too much to keep: blocks below {} were let go, the oldest first.", thousands(cut)), palette.warn);
    }
}

/// On the Overview, and over anything that lists what was read: what it is up to, and from what height
/// it knows, which is what the page's history starts at.
pub fn note(ui: &mut Ui, ctx: &mut Ctx) {
    let look = ctx.look;
    let palette = look.palette;

    super::section(ui, look, "Reading checked blocks", |ui| {
        // With the light client stopped there is nothing to check blocks by, and nothing shows.
        if let Some(trouble) = &ctx.light.trouble {
            ui.add(egui::Label::new(RichText::new(format!("The light client stopped, so nothing is read: {trouble}")).color(palette.bad)).wrap());

            return;
        }

        if ctx.light.client().is_some_and(|client| client.params().finality.is_none()) {
            ui.add(egui::Label::new(RichText::new(NO_FINALITY).color(palette.bad)).wrap());

            return;
        }

        let Some(standing) = ctx.reading.standing() else {
            ui.label(RichText::new("Starting: waiting for the light client.").small().color(palette.muted));

            return;
        };
        let final_height = ctx.light.client().map_or(standing.high, chainv4::light::LightClient::final_height);

        ui.add(egui::Label::new(RichText::new(format!(
            "Read to block {} of {} final, knowing from block {}, with {} downloaded.{}",
            thousands(standing.high),
            thousands(final_height),
            thousands(standing.known_from),
            widgets::bytes_said(standing.downloaded),
            if standing.left_below > 0 { format!(" {} older blocks still to read, back to {}.", thousands(standing.left_below), thousands(standing.floor)) } else { String::new() },
        )).small().color(palette.soft)).wrap());
        said_of(ui, ctx, &standing);
    });
}

/// What a page says of a block or transaction outside what was read: why, and the way to read further
/// back. A block by height can also be fetched by itself, which says that the node sees which.
pub fn outside(ui: &mut Ui, ctx: &mut Ctx, why: &str, height: Option<u64>) {
    let look = ctx.look;
    let palette = look.palette;

    ui.add(egui::Label::new(RichText::new(format!("{why}.")).color(palette.soft)).wrap());
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        if widgets::quiet(ui, &palette, "Read further back, in Settings").clicked() {
            ctx.go(View::Settings);
        }

        let Some(height) = height else {
            return;
        };
        let (fetching, _) = ctx.reading.fetching();

        if fetching {
            ui.spinner();
        } else if widgets::quiet(ui, &palette, "Fetch this one block").on_hover_text("One block by itself: the node sees which").clicked()
            && let Some(client) = ctx.light.client()
        {
            match ctx.reading.fetch_one(height, client) {
                Ok(asked) => ctx.api.read_range(asked),
                Err(why) => ctx.reading.say_fetching(why),
            }
        }
    });

    if height.is_some() {
        super::note(ui, look, "Fetching it by itself asks the node for that block alone, so the node sees which. It is checked against the light client, shown for now, and not counted as read.");

        if let (_, Some(why)) = ctx.reading.fetching() {
            ui.add(egui::Label::new(RichText::new(why).color(palette.bad)).wrap());
        }
    }
}

/// The press that takes a direct proof of a state entry at the final block, saying that the node sees
/// what it is of, and what came of the last one: the value, or why not. Nothing is asked unpressed.
pub fn direct_proof(ui: &mut Ui, ctx: &mut Ctx, what: &str, column: Column, key: &[u8]) -> Option<ProvenValue> {
    let look = ctx.look;
    let palette = look.palette;
    let proven = ctx.reading.proven(column, key).cloned();

    if proven.is_none() {
        super::note(ui, look, format!("Blocks do not say this, and no node was asked. A direct proof asks a node for {what} at the final block, checked against the light client: the node sees what it is of."));
    }

    ui.horizontal_wrapped(|ui| {
        if ctx.reading.proving(column, key) {
            ui.spinner();
            ui.label(RichText::new("Asking for the proof…").color(palette.muted));
        } else if widgets::quiet(ui, &palette, if proven.is_some() { "Prove it again" } else { "Prove it directly" }).on_hover_text("The node sees what it is of").clicked() {
            match ctx.light.client().and_then(|client| ctx.reading.prove(column, key, client)) {
                // Past any hold on what failed: someone pressed for it.
                Some(ask) => ctx.api.retry(ask),
                None => ctx.reading.say_proving(column, key, "The light client has not reached a final block yet.".to_string()),
            }
        }
    });

    match proven {
        Some(Ok(proven)) => Some(proven),
        Some(Err(why)) => {
            ui.add(egui::Label::new(RichText::new(why).color(palette.bad)).wrap());

            None
        }
        None => None,
    }
}

/// Where a direct proof stands: at what final block it was proven, and that it says nothing since.
pub fn proven_at(ui: &mut Ui, ctx: &Ctx, proven: &ProvenValue) {
    let look = ctx.look;

    ui.horizontal_wrapped(|ui| {
        widgets::pill(ui, "proven", look.roles.final_);
        ui.label(RichText::new(format!("at final block {}", thousands(proven.height))).small().color(look.palette.muted)).on_hover_text(&proven.block);
    });
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::reading::tests::{SCREENS, a_reading_with_everything_to_say, drawn},
    };

    #[test]
    fn a_page_from_the_node_says_it_comes_from_the_node_and_an_app_says_nothing() {
        let same = widgets::page_warning(Some("https://node.example"), "https://node.example/", "explorer", APPS_THAT_HOLD).unwrap();
        assert!(same.contains("comes from that node") && same.contains("fakes these checks") && same.contains("explorer does not come from it"), "{same}");

        let other = widgets::page_warning(Some("https://explorer.example"), "https://node.example", "explorer", APPS_THAT_HOLD).unwrap();
        assert!(other.contains("https://explorer.example") && other.contains("fakes these checks"), "{other}");

        assert_eq!(widgets::page_warning(None, "https://node.example", "explorer", APPS_THAT_HOLD), None, "an app on a device");
    }

    #[test]
    fn the_setting_says_every_limit_in_plain_words_and_fits_a_phone_either_way() {
        let limits = [widgets::READ_BLOCKS_LIMITS, EXPLORER_LIMITS].concat().join(" ");

        for words in ["unknown, not unchanged", "10,000 blocks", "--archive", "without being caught", "Receipts cannot be left out", "34 MB", "only while the app is open", "proxy", "not sealed", "the oldest blocks go", "not to why it failed", "plain units"] {
            assert!(limits.contains(words), "{words} in {limits}");
        }

        for size in SCREENS {
            let mut state = ReadingState::default();
            let mut connect = None;
            let (width, _) = drawn(size, true, |ui, ctx| card(ui, ctx, &mut state, &mut connect));
            let (_, said) = drawn((size.0, 8_000.0), true, |ui, ctx| card(ui, ctx, &mut state, &mut connect));

            assert!(width <= size.0, "{width} wide, on {size:?}");
            assert!(said.contains("Read from checked blocks") && said.contains("costs far more"), "{said}");
        }
    }

    #[test]
    fn with_the_light_client_stopped_the_note_says_nothing_is_read_and_why() {
        let mut light = crate::light::Light::default();
        light.trouble = Some("the node is now on another chain".to_string());
        let store = crate::store::Store::default();
        let (_, said, _) = crate::reading::tests::drawn_in(&store, &mut Default::default(), &mut light, (360.0, 8_000.0), true, crate::views::overview::show);

        assert!(said.contains("nothing is read") && said.contains("another chain"), "{said}");
    }

    #[test]
    fn the_setting_turned_on_or_off_starts_over_with_the_node() {
        let mut state = ReadingState::default();
        let mut connect = None;
        let (_, said) = drawn((1200.0, 800.0), false, |ui, ctx| card(ui, ctx, &mut state, &mut connect));

        assert!(said.contains("Read from checked blocks"), "off, it still offers itself");
        assert!(!said.contains("knows from"), "and says nothing of a reading: {said}");
    }

    #[test]
    fn where_it_stands_with_all_it_could_say_fits_a_phone_either_way_and_says_it() {
        for size in SCREENS {
            let mut state = ReadingState::default();
            let (width, _) = drawn_with_a_reading(size, &mut state);
            let (_, said) = drawn_with_a_reading((size.0, 8_000.0), &mut state);

            assert!(width <= size.0, "{width} wide, on {size:?}");

            for words in ["read to", "knows from", "downloaded", "keeps", "Read further back", "Forget what it read", "Try again", "without their receipts", "Too much to keep"] {
                assert!(said.contains(words), "{words} in {said}, on {size:?}");
            }
        }
    }

    fn drawn_with_a_reading(size: (f32, f32), state: &mut ReadingState) -> (f32, String) {
        let standing = a_reading_with_everything_to_say();

        drawn(size, true, |ui, ctx| {
            rows(ui, ctx, state, &standing, 17_280);
            note(ui, ctx);
        })
    }

    #[test]
    fn every_page_is_decided_and_those_turned_off_say_the_same_words_and_fit_a_phone_either_way() {
        for view in [View::Mempool, View::Network, View::Collections, View::Collection(String::new()), View::Dex, View::Swaps, View::Bridges, View::Transfer(String::new()), View::Release(0), View::Governance, View::Pool, View::Multisig(String::new()), View::Faucet] {
            let (title, why) = turned_off(&view).expect("it needs the node");

            for size in SCREENS {
                let (width, said) = drawn(size, true, |ui, ctx| needs_asking(ui, ctx, title, why));

                assert!(width <= size.0, "{view:?} is {width} wide, on {size:?}");
                assert!(said.contains(NEEDS_ASKING), "{view:?}: {said}");
            }
        }

        for view in [View::Overview, View::Map, View::Block(String::new()), View::Tx(String::new()), View::Account(String::new()), View::Tokens, View::Finality, View::Send, View::Settings, View::Tools, View::Names, View::Rich] {
            assert_eq!(turned_off(&view), None, "{view:?} keeps what it can");
        }
    }
}
