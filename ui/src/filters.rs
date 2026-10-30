//! Filtering an address's transactions, as both apps' pages do: what someone asked for, the query
//! it comes to for GET /accounts/{address}/txs, and the same test applied here, to what an older
//! node, which ignores those parameters, sent unfiltered, and to what the wallet proved. Plain data,
//! apart from the controls that set it, and tested as such.

use {
    crate::theme::Palette,
    crate::widgets,
    egui::{RichText, TextEdit, Ui},
};

/// The kinds offered, by the name the API gives them.
pub const KINDS: [(&str, &str); 8] = [
    ("transfer", "Transfer"),
    ("contract_call", "Call"),
    ("evm_call", "EVM call"),
    ("deploy", "Deploy"),
    ("name", "Name"),
    ("claim", "Claim"),
    ("multisig", "Multisig"),
    ("approve_change", "Vote"),
];

/// What the token filter says for the chain's own coin, as the API does.
pub const COIN: &str = "coin";

const DAY: u64 = 86_400_000;
const HOUR: u64 = 3_600_000;

/// Which way a transaction goes for the address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// It took part, and did not send it.
    In,
    /// It sent it.
    Out,
}

/// When it landed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum When {
    #[default]
    Any,
    Day,
    Week,
    Month,
    /// Between the dates typed.
    Dates,
}

/// What someone asked of a listing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TxFilter {
    /// Whether the controls show: not a filter.
    pub open: bool,
    /// The kind, by the name the API gives it.
    pub kind: Option<String>,
    pub direction: Option<Direction>,
    /// An address it touched, as typed.
    pub counterparty: String,
    /// The token picked: COIN, or a contract's address.
    pub token: Option<String>,
    /// Another token's address, as typed: over the one picked.
    pub token_typed: String,
    pub when: When,
    /// The dates typed, as YYYY-MM-DD, in UTC.
    pub from_date: String,
    pub until_date: String,
}

/// What can be said of a transaction, to test it here. What is not known passes.
#[derive(Clone, Debug, Default)]
pub struct Facts<'a> {
    /// Its kind, and what a multisig sent as it; empty when not known.
    pub kinds: Vec<&'a str>,
    /// Whether the address, or any of the account's, sent it, and whether it took part otherwise.
    pub sent: bool,
    pub took_part: bool,
    /// Everyone it names.
    pub parties: Vec<&'a str>,
    /// What it moved: COIN, or tokens' addresses.
    pub tokens: Vec<&'a str>,
    /// Its block's time, in milliseconds since the Unix epoch, if known.
    pub time: Option<u64>,
}

impl TxFilter {
    /// The token to filter by, if any.
    pub fn token(&self) -> Option<&str> {
        match self.token_typed.trim() {
            "" => self.token.as_deref(),
            typed => Some(typed),
        }
    }

    /// How many filters are on.
    pub fn on(&self) -> usize {
        [self.kind.is_some(), self.direction.is_some(), !self.counterparty.trim().is_empty(), self.token().is_some(), self.when != When::Any].into_iter().filter(|on| *on).count()
    }

    /// Turns every filter off, the controls staying as they are.
    pub fn clear(&mut self) {
        *self = TxFilter { open: self.open, ..TxFilter::default() };
    }

    /// The times it starts and ends at, in milliseconds since the Unix epoch, as of now. A span is
    /// counted from the hour, so what is asked stays the same from one frame to the next. A date
    /// that does not read is no bound, for problem to say.
    pub fn bounds(&self, now: u64) -> (Option<u64>, Option<u64>) {
        let back = |span: u64| Some((now - now % HOUR).saturating_sub(span));

        match self.when {
            When::Any => (None, None),
            When::Day => (back(DAY), None),
            When::Week => (back(7 * DAY), None),
            When::Month => (back(30 * DAY), None),
            When::Dates => (date(&self.from_date), date(&self.until_date).map(|until| until + DAY - 1)),
        }
    }

    /// The query string it comes to, without a leading &: empty with none on. As of now.
    pub fn query(&self, now: u64) -> String {
        let (from, until) = self.bounds(now);
        let direction = self.direction.map(|direction| match direction {
            Direction::In => "in".to_string(),
            Direction::Out => "out".to_string(),
        });
        let counterparty = Some(self.counterparty.trim()).filter(|party| !party.is_empty()).map(str::to_string);

        [
            ("kind", self.kind.clone()),
            ("direction", direction),
            ("counterparty", counterparty),
            ("token", self.token().map(str::to_string)),
            ("from", from.map(|time| time.to_string())),
            ("until", until.map(|time| time.to_string())),
        ]
        .into_iter()
        .filter_map(|(name, value)| value.map(|value| format!("{name}={}", encode(&value))))
        .collect::<Vec<_>>()
        .join("&")
    }

    /// What is wrong with what was typed, for the page to say; an address is whatever the app reads
    /// as one.
    pub fn problem(&self, is_address: impl Fn(&str) -> bool) -> Option<String> {
        let party = self.counterparty.trim();
        let token = self.token_typed.trim();

        if !party.is_empty() && !is_address(party) {
            return Some(format!("{party} is not an address."));
        }

        if !token.is_empty() && !is_address(token) {
            return Some(format!("{token} is not a token's address."));
        }

        if self.when == When::Dates {
            for typed in [&self.from_date, &self.until_date] {
                if !typed.trim().is_empty() && date(typed).is_none() {
                    return Some(format!("{} is not a date: write it as 2026-10-30.", typed.trim()));
                }
            }

            let (from, until) = self.bounds(0);

            if from.zip(until).is_some_and(|(from, until)| from > until) {
                return Some("The first date is after the last.".to_string());
            }
        }

        None
    }

    /// Whether a transaction passes, as of now, by what can be said of it.
    pub fn passes(&self, facts: &Facts, now: u64) -> bool {
        let (from, until) = self.bounds(now);
        let party = self.counterparty.trim();

        self.kind.as_ref().is_none_or(|kind| facts.kinds.is_empty() || facts.kinds.contains(&kind.as_str()))
            && self.direction.is_none_or(|direction| match direction {
                Direction::Out => facts.sent,
                Direction::In => facts.took_part,
            })
            && (party.is_empty() || facts.parties.contains(&party))
            && self.token().is_none_or(|token| facts.tokens.contains(&token))
            && facts.time.is_none_or(|time| from.is_none_or(|from| time >= from) && until.is_none_or(|until| time <= until))
    }
}

/// Text made safe for a URL's query.
fn encode(text: &str) -> String {
    text.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b':' => (byte as char).to_string(),
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// The start of a day written YYYY-MM-DD, in milliseconds since the Unix epoch, in UTC.
pub fn date(text: &str) -> Option<u64> {
    let mut parts = text.trim().split('-');
    let (year, month, day): (i64, u32, u32) = (parts.next()?.parse().ok()?, parts.next()?.parse().ok()?, parts.next()?.parse().ok()?);
    let long = [31, if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];

    if parts.next().is_some() || !(1970..=9999).contains(&year) || !(1..=12).contains(&month) || !(1..=*long.get(month as usize - 1)?).contains(&day) {
        return None;
    }

    // Days since 1970-01-01 (Howard Hinnant's algorithm, as the apps' dates are written by).
    let shifted = if month <= 2 { year - 1 } else { year };
    let (era, year_of_era) = (shifted.div_euclid(400), shifted.rem_euclid(400));
    let day_of_year = (153 * i64::from(if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;

    u64::try_from(era * 146_097 + day_of_era - 719_468).ok()?.checked_mul(DAY)
}

/// What the controls offer besides what every chain has.
#[derive(Clone, Copy, Debug, Default)]
pub struct Offered<'a> {
    /// What the chain calls its coin.
    pub coin: &'a str,
    /// Tokens to pick from: each one's address, and what to call it.
    pub tokens: &'a [(String, String)],
}

/// The most tokens offered by name: a phone's rows run out, and any other can be typed.
const TOKENS_SHOWN: usize = 6;

/// The Filters row, saying how many are on, with a way to clear them; below it, while open, a
/// choice or a field for each: all of them wrap to the width there is, as a phone's. `problem` is
/// what the page found wrong with what was typed.
pub fn controls(ui: &mut Ui, palette: &Palette, filter: &mut TxFilter, offered: Offered, problem: Option<&str>) {
    let on = filter.on();
    let label = match on {
        0 => "Filters".to_string(),
        on => format!("Filters · {on} on"),
    };

    ui.horizontal_wrapped(|ui| {
        if widgets::quiet(ui, palette, &label).clicked() {
            filter.open = !filter.open;
        }

        if on > 0 && widgets::quiet(ui, palette, "Clear").clicked() {
            filter.clear();
        }
    });

    if !filter.open {
        return;
    }

    let heading = |ui: &mut Ui, text: &str| {
        ui.add_space(4.0);
        widgets::caps(ui, palette, text);
    };

    heading(ui, "Direction");
    let mut direction = filter.direction;
    widgets::segmented_rows(ui, palette, &mut direction, &[(None, "Any"), (Some(Direction::In), "In"), (Some(Direction::Out), "Out")]);
    filter.direction = direction;

    heading(ui, "Kind");
    let kinds: Vec<(Option<&str>, &str)> = std::iter::once((None, "Any")).chain(KINDS.iter().map(|(kind, name)| (Some(*kind), *name))).collect();
    let mut kind = filter.kind.as_deref();

    if widgets::segmented_rows(ui, palette, &mut kind, &kinds) {
        filter.kind = kind.map(str::to_string);
    }

    heading(ui, "Token");
    let coin = if offered.coin.is_empty() { "Coin" } else { offered.coin };
    // Tokens that go by the same name, as a DEX's pool shares do, cannot be told apart by it.
    let named = |name: &str| offered.tokens.iter().filter(|(_, other)| other == name).count() == 1;
    let tokens: Vec<(Option<&str>, &str)> = [(None, "Any"), (Some(COIN), coin)]
        .into_iter()
        .chain(offered.tokens.iter().filter(|(_, name)| named(name)).take(TOKENS_SHOWN).map(|(address, name)| (Some(address.as_str()), name.as_str())))
        .collect();
    let mut token = filter.token.as_deref();

    if widgets::segmented_rows(ui, palette, &mut token, &tokens) {
        filter.token = token.map(str::to_string);
        filter.token_typed.clear();
    }

    ui.add(TextEdit::singleline(&mut filter.token_typed).hint_text("or a token's address").font(egui::TextStyle::Monospace).desired_width(f32::INFINITY));

    heading(ui, "Counterparty");
    ui.add(TextEdit::singleline(&mut filter.counterparty).hint_text("an address it touched").font(egui::TextStyle::Monospace).desired_width(f32::INFINITY));

    heading(ui, "When");
    widgets::segmented_rows(ui, palette, &mut filter.when, &[(When::Any, "Any time"), (When::Day, "24 hours"), (When::Week, "7 days"), (When::Month, "30 days"), (When::Dates, "Dates")]);

    if filter.when == When::Dates {
        ui.columns(2, |columns| {
            columns[0].add(TextEdit::singleline(&mut filter.from_date).hint_text("from 2026-10-01").desired_width(f32::INFINITY));
            columns[1].add(TextEdit::singleline(&mut filter.until_date).hint_text("until 2026-10-30").desired_width(f32::INFINITY));
        });
    }

    if let Some(problem) = problem {
        ui.add(egui::Label::new(RichText::new(problem).color(palette.bad)).wrap());
    }

    ui.add_space(4.0);
}

/// How a filtered listing's answer stands: whether the node filtered it at all, how much a request
/// looked through, and where to go on from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Answered {
    /// Whether the node said what it went by: an older node, which does not, sent everything.
    pub filtered: bool,
    /// How many entries it looked through for this answer, on a node that says.
    pub scanned: u64,
    /// Whether it stopped at its bound with its page not full: more may match further on.
    pub limited: bool,
}

/// What a listing says of itself under filters, for the line below it: how many, what a page came
/// short for, and what an older node could not do. None with no filter on and nothing to say.
pub fn said(found: usize, more: bool, answered: &Answered, on: usize) -> Option<String> {
    if on == 0 {
        return None;
    }

    let count = match found {
        0 => "None match so far".to_string(),
        1 => "1 matches so far".to_string(),
        found => format!("{found} match so far"),
    };

    Some(match (answered.filtered, answered.limited, more) {
        (false, _, true) => format!("{count}, of what is loaded: this node is older and cannot filter, so only what was read is searched. Read more to look further back."),
        (false, _, false) => format!("{count}, of all this node listed: it is older and cannot filter."),
        (true, true, _) => format!("{count}: the node looked through {} more and stopped there, as it does for each request. Ask it to go on.", answered.scanned),
        (true, false, true) => format!("{count}, and there are more."),
        (true, false, false) => match found {
            0 => "None match, in all of this node's index.".to_string(),
            1 => "1 matches, in all of this node's index.".to_string(),
            found => format!("{found} match, in all of this node's index."),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_793_388_634_374;

    #[test]
    fn dates_read_as_the_day_they_start_in_utc() {
        assert_eq!(date("1970-01-01"), Some(0));
        assert_eq!(date("1970-01-02"), Some(DAY));
        assert_eq!(date("2026-10-30"), Some(1_793_318_400_000));
        assert_eq!(date(" 2024-02-29 "), Some(1_709_164_800_000));

        for bad in ["", "2026", "2026-13-01", "2026-02-29", "2026-10-32", "2026-10-30-1", "soon", "1969-12-31", "2026-0-1"] {
            assert_eq!(date(bad), None, "{bad}");
        }
    }

    #[test]
    fn no_filter_comes_to_no_query_and_each_says_it_is_on() {
        let mut filter = TxFilter::default();

        assert_eq!((filter.on(), filter.query(NOW).as_str()), (0, ""));

        filter.open = true;
        assert_eq!(filter.on(), 0, "showing the controls filters nothing");

        filter.kind = Some("transfer".to_string());
        filter.direction = Some(Direction::In);
        filter.counterparty = " ed25519:0xab ".to_string();
        filter.token = Some(COIN.to_string());
        filter.when = When::Dates;
        filter.from_date = "2026-10-01".to_string();
        filter.until_date = "2026-10-02".to_string();

        assert_eq!(filter.on(), 5);
        assert_eq!(filter.query(NOW), format!("kind=transfer&direction=in&counterparty=ed25519:0xab&token=coin&from={}&until={}", date("2026-10-01").unwrap(), date("2026-10-03").unwrap() - 1));

        filter.clear();
        assert_eq!(filter, TxFilter { open: true, ..TxFilter::default() }, "clearing leaves the controls showing");
    }

    #[test]
    fn a_token_typed_goes_over_the_one_picked() {
        let mut filter = TxFilter { token: Some(COIN.to_string()), ..TxFilter::default() };

        assert_eq!(filter.token(), Some(COIN));
        assert_eq!(filter.query(NOW), "token=coin");

        filter.token_typed = "contract:0xcd".to_string();
        assert_eq!(filter.token(), Some("contract:0xcd"));
        assert_eq!(filter.query(NOW), "token=contract:0xcd");
        assert_eq!(filter.on(), 1);
    }

    #[test]
    fn a_span_of_time_stays_the_same_query_within_the_hour_so_one_frame_asks_what_the_last_did() {
        let filter = TxFilter { when: When::Week, ..TxFilter::default() };
        let (from, until) = filter.bounds(NOW);

        assert_eq!(from, Some(NOW - NOW % HOUR - 7 * DAY));
        assert_eq!(until, None);
        assert_eq!(filter.query(NOW), filter.query(NOW + 1));
        assert_ne!(filter.query(NOW), filter.query(NOW + HOUR));
        assert_eq!(TxFilter { when: When::Day, ..TxFilter::default() }.bounds(1_000).0, Some(0), "never before the epoch");
    }

    #[test]
    fn what_was_typed_is_checked_before_it_is_asked() {
        let is_address = |text: &str| text.starts_with("ed25519:") || text.starts_with("contract:");
        let problem = |filter: TxFilter| filter.problem(is_address);

        assert_eq!(problem(TxFilter::default()), None);
        assert!(problem(TxFilter { counterparty: "bob".to_string(), ..TxFilter::default() }).is_some_and(|why| why.contains("bob")));
        assert_eq!(problem(TxFilter { counterparty: "ed25519:0x1".to_string(), ..TxFilter::default() }), None);
        assert!(problem(TxFilter { token_typed: "nope".to_string(), ..TxFilter::default() }).is_some_and(|why| why.contains("token")));

        let dates = |from: &str, until: &str| TxFilter { when: When::Dates, from_date: from.to_string(), until_date: until.to_string(), ..TxFilter::default() };

        assert_eq!(problem(dates("", "")), None);
        assert_eq!(problem(dates("2026-10-01", "2026-10-01")), None);
        assert!(problem(dates("soon", "")).is_some_and(|why| why.contains("not a date")));
        assert!(problem(dates("2026-10-02", "2026-10-01")).is_some_and(|why| why.contains("after")));
        assert_eq!(problem(TxFilter { when: When::Any, from_date: "soon".to_string(), ..TxFilter::default() }), None, "dates not in use");
    }

    #[test]
    fn the_same_test_runs_here_on_what_a_node_did_not_filter() {
        let facts = Facts { kinds: vec!["transfer"], sent: true, took_part: false, parties: vec!["ed25519:0xaa", "ed25519:0xbb"], tokens: vec![COIN], time: Some(NOW) };
        let passes = |filter: TxFilter| filter.passes(&facts, NOW);

        assert!(passes(TxFilter::default()));
        assert!(passes(TxFilter { kind: Some("transfer".to_string()), ..TxFilter::default() }));
        assert!(!passes(TxFilter { kind: Some("deploy".to_string()), ..TxFilter::default() }));
        assert!(passes(TxFilter { direction: Some(Direction::Out), ..TxFilter::default() }));
        assert!(!passes(TxFilter { direction: Some(Direction::In), ..TxFilter::default() }));
        assert!(passes(TxFilter { counterparty: "ed25519:0xbb".to_string(), ..TxFilter::default() }));
        assert!(!passes(TxFilter { counterparty: "ed25519:0xcc".to_string(), ..TxFilter::default() }));
        assert!(passes(TxFilter { token: Some(COIN.to_string()), ..TxFilter::default() }));
        assert!(!passes(TxFilter { token: Some("contract:0x01".to_string()), ..TxFilter::default() }));
        assert!(passes(TxFilter { when: When::Day, ..TxFilter::default() }));
        assert!(!TxFilter { when: When::Day, ..TxFilter::default() }.passes(&facts, NOW + 2 * DAY));

        // What is not known passes: a record kept from before it said its kind, or its time.
        let vague = Facts { kinds: Vec::new(), time: None, ..facts.clone() };

        assert!(TxFilter { kind: Some("deploy".to_string()), when: When::Day, ..TxFilter::default() }.passes(&vague, NOW));
    }

    #[test]
    fn a_listing_says_what_it_came_to_under_filters() {
        let new = Answered { filtered: true, scanned: 40, limited: false };
        let far = Answered { filtered: true, scanned: 1000, limited: true };
        let old = Answered::default();

        assert_eq!(said(3, false, &new, 0), None, "no filter on: nothing to say");
        assert_eq!(said(3, false, &new, 1).as_deref(), Some("3 match, in all of this node's index."));
        assert_eq!(said(0, false, &new, 2).as_deref(), Some("None match, in all of this node's index."));
        assert!(said(2, true, &new, 1).is_some_and(|said| said.contains("there are more")));
        assert!(said(0, true, &far, 1).is_some_and(|said| said.contains("1000") && said.contains("go on")));
        assert!(said(1, true, &old, 1).is_some_and(|said| said.contains("older") && said.contains("loaded")));
        assert!(said(1, false, &old, 1).is_some_and(|said| said.contains("older")));
    }
}
