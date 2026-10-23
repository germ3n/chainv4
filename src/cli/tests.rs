use super::*;

/// A fresh directory for one test, removed when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> TempDir {
        let path = std::env::temp_dir().join(format!("chainv4-cli-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();

        TempDir(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn key_files_round_trip_and_are_never_overwritten() {
    let dir = TempDir::new("keys");
    let path = dir.0.join("node.key");
    let keypair = Keypair::generate(DevAuthority::dev_keypair().public().key_type()).unwrap();
    write_key(&path, &keypair).unwrap();

    assert_eq!(read_key(&path).unwrap().public(), keypair.public());
    assert!(write_key(&path, &keypair).is_err());

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    }

    fs::write(dir.0.join("bad.key"), "ed25519:0x1234").unwrap();

    assert!(read_key(&dir.0.join("bad.key")).is_err());
}

#[test]
fn arguments_are_typed_by_their_prefix() {
    let address = Address::Contract([7; 32]);

    assert_eq!(parse_argument("i64:-5"), Ok(TxCallArgumentType::I64(-5)));
    assert_eq!(parse_argument("string:a:b"), Ok(TxCallArgumentType::String("a:b".to_string())));
    assert_eq!(parse_argument(&format!("address:{address}")), Ok(TxCallArgumentType::Address(address)));
    assert_eq!(parse_argument("bytes:0x00ff"), Ok(TxCallArgumentType::Bytes(vec![0, 255])));
    assert_eq!(parse_argument("amount:5"), Ok(TxCallArgumentType::Amount(Amount::U128(5))));
    assert!(parse_argument("5").is_err());
    assert!(parse_argument("i64:five").is_err());
    assert!(parse_argument("float:1").is_err());
}

#[test]
fn amounts_pick_the_narrowest_width() {
    let big = "1000000000000000000000000000000000000000";

    assert_eq!(amount(parse_amount("42").unwrap()), Amount::U128(42));
    assert!(matches!(amount(parse_amount(big).unwrap()), Amount::U256(_)));
    assert!(parse_amount("-1").is_err());
    assert!(parse_amount("1.5").is_err());
}

/// Sending needs a node behind an HTTP API to send to.
#[cfg(all(feature = "client", feature = "http"))]
mod send {
    use super::*;
    use crate::chain::Genesis;
    use crate::db::MemoryDb;
    use crate::net::NetLink;
    use crate::node::{Node, now_ms};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn send_signs_with_the_next_nonce_and_reaches_the_mempool() {
        let authority = DevAuthority::dev_keypair();
        let db = Arc::new(MemoryDb::default());
        let consensus = DevAuthority::new(authority.public(), 2_000, None);
        let (link, _net) = NetLink::pair();
        let node = Node::new(db.clone(), &Genesis::dev(authority.public()), Box::new(consensus), link).unwrap();
        let api = crate::api::start(db, Some(node.submitter()), "127.0.0.1:0".parse().unwrap()).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let running = std::thread::spawn({
            let stop = stop.clone();
            let mut node = node;

            move || {
                while !stop.load(Ordering::Relaxed) {
                    node.step(now_ms()).unwrap();
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }

                node
            }
        });
        let to = Address::Ed25519([3; 32]);
        let sent = send(SendArgs {
            node: format!("http://{api}"),
            key: None,
            tx: TxArgs::Transfer { to, amount: u256::new(9) },
        });
        stop.store(true, Ordering::Relaxed);
        let node = running.join().unwrap();

        sent.unwrap();
        assert_eq!(node.mempool().len(), 1);

        let refused = send(SendArgs {
            node: "http://127.0.0.1:1".to_string(),
            key: None,
            tx: TxArgs::Burn { amount: u256::new(1) },
        });

        assert!(refused.unwrap_err().to_string().contains("cannot reach the node"));
    }
}
