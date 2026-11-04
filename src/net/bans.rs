//! Peers that send what breaks the rules, scored and, past BAN_SCORE, banned. Gossip waits until
//! the node has checked it before it is passed on, and the node's verdict comes back here: an
//! invalid block, a transaction or vote whose signature does not hold, bytes that do not decode,
//! or a flood past the backlog, each counts against the peer that sent it, by its weight. What an
//! honest peer sends too, a block whose parent this node lacks, one it has, a transaction it
//! cannot pay for yet, costs nothing.
//!
//! A ban disconnects the peer and refuses its connections until it ends: by its peer id, and with
//! --ban-ips by its IP too, which also catches a new peer id from the same machine but can catch
//! nodes behind the same NAT. Bans live in memory: a restart lifts them.

use {
    libp2p::PeerId,
    r#macro::document,
    std::collections::HashMap,
    std::net::IpAddr,
    std::time::{Duration, Instant},
};

#[document(
    summary = "How much a peer's faults may add up to within SCORE_WINDOW before it is banned, unless --ban-score says otherwise.",
    category = "Node",
    see_also = "Fault, Bans, BanLimits::score",
)]
pub const BAN_SCORE: u32 = 100;

#[document(summary = "The window a peer's faults are added up in, unless --ban-window says otherwise: a peer with none for this long starts again from nothing.", category = "Node", see_also = "Bans, BanLimits::window")]
pub const SCORE_WINDOW: Duration = Duration::from_secs(600);

#[document(summary = "How long a ban lasts unless --ban-for says otherwise.", category = "Node", see_also = "BanLimits")]
pub const DEFAULT_BAN: Duration = Duration::from_secs(3600);

#[document(
    summary = "How peers are banned: how much their faults add up to, within what window, for how long, and whether their IPs too. Set with --ban-score, --ban-window, --ban-for and --ban-ips.",
    category = "Node",
    see_also = "Bans",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BanLimits {
    #[document(summary = "How long a ban lasts.")]
    pub ban_for: Duration,
    #[document(summary = "Whether a ban covers the peer's IP as well as its peer id.")]
    pub ips: bool,
    #[document(summary = "How much a peer's faults add up to, within window, before it is banned: BAN_SCORE by default.")]
    pub score: u32,
    #[document(summary = "The window faults are added up in: SCORE_WINDOW by default.")]
    pub window: Duration,
}

impl Default for BanLimits {
    fn default() -> BanLimits {
        BanLimits { ban_for: DEFAULT_BAN, ips: false, score: BAN_SCORE, window: SCORE_WINDOW }
    }
}

#[document(summary = "Something a peer sent that breaks the rules, which counts against it.", category = "Node", see_also = "Bans, Verdict")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    #[document(summary = "A block that breaks the chain's rules: its work, seal, transactions or roots.")]
    Block,
    #[document(summary = "A transaction whose signature does not hold.")]
    Tx,
    #[document(summary = "A finality vote whose signature does not hold.")]
    Vote,
    #[document(summary = "Gossip that does not decode as what its topic carries.")]
    Garbage,
    #[document(summary = "More gossip than its share of the backlog, again and again: what the backlog disconnects for.")]
    Flood,
}

impl Fault {
    #[document(parent = "Fault", summary = "How much it counts: two invalid blocks, or ten bad signatures, within SCORE_WINDOW ban a peer.")]
    pub fn weight(self) -> u32 {
        match self {
            Fault::Block | Fault::Garbage => 50,
            Fault::Flood => 25,
            Fault::Tx | Fault::Vote => 10,
        }
    }
}

#[document(
    summary = "What the node made of a gossiped message, for the network: pass it on, drop it quietly, or drop it and count it against its sender.",
    category = "Node",
    see_also = "NetCommand::Judge, Fault",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    #[document(summary = "It holds: pass it on to the other peers.")]
    Accept,
    #[document(summary = "Not passed on, and nobody's fault: known already, early, or not for now.")]
    Ignore,
    #[document(summary = "It breaks the rules: not passed on, and the fault counts against the peer.")]
    Reject(Fault),
}

#[document(
    summary = "Peers' faults and bans: what the network thread checks connections against, and adds the node's verdicts to.",
    category = "Node",
    see_also = "BanLimits, Fault",
)]
#[derive(Debug)]
pub struct Bans {
    limits: BanLimits,
    scores: HashMap<PeerId, Score>,
    peers: HashMap<PeerId, Instant>,
    ips: HashMap<IpAddr, Instant>,
    banned: u64,
}

/// A peer's faults added up, since the first in the window.
#[derive(Debug)]
struct Score {
    points: u32,
    since: Instant,
}

impl Bans {
    #[document(parent = "Bans", summary = "No faults and no bans, with these limits.")]
    pub fn new(limits: BanLimits) -> Bans {
        Bans {
            limits,
            scores: HashMap::new(),
            peers: HashMap::new(),
            ips: HashMap::new(),
            banned: 0,
        }
    }

    #[document(
        parent = "Bans",
        summary = "Counts a fault against a peer, at the IP it connects from when known. True when that bans it: its score reached BanLimits::score within BanLimits::window.",
    )]
    pub fn fault(&mut self, peer: PeerId, ip: Option<IpAddr>, fault: Fault, now: Instant) -> bool {
        let score = self.scores.entry(peer).or_insert(Score { points: 0, since: now });

        if now.duration_since(score.since) > self.limits.window {
            *score = Score { points: 0, since: now };
        }

        score.points = score.points.saturating_add(fault.weight());

        if score.points < self.limits.score {
            return false;
        }

        self.scores.remove(&peer);
        self.ban(peer, ip, now);

        true
    }

    #[document(parent = "Bans", summary = "Bans a peer for BanLimits::ban_for, and its IP too with BanLimits::ips.")]
    pub fn ban(&mut self, peer: PeerId, ip: Option<IpAddr>, now: Instant) {
        let until = now + self.limits.ban_for;
        self.peers.insert(peer, until);
        self.banned += 1;

        if let (true, Some(ip)) = (self.limits.ips, ip) {
            self.ips.insert(ip, until);
        }
    }

    #[document(parent = "Bans", summary = "Whether a peer, or the IP it connects from, is banned now.")]
    pub fn banned(&self, peer: &PeerId, ip: Option<IpAddr>, now: Instant) -> bool {
        let live = |until: Option<&Instant>| until.is_some_and(|until| *until > now);

        live(self.peers.get(peer)) || ip.is_some_and(|ip| live(self.ips.get(&ip)))
    }

    #[document(parent = "Bans", summary = "Forgets the bans that ended and the scores whose window closed.")]
    pub fn sweep(&mut self, now: Instant) {
        self.peers.retain(|_, until| *until > now);
        self.ips.retain(|_, until| *until > now);
        let window = self.limits.window;
        self.scores.retain(|_, score| now.duration_since(score.since) <= window);
    }

    #[document(parent = "Bans", summary = "How many peer ids and IPs are banned now, and how many bans there were since the node started.")]
    pub fn counts(&self, now: Instant) -> (usize, usize, u64) {
        let live = |until: &Instant| *until > now;

        (self.peers.values().filter(|until| live(until)).count(), self.ips.values().filter(|until| live(until)).count(), self.banned)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn faults_add_up_to_a_ban_within_the_window_and_harmless_peers_are_left_alone() {
        let mut bans = Bans::new(BanLimits::default());
        let (peer, other) = (PeerId::random(), PeerId::random());
        let now = Instant::now();

        assert!(!bans.fault(peer, None, Fault::Block, now));
        assert!(!bans.banned(&peer, None, now));
        assert!(bans.fault(peer, None, Fault::Block, now + Duration::from_secs(1)), "two invalid blocks");
        assert!(bans.banned(&peer, None, now + Duration::from_secs(2)));
        assert!(!bans.banned(&other, None, now), "only the one that sent them");

        // Nine bad signatures are not enough; the tenth is.
        for n in 0..9 {
            assert!(!bans.fault(other, None, Fault::Tx, now + Duration::from_secs(n)));
        }
        assert!(bans.fault(other, None, Fault::Vote, now + Duration::from_secs(9)));

        // A ban ends, and is swept.
        let later = now + DEFAULT_BAN + Duration::from_secs(10);
        assert!(!bans.banned(&peer, None, later));
        bans.sweep(later);
        assert_eq!(bans.counts(later), (0, 0, 2));
    }

    #[test]
    fn faults_far_apart_never_add_up() {
        let mut bans = Bans::new(BanLimits::default());
        let peer = PeerId::random();
        let mut now = Instant::now();

        for _ in 0..10 {
            assert!(!bans.fault(peer, None, Fault::Block, now));
            now += SCORE_WINDOW + Duration::from_secs(1);
        }
    }

    #[test]
    fn a_ban_covers_the_ip_only_when_asked() {
        let ip: IpAddr = "203.0.113.7".parse().unwrap();
        let now = Instant::now();
        let (peer, fresh) = (PeerId::random(), PeerId::random());

        let mut by_id = Bans::new(BanLimits::default());
        by_id.ban(peer, Some(ip), now);
        assert!(by_id.banned(&peer, Some(ip), now));
        assert!(!by_id.banned(&fresh, Some(ip), now), "a new peer id from the same IP is let in");

        let mut by_ip = Bans::new(BanLimits { ips: true, ban_for: Duration::from_secs(60), ..BanLimits::default() });
        by_ip.ban(peer, Some(ip), now);
        assert!(by_ip.banned(&fresh, Some(ip), now), "and here it is not");
        assert!(!by_ip.banned(&fresh, Some(ip), now + Duration::from_secs(61)), "for --ban-for alone");
        assert_eq!(by_ip.counts(now), (1, 1, 1));
    }

    #[test]
    fn a_higher_ban_score_and_a_shorter_window_take_more_faults_to_ban() {
        let limits = BanLimits { score: 200, window: Duration::from_secs(60), ..BanLimits::default() };
        let mut bans = Bans::new(limits);
        let peer = PeerId::random();
        let now = Instant::now();

        // Two invalid blocks ban at the default score, but take four at 200.
        for n in 0..3 {
            assert!(!bans.fault(peer, None, Fault::Block, now + Duration::from_secs(n)));
        }
        assert!(bans.fault(peer, None, Fault::Block, now + Duration::from_secs(3)), "the fourth reaches 200");

        // Spread out past the minute, they never add up, though they would in the default window.
        let other = PeerId::random();
        for n in 0..8 {
            assert!(!bans.fault(other, None, Fault::Block, now + Duration::from_secs(61 * n)));
        }
    }
}
