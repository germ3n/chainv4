//! The gossip backlog: transactions and votes on their way from the network to the node loop, and
//! transactions the node's clients submitted, counted in all and per peer. The network counts each
//! one in as it arrives and the node counts it out once it handled it, so a flood is turned away
//! at the door instead of queueing without end.
//!
//! A peer that goes over its own share is struck; MAX_STRIKES strikes within STRIKE_WINDOW and it
//! is disconnected. Going over the whole backlog's limit strikes nobody, since no one peer filled
//! it.

use {
    libp2p::PeerId,
    r#macro::document,
    std::collections::HashMap,
    std::sync::{Mutex, MutexGuard},
    std::time::{Duration, Instant},
};

#[document(
    summary = "How many times a peer may go over its share of the backlog within STRIKE_WINDOW before it is disconnected.",
    category = "Node",
    see_also = "Backlog",
)]
pub const MAX_STRIKES: u32 = 10;

#[document(summary = "The window strikes against a peer are counted in.", category = "Node", see_also = "Backlog")]
pub const STRIKE_WINDOW: Duration = Duration::from_secs(60);

#[document(
    summary = "How much gossip may wait for the node: in all, and from any one peer. Set with --max-gossip-backlog and --max-gossip-per-peer.",
    category = "Node",
    see_also = "Backlog",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BacklogLimits {
    #[document(summary = "Transactions and votes waiting in all, submitted ones included.")]
    pub total: usize,
    #[document(summary = "Transactions and votes waiting from any one peer.")]
    pub per_peer: usize,
}

impl Default for BacklogLimits {
    fn default() -> BacklogLimits {
        BacklogLimits {
            total: 10_000,
            per_peer: 1_000,
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
    #[document(summary = "Dropped: the backlog is full, or this peer's share of it is.")]
    Dropped,
    #[document(summary = "Dropped, and the peer went over its share MAX_STRIKES times within STRIKE_WINDOW: disconnect it.")]
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
        let mut counts = self.lock();
        let Counts {
            waiting,
            peers,
            dropped,
            disconnected,
        } = &mut *counts;
        let full = *waiting >= self.limits.total;
        let share = match origin {
            Origin::Peer(peer) => Some(peers.entry(peer).or_default()),
            Origin::Local => None,
        };

        match share {
            Some(share) if share.waiting >= self.limits.per_peer => {
                share.dropped += 1;
                *dropped += 1;

                if share.since.is_none_or(|since| now.duration_since(since) > STRIKE_WINDOW) {
                    share.since = Some(now);
                    share.strikes = 0;
                }

                share.strikes += 1;

                if share.strikes < MAX_STRIKES {
                    return Admission::Dropped;
                }

                share.strikes = 0;
                share.since = None;
                *disconnected += 1;

                Admission::Disconnect
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
                }

                *waiting += 1;

                Admission::Admitted
            }
        }
    }

    #[document(parent = "Backlog", summary = "Counts something out, once the node handled it, or it went no further.")]
    pub fn release(&self, origin: Origin) {
        let mut counts = self.lock();
        counts.waiting = counts.waiting.saturating_sub(1);

        if let Origin::Peer(peer) = origin
            && let Some(share) = counts.peers.get_mut(&peer)
        {
            share.waiting = share.waiting.saturating_sub(1);
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
        Backlog::new(BacklogLimits { total, per_peer })
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
}
