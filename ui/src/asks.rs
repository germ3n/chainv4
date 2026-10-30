//! What became of what an app asked its node, whichever app: what is out, what failed and why, and
//! when to ask it again. Plain data, and tested as such; the requests are each app's.
//!
//! A page asks for what it wants as often as it draws, so a failure holds the next try back for a
//! moment, longer the more it keeps failing, rather than ask a node that is down many times a
//! second. Retry and Reload go past the hold. An answer always comes: the requests themselves have
//! time limits (`net`), so nothing stays out for good, and everything can be asked again.

use {
    crate::net::Failure,
    std::collections::{HashMap, HashSet},
    std::hash::Hash,
};

/// Seconds before asking again what failed: at first, and at most, as it keeps failing.
const HOLD_FIRST: f64 = 3.0;
const HOLD_MOST: f64 = 30.0;
/// Seconds a failure counts after nothing asked for it any more: what asked for it has gone, as a
/// proof at a tip since passed.
const WANTED_FOR: f64 = 20.0;

/// One ask that last failed.
#[derive(Clone, Debug)]
struct Failed {
    failure: Failure,
    /// How many times it failed in a row.
    tries: u32,
    /// When it may be sent again.
    held_until: f64,
    /// When a page last asked for it.
    wanted: f64,
}

/// What is out, and what last failed.
#[derive(Clone, Debug)]
pub struct Asks<A> {
    out: HashSet<A>,
    failed: HashMap<A, Failed>,
    /// What went out since the app last took it.
    sent: Vec<A>,
}

impl<A> Default for Asks<A> {
    fn default() -> Asks<A> {
        Asks { out: HashSet::new(), failed: HashMap::new(), sent: Vec::new() }
    }
}

impl<A: Clone + Eq + Hash> Asks<A> {
    /// Whether to send this now, marking it out if so: not while the same is out, nor while a
    /// failure holds it back.
    pub fn begin(&mut self, ask: &A, now: f64) -> bool {
        if let Some(failed) = self.failed.get_mut(ask) {
            failed.wanted = now;

            if now < failed.held_until {
                return false;
            }
        }

        self.force(ask)
    }

    /// Whether to send this now, whatever failed before: what someone pressed for, and what is sent
    /// once rather than polled. Only the same being out holds it back.
    pub fn force(&mut self, ask: &A) -> bool {
        let sending = self.out.insert(ask.clone());

        if sending {
            self.sent.push(ask.clone());
        }

        sending
    }

    /// Notes what came of something asked: None for an answer, or why there is none.
    pub fn ended(&mut self, ask: &A, failure: Option<&Failure>, now: f64) {
        self.out.remove(ask);

        let Some(failure) = failure else {
            self.failed.remove(ask);

            return;
        };
        let tries = self.failed.get(ask).map_or(0, |failed| failed.tries) + 1;
        // Only trouble on the way grows the hold: a 404 is the answer until it is not, as a
        // transaction not on the chain yet.
        let hold = match failure.is_trouble() {
            true => (HOLD_FIRST * 2f64.powi(tries.min(16) as i32 - 1)).min(HOLD_MOST),
            false => HOLD_FIRST,
        };

        self.failed.insert(ask.clone(), Failed { failure: failure.clone(), tries, held_until: now + hold, wanted: now });
    }

    /// Whether this is out.
    pub fn asking(&self, ask: &A) -> bool {
        self.out.contains(ask)
    }

    /// Why this last failed, until it is answered.
    pub fn failure(&self, ask: &A) -> Option<&Failure> {
        self.failed.get(ask).map(|failed| &failed.failure)
    }

    /// Lets go of what failed before, for it to be asked anew, whatever held it.
    pub fn forget(&mut self, ask: &A) {
        self.failed.remove(ask);
    }

    /// What failed for trouble on the way and is still wanted: what a page says is wrong, each with
    /// why, the one that failed most times first. Nothing once a page stopped asking for it.
    pub fn troubles(&self, now: f64) -> Vec<(&A, &Failure)> {
        let mut troubles: Vec<_> = self.failed.iter().filter(|(_, failed)| failed.failure.is_trouble() && now - failed.wanted <= WANTED_FOR).collect();
        troubles.sort_by_key(|(_, failed)| std::cmp::Reverse(failed.tries));

        troubles.into_iter().map(|(ask, failed)| (ask, &failed.failure)).collect()
    }

    /// What went out since last time, each as often as it did.
    pub fn take_sent(&mut self) -> Vec<A> {
        std::mem::take(&mut self.sent)
    }
}

/// The most pages kept, and asks kept for each: past those, the oldest go.
const PAGES_KEPT: usize = 32;
const ASKS_KEPT: usize = 256;

/// What each page asked since it was first shown, whether it still asks or was answered once for
/// good: what a reload asks again, while what the page shows stays until the answers come.
#[derive(Clone, Debug)]
pub struct PageAsks<P, A> {
    /// The latest page last.
    pages: Vec<(P, Vec<A>)>,
}

impl<P, A> Default for PageAsks<P, A> {
    fn default() -> PageAsks<P, A> {
        PageAsks { pages: Vec::new() }
    }
}

impl<P: Clone + PartialEq, A: Clone + PartialEq> PageAsks<P, A> {
    /// Notes what this page asked.
    pub fn note(&mut self, page: &P, asks: impl IntoIterator<Item = A>) {
        let place = match self.pages.iter().position(|(known, _)| known == page) {
            Some(place) => place,
            None => {
                self.pages.push((page.clone(), Vec::new()));

                if self.pages.len() > PAGES_KEPT {
                    self.pages.remove(0);
                }

                self.pages.len() - 1
            }
        };
        let kept = &mut self.pages[place].1;

        for ask in asks {
            if !kept.contains(&ask) {
                kept.push(ask);
            }
        }

        if kept.len() > ASKS_KEPT {
            kept.drain(..kept.len() - ASKS_KEPT);
        }
    }

    /// What this page asked.
    pub fn of(&self, page: &P) -> &[A] {
        self.pages.iter().find(|(known, _)| known == page).map_or(&[], |(_, asks)| asks)
    }
}

/// The least a reload shows it is working, in seconds, however quick the answers: long enough to see
/// it did something.
const SHOWN_FOR: f64 = 0.5;

/// A reload under way: what it asked, until each has ended.
#[derive(Clone, Debug)]
pub struct Reload<A> {
    waiting: Vec<A>,
    since: Option<f64>,
}

impl<A> Default for Reload<A> {
    fn default() -> Reload<A> {
        Reload { waiting: Vec::new(), since: None }
    }
}

impl<A> Reload<A> {
    /// Starts one, waiting for these.
    pub fn start(&mut self, waiting: Vec<A>, now: f64) {
        self.waiting = waiting;
        self.since = Some(now);
    }

    /// Whether it still works: while any of what it asked is out, or for a moment at least.
    pub fn working(&mut self, now: f64, asking: impl Fn(&A) -> bool) -> bool {
        self.waiting.retain(|ask| asking(ask));

        let Some(since) = self.since else {
            return false;
        };
        let working = !self.waiting.is_empty() || now - since < SHOWN_FOR;

        if !working {
            self.since = None;
        }

        working
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unreachable() -> Failure {
        Failure { status: 0, message: "no answer within 30 seconds".to_string() }
    }

    fn missing() -> Failure {
        Failure { status: 404, message: "no such block".to_string() }
    }

    #[test]
    fn the_same_question_is_not_asked_twice_at_once_and_an_answer_lets_it_be_asked_again() {
        let mut asks = Asks::default();

        assert!(asks.begin(&"status", 0.0));
        assert!(!asks.begin(&"status", 0.0));
        assert!(asks.asking(&"status"));
        assert!(!asks.force(&"status"), "not even when pressed for");

        asks.ended(&"status", None, 1.0);

        assert!(!asks.asking(&"status"));
        assert!(asks.begin(&"status", 1.0));
        assert_eq!(asks.take_sent(), ["status", "status"]);
        assert_eq!(asks.take_sent(), Vec::<&str>::new());
    }

    #[test]
    fn a_failure_holds_the_next_try_longer_the_more_it_fails_until_it_is_answered() {
        let mut asks = Asks::default();
        let mut now = 0.0;

        assert!(asks.begin(&"page", now));
        asks.ended(&"page", Some(&unreachable()), now);

        assert_eq!(asks.failure(&"page"), Some(&unreachable()));
        assert!(!asks.asking(&"page"), "never stuck as out");
        assert!(!asks.begin(&"page", now + 1.0), "held for a moment");
        assert!(asks.begin(&"page", now + HOLD_FIRST), "then asked again");
        assert_eq!(asks.failure(&"page"), Some(&unreachable()), "what failed shows until it is answered");

        now += HOLD_FIRST;
        asks.ended(&"page", Some(&unreachable()), now);
        assert!(!asks.begin(&"page", now + HOLD_FIRST), "twice the hold, the second time");
        assert!(asks.begin(&"page", now + HOLD_FIRST * 2.0));

        // However long it goes on, no longer than the most.
        for _ in 0..40 {
            now += HOLD_MOST;
            asks.ended(&"page", Some(&unreachable()), now);
            assert!(!asks.begin(&"page", now + 1.0));
            assert!(asks.begin(&"page", now + HOLD_MOST));
        }

        asks.ended(&"page", None, now);
        assert_eq!(asks.failure(&"page"), None);
        assert!(asks.begin(&"page", now), "answered: nothing holds it");
    }

    #[test]
    fn a_failure_that_says_something_of_what_was_asked_does_not_grow_its_hold() {
        let mut asks = Asks::default();
        let mut now = 0.0;

        for _ in 0..5 {
            assert!(asks.begin(&"block", now));
            asks.ended(&"block", Some(&missing()), now);
            now += HOLD_FIRST;
        }

        assert_eq!(asks.failure(&"block"), Some(&missing()));
        assert!(asks.troubles(now).is_empty(), "a 404 is no trouble on the way");
    }

    #[test]
    fn retry_goes_past_the_hold_and_clears_the_failure() {
        let mut asks = Asks::default();

        assert!(asks.begin(&"page", 0.0));
        asks.ended(&"page", Some(&unreachable()), 0.0);
        assert!(!asks.begin(&"page", 0.5));

        asks.forget(&"page");
        assert_eq!(asks.failure(&"page"), None, "what failed goes as it is asked again");
        assert!(asks.begin(&"page", 0.5), "and it is asked at once");
        assert!(asks.asking(&"page"));
    }

    #[test]
    fn what_nothing_asks_for_any_more_stops_being_trouble() {
        let mut asks = Asks::default();

        assert!(asks.begin(&"proof at an old tip", 0.0));
        assert!(asks.begin(&"status", 0.0));
        asks.ended(&"proof at an old tip", Some(&unreachable()), 0.0);
        asks.ended(&"status", Some(&unreachable()), 0.0);

        assert_eq!(asks.troubles(1.0).len(), 2);

        // The status goes on being asked for, held or not; the proof does not.
        for time in [5.0, 10.0, 15.0, 20.0, 25.0] {
            let _ = asks.begin(&"status", time);
            asks.ended(&"status", Some(&unreachable()), time);
        }

        assert_eq!(asks.troubles(25.0).iter().map(|(ask, _)| **ask).collect::<Vec<_>>(), ["status"]);
    }

    #[test]
    fn a_page_keeps_what_it_asked_each_once_for_a_reload_to_ask_again() {
        let mut pages: PageAsks<&str, u32> = PageAsks::default();

        pages.note(&"block", [1, 2, 1]);
        pages.note(&"tx", [3]);
        pages.note(&"block", [2, 4]);

        assert_eq!(pages.of(&"block"), [1, 2, 4]);
        assert_eq!(pages.of(&"tx"), [3]);
        assert_eq!(pages.of(&"never shown"), [] as [u32; 0]);

        // Only so many of either.
        pages.note(&"long", 0..(ASKS_KEPT as u32 + 10));
        assert_eq!(pages.of(&"long").len(), ASKS_KEPT);
        assert_eq!(pages.of(&"long")[0], 10, "the latest stay");
    }

    #[test]
    fn a_reload_works_while_what_it_asked_is_out_and_a_moment_at_least() {
        let mut reload = Reload::default();

        assert!(!reload.working(0.0, |_: &u32| false), "none started");

        reload.start(vec![1, 2], 1.0);
        assert!(reload.working(1.0, |_| true));
        assert!(reload.working(1.1, |_| false), "answered at once, it still shows for a moment");
        assert!(!reload.working(1.0 + SHOWN_FOR, |_| false));
        assert!(!reload.working(9.0, |_| true), "over: what is out now is others' asks");

        reload.start(vec![1], 10.0);
        assert!(reload.working(20.0, |ask| *ask == 1), "until it ends, however long");
        assert!(!reload.working(20.1, |_| false));
    }

    #[test]
    fn a_failure_is_put_in_a_sentence() {
        assert_eq!(unreachable().sentence(), "The node did not answer in time: no answer within 30 seconds.");
        assert_eq!(Failure { status: 0, message: "connection refused".to_string() }.sentence(), "Could not reach the node: connection refused.");
        assert_eq!(Failure { status: 503, message: "503 Service Unavailable".to_string() }.sentence(), "The node had trouble answering: 503 Service Unavailable.");
        assert_eq!(missing().sentence(), "No such block.");
        assert!(unreachable().is_trouble() && !missing().is_trouble());
        assert!(Failure { status: 429, message: "slow down".to_string() }.is_trouble());
        assert!(!Failure { status: 400, message: "not hex".to_string() }.is_trouble());
    }
}
