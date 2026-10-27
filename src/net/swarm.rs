use {
    super::codec::SyncCodec,
    super::{Admission, Backlog, Bans, Fault, Gossip, NetCommand, NetConfig, NetEvent, NetLink, Origin, SyncResponse, Verdict},
    crate::crypto::hasher::Hasher,
    crate::structs::block::Block,
    crate::structs::tx::SignedTx,
    crate::structs::vote::SignedVote,
    libp2p::futures::StreamExt,
    libp2p::swarm::behaviour::toggle::Toggle,
    libp2p::swarm::dial_opts::DialOpts,
    libp2p::swarm::{ConnectionId, NetworkBehaviour, Swarm, SwarmEvent},
    libp2p::{Multiaddr, PeerId, StreamProtocol, SwarmBuilder, gossipsub, identify, kad, mdns, request_response},
    r#macro::document,
    std::collections::HashMap,
    std::fmt,
    std::net::IpAddr,
    std::sync::{Arc, mpsc},
    std::time::{Duration, Instant},
    tokio::sync::mpsc as async_mpsc,
};

/// Whether an address is on loopback: 127.0.0.0/8 or ::1.
fn loopback(address: &Multiaddr) -> bool {
    use libp2p::multiaddr::Protocol;

    match address.iter().next() {
        Some(Protocol::Ip4(ip)) => ip.is_loopback(),
        Some(Protocol::Ip6(ip)) => ip.is_loopback(),
        _ => false,
    }
}

/// The names a node's protocols and topics go by: its protocol name and network in each, so nodes
/// of two chains share no DHT, no sync and no gossip, and tell each other apart on identify.
struct Names {
    sync: StreamProtocol,
    kad: StreamProtocol,
    identify: String,
    topic: String,
}

impl Names {
    fn new(protocol: &str, network: &str) -> Result<Names, NetError> {
        let stream = |name: String| StreamProtocol::try_from_owned(name).map_err(|err| NetError(format!("a protocol name that does not fit: {err}")));
        let base = format!("/{protocol}/{network}");

        Ok(Names {
            sync: stream(format!("{base}/sync/1"))?,
            kad: stream(format!("{base}/kad/1"))?,
            identify: format!("{base}/1"),
            topic: base,
        })
    }
}
/// The most transactions one gossiped batch holds.
pub const TX_BATCH: usize = 256;
/// The longest a transaction waits for others to share its batch.
pub const TX_BATCH_WAIT: Duration = Duration::from_millis(50);

/// How long a transaction is remembered as sent or heard of: past it, one heard again is news.
const SEEN_FOR: Duration = Duration::from_secs(120);

/// Transactions sent and heard of lately, by id, in two generations of SEEN_FOR each, so
/// remembering costs no sweep: the older generation goes whole as the newer one ages.
#[derive(Default)]
struct Seen {
    new: HashMap<crate::structs::hash::Hash, bool>,
    old: HashMap<crate::structs::hash::Hash, bool>,
    since: Option<Instant>,
}

impl Seen {
    fn age(&mut self) {
        let now = Instant::now();

        if self.since.is_none_or(|since| now.duration_since(since) > SEEN_FOR) {
            self.old = std::mem::take(&mut self.new);
            self.since = Some(now);
        }
    }

    /// Notes a transaction heard of; false when it was heard of or sent already.
    fn insert(&mut self, id: crate::structs::hash::Hash) -> bool {
        self.age();

        match self.new.contains_key(&id) || self.old.contains_key(&id) {
            true => false,
            false => {
                self.new.insert(id, false);
                true
            }
        }
    }

    /// Notes a transaction about to be sent; true when it was sent already.
    fn sent(&mut self, id: crate::structs::hash::Hash) -> bool {
        self.age();
        let before = self.new.get(&id).or_else(|| self.old.get(&id)).copied();
        self.new.insert(id, true);

        before == Some(true)
    }
}

/// The largest gossip message: a block with plenty of transactions.
const MAX_GOSSIP: usize = 8 * 1024 * 1024;
/// How often a peer given with --peer is dialed again while no connection to it stands: after it
/// restarts, after this node's network comes back, and while it is not up yet. Discovery runs on
/// the same tick.
const REDIAL_EVERY: Duration = Duration::from_secs(15);
/// How often Kademlia looks for peers near this node, which fills its table with the network's.
const BOOTSTRAP_EVERY: Duration = Duration::from_secs(120);
/// How often a bootnode is dialed at most, while the node has fewer peers than it wants.
const BOOTNODE_EVERY: Duration = Duration::from_secs(60);
/// How many peers Kademlia knows the node dials at once, while it has fewer than it wants.
const DIALS_AT_ONCE: usize = 3;
/// How long before a peer Kademlia knows, dialed once, is dialed again.
const RETRY_PEER_AFTER: Duration = Duration::from_secs(300);

/// Where an address to dial came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Given {
    /// --peer: dialed again whenever no connection to it stands.
    Peer,
    /// A bootnode, from --bootnode or, for chain, from genesis or governance: dialed while the
    /// node has fewer peers than it wants.
    Bootnode { chain: bool },
    /// A peer the node met before it last stopped, from its database: dialed as a bootnode is.
    Remembered,
}

/// An address to dial: its kind, and the dial under way to it or the peer it reached. Its peer id
/// is learned on connecting, as nodes take a new one each start.
struct Wanted {
    address: Multiaddr,
    given: Given,
    dialing: Option<ConnectionId>,
    reached: Option<PeerId>,
    /// When it was last dialed.
    tried: Option<Instant>,
}

impl Wanted {
    fn new(address: Multiaddr, given: Given) -> Wanted {
        Wanted {
            address,
            given,
            dialing: None,
            reached: None,
            tried: None,
        }
    }
}

#[derive(NetworkBehaviour)]
struct Behaviour {
    gossipsub: gossipsub::Behaviour,
    kad: kad::Behaviour<kad::store::MemoryStore>,
    mdns: Toggle<mdns::tokio::Behaviour>,
    identify: identify::Behaviour,
    sync: request_response::Behaviour<SyncCodec>,
}

#[document(
    summary = "Why the network could not start.",
    category = "Node",
)]
#[derive(Debug)]
pub struct NetError(pub String);

impl fmt::Display for NetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "network: {}", self.0)
    }
}

impl std::error::Error for NetError {}

fn net_error(err: impl fmt::Display) -> NetError {
    NetError(err.to_string())
}

#[document(
    summary = "Starts libp2p on a thread of its own: QUIC, gossipsub for blocks and transactions, Kademlia and mDNS to find peers, identify, and request-response for sync.",
    category = "Node",
    params = { config = "Where to listen, whom to dial, and which network to join." },
    returns = "The node's end of the network, once it is listening.",
    see_also = "NetLink, Node",
)]
pub fn spawn(config: NetConfig) -> Result<NetLink, NetError> {
    let (command_tx, command_rx) = async_mpsc::unbounded_channel();
    let (event_tx, event_rx) = mpsc::channel();
    let inbox = event_tx.clone();
    let (ready_tx, ready_rx) = mpsc::channel();
    let backlog = Arc::new(Backlog::new(config.backlog));
    let counted = backlog.clone();

    std::thread::Builder::new()
        .name("net".to_string())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(runtime) => runtime,
                Err(err) => {
                    let _ = ready_tx.send(Err(net_error(err)));

                    return;
                }
            };

            runtime.block_on(async move {
                let mut net = match Net::start(&config, counted) {
                    Ok(net) => net,
                    Err(err) => {
                        let _ = ready_tx.send(Err(err));

                        return;
                    }
                };
                let _ = ready_tx.send(Ok(()));
                net.run(command_rx, event_tx).await;
            });
        })
        .map_err(net_error)?;

    ready_rx.recv().map_err(net_error)??;

    Ok(NetLink {
        commands: command_tx,
        events: event_rx,
        inbox,
        check_inline: false,
        backlog,
    })
}

struct Net {
    swarm: Swarm<Behaviour>,
    blocks: gossipsub::IdentTopic,
    txs: gossipsub::IdentTopic,
    votes: gossipsub::IdentTopic,
    /// What identify says this node runs: a peer saying otherwise is on another chain.
    identify: String,
    channels: HashMap<u64, request_response::ResponseChannel<SyncResponse>>,
    next_id: u64,
    /// What waits for the node, counted in here.
    backlog: Arc<Backlog>,
    wanted: Vec<Wanted>,
    /// How many peers the node looks for.
    target_peers: usize,
    /// The peers Kademlia knows that discovery dialed, and when, so it moves on to others.
    tried: HashMap<PeerId, Instant>,
    /// Peers' faults and bans.
    bans: Bans,
    /// Transactions waiting to be gossiped in the next batch.
    outgoing: Vec<SignedTx>,
    /// The transactions sent and heard of lately, by id: one is never sent twice, and one heard
    /// again, as a peer passes on what this node sent it, goes no further than here.
    seen: Seen,
    /// The chain's hash, which transactions are named by.
    hash: crate::structs::hash::HashType,
    /// The IP each connected peer connects from, for bans that cover IPs.
    ips: HashMap<PeerId, IpAddr>,
}

/// The IP an address starts with, if it does.
fn ip_of(address: &Multiaddr) -> Option<IpAddr> {
    use libp2p::multiaddr::Protocol;

    match address.iter().next() {
        Some(Protocol::Ip4(ip)) => Some(IpAddr::V4(ip)),
        Some(Protocol::Ip6(ip)) => Some(IpAddr::V6(ip)),
        _ => None,
    }
}

/// The peer id an address ends in, /p2p/<id>, if it does.
fn peer_of(address: &Multiaddr) -> Option<PeerId> {
    match address.iter().last() {
        Some(libp2p::multiaddr::Protocol::P2p(peer)) => Some(peer),
        _ => None,
    }
}

impl Net {
    fn start(config: &NetConfig, backlog: Arc<Backlog>) -> Result<Net, NetError> {
        let (hash, local) = (config.hash, config.mdns);
        let names = Names::new(&config.protocol, &config.network)?;
        let (sync_protocol, kad_protocol, identify_protocol) = (names.sync.clone(), names.kad.clone(), names.identify.clone());
        // The node's own key, the same each start; a new one when none is given, as in tests.
        let identity = config.key.clone().unwrap_or_else(libp2p::identity::Keypair::generate_ed25519);
        let mut swarm = SwarmBuilder::with_existing_identity(identity)
            .with_tokio()
            .with_quic()
            .with_behaviour(|key| {
                let peer_id = key.public().to_peer_id();
                let gossip_config = gossipsub::ConfigBuilder::default()
                    .max_transmit_size(MAX_GOSSIP)
                    // Nothing is passed on until the node has checked it: its verdict comes back
                    // as NetCommand::Judge.
                    .validate_messages()
                    // Ids by content, so the same block from two peers is one message.
                    .message_id_fn(move |message| {
                        gossipsub::MessageId::from(Hasher::digest(hash, &message.data).to_bytes())
                    })
                    .build()?;
                let gossipsub = gossipsub::Behaviour::new(
                    gossipsub::MessageAuthenticity::Signed(key.clone()),
                    gossip_config,
                )?;
                let mut kad_config = kad::Config::new(kad_protocol);
                kad_config.set_periodic_bootstrap_interval(Some(BOOTSTRAP_EVERY));
                let mut kad = kad::Behaviour::with_config(peer_id, kad::store::MemoryStore::new(peer_id), kad_config);
                kad.set_mode(Some(kad::Mode::Server));
                let mdns = match local {
                    true => Some(mdns::tokio::Behaviour::new(mdns::Config::default(), peer_id)?),
                    false => None,
                };
                let identify = identify::Behaviour::new(identify::Config::new(
                    identify_protocol,
                    key.public(),
                ));
                let sync = request_response::Behaviour::with_codec(
                    SyncCodec,
                    [(sync_protocol, request_response::ProtocolSupport::Full)],
                    request_response::Config::default().with_request_timeout(Duration::from_secs(30)),
                );

                Ok(Behaviour {
                    gossipsub,
                    kad,
                    mdns: Toggle::from(mdns),
                    identify,
                    sync,
                })
            })
            .map_err(net_error)?
            .with_swarm_config(|swarm| swarm.with_idle_connection_timeout(Duration::from_secs(60)))
            .build();
        let blocks = gossipsub::IdentTopic::new(format!("{}/blocks", names.topic));
        let txs = gossipsub::IdentTopic::new(format!("{}/txs-batch", names.topic));
        let votes = gossipsub::IdentTopic::new(format!("{}/votes", names.topic));
        log::info!("[net] protocols {}/…", names.topic);
        swarm.behaviour_mut().gossipsub.subscribe(&blocks).map_err(net_error)?;
        swarm.behaviour_mut().gossipsub.subscribe(&txs).map_err(net_error)?;
        swarm.behaviour_mut().gossipsub.subscribe(&votes).map_err(net_error)?;

        for address in &config.listen {
            // Transport errors print nothing useful through Display, so show what is inside.
            swarm
                .listen_on(address.clone())
                .map_err(|err| NetError(format!("cannot listen on {address}: {err:?}")))?;
        }

        let mut wanted = Vec::new();

        for address in &config.peers {
            log::info!("[net] dialing {address}");
            let dial = DialOpts::unknown_peer_id().address(address.clone()).build();
            let id = dial.connection_id();
            swarm.dial(dial).map_err(|err| NetError(format!("cannot dial {address}: {err}")))?;
            let mut peer = Wanted::new(address.clone(), Given::Peer);
            (peer.dialing, peer.tried) = (Some(id), Some(Instant::now()));
            wanted.push(peer);
        }

        // Dialed on the first tick, as the node has no peers yet.
        wanted.extend(config.bootnodes.iter().map(|address| Wanted::new(address.clone(), Given::Bootnode { chain: false })));
        wanted.extend(config.remembered.iter().map(|address| Wanted::new(address.clone(), Given::Remembered)));

        Ok(Net {
            swarm,
            blocks,
            txs,
            votes,
            identify: names.identify,
            channels: HashMap::new(),
            next_id: 0,
            backlog,
            wanted,
            target_peers: config.target_peers,
            tried: HashMap::new(),
            bans: Bans::new(config.bans),
            outgoing: Vec::new(),
            seen: Seen::default(),
            hash: config.hash,
            ips: HashMap::new(),
        })
    }

    /// Dials each peer given with --peer that no connection stands to, nor a dial under way; and
    /// while the node has fewer peers than it wants, the bootnodes not dialed in the last minute,
    /// and a few of the peers Kademlia knows.
    fn redial(&mut self) {
        let short = self.swarm.connected_peers().count() < self.target_peers;
        let now = Instant::now();
        self.bans.sweep(now);

        for wanted in &mut self.wanted {
            let connected = wanted.reached.is_some_and(|peer| self.swarm.is_connected(&peer));
            let banned = wanted.reached.or_else(|| peer_of(&wanted.address)).is_some_and(|peer| self.bans.banned(&peer, ip_of(&wanted.address), now));

            if connected || wanted.dialing.is_some() || banned {
                continue;
            }

            if matches!(wanted.given, Given::Bootnode { .. } | Given::Remembered) && (!short || wanted.tried.is_some_and(|tried| tried.elapsed() < BOOTNODE_EVERY)) {
                continue;
            }

            let dial = DialOpts::unknown_peer_id().address(wanted.address.clone()).build();
            let id = dial.connection_id();
            wanted.tried = Some(Instant::now());

            match self.swarm.dial(dial) {
                Ok(()) => wanted.dialing = Some(id),
                Err(err) => log::debug!("[net] could not dial {} again: {err}", wanted.address),
            }
        }

        if short {
            self.discover();
        }
    }

    /// Dials a few of the peers Kademlia knows that no connection stands to and that were not
    /// dialed lately: how a node meets the peers its peers know, past the ones it was given.
    fn discover(&mut self) {
        let missing = self.target_peers.saturating_sub(self.swarm.connected_peers().count());
        self.tried.retain(|_, tried| tried.elapsed() < RETRY_PEER_AFTER);
        let local = *self.swarm.local_peer_id();
        let mut known = Vec::new();

        for bucket in self.swarm.behaviour_mut().kad.kbuckets() {
            for entry in bucket.iter() {
                known.push((*entry.node.key.preimage(), entry.node.value.iter().cloned().collect::<Vec<_>>()));
            }
        }

        let fresh: Vec<_> = known
            .into_iter()
            .filter(|(peer, addresses)| *peer != local && !addresses.is_empty() && !self.swarm.is_connected(peer) && !self.tried.contains_key(peer))
            .filter(|(peer, addresses)| !self.bans.banned(peer, addresses.first().and_then(ip_of), Instant::now()))
            .take(missing.min(DIALS_AT_ONCE))
            .collect();

        for (peer, addresses) in fresh {
            log::debug!("[net] dialing {peer}, which Kademlia knows");
            self.tried.insert(peer, Instant::now());

            if let Err(err) = self.swarm.dial(DialOpts::peer_id(peer).addresses(addresses).build()) {
                log::debug!("[net] could not dial {peer}: {err}");
            }
        }
    }

    /// Takes the chain's bootnodes, from genesis or governance, in place of those it gave before.
    fn bootnodes(&mut self, bootnodes: Vec<Multiaddr>) {
        self.wanted.retain(|wanted| wanted.given != Given::Bootnode { chain: true } || bootnodes.contains(&wanted.address));

        for address in bootnodes {
            if !self.wanted.iter().any(|wanted| wanted.address == address) {
                log::info!("[net] bootnode {address}");
                self.wanted.push(Wanted::new(address, Given::Bootnode { chain: true }));
            }
        }

        self.redial();
    }

    async fn run(
        &mut self,
        mut commands: async_mpsc::UnboundedReceiver<NetCommand>,
        events: mpsc::Sender<NetEvent>,
    ) {
        let mut redial = tokio::time::interval(REDIAL_EVERY);
        redial.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut batch = tokio::time::interval(TX_BATCH_WAIT);
        batch.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            tokio::select! {
                _ = redial.tick() => self.redial(),
                _ = batch.tick() => self.flush(),
                event = self.swarm.select_next_some() => {
                    if !self.on_swarm(event, &events) {
                        return;
                    }
                }
                command = commands.recv() => match command {
                    Some(command) => self.on_command(command, &events),
                    // The node is gone.
                    None => return,
                },
            }
        }
    }

    /// Turns what libp2p reports into events for the node. Returns false once the node is gone.
    fn on_swarm(&mut self, event: SwarmEvent<BehaviourEvent>, events: &mpsc::Sender<NetEvent>) -> bool {
        let event = match event {
            SwarmEvent::Behaviour(BehaviourEvent::Gossipsub(gossipsub::Event::Message {
                propagation_source: from,
                message_id,
                message,
            })) => {
                let gossip = Some(Gossip(message_id.clone()));

                if message.topic == self.blocks.hash() {
                    match wincode::deserialize::<Block>(&message.data) {
                        Ok(block) => NetEvent::Block { from, block, gossip },
                        Err(err) => {
                            log::debug!("[net] undecodable block from {from}: {err}");
                            self.judge(from, Some(Gossip(message_id)), Verdict::Reject(Fault::Garbage), events);

                            return true;
                        }
                    }
                } else if message.topic == self.txs.hash() {
                    return self.batch(from, message_id, &message.data, events);
                } else if message.topic == self.votes.hash() {
                    if !self.admit(from, events) {
                        self.judge(from, gossip, Verdict::Ignore, events);

                        return true;
                    }

                    match wincode::deserialize::<SignedVote>(&message.data) {
                        Ok(vote) => NetEvent::Vote { from, vote, gossip },
                        Err(err) => {
                            self.backlog.release(Origin::Peer(from));
                            log::debug!("[net] undecodable vote from {from}: {err}");
                            self.judge(from, Some(Gossip(message_id)), Verdict::Reject(Fault::Garbage), events);

                            return true;
                        }
                    }
                } else {
                    return true;
                }
            }
            SwarmEvent::Behaviour(BehaviourEvent::Mdns(mdns::Event::Discovered(found))) => {
                for (peer, address) in found {
                    self.swarm.behaviour_mut().kad.add_address(&peer, address.clone());

                    if !self.swarm.is_connected(&peer) {
                        log::info!("[net] found {peer} at {address} on the local network");

                        if let Err(err) = self.swarm.dial(address) {
                            log::debug!("[net] could not dial {peer}: {err}");
                        }
                    }
                }

                return true;
            }
            SwarmEvent::Behaviour(BehaviourEvent::Identify(identify::Event::Received {
                peer_id,
                info,
                ..
            })) => {
                // A node of another chain, or of this code under another protocol name: found on
                // the local network, or dialed by mistake. It shares nothing with this one.
                if info.protocol_version != self.identify {
                    log::info!("[net] {peer_id} runs {}, not {}: disconnecting", info.protocol_version, self.identify);
                    self.swarm.behaviour_mut().kad.remove_peer(&peer_id);
                    let _ = self.swarm.disconnect_peer_id(peer_id);

                    return true;
                }

                // A peer's loopback addresses reach it only from its own machine: kept when the
                // peer sees this node on loopback too, so nodes on one machine still find each
                // other, and left out otherwise, so the DHT never hands them to anyone else.
                let local = loopback(&info.observed_addr);
                let addresses: Vec<Multiaddr> = info.listen_addrs.into_iter().filter(|address| local || !loopback(address)).collect();

                for address in &addresses {
                    self.swarm.behaviour_mut().kad.add_address(&peer_id, address.clone());
                }

                // What the node remembers it by, with its id, for its next start.
                let addresses = addresses.into_iter().map(|address| address.with_p2p(peer_id).unwrap_or_else(|address| address)).collect();

                NetEvent::Addresses { peer: peer_id, addresses }
            }
            SwarmEvent::Behaviour(BehaviourEvent::Sync(request_response::Event::Message {
                peer,
                message,
                ..
            })) => match message {
                request_response::Message::Request { request, channel, .. } => {
                    let id = self.next_id;
                    self.next_id += 1;
                    self.channels.insert(id, channel);

                    NetEvent::Request { peer, id, request }
                }
                request_response::Message::Response { response, .. } => NetEvent::Response { peer, response },
            },
            SwarmEvent::Behaviour(BehaviourEvent::Sync(request_response::Event::OutboundFailure {
                peer,
                error,
                ..
            })) => NetEvent::RequestFailed {
                peer,
                error: error.to_string(),
            },
            SwarmEvent::Behaviour(BehaviourEvent::Gossipsub(gossipsub::Event::Subscribed { peer_id, topic })) => {
                log::debug!("[net] {peer_id} subscribed to {topic}");

                return true;
            }
            SwarmEvent::ConnectionEstablished {
                peer_id,
                connection_id,
                endpoint,
                num_established,
                ..
            } => {
                let ip = ip_of(endpoint.get_remote_address());

                // A banned peer, or with --ban-ips any peer from a banned IP, is let in no further.
                if self.bans.banned(&peer_id, ip, Instant::now()) {
                    log::debug!("[net] refused {peer_id} at {}: banned", endpoint.get_remote_address());
                    let _ = self.swarm.disconnect_peer_id(peer_id);

                    return true;
                }

                for wanted in self.wanted.iter_mut().filter(|wanted| wanted.dialing == Some(connection_id)) {
                    log::info!("[net] connected to {peer_id} at {}", wanted.address);
                    (wanted.dialing, wanted.reached) = (None, Some(peer_id));
                }

                if num_established.get() != 1 {
                    return true;
                }

                if let Some(ip) = ip {
                    self.ips.insert(peer_id, ip);
                }

                let address = endpoint.get_remote_address().clone();
                log::debug!("[net] connected to {peer_id} at {address}");

                NetEvent::PeerUp { peer: peer_id, address }
            }
            SwarmEvent::ConnectionClosed {
                peer_id,
                num_established: 0,
                ..
            } => {
                self.ips.remove(&peer_id);

                NetEvent::PeerDown(peer_id)
            }
            SwarmEvent::NewListenAddr { address, .. } => {
                let address = address.with_p2p(*self.swarm.local_peer_id()).unwrap_or_else(|address| address);
                log::info!("[net] listening on {address}");

                NetEvent::Listening(address)
            }
            SwarmEvent::OutgoingConnectionError {
                connection_id,
                peer_id,
                error,
            } => {
                let wanted = self.wanted.iter_mut().find(|wanted| wanted.dialing == Some(connection_id));

                match (wanted, peer_id) {
                    (Some(wanted), _) if wanted.given == Given::Peer => {
                        log::warn!("[net] could not reach {}, trying again in {}s: {error}", wanted.address, REDIAL_EVERY.as_secs());
                        wanted.dialing = None;
                    }
                    (Some(wanted), _) => {
                        log::info!("[net] could not reach bootnode {}, trying again within {}s while short of peers: {error}", wanted.address, BOOTNODE_EVERY.as_secs());
                        wanted.dialing = None;
                    }
                    // Kademlia's own dials, and discovery's: peers it heard of, often gone or
                    // behind NAT, so failing is ordinary.
                    (None, Some(peer)) => log::debug!("[net] could not connect to {peer}: {error}"),
                    (None, None) => log::warn!("[net] could not connect: {error}"),
                }

                return true;
            }
            SwarmEvent::ListenerError { error, .. } => {
                log::warn!("[net] listener failed: {error}");

                return true;
            }
            _ => return true,
        };

        events.send(event).is_ok()
    }

    /// Whether the backlog has room for one more transaction or vote from a peer, disconnecting a
    /// peer that keeps going over its share.
    fn admit(&mut self, from: PeerId, events: &mpsc::Sender<NetEvent>) -> bool {
        match self.backlog.admit(Origin::Peer(from), Instant::now()) {
            Admission::Admitted => true,
            Admission::Dropped => {
                log::debug!("[net] dropped gossip from {from}: the backlog is full");

                false
            }
            Admission::Disconnect => {
                log::warn!("[net] disconnecting {from}: it kept sending more gossip than its share of the backlog");
                let _ = self.swarm.disconnect_peer_id(from);
                self.judge(from, None, Verdict::Reject(Fault::Flood), events);

                false
            }
        }
    }

    /// Tells gossipsub what came of a message, which it passes on only once accepted, and counts
    /// a fault against the peer that sent it, banning it once they add up.
    fn judge(&mut self, peer: PeerId, gossip: Option<Gossip>, verdict: Verdict, events: &mpsc::Sender<NetEvent>) {
        if let Some(Gossip(id)) = gossip {
            let acceptance = match verdict {
                Verdict::Accept => gossipsub::MessageAcceptance::Accept,
                Verdict::Ignore => gossipsub::MessageAcceptance::Ignore,
                Verdict::Reject(_) => gossipsub::MessageAcceptance::Reject,
            };
            self.swarm.behaviour_mut().gossipsub.report_message_validation_result(&id, &peer, acceptance);
        }

        if let Verdict::Reject(fault) = verdict
            && self.bans.fault(peer, self.ips.get(&peer).copied(), fault, Instant::now())
        {
            log::warn!("[net] banning {peer}: it kept sending what breaks the rules");
            self.drop_banned(peer, events);
        }
    }

    /// Cuts a banned peer off: its connections, its place in Kademlia, and the node's memory of it.
    fn drop_banned(&mut self, peer: PeerId, events: &mpsc::Sender<NetEvent>) {
        let _ = self.swarm.disconnect_peer_id(peer);
        self.swarm.behaviour_mut().kad.remove_peer(&peer);
        let _ = events.send(NetEvent::Banned(peer));
    }

    fn on_command(&mut self, command: NetCommand, events: &mpsc::Sender<NetEvent>) {
        match command {
            NetCommand::Judge { peer, gossip, verdict } => self.judge(peer, gossip, verdict, events),
            NetCommand::PublishBlock(block) => self.publish(&self.blocks.clone(), &block),
            NetCommand::PublishTx(tx) => {
                // Sent once, with the others that come within TX_BATCH_WAIT, or at once when
                // TX_BATCH. One heard from a peer is marked seen as it came, so taken, it goes on
                // to the others; sent already, it does not go again.
                let id = tx.id(self.hash);

                if self.seen.sent(id) {
                    return;
                }

                self.outgoing.push(tx);

                if self.outgoing.len() >= TX_BATCH {
                    self.flush();
                }
            }
            NetCommand::PublishVote(vote) => self.publish(&self.votes.clone(), &vote),
            NetCommand::Request { peer, request } => {
                log::debug!("[net] asking {peer} for {request:?}");
                self.swarm.behaviour_mut().sync.send_request(&peer, request);
            }
            NetCommand::Respond { id, response } => {
                if let Some(channel) = self.channels.remove(&id) {
                    // Fails only when the peer stopped waiting.
                    let _ = self.swarm.behaviour_mut().sync.send_response(channel, response);
                }
            }
            NetCommand::Bootnodes(bootnodes) => self.bootnodes(bootnodes),
        }
    }

    /// A batch of transactions gossiped by a peer: each counted into the backlog and handed on as
    /// its own event, to be checked, admitted and judged alone. The batch itself is not passed on:
    /// the transactions this node takes go out again in its own batches, so one that breaks the
    /// rules goes no further than here.
    fn batch(&mut self, from: PeerId, id: gossipsub::MessageId, data: &[u8], events: &mpsc::Sender<NetEvent>) -> bool {
        let txs = match wincode::deserialize::<Vec<SignedTx>>(data) {
            Ok(txs) if !txs.is_empty() && txs.len() <= TX_BATCH => txs,
            Ok(_) | Err(_) => {
                log::debug!("[net] a batch of transactions from {from} that does not read");
                self.judge(from, Some(Gossip(id)), Verdict::Reject(Fault::Garbage), events);

                return true;
            }
        };
        self.judge(from, Some(Gossip(id)), Verdict::Ignore, events);

        for tx in txs {
            // One this node sent or heard already: nothing new, and not a slot of the backlog.
            if !self.seen.insert(tx.id(self.hash)) {
                continue;
            }

            if self.admit(from, events) && events.send(NetEvent::Tx { from, tx, gossip: None }).is_err() {
                return false;
            }
        }

        true
    }

    /// Gossips the transactions waiting to go, as one batch.
    fn flush(&mut self) {
        if self.outgoing.is_empty() {
            return;
        }

        let batch = std::mem::take(&mut self.outgoing);
        self.publish(&self.txs.clone(), &batch);
    }

    fn publish<M>(&mut self, topic: &gossipsub::IdentTopic, message: &M)
    where
        M: wincode::SchemaWrite<wincode::config::DefaultConfig, Src = M>,
    {
        let bytes = wincode::serialize(message).expect("blocks and transactions always encode");

        if let Err(err) = self.swarm.behaviour_mut().gossipsub.publish(topic.clone(), bytes) {
            // Mostly no peers yet; gossip is best effort.
            log::debug!("[net] could not publish: {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::consensus::DevAuthority,
        crate::net::{BacklogLimits, MAX_STRIKES},
        crate::structs::amount::Amount,
        crate::structs::hash::HashType,
        crate::structs::tx::{FeeOffer, Tx},
        libp2p::Multiaddr,
    };

    fn config(network: &str, backlog: BacklogLimits, peers: Vec<Multiaddr>) -> NetConfig {
        NetConfig {
            listen: vec!["/ip4/127.0.0.1/udp/0/quic-v1".parse().unwrap()],
            peers,
            mdns: true,
            hash: HashType::default_chain(),
            network: network.to_string(),
            protocol: crate::net::DEFAULT_PROTOCOL.to_string(),
            bootnodes: Vec::new(),
            target_peers: crate::net::DEFAULT_TARGET_PEERS,
            backlog,
            key: None,
            remembered: Vec::new(),
            bans: crate::net::BanLimits::default(),
        }
    }

    #[test]
    fn loopback_addresses_are_told_apart() {
        for (address, is) in [
            ("/ip4/127.0.0.1/udp/4001/quic-v1", true),
            ("/ip4/127.0.2.2/udp/53184/quic-v1", true),
            ("/ip6/::1/udp/4001/quic-v1", true),
            ("/ip4/150.40.117.173/udp/4001/quic-v1", false),
            ("/ip4/192.168.1.20/udp/4001/quic-v1", false),
        ] {
            assert_eq!(loopback(&address.parse().unwrap()), is, "{address}");
        }
    }

    #[test]
    fn protocols_and_topics_carry_the_name_and_the_network() {
        let names = Names::new("mychain", "00aa11bb22cc33dd").unwrap();

        assert_eq!(names.sync.as_ref(), "/mychain/00aa11bb22cc33dd/sync/1");
        assert_eq!(names.kad.as_ref(), "/mychain/00aa11bb22cc33dd/kad/1");
        assert_eq!(names.identify, "/mychain/00aa11bb22cc33dd/1");
        assert_eq!(names.topic, "/mychain/00aa11bb22cc33dd");
        assert_eq!(crate::net::protocol_name("my-chain.v2_test").as_deref(), Ok("my-chain.v2_test"));

        for wrong in ["", "MyChain", "-chain", "a/b", "chain name", &"x".repeat(33)] {
            assert!(crate::net::protocol_name(wrong).is_err(), "{wrong:?}");
        }
    }

    #[test]
    #[ignore = "opens real sockets on 127.0.0.1"]
    fn two_nodes_given_only_a_bootnode_find_each_other_through_it() {
        let network = format!("discovery-{}", PeerId::random());
        let node = |bootnodes: Vec<Multiaddr>| NetConfig {
            mdns: false,
            bootnodes,
            ..config(&network, BacklogLimits::default(), Vec::new())
        };
        let bootnode = spawn(node(Vec::new())).unwrap();
        let address = wait_for(&bootnode, |event| match event {
            NetEvent::Listening(address) => Some(address),
            _ => None,
        });
        let address: Multiaddr = address.iter().filter(|part| !matches!(part, libp2p::multiaddr::Protocol::P2p(_))).collect();
        let first = spawn(node(vec![address.clone()])).unwrap();
        let first_id = wait_for(&bootnode, |event| match event {
            NetEvent::PeerUp { peer, .. } => Some(peer),
            _ => None,
        });
        let second = spawn(node(vec![address])).unwrap();

        // The second meets the bootnode, then, through Kademlia, the first, which it was never
        // given: discovery runs every 15 seconds while it is short of peers.
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut met = Vec::new();

        while !met.contains(&first_id) {
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "never met the first node: met {met:?}");

            if let Ok(NetEvent::PeerUp { peer, .. }) = second.events.recv_timeout(left) {
                met.push(peer);
            }
        }

        drop((first, second));
    }

    #[test]
    #[ignore = "opens real sockets on 127.0.0.1"]
    fn a_peer_under_another_protocol_name_is_disconnected() {
        let network = format!("names-{}", PeerId::random());
        let alone = |protocol: &str, peers: Vec<Multiaddr>| NetConfig {
            mdns: false,
            protocol: protocol.to_string(),
            ..config(&network, BacklogLimits::default(), peers)
        };
        let ours = spawn(alone("chainv4", Vec::new())).unwrap();
        let address = wait_for(&ours, |event| match event {
            NetEvent::Listening(address) => Some(address),
            _ => None,
        });
        let theirs = spawn(alone("otherchain", vec![address])).unwrap();
        let peer = wait_for(&ours, |event| match event {
            NetEvent::PeerUp { peer, .. } => Some(peer),
            _ => None,
        });

        assert_eq!(wait_for(&ours, |event| match event {
            NetEvent::PeerDown(down) => Some(down),
            _ => None,
        }), peer, "identify told them apart");
        drop(theirs);
    }

    /// The next event matching, waiting up to a deadline.
    fn wait_for<T>(link: &NetLink, mut matches: impl FnMut(NetEvent) -> Option<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(30);

        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let event = link.events.recv_timeout(left).expect("the event came in time");

            if let Some(found) = matches(event) {
                return found;
            }
        }
    }

    #[test]
    #[ignore = "opens real sockets on 127.0.0.1"]
    fn a_peer_given_is_dialed_again_once_it_comes_back() {
        use libp2p::multiaddr::Protocol;

        // Without mDNS, which would find the node again on its own: only redialing can.
        let network = format!("redial-{}", PeerId::random());
        let alone = |peers: Vec<Multiaddr>| NetConfig {
            mdns: false,
            ..config(&network, BacklogLimits::default(), peers)
        };
        let first = spawn(alone(Vec::new())).unwrap();
        let listening = wait_for(&first, |event| match event {
            NetEvent::Listening(address) => Some(address),
            _ => None,
        });
        // As --peer gives it: no peer id, since a node takes a new one each start.
        let address: Multiaddr = listening.iter().filter(|part| !matches!(part, Protocol::P2p(_))).collect();
        let dialer = spawn(alone(vec![address.clone()])).unwrap();
        let before = wait_for(&dialer, |event| match event {
            NetEvent::PeerUp { peer, .. } => Some(peer),
            _ => None,
        });

        // It stops, and starts again at the same address, as a restarted node with a new id.
        drop(first);
        assert_eq!(wait_for(&dialer, |event| match event {
            NetEvent::PeerDown(peer) => Some(peer),
            _ => None,
        }), before);

        let deadline = Instant::now() + Duration::from_secs(10);
        let again = loop {
            let mut back = alone(Vec::new());
            back.listen = vec![address.clone()];

            match spawn(back) {
                Ok(again) => break again,
                Err(err) if Instant::now() < deadline => {
                    log::debug!("the port is not free yet: {err}");
                    std::thread::sleep(Duration::from_millis(200));
                }
                Err(err) => panic!("could not listen there again: {err}"),
            }
        };
        let after = wait_for(&dialer, |event| match event {
            NetEvent::PeerUp { peer, .. } => Some(peer),
            _ => None,
        });

        assert_ne!(after, before, "the node that came back, under its new id");
        drop(again);
    }

    #[test]
    #[ignore = "opens real sockets on 127.0.0.1"]
    fn a_peer_flooding_gossip_is_turned_away_then_disconnected() {
        // Its own network name, so no other node on this machine's network joins in.
        let network = format!("flood-{}", PeerId::random());
        let flooder = spawn(config(&network, BacklogLimits::default(), Vec::new())).unwrap();
        let address = wait_for(&flooder, |event| match event {
            NetEvent::Listening(address) => Some(address),
            _ => None,
        });
        // Nobody drains the victim's events, so two transactions fill the flooder's share.
        let victim = spawn(config(&network, BacklogLimits { total: 1_000, per_peer: 2 }, vec![address])).unwrap();
        let flooder_id = wait_for(&victim, |event| match event {
            NetEvent::PeerUp { peer, .. } => Some(peer),
            _ => None,
        });
        let authority = DevAuthority::dev_keypair();
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut nonce = 0;

        // Gossip reaches the victim once it has told the flooder what it subscribes to.
        while victim.backlog.disconnected() == 0 {
            assert!(Instant::now() < deadline, "never disconnected; dropped {}", victim.backlog.dropped());
            let tx = SignedTx::sign(Tx::Burn { from: authority.public().address(), amount: Amount::U128(1) }, nonce, FeeOffer::NONE, &authority);
            flooder.send(NetCommand::PublishTx(tx));
            nonce += 1;
            std::thread::sleep(Duration::from_millis(20));
        }

        assert_eq!(victim.backlog.waiting(), 2, "its share, and no more");
        assert!(victim.backlog.dropped() >= u64::from(MAX_STRIKES));
        assert_eq!(
            wait_for(&victim, |event| match event {
                NetEvent::PeerDown(peer) => Some(peer),
                _ => None,
            }),
            flooder_id
        );
    }

    #[test]
    #[ignore = "opens real sockets on 127.0.0.1"]
    fn a_node_keeps_its_peer_id_and_a_banned_peer_is_refused_when_it_dials_again() {
        use crate::net::{Fault, Verdict};

        let network = format!("bans-{}", PeerId::random());
        let key = libp2p::identity::Keypair::generate_ed25519();
        let ours = spawn(NetConfig {
            mdns: false,
            key: Some(key.clone()),
            ..config(&network, BacklogLimits::default(), Vec::new())
        })
        .unwrap();
        let address = wait_for(&ours, |event| match event {
            NetEvent::Listening(address) => Some(address),
            _ => None,
        });

        // Its address names it by the key it was given, as it will after every start.
        assert_eq!(peer_of(&address), Some(key.public().to_peer_id()));

        // Theirs dials ours by it, and again every 15 seconds while cut off.
        let theirs = spawn(NetConfig { mdns: false, ..config(&network, BacklogLimits::default(), vec![address]) }).unwrap();
        let peer = wait_for(&ours, |event| match event {
            NetEvent::PeerUp { peer, .. } => Some(peer),
            _ => None,
        });

        for _ in 0..2 {
            ours.send(NetCommand::Judge { peer, gossip: None, verdict: Verdict::Reject(Fault::Block) });
        }

        assert_eq!(
            wait_for(&ours, |event| match event {
                NetEvent::Banned(banned) => Some(banned),
                _ => None,
            }),
            peer
        );

        // Its redials reach ours, and are refused: it never comes up again.
        let deadline = Instant::now() + REDIAL_EVERY * 3;

        while let Ok(event) = ours.events.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            assert!(!matches!(event, NetEvent::PeerUp { peer: up, .. } if up == peer), "a banned peer came back");
        }

        drop(theirs);
    }

    #[test]
    fn a_transaction_goes_out_once_and_one_heard_again_goes_no_further() {
        let mut seen = Seen::default();
        let (mine, theirs) = (crate::structs::hash::Hash::Sha256([1; 32]), crate::structs::hash::Hash::Sha256([2; 32]));

        assert!(!seen.sent(mine), "sent the first time");
        assert!(seen.sent(mine), "and never again");
        assert!(!seen.insert(mine), "heard back from a peer: no news");
        assert!(seen.insert(theirs), "heard first from a peer");
        assert!(!seen.sent(theirs), "taken, it goes on to the others");
        assert!(seen.sent(theirs));

        // Two generations later it is forgotten.
        seen.since = Some(Instant::now() - SEEN_FOR * 2);
        seen.age();
        seen.since = Some(Instant::now() - SEEN_FOR * 2);
        assert!(seen.insert(mine));
    }

    #[test]
    #[ignore = "opens real sockets on 127.0.0.1"]
    fn transactions_go_out_in_batches_and_every_one_arrives() {
        let network = format!("batches-{}", PeerId::random());
        let alone = |peers: Vec<Multiaddr>| NetConfig { mdns: false, ..config(&network, BacklogLimits::default(), peers) };
        let receiver = spawn(alone(Vec::new())).unwrap();
        let address = wait_for(&receiver, |event| match event {
            NetEvent::Listening(address) => Some(address),
            _ => None,
        });
        let sender = spawn(alone(vec![address])).unwrap();
        wait_for(&receiver, |event| match event {
            NetEvent::PeerUp { .. } => Some(()),
            _ => None,
        });
        // Gossip reaches a peer once it has said what it subscribes to.
        std::thread::sleep(Duration::from_secs(2));

        // More than one batch's worth, sent faster than a batch waits.
        let authority = DevAuthority::dev_keypair();
        let count = TX_BATCH + 44;

        for nonce in 0..count as u64 {
            sender.send(NetCommand::PublishTx(SignedTx::sign(Tx::Burn { from: authority.public().address(), amount: Amount::U128(1) }, nonce, FeeOffer::NONE, &authority)));
        }

        let mut nonces = std::collections::BTreeSet::new();
        let deadline = Instant::now() + Duration::from_secs(20);

        while nonces.len() < count {
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "only {} of {count} arrived", nonces.len());

            if let Ok(NetEvent::Tx { tx, gossip, .. }) = receiver.events.recv_timeout(left) {
                assert_eq!(gossip, None, "each judged alone, the batch never forwarded whole");
                receiver.backlog.release(Origin::Peer(PeerId::random()));
                nonces.insert(tx.nonce);
            }
        }

        assert_eq!(nonces, (0..count as u64).collect());
    }
}
