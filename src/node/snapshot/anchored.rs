//! Anchored sync: before a new node trusts any history, it hears from its peers what their chains
//! anchored in Bitcoin, or Ethereum, and Bitcoin's headers from their relays, and starts at the
//! newest checkpoint of the history anchored first (crate::anchor::sync), as --checkpoint would
//! start at one the operator trusts. A long-range fork from old validator keys loses here, however
//! its votes look, as its anchors can sit in Bitcoin no earlier than it was made.

use {
    super::{Asked, Phase},
    crate::anchor::sync::{AnchorVerdict, History, Sources, check_anchors, compare_histories},
    crate::net::{NetCommand, SyncRequest},
    crate::node::{Node, NodeError},
    crate::params::anchoring::AnchorsAnswer,
    crate::structs::u256::u256,
    libp2p::PeerId,
    std::collections::{BTreeSet, HashMap},
};

#[cfg(feature = "anchoring-btc")]
use {crate::anchor::bitcoin::HeaderChain, crate::net::MAX_SYNC_BTC_HEADERS, crate::params::anchoring::BtcStart};
#[cfg(feature = "anchoring-eth")]
use {
    r#macro::document,
    crate::anchor::ethereum::{EthBeacon, EthCommittees},
    crate::params::anchoring::EthStart,
    std::sync::{Arc, mpsc},
};

/// Ethereum's sync committees, followed from each eth_start peers name through the node's beacon
/// APIs, on a thread of their own, as following a year of periods takes a while.
#[cfg(feature = "anchoring-eth")]
#[derive(Default)]
pub(in crate::node) struct EthFollowing {
    /// The node's --beacon, if given: without it Ethereum anchors prove nothing.
    pub(in crate::node) beacon: Option<Arc<dyn EthBeacon>>,
    /// Committees followed, one for each start.
    committees: Vec<EthCommittees>,
    /// Starts peers named, and whether following each was begun.
    starts: Vec<(EthStart, bool)>,
    /// What is being followed now.
    waiting: Option<mpsc::Receiver<Result<EthCommittees, String>>>,
    /// Said once that, without --beacon, Ethereum anchors decide nothing.
    said: bool,
}

/// What one peer showed.
#[derive(Default)]
struct Shown {
    /// Its anchors, once it sent them; None while it holds none.
    answer: Option<AnchorsAnswer>,
    /// Whether it answered at all.
    answered: bool,
    /// The next Bitcoin height to ask its relay for, and whether its headers are all in: at once
    /// for a list with no Bitcoin start, or a build that reads no Bitcoin.
    #[cfg_attr(not(feature = "anchoring-btc"), allow(dead_code))]
    next: u64,
    done: bool,
    /// Places whose proofs to ask it for.
    wanted: BTreeSet<u64>,
}

/// Where picking the checkpoint stands.
#[derive(Default)]
pub(in crate::node) struct Anchored {
    shown: HashMap<PeerId, Shown>,
    /// Bitcoin's headers, one chain for each start a peer named.
    #[cfg(feature = "anchoring-btc")]
    chains: Vec<(BtcStart, HeaderChain)>,
    /// Ethereum's committees, as far as they were followed.
    #[cfg(feature = "anchoring-eth")]
    pub(in crate::node) eth: EthFollowing,
    /// Two histories anchored in the same Bitcoin block: nothing to pick, said once.
    tied: bool,
    /// No peer shows anchors yet: said once.
    waiting: bool,
}

impl Anchored {
    /// The headers to check anchors against: Bitcoin's with the most work any peer showed.
    fn sources(&self) -> Sources<'_> {
        Sources {
            #[cfg(feature = "anchoring-btc")]
            bitcoin: self.chains.iter().map(|(_, chain)| chain).max_by_key(|chain| chain.tip().2),
            #[cfg(feature = "anchoring-eth")]
            ethereum: &self.eth.committees,
            ..Sources::default()
        }
    }
}

impl Node {
    /// Asks a peer for what picking the checkpoint needs next, or picks it once every peer heard
    /// from has shown all it has.
    pub(super) fn anchored_next(&mut self) -> Result<(), NodeError> {
        let least = self.tuning.anchored_peers;
        let snapshot = self.sync.snapshot.as_mut().expect("taking one");
        let anchored = snapshot.anchored.as_mut().expect("picking by anchors");
        let peers: Vec<PeerId> = self.sync.peers.iter().filter(|(id, chain)| !snapshot.distrusted.contains(id) && chain.height > 0).map(|(id, _)| *id).collect();
        let mut ask = None;

        for peer in &peers {
            let shown = anchored.shown.entry(*peer).or_default();

            ask = match &shown.answer {
                _ if !shown.answered => Some((*peer, Asked::Anchors, SyncRequest::Anchors { proofs: Vec::new() })),
                Some(_) if !shown.wanted.is_empty() => Some((*peer, Asked::Anchors, SyncRequest::Anchors { proofs: shown.wanted.iter().copied().collect() })),
                #[cfg(feature = "anchoring-btc")]
                Some(AnchorsAnswer { start: Some(start), .. }) if !shown.done => {
                    let from = shown.next.max(start.height + 1);

                    Some((*peer, Asked::BtcHeaders { from }, SyncRequest::BtcHeaders { from, count: MAX_SYNC_BTC_HEADERS }))
                }
                _ => None,
            };

            if ask.is_some() {
                break;
            }
        }

        if let Some((peer, asked, request)) = ask {
            snapshot.asking.insert(peer, asked);
            self.send(NetCommand::Request { peer, request });

            return Ok(());
        }

        let heard = peers.iter().filter(|peer| anchored.shown.get(peer).is_some_and(|shown| shown.answered)).count();

        if heard < least {
            return Ok(());
        }

        #[cfg(feature = "anchoring-eth")]
        if !anchored.eth.following(crate::node::now_ms() / 1_000) {
            return Ok(());
        }

        self.pick_by_anchors(&peers)
    }

    /// Takes a peer's anchor list, or the proofs it was asked for: proofs join those it sent before
    /// while its list is the same.
    pub(super) fn take_anchors(&mut self, peer: PeerId, answer: Option<AnchorsAnswer>) {
        let anchored = self.sync.snapshot.as_mut().and_then(|snapshot| snapshot.anchored.as_mut()).expect("picking by anchors");
        let shown = anchored.shown.entry(peer).or_default();
        shown.answered = true;
        shown.wanted.clear();

        let Some(mut answer) = answer else {
            log::info!("[node] {peer} shows no anchors");
            shown.answer = None;
            shown.done = true;

            return;
        };

        match shown.answer.take() {
            Some(before) if before.entries == answer.entries => {
                for proof in before.proofs {
                    if !answer.proofs.iter().any(|(index, _)| *index == proof.0) {
                        answer.proofs.push(proof);
                    }
                }
            }
            _ => {
                shown.next = answer.start.as_ref().map_or(0, |start| start.height + 1);
                shown.done = cfg!(not(feature = "anchoring-btc")) || answer.start.is_none();
            }
        }

        #[cfg(feature = "anchoring-eth")]
        if let Some(start) = &answer.eth_start
            && !anchored.eth.starts.iter().any(|(known, _)| known == start)
        {
            anchored.eth.starts.push((start.clone(), false));
        }

        #[cfg(feature = "anchoring-btc")]
        let start = answer.start.clone();
        shown.answer = Some(answer);

        #[cfg(feature = "anchoring-btc")]
        if let Some(start) = start
            && !anchored.chains.iter().any(|(known, _)| *known == start)
        {
            match HeaderChain::new(&start) {
                Ok(chain) => anchored.chains.push((start, chain)),
                Err(err) => {
                    log::warn!("[node] {peer} names a Bitcoin start that is no header: {err}; asking others");
                    self.sync.snapshot.as_mut().expect("taking one").distrusted.insert(peer);
                }
            }
        }
    }

    /// Takes Bitcoin headers from a peer's relay into the chain for its start.
    #[cfg(feature = "anchoring-btc")]
    pub(super) fn take_btc_headers(&mut self, peer: PeerId, from: u64, headers: Vec<Vec<u8>>, now: u64) {
        let snapshot = self.sync.snapshot.as_mut().expect("taking one");
        let anchored = snapshot.anchored.as_mut().expect("picking by anchors");
        let Some(shown) = anchored.shown.get_mut(&peer) else {
            return;
        };
        let Some(answer) = &shown.answer else {
            return;
        };
        let Some((_, chain)) = anchored.chains.iter_mut().find(|(start, _)| Some(start) == answer.start.as_ref()) else {
            return;
        };

        if headers.is_empty() {
            shown.done = true;

            return;
        }

        for header in &headers {
            if let Err(err) = chain.add(header, now / 1_000) {
                log::warn!("[node] {peer}'s relay holds a Bitcoin header that does not check: {err}; asking others");
                snapshot.distrusted.insert(peer);

                return;
            }
        }

        shown.next = from + headers.len() as u64;
        shown.done = shown.next > answer.btc_tip;
    }

    /// Picks the checkpoint: checks every list against the Bitcoin chain with the most work, and
    /// keeps the history anchored first, asking for proofs where two part that it lacks.
    fn pick_by_anchors(&mut self, peers: &[PeerId]) -> Result<(), NodeError> {
        let (genesis, depth) = (self.chain.genesis(), self.tuning.anchored_depth);
        let snapshot = self.sync.snapshot.as_mut().expect("taking one");
        let anchored = snapshot.anchored.as_mut().expect("picking by anchors");
        let shows = peers.iter().any(|peer| anchored.shown.get(peer).is_some_and(|shown| shown.answer.is_some()));

        if !shows {
            if !anchored.waiting {
                log::warn!("[node] no peer shows anchors yet; waiting for one that does. Without anchors, start with --checkpoint instead");
                anchored.waiting = true;
            }

            return Ok(());
        };
        let sources = anchored.sources();
        let mut histories: Vec<(PeerId, History)> = Vec::new();
        let mut distrusted = Vec::new();

        let mut wanted = Vec::new();

        for peer in peers {
            let Some(answer) = anchored.shown.get(peer).and_then(|shown| shown.answer.as_ref()) else {
                continue;
            };

            // Anchors in a chain the node cannot check decide nothing: the list is checked up to
            // its newest in one it can, whose proof is asked for when the peer sent none.
            let answer = match crate::anchor::sync::newest_readable(answer, &sources) {
                None => {
                    log::warn!("[node] {peer} shows anchors only in chains this node cannot check: Ethereum's take --beacon");

                    continue;
                }
                Some(place) if place + 1 == answer.entries.len() as u64 => std::borrow::Cow::Borrowed(answer),
                Some(place) if !answer.proofs.iter().any(|(index, _)| *index == place) => {
                    wanted.push((*peer, place));

                    continue;
                }
                Some(place) => std::borrow::Cow::Owned(crate::anchor::sync::cut_at(answer, place)),
            };

            match check_anchors(&answer, &sources, &genesis, depth) {
                Ok(history) => histories.push((*peer, history)),
                Err(err) => {
                    log::warn!("[node] {peer}'s anchors do not check against Bitcoin's headers: {err}; asking others");
                    distrusted.push(*peer);
                }
            }
        }

        snapshot.distrusted.extend(distrusted);

        if !wanted.is_empty() {
            for (peer, place) in wanted {
                if let Some(shown) = anchored.shown.get_mut(&peer) {
                    shown.wanted.insert(place);
                }
            }

            return self.anchored_next();
        }

        let mut winner: Option<(PeerId, History)> = None;

        for (peer, history) in histories {
            let Some((first, held)) = &winner else {
                winner = Some((peer, history));
                continue;
            };

            let verdict = compare_histories(held, &history);

            match verdict {
                AnchorVerdict::Extends(false) | AnchorVerdict::Earlier(false) => {}
                AnchorVerdict::Extends(true) | AnchorVerdict::Earlier(true) => winner = Some((peer, history)),
                AnchorVerdict::NeedProofs(theirs, its) => {
                    let first = *first;

                    for (asked, at) in [(first, theirs), (peer, its)] {
                        if let Some(shown) = anchored.shown.get_mut(&asked) {
                            shown.wanted.insert(at);
                        }
                    }

                    return self.anchored_next();
                }
                AnchorVerdict::Tie(at) | AnchorVerdict::Undecided(at) => {
                    if !anchored.tied {
                        match matches!(verdict, AnchorVerdict::Tie(_)) {
                            true => log::error!("[node] {first} and {peer} show histories anchored in the same block, after they part at anchor {at}: no rule picks one. Start with --checkpoint from someone you trust"),
                            false => log::error!("[node] {first} and {peer} show histories that part at anchor {at}, after which no chain holds an anchor of both: no rule picks one. Start with --checkpoint from someone you trust"),
                        }
                        anchored.tied = true;
                    }

                    return Ok(());
                }
            }
        }

        let Some((peer, history)) = winner else {
            return Ok(());
        };
        let newest = history.newest();
        log::info!("[node] starting at checkpoint {} of epoch {}, anchored in {} by {peer}'s chain", newest.block, newest.epoch, newest.source.describe());
        snapshot.checkpoint = Some((newest.block, u256::ZERO));
        snapshot.phase = Phase::Checkpoint;
        snapshot.anchored = None;

        self.snapshot_next()
    }
}

#[cfg(feature = "anchoring-eth")]
impl EthFollowing {
    /// Carries following on: takes what a thread followed, and starts the next start on one. Whether
    /// it is all done, so picking may go on.
    fn following(&mut self, now: u64) -> bool {
        if let Some(waiting) = &self.waiting {
            match waiting.try_recv() {
                Err(mpsc::TryRecvError::Empty) => return false,
                Ok(Ok(committees)) => {
                    log::info!("[node] followed Ethereum's sync committees to period {}", committees.last_period());
                    self.committees.push(committees);
                }
                Ok(Err(why)) => log::warn!("[node] following Ethereum's sync committees through --beacon: {why}; its anchors decide nothing"),
                Err(mpsc::TryRecvError::Disconnected) => log::warn!("[node] following Ethereum's sync committees stopped; its anchors decide nothing"),
            }

            self.waiting = None;
        }

        let Some((start, begun)) = self.starts.iter_mut().find(|(_, begun)| !*begun) else {
            return true;
        };

        let Some(beacon) = self.beacon.clone() else {
            if !self.said {
                log::warn!("[node] peers' chains anchor in Ethereum, and no --beacon is given: Ethereum anchors decide nothing");
                self.said = true;
            }

            return true;
        };

        *begun = true;
        let (start, (send, receive)) = (start.clone(), mpsc::channel());
        log::info!("[node] following Ethereum's sync committees from {} through --beacon", crate::structs::hex::to_hex(&start.root));
        std::thread::spawn(move || {
            let _ = send.send(EthCommittees::fetch(beacon.as_ref(), &start, now));
        });
        self.waiting = Some(receive);

        false
    }
}

impl Node {
    #[cfg(feature = "anchoring-eth")]
    #[document(
        parent = "Node",
        summary = "Gives anchored sync a beacon API to follow Ethereum's sync committees through, from each eth_start peers name, so it can check Ethereum anchors' signatures itself: what --beacon gives. Without one, Ethereum anchors decide nothing. Call it after sync_from_anchors; it does nothing otherwise.",
    )]
    pub fn anchor_beacon(&mut self, beacon: Arc<dyn EthBeacon>) {
        if let Some(anchored) = self.sync.snapshot.as_mut().and_then(|snapshot| snapshot.anchored.as_mut()) {
            anchored.eth.beacon = Some(beacon);
        }
    }
}
