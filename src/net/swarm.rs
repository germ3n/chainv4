use {
    super::codec::SyncCodec,
    super::{NetCommand, NetConfig, NetEvent, NetLink, SyncResponse},
    crate::crypto::hasher::Hasher,
    crate::structs::block::Block,
    crate::structs::tx::SignedTx,
    crate::structs::vote::SignedVote,
    libp2p::futures::StreamExt,
    libp2p::swarm::{NetworkBehaviour, Swarm, SwarmEvent},
    libp2p::{StreamProtocol, SwarmBuilder, gossipsub, identify, kad, mdns, request_response},
    r#macro::document,
    std::collections::HashMap,
    std::fmt,
    std::sync::mpsc,
    std::time::Duration,
    tokio::sync::mpsc as async_mpsc,
};

const SYNC_PROTOCOL: StreamProtocol = StreamProtocol::new("/chainv4/sync/1");
const KAD_PROTOCOL: StreamProtocol = StreamProtocol::new("/chainv4/kad/1");
const IDENTIFY_PROTOCOL: &str = "/chainv4/1";
/// The largest gossip message: a block with plenty of transactions.
const MAX_GOSSIP: usize = 4 * 1024 * 1024;

#[derive(NetworkBehaviour)]
struct Behaviour {
    gossipsub: gossipsub::Behaviour,
    kad: kad::Behaviour<kad::store::MemoryStore>,
    mdns: mdns::tokio::Behaviour,
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
                let mut net = match Net::start(&config) {
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
    })
}

struct Net {
    swarm: Swarm<Behaviour>,
    blocks: gossipsub::IdentTopic,
    txs: gossipsub::IdentTopic,
    votes: gossipsub::IdentTopic,
    channels: HashMap<u64, request_response::ResponseChannel<SyncResponse>>,
    next_id: u64,
}

impl Net {
    fn start(config: &NetConfig) -> Result<Net, NetError> {
        let hash = config.hash;
        let mut swarm = SwarmBuilder::with_new_identity()
            .with_tokio()
            .with_quic()
            .with_behaviour(|key| {
                let peer_id = key.public().to_peer_id();
                let gossip_config = gossipsub::ConfigBuilder::default()
                    .max_transmit_size(MAX_GOSSIP)
                    // Ids by content, so the same block from two peers is one message.
                    .message_id_fn(move |message| {
                        gossipsub::MessageId::from(Hasher::digest(hash, &message.data).to_bytes())
                    })
                    .build()?;
                let gossipsub = gossipsub::Behaviour::new(
                    gossipsub::MessageAuthenticity::Signed(key.clone()),
                    gossip_config,
                )?;
                let mut kad = kad::Behaviour::with_config(
                    peer_id,
                    kad::store::MemoryStore::new(peer_id),
                    kad::Config::new(KAD_PROTOCOL),
                );
                kad.set_mode(Some(kad::Mode::Server));
                let mdns = mdns::tokio::Behaviour::new(mdns::Config::default(), peer_id)?;
                let identify = identify::Behaviour::new(identify::Config::new(
                    IDENTIFY_PROTOCOL.to_string(),
                    key.public(),
                ));
                let sync = request_response::Behaviour::with_codec(
                    SyncCodec,
                    [(SYNC_PROTOCOL, request_response::ProtocolSupport::Full)],
                    request_response::Config::default().with_request_timeout(Duration::from_secs(30)),
                );

                Ok(Behaviour {
                    gossipsub,
                    kad,
                    mdns,
                    identify,
                    sync,
                })
            })
            .map_err(net_error)?
            .with_swarm_config(|swarm| swarm.with_idle_connection_timeout(Duration::from_secs(60)))
            .build();
        let blocks = gossipsub::IdentTopic::new(format!("/chainv4/{}/blocks", config.network));
        let txs = gossipsub::IdentTopic::new(format!("/chainv4/{}/txs", config.network));
        let votes = gossipsub::IdentTopic::new(format!("/chainv4/{}/votes", config.network));
        swarm.behaviour_mut().gossipsub.subscribe(&blocks).map_err(net_error)?;
        swarm.behaviour_mut().gossipsub.subscribe(&txs).map_err(net_error)?;
        swarm.behaviour_mut().gossipsub.subscribe(&votes).map_err(net_error)?;

        for address in &config.listen {
            // Transport errors print nothing useful through Display, so show what is inside.
            swarm
                .listen_on(address.clone())
                .map_err(|err| NetError(format!("cannot listen on {address}: {err:?}")))?;
        }

        for address in &config.peers {
            log::info!("[net] dialing {address}");
            swarm
                .dial(address.clone())
                .map_err(|err| NetError(format!("cannot dial {address}: {err}")))?;
        }

        Ok(Net {
            swarm,
            blocks,
            txs,
            votes,
            channels: HashMap::new(),
            next_id: 0,
        })
    }

    async fn run(
        &mut self,
        mut commands: async_mpsc::UnboundedReceiver<NetCommand>,
        events: mpsc::Sender<NetEvent>,
    ) {
        loop {
            tokio::select! {
                event = self.swarm.select_next_some() => {
                    if !self.on_swarm(event, &events) {
                        return;
                    }
                }
                command = commands.recv() => match command {
                    Some(command) => self.on_command(command),
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
                message,
                ..
            })) => {
                if message.topic == self.blocks.hash() {
                    match wincode::deserialize::<Block>(&message.data) {
                        Ok(block) => NetEvent::Block { from, block },
                        Err(err) => {
                            log::debug!("[net] undecodable block from {from}: {err}");

                            return true;
                        }
                    }
                } else if message.topic == self.txs.hash() {
                    match wincode::deserialize::<SignedTx>(&message.data) {
                        Ok(tx) => NetEvent::Tx { from, tx },
                        Err(err) => {
                            log::debug!("[net] undecodable transaction from {from}: {err}");

                            return true;
                        }
                    }
                } else if message.topic == self.votes.hash() {
                    match wincode::deserialize::<SignedVote>(&message.data) {
                        Ok(vote) => NetEvent::Vote { from, vote },
                        Err(err) => {
                            log::debug!("[net] undecodable vote from {from}: {err}");

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
                for address in info.listen_addrs {
                    self.swarm.behaviour_mut().kad.add_address(&peer_id, address);
                }

                return true;
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
                endpoint,
                num_established,
                ..
            } if num_established.get() == 1 => {
                let address = endpoint.get_remote_address().clone();
                log::debug!("[net] connected to {peer_id} at {address}");

                NetEvent::PeerUp { peer: peer_id, address }
            }
            SwarmEvent::ConnectionClosed {
                peer_id,
                num_established: 0,
                ..
            } => NetEvent::PeerDown(peer_id),
            SwarmEvent::NewListenAddr { address, .. } => {
                let address = address.with_p2p(*self.swarm.local_peer_id()).unwrap_or_else(|address| address);
                log::info!("[net] listening on {address}");

                NetEvent::Listening(address)
            }
            SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
                match peer_id {
                    Some(peer) => log::warn!("[net] could not connect to {peer}: {error}"),
                    None => log::warn!("[net] could not connect: {error}"),
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

    fn on_command(&mut self, command: NetCommand) {
        match command {
            NetCommand::PublishBlock(block) => self.publish(&self.blocks.clone(), &block),
            NetCommand::PublishTx(tx) => self.publish(&self.txs.clone(), &tx),
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
        }
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
