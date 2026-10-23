use super::*;
use crate::chain::{Chain, Genesis};
use crate::consensus::DevAuthority;
use crate::crypto::keypair::Keypair;
use crate::db::MemoryDb;
use crate::registry::contracts::contract_address;
use crate::structs::amount::Amount;
use crate::structs::code::Code;
use crate::net::NetEvent;
use crate::structs::hex::to_hex;
use crate::structs::tx::{SignedTx, Tx};
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

/// A chain with a transfer to bob in block 1 and a deploy in block 2, served on a free port.
fn serve() -> Fixture {
    let authority = DevAuthority::dev_keypair();
    let producer = DevAuthority::new(authority.public(), SLOT, Some(authority.clone()));
    let db = Arc::new(MemoryDb::default());
    let mut chain = Chain::open(db.clone(), &Genesis::dev(authority.public())).unwrap();
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
        let signed = SignedTx::sign(tx, nonce as u64, &authority);
        let block = chain.build(vec![signed], &producer, now).unwrap();
        chain.import(&block, &producer, now).unwrap();
    }

    let address = start(db, Some(fake_node()), "127.0.0.1:0".parse().unwrap()).unwrap();

    Fixture { address, chain, bob }
}

/// A Submitter whose node takes every transaction except those with nonce 99, which it calls
/// stale.
fn fake_node() -> Submitter {
    let (inbox, events) = std::sync::mpsc::channel();

    std::thread::spawn(move || {
        for event in events {
            if let NetEvent::Submitted { tx, reply } = event {
                let verdict = if tx.nonce == 99 { Err(MempoolError::Stale) } else { Ok(tx.id()) };
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
    assert!(first["seal"].as_str().unwrap().starts_with("0x"));
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
    let counter = contract_address(&authority, 1);
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

    SignedTx::sign(tx, nonce, &authority)
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
    assert_eq!(body["id"], tx.id().to_string());
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
    Chain::open(db.clone(), &Genesis::dev(DevAuthority::dev_keypair().public())).unwrap();
    let address = start(db, None, "127.0.0.1:0".parse().unwrap()).unwrap();
    let (status, body) = post_tx(address, &wincode::serialize(&signed(0)).unwrap());

    assert_eq!(status, 503, "{body}");
}
