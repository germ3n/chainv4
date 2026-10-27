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

    // And the address Bitcoin's wallets show for BIP84's words.
    #[cfg(feature = "bitcoin")]
    assert_eq!(
        made("f.key", &["--from-phrase", "--scheme", "bitcoin"], "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about\n"),
        "bc1qcr8te4kr609gcawutmrza0j4xv80jy8z306fyu"
    );

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
fn i256_arguments_take_a_sign_and_the_full_range() {
    use crate::structs::u256::i256;

    let min = "-57896044618658097711785492504343953926634992332820282019728792003956564819968";

    assert_eq!(parse_argument("i256:-5"), Ok(TxCallArgumentType::I256(i256::new(-5))));
    assert_eq!(parse_argument(&format!("i256:{min}")), Ok(TxCallArgumentType::I256(i256::MIN)));
    assert_eq!(parse_argument(&format!("i256:{}", i256::MAX)), Ok(TxCallArgumentType::I256(i256::MAX)));
    assert!(parse_argument("i256:57896044618658097711785492504343953926634992332820282019728792003956564819968").is_err());
    assert!(parse_argument("i256:1.5").is_err());
    assert!(parse_argument("i256:").is_err());
    assert!(parse_argument("float:1").unwrap_err().contains("i128, u128, i256, u256"));
}

#[test]
fn amounts_pick_the_narrowest_width() {
    let big = "1000000000000000000000000000000000000000";

    assert_eq!(amount(parse_amount("42").unwrap()), Amount::U128(42));
    assert!(matches!(amount(parse_amount(big).unwrap()), Amount::U256(_)));
    assert!(parse_amount("-1").is_err());
    assert!(parse_amount("1.5").is_err());
}

#[test]
fn db_migrate_goes_up_freely_and_back_down_only_when_told_to() {
    use crate::chain::{
        STORAGE_FORMAT,
        migrate::{self, At, Chunk, Migration},
    };
    use crate::db::{Column, DbError, Read};

    /// Up to this build's format, marks the database; going back, unmarks it.
    struct Mark;

    impl Migration for Mark {
        fn from(&self) -> u32 {
            STORAGE_FORMAT - 1
        }

        fn describe(&self) -> &'static str {
            "marks the database"
        }

        fn up(&self, _: &At) -> Result<Chunk, DbError> {
            Ok(Chunk {
                changes: [((Column::Storage, b"marked".to_vec()), Some(b"yes".to_vec()))].into(),
                next: None,
            })
        }

        fn down(&self, _: &At) -> Result<Chunk, DbError> {
            Ok(Chunk {
                changes: [((Column::Storage, b"marked".to_vec()), None)].into(),
                next: None,
            })
        }
    }

    let dir = TempDir::new("migrate");
    let path = dir.0.join("chain.db");
    migrate::set_format(&crate::db::RocksDb::open(&path).unwrap(), STORAGE_FORMAT - 1);
    let migrations: &[&dyn Migration] = &[&Mark];
    let args = |to: Option<u32>, yes: bool, no_backup: bool| db::MigrateArgs {
        db: path.clone(),
        to,
        yes,
        no_backup,
    };
    let state = || {
        let db = crate::db::RocksDb::open(&path).unwrap();

        (migrate::format_of(&db).unwrap(), db.read(Column::Storage, b"marked").unwrap().is_some())
    };
    let older = STORAGE_FORMAT - 1;

    // Up asks nothing, and copies the database aside first.
    db::migrate_reading(args(None, false, false), migrations, &mut "".as_bytes()).unwrap();

    assert_eq!(state(), (Some(STORAGE_FORMAT), true));
    assert!(dir.0.join(format!("chain.db.format-{older}")).is_dir(), "the backup");

    // Down asks, and anything but yes leaves it as it was.
    let refused = db::migrate_reading(args(Some(older), false, true), migrations, &mut "no\n".as_bytes()).unwrap_err();

    assert!(refused.to_string().contains("is left as it was"), "{refused}");
    assert_eq!(state(), (Some(STORAGE_FORMAT), true));

    db::migrate_reading(args(Some(older), false, true), migrations, &mut "yes\n".as_bytes()).unwrap();

    assert_eq!(state(), (Some(older), false));

    // --yes answers for you; a second backup takes the next free name.
    db::migrate_reading(args(None, false, false), migrations, &mut "".as_bytes()).unwrap();
    db::migrate_reading(args(Some(older), true, false), migrations, &mut "".as_bytes()).unwrap();

    assert_eq!(state(), (Some(older), false));
    assert!(dir.0.join(format!("chain.db.format-{older}.1")).is_dir());
    assert!(dir.0.join(format!("chain.db.format-{STORAGE_FORMAT}")).is_dir());

    // Nothing to do, nowhere to go, and nothing there.
    db::migrate_reading(args(Some(older), false, false), migrations, &mut "".as_bytes()).unwrap();
    db::format(&path, migrations).unwrap();

    assert!(db::migrate_reading(args(Some(older - 1), true, true), migrations, &mut "".as_bytes()).unwrap_err().to_string().contains("no migration goes from"));
    assert!(db::migrate_reading(db::MigrateArgs { db: dir.0.join("missing"), ..args(None, false, false) }, migrations, &mut "".as_bytes())
        .unwrap_err()
        .to_string()
        .contains("there is no database at"));
    assert!(!dir.0.join("missing").exists(), "never made by asking");
}

#[cfg(feature = "client")]
#[test]
fn a_lua_contract_is_put_together_from_its_files_and_checked_as_a_node_would() {
    use crate::{chain::DEV_TX_LIMITS, structs::code::Engine, structs::hash::HashType, vm::Vm};

    let dir = TempDir::new("include");
    fs::create_dir_all(dir.0.join("lib")).unwrap();
    fs::write(dir.0.join("main.lua"), "local M = {}\n--@include \"lib/balances.lua\"\n\nfunction M.balance(owner)\n    return get(owner)\nend\n\nreturn M\n").unwrap();
    fs::write(
        dir.0.join("lib/balances.lua"),
        "--@namespace balances decimal\nlocal balances = storage.namespace(\"balances\")\n\nlocal function get(owner)\n    return balances:get(owner:text()) or \"0\"\nend\n",
    )
    .unwrap();
    fs::write(dir.0.join("broken.lua"), "local M = {}\n--@include \"lib/broken.lua\"\nreturn M\n").unwrap();
    fs::write(dir.0.join("lib/broken.lua"), "local fine = 1\nlocal broken = )\n").unwrap();

    let source = super::send::lua_source(&dir.0.join("main.lua")).unwrap();
    let vm = Vm::new(&[Engine::Lua], HashType::default_chain()).unwrap();
    let loaded = vm.engine(Engine::Lua).unwrap().load(source.as_bytes(), DEV_TX_LIMITS).result.unwrap();

    assert!(source.contains("-- included lib/balances.lua\n--@namespace balances decimal\n"), "{source}");
    assert_eq!(loaded.functions, ["balance"], "the included local serves the entry's function");
    assert!(loaded.layout.is_declared(), "the layout the included file declares");

    // A mistake in an included file is named by that file and line, by send and by layout alike.
    let sent = super::send::lua_source(&dir.0.join("broken.lua")).unwrap_err().to_string();
    let checked = layout(LayoutArgs {
        code: dir.0.join("broken.lua"),
        from: None,
        node: String::new(),
        #[cfg(feature = "client")]
        tls: Default::default(),
    })
    .unwrap_err()
    .to_string();

    assert!(sent.contains("syntax error: lib/broken.lua:2:"), "{sent}");
    assert!(checked.contains("lib/broken.lua:2:"), "{checked}");
    assert!(super::send::lua_source(&dir.0.join("missing.lua")).is_err());
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
            Running::launch_governed(produce, validate, finality, None, set_up)
        }

        /// As launch_with, on a chain with this governance.
        fn launch_governed(
            produce: bool,
            validate: bool,
            finality: Option<FinalityConfig>,
            governance: Option<crate::governance::GovernanceConfig>,
            set_up: impl FnOnce(Node) -> Node,
        ) -> Running {
            Running::launch_serving(produce, validate, finality, governance, set_up, Default::default())
        }

        /// As launch_governed, with the API serving more than the chain, such as a faucet.
        fn launch_serving(
            produce: bool,
            validate: bool,
            finality: Option<FinalityConfig>,
            governance: Option<crate::governance::GovernanceConfig>,
            set_up: impl FnOnce(Node) -> Node,
            options: crate::api::ApiOptions,
        ) -> Running {
            Running::launch_genesis(produce, validate, |genesis| Genesis { finality, governance, ..genesis }, set_up, options)
        }

        /// A node whose dev authority genesis is changed as asked, set up as asked, behind an API.
        fn launch_genesis(produce: bool, validate: bool, change: impl FnOnce(Genesis) -> Genesis, set_up: impl FnOnce(Node) -> Node, options: crate::api::ApiOptions) -> Running {
            const SLOT: u64 = 50;
            let authority = DevAuthority::dev_keypair();
            let genesis = change(Genesis {
                consensus: ConsensusConfig::Authority {
                    key: authority.public(),
                    slot_ms: SLOT,
                },
                timestamp: now_ms(),
                ..Genesis::dev_authority(authority.public())
            });
            let producer = produce.then(|| Box::new(AuthorityProducer::new(authority, SLOT)) as Box<dyn Producer>);
            let db = Arc::new(MemoryDb::default());
            let (link, net) = NetLink::pair();
            let mut node = Node::new(db.clone(), &genesis, producer, link).unwrap();

            if validate {
                node = node.validate(DevAuthority::dev_keypair());
            }

            node = set_up(node);
            let api = crate::api::start(db, Some(node.handle()), &genesis, "127.0.0.1:0".parse().unwrap(), options).unwrap();
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
                tls: Default::default(),
                key: None,
                no_wait: false,
                finalized: false,
                timeout: 10,
                confirmations: 1,
                gas_limit: None,
                anyway: false,
                eip712: false,
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
            ..running.args(TxArgs::Transfer { to: Address::Ed25519([3; 32]).into(), amount: u256::new(9) })
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
        send(running.args(TxArgs::Transfer { to: to.into(), amount: u256::new(9) })).unwrap();
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
            ..running.args(TxArgs::Transfer { to: to.into(), amount: u256::new(9) })
        });
        let node = running.stop();

        sent.unwrap();
        assert!(node.chain().tip().height >= 3);
        assert!(node.mempool().is_empty());
    }

    #[cfg(feature = "secp256k1")]
    #[test]
    fn send_signs_as_typed_data_for_the_chain_with_eip712() {
        use crate::crypto::keypair::{KeyType, Signature};
        use crate::registry::Accounts;

        let running = Running::start(true);
        let ethereum = Keypair::generate(KeyType::Secp256k1).unwrap();
        send(running.args(TxArgs::Transfer { to: ethereum.public().address().into(), amount: u256::new(10u128.pow(21)) })).unwrap();

        let dir = TempDir::new("eip712");
        let key = dir.0.join("ethereum.key");
        fs::write(&key, ethereum.to_key_file()).unwrap();
        let to = Keypair::generate(DevAuthority::dev_keypair().public().key_type()).unwrap().public().address();
        let sent = send(SendArgs {
            key: Some(key),
            eip712: true,
            ..running.args(TxArgs::Transfer { to: to.into(), amount: u256::new(9) })
        });

        // Typed data is Ethereum's: an Ed25519 key cannot sign it.
        let refused = send(SendArgs {
            eip712: true,
            ..running.args(TxArgs::Transfer { to: to.into(), amount: u256::new(1) })
        });
        let node = running.stop();

        sent.unwrap();

        if DevAuthority::dev_keypair().public().key_type() != KeyType::Secp256k1 {
            assert!(refused.unwrap_err().to_string().contains("needs a secp256k1 key"));
        }
        assert_eq!(Accounts::new(&node.chain().state()).get(&to).unwrap().balance, u256::new(9));

        let typed = (1..=node.chain().tip().height)
            .flat_map(|height| node.chain().block_at(height).unwrap().unwrap().txs)
            .find(|tx| tx.sender() == ethereum.public().address())
            .unwrap();

        assert!(matches!(typed.signature, Signature::Secp256k1Eip712(_)), "it went as typed data");
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

    #[cfg(all(feature = "finality-ffg", feature = "governance-council"))]
    #[test]
    fn approve_change_and_withdraw_change_go_through_governance() {
        use crate::governance::GovernanceConfig;

        let dev = DevAuthority::dev_keypair();
        let council = GovernanceConfig::Council {
            members: vec![dev.public().address()],
            threshold: 1,
            approval_epochs: 64,
        };
        let running = Running::launch_governed(true, false, Some(FinalityConfig::Ffg(crate::chain::FfgConfig::dev(dev.public()))), Some(council), |node| node);
        let staked = r#"{"type":"staked","slot_ms":2000,"schedule":"draw"}"#.to_string();
        let windowed = r#"{"type":"pow","hash":"blake3","target_block_ms":2000,"initial_difficulty":"1","min_difficulty":"1","window":5}"#.to_string();

        // A window stays genesis's; nothing to withdraw before approving; the council of one
        // decides at once, so there is nothing to withdraw after either.
        let bootnode = "/ip4/203.0.113.7/udp/4001/quic-v1".to_string();
        let kept = send(running.args(TxArgs::ApproveChange { consensus: Some(windowed), bootnodes: None, name_prices: None, collections: None, federation: None, directory: None, version: None }));
        let nothing = send(running.args(TxArgs::WithdrawChange { version: None }));
        let approved = send(running.args(TxArgs::ApproveChange { consensus: Some(staked), bootnodes: None, name_prices: None, collections: None, federation: None, directory: None, version: None }));
        let decided = send(running.args(TxArgs::WithdrawChange { version: None }));
        let unreadable = send(running.args(TxArgs::ApproveChange { consensus: None, bootnodes: Some(vec!["203.0.113.7:4001".to_string()]), name_prices: None, collections: None, federation: None, directory: None, version: None }));
        let booted = send(running.args(TxArgs::ApproveChange { consensus: None, bootnodes: Some(vec![bootnode.clone()]), name_prices: None, collections: None, federation: None, directory: None, version: None }));
        let governance = crate::cli::send::ok(crate::cli::send::agent(&Default::default()).unwrap().get(format!("http://{}/governance", running.api)).call()).unwrap();
        let node = running.stop();

        assert!(unreadable.unwrap_err().to_string().contains("each bootnode is a libp2p address"));
        booted.unwrap();
        assert_eq!(node.chain().bootnodes().unwrap(), std::slice::from_ref(&bootnode), "decided at once");
        assert_eq!(governance["bootnodes"], serde_json::json!([bootnode]));

        assert!(kept.unwrap_err().to_string().contains("keeps the chain's window"));
        let nothing = nothing.unwrap_err().to_string();
        let decided = decided.unwrap_err().to_string();

        assert!(nothing.contains("no approval"), "{nothing}");
        approved.unwrap();
        assert!(decided.contains("no approval"), "{decided}");
        assert_eq!(governance["version"], 2, "the consensus, then the bootnodes");
        assert_eq!(governance["decided"]["type"], "staked");
        assert!(node.chain().tip().height >= 2);
    }

    #[test]
    fn calls_get_a_gas_limit_from_a_dry_run_and_one_that_would_fail_needs_anyway() {
        let dir = TempDir::new("dry-run");
        let source = dir.0.join("counter.lua");
        fs::write(&source, "local M = {}\n\nfunction M.bump(by)\n    local count = tonumber(storage.get(\"count\") or \"0\") + by\n    storage.set(\"count\", tostring(count))\n    return count\nend\n\nfunction M.boom()\n    error(\"boom\")\nend\n\nreturn M\n").unwrap();
        let running = Running::start(true);
        let deployer = DevAuthority::dev_keypair().public().address();
        let contract = deployed_at(&deployer, 0, crate::structs::hash::HashType::default_chain());
        let call = |function: &str, args: Vec<TxCallArgumentType>| TxArgs::Call { to: contract, function: function.to_string(), amount: u256::ZERO, args };

        send(running.args(TxArgs::Deploy { lua: Some(source), wasm: None, evm: None, constructor: None, constructor_args: Vec::new() })).unwrap();
        send(running.args(call("bump", vec![TxCallArgumentType::I64(2)]))).unwrap();
        let refused = send(running.args(call("boom", Vec::new())));
        let anyway = send(SendArgs { anyway: true, ..running.args(call("boom", Vec::new())) });
        let node = running.stop();

        assert!(refused.unwrap_err().to_string().contains("a dry run says this would fail"), "nothing is sent without --anyway");
        assert!(anyway.unwrap_err().to_string().contains("the transaction failed"), "sent anyway, it lands and fails as the dry run said");

        let calls: Vec<crate::structs::tx::SignedTx> = (1..=node.chain().tip().height)
            .flat_map(|height| node.chain().block_at(height).unwrap().unwrap().txs)
            .filter(|tx| matches!(tx.tx, crate::structs::tx::Tx::ContractCall { .. }))
            .collect();
        let fees = node.chain().fees().unwrap();
        let cap = |tx: &crate::structs::tx::SignedTx| fees.offer(u256::ZERO, crate::chain::DEV_TX_LIMITS.fuel, tx, crate::chain::fees::OfferChoice::default()).gas_limit;

        assert_eq!(calls.len(), 2, "the bump, and the boom sent anyway");
        assert!(calls[0].fee.gas_limit < cap(&calls[0]) / 10, "the bump's limit is what it used and a quarter, not the cap: {}", calls[0].fee.gas_limit);
        assert!(calls[0].fee.gas_limit > fees.schedule().intrinsic(&calls[0]));
        assert_eq!(calls[1].fee.gas_limit, cap(&calls[1]), "a call that would fail keeps the cap, things may change");
    }

    #[cfg(feature = "evm")]
    #[test]
    fn evm_contracts_deploy_take_calls_and_read_from_the_cli() {
        use crate::structs::tx::{FeeOffer, SignedTx, Tx};

        let running = Running::start(true);
        let deployer = DevAuthority::dev_keypair();
        let counter = crate::vm::evm::create_address(&deployer.public().address(), 0);
        let bin = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/contracts/evm/Counter.bin"));
        let call = |function: &str, args: &[&str], read: bool| TxArgs::EvmCall {
            to: counter,
            function: Some(function.to_string()),
            data: None,
            value: u256::ZERO,
            read,
            args: args.iter().map(|arg| arg.to_string()).collect(),
        };

        send(running.args(TxArgs::Deploy {
            lua: None,
            wasm: None,
            evm: Some(bin),
            constructor: Some("(uint256)".to_string()),
            constructor_args: vec!["5".to_string()],
        }))
        .unwrap();
        send(running.args(call("bump(uint256)", &["3"], false))).unwrap();
        send(running.args(call("count()", &[], true))).expect("a read sends nothing and fails at nothing");
        let refused = send(running.args(call("bump(uint256)", &["0"], false)));
        let wrong = send(running.args(call("bump(uint256)", &["three"], false)));

        // What --read prints, from the node's dry run of count().
        let probe = SignedTx::sign(Tx::EvmCall { to: counter, value: Amount::U128(0), input: super::super::evm::call("count()", &[]).unwrap() }, 3, FeeOffer::NONE, &deployer);
        let answer: serde_json::Value = serde_json::from_str(
            &ureq::post(format!("http://{}/offer", running.api))
                .header("content-type", "application/json")
                .send(serde_json::json!({ "tx": crate::structs::hex::to_hex(&wincode::serialize(&probe).unwrap()) }).to_string())
                .unwrap()
                .body_mut()
                .read_to_string()
                .unwrap(),
        )
        .unwrap();
        let shown = super::super::send::read(&answer["estimate"]).unwrap();
        let node = running.stop();

        assert!(shown.starts_with(&format!("0x{:064x} (8)", 8)), "five and three: {shown}");
        assert!(refused.unwrap_err().to_string().contains("nothing to add"), "the revert's reason, from the dry run");
        assert!(wrong.unwrap_err().to_string().contains("three as uint256"));

        let sent: Vec<Tx> = (1..=node.chain().tip().height).flat_map(|height| node.chain().block_at(height).unwrap().unwrap().txs).map(|tx| tx.tx).collect();

        assert_eq!(sent.iter().filter(|tx| matches!(tx, Tx::EvmCall { .. })).count(), 1, "the bump alone: the read and the refused were never sent");
        assert!(matches!(&sent[0], Tx::Deploy { code: crate::structs::code::Code::Evm(code) } if code.ends_with(&u256::new(5).to_be_bytes())), "the constructor's argument after the code");
    }

    #[cfg(feature = "names")]
    #[test]
    fn names_register_point_and_receive_from_the_cli() {
        use super::super::send::NameArgs;
        use crate::registry::names::Names;

        let running = Running::start(true);
        let me = DevAuthority::dev_keypair().public().address();
        let bob = Keypair::generate(DevAuthority::dev_keypair().public().key_type()).unwrap().public().address();
        let name = |action: NameArgs| TxArgs::Name { action };

        send(running.args(name(NameArgs::Register { name: "Alice.dev".into(), years: 2 }))).unwrap();
        send(running.args(name(NameArgs::Subname { name: "alice".into(), label: "pay".into(), owner: Some(bob) }))).unwrap();
        send(running.args(name(NameArgs::Text { name: "alice.dev".into(), key: "url".into(), value: Some("https://alice.example".into()) }))).unwrap();
        send(running.args(name(NameArgs::Primary { name: Some("alice.dev".into()) }))).unwrap();
        // Paid by name: pay.alice.dev resolves to bob.
        send(running.args(TxArgs::Transfer { to: "pay.alice.dev".parse().unwrap(), amount: u256::new(9) })).unwrap();
        let taken = send(running.args(name(NameArgs::Register { name: "alice".into(), years: 1 })));
        let bad = send(running.args(name(NameArgs::Register { name: "al".into(), years: 1 })));
        let nowhere = send(running.args(TxArgs::Transfer { to: "nobody.dev".parse().unwrap(), amount: u256::new(1) }));
        let node = running.stop();
        let state = node.chain().state();
        let now = node.chain().tip().timestamp;
        let names = Names::new(&state);

        let taken = taken.unwrap_err().to_string();

        assert!(taken.contains("a dry run says this would fail") && taken.contains("yours already"), "said before anything is sent: {taken}");
        assert!(bad.unwrap_err().to_string().contains("at least 3"));
        assert!(nowhere.unwrap_err().to_string().contains("resolves to nothing"));
        assert_eq!(names.resolve("alice", now, u64::MAX).unwrap(), Some(me));
        assert_eq!(names.primary(&me, now, u64::MAX).unwrap(), Some("alice".to_string()));
        assert_eq!(names.record("alice").unwrap().unwrap().texts[0].value, "https://alice.example");
        assert_eq!(crate::registry::Accounts::new(&state).get(&bob).unwrap().balance, u256::new(9));
    }

    #[test]
    fn send_pays_fees_with_what_the_flags_say() {
        let running = Running::start(true);
        let to = Keypair::generate(DevAuthority::dev_keypair().public().key_type()).unwrap().public().address();
        let tipped = send(SendArgs {
            tip: Some(u256::new(7)),
            ..running.args(TxArgs::Transfer { to: to.into(), amount: u256::new(9) })
        });
        let starved = send(SendArgs {
            gas_limit: Some(1),
            ..running.args(TxArgs::Transfer { to: to.into(), amount: u256::new(9) })
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
        let owner = Keypair::generate(dev.key_type()).unwrap().public().address();
        let added = send(running.args(approve(vec![format!("{newcomer}@{owner}")], None)));
        let stale = send(running.args(approve(Vec::new(), Some(0))));
        let already = send(running.args(approve(vec![dev.to_string()], None)));
        let garbled = send(running.args(approve(vec!["nobody@nowhere".to_string()], None)));
        // Now two must approve: the dev key's approval waits, until it takes it back.
        let third = Keypair::generate(dev.key_type()).unwrap().public();
        let pending = send(running.args(approve(vec![third.to_string()], None)));
        let withdrawn = send(running.args(TxArgs::WithdrawApproval { version: None }));
        let nothing_left = send(running.args(TxArgs::WithdrawApproval { version: None }));
        let node = running.stop();
        let state = node.chain().state();
        let signers = Signers::new(&state);
        let decided = signers.decided().unwrap();

        added.unwrap();
        assert!(stale.unwrap_err().to_string().contains("the signers changed since"));
        assert!(already.unwrap_err().to_string().contains("is a signer already"));
        assert!(garbled.unwrap_err().to_string().contains("not a public key"));
        pending.unwrap();
        withdrawn.unwrap();
        assert!(nothing_left.unwrap_err().to_string().contains("no approval"));
        assert_eq!(decided.version, 1);
        assert_eq!(decided.set.signers.len(), 2);
        assert!(decided.set.signers.contains(&crate::registry::signers::Signer { key: newcomer, account: owner }), "the newcomer speaks through its owner");
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

    #[cfg(feature = "faucet")]
    #[test]
    fn the_faucet_command_works_out_the_proof_and_the_coins_land() {
        let faucet = crate::api::FaucetConfig { key: DevAuthority::dev_keypair(), amount: u256::new(777), every_ms: 60_000, work_bits: 6 };
        let options = crate::api::ApiOptions { faucet: Some(faucet), ..Default::default() };
        let running = Running::launch_serving(true, false, None, None, |node| node, options);
        let node = format!("http://{}", running.api);
        let to = crate::crypto::keypair::Keypair::generate(crate::crypto::keypair::KeyType::Ed25519).unwrap().public().address();
        let args = |to: String| crate::cli::FaucetArgs { node: node.clone(), tls: Default::default(), to };

        // Bytes that are no key take no transfer: refused, and the address stays free to ask.
        let nobody = crate::cli::faucet(args(Address::Ed25519([0x0f; 32]).to_string())).unwrap_err();

        assert!(nobody.to_string().contains("(400)"), "{nobody}");

        crate::cli::faucet(args(to.to_string())).unwrap();

        let balance = || -> String {
            let answer: serde_json::Value = serde_json::from_str(&ureq::get(format!("{node}/accounts/{to}")).call().unwrap().body_mut().read_to_string().unwrap()).unwrap();
            answer["balance"].as_str().unwrap_or("0").to_string()
        };
        let started = std::time::Instant::now();

        while balance() != "777" {
            assert!(started.elapsed() < std::time::Duration::from_secs(10), "the drip lands within a few blocks");
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        // Once a period.
        let again = crate::cli::faucet(args(to.to_string())).unwrap_err();

        assert!(again.to_string().contains("again in"), "{again}");
        running.stop();
    }

    #[cfg(all(feature = "swap", feature = "sha256"))]
    #[test]
    fn a_swap_locks_and_claims_on_chainv4_and_signs_bitcoins_side() {
        use crate::cli::swap::{BitcoinSpend, LegAction, SwapAction, SwapArgs, bitcoin, swap};
        use crate::crypto::bitcoin::tx::Transaction;
        use crate::crypto::keypair::{Keypair, PrivateKey, PublicKey};
        use crate::structs::hex::{from_hex, to_hex};
        use crate::swap::htlc::{Spend, revealed};
        use crate::swap::{BitcoinLeg, ChainLeg, Leg, Parties, Terms};

        let dir = TempDir::new("swap");
        let source = dir.0.join("swap.lua");
        fs::write(&source, include_str!("../../contracts/lua/swap.lua")).unwrap();
        let deployer = DevAuthority::dev_keypair().public().address();
        let contract = deployed_at(&deployer, 0, crate::structs::hash::HashType::default_chain());
        let options = crate::api::ApiOptions { swap_contract: Some(contract), ..Default::default() };
        // A node that votes, so blocks become final as the claim waits for.
        let running = Running::launch_serving(true, true, FinalityConfig::dev(DevAuthority::dev_keypair().public()), None, |node| node, options);
        send(running.args(TxArgs::Deploy { lua: Some(source), wasm: None, evm: None, constructor: None, constructor_args: Vec::new() })).unwrap();

        // Alice pays bitcoin and starts, with the secret; Bob, the dev key, pays 5,000 on chainv4.
        let (secret, hash) = crate::swap::secret().unwrap();
        let [alice_bitcoin, bob_bitcoin] = [0x11u8, 0x22].map(|byte| Keypair::from_private(PrivateKey::Bitcoin([byte; 32])));
        let bitcoin_key = |key: &Keypair| match key.public() {
            PublicKey::Bitcoin(bytes) => bytes,
            _ => unreachable!(),
        };
        let alice = Parties { chain: Some(Address::Ed25519([0xa; 32])), bitcoin: Some(bitcoin_key(&alice_bitcoin)), evm: None };
        let bob = Parties { chain: Some(deployer), bitcoin: Some(bitcoin_key(&bob_bitcoin)), evm: None };
        let now = now_ms();
        // A new offer's contract is the one the node names, there being no swap name.
        let agent = crate::cli::send::agent(&Default::default()).unwrap();
        assert_eq!(crate::cli::swap::default_contract(&agent, &format!("http://{}", running.api)).unwrap(), contract);
        let pays = Leg::Bitcoin(BitcoinLeg { testnet: true, sats: 100_000, payer: None, receiver: None });
        let gets = Leg::Chain(ChainLeg { item: None, contract, token: None, amount: u256::new(5_000), payer: None, receiver: None });
        let terms = Terms::offer(hash, pays, gets, now + crate::swap::FIRST_DEADLINE_MS, now + crate::swap::SECOND_DEADLINE_MS, &alice).unwrap().answer(&bob).unwrap();
        let node = format!("http://{}", running.api);
        // The terms name the contract the lock goes into. Bob locks second; the first lock is
        // Bitcoin's, which the CLI cannot see, so he checks it himself.
        let action = || LegAction { terms: terms.to_text(), leg: None, node: node.clone(), eth_rpc: Vec::new(), tls: Default::default(), key: None, anyway: false };

        let unchecked = swap(SwapArgs { action: SwapAction::Lock(action()) }).unwrap_err();
        assert!(unchecked.to_string().contains("--anyway"), "{unchecked}");
        swap(SwapArgs { action: SwapAction::Lock(LegAction { anyway: true, ..action() }) }).unwrap();
        // Anyone may send the claim: the coins go to Alice whoever sends it. It shows the secret,
        // so it waits until Bob's lock is final.
        let started = std::time::Instant::now();

        loop {
            match swap(SwapArgs { action: SwapAction::Claim { action: action(), secret: to_hex(&secret) } }) {
                Ok(()) => break,
                Err(held) => {
                    assert!(held.to_string().contains("not final yet"), "{held}");
                    assert!(started.elapsed() < std::time::Duration::from_secs(20), "final within a few epochs");
                    std::thread::sleep(std::time::Duration::from_millis(200));
                }
            }
        }
        let account: serde_json::Value = serde_json::from_str(&ureq::get(format!("{node}/accounts/{}", alice.chain.unwrap())).call().unwrap().body_mut().read_to_string().unwrap()).unwrap();
        assert_eq!(account["balance"], "5000");
        let wrong = swap(SwapArgs { action: SwapAction::Claim { action: action(), secret: to_hex(&[0; 32]) } });
        assert!(wrong.unwrap_err().to_string().contains("not the swap's hash"));
        let bitcoin_leg = swap(SwapArgs { action: SwapAction::Refund(LegAction { leg: Some(crate::cli::swap::LegArg::First), ..action() }) });
        assert!(bitcoin_leg.unwrap_err().to_string().contains("Bitcoin's"));

        // Bob claims Alice's bitcoin with the secret chainv4 showed, offline, with his key file.
        let key = dir.0.join("bob.key");
        fs::write(&key, bob_bitcoin.to_key_file()).unwrap();
        let to = crate::structs::bech32::encode_segwit("tb", &crate::crypto::bitcoin::hash160(&bitcoin_key(&bob_bitcoin)));
        let spend = BitcoinSpend { terms: terms.to_text(), funding: format!("{}:1", "ab".repeat(32)), sats: 100_000, key, to, fee_rate: 2 };
        let signed = bitcoin(&spend, Spend::Claim { secret }).unwrap();
        let (tx, witnessed) = Transaction::read(&from_hex(&format!("0x{signed}")).unwrap()).unwrap();

        assert!(witnessed);
        assert_eq!((tx.inputs[0].txid, tx.inputs[0].vout), ([0xab; 32], 1));
        assert_eq!(tx.inputs[0].witness.last(), Some(&terms.htlc().unwrap().script()));
        assert_eq!(revealed(&tx, &hash), Some(secret), "what Alice would read, had Bob claimed first");

        // His key cannot take the refund, nor hers the claim.
        assert!(bitcoin(&spend, Spend::Refund).unwrap_err().to_string().contains("refund key"));
        running.stop();
    }

    #[cfg(all(feature = "dex", feature = "evm"))]
    #[test]
    fn the_dexes_deploy_trade_add_and_take_out_from_the_cli() {
        use crate::cli::dex::{DexAction, DexArgs, dex};

        let running = Running::start(true);
        let node = format!("http://{}", running.api);
        let me = DevAuthority::dev_keypair().public().address();
        let hash = crate::structs::hash::HashType::default_chain();
        let nonce = |node: &str| -> u64 { serde_json::from_str::<serde_json::Value>(&ureq::get(format!("{node}/accounts/{me}")).call().unwrap().body_mut().read_to_string().unwrap()).unwrap()["nonce"].as_u64().unwrap() };
        let run = |action: DexAction, evm: bool, contract: Option<Address>, router: Option<String>| dex(DexArgs { action, node: node.clone(), tls: Default::default(), key: None, evm, dex: contract, router });
        let read = |contract: &Address, function: &str, args: serde_json::Value| -> serde_json::Value {
            let body = serde_json::json!({ "contract": contract.to_string(), "function": function, "args": args });
            serde_json::from_str(&ureq::post(format!("{node}/call")).header("content-type", "application/json").send(body.to_string()).unwrap().body_mut().read_to_string().unwrap()).unwrap()
        };

        // A token to trade, then the native DEX, deployed and set up.
        let dir = TempDir::new("dex");
        let source = dir.0.join("token.lua");
        fs::write(&source, include_str!("../../contracts/lua/token.lua")).unwrap();
        let token = deployed_at(&me, nonce(&node), hash);
        send(running.args(TxArgs::Deploy { lua: Some(source), wasm: None, evm: None, constructor: None, constructor_args: Vec::new() })).unwrap();
        let start = nonce(&node);
        run(DexAction::Deploy, false, None, None).unwrap();
        let contract = deployed_at(&me, start + 1, hash);
        assert_eq!(read(&contract, "template", serde_json::json!([]))["values"][0]["value"], deployed_at(&me, start, hash).to_string());

        // Opened by its first deposit, then traded both ways, then half taken out.
        let (coin, token_text) = ("coin".to_string(), token.to_string());
        run(DexAction::Add { a: coin.clone(), b: token_text.clone(), amount_a: u256::new(1_000_000), amount_b: u256::new(4_000_000), slippage: 0.5 }, false, Some(contract), None).unwrap();
        run(DexAction::Quote { from: coin.clone(), to: token_text.clone(), amount: u256::new(10_000) }, false, Some(contract), None).unwrap();
        let held = |owner: &Address| read(&token, "balance_of", serde_json::json!([{ "type": "address", "value": owner.to_string() }]))["values"][0]["value"].as_str().unwrap().parse::<u128>().unwrap();
        let before = held(&me);
        run(DexAction::Swap { from: coin.clone(), to: token_text.clone(), amount: u256::new(10_000), slippage: 1.0 }, false, Some(contract), None).unwrap();
        assert_eq!(held(&me) - before, crate::dex::amount_out(u256::new(10_000), u256::new(1_000_000), u256::new(4_000_000)).unwrap().as_u128());
        run(DexAction::Swap { from: token_text.clone(), to: coin.clone(), amount: u256::new(20_000), slippage: 1.0 }, false, Some(contract), None).unwrap();
        let pool = read(&contract, "pool", serde_json::json!([{ "type": "address", "value": crate::dex::COIN.to_string() }, { "type": "address", "value": token_text }]));
        let share: Address = pool["values"][2]["value"].as_str().unwrap().parse().unwrap();
        let shares = read(&share, "balance_of", serde_json::json!([{ "type": "address", "value": me.to_string() }]))["values"][0]["value"].as_str().unwrap().parse::<u128>().unwrap();
        run(DexAction::Remove { a: coin.clone(), b: token_text.clone(), shares: u256::new(shares / 2) }, false, Some(contract), None).unwrap();
        let left = read(&share, "balance_of", serde_json::json!([{ "type": "address", "value": me.to_string() }]))["values"][0]["value"].as_str().unwrap().parse::<u128>().unwrap();
        assert_eq!(left, shares - shares / 2);
        let none = run(DexAction::Swap { from: coin.clone(), to: Address::Contract([9; 32]).to_string(), amount: u256::new(1), slippage: 1.0 }, false, Some(contract), None);
        assert!(none.unwrap_err().to_string().contains("no pool"));

        // The EVM DEX: WDEV, the factory and the router; an ERC-20 against the coin.
        let start = nonce(&node);
        run(DexAction::Deploy, true, None, None).unwrap();
        let router = crate::vm::evm::create_address(&me, start + 2);
        let Address::Evm(router_bytes) = router else { unreachable!() };
        let router = crate::structs::hex::to_hex(&router_bytes);
        let bin = dir.0.join("Token.bin");
        let supply = super::super::evm::encode("(string,string,uint256)", &["T".into(), "T".into(), "1000000000000000000000".into()]).unwrap();
        fs::write(&bin, crate::structs::hex::to_hex(&[crate::structs::hex::from_hex(&format!("0x{}", include_str!("../../contracts/evm/Token.bin").trim())).unwrap(), supply].concat())).unwrap();
        let erc20 = crate::vm::evm::create_address(&me, nonce(&node));
        send(running.args(TxArgs::Deploy { lua: None, wasm: None, evm: Some(bin), constructor: None, constructor_args: Vec::new() })).unwrap();
        let erc20 = erc20.to_string();
        run(DexAction::Add { a: erc20.clone(), b: coin.clone(), amount_a: u256::new(3_000_000), amount_b: u256::new(1_000_000), slippage: 0.5 }, true, None, Some(router.clone())).unwrap();
        run(DexAction::Swap { from: coin.clone(), to: erc20.clone(), amount: u256::new(5_000), slippage: 1.0 }, true, None, Some(router.clone())).unwrap();
        run(DexAction::Swap { from: erc20.clone(), to: coin.clone(), amount: u256::new(9_000), slippage: 1.0 }, true, None, Some(router.clone())).unwrap();
        run(DexAction::Pools, true, None, Some(router.clone())).unwrap();
        let refused = run(DexAction::Swap { from: token.to_string(), to: coin, amount: u256::new(1), slippage: 1.0 }, true, None, Some(router));
        assert!(refused.is_err(), "a native token is no ERC-20 on the EVM DEX");
        running.stop();
    }

    #[cfg(feature = "nft")]
    #[test]
    fn collections_are_made_minted_sent_and_read_from_the_cli() {
        use crate::cli::nft::{NftAction, NftArgs, nft};

        let running = Running::start(true);
        let node = format!("http://{}", running.api);
        let me = DevAuthority::dev_keypair().public().address();
        let hash = crate::structs::hash::HashType::default_chain();
        let nonce = |node: &str| -> u64 { serde_json::from_str::<serde_json::Value>(&ureq::get(format!("{node}/accounts/{me}")).call().unwrap().body_mut().read_to_string().unwrap()).unwrap()["nonce"].as_u64().unwrap() };
        let run = |action: NftAction, factory: Option<Address>| nft(NftArgs { action, node: node.clone(), tls: Default::default(), key: None, factory });
        let read = |contract: &Address, function: &str, args: serde_json::Value| -> serde_json::Value {
            let body = serde_json::json!({ "contract": contract.to_string(), "function": function, "args": args });
            serde_json::from_str(&ureq::post(format!("{node}/call")).header("content-type", "application/json").send(body.to_string()).unwrap().body_mut().read_to_string().unwrap()).unwrap()
        };
        let get = |path: &str| -> serde_json::Value { serde_json::from_str(&ureq::get(format!("{node}{path}")).call().unwrap().body_mut().read_to_string().unwrap()).unwrap() };
        let bob = Address::Ed25519([0xb; 32]);
        let id = |value: u64| serde_json::json!({ "type": "u256", "value": value.to_string() });

        // The factory and its two templates, which are collections themselves.
        let start = nonce(&node);
        run(NftAction::Factory, None).unwrap();
        let factory = deployed_at(&me, start + 2, hash);

        assert!(run(NftAction::Create { kind: "nft".into(), name: "Art".into(), symbol: "ART".into(), base_uri: String::new() }, None).unwrap_err().to_string().contains("no factory"));
        assert!(run(NftAction::Create { kind: "coin".into(), name: "X".into(), symbol: "X".into(), base_uri: String::new() }, Some(factory)).is_err());

        // A collection of each kind, through it.
        run(NftAction::Create { kind: "nft".into(), name: "Art".into(), symbol: "ART".into(), base_uri: "ipfs://bafy/".into() }, Some(factory)).unwrap();
        run(NftAction::Create { kind: "multi".into(), name: "Items".into(), symbol: "ITM".into(), base_uri: String::new() }, Some(factory)).unwrap();
        let made = |n: u64| -> Address { read(&factory, "at", serde_json::json!([id(n)]))["values"][0]["value"].as_str().unwrap().parse().unwrap() };
        let (art, items) = (made(0), made(1));
        let kinds: Vec<_> = get("/collections?symbol=")["collections"].as_array().unwrap().iter().map(|found| (found["symbol"].as_str().unwrap().to_string(), found["kind"].as_str().unwrap().to_string())).collect();

        assert!(kinds.contains(&("ART".to_string(), "nft".to_string())) && kinds.contains(&("ITM".to_string(), "multi".to_string())), "{kinds:?}");
        run(NftAction::Collections { symbol: Some("ar".into()) }, None).unwrap();

        // Minted, shown, sent, and listed.
        run(NftAction::Mint { collection: art, to: me, id: None, amount: u256::new(1), uri: None }, None).unwrap();
        run(NftAction::Mint { collection: art, to: me, id: None, amount: u256::new(1), uri: Some("https://example.com/2.json".into()) }, None).unwrap();
        assert!(run(NftAction::Mint { collection: art, to: me, id: Some(u256::new(5)), amount: u256::new(1), uri: None }, None).is_err(), "an NFT picks its own id");
        run(NftAction::Mint { collection: items, to: me, id: Some(u256::new(7)), amount: u256::new(10), uri: None }, None).unwrap();
        assert!(run(NftAction::Mint { collection: items, to: me, id: None, amount: u256::new(1), uri: None }, None).is_err(), "a multi-token mint names its id");
        assert_eq!(read(&art, "token_uri", serde_json::json!([id(1)]))["values"][0]["value"], "ipfs://bafy/1");
        run(NftAction::Show { collection: art, id: None }, None).unwrap();
        run(NftAction::Show { collection: art, id: Some(u256::new(2)) }, None).unwrap();
        run(NftAction::Show { collection: items, id: Some(u256::new(7)) }, None).unwrap();

        run(NftAction::Send { collection: art, to: bob, id: u256::new(1), amount: u256::new(1), safe: false }, None).unwrap();
        run(NftAction::Send { collection: items, to: bob, id: u256::new(7), amount: u256::new(3), safe: true }, None).unwrap();
        assert_eq!(read(&art, "owner_of", serde_json::json!([id(1)]))["values"][0]["value"], bob.to_string());
        assert_eq!(read(&items, "balance_of", serde_json::json!([{ "type": "address", "value": bob.to_string() }, id(7)]))["values"][0]["value"], "3");
        run(NftAction::Held { collection: art, owner: me }, None).unwrap();
        run(NftAction::Held { collection: items, owner: bob }, None).unwrap();

        run(NftAction::Operator { collection: art, operator: bob, revoke: false }, None).unwrap();
        assert_eq!(read(&art, "is_approved_for_all", serde_json::json!([{ "type": "address", "value": me.to_string() }, { "type": "address", "value": bob.to_string() }]))["values"][0]["value"], true);
        assert!(run(NftAction::Show { collection: factory, id: None }, None).unwrap_err().to_string().contains("not in the collection registry"));
        running.stop();
    }

/// An Esplora API in miniature, for the Bitcoin bridge's commands: what a test puts in it,
    /// served at its paths, and what is posted to /tx, kept.
    #[cfg(all(feature = "bridge", feature = "swap", feature = "lvm", feature = "wasm"))]
    struct FakeEsplora {
        url: String,
        routes: Arc<std::sync::Mutex<std::collections::HashMap<String, String>>>,
        posted: Arc<std::sync::Mutex<Vec<String>>>,
    }

    #[cfg(all(feature = "bridge", feature = "swap", feature = "lvm", feature = "wasm"))]
    impl FakeEsplora {
        fn start() -> FakeEsplora {
            use std::io::{BufRead, BufReader, Read, Write};

            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let routes: Arc<std::sync::Mutex<std::collections::HashMap<String, String>>> = Arc::default();
            let posted: Arc<std::sync::Mutex<Vec<String>>> = Arc::default();
            let (served, kept) = (routes.clone(), posted.clone());

            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    let Ok(mut stream) = stream else { continue };
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut first = String::new();
                    reader.read_line(&mut first).unwrap();
                    let mut length = 0;

                    loop {
                        let mut line = String::new();
                        reader.read_line(&mut line).unwrap();

                        if line.trim().is_empty() {
                            break;
                        }

                        if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                            length = value.trim().parse().unwrap();
                        }
                    }

                    let mut body = vec![0; length];
                    reader.read_exact(&mut body).unwrap();
                    let mut parts = first.split_whitespace();
                    let (method, path) = (parts.next().unwrap_or_default(), parts.next().unwrap_or_default().to_string());
                    let answer = match method {
                        "POST" => {
                            kept.lock().unwrap().push(String::from_utf8(body).unwrap());
                            Some("posted".to_string())
                        }
                        _ => served.lock().unwrap().get(&path).cloned(),
                    };
                    let (status, body) = match answer {
                        Some(body) => ("200 OK", body),
                        None => ("404 Not Found", "not found".to_string()),
                    };
                    let _ = write!(stream, "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                }
            });

            FakeEsplora { url, routes, posted }
        }

        fn serve(&self, path: &str, body: String) {
            self.routes.lock().unwrap().insert(path.to_string(), body);
        }
    }

    /// A regtest chain for the fake Esplora: headers mined at regtest's difficulty, each block's
    /// transactions with their Merkle proofs.
    #[cfg(all(feature = "bridge", feature = "swap", feature = "lvm", feature = "wasm"))]
    struct FakeBitcoin {
        esplora: FakeEsplora,
        tip: [u8; 32],
        height: u64,
        time: u32,
    }

    #[cfg(all(feature = "bridge", feature = "swap", feature = "lvm", feature = "wasm"))]
    impl FakeBitcoin {
        fn start() -> FakeBitcoin {
            let mut chain = FakeBitcoin { esplora: FakeEsplora::start(), tip: [0; 32], height: 0, time: 1_700_000_000 };
            chain.block(&[], true);

            chain
        }

        fn display(hash: &[u8]) -> String {
            let mut bytes = hash.to_vec();
            bytes.reverse();

            crate::structs::hex::to_hex(&bytes).trim_start_matches("0x").to_string()
        }

        /// A block on the tip holding a coinbase and these transactions, served as Esplora would.
        fn block(&mut self, txs: &[Vec<u8>], first: bool) {
            use crate::crypto::bitcoin::sha256d;

            let coinbase = sha256d(&self.time.to_le_bytes());
            let ids: Vec<[u8; 32]> = std::iter::once(coinbase).chain(txs.iter().map(|tx| sha256d(tx))).collect();
            // Two leaves at most here: the root, and each one's sibling as its path.
            let root = match ids.as_slice() {
                [one] => *one,
                [a, b] => sha256d(&[*a, *b].concat()),
                _ => panic!("one transaction a block here"),
            };
            let header = crate::vm::tests::btc_relay_mined(&self.tip, &root, self.time, 0x207f_ffff);
            let hash = sha256d(&header);
            let height = if first { 0 } else { self.height + 1 };
            let esplora = &self.esplora;
            esplora.serve(&format!("/block-height/{height}"), FakeBitcoin::display(&hash));
            esplora.serve(&format!("/block/{}/header", FakeBitcoin::display(&hash)), crate::structs::hex::to_hex(&header).trim_start_matches("0x").to_string());
            esplora.serve("/blocks/tip/height", height.to_string());

            for (tx, id) in txs.iter().zip(&ids[1..]) {
                let txid = FakeBitcoin::display(id);
                esplora.serve(&format!("/tx/{txid}/hex"), crate::structs::hex::to_hex(tx).trim_start_matches("0x").to_string());
                esplora.serve(&format!("/tx/{txid}/merkle-proof"), serde_json::json!({ "block_height": height, "merkle": [FakeBitcoin::display(&coinbase)], "pos": 1 }).to_string());
            }

            (self.tip, self.height, self.time) = (hash, height, self.time + 600);
        }
    }

    #[cfg(all(feature = "bridge", feature = "swap", feature = "lvm", feature = "wasm"))]
    #[test]
    fn bitcoin_is_deposited_as_wbtc_and_withdrawn_through_a_release_the_federation_signs() {
        use crate::cli::bridge::{BridgeAction, BridgeArgs, bridge};
        use crate::cli::bridge::btc::BtcAction;
        use crate::crypto::bitcoin::tx::{Transaction, TxIn, TxOut};
        use crate::crypto::keypair::{Keypair, PrivateKey};
        use crate::registry::federation::Federation;

        let federation = Federation {
            signers: [21u8, 22, 23].iter().map(|byte| crate::crypto::secp256k1::public_key(&[*byte; 32]).unwrap()).collect(),
            threshold: 2,
            chain_depth: 20,
            evm_depth: 12,
            bitcoin_depth: 2,
        };
        let me = DevAuthority::dev_keypair().public().address();
        let hash = crate::structs::hash::HashType::default_chain();
        // The chain announces the gateway the dev key's fourth transaction deploys: after the
        // relay, its start, and wBTC's template.
        let announced = deployed_at(&me, 3, hash);
        let directory = crate::registry::directory::Directory { btc_gateway: Some(announced), ..Default::default() };
        let running = Running::launch_genesis(true, false, |genesis| Genesis { federation: Some(federation.clone()), directory: Some(directory.clone()), ..genesis }, |node| node, Default::default());
        let node = format!("http://{}", running.api);
        let mut bitcoin = FakeBitcoin::start();
        let dir = TempDir::new("btc-bridge");
        let nonce = |node: &str| -> u64 { serde_json::from_str::<serde_json::Value>(&ureq::get(format!("{node}/accounts/{me}")).call().unwrap().body_mut().read_to_string().unwrap()).unwrap()["nonce"].as_u64().unwrap() };
        let read = |contract: &Address, function: &str, args: serde_json::Value| -> serde_json::Value {
            let body = serde_json::json!({ "contract": contract.to_string(), "function": function, "args": args });
            serde_json::from_str::<serde_json::Value>(&ureq::post(format!("{node}/call")).header("content-type", "application/json").send(body.to_string()).unwrap().body_mut().read_to_string().unwrap()).unwrap()["values"].clone()
        };

        // The relay, from contracts/examples/btc_relay, started at the fake chain's first block.
        let wasm = dir.0.join("btc_relay.wasm");
        fs::write(&wasm, crate::vm::tests::example("btc_relay")).unwrap();
        assert_eq!(nonce(&node), 0);
        let relay = deployed_at(&me, 0, hash);
        send(running.args(TxArgs::Deploy { lua: None, wasm: Some(wasm), evm: None, constructor: None, constructor_args: Vec::new() })).unwrap();
        let start = nonce(&node);
        let esplora = bitcoin.esplora.url.clone();
        let run = |action: BtcAction, btc_gateway: Option<Address>, evm_key: Option<PathBuf>| {
            bridge(BridgeArgs {
                action: BridgeAction::Btc { action },
                node: node.clone(),
                tls: Default::default(),
                key: None,
                gateway: None,
                eth_rpc: Vec::new(),
                evm_key,
                btc_gateway,
                esplora: Some(esplora.clone()),
            })
        };
        run(BtcAction::RelayStart { relay, height: 0, network: "regtest".into() }, None, None).unwrap();

        // The gateway, after its wBTC template, on that relay.
        run(BtcAction::Deploy { relay, fee_cap: 10_000, grace_hours: 1 }, None, None).unwrap();
        let gateway = deployed_at(&me, start + 2, hash);
        assert_eq!(gateway, announced);
        run(BtcAction::Address { recipient: me, network: "main".into() }, None, None).unwrap();

        // 50,000 satoshis to the dev key's deposit address, buried two deep, shown, minted.
        let output = read(&gateway, "deposit_script", serde_json::json!([{ "type": "address", "value": me.to_string() }]))[1]["value"].as_str().map(|hex| crate::structs::hex::from_hex(hex).unwrap()).unwrap();
        let deposit = Transaction {
            version: 2,
            inputs: vec![TxIn { txid: [0x5e; 32], vout: 0, script_sig: Vec::new(), sequence: 0xffff_ffff, witness: Vec::new() }],
            outputs: vec![TxOut { value: 50_000, script: output }],
            lock_time: 0,
        }
        .base();
        bitcoin.block(std::slice::from_ref(&deposit), false);
        bitcoin.block(&[], false);
        run(BtcAction::RelaySync { relay, batch: 10 }, None, None).unwrap();
        let txid = FakeBitcoin::display(&crate::crypto::bitcoin::sha256d(&deposit));
        run(BtcAction::Deposit { txid, vout: 0, recipient: me }, None, None).unwrap();
        let wbtc: Address = read(&gateway, "wbtc", serde_json::json!([]))[0]["value"].as_str().unwrap().parse().unwrap();
        let wbtc_of = |holder: &Address| read(&wbtc, "balance_of", serde_json::json!([{ "type": "address", "value": holder.to_string() }]))[0]["value"].as_str().unwrap().to_string();

        assert_eq!(wbtc_of(&me), "50000");

        // 20,000 withdrawn to a bc1q… address, proposed with a 1,000 fee, signed by two signers.
        let to = crate::structs::bech32::encode_segwit("bc", &[0xb0; 20]);
        run(BtcAction::Withdraw { address: to, sats: 20_000 }, None, None).unwrap();
        assert_eq!(wbtc_of(&me), "30000");
        run(BtcAction::Propose { fee: 1_000 }, None, None).unwrap();

        for secret in [21u8, 22] {
            let key = dir.0.join(format!("signer-{secret}.key"));
            fs::write(&key, Keypair::from_private(PrivateKey::Secp256k1([secret; 32])).to_key_file()).unwrap();
            run(BtcAction::Sign, None, Some(key.clone())).unwrap();
            run(BtcAction::Sign, None, Some(key)).unwrap();
        }

        // Assembled and broadcast: two signatures and the script in its witness.
        run(BtcAction::Finish { proposal: 1 }, None, None).unwrap();
        let posted = bitcoin.esplora.posted.lock().unwrap().pop().unwrap();
        let (signed, witnessed) = Transaction::read(&crate::structs::hex::from_hex(&format!("0x{posted}")).unwrap()).unwrap();

        assert!(witnessed);
        assert_eq!(signed.inputs[0].witness.len(), 4, "the extra pop, two signatures, the script");
        assert_eq!((signed.outputs[0].value, signed.outputs[1].value), (20_000, 29_000));

        // Mined and buried: shown released, the withdrawal paid, the change held.
        let release = signed.base();
        bitcoin.block(std::slice::from_ref(&release), false);
        bitcoin.block(&[], false);
        run(BtcAction::RelaySync { relay, batch: 10 }, None, None).unwrap();
        run(BtcAction::Released { proposal: 1 }, None, None).unwrap();

        assert_eq!(read(&gateway, "withdrawal", serde_json::json!([{ "type": "u64", "value": "1" }]))[2]["value"], "paid");
        assert_eq!(read(&gateway, "utxo_count", serde_json::json!([]))[0]["value"], "2");
        assert!(run(BtcAction::Propose { fee: 1_000 }, None, None).unwrap_err().to_string().contains("no withdrawals are queued"));
        running.stop();
    }

    #[cfg(all(feature = "bridge", feature = "swap", feature = "ethereum", feature = "finality-ffg"))]
    #[test]
    fn the_coin_crosses_to_an_evm_chain_and_back_through_the_gateways_and_the_federation() {
        use crate::abi::{AbiType, AbiValue, decode_values, encode_values};
        use crate::chain::{IndexKind, KeepFrom};
        use crate::cli::bridge::{BridgeAction, BridgeArgs, bridge};
        use crate::cli::swap::rpc::{Rpc, from_key};
        use crate::crypto::keypair::{Keypair, PrivateKey};
        use crate::registry::federation::Federation;
        use crate::structs::hex::to_hex;

        // Chain A runs the gateway here and the federation; chain B's EVM is the other chain.
        let federation = Federation {
            signers: [11u8, 12, 13].iter().map(|byte| crate::crypto::secp256k1::public_key(&[*byte; 32]).unwrap()).collect(),
            threshold: 2,
            chain_depth: 20,
            evm_depth: 12,
            bitcoin_depth: 6,
        };
        let finality = FinalityConfig::dev(DevAuthority::dev_keypair().public());
        let indexed = |node: Node| node.keep_index(IndexKind::Events, Some(KeepFrom::Height(0))).unwrap();
        let a = Running::launch_genesis(true, true, |genesis| Genesis { federation: Some(federation.clone()), finality: finality.clone(), ..genesis }, indexed, Default::default());
        let b = Running::launch_genesis(true, true, |genesis| Genesis { finality: finality.clone(), ..genesis }, |node| node, Default::default());
        let (node_a, rpc_b) = (format!("http://{}", a.api), format!("http://{}/rpc", b.api));
        let dir = TempDir::new("bridge");
        let key_file = |name: &str, secret: [u8; 32]| {
            let path = dir.0.join(name);
            fs::write(&path, Keypair::from_private(PrivateKey::Secp256k1(secret)).to_key_file()).unwrap();
            path
        };
        let relayer = key_file("relayer.key", [0x42; 32]);
        let (signer_one, signer_two) = (key_file("eleven.key", [11; 32]), key_file("twelve.key", [12; 32]));
        let relayer_address = from_key(&[0x42; 32]).unwrap();
        send(b.args(TxArgs::Transfer { to: Address::Secp256k1(relayer_address).into(), amount: u256::from(10u64).pow(20) })).unwrap();
        let run = |action: BridgeAction, evm_key: Option<PathBuf>| {
            bridge(BridgeArgs { action, node: node_a.clone(), tls: Default::default(), key: None, gateway: None, eth_rpc: vec![rpc_b.clone()], evm_key, btc_gateway: None, esplora: None })
        };
        let wait = |what: &str, done: &dyn Fn() -> bool| {
            let started = std::time::Instant::now();

            while !done() {
                assert!(started.elapsed() < std::time::Duration::from_secs(60), "{what} never happened");
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        };

        // The gateway here, Gateway.sol there under the federation, each the other's peer.
        let me = DevAuthority::dev_keypair().public().address();
        let nonce = serde_json::from_str::<serde_json::Value>(&ureq::get(format!("{node_a}/accounts/{me}")).call().unwrap().body_mut().read_to_string().unwrap()).unwrap()["nonce"].as_u64().unwrap();
        let gateway = deployed_at(&me, nonce + 3, crate::structs::hash::HashType::default_chain());
        assert!(run(BridgeAction::Deploy, None).is_ok());
        let with_gateway = |action: BridgeAction, evm_key: Option<PathBuf>| {
            bridge(BridgeArgs { action, node: node_a.clone(), tls: Default::default(), key: None, gateway: Some(gateway), eth_rpc: vec![rpc_b.clone()], evm_key, btc_gateway: None, esplora: None })
        };
        with_gateway(BridgeAction::DeployEvm { coin_name: "Dev".into(), coin_symbol: "DEV".into() }, Some(relayer.clone())).unwrap();
        let Address::Evm(evm_gateway) = crate::vm::evm::create_address(&Address::Secp256k1(relayer_address), 3) else { unreachable!() };
        with_gateway(BridgeAction::Peer { evm_gateway: to_hex(&evm_gateway) }, Some(relayer.clone())).unwrap();

        // 1,000 of the coin to the relayer's address there: held here, then signed by two, then
        // delivered, minted there as wrapped DEV.
        with_gateway(BridgeAction::Send { kind: "coin".into(), recipient: to_hex(&relayer_address), amount: u256::new(1_000), asset: None, id: u256::ZERO, from_evm: false }, None).unwrap();
        let rpc = Rpc::new(crate::cli::send::agent(&Default::default()).unwrap(), &rpc_b);
        let here = serde_json::from_str::<serde_json::Value>(&ureq::get(format!("{node_a}/status")).call().unwrap().body_mut().read_to_string().unwrap()).unwrap()["chain_id"].as_u64().unwrap();
        let salt = crate::crypto::hasher::Hasher::digest(
            crate::structs::hash::HashType::Keccak256,
            &encode_values(&[AbiType::Uint(64), AbiType::Uint(8), AbiType::Bytes], &[AbiValue::Uint(u256::from(here)), AbiValue::Uint(u256::from(1u64)), AbiValue::Bytes(Vec::new())]).unwrap(),
        )
        .to_bytes();
        let call = |to: &[u8; 20], signature: &str, values: &[AbiValue], returns: &[AbiType]| {
            let (_, types) = crate::abi::parse_signature(signature).unwrap();
            let input = [crate::crypto::hasher::Hasher::digest(crate::structs::hash::HashType::Keccak256, signature.as_bytes()).to_bytes()[..4].to_vec(), encode_values(&types, values).unwrap()].concat();
            decode_values(returns, &rpc.call(to, &input).unwrap()).unwrap()
        };
        let wrapped = || match call(&evm_gateway, "wrappedOf(bytes32)", &[AbiValue::FixedBytes(salt.clone())], &[AbiType::Address])[..] {
            [AbiValue::Address(address)] => address,
            _ => unreachable!(),
        };

        wait("the federation signing what left here", &|| {
            for key in [&signer_one, &signer_two] {
                with_gateway(BridgeAction::Sign { from_block: 0 }, Some(key.clone())).unwrap();
            }

            with_gateway(BridgeAction::Relay, Some(relayer.clone())).unwrap();
            wrapped() != [0; 20]
        });
        let held = |address: [u8; 20]| call(&wrapped(), "balanceOf(address)", &[AbiValue::Address(address)], &[AbiType::Uint(256)]);

        assert_eq!(held(relayer_address), [AbiValue::Uint(u256::from(1_000u64))]);

        // 400 of it back to Bob here: burned there, signed by two once final there, released here.
        let bob = Address::Ed25519([0xb0; 32]);
        with_gateway(BridgeAction::Send { kind: "token".into(), recipient: bob.to_string(), amount: u256::new(400), asset: Some(to_hex(&wrapped())), id: u256::ZERO, from_evm: true }, Some(relayer.clone())).unwrap();

        assert_eq!(held(relayer_address), [AbiValue::Uint(u256::from(600u64))]);
        let balance = |address: &Address| serde_json::from_str::<serde_json::Value>(&ureq::get(format!("{node_a}/accounts/{address}")).call().unwrap().body_mut().read_to_string().unwrap()).unwrap()["balance"].as_str().unwrap_or("0").to_string();

        wait("the coin coming back", &|| {
            for key in [&signer_one, &signer_two] {
                with_gateway(BridgeAction::Sign { from_block: 0 }, Some(key.clone())).unwrap();
            }

            balance(&bob) == "400"
        });
        assert_eq!(balance(&gateway), "600");

        // Someone outside the federation signs nothing.
        let stranger = key_file("stranger.key", [0x77; 32]);
        assert!(with_gateway(BridgeAction::Sign { from_block: 0 }, Some(stranger)).unwrap_err().to_string().contains("not a signer"));
        a.stop();
        b.stop();
    }

    #[cfg(all(feature = "swap", feature = "sha256", feature = "ethereum"))]
    #[test]
    fn a_swap_between_chainv4_and_an_evm_chain_locks_checks_and_claims_both_legs() {
        use crate::cli::swap::rpc::{Rpc, from_key};
        use crate::cli::swap::{LegAction, LegArg, SwapAction, SwapArgs, swap};
        use crate::crypto::keypair::{Keypair, PrivateKey};
        use crate::structs::hex::to_hex;
        use crate::swap::{ChainLeg, EvmLeg, Leg, LockState, Parties, Terms, evm};

        let dir = TempDir::new("evm-swap");
        let source = dir.0.join("swap.lua");
        fs::write(&source, include_str!("../../contracts/lua/swap.lua")).unwrap();
        let deployer = DevAuthority::dev_keypair().public().address();
        let contract = deployed_at(&deployer, 0, crate::structs::hash::HashType::default_chain());
        let running = Running::start(true);
        let node = format!("http://{}", running.api);
        send(running.args(TxArgs::Deploy { lua: Some(source), wasm: None, evm: None, constructor: None, constructor_args: Vec::new() })).unwrap();

        // Bob's Ethereum key, funded, deploys Swap.sol on the node's own EVM through its JSON-RPC:
        // the EVM chain this swap's second leg is on. The chain has no deployment proxy, so it is
        // a plain create.
        let bob_key = Keypair::from_private(PrivateKey::Secp256k1([0x42; 32]));
        let bob_file = dir.0.join("bob.key");
        fs::write(&bob_file, bob_key.to_key_file()).unwrap();
        send(running.args(TxArgs::Transfer { to: bob_key.public().address().into(), amount: u256::from(10u64).pow(20) })).unwrap();
        let rpc = Rpc::new(crate::cli::send::agent(&Default::default()).unwrap(), &format!("{node}/rpc"));
        let swap_sol = rpc.deploy(&evm::init_code(), &[0x42; 32]).unwrap();
        assert_ne!(swap_sol, evm::deployed_address(), "no proxy here");
        assert!(rpc.has_code(&swap_sol).unwrap());

        // Alice, the dev key, pays 5,000 on chainv4 and starts; Bob pays 7,000 wei on the EVM.
        let alice_evm = [0xa1; 20];
        let alice = Parties { chain: Some(deployer), bitcoin: None, evm: Some(alice_evm) };
        let bob = Parties { chain: Some(Address::Ed25519([0xb; 32])), bitcoin: None, evm: Some(from_key(&[0x42; 32]).unwrap()) };
        let (secret, hash) = crate::swap::secret().unwrap();
        let now = now_ms();
        let pays = Leg::Chain(ChainLeg { item: None, contract, token: None, amount: u256::new(5_000), payer: None, receiver: None });
        let gets = Leg::Evm(EvmLeg { item: None, chain_id: rpc.chain_id().unwrap(), contract: swap_sol, token: None, amount: u256::new(7_000), payer: None, receiver: None });
        let terms = Terms::offer(hash, pays, gets, now + crate::swap::FIRST_DEADLINE_MS, now + crate::swap::SECOND_DEADLINE_MS, &alice).unwrap().answer(&bob).unwrap();
        terms.check(now).unwrap();
        let action = |key: Option<PathBuf>, leg: Option<LegArg>| LegAction { terms: terms.to_text(), leg, node: node.clone(), eth_rpc: vec![format!("{node}/rpc")], tls: Default::default(), key, anyway: false };

        // Bob cannot lock before Alice: her lock does not stand yet.
        let early = swap(SwapArgs { action: SwapAction::Lock(action(Some(bob_file.clone()), None)) }).unwrap_err();
        assert!(early.to_string().contains("nothing is locked yet"), "{early}");

        swap(SwapArgs { action: SwapAction::Lock(action(None, None)) }).unwrap();

        // Alice's lock stands, but nothing on this node is final: Bob is held, until he says so.
        let unfinal = swap(SwapArgs { action: SwapAction::Lock(action(Some(bob_file.clone()), None)) }).unwrap_err();
        assert!(unfinal.to_string().contains("not final yet") && unfinal.to_string().contains("--anyway"), "{unfinal}");
        swap(SwapArgs { action: SwapAction::Lock(LegAction { anyway: true, ..action(Some(bob_file.clone()), None) }) }).unwrap();
        let locked = evm::read_lock(&rpc.call(&swap_sol, &evm::get_input(&hash)).unwrap()).unwrap();
        assert_eq!(terms.second.stands(&locked, terms.second_deadline), Ok(()));
        assert!(locked.height.is_some(), "Swap.sol says the block it was locked in");

        // Alice claims Bob's lock on the EVM, sent by anyone, which shows the secret there: held
        // too, as its block is not final, until she says so.
        let held = swap(SwapArgs { action: SwapAction::Claim { action: action(Some(bob_file.clone()), Some(LegArg::Second)), secret: to_hex(&secret) } }).unwrap_err();
        assert!(held.to_string().contains("not final yet"), "{held}");
        swap(SwapArgs { action: SwapAction::Claim { action: LegAction { anyway: true, ..action(Some(bob_file.clone()), Some(LegArg::Second)) }, secret: to_hex(&secret) } }).unwrap();
        let claimed = evm::read_lock(&rpc.call(&swap_sol, &evm::get_input(&hash)).unwrap()).unwrap();
        assert_eq!((claimed.state, claimed.secret), (LockState::Claimed, Some(secret)));
        let paid: serde_json::Value = serde_json::from_str(
            &ureq::post(format!("{node}/rpc"))
                .header("content-type", "application/json")
                .send(serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "eth_getBalance", "params": [to_hex(&alice_evm), "latest"] }).to_string())
                .unwrap()
                .body_mut()
                .read_to_string()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(paid["result"], "0x1b58", "7,000 wei to Alice's address there");

        // Bob claims Alice's lock on chainv4 with the secret it showed.
        swap(SwapArgs { action: SwapAction::Claim { action: action(None, Some(LegArg::First)), secret: to_hex(&secret) } }).unwrap();
        let account: serde_json::Value = serde_json::from_str(&ureq::get(format!("{node}/accounts/{}", bob.chain.unwrap())).call().unwrap().body_mut().read_to_string().unwrap()).unwrap();
        assert_eq!(account["balance"], "5000");

        // Neither refunds now: both are claimed.
        let refund = swap(SwapArgs { action: SwapAction::Refund(action(Some(bob_file), None)) }).unwrap_err();
        assert!(refund.to_string().contains("no swap is locked"), "{refund}");
        running.stop();
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
            #[cfg(feature = "client")]
            tls: Default::default(),
        })
        .map_err(|err| err.to_string())
    };

    check(&v2, &v1).unwrap();
    assert!(check(&retyped, &v2).unwrap_err().contains("held number, and now holds text"));
    assert!(check(&v1, &v2).unwrap_err().contains("stays retired"));
    assert!(check(&v1, &dir.0.join("v1.txt")).unwrap_err().contains(".lua, .luau or .wasm"));
}

#[test]
fn coins_and_spans_read_as_people_write_them() {
    use crate::cli::{parse_coins, parse_duration};

    assert_eq!(parse_coins("10", 18).unwrap(), u256::from(10u64) * u256::from(10u64).pow(18));
    assert_eq!(parse_coins("0.5", 2).unwrap(), u256::from(50u64));
    assert_eq!(parse_coins(".25", 2).unwrap(), u256::from(25u64));
    assert_eq!(parse_coins("7", 0).unwrap(), u256::from(7u64));
    assert!(parse_coins("0.001", 2).unwrap_err().contains("decimal places"));
    assert!(parse_coins("-1", 18).is_err() && parse_coins("ten", 18).is_err() && parse_coins(".", 18).is_err());

    assert_eq!(parse_duration("24h").unwrap(), 86_400_000);
    assert_eq!(parse_duration("30m").unwrap(), 1_800_000);
    assert_eq!((parse_duration("90s").unwrap(), parse_duration("2d").unwrap()), (90_000, 172_800_000));
    assert!(parse_duration("24").is_err() && parse_duration("h").is_err() && parse_duration("5w").is_err());
}

#[cfg(feature = "client")]
#[test]
fn bench_chain_fills_blocks_and_times_each_part() {
    let times = super::bench::chain(2, 50, None).unwrap();

    assert_eq!(times.len(), 2);
    assert!(times.iter().all(|block| block.txs == 50), "every sender's transfer in each block");
    assert!(times.iter().all(|block| block.import > 0.0 && block.import_tps() > 0.0));
}
