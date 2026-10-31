//! Read from checked blocks, on the pages: the setting itself, with everything it costs and cannot do
//! said plainly; where the reading stands; asking a restored wallet from what height to read; and what
//! every page turned off by it says.

use {
    super::{Ctx, Page},
    crate::format,
    crate::node::NEEDS_ASKING,
    crate::theme,
    chainv4_ui::widgets,
    egui::{RichText, Ui},
};

/// Why this page needs the node, in a few words: None for one that works from blocks, or wants no
/// chain.
pub fn turned_off(page: Page) -> Option<&'static str> {
    match page {
        Page::Batch => Some("A batch is tried on the node before it can be priced"),
        Page::Mining => Some("What a pool owes is known only to its node, and mining goes through one"),
        Page::Multisig => Some("A multisig's members and proposals come from the node"),
        Page::Governance => Some("What stands and what is open come from the node"),
        Page::Contracts => Some("Reading a contract runs it on the node"),
        #[cfg(feature = "swap")]
        Page::Swaps => Some("A swap's locks are read by running the contract on the node"),
        #[cfg(feature = "dex")]
        Page::Trade => Some("A pool's prices and quotes come from the node"),
        #[cfg(feature = "nft")]
        Page::Nfts => Some("Collections are listed by the node"),
        #[cfg(feature = "bridge")]
        Page::Bridge => Some("A bridge's gateways are read by running them on the node"),
        _ => None,
    }
}

/// What a page turned off says: the same plain words on each, and where to turn it back on.
pub fn needs_asking(ui: &mut Ui, ctx: &mut Ctx, why: &str) {
    let palette = *ctx.palette;

    if let Some((_, name)) = super::nav(ctx.settings.role()).iter().find(|(page, _)| *page == *ctx.page) {
        theme::title(ui, &palette, name, "Read from checked blocks is on.");
    }

    if widgets::needs_asking(ui, &palette, &format!("{why}: {NEEDS_ASKING}.")) {
        *ctx.page = Page::Settings;
    }
}

/// What the page keeps between frames.
#[derive(Default)]
pub struct ReadingState {
    /// A height being typed: to start from, or to read further back to.
    height: String,
    trouble: Option<String>,
    /// Asked once whether to forget what was read.
    forgetting: bool,
}

/// Where the checks hold against whoever serves a page.
const APPS_THAT_HOLD: &str = "the desktop and phone apps, and the browser extension";

/// The setting, in Settings: everything it does, costs and cannot do, then where it stands.
pub fn card(ui: &mut Ui, ctx: &mut Ctx, state: &mut ReadingState) {
    let palette = *ctx.palette;
    let node = ctx.settings.node.clone();

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Read from checked blocks");
        let mut on = ctx.settings.read_blocks;

        if widgets::read_blocks_switch(ui, &palette, &mut on, "wallet") {
            ctx.settings.read_blocks = on;
            ctx.settings.persist_now = true;
            *ctx.connect = Some(node.clone());
        }

        if let Some(warning) = widgets::page_warning(widgets::page_origin().as_deref(), &node, "wallet", APPS_THAT_HOLD) {
            ui.add_space(4.0);
            ui.add(egui::Label::new(RichText::new(warning).small().color(palette.warn)).wrap());
        }

        egui::CollapsingHeader::new(RichText::new("What it cannot do").color(palette.soft)).id_salt("reading limits").show(ui, |ui| {
            for limit in widgets::READ_BLOCKS_LIMITS {
                ui.add(egui::Label::new(RichText::new(*limit).small().color(palette.muted)).wrap());
                ui.add_space(4.0);
            }

            for limit in WALLET_LIMITS {
                ui.add(egui::Label::new(RichText::new(*limit).small().color(palette.muted)).wrap());
                ui.add_space(4.0);
            }
        });

        if ctx.settings.read_blocks {
            ui.add_space(6.0);
            standing(ui, ctx, state);
        }
    });
}

/// What only a wallet has to say of its limits.
const WALLET_LIMITS: &[&str] = &[
    "An account crossing a gap takes one direct proof to catch up, and the wallet says so, because the node sees that account.",
    "A balance can be proven but out of date, if a node left an entry out of a block's list. Check now does a direct proof, and the node sees that account.",
    "A start, at height 0 as anywhere, takes its first proofs from the node, as what genesis allocated is in no block.",
    "Your own sent transaction keeps one lookup: its receipt proof, shown as included, not final, until the reading reaches it, when what the reading found takes over. It is checked, and the node already has that transaction.",
    "Fees are suggested from what checked blocks show the same kind of transaction used, and from the newest checked header's base price, and you can change them. Nothing tries the transaction first, so a call that would fail can still cost its fee.",
    "Tokens show once a transaction moving them to or from this wallet has been read, and names once this wallet has been seen using them. Each takes one direct proof of its record, which the node sees. Paying to a name needs asking the node, so give the address.",
    "Off while this is on: contracts, batches, swaps, trade, multisig, pool earnings and mining, governance, NFTs, bridges, the faucet, and what sites ask. The coin's name and decimals are as a node last said them on this device, or amounts show as plain units.",
    "Fast resume is off while this is on: the light client walks every header.",
    "Addresses in Activity and balances are kept sealed with this wallet's key, in an index of their own that is read only while the wallet is unlocked. Losing it costs a rescan, never the wallet. What you send is still kept in the settings, as it always was, until it is final, and so is the account last used.",
];

/// Where the reading stands, in full: what Settings shows.
fn standing(ui: &mut Ui, ctx: &mut Ctx, state: &mut ReadingState) {
    let palette = *ctx.palette;
    let Some(client) = ctx.chain.client.as_ref() else {
        ui.label(RichText::new("Waiting for the light client to start.").small().color(palette.muted));

        return;
    };

    if client.params().finality.is_none() {
        ui.colored_label(palette.bad, "This chain has no finality, so it has no final blocks to read, and nothing can be shown from them.");

        return;
    }

    let final_height = client.final_height();

    if ctx.reading.needs_floor() {
        floor_question(ui, ctx, state);

        return;
    }

    let Some(standing) = ctx.reading.standing() else {
        ui.label(RichText::new(match ctx.reading.is_open() {
            true => "Starting at the newest final block.",
            false => "Opening what it read before. It reads only while this wallet is unlocked.",
        }).small().color(palette.muted));

        return;
    };

    rows(ui, ctx, state, &standing, final_height);
}

/// How far it has read, what that cost, what it has to say, and what can be done about it.
fn rows(ui: &mut Ui, ctx: &mut Ctx, state: &mut ReadingState, standing: &crate::reading::Standing, final_height: u64) {
    let palette = *ctx.palette;
    let thousands = |height: u64| format::thousands(&height.to_string());

    super::row(ui, &palette, "Read to", |ui| {
        ui.label(format!("block {} of {} final", thousands(standing.high), thousands(final_height)));
    });
    super::row(ui, &palette, "Knows from", |ui| {
        ui.label(format!("block {}", thousands(standing.known_from)));
    });

    if standing.left_below > 0 {
        super::row(ui, &palette, "Still to read", |ui| {
            ui.label(format!("{} blocks, back to {}", thousands(standing.left_below), thousands(standing.floor)));
        });
    }

    super::row(ui, &palette, "Downloaded", |ui| {
        ui.label(format!("{}, {} blocks", widgets::bytes_said(standing.downloaded), thousands(standing.read)));
    });
    super::row(ui, &palette, "Direct proofs", |ui| {
        ui.label(format!("{} to start or catch up, {} from Check now", standing.proofs, standing.checks));
    });

    said_of(ui, ctx, standing);

    if !ctx.reading.lasts() {
        ui.colored_label(palette.warn, "There is nowhere on this device to keep what it read, so it reads again on each start.");
    }

    ui.add_space(6.0);
    ui.label(RichText::new("Read further back, down to this height: what it read stands, and it says from what height it knows. It shows nothing older.").small().color(palette.muted));
    let further = super::beside(
        ui,
        |ui| theme::quiet(ui, &palette, "Read further back").clicked(),
        |ui, width| {
            ui.add(egui::TextEdit::singleline(&mut state.height).hint_text("a height, such as 0").desired_width(width));
        },
    );

    if further {
        let moved = state.height.trim().parse::<u64>().map(|floor| ctx.chain.client.as_ref().is_some_and(|client| ctx.reading.further_back(client, floor)));

        match moved {
            Ok(true) => {
                state.height.clear();
                state.trouble = None;
            }
            Ok(false) => state.trouble = Some(format!("It reads from block {} already.", thousands(standing.floor))),
            Err(_) => state.trouble = Some("That is not a height.".to_string()),
        }
    }

    if let Some(trouble) = &state.trouble {
        ui.colored_label(palette.bad, trouble);
    }

    ui.add_space(6.0);
    check_now(ui, ctx);
    ui.add_space(6.0);

    match state.forgetting {
        false => {
            if theme::quiet(ui, &palette, "Forget what it read").on_hover_text("Read it all again from a height you pick").clicked() {
                state.forgetting = true;
            }
        }
        true => {
            ui.label(RichText::new("This wallet's index goes, and reading starts again from a height you pick, which costs its download again. Nothing of the wallet goes.").small().color(palette.warn));
            ui.horizontal_wrapped(|ui| {
                if theme::quiet(ui, &palette, "Yes, forget it").clicked() {
                    state.forgetting = false;
                    ctx.settings.forget_reading = true;
                }

                if theme::quiet(ui, &palette, "Keep it").clicked() {
                    state.forgetting = false;
                }
            });
        }
    }
}

/// What the reading has to say for itself that needs saying wherever it is shown.
fn said_of(ui: &mut Ui, ctx: &mut Ctx, standing: &crate::reading::Standing) {
    let palette = *ctx.palette;
    let thousands = |height: u64| format::thousands(&height.to_string());

    if let Some(why) = &standing.trouble {
        ui.add(egui::Label::new(RichText::new(why).color(palette.bad)).wrap());

        if theme::quiet(ui, &palette, "Try again").clicked() {
            ctx.reading.retry();
        }
    }

    if let Some(why) = &standing.waiting {
        ui.add(egui::Label::new(RichText::new(why).small().color(palette.warn)).wrap());
    }

    if standing.catching_up {
        ui.add(egui::Label::new(RichText::new("Blocks came while the app was shut: balances show again once those are read, as until then they could be out of date.").small().color(palette.warn)).wrap());
    }

    if standing.stale > 0 && !standing.catching_up {
        let gap = standing.state_gap.map_or_else(String::new, |gap| format!(" Block {} came without its proofs, and counts as unknown.", thousands(gap)));

        ui.add(egui::Label::new(RichText::new(format!("{} values are not shown until one direct proof each brings them up to date, as they are asked now. The node sees each one.{gap}", standing.stale)).small().color(palette.warn)).wrap());
    }

    if standing.unread > 0 {
        ui.add(egui::Label::new(RichText::new(format!("{} blocks came without their receipts, so what they hold of this wallet is not known.", thousands(standing.unread))).small().color(palette.warn)).wrap());
    }

    if let Some(held) = standing.held_from {
        ui.add(egui::Label::new(RichText::new(format!("This node holds nothing below block {}, so nothing older is known.", thousands(held))).small().color(palette.warn)).wrap());
    }

    if let Some(cut) = standing.cut_below {
        ui.add(egui::Label::new(RichText::new(format!("Too many transactions to keep: those below block {} are let go.", thousands(cut))).small().color(palette.warn)).wrap());
    }

    for omitted in &standing.omitted {
        ui.add(egui::Label::new(RichText::new(format!("Check now found this node's blocks left out a change to {omitted}. The proof stands now: consider another node.")).color(palette.bad)).wrap());
    }
}

/// Asks directly for proof of what the reading holds, which is what finds a node leaving things out.
fn check_now(ui: &mut Ui, ctx: &mut Ctx) {
    let palette = *ctx.palette;
    let left = ctx.reading.checks_left();

    ui.label(RichText::new("A node can leave an entry out of a block's list without being caught, which leaves a balance proven but out of date. Check now proves each directly. The node sees each account.").small().color(palette.muted));
    ui.horizontal_wrapped(|ui| {
        if theme::quiet(ui, &palette, "Check now").clicked() {
            ctx.reading.check_now();
        }

        if left > 0 {
            ui.spinner();
            ui.label(RichText::new(format!("{left} to prove")).small().color(palette.muted));
        }
    });
}

/// Asks a wallet that could have a history from what height to read it.
fn floor_question(ui: &mut Ui, ctx: &mut Ctx, state: &mut ReadingState) {
    let palette = *ctx.palette;

    ui.add(egui::Label::new(RichText::new("From what height should this wallet read? It knows nothing older, and shows no older history. Reading further back later is always possible. About 2 KB an empty block: 34 MB a day of the devnet's blocks. Nodes prove balances only for their last 10,000 blocks by default, but give the blocks and receipts of older ones.").small().color(palette.soft)).wrap());
    let start = super::beside(
        ui,
        |ui| theme::primary(ui, &palette, "Read from here", true).clicked(),
        |ui, width| {
            ui.add(egui::TextEdit::singleline(&mut state.height).hint_text("a height, such as 0").desired_width(width));
        },
    );

    if start {
        match state.height.trim().parse::<u64>() {
            Ok(floor) => {
                ctx.reading.start_from(floor);
                state.height.clear();
                state.trouble = None;
            }
            Err(_) => state.trouble = Some("That is not a height.".to_string()),
        }
    }

    if theme::quiet(ui, &palette, "Only from now on").clicked() {
        ctx.reading.start_from(u64::MAX);
    }

    if let Some(trouble) = &state.trouble {
        ui.colored_label(palette.bad, trouble);
    }
}

/// On the Overview: what it is up to, and what it needs. Nothing once all is well and it is done,
/// but the height it knows from, which is what the page's history starts at.
pub fn note(ui: &mut Ui, ctx: &mut Ctx, state: &mut ReadingState) {
    if !ctx.settings.read_blocks {
        return;
    }

    let palette = *ctx.palette;
    let thousands = |height: u64| format::thousands(&height.to_string());

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Reading checked blocks");

        if ctx.chain.client.as_ref().is_some_and(|client| client.params().finality.is_none()) {
            ui.colored_label(palette.bad, "This chain has no finality, so it has no final blocks to read, and nothing can be shown from them. Turn this setting off to use it.");

            return;
        }

        if ctx.reading.needs_floor() {
            floor_question(ui, ctx, state);

            return;
        }

        let Some(standing) = ctx.reading.standing() else {
            ui.label(RichText::new("Starting: waiting for the light client.").small().color(palette.muted));

            return;
        };
        let final_height = ctx.chain.client.as_ref().map_or(0, chainv4::light::LightClient::final_height);

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
    theme::section_gap(ui);
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::views::drawn::{SCREENS, World, page_in, said_in},
    };

    fn reading_world() -> World {
        let mut world = World::default();
        world.settings.read_blocks = true;
        world.settings.node = "http://here".to_string();

        world
    }

    #[test]
    fn a_page_in_a_browser_says_it_comes_from_who_serves_it_and_an_extension_or_an_app_does_not() {
        let warning = |origin, node| widgets::page_warning(origin, node, "wallet", APPS_THAT_HOLD);
        let same = warning(Some("https://node.example"), "https://node.example/").unwrap();
        assert!(same.contains("comes from that node") && same.contains("fakes these checks") && same.contains("browser extension"), "{same}");

        let other = warning(Some("https://wallet.example"), "https://node.example").unwrap();
        assert!(other.contains("https://wallet.example") && other.contains("fakes these checks"), "{other}");

        assert_eq!(warning(Some("chrome-extension://abcdef"), "https://node.example"), None);
        assert_eq!(warning(Some("moz-extension://abcdef"), "https://node.example"), None);
        assert_eq!(warning(None, "https://node.example"), None, "an app on a device");
    }

    #[test]
    fn the_setting_says_every_limit_in_plain_words_and_fits_a_phone_either_way() {
        let (mut book, mut holders) = (crate::vault::Book::default(), Vec::new());

        for size in SCREENS {
            let mut world = reading_world();
            let mut state = ReadingState::default();
            let (width, _) = page_in(&mut world, &mut book, &mut holders, size, |ui, ctx| card(ui, ctx, &mut state));

            assert!(width <= size.0, "{width} wide, on {size:?}");
        }

        let mut world = reading_world();
        let mut state = ReadingState::default();
        let said = said_in(&mut world, &mut book, &mut holders, (1200.0, 800.0), |ui, ctx| card(ui, ctx, &mut state));

        for words in ["costs far more", "turned off", "Read from checked blocks"] {
            assert!(said.contains(words), "{words} in {said}");
        }

        let limits = [widgets::READ_BLOCKS_LIMITS, WALLET_LIMITS].concat().join(" ");

        for words in ["unknown, not unchanged", "10,000 blocks", "--archive", "without being caught", "Check now", "Genesis", "34 MB", "only while the app is open", "proxy", "included, not final", "Fees are suggested"] {
            assert!(limits.contains(words), "{words} in {limits}");
        }
    }

    #[test]
    fn every_page_turned_off_says_the_same_words_and_fits_a_phone_either_way() {
        let (mut book, mut holders) = (crate::vault::Book::default(), Vec::new());
        let pages = [
            Page::Batch,
            Page::Mining,
            Page::Multisig,
            Page::Governance,
            Page::Contracts,
            #[cfg(feature = "swap")]
            Page::Swaps,
            #[cfg(feature = "dex")]
            Page::Trade,
            #[cfg(feature = "nft")]
            Page::Nfts,
            #[cfg(feature = "bridge")]
            Page::Bridge,
        ];

        for page in pages {
            let what = turned_off(page).expect("it needs the node");

            for size in SCREENS {
                let mut world = reading_world();
                let (width, _) = page_in(&mut world, &mut book, &mut holders, size, |ui, ctx| needs_asking(ui, ctx, what));
                assert!(width <= size.0, "{page:?} is {width} wide, on {size:?}");
            }

            let mut world = reading_world();
            let said = said_in(&mut world, &mut book, &mut holders, (360.0, 740.0), |ui, ctx| needs_asking(ui, ctx, what));
            assert!(said.contains(NEEDS_ASKING), "{page:?}: {said}");
        }

        for page in [Page::Overview, Page::Send, Page::Receive, Page::Activity, Page::Accounts, Page::Settings] {
            assert_eq!(turned_off(page), None, "{page:?} works from blocks");
        }
    }

    #[test]
    fn the_question_of_where_to_start_fits_a_phone_either_way() {
        let (mut book, mut holders) = (crate::vault::Book::default(), Vec::new());

        for size in SCREENS {
            let mut world = reading_world();
            let mut state = ReadingState::default();
            let (width, _) = page_in(&mut world, &mut book, &mut holders, size, |ui, ctx| floor_question(ui, ctx, &mut state));

            assert!(width <= size.0, "{width} wide, on {size:?}");
        }
    }

    #[test]
    fn where_it_stands_with_all_it_could_say_fits_a_phone_either_way_and_says_it() {
        let (mut book, mut holders) = (crate::vault::Book::default(), Vec::new());
        let draw = |ui: &mut Ui, ctx: &mut Ctx, state: &mut ReadingState| {
            let standing = ctx.reading.standing().expect("a reading under way");

            note(ui, ctx, state);
            card(ui, ctx, state);
            rows(ui, ctx, state, &standing, 17_280);
        };

        for size in SCREENS {
            let mut world = reading_world();
            world.reading = crate::reading::tests::a_reading_with_everything_to_say();
            let mut state = ReadingState::default();
            let (width, _) = page_in(&mut world, &mut book, &mut holders, size, |ui, ctx| draw(ui, ctx, &mut state));

            assert!(width <= size.0, "{width} wide, on {size:?}");
        }

        let mut world = reading_world();
        world.reading = crate::reading::tests::a_reading_with_everything_to_say();
        let mut state = ReadingState::default();
        let said = said_in(&mut world, &mut book, &mut holders, (360.0, 4_000.0), |ui, ctx| draw(ui, ctx, &mut state));

        for words in ["Read to", "Knows from", "Downloaded", "Direct proofs", "Check now", "Read further back", "Forget what it read", "Try again", "do not fit", "direct proof each", "without their receipts", "left out a change"] {
            assert!(said.contains(words), "{words} in {said}");
        }
    }

    #[test]
    fn with_the_setting_off_the_overview_says_nothing_of_it() {
        let (mut book, mut holders) = (crate::vault::Book::default(), Vec::new());
        let mut world = World::default();
        let mut state = ReadingState::default();
        let said = said_in(&mut world, &mut book, &mut holders, (1200.0, 800.0), |ui, ctx| note(ui, ctx, &mut state));

        assert!(said.is_empty(), "{said}");
    }
}
