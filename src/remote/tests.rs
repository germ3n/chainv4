//! Outside connections against fake transports: no test here touches the network.

use {
    super::{
        beacon::{Beacon, response_chunks},
        bitcoin::{BitcoinApi, Bitcoind, BtcCoin, Esplora, esplora_fee_map, merkle_block_path, reply, scanned_coins},
        *,
    },
    serde_json::{Value, json},
    std::sync::{Arc, Mutex},
};

/// How a fake answers a request.
type Answering = Box<dyn Fn(&Request) -> Result<Reply, String> + Send + Sync>;

/// A transport answering by a function of the request, and keeping every request it saw.
struct Fake {
    answer: Answering,
    seen: Mutex<Vec<Request>>,
}

impl Fake {
    fn new(answer: impl Fn(&Request) -> Result<Reply, String> + Send + Sync + 'static) -> Arc<Fake> {
        Arc::new(Fake { answer: Box::new(answer), seen: Mutex::new(Vec::new()) })
    }

    fn hosts(&self) -> Vec<String> {
        self.seen.lock().unwrap().iter().map(|request| host_of(&request.url)).collect()
    }
}

impl Http for Fake {
    fn send(&self, request: &Request) -> Result<Reply, String> {
        self.seen.lock().unwrap().push(request.clone());

        (self.answer)(request)
    }
}

fn plain(urls: &[&str]) -> Endpoints {
    Endpoints::new(urls.iter().map(|url| Endpoint { url: url.to_string(), auth: Auth::None }).collect())
}

/// Asks through endpoints at a time, by a fake, as a client would: the reply judged.
fn ask(endpoints: &Endpoints, fake: &Fake, now: Instant) -> Result<String, String> {
    endpoints.ask_at(now, "a test", |endpoint| {
        let reply = judged(fake.send(&endpoint.request(false, "/x", &[], Vec::new())?).map_err(EndpointFailure::Endpoint)?)?;

        match reply.status {
            200 => Ok(reply.text()),
            _ => Err(EndpointFailure::Answer(reply.text())),
        }
    })
}

#[test]
fn the_first_url_is_asked_and_the_next_on_an_error_a_timeout_or_a_429() {
    let down = Arc::new(Mutex::new(vec!["https://a.example".to_string()]));
    let down_now = down.clone();
    let fake = Fake::new(move |request| {
        let host = host_of(&request.url);

        match host.as_str() {
            _ if down_now.lock().unwrap().contains(&host) => Err("timed out".to_string()),
            "https://b.example" => Ok(reply(429, "slow down")),
            _ => Ok(reply(200, &host)),
        }
    });
    let endpoints = plain(&["https://a.example", "https://b.example", "https://c.example/key/abc?token=1"]);
    let start = Instant::now();

    // a times out, b throttles, c answers: and c stays in use.
    assert_eq!(ask(&endpoints, &fake, start).unwrap(), "https://c.example");
    assert_eq!(fake.hosts(), ["https://a.example", "https://b.example", "https://c.example"]);
    fake.seen.lock().unwrap().clear();
    assert_eq!(ask(&endpoints, &fake, start + Duration::from_secs(60)).unwrap(), "https://c.example");
    assert_eq!(fake.hosts(), ["https://c.example"], "the one that works is asked first");

    // A few minutes on, the first is tried again, and kept once it answers.
    down.lock().unwrap().clear();
    fake.seen.lock().unwrap().clear();
    assert_eq!(ask(&endpoints, &fake, start + BACK_TO_FIRST + Duration::from_secs(1)).unwrap(), "https://a.example");
    assert_eq!(fake.hosts(), ["https://a.example"]);

    // An answer that is a failure is no endpoint failing: the next is not asked.
    let refusing = Fake::new(|_| Ok(reply(400, "bad request")));
    assert!(ask(&endpoints, &refusing, start).unwrap_err().contains("bad request"));
    assert_eq!(refusing.hosts().len(), 1);

    // Every one failing says so of each, by host alone.
    let failing = Fake::new(|_| Ok(reply(503, "")));
    let failed = ask(&endpoints, &failing, start).unwrap_err();
    assert!(failed.contains("https://a.example: it answered 503") && failed.contains("https://c.example: it answered 503"), "{failed}");
    assert!(!failed.contains("token") && !failed.contains("key/abc"), "{failed}");
}

#[test]
fn urls_are_shown_by_scheme_host_and_port_alone() {
    assert_eq!(host_of("https://user:secret@rpc.example:8443/v3/0123abcd?apikey=s3cret#x"), "https://rpc.example:8443");
    assert_eq!(host_of("http://127.0.0.1:8332"), "http://127.0.0.1:8332");
    assert_eq!(host_of("socks5h://me:pw@127.0.0.1:9050"), "socks5h://127.0.0.1:9050");
    assert_eq!(host_of("node.example/path"), "node.example");
}

#[test]
fn logins_come_from_a_cookie_a_private_file_or_the_environment_never_the_flag() {
    let dir = std::env::temp_dir().join(format!("chainv4-auth-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("login");
    std::fs::write(&file, "alice:wonder land\n").unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(Auth::parse(&format!("file:{}", file.display()), false).unwrap_err().contains("chmod 600"));
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    let basic = Auth::parse(&format!("file:{}", file.display()), false).unwrap();
    assert_eq!(basic, Auth::Basic { user: "alice".into(), password: "wonder land".into() });
    assert_eq!(basic.header().unwrap().unwrap(), format!("Basic {}", base64(b"alice:wonder land")));
    assert_eq!(format!("{basic:?}"), "a password", "never the secret");

    // A token alone is a bearer token.
    // SAFETY: tests set a variable of their own name, read nowhere else.
    unsafe { std::env::set_var("CHAINV4_TEST_BEACON_TOKEN", "t0ken") };
    assert_eq!(Auth::parse("env:CHAINV4_TEST_BEACON_TOKEN", false).unwrap().header().unwrap().unwrap(), "Bearer t0ken");
    assert!(Auth::parse("env:CHAINV4_TEST_NOT_SET", false).unwrap_err().contains("not set"));

    // bitcoind's cookie is read at each request, so a restarted bitcoind's new one is used.
    let cookie = dir.join(".cookie");
    std::fs::write(&cookie, "__cookie__:first").unwrap();
    let auth = Auth::parse(&format!("cookie:{}", cookie.display()), true).unwrap();
    assert_eq!(auth.header().unwrap().unwrap(), format!("Basic {}", base64(b"__cookie__:first")));
    std::fs::write(&cookie, "__cookie__:second").unwrap();
    assert_eq!(auth.header().unwrap().unwrap(), format!("Basic {}", base64(b"__cookie__:second")));
    assert!(Auth::parse(&format!("cookie:{}", cookie.display()), false).is_err(), "a cookie is bitcoind's");

    // A password as the flag's own value is refused, and the error does not repeat it.
    let refused = Auth::parse("hunter2", false).unwrap_err();
    assert!(refused.contains("never the password") && !refused.contains("hunter2"), "{refused}");

    // Matched to URLs by order; more logins than URLs is refused.
    let endpoints = Endpoints::from_flags(&["http://a:8332".into(), "http://b:8332".into()], &[format!("cookie:{}", cookie.display())], "bitcoind", true).unwrap();
    assert!(matches!(endpoints.list()[0].auth, Auth::Cookie(_)));
    assert_eq!(endpoints.list()[1].auth, Auth::None);
    assert!(Endpoints::from_flags(&["http://a".into()], &["env:A".into(), "env:B".into()], "beacon", false).is_err());

    assert_eq!(base64(b""), "");
    assert_eq!(base64(b"f"), "Zg==");
    assert_eq!(base64(b"fo"), "Zm8=");
    assert_eq!(base64(b"foo"), "Zm9v");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_proxy_is_socks5_or_socks5h() {
    assert!(parse_proxy("socks5://127.0.0.1:9050").is_ok());
    assert!(parse_proxy("socks5h://127.0.0.1:9050").is_ok());
    let refused = parse_proxy("http://user:pw@proxy.example:3128").unwrap_err();
    assert!(refused.contains("socks5") && !refused.contains("pw"), "{refused}");
    assert!(UreqHttp::new(Some("socks5h://127.0.0.1:9050")).is_ok());
}

/// A bitcoind answering each method from a table, as JSON-RPC.
fn bitcoind(answers: Vec<(&'static str, Value)>) -> (Bitcoind, Arc<Fake>) {
    let fake = Fake::new(move |request| {
        let asked: Value = serde_json::from_slice(&request.body).unwrap();
        let method = asked["method"].as_str().unwrap();

        match answers.iter().find(|(name, _)| *name == method) {
            Some((_, Value::Null)) => Ok(reply(500, &json!({ "result": null, "error": { "code": -5, "message": "No such mempool or blockchain transaction" }, "id": "chainv4" }).to_string())),
            Some((_, answer)) => Ok(reply(200, &json!({ "result": answer, "error": null, "id": "chainv4" }).to_string())),
            None => Ok(reply(500, &json!({ "result": null, "error": { "code": -32601, "message": "Method not found" }, "id": "chainv4" }).to_string())),
        }
    });

    (Bitcoind::new(fake.clone(), plain(&["http://127.0.0.1:8332"])), fake)
}

/// A merkleblock for some transactions, matching one, as gettxoutproof writes it.
fn merkleblock(ids: &[[u8; 32]], matched: usize) -> Vec<u8> {
    use crate::crypto::bitcoin::sha256d;

    let total = ids.len() as u64;
    let width = |height: u32| (total + (1 << height) - 1) >> height;
    let mut height = 0;

    while width(height) > 1 {
        height += 1;
    }

    fn node(ids: &[[u8; 32]], height: u32, pos: u64, width: &dyn Fn(u32) -> u64) -> [u8; 32] {
        match height {
            0 => ids[pos as usize],
            _ => {
                let left = node(ids, height - 1, pos * 2, width);
                let right = if pos * 2 + 1 < width(height - 1) { node(ids, height - 1, pos * 2 + 1, width) } else { left };

                crate::crypto::bitcoin::sha256d(&[left, right].concat())
            }
        }
    }

    fn build(ids: &[[u8; 32]], matched: usize, height: u32, pos: u64, width: &dyn Fn(u32) -> u64, bits: &mut Vec<bool>, hashes: &mut Vec<[u8; 32]>) {
        let above = (matched as u64 >> height) == pos;
        bits.push(above);

        if height == 0 || !above {
            hashes.push(node(ids, height, pos, width));
        } else {
            build(ids, matched, height - 1, pos * 2, width, bits, hashes);

            if pos * 2 + 1 < width(height - 1) {
                build(ids, matched, height - 1, pos * 2 + 1, width, bits, hashes);
            }
        }
    }

    let (mut bits, mut hashes) = (Vec::new(), Vec::new());
    build(ids, matched, height, 0, &width, &mut bits, &mut hashes);
    let mut header = vec![0x20; 80];
    header[36..68].copy_from_slice(&node(ids, height, 0, &width));
    let flags: Vec<u8> = bits.chunks(8).map(|byte| byte.iter().enumerate().fold(0u8, |acc, (at, bit)| acc | (u8::from(*bit) << at))).collect();
    let _ = sha256d;

    [header, (total as u32).to_le_bytes().to_vec(), vec![hashes.len() as u8], hashes.concat(), vec![flags.len() as u8], flags].concat()
}

#[test]
fn bitcoind_s_merkleblock_reads_as_the_path_the_proofs_take() {
    for (count, matched) in [(1, 0), (2, 1), (5, 3), (5, 4), (7, 0), (13, 12)] {
        let ids: Vec<[u8; 32]> = (0..count).map(|index| [index as u8 + 1; 32]).collect();
        let proof = merkleblock(&ids, matched);
        let (block, pos, path) = merkle_block_path(&proof, &ids[matched]).unwrap();

        assert_eq!(pos, matched as u64, "{count} transactions");
        assert_eq!(path, crate::crypto::bitcoin::merkle_path(&ids, matched), "{count} transactions, the {matched}th");
        assert_eq!(block, crate::crypto::bitcoin::sha256d(&proof[..80]));
        assert!(merkle_block_path(&proof, &[0xee; 32]).unwrap_err().contains("not of that transaction"));
    }

    let ids: Vec<[u8; 32]> = (0..5).map(|index| [index as u8 + 1; 32]).collect();
    let mut wrong = merkleblock(&ids, 2);
    wrong[40] ^= 1;
    assert!(merkle_block_path(&wrong, &ids[2]).unwrap_err().contains("Merkle root"));
    assert!(merkle_block_path(&wrong[..90], &ids[2]).is_err());

    // Through the RPC, its height read from the block's header.
    let mut display = ids[3];
    display.reverse();
    let txid = crate::structs::hex::to_hex(&display).trim_start_matches("0x").to_string();
    let (node, fake) = bitcoind(vec![("gettxoutproof", json!(crate::structs::hex::to_hex(&merkleblock(&ids, 3)).trim_start_matches("0x"))), ("getblockheader", json!({ "height": 840_000 }))]);
    let proof = node.merkle_proof(&txid, None).unwrap();
    assert_eq!((proof.block_height, proof.pos, proof.path), (840_000, 3, crate::crypto::bitcoin::merkle_path(&ids, 3)));
    let asked: Value = serde_json::from_slice(&fake.seen.lock().unwrap()[0].body).unwrap();
    assert_eq!(asked["params"], json!([[txid]]));
}

#[test]
fn bitcoind_s_coins_fees_and_lookups_read_from_its_json() {
    let scanned = json!({
        "success": true,
        "txouts": 160_000_000,
        "height": 900_000,
        "bestblock": "00000000000000000001",
        "unspents": [
            { "txid": "aa".repeat(32), "vout": 1, "scriptPubKey": "0014", "desc": "addr(bc1q…)#x", "amount": 0.00012345, "coinbase": false, "height": 899_000 },
            { "txid": "bb".repeat(32), "vout": 0, "scriptPubKey": "0014", "desc": "addr(bc1q…)#x", "amount": 20999999.9769, "coinbase": false, "height": 1 },
        ],
        "total_amount": 21000000.0,
    });
    assert_eq!(
        scanned_coins(&scanned).unwrap(),
        [BtcCoin { txid: "aa".repeat(32), vout: 1, value: 12_345 }, BtcCoin { txid: "bb".repeat(32), vout: 0, value: 2_099_999_997_690_000 }]
    );
    assert!(scanned_coins(&json!({ "success": false })).is_err());

    let (node, fake) = bitcoind(vec![
        ("scantxoutset", scanned),
        ("estimatesmartfee", json!({ "feerate": 0.00012, "blocks": 2 })),
        ("getrawtransaction", Value::Null),
        ("getblockcount", json!(900_123)),
        ("sendrawtransaction", json!("cc".repeat(32))),
    ]);
    assert_eq!(node.coins("bc1qexample").unwrap().len(), 2);
    let asked: Value = serde_json::from_slice(&fake.seen.lock().unwrap()[0].body).unwrap();
    assert_eq!(asked["params"], json!(["start", ["addr(bc1qexample)"]]), "no wallet: the UTXO set is scanned");
    assert!((node.fee_estimates().unwrap()[&1] - 12.0).abs() < 1e-9, "0.00012 bitcoin a kvB is 12 satoshis a vbyte");
    assert_eq!(node.raw_tx(&"dd".repeat(32)).unwrap(), None, "one it does not know");
    assert!(!node.tx_status(&"dd".repeat(32)).unwrap().confirmed);
    assert_eq!(node.tip_height().unwrap(), 900_123);
    assert_eq!(node.broadcast("0x0200").unwrap(), "cc".repeat(32));
    assert!(node.block_hash(5).unwrap_err().contains("Method not found"), "an RPC error is its answer, not it failing");

    // Regtest's estimatesmartfee has no rate to give.
    let (regtest, _) = bitcoind(vec![("estimatesmartfee", json!({ "errors": ["Insufficient data or no feerate found"], "blocks": 0 }))]);
    assert!(regtest.fee_estimates().unwrap().is_empty());

    // A refused login moves on to the next URL.
    let fake = Fake::new(|request| match host_of(&request.url).as_str() {
        "http://first:8332" => Ok(reply(401, "")),
        _ => Ok(reply(200, &json!({ "result": 7, "error": null }).to_string())),
    });
    let two = Bitcoind::new(fake, plain(&["http://first:8332", "http://second:8332"]));
    assert_eq!(two.tip_height().unwrap(), 7);
}

#[test]
fn esplora_reads_the_same_through_its_routes() {
    let fake = Fake::new(|request| {
        let path = request.url.trim_start_matches("https://esplora.example");

        Ok(match path {
            "/blocks/tip/height" => reply(200, "840000"),
            "/tx/ab/status" => reply(200, r#"{"confirmed":true,"block_height":839990,"block_hash":"00ff"}"#),
            "/tx/ab/merkle-proof" => reply(200, &format!(r#"{{"block_height":839990,"merkle":["{}"],"pos":1}}"#, "01".repeat(32))),
            "/fee-estimates" => reply(200, r#"{"1":87.5,"3":20.3}"#),
            "/tx" => reply(400, "sendrawtransaction RPC error: non-final"),
            _ => reply(404, "Transaction not found"),
        })
    });
    let esplora = Esplora::unpaced(fake, plain(&["https://esplora.example"]));

    assert_eq!(esplora.tip_height().unwrap(), 840_000);
    let status = esplora.tx_status("ab").unwrap();
    assert_eq!((status.confirmed, status.block_height, status.block_hash.as_deref()), (true, Some(839_990), Some("00ff")));
    assert!(!esplora.tx_status("cd").unwrap().confirmed, "one it does not know");
    assert_eq!(esplora.raw_tx("cd").unwrap(), None);
    assert_eq!(esplora.merkle_proof("ab", None).unwrap().path, vec![[1; 32]]);
    assert_eq!(esplora.fee_estimates().unwrap().len(), 2);
    assert!(esplora.broadcast("00").unwrap_err().contains("non-final"));
    assert_eq!(esplora_fee_map(r#"{"1":0,"2":-3,"x":4}"#).unwrap().len(), 0);
}

#[test]
fn the_beacon_api_s_updates_read_as_response_chunks() {
    let chunk = |payload: &[u8], with_context: bool| [((payload.len() + if with_context { 4 } else { 0 }) as u64).to_le_bytes().to_vec(), vec![0xd3, 0x1f, 0x61, 0x91], payload.to_vec()].concat();

    for with_context in [true, false] {
        let body = [chunk(&[1; 40], with_context), chunk(&[2; 7], with_context)].concat();
        assert_eq!(response_chunks(&body).unwrap(), [vec![1; 40], vec![2; 7]], "{with_context}");
    }

    assert!(response_chunks(&[1, 2, 3]).is_err());
    assert_eq!(response_chunks(&[]).unwrap(), Vec::<Vec<u8>>::new());

    // Asked for as SSZ, its login sent, and a 404 is nothing there.
    let fake = Fake::new(|request| {
        Ok(match request.url.as_str() {
            url if url.ends_with("/finality_update") => reply(200, "ssz"),
            url if url.contains("/blocks/9/root") => reply(200, &json!({ "data": { "root": format!("0x{}", "ab".repeat(32)) } }).to_string()),
            _ => reply(404, "not found"),
        })
    });
    let beacon = Beacon::new(fake.clone(), Endpoints::new(vec![Endpoint { url: "https://lodestar.example/".into(), auth: Auth::Bearer("t".into()) }]));
    assert_eq!(beacon.finality_update().unwrap(), b"ssz");
    let request = fake.seen.lock().unwrap()[0].clone();
    assert_eq!(request.url, "https://lodestar.example/eth/v1/beacon/light_client/finality_update");
    assert!(request.headers.contains(&("accept".to_string(), "application/octet-stream".to_string())));
    assert!(request.headers.contains(&("authorization".to_string(), "Bearer t".to_string())));
    assert_eq!(beacon.block_root(9).unwrap(), Some([0xab; 32]));
    assert_eq!(beacon.block_root(10).unwrap(), None);
    assert!(beacon.bootstrap(&[0; 32]).unwrap_err().contains("nothing at"));
}
