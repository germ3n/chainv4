use super::*;
use crate::chain::{Chain, Genesis};
use crate::consensus::{AuthorityProducer, DevAuthority, Producer};
use crate::crypto::keypair::Keypair;
use crate::db::MemoryDb;
use crate::registry::contracts::contract_address;
use crate::structs::amount::Amount;
use crate::structs::code::Code;
use crate::net::NetEvent;
use crate::structs::hex::to_hex;
use crate::structs::tx::{FeeOffer, SignedTx, Tx};
use std::io::{Read as _, Write as _};
use std::net::TcpStream;

const SLOT: u64 = 2_000;

const COUNTER: &str = r#"
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
    let address = start(db, Some(fake_node()), &genesis, "127.0.0.1:0".parse().unwrap()).unwrap();

    Fixture { address, chain, bob }
}

/// A Submitter whose node takes every transaction except those with nonce 99, which it calls
/// stale.
fn fake_node() -> Submitter {
    let (inbox, events) = std::sync::mpsc::channel();

    std::thread::spawn(move || {
        for event in events {
            if let NetEvent::Submitted { tx, reply } = event {
                let verdict = if tx.nonce == 99 { Err(MempoolError::Stale) } else { Ok(tx.id(HashType::default_chain())) };
                let _ = reply.send(verdict);
            }
        }
    });

    Submitter::new(inbox)
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
    let address = start(db, None, &genesis, "127.0.0.1:0".parse().unwrap()).unwrap();
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
    let address = start(db, None, &genesis, "127.0.0.1:0".parse().unwrap()).unwrap();
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
    let address = start(db, None, &genesis, "127.0.0.1:0".parse().unwrap()).unwrap();

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

    let address = start(db, Some(fake_node()), &genesis, "127.0.0.1:0".parse().unwrap()).unwrap();

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
    let (address, _) = serve_with(crate::chain::FinalityConfig::Authority(config), 3, |height| match height {
        1 => vec![approve.clone()],
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

    assert_eq!(current["version"], 0);
    assert_eq!(current["decided"], set);
    assert_eq!(current["active"], set);
    assert_eq!(
        current["proposals"],
        json!([{
            "signers": names(&proposed),
            "threshold": 2,
            "change_threshold": 2,
            "approvals": [keys[0].public().to_string()],
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
        let address = start(db, Some(fake_node()), &genesis, "127.0.0.1:0".parse().unwrap()).unwrap();

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
