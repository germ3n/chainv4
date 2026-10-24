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
use crate::structs::hex::to_hex;
use crate::structs::tx::{FeeOffer, SignedTx, Tx, TxCallArgumentType};
use std::io::{Read as _, Write as _};
use std::net::TcpStream;

const SLOT: u64 = 2_000;

const COUNTER: &str = r#"
--@namespace counter number
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
        let signed = SignedTx::sign(tx, nonce as u64, FeeOffer::NONE, &authority);
        let mut block = chain.build(vec![signed], Vec::new(), producer.address(), now).unwrap();
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
    let (inbox, events) = std::sync::mpsc::channel();
    let backlog = Arc::new(Backlog::new(limits));
    let handled = backlog.clone();

    std::thread::spawn(move || {
        for event in events {
            match event {
                NetEvent::Submitted { tx, reply } => {
                    handled.release(crate::net::Origin::Local);
                    let verdict = if tx.nonce == 99 { Err(MempoolError::Stale) } else { Ok(tx.id(HashType::default_chain())) };
                    let _ = reply.send(verdict);
                }
                NetEvent::Report { txs, reply } => {
                    let _ = reply.send(fake_report(txs));
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
    let mut block = chain.build(txs, Vec::new(), producer.address(), now).unwrap();
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
    assert_eq!(contract["layout"], json!([{"name": "counter", "shape": "map", "kind": "number"}]));
    assert!(contract["code_size"].as_u64().unwrap() > 0);
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

    SignedTx::sign(tx, nonce, FeeOffer::NONE, &authority)
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
    let full = fake_node_with(BacklogLimits { total: 0, per_peer: 0 });
    let address = start(db, Some(full), &free(), "127.0.0.1:0".parse().unwrap(), ApiOptions::default()).unwrap();
    let (status, body) = post_tx(address, &wincode::serialize(&signed(5)).unwrap());

    assert_eq!(status, 503, "{body}");
    assert_eq!(body["error"], "the node is busy; try again shortly");
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
    let mut block = chain.build(vec![SignedTx::sign(transfer, 0, fee, &authority)], Vec::new(), producer.address(), now).unwrap();
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
        let mut block = chain.build(txs(height), Vec::new(), producer.address(), now).unwrap();
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
    let mut signers: Vec<_> = keys.iter().map(Keypair::public).collect();
    let config = crate::chain::AuthorityConfig {
        signers: signers.clone(),
        threshold: 2,
        change_threshold: 2,
        ..crate::chain::AuthorityConfig::dev(signers[0])
    };
    let proposal = vec![signers[0], signers[1]];
    let approve = SignedTx::sign(
        Tx::ApproveSigners {
            version: 0,
            signers: proposal.clone(),
            threshold: 2,
            change_threshold: 2,
        },
        0,
        FeeOffer::NONE,
        &keys[0],
    );
    // Another signer approves something else, then takes it back.
    let other = SignedTx::sign(
        Tx::ApproveSigners {
            version: 0,
            signers: vec![signers[1], signers[2]],
            threshold: 2,
            change_threshold: 2,
        },
        0,
        FeeOffer::NONE,
        &keys[1],
    );
    let withdraw = SignedTx::sign(Tx::WithdrawApproval { version: 0 }, 1, FeeOffer::NONE, &keys[1]);
    let (address, _) = serve_with(crate::chain::FinalityConfig::Authority(config), 3, |height| match height {
        1 => vec![approve.clone(), other.clone()],
        2 => vec![withdraw.clone()],
        _ => Vec::new(),
    });
    let listing = ok(address, "/validators");
    signers.sort();
    let mut proposed = proposal.clone();
    proposed.sort();
    let names = |keys: &[crate::crypto::keypair::PublicKey]| keys.iter().map(|key| key.to_string()).collect::<Vec<_>>();

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

#[cfg(feature = "finality-depth")]
#[test]
fn depth_chains_finalize_by_depth() {
    let authority = DevAuthority::dev_keypair();
    let burn = |nonce: u64| {
        let tx = Tx::Burn {
            from: authority.public().address(),
            amount: Amount::U128(1),
        };

        SignedTx::sign(tx, nonce, FeeOffer::NONE, &authority)
    };
    let (address, _) = serve_with(crate::chain::FinalityConfig::Depth(crate::chain::DepthConfig { depth: 3 }), 6, |height| {
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
    assert_eq!(ok(address, &format!("/txs/{}", id(1)))["final"], true);
    assert_eq!(ok(address, &format!("/txs/{}", id(5)))["final"], false);
    assert_eq!(get(address, "/validators").0, 404);
    assert_eq!(get(address, "/signers").0, 404);
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
        let newcomer = Keypair::generate(authority.public().key_type()).unwrap();
        let bond = SignedTx::sign(
            Tx::Bond {
                voter: newcomer.public(),
                amount: Amount::U128(10u128.pow(20)),
            },
            0,
            FeeOffer::NONE,
            &authority,
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
        );

        for height in 1..=18 {
            let now = chain.tip().timestamp + SLOT;
            let txs = match height {
                1 => vec![bond.clone()],
                12 => vec![transfer.clone()],
                _ => Vec::new(),
            };
            let proofs = chain.unpaid_proofs().unwrap();
            let mut block = chain.build(txs, proofs, producer.address(), now).unwrap();
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
    assert_eq!(node["backlog"], json!({ "waiting": 12, "total": 10_000, "per_peer": 1_000 }));
    assert_eq!(node["syncing"], Value::Null);

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
    let address = start(db, None, &genesis, "127.0.0.1:0".parse().unwrap(), ApiOptions::default()).unwrap();
    let probe = SignedTx::sign(
        Tx::Deploy {
            code: Code::Lua(COUNTER.to_string()),
        },
        0,
        FeeOffer::NONE,
        &authority,
    );
    let body = |extra: Value| {
        let mut body = json!({ "tx": to_hex(&wincode::serialize(&probe).unwrap()) });
        body.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());

        body.to_string()
    };
    let (status, _, offered) = request(address, "POST", "/offer", &body(json!({})));
    let fees = genesis.fees.as_ref().unwrap().build();
    let expected = fees.offer(chain.next_base_price(), genesis.tx_limits.fuel, &probe, OfferChoice::default());

    assert_eq!(status, 200, "{offered}");
    assert_eq!(offered, json::fee_offer(&expected));
    assert!(expected.gas_limit > fees.schedule().for_fuel(genesis.tx_limits.fuel));

    let (_, _, chosen) = request(address, "POST", "/offer", &body(json!({ "gas_limit": 77, "tip": "5" })));

    assert_eq!(chosen["gas_limit"], 77);
    assert_eq!(chosen["tip"], "5");
    assert_eq!(request(address, "POST", "/offer", &body(json!({ "tip": 5 }))).0, 400);

    let free = serve();

    assert_eq!(
        request(free.address, "POST", "/offer", &body(json!({}))).2,
        json!({ "gas_limit": 0, "max_price": "0", "tip": "0" })
    );
}

#[test]
fn pages_from_allowed_origins_may_call_the_api() {
    let db = Arc::new(MemoryDb::default());
    Chain::open(db.clone(), &free()).unwrap();
    let options = ApiOptions {
        cors: vec!["http://app.example".to_string()],
        explorer: None,
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

#[test]
fn the_explorer_is_served_beside_the_routes() {
    let dir = std::env::temp_dir().join(format!("chainv4-explorer-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("assets")).unwrap();
    std::fs::write(dir.join("index.html"), "<html>explorer</html>").unwrap();
    std::fs::write(dir.join("assets/app.wasm"), b"\0asm").unwrap();
    let db = Arc::new(MemoryDb::default());
    Chain::open(db.clone(), &free()).unwrap();
    let options = ApiOptions {
        cors: Vec::new(),
        explorer: Some(dir.clone()),
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

    std::fs::remove_dir_all(dir).unwrap();
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
    );
    produce(&mut fixture.chain, vec![deploy]);
    let bumper = contract_address(&authority.public().address(), 2, HashType::default_chain());
    let call = |body: Value| request(fixture.address, "POST", "/call", &body.to_string());
    let bump = json!({ "contract": bumper.to_string(), "function": "bump", "args": [{ "type": "number", "value": 5 }] });

    for _ in 0..2 {
        let (status, _, answer) = call(bump.clone());

        assert_eq!(status, 200, "{answer}");
        assert_eq!(answer["status"], "returned");
        assert_eq!(answer["values"], json!([{ "type": "number", "value": 5.0 }]));
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
    let address = start(db.clone(), None, &free(), "127.0.0.1:0".parse().unwrap(), ApiOptions::default()).unwrap();
    let deployed = |nonce: u64| contract_address(&authority.public().address(), nonce, HashType::default_chain());
    let (token, greeter) = (deployed(0), deployed(1));
    let deploy = |code: &str, nonce: u64| SignedTx::sign(Tx::Deploy { code: Code::Lua(code.to_string()) }, nonce, FeeOffer::NONE, &authority);
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

#[test]
fn events_are_listed_from_the_index() {
    let authority = DevAuthority::dev_keypair();
    let db = Arc::new(MemoryDb::default());
    let mut chain = Chain::open(db.clone(), &free()).unwrap();
    let address = start(db.clone(), None, &free(), "127.0.0.1:0".parse().unwrap(), ApiOptions::default()).unwrap();
    let greeter = contract_address(&authority.public().address(), 0, HashType::default_chain());

    assert_eq!(get(address, &format!("/events?contract={greeter}")).0, 404, "no index yet");

    chain.index_events(None).unwrap();
    let bob = Keypair::generate(authority.public().key_type()).unwrap().public().address();
    let greet = |nonce: u64| {
        let tx = Tx::ContractCall {
            to: greeter,
            func: Hash::Xxh3_64(crate::vm::selector("greet")),
            amount: Amount::U128(0),
            data: vec![TxCallArgumentType::Address(bob)],
        };

        SignedTx::sign(tx, nonce, FeeOffer::NONE, &authority)
    };
    produce(&mut chain, vec![SignedTx::sign(Tx::Deploy { code: Code::Lua(GREETER.to_string()) }, 0, FeeOffer::NONE, &authority)]);
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

    while chain.catch_up_events().unwrap() {}
    let caught_up = ok(address, &format!("/events?contract={greeter}"));

    assert_eq!((caught_up["indexed_from"].clone(), caught_up["catching_up_to"].clone()), (json!(0), Value::Null));
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
}
