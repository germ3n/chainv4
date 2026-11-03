//! Finding a ballot among many, as the apps' Governance pages do: what each says of a change or a
//! decision, which of them a search and a filter leave, in which order, and a page of them at a
//! time, older ones read from the node as they are reached. Plain data, apart from the controls
//! that set it, and tested as such; what a change means is each app's own.

use {
    crate::theme::Palette,
    crate::widgets,
    egui::{RichText, Ui},
    std::collections::BTreeMap,
    std::ops::Range,
};

/// How many a page shows: what a phone scrolls through without losing its place.
pub const PER_PAGE: usize = 10;

/// The orders open changes come in.
pub const OPEN_ORDERS: [(Order, &str); 3] = [(Order::Newest, "Newest"), (Order::MostSupport, "Most support"), (Order::ClosingSoonest, "Closing soon")];

/// The orders decisions come in: they have no support left to weigh, and nothing closes.
pub const DECIDED_ORDERS: [(Order, &str); 2] = [(Order::Newest, "Newest"), (Order::Oldest, "Oldest")];

/// An order to list ballots in. Ties keep the order they came in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Order {
    /// The one made last first: an open change's latest vote, a decision's count.
    #[default]
    Newest,
    Oldest,
    /// The one nearest to what it needs first.
    MostSupport,
    /// The one whose oldest vote stops counting first.
    ClosingSoonest,
}

/// What a list knows of one ballot, to find it and to order it by.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Item {
    /// Its kind, by the name the node gives it: what the filter picks.
    pub kind: String,
    /// All a search looks through: its kind, its change in words and as JSON, who backs it.
    pub text: String,
    /// How recent it is: the higher the newer.
    pub place: u64,
    /// How far its Yes got, what it needs being 1.
    pub support: f64,
    /// The epoch its oldest vote counts until, if it has any.
    pub closes: Option<u64>,
}

/// What someone asked of a list: the words searched for, the kind picked, the order, the page.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Finder {
    pub search: String,
    pub kind: Option<String>,
    pub order: Order,
    pub page: usize,
}

impl Finder {
    /// Whether it narrows the list, rather than only ordering it.
    pub fn narrows(&self) -> bool {
        self.kind.is_some() || !self.search.trim().is_empty()
    }
}

/// Whether a search's every word is in the text, whatever its case: partial addresses and amounts
/// match as much as words do.
pub fn matches(text: &str, search: &str) -> bool {
    let text = text.to_lowercase();

    search.to_lowercase().split_whitespace().all(|word| text.contains(word))
}

/// The places of the items a finder leaves, in its order.
pub fn select(items: &[Item], finder: &Finder) -> Vec<usize> {
    let mut found: Vec<usize> = (0..items.len())
        .filter(|index| finder.kind.as_ref().is_none_or(|kind| items[*index].kind == *kind) && matches(&items[*index].text, &finder.search))
        .collect();
    let closes = |index: &usize| items[*index].closes.unwrap_or(u64::MAX);

    match finder.order {
        Order::Newest => found.sort_by_key(|index| std::cmp::Reverse(items[*index].place)),
        Order::Oldest => found.sort_by_key(|index| items[*index].place),
        Order::MostSupport => found.sort_by(|one, other| items[*other].support.total_cmp(&items[*one].support)),
        Order::ClosingSoonest => found.sort_by_key(closes),
    }

    found
}

/// The kinds among the items, each once, in the order the kinds are listed.
pub fn kinds_among(items: &[Item]) -> Vec<String> {
    let mut kinds: Vec<String> = items.iter().map(|item| item.kind.clone()).collect();
    kinds.sort();
    kinds.dedup();

    kinds
}

/// How many pages so many found take, one at least.
pub fn pages(found: usize, per: usize) -> usize {
    found.div_ceil(per.max(1)).max(1)
}

/// The found ones a page shows: past the last page, the last page.
pub fn window(found: usize, page: usize, per: usize) -> Range<usize> {
    let per = per.max(1);
    let page = page.min(pages(found, per) - 1);

    (page * per).min(found)..((page + 1) * per).min(found)
}

/// Whether older pages should be read from the node for this page: it is not full and the node has
/// more, or the oldest are asked for first, which only the end of the list says.
pub fn wants_older(found: usize, finder: &Finder, per: usize, more: bool) -> bool {
    more && (finder.order == Order::Oldest || found < (finder.page + 1) * per.max(1))
}

/// What a page says of itself: 11 to 20 of 45, and how much of the history a search has been through
/// while the node has more.
pub fn summary(found: usize, page: usize, per: usize, narrowed: bool, read: usize, total: usize) -> String {
    let shown = window(found, page, per);
    let of = match (narrowed, read < total) {
        (true, true) => format!("{found} found in the {read} read of {total}"),
        (true, false) => format!("{found} found"),
        (false, true) => format!("{found} read of {total}"),
        (false, false) => found.to_string(),
    };

    match found {
        0 => of,
        _ => format!("{} to {} of {of}", shown.start + 1, shown.end),
    }
}

/// One share of another, each a decimal number of any size: how far votes got, 0 when the whole
/// is nothing or either does not read.
pub fn share(part: &str, whole: &str) -> f64 {
    match (part.parse::<f64>(), whole.parse::<f64>()) {
        (Ok(part), Ok(whole)) if whole > 0.0 => part / whole,
        _ => 0.0,
    }
}

/// Something counted by its version, as a decision is.
pub trait Counted {
    fn version(&self) -> u64;
}

/// The chain's decisions read so far, by version, and how far down the pages read reach. Versions
/// count up by one a decision, so any missing between the lowest read and the newest is a gap: a
/// decision the node did not list.
#[derive(Clone, Debug)]
pub struct Decided<T> {
    pub items: BTreeMap<u64, T>,
    /// The newest version, as the node last said.
    pub version: u64,
    /// The oldest version the node keeps: 1 unless it synced from a snapshot.
    pub from: u64,
    /// The lowest version the pages read reach: every one from it to the newest was listed, or is
    /// a gap. None before the first page.
    pub bottom: Option<u64>,
}

impl<T> Default for Decided<T> {
    fn default() -> Decided<T> {
        Decided { items: BTreeMap::new(), version: 0, from: 0, bottom: None }
    }
}

impl<T: Counted> Decided<T> {
    /// Takes a page: the newest, with before None, or an older one, below before.
    pub fn take(&mut self, version: u64, from: u64, decisions: Vec<T>, before: Option<u64>) {
        let lowest = decisions.iter().map(Counted::version).min();
        // A page that lists nothing says there is nothing from the oldest kept on.
        let reaches = lowest.unwrap_or(from);

        if before.is_none() {
            // A reorganization undid decisions: what was above the newest is gone.
            self.items.retain(|known, _| *known <= version);

            // So many came since that the newest page no longer meets what was read: start over.
            if self.bottom.is_some() && reaches > self.version.min(version) + 1 {
                self.items.clear();
                self.bottom = None;
            }

            self.version = version;
        }

        self.from = from;
        self.bottom = Some(self.bottom.map_or(reaches, |bottom| bottom.min(reaches)));

        for decision in decisions {
            self.items.insert(decision.version(), decision);
        }
    }

    /// Where the next older page starts, while there are older ones the node keeps.
    pub fn next(&self) -> Option<u64> {
        self.bottom.filter(|bottom| *bottom > self.from && *bottom > 1)
    }

    /// The versions missing between the lowest read and the newest, as runs from and to.
    pub fn gaps(&self) -> Vec<(u64, u64)> {
        let Some(bottom) = self.bottom else {
            return Vec::new();
        };
        let mut gaps: Vec<(u64, u64)> = Vec::new();

        for version in bottom.max(1)..=self.version {
            if self.items.contains_key(&version) {
                continue;
            }

            match gaps.last_mut() {
                Some((_, to)) if *to + 1 == version => *to = version,
                _ => gaps.push((version, version)),
            }
        }

        gaps
    }

    /// The decisions read, newest first.
    pub fn newest_first(&self) -> impl Iterator<Item = &T> {
        self.items.values().rev()
    }

    /// How many the node keeps, of those decided.
    pub fn kept(&self) -> usize {
        match self.version >= self.from {
            true => (self.version - self.from + 1) as usize,
            false => 0,
        }
    }
}

/// What a kind is called on the page.
pub type KindName<'a> = &'a dyn Fn(&str) -> String;

/// The search box, then the order, then the kind: an order only for two or more, a filter only for
/// two or more kinds, so a short list keeps a short page. Choices wrap to the width there is, as a
/// phone's. A change of any of them goes back to the first page.
pub fn controls(ui: &mut Ui, palette: &Palette, finder: &mut Finder, hint: &str, items: &[Item], orders: &[(Order, &str)], kind_name: KindName) {
    if items.is_empty() && !finder.narrows() {
        return;
    }

    let was = finder.clone();
    ui.add(egui::TextEdit::singleline(&mut finder.search).hint_text(hint).desired_width(f32::INFINITY));

    if items.len() > 1 {
        ui.add_space(2.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Sort").small().color(palette.muted));
            widgets::segmented_rows(ui, palette, &mut finder.order, orders);
        });
    }

    let kinds = kinds_among(items);

    // A kind picked stays pickable, should what it leaves be nothing for now.
    if finder.kind.as_ref().is_some_and(|kind| !kinds.contains(kind)) {
        finder.kind = None;
    }

    if kinds.len() > 1 {
        let names: Vec<String> = kinds.iter().map(|kind| kind_name(kind)).collect();
        let options: Vec<(usize, &str)> = std::iter::once((0, "All")).chain(names.iter().enumerate().map(|(place, name)| (place + 1, name.as_str()))).collect();
        let mut picked = finder.kind.as_ref().and_then(|kind| kinds.iter().position(|known| known == kind)).map_or(0, |place| place + 1);

        ui.add_space(2.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Kind").small().color(palette.muted));

            if widgets::segmented_rows(ui, palette, &mut picked, &options) {
                finder.kind = picked.checked_sub(1).map(|place| kinds[place].clone());
            }
        });
    }

    if finder.search != was.search || finder.kind != was.kind || finder.order != was.order {
        finder.page = 0;
    }

    ui.add_space(4.0);
}

/// Newer and Older, with what the page shows between them, and the older ones on their way when
/// they are. Older takes a page more of the list, from the node as it comes to it.
pub fn pager(ui: &mut Ui, palette: &Palette, finder: &mut Finder, found: usize, more: bool, reading: bool, said: &str) {
    let last = pages(found, PER_PAGE) - 1;
    finder.page = match more {
        true => finder.page,
        false => finder.page.min(last),
    };
    let (newer, older) = (finder.page > 0, finder.page < last);

    if !newer && !older && !more && !reading {
        ui.add(egui::Label::new(RichText::new(said).small().color(palette.muted)).wrap());

        return;
    }

    ui.add_space(6.0);
    ui.add(egui::Label::new(RichText::new(said).small().color(palette.muted)).wrap());
    ui.horizontal_wrapped(|ui| {
        if ui.add_enabled(newer, egui::Button::new("Newer page")).clicked() {
            finder.page -= 1;
        }

        if reading {
            ui.spinner();
        }

        if ui.add_enabled(older || more, egui::Button::new("Older page")).clicked() {
            finder.page += 1;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(kind: &str, text: &str, place: u64, support: f64, closes: Option<u64>) -> Item {
        Item { kind: kind.to_string(), text: text.to_string(), place, support, closes }
    }

    fn list() -> Vec<Item> {
        vec![
            item("fees", "Fees: 50% burned, by ed25519:0xAB12", 3, 0.5, Some(30)),
            item("name", "Rename the chain to second, by ed25519:0xcd34", 7, 1.0, Some(20)),
            item("fees", "Fees: 60% burned, by ed25519:0xcd34", 5, 0.25, Some(40)),
            item("reward", "Block reward: 50 DEV", 1, 0.25, None),
        ]
    }

    #[test]
    fn a_search_takes_every_word_in_any_case_and_a_part_of_an_address() {
        let found = |search: &str| select(&list(), &Finder { search: search.to_string(), ..Finder::default() });

        assert_eq!(found(""), [1, 2, 0, 3], "all of them, newest first");
        assert_eq!(found("  "), [1, 2, 0, 3], "spaces ask for nothing");
        assert_eq!(found("fees"), [2, 0]);
        assert_eq!(found("FEES 60%"), [2], "every word, whatever its case");
        assert_eq!(found("0xab1"), [0], "a part of an address");
        assert_eq!(found("cd34 rename"), [1], "in any order");
        assert_eq!(found("weather"), Vec::<usize>::new());
    }

    #[test]
    fn a_filter_leaves_one_kind_and_lists_each_kind_once() {
        let finder = Finder { kind: Some("fees".to_string()), ..Finder::default() };

        assert_eq!(select(&list(), &finder), [2, 0]);
        assert_eq!(kinds_among(&list()), ["fees", "name", "reward"]);
        assert!(kinds_among(&[]).is_empty());
        assert!(finder.narrows() && !Finder::default().narrows());
        assert!(!Finder { search: " ".to_string(), ..Finder::default() }.narrows(), "spaces narrow nothing");
    }

    #[test]
    fn each_order_sorts_by_what_it_says_and_ties_keep_their_places() {
        let by = |order: Order| select(&list(), &Finder { order, ..Finder::default() });

        assert_eq!(by(Order::Newest), [1, 2, 0, 3]);
        assert_eq!(by(Order::Oldest), [3, 0, 2, 1]);
        assert_eq!(by(Order::MostSupport), [1, 0, 2, 3], "0.25 twice: the one listed first stays first");
        assert_eq!(by(Order::ClosingSoonest), [1, 0, 2, 3], "one that never closes goes last");
    }

    #[test]
    fn a_page_is_a_window_and_past_the_end_it_is_the_last() {
        assert_eq!(pages(0, 10), 1);
        assert_eq!(pages(10, 10), 1);
        assert_eq!(pages(11, 10), 2);
        assert_eq!(window(25, 0, 10), 0..10);
        assert_eq!(window(25, 2, 10), 20..25);
        assert_eq!(window(25, 9, 10), 20..25, "past the end");
        assert_eq!(window(0, 0, 10), 0..0);
    }

    #[test]
    fn older_pages_are_read_as_a_page_comes_up_short_or_the_oldest_are_asked_for() {
        let on = |page: usize, order: Order| Finder { page, order, ..Finder::default() };

        assert!(!wants_older(20, &on(0, Order::Newest), 10, true), "the page is full");
        assert!(wants_older(20, &on(2, Order::Newest), 10, true), "the third page is not");
        assert!(wants_older(5, &on(0, Order::Newest), 10, true), "a search left it short");
        assert!(!wants_older(5, &on(0, Order::Newest), 10, false), "and the node has no more");
        assert!(wants_older(20, &on(0, Order::Oldest), 10, true), "the oldest are at the far end");
    }

    #[test]
    fn a_page_says_what_it_shows_and_how_far_a_search_has_been() {
        assert_eq!(summary(45, 1, 10, false, 45, 45), "11 to 20 of 45");
        assert_eq!(summary(20, 0, 10, false, 20, 45), "1 to 10 of 20 read of 45");
        assert_eq!(summary(3, 0, 10, true, 20, 45), "1 to 3 of 3 found in the 20 read of 45");
        assert_eq!(summary(3, 0, 10, true, 45, 45), "1 to 3 of 3 found");
        assert_eq!(summary(0, 0, 10, true, 45, 45), "0 found");
    }

    #[test]
    fn a_share_is_of_numbers_of_any_size() {
        assert_eq!(share("1", "2"), 0.5);
        assert_eq!(share("0", "0"), 0.0);
        assert_eq!(share("nonsense", "2"), 0.0);
        assert!((share("6666666666666666666666666666666666666", "10000000000000000000000000000000000000") - 0.6667).abs() < 0.001);
    }

    #[derive(Clone, Debug, PartialEq)]
    struct One(u64);

    impl Counted for One {
        fn version(&self) -> u64 {
            self.0
        }
    }

    fn ones(versions: &[u64]) -> Vec<One> {
        versions.iter().map(|version| One(*version)).collect()
    }

    #[test]
    fn decided_reads_pages_down_to_the_oldest_kept_and_sees_gaps() {
        let mut listing = Decided::<One>::default();

        assert_eq!(listing.next(), None, "nothing read");
        listing.take(45, 1, ones(&[45, 44, 42]), None);
        assert_eq!((listing.version, listing.bottom, listing.next()), (45, Some(42), Some(42)));
        assert_eq!(listing.gaps(), [(43, 43)]);
        assert_eq!(listing.kept(), 45);
        listing.take(45, 1, ones(&[41, 40]), Some(42));
        assert_eq!(listing.newest_first().map(|one| one.0).collect::<Vec<_>>(), [45, 44, 42, 41, 40]);
        listing.take(45, 1, ones(&[1]), Some(2));
        assert_eq!(listing.next(), None, "all the way down");

        let mut empty = Decided::<One>::default();
        empty.take(0, 1, Vec::new(), None);
        assert_eq!((empty.next(), empty.kept()), (None, 0), "nothing decided");
    }

    #[test]
    fn a_change_of_the_search_goes_back_to_the_first_page() {
        let ctx = egui::Context::default();
        let palette = PALETTE;
        let mut finder = Finder { page: 3, ..Finder::default() };
        let items = list();
        let frame = |finder: &mut Finder, events: Vec<egui::Event>| {
            let mut output = ctx.run_ui(egui::RawInput { events, ..Default::default() }, |ui| {
                controls(ui, &palette, finder, "Search", &items, &OPEN_ORDERS, &|kind: &str| kind.to_string());
            });
            output.textures_delta.clear();
        };

        frame(&mut finder, Vec::new());
        assert_eq!(finder.page, 3, "nothing asked: the page stays");

        // Tapping the box, then typing.
        let at = egui::pos2(30.0, 15.0);
        let press = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        frame(&mut finder, vec![egui::Event::PointerMoved(at), press(true)]);
        frame(&mut finder, vec![press(false)]);
        frame(&mut finder, vec![egui::Event::Text("fee".to_string())]);

        assert_eq!(finder.search, "fee");
        assert_eq!(finder.page, 0);
    }

    #[test]
    fn a_short_list_keeps_a_short_page_and_a_long_one_stays_in_its_width() {
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(320.0, 600.0));
        let many: Vec<Item> = ["consensus", "bootnodes", "name_prices", "collections", "federation", "directory", "fees", "reward", "limits", "upgrade", "name"]
            .iter()
            .map(|kind| item(kind, kind, 1, 0.0, None))
            .collect();
        let height = |items: &[Item]| {
            let mut drawn = egui::Rect::NOTHING;

            for _ in 0..2 {
                let mut output = ctx.run_ui(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |ui| {
                    drawn = ui.scope(|ui| controls(ui, &PALETTE, &mut Finder::default(), "Search", items, &OPEN_ORDERS, &|kind: &str| kind.to_string())).response.rect;
                });
                output.textures_delta.clear();
            }

            drawn
        };
        let (none, one, all) = (height(&[]), height(&many[..1]), height(&many));

        assert!(none.height() < 1.0, "nothing to find: nothing shown: {none:?}");
        assert!(one.height() < 60.0, "one: a search box alone: {one:?}");
        assert!(all.height() > one.height() * 2.0, "many: an order and kinds: {all:?}");
        assert!(all.right() <= screen.right() + 1.0, "within the width: {all:?}");
    }

    const PALETTE: Palette = Palette {
        paper: egui::Color32::BLACK,
        nav: egui::Color32::BLACK,
        card: egui::Color32::from_gray(20),
        row: egui::Color32::from_gray(30),
        line: egui::Color32::from_gray(40),
        ink: egui::Color32::WHITE,
        soft: egui::Color32::LIGHT_GRAY,
        muted: egui::Color32::GRAY,
        heading: egui::Color32::GRAY,
        accent: egui::Color32::LIGHT_BLUE,
        on_accent: egui::Color32::BLACK,
        good: egui::Color32::GREEN,
        warn: egui::Color32::YELLOW,
        bad: egui::Color32::RED,
        dark: true,
    };
}
