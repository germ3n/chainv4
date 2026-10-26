use super::*;
use crate::net::{MAX_SYNC_BLOCKS, SyncRequest, SyncResponse};
use crate::chain::Reward;
use crate::consensus::{AuthorityProducer, ConsensusConfig, DevAuthority, Miner, PowConfig, meets, work_hash};
use crate::structs::address::Address;
use crate::structs::hash::{Hash, HashType};
use crate::structs::block::{BlockHeader, Seal};
use crate::crypto::keypair::Keypair;
use crate::db::MemoryDb;
use crate::net::FakeNet;
use crate::registry::Accounts;
use crate::structs::amount::Amount;
use crate::structs::block::Block;
use crate::structs::tx::{FeeOffer, Tx};
use crate::structs::u256::u256;
use libp2p::PeerId;

const SLOT: u64 = 2_000;

fn node(produce: bool) -> (Node, FakeNet) {
    let authority = DevAuthority::dev_keypair();
    let genesis = Genesis {
        fees: None,
        ..Genesis::dev_authority(authority.public())
    };
    let producer = produce.then(|| Box::new(AuthorityProducer::new(authority, SLOT)) as Box<dyn Producer>);
    let (link, fake) = NetLink::pair();
    let node = Node::new(Arc::new(MemoryDb::default()), &genesis, producer, link).unwrap();

    (node, fake)
}

/// A proof-of-work chain where any nonce does, so tests mine instantly.
fn easy_pow() -> Genesis {
    Genesis {
        consensus: ConsensusConfig::ProofOfWork(PowConfig {
            hash: HashType::default_chain(),
            target_block_ms: 1_000,
            initial_difficulty: 1,
            min_difficulty: 1,
        }),
        reward: Reward {
            initial: u256::new(5),
            halving_interval: None,
        },
        fees: None,
        ..Genesis::dev_pow(DevAuthority::dev_keypair().public())
    }
}

fn pow_node() -> (Node, FakeNet) {
    let (link, fake) = NetLink::pair();
    let node = Node::new(Arc::new(MemoryDb::default()), &easy_pow(), None, link).unwrap();

    (node, fake)
}

/// Mines a block on parent by hand, one target later.
fn mine_on(node: &Node, parent: &BlockHeader, miner: u8) -> Block {
    let timestamp = parent.timestamp + 1_000;
    let mut block = node
        .chain()
        .build_on(&parent.id(), Vec::new(), Vec::new(), Address::Ed25519([miner; 32]), timestamp)
        .unwrap();
    let nonce = (0..)
        .find(|nonce| meets(work_hash(HashType::default_chain(), &block.id(), *nonce), block.header.difficulty))
        .unwrap();
    block.seal = Seal::Work { nonce };

    block
}

/// Far enough ahead that no block in these tests is from the future.
const LATER: u64 = 10_000_000;

/// Everything the node asked the network to do since the last call.
fn sent(fake: &mut FakeNet) -> Vec<NetCommand> {
    let mut out = Vec::new();

    while let Ok(command) = fake.commands.try_recv() {
        out.push(command);
    }

    out
}

fn published_block(fake: &mut FakeNet) -> Block {
    sent(fake)
        .into_iter()
        .find_map(|command| match command {
            NetCommand::PublishBlock(block) => Some(block),
            _ => None,
        })
        .expect("a block was published")
}

/// A producing node on a chain of this genesis.
fn producing_on(genesis: &Genesis) -> (Node, FakeNet) {
    let authority = DevAuthority::dev_keypair();
    let producer = Box::new(AuthorityProducer::new(authority, SLOT)) as Box<dyn Producer>;
    let (link, fake) = NetLink::pair();

    (Node::new(Arc::new(MemoryDb::default()), genesis, Some(producer), link).unwrap(), fake)
}

/// The development key's transfers, from this nonce on.
fn transfers(count: u64) -> impl Iterator<Item = SignedTx> {
    let authority = DevAuthority::dev_keypair();
    let to = Keypair::generate(authority.public().key_type()).unwrap().public().address();

    (0..count).map(move |nonce| {
        let tx = Tx::Transfer { from: authority.public().address(), to, amount: Amount::U128(1) };

        SignedTx::sign(tx, nonce, FeeOffer::NONE, &authority)
    })
}

#[test]
fn blocks_fill_to_the_chains_limits_not_a_fixed_count() {
    let authority = DevAuthority::dev_keypair();
    let bounded = Genesis { fees: None, ..Genesis::dev_authority(authority.public()) };

    // Bounded by max_block_bytes alone, a block holds all 1,500.
    let (mut producer, mut net) = producing_on(&bounded);
    transfers(1_500).for_each(|tx| producer.submit(tx).unwrap());
    producer.step(SLOT).unwrap();

    assert_eq!(published_block(&mut net).txs.len(), 1_500);
    assert!(producer.mempool().is_empty());

    // The node's own cap holds its blocks to it.
    let (producer, mut net) = producing_on(&bounded);
    let mut producer = producer.max_block_txs(Some(100));
    transfers(300).for_each(|tx| producer.submit(tx).unwrap());
    producer.step(SLOT).unwrap();

    assert_eq!(published_block(&mut net).txs.len(), 100);
    assert_eq!(producer.mempool().len(), 200);

    // A chain that bounds blocks by neither bytes nor gas takes 1,000 a block.
    let unbounded = Genesis { max_block_bytes: None, ..bounded };
    let (mut producer, mut net) = producing_on(&unbounded);
    transfers(1_200).for_each(|tx| producer.submit(tx).unwrap());
    producer.step(SLOT).unwrap();

    assert_eq!(published_block(&mut net).txs.len(), UNBOUNDED_CHAIN_TXS);
}

#[test]
fn a_time_budget_seals_what_ran_by_then_and_the_rest_wait() {
    let authority = DevAuthority::dev_keypair();
    let genesis = Genesis { fees: None, ..Genesis::dev_authority(authority.public()) };

    // A budget of a millisecond: 1,500 transfers take longer, so the block is cut short and the
    // rest stay for the next block, which gets the time it needs.
    let (producer, mut net) = producing_on(&genesis);
    let mut producer = producer.max_build_ms(Some(1));
    transfers(1_500).for_each(|tx| producer.submit(tx).unwrap());
    producer.step(SLOT).unwrap();

    let first = published_block(&mut net);

    assert!(!first.txs.is_empty() && first.txs.len() < 1_500, "{} went in", first.txs.len());
    assert_eq!(producer.mempool().len(), 1_500 - first.txs.len());
    assert_eq!(producer.chain().tip().height, 1);

    let mut producer = producer.max_build_ms(None);
    producer.step(2 * SLOT).unwrap();

    assert_eq!(published_block(&mut net).txs.len(), 1_500 - first.txs.len());
    assert!(producer.mempool().is_empty());
}

#[cfg(feature = "finality-authority")]
#[test]
fn a_rotating_authority_s_one_signer_produces_every_slot() {
    use crate::consensus::RotatingProducer;

    let dev = DevAuthority::dev_keypair();
    let genesis = Genesis::dev_rotating(dev.public());
    let producer = Box::new(RotatingProducer::new(dev.clone(), dev.public().address(), SLOT)) as Box<dyn Producer>;
    let (link, mut net) = NetLink::pair();
    let mut node = Node::new(Arc::new(MemoryDb::default()), &genesis, Some(producer), link).unwrap();

    node.step(SLOT - 1).unwrap();
    assert_eq!(node.chain().tip().height, 0);
    node.step(SLOT).unwrap();
    assert_eq!(node.chain().tip().height, 1);
    assert_eq!(published_block(&mut net).header.producer, dev.public().address());
    node.step(2 * SLOT).unwrap();
    assert_eq!((node.chain().tip().height, node.chain().tip().difficulty), (2, 0));
    assert_eq!(node.chain().tip_work(), u256::new(4), "in turn, each weighs 2");

    // A node whose key is no signer never produces.
    let stranger = Keypair::generate(dev.public().key_type()).unwrap();
    let producer = Box::new(RotatingProducer::new(stranger.clone(), stranger.public().address(), SLOT)) as Box<dyn Producer>;
    let (link, _net) = NetLink::pair();
    let mut idle = Node::new(Arc::new(MemoryDb::default()), &genesis, Some(producer), link).unwrap();
    idle.step(10 * SLOT).unwrap();
    assert_eq!(idle.chain().tip().height, 0);
}

#[test]
fn the_producer_makes_a_block_when_its_slot_comes() {
    let (mut producer, mut net) = node(true);
    producer.step(SLOT - 1).unwrap();

    assert_eq!(producer.chain().tip().height, 0);
    assert!(sent(&mut net).is_empty());

    producer.step(SLOT).unwrap();

    assert_eq!(producer.chain().tip().height, 1);
    assert_eq!(published_block(&mut net).header.height, 1);
}

#[test]
fn the_loop_catches_each_index_up_and_moves_its_window() {
    let (mut producer, _net) = node(true);

    for slot in 1..=3 {
        producer.step(slot * SLOT).unwrap();
    }

    let producer = producer.keep_index(IndexKind::Events, Some(KeepFrom::Latest(2))).unwrap();
    let mut producer = producer.keep_index(IndexKind::Addresses, None).unwrap();
    let from = |producer: &Node, kind| producer.chain().index_window(kind).unwrap().from;

    assert_eq!(from(&producer, IndexKind::Events), 4, "a new index starts empty at the next block");
    assert_eq!(from(&producer, IndexKind::Addresses), 4);
    assert!(producer.catching_up.contains(&IndexKind::Events), "so the loop does not sleep until it holds heights 2 and 3");
    assert!(producer.catching_up.contains(&IndexKind::Addresses));

    producer.step(3 * SLOT + 1).unwrap();

    assert_eq!(from(&producer, IndexKind::Events), 2);
    assert_eq!(from(&producer, IndexKind::Addresses), 0, "each to its own window");
    assert!(producer.catching_up.is_empty());

    producer.step(4 * SLOT).unwrap();

    assert_eq!(producer.chain().tip().height, 4);
    assert_eq!(from(&producer, IndexKind::Events), 3, "the same pass drops height 2");
    assert_eq!(from(&producer, IndexKind::Addresses), 0);

    let producer = producer.drop_index(IndexKind::Events).unwrap();

    assert_eq!(producer.chain().index_window(IndexKind::Events), None);
    assert_eq!(from(&producer, IndexKind::Addresses), 0, "dropping one keeps the other");
}

#[test]
fn gossip_leaves_the_backlog_once_the_loop_handled_it() {
    let (mut follower, follower_net) = node(false);
    let authority = DevAuthority::dev_keypair();
    let peer = PeerId::random();
    let backlog = follower.backlog.clone();

    for nonce in 0..2 {
        let tx = SignedTx::sign(Tx::Burn { from: authority.public().address(), amount: Amount::U128(1) }, nonce, FeeOffer::NONE, &authority);

        // As the network does: counted in, then handed on.
        assert_eq!(backlog.admit(crate::net::Origin::Peer(peer), std::time::Instant::now()), crate::net::Admission::Admitted);
        follower_net.events.send(NetEvent::Tx { from: peer, tx }).unwrap();
    }

    assert_eq!(backlog.waiting(), 2);

    follower.step(1).unwrap();

    assert_eq!(backlog.waiting(), 0, "taken and refused alike");
    assert_eq!(follower.report(1, 0).backlog.0, 0);
}

#[test]
fn followers_import_gossiped_blocks_and_never_produce() {
    let (mut producer, mut producer_net) = node(true);
    let (mut follower, mut follower_net) = node(false);
    producer.step(SLOT).unwrap();
    let block = published_block(&mut producer_net);
    follower_net
        .events
        .send(NetEvent::Block {
            from: PeerId::random(),
            block,
        })
        .unwrap();
    follower.step(SLOT * 10).unwrap();

    assert_eq!(follower.chain().tip().id(), producer.chain().tip().id());
    assert!(sent(&mut follower_net).is_empty());
}

#[cfg(feature = "secp256k1")]
#[test]
fn a_transfer_signed_as_typed_data_goes_into_a_block_as_any_other() {
    use crate::chain::Allocation;
    use crate::crypto::keypair::KeyType;

    let authority = DevAuthority::dev_keypair();
    let ethereum = Keypair::generate(KeyType::Secp256k1).unwrap();
    let mut genesis = Genesis {
        fees: None,
        ..Genesis::dev_authority(authority.public())
    };
    genesis.allocations.push(Allocation { address: ethereum.public().address(), amount: u256::new(1_000) });
    let start = |producer: Option<Box<dyn Producer>>| {
        let (link, fake) = NetLink::pair();

        (Node::new(Arc::new(MemoryDb::default()), &genesis, producer, link).unwrap(), fake)
    };
    let (mut producer, mut producer_net) = start(Some(Box::new(AuthorityProducer::new(authority, SLOT))));
    let (mut follower, follower_net) = start(None);
    let bob = Keypair::generate(DevAuthority::dev_keypair().public().key_type()).unwrap().public().address();
    let transfer = Tx::Transfer {
        from: ethereum.public().address(),
        to: bob,
        amount: Amount::U128(42),
    };

    // Signed for another chain, it is refused at the door.
    let elsewhere = Hash::from_bytes(HashType::default_chain(), &[9; 32]).unwrap();
    let foreign = SignedTx::sign_eip712(transfer.clone(), 0, FeeOffer::NONE, &ethereum, &elsewhere).unwrap();

    assert!(producer.submit(foreign).is_err());

    // For this one, it goes into the producer's block, and the follower imports it.
    let signed = SignedTx::sign_eip712(transfer, 0, FeeOffer::NONE, &ethereum, &producer.chain().genesis()).unwrap();
    producer.submit(signed.clone()).unwrap();
    producer.step(SLOT).unwrap();
    let block = published_block(&mut producer_net);

    assert_eq!(block.txs, [signed]);

    follower_net.events.send(NetEvent::Block { from: PeerId::random(), block }).unwrap();
    follower.step(SLOT).unwrap();

    assert_eq!(Accounts::new(&follower.chain().state()).get(&bob).unwrap().balance, u256::new(42));
    assert_eq!(Accounts::new(&follower.chain().state()).get(&ethereum.public().address()).unwrap().nonce, 1);
}

#[test]
fn transactions_travel_to_the_producer_and_into_a_block() {
    let (mut producer, mut producer_net) = node(true);
    let (mut follower, mut follower_net) = node(false);
    let authority = DevAuthority::dev_keypair();
    let bob = Keypair::generate(authority.public().key_type()).unwrap().public().address();
    let tx = SignedTx::sign(
        Tx::Transfer {
            from: authority.public().address(),
            to: bob,
            amount: Amount::U128(42),
        },
        0,
        FeeOffer::NONE,
        &authority,
    );
    follower.submit(tx.clone()).unwrap();
    let gossiped = sent(&mut follower_net)
        .into_iter()
        .find_map(|command| match command {
            NetCommand::PublishTx(tx) => Some(tx),
            _ => None,
        })
        .unwrap();

    assert_eq!(gossiped, tx);

    producer_net
        .events
        .send(NetEvent::Tx {
            from: PeerId::random(),
            tx: gossiped,
        })
        .unwrap();
    producer.step(SLOT).unwrap();
    let block = published_block(&mut producer_net);

    assert_eq!(block.txs, [tx]);

    follower_net
        .events
        .send(NetEvent::Block {
            from: PeerId::random(),
            block,
        })
        .unwrap();
    follower.step(SLOT).unwrap();

    assert_eq!(Accounts::new(&follower.chain().state()).get(&bob).unwrap().balance, u256::new(42));
    assert!(follower.mempool().is_empty());
    assert!(producer.mempool().is_empty());
}

#[test]
fn blocks_ahead_of_the_tip_wait_for_sync() {
    let (mut producer, mut producer_net) = node(true);
    let (mut follower, follower_net) = node(false);
    producer.step(SLOT).unwrap();
    published_block(&mut producer_net);
    producer.step(SLOT * 2).unwrap();
    let second = published_block(&mut producer_net);
    follower_net
        .events
        .send(NetEvent::Block {
            from: PeerId::random(),
            block: second,
        })
        .unwrap();
    follower.step(SLOT * 2).unwrap();

    assert_eq!(follower.chain().tip().height, 0);
}

#[test]
fn peers_get_status_and_blocks() {
    let (mut producer, mut net) = node(true);
    producer.step(SLOT).unwrap();
    sent(&mut net);
    let peer = PeerId::random();
    net.events
        .send(NetEvent::Request {
            peer,
            id: 1,
            request: SyncRequest::Status,
        })
        .unwrap();
    net.events
        .send(NetEvent::Request {
            peer,
            id: 2,
            request: SyncRequest::Blocks { from: 0, count: 10 },
        })
        .unwrap();
    producer.step(SLOT + 1).unwrap();
    let answers = sent(&mut net);

    assert!(matches!(
        &answers[0],
        NetCommand::Respond {
            id: 1,
            response: SyncResponse::Status { height: 1, .. },
        }
    ));
    assert!(matches!(
        &answers[1],
        NetCommand::Respond {
            id: 2,
            response: SyncResponse::Blocks(blocks),
        } if blocks.len() == 2 && blocks[1].header.height == 1
    ));
}

/// One node in a relay, with its fake network and the peer id the other node knows it by.
struct Side<'a> {
    node: &'a mut Node,
    net: &'a mut FakeNet,
    peer: PeerId,
}

/// Passes sync requests between two nodes and their answers back, until neither has anything
/// left to ask. Returns everything else they sent.
fn relay(mut a: Side, mut b: Side, now: u64) -> Vec<NetCommand> {
    let mut other = Vec::new();

    loop {
        let moved = pass(&mut a, &mut b, now, &mut other) | pass(&mut b, &mut a, now, &mut other);

        if !moved {
            return other;
        }
    }
}

/// Delivers from's requests for to, and to's answers back. Returns whether anything moved.
fn pass(from: &mut Side, to: &mut Side, now: u64, other: &mut Vec<NetCommand>) -> bool {
    let mut moved = false;

    for command in sent(from.net) {
        match command {
            NetCommand::Request { peer, request } if peer == to.peer => {
                moved = true;
                to.net.events.send(NetEvent::Request { peer: from.peer, id: 1, request }).unwrap();
                to.node.step(now).unwrap();

                for answer in sent(to.net) {
                    match answer {
                        NetCommand::Respond { response, .. } => {
                            from.net.events.send(NetEvent::Response { peer: to.peer, response }).unwrap();
                        }
                        command => other.push(command),
                    }
                }

                from.node.step(now).unwrap();
            }
            command => other.push(command),
        }
    }

    moved
}

#[test]
fn a_late_node_catches_up_in_batches() {
    let (mut producer, mut producer_net) = node(true);
    let (mut late, mut late_net) = node(false);
    let blocks = u64::from(MAX_SYNC_BLOCKS) + 5;

    for slot in 1..=blocks {
        producer.step(SLOT * slot).unwrap();
    }

    sent(&mut producer_net);
    let (producer_peer, late_peer) = (PeerId::random(), PeerId::random());
    late_net.events.send(NetEvent::PeerUp { peer: producer_peer, address: Multiaddr::empty() }).unwrap();
    late.step(SLOT * blocks).unwrap();
    relay(
        Side { node: &mut late, net: &mut late_net, peer: late_peer },
        Side { node: &mut producer, net: &mut producer_net, peer: producer_peer },
        SLOT * blocks,
    );

    assert_eq!(late.chain().tip().height, blocks);
    assert_eq!(late.chain().tip().id(), producer.chain().tip().id());
}

#[test]
fn a_block_from_the_future_makes_the_node_ask_its_sender() {
    let (mut producer, mut producer_net) = node(true);
    let (mut follower, mut follower_net) = node(false);

    for slot in 1..=3 {
        producer.step(SLOT * slot).unwrap();
    }

    let third = sent(&mut producer_net)
        .into_iter()
        .rev()
        .find_map(|command| match command {
            NetCommand::PublishBlock(block) => Some(block),
            _ => None,
        })
        .unwrap();
    let (producer_peer, follower_peer) = (PeerId::random(), PeerId::random());
    follower_net.events.send(NetEvent::Block { from: producer_peer, block: third }).unwrap();
    follower.step(SLOT * 3).unwrap();
    relay(
        Side { node: &mut follower, net: &mut follower_net, peer: follower_peer },
        Side { node: &mut producer, net: &mut producer_net, peer: producer_peer },
        SLOT * 3,
    );

    assert_eq!(follower.chain().tip().height, 3);
}

#[test]
fn peers_on_another_chain_are_never_asked_for_blocks() {
    let (mut follower, mut net) = node(false);
    let stranger = PeerId::random();
    net.events.send(NetEvent::PeerUp { peer: stranger, address: Multiaddr::empty() }).unwrap();
    follower.step(SLOT).unwrap();

    assert!(matches!(&sent(&mut net)[..], [NetCommand::Request { request: SyncRequest::Status, .. }]));

    net.events
        .send(NetEvent::Response {
            peer: stranger,
            response: SyncResponse::Status {
                genesis: Hash::from_bytes(HashType::default_chain(), &[1; 32]).unwrap(),
                tip: Hash::from_bytes(HashType::default_chain(), &[2; 32]).unwrap(),
                height: 50,
                work: u256::new(50),
                justified: 0,
            },
        })
        .unwrap();
    follower.step(SLOT).unwrap();

    assert!(sent(&mut net).is_empty());
}

#[test]
fn submitted_transactions_are_queued_gossiped_and_answered() {
    let (mut node, mut net) = node(false);
    let submitter = node.handle();
    let authority = DevAuthority::dev_keypair();
    let tx = SignedTx::sign(
        Tx::Transfer {
            from: authority.public().address(),
            to: Keypair::generate(authority.public().key_type()).unwrap().public().address(),
            amount: Amount::U128(1),
        },
        0,
        FeeOffer::NONE,
        &authority,
    );
    let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
    let pending = std::thread::spawn({
        let submitter = submitter.clone();
        let tx = tx.clone();

        move || runtime.block_on(submitter.submit(tx))
    });

    while node.mempool().is_empty() {
        node.step(SLOT).unwrap();
    }

    assert_eq!(pending.join().unwrap(), Ok(tx.id(HashType::default_chain())));
    assert!(sent(&mut net).iter().any(|command| matches!(command, NetCommand::PublishTx(gossiped) if *gossiped == tx)));

    let again = std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(submitter.submit(tx))
    });

    loop {
        node.step(SLOT).unwrap();

        if again.is_finished() {
            break;
        }
    }

    assert_eq!(again.join().unwrap(), Err(SubmitError::Refused(MempoolError::Duplicate)));
}

#[test]
fn a_miner_seals_on_its_own_threads_and_publishes() {
    let (link, mut net) = NetLink::pair();
    let events = net.events.clone();
    let miner = Miner::new(Address::Ed25519([9; 32]), HashType::default_chain(), 1, move |block, seal| {
        let _ = events.send(NetEvent::Sealed { block, seal });
    });
    let mut node = Node::new(Arc::new(MemoryDb::default()), &easy_pow(), Some(Box::new(miner)), link).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);

    while node.chain().tip().height < 2 {
        assert!(std::time::Instant::now() < deadline, "no block was mined");
        node.step(now_ms()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }

    let published = sent(&mut net)
        .into_iter()
        .filter(|command| matches!(command, NetCommand::PublishBlock(_)))
        .count();

    assert!(published >= 2);
    assert!(Accounts::new(&node.chain().state()).get(&Address::Ed25519([9; 32])).unwrap().balance >= u256::new(10));
}

#[test]
fn forked_nodes_settle_on_the_heavier_chain() {
    let (mut heavy, mut heavy_net) = pow_node();
    let (mut light, mut light_net) = pow_node();
    let genesis = heavy.chain().tip().clone();
    let feed = |node: &mut Node, net: &FakeNet, block: Block| {
        net.events.send(NetEvent::Block { from: PeerId::random(), block }).unwrap();
        node.step(LATER).unwrap();
    };
    let mut parent = genesis.clone();

    for _ in 0..3 {
        let block = mine_on(&heavy, &parent, 1);
        parent = block.header.clone();
        feed(&mut heavy, &heavy_net, block);
    }

    let lonely = mine_on(&light, &genesis, 2);
    feed(&mut light, &light_net, lonely);
    sent(&mut heavy_net);
    sent(&mut light_net);

    assert_eq!((heavy.chain().tip().height, light.chain().tip().height), (3, 1));

    let (heavy_peer, light_peer) = (PeerId::random(), PeerId::random());
    light_net.events.send(NetEvent::PeerUp { peer: heavy_peer, address: Multiaddr::empty() }).unwrap();
    light.step(LATER).unwrap();
    relay(
        Side { node: &mut light, net: &mut light_net, peer: light_peer },
        Side { node: &mut heavy, net: &mut heavy_net, peer: heavy_peer },
        LATER,
    );

    assert_eq!(light.chain().tip().id(), heavy.chain().tip().id());
    assert_eq!(Accounts::new(&light.chain().state()).get(&Address::Ed25519([2; 32])).unwrap().balance, u256::ZERO);
}

// Finality. The dev chains run FFG with the dev key as their one validator, epochs of 8 blocks, and
// votes two blocks behind the tip; the authority dev settings time votes the same way.
#[cfg(any(feature = "finality-ffg", feature = "finality-authority", feature = "finality-depth"))]
mod finality {
    use super::*;
    use crate::chain::FinalityConfig;

    #[cfg(any(feature = "finality-ffg", feature = "finality-authority"))]
    const EPOCH: u64 = 8;

    /// A producer on the dev authority chain with this finality instead, voting with the dev key.
    #[cfg(any(feature = "finality-authority", feature = "finality-depth"))]
    fn validating_on(finality: FinalityConfig) -> (Node, FakeNet) {
        let authority = DevAuthority::dev_keypair();
        let genesis = Genesis {
            finality: Some(finality),
            fees: None,
            ..Genesis::dev_authority(authority.public())
        };
        let producer = Box::new(AuthorityProducer::new(authority.clone(), SLOT)) as Box<dyn Producer>;
        let (link, fake) = NetLink::pair();
        let node = Node::new(Arc::new(MemoryDb::default()), &genesis, Some(producer), link).unwrap();

        (node.validate(authority), fake)
    }

    fn votes(commands: &[NetCommand]) -> Vec<crate::structs::vote::SignedVote> {
        commands
            .iter()
            .filter_map(|command| match command {
                NetCommand::PublishVote(vote) => Some(*vote),
                _ => None,
            })
            .collect()
    }

    #[cfg(feature = "finality-ffg")]
    fn validating_producer() -> (Node, FakeNet) {
        let (producer, net) = node(true);

        (producer.validate(DevAuthority::dev_keypair()), net)
    }

    #[cfg(feature = "finality-ffg")]
    #[test]
    fn a_validating_producer_votes_once_per_epoch_and_finalizes() {
        let (mut producer, mut net) = validating_producer();
        let dev = DevAuthority::dev_keypair().public();
        let funds = Accounts::new(&producer.chain().state()).get(&dev.address()).unwrap().balance;

        for slot in 1..=EPOCH + 1 {
            producer.step(SLOT * slot).unwrap();
        }

        assert!(votes(&sent(&mut net)).is_empty(), "too close to the tip to vote yet");
        assert_eq!(producer.chain().justified().unwrap().epoch, 0);

        producer.step(SLOT * (EPOCH + 2)).unwrap();
        let first = votes(&sent(&mut net));

        assert_eq!(first.len(), 1);
        assert_eq!((first[0].vote.source.epoch, first[0].vote.target.epoch), (0, 1));
        assert_eq!(producer.chain().justified().unwrap().epoch, 1);

        // The next block pays for the proof, and nothing more is owed until the next epoch.
        producer.step(SLOT * (EPOCH + 3)).unwrap();
        let paid = published_block(&mut net);

        assert_eq!(paid.proofs.len(), 1);
        assert_eq!(paid.proofs[0].target, first[0].vote.target);
        assert_eq!(
            Accounts::new(&producer.chain().state()).get(&dev.address()).unwrap().balance,
            funds + match producer.chain().finality_config() {
                Some(FinalityConfig::Ffg(ffg)) => ffg.vote_reward,
                _ => unreachable!("the dev chains run FFG"),
            }
        );

        for slot in EPOCH + 4..=2 * EPOCH + 2 {
            producer.step(SLOT * slot).unwrap();
        }

        let second = votes(&sent(&mut net));

        assert_eq!(second.len(), 1);
        assert_eq!((second[0].vote.source.epoch, second[0].vote.target.epoch), (1, 2));
        assert_eq!(producer.chain().finalized().unwrap(), first[0].vote.target);
    }

    #[cfg(feature = "finality-ffg")]
    #[test]
    fn a_follower_counts_gossiped_votes() {
        let (mut producer, mut producer_net) = validating_producer();
        let (mut follower, mut follower_net) = node(false);
        let peer = PeerId::random();

        for slot in 1..=EPOCH + 2 {
            producer.step(SLOT * slot).unwrap();

            for command in sent(&mut producer_net) {
                let event = match command {
                    NetCommand::PublishBlock(block) => NetEvent::Block { from: peer, block },
                    NetCommand::PublishVote(vote) => NetEvent::Vote { from: peer, vote },
                    _ => continue,
                };
                follower_net.events.send(event).unwrap();
            }

            follower.step(SLOT * slot).unwrap();
        }

        assert_eq!(follower.chain().justified(), producer.chain().justified());
        assert_eq!(follower.chain().justified().unwrap().epoch, 1);
        assert!(votes(&sent(&mut follower_net)).is_empty(), "a follower never votes");
    }

    #[cfg(feature = "finality-ffg")]
    #[test]
    fn a_late_node_syncs_justifications_with_the_blocks() {
        let (mut producer, mut producer_net) = validating_producer();
        let (mut late, mut late_net) = node(false);
        let blocks = 3 * EPOCH + 2;

        for slot in 1..=blocks {
            producer.step(SLOT * slot).unwrap();
        }

        sent(&mut producer_net);

        assert_eq!(producer.chain().justified().unwrap().epoch, 3);

        let (producer_peer, late_peer) = (PeerId::random(), PeerId::random());
        late_net.events.send(NetEvent::PeerUp { peer: producer_peer, address: Multiaddr::empty() }).unwrap();
        late.step(SLOT * blocks).unwrap();
        relay(
            Side { node: &mut late, net: &mut late_net, peer: late_peer },
            Side { node: &mut producer, net: &mut producer_net, peer: producer_peer },
            SLOT * blocks,
        );

        assert_eq!(late.chain().tip().id(), producer.chain().tip().id());
        assert_eq!(late.chain().justified(), producer.chain().justified());
        assert_eq!(late.chain().finalized(), producer.chain().finalized());
    }

    #[cfg(feature = "finality-ffg")]
    #[test]
    fn peers_get_justifications() {
        let (mut producer, mut net) = validating_producer();

        for slot in 1..=2 * EPOCH + 2 {
            producer.step(SLOT * slot).unwrap();
        }

        sent(&mut net);
        net.events
            .send(NetEvent::Request {
                peer: PeerId::random(),
                id: 7,
                request: SyncRequest::Justifications { from: 1, count: 100 },
            })
            .unwrap();
        producer.step(SLOT * (2 * EPOCH + 2) + 1).unwrap();

        assert!(matches!(
            &sent(&mut net)[..],
            [NetCommand::Respond {
                id: 7,
                response: SyncResponse::Justifications(proofs),
            }] if proofs.iter().map(|proof| proof.target.epoch).eq([1, 2])
        ));
    }

    #[cfg(feature = "finality-authority")]
    #[test]
    fn an_authority_signer_votes_and_finalizes_without_proofs() {
        let dev = DevAuthority::dev_keypair().public();
        let (mut producer, mut net) = validating_on(FinalityConfig::Authority(crate::chain::AuthorityConfig::dev(dev)));
        let mut published = Vec::new();

        for slot in 1..=2 * EPOCH + 2 {
            producer.step(SLOT * slot).unwrap();
            published.extend(sent(&mut net));
        }

        let cast = votes(&published);

        assert_eq!(cast.len(), 2);
        assert_eq!((cast[1].vote.source.epoch, cast[1].vote.target.epoch), (1, 2));
        assert_eq!(producer.chain().finalized().unwrap(), cast[0].vote.target);
        // Signers earn nothing in the dev settings, so blocks carry no proofs.
        assert!(
            published
                .iter()
                .all(|command| !matches!(command, NetCommand::PublishBlock(block) if !block.proofs.is_empty()))
        );
    }

    #[cfg(feature = "finality-depth")]
    #[test]
    fn a_depth_chain_finalizes_without_votes() {
        let (mut producer, mut net) = validating_on(FinalityConfig::Depth(crate::chain::DepthConfig { depth: 3 }));

        for slot in 1..=10 {
            producer.step(SLOT * slot).unwrap();
        }

        assert!(votes(&sent(&mut net)).is_empty(), "nothing to vote on");
        assert_eq!(producer.chain().finalized().unwrap().epoch, 7);
    }
}

#[test]
fn the_report_counts_what_peers_sent_and_shows_what_waits() {
    let (mut node, mut net) = node(false);
    let authority = DevAuthority::dev_keypair();
    let peer = PeerId::random();
    let address: Multiaddr = "/ip4/10.0.0.1/udp/1/quic-v1".parse().unwrap();
    let tx = SignedTx::sign(
        Tx::Burn {
            from: authority.public().address(),
            amount: Amount::U128(1),
        },
        0,
        FeeOffer::NONE,
        &authority,
    );
    net.events.send(NetEvent::Listening("/ip4/127.0.0.1/udp/9/quic-v1".parse().unwrap())).unwrap();
    net.events.send(NetEvent::PeerUp { peer, address: address.clone() }).unwrap();
    net.events.send(NetEvent::Tx { from: peer, tx: tx.clone() }).unwrap();
    net.events.send(NetEvent::Tx { from: peer, tx: tx.clone() }).unwrap();
    node.step(SLOT).unwrap();
    sent(&mut net);
    let report = node.report(SLOT * 2, 10);

    assert_eq!((report.started, report.now), (SLOT, SLOT * 2));
    assert_eq!(report.listening.len(), 1);
    assert_eq!(report.peers.len(), 1);
    assert_eq!((report.peers[0].id, &report.peers[0].address, report.peers[0].since), (peer, &address, SLOT));
    assert_eq!(report.peers[0].heard, Heard { blocks: 0, txs: 2, votes: 0 });
    assert_eq!(report.peers[0].chain, None);
    assert_eq!(report.waiting, 1);
    assert_eq!(report.txs, [tx]);
    assert_eq!(
        (report.counters.txs_heard, report.counters.txs_refused, report.counters.connections, report.counters.requests_sent),
        (2, 1, 1, 1)
    );
    assert!(node.report(SLOT * 2, 0).txs.is_empty());
}

#[test]
fn a_new_node_syncs_from_a_snapshot_and_runs_only_the_blocks_after_it() {
    use crate::db::stored_root;
    use super::snapshot::PIVOT_DEPTH;

    let (mut producer, mut producer_net) = node(true);
    let authority = DevAuthority::dev_keypair();
    let blocks = 60;
    let mut paid = Vec::new();

    // Transfers to new accounts along the way, so the state is worth taking.
    for slot in 1..=blocks {
        if slot % 2 == 0 {
            let to = Keypair::generate(authority.public().key_type()).unwrap().public().address();
            let tx = Tx::Transfer {
                from: authority.public().address(),
                to,
                amount: Amount::U128(u128::from(slot)),
            };
            producer.submit(SignedTx::sign(tx, slot / 2 - 1, FeeOffer::NONE, &authority)).unwrap();
            paid.push((to, slot));
        }

        producer.step(SLOT * slot).unwrap();
    }

    sent(&mut producer_net);
    let (mut late, mut late_net) = node(false);

    assert!(late.sync_from_snapshot(None).unwrap());

    let (producer_peer, late_peer) = (PeerId::random(), PeerId::random());
    late_net.events.send(NetEvent::PeerUp { peer: producer_peer, address: Multiaddr::empty() }).unwrap();
    late.step(SLOT * blocks).unwrap();
    relay(
        Side { node: &mut late, net: &mut late_net, peer: late_peer },
        Side { node: &mut producer, net: &mut producer_net, peer: producer_peer },
        SLOT * blocks,
    );
    let pivot = blocks - PIVOT_DEPTH;

    assert_eq!(late.chain().tip().id(), producer.chain().tip().id(), "it ran the blocks after the pivot");
    assert_eq!(late.chain().snapshot(), Some(pivot));
    assert_eq!(stored_root(&**late.chain().db(), late.chain().hash()).unwrap(), late.chain().tip().state_root);

    for (to, amount) in &paid {
        assert_eq!(Accounts::new(&late.chain().state()).get(to).unwrap().balance, u256::new(u128::from(*amount)));
    }

    // It never ran the blocks before the pivot: it keeps their headers only.
    assert!(late.chain().block_at(pivot - 1).unwrap().is_none());
    assert!(late.chain().block_at(pivot).unwrap().is_some());
    assert_eq!(late.chain().headers_from(1, 1_000).unwrap().len() as u64, blocks);
}

// The dev chain's FFG finality makes height 40 a checkpoint that opens an epoch; without it,
// any block is one and the start holds that header alone.
#[cfg(feature = "finality-ffg")]
#[test]
fn a_node_starting_at_a_checkpoint_fetches_nothing_below_it_and_holds_it_final() {
    use super::snapshot::PIVOT_DEPTH;
    use crate::chain::{BlockError, ImportError};
    use crate::structs::vote::Checkpoint;

    let (mut producer, mut producer_net) = node(true);
    let authority = DevAuthority::dev_keypair();
    let blocks = 60;

    for slot in 1..=blocks {
        if slot % 2 == 0 {
            let to = Keypair::generate(authority.public().key_type()).unwrap().public().address();
            let tx = Tx::Transfer { from: authority.public().address(), to, amount: Amount::U128(u128::from(slot)) };
            producer.submit(SignedTx::sign(tx, slot / 2 - 1, FeeOffer::NONE, &authority)).unwrap();
        }

        producer.step(SLOT * slot).unwrap();
    }

    sent(&mut producer_net);
    // Epochs are 8 blocks long on the dev chain: height 40 opens epoch 5. The operator trusts it.
    let trusted = producer.chain().block_at(40).unwrap().unwrap().id();
    let (mut late, mut late_net) = node(false);

    assert!(late.sync_from_snapshot(Some(trusted)).unwrap());
    assert_eq!(late.sync.snapshot.as_ref().unwrap().stage(), "fetching the checkpoint");

    let (producer_peer, late_peer) = (PeerId::random(), PeerId::random());
    late_net.events.send(NetEvent::PeerUp { peer: producer_peer, address: Multiaddr::empty() }).unwrap();
    late.step(SLOT * blocks).unwrap();
    relay(
        Side { node: &mut late, net: &mut late_net, peer: late_peer },
        Side { node: &mut producer, net: &mut producer_net, peer: producer_peer },
        SLOT * blocks,
    );
    let pivot = blocks - PIVOT_DEPTH;

    assert_eq!(late.chain().tip().id(), producer.chain().tip().id(), "it ran the blocks after the pivot");
    assert_eq!(late.chain().snapshot(), Some(pivot));
    assert_eq!(late.chain().tip_work(), producer.chain().tip_work(), "its work counts from the peer's figure at the checkpoint");
    assert_eq!(late.chain().finalized(), Some(Checkpoint { epoch: 5, block: trusted }));
    assert_eq!(late.chain().justified(), Some(Checkpoint { epoch: 5, block: trusted }));

    // The start reaches back to the checkpoint activation_delay epochs before, height 32, and no
    // further: nothing below is fetched, and the chain never goes there.
    assert!(late.chain().headers_from(31, 1).unwrap().is_empty());
    assert_eq!(late.chain().headers_from(32, 1_000).unwrap().len() as u64, blocks - 32 + 1);
    assert!(late.chain().block_at(pivot - 1).unwrap().is_none());

    let below = producer.chain().block_at(39).unwrap().unwrap();

    assert!(matches!(late.chain.import(&below, SLOT * blocks), Err(ImportError::Invalid(BlockError::ConflictsWithFinality))));
}

/// Syncs a late node from a snapshot of a chain whose producers finality names, from genesis or
/// from a trusted checkpoint, and checks it got there: the producer votes, so finality's updates
/// name each epoch's producers to the late node's light client.
#[cfg(any(feature = "finality-authority", feature = "finality-ffg"))]
fn snapshot_of_a_chain_with_producers_from_finality(genesis: Genesis, producer: Box<dyn Producer>, checkpoint: Option<u64>) {
    use super::snapshot::PIVOT_DEPTH;

    let dev = DevAuthority::dev_keypair();
    let (link, mut producer_net) = NetLink::pair();
    let mut producer = Node::new(Arc::new(MemoryDb::default()), &genesis, Some(producer), link).unwrap().validate(dev.clone());
    let blocks = 60;
    let mut paid = Vec::new();

    for slot in 1..=blocks {
        if slot % 2 == 0 {
            let to = Keypair::generate(dev.public().key_type()).unwrap().public().address();
            let tx = Tx::Transfer { from: dev.public().address(), to, amount: Amount::U128(u128::from(slot)) };
            producer.submit(SignedTx::sign(tx, slot / 2 - 1, FeeOffer::NONE, &dev)).unwrap();
            paid.push((to, slot));
        }

        producer.step(SLOT * slot).unwrap();
    }

    assert_eq!(producer.chain().tip().height, blocks);
    assert_eq!(producer.chain().justified().unwrap().epoch, 7, "it voted for every checkpoint two blocks deep");
    sent(&mut producer_net);
    let trusted = checkpoint.map(|height| producer.chain().block_at(height).unwrap().unwrap().id());
    let (link, mut late_net) = NetLink::pair();
    let mut late = Node::new(Arc::new(MemoryDb::default()), &genesis, None, link).unwrap();

    assert!(late.sync_from_snapshot(trusted).unwrap());

    let (producer_peer, late_peer) = (PeerId::random(), PeerId::random());
    late_net.events.send(NetEvent::PeerUp { peer: producer_peer, address: Multiaddr::empty() }).unwrap();
    late.step(SLOT * blocks).unwrap();
    relay(
        Side { node: &mut late, net: &mut late_net, peer: late_peer },
        Side { node: &mut producer, net: &mut producer_net, peer: producer_peer },
        SLOT * blocks,
    );
    let pivot = blocks - PIVOT_DEPTH;

    assert_eq!(late.chain().tip().id(), producer.chain().tip().id(), "it ran the blocks after the pivot");
    assert_eq!(late.chain().snapshot(), Some(pivot));
    assert_eq!(late.chain().tip_work(), producer.chain().tip_work());
    assert!(late.chain().block_at(pivot - 1).unwrap().is_none(), "it never ran the blocks before the pivot");

    for (to, amount) in &paid {
        assert_eq!(Accounts::new(&late.chain().state()).get(to).unwrap().balance, u256::new(u128::from(*amount)));
    }

    match checkpoint {
        // The start reaches back to the checkpoint activation_delay epochs before: from epoch 5's
        // to epoch 3's, height 32 for 40, and no further.
        Some(height) => {
            assert!(late.chain().headers_from(height - 9, 1).unwrap().is_empty(), "nothing below the start");
            assert_eq!(late.chain().headers_from(height - 8, 1).unwrap().len(), 1, "the start reaches back activation_delay epochs");
        }
        None => assert_eq!(late.chain().headers_from(1, 1_000).unwrap().len() as u64, blocks),
    }

    // The late node goes on producing nothing, but checks the producer's next blocks against the
    // producers it holds in state now.
    for slot in blocks + 1..=blocks + 3 {
        producer.step(SLOT * slot).unwrap();
        let block = published_block(&mut producer_net);
        late_net.events.send(NetEvent::Block { from: producer_peer, block }).unwrap();
        late.step(SLOT * slot).unwrap();
    }

    assert_eq!(late.chain().tip().height, blocks + 3);
}

#[cfg(feature = "finality-authority")]
#[test]
fn a_rotating_chain_syncs_from_a_snapshot_by_learning_its_signers_from_finality() {
    use crate::consensus::RotatingProducer;

    let dev = DevAuthority::dev_keypair();
    let genesis = Genesis { fees: None, ..Genesis::dev_rotating(dev.public()) };
    let producer = || Box::new(RotatingProducer::new(dev.clone(), dev.public().address(), SLOT)) as Box<dyn Producer>;

    snapshot_of_a_chain_with_producers_from_finality(genesis.clone(), producer(), None);
    snapshot_of_a_chain_with_producers_from_finality(genesis, producer(), Some(40));
}

#[cfg(feature = "finality-ffg")]
#[test]
fn a_staked_chain_syncs_from_a_snapshot_by_learning_its_validators_from_finality() {
    use crate::consensus::{Schedule, Staked, StakedProducer};

    let dev = DevAuthority::dev_keypair();
    let genesis = Genesis { fees: None, ..Genesis::dev_staked(dev.public()) };
    let hash = genesis.hash;
    let producer = || Box::new(StakedProducer::new(dev.clone(), Staked::new(SLOT, Schedule::Draw, hash, 8))) as Box<dyn Producer>;

    snapshot_of_a_chain_with_producers_from_finality(genesis.clone(), producer(), None);
    snapshot_of_a_chain_with_producers_from_finality(genesis, producer(), Some(40));
}

#[cfg(all(feature = "finality-ffg", feature = "governance-council"))]
#[test]
fn a_node_switches_its_producer_when_governance_changes_the_consensus() {
    use crate::consensus::Schedule;
    use crate::governance::{Change, GovernanceConfig};

    let dev = DevAuthority::dev_keypair();
    // The dev key is the authority, the one FFG validator, and the council: it decides alone.
    let genesis = Genesis {
        fees: None,
        governance: Some(GovernanceConfig::Council {
            members: vec![dev.public().address()],
            threshold: 1,
            approval_epochs: 64,
        }),
        ..Genesis::dev_authority(dev.public())
    };
    let staked = ConsensusConfig::Staked { slot_ms: SLOT, schedule: Schedule::Draw };
    let (link, mut net) = NetLink::pair();
    let mut node = Node::new(Arc::new(MemoryDb::default()), &genesis, None, link)
        .unwrap()
        .producing(Producing { mine: None, sign: Some(dev.clone()) })
        .validate(dev.clone());
    let (link, follower_net) = NetLink::pair();
    let mut follower = Node::new(Arc::new(MemoryDb::default()), &genesis, None, link).unwrap();
    let peer = PeerId::random();
    let relay = |follower: &mut Node, net: &mut FakeNet, now: u64| {
        for block in sent(net).into_iter().filter_map(|command| match command {
            NetCommand::PublishBlock(block) => Some(block),
            _ => None,
        }) {
            follower_net.events.send(NetEvent::Block { from: peer, block }).unwrap();
            follower.step(now).unwrap();
        }
    };

    // Three authority blocks, then the council approves stake in block 4, a checkpoint under
    // the dev chain's finality of 8-block epochs: fixed for epoch 1 + 2, from height 24 on.
    for slot in 1..=3 {
        node.step(SLOT * slot).unwrap();
        relay(&mut follower, &mut net, SLOT * slot);
    }

    assert_eq!(node.chain().tip().difficulty, 0);
    assert_eq!(node.chain().tip_work(), u256::new(3), "authority blocks weigh 1");
    node.submit(SignedTx::sign(Tx::ApproveChange { version: 0, change: Change::Consensus(staked.clone()) }, 0, FeeOffer::NONE, &dev)).unwrap();

    for slot in 4..=30 {
        node.step(SLOT * slot).unwrap();
        relay(&mut follower, &mut net, SLOT * slot);
    }

    assert_eq!(node.chain().tip().height, 30, "it produced through the switch without a pause");
    assert_eq!(node.chain().consensus_config_at(23).unwrap(), genesis.consensus);
    assert_eq!(node.chain().consensus_config_at(24).unwrap(), staked);
    assert_eq!(node.chain().block_at(4).unwrap().unwrap().txs.len(), 1);
    assert_eq!(node.chain().tip_work(), u256::new(23 + 7 * 2), "23 authority blocks weighing 1, then 7 staked ones in turn weighing 2");
    assert_eq!(follower.chain().tip().id(), node.chain().tip().id(), "a following node checks the new blocks by the fixed consensus too");
}

#[test]
fn a_snapshot_whose_pivot_grows_too_old_moves_it_up_and_fetches_only_what_changed() {
    let authority = DevAuthority::dev_keypair();
    // Undo records for 20 blocks only, so peers serve state 20 blocks back at most.
    let genesis = Genesis {
        fees: None,
        max_reorg_depth: 20,
        ..Genesis::dev_authority(authority.public())
    };
    let start = |produce: bool| {
        let producer = produce.then(|| Box::new(AuthorityProducer::new(authority.clone(), SLOT)) as Box<dyn Producer>);
        let (link, fake) = NetLink::pair();

        (Node::new(Arc::new(MemoryDb::default()), &genesis, producer, link).unwrap(), fake)
    };
    let (mut producer, mut producer_net) = start(true);
    let (mut late, mut late_net) = start(false);
    // A transfer to a new account in every block, so the state the pivot moves to differs from
    // what the late node holds in every build, finality's records or not.
    let pay = |producer: &mut Node, slot: u64| {
        let to = Keypair::generate(authority.public().key_type()).unwrap().public().address();
        let tx = Tx::Transfer {
            from: authority.public().address(),
            to,
            amount: Amount::U128(u128::from(slot)),
        };
        producer.submit(SignedTx::sign(tx, slot - 1, FeeOffer::NONE, &authority)).unwrap();
    };

    for slot in 1..=60 {
        pay(&mut producer, slot);
        producer.step(SLOT * slot).unwrap();
    }

    sent(&mut producer_net);
    assert!(late.sync_from_snapshot(None).unwrap());
    let (producer_peer, late_peer) = (PeerId::random(), PeerId::random());
    late_net.events.send(NetEvent::PeerUp { peer: producer_peer, address: Multiaddr::empty() }).unwrap();
    late.step(SLOT * 60).unwrap();

    // Headers first; then the chain runs on 30 blocks before any state is asked for.
    while late.sync.snapshot.as_ref().is_some_and(|snapshot| snapshot.stage().starts_with("checking headers")) {
        let mut other = Vec::new();
        let moved = pass(
            &mut Side { node: &mut late, net: &mut late_net, peer: late_peer },
            &mut Side { node: &mut producer, net: &mut producer_net, peer: producer_peer },
            SLOT * 60,
            &mut other,
        );
        assert!(moved, "it keeps asking until it picks a pivot");
    }

    assert_eq!(late.sync.snapshot.as_ref().unwrap().stage(), "taking state at height 50, 0 entries so far");

    for slot in 61..=90 {
        pay(&mut producer, slot);
        producer.step(SLOT * slot).unwrap();
    }

    sent(&mut producer_net);
    relay(
        Side { node: &mut late, net: &mut late_net, peer: late_peer },
        Side { node: &mut producer, net: &mut producer_net, peer: producer_peer },
        SLOT * 90,
    );

    assert_eq!(late.chain().snapshot(), Some(80), "the pivot moved up, 10 below the new tip");
    assert_eq!(late.chain().tip().id(), producer.chain().tip().id());
}
