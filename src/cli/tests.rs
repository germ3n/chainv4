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

#[cfg(feature = "mnemonic")]
#[test]
fn keygen_writes_the_keys_a_seed_phrase_gives() {
    use clap::Parser;

    #[derive(Parser)]
    struct Keygen {
        #[command(flatten)]
        args: KeygenArgs,
    }

    let dir = TempDir::new("phrase");
    let made = |name: &str, flags: &[&str], input: &str| {
        let out = dir.0.join(name);
        let out_text = out.display().to_string();
        let args = Keygen::try_parse_from(["keygen", "--out", out_text.as_str()].into_iter().chain(flags.iter().copied())).unwrap().args;
        keygen_reading(args, &mut input.as_bytes()).unwrap();

        read_key(&out).unwrap().public().address().to_string()
    };
    let words = "test test test test test test test test test test test junk\n";

    // The addresses Ethereum's wallets show for these words.
    assert_eq!(made("a.key", &["--from-phrase", "--scheme", "secp256k1"], words), "secp256k1:0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266");
    assert_eq!(made("b.key", &["--from-phrase", "--scheme", "secp256k1", "--account", "1"], words), "secp256k1:0x70997970c51812dc3a010c7d01b50e0d17dc79c8");

    // The library's keys for the same account, and another passphrase makes another wallet.
    let seed = crate::crypto::mnemonic::Phrase::parse(words).unwrap().seed("");

    assert_eq!(made("c.key", &["--from-phrase"], words), seed.key(KeyType::Ed25519, 0).public().address().to_string());
    assert_ne!(made("d.key", &["--from-phrase", "--passphrase"], &format!("{words}secret\n")), made("e.key", &["--from-phrase"], words));
    assert!(Keygen::try_parse_from(["keygen", "--phrase", "--from-phrase"]).is_err());
    assert!(Keygen::try_parse_from(["keygen", "--phrase", "--words", "13"]).is_err());
    assert!(keygen_reading(Keygen::try_parse_from(["keygen", "--from-phrase", "--out", "/dev/null/x"]).unwrap().args, &mut "not a phrase\n".as_bytes()).is_err());
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
    assert_eq!(parse_argument("nil"), Ok(TxCallArgumentType::Nil));
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
        genesis: crate::structs::hash::Hash,
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
            Running::launch_with(produce, validate, finality, |node| node)
        }

        /// As launch, with the node set up further before it starts.
        fn launch_with(produce: bool, validate: bool, finality: Option<FinalityConfig>, set_up: impl FnOnce(Node) -> Node) -> Running {
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

            node = set_up(node);
            let api = crate::api::start(db, Some(node.handle()), &genesis, "127.0.0.1:0".parse().unwrap(), Default::default()).unwrap();
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

            Running {
                api,
                genesis: genesis.block().id(),
                stop,
                thread,
            }
        }

        fn args(&self, tx: TxArgs) -> SendArgs {
            SendArgs {
                node: format!("http://{}", self.api),
                key: None,
                no_wait: false,
                finalized: false,
                timeout: 10,
                confirmations: 1,
                gas_limit: None,
                max_price: None,
                tip: None,
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
    fn light_history_proves_each_transaction_that_touched_an_address() {
        use crate::chain::IndexKind;
        use clap::Parser;

        #[derive(Parser)]
        struct Light {
            #[command(flatten)]
            args: LightArgs,
        }

        let running = Running::launch_with(true, false, None, |node| node.keep_index(IndexKind::Addresses, None).unwrap());
        let to = Keypair::generate(DevAuthority::dev_keypair().public().key_type()).unwrap().public().address();
        send(running.args(TxArgs::Transfer { to, amount: u256::new(9) })).unwrap();
        let light_args = |query: &[&str]| {
            let node = format!("http://{}", running.api);
            let genesis = running.genesis.to_string();
            let args = ["chainv4", "--node", &node, "--genesis-id", &genesis].into_iter().chain(query.iter().copied());

            Light::try_parse_from(args).unwrap().args
        };
        let to_text = to.to_string();

        light(light_args(&["history", &to_text, "--limit", "5"])).unwrap();
        assert!(Light::try_parse_from(["chainv4", "history", &to_text, "--limit", "0"]).is_err(), "at least one");

        let unindexed = Running::start(true);
        let node = format!("http://{}", unindexed.api);
        let genesis = unindexed.genesis.to_string();
        let refused = light(Light::try_parse_from(["chainv4", "--node", &node, "--genesis-id", &genesis, "history", &to_text]).unwrap().args);

        assert!(refused.unwrap_err().to_string().contains("--index-addresses"));
        running.stop();
        unindexed.stop();
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

    #[test]
    fn send_pays_fees_with_what_the_flags_say() {
        let running = Running::start(true);
        let to = Keypair::generate(DevAuthority::dev_keypair().public().key_type()).unwrap().public().address();
        let tipped = send(SendArgs {
            tip: Some(u256::new(7)),
            ..running.args(TxArgs::Transfer { to, amount: u256::new(9) })
        });
        let starved = send(SendArgs {
            gas_limit: Some(1),
            ..running.args(TxArgs::Transfer { to, amount: u256::new(9) })
        });
        let node = running.stop();

        tipped.unwrap();
        assert!(starved.unwrap_err().to_string().contains("gas limit under its intrinsic cost"));

        // The tip went in as given, and the default gas limit was the transfer's intrinsic cost.
        let block = node.chain().block_at(node.chain().tip().height).unwrap().unwrap();
        let landed = (1..=node.chain().tip().height)
            .flat_map(|height| node.chain().block_at(height).unwrap().unwrap().txs)
            .find(|tx| matches!(tx.tx, crate::structs::tx::Tx::Transfer { .. }))
            .expect("the transfer landed");
        let fees = node.chain().fees().unwrap();

        assert_eq!(landed.fee.tip, u256::new(7));
        assert_eq!(landed.fee.gas_limit, fees.schedule().intrinsic(&landed));
        assert!(block.header.base_price > u256::ZERO);
    }

    #[cfg(feature = "finality-authority")]
    #[test]
    fn signers_change_with_send() {
        use crate::registry::signers::Signers;

        let dev = DevAuthority::dev_keypair().public();
        let finality = FinalityConfig::Authority(crate::chain::AuthorityConfig::dev(dev));
        let running = Running::launch(true, false, Some(finality));
        let newcomer = Keypair::generate(dev.key_type()).unwrap().public();
        let approve = |add: Vec<_>, version: Option<u64>| TxArgs::ApproveSigners {
            add,
            remove: Vec::new(),
            threshold: Some(2),
            change_threshold: Some(2),
            version,
        };
        // The dev key is the one signer, and one approval changes the set.
        let added = send(running.args(approve(vec![newcomer], None)));
        let stale = send(running.args(approve(Vec::new(), Some(0))));
        let already = send(running.args(approve(vec![dev], None)));
        // Now two must approve: the dev key's approval waits, until it takes it back.
        let third = Keypair::generate(dev.key_type()).unwrap().public();
        let pending = send(running.args(approve(vec![third], None)));
        let withdrawn = send(running.args(TxArgs::WithdrawApproval { version: None }));
        let nothing_left = send(running.args(TxArgs::WithdrawApproval { version: None }));
        let node = running.stop();
        let state = node.chain().state();
        let signers = Signers::new(&state);
        let decided = signers.decided().unwrap();

        added.unwrap();
        assert!(stale.unwrap_err().to_string().contains("the signers changed since"));
        assert!(already.unwrap_err().to_string().contains("is a signer already"));
        pending.unwrap();
        withdrawn.unwrap();
        assert!(nothing_left.unwrap_err().to_string().contains("no approval"));
        assert_eq!(decided.version, 1);
        assert_eq!(decided.set.signers.len(), 2);
        assert!(decided.set.signers.contains(&newcomer));
        assert!(signers.ballot().unwrap().approvals.is_empty());
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

#[test]
fn layout_checks_new_code_against_older_files() {
    let dir = TempDir::new("layout");
    let write = |name: &str, source: &str| {
        let path = dir.0.join(name);
        fs::write(&path, format!("{source}\nlocal M = {{}}\nfunction M.f() end\nreturn M\n")).unwrap();

        path
    };
    let v1 = write("v1.lua", "--@namespace counter number\n--@namespace fees u256");
    let v2 = write("v2.lua", "--@namespace counter number\n--@retired fees");
    let retyped = write("retyped.lua", "--@namespace counter text\n--@retired fees");
    let check = |code: &PathBuf, from: &PathBuf| {
        layout(LayoutArgs {
            code: code.clone(),
            from: Some(from.display().to_string()),
            node: String::new(),
        })
        .map_err(|err| err.to_string())
    };

    check(&v2, &v1).unwrap();
    assert!(check(&retyped, &v2).unwrap_err().contains("held number, and now holds text"));
    assert!(check(&v1, &v2).unwrap_err().contains("stays retired"));
    assert!(check(&v1, &dir.0.join("v1.txt")).unwrap_err().contains(".lua, .luau or .wasm"));
}
