use super::*;
use crate::net::{MAX_SYNC_BLOCKS, SyncRequest, SyncResponse};
use crate::chain::Reward;
use crate::consensus::{AuthorityProducer, ConsensusConfig, DevAuthority, Miner, PowConfig, PowHash, meets, work_hash};
use crate::structs::address::Address;
use crate::structs::block::{BlockHeader, Seal};
use crate::crypto::keypair::Keypair;
use crate::db::MemoryDb;
use crate::net::FakeNet;
use crate::registry::Accounts;
use crate::structs::amount::Amount;
use crate::structs::block::Block;
use crate::structs::tx::Tx;
use crate::structs::u256::u256;
use libp2p::PeerId;

const SLOT: u64 = 2_000;

fn node(produce: bool) -> (Node, FakeNet) {
    let authority = DevAuthority::dev_keypair();
    let genesis = Genesis::dev_authority(authority.public());
    let producer = produce.then(|| Box::new(AuthorityProducer::new(authority, SLOT)) as Box<dyn Producer>);
    let (link, fake) = NetLink::pair();
    let node = Node::new(Arc::new(MemoryDb::default()), &genesis, producer, link).unwrap();

    (node, fake)
}

/// A proof-of-work chain where any nonce does, so tests mine instantly.
fn easy_pow() -> Genesis {
    Genesis {
        consensus: ConsensusConfig::ProofOfWork(PowConfig {
            hash: PowHash::Blake3,
            target_block_ms: 1_000,
            initial_difficulty: 1,
            min_difficulty: 1,
        }),
        reward: Reward {
            initial: u256::new(5),
            halving_interval: None,
        },
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
    let header = BlockHeader {
        parent: parent.id(),
        height: parent.height + 1,
        timestamp,
        producer: Address::Ed25519([miner; 32]),
        difficulty: node.chain().consensus().difficulty(parent, timestamp),
        tx_root: Block::tx_root(&[]),
    };
    let nonce = (0..)
        .find(|nonce| meets(work_hash(PowHash::Blake3, &header.id(), *nonce), header.difficulty))
        .unwrap();

    Block {
        header,
        txs: Vec::new(),
        seal: Seal::Work { nonce },
    }
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
    late_net.events.send(NetEvent::PeerUp(producer_peer)).unwrap();
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
    net.events.send(NetEvent::PeerUp(stranger)).unwrap();
    follower.step(SLOT).unwrap();

    assert!(matches!(&sent(&mut net)[..], [NetCommand::Request { request: SyncRequest::Status, .. }]));

    net.events
        .send(NetEvent::Response {
            peer: stranger,
            response: SyncResponse::Status {
                genesis: crate::structs::hash::Hash::Blake3_256([1; 32]),
                tip: crate::structs::hash::Hash::Blake3_256([2; 32]),
                height: 50,
                work: u256::new(50),
            },
        })
        .unwrap();
    follower.step(SLOT).unwrap();

    assert!(sent(&mut net).is_empty());
}

#[test]
fn submitted_transactions_are_queued_gossiped_and_answered() {
    let (mut node, mut net) = node(false);
    let submitter = node.submitter();
    let authority = DevAuthority::dev_keypair();
    let tx = SignedTx::sign(
        Tx::Transfer {
            from: authority.public().address(),
            to: Keypair::generate(authority.public().key_type()).unwrap().public().address(),
            amount: Amount::U128(1),
        },
        0,
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

    assert_eq!(pending.join().unwrap(), Ok(tx.id()));
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
    let miner = Miner::new(Address::Ed25519([9; 32]), PowHash::Blake3, 1, move |block, seal| {
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
    light_net.events.send(NetEvent::PeerUp(heavy_peer)).unwrap();
    light.step(LATER).unwrap();
    relay(
        Side { node: &mut light, net: &mut light_net, peer: light_peer },
        Side { node: &mut heavy, net: &mut heavy_net, peer: heavy_peer },
        LATER,
    );

    assert_eq!(light.chain().tip().id(), heavy.chain().tip().id());
    assert_eq!(Accounts::new(&light.chain().state()).get(&Address::Ed25519([2; 32])).unwrap().balance, u256::ZERO);
}
