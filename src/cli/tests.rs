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
    use crate::chain::{FinalityConfig, Genesis};
    use crate::consensus::{AuthorityProducer, ConsensusConfig, Producer};
    use crate::db::MemoryDb;
    use crate::net::{FakeNet, NetLink};
    use crate::node::{Node, now_ms};
    use std::net::SocketAddr;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread::JoinHandle;

    /// A node behind an HTTP API, stepping on a thread of its own until stopped.
    struct Running {
        api: SocketAddr,
        stop: Arc<AtomicBool>,
        thread: JoinHandle<(Node, FakeNet)>,
    }

    impl Running {
        /// A node on the dev authority chain, with blocks every 50 ms when it produces.
        fn start(produce: bool) -> Running {
            Running::launch(produce, false, FinalityConfig::dev(DevAuthority::dev_keypair().public()))
        }

        /// A producing node that also votes with the dev key, the dev chain's one validator, so
        /// it finalizes an epoch of 8 blocks about every 400 ms.
        #[cfg(feature = "finality-ffg")]
        fn validating() -> Running {
            Running::launch(true, true, FinalityConfig::dev(DevAuthority::dev_keypair().public()))
        }

        fn launch(produce: bool, validate: bool, finality: Option<FinalityConfig>) -> Running {
            const SLOT: u64 = 50;
            let authority = DevAuthority::dev_keypair();
            let genesis = Genesis {
                consensus: ConsensusConfig::Authority {
                    key: authority.public(),
                    slot_ms: SLOT,
                },
                timestamp: now_ms(),
                finality,
                ..Genesis::dev_authority(authority.public())
            };
            let producer = produce.then(|| Box::new(AuthorityProducer::new(authority, SLOT)) as Box<dyn Producer>);
            let db = Arc::new(MemoryDb::default());
            let (link, net) = NetLink::pair();
            let mut node = Node::new(db.clone(), &genesis, producer, link).unwrap();

            if validate {
                node = node.validate(DevAuthority::dev_keypair());
            }

            let api = crate::api::start(db, Some(node.submitter()), &genesis, "127.0.0.1:0".parse().unwrap()).unwrap();
            let stop = Arc::new(AtomicBool::new(false));
            let thread = std::thread::spawn({
                let stop = stop.clone();
                let mut node = node;

                move || {
                    while !stop.load(Ordering::Relaxed) {
                        node.step(now_ms()).unwrap();
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }

                    (node, net)
                }
            });

            Running { api, stop, thread }
        }

        fn args(&self, tx: TxArgs) -> SendArgs {
            SendArgs {
                node: format!("http://{}", self.api),
                key: None,
                no_wait: false,
                finalized: false,
                timeout: 10,
                confirmations: 1,
                tx,
            }
        }

        fn stop(self) -> Node {
            self.stop.store(true, Ordering::Relaxed);

            self.thread.join().unwrap().0
        }
    }

    #[test]
    fn without_waiting_send_returns_once_the_mempool_has_it() {
        let running = Running::start(false);
        let sent = send(SendArgs {
            no_wait: true,
            ..running.args(TxArgs::Transfer { to: Address::Ed25519([3; 32]), amount: u256::new(9) })
        });
        let node = running.stop();

        sent.unwrap();
        assert_eq!(node.mempool().len(), 1);

        let refused = send(SendArgs {
            node: "http://127.0.0.1:1".to_string(),
            ..Running::start(false).args(TxArgs::Burn { amount: u256::new(1) })
        });

        assert!(refused.unwrap_err().to_string().contains("cannot reach the node"));
    }

    #[test]
    fn send_waits_until_enough_blocks_hold_the_transaction() {
        let running = Running::start(true);
        let to = Keypair::generate(DevAuthority::dev_keypair().public().key_type()).unwrap().public().address();
        let sent = send(SendArgs {
            confirmations: 3,
            ..running.args(TxArgs::Transfer { to, amount: u256::new(9) })
        });
        let node = running.stop();

        sent.unwrap();
        assert!(node.chain().tip().height >= 3);
        assert!(node.mempool().is_empty());
    }

    #[test]
    fn a_transaction_that_lands_but_fails_is_an_error() {
        let running = Running::start(true);
        let burned = send(running.args(TxArgs::Burn { amount: u256::MAX }));
        running.stop();

        assert!(burned.unwrap_err().to_string().contains("the transaction failed: insufficient balance"));
    }

    #[test]
    fn waiting_gives_up_after_the_timeout() {
        let running = Running::start(false);
        let started = std::time::Instant::now();
        let waited = send(SendArgs {
            timeout: 1,
            ..running.args(TxArgs::Burn { amount: u256::new(1) })
        });
        running.stop();

        assert!(waited.unwrap_err().to_string().contains("was not in a block"));
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }

    #[cfg(feature = "finality-ffg")]
    #[test]
    fn final_waits_for_finality_not_just_a_block() {
        let running = Running::validating();
        let sent = send(SendArgs {
            finalized: true,
            ..running.args(TxArgs::Burn { amount: u256::new(1) })
        });
        let node = running.stop();

        sent.unwrap();
        // The burn is in block 1 or 2, final once epoch 1's checkpoint at height 8 is, which
        // takes epoch 2 justified on top of it.
        assert!(node.chain().finalized().unwrap().epoch >= 1);
        assert!(node.chain().tip().height >= 18);
    }

    #[test]
    fn final_gives_up_when_nothing_finalizes() {
        let running = Running::start(true);
        let waited = send(SendArgs {
            finalized: true,
            timeout: 1,
            ..running.args(TxArgs::Burn { amount: u256::new(1) })
        });
        running.stop();

        assert!(waited.unwrap_err().to_string().contains("was not final after 1s"));
    }

    #[cfg(feature = "finality-depth")]
    #[test]
    fn final_on_a_depth_chain_waits_for_the_depth() {
        let depth = crate::chain::DepthConfig { depth: 5 };
        let running = Running::launch(true, false, Some(FinalityConfig::Depth(depth)));
        let sent = send(SendArgs {
            finalized: true,
            ..running.args(TxArgs::Burn { amount: u256::new(1) })
        });
        let node = running.stop();

        sent.unwrap();
        // In block 1 or 2, so final once the tip is 5 blocks further.
        assert!(node.chain().tip().height >= 6);
        assert!(node.chain().finalized().unwrap().epoch >= 1);
    }

    #[cfg(feature = "finality-ffg")]
    #[test]
    fn validators_bond_and_unbond_with_send() {
        use crate::registry::validators::Validators;

        let running = Running::start(true);
        let voter = Keypair::generate(DevAuthority::dev_keypair().public().key_type()).unwrap().public();
        let bond = u256::new(10u128.pow(20));
        send(running.args(TxArgs::Bond { voter, amount: bond })).unwrap();
        send(running.args(TxArgs::Unbond { voter })).unwrap();
        let early = send(running.args(TxArgs::Withdraw { voter }));
        let node = running.stop();
        let record = Validators::new(&node.chain().state()).get(&voter).unwrap().unwrap();

        assert_eq!(record.account, DevAuthority::dev_keypair().public().address());
        assert_eq!(record.bond, bond);
        assert!(record.active_until.is_some());
        assert!(early.unwrap_err().to_string().contains("the transaction failed"));
    }
}
