use super::*;
use crate::chain::{Chain, Genesis};
use crate::consensus::{AuthorityProducer, DevAuthority, Producer};
use crate::crypto::keypair::Keypair;
use crate::db::MemoryDb;
use crate::registry::contracts::contract_address;
use crate::structs::amount::Amount;
use crate::structs::code::Code;
use crate::net::{Backlog, BacklogLimits, NetEvent};
use crate::node::{Counters, Heard, NodeReport, PeerReport};
use crate::structs::block::Block;
use crate::structs::hash::HashType;
use crate::structs::hex::{from_hex, to_hex};
use crate::structs::tx::{FeeOffer, SignedTx, Tx, TxCallArgumentType};
use std::io::{Read as _, Write as _};
use std::net::TcpStream;

const SLOT: u64 = 2_000;

const COUNTER: &str = r#"
--@namespace counter text -> number:clicks
local M = {}

function M.increment(by)
    return by
end

return M
"#;

struct Fixture {
    address: SocketAddr,
    chain: Chain,
    bob: Address,
}

/// The dev authority chain without fees, so balances move only as the tests say. Fees have tests
/// of their own.
fn free() -> Genesis {
    Genesis {
        fees: None,
        ..Genesis::dev_authority(DevAuthority::dev_keypair().public())
    }
}

/// A chain with a transfer to bob in block 1 and a deploy in block 2, served on a free port.
fn serve() -> Fixture {
    let authority = DevAuthority::dev_keypair();
    let mut producer = AuthorityProducer::new(authority.clone(), SLOT);
    let db = Arc::new(MemoryDb::default());
    let mut chain = Chain::open(db.clone(), &free()).unwrap();
    let genesis_id = chain.genesis();
    let bob = Keypair::generate(authority.public().key_type()).unwrap().public().address();
    let transfer = Tx::Transfer {
        from: authority.public().address(),
        to: bob,
        amount: Amount::U128(100),
    };
    let deploy = Tx::Deploy {
        code: Code::Lua(COUNTER.to_string()),
    };

    for (nonce, tx) in [transfer, deploy].into_iter().enumerate() {
        let now = chain.tip().timestamp + SLOT;
        let signed = SignedTx::sign(tx, nonce as u64, FeeOffer::NONE, &authority, &genesis_id);
        let mut block = chain.build(vec![signed], Vec::new(), producer.address(&[]), now).unwrap();
        block.seal = producer.seal(&block.header).unwrap();
        chain.import(&block, now).unwrap();
    }

    let genesis = free();
    let address = start(db, Some(fake_node()), &genesis, "127.0.0.1:0".parse().unwrap(), ApiOptions::default()).unwrap();

    Fixture { address, chain, bob }
}

/// A handle to a node that takes every transaction except those with nonce 99, which it calls
/// stale, and reports one peer and two transactions waiting.
fn fake_node() -> NodeHandle {
    fake_node_with(BacklogLimits::default())
}

fn fake_node_with(limits: BacklogLimits) -> NodeHandle {
    fake_node_holding(limits, None)
}

/// A fake node whose mempool, as it reports it, holds these transactions in place of its usual two.
fn fake_node_holding(limits: BacklogLimits, waiting: Option<Vec<SignedTx>>) -> NodeHandle {
    let (inbox, events) = std::sync::mpsc::channel();
    let backlog = Arc::new(Backlog::new(limits));
    let handled = backlog.clone();

    std::thread::spawn(move || {
        for event in events {
            match event {
                NetEvent::Submitted { tx, reply } => {
                    handled.release(crate::net::Origin::Local);
                    // Nonces from 95 up stand for each refusal the mempool's limits make.
                    let verdict = match tx.nonce {
                        99 => Err(MempoolError::Stale),
                        98 => Err(MempoolError::TooMany { most: 64 }),
                        97 => Err(MempoolError::ReplacementTooLow { bump: 10 }),
                        96 => Err(MempoolError::TooFarAhead { next: 3, most: 64 }),
                        95 => Err(MempoolError::Full),
                        _ => Ok(tx.id(HashType::default_chain())),
                    };
                    let _ = reply.send(verdict);
                }
                // Lifts as many as asked for: all two it holds, or one.
                NetEvent::Unban { which, reply } => {
                    let _ = reply.send(if which == crate::net::Unban::All { 2 } else { 1 });
                }
                NetEvent::Report { txs, reply } => {
                    let mut report = fake_report(txs);

                    if let Some(waiting) = &waiting {
                        report.txs = waiting.iter().take(txs).cloned().collect();
                    }

                    let _ = reply.send(report);
                }
                #[cfg(feature = "ethereum")]
                NetEvent::MempoolAsk { query, reply } => {
                    use crate::node::{MempoolAnswer, MempoolQuery};

                    let txs = waiting.clone().unwrap_or_else(|| vec![signed(0), signed(1)]);
                    let answer = match query {
                        MempoolQuery::HighestNonce(account) => MempoolAnswer::HighestNonce(txs.iter().filter(|tx| tx.sender() == account).map(|tx| tx.nonce).max()),
                        MempoolQuery::EthereumByHash(hash) => MempoolAnswer::Ethereum(txs.into_iter().find(|tx| matches!(&tx.tx, crate::structs::tx::Tx::Ethereum { raw } if crate::crypto::ethereum::tx_hash(raw) == hash)).map(Box::new)),
                    };

                    let _ = reply.send(answer);
                }
                _ => {}
            }
        }
    });

    NodeHandle::new(inbox, backlog)
}

fn fake_report(txs: usize) -> NodeReport {
    NodeReport {
        started: 1_000,
        now: 61_000,
        listening: vec!["/ip4/127.0.0.1/udp/4001/quic-v1".parse().unwrap()],
        producing: true,
        validating: false,
        mining: Some(crate::consensus::MiningReport {
            devices: vec![crate::consensus::DeviceReport { name: "CPU, 2 threads".to_string(), rate: 1_000.0 }],
            listening: Some("0.0.0.0:4002".to_string()),
            workers: vec![crate::consensus::WorkerReport {
                name: "rig".to_string(),
                address: "10.0.0.3:50000".to_string(),
                since: 3_000,
                devices: vec![
                    crate::consensus::DeviceReport { name: "GPU 0".to_string(), rate: 200e6 },
                    crate::consensus::DeviceReport { name: "GPU 1".to_string(), rate: 100e6 },
                ],
                found: 4,
                refused: 1,
                payout: None,
                shares: 0,
                share_difficulty: 0,
                solo: false,
            }],
            pool: None,
            ..Default::default()
        }),
        peers: vec![PeerReport {
            id: libp2p::PeerId::random(),
            address: "/ip4/10.0.0.2/udp/4001/quic-v1".parse().unwrap(),
            since: 2_000,
            heard: Heard { blocks: 3, txs: 4, votes: 0 },
            dropped: 7,
            chain: Some((5, crate::structs::u256::u256::new(5), 0)),
        }],
        syncing: None,
        waiting: 2,
        capacity: 50_000,
        txs: [signed(0), signed(1)].into_iter().take(txs).collect(),
        counters: Counters {
            blocks_heard: 3,
            txs_heard: 4,
            gossip_dropped: 7,
            ..Counters::default()
        },
        backlog: (12, BacklogLimits::default()),
    }
}

/// Produces the next block on a chain with these transactions.
fn produce(chain: &mut Chain, txs: Vec<SignedTx>) -> Block {
    let mut producer = AuthorityProducer::new(DevAuthority::dev_keypair(), SLOT);
    let now = chain.tip().timestamp + SLOT;
    let mut block = chain.build(txs, Vec::new(), producer.address(&[]), now).unwrap();
    block.seal = producer.seal(&block.header).unwrap();
    chain.import(&block, now).unwrap();

    block
}

/// Sends a request with extra headers and reads the status, the headers and the raw body.
fn raw(address: SocketAddr, method: &str, path: &str, headers: &str) -> (u16, String, Vec<u8>) {
    let mut stream = TcpStream::connect(address).unwrap();
    write!(stream, "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n{headers}\r\n").unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).unwrap();
    let split = response.windows(4).position(|window| window == b"\r\n\r\n").unwrap();
    let head = String::from_utf8(response[..split].to_vec()).unwrap().to_ascii_lowercase();
    let status = head.split(' ').nth(1).unwrap().parse().unwrap();

    (status, head, response[split + 4..].to_vec())
}

/// Sends a GET and reads the status, content type and JSON body of the answer.
fn get(address: SocketAddr, path: &str) -> (u16, String, Value) {
    request(address, "GET", path, "")
}

fn request(address: SocketAddr, method: &str, path: &str, body: &str) -> (u16, String, Value) {
    let mut stream = TcpStream::connect(address).unwrap();
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    let (head, body) = response.split_once("\r\n\r\n").unwrap();
    let status = head.split(' ').nth(1).unwrap().parse().unwrap();
    let content_type = head
        .lines()
        .find_map(|line| line.to_ascii_lowercase().strip_prefix("content-type: ").map(str::to_string))
        .unwrap_or_default();

    (status, content_type, serde_json::from_str(body).unwrap())
}

fn ok(address: SocketAddr, path: &str) -> Value {
    let (status, content_type, body) = get(address, path);

    assert_eq!(status, 200, "{path}: {body}");
    assert_eq!(content_type, "application/json", "{path}");

    body
}

#[test]
fn status_names_the_chain_and_its_tip() {
    let fixture = serve();
    let status = ok(fixture.address, "/status");

    assert_eq!(status["genesis"], fixture.chain.genesis().to_string());
    assert_eq!(status["tip"], fixture.chain.tip().id().to_string());
    assert_eq!(status["height"], 2);
    assert_eq!(status["timestamp"], fixture.chain.tip().timestamp);
}

#[test]
fn a_block_is_found_by_height_by_id_and_as_latest() {
    let fixture = serve();
    let first = ok(fixture.address, "/blocks/1");
    let id = first["id"].as_str().unwrap();
    let tx = &first["txs"][0];

    assert_eq!(ok(fixture.address, &format!("/blocks/{id}")), first);
    assert_eq!(first["height"], 1);
    assert_eq!(first["tx_count"], 1);
    assert_eq!(first["seal"]["type"], "signature");
    assert!(first["seal"]["signature"].as_str().unwrap().starts_with("0x"));
    assert_eq!(first["difficulty"], "0");
    assert_eq!(tx["receipt"]["status"], "done");
    assert_eq!(tx["receipt"]["events"], json!([]));
    assert_eq!(tx["nonce"], 0);
    assert_eq!(tx["sender"], DevAuthority::dev_keypair().public().address().to_string());
    assert_eq!(tx["tx"]["type"], "transfer");
    assert_eq!(tx["tx"]["to"], fixture.bob.to_string());
    assert_eq!(tx["tx"]["amount"], "100");

    let latest = ok(fixture.address, "/blocks/latest");

    assert_eq!(latest["height"], 2);
    assert_eq!(latest["txs"][0]["tx"]["type"], "deploy");
    assert_eq!(latest["txs"][0]["tx"]["engine"], "lua");
    assert_eq!(latest["txs"][0]["tx"]["source"], COUNTER);
    assert_eq!(ok(fixture.address, "/blocks/0")["seal"], Value::Null);
}

#[test]
fn listings_page_newest_first() {
    let fixture = serve();
    let page = ok(fixture.address, "/blocks?limit=2");
    let heights = |page: &Value| -> Vec<u64> {
        page["blocks"].as_array().unwrap().iter().map(|block| block["height"].as_u64().unwrap()).collect()
    };

    assert_eq!(heights(&page), [2, 1]);
    assert_eq!(page["blocks"][0].get("txs"), None);
    assert_eq!(page["next"], 1);

    let rest = ok(fixture.address, "/blocks?before=1&limit=2");

    assert_eq!(heights(&rest), [0]);
    assert_eq!(rest["next"], Value::Null);
    assert_eq!(heights(&ok(fixture.address, "/blocks?before=99")), [2, 1, 0]);
}

#[test]
fn accounts_and_contracts_are_read_from_state() {
    let fixture = serve();
    let authority = DevAuthority::dev_keypair().public().address();
    let bob = ok(fixture.address, &format!("/accounts/{}", fixture.bob));
    let counter = contract_address(&authority, 1, HashType::default_chain());
    let contract = ok(fixture.address, &format!("/contracts/{counter}"));

    assert_eq!(bob["balance"], "100");
    assert_eq!(bob["nonce"], 0);
    assert_eq!(ok(fixture.address, &format!("/accounts/{authority}"))["nonce"], 2);
    assert_eq!(contract["deployer"], authority.to_string());
    assert_eq!(contract["functions"], json!(["increment"]));
    assert_eq!(contract["engine"], "lua");
    assert_eq!(
        contract["layout"],
        json!([{"name": "counter", "shape": "map", "kind": "number", "unit": "clicks", "key_kind": "text", "key_unit": null}])
    );
    assert!(contract["code_size"].as_u64().unwrap() > 0);
}

#[test]
fn any_node_says_what_a_miner_claimed_and_the_root_its_pools_block_committed_to() {
    use crate::registry::pools::{PoolClaim, PoolRoot, claim_leaf, pool_escrow};
    use crate::structs::block::PoolCommitment;
    use crate::structs::u256::u256;

    let authority = DevAuthority::dev_keypair();
    let mut producer = AuthorityProducer::new(authority.clone(), SLOT);
    let db = Arc::new(MemoryDb::default());
    let mut chain = Chain::open(db.clone(), &free()).unwrap();
    let hash = chain.hash();
    let (pool, miner) = (producer.address(&[]), Keypair::from_seed([8; 32]).public().address());
    let leaves = [claim_leaf(hash, &miner, u256::new(40))];
    let root = crate::crypto::merkle::root(hash, &leaves);
    let now = chain.tip().timestamp + SLOT;
    let mut block = chain.build_declaring(Vec::new(), Vec::new(), pool, now, None, None, Some(PoolCommitment { parts: 0, root, count: 1 })).unwrap();
    block.seal = producer.seal(&block.header).unwrap();
    chain.import(&block, now).unwrap();
    let address = start(db, None, &free(), "127.0.0.1:0".parse().unwrap(), ApiOptions::default()).unwrap();

    // No pool's node behind it, and still it checks a claim a miner kept.
    let claims = ok(address, &format!("/claims/{pool}/{miner}?height=1"));
    assert_eq!((claims["claimed"].as_str(), claims["height"].as_u64()), (Some("0"), Some(1)));
    assert_eq!(claims["escrow"], pool_escrow(&pool, hash).to_string());
    assert_eq!(claims["root"], json!({ "root": root.to_string(), "count": 1 }));

    let kept = PoolClaim { pool, height: 1, block: block.id(), to: miner, earned: u256::new(40), index: 0, count: 1, path: Vec::new() };
    let on_chain = PoolRoot { pool, root: from_hex(claims["root"]["root"].as_str().unwrap()).and_then(|bytes| Hash::from_bytes(hash, &bytes)).unwrap(), count: 1 };
    assert!(kept.holds(&on_chain, hash));

    assert_eq!(ok(address, &format!("/claims/{pool}/{miner}"))["root"], Value::Null, "no height asked about");
    assert_eq!(ok(address, &format!("/claims/{pool}/{miner}?height=0"))["root"], Value::Null, "a block with no root");
    assert_eq!(ok(address, &format!("/claims/{miner}/{miner}?height=1"))["root"], Value::Null, "another pool's");
}

#[test]
fn bad_requests_and_missing_things_answer_with_an_error() {
    let fixture = serve();
    let nobody = Address::Contract([9; 32]);
    let cases = [
        ("/blocks/99", 404),
        (&format!("/blocks/0x{}", "00".repeat(32)), 404),
        ("/blocks/tall", 400),
        ("/blocks/0x1234", 400),
        ("/blocks?limit=lots", 400),
        ("/accounts/bob", 400),
        (&format!("/contracts/{nobody}"), 404),
        ("/proofs/blocks/0x00", 400),
        ("/proofs/accounts/zz", 400),
        (&format!("/proofs/receipts/0x{}", "00".repeat(32)), 404),
        ("/nowhere", 404),
    ];

    for (path, expected) in cases {
        let (status, content_type, body) = get(fixture.address, path);

        assert_eq!(status, expected, "{path}");
        assert_eq!(content_type, "application/json", "{path}");
        assert!(body["error"].is_string(), "{path}: {body}");
    }
}

fn signed(nonce: u64) -> SignedTx {
    let authority = DevAuthority::dev_keypair();
    let tx = Tx::Burn {
        from: authority.public().address(),
        amount: Amount::U128(1),
    };

    SignedTx::sign(tx, nonce, FeeOffer::NONE, &authority, &free().block().id())
}

fn post_tx(address: SocketAddr, bytes: &[u8]) -> (u16, Value) {
    let (status, _, body) = request(address, "POST", "/txs", &json!({ "tx": to_hex(bytes) }).to_string());

    (status, body)
}

#[test]
fn transactions_are_submitted_as_hex() {
    let fixture = serve();
    let tx = signed(5);
    let (status, body) = post_tx(fixture.address, &wincode::serialize(&tx).unwrap());

    assert_eq!(status, 202, "{body}");
    assert_eq!(body["id"], tx.id(HashType::default_chain()).to_string());
}

#[test]
fn a_node_with_no_room_says_it_is_busy() {
    let db = Arc::new(MemoryDb::default());
    Chain::open(db.clone(), &free()).unwrap();
    let full = fake_node_with(BacklogLimits { total: 0, per_peer: 0, ..BacklogLimits::default() });
    let address = start(db, Some(full), &free(), "127.0.0.1:0".parse().unwrap(), ApiOptions::default()).unwrap();
    let (status, body) = post_tx(address, &wincode::serialize(&signed(5)).unwrap());

    assert_eq!(status, 503, "{body}");
    assert_eq!(body["error"], "the node is busy; try again shortly");
}

#[test]
fn each_refusal_of_the_mempools_limits_tells_a_wallet_what_to_do() {
    let fixture = serve();
    let refused = |nonce: u64| post_tx(fixture.address, &wincode::serialize(&signed(nonce)).unwrap());
    let (status, body) = refused(98);

    assert_eq!(status, 429, "slow down: {body}");
    assert!(body["error"].as_str().unwrap().contains("64 transactions waiting already"), "{body}");

    let (status, body) = refused(97);

    assert_eq!(status, 409, "{body}");
    assert!(body["error"].as_str().unwrap().contains("raise both its max price and its tip by at least 10%"), "{body}");

    let (status, body) = refused(96);

    assert_eq!(status, 400, "{body}");
    assert!(body["error"].as_str().unwrap().contains("the account's next is 3"), "{body}");

    let (status, body) = refused(95);

    assert_eq!(status, 503, "{body}");
    assert!(body["error"].as_str().unwrap().contains("offer a higher tip"), "{body}");
}

#[test]
fn bad_or_refused_submissions_say_why() {
    let fixture = serve();
    let mut trailing = wincode::serialize(&signed(5)).unwrap();
    trailing.push(0);
    let (status, body) = post_tx(fixture.address, &wincode::serialize(&signed(99)).unwrap());

    assert_eq!(status, 409, "{body}");
    assert_eq!(body["error"], "nonce already used");

    for (status, body) in [
        post_tx(fixture.address, &[1, 2, 3]),
        post_tx(fixture.address, &trailing),
        {
            let (status, _, body) = request(fixture.address, "POST", "/txs", "{\"tx\": \"zz\"}");
            (status, body)
        },
        {
            let (status, _, body) = request(fixture.address, "POST", "/txs", "not json");
            (status, body)
        },
    ] {
        assert_eq!(status, 400, "{body}");
        assert!(body["error"].is_string(), "{body}");
    }
}

#[test]
fn an_api_without_a_node_takes_no_transactions() {
    let db = Arc::new(MemoryDb::default());
    Chain::open(db.clone(), &free()).unwrap();
    let genesis = free();
    let address = start(db, None, &genesis, "127.0.0.1:0".parse().unwrap(), ApiOptions::default()).unwrap();
    let (status, body) = post_tx(address, &wincode::serialize(&signed(0)).unwrap());

    assert_eq!(status, 503, "{body}");
}

#[test]
fn transactions_are_looked_up_by_id_once_they_land() {
    let fixture = serve();
    let first = ok(fixture.address, "/blocks/1");
    let id = first["txs"][0]["id"].as_str().unwrap().to_string();
    let receipt = ok(fixture.address, &format!("/txs/{id}"));

    assert_eq!(receipt["id"], id);
    assert_eq!(receipt["block"], first["id"]);
    assert_eq!(receipt["height"], 1);
    assert_eq!(receipt["index"], 0);
    assert_eq!(receipt["confirmations"], 2);
    assert_eq!(receipt["status"], "done");
    assert_eq!(receipt["payments"], json!([]), "a plain transfer: no contract paid anything");

    let deploy = ok(fixture.address, "/blocks/2")["txs"][0]["id"].as_str().unwrap().to_string();

    assert_eq!(ok(fixture.address, &format!("/txs/{deploy}"))["status"], "deployed");

    let (status, _, body) = get(fixture.address, &format!("/txs/0x{}", "11".repeat(32)));

    assert_eq!(status, 404, "{body}");
    assert_eq!(get(fixture.address, "/txs/nope").0, 400);
}

#[test]
fn fees_show_in_status_blocks_and_receipts() {
    let authority = DevAuthority::dev_keypair();
    let mut producer = AuthorityProducer::new(authority.clone(), SLOT);
    let db = Arc::new(MemoryDb::default());
    let genesis = Genesis::dev_authority(authority.public());
    let mut chain = Chain::open(db.clone(), &genesis).unwrap();
    let genesis_id = chain.genesis();
    let base = chain.next_base_price();
    let fee = FeeOffer {
        gas_limit: 100_000,
        max_price: base * crate::structs::u256::u256::from(2u64),
        tip: crate::structs::u256::u256::from(3u64),
    };
    let transfer = Tx::Transfer {
        from: authority.public().address(),
        to: Keypair::generate(authority.public().key_type()).unwrap().public().address(),
        amount: Amount::U128(10),
    };
    let now = chain.tip().timestamp + SLOT;
    let mut block = chain.build(vec![SignedTx::sign(transfer, 0, fee, &authority, &genesis_id)], Vec::new(), producer.address(&[]), now).unwrap();
    block.seal = producer.seal(&block.header).unwrap();
    chain.import(&block, now).unwrap();
    let address = start(db, None, &genesis, "127.0.0.1:0".parse().unwrap(), ApiOptions::default()).unwrap();
    let fees = &ok(address, "/status")["fees"];

    assert_eq!(fees["base_price"], chain.next_base_price().to_string());
    assert_eq!(fees["block_gas_limit"], 50_000_000);
    assert_eq!(fees["burn_permille"], 500);
    assert_eq!(fees["schedule"]["type"], "weighted");
    assert_eq!(fees["pricing"]["type"], "base_fee");
    assert_eq!(fees["tx_fuel"], 10_000_000);

    let shown = ok(address, "/blocks/1");
    let tx = &shown["txs"][0];
    let gas = block.header.gas_used;

    assert_eq!(shown["gas_used"], gas);
    assert_eq!(shown["base_price"], base.to_string());
    assert_eq!(tx["fee"], json!({ "gas_limit": 100_000, "max_price": fee.max_price.to_string(), "tip": "3" }));
    assert_eq!(tx["receipt"]["gas_used"], gas);

    let paid = crate::structs::u256::u256::from(gas) * (base + fee.tip);
    let receipt = ok(address, &format!("/txs/{}", tx["id"].as_str().unwrap()));

    assert_eq!(receipt["fee"], paid.to_string());
    assert_eq!(receipt["gas_used"], gas);
    assert_eq!(ok(serve().address, "/status")["fees"], Value::Null, "the free fixture");
}

// Finality: the dev authority chain, where under FFG the dev key is the one validator and epochs
// are 8 blocks long, then the same chain with the other kinds.

#[test]
fn a_chain_without_finality_says_so() {
    let genesis = Genesis {
        finality: None,
        ..free()
    };
    let db = Arc::new(MemoryDb::default());
    Chain::open(db.clone(), &genesis).unwrap();
    let address = start(db, None, &genesis, "127.0.0.1:0".parse().unwrap(), ApiOptions::default()).unwrap();

    assert_eq!(ok(address, "/status")["finality"], Value::Null);
    assert_eq!(get(address, "/validators").0, 404);
    assert_eq!(ok(address, "/evidence")["evidence"], json!([]));

    // Ethereum's tools are told no block is finalized, as on proof of work: they count depth.
    #[cfg(feature = "ethereum")]
    for tag in ["finalized", "safe"] {
        let (_, _, answer) = request(address, "POST", "/rpc", &json!({ "jsonrpc": "2.0", "id": 1, "method": "eth_getBlockByNumber", "params": [tag, false] }).to_string());
        assert_eq!(answer["result"], Value::Null, "{tag}");
    }
}

/// Produces blocks on a chain with this finality, the transactions given for a height going into
/// its block, and serves it.
#[cfg(any(feature = "finality-authority", feature = "finality-depth"))]
fn serve_with(finality: crate::chain::FinalityConfig, height: u64, txs: impl Fn(u64) -> Vec<SignedTx>) -> (SocketAddr, Chain) {
    let authority = DevAuthority::dev_keypair();
    let mut producer = AuthorityProducer::new(authority.clone(), SLOT);
    let db = Arc::new(MemoryDb::default());
    let genesis = Genesis {
        finality: Some(finality),
        ..free()
    };
    let mut chain = Chain::open(db.clone(), &genesis).unwrap();

    for height in 1..=height {
        let now = chain.tip().timestamp + SLOT;
        let mut block = chain.build(txs(height), Vec::new(), producer.address(&[]), now).unwrap();
        block.seal = producer.seal(&block.header).unwrap();
        chain.import(&block, now).unwrap();
    }

    let address = start(db, Some(fake_node()), &genesis, "127.0.0.1:0".parse().unwrap(), ApiOptions::default()).unwrap();

    (address, chain)
}

#[cfg(feature = "finality-authority")]
#[test]
fn authority_chains_list_their_signers_and_changes() {
    let scheme = DevAuthority::dev_keypair().public().key_type();
    let keys: Vec<Keypair> = (0..3).map(|_| Keypair::generate(scheme).unwrap()).collect();
    use crate::registry::signers::Signer;

    let mut signers: Vec<_> = keys.iter().map(Keypair::public).collect();
    let config = crate::chain::AuthorityConfig {
        signers: signers.iter().copied().map(Signer::own).collect(),
        threshold: 2,
        change_threshold: 2,
        ..crate::chain::AuthorityConfig::dev(signers[0])
    };
    let proposal = vec![signers[0], signers[1]];
    let finality = crate::chain::FinalityConfig::Authority(config);
    let genesis_id = Genesis { finality: Some(finality.clone()), ..free() }.block().id();
    let approve = SignedTx::sign(
        Tx::ApproveSigners {
            version: 0,
            signers: proposal.iter().copied().map(Signer::own).collect(),
            threshold: 2,
            change_threshold: 2,
        },
        0,
        FeeOffer::NONE,
        &keys[0],
        &genesis_id,
    );
    // Another signer approves something else, then takes it back.
    let other = SignedTx::sign(
        Tx::ApproveSigners {
            version: 0,
            signers: vec![Signer::own(signers[1]), Signer::own(signers[2])],
            threshold: 2,
            change_threshold: 2,
        },
        0,
        FeeOffer::NONE,
        &keys[1],
        &genesis_id,
    );
    let withdraw = SignedTx::sign(Tx::WithdrawApproval { version: 0 }, 1, FeeOffer::NONE, &keys[1], &genesis_id);
    let (address, _) = serve_with(finality, 3, |height| match height {
        1 => vec![approve.clone(), other.clone()],
        2 => vec![withdraw.clone()],
        _ => Vec::new(),
    });
    let listing = ok(address, "/validators");
    signers.sort();
    let mut proposed = proposal.clone();
    proposed.sort();
    let names = |keys: &[crate::crypto::keypair::PublicKey]| keys.iter().map(|key| json!({ "key": key.to_string(), "account": key.address().to_string() })).collect::<Vec<_>>();

    assert_eq!(ok(address, "/status")["finality"]["type"], "authority");
    assert_eq!(listing["validators"], json!([]), "nobody bonds");
    assert_eq!(
        listing["active"],
        json!(signers.iter().map(|signer| json!({
            "voter": signer.to_string(),
            "account": signer.address().to_string(),
            "weight": "1",
        })).collect::<Vec<_>>())
    );
    assert_eq!(listing["total_weight"], "3");

    let tx = &ok(address, "/blocks/1")["txs"][0]["tx"];

    assert_eq!(tx["type"], "approve_signers");
    assert_eq!(tx["version"], 0);
    assert_eq!(tx["signers"], json!(names(&proposed)));

    let current = ok(address, "/signers");
    let set = json!({ "signers": names(&signers), "threshold": 2, "change_threshold": 2 });

    assert_eq!(ok(address, "/blocks/2")["txs"][0]["tx"], json!({ "type": "withdraw_approval", "version": 0 }));
    assert_eq!(current["version"], 0);
    assert_eq!(current["decided"], set);
    assert_eq!(current["active"], set);
    assert_eq!(current["approval_epochs"], 64);
    assert_eq!(
        current["proposals"],
        json!([{
            "signers": names(&proposed),
            "threshold": 2,
            "change_threshold": 2,
            "approvals": [{ "signer": keys[0].public().to_string(), "epoch": 0, "counts_until": 63 }],
        }])
    );
}

#[cfg(all(feature = "finality-ffg", feature = "governance-council"))]
#[test]
fn governance_lists_the_decision_and_the_ballot() {
    use crate::consensus::{ConsensusConfig, Schedule};
    use crate::governance::{Change, GovernanceConfig};

    let dev = DevAuthority::dev_keypair();
    let other = Keypair::generate(dev.public().key_type()).unwrap();
    let genesis = Genesis {
        governance: Some(GovernanceConfig::Council {
            members: vec![dev.public().address(), other.public().address()],
            threshold: 2,
            approval_epochs: 16,
            quorum_permille: 0,
        }),
        ..free()
    };
    let staked = ConsensusConfig::Staked { slot_ms: SLOT, schedule: Schedule::Priority };
    let approve = SignedTx::sign(Tx::VoteChange { vote: crate::governance::ChangeVote::Yes, version: 0, change: Change::Consensus(staked.clone()) }, 0, FeeOffer::NONE, &dev, &genesis.block().id());
    let renamed = Change::Name("second".to_string());
    let abstain = SignedTx::sign(Tx::VoteChange { vote: crate::governance::ChangeVote::Abstain, version: 0, change: renamed.clone() }, 0, FeeOffer::NONE, &other, &genesis.block().id());
    let mut producer = AuthorityProducer::new(dev.clone(), SLOT);
    let db = Arc::new(MemoryDb::default());
    let mut chain = Chain::open(db.clone(), &genesis).unwrap();

    for height in 1..=2 {
        let now = chain.tip().timestamp + SLOT;
        let txs = match height {
            1 => vec![approve.clone()],
            _ => vec![abstain.clone()],
        };
        let mut block = chain.build(txs, Vec::new(), producer.address(&[]), now).unwrap();
        block.seal = producer.seal(&block.header).unwrap();
        chain.import(&block, now).unwrap();
    }

    let address = start(db, Some(fake_node()), &genesis, "127.0.0.1:0".parse().unwrap(), ApiOptions::default()).unwrap();
    let listing = ok(address, "/governance");
    let consensus = |config: &ConsensusConfig| crate::cli::genesis::consensus_json(config, None);

    assert_eq!(listing["kind"], "council");
    assert_eq!(listing["epoch"], 0);
    assert_eq!(listing["version"], 0);
    assert_eq!(listing["needed"], "2");
    assert_eq!(listing["total"], "2");
    assert_eq!(listing["quorum_permille"], 0);
    assert_eq!(listing["approval_epochs"], 16);
    assert_eq!(listing["voters"], json!([{ "account": dev.public().address().to_string(), "weight": "1" }, { "account": other.public().address().to_string(), "weight": "1" }]));
    assert_eq!(listing["active"], consensus(&genesis.consensus));
    assert_eq!(listing["decided"], consensus(&genesis.consensus));
    assert_eq!(
        listing["proposals"],
        json!([{
            "consensus": consensus(&staked),
            "kind": "consensus",
            "version": 0,
            "closes": 15,
            "tally": { "yes": "1", "no": "0", "abstain": "0", "voted": "1", "total": "2", "needed": "2", "quorum": "0", "more_needed": "1", "quorum_met": true, "can_pass": true },
            "votes": [{ "account": dev.public().address().to_string(), "vote": "yes", "weight": "1", "epoch": 0, "counts_until": 15 }],
        }, {
            "name": "second",
            "kind": "name",
            "version": 0,
            "closes": 15,
            "tally": { "yes": "0", "no": "0", "abstain": "1", "voted": "1", "total": "2", "needed": "2", "quorum": "0", "more_needed": "2", "quorum_met": true, "can_pass": true },
            "votes": [{ "account": other.public().address().to_string(), "vote": "abstain", "weight": "1", "epoch": 0, "counts_until": 15 }],
        }])
    );
    assert_eq!(ok(address, &format!("/governance/weight/{}", other.public().address()))["voted"][0]["vote"], "abstain");

    let tx = &ok(address, "/blocks/1")["txs"][0]["tx"];

    assert_eq!(tx["type"], "vote_change");
    assert_eq!(tx["vote"], "yes");
    assert_eq!(tx["kind"], "consensus");
    assert_eq!(tx["version"], 0);
    assert_eq!(tx["change"]["consensus"], consensus(&staked));
    assert_eq!(get(serve().address, "/governance").0, 404, "the plain dev chain has none");
}

#[cfg(all(feature = "finality-ffg", feature = "governance-council"))]
#[test]
fn governance_lists_its_decisions_what_stands_and_who_may_vote() {
    use crate::governance::{Change, GovernanceConfig};

    let dev = DevAuthority::dev_keypair();
    let other = Keypair::generate(dev.public().key_type()).unwrap();
    let genesis = Genesis {
        governance: Some(GovernanceConfig::Council { members: vec![dev.public().address(), other.public().address()], threshold: 1, approval_epochs: 16, quorum_permille: 0 }),
        name: Some("first".to_string()),
        ..free()
    };
    let bootnodes = vec!["/dns4/boot.example/udp/4001/quic-v1".to_string()];
    let renamed = SignedTx::sign(Tx::VoteChange { vote: crate::governance::ChangeVote::Yes, version: 0, change: Change::Name("second".to_string()) }, 0, FeeOffer::NONE, &dev, &genesis.block().id());
    // Each kind its own version: the bootnodes' first decision, after the name's.
    let booted = SignedTx::sign(Tx::VoteChange { vote: crate::governance::ChangeVote::Yes, version: 0, change: Change::Bootnodes(bootnodes.clone()) }, 1, FeeOffer::NONE, &dev, &genesis.block().id());
    let mut producer = AuthorityProducer::new(dev.clone(), SLOT);
    let db = Arc::new(MemoryDb::default());
    let mut chain = Chain::open(db.clone(), &genesis).unwrap();

    for txs in [vec![renamed.clone()], vec![booted.clone()], Vec::new()] {
        let now = chain.tip().timestamp + SLOT;
        let mut block = chain.build(txs, Vec::new(), producer.address(&[]), now).unwrap();
        block.seal = producer.seal(&block.header).unwrap();
        chain.import(&block, now).unwrap();
    }

    let address = start(db, Some(fake_node()), &genesis, "127.0.0.1:0".parse().unwrap(), ApiOptions::default()).unwrap();
    let renamed_id = renamed.id(genesis.hash).to_string();
    let booted_id = booted.id(genesis.hash).to_string();
    let receipt = ok(address, &format!("/txs/{renamed_id}"));

    assert_eq!((receipt["status"].clone(), receipt["version"].clone()), (json!("decided"), json!(1)), "the approval that decides says so");
    assert_eq!((receipt["kind"].clone(), receipt["kind_version"].clone()), (json!("name"), json!(1)), "and of which kind, as which of its versions");

    let history = ok(address, "/governance/history");

    assert_eq!((history["version"].clone(), history["from"].clone()), (json!(2), json!(1)));
    assert_eq!(history["decisions"].as_array().unwrap().len(), 2);
    assert_eq!(history["decisions"][0]["version"], 2, "newest first");
    assert_eq!(history["decisions"][0]["tx"], booted_id);
    assert_eq!(history["decisions"][0]["kind"], "bootnodes");
    assert_eq!(history["decisions"][0]["height"], 2);
    assert_eq!(history["decisions"][0]["sender"], dev.public().address().to_string());
    assert_eq!(history["decisions"][1]["tx"], renamed_id);
    assert_eq!(history["decisions"][1]["change"], json!({ "name": "second" }));

    let older = ok(address, "/governance/history?before=2");

    assert_eq!(older["decisions"].as_array().unwrap().len(), 1);
    assert_eq!(older["decisions"][0]["version"], 1);

    let listing = ok(address, "/governance");
    let in_force = &listing["in_force"];

    assert_eq!(listing["members"], json!([dev.public().address().to_string(), other.public().address().to_string()]));
    assert_eq!(in_force["name"], json!({ "value": "second", "decided": 1 }));
    assert_eq!(in_force["bootnodes"], json!({ "value": bootnodes, "decided": 2 }));
    assert_eq!(in_force["reward"]["decided"], Value::Null, "genesis's stands");
    assert_eq!(in_force["reward"]["value"], crate::cli::genesis::reward_json(&genesis.reward));
    assert!(in_force["consensus"]["value"].is_object());
    assert!(in_force.get("upgrade").is_none(), "no upgrade was ever decided");

    let member = ok(address, &format!("/governance/weight/{}", other.public().address()));
    let stranger = Keypair::generate(dev.public().key_type()).unwrap().public().address();

    assert_eq!((member["weight"].clone(), member["needed"].clone()), (json!("1"), json!("1")));
    assert_eq!(member["voted"], json!([]), "no vote of any kind that still counts");
    assert_eq!(member["total"], "2");
    assert_eq!(listing["versions"]["name"], 1);
    assert_eq!(listing["versions"]["bootnodes"], 1);
    assert_eq!(listing["versions"]["fees"], 0, "each kind its own count");
    assert_eq!(ok(address, &format!("/governance/weight/{stranger}"))["weight"], "0");
}

#[cfg(all(feature = "finality-ffg", feature = "governance-council"))]
#[test]
fn a_rejection_is_listed_and_a_light_client_proves_it_from_its_receipt() {
    use crate::governance::{Change, ChangeKind, ChangeVote, GovernanceConfig};
    use crate::structs::receipt::CommittedOutcome;

    let dev = DevAuthority::dev_keypair();
    let other = Keypair::generate(dev.public().key_type()).unwrap();
    let genesis = Genesis {
        governance: Some(GovernanceConfig::Council { members: vec![dev.public().address(), other.public().address()], threshold: 2, approval_epochs: 16, quorum_permille: 0 }),
        ..free()
    };
    let hash = genesis.hash;
    let renamed = Change::Name("second".to_string());
    // A No from one of a council of two that needs both: it can no longer pass.
    let no = SignedTx::sign(Tx::VoteChange { version: 0, change: renamed.clone(), vote: ChangeVote::No }, 0, FeeOffer::NONE, &dev, &genesis.block().id());
    let again = SignedTx::sign(Tx::VoteChange { version: 0, change: renamed.clone(), vote: ChangeVote::Yes }, 1, FeeOffer::NONE, &dev, &genesis.block().id());
    let mut producer = AuthorityProducer::new(dev.clone(), SLOT);
    let db = Arc::new(MemoryDb::default());
    let mut chain = Chain::open(db.clone(), &genesis).unwrap();

    for txs in [vec![no.clone()], vec![again.clone()], Vec::new(), Vec::new()] {
        let now = chain.tip().timestamp + SLOT;
        let mut block = chain.build(txs, Vec::new(), producer.address(&[]), now).unwrap();
        block.seal = producer.seal(&block.header).unwrap();
        chain.import(&block, now).unwrap();
    }

    let address = start(db, Some(fake_node()), &genesis, "127.0.0.1:0".parse().unwrap(), ApiOptions::default()).unwrap();
    let receipt = ok(address, &format!("/txs/{}", no.id(hash)));

    assert_eq!((receipt["status"].clone(), receipt["rejection"].clone(), receipt["kind"].clone(), receipt["kind_version"].clone()), (json!("rejected"), json!(1), json!("name"), json!(0)));
    assert!(ok(address, &format!("/txs/{}", again.id(hash)))["error"].as_str().unwrap_or_default().contains("rejected"), "it stays rejected at its version");

    let listed = ok(address, "/governance/rejections");

    assert_eq!((listed["rejection"].clone(), listed["from"].clone()), (json!(1), json!(1)));
    assert_eq!(listed["rejections"][0]["tx"], no.id(hash).to_string());
    assert_eq!(listed["rejections"][0]["change"], json!({ "name": "second" }));
    assert_eq!((listed["rejections"][0]["of"].clone(), listed["rejections"][0]["kind_version"].clone()), (json!("name"), json!(0)));
    assert_eq!(ok(address, "/governance")["rejections"], 1);
    assert_eq!(ok(address, "/governance")["proposals"], json!([]), "its votes went with it");

    let client = followed(address, chain.genesis());
    let decode = |value: &Value| from_hex(value.as_str().unwrap()).unwrap();
    let hashes = |value: &Value| value.as_array().unwrap().iter().map(|one| Hash::from_bytes(hash, &decode(one)).unwrap()).collect::<Vec<_>>();
    let proof = ok(address, &format!("/proofs/receipts/{}", no.id(hash)));
    let (block, count) = (Hash::from_bytes(hash, &decode(&proof["block"])).unwrap(), proof["count"].as_u64().unwrap() as usize);
    let (receipt, _) = client.check_receipt(&block, &decode(&proof["receipt"]), 0, count, &hashes(&proof["path"])).unwrap();

    assert_eq!(receipt.outcome, CommittedOutcome::Rejected { rejection: 1, kind: ChangeKind::Name, version: 0 });
}

#[cfg(feature = "finality-depth")]
#[test]
fn depth_chains_finalize_by_depth() {
    let authority = DevAuthority::dev_keypair();
    let finality = crate::chain::FinalityConfig::Depth(crate::chain::DepthConfig { depth: 3 });
    let genesis_id = Genesis { finality: Some(finality.clone()), ..free() }.block().id();
    let burn = |nonce: u64| {
        let tx = Tx::Burn {
            from: authority.public().address(),
            amount: Amount::U128(1),
        };

        SignedTx::sign(tx, nonce, FeeOffer::NONE, &authority, &genesis_id)
    };
    let (address, _) = serve_with(finality, 6, |height| {
        match height {
            1 => vec![burn(0)],
            5 => vec![burn(1)],
            _ => Vec::new(),
        }
    });
    let finality = &ok(address, "/status")["finality"];
    let id = |height: u64| ok(address, &format!("/blocks/{height}"))["txs"][0]["id"].as_str().unwrap().to_string();

    assert_eq!(finality["type"], "depth");
    assert_eq!(finality["checkpoint_interval"], 1);
    assert_eq!(finality["finalized"]["height"], 3);

    // Ethereum's finalized tag names the finalized block, not the tip.
    #[cfg(feature = "ethereum")]
    {
        let (_, _, answer) = request(address, "POST", "/rpc", &json!({ "jsonrpc": "2.0", "id": 1, "method": "eth_getBlockByNumber", "params": ["finalized", false] }).to_string());
        assert_eq!(answer["result"]["number"], "0x3");
        let (_, _, answer) = request(address, "POST", "/rpc", &json!({ "jsonrpc": "2.0", "id": 1, "method": "eth_getBlockByNumber", "params": ["latest", false] }).to_string());
        assert_eq!(answer["result"]["number"], "0x6");
    }
    assert_eq!(ok(address, &format!("/txs/{}", id(1)))["final"], true);
    assert_eq!(ok(address, &format!("/txs/{}", id(5)))["final"], false);
    assert_eq!(get(address, "/validators").0, 404);
    assert_eq!(get(address, "/signers").0, 404);
    assert_eq!(get(address, "/light/finality").0, 404, "nothing votes");

    // Any block is a checkpoint here: the final one, and only it, with nothing to prove voters by.
    let checkpoint = ok(address, "/light/checkpoint");
    let start: crate::light::CheckpointStart = wincode::deserialize(&from_hex(checkpoint["start"].as_str().unwrap()).unwrap()).unwrap();

    assert_eq!(checkpoint["checkpoint"]["height"], 3);
    assert_eq!((start.headers.len(), start.updates.len()), (1, 0));

    let light = ok(address, "/light");
    let decode = |value: &Value| from_hex(value.as_str().unwrap()).unwrap();
    let params: crate::light::LightParams = wincode::deserialize(&decode(&light["params"])).unwrap();
    let genesis: crate::structs::block::SealedHeader = wincode::deserialize(&decode(&light["header"])).unwrap();
    let block = Hash::from_bytes(HashType::default_chain(), &decode(&checkpoint["checkpoint"]["block"])).unwrap();
    let client = crate::light::LightClient::from_checkpoint(params, genesis.clone(), genesis.id(), block, start).unwrap();

    assert_eq!((client.lowest(), client.tip().height), (3, 3), "it starts there, and follows on");
}

#[cfg(feature = "finality-ffg")]
mod ffg {
    use super::*;

    struct Finalized {
        address: SocketAddr,
        chain: Chain,
        newcomer: Keypair,
    }

    /// A dev chain 18 blocks long with a bond in block 1 and a transfer in block 12, where the dev key justified epochs 1 and 2,
    /// which finalized epoch 1, and signed one double vote.
    fn serve_finalized() -> Finalized {
        use crate::structs::vote::{Checkpoint, SignedVote, Vote};

        let authority = DevAuthority::dev_keypair();
        let mut producer = AuthorityProducer::new(authority.clone(), SLOT);
        let db = Arc::new(MemoryDb::default());
        let genesis = free();
        let mut chain = Chain::open(db.clone(), &genesis).unwrap();
        let genesis_id = chain.genesis();
        let newcomer = Keypair::generate(authority.public().key_type()).unwrap();
        let bond = SignedTx::sign(
            Tx::Bond {
                voter: newcomer.public(),
                amount: Amount::U128(10u128.pow(20)),
            },
            0,
            FeeOffer::NONE,
            &authority,
            &genesis_id,
        );
        let transfer = SignedTx::sign(
            Tx::Transfer {
                from: authority.public().address(),
                to: newcomer.public().address(),
                amount: Amount::U128(1),
            },
            1,
            FeeOffer::NONE,
            &authority,
            &genesis_id,
        );

        for height in 1..=18 {
            let now = chain.tip().timestamp + SLOT;
            let txs = match height {
                1 => vec![bond.clone()],
                12 => vec![transfer.clone()],
                _ => Vec::new(),
            };
            let proofs = chain.unpaid_proofs().unwrap();
            let mut block = chain.build(txs, proofs, producer.address(&[]), now).unwrap();
            block.seal = producer.seal(&block.header).unwrap();
            chain.import(&block, now).unwrap();

            if let Some(vote) = chain.vote_for(&authority.public()).unwrap() {
                chain.record_own_vote(&vote).unwrap();
                chain.add_vote(SignedVote::sign(vote, &authority, &chain.genesis())).unwrap();
            }
        }

        let double = Vote {
            target: Checkpoint {
                epoch: 2,
                block: chain.block_at(1).unwrap().unwrap().id(),
            },
            ..chain.justification(&chain.justified().unwrap().block).unwrap().unwrap().votes[0].vote
        };
        chain.add_vote(SignedVote::sign(double, &authority, &chain.genesis())).unwrap();
        let address = start(db, Some(fake_node()), &genesis, "127.0.0.1:0".parse().unwrap(), ApiOptions::default()).unwrap();

        Finalized { address, chain, newcomer }
    }

    #[test]
    fn status_tells_how_far_finality_has_come() {
        let fixture = serve_finalized();
        let finality = &ok(fixture.address, "/status")["finality"];

        assert_eq!(finality["epoch"], 2);
        assert_eq!(finality["checkpoint_interval"], 8);
        assert_eq!(finality["justified"]["epoch"], 2);
        assert_eq!(finality["justified"]["height"], 16);
        assert_eq!(finality["justified"]["block"], fixture.chain.justified().unwrap().block.to_string());
        assert_eq!(finality["finalized"]["epoch"], 1);
        assert_eq!(finality["finalized"]["height"], 8);
    }

    #[test]
    fn light_finality_comes_an_epoch_at_a_time_up_to_the_last_justified() {
        use crate::light::{FinalityStep, MAX_FINALITY_STEPS};

        let fixture = serve_finalized();
        let steps = |path: &str| -> Vec<FinalityStep> {
            ok(fixture.address, path)["steps"]
                .as_array()
                .unwrap()
                .iter()
                .map(|step| wincode::deserialize(&from_hex(step.as_str().unwrap()).unwrap()).unwrap())
                .collect()
        };
        let all = steps("/light/finality");
        let checkpoint = |epoch: u64| fixture.chain.block_at(epoch * 8).unwrap().unwrap().id();

        // Genesis, with its update and no justification, then epochs 1 and 2: nothing past the
        // last justified.
        assert_eq!(all.len(), 3);
        assert!(all[0].justification.is_none() && all[0].update.as_ref().is_some_and(|update| update.block == fixture.chain.genesis()));

        for epoch in [1, 2] {
            let justification = all[epoch as usize].justification.as_ref().unwrap();

            assert_eq!((justification.target.epoch, justification.target.block), (epoch, checkpoint(epoch)));
            assert_eq!(all[epoch as usize].update.as_ref().unwrap().block, checkpoint(epoch));
        }

        assert_eq!(steps("/light/finality?from=2"), all[2..]);
        assert_eq!(steps("/light/finality?from=1&count=1"), all[1..2]);
        assert_eq!(steps("/light/finality?from=3"), Vec::new());
        assert_eq!(steps(&format!("/light/finality?count={}", MAX_FINALITY_STEPS * 10)), all, "asking for more than there is");
    }

    #[test]
    fn a_light_client_starts_at_the_nodes_final_checkpoint_or_one_named() {
        use crate::light::{CheckpointStart, LightClient, LightParams};
        use crate::structs::block::SealedHeader;

        let fixture = serve_finalized();
        let decode = |value: &Value| from_hex(value.as_str().unwrap()).unwrap();
        let start = ok(fixture.address, "/light");
        let params: LightParams = wincode::deserialize(&decode(&start["params"])).unwrap();
        let genesis: SealedHeader = wincode::deserialize(&decode(&start["header"])).unwrap();
        let finalized = fixture.chain.finalized().unwrap();

        // The node's own: its finalized checkpoint, epoch 1 at height 8.
        let own = ok(fixture.address, "/light/checkpoint");
        let block = Hash::from_bytes(HashType::default_chain(), &decode(&own["checkpoint"]["block"])).unwrap();
        let sent: CheckpointStart = wincode::deserialize(&decode(&own["start"])).unwrap();

        assert_eq!((own["checkpoint"]["epoch"].as_u64(), own["checkpoint"]["height"].as_u64(), block), (Some(1), Some(8), finalized.block));

        let client = LightClient::from_checkpoint(params.clone(), genesis.clone(), fixture.chain.genesis(), block, sent).unwrap();

        assert_eq!(client.finalized(), finalized);

        // One named: the justified checkpoint at 16, the same as the node's own would be later.
        let justified = fixture.chain.justified().unwrap();
        let named = ok(fixture.address, &format!("/light/checkpoint/{}", justified.block));
        let sent: CheckpointStart = wincode::deserialize(&decode(&named["start"])).unwrap();

        assert_eq!(named["checkpoint"]["height"], 16);
        assert!(LightClient::from_checkpoint(params, genesis, fixture.chain.genesis(), justified.block, sent).is_ok());

        let off_epoch = fixture.chain.block_at(9).unwrap().unwrap().id();

        assert_eq!(get(fixture.address, &format!("/light/checkpoint/{off_epoch}")).0, 400, "it opens no epoch");
        assert_eq!(get(fixture.address, &format!("/light/checkpoint/0x{}", "00".repeat(32))).0, 404);
    }

    #[test]
    fn receipts_say_whether_they_are_final() {
        let fixture = serve_finalized();
        let bond = ok(fixture.address, "/blocks/1")["txs"][0].clone();

        assert_eq!(bond["tx"]["type"], "bond");
        assert_eq!(bond["tx"]["voter"], fixture.newcomer.public().to_string());
        assert_eq!(bond["tx"]["amount"], "100000000000000000000");
        assert_eq!(ok(fixture.address, &format!("/txs/{}", bond["id"].as_str().unwrap()))["final"], true);

        // Past the finalized checkpoint at height 8, so not final yet.
        let transfer = ok(fixture.address, "/blocks/12")["txs"][0]["id"].as_str().unwrap().to_string();

        assert_eq!(ok(fixture.address, &format!("/txs/{transfer}"))["final"], false);

        let unvoted = serve();
        let id = ok(unvoted.address, "/blocks/1")["txs"][0]["id"].as_str().unwrap().to_string();

        assert_eq!(ok(unvoted.address, &format!("/txs/{id}"))["final"], false);
    }

    #[test]
    fn proofs_show_in_the_blocks_that_pay_them() {
        let fixture = serve_finalized();
        let dev = DevAuthority::dev_keypair().public();
        // The dev key voted for epoch 1 at height 10, and block 11 is the first that could pay it.
        let paying = ok(fixture.address, "/blocks/11");

        assert_eq!(paying["proofs"][0]["target"]["epoch"], 1);
        assert_eq!(paying["proofs"][0]["source"]["epoch"], 0);
        assert_eq!(paying["proofs"][0]["voters"], json!([dev.to_string()]));
    }

    #[test]
    fn validators_are_listed_with_who_votes_now() {
        let fixture = serve_finalized();
        let dev = DevAuthority::dev_keypair().public();
        let listing = ok(fixture.address, "/validators");
        let voters: Vec<&str> = listing["validators"]
            .as_array()
            .unwrap()
            .iter()
            .map(|record| record["voter"].as_str().unwrap())
            .collect();

        assert_eq!(listing["epoch"], 2);
        assert_eq!(voters.len(), 2);
        assert!(voters.contains(&dev.to_string().as_str()));
        assert!(voters.contains(&fixture.newcomer.public().to_string().as_str()));
        // Bonded in epoch 0, the newcomer votes from epoch 3.
        assert_eq!(listing["active"].as_array().unwrap().len(), 1);
        assert_eq!(listing["active"][0]["voter"], dev.to_string());
        assert_eq!(listing["total_weight"], listing["active"][0]["weight"]);

        let newcomer = ok(fixture.address, &format!("/validators/{}", fixture.newcomer.public()));

        assert_eq!(newcomer["bond"], "100000000000000000000");
        assert_eq!(newcomer["active_from"], 3);
        assert_eq!(newcomer["active_until"], Value::Null);
        assert_eq!(newcomer["slashed"], false);

        let stranger = Keypair::generate(dev.key_type()).unwrap().public();

        assert_eq!(get(fixture.address, &format!("/validators/{stranger}")).0, 404);
        assert_eq!(get(fixture.address, "/validators/nope").0, 400);
    }

    #[test]
    fn evidence_lists_conflicting_votes_ready_to_report() {
        let fixture = serve_finalized();
        let evidence = ok(fixture.address, "/evidence")["evidence"].clone();
        let dev = DevAuthority::dev_keypair().public();

        assert_eq!(evidence.as_array().unwrap().len(), 1);
        assert_eq!(evidence[0]["voter"], dev.to_string());
        assert_eq!(evidence[0]["first"]["target"]["epoch"], 2);
        assert_eq!(evidence[0]["second"]["target"]["epoch"], 2);
        assert_ne!(evidence[0]["first"]["target"]["block"], evidence[0]["second"]["target"]["block"]);

        let encoded = crate::structs::hex::from_hex(evidence[0]["second"]["encoded"].as_str().unwrap()).unwrap();
        let vote: crate::structs::vote::SignedVote = wincode::deserialize(&encoded).unwrap();

        assert_eq!(vote.verify(&fixture.chain.genesis()), Ok(()));
    }
}

#[test]
fn the_node_tells_how_it_is_doing_and_what_waits() {
    let fixture = serve();
    let node = ok(fixture.address, "/node");

    assert_eq!(node["uptime_ms"], 60_000);
    assert_eq!(node["producing"], true);
    assert_eq!(node["listening"], json!(["/ip4/127.0.0.1/udp/4001/quic-v1"]));
    assert_eq!(node["mempool"], json!({ "count": 2, "capacity": 50_000 }));
    assert_eq!(node["counters"]["blocks_heard"], 3);
    assert_eq!(node["peers"][0]["heard"], json!({ "blocks": 3, "txs": 4, "votes": 0 }));
    assert_eq!(node["peers"][0]["chain"], json!({ "height": 5, "work": "5", "justified": 0 }));
    assert_eq!((node["peers"][0]["dropped"].clone(), node["counters"]["gossip_dropped"].clone()), (json!(7), json!(7)));
    assert_eq!(node["backlog"], json!({ "waiting": 12, "total": 10_000, "per_peer": 1_000, "per_ip": 4_000, "requests": 10, "request_burst": 50, "strikes": 10, "strike_window_ms": 60_000 }));
    assert_eq!((node["limits"]["ban.score"].clone(), node["limits"]["sync.headers"].clone(), node["limits"]["net.max-peers-in"].clone()), (json!(100), json!(1_024), Value::Null), "every knob, at its default here");
    assert_eq!(node["limits"].as_object().unwrap().len(), crate::node::tuning::KNOBS.len());
    assert_eq!(node["syncing"], Value::Null);
    assert_eq!(node["mining"]["rate"], 300_001_000.0);
    assert_eq!(node["mining"]["listening"], "0.0.0.0:4002");
    assert_eq!(node["mining"]["devices"], json!([{ "name": "CPU, 2 threads", "rate": 1_000.0 }]));
    assert_eq!(node["mining"]["workers"][0]["rate"], 300e6);
    assert_eq!(node["mining"]["workers"][0]["devices"][1], json!({ "name": "GPU 1", "rate": 100e6 }));
    assert_eq!((node["mining"]["workers"][0]["found"].clone(), node["mining"]["workers"][0]["refused"].clone()), (json!(4), json!(1)));

    let mempool = ok(fixture.address, "/mempool?limit=1");

    assert_eq!(mempool["count"], 2);
    assert_eq!(mempool["txs"].as_array().unwrap().len(), 1);
    assert_eq!(mempool["txs"][0]["id"], signed(0).id(HashType::default_chain()).to_string());

    let db = Arc::new(MemoryDb::default());
    Chain::open(db.clone(), &free()).unwrap();
    let alone = start(db, None, &free(), "127.0.0.1:0".parse().unwrap(), ApiOptions::default()).unwrap();

    assert_eq!(get(alone, "/node").0, 503);
    assert_eq!(get(alone, "/mempool").0, 503);
}

#[test]
fn receipts_show_where_a_call_went() {
    let mut fixture = serve();
    let authority = DevAuthority::dev_keypair().public().address();
    let counter = contract_address(&authority, 1, HashType::default_chain());
    let call = SignedTx::sign(
        Tx::ContractCall {
            to: counter,
            func: Hash::Xxh3_64(crate::vm::selector("increment")),
            amount: Amount::U128(0),
            data: vec![crate::structs::tx::TxCallArgumentType::I64(3)],
        },
        2,
        FeeOffer::NONE,
        &DevAuthority::dev_keypair(),
        &fixture.chain.genesis(),
    );
    produce(&mut fixture.chain, vec![call.clone()]);
    let receipt = ok(fixture.address, &format!("/txs/{}", call.id(HashType::default_chain())));

    assert_eq!(receipt["status"], "returned");
    assert_eq!(
        receipt["trace"],
        json!({
            "hops": [{
                "depth": 0,
                "kind": "call",
                "from": authority.to_string(),
                "to": counter.to_string(),
                "function": "increment",
                "value": "0",
                "fuel_used": receipt["fuel_used"],
                "error": null,
                "events": [],
            }],
            "dropped": 0,
        })
    );
    assert_eq!(ok(fixture.address, "/blocks/1")["txs"][0]["receipt"]["trace"], json!({ "hops": [], "dropped": 0 }));
}

#[test]
fn side_branches_are_listed_back_to_the_main_chain() {
    let mut fixture = serve();
    let tip = fixture.chain.block_at(2).unwrap().unwrap();
    let mut sibling = tip.clone();
    sibling.header.timestamp += SLOT;
    sibling.seal = AuthorityProducer::new(DevAuthority::dev_keypair(), SLOT).seal(&sibling.header).unwrap();

    assert!(matches!(fixture.chain.import(&sibling, sibling.header.timestamp), Ok(crate::chain::Imported::Side)));

    let forks = ok(fixture.address, "/forks");

    assert_eq!(forks["blocks"].as_array().unwrap().len(), 1);
    assert_eq!(forks["blocks"][0]["id"], sibling.id().to_string());
    assert_eq!(forks["blocks"][0]["parent"], tip.header.parent.to_string());
    assert_eq!(forks["tips"].as_array().unwrap().len(), 2);

    let side = ok(fixture.address, &format!("/blocks/{}", sibling.id()));

    assert_eq!(side["main"], false);
    assert_eq!(side["txs"][0]["receipt"], Value::Null);
    assert_eq!(ok(fixture.address, "/blocks/latest")["main"], true);
    assert_eq!(ok(fixture.address, "/forks?limit=0")["blocks"], json!([]));
}

#[test]
fn offers_cover_what_a_transaction_costs_now() {
    let authority = DevAuthority::dev_keypair();
    let db = Arc::new(MemoryDb::default());
    let genesis = Genesis::dev_authority(authority.public());
    let chain = Chain::open(db.clone(), &genesis).unwrap();
    let genesis_id = chain.genesis();
    let address = start(db.clone(), None, &genesis, "127.0.0.1:0".parse().unwrap(), ApiOptions::default()).unwrap();
    let probe = SignedTx::sign(
        Tx::Deploy {
            code: Code::Lua(COUNTER.to_string()),
        },
        0,
        FeeOffer::NONE,
        &authority,
        &genesis_id,
    );
    let body = |probe: &SignedTx, extra: Value| {
        let mut body = json!({ "tx": to_hex(&wincode::serialize(probe).unwrap()) });
        body.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());

        body.to_string()
    };
    let (status, _, offered) = request(address, "POST", "/offer", &body(&probe, json!({})));
    let fees = genesis.fees.as_ref().unwrap().build();
    let base = chain.next_base_price();
    let open = fees.offer(base, genesis.tx_limits.fuel, &probe, OfferChoice::default());
    let vm = Arc::new(crate::vm::Vm::new(&genesis.engines, genesis.hash).unwrap());
    let ran = crate::chain::dry_run(db.clone(), &genesis.rules(), &vm, &probe, open.gas_limit, base).unwrap();
    let estimated = FeeOffer { gas_limit: fees.estimated(genesis.tx_limits.fuel, &probe, ran.gas_used), ..open };

    // A deploy's limit is what its dry run used and a quarter more of what it ran, not the cap.
    assert_eq!(status, 200, "{offered}");
    assert_eq!(ran.failure, None);
    assert!(ran.gas_used > fees.schedule().intrinsic(&probe), "loading and running it cost fuel");
    assert_eq!(offered["gas_limit"], estimated.gas_limit);
    assert_eq!((offered["max_price"].clone(), offered["tip"].clone()), (json!(open.max_price.to_string()), json!("0")));
    assert_eq!(offered["estimate"], json::dry_run(&ran));
    assert!(estimated.gas_limit < open.gas_limit && estimated.gas_limit >= ran.gas_used);
    assert!(open.gas_limit > fees.schedule().for_fuel(genesis.tx_limits.fuel), "the cap is what a failing run gets");

    let (_, _, chosen) = request(address, "POST", "/offer", &body(&probe, json!({ "gas_limit": 77, "tip": "5" })));

    assert_eq!(chosen["gas_limit"], 77, "a limit the sender chose stands");
    assert_eq!(chosen["tip"], "5");
    assert_eq!(request(address, "POST", "/offer", &body(&probe, json!({ "tip": 5 }))).0, 400);

    // A transfer needs no dry run: its intrinsic cost is all it uses.
    let transfer = SignedTx::sign(
        Tx::Transfer { from: authority.public().address(), to: Keypair::generate(authority.public().key_type()).unwrap().public().address(), amount: Amount::U128(1) },
        0,
        FeeOffer::NONE,
        &authority,
        &genesis_id,
    );
    let (_, _, plain) = request(address, "POST", "/offer", &body(&transfer, json!({})));

    assert_eq!(plain["estimate"], Value::Null);
    assert_eq!(plain["gas_limit"], fees.schedule().intrinsic(&transfer));

    // A call that would fail says why, and keeps the cap, since things may change before it lands.
    let missing = SignedTx::sign(
        Tx::ContractCall { to: contract_address(&authority.public().address(), 41, genesis.hash), func: crate::crypto::hasher::Hasher::digest(HashType::Xxh3_64, b"nope"), amount: Amount::U128(0), data: Vec::new() },
        0,
        FeeOffer::NONE,
        &authority,
        &genesis_id,
    );
    let (status, _, failing) = request(address, "POST", "/offer", &body(&missing, json!({})));

    assert_eq!(status, 200);
    assert_eq!(failing["estimate"]["status"], "failed");
    assert!(failing["estimate"]["error"].as_str().unwrap().contains("no contract"), "{failing}");
    assert_eq!(failing["gas_limit"], fees.offer(base, genesis.tx_limits.fuel, &missing, OfferChoice::default()).gas_limit);

    let free = serve();
    let offered_free = request(free.address, "POST", "/offer", &body(&probe, json!({}))).2;

    assert_eq!((offered_free["gas_limit"].clone(), offered_free["max_price"].clone()), (json!(0), json!("0")), "no fees, no gas");
    assert_eq!(offered_free["estimate"]["status"], "ok", "but still a dry run, to say whether it would fail");
}

#[cfg(feature = "evm")]
#[test]
fn evm_deploys_and_calls_are_dry_run_at_offer_and_read_as_json() {
    let authority = DevAuthority::dev_keypair();
    let db = Arc::new(MemoryDb::default());
    let genesis = Genesis::dev_authority(authority.public());
    let _chain = Chain::open(db.clone(), &genesis).unwrap();
    let address = start(db.clone(), None, &genesis, "127.0.0.1:0".parse().unwrap(), ApiOptions::default()).unwrap();
    let init = [from_hex(include_str!("../../contracts/evm/Counter.bin").trim()).unwrap(), crate::structs::u256::u256::new(5).to_be_bytes().to_vec()].concat();
    let offer = |tx: Tx| {
        let probe = SignedTx::sign(tx, 0, FeeOffer::NONE, &authority, &genesis.block().id());

        request(address, "POST", "/offer", &json!({ "tx": to_hex(&wincode::serialize(&probe).unwrap()) }).to_string()).2
    };
    let deploy = offer(Tx::Deploy { code: Code::Evm(init) });
    let fees = genesis.fees.as_ref().unwrap().build();
    let tx = SignedTx::sign(Tx::EvmCall { to: Address::Evm([7; 20]), value: Amount::U128(3), input: vec![0xb2, 0x0e] }, 0, FeeOffer::NONE, &authority, &genesis.block().id());

    assert_eq!(deploy["estimate"]["status"], "ok", "{deploy}");
    assert_eq!(deploy["estimate"]["deployed"], crate::vm::evm::create_address(&authority.public().address(), 0).to_string());
    assert!(deploy["gas_limit"].as_u64().unwrap() > deploy["estimate"]["gas_used"].as_u64().unwrap(), "what it used and a quarter");

    // Nothing lives at the address: the call runs, returns nothing, and pays the 3 there.
    let paid = offer(tx.tx.clone());

    assert_eq!(paid["estimate"]["status"], "ok", "{paid}");
    assert_eq!(paid["estimate"]["returned"], json!([{ "type": "bytes", "value": "0x" }]));
    assert!(paid["gas_limit"].as_u64().unwrap() > fees.schedule().intrinsic(&tx), "an EVM call is offered gas to run, not just its intrinsic cost");
    assert_eq!(
        json::tx(&tx.tx),
        json!({ "type": "evm_call", "to": format!("evm:0x{}", "07".repeat(20)), "value": "3", "input": "0xb20e" })
    );
}

#[test]
fn supply_counts_what_was_made_and_the_rich_list_the_richest_first() {
    let fixture = serve();
    let supply = ok(fixture.address, "/supply");
    let allocated: u256 = free().allocations.iter().fold(u256::ZERO, |sum, allocation| sum + allocation.amount);
    let minted: u256 = supply["minted"].as_str().unwrap().parse().unwrap();

    #[cfg(feature = "finality-ffg")]
    assert!(minted > allocated, "the allocations, and the genesis validator's bond: {supply}");
    #[cfg(not(feature = "finality-ffg"))]
    assert_eq!(minted, allocated, "the allocations, with no bond to make: {supply}");
    assert_eq!(crate::registry::supply::Supply::of(&fixture.chain.state()).unwrap().minted, minted);
    assert_eq!(crate::registry::supply::adds_up(&fixture.chain.state()).unwrap(), Ok(()), "and that is what is held and bonded");
    assert_eq!(supply["burned"], "0", "no fees on the free chain");
    assert_eq!(supply["supply"], supply["minted"]);
    assert_eq!(supply["key"], to_hex(crate::registry::supply::SUPPLY_KEY));
    assert_eq!(supply["height"], 2);

    let rich = ok(fixture.address, "/rich?limit=1");
    let accounts = rich["accounts"].as_array().unwrap();
    let all = ok(fixture.address, "/rich");

    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0]["address"], DevAuthority::dev_keypair().public().address().to_string(), "the authority holds the most");
    assert!(rich["holders"].as_u64().unwrap() >= 2, "bob holds some too: {rich}");
    assert!(all["accounts"].as_array().unwrap().iter().any(|account| account["address"] == fixture.bob.to_string() && account["balance"] == "100"));

    let balances: Vec<u256> = all["accounts"].as_array().unwrap().iter().map(|account| account["balance"].as_str().unwrap().parse().unwrap()).collect();

    assert!(balances.windows(2).all(|pair| pair[0] >= pair[1]), "richest first: {balances:?}");
}

#[cfg(feature = "names")]
#[test]
fn every_name_lists_in_name_order_a_page_at_a_time() {
    use crate::registry::names::NameAction;

    let authority = DevAuthority::dev_keypair();
    let mut producer = AuthorityProducer::new(authority.clone(), SLOT);
    let db = Arc::new(MemoryDb::default());
    let mut chain = Chain::open(db.clone(), &free()).unwrap();
    let genesis_id = chain.genesis();
    let txs: Vec<Tx> = ["carol", "alice", "bob"].into_iter().map(|name| Tx::Name { name: name.into(), action: NameAction::Register { years: 1 } }).collect();
    let now = chain.tip().timestamp + SLOT;
    let signed: Vec<SignedTx> = txs.into_iter().enumerate().map(|(nonce, tx)| SignedTx::sign(tx, nonce as u64, FeeOffer::NONE, &authority, &genesis_id)).collect();
    let mut block = chain.build(signed, Vec::new(), producer.address(&[]), now).unwrap();
    block.seal = producer.seal(&block.header).unwrap();
    chain.import(&block, now).unwrap();
    let address = start(db, None, &free(), "127.0.0.1:0".parse().unwrap(), ApiOptions::default()).unwrap();

    let first = ok(address, "/names/list?limit=2");
    let names = |page: &Value| page["names"].as_array().unwrap().iter().map(|name| name["name"].as_str().unwrap().to_string()).collect::<Vec<_>>();

    assert_eq!(names(&first), ["alice", "bob"], "in name order");
    assert_eq!(first["next"], "bob");
    assert_eq!(first["names"][0]["display"], "alice.dev");
    assert_eq!(first["names"][0]["status"], "active");
    assert_eq!(first["names"][0]["owner"], authority.public().address().to_string());

    let second = ok(address, "/names/list?limit=2&after=bob.dev");

    assert_eq!(names(&second), ["carol"]);
    assert_eq!(second["next"], Value::Null, "the last page");
    assert_eq!(get(serve().address, "/names/list").0, 200, "a chain with names lists none");
}

#[cfg(feature = "names")]
#[test]
fn names_are_looked_up_by_name_and_accounts_show_their_primary_one() {
    use crate::registry::names::{NameAction, NamesConfig, YEAR_MS};

    let authority = DevAuthority::dev_keypair();
    let me = authority.public().address();
    let mut producer = AuthorityProducer::new(authority.clone(), SLOT);
    let db = Arc::new(MemoryDb::default());
    let mut chain = Chain::open(db.clone(), &free()).unwrap();
    let genesis_id = chain.genesis();
    let txs = [
        Tx::Name { name: "alice".into(), action: NameAction::Register { years: 1 } },
        Tx::Name { name: "alice".into(), action: NameAction::Text { key: "url".into(), value: Some("https://alice.example".into()) } },
        Tx::Name { name: "alice".into(), action: NameAction::Subname { label: "pay".into(), owner: me } },
        Tx::PrimaryName { name: Some("alice".into()) },
    ];
    let now = chain.tip().timestamp + SLOT;
    let signed: Vec<SignedTx> = txs.into_iter().enumerate().map(|(nonce, tx)| SignedTx::sign(tx, nonce as u64, FeeOffer::NONE, &authority, &genesis_id)).collect();
    let mut block = chain.build(signed, Vec::new(), producer.address(&[]), now).unwrap();
    block.seal = producer.seal(&block.header).unwrap();
    chain.import(&block, now).unwrap();
    let address = start(db, None, &free(), "127.0.0.1:0".parse().unwrap(), ApiOptions::default()).unwrap();

    let (status, _, names) = request(address, "GET", "/names", "");
    let dev = NamesConfig::dev();

    assert_eq!(status, 200);
    assert_eq!(names["suffix"], "dev");
    assert_eq!(names["prices"], json!(dev.prices.iter().map(|price| price.to_string()).collect::<Vec<_>>()));
    assert_eq!(names["prices"], names["genesis_prices"], "governance decided none");
    assert_eq!(names["grace_ms"], dev.grace_ms);

    let (status, _, alice) = request(address, "GET", "/names/Alice.dev", "");

    assert_eq!(status, 200, "{alice}");
    assert_eq!(alice["name"], "alice", "the suffix and case go");
    assert_eq!(alice["display"], "alice.dev");
    assert_eq!(alice["status"], "active");
    assert_eq!((alice["owner"].clone(), alice["resolves_to"].clone()), (json!(me.to_string()), json!(me.to_string())));
    assert_eq!(alice["expires"], now + YEAR_MS);
    assert_eq!(alice["grace_ends"], now + YEAR_MS + dev.grace_ms);
    assert_eq!(alice["texts"], json!({ "url": "https://alice.example" }));
    assert_eq!(alice["price_per_year"], dev.prices[2].to_string());

    let (_, _, pay) = request(address, "GET", "/names/pay.alice", "");

    assert_eq!(pay["resolves_to"], me.to_string());
    assert_eq!(pay["price_per_year"], Value::Null, "subnames are free");

    let (_, _, free_name) = request(address, "GET", "/names/carol", "");

    assert_eq!(free_name["status"], "available");
    assert_eq!(free_name["owner"], Value::Null);
    assert_eq!(free_name["price_per_year"], dev.prices[2].to_string(), "what registering it costs");
    assert_eq!(request(address, "GET", "/names/ab", "").0, 400);

    let (_, _, account) = request(address, "GET", &format!("/accounts/{me}"), "");

    assert_eq!(account["name"], "alice.dev", "the reverse lookup");

    // What it owns: the name and its subname, in name order, a page at a time.
    let (status, _, owned) = request(address, "GET", &format!("/accounts/{me}/names"), "");

    assert_eq!(status, 200, "{owned}");
    assert_eq!(owned["names"].as_array().unwrap().iter().map(|name| name["display"].as_str().unwrap()).collect::<Vec<_>>(), ["alice.dev", "pay.alice.dev"]);
    assert_eq!(owned["names"][0]["status"], "active");
    assert_eq!(owned["names"][1]["expires"], now + YEAR_MS, "a subname's is its name's");

    let (_, _, first) = request(address, "GET", &format!("/accounts/{me}/names?limit=1"), "");
    let (_, _, second) = request(address, "GET", &format!("/accounts/{me}/names?limit=1&cursor={}", first["next"].as_str().unwrap()), "");

    assert_eq!((first["names"][0]["name"].clone(), second["names"][0]["name"].clone()), (json!("alice"), json!("pay.alice")));
    assert_eq!(second["next"], Value::Null);

    // A chain without names says so.
    let nameless = Genesis { names: None, ..free() };
    let db = Arc::new(MemoryDb::default());
    Chain::open(db.clone(), &nameless).unwrap();
    let address = start(db, None, &nameless, "127.0.0.1:0".parse().unwrap(), ApiOptions::default()).unwrap();

    assert_eq!(request(address, "GET", "/names", "").0, 404);
    assert_eq!(request(address, "GET", &format!("/accounts/{me}"), "").2.get("name"), None);
}

#[cfg(feature = "ethereum")]
#[test]
fn ethereum_json_rpc_answers_as_ethereums_tools_ask() {
    use crate::crypto::ethereum::{EthereumKind, EthereumTx, chain_id, tx_hash};
    use crate::crypto::keypair::PrivateKey;

    let authority = DevAuthority::dev_keypair();
    let mut producer = AuthorityProducer::new(authority.clone(), SLOT);
    let db = Arc::new(MemoryDb::default());
    let genesis = Genesis::dev_authority(authority.public());
    let mut chain = Chain::open(db.clone(), &genesis).unwrap();
    let genesis_id = chain.genesis();
    let secret = [0x46; 32];
    let sender = Keypair::from_private(PrivateKey::Secp256k1(secret)).public().address();
    let Address::Secp256k1(from) = sender else { unreachable!() };
    let chain_id = chain_id(&chain.genesis());
    let ethereum = |nonce: u64, to: Option<[u8; 20]>, input: Vec<u8>| {
        EthereumTx {
            kind: EthereumKind::DynamicFee,
            chain_id,
            nonce,
            max_price: u256::from(10_000_000_000u64),
            tip: u256::ZERO,
            gas_limit: 3_000_000,
            to,
            value: u256::ZERO,
            input,
            access_list: Vec::new(),
            signature: [0; 64],
            parity: 0,
        }
        .signed(&secret)
        .encode()
    };
    let word = |value: u64| crate::structs::u256::u256::from(value).to_be_bytes().to_vec();
    let selector = |signature: &str| crate::crypto::secp256k1::keccak(signature.as_bytes())[..4].to_vec();
    let init = [from_hex(include_str!("../../contracts/evm/Counter.bin").trim()).unwrap(), word(5)].concat();
    let counter = crate::vm::evm::create_address(&sender, 0);
    let Address::Evm(counter_bytes) = counter else { unreachable!() };
    let deploy = ethereum(0, None, init);
    let bump = ethereum(1, Some(counter_bytes), [selector("bump(uint256)"), word(3)].concat());
    let fund = SignedTx::sign(
        Tx::Transfer { from: authority.public().address(), to: sender, amount: Amount::U128(10u128.pow(21)) },
        0,
        FeeOffer { gas_limit: 100_000, max_price: u256::from(10_000_000_000u64), tip: u256::ZERO },
        &authority,
        &genesis_id,
    );
    let blocks = [vec![fund], vec![SignedTx::from_ethereum(deploy.clone()).unwrap()], vec![SignedTx::from_ethereum(bump.clone()).unwrap()]];

    for txs in blocks {
        let now = chain.tip().timestamp + SLOT;
        let mut block = chain.build(txs, Vec::new(), producer.address(&[]), now).unwrap();
        block.seal = producer.seal(&block.header).unwrap();
        chain.import(&block, now).unwrap();
    }

    // The node's mempool holds the next one, sent but not yet in a block.
    let waiting = ethereum(2, Some([6; 20]), Vec::new());
    let node = fake_node_holding(BacklogLimits::default(), Some(vec![signed(0), SignedTx::from_ethereum(waiting.clone()).unwrap()]));
    let address = start(db, Some(node), &genesis, "127.0.0.1:0".parse().unwrap(), ApiOptions::default()).unwrap();
    let rpc = |method: &str, params: Value| {
        let (status, _, answer) = request(address, "POST", "/rpc", &json!({ "jsonrpc": "2.0", "id": 7, "method": method, "params": params }).to_string());
        assert_eq!((status, answer["id"].clone()), (200, json!(7)), "{answer}");

        answer
    };
    let hex = |bytes: &[u8]| to_hex(bytes);

    assert_eq!(rpc("eth_chainId", json!([]))["result"], format!("{chain_id:#x}"));
    assert_eq!(rpc("net_version", json!([]))["result"], chain_id.to_string());
    assert_eq!(rpc("eth_blockNumber", json!([]))["result"], "0x3");
    assert_eq!(rpc("eth_getTransactionCount", json!([hex(&from), "latest"]))["result"], "0x2");
    assert_eq!(rpc("eth_getStorageAt", json!([hex(&counter_bytes), "0x0", "latest"]))["result"], hex(&word(8)), "five and three");
    assert!(rpc("eth_getCode", json!([hex(&counter_bytes), "latest"]))["result"].as_str().unwrap().len() > 100);
    assert_eq!(rpc("eth_getCode", json!([hex(&from), "latest"]))["result"], "0x", "an account has none");

    // The deploy's receipt, by the keccak256 of its bytes, and the bump's log.
    let receipt = rpc("eth_getTransactionReceipt", json!([hex(&tx_hash(&deploy))]))["result"].clone();

    assert_eq!((receipt["status"].clone(), receipt["contractAddress"].clone()), (json!("0x1"), json!(hex(&counter_bytes))));
    assert_eq!(receipt["from"], hex(&from));

    let bumped = rpc("eth_getTransactionReceipt", json!([hex(&tx_hash(&bump))]))["result"].clone();
    let log = &bumped["logs"][0];

    assert_eq!(log["address"], hex(&counter_bytes));
    assert_eq!(log["topics"][0], hex(&crate::crypto::secp256k1::keccak(b"Bumped(address,uint256)")));
    assert_eq!(log["data"], hex(&word(8)));
    assert_eq!(rpc("eth_getTransactionReceipt", json!([hex(&[9; 32])]))["result"], Value::Null, "nothing by that hash");

    let tx = rpc("eth_getTransactionByHash", json!([hex(&tx_hash(&bump))]))["result"].clone();

    assert_eq!((tx["nonce"].clone(), tx["to"].clone(), tx["type"].clone()), (json!("0x1"), json!(hex(&counter_bytes)), json!("0x2")));

    let block = rpc("eth_getBlockByNumber", json!(["0x3", false]))["result"].clone();

    assert_eq!(block["transactions"], json!([hex(&tx_hash(&bump))]));
    assert_eq!(rpc("eth_getBlockByNumber", json!(["0x1", false]))["result"]["transactions"], json!([]), "the native transfer is not listed");
    assert_eq!(rpc("eth_getBlockByHash", json!([block["hash"], true]))["result"]["transactions"][0]["hash"], hex(&tx_hash(&bump)));

    let logs = rpc("eth_getLogs", json!([{ "fromBlock": "0x0", "toBlock": "latest", "address": hex(&counter_bytes) }]))["result"].clone();

    assert_eq!(logs.as_array().unwrap().len(), 1);
    assert_eq!(rpc("eth_getLogs", json!([{ "fromBlock": "0x0", "topics": [hex(&[0; 32])] }]))["result"], json!([]));

    // Many addresses and topics match as one does, the log found among them, up to the caps.
    let others: Vec<String> = (0..999u16).map(|n| hex(&[[0u8; 18].as_slice(), &n.to_be_bytes()].concat())).collect();
    let mut many = others.clone();
    many.push(hex(&counter_bytes));
    let topic = logs[0]["topics"][0].clone();

    assert_eq!(rpc("eth_getLogs", json!([{ "fromBlock": "0x0", "address": many, "topics": [[topic, hex(&[1; 32])]] }]))["result"], logs);
    let mut past = others.clone();
    past.extend([hex(&[9; 20]), hex(&[8; 20])]);

    assert!(rpc("eth_getLogs", json!([{ "fromBlock": "0x0", "address": past }]))["error"]["message"].as_str().unwrap().contains("at most 1000 addresses"));
    let topics: Vec<String> = (0..1_001u16).map(|n| hex(&[[0u8; 30].as_slice(), &n.to_be_bytes()].concat())).collect();

    assert!(rpc("eth_getLogs", json!([{ "fromBlock": "0x0", "topics": [topics] }]))["error"]["message"].as_str().unwrap().contains("at most 1000 topics"));
    assert!(rpc("eth_getLogs", json!([{ "fromBlock": "0x0", "topics": [null, null, null, null, null] }]))["error"]["message"].as_str().unwrap().contains("4 places"));
    assert_eq!(rpc("eth_getLogs", json!([{ "blockHash": block["hash"], "address": hex(&counter_bytes) }]))["result"], logs, "one block, by its hash");

    // Calls: what count() returns, and a revert with its reason and bytes.
    let call = |input: Vec<u8>| json!([{ "from": hex(&from), "to": hex(&counter_bytes), "data": hex(&input) }, "latest"]);

    assert_eq!(rpc("eth_call", call(selector("count()")))["result"], hex(&word(8)));

    let reverted = rpc("eth_call", call([selector("bump(uint256)"), word(0)].concat()));

    assert_eq!(reverted["error"]["code"], 3);
    assert_eq!(reverted["error"]["message"], "execution reverted: nothing to add");
    assert!(reverted["error"]["data"].as_str().unwrap().starts_with("0x08c379a0"), "Error(string), as Solidity encodes it");

    let estimate = rpc("eth_estimateGas", call([selector("bump(uint256)"), word(1)].concat()))["result"].as_str().unwrap().to_string();

    assert!(u64::from_str_radix(&estimate[2..], 16).unwrap() > 21_000, "{estimate}");
    // A transaction waiting in the mempool is found by its hash, in no block yet, with no receipt.
    let pending = rpc("eth_getTransactionByHash", json!([hex(&tx_hash(&waiting))]))["result"].clone();

    assert_eq!((pending["nonce"].clone(), pending["to"].clone()), (json!("0x2"), json!(hex(&[6; 20]))));
    assert_eq!((pending["blockHash"].clone(), pending["blockNumber"].clone(), pending["transactionIndex"].clone()), (Value::Null, Value::Null, Value::Null));
    assert_eq!(rpc("eth_getTransactionReceipt", json!([hex(&tx_hash(&waiting))]))["result"], Value::Null);
    assert_eq!(rpc("eth_getTransactionByHash", json!([hex(&[8; 32])]))["result"], Value::Null, "nothing by that hash, waiting or landed");

    // State at older blocks, through history: by number, tag or hash (EIP-1898), all alike.
    let at = |block: Value| rpc("eth_getBalance", json!([hex(&from), block]))["result"].clone();
    let genesis_hash = rpc("eth_getBlockByNumber", json!(["0x0", false]))["result"]["hash"].clone();

    assert_eq!(at(json!("0x0")), at(json!("earliest")));
    assert_eq!(at(json!("0x0")), at(json!({ "blockHash": genesis_hash })));
    assert_ne!(at(json!("0x0")), at(json!("latest")), "it has sent since genesis");
    assert!(rpc("eth_getBalance", json!([hex(&from), "0xffff"]))["error"]["message"].as_str().unwrap().contains("past the tip"));

    // Sending: one for this chain goes to the node; one for another is refused.
    let next = ethereum(2, Some([5; 20]), Vec::new());

    assert_eq!(rpc("eth_sendRawTransaction", json!([hex(&next)]))["result"], hex(&tx_hash(&next)));

    let elsewhere = EthereumTx { chain_id: chain_id + 1, ..crate::crypto::ethereum::decode(&next).unwrap() }.signed(&secret).encode();

    assert!(rpc("eth_sendRawTransaction", json!([hex(&elsewhere)]))["error"]["message"].as_str().unwrap().contains("not this one"));
    assert_eq!(rpc("eth_nope", json!([]))["error"]["code"], -32601);

    // A batch, answered in order.
    let (_, _, batch) = request(address, "POST", "/rpc", r#"[{"jsonrpc":"2.0","id":1,"method":"eth_chainId"},{"jsonrpc":"2.0","id":2,"method":"eth_syncing"}]"#);

    assert_eq!((batch[0]["id"].clone(), batch[1]["result"].clone()), (json!(1), json!(false)));
}

#[test]
fn pages_from_allowed_origins_may_call_the_api() {
    let db = Arc::new(MemoryDb::default());
    Chain::open(db.clone(), &free()).unwrap();
    let options = ApiOptions {
        cors: vec!["http://app.example".to_string()],
        ..ApiOptions::default()
    };
    let address = start(db, None, &free(), "127.0.0.1:0".parse().unwrap(), options).unwrap();
    let (status, head, _) = raw(address, "GET", "/status", "Origin: http://app.example\r\n");

    assert_eq!(status, 200);
    assert!(head.contains("access-control-allow-origin: http://app.example"), "{head}");

    let (status, head, _) = raw(
        address,
        "OPTIONS",
        "/txs",
        "Origin: http://app.example\r\nAccess-Control-Request-Method: POST\r\nAccess-Control-Request-Headers: content-type\r\n",
    );

    assert_eq!(status, 204);
    assert!(head.contains("access-control-allow-methods: get, post"), "{head}");
    assert!(head.contains("access-control-allow-headers: content-type"), "{head}");

    let (_, head, _) = raw(address, "GET", "/status", "Origin: http://elsewhere.example\r\n");

    assert!(!head.contains("access-control-allow-origin"), "{head}");
}

/// Who asks from afar, through a proxy on the node's machine, as nginx says.
const FROM_AFAR: &str = "X-Forwarded-For: 203.0.113.7\r\n";
const REPORT_TOKEN: &str = "0123456789abcdef0123456789abcdef";

fn sharing_node(sharing: super::sharing::Sharing) -> SocketAddr {
    let db = Arc::new(MemoryDb::default());
    Chain::open(db.clone(), &free()).unwrap();
    let options = ApiOptions { sharing, report_token: Some(REPORT_TOKEN.as_bytes().to_vec()), ..ApiOptions::default() };

    start(db, Some(fake_node()), &free(), "127.0.0.1:0".parse().unwrap(), options).unwrap()
}

fn node_as(address: SocketAddr, headers: &str) -> Value {
    let (status, _, body) = raw(address, "GET", "/node", headers);
    assert_eq!(status, 200);

    serde_json::from_slice(&body).unwrap()
}

#[test]
fn only_the_operator_lifts_bans() {
    let address = sharing_node(super::sharing::Sharing::default());
    // Exactly the request, nothing past its body: bytes the server never reads can have the
    // connection reset under the answer as it closes.
    let post = |body: &str, headers: &str| {
        let mut stream = TcpStream::connect(address).unwrap();
        write!(stream, "POST /node/unban HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n{headers}Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len()).unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).unwrap();
        let split = response.windows(4).position(|window| window == b"\r\n\r\n").unwrap();
        let status = String::from_utf8_lossy(&response[..split]).split(' ').nth(1).unwrap().parse::<u16>().unwrap();
        (status, serde_json::from_slice::<Value>(&response[split + 4..]).unwrap_or(Value::Null))
    };

    // From afar, without the token: refused, whatever it names.
    let (status, answer) = post(r#"{"all": true}"#, FROM_AFAR);
    assert_eq!(status, 403);
    assert!(answer["error"].as_str().unwrap().contains("operator"));

    // With the token, or from this machine: lifted.
    assert_eq!(post(r#"{"all": true}"#, &format!("{FROM_AFAR}Authorization: Bearer {REPORT_TOKEN}\r\n")), (200, json!({ "lifted": 2 })));
    let peer = libp2p::PeerId::random();
    assert_eq!(post(&format!(r#"{{"peer": "{peer}"}}"#), ""), (200, json!({ "lifted": 1 })));
    assert_eq!(post(r#"{"ip": "203.0.113.7"}"#, ""), (200, json!({ "lifted": 1 })));

    // What it names must read, and be one thing.
    assert_eq!(post(r#"{"peer": "nobody"}"#, "").0, 400);
    assert_eq!(post(r#"{"ip": "300.1.1.1"}"#, "").0, 400);
    assert_eq!(post(r#"{"all": true, "ip": "203.0.113.7"}"#, "").0, 400);
    assert_eq!(post(r#"{}"#, "").0, 400);
}

#[test]
fn a_node_shares_what_its_operator_chose_with_all_but_the_operator() {
    use super::sharing::{ReportField, ReportPreset, Sharing};

    let address = sharing_node(Sharing::default());
    let afar = node_as(address, FROM_AFAR);

    // By default, the addresses and the workers' names go; each keeps its place, for older apps.
    assert_eq!(afar["hidden"], json!(["peer-addresses", "listening", "worker-addresses", "worker-names"]));
    assert_eq!(afar["listening"], json!([]));
    assert_eq!(afar["peers"][0]["address"], "");
    assert_eq!(afar["mining"]["listening"], Value::Null);
    assert_eq!(afar["mining"]["workers"][0]["address"], "");
    assert_eq!(afar["mining"]["workers"][0]["name"], "worker 1");
    // The rest stays.
    assert_ne!(afar["peers"][0]["id"], "");
    assert_eq!(afar["peers"][0]["heard"]["blocks"], 3);
    assert_eq!(afar["mining"]["rate"], 300_001_000.0);
    assert_eq!(afar["counters"]["blocks_heard"], 3);
    assert_eq!(afar["uptime_ms"], 60_000);
    assert_eq!(afar["token_refused"], false);
    let text = afar.to_string();
    assert!(!text.contains("10.0.0.3") && !text.contains("4001") && !text.contains("4002") && !text.contains("\"rig\""), "{text}");

    // The operator: on this machine, or from afar with the token.
    for operator in [node_as(address, ""), node_as(address, &format!("{FROM_AFAR}Authorization: Bearer {REPORT_TOKEN}\r\n"))] {
        assert_eq!(operator["hidden"], json!([]));
        assert_eq!(operator["listening"], json!(["/ip4/127.0.0.1/udp/4001/quic-v1"]));
        assert_eq!(operator["mining"]["listening"], "0.0.0.0:4002");
        assert_eq!(operator["mining"]["workers"][0]["address"], "10.0.0.3:50000");
        assert_eq!(operator["mining"]["workers"][0]["name"], "rig");
        assert_eq!(operator["token_refused"], false);
    }

    // A token the node does not take shares no more, and says so.
    let wrong = node_as(address, &format!("{FROM_AFAR}Authorization: Bearer not-the-token\r\n"));
    assert_eq!((wrong["listening"].clone(), wrong["token_refused"].clone()), (json!([]), json!(true)));

    // Minimal: its own state alone.
    let minimal = node_as(sharing_node(Sharing::new(ReportPreset::Minimal, &[], &[]).unwrap()), FROM_AFAR);
    assert_eq!(minimal["peers"], json!([]));
    assert_eq!(minimal["mining"], Value::Null);
    assert_eq!(minimal["counters"], json!({}));
    assert_eq!(minimal["backlog"], Value::Null);
    assert_eq!(minimal["limits"], json!({}), "nor what its operator tuned");
    assert!(minimal["hidden"].as_array().unwrap().contains(&json!("limits")));
    assert_eq!(afar["limits"]["http.rate"], 0, "the public preset shows them");
    assert_eq!((minimal["uptime_ms"].clone(), minimal["producing"].clone(), minimal["mempool"]["count"].clone()), (json!(60_000), json!(true), json!(2)));

    // A preset, tweaked; and full.
    let tweaked = sharing_node(Sharing::new(ReportPreset::Public, &[ReportField::Uptime, ReportField::Role, ReportField::PeerIds, ReportField::Devices, ReportField::WorkerPayouts], &[ReportField::PeerAddresses]).unwrap());
    let some = node_as(tweaked, FROM_AFAR);
    assert_eq!((some["uptime_ms"].clone(), some["started"].clone(), some["producing"].clone()), (json!(0), json!(0), json!(false)));
    assert_eq!((some["peers"][0]["id"].clone(), some["peers"][0]["address"].clone()), (json!(""), json!("/ip4/10.0.0.2/udp/4001/quic-v1")));
    assert_eq!((some["mining"]["devices"].clone(), some["mining"]["workers"][0]["devices"].clone()), (json!([]), json!([])));
    assert_eq!(some["mining"]["workers"][0]["payout"], Value::Null);
    assert_eq!(node_as(sharing_node(Sharing::new(ReportPreset::Full, &[], &[]).unwrap()), FROM_AFAR)["hidden"], json!([]));

    // GET /mining's count of workers goes with them; its port does not, as miners need it.
    let quiet = sharing_node(Sharing::new(ReportPreset::Public, &[ReportField::Workers], &[]).unwrap());
    let (_, _, about) = raw(quiet, "GET", "/mining", FROM_AFAR);
    let about: Value = serde_json::from_slice(&about).unwrap();
    assert_eq!((about["workers"].clone(), about["port"].clone()), (Value::Null, json!(4002)));
    let (_, _, about) = raw(quiet, "GET", "/mining", "");
    assert_eq!(serde_json::from_slice::<Value>(&about).unwrap()["workers"], 1);

    // A page from elsewhere may send the token.
    let cors = ApiOptions { cors: vec!["http://app.example".to_string()], ..ApiOptions::default() };
    let db = Arc::new(MemoryDb::default());
    Chain::open(db.clone(), &free()).unwrap();
    let elsewhere = start(db, Some(fake_node()), &free(), "127.0.0.1:0".parse().unwrap(), cors).unwrap();
    let (_, head, _) = raw(elsewhere, "OPTIONS", "/node", "Origin: http://app.example\r\nAccess-Control-Request-Method: GET\r\nAccess-Control-Request-Headers: authorization\r\n");
    assert!(head.contains("access-control-allow-headers: content-type, authorization"), "{head}");
}

#[cfg(feature = "pool")]
#[test]
fn a_miner_s_workers_are_named_as_the_node_chose() {
    use super::sharing::{ReportField, ReportPreset, Sharing};

    let miner = Address::Ed25519([4; 32]);
    let (inbox, events) = std::sync::mpsc::channel();
    let backlog = Arc::new(Backlog::new(BacklogLimits::default()));

    std::thread::spawn(move || {
        for event in events {
            if let NetEvent::Report { txs, reply } = event {
                let mut report = fake_report(txs);
                let mining = report.mining.as_mut().unwrap();
                mining.workers[0].payout = Some(miner);
                mining.pool = Some(crate::consensus::PoolReport { producer: Some(Address::Ed25519([1; 32])), ..Default::default() });
                let _ = reply.send(report);
            }
        }
    });

    let serving = |sharing: Sharing| {
        let db = Arc::new(MemoryDb::default());
        Chain::open(db.clone(), &free()).unwrap();
        let handle = NodeHandle::new(inbox.clone(), backlog.clone());

        start(db, Some(handle), &free(), "127.0.0.1:0".parse().unwrap(), ApiOptions { sharing, ..ApiOptions::default() }).unwrap()
    };
    let workers = |address, headers: &str| {
        let (status, _, body) = raw(address, "GET", &format!("/pool/{miner}"), headers);
        assert_eq!(status, 200);

        serde_json::from_slice::<Value>(&body).unwrap()["workers"].clone()
    };
    let public = serving(Sharing::default());

    assert_eq!(workers(public, FROM_AFAR)[0]["name"], "worker 1");
    assert_eq!(workers(public, "")[0]["name"], "rig", "its operator sees its name");
    assert_eq!(workers(serving(Sharing::new(ReportPreset::Public, &[ReportField::Workers], &[]).unwrap()), FROM_AFAR), json!([]));
}

#[test]
fn the_explorer_and_the_wallet_are_served_beside_the_routes() {
    let dir = std::env::temp_dir().join(format!("chainv4-explorer-{}", std::process::id()));
    let wallet = std::env::temp_dir().join(format!("chainv4-wallet-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("assets")).unwrap();
    std::fs::create_dir_all(&wallet).unwrap();
    std::fs::write(dir.join("index.html"), "<html>explorer</html>").unwrap();
    std::fs::write(dir.join("assets/app.wasm"), b"\0asm").unwrap();
    std::fs::write(wallet.join("index.html"), "<html>wallet</html>").unwrap();
    std::fs::write(wallet.join("wallet.js"), "export {}").unwrap();
    let db = Arc::new(MemoryDb::default());
    Chain::open(db.clone(), &free()).unwrap();
    let options = ApiOptions {
        explorer: Some(dir.clone()),
        wallet: Some(wallet.clone()),
        ..ApiOptions::default()
    };
    let address = start(db, None, &free(), "127.0.0.1:0".parse().unwrap(), options).unwrap();
    let (status, head, body) = raw(address, "GET", "/", "");

    assert_eq!(status, 200);
    assert!(head.contains("content-type: text/html"), "{head}");
    assert_eq!(body, b"<html>explorer</html>");

    let (status, head, body) = raw(address, "GET", "/assets/app.wasm", "");

    assert_eq!(status, 200);
    assert!(head.contains("content-type: application/wasm"), "{head}");
    assert_eq!(body, b"\0asm");
    assert_eq!(raw(address, "GET", "/assets/../index.html", "").0, 404);
    assert_eq!(raw(address, "GET", "/missing.js", "").0, 404);
    assert_eq!(ok(address, "/status")["height"], 0);

    // The wallet under /wallet/, its files relative to a page that ends in a slash.
    let (status, head, _) = raw(address, "GET", "/wallet", "");

    assert_eq!(status, 308);
    assert!(head.contains("location: /wallet/"), "{head}");
    assert_eq!(raw(address, "GET", "/wallet/", "").2, b"<html>wallet</html>");

    let (status, head, _) = raw(address, "GET", "/wallet/wallet.js", "");

    assert_eq!(status, 200);
    assert!(head.contains("content-type: text/javascript"), "{head}");
    assert_eq!(raw(address, "GET", "/wallet/../index.html", "").0, 404);
    assert_eq!(raw(address, "GET", "/wallet/missing.js", "").0, 404, "never the explorer's file");

    std::fs::remove_dir_all(dir).unwrap();
    std::fs::remove_dir_all(wallet).unwrap();
}

#[cfg(feature = "stream")]
#[test]
fn the_stream_keeps_to_itself_what_the_node_does() {
    use super::sharing::{ReportPreset, Sharing};
    use crate::net::NetLink;
    use crate::node::Node;

    /// What comes of a peer connecting, to one listening from afar.
    fn heard(sharing: Sharing) -> String {
        let db = Arc::new(MemoryDb::default());
        let (link, fake) = NetLink::pair();
        let mut node = Node::new(db.clone(), &free(), None, link).unwrap().stream(16);
        let address = start(db, Some(node.handle()), &free(), "127.0.0.1:0".parse().unwrap(), ApiOptions { sharing, ..ApiOptions::default() }).unwrap();
        let listen = |headers: &str| {
            let mut stream = TcpStream::connect(address).unwrap();
            stream.set_read_timeout(Some(std::time::Duration::from_secs(2))).unwrap();
            write!(stream, "GET /stream HTTP/1.1\r\nHost: localhost\r\n{headers}\r\n").unwrap();
            let mut seen = String::new();
            let mut buffer = [0; 4096];

            while !seen.contains("\r\n\r\n") {
                let read = stream.read(&mut buffer).unwrap();
                seen.push_str(&String::from_utf8_lossy(&buffer[..read]));
            }

            stream
        };
        let (mut afar, mut here) = (listen(FROM_AFAR), listen(""));
        fake.events.send(NetEvent::PeerUp { peer: libp2p::PeerId::random(), address: "/ip4/10.0.0.9/udp/1/quic-v1".parse().unwrap() }).unwrap();
        node.step(SLOT).unwrap();
        let mut buffer = [0; 4096];

        // This machine hears it first: then the other has had its chance to.
        let mut seen = String::new();
        while !seen.contains("event: peer_up") {
            let read = here.read(&mut buffer).unwrap();
            seen.push_str(&String::from_utf8_lossy(&buffer[..read]));
        }
        assert!(seen.contains("10.0.0.9"), "{seen}");

        afar.set_read_timeout(Some(std::time::Duration::from_millis(500))).unwrap();
        match afar.read(&mut buffer) {
            Ok(read) => String::from_utf8_lossy(&buffer[..read]).into_owned(),
            Err(_) => String::new(),
        }
    }

    let public = heard(Sharing::default());
    assert!(public.contains("event: peer_up") && !public.contains("10.0.0.9"), "{public}");
    assert!(!heard(Sharing::new(ReportPreset::Minimal, &[], &[]).unwrap()).contains("peer_up"), "no peers, no word of them");
}

#[cfg(feature = "stream")]
#[test]
fn a_streaming_node_pushes_what_it_does() {
    use crate::net::NetLink;
    use crate::node::Node;

    let db = Arc::new(MemoryDb::default());
    let (link, fake) = NetLink::pair();
    let mut node = Node::new(db.clone(), &free(), None, link).unwrap().stream(16);
    let address = start(db.clone(), Some(node.handle()), &free(), "127.0.0.1:0".parse().unwrap(), ApiOptions::default()).unwrap();
    let mut stream = TcpStream::connect(address).unwrap();
    stream.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
    write!(stream, "GET /stream HTTP/1.1\r\nHost: localhost\r\n\r\n").unwrap();
    let mut seen = String::new();
    let mut buffer = [0; 4096];

    // Subscribed once the answer's head is back.
    while !seen.contains("\r\n\r\n") {
        let read = stream.read(&mut buffer).unwrap();
        seen.push_str(&String::from_utf8_lossy(&buffer[..read]));
    }

    assert!(seen.to_ascii_lowercase().contains("content-type: text/event-stream"), "{seen}");

    let peer = libp2p::PeerId::random();
    fake.events
        .send(NetEvent::PeerUp {
            peer,
            address: "/ip4/10.0.0.9/udp/1/quic-v1".parse().unwrap(),
        })
        .unwrap();
    node.step(SLOT).unwrap();

    while !seen.contains("event: peer_up") {
        let read = stream.read(&mut buffer).unwrap();
        seen.push_str(&String::from_utf8_lossy(&buffer[..read]));
    }

    assert!(seen.contains(&peer.to_string()), "{seen}");
    assert!(seen.contains("/ip4/10.0.0.9/udp/1/quic-v1"), "this machine sees where it connects from: {seen}");

    let quiet = start(db, Some(fake_node()), &free(), "127.0.0.1:0".parse().unwrap(), ApiOptions::default()).unwrap();

    assert_eq!(get(quiet, "/stream").0, 404);
}

const BUMPER: &str = r#"
local M = {}

function M.bump(by)
    local count = tonumber(storage.get("count") or "0") + by
    storage.set("count", tostring(count))
    events.emit("Bumped", count)
    return count
end

return M
"#;

#[test]
fn read_only_calls_run_on_a_copy_that_is_thrown_away() {
    let mut fixture = serve();
    let authority = DevAuthority::dev_keypair();
    let deploy = SignedTx::sign(
        Tx::Deploy {
            code: Code::Lua(BUMPER.to_string()),
        },
        2,
        FeeOffer::NONE,
        &authority,
        &fixture.chain.genesis(),
    );
    produce(&mut fixture.chain, vec![deploy]);
    let bumper = contract_address(&authority.public().address(), 2, HashType::default_chain());
    let call = |body: Value| request(fixture.address, "POST", "/call", &body.to_string());
    let bump = json!({ "contract": bumper.to_string(), "function": "bump", "args": [{ "type": "number", "value": 5 }] });

    for _ in 0..2 {
        let (status, _, answer) = call(bump.clone());

        assert_eq!(status, 200, "{answer}");
        assert_eq!(answer["status"], "returned");
        assert_eq!(answer["values"], json!([{ "type": "i64", "value": "5" }]));
        assert_eq!(answer["events"][0]["name"], "Bumped");
        assert_eq!(answer["trace"]["hops"][0]["function"], "bump");
        assert!(answer["fuel_used"].as_u64().unwrap() > 0);
    }

    let (_, _, missing) = call(json!({ "contract": bumper.to_string(), "function": "nope" }));

    assert_eq!(missing["status"], "failed");
    assert_eq!(call(json!({ "contract": fixture.bob.to_string(), "function": "bump" })).0, 404);
    assert_eq!(call(json!({ "contract": bumper.to_string(), "function": "bump", "args": [{ "type": "i64", "value": "x" }] })).0, 400);
    assert_eq!(call(json!({ "function": "bump" })).0, 400);
}

#[test]
fn status_names_the_coin() {
    let fixture = serve();

    assert_eq!(ok(fixture.address, "/status")["coin"], json!({ "symbol": "DEV", "decimals": 18 }));
}

#[test]
fn every_integer_type_round_trips_through_json_as_text() {
    use crate::structs::u256::{i256, u256};

    for argument in [
        TxCallArgumentType::I64(i64::MIN),
        TxCallArgumentType::U64(u64::MAX),
        TxCallArgumentType::I128(i128::MIN),
        TxCallArgumentType::U128(u128::MAX),
        TxCallArgumentType::I256(i256::MIN),
        TxCallArgumentType::I256(i256::MAX),
        TxCallArgumentType::U256(u256::MAX),
    ] {
        let written = json::argument(&argument);

        assert!(written["value"].is_string(), "{written}: past 2^53, so text");
        assert_eq!(json::parse_argument(&written), Ok(argument));
    }

    assert_eq!(json::argument(&TxCallArgumentType::I256(i256::new(-5))), json!({ "type": "i256", "value": "-5" }));
    assert_eq!(json::parse_argument(&json!({ "type": "i256", "value": -5 })), Ok(TxCallArgumentType::I256(i256::new(-5))));
    assert!(json::parse_argument(&json!({ "type": "i256", "value": "five" })).unwrap_err().contains("not an i256: five"));
}

#[test]
fn nil_is_written_as_null() {
    assert_eq!(json::argument(&TxCallArgumentType::Nil), json!({ "type": "nil", "value": null }));
    assert_eq!(json::parse_argument(&json!({ "type": "nil" })), Ok(TxCallArgumentType::Nil));
    assert_eq!(json::parse_argument(&json!({ "type": "nil", "value": null })), Ok(TxCallArgumentType::Nil));
}

const GREETER: &str = r#"
local M = {}

function M.greet(who)
    events.emit("Hello", who)
end

return M
"#;

#[test]
fn tokens_are_listed_from_the_registry() {
    let authority = DevAuthority::dev_keypair();
    let db = Arc::new(MemoryDb::default());
    let mut chain = Chain::open(db.clone(), &free()).unwrap();
    let genesis_id = chain.genesis();
    let address = start(db.clone(), None, &free(), "127.0.0.1:0".parse().unwrap(), ApiOptions::default()).unwrap();
    let deployed = |nonce: u64| contract_address(&authority.public().address(), nonce, HashType::default_chain());
    let (token, greeter) = (deployed(0), deployed(1));
    let deploy = |code: &str, nonce: u64| SignedTx::sign(Tx::Deploy { code: Code::Lua(code.to_string()) }, nonce, FeeOffer::NONE, &authority, &genesis_id);
    produce(&mut chain, vec![deploy(include_str!("../../contracts/lua/token.lua"), 0), deploy(GREETER, 1)]);

    assert_eq!(
        ok(address, "/status")["token_functions"],
        json!(["allowance", "approve", "balance_of", "total_supply", "transfer", "transfer_from"])
    );
    assert_eq!(
        ok(address, "/tokens"),
        json!({
            "tokens": [{ "address": token.to_string(), "name": "Example Token", "symbol": "EXT", "decimals": 18 }],
            "next": null,
        })
    );
    assert_eq!(ok(address, "/tokens?symbol=ex")["tokens"].as_array().unwrap().len(), 1, "symbols are searched by how they start");
    assert!(ok(address, "/tokens?symbol=abc")["tokens"].as_array().unwrap().is_empty());
    assert_eq!(ok(address, &format!("/contracts/{token}"))["token"]["symbol"], "EXT");
    assert_eq!(ok(address, &format!("/contracts/{greeter}"))["token"], Value::Null, "a contract that is not a token");
    assert_eq!(get(address, "/tokens?cursor=zz").0, 400);
}

#[cfg(feature = "secp256k1")]
#[test]
fn status_names_the_federation_in_force() {
    let empty = Arc::new(MemoryDb::default());
    Chain::open(empty.clone(), &free()).unwrap();
    let bare = start(empty, None, &free(), "127.0.0.1:0".parse().unwrap(), ApiOptions::default()).unwrap();
    let db = Arc::new(MemoryDb::default());
    let federation = crate::registry::federation::Federation {
        signers: vec![crate::crypto::secp256k1::public_key(&[1; 32]).unwrap()],
        threshold: 1,
        chain_depth: 20,
        evm_depth: 12,
        bitcoin_depth: 6,
        btc: None,
    };
    let genesis = Genesis { federation: Some(federation.clone()), ..free() };
    Chain::open(db.clone(), &genesis).unwrap();
    let address = start(db, None, &genesis, "127.0.0.1:0".parse().unwrap(), ApiOptions::default()).unwrap();
    let status = ok(address, "/status");

    assert_eq!(ok(bare, "/status")["federation"], Value::Null);
    assert_eq!((status["federation"]["version"].as_u64(), status["federation"]["threshold"].as_u64()), (Some(0), Some(1)));
    assert_eq!(status["federation"]["signers"][0], crate::structs::hex::to_hex(&federation.signers[0]));
}

#[test]
fn status_names_the_directory() {
    use crate::registry::directory::{Directory, EvmGateway};

    let empty = Arc::new(MemoryDb::default());
    Chain::open(empty.clone(), &free()).unwrap();
    let bare = start(empty, None, &free(), "127.0.0.1:0".parse().unwrap(), ApiOptions::default()).unwrap();
    let db = Arc::new(MemoryDb::default());
    let directory = Directory { gateway: Some(Address::Contract([1; 32])), evm_gateways: vec![EvmGateway { chain_id: 8453, gateway: [4; 20] }], nft_factory: Some(Address::Contract([2; 32])), ..Directory::default() };
    let genesis = Genesis { directory: Some(directory), ..free() };
    Chain::open(db.clone(), &genesis).unwrap();
    let address = start(db, None, &genesis, "127.0.0.1:0".parse().unwrap(), ApiOptions::default()).unwrap();
    let status = ok(address, "/status");

    assert_eq!(ok(bare, "/status")["directory"], Value::Null);
    assert_eq!(status["directory"]["gateway"], Address::Contract([1; 32]).to_string());
    assert_eq!(status["directory"]["nft_factory"], Address::Contract([2; 32]).to_string());
    assert_eq!(status["directory"]["swap"], Value::Null);
    assert_eq!((status["directory"]["evm_gateways"][0]["chain_id"].as_u64(), status["directory"]["evm_gateways"][0]["gateway"].as_str()), (Some(8453), Some(format!("0x{}", "04".repeat(20)).as_str())));
}

#[test]
fn collections_are_listed_from_the_registry() {
    let authority = DevAuthority::dev_keypair();
    let db = Arc::new(MemoryDb::default());
    let mut chain = Chain::open(db.clone(), &free()).unwrap();
    let genesis_id = chain.genesis();
    let address = start(db.clone(), None, &free(), "127.0.0.1:0".parse().unwrap(), ApiOptions::default()).unwrap();
    let deployed = |nonce: u64| contract_address(&authority.public().address(), nonce, HashType::default_chain());
    let (nft, multi, greeter) = (deployed(0), deployed(1), deployed(2));
    let deploy = |code: &str, nonce: u64| SignedTx::sign(Tx::Deploy { code: Code::Lua(code.to_string()) }, nonce, FeeOffer::NONE, &authority, &genesis_id);
    produce(
        &mut chain,
        vec![
            deploy(include_str!("../../contracts/lua/nft.lua"), 0),
            deploy(include_str!("../../contracts/lua/multi.lua"), 1),
            deploy(GREETER, 2),
        ],
    );

    assert_eq!(ok(address, "/status")["collections"]["nft"].as_array().unwrap().len(), 12);
    assert_eq!(ok(address, "/status")["collections"]["multi"][0], "balance_of");
    assert_eq!(
        ok(address, "/collections"),
        json!({
            "collections": [
                { "address": nft.to_string(), "kind": "nft", "name": "Example Collection", "symbol": "EXC" },
                { "address": multi.to_string(), "kind": "multi", "name": "Example Items", "symbol": "EXI" },
            ],
            "next": null,
        })
    );
    assert_eq!(ok(address, "/collections?symbol=exi")["collections"][0]["kind"], "multi");
    assert_eq!(ok(address, "/collections?limit=1")["collections"].as_array().unwrap().len(), 1);
    assert!(ok(address, "/tokens")["tokens"].as_array().unwrap().is_empty(), "collections are not tokens");
    assert_eq!(ok(address, &format!("/contracts/{nft}"))["collection"]["kind"], "nft");
    assert_eq!(ok(address, &format!("/contracts/{greeter}"))["collection"], Value::Null);
    assert_eq!(get(address, "/collections?cursor=zz").0, 400);
}

#[test]
fn events_are_listed_from_the_index() {
    let authority = DevAuthority::dev_keypair();
    let db = Arc::new(MemoryDb::default());
    let mut chain = Chain::open(db.clone(), &free()).unwrap();
    let genesis_id = chain.genesis();
    let address = start(db.clone(), None, &free(), "127.0.0.1:0".parse().unwrap(), ApiOptions::default()).unwrap();
    let greeter = contract_address(&authority.public().address(), 0, HashType::default_chain());

    assert_eq!(get(address, &format!("/events?contract={greeter}")).0, 404, "no index yet");

    chain.keep_index(IndexKind::Events, None).unwrap();
    let bob = Keypair::generate(authority.public().key_type()).unwrap().public().address();
    let greet = |nonce: u64| {
        let tx = Tx::ContractCall {
            to: greeter,
            func: Hash::Xxh3_64(crate::vm::selector("greet")),
            amount: Amount::U128(0),
            data: vec![TxCallArgumentType::Address(bob)],
        };

        SignedTx::sign(tx, nonce, FeeOffer::NONE, &authority, &genesis_id)
    };
    produce(&mut chain, vec![SignedTx::sign(Tx::Deploy { code: Code::Lua(GREETER.to_string()) }, 0, FeeOffer::NONE, &authority, &genesis_id)]);
    produce(&mut chain, vec![greet(1)]);
    let last = produce(&mut chain, vec![greet(2)]);
    let listing = ok(address, &format!("/events?contract={greeter}"));

    assert_eq!((listing["indexed_from"].clone(), listing["catching_up_to"].clone()), (json!(1), json!(0)), "genesis is still to come");
    assert_eq!(listing["next"], Value::Null);
    assert_eq!(listing["events"].as_array().unwrap().len(), 2);
    assert_eq!(
        listing["events"][0],
        json!({
            "contract": greeter.to_string(),
            "name": "Hello",
            "data": [{ "type": "address", "value": bob.to_string() }],
            "tx": greet(2).id(HashType::default_chain()).to_string(),
            "block": last.id().to_string(),
            "height": 3,
            "index": 0,
            "position": 0,
        })
    );
    assert_eq!(ok(address, &format!("/events?address={bob}"))["events"], listing["events"]);

    // A page at a time.
    let first = ok(address, &format!("/events?contract={greeter}&name=Hello&limit=1"));
    let cursor = first["next"].as_str().unwrap();
    let second = ok(address, &format!("/events?contract={greeter}&name=Hello&limit=1&cursor={cursor}"));

    assert_eq!((first["events"][0]["height"].clone(), second["events"][0]["height"].clone()), (json!(3), json!(2)));
    assert_eq!(second["next"], Value::Null);
    assert!(ok(address, &format!("/events?contract={greeter}&name=Bye"))["events"].as_array().unwrap().is_empty());

    for bad in ["/events", &format!("/events?contract={greeter}&address={bob}"), "/events?address=nope", "/events?name=Hello"] {
        assert_eq!(get(address, bad).0, 400, "{bad}");
    }

    while chain.catch_up_index(IndexKind::Events).unwrap() {}
    let caught_up = ok(address, &format!("/events?contract={greeter}"));

    assert_eq!((caught_up["indexed_from"].clone(), caught_up["catching_up_to"].clone()), (json!(0), Value::Null));
}

#[cfg(feature = "evm")]
#[test]
fn evm_logs_are_listed_by_their_topic_and_by_the_addresses_in_them() {
    let authority = DevAuthority::dev_keypair();
    let me = authority.public().address();
    let db = Arc::new(MemoryDb::default());
    let mut chain = Chain::open(db.clone(), &free()).unwrap();
    let genesis_id = chain.genesis();
    let address = start(db.clone(), None, &free(), "127.0.0.1:0".parse().unwrap(), ApiOptions::default()).unwrap();
    let word = |value: u64| crate::structs::u256::u256::from(value).to_be_bytes().to_vec();
    let init = [from_hex(include_str!("../../contracts/evm/Counter.bin").trim()).unwrap(), word(5)].concat();
    let counter = crate::vm::evm::create_address(&me, 0);

    chain.keep_index(IndexKind::Events, None).unwrap();
    produce(&mut chain, vec![SignedTx::sign(Tx::Deploy { code: Code::Evm(init) }, 0, FeeOffer::NONE, &authority, &genesis_id)]);

    let bump = |nonce: u64, by: u64| {
        let input = [crate::crypto::secp256k1::keccak(b"bump(uint256)")[..4].to_vec(), word(by)].concat();

        SignedTx::sign(Tx::EvmCall { to: counter, value: Amount::U128(0), input }, nonce, FeeOffer::NONE, &authority, &genesis_id)
    };
    produce(&mut chain, vec![bump(1, 2)]);
    produce(&mut chain, vec![bump(2, 3)]);

    let bumped = to_hex(&crate::abi::signature_topic("Bumped(address,uint256)").unwrap());
    let by_me = to_hex(&[vec![0; 12], crate::abi::evm_address(&me).to_vec()].concat());
    let heights = |path: &str| ok(address, path)["events"].as_array().unwrap().iter().map(|event| event["height"].as_u64().unwrap()).collect::<Vec<_>>();

    // By its signature, from any contract or from one, by the name the EVM gives logs too.
    let listing = ok(address, &format!("/events?topic={bumped}"));

    assert_eq!(listing["topics"], true, "a client can tell this node reads a topic it is given");
    assert_eq!(listing["events"][0]["name"], "log");
    assert_eq!(listing["events"][0]["data"][0], json!({ "type": "bytes", "value": bumped }));
    assert_eq!(heights(&format!("/events?topic={bumped}")), [3, 2]);
    assert_eq!(heights(&format!("/events?contract={counter}&topic={bumped}")), [3, 2]);
    assert_eq!(heights(&format!("/events?contract={counter}&name=log")), [3, 2]);
    assert!(heights(&format!("/events?contract={counter}&name=LOG")).is_empty(), "names match by bytes");
    assert!(heights(&format!("/events?contract=evm:0x{}&topic={bumped}", "09".repeat(20))).is_empty());

    // By the address in its topic, as the account it is and as that topic.
    assert_eq!(heights(&format!("/events?address={me}")), [3, 2]);
    assert_eq!(heights(&format!("/events?topic={by_me}")), [3, 2]);
    assert!(heights(&format!("/events?address=secp256k1:0x{}", "09".repeat(20))).is_empty());

    // A page at a time, within the same limits as any listing.
    let first = ok(address, &format!("/events?address={me}&limit=1"));
    let cursor = first["next"].as_str().unwrap();
    let second = ok(address, &format!("/events?address={me}&limit=1&cursor={cursor}"));

    assert_eq!((first["events"][0]["height"].clone(), second["events"][0]["height"].clone(), second["next"].clone()), (json!(3), json!(2), Value::Null));

    let first = ok(address, &format!("/events?topic={bumped}&limit=1"));
    let second = ok(address, &format!("/events?topic={bumped}&limit=1&cursor={}", first["next"].as_str().unwrap()));
    assert_eq!(second["events"][0]["height"], 2);
    assert_eq!(heights(&format!("/events?topic={bumped}&limit=0")).len(), 1, "at least one");

    // What it cannot answer it refuses, rather than answering something else.
    for bad in [
        "/events?topic=0x01".to_string(),
        "/events?topic=nope".to_string(),
        format!("/events?contract={counter}&name=log&topic={bumped}"),
        format!("/events?address={me}&topic={bumped}"),
        format!("/events?name=log&topic={bumped}"),
    ] {
        assert_eq!(get(address, &bad).0, 400, "{bad}");
    }
}

#[test]
fn an_address_s_transactions_are_listed_from_the_index() {
    let authority = DevAuthority::dev_keypair();
    let me = authority.public().address();
    let db = Arc::new(MemoryDb::default());
    let mut chain = Chain::open(db.clone(), &free()).unwrap();
    let genesis_id = chain.genesis();
    let address = start(db.clone(), None, &free(), "127.0.0.1:0".parse().unwrap(), ApiOptions::default()).unwrap();
    let bob = Keypair::generate(authority.public().key_type()).unwrap().public().address();
    let pay = |nonce: u64| SignedTx::sign(Tx::Transfer { from: me, to: bob, amount: Amount::U128(7) }, nonce, FeeOffer::NONE, &authority, &genesis_id);

    assert_eq!(get(address, &format!("/accounts/{bob}/txs")).0, 404, "no index yet");

    chain.keep_index(IndexKind::Addresses, None).unwrap();
    produce(&mut chain, vec![pay(0)]);
    let last = produce(&mut chain, vec![pay(1)]);
    let listing = ok(address, &format!("/accounts/{bob}/txs"));

    assert_eq!((listing["indexed_from"].clone(), listing["catching_up_to"].clone()), (json!(1), json!(0)), "genesis is still to come");
    assert_eq!(listing["next"], Value::Null);
    assert_eq!(listing["txs"].as_array().unwrap().len(), 2);
    assert_eq!(listing["txs"][0]["id"], json!(pay(1).id(HashType::default_chain()).to_string()));
    assert_eq!((listing["txs"][0]["block"].clone(), listing["txs"][0]["height"].clone()), (json!(last.id().to_string()), json!(2)));
    assert_eq!(listing["txs"][0]["tx"]["type"], "transfer", "{listing}");
    assert_eq!(listing["txs"][0]["receipt"]["status"], "done");
    assert_eq!(ok(address, &format!("/accounts/{me}/txs"))["txs"], listing["txs"], "the sender's, too");

    // A page at a time.
    let first = ok(address, &format!("/accounts/{bob}/txs?limit=1"));
    let cursor = first["next"].as_str().unwrap();
    let second = ok(address, &format!("/accounts/{bob}/txs?limit=1&cursor={cursor}"));

    assert_eq!((first["txs"][0]["height"].clone(), second["txs"][0]["height"].clone()), (json!(2), json!(1)));
    assert_eq!(second["next"], Value::Null);

    for bad in ["/accounts/nope/txs", &format!("/accounts/{bob}/txs?cursor=zz"), &format!("/accounts/{bob}/txs?limit=x")] {
        assert_eq!(get(address, bad).0, 400, "{bad}");
    }

    while chain.catch_up_index(IndexKind::Addresses).unwrap() {}

    assert_eq!(ok(address, &format!("/accounts/{bob}/txs"))["catching_up_to"], Value::Null);
}

/// Heights in a listing, in order.
fn heights(listing: &Value) -> Vec<u64> {
    listing["txs"].as_array().unwrap().iter().map(|tx| tx["height"].as_u64().unwrap()).collect()
}

#[test]
fn a_listing_is_filtered_by_kind_direction_counterparty_token_and_when_and_pages_hold_matches() {
    let authority = DevAuthority::dev_keypair();
    let me = authority.public().address();
    let db = Arc::new(MemoryDb::default());
    let mut chain = Chain::open(db.clone(), &free()).unwrap();
    let genesis_id = chain.genesis();
    let address = start(db.clone(), None, &free(), "127.0.0.1:0".parse().unwrap(), ApiOptions::default()).unwrap();
    let bob_key = Keypair::generate(authority.public().key_type()).unwrap();
    let (bob, carol) = (bob_key.public().address(), Keypair::generate(authority.public().key_type()).unwrap().public().address());
    let contract = contract_address(&me, 1, HashType::default_chain());

    chain.keep_index(IndexKind::Addresses, None).unwrap();
    // Height 1: a transfer to bob; 2: one back; 3: a deploy; 4: a call; 5: a transfer to carol.
    produce(&mut chain, vec![SignedTx::sign(Tx::Transfer { from: me, to: bob, amount: Amount::U128(7) }, 0, FeeOffer::NONE, &authority, &genesis_id)]);
    produce(&mut chain, vec![SignedTx::sign(Tx::Transfer { from: bob, to: me, amount: Amount::U128(3) }, 0, FeeOffer::NONE, &bob_key, &genesis_id)]);
    produce(&mut chain, vec![SignedTx::sign(Tx::Deploy { code: Code::Lua(COUNTER.to_string()) }, 1, FeeOffer::NONE, &authority, &genesis_id)]);
    produce(&mut chain, vec![SignedTx::sign(Tx::ContractCall { to: contract, func: Hash::Xxh3_64(crate::vm::selector("increment")), amount: Amount::U128(0), data: vec![TxCallArgumentType::I64(1)] }, 2, FeeOffer::NONE, &authority, &genesis_id)]);
    let last = produce(&mut chain, vec![SignedTx::sign(Tx::Transfer { from: me, to: carol, amount: Amount::U128(5) }, 3, FeeOffer::NONE, &authority, &genesis_id)]);
    let time = |height: u64| chain.tip().timestamp - (5 - height) * SLOT;

    assert_eq!(last.header.timestamp, time(5));

    let list = |query: &str| ok(address, &format!("/accounts/{me}/txs{query}"));
    let every = list("");

    assert_eq!(heights(&every), [5, 4, 3, 2, 1]);
    assert_eq!(every["applied"], json!([]), "a new node says what it went by, an older one says nothing");
    assert_eq!((every["limited"].clone(), every["scanned"].clone()), (json!(false), json!(5)));
    assert_eq!(every["txs"][0]["time"], json!(time(5)));

    // By what kind of transaction it is.
    let kinds = list("?kind=transfer");
    assert_eq!(heights(&kinds), [5, 2, 1]);
    assert_eq!(kinds["applied"], json!(["kind"]));
    assert_eq!(heights(&list("?kind=deploy")), [3]);
    assert_eq!(heights(&list("?kind=contract_call")), [4]);
    assert_eq!(heights(&list("?kind=nothing_of_the_sort")), Vec::<u64>::new());

    // Whether it sent it, or only took part.
    assert_eq!(heights(&list("?direction=out")), [5, 4, 3, 1]);
    assert_eq!(heights(&list("?direction=in")), [2]);

    // Who else it touched.
    assert_eq!(heights(&list(&format!("?counterparty={bob}"))), [2, 1]);
    assert_eq!(heights(&list(&format!("?counterparty={carol}"))), [5]);

    // What it moved: the coin, or a contract called or emitting.
    assert_eq!(heights(&list("?token=coin")), [5, 2, 1]);
    assert_eq!(heights(&list(&format!("?token={contract}"))), [4]);

    // When: by height, by time, each end included, and any together.
    assert_eq!(heights(&list("?from_height=2&until_height=4")), [4, 3, 2]);
    assert_eq!(heights(&list(&format!("?from={}&until={}", time(2), time(3)))), [3, 2]);
    assert_eq!(heights(&list(&format!("?until={}", time(1)))), [1]);
    assert_eq!(heights(&list(&format!("?from={}", time(5) + 1))), Vec::<u64>::new());
    assert_eq!(heights(&list("?from_height=4&until_height=2")), Vec::<u64>::new(), "an empty range");

    let together = list(&format!("?kind=transfer&direction=out&counterparty={bob}&from_height=1"));
    assert_eq!(heights(&together), [1]);
    assert_eq!(together["applied"], json!(["kind", "direction", "counterparty", "from_height"]));

    // A page holds matches, however many others lie between, and its cursor goes on from there.
    let first = list("?kind=transfer&limit=1");
    assert_eq!((heights(&first), first["limited"].clone()), (vec![5], json!(false)));
    let second = list(&format!("?kind=transfer&limit=1&cursor={}", first["next"].as_str().unwrap()));
    assert_eq!(heights(&second), [2], "the deploy and the call between are skipped");
    let third = list(&format!("?kind=transfer&limit=1&cursor={}", second["next"].as_str().unwrap()));
    assert_eq!((heights(&third), third["next"].clone()), (vec![1], Value::Null));

    // A cursor stays good under a filter's own start.
    let from_top = list("?until_height=4&limit=2");
    assert_eq!(heights(&from_top), [4, 3]);
    assert_eq!(heights(&list(&format!("?until_height=4&limit=2&cursor={}", from_top["next"].as_str().unwrap()))), [2, 1]);

    // An empty parameter is one not given, as a form sends it.
    assert_eq!(heights(&list("?kind=&direction=&counterparty=&token=&from=&until=")), [5, 4, 3, 2, 1]);

    for bad in ["direction=sideways", "counterparty=nope", "token=nope", "from=soon", "until=x", "from_height=-1", "until_height=y"] {
        assert_eq!(get(address, &format!("/accounts/{me}/txs?{bad}")).0, 400, "{bad}");
    }
}

#[test]
fn a_listing_looks_through_no_more_than_its_bound_and_says_where_to_go_on_from() {
    let authority = DevAuthority::dev_keypair();
    let me = authority.public().address();
    let db = Arc::new(MemoryDb::default());
    let mut chain = Chain::open(db.clone(), &free()).unwrap();
    let genesis_id = chain.genesis();
    let address = start(db.clone(), None, &free(), "127.0.0.1:0".parse().unwrap(), ApiOptions::default()).unwrap();
    let bob = Keypair::generate(authority.public().key_type()).unwrap().public().address();

    chain.keep_index(IndexKind::Addresses, None).unwrap();
    // The deploy first, oldest, under more transfers than one request looks through.
    produce(&mut chain, vec![SignedTx::sign(Tx::Deploy { code: Code::Lua(COUNTER.to_string()) }, 0, FeeOffer::NONE, &authority, &genesis_id)]);
    let mut nonce = 1;

    while (nonce as usize) <= filters::MAX_TXS_SCANNED + 10 {
        let txs: Vec<SignedTx> = (nonce..nonce + 256).map(|nonce| SignedTx::sign(Tx::Transfer { from: me, to: bob, amount: Amount::U128(1) }, nonce, FeeOffer::NONE, &authority, &genesis_id)).collect();
        nonce += txs.len() as u64;
        produce(&mut chain, txs);
    }

    let first = ok(address, &format!("/accounts/{me}/txs?kind=deploy"));

    assert_eq!(heights(&first), Vec::<u64>::new());
    assert_eq!((first["limited"].clone(), first["scanned"].clone()), (json!(true), json!(filters::MAX_TXS_SCANNED)));

    let cursor = first["next"].as_str().expect("it says where to go on from");
    let mut answer = ok(address, &format!("/accounts/{me}/txs?kind=deploy&cursor={cursor}"));

    while answer["limited"] == json!(true) {
        answer = ok(address, &format!("/accounts/{me}/txs?kind=deploy&cursor={}", answer["next"].as_str().unwrap()));
    }

    assert_eq!((heights(&answer), answer["next"].clone()), (vec![1], Value::Null), "found, however far back");

    // Without a filter a page fills long before the bound.
    let plain = ok(address, &format!("/accounts/{me}/txs?limit=10"));

    assert_eq!((plain["limited"].clone(), plain["scanned"].clone()), (json!(false), json!(10)));
    assert!(plain["next"].is_string());
}

#[test]
fn proofs_check_against_the_roots_in_headers() {
    use crate::db::{Column, StateProof, address_key};

    let fixture = serve();
    let hash = HashType::default_chain();
    let hash_of = |value: &Value| Hash::from_bytes(hash, &from_hex(value.as_str().unwrap()).unwrap()).unwrap();
    let state_proof = |proof: &Value| StateProof {
        siblings: proof["siblings"].as_array().unwrap().iter().map(hash_of).collect(),
        leaf: (!proof["leaf"].is_null()).then(|| {
            let path = from_hex(proof["leaf"]["path"].as_str().unwrap()).unwrap().try_into().unwrap();

            (path, hash_of(&proof["leaf"]["value_hash"]))
        }),
    };

    // What a light client does: trust a header, then check what it is told against its root.
    let key = address_key(&fixture.bob);
    let held = ok(fixture.address, &format!("/proofs/accounts/{}", to_hex(&key)));
    let header = ok(fixture.address, &format!("/blocks/{}", held["height"]));
    let root = hash_of(&header["state_root"]);
    let value = from_hex(held["value"].as_str().unwrap()).unwrap();

    assert_eq!(held["state_root"], header["state_root"]);
    assert_eq!(held["block"], fixture.chain.tip().id().to_string());
    assert!(state_proof(&held).verify(&root, Column::Accounts, &key, Some(&value)));
    assert_eq!(
        crate::registry::Accounts::new(&fixture.chain.state()).get(&fixture.bob).unwrap().balance,
        u256::new(100),
        "the value proven is bob's account"
    );

    let nobody = address_key(&Address::Contract([9; 32]));
    let absent = ok(fixture.address, &format!("/proofs/accounts/{}", to_hex(&nobody)));

    assert!(absent["value"].is_null());
    assert!(state_proof(&absent).verify(&root, Column::Accounts, &nobody, None));

    // The deploy in block 2: its receipt is the only leaf of that block's receipts root.
    let block = ok(fixture.address, "/blocks/2");
    let tx = block["txs"][0]["id"].as_str().unwrap().to_string();
    let proof = ok(fixture.address, &format!("/proofs/receipts/{tx}"));
    let receipt = from_hex(proof["receipt"].as_str().unwrap()).unwrap();
    let path: Vec<Hash> = proof["path"].as_array().unwrap().iter().map(hash_of).collect();
    let (index, count) = (proof["index"].as_u64().unwrap() as usize, proof["count"].as_u64().unwrap() as usize);

    assert_eq!((proof["tx"].as_str(), index, count), (Some(tx.as_str()), 0, 1));
    assert!(crate::crypto::merkle::verify(
        &hash_of(&block["receipts_root"]),
        &crate::crypto::merkle::leaf(hash, &receipt),
        index,
        count,
        &path
    ));

    // And the transaction itself, at the same place in tx_root.
    let signed = from_hex(proof["signed"].as_str().unwrap()).unwrap();
    let tx_path: Vec<Hash> = proof["tx_path"].as_array().unwrap().iter().map(hash_of).collect();

    assert_eq!(proof["tx_root"], block["tx_root"]);
    assert!(crate::crypto::merkle::verify(&hash_of(&block["tx_root"]), &crate::crypto::merkle::leaf(hash, &signed), index, count, &tx_path));
    assert_eq!(wincode::deserialize::<SignedTx>(&signed).unwrap().id(hash).to_string(), tx);
}

#[test]
fn a_light_client_follows_the_chain_from_the_api_alone() {
    use crate::db::{Column, StateProof, address_key};

    let fixture = serve();
    let decode = |value: &Value| from_hex(value.as_str().unwrap()).unwrap();
    let client = followed(fixture.address, fixture.chain.genesis());

    assert_eq!(client.tip().id(), fixture.chain.tip().id());

    // Bob's account, proven at block 1 and at the tip: a client trusting the genesis id alone
    // checks both.
    let key = address_key(&fixture.bob);
    let block1 = fixture.chain.block_at(1).unwrap().unwrap().id();

    for at in [format!("?block={block1}"), String::new()] {
        let answer = ok(fixture.address, &format!("/proofs/accounts/{}{at}", to_hex(&key)));
        let proof: StateProof = wincode::deserialize(&decode(&answer["proof"])).unwrap();
        let block = Hash::from_bytes(HashType::default_chain(), &decode(&answer["block"])).unwrap();
        let value = decode(&answer["value"]);

        assert!(client.check_state(&block, Column::Accounts, &key, Some(&value), &proof).is_ok(), "{at}");
    }

    let (status, _, _) = get(fixture.address, &format!("/proofs/accounts/{}?block=0x{}", to_hex(&key), "00".repeat(32)));

    assert_eq!(status, 404, "a block it cannot prove at");
}

/// Asks for a block range and reads the answer as a client does.
fn block_range(address: SocketAddr, query: &str) -> crate::light::BlockRange {
    let (status, head, body) = raw(address, "GET", &format!("/blocks/range?{query}"), "");

    assert_eq!(status, 200, "{query}: {}", String::from_utf8_lossy(&body));
    assert!(head.contains("content-type: application/octet-stream"), "{head}");

    crate::light::decode_block_range(&body).unwrap()
}

#[test]
fn a_light_client_checks_every_block_of_a_range_from_the_route_alone() {
    use crate::light::{LightError, RangeBody, RangeChanges, RangeCursor, RangeError};

    let fixture = serve();
    let client = followed(fixture.address, fixture.chain.genesis());
    let range = block_range(fixture.address, "from=1");

    range.check_shape(1, 0).unwrap();
    assert_eq!((range.tip, range.next, range.items.len()), (2, None, 2));

    let mut seen = Vec::new();

    for item in range.items.clone() {
        let (checked, assurance) = client.check_range_item(item).unwrap();

        assert_eq!((assurance.block, assurance.height), (checked.block, checked.height()));
        assert!(matches!(&checked.body, RangeBody::Whole { txs, receipts: Some(receipts), .. } if txs.len() == 1 && receipts.len() == 1));
        seen.extend(checked.entries().iter().map(|entry| entry.key.clone()));
    }

    assert!(seen.contains(&crate::db::address_key(&fixture.bob)), "the block that paid bob proves his account");

    // A client asks on from where it was told, and past the tip gets nothing and no cursor.
    let page = block_range(fixture.address, "from=1&count=1");
    assert_eq!((page.items.len(), page.next), (1, Some(RangeCursor { height: 2, entry: 0 })));
    assert_eq!(block_range(fixture.address, "from=2&count=1").items[0], range.items[1]);
    let past = block_range(fixture.address, "from=3");
    assert_eq!((past.tip, past.items.len(), past.next), (2, 0, None));
    past.check_shape(3, 0).unwrap();

    // The route's bytes, changed, are refused by name; a height it has not followed is told apart.
    let mut bent = range.items[0].clone();
    let RangeChanges::Proven { entries, .. } = &mut bent.changes else { panic!("proven") };
    let column = entries[0].column;
    entries[0].key.push(0);
    assert_eq!(client.check_range_item(bent).err(), Some(LightError::Range(RangeError::BadEntry { index: 0, column })));
    let mut far = range.items[0].clone();
    far.sealed.header.height = 99;
    assert_eq!(client.check_range_item(far).err(), Some(LightError::NotOnChain));
}

#[test]
fn a_range_asked_for_wrongly_is_told_why_in_json_as_every_error_is() {
    let fixture = serve();
    let asked = |query: &str| get(fixture.address, &format!("/blocks/range{query}"));

    assert_eq!(asked("").0, 400, "from is needed");
    assert_eq!(asked("?from=x").0, 400);
    assert_eq!(asked("?from=1&entry=99999999999").0, 400, "past what entries can be");
    let (status, _, body) = asked("?from=1&entry=1000");
    assert_eq!(status, 400, "past the block's list");
    assert!(body["error"].as_str().unwrap().contains("no entry"), "{body}");

    // The word range is the route's, and other words are still refused as blocks.
    assert_eq!(get(fixture.address, "/blocks/ranges").0, 400);
}

#[tokio::test]
async fn a_client_waits_its_turn_for_a_range_or_is_told_to_come_back() {
    let wait = std::time::Duration::from_millis(50);
    let gate = RangeGate::new(1, wait, RANGE_PER_IP);
    let (one, other): (IpAddr, IpAddr) = ("192.0.2.1".parse().unwrap(), "192.0.2.2".parse().unwrap());
    let turn = gate.enter(one).await.ok().expect("nobody else is building");

    // The builders are all busy, so the wait runs out, for anyone.
    assert!(matches!(gate.enter(other).await, Err(ApiError::Unavailable(_))));

    // Its own: one waiting, and a third at once refused. It may only have so many under way.
    let waiting = tokio::spawn({
        let gate = gate.clone();

        async move { gate.enter(one).await.is_ok() }
    });
    tokio::task::yield_now().await;
    assert!(matches!(gate.enter(one).await, Err(ApiError::TooMany(_))), "the client has {RANGE_PER_IP} under way");

    // A turn given back goes to who waits.
    drop(turn);
    assert!(waiting.await.unwrap(), "the waiting one took the turn");
    assert!(gate.enter(other).await.is_ok(), "and all is free again");
}

/// A light client that trusts this genesis, caught up with the node behind the API.
fn followed(address: SocketAddr, genesis: Hash) -> crate::light::LightClient {
    use crate::light::{FinalityStep, LightClient, LightParams, LightRequest};
    use crate::structs::block::SealedHeader;

    let decode = |value: &Value| from_hex(value.as_str().unwrap()).unwrap();
    let start = ok(address, "/light");
    let params: LightParams = wincode::deserialize(&decode(&start["params"])).unwrap();
    let header: SealedHeader = wincode::deserialize(&decode(&start["header"])).unwrap();
    let mut client = LightClient::new(params, header, genesis).unwrap();

    while let Some(request) = client.next() {
        match request {
            LightRequest::Headers { from, count } => {
                let answer = ok(address, &format!("/headers?from={from}&count={count}"));
                let headers: Vec<SealedHeader> = answer["headers"].as_array().unwrap().iter().map(|header| wincode::deserialize(&decode(header)).unwrap()).collect();
                client.deliver_headers(from, headers, u64::MAX / 2).unwrap();
            }
            LightRequest::Finality { from, count } => {
                let answer = ok(address, &format!("/light/finality?from={from}&count={count}"));
                let steps: Vec<FinalityStep> = answer["steps"].as_array().unwrap().iter().map(|step| wincode::deserialize(&decode(step)).unwrap()).collect();
                client.deliver_finality(from, steps).unwrap();
            }
        }
    }

    client
}

#[test]
fn a_light_client_proves_who_sent_what_one_transaction_at_a_time() {
    use crate::light::LightError;

    let authority = DevAuthority::dev_keypair();
    let me = authority.public().address();
    let db = Arc::new(MemoryDb::default());
    let mut chain = Chain::open(db.clone(), &free()).unwrap();
    let genesis_id = chain.genesis();
    let address = start(db.clone(), None, &free(), "127.0.0.1:0".parse().unwrap(), ApiOptions::default()).unwrap();
    let hash = HashType::default_chain();
    let decode = |value: &Value| from_hex(value.as_str().unwrap()).unwrap();
    let hashes = |value: &Value| value.as_array().unwrap().iter().map(|one| Hash::from_bytes(hash, &decode(one)).unwrap()).collect::<Vec<_>>();
    let pays: Vec<SignedTx> = (0..5u64)
        .map(|nonce| {
            let to = Keypair::generate(authority.public().key_type()).unwrap().public().address();

            SignedTx::sign(Tx::Transfer { from: me, to, amount: Amount::U128(u128::from(nonce) + 1) }, nonce, FeeOffer::NONE, &authority, &genesis_id)
        })
        .collect();
    let block = produce(&mut chain, pays.clone());
    let client = followed(address, chain.genesis());

    for (index, pay) in pays.iter().enumerate() {
        let proof = ok(address, &format!("/proofs/receipts/{}", pay.id(hash)));
        let (block_id, count) = (Hash::from_bytes(hash, &decode(&proof["block"])).unwrap(), proof["count"].as_u64().unwrap() as usize);
        let (signed, assurance) = client.check_tx(&block_id, &decode(&proof["signed"]), index, count, &hashes(&proof["tx_path"])).unwrap();
        let (receipt, _) = client.check_receipt(&block_id, &decode(&proof["receipt"]), index, count, &hashes(&proof["path"])).unwrap();

        assert_eq!((&signed, assurance.block, count), (pay, block.id(), 5));
        assert_eq!(receipt.tx, pay.id(hash));

        // Its bytes at another place, or another's bytes at its place, do not check.
        let other = (index + 1) % count;

        assert_eq!(client.check_tx(&block_id, &decode(&proof["signed"]), other, count, &hashes(&proof["tx_path"])).err(), Some(LightError::BadProof));
        assert_eq!(
            client.check_tx(&block_id, &pays[other].hashable_bytes_with_tag(), index, count, &hashes(&proof["tx_path"])).err(),
            Some(LightError::BadProof)
        );
    }
}

#[test]
fn blocks_say_what_they_minted() {
    use crate::chain::Reward;

    let authority = DevAuthority::dev_keypair();
    let mut producer = AuthorityProducer::new(authority.clone(), SLOT);
    let db = Arc::new(MemoryDb::default());
    let initial = crate::structs::u256::u256::from(50u64) * crate::structs::u256::u256::from(10u64).pow(18);
    let genesis = Genesis {
        reward: Reward {
            initial,
            halving_interval: Some(2),
        },
        ..free()
    };
    let mut chain = Chain::open(db.clone(), &genesis).unwrap();

    for _ in 0..3 {
        let now = chain.tip().timestamp + SLOT;
        let mut block = chain.build(Vec::new(), Vec::new(), producer.address(&[]), now).unwrap();
        block.seal = producer.seal(&block.header).unwrap();
        chain.import(&block, now).unwrap();
    }

    let address = start(db, None, &genesis, "127.0.0.1:0".parse().unwrap(), ApiOptions::default()).unwrap();
    let page = ok(address, "/blocks?limit=4");
    let rewards: Vec<&str> = page["blocks"].as_array().unwrap().iter().map(|block| block["reward"].as_str().unwrap()).collect();
    let half = (initial >> 1u32).to_string();

    assert_eq!(rewards, [half.as_str(), half.as_str(), &initial.to_string(), "0"], "newest first, halving every 2 blocks, and nothing for genesis");
    assert_eq!(ok(address, "/blocks/1")["reward"], initial.to_string());
    assert_eq!(ok(address, "/blocks/0")["reward"], "0");
    assert_eq!(ok(serve().address, "/blocks/1")["reward"], "0", "a chain without a reward");
}

/// Asks a faucet, from an IP a proxy on this machine names: a request with a proof of work worked
/// out for that address and window.
#[cfg(feature = "faucet")]
fn drip(address: SocketAddr, to: &Address, window: u64, nonce: u64, ip: &str) -> (u16, Value) {
    let body = json!({ "to": to.to_string(), "window": window, "nonce": nonce }).to_string();
    let headers = format!("Content-Type: application/json\r\nContent-Length: {}\r\nX-Forwarded-For: {ip}\r\n", body.len());
    let mut stream = TcpStream::connect(address).unwrap();
    write!(stream, "POST /faucet HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n{headers}\r\n{body}").unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    let (head, body) = response.split_once("\r\n\r\n").unwrap();

    (head.split(' ').nth(1).unwrap().parse().unwrap(), serde_json::from_str(body).unwrap())
}

#[cfg(feature = "faucet")]
#[test]
fn a_faucet_drips_to_each_address_and_ip_once_a_period_for_a_moments_work() {
    use crate::faucet::{faucet_window, solve_faucet};

    let genesis = free();
    let db = Arc::new(MemoryDb::default());
    let chain = Chain::open(db.clone(), &genesis).unwrap();
    let id = chain.genesis();
    let faucet = crate::api::FaucetConfig { key: DevAuthority::dev_keypair(), amount: u256::from(5_000u64), every_ms: 3_600_000, work_bits: 8 };
    let options = ApiOptions { faucet: Some(faucet), ..ApiOptions::default() };
    let address = start(db, Some(fake_node()), &genesis, "127.0.0.1:0".parse().unwrap(), options).unwrap();
    let window = faucet_window(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64);
    let work = |to: &Address, window: u64| solve_faucet(&id, to, window, 8, 0, 1 << 20).unwrap();
    let [alice, bob, carol] = [1u8, 2, 3].map(|byte| Address::Ed25519([byte; 32]));

    // What clients read before they ask.
    let about = ok(address, "/faucet");

    assert_eq!((about["amount"].as_str(), about["work_bits"].as_u64(), about["every_ms"].as_u64()), (Some("5000"), Some(8), Some(3_600_000)));
    assert_eq!(about["address"], DevAuthority::dev_keypair().public().address().to_string());
    assert!(about["balance"].as_str().unwrap().parse::<u128>().unwrap() > 0);
    assert_eq!(ok(address, "/status")["faucet"]["amount"], "5000");

    // Work for this address and window: a drip.
    let (status, answer) = drip(address, &alice, window, work(&alice, window), "203.0.113.1");

    assert_eq!(status, 200, "{answer}");
    assert_eq!((answer["to"].as_str(), answer["amount"].as_str()), (Some(alice.to_string().as_str()), Some("5000")));

    // Again for the same address, from anywhere: too soon. Another address from the same IP: too.
    let (status, answer) = drip(address, &alice, window, work(&alice, window), "203.0.113.2");

    assert_eq!(status, 429, "{answer}");
    assert!(answer["error"].as_str().unwrap().contains("again in 59m") || answer["error"].as_str().unwrap().contains("again in 1h"), "{answer}");
    assert_eq!(drip(address, &bob, window, work(&bob, window), "203.0.113.1").0, 429);

    // Another address from another IP, with the last window's work: a drip, at the next nonce.
    let (status, second) = drip(address, &bob, window - 1, work(&bob, window - 1), "203.0.113.2");

    assert_eq!(status, 200, "{second}");
    assert_ne!(second["id"], answer["id"]);

    // Work that falls short, is for another address, or for a window gone by, is refused, and
    // leaves the address free to ask properly.
    let short = (0..).find(|nonce| !crate::faucet::enough_work(&crate::faucet::faucet_hash(&id, &carol, window, *nonce), 8)).unwrap();

    assert_eq!(drip(address, &carol, window, short, "203.0.113.3").0, 400);
    assert_eq!(drip(address, &carol, window, work(&alice, window), "203.0.113.3").0, 400);
    assert_eq!(drip(address, &carol, window - 2, work(&carol, window - 2), "203.0.113.3").0, 400);
    assert_eq!(drip(address, &carol, window, work(&carol, window), "203.0.113.3").0, 200);

    // A node without a faucet has none to show.
    let plain = serve();

    assert_eq!(get(plain.address, "/faucet").0, 404);
    assert_eq!(ok(plain.address, "/status")["faucet"], Value::Null);
}

#[cfg(feature = "swap")]
#[test]
fn status_names_the_swap_contract_when_the_node_was_given_one() {
    let contract = Address::Contract([0x5a; 32]);

    for (given, expected) in [(None, Value::Null), (Some(contract), json!({ "contract": contract.to_string() }))] {
        let db = Arc::new(MemoryDb::default());
        Chain::open(db.clone(), &free()).unwrap();
        let options = ApiOptions { swap_contract: given, ..ApiOptions::default() };
        let address = start(db, None, &free(), "127.0.0.1:0".parse().unwrap(), options).unwrap();

        assert_eq!(ok(address, "/status")["swap"], expected);
    }
}

#[cfg(all(feature = "swap", feature = "dex"))]
#[test]
fn the_directory_wins_over_the_operators_flags_which_fill_only_what_it_leaves_unset() {
    use crate::registry::directory::Directory;

    let (governed, flagged) = (Address::Contract([0x11; 32]), Address::Contract([0x22; 32]));
    let options = || ApiOptions { swap_contract: Some(flagged), dex_contract: Some(flagged), dex_router: Some([0xab; 20]), ..ApiOptions::default() };
    // The directory names the swap contract and the DEX, but no router.
    let genesis = Genesis { directory: Some(Directory { swap: Some(governed), dex: Some(governed), ..Directory::default() }), ..free() };
    let db = Arc::new(MemoryDb::default());
    Chain::open(db.clone(), &genesis).unwrap();
    let status = ok(start(db, None, &genesis, "127.0.0.1:0".parse().unwrap(), options()).unwrap(), "/status");

    assert_eq!(status["swap"], json!({ "contract": governed.to_string() }));
    assert_eq!(status["dex"], json!({ "contract": governed.to_string(), "router": format!("0x{}", "ab".repeat(20)) }), "the router the directory leaves unset comes from the flag");
    assert_eq!(status["operator"], json!({ "swap": flagged.to_string(), "dex": flagged.to_string(), "dex_router": format!("0x{}", "ab".repeat(20)) }), "the flags, shown apart");
    assert_eq!(status["directory"]["swap"], governed.to_string());

    // Given no flags, the directory's alone, and no operator's.
    let db = Arc::new(MemoryDb::default());
    Chain::open(db.clone(), &genesis).unwrap();
    let status = ok(start(db, None, &genesis, "127.0.0.1:0".parse().unwrap(), ApiOptions::default()).unwrap(), "/status");

    assert_eq!(status["swap"], json!({ "contract": governed.to_string() }));
    assert_eq!(status["dex"], json!({ "contract": governed.to_string(), "router": null }));
    assert_eq!(status["operator"], Value::Null);
}

#[cfg(feature = "dex")]
#[test]
fn status_names_the_dexes_the_node_was_given() {
    let contract = Address::Contract([0xde; 32]);
    let cases = [
        (None, None, Value::Null),
        (Some(contract), None, json!({ "contract": contract.to_string(), "router": null })),
        (Some(contract), Some([0xab; 20]), json!({ "contract": contract.to_string(), "router": format!("0x{}", "ab".repeat(20)) })),
    ];

    for (dex_contract, dex_router, expected) in cases {
        let db = Arc::new(MemoryDb::default());
        Chain::open(db.clone(), &free()).unwrap();
        let options = ApiOptions { dex_contract, dex_router, ..ApiOptions::default() };
        let address = start(db, None, &free(), "127.0.0.1:0".parse().unwrap(), options).unwrap();

        assert_eq!(ok(address, "/status")["dex"], expected);
    }
}

#[test]
fn a_client_may_have_only_so_many_requests_in_flight() {
    let in_flight = Arc::new(super::InFlight { per_ip: 2, counts: Default::default() });
    let (one, other): (std::net::IpAddr, std::net::IpAddr) = ("203.0.113.1".parse().unwrap(), "203.0.113.2".parse().unwrap());

    let first = super::InFlight::enter(&in_flight, one).unwrap();
    let second = super::InFlight::enter(&in_flight, one).unwrap();
    assert!(super::InFlight::enter(&in_flight, one).is_none(), "a third waits its turn");
    assert!(super::InFlight::enter(&in_flight, other).is_some(), "another client is not held up");

    // Answered, a request makes room again, and a client with none in flight is forgotten.
    drop(first);
    assert!(super::InFlight::enter(&in_flight, one).is_some());
    drop(second);
    assert!(!in_flight.counts.lock().unwrap().contains_key(&other));
}

#[test]
fn many_requests_are_answered_side_by_side_and_one_client_is_turned_away_past_its_share() {
    let db = Arc::new(MemoryDb::default());
    Chain::open(db.clone(), &free()).unwrap();
    let options = ApiOptions { threads: 4, per_ip: 4, ..ApiOptions::default() };
    let address = start(db, None, &free(), "127.0.0.1:0".parse().unwrap(), options).unwrap();

    // Sixteen at once from one client: some are answered, the rest turned away with 429, none fail.
    let statuses: Vec<u16> = std::thread::scope(|scope| {
        let asks: Vec<_> = (0..16).map(|_| scope.spawn(move || get(address, "/status").0)).collect();

        asks.into_iter().map(|ask| ask.join().unwrap()).collect()
    });

    assert!(statuses.iter().all(|status| matches!(status, 200 | 429)), "{statuses:?}");
    assert!(statuses.contains(&200));
    assert_eq!(get(address, "/status").0, 200, "and served again once they are answered");
}

#[test]
fn accounts_and_calls_read_at_an_older_block() {
    let fixture = serve();
    let authority = DevAuthority::dev_keypair().public().address();
    let block1 = fixture.chain.block_at(1).unwrap().unwrap().id();
    let held = |at: &str| ok(fixture.address, &format!("/accounts/{}{at}", fixture.bob))["balance"].clone();

    // Bob was paid in block 1: nothing before, 100 from then on, by height or by id.
    assert_eq!(held("?block=0"), "0");
    assert_eq!(held("?block=1"), "100");
    assert_eq!(held(&format!("?block={block1}")), "100");
    assert_eq!(ok(fixture.address, &format!("/accounts/{}?block=1", fixture.bob))["height"], 1);
    assert_eq!(ok(fixture.address, &format!("/accounts/{}", fixture.bob))["height"], 2, "the tip, unless asked");
    assert_eq!(get(fixture.address, &format!("/accounts/{}?block=9", fixture.bob)).0, 404);

    // The counter was deployed in block 2: no contract at block 1.
    let counter = contract_address(&authority, 1, HashType::default_chain());
    let call = |block: Value| {
        let body = json!({ "contract": counter.to_string(), "function": "increment", "args": [{ "type": "number", "value": 3 }], "block": block });

        request(fixture.address, "POST", "/call", &body.to_string())
    };

    assert_eq!(call(json!(1)).0, 404);
    assert_eq!(call(json!(2)).2["values"], json!([{ "type": "i64", "value": "3" }]));
    assert_eq!(call(Value::Null).2["values"], json!([{ "type": "i64", "value": "3" }]));
}

/// A chain with a contract that emits, and a batch of three steps and a failed one on it, indexed
/// by address: the node's view of each.
struct Batches {
    address: SocketAddr,
    good: SignedTx,
    bad: SignedTx,
    contract: Address,
    bob: Address,
    chain: Chain,
}

fn batches() -> Batches {
    let authority = DevAuthority::dev_keypair();
    let me = authority.public().address();
    let db = Arc::new(MemoryDb::default());
    let mut chain = Chain::open(db.clone(), &free()).unwrap();
    let genesis_id = chain.genesis();
    let hash = HashType::default_chain();
    let contract = contract_address(&me, 0, hash);
    let bob = Keypair::from_seed([40; 32]).public().address();
    let carol = Keypair::from_seed([41; 32]).public().address();
    let call = Tx::ContractCall { to: contract, func: Hash::Xxh3_64(crate::vm::selector("increment")), amount: Amount::U128(0), data: vec![TxCallArgumentType::I64(1)] };
    let pay = |to: Address, amount: u128| Tx::Transfer { from: me, to, amount: Amount::U128(amount) };
    let good = SignedTx::sign(Tx::Batch { steps: vec![Tx::Deploy { code: Code::Lua(COUNTER.to_string()) }, call.clone(), pay(bob, 7)] }, 0, FeeOffer::NONE, &authority, &genesis_id);
    let bad = SignedTx::sign(Tx::Batch { steps: vec![pay(carol, 1), Tx::Transfer { from: me, to: carol, amount: Amount::U256(u256::MAX) }] }, 1, FeeOffer::NONE, &authority, &genesis_id);

    chain.keep_index(IndexKind::Addresses, None).unwrap();
    produce(&mut chain, vec![good.clone()]);
    produce(&mut chain, vec![bad.clone()]);

    let address = start(db, Some(fake_node()), &free(), "127.0.0.1:0".parse().unwrap(), ApiOptions::default()).unwrap();

    Batches { address, good, bad, contract, bob, chain }
}

#[test]
fn a_batch_s_transaction_and_receipt_say_each_step_and_its_result_or_which_failed_and_why() {
    let Batches { address, good, bad, contract, .. } = batches();
    let hash = HashType::default_chain();
    let view = ok(address, &format!("/txs/{}", good.id(hash)));

    assert_eq!(view["status"], "batch", "{view}");
    assert_eq!(view["steps"][0]["status"], "deployed");
    assert_eq!(view["steps"][0]["contract"], contract.to_string());
    assert_eq!(view["steps"][1]["status"], "returned");
    assert_eq!(view["steps"][2]["status"], "done");
    assert_eq!(view["steps"][2]["events"], 0);

    let failed = ok(address, &format!("/txs/{}", bad.id(hash)));

    assert_eq!(failed["status"], "failed", "what reads no further sees that: {failed}");
    assert_eq!(failed["failed_step"], 1);
    assert!(failed["error"].as_str().unwrap().contains("step 2 failed, undoing every step: insufficient balance"), "{failed}");

    let tx = crate::structs::json::signed_tx(&good, hash);

    assert_eq!(tx["tx"]["type"], "batch");
    assert_eq!(tx["tx"]["steps"].as_array().unwrap().iter().map(|step| step["type"].as_str().unwrap()).collect::<Vec<_>>(), ["deploy", "contract_call", "transfer"]);
}

#[test]
fn a_batch_is_listed_under_everyone_its_steps_touch_and_found_by_what_they_are() {
    let Batches { address, good, contract, bob, .. } = batches();
    let id = good.id(HashType::default_chain()).to_string();
    let me = DevAuthority::dev_keypair().public().address();

    for who in [me, bob, contract] {
        let listing = ok(address, &format!("/accounts/{who}/txs"));

        assert!(listing["txs"].as_array().unwrap().iter().any(|tx| tx["id"] == id), "{who}: {listing}");
    }

    let carol = Keypair::from_seed([41; 32]).public().address();

    assert_eq!(ok(address, &format!("/accounts/{carol}/txs"))["txs"].as_array().unwrap().len(), 1, "a failed batch shows too, as a failed transfer does");

    // As a batch, as what any of its steps is, and for coin one moved.
    for kind in ["batch", "transfer", "deploy", "contract_call"] {
        let found = ok(address, &format!("/accounts/{bob}/txs?kind={kind}"));

        assert_eq!(found["txs"].as_array().unwrap().len(), 1, "{kind}: {found}");
    }

    assert_eq!(ok(address, &format!("/accounts/{bob}/txs?kind=burn"))["txs"], json!([]));
    assert_eq!(ok(address, &format!("/accounts/{bob}/txs?direction=in"))["txs"].as_array().unwrap().len(), 1);
    assert_eq!(ok(address, &format!("/accounts/{bob}/txs?token=coin"))["txs"].as_array().unwrap().len(), 1);
    assert_eq!(ok(address, &format!("/accounts/{bob}/txs?token={contract}"))["txs"].as_array().unwrap().len(), 1);
}

#[test]
fn a_light_client_proves_a_batch_and_which_of_its_steps_failed() {
    use crate::structs::receipt::{CommittedOutcome, StepEffect};

    let Batches { address, good, bad, chain, contract, .. } = batches();
    let hash = HashType::default_chain();
    let client = followed(address, chain.genesis());
    let decode = |value: &Value| from_hex(value.as_str().unwrap()).unwrap();
    let hashes = |value: &Value| value.as_array().unwrap().iter().map(|one| Hash::from_bytes(hash, &decode(one)).unwrap()).collect::<Vec<_>>();
    let proven = |tx: &SignedTx| {
        let proof = ok(address, &format!("/proofs/receipts/{}", tx.id(hash)));
        let (block, count) = (Hash::from_bytes(hash, &decode(&proof["block"])).unwrap(), proof["count"].as_u64().unwrap() as usize);
        let (signed, _) = client.check_tx(&block, &decode(&proof["signed"]), 0, count, &hashes(&proof["tx_path"])).unwrap();
        let (receipt, _) = client.check_receipt(&block, &decode(&proof["receipt"]), 0, count, &hashes(&proof["path"])).unwrap();

        (signed, receipt)
    };

    let (signed, receipt) = proven(&good);

    assert_eq!(signed, good, "every step proven with it");
    assert_eq!(receipt.tx, good.id(hash));

    let CommittedOutcome::Batch(steps) = &receipt.outcome else {
        panic!("{:?}", receipt.outcome);
    };

    assert_eq!(steps[0].effect, StepEffect::Deployed(contract));
    assert_eq!(steps[2].effect, StepEffect::Done);

    let (signed, receipt) = proven(&bad);

    assert_eq!(signed, bad);
    assert_eq!(receipt.outcome, CommittedOutcome::BatchFailed { step: 1 });
    assert!(receipt.outcome.failed());
}

#[test]
fn a_dry_run_and_an_offer_price_a_batch_whole_and_say_what_would_fail() {
    let authority = DevAuthority::dev_keypair();
    let me = authority.public().address();
    let genesis = Genesis::dev_authority(authority.public());
    let db = Arc::new(MemoryDb::default());
    Chain::open(db.clone(), &genesis).unwrap();
    let address = start(db, Some(fake_node()), &genesis, "127.0.0.1:0".parse().unwrap(), ApiOptions::default()).unwrap();
    let fees = genesis.fees.clone().unwrap().build();
    let bob = Keypair::from_seed([40; 32]).public().address();
    let pay = |amount: u128| Tx::Transfer { from: me, to: bob, amount: Amount::U128(amount) };
    let body = |tx: Tx| {
        let probe = SignedTx::sign(tx, 0, FeeOffer::NONE, &authority, &genesis.block().id());

        (json!({ "tx": to_hex(&wincode::serialize(&probe).unwrap()) }).to_string(), probe)
    };
    let (asked, probe) = body(Tx::Batch { steps: vec![pay(1), pay(2)] });
    let (status, _, offered) = request(address, "POST", "/offer", &asked);

    assert_eq!(status, 200, "{offered}");
    assert_eq!(offered["gas_limit"], fees.schedule().intrinsic(&probe), "moving coin runs nothing, so its intrinsic cost is all");
    assert_eq!(offered["estimate"]["status"], "ok", "{offered}");
    assert_eq!(offered["estimate"]["steps"].as_array().unwrap().len(), 2);

    let (asked, _) = body(Tx::Batch { steps: vec![pay(1), Tx::Transfer { from: me, to: bob, amount: Amount::U256(u256::MAX) }] });
    let (_, _, failing) = request(address, "POST", "/offer", &asked);

    assert_eq!(failing["estimate"]["status"], "failed");
    assert!(failing["estimate"]["error"].as_str().unwrap().contains("step 2 failed"), "{failing}");

    // Ones that could never land say so as a dry run, not as a run.
    let (asked, _) = body(Tx::Batch { steps: vec![Tx::Mint { to: bob, amount: Amount::U128(1) }] });
    let (_, _, refused) = request(address, "POST", "/offer", &asked);

    assert!(refused["estimate"]["error"].as_str().unwrap().contains("a batch holds only"), "{refused}");
}

#[cfg(feature = "evm")]
#[test]
fn ethereum_tools_see_a_batch_s_logs_in_order_and_never_as_one_call() {
    use crate::vm::evm::create_address;

    let authority = DevAuthority::dev_keypair();
    let me = authority.public().address();
    let db = Arc::new(MemoryDb::default());
    let mut chain = Chain::open(db.clone(), &free()).unwrap();
    let genesis_id = chain.genesis();
    let word = |value: u64| crate::structs::u256::u256::from(value).to_be_bytes().to_vec();
    let selector = |signature: &str| crate::crypto::secp256k1::keccak(signature.as_bytes())[..4].to_vec();
    let init = [from_hex(include_str!("../../contracts/evm/Counter.bin").trim()).unwrap(), word(5)].concat();
    let counter = create_address(&me, 0);
    let Address::Evm(counter_bytes) = counter else { unreachable!() };
    let bump = |by: u64| Tx::EvmCall { to: counter, value: Amount::U128(0), input: [selector("bump(uint256)"), word(by)].concat() };
    let batch = SignedTx::sign(Tx::Batch { steps: vec![Tx::Deploy { code: Code::Evm(init) }, bump(1), bump(2)] }, 0, FeeOffer::NONE, &authority, &genesis_id);
    let failed = SignedTx::sign(Tx::Batch { steps: vec![bump(4), Tx::Transfer { from: me, to: counter, amount: Amount::U256(u256::MAX) }] }, 1, FeeOffer::NONE, &authority, &genesis_id);
    let single = SignedTx::sign(bump(8), 2, FeeOffer::NONE, &authority, &genesis_id);
    let hash = HashType::default_chain();
    produce(&mut chain, vec![batch.clone()]);
    produce(&mut chain, vec![failed.clone(), single.clone()]);

    let address = start(db, Some(fake_node()), &free(), "127.0.0.1:0".parse().unwrap(), ApiOptions::default()).unwrap();
    let rpc = |method: &str, params: Value| {
        let (status, _, answer) = request(address, "POST", "/rpc", &json!({ "jsonrpc": "2.0", "id": 7, "method": method, "params": params }).to_string());
        assert_eq!((status, answer["id"].clone()), (200, json!(7)), "{answer}");

        answer["result"].clone()
    };
    let logs = rpc("eth_getLogs", json!([{ "fromBlock": "0x0", "toBlock": "latest", "address": to_hex(&counter_bytes) }]));
    let logs = logs.as_array().unwrap();

    // The batch's two bumps and the single one after the failed batch, whose own log is gone.
    assert_eq!(logs.iter().map(|log| log["data"].as_str().unwrap().to_string()).collect::<Vec<_>>(), [to_hex(&word(6)), to_hex(&word(8)), to_hex(&word(16))]);
    assert_eq!(logs[0]["transactionHash"], to_hex(&batch.id(hash).to_bytes()), "a batch's logs are its own");
    assert_eq!(logs[1]["transactionHash"], logs[0]["transactionHash"]);
    assert_eq!((logs[0]["logIndex"].clone(), logs[1]["logIndex"].clone()), (json!("0x0"), json!("0x1")), "the block's logs, numbered in order");
    assert_eq!(logs[2]["transactionHash"], to_hex(&single.id(hash).to_bytes()));

    // By its id it is no Ethereum transaction, so it is never shown as one call with one to and one
    // input; no block lists it among them either.
    for tx in [&batch, &failed] {
        assert_eq!(rpc("eth_getTransactionByHash", json!([to_hex(&tx.id(hash).to_bytes())])), Value::Null);
        assert_eq!(rpc("eth_getTransactionReceipt", json!([to_hex(&tx.id(hash).to_bytes())])), Value::Null);
    }

    assert_eq!(rpc("eth_getBlockByNumber", json!(["0x1", false]))["transactions"], json!([]));
    assert_eq!(rpc("eth_getTransactionCount", json!([to_hex(&crate::vm::evm::evm_alias(&me)), "latest"])), "0x3", "one nonce a batch");
}


#[cfg(feature = "secp256k1")]
#[test]
fn an_address_s_multisigs_are_listed_from_the_member_index_by_its_address_or_its_key() {
    use crate::registry::multisig::MultisigMember;
    use crate::structs::address::multisig_address;

    let authority = DevAuthority::dev_keypair();
    let me = authority.public().address();
    let db = Arc::new(MemoryDb::default());
    let mut chain = Chain::open(db.clone(), &free()).unwrap();
    let genesis_id = chain.genesis();
    let address = start(db.clone(), None, &free(), "127.0.0.1:0".parse().unwrap(), ApiOptions::default()).unwrap();
    let alice = Keypair::from_seed([41; 32]);
    let ethereum = Keypair::from_private(crate::crypto::keypair::PrivateKey::Secp256k1([42; 32]));
    let make = |keys: &[&Keypair], nonce: u64| {
        let members = keys.iter().map(|keypair| MultisigMember { key: keypair.public(), bitcoin: None, evm: None }).collect();

        SignedTx::sign(Tx::CreateMultisig { members, threshold: 1, testnet: false }, nonce, FeeOffer::NONE, &authority, &genesis_id)
    };
    let alice_at = alice.public().address();

    assert_eq!(get(address, &format!("/accounts/{alice_at}/multisigs")).0, 404, "no index yet");

    chain.keep_member_index().unwrap();
    produce(&mut chain, vec![make(&[&alice], 0), make(&[&alice, &ethereum], 1)]);
    let (first, second) = (multisig_address(&me, 0, HashType::default_chain()), multisig_address(&me, 1, HashType::default_chain()));
    let listing = ok(address, &format!("/accounts/{alice_at}/multisigs"));
    let mut found: Vec<&str> = listing["multisigs"].as_array().unwrap().iter().map(|multisig| multisig["address"].as_str().unwrap()).collect();
    found.sort();
    let mut made = vec![first.to_string(), second.to_string()];
    made.sort();

    assert_eq!(found, made);
    assert_eq!((listing["member"].clone(), listing["next"].clone()), (json!(alice_at.to_string()), Value::Null));

    // An Ethereum account's key is no address: found by either.
    let by_key = ok(address, &format!("/accounts/{}/multisigs", ethereum.public()));
    let by_address = ok(address, &format!("/accounts/{}/multisigs", ethereum.public().address()));

    assert_eq!(by_key, by_address);
    assert_eq!(by_key["multisigs"], json!([{ "address": second.to_string(), "threshold": 1, "members": 2, "bitcoin": false, "evm": false }]));

    // A page at a time.
    let page = ok(address, &format!("/accounts/{alice_at}/multisigs?limit=1"));
    let cursor = page["next"].as_str().unwrap();
    let rest = ok(address, &format!("/accounts/{alice_at}/multisigs?limit=1&cursor={cursor}"));

    assert_eq!((page["multisigs"].as_array().unwrap().len(), rest["multisigs"].as_array().unwrap().len(), rest["next"].clone()), (1, 1, Value::Null));
    assert_ne!(page["multisigs"][0], rest["multisigs"][0]);
    assert_eq!(ok(address, &format!("/accounts/{me}/multisigs"))["multisigs"], json!([]), "the maker is no member");

    for bad in ["/accounts/nope/multisigs".to_string(), format!("/accounts/{alice_at}/multisigs?cursor=zz"), format!("/accounts/{alice_at}/multisigs?limit=x")] {
        assert_eq!(get(address, &bad).0, 400, "{bad}");
    }
}

#[test]
fn a_client_asking_faster_than_http_rate_waits_for_its_bucket_to_fill() {
    let rate = super::Rate::new(3);
    let (one, other): (std::net::IpAddr, std::net::IpAddr) = ("203.0.113.1".parse().unwrap(), "203.0.113.2".parse().unwrap());
    let now = std::time::Instant::now();

    // A second's worth at once, then nothing until the bucket fills a third of a second later.
    assert!((0..3).all(|_| rate.take(one, now)));
    assert!(!rate.take(one, now), "a fourth within the second");
    assert!(rate.take(other, now), "another client has a bucket of its own");
    assert!(!rate.take(one, now + std::time::Duration::from_millis(300)));
    assert!(rate.take(one, now + std::time::Duration::from_millis(340)));

    // Left alone for a second, it is full again, and never holds more than a second's worth.
    let later = now + std::time::Duration::from_secs(10);
    assert!((0..3).all(|_| rate.take(one, later)));
    assert!(!rate.take(one, later));

    // 0 counts nothing.
    let unlimited = super::Rate::new(0);
    assert!((0..1_000).all(|_| unlimited.take(one, now)));
}

#[test]
fn a_node_started_with_http_rate_turns_a_fast_client_away_with_retry_after() {
    let db = Arc::new(MemoryDb::default());
    Chain::open(db.clone(), &free()).unwrap();
    let options = ApiOptions { limits: ApiLimits { rate: 2, ..ApiLimits::DEFAULT }, ..ApiOptions::default() };
    let address = start(db, None, &free(), "127.0.0.1:0".parse().unwrap(), options).unwrap();
    let from = |ip: &str| raw(address, "GET", "/status", &format!("X-Forwarded-For: {ip}\r\n"));

    assert_eq!(from("203.0.113.5").0, 200);
    assert_eq!(from("203.0.113.5").0, 200);
    let (status, head, body) = from("203.0.113.5");
    assert_eq!(status, 429, "the third within the second");
    assert!(head.contains("retry-after: 1"), "{head}");
    assert!(serde_json::from_slice::<Value>(&body).unwrap()["error"].as_str().unwrap().contains("2 requests a second"));
    assert_eq!(from("203.0.113.6").0, 200, "another client is not held up");

    std::thread::sleep(std::time::Duration::from_millis(600));
    assert_eq!(from("203.0.113.5").0, 200, "and served again once its bucket fills");

    // Without the flag, as before, nothing is counted.
    let db = Arc::new(MemoryDb::default());
    Chain::open(db.clone(), &free()).unwrap();
    let open = start(db, None, &free(), "127.0.0.1:0".parse().unwrap(), ApiOptions::default()).unwrap();
    assert!((0..20).all(|_| get(open, "/status").0 == 200));
}

/// Posts a body of about so many bytes to /txs and reads the status alone: the node may close the
/// connection before it has all of a body it refuses.
fn post_bytes(address: SocketAddr, bytes: usize) -> u16 {
    let mut stream = TcpStream::connect(address).unwrap();
    let body = format!("{{\"tx\": \"0x{}\"}}", "ab".repeat(bytes.saturating_sub(12) / 2));
    let head = format!("POST /txs HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n", body.len());
    let mut writer = stream.try_clone().unwrap();
    let sending = std::thread::spawn(move || {
        let _ = writer.write_all(head.as_bytes()).and_then(|_| writer.write_all(body.as_bytes()));
    });
    let mut response = Vec::new();
    let _ = stream.read_to_end(&mut response);
    let _ = sending.join();
    let text = String::from_utf8_lossy(&response);

    text.split(' ').nth(1).and_then(|status| status.parse().ok()).expect("an answer")
}

#[test]
fn a_body_is_held_to_room_for_the_chains_largest_transaction_and_never_less_than_max_body() {
    assert_eq!(body_cap(None), MAX_BODY);
    assert_eq!(body_cap(Some(100_000)), MAX_BODY, "a small max_tx_bytes never lowers it");
    assert_eq!(body_cap(Some(4 * 1024 * 1024)), 8 * 1024 * 1024 + 64 * 1024, "room for one hex-encoded");

    // Past 2 MiB a node refuses a body as it reads it, before it is a transaction at all.
    let db = Arc::new(MemoryDb::default());
    Chain::open(db.clone(), &free()).unwrap();
    let address = start(db, Some(fake_node()), &free(), "127.0.0.1:0".parse().unwrap(), ApiOptions::default()).unwrap();
    assert_eq!(post_bytes(address, 3 * 1024 * 1024), 413);
    assert_eq!(post_bytes(address, 1024 * 1024), 400, "within it, read, and refused as no transaction");

    // A chain whose limits take transactions of 2 MiB takes a body twice that, hex-encoded.
    let big = Genesis { max_tx_bytes: Some(2 * 1024 * 1024), ..free() };
    let db = Arc::new(MemoryDb::default());
    Chain::open(db.clone(), &big).unwrap();
    let address = start(db, Some(fake_node()), &big, "127.0.0.1:0".parse().unwrap(), ApiOptions::default()).unwrap();
    assert_eq!(post_bytes(address, 3 * 1024 * 1024), 400, "read whole: room for the chain's largest");
    assert_eq!(post_bytes(address, 5 * 1024 * 1024), 413);
}

#[test]
fn api_max_page_holds_every_listing_to_it_and_its_default_below_it() {
    let query = |limit: Option<&str>| limit.map_or_else(HashMap::new, |limit| HashMap::from([("limit".to_string(), limit.to_string())]));
    let own = ApiLimits::DEFAULT;
    let capped = ApiLimits { max_page: Some(10), ..ApiLimits::DEFAULT };

    assert_eq!(super::page(&own, &query(Some("50")), DEFAULT_PAGE, MAX_PAGE).ok().unwrap(), 50);
    assert_eq!(super::page(&own, &query(Some("500")), DEFAULT_PAGE, MAX_PAGE).ok().unwrap(), MAX_PAGE);
    assert_eq!(super::page(&own, &query(None), DEFAULT_PAGE, MAX_PAGE).ok().unwrap(), DEFAULT_PAGE);
    assert_eq!(super::page(&capped, &query(Some("50")), DEFAULT_PAGE, MAX_PAGE).ok().unwrap(), 10);
    assert_eq!(super::page(&capped, &query(None), DEFAULT_PAGE, MAX_PAGE).ok().unwrap(), 10, "the default too");
    assert_eq!(super::page(&ApiLimits { max_page: Some(2_000), ..ApiLimits::DEFAULT }, &query(Some("1500")), DEFAULT_MEMPOOL, MAX_MEMPOOL).ok().unwrap(), 1_500, "raised past a listing's own");

    // Over HTTP, a node with a cap of 10 pages its blocks in tens.
    let fixture = serve();
    let mut chain = fixture.chain;
    for _ in 0..12 {
        produce(&mut chain, Vec::new());
    }
    let options = ApiOptions { limits: capped, ..ApiOptions::default() };
    let address = start(chain.db().clone(), None, &free(), "127.0.0.1:0".parse().unwrap(), options).unwrap();
    let first = ok(address, "/blocks?limit=100");
    assert_eq!(first["blocks"].as_array().unwrap().len(), 10);
    assert_eq!(ok(fixture.address, "/blocks?limit=100")["blocks"].as_array().unwrap().len(), 15, "the uncapped node lists them all");
}

#[cfg(feature = "ethereum")]
#[test]
fn suggest_multiplier_is_what_eth_gas_price_and_an_open_offer_ask_over_the_base_price() {
    let authority = DevAuthority::dev_keypair();
    let genesis = Genesis::dev_authority(authority.public());
    let db = Arc::new(MemoryDb::default());
    let chain = Chain::open(db.clone(), &genesis).unwrap();
    let base = chain.next_base_price();
    let probe = SignedTx::sign(
        Tx::Transfer { from: authority.public().address(), to: Keypair::generate(authority.public().key_type()).unwrap().public().address(), amount: Amount::U128(1) },
        0,
        FeeOffer::NONE,
        &authority,
        &chain.genesis(),
    );
    let asked = |times: u64| {
        let options = ApiOptions { limits: ApiLimits { suggest: times, ..ApiLimits::DEFAULT }, ..ApiOptions::default() };
        let address = start(db.clone(), None, &genesis, "127.0.0.1:0".parse().unwrap(), options).unwrap();
        let (_, _, price) = request(address, "POST", "/rpc", &json!({ "jsonrpc": "2.0", "id": 1, "method": "eth_gasPrice", "params": [] }).to_string());
        let price = u256::from_str_radix(price["result"].as_str().unwrap().trim_start_matches("0x"), 16).unwrap();
        let (_, _, offer) = request(address, "POST", "/offer", &json!({ "tx": to_hex(&wincode::serialize(&probe).unwrap()) }).to_string());

        (price, u256::from_str_radix(offer["max_price"].as_str().unwrap(), 10).unwrap())
    };

    assert!(base > u256::ZERO);
    assert_eq!(asked(2), (base * u256::from(2u64), base * u256::from(2u64)), "twice, as before");
    assert_eq!(asked(3), (base * u256::from(3u64), base * u256::from(3u64)));
    assert_eq!(asked(1), (base, base));
}

#[cfg(feature = "ethereum")]
#[test]
fn rpc_limits_hold_a_batch_to_the_operators_size() {
    let db = Arc::new(MemoryDb::default());
    Chain::open(db.clone(), &free()).unwrap();
    let options = ApiOptions { limits: ApiLimits { rpc: RpcLimits { batch: 1, ..RpcLimits::DEFAULT }, ..ApiLimits::DEFAULT }, ..ApiOptions::default() };
    let address = start(db, None, &free(), "127.0.0.1:0".parse().unwrap(), options).unwrap();
    let (_, _, two) = request(address, "POST", "/rpc", r#"[{"jsonrpc":"2.0","id":1,"method":"eth_chainId"},{"jsonrpc":"2.0","id":2,"method":"eth_syncing"}]"#);
    let (_, _, one) = request(address, "POST", "/rpc", r#"[{"jsonrpc":"2.0","id":1,"method":"eth_chainId"}]"#);

    assert!(two["error"]["message"].as_str().unwrap().contains("at most 1 requests"), "{two}");
    assert!(one[0]["result"].is_string(), "{one}");
}
