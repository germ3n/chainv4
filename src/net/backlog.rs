//! The gossip backlog: blocks, transactions and votes on their way from the network to the node
//! loop, peers' sync requests until they are answered, and transactions the node's clients
//! submitted, counted in all, per peer and per IP. The network counts each one in as it arrives and
//! the node counts it out once it handled it, so a flood is turned away at the door instead of
//! queueing without end.
//!
//! A peer that goes over its own share, or whose IP holds per_ip already, is struck, and so is one
//! asking for more than its rate of sync requests; MAX_STRIKES strikes within STRIKE_WINDOW and it
//! is disconnected, or as many as --max-strikes and --strike-window say. Going over the whole
//! backlog's limit strikes nobody, since no one peer filled it.
//!
//! Many peer ids from one IP share its cap, so one host cannot take many shares. Loopback, which
//! every peer over Tor comes from, and private and link-local addresses, as a home's NAT or a
//! container network gives, are not shared: each peer there keeps its own share alone.

use {
    libp2p::PeerId,
    r#macro::document,
    std::collections::HashMap,
    std::net::IpAddr,
    std::sync::{Mutex, MutexGuard},
    std::time::{Duration, Instant},
};

#[document(
    summary = "How many times a peer may go over its share of the backlog within STRIKE_WINDOW before it is disconnected, unless --max-strikes says otherwise.",
    category = "Node",
    see_also = "Backlog, BacklogLimits::strikes",
)]
pub const MAX_STRIKES: u32 = 10;

#[document(summary = "The window strikes against a peer are counted in, unless --strike-window says otherwise.", category = "Node", see_also = "Backlog, BacklogLimits::strike_window")]
pub const STRIKE_WINDOW: Duration = Duration::from_secs(60);

#[document(
    summary = "How much gossip may wait for the node: in all, from any one peer and from any one IP; how fast a peer may ask for sync; and how often a peer may go over these before it is disconnected. Set with --max-gossip-backlog, --max-gossip-per-peer, --max-gossip-per-ip, --sync-rate, --sync-burst, --max-strikes and --strike-window.",
    category = "Node",
    see_also = "Backlog",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BacklogLimits {
    #[document(summary = "Blocks, transactions, votes and sync requests waiting in all, submitted transactions included.")]
    pub total: usize,
    #[document(summary = "What may wait from any one peer.")]
    pub per_peer: usize,
    #[document(summary = "What may wait from the peers of any one public IP together: a few peers' shares. Loopback, private and link-local addresses have no such cap, each peer there its share alone.")]
    pub per_ip: usize,
    #[document(summary = "Sync requests a second a peer may make, beyond its burst.")]
    pub requests: u32,
    #[document(summary = "Sync requests a peer may make at once, before its rate holds it back.")]
    pub request_burst: u32,
    #[document(summary = "How many times a peer may go over its share within strike_window before it is disconnected: MAX_STRIKES by default.")]
    pub strikes: u32,
    #[document(summary = "The window strikes are counted in: STRIKE_WINDOW by default.")]
    pub strike_window: Duration,
}

impl Default for BacklogLimits {
    fn default() -> BacklogLimits {
        BacklogLimits {
            total: 10_000,
            per_peer: 1_000,
            per_ip: 4_000,
            requests: 10,
            request_burst: 50,
            strikes: MAX_STRIKES,
            strike_window: STRIKE_WINDOW,
        }
    }
}

#[document(summary = "Where something in the backlog came from.", category = "Node", see_also = "Backlog")]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Origin {
    #[document(summary = "Gossip, from the peer that passed it on.")]
    Peer(PeerId),
    #[document(summary = "A client of this node, through a NodeHandle.")]
    Local,
}

#[document(summary = "What the backlog makes of something arriving.", category = "Node", see_also = "Backlog::admit")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Admission {
    #[document(summary = "It goes on to the node, and counts until the node handled it.")]
    Admitted,
    #[document(summary = "Dropped: the backlog is full, or this peer's share of it is, or its IP's, or it asked faster than its rate.")]
    Dropped,
    #[document(summary = "Dropped, and the peer went over its share BacklogLimits::strikes times within its strike_window: disconnect it.")]
    Disconnect,
}

#[document(
    summary = "Gossip and submissions on their way to the node, counted so a flood is turned away at the door. Shared by the network thread, which counts things in, the node, which counts them out, and node handles.",
    category = "Node",
    see_also = "BacklogLimits, NetLink",
)]
#[derive(Debug)]
pub struct Backlog {
    limits: BacklogLimits,
    counts: Mutex<Counts>,
}

#[derive(Debug, Default)]
struct Counts {
    waiting: usize,
    peers: HashMap<PeerId, Share>,
    /// What waits from each public IP's peers together.
    ips: HashMap<IpAddr, usize>,
    dropped: u64,
    disconnected: u64,
}

/// One peer's part of the backlog.
#[derive(Debug, Default)]
struct Share {
    waiting: usize,
    dropped: u64,
    strikes: u32,
    /// When the strikes being counted began.
    since: Option<Instant>,
    /// The public IP it connects from, whose cap it shares: None for one that has no such cap.
    ip: Option<IpAddr>,
    /// Its sync requests' bucket: how many it may make now, and when that was worked out. None
    /// for a full one.
    tokens: Option<(f64, Instant)>,
}

/// Whether peers at this IP share its cap: a public address. Loopback, which every peer over Tor
/// comes from, and private, link-local, shared (100.64/10) and unspecified addresses do not, as
/// many unrelated peers may come from one of them.
fn shared(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => !(ip.is_loopback() || ip.is_private() || ip.is_link_local() || ip.is_unspecified() || (ip.octets()[0] == 100 && ip.octets()[1] & 0xc0 == 64)),
        IpAddr::V6(ip) => match ip.to_ipv4_mapped() {
            Some(v4) => shared(IpAddr::V4(v4)),
            None => !(ip.is_loopback() || ip.is_unspecified() || ip.is_unique_local() || ip.is_unicast_link_local()),
        },
    }
}

impl Share {
    /// One more strike, within the window or starting it again: true once it reached strikes.
    fn strike(&mut self, limits: &BacklogLimits, now: Instant) -> bool {
        self.dropped += 1;

        if self.since.is_none_or(|since| now.duration_since(since) > limits.strike_window) {
            self.since = Some(now);
            self.strikes = 0;
        }

        self.strikes += 1;

        if self.strikes < limits.strikes {
            return false;
        }

        self.strikes = 0;
        self.since = None;

        true
    }

    /// Takes one sync request from its bucket, refilled at limits.requests a second up to
    /// request_burst: false when it is empty.
    fn take_token(&mut self, limits: &BacklogLimits, now: Instant) -> bool {
        let burst = f64::from(limits.request_burst.max(1));
        let (tokens, at) = self.tokens.unwrap_or((burst, now));
        let tokens = (tokens + now.saturating_duration_since(at).as_secs_f64() * f64::from(limits.requests)).min(burst);

        match tokens >= 1.0 {
            true => {
                self.tokens = Some((tokens - 1.0, now));

                true
            }
            false => {
                self.tokens = Some((tokens, now));

                false
            }
        }
    }
}

impl Backlog {
    #[document(parent = "Backlog", summary = "An empty backlog with these limits.")]
    pub fn new(limits: BacklogLimits) -> Backlog {
        Backlog {
            limits,
            counts: Mutex::default(),
        }
    }

    #[document(parent = "Backlog", summary = "Its limits.")]
    pub fn limits(&self) -> BacklogLimits {
        self.limits
    }

    #[document(
        parent = "Backlog",
        summary = "Counts something in, unless there is no room for it: none left in all, or none in its peer's share, which strikes the peer.",
        params = {
            origin = "Where it came from.",
            now = "The time, to count strikes in.",
        },
    )]
    pub fn admit(&self, origin: Origin, now: Instant) -> Admission {
        self.admit_from(origin, None, now)
    }

    #[document(
        parent = "Backlog",
        summary = "Counts something in from a peer at an IP, unless there is no room for it: none left in all, in its peer's share, or in its IP's, the last two of which strike the peer.",
        params = {
            origin = "Where it came from.",
            ip = "The IP a peer connects from, when known: a public one shares its cap with the other peers there.",
            now = "The time, to count strikes in.",
        },
    )]
    pub fn admit_from(&self, origin: Origin, ip: Option<IpAddr>, now: Instant) -> Admission {
        let mut counts = self.lock();
        let Counts {
            waiting,
            peers,
            ips,
            dropped,
            disconnected,
        } = &mut *counts;
        let full = *waiting >= self.limits.total;
        let share = match origin {
            Origin::Peer(peer) => {
                let share = peers.entry(peer).or_default();

                if let Some(ip) = ip.filter(|ip| shared(*ip))
                    && share.waiting == 0
                {
                    share.ip = Some(ip);
                }

                Some(share)
            }
            Origin::Local => None,
        };

        match share {
            Some(share) if share.waiting >= self.limits.per_peer || share.ip.is_some_and(|ip| ips.get(&ip).is_some_and(|held| *held >= self.limits.per_ip)) => {
                *dropped += 1;

                match share.strike(&self.limits, now) {
                    true => {
                        *disconnected += 1;

                        Admission::Disconnect
                    }
                    false => Admission::Dropped,
                }
            }
            Some(share) if full => {
                share.dropped += 1;
                *dropped += 1;

                Admission::Dropped
            }
            None if full => {
                *dropped += 1;

                Admission::Dropped
            }
            share => {
                if let Some(share) = share {
                    share.waiting += 1;

                    if let Some(ip) = share.ip {
                        *ips.entry(ip).or_default() += 1;
                    }
                }

                *waiting += 1;

                Admission::Admitted
            }
        }
    }

    #[document(
        parent = "Backlog",
        summary = "Counts a peer's sync request in, as admit_from does, once its rate allows it: past BacklogLimits::requests a second beyond its request_burst it is dropped, and the peer struck.",
        params = {
            peer = "Who asked.",
            ip = "The IP it connects from, when known.",
            now = "The time, to refill its rate and count strikes in.",
        },
    )]
    pub fn admit_request(&self, peer: PeerId, ip: Option<IpAddr>, now: Instant) -> Admission {
        {
            let mut counts = self.lock();
            let share = counts.peers.entry(peer).or_default();

            if !share.take_token(&self.limits, now) {
                let struck = share.strike(&self.limits, now);
                counts.dropped += 1;

                return match struck {
                    true => {
                        counts.disconnected += 1;

                        Admission::Disconnect
                    }
                    false => Admission::Dropped,
                };
            }
        }

        self.admit_from(Origin::Peer(peer), ip, now)
    }

    #[document(parent = "Backlog", summary = "Counts something out, once the node handled it, or it went no further.")]
    pub fn release(&self, origin: Origin) {
        let mut counts = self.lock();
        counts.waiting = counts.waiting.saturating_sub(1);

        if let Origin::Peer(peer) = origin
            && let Some(share) = counts.peers.get_mut(&peer)
            && share.waiting > 0
        {
            share.waiting -= 1;

            if let Some(ip) = share.ip
                && let Some(held) = counts.ips.get_mut(&ip)
            {
                *held = held.saturating_sub(1);

                if *held == 0 {
                    counts.ips.remove(&ip);
                }
            }
        }
    }

    #[document(
        parent = "Backlog",
        summary = "Forgets a peer that left, once nothing of its is waiting. What it had dropped still counts in all.",
    )]
    pub fn forget(&self, peer: &PeerId) {
        let mut counts = self.lock();

        if counts.peers.get(peer).is_some_and(|share| share.waiting == 0) {
            counts.peers.remove(peer);
        }
    }

    #[document(parent = "Backlog", summary = "How much is waiting now.")]
    pub fn waiting(&self) -> usize {
        self.lock().waiting
    }

    #[document(parent = "Backlog", summary = "How much is waiting now from the peers of one IP, as its cap counts it: 0 for an IP without one.")]
    pub fn waiting_from_ip(&self, ip: &IpAddr) -> usize {
        self.lock().ips.get(ip).copied().unwrap_or(0)
    }

    #[document(parent = "Backlog", summary = "How much it dropped since the node started: transactions, votes and submissions.")]
    pub fn dropped(&self) -> u64 {
        self.lock().dropped
    }

    #[document(parent = "Backlog", summary = "How much it dropped from one peer while it was connected.")]
    pub fn dropped_from(&self, peer: &PeerId) -> u64 {
        self.lock().peers.get(peer).map_or(0, |share| share.dropped)
    }

    #[document(parent = "Backlog", summary = "How many peers it had disconnected for flooding.")]
    pub fn disconnected(&self) -> u64 {
        self.lock().disconnected
    }

    fn lock(&self) -> MutexGuard<'_, Counts> {
        self.counts.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn backlog(total: usize, per_peer: usize) -> Backlog {
        Backlog::new(BacklogLimits { total, per_peer, ..BacklogLimits::default() })
    }

    #[test]
    fn a_peer_gets_its_share_and_no_more_until_the_node_catches_up() {
        let backlog = backlog(10, 2);
        let (flooder, other) = (PeerId::random(), PeerId::random());
        let now = Instant::now();

        assert_eq!(backlog.admit(Origin::Peer(flooder), now), Admission::Admitted);
        assert_eq!(backlog.admit(Origin::Peer(flooder), now), Admission::Admitted);
        assert_eq!(backlog.admit(Origin::Peer(flooder), now), Admission::Dropped);
        assert_eq!(backlog.admit(Origin::Peer(other), now), Admission::Admitted, "the others' shares are theirs");

        backlog.release(Origin::Peer(flooder));

        assert_eq!(backlog.admit(Origin::Peer(flooder), now), Admission::Admitted);
        assert_eq!((backlog.waiting(), backlog.dropped(), backlog.dropped_from(&flooder)), (3, 1, 1));
    }

    #[test]
    fn a_full_backlog_turns_everyone_away_but_strikes_nobody() {
        let backlog = backlog(2, 5);
        let peer = PeerId::random();
        let now = Instant::now();

        assert_eq!(backlog.admit(Origin::Local, now), Admission::Admitted);
        assert_eq!(backlog.admit(Origin::Peer(peer), now), Admission::Admitted);

        for _ in 0..MAX_STRIKES * 2 {
            assert_eq!(backlog.admit(Origin::Peer(peer), now), Admission::Dropped);
        }

        assert_eq!(backlog.admit(Origin::Local, now), Admission::Dropped, "clients wait too");
        assert_eq!(backlog.disconnected(), 0);

        backlog.release(Origin::Local);

        assert_eq!(backlog.admit(Origin::Local, now), Admission::Admitted);
    }

    #[test]
    fn a_peer_that_keeps_flooding_is_disconnected() {
        let backlog = backlog(100, 1);
        let peer = PeerId::random();
        let start = Instant::now();
        backlog.admit(Origin::Peer(peer), start);

        // Strikes far apart never add up.
        for minute in 1..=MAX_STRIKES {
            let later = start + STRIKE_WINDOW * minute + Duration::from_secs(1) * minute;

            assert_eq!(backlog.admit(Origin::Peer(peer), later), Admission::Dropped);
        }

        // Within a window they do.
        let now = start + STRIKE_WINDOW * 20;

        for _ in 1..MAX_STRIKES {
            assert_eq!(backlog.admit(Origin::Peer(peer), now), Admission::Dropped);
        }

        assert_eq!(backlog.admit(Origin::Peer(peer), now), Admission::Disconnect);
        assert_eq!(backlog.disconnected(), 1);

        // Gone, and forgotten once nothing of its waits.
        backlog.release(Origin::Peer(peer));
        backlog.forget(&peer);

        assert_eq!((backlog.waiting(), backlog.dropped_from(&peer)), (0, 0));
    }

    #[test]
    fn more_strikes_in_a_shorter_window_take_longer_to_disconnect_a_peer() {
        let backlog = Backlog::new(BacklogLimits { total: 100, per_peer: 1, strikes: 3, strike_window: Duration::from_secs(5), ..BacklogLimits::default() });
        let peer = PeerId::random();
        let start = Instant::now();
        backlog.admit(Origin::Peer(peer), start);

        // Ten seconds apart, past the 5-second window, strikes never add up, though within the
        // default minute ten would disconnect the peer.
        for n in 1..=12u32 {
            assert_eq!(backlog.admit(Origin::Peer(peer), start + Duration::from_secs(u64::from(n) * 10)), Admission::Dropped);
        }

        // Three within the window do, where the default takes ten.
        let now = start + Duration::from_secs(1_000);
        assert_eq!(backlog.admit(Origin::Peer(peer), now), Admission::Dropped);
        assert_eq!(backlog.admit(Origin::Peer(peer), now), Admission::Dropped);
        assert_eq!(backlog.admit(Origin::Peer(peer), now), Admission::Disconnect);
    }

    #[test]
    fn peers_of_one_public_ip_share_its_cap_and_tor_or_a_home_network_do_not() {
        let backlog = Backlog::new(BacklogLimits { total: 100, per_peer: 3, per_ip: 4, ..BacklogLimits::default() });
        let public: IpAddr = "203.0.113.50".parse().unwrap();
        let now = Instant::now();
        let ids: Vec<PeerId> = (0..3).map(|_| PeerId::random()).collect();

        // Three peer ids at one public IP: four between them, then each is turned away and struck.
        assert_eq!(backlog.admit_from(Origin::Peer(ids[0]), Some(public), now), Admission::Admitted);
        assert_eq!(backlog.admit_from(Origin::Peer(ids[0]), Some(public), now), Admission::Admitted);
        assert_eq!(backlog.admit_from(Origin::Peer(ids[1]), Some(public), now), Admission::Admitted);
        assert_eq!(backlog.admit_from(Origin::Peer(ids[2]), Some(public), now), Admission::Admitted);
        assert_eq!(backlog.admit_from(Origin::Peer(ids[2]), Some(public), now), Admission::Dropped, "the IP holds four");
        assert_eq!(backlog.waiting_from_ip(&public), 4);

        // One handled, there is room for one more from any of them.
        backlog.release(Origin::Peer(ids[0]));
        assert_eq!(backlog.waiting_from_ip(&public), 3);
        assert_eq!(backlog.admit_from(Origin::Peer(ids[1]), Some(public), now), Admission::Admitted);

        // Over Tor every peer comes from loopback, and at home from a private address: no cap
        // shared, each its own share alone.
        for ip in ["127.0.0.1", "::1", "192.168.1.20", "10.88.0.4", "100.64.1.1", "fd00::5", "::ffff:127.0.0.1"] {
            let ip: IpAddr = ip.parse().unwrap();
            let peers: Vec<PeerId> = (0..3).map(|_| PeerId::random()).collect();

            for peer in &peers {
                for _ in 0..3 {
                    assert_eq!(backlog.admit_from(Origin::Peer(*peer), Some(ip), now), Admission::Admitted, "{ip}");
                }
            }

            assert_eq!(backlog.waiting_from_ip(&ip), 0, "{ip} has no cap");
        }
    }

    #[test]
    fn a_peer_crowding_its_ip_is_struck_until_it_is_disconnected() {
        let backlog = Backlog::new(BacklogLimits { total: 1_000, per_peer: 100, per_ip: 2, strikes: 3, ..BacklogLimits::default() });
        let ip: IpAddr = "198.51.100.9".parse().unwrap();
        let (one, two) = (PeerId::random(), PeerId::random());
        let now = Instant::now();
        backlog.admit_from(Origin::Peer(one), Some(ip), now);
        backlog.admit_from(Origin::Peer(one), Some(ip), now);

        assert_eq!(backlog.admit_from(Origin::Peer(two), Some(ip), now), Admission::Dropped);
        assert_eq!(backlog.admit_from(Origin::Peer(two), Some(ip), now), Admission::Dropped);
        assert_eq!(backlog.admit_from(Origin::Peer(two), Some(ip), now), Admission::Disconnect);
    }

    #[test]
    fn sync_requests_come_at_their_rate_past_a_burst_and_count_until_answered() {
        let backlog = Backlog::new(BacklogLimits { total: 1_000, per_peer: 1_000, requests: 2, request_burst: 5, strikes: 4, ..BacklogLimits::default() });
        let peer = PeerId::random();
        let start = Instant::now();

        // A burst of five at once, then nothing until the rate refills.
        for _ in 0..5 {
            assert_eq!(backlog.admit_request(peer, None, start), Admission::Admitted);
        }
        assert_eq!(backlog.admit_request(peer, None, start), Admission::Dropped);
        assert_eq!(backlog.waiting(), 5, "each waits until the node answered it");

        // Two a second: half a second gives one more.
        assert_eq!(backlog.admit_request(peer, None, start + Duration::from_millis(500)), Admission::Admitted);
        assert_eq!(backlog.admit_request(peer, None, start + Duration::from_millis(500)), Admission::Dropped);

        // Kept up, the strikes add up to a disconnect.
        assert_eq!(backlog.admit_request(peer, None, start + Duration::from_millis(500)), Admission::Dropped);
        assert_eq!(backlog.admit_request(peer, None, start + Duration::from_millis(500)), Admission::Disconnect);

        // A peer that asks at its rate is never held back.
        let steady = PeerId::random();
        for n in 0..100u64 {
            assert_eq!(backlog.admit_request(steady, None, start + Duration::from_millis(500 * n)), Admission::Admitted, "{n}");
            backlog.release(Origin::Peer(steady));
        }
    }
}
